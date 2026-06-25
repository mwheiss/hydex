use super::*;
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
