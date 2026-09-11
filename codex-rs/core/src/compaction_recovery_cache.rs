use crate::compaction_recovery::is_remote_compaction_item;
use crate::compaction_recovery::suffix_most_remote_compaction_item;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result as CodexResult;
use codex_protocol::models::ResponseItem;
use codex_utils_cache::sha1_digest;
use serde::Serialize;
use std::collections::HashMap;
use std::collections::VecDeque;

pub(crate) const REMOTE_COMPACTION_RECOVERY_PROMPT_VERSION: &str =
    "hydex-remote-compaction-verbatim-simple-v2";
pub(crate) const REMOTE_COMPACTION_RECOVERY_ALGORITHM_VERSION: &str =
    "hydex-remote-compaction-recovery-v1";
const REMOTE_COMPACTION_RECOVERY_CACHE_MAX_ENTRIES: usize = 4;
const REMOTE_COMPACTION_RECOVERY_CACHE_MAX_BYTES: usize = 128 * 1024 * 1024;

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

#[derive(Debug)]
pub(crate) struct RemoteCompactionRecoveryCache {
    entries: HashMap<RemoteCompactionRecoveryCacheKey, RemoteCompactionRecoveryCacheEntry>,
    insertion_order: VecDeque<RemoteCompactionRecoveryCacheKey>,
    recovered_text_bytes: usize,
    max_entries: usize,
    max_bytes: usize,
}

impl Default for RemoteCompactionRecoveryCache {
    fn default() -> Self {
        Self::with_limits(
            REMOTE_COMPACTION_RECOVERY_CACHE_MAX_ENTRIES,
            REMOTE_COMPACTION_RECOVERY_CACHE_MAX_BYTES,
        )
    }
}

impl RemoteCompactionRecoveryCache {
    pub(crate) fn get(
        &self,
        key: &RemoteCompactionRecoveryCacheKey,
    ) -> Option<RemoteCompactionRecoveryCacheEntry> {
        self.entries.get(key).cloned()
    }

    pub(crate) fn insert(
        &mut self,
        key: RemoteCompactionRecoveryCacheKey,
        entry: RemoteCompactionRecoveryCacheEntry,
    ) {
        let entry_bytes = entry.recovered_text.len();
        if self.max_entries == 0 || entry_bytes > self.max_bytes {
            return;
        }

        if let Some(previous) = self.entries.remove(&key) {
            self.recovered_text_bytes = self
                .recovered_text_bytes
                .saturating_sub(previous.recovered_text.len());
            self.insertion_order.retain(|cached_key| cached_key != &key);
        }

        while self.entries.len() >= self.max_entries
            || self.recovered_text_bytes.saturating_add(entry_bytes) > self.max_bytes
        {
            let Some(oldest_key) = self.insertion_order.pop_front() else {
                break;
            };
            if let Some(oldest_entry) = self.entries.remove(&oldest_key) {
                self.recovered_text_bytes = self
                    .recovered_text_bytes
                    .saturating_sub(oldest_entry.recovered_text.len());
            }
        }

        self.recovered_text_bytes = self.recovered_text_bytes.saturating_add(entry_bytes);
        self.insertion_order.push_back(key.clone());
        self.entries.insert(key, entry);
    }

    fn with_limits(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            entries: HashMap::new(),
            insertion_order: VecDeque::new(),
            recovered_text_bytes: 0,
            max_entries,
            max_bytes,
        }
    }
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
    let remote_compaction_item =
        suffix_most_remote_compaction_item(active_history).ok_or_else(|| {
            CodexErr::InvalidRequest(
                "Cannot cache remote compaction recovery: no encrypted compaction item is active."
                    .to_string(),
            )
        })?;
    let compacted_state_hash = hash_json(remote_compaction_item)?;
    Ok(RemoteCompactionRecoveryCacheKey {
        compacted_state_hash,
        prompt_version: prompt_version.to_string(),
        recovery_model: recovery_model.to_string(),
        recovery_reasoning_effort: recovery_reasoning_effort.to_string(),
        algorithm_version: algorithm_version.to_string(),
    })
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
