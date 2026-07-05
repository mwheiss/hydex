//! Optional Hydex smoke tests that hit a real local Responses-compatible server.
//!
//! These tests are ignored by default. To run against a local llama-server:
//!
//! `HYDEX_LLAMA_SERVER_SMOKE=1 cargo test -p codex-core live_local_offload_responses_turn_completes --test all -- --ignored`

use codex_config::config_toml::ModelOffloadCompactionPolicy;
use codex_core::ModelClient;
use codex_core::Prompt;
use codex_core::ResponseEvent;
use codex_core::config::ModelOffloadConfig;
use codex_login::AuthManager;
use codex_login::CodexAuth;
use codex_model_provider_info::ModelProviderInfo;
use codex_model_provider_info::WireApi;
use codex_otel::SessionTelemetry;
use codex_protocol::ThreadId;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::SessionSource;
use core_test_support::TestCodexResponsesRequestKind;
use core_test_support::load_default_config_for_test;
use core_test_support::responses_metadata as test_responses_metadata;
use futures::StreamExt;
use serde_json::Value;
use std::sync::Arc;
use tempfile::TempDir;

const DEFAULT_LLAMA_SERVER_BASE_URL: &str = "http://localhost:8020/v1";
const TEST_INSTALLATION_ID: &str = "11111111-1111-4111-8111-111111111111";
const TEST_WINDOW_ID: &str = "hydex-live-offload:0";

fn live_smoke_enabled() -> bool {
    std::env::var("HYDEX_LLAMA_SERVER_SMOKE")
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
}

async fn discover_llama_server_model(base_url: &str) -> String {
    if let Ok(model) = std::env::var("HYDEX_LLAMA_SERVER_MODEL")
        && !model.trim().is_empty()
    {
        return model;
    }

    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let response = reqwest::Client::new()
        .get(&url)
        .send()
        .await
        .unwrap_or_else(|err| panic!("failed to query local llama-server models at {url}: {err}"));
    assert!(
        response.status().is_success(),
        "local llama-server models query failed: {}",
        response.status()
    );
    let body: Value = response
        .json()
        .await
        .expect("local llama-server models response should be JSON");
    body.get("data")
        .and_then(Value::as_array)
        .and_then(|models| models.first())
        .and_then(|model| model.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .expect("local llama-server should report at least one model id")
}

fn test_turn_responses_metadata(
    _client: &ModelClient,
    thread_id: ThreadId,
) -> codex_core::CodexResponsesMetadata {
    let thread_id = thread_id.to_string();
    test_responses_metadata(
        TEST_INSTALLATION_ID,
        &thread_id,
        &thread_id,
        /*turn_id*/ None,
        TEST_WINDOW_ID.to_string(),
        &SessionSource::Exec,
        /*parent_thread_id*/ None,
        TestCodexResponsesRequestKind::Turn,
    )
}

#[ignore = "requires HYDEX_LLAMA_SERVER_SMOKE=1 and a local llama-server on localhost:8020"]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_local_offload_responses_turn_completes() {
    if !live_smoke_enabled() {
        eprintln!("skipping live Hydex offload smoke; set HYDEX_LLAMA_SERVER_SMOKE=1 to run");
        return;
    }

    let base_url = std::env::var("HYDEX_LLAMA_SERVER_BASE_URL")
        .unwrap_or_else(|_| DEFAULT_LLAMA_SERVER_BASE_URL.to_string());
    let local_model = discover_llama_server_model(&base_url).await;
    let local_provider = ModelProviderInfo {
        name: "llama-server".into(),
        base_url: Some(base_url.clone()),
        env_key: None,
        env_key_instructions: None,
        experimental_bearer_token: None,
        auth: None,
        aws: None,
        wire_api: WireApi::Responses,
        query_params: None,
        http_headers: None,
        env_http_headers: None,
        request_max_retries: Some(0),
        stream_max_retries: Some(0),
        stream_idle_timeout_ms: Some(30_000),
        websocket_connect_timeout_ms: None,
        requires_openai_auth: false,
        supports_websockets: false,
    };
    let offload_config = ModelOffloadConfig {
        enabled: true,
        runtime_override: None,
        compaction_runtime_override: None,
        memory_mode: codex_config::config_toml::ModelOffloadMemoryMode::Local,
        provider_id: Some(local_provider.name.clone()),
        provider: Some(local_provider.clone()),
        model: Some(local_model.clone()),
        compaction_policy: ModelOffloadCompactionPolicy::Local,
        compaction_local_handoff_role:
            codex_config::config_toml::ModelOffloadCompactionLocalHandoffRole::UserSummary,
        compaction_recovery: codex_core::config::ModelOffloadCompactionRecoveryConfig::default(),
        context: Default::default(),
        validation: Default::default(),
    };

    let codex_home = TempDir::new().expect("create codex home");
    let mut config = load_default_config_for_test(&codex_home).await;
    config.model = Some(local_model.clone());
    let effort = config.model_reasoning_effort.clone();
    let summary = config
        .model_reasoning_summary
        .unwrap_or(codex_protocol::config_types::ReasoningSummary::Auto);
    let config = Arc::new(config);
    let model_info =
        codex_core::test_support::construct_model_info_offline(local_model.as_str(), &config);
    let thread_id = ThreadId::new();
    let session_telemetry = SessionTelemetry::new(
        thread_id,
        local_model.as_str(),
        model_info.slug.as_str(),
        /*account_id*/ None,
        Some("test@test.com".to_string()),
        /*auth_mode*/ None,
        "hydex_live_offload".to_string(),
        /*log_user_prompts*/ false,
        "test".to_string(),
        SessionSource::Exec,
    );
    let primary_provider =
        ModelProviderInfo::create_openai_provider(Some("https://primary.invalid/v1".to_string()));
    let client = ModelClient::new(
        Some(AuthManager::from_auth_for_testing(
            CodexAuth::create_dummy_chatgpt_auth_for_testing(),
        )),
        thread_id,
        primary_provider,
        SessionSource::Exec,
        config.model_verbosity,
        /*enable_request_compression*/ false,
        /*include_timing_metrics*/ false,
        /*beta_features_header*/ None,
        /*attestation_provider*/ None,
        offload_config,
    );
    let responses_metadata = test_turn_responses_metadata(&client, thread_id);
    let mut prompt = Prompt::default();
    prompt.input.push(ResponseItem::Message {
        id: None,
        role: "user".to_string(),
        content: vec![ContentItem::InputText {
            text: "Reply with a short sentence containing the words Hydex smoke.".to_string(),
        }],
        phase: None,
        metadata: None,
    });

    let mut stream = client
        .new_session()
        .stream(
            &prompt,
            &model_info,
            &session_telemetry,
            effort,
            summary,
            /*service_tier*/ None,
            &responses_metadata,
            &codex_rollout_trace::InferenceTraceContext::disabled(),
        )
        .await
        .expect("local offload Responses stream should start");
    let mut completed = false;
    let mut output_text = String::new();
    while let Some(event) = stream.next().await {
        match event.expect("local offload stream event should parse") {
            ResponseEvent::Completed { .. } => {
                completed = true;
                break;
            }
            ResponseEvent::OutputTextDelta(delta) => output_text.push_str(&delta),
            ResponseEvent::OutputItemDone(ResponseItem::Message { content, .. }) => {
                for item in content {
                    if let ContentItem::OutputText { text } = item {
                        output_text.push_str(&text);
                    }
                }
            }
            _ => {}
        }
    }

    assert!(completed, "local offload stream should complete");
    assert!(
        !output_text.trim().is_empty(),
        "local offload stream should produce assistant text"
    );
}
