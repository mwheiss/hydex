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
fn rejects_inline_paired_reasoning_leakage() {
    assert_eq!(
        cheap_validate_local_output(
            &ModelOffloadValidationConfig::default(),
            LocalOutputKind::CompactionPayload,
            "Need / current state:\n- <think>private scratch</think>",
        ),
        CheapValidationOutcome::Reject("visible reasoning leakage")
    );
}

#[test]
fn accepts_legitimate_reasoning_vocabulary() {
    assert_eq!(
        cheap_validate_local_output(
            &ModelOffloadValidationConfig::default(),
            LocalOutputKind::FinalText,
            "The article discusses chain of thought prompting. Update scratchpad.md next.",
        ),
        CheapValidationOutcome::Pass
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
fn rejects_punctuation_only_durable_payloads_but_allows_terse_final_text() {
    let config = ModelOffloadValidationConfig::default();

    assert_eq!(
        cheap_validate_local_output(&config, LocalOutputKind::CompactionPayload, "---"),
        CheapValidationOutcome::Reject("content-free durable payload")
    );
    assert_eq!(
        cheap_validate_local_output(&config, LocalOutputKind::MemoryPayload, "..."),
        CheapValidationOutcome::Reject("content-free durable payload")
    );
    assert_eq!(
        cheap_validate_local_output(&config, LocalOutputKind::FinalText, "..."),
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

#[test]
fn candidate_limit_scales_with_local_context_and_has_a_hard_cap() {
    assert_eq!(local_output_candidate_max_bytes(None), 512_000);
    assert_eq!(local_output_candidate_max_bytes(Some(200_000)), 3_200_000);
    assert_eq!(
        local_output_candidate_max_bytes(Some(10_000_000)),
        64 * 1024 * 1024
    );
    assert_eq!(local_output_candidate_max_bytes(Some(0)), 512_000);
}

#[test]
fn stream_limit_adds_overhead_and_stays_within_global_bounds() {
    assert_eq!(local_output_stream_max_bytes(None), 64 * 1024 * 1024);
    assert_eq!(local_output_stream_max_bytes(Some(1)), 4 * 1024 * 1024);
    assert_eq!(
        local_output_stream_max_bytes(Some(200_000)),
        3_200_000 + 1024 * 1024
    );
    assert_eq!(
        local_output_stream_max_bytes(Some(10_000_000)),
        64 * 1024 * 1024
    );
}

#[test]
fn completed_candidate_uses_context_scaled_limit() {
    let candidate = "x".repeat(512_001);
    let config = ModelOffloadValidationConfig::default();

    assert_eq!(
        cheap_validate_local_output(&config, LocalOutputKind::FinalText, &candidate),
        CheapValidationOutcome::Reject("output exceeds sanity limit")
    );
    assert_eq!(
        cheap_validate_local_output_with_context(
            &config,
            LocalOutputKind::FinalText,
            &candidate,
            Some(200_000),
        ),
        CheapValidationOutcome::Pass
    );
}

#[test]
fn validator_request_reserves_room_for_validator_output() {
    assert!(validator_request_fits_local_context(
        LocalOutputKind::CompactionPayload,
        "compact continuation state",
        Some(4_096),
    ));
    assert!(!validator_request_fits_local_context(
        LocalOutputKind::CompactionPayload,
        &"x".repeat(32_000),
        Some(4_096),
    ));
    assert!(validator_request_fits_local_context(
        LocalOutputKind::CompactionPayload,
        &"x".repeat(32_000),
        None,
    ));
}

#[test]
fn validator_output_has_a_small_hard_limit() {
    let mut output = String::new();
    append_validator_output(&mut output, &"x".repeat(VALIDATOR_OUTPUT_MAX_BYTES))
        .expect("output at the validator limit should be accepted");
    let err = append_validator_output(&mut output, "x")
        .expect_err("output beyond the validator limit should fail");

    assert!(err.contains("4096-byte response limit"));
}

#[test]
fn validator_collector_rejects_additional_or_non_message_output_items() {
    let message = ResponseItem::Message {
        id: None,
        role: "assistant".to_string(),
        content: vec![ContentItem::OutputText {
            text: r#"{"accept": true}"#.to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    };
    let function_call = ResponseItem::FunctionCall {
        id: None,
        name: "unexpected".to_string(),
        namespace: None,
        arguments: "{}".to_string(),
        call_id: "call-1".to_string(),
        internal_chat_message_metadata_passthrough: None,
    };
    let mut output = String::new();
    let mut completed_output_items = 0;

    record_validator_output_item(&mut output, &mut completed_output_items, &message)
        .expect("first message item should be accepted");
    let err =
        record_validator_output_item(&mut output, &mut completed_output_items, &function_call)
            .expect_err("additional output item should fail the validator attempt");
    assert_eq!(err, "validator returned additional output items");

    let mut output = String::new();
    let mut completed_output_items = 0;
    let err =
        record_validator_output_item(&mut output, &mut completed_output_items, &function_call)
            .expect_err("non-message validator item should fail");
    assert_eq!(err, "validator returned a non-message output item");
}
