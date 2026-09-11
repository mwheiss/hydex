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
fn cache_key_uses_only_suffix_most_remote_compaction_item() {
    let active = remote_compaction_item("active encrypted state");
    let first_history = vec![remote_compaction_item("stale-a"), active.clone()];
    let second_history = vec![remote_compaction_item("stale-b"), active];

    assert_eq!(
        remote_compaction_recovery_cache_key(&first_history, "gpt-5.4", "none").expect("first key"),
        remote_compaction_recovery_cache_key(&second_history, "gpt-5.4", "none")
            .expect("second key")
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

#[test]
fn cache_evicts_oldest_entry_at_entry_limit() {
    let mut cache = RemoteCompactionRecoveryCache::with_limits(2, usize::MAX);
    let first =
        remote_compaction_recovery_cache_key(&[remote_compaction_item("first")], "gpt-5.4", "none")
            .expect("first key");
    let second = remote_compaction_recovery_cache_key(
        &[remote_compaction_item("second")],
        "gpt-5.4",
        "none",
    )
    .expect("second key");
    let third =
        remote_compaction_recovery_cache_key(&[remote_compaction_item("third")], "gpt-5.4", "none")
            .expect("third key");

    cache.insert(
        first.clone(),
        remote_compaction_recovery_cache_entry("one".to_string(), 1),
    );
    cache.insert(
        second.clone(),
        remote_compaction_recovery_cache_entry("two".to_string(), 1),
    );
    cache.insert(
        third.clone(),
        remote_compaction_recovery_cache_entry("three".to_string(), 1),
    );

    assert_eq!(cache.get(&first), None);
    assert!(cache.get(&second).is_some());
    assert!(cache.get(&third).is_some());
}

#[test]
fn cache_evicts_oldest_entries_to_stay_within_byte_limit() {
    let mut cache = RemoteCompactionRecoveryCache::with_limits(4, 8);
    let first =
        remote_compaction_recovery_cache_key(&[remote_compaction_item("first")], "gpt-5.4", "none")
            .expect("first key");
    let second = remote_compaction_recovery_cache_key(
        &[remote_compaction_item("second")],
        "gpt-5.4",
        "none",
    )
    .expect("second key");
    let third =
        remote_compaction_recovery_cache_key(&[remote_compaction_item("third")], "gpt-5.4", "none")
            .expect("third key");

    cache.insert(
        first.clone(),
        remote_compaction_recovery_cache_entry("1234".to_string(), 1),
    );
    cache.insert(
        second.clone(),
        remote_compaction_recovery_cache_entry("5678".to_string(), 1),
    );
    cache.insert(
        third.clone(),
        remote_compaction_recovery_cache_entry("abcde".to_string(), 1),
    );

    assert_eq!(cache.get(&first), None);
    assert_eq!(cache.get(&second), None);
    assert!(cache.get(&third).is_some());
}
