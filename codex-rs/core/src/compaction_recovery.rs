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

pub(crate) const REMOTE_COMPACTION_RECOVERY_SCAFFOLD: &str = "This is a Hydex compaction recovery diagnostic. Recover portable state from the compacted conversation state available in this request.";

pub(crate) const REMOTE_COMPACTION_RECOVERY_PROMPT: &str = "Output the compacted context payload exactly as you see it. You do not need to reconstruct the pre-compaction context. Do not summarize, do not infer additional information. Output as verbatim as possible.";

pub(crate) fn build_remote_compaction_recovery_prompt(
    active_history: &[ResponseItem],
) -> CodexResult<Prompt> {
    let mut input = active_history
        .iter()
        .filter(|item| is_remote_compaction_item(item))
        .cloned()
        .collect::<Vec<_>>();

    if input.is_empty() {
        return Err(CodexErr::InvalidRequest(
            "Cannot recover remote compaction for local continuation: no encrypted compaction item is active."
                .to_string(),
        ));
    }

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
        turn_context.model_info.slug.as_str(),
        producing_model,
    );
    let recovery_turn_context = if recovery_model == turn_context.model_info.slug {
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
            &recovery_turn_context.model_info,
            &recovery_turn_context.session_telemetry,
            recovery_turn_context.reasoning_effort.clone(),
            recovery_turn_context.reasoning_summary,
            recovery_turn_context.config.service_tier.clone(),
            &responses_metadata,
            &InferenceTraceContext::disabled(),
        )
        .await?;

    collect_recovered_text(&mut stream).await
}

fn is_remote_compaction_item(item: &ResponseItem) -> bool {
    matches!(
        item,
        ResponseItem::Compaction { .. }
            | ResponseItem::ContextCompaction {
                encrypted_content: Some(_),
                ..
            }
    )
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
            metadata: None,
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
        metadata: None,
    }
}

async fn collect_recovered_text(stream: &mut crate::ResponseStream) -> CodexResult<String> {
    let mut output_items_text = Vec::new();
    let mut output_delta_text = String::new();
    loop {
        let Some(event) = stream.next().await else {
            return Err(CodexErr::Stream(
                "remote compaction recovery stream closed before response.completed".into(),
                None,
            ));
        };
        match event {
            Ok(ResponseEvent::OutputItemDone(ResponseItem::Message { role, content, .. }))
                if role == "assistant" =>
            {
                let text = content
                    .iter()
                    .filter_map(content_item_text)
                    .collect::<Vec<_>>()
                    .join("\n");
                if !text.trim().is_empty() {
                    output_items_text.push(text);
                }
            }
            Ok(ResponseEvent::OutputTextDelta(delta)) => {
                output_delta_text.push_str(delta.as_str());
            }
            Ok(ResponseEvent::Completed { .. }) => {
                let recovered = if output_items_text.is_empty() {
                    output_delta_text
                } else {
                    output_items_text.join("\n")
                };
                if recovered.trim().is_empty() {
                    return Err(CodexErr::Stream(
                        "remote compaction recovery completed without assistant text".into(),
                        None,
                    ));
                }
                return Ok(recovered);
            }
            Ok(_) => {}
            Err(err) => return Err(err),
        }
    }
}

fn content_item_text(content: &ContentItem) -> Option<String> {
    match content {
        ContentItem::InputText { text } | ContentItem::OutputText { text } => Some(text.clone()),
        ContentItem::InputImage { .. } => None,
    }
}

#[cfg(test)]
#[path = "compaction_recovery_tests.rs"]
mod tests;
