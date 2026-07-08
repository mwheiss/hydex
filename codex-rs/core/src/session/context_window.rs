use super::session::Session;
use super::turn_context::TurnContext;
use codex_protocol::config_types::AutoCompactTokenLimitScope;

#[derive(Debug)]
pub(crate) struct ContextWindowTokenStatus {
    pub(crate) tokens_until_compaction: Option<i64>,
}

struct BodyAfterPrefixWindowStatus {
    full_context_window_limit: Option<i64>,
}

pub(crate) async fn context_window_token_status(
    sess: &Session,
    turn_context: &TurnContext,
) -> ContextWindowTokenStatus {
    let active_context_tokens = sess.get_total_token_usage().await;

    let (auto_compact_scope_tokens, auto_compact_scope_limit, body_window) =
        match turn_context.config.model_auto_compact_token_limit_scope {
            AutoCompactTokenLimitScope::Total => (
                active_context_tokens,
                turn_context.model_info.auto_compact_token_limit(),
                None,
            ),
            AutoCompactTokenLimitScope::BodyAfterPrefix => {
                let window = sess.auto_compact_window_snapshot().await;
                let baseline = window.prefill_input_tokens.unwrap_or(active_context_tokens);

                let scope_limit = turn_context
                    .config
                    .model_auto_compact_token_limit
                    .or_else(|| turn_context.model_info.auto_compact_token_limit());
                let full_context_window_limit = turn_context.model_context_window();

                (
                    active_context_tokens.saturating_sub(baseline),
                    scope_limit,
                    Some(BodyAfterPrefixWindowStatus {
                        full_context_window_limit,
                    }),
                )
            }
        };

    let full_context_window_limit = body_window
        .as_ref()
        .and_then(|window| window.full_context_window_limit);
    let auto_compact_scope_remaining = auto_compact_scope_limit
        .map(|limit| limit.saturating_sub(auto_compact_scope_tokens).max(0));
    let full_context_remaining =
        full_context_window_limit.map(|limit| limit.saturating_sub(active_context_tokens).max(0));
    let tokens_until_compaction = match (auto_compact_scope_remaining, full_context_remaining) {
        (Some(scope_remaining), Some(full_remaining)) => Some(scope_remaining.min(full_remaining)),
        (scope_remaining, full_remaining) => scope_remaining.or(full_remaining),
    };

    ContextWindowTokenStatus {
        tokens_until_compaction,
    }
}
