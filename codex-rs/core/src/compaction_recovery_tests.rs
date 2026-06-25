use super::*;
use codex_config::config_toml::ModelOffloadCompactionRecoveryProjection;
use pretty_assertions::assert_eq;

fn user_text(text: &str) -> ResponseItem {
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

fn assistant_text(text: &str) -> ResponseItem {
    ResponseItem::Message {
        id: None,
        role: "assistant".to_string(),
        content: vec![ContentItem::OutputText {
            text: text.to_string(),
        }],
        phase: None,
        metadata: None,
    }
}

#[test]
fn recovery_prompt_keeps_encrypted_compaction_and_strips_cleartext_history() {
    let history = vec![
        user_text("old cleartext user message"),
        ResponseItem::Compaction {
            encrypted_content: "encrypted-v2-state".to_string(),
            metadata: None,
        },
        assistant_text("old cleartext assistant message"),
        ResponseItem::ContextCompaction {
            encrypted_content: Some("encrypted-v1-state".to_string()),
            metadata: None,
        },
    ];

    let prompt = build_remote_compaction_recovery_prompt(&history).expect("recovery prompt");

    assert_eq!(
        prompt.input,
        vec![
            ResponseItem::Compaction {
                encrypted_content: "encrypted-v2-state".to_string(),
                metadata: None,
            },
            ResponseItem::ContextCompaction {
                encrypted_content: Some("encrypted-v1-state".to_string()),
                metadata: None,
            },
            user_message(REMOTE_COMPACTION_RECOVERY_SCAFFOLD),
            user_message(REMOTE_COMPACTION_RECOVERY_PROMPT),
        ]
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
            encrypted_content: "encrypted".to_string(),
            metadata: None,
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
        encrypted_content: Some("encrypted".to_string()),
        metadata: None,
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
            encrypted_content: "old".to_string(),
            metadata: None,
        },
        user_text("retained user"),
        ResponseItem::Compaction {
            encrypted_content: "new".to_string(),
            metadata: None,
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
        encrypted_content: "encrypted".to_string(),
        metadata: None,
    }];

    assert!(!remote_compaction_recovery_needed(false, &history));
}

#[test]
fn local_route_with_encrypted_compaction_needs_remote_compaction_recovery() {
    let history = vec![ResponseItem::ContextCompaction {
        encrypted_content: Some("encrypted".to_string()),
        metadata: None,
    }];

    assert!(remote_compaction_recovery_needed(true, &history));
}

#[test]
fn local_route_without_encrypted_compaction_does_not_need_remote_compaction_recovery() {
    let history = vec![user_text("ordinary history")];

    assert!(!remote_compaction_recovery_needed(true, &history));
}
