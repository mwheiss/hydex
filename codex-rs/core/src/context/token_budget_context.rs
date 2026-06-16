use super::ContextualUserFragment;
use codex_protocol::ThreadId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TokenBudgetContext {
    thread_id: ThreadId,
    window_id: u64,
    tokens_left: i64,
}

impl TokenBudgetContext {
    pub(crate) fn new(thread_id: ThreadId, window_id: u64, tokens_left: i64) -> Self {
        Self {
            thread_id,
            window_id,
            tokens_left,
        }
    }
}

impl ContextualUserFragment for TokenBudgetContext {
    fn role(&self) -> &'static str {
        "developer"
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        ("<token_budget>\n", "\n</token_budget>")
    }

    fn body(&self) -> String {
        let thread_id = self.thread_id;
        let window_id = self.window_id;
        let tokens_left = self.tokens_left;
        format!(
            "Thread id {thread_id}.\nCurrent context window {window_id}.\nYou have {tokens_left} tokens left in this context window."
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TokenBudgetRemainingContext {
    tokens_left: Option<i64>,
}

impl TokenBudgetRemainingContext {
    pub(crate) fn new(tokens_left: i64) -> Self {
        Self {
            tokens_left: Some(tokens_left),
        }
    }

    pub(crate) fn unknown() -> Self {
        Self { tokens_left: None }
    }
}

impl ContextualUserFragment for TokenBudgetRemainingContext {
    fn role(&self) -> &'static str {
        "developer"
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        ("<token_budget>\n", "\n</token_budget>")
    }

    fn body(&self) -> String {
        match self.tokens_left {
            Some(tokens_left) => {
                format!("You have {tokens_left} tokens left in this context window.")
            }
            None => "You have unknown tokens left in this context window.".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SessionTokenBudgetContext {
    Declaration { limit_tokens: i64 },
    Reminder { remaining_tokens: i64 },
}

impl ContextualUserFragment for SessionTokenBudgetContext {
    fn role(&self) -> &'static str {
        "developer"
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        ("<session_token_budget>\n", "\n</session_token_budget>")
    }

    fn body(&self) -> String {
        match self {
            Self::Declaration { limit_tokens } => format!(
                "This session has a shared token budget of {limit_tokens} tokens across all threads."
            ),
            Self::Reminder { remaining_tokens } => format!(
                "You have {remaining_tokens} tokens left in the shared session token budget."
            ),
        }
    }
}
