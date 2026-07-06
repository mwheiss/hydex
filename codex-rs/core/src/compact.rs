use std::sync::Arc;
use std::time::Instant;

use crate::Prompt;
use crate::client::ModelClientSession;
use crate::client_common::ResponseEvent;
use crate::hook_runtime::PostCompactHookOutcome;
use crate::hook_runtime::PreCompactHookOutcome;
use crate::hook_runtime::run_post_compact_hooks;
use crate::hook_runtime::run_pre_compact_hooks;
use crate::local_output_validation::CheapValidationOutcome;
use crate::local_output_validation::LocalOutputKind;
use crate::local_output_validation::LocalOutputValidationResult;
use crate::local_output_validation::cheap_validate_local_output;
use crate::local_output_validation::validate_local_output_with_model;
use crate::responses_metadata::CodexResponsesMetadata;
use crate::responses_metadata::CodexResponsesRequestKind;
use crate::responses_metadata::CompactionTurnMetadata;
#[cfg(test)]
use crate::session::PreviousTurnSettings;
use crate::session::session::Session;
use crate::session::turn::get_last_assistant_message_from_turn;
use crate::session::turn_context::TurnContext;
use crate::util::backoff;
use codex_analytics::CodexCompactionEvent;
use codex_analytics::CompactionImplementation;
use codex_analytics::CompactionPhase;
use codex_analytics::CompactionReason;
use codex_analytics::CompactionStatus;
use codex_analytics::CompactionStrategy;
use codex_analytics::CompactionTrigger;
use codex_analytics::now_unix_seconds;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result as CodexResult;
use codex_protocol::items::ContextCompactionItem;
use codex_protocol::items::TurnItem;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseInputItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::models::ResponseItemMetadata;
use codex_protocol::protocol::CompactedItem;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::TurnStartedEvent;
use codex_protocol::protocol::WarningEvent;
use codex_protocol::user_input::UserInput;
use codex_rollout_trace::InferenceTraceContext;
use codex_utils_output_truncation::TruncationPolicy;
use codex_utils_output_truncation::approx_token_count;
use codex_utils_output_truncation::truncate_text;
use futures::prelude::*;
use tracing::error;

use codex_config::config_toml::ModelOffloadCompactionLocalHandoffRole;
use codex_config::config_toml::ModelOffloadCompactionPolicy;
use codex_model_provider_info::ModelProviderInfo;

pub use codex_prompts::ASSISTANT_STATE_LOCAL_COMPACTION_PROMPT;
pub use codex_prompts::SUMMARIZATION_PROMPT;
pub use codex_prompts::SUMMARY_PREFIX;
const COMPACT_USER_MESSAGE_MAX_TOKENS: usize = 20_000;

/// Controls whether compaction replacement history must include initial context.
///
/// Pre-turn/manual compaction variants use `DoNotInject`: they replace history with a summary and
/// clear `reference_context_item`, so the next regular turn will fully reinject initial context
/// after compaction.
///
/// Mid-turn compaction must use `BeforeLastUserMessage` because the model is trained to see the
/// compaction summary as the last item in history after mid-turn compaction; we therefore inject
/// initial context into the replacement history just above the last real user message.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InitialContextInjection {
    BeforeLastUserMessage,
    DoNotInject,
}

pub(crate) fn should_use_remote_compact_task(provider: &ModelProviderInfo) -> bool {
    provider.supports_remote_compaction()
}

pub(crate) fn should_use_remote_compact_task_with_offload_policy(
    provider: &ModelProviderInfo,
    offload_ever_used: bool,
    effective_model_offload_enabled: bool,
    offload_compaction_policy: ModelOffloadCompactionPolicy,
) -> bool {
    if effective_model_offload_enabled
        && offload_ever_used
        && offload_compaction_policy == ModelOffloadCompactionPolicy::Local
    {
        return false;
    }

    should_use_remote_compact_task(provider)
}

pub(crate) async fn run_inline_auto_compact_task(
    sess: Arc<Session>,
    turn_context: Arc<TurnContext>,
    initial_context_injection: InitialContextInjection,
    reason: CompactionReason,
    phase: CompactionPhase,
) -> CodexResult<()> {
    let prompt = local_compaction_prompt(&turn_context).to_string();
    let input = vec![UserInput::Text {
        text: prompt,
        // Compaction prompt is synthesized; no UI element ranges to preserve.
        text_elements: Vec::new(),
    }];

    run_compact_task_inner(
        sess,
        turn_context,
        input,
        initial_context_injection,
        CompactionTrigger::Auto,
        reason,
        phase,
    )
    .await?;
    Ok(())
}

pub(crate) async fn run_compact_task(
    sess: Arc<Session>,
    turn_context: Arc<TurnContext>,
    input: Vec<UserInput>,
) -> CodexResult<()> {
    let start_event = EventMsg::TurnStarted(TurnStartedEvent {
        turn_id: turn_context.sub_id.clone(),
        trace_id: turn_context.trace_id.clone(),
        started_at: turn_context.turn_timing_state.started_at_unix_secs().await,
        model_context_window: turn_context.model_context_window(),
        collaboration_mode_kind: turn_context.collaboration_mode.mode,
    });
    sess.send_event(&turn_context, start_event).await;
    run_compact_task_inner(
        sess.clone(),
        turn_context,
        input,
        InitialContextInjection::DoNotInject,
        CompactionTrigger::Manual,
        CompactionReason::UserRequested,
        CompactionPhase::StandaloneTurn,
    )
    .await?;
    Ok(())
}

pub(crate) fn local_compaction_prompt(turn_context: &TurnContext) -> &str {
    turn_context.config.compact_prompt.as_deref().unwrap_or(
        match turn_context
            .config
            .model_offload
            .compaction_local_handoff_role
        {
            ModelOffloadCompactionLocalHandoffRole::UserSummary => SUMMARIZATION_PROMPT,
            ModelOffloadCompactionLocalHandoffRole::AssistantState => {
                ASSISTANT_STATE_LOCAL_COMPACTION_PROMPT
            }
        },
    )
}

fn local_compaction_summary_text(
    summary_suffix: &str,
    local_handoff_role: ModelOffloadCompactionLocalHandoffRole,
) -> String {
    match local_handoff_role {
        ModelOffloadCompactionLocalHandoffRole::UserSummary => {
            format!("{SUMMARY_PREFIX}\n{summary_suffix}")
        }
        ModelOffloadCompactionLocalHandoffRole::AssistantState => summary_suffix.to_string(),
    }
}

async fn run_compact_task_inner(
    sess: Arc<Session>,
    turn_context: Arc<TurnContext>,
    input: Vec<UserInput>,
    initial_context_injection: InitialContextInjection,
    trigger: CompactionTrigger,
    reason: CompactionReason,
    phase: CompactionPhase,
) -> CodexResult<()> {
    let compaction_metadata =
        CompactionTurnMetadata::new(trigger, reason, CompactionImplementation::Responses, phase);
    let attempt = CompactionAnalyticsAttempt::begin(
        sess.as_ref(),
        turn_context.as_ref(),
        trigger,
        reason,
        CompactionImplementation::Responses,
        phase,
    )
    .await;
    let pre_compact_outcome = run_pre_compact_hooks(&sess, &turn_context, trigger).await;
    match pre_compact_outcome {
        PreCompactHookOutcome::Continue => {}
        PreCompactHookOutcome::Stopped => {
            let error = CodexErr::TurnAborted;
            attempt
                .track(
                    sess.as_ref(),
                    CompactionStatus::Interrupted,
                    Some(&error),
                    CompactionAnalyticsDetails::default(),
                )
                .await;
            return Err(error);
        }
    }
    let result = run_compact_task_inner_impl(
        Arc::clone(&sess),
        Arc::clone(&turn_context),
        input,
        initial_context_injection,
        compaction_metadata,
    )
    .await;
    let status = compaction_status_from_result(&result);
    let codex_error = result.as_ref().err();
    if result.is_ok() {
        let post_compact_outcome = run_post_compact_hooks(&sess, &turn_context, trigger).await;
        if let PostCompactHookOutcome::Stopped = post_compact_outcome {
            attempt
                .track(
                    sess.as_ref(),
                    status,
                    codex_error,
                    CompactionAnalyticsDetails::default(),
                )
                .await;
            return Err(CodexErr::TurnAborted);
        }
    }
    attempt
        .track(
            sess.as_ref(),
            status,
            codex_error,
            CompactionAnalyticsDetails::default(),
        )
        .await;
    result.map(|_| ())
}

async fn run_compact_task_inner_impl(
    sess: Arc<Session>,
    turn_context: Arc<TurnContext>,
    input: Vec<UserInput>,
    initial_context_injection: InitialContextInjection,
    compaction_metadata: CompactionTurnMetadata,
) -> CodexResult<String> {
    let compaction_item = TurnItem::ContextCompaction(ContextCompactionItem::new());
    sess.emit_turn_item_started(&turn_context, &compaction_item)
        .await;
    let initial_input_for_turn: ResponseInputItem = ResponseInputItem::from(input);

    let mut history = sess.clone_history().await;
    history.record_items(
        &[initial_input_for_turn.into()],
        turn_context.model_info.truncation_policy.into(),
    );

    let max_retries = turn_context.provider.info().stream_max_retries();
    let mut retries = 0;
    let mut client_session = sess.services.model_client.new_session();
    // Reuse one client session so turn-scoped state (sticky routing, websocket incremental
    // request tracking)
    // survives retries within this compact turn.
    let window_id = sess.current_window_id().await;
    let responses_metadata = turn_context.turn_metadata_state.to_responses_metadata(
        sess.installation_id.clone(),
        window_id.clone(),
        CodexResponsesRequestKind::Compaction(compaction_metadata),
    );
    let compaction_routes_local = client_session.is_local_offload_route_for(&responses_metadata);

    let validation_metadata = turn_context.turn_metadata_state.to_responses_metadata(
        sess.installation_id.clone(),
        window_id.clone(),
        CodexResponsesRequestKind::LocalOutputValidation,
    );
    let generation_retries = turn_context
        .config
        .model_offload
        .validation
        .generation_retries;
    let mut generation_attempts = 0;
    let (history_snapshot, summary_suffix, summary_text) = loop {
        loop {
            // Clone is required because of the loop
            let turn_input = history
                .clone()
                .for_prompt(&turn_context.model_info.input_modalities);
            let turn_input_len = turn_input.len();
            let prompt = Prompt {
                input: turn_input,
                temperature: (generation_attempts > 0).then_some(
                    turn_context
                        .config
                        .model_offload
                        .validation
                        .retry_temperature,
                ),
                base_instructions: sess.get_base_instructions().await,
                ..Default::default()
            };
            let attempt_result = drain_to_completed(
                &sess,
                turn_context.as_ref(),
                &mut client_session,
                &responses_metadata,
                &prompt,
            )
            .await;

            match attempt_result {
                Ok(()) => {
                    break;
                }
                Err(CodexErr::Interrupted) => {
                    return Err(CodexErr::Interrupted);
                }
                Err(e @ CodexErr::ContextWindowExceeded) => {
                    if turn_input_len > 1 {
                        // Trim from the beginning to preserve cache (prefix-based) and keep recent messages intact.
                        error!(
                            "Context window exceeded while compacting; removing oldest history item. Error: {e}"
                        );
                        history.remove_first_item();
                        retries = 0;
                        continue;
                    }
                    sess.set_total_tokens_full(turn_context.as_ref()).await;
                    sess.track_turn_codex_error(turn_context.as_ref(), &e);
                    let event = EventMsg::Error(e.to_error_event(/*message_prefix*/ None));
                    sess.send_event(&turn_context, event).await;
                    return Err(e);
                }
                Err(e) => {
                    if retries < max_retries {
                        retries += 1;
                        let delay = backoff(retries);
                        sess.notify_stream_error(
                            turn_context.as_ref(),
                            format!("Reconnecting... {retries}/{max_retries}"),
                            e,
                        )
                        .await;
                        tokio::time::sleep(delay).await;
                        continue;
                    } else {
                        sess.track_turn_codex_error(turn_context.as_ref(), &e);
                        let event = EventMsg::Error(e.to_error_event(/*message_prefix*/ None));
                        sess.send_event(&turn_context, event).await;
                        return Err(e);
                    }
                }
            }
        }

        let history_snapshot = sess.clone_history().await;
        let history_items = history_snapshot.raw_items();
        let summary_suffix =
            get_last_assistant_message_from_turn(history_items).unwrap_or_default();
        let local_handoff_role = turn_context
            .config
            .model_offload
            .compaction_local_handoff_role;
        let summary_text = local_compaction_summary_text(&summary_suffix, local_handoff_role);
        match validate_local_compaction_payload_with_model(
            turn_context.as_ref(),
            &mut client_session,
            &validation_metadata,
            &summary_text,
            compaction_routes_local,
        )
        .await?
        {
            LocalOutputValidationResult::Accepted | LocalOutputValidationResult::Disabled => {
                break (history_snapshot, summary_suffix, summary_text);
            }
            LocalOutputValidationResult::Rejected(reason)
                if generation_attempts < generation_retries =>
            {
                generation_attempts += 1;
                tracing::warn!(
                    "local compaction output rejected by sanity validator; retrying generation ({generation_attempts}/{generation_retries}): {reason}"
                );
            }
            LocalOutputValidationResult::Rejected(reason) => {
                return Err(CodexErr::InvalidRequest(format!(
                    "Local compaction output failed sanity validation: {reason}"
                )));
            }
            LocalOutputValidationResult::ValidationUnavailable(reason) => {
                return Err(CodexErr::InvalidRequest(format!(
                    "Local compaction output validation unavailable: {reason}"
                )));
            }
        }
    };
    let history_items = history_snapshot.raw_items();
    let local_handoff_role = turn_context
        .config
        .model_offload
        .compaction_local_handoff_role;
    let user_messages = collect_user_messages(history_items);

    let mut new_history = build_compacted_history_with_handoff_role(
        Vec::new(),
        &user_messages,
        &summary_text,
        local_handoff_role,
    );
    let window_id = sess.advance_auto_compact_window_id().await;

    if matches!(
        initial_context_injection,
        InitialContextInjection::BeforeLastUserMessage
    ) {
        let initial_context = sess.build_initial_context(turn_context.as_ref()).await;
        new_history =
            insert_initial_context_before_last_real_user_or_summary(new_history, initial_context);
    }
    let reference_context_item = match initial_context_injection {
        InitialContextInjection::DoNotInject => None,
        InitialContextInjection::BeforeLastUserMessage => {
            Some(sess.turn_context_item(&turn_context))
        }
    };
    let compacted_item = CompactedItem {
        message: summary_text.clone(),
        replacement_history: Some(new_history.clone()),
        remote_compaction_model: None,
        window_id: Some(window_id),
    };
    sess.replace_compacted_history(new_history, reference_context_item, compacted_item)
        .await;
    sess.recompute_token_usage(&turn_context).await;

    sess.emit_turn_item_completed(&turn_context, compaction_item)
        .await;
    let warning = EventMsg::Warning(WarningEvent {
        message: "Heads up: Long threads and multiple compactions can cause the model to be less accurate. Start a new thread when possible to keep threads small and targeted.".to_string(),
    });
    sess.send_event(&turn_context, warning).await;
    Ok(summary_suffix)
}

fn validate_local_compaction_payload(
    turn_context: &TurnContext,
    summary_text: &str,
) -> CodexResult<()> {
    match cheap_validate_local_output(
        &turn_context.config.model_offload.validation,
        LocalOutputKind::CompactionPayload,
        summary_text,
    ) {
        CheapValidationOutcome::Pass | CheapValidationOutcome::Disabled => {}
        CheapValidationOutcome::Reject(reason) => {
            return Err(CodexErr::InvalidRequest(format!(
                "Local compaction output failed sanity validation: {reason}"
            )));
        }
    }
    Ok(())
}

async fn validate_local_compaction_payload_with_model(
    turn_context: &TurnContext,
    client_session: &mut ModelClientSession,
    responses_metadata: &CodexResponsesMetadata,
    summary_text: &str,
    compaction_routes_local: bool,
) -> CodexResult<LocalOutputValidationResult> {
    validate_local_compaction_payload(turn_context, summary_text)?;
    if !compaction_routes_local {
        return Ok(LocalOutputValidationResult::Disabled);
    }
    validate_local_output_with_model(
        &turn_context.config.model_offload.validation,
        LocalOutputKind::CompactionPayload,
        summary_text,
        client_session,
        &turn_context.model_info,
        &turn_context.session_telemetry,
        turn_context.reasoning_effort.clone(),
        turn_context.reasoning_summary,
        turn_context.config.service_tier.clone(),
        responses_metadata,
    )
    .await
}

pub(crate) struct CompactionAnalyticsAttempt {
    thread_id: String,
    turn_id: String,
    trigger: CompactionTrigger,
    reason: CompactionReason,
    implementation: CompactionImplementation,
    phase: CompactionPhase,
    active_context_tokens_before: i64,
    started_at: u64,
    start_instant: Instant,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct CompactionAnalyticsDetails {
    pub(crate) active_context_tokens_before: Option<i64>,
    pub(crate) retained_image_count: Option<usize>,
    pub(crate) compaction_summary_tokens: Option<i64>,
    pub(crate) cached_input_tokens: Option<i64>,
}

impl CompactionAnalyticsAttempt {
    pub(crate) async fn begin(
        sess: &Session,
        turn_context: &TurnContext,
        trigger: CompactionTrigger,
        reason: CompactionReason,
        implementation: CompactionImplementation,
        phase: CompactionPhase,
    ) -> Self {
        let active_context_tokens_before = sess.get_total_token_usage().await;
        Self {
            thread_id: sess.thread_id.to_string(),
            turn_id: turn_context.sub_id.clone(),
            trigger,
            reason,
            implementation,
            phase,
            active_context_tokens_before,
            started_at: now_unix_seconds(),
            start_instant: Instant::now(),
        }
    }

    pub(crate) async fn track(
        self,
        sess: &Session,
        status: CompactionStatus,
        codex_error: Option<&CodexErr>,
        details: CompactionAnalyticsDetails,
    ) {
        let CompactionAnalyticsDetails {
            active_context_tokens_before,
            retained_image_count,
            compaction_summary_tokens,
            cached_input_tokens,
        } = details;
        let active_context_tokens_before =
            active_context_tokens_before.unwrap_or(self.active_context_tokens_before);
        let active_context_tokens_after = sess.get_total_token_usage().await;
        sess.services
            .analytics_events_client
            .track_compaction(CodexCompactionEvent {
                thread_id: self.thread_id,
                turn_id: self.turn_id,
                trigger: self.trigger,
                reason: self.reason,
                implementation: self.implementation,
                phase: self.phase,
                strategy: CompactionStrategy::Memento,
                status,
                codex_error_kind: codex_error.map(Into::into),
                codex_error_http_status_code: codex_error
                    .and_then(CodexErr::http_status_code_value),
                active_context_tokens_before,
                active_context_tokens_after,
                retained_image_count,
                compaction_summary_tokens,
                cached_input_tokens,
                started_at: self.started_at,
                completed_at: now_unix_seconds(),
                duration_ms: Some(
                    u64::try_from(self.start_instant.elapsed().as_millis()).unwrap_or(u64::MAX),
                ),
            });
    }
}

pub(crate) fn compaction_status_from_result<T>(result: &CodexResult<T>) -> CompactionStatus {
    match result {
        Ok(_) => CompactionStatus::Completed,
        Err(CodexErr::Interrupted | CodexErr::TurnAborted) => CompactionStatus::Interrupted,
        Err(_) => CompactionStatus::Failed,
    }
}

pub fn content_items_to_text(content: &[ContentItem]) -> Option<String> {
    let mut pieces = Vec::new();
    for item in content {
        match item {
            ContentItem::InputText { text } | ContentItem::OutputText { text } => {
                if !text.is_empty() {
                    pieces.push(text.as_str());
                }
            }
            ContentItem::InputImage { .. } => {}
        }
    }
    if pieces.is_empty() {
        None
    } else {
        Some(pieces.join("\n"))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CompactedUserMessage {
    message: String,
    metadata: Option<ResponseItemMetadata>,
}

pub(crate) fn collect_user_messages(items: &[ResponseItem]) -> Vec<CompactedUserMessage> {
    items
        .iter()
        .filter_map(|item| match crate::event_mapping::parse_turn_item(item) {
            Some(TurnItem::UserMessage(user)) => {
                if is_summary_message(&user.message()) {
                    None
                } else {
                    Some(CompactedUserMessage {
                        message: user.message(),
                        metadata: match item {
                            ResponseItem::Message { metadata, .. } => metadata.clone(),
                            _ => None,
                        },
                    })
                }
            }
            _ => None,
        })
        .collect()
}

pub(crate) fn is_summary_message(message: &str) -> bool {
    message.starts_with(format!("{SUMMARY_PREFIX}\n").as_str())
}

/// Inserts canonical initial context into compacted replacement history at the
/// model-expected boundary.
///
/// Placement rules:
/// - Prefer immediately before the last real user message.
/// - If no real user messages remain, insert before the compaction summary so
///   the summary stays last.
/// - If there are no user messages, insert before the last compaction item so
///   that item remains last (remote compaction may return only compaction items).
/// - If there are no user messages or compaction items, append the context.
pub(crate) fn insert_initial_context_before_last_real_user_or_summary(
    mut compacted_history: Vec<ResponseItem>,
    initial_context: Vec<ResponseItem>,
) -> Vec<ResponseItem> {
    let mut last_user_or_summary_index = None;
    let mut last_real_user_index = None;
    for (i, item) in compacted_history.iter().enumerate().rev() {
        let Some(TurnItem::UserMessage(user)) = crate::event_mapping::parse_turn_item(item) else {
            continue;
        };
        // Compaction summaries are encoded as user messages, so track both:
        // the last real user message (preferred insertion point) and the last
        // user-message-like item (fallback summary insertion point).
        last_user_or_summary_index.get_or_insert(i);
        if !is_summary_message(&user.message()) {
            last_real_user_index = Some(i);
            break;
        }
    }
    let last_compaction_index = compacted_history
        .iter()
        .enumerate()
        .rev()
        .find_map(|(i, item)| {
            matches!(
                item,
                ResponseItem::Compaction { .. } | ResponseItem::ContextCompaction { .. }
            )
            .then_some(i)
        });
    let insertion_index = last_real_user_index
        .or(last_user_or_summary_index)
        .or(last_compaction_index);

    // Re-inject canonical context from the current session since we stripped it
    // from the pre-compaction history. Prefer placing it before the last real
    // user message; if there is no real user message left, place it before the
    // summary or compaction item so the compaction item remains last.
    if let Some(insertion_index) = insertion_index {
        compacted_history.splice(insertion_index..insertion_index, initial_context);
    } else {
        compacted_history.extend(initial_context);
    }

    compacted_history
}

pub(crate) fn build_compacted_history(
    initial_context: Vec<ResponseItem>,
    user_messages: &[CompactedUserMessage],
    summary_text: &str,
) -> Vec<ResponseItem> {
    build_compacted_history_with_handoff_role(
        initial_context,
        user_messages,
        summary_text,
        ModelOffloadCompactionLocalHandoffRole::UserSummary,
    )
}

pub(crate) fn build_compacted_history_with_handoff_role(
    initial_context: Vec<ResponseItem>,
    user_messages: &[CompactedUserMessage],
    summary_text: &str,
    local_handoff_role: ModelOffloadCompactionLocalHandoffRole,
) -> Vec<ResponseItem> {
    build_compacted_history_with_limit(
        initial_context,
        user_messages,
        summary_text,
        COMPACT_USER_MESSAGE_MAX_TOKENS,
        local_handoff_role,
    )
}

fn build_compacted_history_with_limit(
    mut history: Vec<ResponseItem>,
    user_messages: &[CompactedUserMessage],
    summary_text: &str,
    max_tokens: usize,
    local_handoff_role: ModelOffloadCompactionLocalHandoffRole,
) -> Vec<ResponseItem> {
    let mut selected_messages: Vec<CompactedUserMessage> = Vec::new();
    if max_tokens > 0 {
        let mut remaining = max_tokens;
        for message in user_messages.iter().rev() {
            if remaining == 0 {
                break;
            }
            let tokens = approx_token_count(&message.message);
            if tokens <= remaining {
                selected_messages.push(message.clone());
                remaining = remaining.saturating_sub(tokens);
            } else {
                let truncated =
                    truncate_text(&message.message, TruncationPolicy::Tokens(remaining));
                selected_messages.push(CompactedUserMessage {
                    message: truncated,
                    metadata: message.metadata.clone(),
                });
                break;
            }
        }
        selected_messages.reverse();
    }

    for message in &selected_messages {
        history.push(ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: message.message.clone(),
            }],
            phase: None,
            metadata: message.metadata.clone(),
        });
    }

    let summary_text = if summary_text.is_empty() {
        "(no summary available)".to_string()
    } else {
        summary_text.to_string()
    };

    let (role, content) = match local_handoff_role {
        ModelOffloadCompactionLocalHandoffRole::UserSummary => (
            "user".to_string(),
            vec![ContentItem::InputText { text: summary_text }],
        ),
        ModelOffloadCompactionLocalHandoffRole::AssistantState => (
            "assistant".to_string(),
            vec![ContentItem::OutputText { text: summary_text }],
        ),
    };
    history.push(ResponseItem::Message {
        id: None,
        role,
        content,
        phase: None,
        metadata: None,
    });

    history
}

async fn drain_to_completed(
    sess: &Session,
    turn_context: &TurnContext,
    client_session: &mut ModelClientSession,
    responses_metadata: &CodexResponsesMetadata,
    prompt: &Prompt,
) -> CodexResult<()> {
    let mut stream = client_session
        .stream(
            prompt,
            &turn_context.model_info,
            &turn_context.session_telemetry,
            turn_context.reasoning_effort.clone(),
            turn_context.reasoning_summary,
            turn_context.config.service_tier.clone(),
            responses_metadata,
            // Rollout tracing currently models remote compaction only; local compaction streams
            // are left untraced until the reducer has a first-class local compaction lifecycle.
            &InferenceTraceContext::disabled(),
        )
        .await?;
    loop {
        let maybe_event = stream.next().await;
        let Some(event) = maybe_event else {
            return Err(CodexErr::Stream(
                "stream closed before response.completed".into(),
                None,
            ));
        };
        match event {
            Ok(ResponseEvent::OutputItemDone(item)) => {
                sess.record_conversation_items(turn_context, std::slice::from_ref(&item))
                    .await;
            }
            Ok(ResponseEvent::ServerReasoningIncluded(included)) => {
                sess.set_server_reasoning_included(included).await;
            }
            Ok(ResponseEvent::RateLimits(snapshot)) => {
                sess.update_rate_limits(turn_context, snapshot).await;
            }
            Ok(ResponseEvent::Completed { token_usage, .. }) => {
                sess.update_token_usage_info(turn_context, token_usage.as_ref())
                    .await;
                return Ok(());
            }
            Ok(_) => continue,
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
#[path = "compact_tests.rs"]
mod tests;
