use crate::config::SessionTokenBudgetConfig;
use codex_protocol::ThreadId;
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::OnceLock;

pub(crate) struct SessionTokenBudgetReminder {
    pub(crate) remaining_tokens: i64,
    reminder_index: i64,
}

/// Shared accounting and reminder state for one root-thread session tree.
#[derive(Default)]
pub(crate) struct SessionTokenBudget {
    state: OnceLock<Mutex<SessionTokenBudgetState>>,
}

struct SessionTokenBudgetState {
    config: SessionTokenBudgetConfig,
    used_tokens: i64,
    /// Last reminder delivered to each thread, so every thread observes crossed thresholds.
    deliveries: HashMap<ThreadId, ThreadBudgetDelivery>,
}

struct ThreadBudgetDelivery {
    window_id: String,
    reminder_index: i64,
}

impl SessionTokenBudget {
    pub(crate) fn configure(&self, config: SessionTokenBudgetConfig) {
        self.state.get_or_init(|| {
            Mutex::new(SessionTokenBudgetState {
                config,
                used_tokens: 0,
                deliveries: HashMap::new(),
            })
        });
    }

    /// Returns true exactly once, when shared usage first reaches the limit.
    pub(crate) fn record_usage(&self, tokens: i64) -> bool {
        let Some(mut state) = self.lock() else {
            return false;
        };
        let was_below_limit = state.used_tokens < state.config.limit_tokens;
        state.used_tokens = state.used_tokens.saturating_add(tokens.max(0));
        was_below_limit && state.used_tokens >= state.config.limit_tokens
    }

    pub(crate) fn pending_reminder(
        &self,
        thread_id: ThreadId,
        window_id: &str,
    ) -> Option<SessionTokenBudgetReminder> {
        let state = self.lock()?;
        let reminder_index = state.used_tokens / state.config.reminder_interval_tokens;
        if state.deliveries.get(&thread_id).is_some_and(|delivery| {
            delivery.window_id.as_str() == window_id && delivery.reminder_index >= reminder_index
        }) {
            return None;
        }
        Some(SessionTokenBudgetReminder {
            remaining_tokens: state
                .config
                .limit_tokens
                .saturating_sub(state.used_tokens)
                .max(0),
            reminder_index,
        })
    }

    pub(crate) fn mark_reminder_delivered(
        &self,
        thread_id: ThreadId,
        window_id: &str,
        reminder: SessionTokenBudgetReminder,
    ) {
        // Mark delivery only after history insertion; cancellation before then should retry it.
        let Some(mut state) = self.lock() else {
            return;
        };
        state.deliveries.insert(
            thread_id,
            ThreadBudgetDelivery {
                window_id: window_id.to_string(),
                reminder_index: reminder.reminder_index,
            },
        );
    }

    fn lock(&self) -> Option<MutexGuard<'_, SessionTokenBudgetState>> {
        self.state.get().map(|state| {
            state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        })
    }
}
