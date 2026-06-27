use codex_protocol::error::Result as CodexResult;
use codex_protocol::models::ResponseItem;
use codex_utils_cache::sha1_digest;
use serde::Serialize;

pub(crate) const REMOTE_COMPACTION_RECOVERY_PROMPT_VERSION: &str =
    "hydex-remote-compaction-verbatim-simple-v1";
pub(crate) const REMOTE_COMPACTION_RECOVERY_ALGORITHM_VERSION: &str =
    "hydex-remote-compaction-recovery-v1";

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct RemoteCompactionRecoveryCacheKey {
    pub(crate) compacted_state_hash: String,
    pub(crate) prompt_version: String,
    pub(crate) recovery_model: String,
    pub(crate) recovery_reasoning_effort: String,
    pub(crate) algorithm_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemoteCompactionRecoveryCacheEntry {
    pub(crate) recovered_text: String,
    pub(crate) recovered_text_hash: String,
    pub(crate) compaction_item_count: usize,
}

pub(crate) fn remote_compaction_recovery_cache_key(
    active_history: &[ResponseItem],
    recovery_model: &str,
    recovery_reasoning_effort: &str,
) -> CodexResult<RemoteCompactionRecoveryCacheKey> {
    remote_compaction_recovery_cache_key_with_versions(
        active_history,
        recovery_model,
        recovery_reasoning_effort,
        REMOTE_COMPACTION_RECOVERY_PROMPT_VERSION,
        REMOTE_COMPACTION_RECOVERY_ALGORITHM_VERSION,
    )
}

pub(crate) fn remote_compaction_recovery_cache_entry(
    recovered_text: String,
    compaction_item_count: usize,
) -> RemoteCompactionRecoveryCacheEntry {
    RemoteCompactionRecoveryCacheEntry {
        recovered_text_hash: hash_bytes(recovered_text.as_bytes()),
        recovered_text,
        compaction_item_count,
    }
}

pub(crate) fn remote_compaction_item_count(active_history: &[ResponseItem]) -> usize {
    active_history
        .iter()
        .filter(|item| is_remote_compaction_item(item))
        .count()
}

fn remote_compaction_recovery_cache_key_with_versions(
    active_history: &[ResponseItem],
    recovery_model: &str,
    recovery_reasoning_effort: &str,
    prompt_version: &str,
    algorithm_version: &str,
) -> CodexResult<RemoteCompactionRecoveryCacheKey> {
    let remote_compaction_items = active_history
        .iter()
        .filter(|item| is_remote_compaction_item(item))
        .collect::<Vec<_>>();
    let compacted_state_hash = hash_json(&remote_compaction_items)?;
    Ok(RemoteCompactionRecoveryCacheKey {
        compacted_state_hash,
        prompt_version: prompt_version.to_string(),
        recovery_model: recovery_model.to_string(),
        recovery_reasoning_effort: recovery_reasoning_effort.to_string(),
        algorithm_version: algorithm_version.to_string(),
    })
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

fn hash_json(value: &impl Serialize) -> CodexResult<String> {
    Ok(hash_bytes(&serde_json::to_vec(value)?))
}

fn hash_bytes(bytes: &[u8]) -> String {
    sha1_digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}

#[cfg(test)]
#[path = "compaction_recovery_cache_tests.rs"]
mod tests;
