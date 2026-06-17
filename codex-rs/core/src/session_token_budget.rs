use crate::config::SessionTokenBudgetConfig;
use codex_protocol::ThreadId;
use codex_protocol::protocol::TokenUsage;
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
    sampling_tokens: i64,
    prefill_tokens: i64,
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
                sampling_tokens: 0,
                prefill_tokens: 0,
                deliveries: HashMap::new(),
            })
        });
    }

    /// Returns true exactly once, when shared usage first reaches the limit.
    pub(crate) fn record_usage(&self, usage: &TokenUsage) -> bool {
        let Some(mut state) = self.lock() else {
            return false;
        };
        let was_below_limit = state.weighted_usage() < state.config.limit_tokens as f64;
        state.sampling_tokens = state
            .sampling_tokens
            .saturating_add(usage.output_tokens.max(0));
        state.prefill_tokens = state
            .prefill_tokens
            .saturating_add(usage.non_cached_input());
        was_below_limit && state.weighted_usage() >= state.config.limit_tokens as f64
    }

    pub(crate) fn pending_reminder(
        &self,
        thread_id: ThreadId,
        window_id: &str,
    ) -> Option<SessionTokenBudgetReminder> {
        let state = self.lock()?;
        let weighted_usage = state.weighted_usage();
        let reminder_index =
            (weighted_usage / state.config.reminder_interval_tokens as f64).floor() as i64;
        if state.deliveries.get(&thread_id).is_some_and(|delivery| {
            delivery.window_id.as_str() == window_id && delivery.reminder_index >= reminder_index
        }) {
            return None;
        }
        Some(SessionTokenBudgetReminder {
            remaining_tokens: (state.config.limit_tokens as f64 - weighted_usage)
                .max(0.0)
                .floor() as i64,
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

impl SessionTokenBudgetState {
    fn weighted_usage(&self) -> f64 {
        self.sampling_tokens as f64 * self.config.sampling_token_weight
            + self.prefill_tokens as f64 * self.config.prefill_token_weight
    }
}
