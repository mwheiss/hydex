use crate::config::ModelOffloadValidationConfig;

const MAX_CANDIDATE_CHARS: usize = 512_000;

/// Broad local/offload output class used by the shallow sanity validator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalOutputKind {
    FinalText,
    ToolCalls,
    StructuredOutput,
    MemoryPayload,
    CompactionPayload,
}

/// Result of cheap deterministic local-output sanity checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheapValidationOutcome {
    Pass,
    Reject(&'static str),
    Disabled,
}

pub fn validation_enabled_for_kind(
    config: &ModelOffloadValidationConfig,
    kind: LocalOutputKind,
) -> bool {
    config.enabled
        && match kind {
            LocalOutputKind::FinalText => config.final_text,
            LocalOutputKind::ToolCalls => config.tool_calls,
            LocalOutputKind::StructuredOutput => config.structured_outputs,
            LocalOutputKind::MemoryPayload => config.memory,
            LocalOutputKind::CompactionPayload => config.compaction,
        }
}

pub fn cheap_validate_local_output(
    config: &ModelOffloadValidationConfig,
    kind: LocalOutputKind,
    candidate: &str,
) -> CheapValidationOutcome {
    if !validation_enabled_for_kind(config, kind) {
        return CheapValidationOutcome::Disabled;
    }

    let trimmed = candidate.trim();
    if expects_non_empty_text(kind) && trimmed.is_empty() {
        return CheapValidationOutcome::Reject("empty output");
    }
    if trimmed.len() > MAX_CANDIDATE_CHARS {
        return CheapValidationOutcome::Reject("output exceeds sanity limit");
    }
    if expects_non_empty_text(kind) && is_placeholder_output(trimmed) {
        return CheapValidationOutcome::Reject("placeholder output");
    }
    if contains_visible_reasoning_leak(trimmed) {
        return CheapValidationOutcome::Reject("visible reasoning leakage");
    }
    if has_obvious_repetition_loop(trimmed) {
        return CheapValidationOutcome::Reject("repetitive loop");
    }
    if matches!(
        kind,
        LocalOutputKind::FinalText
            | LocalOutputKind::MemoryPayload
            | LocalOutputKind::CompactionPayload
    ) && looks_like_tool_call_stub(trimmed)
    {
        return CheapValidationOutcome::Reject("tool-call stub in text output");
    }
    if matches!(
        kind,
        LocalOutputKind::ToolCalls
            | LocalOutputKind::StructuredOutput
            | LocalOutputKind::MemoryPayload
    ) && trimmed.starts_with('{')
        && serde_json::from_str::<serde_json::Value>(trimmed).is_err()
    {
        return CheapValidationOutcome::Reject("malformed JSON-like output");
    }

    CheapValidationOutcome::Pass
}

fn expects_non_empty_text(kind: LocalOutputKind) -> bool {
    !matches!(kind, LocalOutputKind::ToolCalls)
}

fn is_placeholder_output(trimmed: &str) -> bool {
    let lower = trimmed.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "todo" | "tbd" | "n/a" | "none" | "null" | "undefined" | "placeholder" | "[placeholder]"
    ) || lower.contains("lorem ipsum")
}

fn contains_visible_reasoning_leak(trimmed: &str) -> bool {
    let lower = trimmed.to_ascii_lowercase();
    lower.contains("<think>")
        || lower.contains("</think>")
        || lower.contains("chain of thought")
        || lower.contains("scratchpad")
}

fn has_obvious_repetition_loop(trimmed: &str) -> bool {
    let words = trimmed.split_whitespace().take(80).collect::<Vec<_>>();
    if words.len() < 24 {
        return false;
    }
    for window in 3..=8 {
        if words.len() < window * 4 {
            continue;
        }
        let first = &words[0..window];
        if (1..4).all(|repeat| words[repeat * window..(repeat + 1) * window] == *first) {
            return true;
        }
    }
    false
}

fn looks_like_tool_call_stub(trimmed: &str) -> bool {
    let lower = trimmed.to_ascii_lowercase();
    (lower.contains("\"tool_calls\"") || lower.contains("\"function\""))
        && lower.contains("\"arguments\"")
}

#[cfg(test)]
#[path = "local_output_validation_tests.rs"]
mod tests;
