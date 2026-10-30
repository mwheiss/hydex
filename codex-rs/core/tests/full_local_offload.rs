use anyhow::Result;
use codex_config::config_toml::ModelOffloadMemoryMode;
use codex_core::config::ModelOffloadConfig;
use codex_core::config::ModelOffloadContextConfig;
use codex_model_provider_info::WireApi;
use codex_model_provider_info::create_oss_provider_with_base_url;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_sse_once;
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
