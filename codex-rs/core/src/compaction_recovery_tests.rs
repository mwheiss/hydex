use super::*;
use codex_api::ResponsesApiRequest;
use codex_config::config_toml::ModelOffloadCompactionRecoveryProjection;
use codex_protocol::error::Result as CodexResult;
use pretty_assertions::assert_eq;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

fn user_text(text: &str) -> ResponseItem {
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

fn assistant_text(text: &str) -> ResponseItem {
    ResponseItem::Message {
        id: None,
        role: "assistant".to_string(),
        content: vec![ContentItem::OutputText {
            text: text.to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    }
}

fn response_stream(events: Vec<CodexResult<ResponseEvent>>) -> crate::ResponseStream {
    let (tx_event, rx_event) = mpsc::channel(events.len().max(1));
    for event in events {
        tx_event
            .try_send(event)
            .expect("response stream test channel should have capacity");
    }
    drop(tx_event);
    crate::ResponseStream {
        rx_event,
        consumer_dropped: CancellationToken::new(),
    }
}

fn completed_event() -> ResponseEvent {
    ResponseEvent::Completed {
        response_id: "response-id".to_string(),
        token_usage: None,
        usage_metadata: None,
        end_turn: Some(true),
    }
}

#[test]
fn recovery_prompt_keeps_only_suffix_most_encrypted_compaction_and_strips_cleartext_history() {
    let history = vec![
        user_text("old cleartext user message"),
        ResponseItem::Compaction {
            id: None,
            encrypted_content: "encrypted-v2-state".to_string(),
            internal_chat_message_metadata_passthrough: None,
        },
        assistant_text("old cleartext assistant message"),
        ResponseItem::ContextCompaction {
            id: None,
            encrypted_content: Some("encrypted-v1-state".to_string()),
            internal_chat_message_metadata_passthrough: None,
        },
    ];

    let prompt = build_remote_compaction_recovery_prompt(&history).expect("recovery prompt");

    assert_eq!(
        prompt.input,
        vec![
            ResponseItem::ContextCompaction {
                id: None,
                encrypted_content: Some("encrypted-v1-state".to_string()),
                internal_chat_message_metadata_passthrough: None,
            },
            user_message(REMOTE_COMPACTION_RECOVERY_SCAFFOLD),
            user_message(REMOTE_COMPACTION_RECOVERY_PROMPT),
        ]
    );
}

#[test]
fn recovery_output_limit_uses_only_suffix_most_encrypted_compaction() {
    let history = vec![
        ResponseItem::Compaction {
            id: None,
            encrypted_content: "x".repeat(1024 * 1024),
            internal_chat_message_metadata_passthrough: None,
        },
        ResponseItem::ContextCompaction {
            id: None,
            encrypted_content: Some("new".to_string()),
            internal_chat_message_metadata_passthrough: None,
        },
    ];

    assert_eq!(
        remote_compaction_recovery_output_byte_limit(&history),
        RECOVERY_OUTPUT_MIN_BYTES
    );
}

#[test]
fn recovery_prompt_rejects_history_without_encrypted_compaction() {
    let err = build_remote_compaction_recovery_prompt(&[user_text("plain")])
        .expect_err("missing encrypted compaction should fail");

    assert!(
        err.to_string()
            .contains("no encrypted compaction item is active"),
        "unexpected error: {err}"
    );
}

#[test]
fn recovery_output_limit_has_generous_floor_and_absolute_ceiling() {
    assert_eq!(
        recovery_output_byte_limit_for_encoded_len(0),
        RECOVERY_OUTPUT_MIN_BYTES
    );
    assert_eq!(
        recovery_output_byte_limit_for_encoded_len(1024 * 1024),
        25 * 1024 * 1024
    );
    assert_eq!(
        recovery_output_byte_limit_for_encoded_len(usize::MAX),
        RECOVERY_OUTPUT_MAX_BYTES
    );
}

#[tokio::test]
async fn recovery_output_accepts_exact_byte_limit() {
    let mut stream = response_stream(vec![
        Ok(ResponseEvent::OutputTextDelta("abcd".to_string())),
        Ok(completed_event()),
    ]);

    assert_eq!(
        collect_recovered_text(&mut stream, 4)
            .await
            .expect("recovery at byte limit should succeed"),
        "abcd"
    );
}

#[tokio::test]
async fn recovery_output_rejects_delta_beyond_byte_limit() {
    let mut stream = response_stream(vec![
        Ok(ResponseEvent::OutputTextDelta("abcde".to_string())),
        Ok(completed_event()),
    ]);

    let error = collect_recovered_text(&mut stream, 4)
        .await
        .expect_err("oversized recovery delta should fail");

    assert!(
        error
            .to_string()
            .contains("exceeded safety limit of 4 bytes"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn completed_output_item_replaces_redundant_deltas_within_limit() {
    let mut stream = response_stream(vec![
        Ok(ResponseEvent::OutputTextDelta("abcd".to_string())),
        Ok(ResponseEvent::OutputItemDone(assistant_text("abcd"))),
        Ok(completed_event()),
    ]);

    assert_eq!(
        collect_recovered_text(&mut stream, 4)
            .await
            .expect("completed item should replace duplicate deltas"),
        "abcd"
    );
}

#[tokio::test]
async fn recovery_output_rejects_completed_item_beyond_byte_limit() {
    let mut stream = response_stream(vec![
        Ok(ResponseEvent::OutputItemDone(assistant_text("abcde"))),
        Ok(completed_event()),
    ]);

    let error = collect_recovered_text(&mut stream, 4)
        .await
        .expect_err("oversized completed recovery item should fail");

    assert!(
        error
            .to_string()
            .contains("exceeded safety limit of 4 bytes"),
        "unexpected error: {error}"
    );
}

#[test]
fn recovery_model_auto_uses_producing_model() {
    assert_eq!(
        resolve_remote_compaction_recovery_model(
            &ModelOffloadCompactionRecoveryModel::Auto,
            "gpt-primary",
            Some("gpt-producing"),
        ),
        "gpt-producing"
    );
}

#[test]
fn recovery_model_auto_falls_back_to_primary_without_provenance() {
    assert_eq!(
        resolve_remote_compaction_recovery_model(
            &ModelOffloadCompactionRecoveryModel::Auto,
            "gpt-primary",
            None,
        ),
        "gpt-primary"
    );
}

#[test]
fn recovery_model_primary_uses_current_primary_model() {
    assert_eq!(
        resolve_remote_compaction_recovery_model(
            &ModelOffloadCompactionRecoveryModel::Primary,
            "gpt-primary",
            Some("gpt-producing"),
        ),
        "gpt-primary"
    );
}

#[test]
fn recovery_model_explicit_uses_configured_model() {
    assert_eq!(
        resolve_remote_compaction_recovery_model(
            &ModelOffloadCompactionRecoveryModel::Explicit("gpt-explicit".to_string()),
            "gpt-primary",
            Some("gpt-producing"),
        ),
        "gpt-explicit"
    );
}

#[test]
fn assistant_state_projection_replaces_encrypted_compaction_with_assistant_message() {
    let history = vec![
        user_text("retained user"),
        ResponseItem::Compaction {
            id: None,
            encrypted_content: "encrypted".to_string(),
            internal_chat_message_metadata_passthrough: None,
        },
        user_text("next user"),
    ];

    let projected = project_recovered_remote_compaction(
        &history,
        "recovered state".to_string(),
        ModelOffloadCompactionRecoveryProjection::AssistantState,
    )
    .expect("projected history");

    assert_eq!(
        projected,
        vec![
            user_text("retained user"),
            assistant_text("recovered state"),
            user_text("next user"),
        ]
    );
}

#[test]
fn user_handoff_projection_replaces_encrypted_compaction_with_user_message() {
    let history = vec![ResponseItem::ContextCompaction {
        id: None,
        encrypted_content: Some("encrypted".to_string()),
        internal_chat_message_metadata_passthrough: None,
    }];

    let projected = project_recovered_remote_compaction(
        &history,
        "recovered state".to_string(),
        ModelOffloadCompactionRecoveryProjection::UserHandoff,
    )
    .expect("projected history");

    assert_eq!(
        projected,
        vec![user_text(
            "Hydex recovered remote compaction state for local continuation:\n\nrecovered state"
        )]
    );
}

#[test]
fn projection_drops_older_malformed_duplicate_encrypted_compactions() {
    let history = vec![
        ResponseItem::Compaction {
            id: None,
            encrypted_content: "old".to_string(),
            internal_chat_message_metadata_passthrough: None,
        },
        user_text("retained user"),
        ResponseItem::Compaction {
            id: None,
            encrypted_content: "new".to_string(),
            internal_chat_message_metadata_passthrough: None,
        },
    ];

    let projected = project_recovered_remote_compaction(
        &history,
        "recovered state".to_string(),
        ModelOffloadCompactionRecoveryProjection::AssistantState,
    )
    .expect("projected history");

    assert_eq!(
        projected,
        vec![
            user_text("retained user"),
            assistant_text("recovered state"),
        ]
    );
    assert!(!active_history_has_remote_compaction(&projected));
}

#[test]
fn primary_route_does_not_need_remote_compaction_recovery() {
    let history = vec![ResponseItem::Compaction {
        id: None,
        encrypted_content: "encrypted".to_string(),
        internal_chat_message_metadata_passthrough: None,
    }];

    assert!(!remote_compaction_recovery_needed(false, &history));
}

#[test]
fn local_route_with_encrypted_compaction_needs_remote_compaction_recovery() {
    let history = vec![ResponseItem::ContextCompaction {
        id: None,
        encrypted_content: Some("encrypted".to_string()),
        internal_chat_message_metadata_passthrough: None,
    }];

    assert!(remote_compaction_recovery_needed(true, &history));
}

#[test]
fn local_route_without_encrypted_compaction_does_not_need_remote_compaction_recovery() {
    let history = vec![user_text("ordinary history")];

    assert!(!remote_compaction_recovery_needed(true, &history));
}

#[test]
fn local_route_recovers_new_remote_compaction_after_reentry_compaction() {
    let initial_remote_history = vec![ResponseItem::Compaction {
        id: None,
        encrypted_content: "old encrypted remote state".to_string(),
        internal_chat_message_metadata_passthrough: None,
    }];
    let promoted_history = project_recovered_remote_compaction(
        &initial_remote_history,
        "old recovered state".to_string(),
        ModelOffloadCompactionRecoveryProjection::AssistantState,
    )
    .expect("initial projection");
    let reentry_remote_history = vec![
        promoted_history[0].clone(),
        ResponseItem::Compaction {
            id: None,
            encrypted_content: "new encrypted remote state".to_string(),
            internal_chat_message_metadata_passthrough: None,
        },
    ];

    assert!(remote_compaction_recovery_needed(
        true,
        &reentry_remote_history
    ));

    let final_history = project_recovered_remote_compaction(
        &reentry_remote_history,
        "new recovered state".to_string(),
        ModelOffloadCompactionRecoveryProjection::AssistantState,
    )
    .expect("second projection");

    assert_eq!(
        final_history,
        vec![
            assistant_text("old recovered state"),
            assistant_text("new recovered state"),
        ]
    );
    assert!(!remote_compaction_recovery_needed(true, &final_history));
}

#[test]
fn assistant_state_projection_reaches_local_wire_as_assistant_history() {
    let projected = project_recovered_remote_compaction(
        &[
            ResponseItem::Compaction {
                id: None,
                encrypted_content: "encrypted".to_string(),
                internal_chat_message_metadata_passthrough: None,
            },
            user_text("next user"),
        ],
        "recovered compacted state".to_string(),
        ModelOffloadCompactionRecoveryProjection::AssistantState,
    )
    .expect("projected history");
    let mut request = ResponsesApiRequest {
        model: "local-model".to_string(),
        instructions: String::new(),
        input: projected,
        tools: None,
        tool_choice: "auto".to_string(),
        parallel_tool_calls: false,
        temperature: None,
        reasoning: None,
        store: false,
        stream: true,
        stream_options: None,
        include: Vec::new(),
        service_tier: None,
        prompt_cache_key: None,
        text: None,
        client_metadata: None,
        access_programs: None,
    };

    crate::local_offload::transform_request_for_local_offload(&mut request, &[])
        .expect("local request transform");

    assert_eq!(
        request.input,
        vec![
            assistant_text("recovered compacted state"),
            user_text("next user"),
        ]
    );
}
