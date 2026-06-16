use crate::config::SessionTokenBudgetConfig;
use codex_protocol::ThreadId;
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SessionTokenBudgetSnapshot {
    pub(crate) limit_tokens: i64,
    pub(crate) remaining_tokens: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionTokenBudgetContextUpdate {
    Initial {
        snapshot: SessionTokenBudgetSnapshot,
        reminder_index: i64,
    },
    Remind {
        snapshot: SessionTokenBudgetSnapshot,
        reminder_index: i64,
    },
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

    pub(crate) fn begin_context_window(
        &self,
        thread_id: ThreadId,
        window_id: &str,
    ) -> Option<SessionTokenBudgetSnapshot> {
        let mut state = self.lock()?;
        let reminder_index = state.used_tokens / state.config.reminder_interval_tokens;
        let snapshot = snapshot(&state.config, state.used_tokens);
        state.deliveries.insert(
            thread_id,
            ThreadBudgetDelivery {
                window_id: window_id.to_string(),
                reminder_index,
            },
        );
        Some(snapshot)
    }

    pub(crate) fn pending_context_update(
        &self,
        thread_id: ThreadId,
        window_id: &str,
    ) -> Option<SessionTokenBudgetContextUpdate> {
        let state = self.lock()?;
        let reminder_index = state.used_tokens / state.config.reminder_interval_tokens;
        let snapshot = snapshot(&state.config, state.used_tokens);
        match state.deliveries.get(&thread_id) {
            Some(delivery)
                if delivery.window_id.as_str() == window_id
                    && reminder_index > delivery.reminder_index =>
            {
                Some(SessionTokenBudgetContextUpdate::Remind {
                    snapshot,
                    reminder_index,
                })
            }
            Some(delivery) if delivery.window_id.as_str() == window_id => None,
            Some(_) | None => Some(SessionTokenBudgetContextUpdate::Initial {
                snapshot,
                reminder_index,
            }),
        }
    }

    pub(crate) fn mark_context_delivered(
        &self,
        thread_id: ThreadId,
        window_id: &str,
        reminder_index: i64,
    ) {
        // Mark delivery only after history insertion; cancellation before then should retry it.
        let Some(mut state) = self.lock() else {
            return;
        };
        state.deliveries.insert(
            thread_id,
            ThreadBudgetDelivery {
                window_id: window_id.to_string(),
                reminder_index,
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

fn snapshot(config: &SessionTokenBudgetConfig, used_tokens: i64) -> SessionTokenBudgetSnapshot {
    SessionTokenBudgetSnapshot {
        limit_tokens: config.limit_tokens,
        remaining_tokens: config.limit_tokens.saturating_sub(used_tokens).max(0),
    }
}
