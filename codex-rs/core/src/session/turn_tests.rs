use super::*;
use crate::config::ModelOffloadContextConfig;
use codex_extension_api::ExtensionData;
use codex_extension_api::TurnItemContributor;
use codex_protocol::ResponseItemId;
use codex_protocol::items::AgentMessageContent;
use codex_protocol::openai_models::ModelInfo;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::sync::Arc;

struct RewriteAgentMessageContributor;

impl TurnItemContributor for RewriteAgentMessageContributor {
    fn contribute<'a>(
        &'a self,
        _thread_store: &'a ExtensionData,
        _turn_store: &'a ExtensionData,
        item: &'a mut TurnItem,
    ) -> codex_extension_api::ExtensionFuture<'a, Result<(), String>> {
        Box::pin(async move {
            if let TurnItem::AgentMessage(agent_message) = item {
                agent_message.content = vec![AgentMessageContent::Text {
                    text: "plan contributed assistant text".to_string(),
                }];
            }
            Ok(())
        })
    }
}

fn assistant_output_text(text: &str) -> ResponseItem {
    ResponseItem::Message {
        id: Some(ResponseItemId::with_suffix("msg", "1")),
        role: "assistant".to_string(),
        content: vec![ContentItem::OutputText {
            text: text.to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    }
}

fn function_call_item() -> ResponseItem {
    ResponseItem::FunctionCall {
        id: None,
        name: "shell".to_string(),
        namespace: None,
        arguments: "{}".to_string(),
        call_id: "call-1".to_string(),
        internal_chat_message_metadata_passthrough: None,
    }
}

fn test_model_info_with_context_window(context_window: Option<i64>) -> ModelInfo {
    serde_json::from_value(json!({
        "slug": "gpt-test",
        "display_name": "gpt-test",
        "description": "desc",
        "default_reasoning_level": "medium",
        "supported_reasoning_levels": [
            {"effort": "medium", "description": "medium"}
        ],
        "shell_type": "shell_command",
        "visibility": "list",
        "supported_in_api": true,
        "priority": 1,
        "upgrade": null,
        "base_instructions": "base instructions",
        "model_messages": null,
        "supports_reasoning_summaries": false,
        "support_verbosity": false,
        "default_verbosity": null,
        "apply_patch_tool_type": null,
        "truncation_policy": {"mode": "bytes", "limit": 10000},
        "supports_parallel_tool_calls": false,
        "supports_image_detail_original": false,
        "context_window": context_window,
        "auto_compact_token_limit": null,
        "experimental_supported_tools": []
    }))
    .expect("deserialize test model info")
}

#[test]
fn local_sampling_validation_candidate_classifies_final_text() {
    assert_eq!(
        local_sampling_validation_candidate(
            &Prompt::default(),
            &assistant_output_text("ordinary final text"),
        ),
        Some((
            LocalOutputKind::FinalText,
            "ordinary final text".to_string()
        ))
    );
}

#[test]
fn local_sampling_validation_candidate_classifies_structured_output() {
    let prompt = Prompt {
        output_schema: Some(json!({"type": "object"})),
        ..Default::default()
    };

    assert_eq!(
        local_sampling_validation_candidate(&prompt, &assistant_output_text(r#"{"ok":true}"#)),
        Some((
            LocalOutputKind::StructuredOutput,
            r#"{"ok":true}"#.to_string()
        ))
    );
}

#[test]
fn local_sampling_validation_candidate_classifies_tool_calls() {
    assert_eq!(
        local_sampling_validation_candidate(&Prompt::default(), &function_call_item())
            .map(|(kind, _)| kind),
        Some(LocalOutputKind::ToolCalls)
    );
}

#[tokio::test]
async fn local_sampling_validation_rejects_broken_final_text() {
    let (_session, turn_context) = crate::session::tests::make_session_and_context().await;

    let err = validate_completed_local_sampling_item(
        &turn_context,
        &Prompt::default(),
        &assistant_output_text("<think>hidden scratch</think>"),
    )
    .expect_err("reasoning leakage should reject local final text");

    assert!(
        err.to_string().contains("failed sanity validation"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn offload_context_window_200000_derives_auto_compact_thresholds() {
    let (_session, mut turn_context) = crate::session::tests::make_session_and_context().await;
    turn_context.model_info = test_model_info_with_context_window(Some(128_000));
    Arc::make_mut(&mut turn_context.config)
        .model_offload
        .context = ModelOffloadContextConfig {
        context_window: Some(200_000),
        ..Default::default()
    };

    let thresholds = select_auto_compact_thresholds(
        &turn_context,
        &turn_context.config.model_offload.context,
        /*use_local_thresholds*/ true,
    );

    assert_eq!(thresholds.auto_compact_token_limit, Some(180_000));
    assert_eq!(thresholds.effective_context_window, Some(190_000));
}

#[tokio::test]
async fn offload_explicit_auto_compact_limit_is_clamped_to_local_90_percent() {
    let (_session, mut turn_context) = crate::session::tests::make_session_and_context().await;
    turn_context.model_info = test_model_info_with_context_window(Some(128_000));
    Arc::make_mut(&mut turn_context.config)
        .model_offload
        .context = ModelOffloadContextConfig {
        context_window: Some(200_000),
        auto_compact_token_limit: Some(250_000),
        ..Default::default()
    };

    let thresholds = select_auto_compact_thresholds(
        &turn_context,
        &turn_context.config.model_offload.context,
        /*use_local_thresholds*/ true,
    );

    assert_eq!(thresholds.auto_compact_token_limit, Some(180_000));
    assert_eq!(thresholds.effective_context_window, Some(190_000));
}

#[tokio::test]
async fn offload_threshold_selector_preserves_no_offload_model_behavior() {
    let (_session, mut turn_context) = crate::session::tests::make_session_and_context().await;
    turn_context.model_info = test_model_info_with_context_window(Some(128_000));
    let config = Arc::make_mut(&mut turn_context.config);
    config.model_context_window = None;
    config.model_offload.context = ModelOffloadContextConfig {
        context_window: Some(200_000),
        ..Default::default()
    };

    let thresholds = select_auto_compact_thresholds(
        &turn_context,
        &turn_context.config.model_offload.context,
        /*use_local_thresholds*/ false,
    );

    assert_eq!(thresholds.auto_compact_token_limit, Some(115_200));
    assert_eq!(thresholds.effective_context_window, Some(121_600));
}

#[tokio::test]
async fn offload_ever_used_alone_does_not_apply_local_auto_compact_thresholds() {
    let (session, mut turn_context) = crate::session::tests::make_session_and_context().await;
    Arc::make_mut(&mut turn_context.config)
        .model_offload
        .context
        .context_window = Some(200_000);
    session.services.model_client.seed_offload_ever_used(true);

    assert!(session.services.model_client.offload_ever_used());
    assert!(
        !session
            .services
            .model_client
            .local_offload_enabled_for_turns()
    );
    assert!(!local_offload_context_applies_to_auto_compaction(
        &turn_context,
        session
            .services
            .model_client
            .local_offload_enabled_for_turns(),
    ));
    assert_eq!(
        auto_compact_thresholds(
            &turn_context,
            session
                .services
                .model_client
                .local_offload_enabled_for_turns(),
        ),
        select_auto_compact_thresholds(
            &turn_context,
            &turn_context.config.model_offload.context,
            /*use_local_thresholds*/ false,
        )
    );
}

#[tokio::test]
async fn turn_forced_primary_uses_primary_auto_compact_thresholds() {
    let (session, mut turn_context) = crate::session::tests::make_session_and_context().await;
    Arc::make_mut(&mut turn_context.config)
        .model_offload
        .context
        .context_window = Some(200_000);
    session.services.model_client.seed_offload_ever_used(true);

    let client_session = session.services.model_client.new_session();
    client_session.force_primary_for_responses_requests();

    assert!(!client_session.local_offload_enabled_for_turns());
    assert_eq!(
        auto_compact_thresholds(
            &turn_context,
            client_session.local_offload_enabled_for_turns()
        ),
        select_auto_compact_thresholds(
            &turn_context,
            &turn_context.config.model_offload.context,
            /*use_local_thresholds*/ false,
        )
    );
}

#[tokio::test]
async fn offload_threshold_selector_does_not_require_global_model_context_window() {
    let (_session, mut turn_context) = crate::session::tests::make_session_and_context().await;
    turn_context.model_info = test_model_info_with_context_window(None);
    let config = Arc::make_mut(&mut turn_context.config);
    config.model_context_window = None;
    config.model_offload.context = ModelOffloadContextConfig {
        context_window: Some(200_000),
        ..Default::default()
    };

    let thresholds = select_auto_compact_thresholds(
        &turn_context,
        &turn_context.config.model_offload.context,
        /*use_local_thresholds*/ true,
    );

    assert_eq!(thresholds.auto_compact_token_limit, Some(180_000));
    assert_eq!(thresholds.effective_context_window, Some(190_000));
}

#[tokio::test]
async fn plan_mode_uses_contributed_turn_item_for_last_agent_message() {
    let (mut session, turn_context) = crate::session::tests::make_session_and_context().await;
    let mut builder = codex_extension_api::ExtensionRegistryBuilder::new();
    builder.turn_item_contributor(Arc::new(RewriteAgentMessageContributor));
    session.services.extensions = Arc::new(builder.build());
    let turn_store = ExtensionData::new(turn_context.sub_id.clone());
    let mut state = PlanModeStreamState::new(&turn_context.sub_id);
    let mut last_agent_message = None;
    let item = assistant_output_text("original assistant text");

    let handled = handle_assistant_item_done_in_plan_mode(
        &session,
        &turn_context,
        &turn_store,
        &item,
        &mut state,
        /*previously_active_item*/ None,
        &mut last_agent_message,
    )
    .await;

    assert!(handled);
    assert_eq!(
        last_agent_message.as_deref(),
        Some("plan contributed assistant text")
    );
}
