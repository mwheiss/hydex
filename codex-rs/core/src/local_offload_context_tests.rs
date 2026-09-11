use super::*;
use pretty_assertions::assert_eq;
use serde_json::json;

#[test]
fn ik_llama_codex_model_metadata_has_priority() {
    let response = json!({
        "data": [{
            "id": "local-model",
            "max_model_len": 180_224
        }],
        "models": [{
            "slug": "local-model",
            "context_window": 180_000,
            "max_context_window": 180_000,
            "auto_compact_token_limit": 162_000,
            "effective_context_window_percent": 95,
            "truncation_policy": {"mode": "tokens", "limit": 180_000}
        }]
    });

    assert_eq!(
        context_from_models(&response, Some("local-model")),
        Some(AdvertisedLocalOffloadContext {
            context_window: Some(180_000),
            effective_context_window_percent: Some(95),
            auto_compact_token_limit: Some(162_000),
        })
    );
}

#[test]
fn vllm_max_model_len_is_used_for_matching_model() {
    let response = json!({
        "object": "list",
        "data": [
            {"id": "other-model", "max_model_len": 32_768},
            {"id": "local-model", "max_model_len": 131_072}
        ]
    });

    assert_eq!(
        context_from_models(&response, Some("local-model")),
        Some(AdvertisedLocalOffloadContext {
            context_window: Some(131_072),
            ..Default::default()
        })
    );
}

#[test]
fn vllm_lora_uses_parent_model_runtime_context() {
    let response = json!({
        "object": "list",
        "data": [
            {"id": "base-model", "max_model_len": 131_072},
            {"id": "local-lora", "parent": "base-model"}
        ]
    });

    assert_eq!(
        context_from_models(&response, Some("local-lora")),
        Some(AdvertisedLocalOffloadContext {
            context_window: Some(131_072),
            ..Default::default()
        })
    );
}

#[test]
fn llama_cpp_props_report_runtime_context() {
    let props = json!({
        "default_generation_settings": {"n_ctx": 65_536},
        "model_path": "/models/local.gguf"
    });

    assert_eq!(
        context_from_props(&props),
        Some(AdvertisedLocalOffloadContext {
            context_window: Some(65_536),
            ..Default::default()
        })
    );
}

#[test]
fn ollama_processes_report_loaded_context() {
    let response = json!({
        "models": [{
            "name": "qwen3:latest",
            "model": "qwen3:latest",
            "context_length": 32_768
        }]
    });

    assert_eq!(
        context_from_ollama_processes(&response, Some("qwen3:latest")),
        Some(AdvertisedLocalOffloadContext {
            context_window: Some(32_768),
            ..Default::default()
        })
    );
}

#[test]
fn lm_studio_uses_loaded_instance_context_not_model_maximum() {
    let response = json!({
        "models": [{
            "key": "local-model",
            "max_context_length": 131_072,
            "loaded_instances": [{
                "id": "local-model",
                "config": {"context_length": 49_152}
            }]
        }]
    });

    assert_eq!(
        context_from_lm_studio_models(&response, Some("local-model")),
        Some(AdvertisedLocalOffloadContext {
            context_window: Some(49_152),
            ..Default::default()
        })
    );
}

#[test]
fn sglang_and_tgi_runtime_limits_are_recognized() {
    assert_eq!(
        ContextProbe::SglangModelInfo.parse(
            &json!({"model_path": "local-model", "context_length": 65_536}),
            Some("local-model")
        ),
        AdvertisedLocalOffloadContext {
            context_window: Some(65_536),
            ..Default::default()
        }
    );
    assert_eq!(
        ContextProbe::TgiInfo.parse(
            &json!({"model_id": "local-model", "max_total_tokens": 8_192}),
            Some("local-model")
        ),
        AdvertisedLocalOffloadContext {
            context_window: Some(8_192),
            ..Default::default()
        }
    );
}

#[test]
fn llama_cpp_training_context_is_not_treated_as_runtime_context() {
    let response = json!({
        "object": "list",
        "data": [{
            "id": "local-model",
            "meta": {"n_ctx_train": 262_144}
        }]
    });

    assert_eq!(context_from_models(&response, Some("local-model")), None);
}

#[test]
fn advertised_values_override_configured_fallback_field_by_field() {
    let fallback = ModelOffloadContextConfig {
        context_window: Some(200_000),
        effective_context_window_percent: 90,
        auto_compact_token_limit: Some(170_000),
    };
    let advertised = AdvertisedLocalOffloadContext {
        context_window: Some(180_000),
        effective_context_window_percent: Some(95),
        auto_compact_token_limit: Some(162_000),
    };

    assert_eq!(
        advertised.apply_over(fallback),
        ModelOffloadContextConfig {
            context_window: Some(180_000),
            effective_context_window_percent: 95,
            auto_compact_token_limit: Some(162_000),
        }
    );
}

#[test]
fn missing_advertised_fields_retain_configured_fallbacks() {
    let fallback = ModelOffloadContextConfig {
        context_window: Some(200_000),
        effective_context_window_percent: 94,
        auto_compact_token_limit: Some(175_000),
    };

    assert_eq!(
        AdvertisedLocalOffloadContext::default().apply_over(fallback),
        fallback
    );
}

#[test]
fn auxiliary_advertised_values_are_useful_without_an_advertised_window() {
    let fallback = ModelOffloadContextConfig {
        context_window: Some(200_000),
        effective_context_window_percent: 90,
        auto_compact_token_limit: Some(170_000),
    };
    let advertised = AdvertisedLocalOffloadContext {
        context_window: None,
        effective_context_window_percent: Some(94),
        auto_compact_token_limit: Some(175_000),
    };

    assert!(advertised.is_useful());
    assert_eq!(
        advertised.apply_over(fallback),
        ModelOffloadContextConfig {
            context_window: Some(200_000),
            effective_context_window_percent: 94,
            auto_compact_token_limit: Some(175_000),
        }
    );
}

#[test]
fn props_url_targets_server_root_and_selects_model() {
    assert_eq!(
        root_endpoint_url(
            "http://127.0.0.1:8020/v1?api-version=test",
            "props",
            Some("local/model")
        ),
        Ok("http://127.0.0.1:8020/props?api-version=test&model=local%2Fmodel".to_string())
    );
}
