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

#[test]
fn parses_exact_validator_accept_json() {
    assert_eq!(parse_validator_acceptance(r#"{"accept": true}"#), Ok(true));
    assert_eq!(
        parse_validator_acceptance(r#"{"accept": false}"#),
        Ok(false)
    );
}

#[test]
fn rejects_validator_output_with_extra_text_or_fields() {
    assert!(
        parse_validator_acceptance("sure\n{\"accept\": true}")
            .expect_err("extra prose should fail")
            .contains("invalid JSON")
    );
    assert!(
        parse_validator_acceptance(r#"{"accept": true, "reason": "ok"}"#)
            .expect_err("extra fields should fail")
            .contains("only boolean accept")
    );
    assert!(
        parse_validator_acceptance(r#"{"accept": "true"}"#)
            .expect_err("non-boolean accept should fail")
            .contains("invalid validator schema")
    );
}
