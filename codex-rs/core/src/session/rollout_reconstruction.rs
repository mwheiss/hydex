use super::*;
use crate::compaction_recovery::active_history_has_remote_compaction;
use crate::context::world_state::WorldStateSnapshot;
use crate::context_manager::is_user_turn_boundary;
use codex_protocol::protocol::SessionContextWindow;
use uuid::Uuid;

// Return value of `Session::reconstruct_history_from_rollout`, bundling the rebuilt history with
// the resume/fork hydration metadata derived from the same replay.
#[derive(Debug)]
pub(super) struct RolloutReconstruction {
    pub(super) history: Vec<ResponseItem>,
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

#[derive(Debug, Default)]
struct ActiveReplaySegment<'a> {
    turn_id: Option<String>,
    counts_as_user_turn: bool,
    previous_turn_settings: Option<PreviousTurnSettings>,
    reference_context_item: TurnReferenceContextItem,
    world_state_replay: Vec<&'a RolloutItem>,
    base_replacement_history: Option<&'a [ResponseItem]>,
    window: Option<ReconstructedWindow>,
    base_remote_compaction_model: Option<&'a str>,
    base_compacted_index: Option<usize>,
    segment_start_index: Option<usize>,
    segment_end_index: Option<usize>,
}

#[derive(Debug, Default)]
struct ActiveReplayProgress<'a> {
    base_replacement_history: Option<&'a [ResponseItem]>,
    previous_turn_settings: Option<PreviousTurnSettings>,
    reference_context_item: TurnReferenceContextItem,
    world_state_replay: Vec<&'a RolloutItem>,
    window: Option<ReconstructedWindow>,
    active_remote_compaction_model: Option<String>,
    surviving_newer_rollout_items: Vec<RolloutItem>,
    surviving_rollout_suffix: Option<Vec<RolloutItem>>,
    pending_rollback_turns: usize,
}

impl ActiveReplaySegment<'_> {
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

#[derive(Debug)]
struct MaterializedRolloutHistory {
    history: Vec<ResponseItem>,
    saw_legacy_compaction_without_replacement_history: bool,
}

fn turn_ids_are_compatible(active_turn_id: Option<&str>, item_turn_id: Option<&str>) -> bool {
    active_turn_id
        .is_none_or(|turn_id| item_turn_id.is_none_or(|item_turn_id| item_turn_id == turn_id))
}

fn finalize_active_segment<'a>(
    active_segment: ActiveReplaySegment<'a>,
    progress: &mut ActiveReplayProgress<'a>,
    rollout_items: &'a [RolloutItem],
) {
    // Thread rollback drops the newest surviving real user-message boundaries. In replay, that
    // means skipping the next finalized segments that contain a non-contextual
    // `EventMsg::UserMessage`.
    if progress.pending_rollback_turns > 0 {
        if active_segment.counts_as_user_turn {
            progress.pending_rollback_turns -= 1;
        }
        return;
    }

    // A surviving replacement-history checkpoint is a complete history base. Once we
    // know the newest surviving one, older rollout items do not affect rebuilt history.
    if progress.base_replacement_history.is_none()
        && let Some(segment_base_replacement_history) = active_segment.base_replacement_history
    {
        progress.base_replacement_history = Some(segment_base_replacement_history);
        progress.active_remote_compaction_model = active_segment
            .base_remote_compaction_model
            .map(str::to_string);
        let mut suffix = active_segment
            .base_compacted_index
            .zip(active_segment.segment_end_index)
            .map(|(base_index, segment_end)| {
                rollout_items[base_index.saturating_add(1)..segment_end].to_vec()
            })
            .unwrap_or_default();
        suffix.append(&mut progress.surviving_newer_rollout_items);
        progress.surviving_rollout_suffix = Some(suffix);
    } else if progress.base_replacement_history.is_none()
        && let (Some(start), Some(end)) = (
            active_segment.segment_start_index,
            active_segment.segment_end_index,
        )
    {
        let mut segment_items = rollout_items[start..end].to_vec();
        segment_items.append(&mut progress.surviving_newer_rollout_items);
        progress.surviving_newer_rollout_items = segment_items;
    }

    progress
        .world_state_replay
        .extend(active_segment.world_state_replay);

    if progress.window.is_none() {
        progress.window = active_segment.window;
    }

    // `previous_turn_settings` come from the newest surviving user turn that established them.
    if progress.previous_turn_settings.is_none() && active_segment.counts_as_user_turn {
        progress.previous_turn_settings = active_segment.previous_turn_settings;
    }

    // `reference_context_item` comes from the newest surviving user turn baseline, or
    // from a surviving compaction that explicitly cleared that baseline.
    if matches!(
        progress.reference_context_item,
        TurnReferenceContextItem::NeverSet
    ) && (active_segment.counts_as_user_turn
        || matches!(
            active_segment.reference_context_item,
            TurnReferenceContextItem::Cleared
        ))
    {
        progress.reference_context_item = active_segment.reference_context_item;
    }
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
                    .is_some_and(active_history_has_remote_compaction) =>
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
                active_segment.counts_as_user_turn |= is_user_turn_boundary(response_item);
            }
            RolloutItem::InterAgentCommunication(_) => {
                let active_segment =
                    active_segment.get_or_insert_with(CheckpointReplaySegment::default);
                active_segment.include_rollout_index(index);
                active_segment.counts_as_user_turn = true;
            }
            RolloutItem::EventMsg(_)
            | RolloutItem::SessionMeta(_)
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

fn newest_raw_remote_compaction_checkpoint(
    rollout_items: &[RolloutItem],
) -> Option<ActiveRemoteCompactionCheckpoint> {
    rollout_items
        .iter()
        .enumerate()
        .rev()
        .find_map(|(index, item)| match item {
            RolloutItem::Compacted(compacted)
                if compacted
                    .replacement_history
                    .as_deref()
                    .is_some_and(active_history_has_remote_compaction) =>
            {
                Some(ActiveRemoteCompactionCheckpoint {
                    index,
                    surviving_suffix: rollout_items[index.saturating_add(1)..].to_vec(),
                })
            }
            _ => None,
        })
}

fn materialize_rollout_items(
    turn_context: &TurnContext,
    initial_history: Vec<ResponseItem>,
    rollout_items: &[RolloutItem],
) -> MaterializedRolloutHistory {
    let mut history = ContextManager::new();
    history.replace(initial_history);
    let mut saw_legacy_compaction_without_replacement_history = false;

    for item in rollout_items {
        match item {
            RolloutItem::ResponseItem(response_item) => {
                history.record_items(
                    std::iter::once(response_item),
                    turn_context.model_info.truncation_policy.into(),
                );
            }
            RolloutItem::InterAgentCommunication(communication) => {
                let response_item = communication.to_model_input_item();
                history.record_items(
                    std::iter::once(&response_item),
                    turn_context.model_info.truncation_policy.into(),
                );
            }
            RolloutItem::Compacted(compacted) => {
                if let Some(replacement_history) = &compacted.replacement_history {
                    history.replace(replacement_history.clone());
                } else {
                    saw_legacy_compaction_without_replacement_history = true;
                    let user_messages = compact::collect_user_messages(history.raw_items());
                    let rebuilt = compact::build_compacted_history(
                        Vec::new(),
                        &user_messages,
                        &compacted.message,
                    );
                    history.replace(rebuilt);
                }
            }
            RolloutItem::EventMsg(EventMsg::ThreadRolledBack(rollback)) => {
                history.drop_last_n_user_turns(rollback.num_turns);
            }
            RolloutItem::EventMsg(_)
            | RolloutItem::TurnContext(_)
            | RolloutItem::WorldState(_)
            | RolloutItem::InterAgentCommunicationMetadata { .. }
            | RolloutItem::SessionMeta(_) => {}
        }
    }

    MaterializedRolloutHistory {
        history: history.raw_items().to_vec(),
        saw_legacy_compaction_without_replacement_history,
    }
}

pub(super) fn reconstruct_retro_local_history_from_rollout(
    turn_context: &TurnContext,
    rollout_items: &[RolloutItem],
) -> CodexResult<Vec<ResponseItem>> {
    let Some(remote_checkpoint) = active_remote_compaction_checkpoint(rollout_items)
        .or_else(|| newest_raw_remote_compaction_checkpoint(rollout_items))
    else {
        return Err(CodexErr::InvalidRequest(
            "Cannot run retro-local fallback: no remote compaction checkpoint with replacement history is available."
                .to_string(),
        ));
    };
    let remote_checkpoint_index = remote_checkpoint.index;

    let prefix = materialize_rollout_items(
        turn_context,
        Vec::new(),
        &rollout_items[..remote_checkpoint_index],
    );
    if active_history_has_remote_compaction(&prefix.history) {
        return Err(CodexErr::InvalidRequest(
            "Cannot run retro-local fallback: readable source history still contains encrypted remote compaction before the selected checkpoint."
                .to_string(),
        ));
    }

    let reconstructed = materialize_rollout_items(
        turn_context,
        prefix.history,
        &remote_checkpoint.surviving_suffix,
    )
    .history;
    if active_history_has_remote_compaction(&reconstructed) {
        return Err(CodexErr::InvalidRequest(
            "Cannot run retro-local fallback: reconstructed suffix still contains encrypted remote compaction."
                .to_string(),
        ));
    }

    Ok(reconstructed)
}

impl Session {
    pub(crate) async fn reconstruct_retro_local_history_from_persisted_rollout(
        &self,
        turn_context: &TurnContext,
    ) -> CodexResult<Vec<ResponseItem>> {
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
        reconstruct_retro_local_history_from_rollout(turn_context, &history.items)
    }

    pub(super) async fn reconstruct_history_from_rollout(
        &self,
        turn_context: &TurnContext,
        rollout_items: &[RolloutItem],
    ) -> RolloutReconstruction {
        // Replay metadata should already match the shape of the future lazy reverse loader, even
        // while history materialization still uses an eager bridge. Scan newest-to-oldest,
        // stopping once a surviving replacement-history checkpoint and the required resume metadata
        // are both known; then replay only the buffered surviving tail forward to preserve exact
        // history semantics.
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
        let mut offload_ever_used = false;
        let mut progress = ActiveReplayProgress::default();
        // Reverse replay accumulates rollout items into the newest in-progress turn segment until
        // we hit its matching `TurnStarted`, at which point the segment can be finalized.
        let mut active_segment: Option<ActiveReplaySegment<'_>> = None;

        for (index, item) in rollout_items.iter().enumerate().rev() {
            match item {
                RolloutItem::Compacted(compacted) => {
                    let active_segment =
                        active_segment.get_or_insert_with(ActiveReplaySegment::default);
                    active_segment.include_rollout_index(index);
                    active_segment.world_state_replay.push(item);
                    if active_segment.window.is_none()
                        && let Some(window_number) = compacted.window_number
                    {
                        active_segment.window = Some(ReconstructedWindow {
                            number: window_number,
                            first_id: compacted.first_window_id.as_deref().and_then(parse_uuid_v7),
                            previous_id: compacted
                                .previous_window_id
                                .as_deref()
                                .and_then(parse_uuid_v7),
                            id: compacted.window_id.as_deref().and_then(parse_uuid_v7),
                        });
                    }
                    // Looking backward, compaction clears any older baseline unless a newer
                    // `TurnContextItem` in this same segment has already re-established it.
                    if matches!(
                        active_segment.reference_context_item,
                        TurnReferenceContextItem::NeverSet
                    ) {
                        active_segment.reference_context_item = TurnReferenceContextItem::Cleared;
                    }
                    if active_segment.base_replacement_history.is_none()
                        && let Some(replacement_history) = &compacted.replacement_history
                    {
                        active_segment.base_replacement_history = Some(replacement_history);
                        active_segment.base_remote_compaction_model =
                            compacted.remote_compaction_model.as_deref();
                        active_segment.base_compacted_index = Some(index);
                    }
                }
                RolloutItem::EventMsg(EventMsg::ThreadRolledBack(rollback)) => {
                    progress.pending_rollback_turns = progress
                        .pending_rollback_turns
                        .saturating_add(usize::try_from(rollback.num_turns).unwrap_or(usize::MAX));
                }
                RolloutItem::EventMsg(EventMsg::TurnComplete(event)) => {
                    let active_segment =
                        active_segment.get_or_insert_with(ActiveReplaySegment::default);
                    active_segment.include_rollout_index(index);
                    // Reverse replay often sees `TurnComplete` before any turn-scoped metadata.
                    // Capture the turn id early so later `TurnContext` / abort items can match it.
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
                        active_segment = Some(ActiveReplaySegment {
                            turn_id: Some(turn_id.clone()),
                            segment_start_index: Some(index),
                            segment_end_index: Some(index.saturating_add(1)),
                            ..Default::default()
                        });
                    }
                }
                RolloutItem::EventMsg(EventMsg::UserMessage(_)) => {
                    let active_segment =
                        active_segment.get_or_insert_with(ActiveReplaySegment::default);
                    active_segment.include_rollout_index(index);
                    active_segment.counts_as_user_turn = true;
                }
                RolloutItem::TurnContext(ctx) => {
                    offload_ever_used |= ctx.offload_ever_used;
                    let active_segment =
                        active_segment.get_or_insert_with(ActiveReplaySegment::default);
                    active_segment.include_rollout_index(index);
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
                    active_segment.include_rollout_index(index);
                    active_segment.world_state_replay.push(item);
                }
                RolloutItem::EventMsg(EventMsg::TurnStarted(event)) => {
                    // `TurnStarted` is the oldest boundary of the active reverse segment.
                    if active_segment.as_ref().is_some_and(|active_segment| {
                        turn_ids_are_compatible(
                            active_segment.turn_id.as_deref(),
                            Some(event.turn_id.as_str()),
                        )
                    }) && let Some(mut active_segment) = active_segment.take()
                    {
                        active_segment.include_rollout_index(index);
                        finalize_active_segment(active_segment, &mut progress, rollout_items);
                    }
                }
                RolloutItem::ResponseItem(response_item) => {
                    let active_segment =
                        active_segment.get_or_insert_with(ActiveReplaySegment::default);
                    active_segment.include_rollout_index(index);
                    active_segment.counts_as_user_turn |= is_user_turn_boundary(response_item);
                }
                RolloutItem::InterAgentCommunication(_) => {
                    let active_segment =
                        active_segment.get_or_insert_with(ActiveReplaySegment::default);
                    active_segment.include_rollout_index(index);
                    active_segment.counts_as_user_turn = true;
                }
                RolloutItem::EventMsg(_)
                | RolloutItem::SessionMeta(_)
                | RolloutItem::InterAgentCommunicationMetadata { .. } => {
                    if let Some(active_segment) = active_segment.as_mut() {
                        active_segment.include_rollout_index(index);
                    }
                }
            }

            if progress.base_replacement_history.is_some()
                && progress.previous_turn_settings.is_some()
                && !matches!(
                    progress.reference_context_item,
                    TurnReferenceContextItem::NeverSet
                )
            {
                // At this point we have both eager resume metadata values and the replacement-
                // history base for the surviving tail, so older rollout items cannot affect this
                // result.
                break;
            }
        }

        if let Some(active_segment) = active_segment.take() {
            finalize_active_segment(active_segment, &mut progress, rollout_items);
        }

        let fallback_window_number = u64::try_from(
            rollout_items
                .iter()
                .filter(|item| matches!(item, RolloutItem::Compacted(_)))
                .count(),
        )
        .unwrap_or(u64::MAX);

        let initial_history = progress
            .base_replacement_history
            .map(<[ResponseItem]>::to_vec)
            .unwrap_or_default();
        let materialized = if progress.base_replacement_history.is_some() {
            materialize_rollout_items(
                turn_context,
                initial_history,
                &progress.surviving_rollout_suffix.unwrap_or_default(),
            )
        } else {
            materialize_rollout_items(turn_context, initial_history, rollout_items)
        };
        let saw_legacy_compaction_without_replacement_history =
            materialized.saw_legacy_compaction_without_replacement_history;

        let reference_context_item = match progress.reference_context_item {
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
        let offload_ever_used = offload_ever_used
            || reference_context_item
                .as_ref()
                .is_some_and(|context_item| context_item.offload_ever_used);

        // Segments and their contents were collected newest-first; replay the surviving records
        // chronologically so compaction resets and merge patches have their original meaning.
        progress.world_state_replay.reverse();
        let mut world_state_baseline: Option<WorldStateSnapshot> = None;
        for item in progress.world_state_replay {
            match item {
                RolloutItem::Compacted(_) => world_state_baseline = None,
                RolloutItem::WorldState(world_state) if world_state.full => {
                    world_state_baseline = match serde_json::from_value(world_state.state.clone()) {
                        Ok(snapshot) => Some(snapshot),
                        Err(err) => {
                            tracing::warn!(%err, "failed to restore world-state snapshot");
                            None
                        }
                    };
                }
                RolloutItem::WorldState(world_state) => {
                    let Some(baseline) = world_state_baseline.as_mut() else {
                        tracing::warn!("ignored world-state patch without a full snapshot");
                        continue;
                    };
                    if let Err(err) = baseline.apply_merge_patch(&world_state.state) {
                        tracing::warn!(%err, "failed to apply world-state patch");
                        world_state_baseline = None;
                    }
                }
                RolloutItem::SessionMeta(_)
                | RolloutItem::ResponseItem(_)
                | RolloutItem::InterAgentCommunication(_)
                | RolloutItem::InterAgentCommunicationMetadata { .. }
                | RolloutItem::TurnContext(_)
                | RolloutItem::EventMsg(_) => {
                    unreachable!("only world-state replay items are collected")
                }
            }
        }

        let window = progress
            .window
            .or(initial_window)
            .unwrap_or(ReconstructedWindow {
                number: fallback_window_number,
                first_id: None,
                previous_id: None,
                id: None,
            });
        RolloutReconstruction {
            history: materialized.history,
            previous_turn_settings: progress.previous_turn_settings,
            reference_context_item,
            world_state_baseline,
            window_number: window.number,
            first_window_id: window.first_id,
            previous_window_id: window.previous_id,
            window_id: window.id,
            offload_ever_used,
            active_remote_compaction_model: progress.active_remote_compaction_model,
        }
    }
}

fn parse_uuid_v7(value: &str) -> Option<Uuid> {
    Uuid::parse_str(value)
        .ok()
        .filter(|uuid| uuid.get_version_num() == 7)
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
