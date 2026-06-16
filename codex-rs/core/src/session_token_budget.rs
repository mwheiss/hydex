use crate::config::SessionTokenBudgetConfig;
use codex_protocol::ThreadId;
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SessionTokenBudgetSnapshot {
    pub(crate) limit_tokens: i64,
    pub(crate) remaining_tokens: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionTokenBudgetAction {
    Proceed,
    Initialize {
        snapshot: SessionTokenBudgetSnapshot,
        generation: i64,
    },
    Remind {
        snapshot: SessionTokenBudgetSnapshot,
        generation: i64,
    },
}

#[derive(Default)]
pub(crate) struct SessionTokenBudget {
    state: Mutex<Option<SessionTokenBudgetState>>,
}

struct SessionTokenBudgetState {
    config: SessionTokenBudgetConfig,
    used_tokens: i64,
    deliveries: HashMap<ThreadId, SessionTokenBudgetDelivery>,
}

struct SessionTokenBudgetDelivery {
    window_id: String,
    generation: i64,
}

impl SessionTokenBudget {
    pub(crate) fn configure(&self, config: SessionTokenBudgetConfig) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.is_none() {
            *state = Some(SessionTokenBudgetState {
                config,
                used_tokens: 0,
                deliveries: HashMap::new(),
            });
        }
    }

    /// Returns true exactly once, when shared usage first reaches the limit.
    pub(crate) fn record_usage(&self, tokens: i64) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(state) = state.as_mut() else {
            return false;
        };
        let was_below_limit = state.used_tokens < state.config.limit_tokens;
        state.used_tokens = state.used_tokens.saturating_add(tokens.max(0));
        was_below_limit && state.used_tokens >= state.config.limit_tokens
    }

    pub(crate) fn before_sampling(
        &self,
        thread_id: ThreadId,
        window_id: &str,
    ) -> SessionTokenBudgetAction {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(state) = state.as_ref() else {
            return SessionTokenBudgetAction::Proceed;
        };
        let generation = state.used_tokens / state.config.reminder_interval_tokens;
        let snapshot = snapshot(&state.config, state.used_tokens);
        match state.deliveries.get(&thread_id) {
            Some(delivery)
                if delivery.window_id.as_str() == window_id && generation > delivery.generation =>
            {
                SessionTokenBudgetAction::Remind {
                    snapshot,
                    generation,
                }
            }
            Some(delivery) if delivery.window_id.as_str() == window_id => {
                SessionTokenBudgetAction::Proceed
            }
            Some(_) | None => SessionTokenBudgetAction::Initialize {
                snapshot,
                generation,
            },
        }
    }

    pub(crate) fn acknowledge(&self, thread_id: ThreadId, window_id: &str, generation: i64) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(state) = state.as_mut() else {
            return;
        };
        state.deliveries.insert(
            thread_id,
            SessionTokenBudgetDelivery {
                window_id: window_id.to_string(),
                generation,
            },
        );
    }
}

fn snapshot(config: &SessionTokenBudgetConfig, used_tokens: i64) -> SessionTokenBudgetSnapshot {
    SessionTokenBudgetSnapshot {
        limit_tokens: config.limit_tokens,
        remaining_tokens: config.limit_tokens.saturating_sub(used_tokens).max(0),
    }
}
