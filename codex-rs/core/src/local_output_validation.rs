use crate::Prompt;
use crate::client::ModelClientSession;
use crate::client_common::ResponseEvent;
use crate::config::ModelOffloadValidationConfig;
use crate::responses_metadata::CodexResponsesMetadata;
use codex_otel::SessionTelemetry;
use codex_protocol::config_types::ReasoningSummary;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result as CodexResult;
use codex_protocol::models::BaseInstructions;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseInputItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::openai_models::ReasoningEffort;
use codex_rollout_trace::InferenceTraceContext;
use futures::StreamExt;
use serde::Deserialize;

const MAX_CANDIDATE_CHARS: usize = 512_000;
const VALIDATOR_INSTRUCTIONS: &str = r#"You are checking a completed local model output for superficial structural sanity only.

Reject only if the candidate is clearly broken: empty, placeholder, repetitive loop, visible reasoning/thinking leakage, malformed protocol output, tool-call stub in text, or obviously not the expected broad output type.

Do not judge quality, correctness, completeness, helpfulness, style, factuality, or optimality.
Do not critique, rewrite, rank, score, or explain the candidate.

Return exactly one JSON object and no other text:
{"accept": true}
or
{"accept": false}"#;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalOutputValidationResult {
    Accepted,
    Rejected(String),
    ValidationUnavailable(String),
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

#[allow(clippy::too_many_arguments)]
pub async fn validate_local_output_with_model(
    config: &ModelOffloadValidationConfig,
    kind: LocalOutputKind,
    candidate: &str,
    client_session: &mut ModelClientSession,
    model_info: &ModelInfo,
    session_telemetry: &SessionTelemetry,
    reasoning_effort: Option<ReasoningEffort>,
    reasoning_summary: ReasoningSummary,
    service_tier: Option<String>,
    responses_metadata: &CodexResponsesMetadata,
) -> CodexResult<LocalOutputValidationResult> {
    match cheap_validate_local_output(config, kind, candidate) {
        CheapValidationOutcome::Pass => {}
        CheapValidationOutcome::Reject(reason) => {
            return Ok(LocalOutputValidationResult::Rejected(format!(
                "cheap sanity validation failed: {reason}"
            )));
        }
        CheapValidationOutcome::Disabled => return Ok(LocalOutputValidationResult::Disabled),
    }

    let attempts = config.validator_attempts.max(1);
    let mut last_error = String::new();
    for attempt in 1..=attempts {
        let raw_output = collect_validator_output(
            client_session,
            model_info,
            session_telemetry,
            reasoning_effort.clone(),
            reasoning_summary,
            service_tier.clone(),
            responses_metadata,
            kind,
            candidate,
        )
        .await?;
        match parse_validator_acceptance(&raw_output) {
            Ok(true) => return Ok(LocalOutputValidationResult::Accepted),
            Ok(false) => {
                return Ok(LocalOutputValidationResult::Rejected(
                    "model validator rejected output".to_string(),
                ));
            }
            Err(err) => {
                tracing::warn!(
                    "local output validator returned malformed JSON on attempt {attempt}/{attempts}: {err}"
                );
                last_error = err;
            }
        }
    }

    Ok(LocalOutputValidationResult::ValidationUnavailable(
        if last_error.is_empty() {
            "validator did not return a response".to_string()
        } else {
            format!("validator did not satisfy JSON contract: {last_error}")
        },
    ))
}

pub fn parse_validator_acceptance(raw_output: &str) -> std::result::Result<bool, String> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ValidatorResponse {
        accept: bool,
    }

    let trimmed = raw_output.trim();
    if trimmed.is_empty() {
        return Err("empty validator output".to_string());
    }
    let value = serde_json::from_str::<serde_json::Value>(trimmed)
        .map_err(|err| format!("invalid JSON: {err}"))?;
    let Some(object) = value.as_object() else {
        return Err("validator output was not a JSON object".to_string());
    };
    if object.len() != 1 || !object.contains_key("accept") {
        return Err("validator output must contain only boolean accept".to_string());
    }
    serde_json::from_value::<ValidatorResponse>(value)
        .map(|response| response.accept)
        .map_err(|err| format!("invalid validator schema: {err}"))
}

#[allow(clippy::too_many_arguments)]
async fn collect_validator_output(
    client_session: &mut ModelClientSession,
    model_info: &ModelInfo,
    session_telemetry: &SessionTelemetry,
    reasoning_effort: Option<ReasoningEffort>,
    reasoning_summary: ReasoningSummary,
    service_tier: Option<String>,
    responses_metadata: &CodexResponsesMetadata,
    kind: LocalOutputKind,
    candidate: &str,
) -> CodexResult<String> {
    let prompt = Prompt {
        input: vec![
            ResponseInputItem::Message {
                role: "user".to_string(),
                content: vec![ContentItem::InputText {
                    text: validator_user_message(kind, candidate),
                }],
                phase: None,
            }
            .into(),
        ],
        base_instructions: BaseInstructions {
            text: VALIDATOR_INSTRUCTIONS.to_string(),
        },
        ..Default::default()
    };
    let mut stream = client_session
        .stream(
            &prompt,
            model_info,
            session_telemetry,
            reasoning_effort,
            reasoning_summary,
            service_tier,
            responses_metadata,
            &InferenceTraceContext::disabled(),
        )
        .await?;

    let mut result = String::new();
    let mut completed = false;
    while let Some(message) = stream.next().await.transpose()? {
        match message {
            ResponseEvent::OutputTextDelta(delta) => result.push_str(&delta),
            ResponseEvent::OutputItemDone(item) => {
                if result.is_empty()
                    && let Some(text) = output_text_from_item(&item)
                {
                    result.push_str(&text);
                }
            }
            ResponseEvent::Completed { .. } => {
                completed = true;
                break;
            }
            _ => {}
        }
    }
    if !completed {
        return Err(CodexErr::Stream(
            "local output validator stream ended before completion".to_string(),
            None,
        ));
    }
    Ok(result)
}

fn validator_user_message(kind: LocalOutputKind, candidate: &str) -> String {
    let kind = match kind {
        LocalOutputKind::FinalText => "final_text",
        LocalOutputKind::ToolCalls => "tool_calls",
        LocalOutputKind::StructuredOutput => "structured_output",
        LocalOutputKind::MemoryPayload => "memory_payload",
        LocalOutputKind::CompactionPayload => "compaction_payload",
    };
    format!(
        "Expected broad output type: {kind}\n\nCandidate output:\n<HYDEX_LOCAL_OUTPUT_CANDIDATE>\n{candidate}\n</HYDEX_LOCAL_OUTPUT_CANDIDATE>"
    )
}

fn output_text_from_item(item: &ResponseItem) -> Option<String> {
    let ResponseItem::Message { content, .. } = item else {
        return None;
    };
    let text = content
        .iter()
        .filter_map(|content_item| match content_item {
            ContentItem::InputText { text } | ContentItem::OutputText { text } => {
                Some(text.as_str())
            }
            ContentItem::InputImage { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    if text.is_empty() { None } else { Some(text) }
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
