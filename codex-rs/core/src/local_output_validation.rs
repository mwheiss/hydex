use crate::Prompt;
use crate::client::ModelClientSession;
use crate::client_common::ResponseEvent;
use crate::config::ModelOffloadValidationConfig;
use crate::responses_metadata::CodexResponsesMetadata;
use codex_otel::SessionTelemetry;
use codex_protocol::config_types::ReasoningSummary;
use codex_protocol::error::CodexErr;
use codex_protocol::error::CodexErrorDetails;
use codex_protocol::error::Result as CodexResult;
use codex_protocol::models::BaseInstructions;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseInputItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::openai_models::ReasoningEffort;
use codex_rollout_trace::InferenceTraceContext;
use codex_utils_output_truncation::approx_token_count;
use futures::StreamExt;
use serde::Deserialize;

const DEFAULT_MAX_CANDIDATE_BYTES: usize = 512_000;
// Four bytes per token is common for prose; 16 keeps this a runaway guard rather than a normal
// generation limit for source code, JSON, or unusual tokenizers.
const LOCAL_OUTPUT_BYTES_PER_CONTEXT_TOKEN: usize = 16;
const LOCAL_OUTPUT_STREAM_FIXED_OVERHEAD_BYTES: usize = 1024 * 1024;
const LOCAL_OUTPUT_STREAM_MIN_BYTES: usize = 4 * 1024 * 1024;
const LOCAL_OUTPUT_MAX_BYTES: usize = 64 * 1024 * 1024;
const VALIDATOR_CONTEXT_RESERVE_TOKENS: usize = 1024;
const VALIDATOR_OUTPUT_MAX_BYTES: usize = 4 * 1024;
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

pub(crate) fn retry_temperature_after_greedy_local_call(
    initial_temperature: Option<f64>,
    retry_temperature: f64,
) -> Option<f64> {
    (initial_temperature == Some(0.0)).then_some(retry_temperature)
}

enum ValidatorOutputError {
    Call(CodexErr),
    InvalidOutput(String),
}

fn validation_enabled_for_kind(
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

#[cfg(test)]
fn cheap_validate_local_output(
    config: &ModelOffloadValidationConfig,
    kind: LocalOutputKind,
    candidate: &str,
) -> CheapValidationOutcome {
    cheap_validate_local_output_with_context(config, kind, candidate, None)
}

pub fn cheap_validate_local_output_with_context(
    config: &ModelOffloadValidationConfig,
    kind: LocalOutputKind,
    candidate: &str,
    local_context_window: Option<i64>,
) -> CheapValidationOutcome {
    if !validation_enabled_for_kind(config, kind) {
        return CheapValidationOutcome::Disabled;
    }

    let trimmed = candidate.trim();
    if expects_non_empty_text(kind) && trimmed.is_empty() {
        return CheapValidationOutcome::Reject("empty output");
    }
    if trimmed.len() > local_output_candidate_max_bytes(local_context_window) {
        return CheapValidationOutcome::Reject("output exceeds sanity limit");
    }
    if expects_non_empty_text(kind) && is_placeholder_output(trimmed) {
        return CheapValidationOutcome::Reject("placeholder output");
    }
    if is_durable_payload(kind) && !trimmed.chars().any(char::is_alphanumeric) {
        return CheapValidationOutcome::Reject("content-free durable payload");
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

pub(crate) fn local_output_candidate_max_bytes(local_context_window: Option<i64>) -> usize {
    local_context_window
        .and_then(|tokens| usize::try_from(tokens).ok())
        .filter(|tokens| *tokens > 0)
        .map(|tokens| {
            tokens
                .saturating_mul(LOCAL_OUTPUT_BYTES_PER_CONTEXT_TOKEN)
                .min(LOCAL_OUTPUT_MAX_BYTES)
        })
        .unwrap_or(DEFAULT_MAX_CANDIDATE_BYTES)
}

pub(crate) fn local_output_stream_max_bytes(local_context_window: Option<i64>) -> usize {
    local_context_window
        .and_then(|tokens| usize::try_from(tokens).ok())
        .filter(|tokens| *tokens > 0)
        .map(|tokens| {
            tokens
                .saturating_mul(LOCAL_OUTPUT_BYTES_PER_CONTEXT_TOKEN)
                .saturating_add(LOCAL_OUTPUT_STREAM_FIXED_OVERHEAD_BYTES)
                .clamp(LOCAL_OUTPUT_STREAM_MIN_BYTES, LOCAL_OUTPUT_MAX_BYTES)
        })
        .unwrap_or(LOCAL_OUTPUT_MAX_BYTES)
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
    local_context_window: Option<i64>,
) -> CodexResult<LocalOutputValidationResult> {
    match cheap_validate_local_output_with_context(config, kind, candidate, local_context_window) {
        CheapValidationOutcome::Pass => {}
        CheapValidationOutcome::Reject(reason) => {
            return Ok(LocalOutputValidationResult::Rejected(format!(
                "cheap sanity validation failed: {reason}"
            )));
        }
        CheapValidationOutcome::Disabled => return Ok(LocalOutputValidationResult::Disabled),
    }

    if !validator_request_fits_local_context(kind, candidate, local_context_window) {
        return Ok(LocalOutputValidationResult::ValidationUnavailable(format!(
            "validator request would exceed local context window after reserving {VALIDATOR_CONTEXT_RESERVE_TOKENS} tokens"
        )));
    }

    let attempts = config.validator_attempts.max(1);
    let mut last_error = String::new();
    for attempt in 1..=attempts {
        let raw_output = match collect_validator_output(
            client_session,
            model_info,
            session_telemetry,
            reasoning_effort.clone(),
            reasoning_summary,
            service_tier.clone(),
            responses_metadata,
            kind,
            candidate,
            (attempt > 1)
                .then(|| {
                    retry_temperature_after_greedy_local_call(
                        config.validator_temperature,
                        config.retry_temperature,
                    )
                })
                .flatten(),
        )
        .await
        {
            Ok(raw_output) => raw_output,
            Err(ValidatorOutputError::InvalidOutput(err)) => {
                tracing::warn!(
                    "local output validator produced unusable output on attempt {attempt}/{attempts}: {err}"
                );
                last_error = err;
                continue;
            }
            Err(ValidatorOutputError::Call(err))
                if matches!(
                    err.details(),
                    CodexErrorDetails::Interrupted
                        | CodexErrorDetails::TurnAborted
                        | CodexErrorDetails::SessionBudgetExceeded
                ) =>
            {
                return Err(err);
            }
            Err(ValidatorOutputError::Call(err)) => {
                tracing::warn!(
                    "local output validator call failed on attempt {attempt}/{attempts}: {err}"
                );
                last_error = format!("validator call failed: {err}");
                continue;
            }
        };
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

    let reason = if last_error.is_empty() {
        "validator did not return a response".to_string()
    } else {
        format!("validator unavailable after {attempts} attempts: {last_error}")
    };
    Ok(LocalOutputValidationResult::ValidationUnavailable(reason))
}

fn parse_validator_acceptance(raw_output: &str) -> std::result::Result<bool, String> {
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
    temperature: Option<f64>,
) -> std::result::Result<String, ValidatorOutputError> {
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
            provenance: None,
        },
        temperature,
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
        .await
        .map_err(ValidatorOutputError::Call)?;

    let mut result = String::new();
    let mut completed = false;
    let mut completed_output_items = 0usize;
    while let Some(message) = stream.next().await {
        let message = message.map_err(ValidatorOutputError::Call)?;
        match message {
            ResponseEvent::OutputTextDelta(delta) => {
                append_validator_output(&mut result, &delta)
                    .map_err(ValidatorOutputError::InvalidOutput)?;
            }
            ResponseEvent::OutputItemDone(item) => {
                record_validator_output_item(&mut result, &mut completed_output_items, &item)
                    .map_err(ValidatorOutputError::InvalidOutput)?;
            }
            ResponseEvent::Completed { .. } => {
                completed = true;
                break;
            }
            _ => {}
        }
    }
    if !completed {
        return Err(ValidatorOutputError::Call(CodexErr::Stream(
            "local output validator stream ended before completion".to_string(),
        )));
    }
    Ok(result)
}

fn record_validator_output_item(
    output: &mut String,
    completed_output_items: &mut usize,
    item: &ResponseItem,
) -> std::result::Result<(), String> {
    if *completed_output_items != 0 {
        return Err("validator returned additional output items".to_string());
    }
    *completed_output_items += 1;
    let Some(text) = output_text_from_item(item) else {
        return Err("validator returned a non-message output item".to_string());
    };
    if output.is_empty() {
        append_validator_output(output, &text)?;
    }
    Ok(())
}

fn append_validator_output(output: &mut String, text: &str) -> std::result::Result<(), String> {
    if output.len().saturating_add(text.len()) > VALIDATOR_OUTPUT_MAX_BYTES {
        return Err(format!(
            "local output validator exceeded {VALIDATOR_OUTPUT_MAX_BYTES}-byte response limit"
        ));
    }
    output.push_str(text);
    Ok(())
}

fn validator_request_fits_local_context(
    kind: LocalOutputKind,
    candidate: &str,
    local_context_window: Option<i64>,
) -> bool {
    let Some(context_window) = local_context_window
        .and_then(|tokens| usize::try_from(tokens).ok())
        .filter(|tokens| *tokens > VALIDATOR_CONTEXT_RESERVE_TOKENS)
    else {
        return local_context_window.is_none();
    };
    let estimated_tokens = approx_token_count(VALIDATOR_INSTRUCTIONS)
        .saturating_add(approx_token_count(&validator_user_message(kind, candidate)));
    estimated_tokens <= context_window.saturating_sub(VALIDATOR_CONTEXT_RESERVE_TOKENS)
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
            ContentItem::InputImage { .. } | ContentItem::InputAudio { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    if text.is_empty() { None } else { Some(text) }
}

fn expects_non_empty_text(kind: LocalOutputKind) -> bool {
    !matches!(kind, LocalOutputKind::ToolCalls)
}

fn is_durable_payload(kind: LocalOutputKind) -> bool {
    matches!(
        kind,
        LocalOutputKind::MemoryPayload | LocalOutputKind::CompactionPayload
    )
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
    lower.trim_start().starts_with("<think>")
        || lower
            .find("<think>")
            .is_some_and(|start| lower[start + "<think>".len()..].contains("</think>"))
        || lower
            .lines()
            .any(|line| matches!(line.trim(), "<think>" | "</think>"))
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
