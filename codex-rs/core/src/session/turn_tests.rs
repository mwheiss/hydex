use super::*;
use crate::config::ModelOffloadContextConfig;
use codex_extension_api::ExtensionData;
use codex_extension_api::TurnItemContributor;
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
        id: Some("msg-1".to_string()),
        role: "assistant".to_string(),
        content: vec![ContentItem::OutputText {
            text: text.to_string(),
        }],
        phase: None,
        metadata: None,
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
fn offload_context_window_200000_derives_auto_compact_thresholds() {
    let model_info = test_model_info_with_context_window(Some(128_000));
    let local_context = ModelOffloadContextConfig {
        context_window: Some(200_000),
        ..Default::default()
    };

    let thresholds = select_auto_compact_thresholds(
        &model_info,
        &local_context,
        /*use_local_thresholds*/ true,
    );

    assert_eq!(thresholds.auto_compact_token_limit, Some(180_000));
    assert_eq!(thresholds.effective_context_window, Some(190_000));
}

#[test]
fn offload_explicit_auto_compact_limit_is_clamped_to_local_90_percent() {
    let model_info = test_model_info_with_context_window(Some(128_000));
    let local_context = ModelOffloadContextConfig {
        context_window: Some(200_000),
        auto_compact_token_limit: Some(250_000),
        ..Default::default()
    };

    let thresholds = select_auto_compact_thresholds(
        &model_info,
        &local_context,
        /*use_local_thresholds*/ true,
    );

    assert_eq!(thresholds.auto_compact_token_limit, Some(180_000));
    assert_eq!(thresholds.effective_context_window, Some(190_000));
}

#[test]
fn offload_threshold_selector_preserves_no_offload_model_behavior() {
    let model_info = test_model_info_with_context_window(Some(128_000));
    let local_context = ModelOffloadContextConfig {
        context_window: Some(200_000),
        ..Default::default()
    };

    let thresholds = select_auto_compact_thresholds(
        &model_info,
        &local_context,
        /*use_local_thresholds*/ false,
    );

    assert_eq!(thresholds.auto_compact_token_limit, Some(115_200));
    assert_eq!(thresholds.effective_context_window, Some(121_600));
}

#[test]
fn offload_threshold_selector_does_not_require_global_model_context_window() {
    let model_info = test_model_info_with_context_window(None);
    let local_context = ModelOffloadContextConfig {
        context_window: Some(200_000),
        ..Default::default()
    };

    let thresholds = select_auto_compact_thresholds(
        &model_info,
        &local_context,
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
