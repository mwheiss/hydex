use std::sync::Arc;

use crate::Prompt;
use crate::ResponseEvent;
use crate::client::ModelClientSession;
use crate::config::ModelOffloadCompactionRecoveryModel;
use crate::responses_metadata::CodexResponsesRequestKind;
use crate::session::session::Session;
use crate::session::turn_context::TurnContext;
use codex_config::config_toml::ModelOffloadCompactionRecoveryProjection;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result as CodexResult;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use codex_rollout_trace::InferenceTraceContext;
use futures::StreamExt;
use tracing::debug;

pub(crate) const REMOTE_COMPACTION_RECOVERY_SCAFFOLD: &str =
    "The assistant message above this line is the payload. Output the payload verbatim.";

pub(crate) const REMOTE_COMPACTION_RECOVERY_PROMPT: &str =
    "Do not add anything before or after the payload.";

// Recovery can legitimately expand an opaque checkpoint substantially. This ceiling is only a
// runaway-allocation guard; local token pressure is handled after promotion.
const RECOVERY_OUTPUT_MIN_BYTES: usize = 4 * 1024 * 1024;
const RECOVERY_OUTPUT_MAX_BYTES: usize = 64 * 1024 * 1024;
const RECOVERY_OUTPUT_FIXED_OVERHEAD_BYTES: usize = 1024 * 1024;
const RECOVERY_OUTPUT_EXPANSION_FACTOR: usize = 32;

pub(crate) fn build_remote_compaction_recovery_prompt(
    active_history: &[ResponseItem],
) -> CodexResult<Prompt> {
    let compaction_item_count = active_history
        .iter()
        .filter(|item| is_remote_compaction_item(item))
        .count();
    let Some(compaction_item) = suffix_most_remote_compaction_item(active_history) else {
        return Err(CodexErr::InvalidRequest(
            "Cannot recover remote compaction for local continuation: no encrypted compaction item is active."
                .to_string(),
        ));
    };
    if compaction_item_count > 1 {
        tracing::warn!(
            compaction_item_count,
            "remote compaction recovery found multiple active encrypted compaction items; recovering only the newest one"
        );
    }

    let mut input = vec![compaction_item.clone()];
    input.push(user_message(REMOTE_COMPACTION_RECOVERY_SCAFFOLD));
    input.push(user_message(REMOTE_COMPACTION_RECOVERY_PROMPT));

    Ok(Prompt {
        input,
        ..Prompt::default()
    })
}

pub(crate) fn active_history_has_remote_compaction(active_history: &[ResponseItem]) -> bool {
    active_history.iter().any(is_remote_compaction_item)
}

pub(crate) fn remote_compaction_recovery_needed(
    local_route_enabled: bool,
    active_history: &[ResponseItem],
) -> bool {
    local_route_enabled && active_history_has_remote_compaction(active_history)
}

pub(crate) fn project_recovered_remote_compaction(
    active_history: &[ResponseItem],
    recovered_text: String,
    projection: ModelOffloadCompactionRecoveryProjection,
) -> CodexResult<Vec<ResponseItem>> {
    let compaction_indices = active_history
        .iter()
        .enumerate()
        .filter_map(|(index, item)| is_remote_compaction_item(item).then_some(index))
        .collect::<Vec<_>>();
    let Some(last_compaction_index) = compaction_indices.last().copied() else {
        return Err(CodexErr::InvalidRequest(
            "Cannot promote recovered remote compaction: no encrypted compaction item is active."
                .to_string(),
        ));
    };
    if compaction_indices.len() > 1 {
        tracing::warn!(
            compaction_item_count = compaction_indices.len(),
            "remote compaction recovery found multiple active encrypted compaction items; promoting the newest one"
        );
    }

    let removed_before_insert = compaction_indices
        .iter()
        .filter(|index| **index < last_compaction_index)
        .count();
    let insert_index = last_compaction_index.saturating_sub(removed_before_insert);
    let mut promoted = active_history
        .iter()
        .filter(|item| !is_remote_compaction_item(item))
        .cloned()
        .collect::<Vec<_>>();
    promoted.insert(
        insert_index,
        projected_recovery_message(recovered_text, projection),
    );
    Ok(promoted)
}

pub(crate) fn resolve_remote_compaction_recovery_model(
    configured_model: &ModelOffloadCompactionRecoveryModel,
    primary_model: &str,
    producing_model: Option<&str>,
) -> String {
    match configured_model {
        ModelOffloadCompactionRecoveryModel::Auto => match producing_model {
            Some(model) => model.to_string(),
            None => {
                debug!(
                    primary_model,
                    "model_offload.compaction.recovery.model=auto could not resolve remote compaction producing model; falling back to primary model"
                );
                primary_model.to_string()
            }
        },
        ModelOffloadCompactionRecoveryModel::Primary => primary_model.to_string(),
        ModelOffloadCompactionRecoveryModel::Explicit(model) => model.clone(),
    }
}

pub(crate) async fn recover_remote_compaction_payload(
    sess: &Arc<Session>,
    turn_context: &Arc<TurnContext>,
    client_session: &mut ModelClientSession,
    active_history: &[ResponseItem],
    producing_model: Option<&str>,
) -> CodexResult<String> {
    let mut prompt = build_remote_compaction_recovery_prompt(active_history)?;
    prompt.base_instructions = sess.get_base_instructions().await;

    let recovery_model = resolve_remote_compaction_recovery_model(
        &turn_context.config.model_offload.compaction_recovery.model,
        turn_context.model_info().slug.as_str(),
        producing_model,
    );
    let recovery_turn_context = if recovery_model == turn_context.model_info().slug {
        Arc::clone(turn_context)
    } else {
        Arc::new(
            turn_context
                .with_model(recovery_model, &sess.services.models_manager)
                .await,
        )
    };

    let window_id = sess.current_window_id().await;
    let responses_metadata = recovery_turn_context
        .turn_metadata_state
        .to_responses_metadata(
            sess.installation_id.clone(),
            window_id,
            CodexResponsesRequestKind::CompactionRecovery,
        );
    let mut stream = client_session
        .stream(
            &prompt,
            recovery_turn_context.model_info().as_ref(),
            &recovery_turn_context.session_telemetry,
            Some(
                recovery_turn_context
                    .config
                    .model_offload
                    .compaction_recovery
                    .reasoning_effort
                    .clone(),
            ),
            recovery_turn_context.reasoning_summary(),
            recovery_turn_context.config.service_tier.clone(),
            &responses_metadata,
            &InferenceTraceContext::disabled(),
        )
        .await?;

    let output_byte_limit = remote_compaction_recovery_output_byte_limit(active_history);
    collect_recovered_text(&mut stream, output_byte_limit).await
}

pub(crate) fn is_remote_compaction_item(item: &ResponseItem) -> bool {
    matches!(
        item,
        ResponseItem::Compaction { .. }
            | ResponseItem::ContextCompaction {
                encrypted_content: Some(_),
                ..
            }
    )
}

pub(crate) fn suffix_most_remote_compaction_item(
    active_history: &[ResponseItem],
) -> Option<&ResponseItem> {
    active_history
        .iter()
        .rev()
        .find(|item| is_remote_compaction_item(item))
}

fn projected_recovery_message(
    recovered_text: String,
    projection: ModelOffloadCompactionRecoveryProjection,
) -> ResponseItem {
    match projection {
        ModelOffloadCompactionRecoveryProjection::AssistantState => ResponseItem::Message {
            id: None,
            role: "assistant".to_string(),
            content: vec![ContentItem::OutputText {
                text: recovered_text,
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        },
        ModelOffloadCompactionRecoveryProjection::UserHandoff => {
            let text = format!(
                "Hydex recovered remote compaction state for local continuation:\n\n{recovered_text}"
            );
            user_message(&text)
        }
    }
}

fn user_message(text: &str) -> ResponseItem {
    ResponseItem::Message {
        id: None,
        role: "user".to_string(),
        content: vec![ContentItem::InputText {
            text: text.to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    }
}

fn remote_compaction_recovery_output_byte_limit(active_history: &[ResponseItem]) -> usize {
    let encoded_len = suffix_most_remote_compaction_item(active_history)
        .and_then(|item| match item {
            ResponseItem::Compaction {
                encrypted_content, ..
            } => Some(encrypted_content.len()),
            ResponseItem::ContextCompaction {
                encrypted_content: Some(encrypted_content),
                ..
            } => Some(encrypted_content.len()),
            _ => None,
        })
        .unwrap_or(0);
    recovery_output_byte_limit_for_encoded_len(encoded_len)
}

fn recovery_output_byte_limit_for_encoded_len(encoded_len: usize) -> usize {
    let estimated_blob_bytes = encoded_len.saturating_mul(3).checked_div(4).unwrap_or(0);
    estimated_blob_bytes
        .saturating_mul(RECOVERY_OUTPUT_EXPANSION_FACTOR)
        .saturating_add(RECOVERY_OUTPUT_FIXED_OVERHEAD_BYTES)
        .clamp(RECOVERY_OUTPUT_MIN_BYTES, RECOVERY_OUTPUT_MAX_BYTES)
}

async fn collect_recovered_text(
    stream: &mut crate::ResponseStream,
    output_byte_limit: usize,
) -> CodexResult<String> {
    let mut output_items_text = String::new();
    let mut output_delta_text = String::new();
    loop {
        let Some(event) = stream.next().await else {
            return Err(CodexErr::Stream(
                "remote compaction recovery stream closed before response.completed".into(),
            ));
        };
        match event {
            Ok(ResponseEvent::OutputItemDone(ResponseItem::Message { role, content, .. }))
                if role == "assistant" =>
            {
                // The completed item supersedes streamed deltas. Release our duplicate buffer
                // before assembling it so Hydex retains at most one bounded recovery copy.
                output_delta_text.clear();
                let mut text = String::new();
                for content_text in content.iter().filter_map(content_item_text) {
                    if !text.is_empty() {
                        append_recovered_text(&mut text, "\n", output_byte_limit)?;
                    }
                    append_recovered_text(&mut text, content_text, output_byte_limit)?;
                }
                if !text.trim().is_empty() {
                    if !output_items_text.is_empty() {
                        append_recovered_text(&mut output_items_text, "\n", output_byte_limit)?;
                    }
                    append_recovered_text(
                        &mut output_items_text,
                        text.as_str(),
                        output_byte_limit,
                    )?;
                }
            }
            Ok(ResponseEvent::OutputTextDelta(delta)) => {
                if output_items_text.is_empty() {
                    append_recovered_text(
                        &mut output_delta_text,
                        delta.as_str(),
                        output_byte_limit,
                    )?;
                }
            }
            Ok(ResponseEvent::Completed { .. }) => {
                let recovered = if output_items_text.is_empty() {
                    output_delta_text
                } else {
                    output_items_text
                };
                if recovered.trim().is_empty() {
                    return Err(CodexErr::Stream(
                        "remote compaction recovery completed without assistant text".into(),
                    ));
                }
                return Ok(recovered);
            }
            Ok(_) => {}
            Err(err) => return Err(err),
        }
    }
}

fn append_recovered_text(
    output: &mut String,
    text: &str,
    output_byte_limit: usize,
) -> CodexResult<()> {
    if output.len().saturating_add(text.len()) > output_byte_limit {
        return Err(CodexErr::Stream(format!(
            "remote compaction recovery output exceeded safety limit of {output_byte_limit} bytes"
        )));
    }
    output.push_str(text);
    Ok(())
}

fn content_item_text(content: &ContentItem) -> Option<&str> {
    match content {
        ContentItem::InputText { text } | ContentItem::OutputText { text } => Some(text.as_str()),
        ContentItem::InputImage { .. } | ContentItem::InputAudio { .. } => None,
    }
}

#[cfg(test)]
#[path = "compaction_recovery_tests.rs"]
mod tests;
