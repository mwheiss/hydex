use super::*;
use codex_protocol::models::ResponseItem;
use pretty_assertions::assert_eq;

fn remote_compaction_item(encrypted_content: &str) -> ResponseItem {
    ResponseItem::Compaction {
        id: None,
        encrypted_content: encrypted_content.to_string(),
        internal_chat_message_metadata_passthrough: None,
    }
}

#[test]
fn cache_key_is_stable_for_same_compacted_state_and_model() {
    let history = vec![remote_compaction_item("encrypted")];

    assert_eq!(
        remote_compaction_recovery_cache_key(&history, "gpt-5.4", "none").expect("first key"),
        remote_compaction_recovery_cache_key(&history, "gpt-5.4", "none").expect("second key")
    );
}

#[test]
fn cache_key_misses_when_recovery_model_changes() {
    let history = vec![remote_compaction_item("encrypted")];

    assert_ne!(
        remote_compaction_recovery_cache_key(&history, "gpt-5.4", "none").expect("first key"),
        remote_compaction_recovery_cache_key(&history, "gpt-5.4-mini", "none").expect("second key")
    );
}

#[test]
fn cache_key_misses_when_recovery_reasoning_effort_changes() {
    let history = vec![remote_compaction_item("encrypted")];

    assert_ne!(
        remote_compaction_recovery_cache_key(&history, "gpt-5.4", "none").expect("first key"),
        remote_compaction_recovery_cache_key(&history, "gpt-5.4", "medium").expect("second key")
    );
}

#[test]
fn cache_key_misses_when_prompt_version_changes() {
    let history = vec![remote_compaction_item("encrypted")];

    assert_ne!(
        remote_compaction_recovery_cache_key_with_versions(
            &history,
            "gpt-5.4",
            "none",
            "prompt-v1",
            REMOTE_COMPACTION_RECOVERY_ALGORITHM_VERSION,
        )
        .expect("first key"),
        remote_compaction_recovery_cache_key_with_versions(
            &history,
            "gpt-5.4",
            "none",
            "prompt-v2",
            REMOTE_COMPACTION_RECOVERY_ALGORITHM_VERSION,
        )
        .expect("second key")
    );
}

#[test]
fn cache_entry_records_recovered_text_hash_and_compaction_count() {
    let entry = remote_compaction_recovery_cache_entry("recovered".to_string(), 1);

    assert_eq!(
        entry,
        RemoteCompactionRecoveryCacheEntry {
            recovered_text: "recovered".to_string(),
            recovered_text_hash: "26cf9476bd022b35c985a12dea4b1fcafba84314".to_string(),
            compaction_item_count: 1,
        }
    );
}
