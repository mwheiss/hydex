use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use anyhow::Result;
use codex_config::config_toml::ModelOffloadMemoryMode;
use codex_core::TurnInputRequest;
use codex_core::config::ModelOffloadConfig;
use codex_core::config::ModelOffloadContextConfig;
use codex_history::InitialHistory;
use codex_history::RolloutItem;
use codex_model_provider_info::WireApi;
use codex_model_provider_info::create_oss_provider_with_base_url;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::InternalSessionSource;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::SessionSource;
use codex_protocol::user_input::UserInput;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_sse_once;
use core_test_support::responses::sse;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use pretty_assertions::assert_eq;
use serde_json::json;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::header;
use wiremock::matchers::method;
use wiremock::matchers::path;

const LOCAL_MODEL: &str = "local-responses-model";

fn model_offload_config(local_server: &MockServer, enabled: bool) -> ModelOffloadConfig {
    let mut provider = create_oss_provider_with_base_url(
        &format!("{}/v1", local_server.uri()),
        WireApi::Responses,
    );
    provider.name = "local-test-provider".to_string();
    provider.http_headers = Some(HashMap::from([(
        "x-local-test-token".to_string(),
        "local-only".into(),
    )]));
    provider.request_max_retries = Some(0);
    provider.stream_max_retries = Some(0);
    provider.supports_websockets = false;

    let mut config = ModelOffloadConfig {
        enabled,
        provider_id: Some(provider.name.clone()),
        provider: Some(provider),
        model: Some(LOCAL_MODEL.to_string()),
        context: ModelOffloadContextConfig {
            context_window: Some(200_000),
            ..Default::default()
        },
        ..Default::default()
    };
    config.validation.enabled = false;
    config
}

fn completed_response(id: &str) -> String {
    sse(vec![
        ev_response_created(id),
        ev_assistant_message(
            &format!("msg-{id}"),
            "The requested Hydex integration turn completed successfully.",
        ),
        ev_completed(id),
    ])
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_turn_discovers_context_before_local_sampling_without_primary_auth() -> Result<()> {
    let primary_server = MockServer::start().await;
    let local_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(header("x-local-test-token", "local-only"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{
                "id": LOCAL_MODEL,
                "context_window": 180_000,
                "effective_context_window_percent": 95,
                "auto_compact_token_limit": 162_000
            }]
        })))
        .expect(1)
        .mount(&local_server)
        .await;
    let local_responses = mount_sse_once(&local_server, completed_response("local-response")).await;
    let primary_responses =
        mount_sse_once(&primary_server, completed_response("primary-response")).await;

    let offload = model_offload_config(&local_server, /*enabled*/ true);
    let test = test_codex()
        .with_config(move |config| config.model_offload = offload)
        .build_with_auto_env(&primary_server)
        .await?;

    test.submit_turn("hello local model").await?;

    assert!(
        primary_responses.requests().is_empty(),
        "primary provider must not receive a local-routed turn"
    );
    let local_request = local_responses.single_request();
    assert_eq!(local_request.path(), "/v1/responses");
    assert_eq!(local_request.body_json()["model"], LOCAL_MODEL);

    let local_requests = local_server
        .received_requests()
        .await
        .expect("local server requests should be available");
    assert_eq!(local_requests.len(), 2);
    assert_eq!(local_requests[0].method.as_str(), "GET");
    assert_eq!(local_requests[0].url.path(), "/v1/models");
    assert!(
        !local_requests[0].headers.contains_key("authorization"),
        "local context discovery must not receive primary authorization"
    );
    assert_eq!(local_requests[1].method.as_str(), "POST");
    assert_eq!(local_requests[1].url.path(), "/v1/responses");

    local_server.verify().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disabled_offload_preserves_primary_turn_without_local_discovery() -> Result<()> {
    let primary_server = MockServer::start().await;
    let local_server = MockServer::start().await;
    let primary_responses =
        mount_sse_once(&primary_server, completed_response("primary-response")).await;

    let offload = model_offload_config(&local_server, /*enabled*/ false);
    let test = test_codex()
        .with_config(move |config| config.model_offload = offload)
        .build_with_auto_env(&primary_server)
        .await?;

    test.submit_turn("hello primary model").await?;

    let primary_request = primary_responses.single_request();
    assert_eq!(primary_request.path(), "/v1/responses");
    assert_eq!(primary_request.body_json()["model"], "gpt-5.5");
    assert!(
        local_server
            .received_requests()
            .await
            .expect("local server requests should be available")
            .is_empty(),
        "disabled offload must not probe or sample from the local provider"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_turn_without_discovered_or_configured_context_window_fails_before_sampling()
-> Result<()> {
    let primary_server = MockServer::start().await;
    let local_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{"id": LOCAL_MODEL}]
        })))
        .expect(1)
        .mount(&local_server)
        .await;

    let mut offload = model_offload_config(&local_server, /*enabled*/ true);
    offload.context = ModelOffloadContextConfig::default();
    let test = test_codex()
        .with_config(move |config| config.model_offload = offload)
        .build_with_auto_env(&primary_server)
        .await?;

    test.codex
        .start_or_steer_turn(TurnInputRequest::user_input(vec![UserInput::Text {
            text: "this must not reach an unknown local context".to_string(),
            text_elements: Vec::new(),
        }]))
        .await?;
    let error = wait_for_event(&test.codex, |event| matches!(event, EventMsg::Error(_))).await;
    let EventMsg::Error(error) = error else {
        panic!("expected missing local context error");
    };
    assert_eq!(
        error.message,
        "Cannot use model offload: the local endpoint did not advertise a context window and model_offload.context.context_window is not configured."
    );
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    let local_requests = local_server
        .received_requests()
        .await
        .expect("local server requests should be available");
    assert!(!local_requests.is_empty(), "context discovery should run");
    assert!(
        local_requests
            .iter()
            .all(|request| request.method.as_str() == "GET"),
        "no local inference request may be sent without a known context window"
    );
    assert!(
        primary_server
            .received_requests()
            .await
            .expect("primary server requests should be available")
            .is_empty(),
        "missing local context must not silently reroute an ordinary turn"
    );

    let rollout_path = test
        .session_configured
        .rollout_path
        .as_ref()
        .expect("test thread should persist a rollout");
    let InitialHistory::Resumed(resumed) =
        codex_rollout::RolloutRecorder::get_rollout_history(rollout_path).await?
    else {
        panic!("expected persisted rollout history");
    };
    assert!(
        resumed.history.iter().all(|item| match item {
            RolloutItem::TurnContext(context) => !context.offload_ever_used,
            _ => true,
        }),
        "a refused local turn must not persist offload_ever_used"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn manual_local_compaction_requires_context_before_compaction_request() -> Result<()> {
    let primary_server = MockServer::start().await;
    let local_server = MockServer::start().await;
    let discovery_count = Arc::new(AtomicUsize::new(0));

    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(move |_request: &wiremock::Request| {
            let model = if discovery_count.fetch_add(1, Ordering::SeqCst) == 0 {
                json!({"id": LOCAL_MODEL, "context_window": 64_000})
            } else {
                json!({"id": LOCAL_MODEL})
            };
            ResponseTemplate::new(200).set_body_json(json!({"data": [model]}))
        })
        .expect(2)
        .mount(&local_server)
        .await;
    let local_responses = mount_sse_once(&local_server, completed_response("local-response")).await;

    let mut offload = model_offload_config(&local_server, /*enabled*/ true);
    offload.context = ModelOffloadContextConfig::default();
    let test = test_codex()
        .with_config(move |config| config.model_offload = offload)
        .build_with_auto_env(&primary_server)
        .await?;

    test.submit_turn("mark this thread as locally offloaded")
        .await?;
    assert_eq!(local_responses.requests().len(), 1);

    test.codex.submit(Op::Compact).await?;
    let error = wait_for_event(&test.codex, |event| matches!(event, EventMsg::Error(_))).await;
    let EventMsg::Error(error) = error else {
        panic!("expected missing local context error");
    };
    assert_eq!(
        error.message,
        "Cannot use model offload: the local endpoint did not advertise a context window and model_offload.context.context_window is not configured."
    );
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    let local_requests = local_server
        .received_requests()
        .await
        .expect("local server requests should be available");
    assert_eq!(
        local_requests
            .iter()
            .filter(|request| request.method.as_str() == "POST")
            .count(),
        1,
        "manual compaction must fail before a second local inference request"
    );
    assert!(
        primary_server
            .received_requests()
            .await
            .expect("primary server requests should be available")
            .is_empty(),
        "missing local context must not silently reroute manual local compaction"
    );
    local_server.verify().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_memory_consolidation_uses_memory_sampling_and_completed_output_gate() -> Result<()> {
    let primary_server = MockServer::start().await;
    let local_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{
                "id": LOCAL_MODEL,
                "context_window": 64_000
            }]
        })))
        .expect(1)
        .mount(&local_server)
        .await;

    let local_responses = mount_sse_once(
        &local_server,
        sse(vec![
            ev_response_created("memory-response"),
            ev_assistant_message("msg-memory", "<think>broken memory output</think>"),
            ev_completed("memory-response"),
        ]),
    )
    .await;

    let mut offload = model_offload_config(&local_server, /*enabled*/ true);
    offload.memory_mode = ModelOffloadMemoryMode::Local;
    offload.validation.enabled = true;
    offload.validation.memory_temperature = Some(0.03);
    let test = test_codex()
        .with_session_source(SessionSource::Internal(
            InternalSessionSource::MemoryConsolidation,
        ))
        .with_config(move |config| config.model_offload = offload)
        .build_with_auto_env(&primary_server)
        .await?;

    test.codex
        .start_or_steer_turn(TurnInputRequest::user_input(vec![UserInput::Text {
            text: "consolidate memory".to_string(),
            text_elements: Vec::new(),
        }]))
        .await?;

    let error = wait_for_event(&test.codex, |event| matches!(event, EventMsg::Error(_))).await;
    let EventMsg::Error(error) = error else {
        panic!("expected local output validation error");
    };
    assert!(
        error
            .message
            .contains("Local model output failed sanity validation"),
        "unexpected error: {}",
        error.message
    );
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    assert!(
        primary_server
            .received_requests()
            .await
            .expect("primary server requests should be available")
            .is_empty(),
        "primary provider must not receive a local memory-consolidation turn"
    );
    let local_request = local_responses.single_request();
    assert_eq!(local_request.body_json()["model"], LOCAL_MODEL);
    assert_eq!(local_request.body_json()["temperature"], 0.03);
    let local_requests = local_server
        .received_requests()
        .await
        .expect("local server requests should be available");
    assert_eq!(local_requests[0].method.as_str(), "GET");
    assert_eq!(local_requests[0].url.path(), "/v1/models");
    assert_eq!(local_requests[1].method.as_str(), "POST");
    assert_eq!(local_requests[1].url.path(), "/v1/responses");
    local_server.verify().await;
    Ok(())
}
