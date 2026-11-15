use anyhow::Result;
use codex_config::config_toml::ModelOffloadMemoryMode;
use codex_core::TurnInputRequest;
use codex_core::config::ModelOffloadConfig;
use codex_core::config::ModelOffloadContextConfig;
use codex_features::Feature;
use codex_model_provider_info::WireApi;
use codex_model_provider_info::create_oss_provider_with_base_url;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::user_input::UserInput;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_function_call;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_sse_once;
use core_test_support::responses::mount_sse_sequence;
use core_test_support::responses::sse;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::collections::HashMap;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
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
async fn full_local_completes_with_an_exhausted_primary_provider() -> Result<()> {
    let primary_server = MockServer::start().await;
    let local_server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(402).set_body_json(json!({
            "error": {"message": "Workspace credits exhausted", "type": "insufficient_quota"}
        })))
        .expect(0)
        .mount(&primary_server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{"id": LOCAL_MODEL, "context_window": 200_000}]
        })))
        .mount(&local_server)
        .await;
    let local_response = mount_sse_once(&local_server, completed_response("full-local")).await;
    let mut offload = model_offload_config(&local_server, true);
    offload.mode = codex_config::config_toml::ModelOffloadMode::Full;
    offload.memory_mode = ModelOffloadMemoryMode::Primary;
    offload.compaction_policy = codex_config::config_toml::ModelOffloadCompactionPolicy::Primary;
    let test = test_codex()
        .with_config(move |config| config.model_offload = offload)
        .build_with_auto_env(&primary_server)
        .await?;
    test.submit_turn("Complete this turn using only the local model.")
        .await?;
    assert_eq!(
        local_response.single_request().body_json()["model"],
        LOCAL_MODEL
    );
    let compact_response =
        mount_sse_once(&local_server, completed_response("full-local-compact")).await;
    test.codex.submit(Op::Compact).await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    assert_eq!(
        compact_response.single_request().body_json()["model"],
        LOCAL_MODEL
    );
    assert!(
        primary_server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|request| request.method.as_str() != "POST")
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_local_discovers_tools_with_plain_function_calls_and_no_primary_requests() -> Result<()>
{
    let primary_server = MockServer::start().await;
    let local_server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(402))
        .expect(0)
        .mount(&primary_server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{"id": LOCAL_MODEL, "context_window": 200_000}]
        })))
        .mount(&local_server)
        .await;
    let response = mount_sse_sequence(
        &local_server,
        vec![
            sse(vec![
                ev_response_created("discover"),
                ev_function_call(
                    "find-wait",
                    "tool_search",
                    r#"{"query":"wait_agent","limit":1}"#,
                ),
                ev_completed("discover"),
            ]),
            completed_response("after-discovery"),
        ],
    )
    .await;
    let mut offload = model_offload_config(&local_server, true);
    offload.mode = codex_config::config_toml::ModelOffloadMode::Full;
    let test = test_codex()
        .with_model_info_override("gpt-5.2", |model| {
            model.tool_mode = Some(codex_protocol::openai_models::ToolMode::Direct);
            model.supports_search_tool = false;
        })
        .with_config(move |config| {
            config.model_offload = offload;
            config.features.enable(Feature::Collab).unwrap();
            config.features.disable(Feature::MultiAgentV2).unwrap();
        })
        .build_with_auto_env(&primary_server)
        .await?;
    test.submit_turn("Find the tool for waiting on an agent. Do not start an agent.")
        .await?;
    let requests = response.requests();
    assert_eq!(requests.len(), 2);
    let first = requests[0].body_json();
    assert!(
        first["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["type"] == "function" && tool["name"] == "tool_search")
    );
    assert!(!first.to_string().contains("ns__functions__wait_agent"));
    let second = requests[1].body_json();
    assert!(second["tools"].as_array().unwrap().iter().any(|tool| {
        tool["name"]
            .as_str()
            .is_some_and(|name| name.ends_with("__wait_agent"))
    }));
    let output = second["input"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["type"] == "function_call_output" && item["call_id"] == "find-wait")
        .expect("client discovery returned a portable function output");
    assert!(output["output"].as_str().unwrap().contains("loaded_tools"));
    assert!(
        second["input"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["type"] != "tool_search_call" && item["type"] != "tool_search_output")
    );
    assert!(
        primary_server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|request| request.method.as_str() != "POST")
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_local_does_not_fall_back_when_the_local_endpoint_fails() -> Result<()> {
    let primary_server = MockServer::start().await;
    let local_server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(402).set_body_json(json!({
            "error": {"message": "Workspace credits exhausted", "type": "insufficient_quota"}
        })))
        .expect(0)
        .mount(&primary_server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{"id": LOCAL_MODEL, "context_window": 200_000}]
        })))
        .mount(&local_server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(503).set_body_json(json!({
            "error": {"message": "Local test endpoint unavailable", "type": "server_error"}
        })))
        .expect(1)
        .mount(&local_server)
        .await;
    let mut offload = model_offload_config(&local_server, true);
    offload.mode = codex_config::config_toml::ModelOffloadMode::Full;
    let test = test_codex()
        .with_config(move |config| config.model_offload = offload)
        .build_with_auto_env(&primary_server)
        .await?;
    test.codex
        .start_or_steer_turn(TurnInputRequest::user_input(vec![UserInput::Text {
            text: "Report the local endpoint failure without changing providers.".into(),
            text_elements: Vec::new(),
        }]))
        .await?;
    let error = wait_for_event(&test.codex, |event| matches!(event, EventMsg::Error(_))).await;
    assert!(
        matches!(error, EventMsg::Error(error) if error.message.contains("Local test endpoint unavailable"))
    );
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    assert!(
        primary_server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|request| request.method.as_str() != "POST")
    );
    Ok(())
}
