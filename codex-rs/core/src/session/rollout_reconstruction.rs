use super::*;
use crate::context::world_state::WorldStateSnapshot;
use crate::context_manager::is_user_turn_boundary;
use codex_history::ResponseItemEnvelope;
use codex_protocol::protocol::SessionContextWindow;
use codex_protocol::protocol::ThreadHistoryMode;
use uuid::Uuid;

// Return value of `Session::reconstruct_history_from_rollout`, bundling the rebuilt history with
// the resume/fork hydration metadata derived from the same replay.
#[derive(Debug, PartialEq)]
pub(super) struct RolloutReconstruction {
    pub(super) history: Vec<ResponseItemEnvelope>,
    pub(super) retained_context: codex_history::RetainedContext,
    pub(super) guardian_history: Option<codex_history::GuardianHistoryCheckpoint>,
    pub(super) last_started_turn_id: Option<String>,
    pub(super) previous_turn_settings: Option<PreviousTurnSettings>,
    pub(super) reference_context_item: Option<TurnContextItem>,
    pub(super) world_state_baseline: Option<WorldStateSnapshot>,
    pub(super) window_number: u64,
    pub(super) first_window_id: Option<Uuid>,
    pub(super) previous_window_id: Option<Uuid>,
    pub(super) window_id: Option<Uuid>,
    pub(super) offload_ever_used: bool,
    pub(super) active_remote_compaction_model: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct ReconstructedWindow {
    number: u64,
    first_id: Option<Uuid>,
    previous_id: Option<Uuid>,
    id: Option<Uuid>,
}

#[derive(Debug, Default)]
enum TurnReferenceContextItem {
    /// No `TurnContextItem` has been seen for this replay span yet.
    ///
    /// This differs from `Cleared`: `NeverSet` means there is no evidence this turn ever
    /// established a baseline, while `Cleared` means a baseline existed and a later compaction
    /// invalidated it. Only the latter must emit an explicit clearing segment for resume/fork
    /// hydration.
    #[default]
    NeverSet,
    /// A previously established baseline was invalidated by later compaction.
    Cleared,
    /// The latest baseline established by this replay span.
    Latest(Box<TurnContextItem>),
}

#[derive(Debug, Clone, Copy)]
// The selected compaction and its replay tail must belong to the same surviving segment.
struct ReplayCheckpoint<'a> {
    compacted: &'a CompactedItem,
    suffix: &'a [RolloutItem],
}

/// Selects the newest compaction that can safely bound replay.
///
/// Returns `None` when reconstruction must replay all supplied items, either because there is no
/// compaction or the newest compaction cannot bound replay.
fn select_input_compaction(
    rollout_items: &[RolloutItem],
    history_mode: ThreadHistoryMode,
) -> Option<ReplayCheckpoint<'_>> {
    // Only the newest compaction can bound replay. If it is incomplete, an older compaction
    // cannot replace the history or window state that the newer one may have changed.
    let (index, compacted) = rollout_items
        .iter()
        .enumerate()
        .rev()
        .find_map(|(index, item)| match item {
            RolloutItem::Compacted(compacted) => Some((index, compacted)),
            _ => None,
        })?;
    // Paginated histories always honor this boundary. Other histories only do so when resume
    // metadata identifies a compaction written under the newer resume contract.
    if compacted.replacement_history.is_none()
        || compacted.window_number.is_none()
        || (compacted.resume_metadata.is_none()
            && !matches!(history_mode, ThreadHistoryMode::Paginated))
    {
        return None;
    }
    Some(ReplayCheckpoint {
        compacted,
        suffix: &rollout_items[index + 1..],
    })
}

#[derive(Debug, Default)]
struct ActiveReplaySegment<'a> {
    turn_id: Option<String>,
    turn_completed: bool,
    counts_as_user_turn: bool,
    previous_turn_settings: Option<PreviousTurnSettings>,
    reference_context_item: TurnReferenceContextItem,
    world_state_replay: Vec<&'a RolloutItem>,
    history_checkpoint: Option<ReplayCheckpoint<'a>>,
    window: Option<ReconstructedWindow>,
}

fn turn_ids_are_compatible(active_turn_id: Option<&str>, item_turn_id: Option<&str>) -> bool {
    active_turn_id
        .is_none_or(|turn_id| item_turn_id.is_none_or(|item_turn_id| item_turn_id == turn_id))
}

fn finalize_active_segment<'a>(
    active_segment: ActiveReplaySegment<'a>,
    history_checkpoint: &mut Option<ReplayCheckpoint<'a>>,
    previous_turn_settings: &mut Option<PreviousTurnSettings>,
    reference_context_item: &mut TurnReferenceContextItem,
    world_state_replay: &mut Vec<&'a RolloutItem>,
    window: &mut Option<ReconstructedWindow>,
    pending_rollback_turns: &mut usize,
) {
    // Thread rollback drops the newest surviving real user-message boundaries. In replay, that
    // means skipping the next finalized segments that contain a non-contextual
    // `EventMsg::UserMessage`.
    if *pending_rollback_turns > 0 {
        if active_segment.counts_as_user_turn {
            *pending_rollback_turns -= 1;
        }
        return;
    }

    // Full world-state snapshots are persisted after installing initial context. They still
    // establish a baseline when a child fork removes the parent turn's agent message. Do not
    // count these context-only segments as user turns for rollback, or use a snapshot from
    // before the segment's latest compaction.
    let has_context_baseline = active_segment.counts_as_user_turn
        || active_segment
            .world_state_replay
            .iter()
            .take_while(|item| !matches!(item, RolloutItem::Compacted(_)))
            .any(|item| matches!(item, RolloutItem::WorldState(state) if state.full));
    world_state_replay.extend(active_segment.world_state_replay);

    // A surviving replacement-history compaction is a complete history base. Once we
    // know the newest surviving one, older rollout items do not affect rebuilt history.
    if history_checkpoint.is_none()
        && let Some(segment_history_checkpoint) = active_segment.history_checkpoint
    {
        *history_checkpoint = Some(segment_history_checkpoint);
    }

    if window.is_none() {
        *window = active_segment.window;
    }

    // Restore settings from the newest surviving context baseline.
    if previous_turn_settings.is_none() && has_context_baseline {
        *previous_turn_settings = active_segment.previous_turn_settings;
    }

    // `reference_context_item` comes from the newest surviving context baseline, or
    // from a surviving compaction that explicitly cleared that baseline.
    if matches!(reference_context_item, TurnReferenceContextItem::NeverSet)
        && (has_context_baseline
            || matches!(
                active_segment.reference_context_item,
                TurnReferenceContextItem::Cleared
            ))
    {
        *reference_context_item = active_segment.reference_context_item;
    }
}

#[derive(Debug)]
struct MaterializedRolloutHistory {
    history: Vec<ResponseItemEnvelope>,
    retained_context: codex_history::RetainedContext,
    guardian_history: Option<codex_history::GuardianHistoryCheckpoint>,
}

#[derive(Default)]
struct CheckpointReplaySegment {
    turn_id: Option<String>,
    counts_as_user_turn: bool,
    remote_compaction_indices_newest_first: Vec<usize>,
    segment_start_index: Option<usize>,
    segment_end_index: Option<usize>,
}

impl CheckpointReplaySegment {
    fn include_rollout_index(&mut self, index: usize) {
        self.segment_start_index = Some(
            self.segment_start_index
                .map_or(index, |start| start.min(index)),
        );
        self.segment_end_index = Some(
            self.segment_end_index
                .map_or(index.saturating_add(1), |end| {
                    end.max(index.saturating_add(1))
                }),
        );
    }
}

struct ActiveRemoteCompactionCheckpoint {
    index: usize,
    surviving_suffix: Vec<RolloutItem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemoteCompactionFingerprint<'a> {
    Compaction(&'a str),
    ContextCompaction(&'a str),
}

enum CheckpointSegmentOutcome {
    Found(ActiveRemoteCompactionCheckpoint),
    NotFound,
}

fn finalize_checkpoint_segment(
    segment: CheckpointReplaySegment,
    surviving_newer_rollout_items: &mut Vec<RolloutItem>,
    rollout_items: &[RolloutItem],
    pending_rollback_turns: &mut usize,
) -> CheckpointSegmentOutcome {
    if *pending_rollback_turns > 0 {
        if segment.counts_as_user_turn {
            *pending_rollback_turns -= 1;
        }
        return CheckpointSegmentOutcome::NotFound;
    }

    if let Some(index) = segment
        .remote_compaction_indices_newest_first
        .first()
        .copied()
    {
        let mut surviving_suffix = segment
            .segment_end_index
            .map(|segment_end| rollout_items[index.saturating_add(1)..segment_end].to_vec())
            .unwrap_or_default();
        surviving_suffix.append(surviving_newer_rollout_items);
        return CheckpointSegmentOutcome::Found(ActiveRemoteCompactionCheckpoint {
            index,
            surviving_suffix,
        });
    }

    if let (Some(start), Some(end)) = (segment.segment_start_index, segment.segment_end_index) {
        let mut segment_items = rollout_items[start..end].to_vec();
        segment_items.append(surviving_newer_rollout_items);
        *surviving_newer_rollout_items = segment_items;
    }
    CheckpointSegmentOutcome::NotFound
}

fn active_remote_compaction_checkpoint(
    rollout_items: &[RolloutItem],
) -> Option<ActiveRemoteCompactionCheckpoint> {
    let mut pending_rollback_turns = 0usize;
    let mut surviving_newer_rollout_items = Vec::new();
    let mut active_segment: Option<CheckpointReplaySegment> = None;

    for (index, item) in rollout_items.iter().enumerate().rev() {
        match item {
            RolloutItem::Compacted(compacted)
                if compacted
                    .replacement_history
                    .as_deref()
                    .is_some_and(|history| {
                        history.iter().any(|envelope| {
                            matches!(
                                envelope.item,
                                ResponseItem::Compaction { .. }
                                    | ResponseItem::ContextCompaction { .. }
                            )
                        })
                    }) =>
            {
                let active_segment =
                    active_segment.get_or_insert_with(CheckpointReplaySegment::default);
                active_segment.include_rollout_index(index);
                active_segment
                    .remote_compaction_indices_newest_first
                    .push(index);
            }
            RolloutItem::Compacted(_) => {
                active_segment
                    .get_or_insert_with(CheckpointReplaySegment::default)
                    .include_rollout_index(index);
            }
            RolloutItem::EventMsg(EventMsg::ThreadRolledBack(rollback)) => {
                pending_rollback_turns = pending_rollback_turns
                    .saturating_add(usize::try_from(rollback.num_turns).unwrap_or(usize::MAX));
            }
            RolloutItem::EventMsg(EventMsg::TurnComplete(event)) => {
                let active_segment =
                    active_segment.get_or_insert_with(CheckpointReplaySegment::default);
                active_segment.include_rollout_index(index);
                if active_segment.turn_id.is_none() {
                    active_segment.turn_id = Some(event.turn_id.clone());
                }
            }
            RolloutItem::EventMsg(EventMsg::TurnAborted(event)) => {
                if let Some(active_segment) = active_segment.as_mut() {
                    active_segment.include_rollout_index(index);
                    if active_segment.turn_id.is_none()
                        && let Some(turn_id) = &event.turn_id
                    {
                        active_segment.turn_id = Some(turn_id.clone());
                    }
                } else if let Some(turn_id) = &event.turn_id {
                    active_segment = Some(CheckpointReplaySegment {
                        turn_id: Some(turn_id.clone()),
                        segment_start_index: Some(index),
                        segment_end_index: Some(index.saturating_add(1)),
                        ..Default::default()
                    });
                }
            }
            RolloutItem::EventMsg(EventMsg::UserMessage(_)) => {
                let active_segment =
                    active_segment.get_or_insert_with(CheckpointReplaySegment::default);
                active_segment.include_rollout_index(index);
                active_segment.counts_as_user_turn = true;
            }
            RolloutItem::TurnContext(ctx) => {
                let active_segment =
                    active_segment.get_or_insert_with(CheckpointReplaySegment::default);
                active_segment.include_rollout_index(index);
                if active_segment.turn_id.is_none() {
                    active_segment.turn_id = ctx.turn_id.clone();
                }
            }
            RolloutItem::WorldState(_) => {
                active_segment
                    .get_or_insert_with(CheckpointReplaySegment::default)
                    .include_rollout_index(index);
            }
            RolloutItem::EventMsg(EventMsg::TurnStarted(event)) => {
                if active_segment.as_ref().is_some_and(|active_segment| {
                    turn_ids_are_compatible(
                        active_segment.turn_id.as_deref(),
                        Some(event.turn_id.as_str()),
                    )
                }) && let Some(mut active_segment) = active_segment.take()
                {
                    active_segment.include_rollout_index(index);
                    match finalize_checkpoint_segment(
                        active_segment,
                        &mut surviving_newer_rollout_items,
                        rollout_items,
                        &mut pending_rollback_turns,
                    ) {
                        CheckpointSegmentOutcome::Found(checkpoint) => return Some(checkpoint),
                        CheckpointSegmentOutcome::NotFound => {}
                    }
                }
            }
            RolloutItem::ResponseItem(response_item) => {
                let active_segment =
                    active_segment.get_or_insert_with(CheckpointReplaySegment::default);
                active_segment.include_rollout_index(index);
                active_segment.counts_as_user_turn |= is_user_turn_boundary(&response_item.item);
            }
            RolloutItem::InterAgentCommunication(_) => {
                let active_segment =
                    active_segment.get_or_insert_with(CheckpointReplaySegment::default);
                active_segment.include_rollout_index(index);
                active_segment.counts_as_user_turn = true;
            }
            RolloutItem::EventMsg(_)
            | RolloutItem::RealtimeItem(_)
            | RolloutItem::SessionMeta(_)
            | RolloutItem::RetainedContext(_)
            | RolloutItem::SecurityRiskScore(_)
            | RolloutItem::TokenUsageRecord(_)
            | RolloutItem::InterAgentCommunicationMetadata { .. } => {
                if let Some(active_segment) = active_segment.as_mut() {
                    active_segment.include_rollout_index(index);
                }
            }
        }
    }

    active_segment.and_then(|active_segment| {
        match finalize_checkpoint_segment(
            active_segment,
            &mut surviving_newer_rollout_items,
            rollout_items,
            &mut pending_rollback_turns,
        ) {
            CheckpointSegmentOutcome::Found(checkpoint) => Some(checkpoint),
            CheckpointSegmentOutcome::NotFound => None,
        }
    })
}

fn suffix_most_remote_compaction_fingerprint<'a>(
    history: impl DoubleEndedIterator<Item = &'a ResponseItem>,
) -> Option<RemoteCompactionFingerprint<'a>> {
    history.rev().find_map(|item| match item {
        ResponseItem::Compaction {
            encrypted_content, ..
        } => Some(RemoteCompactionFingerprint::Compaction(encrypted_content)),
        ResponseItem::ContextCompaction {
            encrypted_content: Some(encrypted_content),
            ..
        } => Some(RemoteCompactionFingerprint::ContextCompaction(
            encrypted_content,
        )),
        _ => None,
    })
}

fn checkpoint_matches_active_remote_compaction(
    rollout_items: &[RolloutItem],
    checkpoint: &ActiveRemoteCompactionCheckpoint,
    active_fingerprint: &RemoteCompactionFingerprint<'_>,
) -> bool {
    let Some(RolloutItem::Compacted(compacted)) = rollout_items.get(checkpoint.index) else {
        return false;
    };
    compacted
        .replacement_history
        .as_deref()
        .and_then(|history| {
            suffix_most_remote_compaction_fingerprint(history.iter().map(|envelope| &envelope.item))
        })
        .is_some_and(|fingerprint| fingerprint == *active_fingerprint)
}

fn materialize_rollout_items(
    turn_context: &TurnContext,
    guardian_context_mode: GuardianContextMode,
    initial_history: Vec<ResponseItemEnvelope>,
    initial_retained_context: Option<&codex_history::RetainedContext>,
    initial_guardian_history: Option<&codex_history::GuardianHistoryCheckpoint>,
    rollout_items: &[RolloutItem],
) -> MaterializedRolloutHistory {
    let mut history = ContextManager::with_guardian_context_mode(
        guardian_context_mode,
        &turn_context.session_source,
    );
    history.replace_annotated(initial_history);
    history.restore_review_context(
        initial_retained_context,
        initial_guardian_history,
        /*reviewer_compaction_hash*/ None,
    );

    for item in rollout_items {
        match item {
            RolloutItem::RetainedContext(event) => {
                history.record_retained_context(event);
            }
            RolloutItem::ResponseItem(response_item) => {
                history.record_annotated_items(
                    std::slice::from_ref(response_item),
                    turn_context.model_info().truncation_policy.into(),
                );
            }
            RolloutItem::InterAgentCommunication(communication) => {
                let response_item = communication.to_model_input_item();
                history.record_items(
                    std::iter::once(&response_item),
                    turn_context.model_info().truncation_policy.into(),
                );
            }
            RolloutItem::Compacted(compacted) => {
                if let Some(replacement_history) = &compacted.replacement_history {
                    history.replace_annotated(replacement_history.clone());
                    history.restore_review_context(
                        compacted.retained_context.as_ref(),
                        compacted.guardian_history.as_ref(),
                        /*reviewer_compaction_hash*/ None,
                    );
                } else {
                    let identity = if guardian_context_mode == GuardianContextMode::ThreadOwned {
                        compact::CompactedMessageIdentity::Preserve
                    } else {
                        compact::CompactedMessageIdentity::Regenerate
                    };
                    let user_messages = compact::collect_annotated_user_messages(
                        history.annotated_items(),
                        identity,
                    );
                    let rebuilt = compact::build_compacted_history(
                        Vec::new(),
                        &user_messages,
                        &compacted.message,
                    );
                    let retained_context = history.retained_context().clone();
                    history.replace_annotated(rebuilt);
                    history.restore_retained_context(Some(&retained_context));
                }
            }
            RolloutItem::EventMsg(EventMsg::ThreadRolledBack(rollback)) => {
                history.drop_last_n_user_turns(rollback.num_turns);
            }
            RolloutItem::EventMsg(_)
            | RolloutItem::TurnContext(_)
            | RolloutItem::RealtimeItem(_)
            | RolloutItem::WorldState(_)
            | RolloutItem::SecurityRiskScore(_)
            | RolloutItem::TokenUsageRecord(_)
            | RolloutItem::InterAgentCommunicationMetadata { .. }
            | RolloutItem::SessionMeta(_) => {}
        }
    }

    MaterializedRolloutHistory {
        guardian_history: history.guardian_history_checkpoint(),
        retained_context: history.retained_context().clone(),
        history: history.into_annotated_items(),
    }
}

pub(super) fn reconstruct_retro_local_history_from_rollout(
    turn_context: &TurnContext,
    rollout_items: &[RolloutItem],
    active_history: &[ResponseItem],
) -> CodexResult<Vec<ResponseItem>> {
    let Some(active_fingerprint) = suffix_most_remote_compaction_fingerprint(active_history.iter())
    else {
        return Err(CodexErr::InvalidRequest(
            "Cannot run retro-local fallback: active history has no encrypted remote compaction item."
                .to_string(),
        ));
    };
    let Some(remote_checkpoint) = active_remote_compaction_checkpoint(rollout_items) else {
        return Err(CodexErr::InvalidRequest(
            "Cannot run retro-local fallback: no surviving remote compaction checkpoint with replacement history is available."
                .to_string(),
        ));
    };
    if !checkpoint_matches_active_remote_compaction(
        rollout_items,
        &remote_checkpoint,
        &active_fingerprint,
    ) {
        return Err(CodexErr::InvalidRequest(
            "Cannot run retro-local fallback: the surviving remote compaction checkpoint does not match active history."
                .to_string(),
        ));
    }
    let remote_checkpoint_index = remote_checkpoint.index;

    let prefix = materialize_rollout_items(
        turn_context,
        GuardianContextMode::Legacy,
        Vec::new(),
        /*initial_retained_context*/ None,
        /*initial_guardian_history*/ None,
        &rollout_items[..remote_checkpoint_index],
    );
    if prefix.history.iter().any(|envelope| {
        matches!(
            envelope.item,
            ResponseItem::Compaction { .. } | ResponseItem::ContextCompaction { .. }
        )
    }) {
        return Err(CodexErr::InvalidRequest(
            "Cannot run retro-local fallback: readable source history still contains encrypted remote compaction before the selected checkpoint."
                .to_string(),
        ));
    }

    let reconstructed = materialize_rollout_items(
        turn_context,
        GuardianContextMode::Legacy,
        prefix.history,
        Some(&prefix.retained_context),
        prefix.guardian_history.as_ref(),
        &remote_checkpoint.surviving_suffix,
    )
    .history;
    if reconstructed.iter().any(|envelope| {
        matches!(
            envelope.item,
            ResponseItem::Compaction { .. } | ResponseItem::ContextCompaction { .. }
        )
    }) {
        return Err(CodexErr::InvalidRequest(
            "Cannot run retro-local fallback: reconstructed suffix still contains encrypted remote compaction."
                .to_string(),
        ));
    }

    Ok(reconstructed
        .into_iter()
        .map(ResponseItemEnvelope::into_item)
        .collect())
}

impl Session {
    pub(crate) async fn reconstruct_retro_local_history_from_persisted_rollout(
        &self,
        turn_context: &TurnContext,
    ) -> CodexResult<Vec<ResponseItem>> {
        let active_history = self.clone_history().await.raw_items().cloned().collect::<Vec<_>>();
        let Some(live_thread) = self.live_thread() else {
            return Err(CodexErr::InvalidRequest(
                "Cannot run retro-local fallback: persisted thread history is unavailable."
                    .to_string(),
            ));
        };
        live_thread.flush().await.map_err(|err| {
            CodexErr::InvalidRequest(format!(
                "Cannot run retro-local fallback: failed to flush persisted thread history: {err}"
            ))
        })?;
        let history = live_thread.load_history(/*include_archived*/ true).await.map_err(|err| {
            CodexErr::InvalidRequest(format!(
                "Cannot run retro-local fallback: failed to load persisted thread history: {err}"
            ))
        })?;
        reconstruct_retro_local_history_from_rollout(turn_context, &history.items, &active_history)
    }

    pub(super) async fn reconstruct_history_from_rollout(
        &self,
        turn_context: &TurnContext,
        rollout_items: &[RolloutItem],
    ) -> RolloutReconstruction {
        // Select the compaction and suffix that can affect reconstruction.
        let has_legacy_compaction_without_window_number =
            rollout_items.iter().any(|item| {
                matches!(item, RolloutItem::Compacted(compacted) if compacted.window_number.is_none())
            });
        let initial_window = if has_legacy_compaction_without_window_number {
            None
        } else {
            rollout_items.iter().find_map(|item| match item {
                RolloutItem::SessionMeta(session_meta) => session_meta
                    .meta
                    .context_window
                    .as_ref()
                    .and_then(reconstructed_window_from_session_context_window),
                _ => None,
            })
        };
        let input_checkpoint = select_input_compaction(rollout_items, turn_context.history_mode);
        let replay_items = input_checkpoint.map_or(rollout_items, |checkpoint| checkpoint.suffix);
        let resume_metadata =
            input_checkpoint.and_then(|checkpoint| checkpoint.compacted.resume_metadata.as_ref());
        let mut history_checkpoint = input_checkpoint;
        let mut window = input_checkpoint
            .and_then(|checkpoint| reconstructed_window_from_compaction(checkpoint.compacted));

        // Scan the selected items backward to find the newest surviving turn state.
        let last_started_turn_id = replay_items
            .iter()
            .rev()
            .find_map(|item| match item {
                RolloutItem::EventMsg(EventMsg::TurnStarted(event)) => Some(event.turn_id.clone()),
                RolloutItem::SessionMeta(_)
                | RolloutItem::ResponseItem(_)
                | RolloutItem::InterAgentCommunication(_)
                | RolloutItem::InterAgentCommunicationMetadata { .. }
                | RolloutItem::TurnContext(_)
                | RolloutItem::WorldState(_)
                | RolloutItem::RetainedContext(_)
                | RolloutItem::SecurityRiskScore(_)
                | RolloutItem::TokenUsageRecord(_)
                | RolloutItem::RealtimeItem(_)
                | RolloutItem::Compacted(_)
                | RolloutItem::EventMsg(_) => None,
            })
            .or_else(|| resume_metadata.and_then(|metadata| metadata.last_started_turn_id.clone()));

        let mut previous_turn_settings = None;
        let mut reference_context_item = TurnReferenceContextItem::NeverSet;
        let mut world_state_replay = Vec::new();
        // Rollback is "drop the newest N user turns". While scanning in reverse, that becomes
        // "skip the next N user-turn segments we finalize".
        let mut pending_rollback_turns = 0usize;
        // Reverse replay accumulates rollout items into the newest in-progress turn segment until
        // we hit its matching `TurnStarted`, at which point the segment can be finalized.
        let mut active_segment: Option<ActiveReplaySegment<'_>> = None;

        for (index, item) in replay_items.iter().enumerate().rev() {
            match item {
                RolloutItem::Compacted(compacted) => {
                    let active_segment =
                        active_segment.get_or_insert_with(ActiveReplaySegment::default);
                    active_segment.world_state_replay.push(item);
                    if active_segment.window.is_none()
                        && let Some(compaction_window) =
                            reconstructed_window_from_compaction(compacted)
                    {
                        active_segment.window = Some(compaction_window);
                    }
                    // Looking backward, compaction clears any older baseline unless a newer
                    // `TurnContextItem` in this same segment has already re-established it.
                    if matches!(
                        active_segment.reference_context_item,
                        TurnReferenceContextItem::NeverSet
                    ) {
                        active_segment.reference_context_item = TurnReferenceContextItem::Cleared;
                    }
                    if active_segment.history_checkpoint.is_none()
                        && compacted.replacement_history.is_some()
                    {
                        active_segment.history_checkpoint = Some(ReplayCheckpoint {
                            compacted,
                            suffix: &replay_items[index + 1..],
                        });
                    }
                }
                RolloutItem::EventMsg(EventMsg::ThreadRolledBack(rollback)) => {
                    pending_rollback_turns = pending_rollback_turns
                        .saturating_add(usize::try_from(rollback.num_turns).unwrap_or(usize::MAX));
                }
                RolloutItem::EventMsg(EventMsg::TurnComplete(event)) => {
                    let active_segment =
                        active_segment.get_or_insert_with(ActiveReplaySegment::default);
                    active_segment.turn_completed = true;
                    // Reverse replay often sees `TurnComplete` before any turn-scoped metadata.
                    // Capture the turn id early so later `TurnContext` / abort items can match it.
                    if active_segment.turn_id.is_none() {
                        active_segment.turn_id = Some(event.turn_id.clone());
                    }
                }
                RolloutItem::EventMsg(EventMsg::TurnAborted(event)) => {
                    if let Some(active_segment) = active_segment.as_mut() {
                        if active_segment.turn_id.is_none()
                            && let Some(turn_id) = &event.turn_id
                        {
                            active_segment.turn_id = Some(turn_id.clone());
                        }
                    } else if let Some(turn_id) = &event.turn_id {
                        active_segment = Some(ActiveReplaySegment {
                            turn_id: Some(turn_id.clone()),
                            ..Default::default()
                        });
                    }
                }
                RolloutItem::EventMsg(EventMsg::UserMessage(_)) => {
                    let active_segment =
                        active_segment.get_or_insert_with(ActiveReplaySegment::default);
                    active_segment.counts_as_user_turn = true;
                }
                RolloutItem::TurnContext(ctx) => {
                    let active_segment =
                        active_segment.get_or_insert_with(ActiveReplaySegment::default);
                    // `TurnContextItem` can attach metadata to an existing segment, but only a
                    // real `UserMessage` event should make the segment count as a user turn.
                    if active_segment.turn_id.is_none() {
                        active_segment.turn_id = ctx.turn_id.clone();
                    }
                    if turn_ids_are_compatible(
                        active_segment.turn_id.as_deref(),
                        ctx.turn_id.as_deref(),
                    ) {
                        active_segment.previous_turn_settings = Some(PreviousTurnSettings {
                            model: ctx.model.clone(),
                            cyber_access_program: ctx.cyber_access_program,
                            comp_hash: ctx.comp_hash.clone(),
                            realtime_active: ctx.realtime_active,
                        });
                        if matches!(
                            active_segment.reference_context_item,
                            TurnReferenceContextItem::NeverSet
                        ) {
                            active_segment.reference_context_item =
                                TurnReferenceContextItem::Latest(Box::new(ctx.clone()));
                        }
                    }
                }
                RolloutItem::WorldState(_) => {
                    let active_segment =
                        active_segment.get_or_insert_with(ActiveReplaySegment::default);
                    active_segment.world_state_replay.push(item);
                }
                RolloutItem::EventMsg(EventMsg::TurnStarted(event)) => {
                    // `TurnStarted` is the oldest boundary of the active reverse segment.
                    if active_segment.as_ref().is_some_and(|active_segment| {
                        turn_ids_are_compatible(
                            active_segment.turn_id.as_deref(),
                            Some(event.turn_id.as_str()),
                        )
                    }) && let Some(active_segment) = active_segment.take()
                    {
                        finalize_active_segment(
                            active_segment,
                            &mut history_checkpoint,
                            &mut previous_turn_settings,
                            &mut reference_context_item,
                            &mut world_state_replay,
                            &mut window,
                            &mut pending_rollback_turns,
                        );
                    }
                }
                RolloutItem::ResponseItem(response_item) => {
                    let active_segment =
                        active_segment.get_or_insert_with(ActiveReplaySegment::default);
                    active_segment.counts_as_user_turn |=
                        is_user_turn_boundary(&response_item.item);
                }
                RolloutItem::InterAgentCommunication(_) => {
                    let active_segment =
                        active_segment.get_or_insert_with(ActiveReplaySegment::default);
                    active_segment.counts_as_user_turn = true;
                }
                RolloutItem::EventMsg(_)
                | RolloutItem::SessionMeta(_)
                | RolloutItem::RealtimeItem(_)
                | RolloutItem::RetainedContext(_)
                | RolloutItem::SecurityRiskScore(_)
                | RolloutItem::TokenUsageRecord(_)
                | RolloutItem::InterAgentCommunicationMetadata { .. } => {}
            }
        }

        if let Some(mut active_segment) = active_segment.take() {
            // A companion turn context only restores the context baseline. Once that turn
            // completes, its settings are newer than the compaction metadata.
            if resume_metadata.is_some() && !active_segment.turn_completed {
                active_segment.previous_turn_settings = None;
            }
            finalize_active_segment(
                active_segment,
                &mut history_checkpoint,
                &mut previous_turn_settings,
                &mut reference_context_item,
                &mut world_state_replay,
                &mut window,
                &mut pending_rollback_turns,
            );
        }

        if previous_turn_settings.is_none() {
            previous_turn_settings =
                resume_metadata.and_then(|metadata| metadata.previous_turn_settings.clone());
        }

        let fallback_window_number = u64::try_from(
            rollout_items
                .iter()
                .filter(|item| matches!(item, RolloutItem::Compacted(_)))
                .count(),
        )
        .unwrap_or(u64::MAX);

        // Build model-visible history from the selected compaction and its newer suffix.
        let mut history = ContextManager::for_session(
            &turn_context.session_source,
            &turn_context.config.features,
        );
        let mut saw_legacy_compaction_without_replacement_history = false;
        if let Some(checkpoint) = history_checkpoint
            && let Some(items) = &checkpoint.compacted.replacement_history
        {
            history.replace_annotated(items.clone());
            history.restore_review_context(
                checkpoint.compacted.retained_context.as_ref(),
                checkpoint.compacted.guardian_history.as_ref(),
                // Keep the backup during replay; the installing session resolves its reviewer.
                /*reviewer_compaction_hash*/
                None,
            );
        }
        let rollout_suffix =
            history_checkpoint.map_or(rollout_items, |checkpoint| checkpoint.suffix);
        for item in rollout_suffix {
            match item {
                RolloutItem::RetainedContext(event) => {
                    history.record_retained_context(event);
                }
                RolloutItem::ResponseItem(response_item) => {
                    history.replay_annotated_item(
                        response_item,
                        turn_context.model_info().truncation_policy.into(),
                    );
                }
                RolloutItem::InterAgentCommunication(communication) => {
                    let response_item = communication.to_model_input_item();
                    history.record_items(
                        std::iter::once(&response_item),
                        turn_context.model_info().truncation_policy.into(),
                    );
                }
                RolloutItem::InterAgentCommunicationMetadata { .. } => {}
                RolloutItem::Compacted(compacted) => {
                    // Reverse replay already chose the newest surviving compaction. Any newer
                    // replacement compaction belongs to a rolled-back turn; replay its original
                    // items so the rollback can still find the removed user boundary.
                    if compacted.replacement_history.is_none() {
                        saw_legacy_compaction_without_replacement_history = true;
                        // Legacy rollouts without `replacement_history` should rebuild the
                        // historical TurnContext at the correct insertion point from persisted
                        // `TurnContextItem`s. These are rare enough that we currently just clear
                        // `reference_context_item`, reinject canonical context at the end of the
                        // resumed conversation, and accept the temporary out-of-distribution
                        // prompt shape.
                        // TODO(ccunningham): if we drop support for None replacement_history compaction items,
                        // we can get rid of this second loop entirely and just build `history` directly in the first loop.
                        let user_messages =
                            compact::collect_annotated_user_messages(history.annotated_items());
                        let rebuilt = compact::build_compacted_history(
                            Vec::new(),
                            &user_messages,
                            &compacted.message,
                        );
                        let retained_context = history.retained_context().clone();
                        history.replace_annotated(rebuilt);
                        history.restore_retained_context(Some(&retained_context));
                    }
                }
                RolloutItem::EventMsg(EventMsg::ThreadRolledBack(rollback)) => {
                    history.drop_last_n_user_turns(rollback.num_turns);
                }
                RolloutItem::EventMsg(_)
                | RolloutItem::TurnContext(_)
                | RolloutItem::RealtimeItem(_)
                | RolloutItem::WorldState(_)
                | RolloutItem::SecurityRiskScore(_)
                | RolloutItem::TokenUsageRecord(_)
                | RolloutItem::SessionMeta(_) => {}
            }
        }

        let reference_context_item = match reference_context_item {
            TurnReferenceContextItem::NeverSet | TurnReferenceContextItem::Cleared => None,
            TurnReferenceContextItem::Latest(turn_reference_context_item) => {
                Some(*turn_reference_context_item)
            }
        };
        let reference_context_item = if saw_legacy_compaction_without_replacement_history {
            None
        } else {
            reference_context_item
        };

        // Replay the collected world-state records chronologically so compaction resets and merge
        // patches keep their original meaning.
        world_state_replay.reverse();
        let mut world_state_baseline: Option<WorldStateSnapshot> = None;
        for item in world_state_replay {
            match item {
                RolloutItem::Compacted(_) => world_state_baseline = None,
                RolloutItem::WorldState(world_state) if world_state.full => {
                    world_state_baseline = Some(WorldStateSnapshot::from(&world_state.state));
                }
                RolloutItem::WorldState(world_state) => {
                    let Some(baseline) = world_state_baseline.as_mut() else {
                        tracing::warn!("ignored world-state patch without a full snapshot");
                        continue;
                    };
                    baseline.apply_merge_patch(&world_state.state);
                }
                RolloutItem::SessionMeta(_)
                | RolloutItem::ResponseItem(_)
                | RolloutItem::InterAgentCommunication(_)
                | RolloutItem::InterAgentCommunicationMetadata { .. }
                | RolloutItem::TurnContext(_)
                | RolloutItem::RealtimeItem(_)
                | RolloutItem::TokenUsageRecord(_)
                | RolloutItem::RetainedContext(_)
                | RolloutItem::SecurityRiskScore(_)
                | RolloutItem::EventMsg(_) => {
                    unreachable!("only world-state replay items are collected")
                }
            }
        }

        let window = window.or(initial_window).unwrap_or(ReconstructedWindow {
            number: fallback_window_number,
            first_id: None,
            previous_id: None,
            id: None,
        });
        let offload_ever_used = rollout_items.iter().any(|item| matches!(item, RolloutItem::TurnContext(context) if context.offload_ever_used));
        let active_remote_compaction_model = history_checkpoint.and_then(|checkpoint| checkpoint.compacted.remote_compaction_model.clone());
        RolloutReconstruction {
            offload_ever_used,
            active_remote_compaction_model,
            retained_context: history.retained_context().clone(),
            guardian_history: history.guardian_history_checkpoint(),
            last_started_turn_id,
            history: history.into_annotated_items(),
            previous_turn_settings,
            reference_context_item,
            world_state_baseline,
            window_number: window.number,
            first_window_id: window.first_id,
            previous_window_id: window.previous_id,
            window_id: window.id,
        }
    }
}

fn parse_uuid_v7(value: &str) -> Option<Uuid> {
    Uuid::parse_str(value)
        .ok()
        .filter(|uuid| uuid.get_version_num() == 7)
}

fn reconstructed_window_from_compaction(compacted: &CompactedItem) -> Option<ReconstructedWindow> {
    Some(ReconstructedWindow {
        number: compacted.window_number?,
        first_id: compacted.first_window_id.as_deref().and_then(parse_uuid_v7),
        previous_id: compacted
            .previous_window_id
            .as_deref()
            .and_then(parse_uuid_v7),
        id: compacted.window_id.as_deref().and_then(parse_uuid_v7),
    })
}

fn reconstructed_window_from_session_context_window(
    context_window: &SessionContextWindow,
) -> Option<ReconstructedWindow> {
    let id = parse_uuid_v7(&context_window.window_id)?;
    Some(ReconstructedWindow {
        number: 0,
        first_id: Some(id),
        previous_id: None,
        id: Some(id),
    })
}
