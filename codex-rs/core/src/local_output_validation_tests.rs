use super::*;
use pretty_assertions::assert_eq;

#[test]
fn validation_can_be_disabled_per_kind() {
    let config = ModelOffloadValidationConfig {
        compaction: false,
        ..Default::default()
    };

    assert_eq!(
        cheap_validate_local_output(&config, LocalOutputKind::CompactionPayload, ""),
        CheapValidationOutcome::Disabled
    );
}

#[test]
fn rejects_empty_text_payloads() {
    assert_eq!(
        cheap_validate_local_output(
            &ModelOffloadValidationConfig::default(),
            LocalOutputKind::FinalText,
            "   ",
        ),
        CheapValidationOutcome::Reject("empty output")
    );
}

#[test]
fn rejects_visible_reasoning_leakage() {
    assert_eq!(
        cheap_validate_local_output(
            &ModelOffloadValidationConfig::default(),
            LocalOutputKind::CompactionPayload,
            "<think>private scratch</think>\nsummary",
        ),
        CheapValidationOutcome::Reject("visible reasoning leakage")
    );
}

#[test]
fn rejects_repetitive_loops() {
    let looped = "alpha beta gamma alpha beta gamma alpha beta gamma alpha beta gamma alpha beta gamma alpha beta gamma alpha beta gamma alpha beta gamma";

    assert_eq!(
        cheap_validate_local_output(
            &ModelOffloadValidationConfig::default(),
            LocalOutputKind::MemoryPayload,
            looped,
        ),
        CheapValidationOutcome::Reject("repetitive loop")
    );
}

#[test]
fn rejects_tool_call_stub_for_text_payload() {
    assert_eq!(
        cheap_validate_local_output(
            &ModelOffloadValidationConfig::default(),
            LocalOutputKind::FinalText,
            r#"{"tool_calls":[{"function":{"name":"shell","arguments":"{}"}}]}"#,
        ),
        CheapValidationOutcome::Reject("tool-call stub in text output")
    );
}

#[test]
fn rejects_malformed_json_like_memory_payload() {
    assert_eq!(
        cheap_validate_local_output(
            &ModelOffloadValidationConfig::default(),
            LocalOutputKind::MemoryPayload,
            r#"{"memories":["unterminated"]"#,
        ),
        CheapValidationOutcome::Reject("malformed JSON-like output")
    );
}

#[test]
fn accepts_coherent_compaction_text() {
    assert_eq!(
        cheap_validate_local_output(
            &ModelOffloadValidationConfig::default(),
            LocalOutputKind::CompactionPayload,
            "Need / current state:\n- Continue implementing Hydex validation.\n\nLiteral anchors:\n- model_offload.validation.enabled",
        ),
        CheapValidationOutcome::Pass
    );
}
