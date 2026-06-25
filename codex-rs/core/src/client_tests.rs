use super::AuthRequestTelemetryContext;
use super::ModelClient;
use super::PendingUnauthorizedRetry;
use super::UnauthorizedRecoveryExecution;
use super::X_CODEX_INSTALLATION_ID_HEADER;
use super::X_CODEX_PARENT_THREAD_ID_HEADER;
use super::X_CODEX_TURN_METADATA_HEADER;
use super::X_CODEX_WINDOW_ID_HEADER;
use super::X_OPENAI_SUBAGENT_HEADER;
use crate::AttestationContext;
use crate::AttestationProvider;
use crate::GenerateAttestationFuture;
use crate::client_common::Prompt;
use crate::config::ModelOffloadConfig;
use crate::local_offload::transform_request_for_local_offload;
use crate::responses_metadata::CodexResponsesMetadata;
use crate::responses_metadata::CodexResponsesRequestKind;
use crate::responses_metadata::CompactionTurnMetadata;
use crate::test_support::TestCodexResponsesRequestKind;
use crate::test_support::responses_metadata as test_responses_metadata;
use codex_analytics::CompactionImplementation;
use codex_analytics::CompactionPhase;
use codex_analytics::CompactionReason;
use codex_analytics::CompactionTrigger;
use codex_api::ApiError;
use codex_api::Compression;
use codex_api::ResponseEvent;
use codex_app_server_protocol::AuthMode;
use codex_config::config_toml::ModelOffloadCompactionPolicy;
use codex_login::AuthManager;
use codex_login::CodexAuth;
use codex_model_provider::BearerAuthProvider;
use codex_model_provider_info::CHATGPT_CODEX_BASE_URL;
use codex_model_provider_info::ModelProviderInfo;
use codex_model_provider_info::WireApi;
use codex_model_provider_info::create_oss_provider_with_base_url;
use codex_otel::SessionTelemetry;
use codex_protocol::ThreadId;
use codex_protocol::config_types::ModelOffloadCompactionRuntimeOverride;
use codex_protocol::config_types::ModelOffloadRuntimeOverride;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::protocol::InternalSessionSource;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::SubAgentSource;
use codex_rollout_trace::ExecutionStatus;
use codex_rollout_trace::InferenceTraceAttempt;
use codex_rollout_trace::InferenceTraceContext;
use codex_rollout_trace::RawTraceEventPayload;
use codex_rollout_trace::RolloutTrace;
use codex_rollout_trace::TraceWriter;
use codex_rollout_trace::replay_bundle;
use codex_tools::ResponsesApiNamespace;
use codex_tools::ResponsesApiNamespaceTool;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolSpec;
use futures::StreamExt;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::collections::BTreeMap;
use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::task::Context;
use std::task::Poll;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::Notify;
use tracing::Event;
use tracing::Subscriber;
use tracing::field::Visit;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context as LayerContext;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;

const TEST_INSTALLATION_ID: &str = "11111111-1111-4111-8111-111111111111";

fn test_model_client(session_source: SessionSource) -> ModelClient {
    let provider = create_oss_provider_with_base_url("https://example.com/v1", WireApi::Responses);
    let thread_id = ThreadId::new();
    ModelClient::new(
        /*auth_manager*/ None,
        thread_id,
        provider,
        session_source,
        /*model_verbosity*/ None,
        /*enable_request_compression*/ false,
        /*include_timing_metrics*/ false,
        /*beta_features_header*/ None,
        /*attestation_provider*/ None,
        crate::config::ModelOffloadConfig::default(),
    )
}

fn test_model_client_with_local_offload(session_source: SessionSource) -> ModelClient {
    test_model_client_with_local_offload_config(session_source, ModelOffloadCompactionPolicy::Local)
}

fn test_model_client_with_local_offload_config(
    session_source: SessionSource,
    compaction_policy: ModelOffloadCompactionPolicy,
) -> ModelClient {
    let primary_provider =
        ModelProviderInfo::create_openai_provider(Some(CHATGPT_CODEX_BASE_URL.to_string()));
    let local_provider =
        create_oss_provider_with_base_url("http://127.0.0.1:11434/v1", WireApi::Responses);
    ModelClient::new(
        Some(AuthManager::from_auth_for_testing(
            CodexAuth::create_dummy_chatgpt_auth_for_testing(),
        )),
        ThreadId::new(),
        primary_provider,
        session_source,
        /*model_verbosity*/ None,
        /*enable_request_compression*/ false,
        /*include_timing_metrics*/ false,
        /*beta_features_header*/ None,
        /*attestation_provider*/ None,
        ModelOffloadConfig {
            enabled: true,
            runtime_override: None,
            compaction_runtime_override: None,
            provider_id: Some("local".to_string()),
            provider: Some(local_provider),
            model: Some("local-responses-model".to_string()),
            compaction_policy,
            compaction_recovery: crate::config::ModelOffloadCompactionRecoveryConfig::default(),
            context: Default::default(),
        },
    )
}

fn test_responses_metadata_for_client(
    client: &ModelClient,
    turn_id: Option<&str>,
    window_id: String,
    parent_thread_id: Option<ThreadId>,
    request_kind: TestCodexResponsesRequestKind,
) -> CodexResponsesMetadata {
    let thread_id = client.state.thread_id.to_string();
    test_responses_metadata(
        TEST_INSTALLATION_ID,
        &thread_id,
        &thread_id,
        turn_id,
        window_id,
        &client.state.session_source,
        parent_thread_id,
        request_kind,
    )
}

fn compaction_responses_metadata_for_client(
    client: &ModelClient,
    implementation: CompactionImplementation,
) -> CodexResponsesMetadata {
    let mut metadata = CodexResponsesMetadata::new(
        TEST_INSTALLATION_ID.to_string(),
        client.state.thread_id.to_string(),
        client.state.thread_id.to_string(),
        format!("{}:0", client.state.thread_id),
    );
    metadata.turn_id = Some("turn-compact".to_string());
    metadata.request_kind = Some(CodexResponsesRequestKind::Compaction(
        CompactionTurnMetadata::new(
            CompactionTrigger::Manual,
            CompactionReason::UserRequested,
            implementation,
            CompactionPhase::StandaloneTurn,
        ),
    ));
    metadata
}

fn test_model_info() -> ModelInfo {
    serde_json::from_value(json!({
        "slug": "gpt-test",
        "display_name": "gpt-test",
        "description": "desc",
        "default_reasoning_level": "medium",
        "supported_reasoning_levels": [
            {"effort": "medium", "description": "medium"}
        ],
        "shell_type": "shell_command",
        "visibility": "list",
        "supported_in_api": true,
        "priority": 1,
        "upgrade": null,
        "base_instructions": "base instructions",
        "model_messages": null,
        "supports_reasoning_summaries": false,
        "support_verbosity": false,
        "default_verbosity": null,
        "apply_patch_tool_type": null,
        "truncation_policy": {"mode": "bytes", "limit": 10000},
        "supports_parallel_tool_calls": false,
        "supports_image_detail_original": false,
        "context_window": 272000,
        "auto_compact_token_limit": null,
        "experimental_supported_tools": []
    }))
    .expect("deserialize test model info")
}

fn test_web_run_namespace_tool() -> ToolSpec {
    ToolSpec::Namespace(ResponsesApiNamespace {
        name: "web".to_string(),
        description: "Web tools.".to_string(),
        tools: vec![ResponsesApiNamespaceTool::Function(ResponsesApiTool {
            name: "run".to_string(),
            description: "Run web commands.".to_string(),
            strict: false,
            defer_loading: None,
            parameters: serde_json::from_value(json!({"type": "object"})).expect("valid schema"),
            output_schema: None,
        })],
    })
}

fn test_session_telemetry() -> SessionTelemetry {
    SessionTelemetry::new(
        ThreadId::new(),
        "gpt-test",
        "gpt-test",
        /*account_id*/ None,
        /*account_email*/ None,
        /*auth_mode*/ None,
        "test-originator".to_string(),
        /*log_user_prompts*/ false,
        "test-terminal".to_string(),
        SessionSource::Cli,
    )
}

#[derive(Default)]
struct TagCollectorVisitor {
    tags: BTreeMap<String, String>,
}

impl Visit for TagCollectorVisitor {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.tags
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.tags
            .insert(field.name().to_string(), format!("{value:?}"));
    }
}

#[derive(Clone)]
struct TagCollectorLayer {
    tags: Arc<Mutex<BTreeMap<String, String>>>,
}

impl<S> Layer<S> for TagCollectorLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &Event<'_>, _ctx: LayerContext<'_, S>) {
        if event.metadata().target() != "feedback_tags" {
            return;
        }
        let mut visitor = TagCollectorVisitor::default();
        event.record(&mut visitor);
        self.tags.lock().unwrap().extend(visitor.tags);
    }
}

fn started_inference_attempt(temp: &TempDir) -> anyhow::Result<InferenceTraceAttempt> {
    let writer = Arc::new(TraceWriter::create(
        temp.path(),
        "trace-1".to_string(),
        "rollout-1".to_string(),
        "thread-root".to_string(),
    )?);
    writer.append(RawTraceEventPayload::ThreadStarted {
        thread_id: "thread-root".to_string(),
        agent_path: "/root".to_string(),
        metadata_payload: None,
    })?;
    writer.append(RawTraceEventPayload::CodexTurnStarted {
        codex_turn_id: "turn-1".to_string(),
        thread_id: "thread-root".to_string(),
    })?;

    let inference_trace = InferenceTraceContext::enabled(
        writer,
        "thread-root".to_string(),
        "turn-1".to_string(),
        "gpt-test".to_string(),
        "test-provider".to_string(),
    );
    let attempt = inference_trace.start_attempt();
    attempt.record_started(&json!({
        "model": "gpt-test",
        "input": [{
            "type": "message",
            "role": "user",
            "content": [{"type": "input_text", "text": "hello"}]
        }],
    }));
    Ok(attempt)
}

fn output_message(id: &str, text: &str) -> ResponseItem {
    ResponseItem::Message {
        id: Some(id.to_string()),
        role: "assistant".to_string(),
        content: vec![ContentItem::OutputText {
            text: text.to_string(),
        }],
        phase: None,
        metadata: None,
    }
}

async fn replay_until_cancelled(temp: &TempDir) -> anyhow::Result<RolloutTrace> {
    let mut rollout = replay_bundle(temp.path())?;
    for _ in 0..50 {
        let inference = rollout
            .inference_calls
            .values()
            .next()
            .expect("inference should be reduced");
        if inference.execution.status == ExecutionStatus::Cancelled {
            return Ok(rollout);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
        rollout = replay_bundle(temp.path())?;
    }
    Ok(rollout)
}

struct NotifyAfterEventStream {
    events: VecDeque<ResponseEvent>,
    yielded: usize,
    notify_after: usize,
    notify: Arc<Notify>,
}

impl futures::Stream for NotifyAfterEventStream {
    type Item = std::result::Result<ResponseEvent, ApiError>;

    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let Some(event) = self.events.pop_front() else {
            return Poll::Pending;
        };
        self.yielded += 1;
        if self.yielded == self.notify_after {
            self.notify.notify_one();
        }
        Poll::Ready(Some(Ok(event)))
    }
}

#[test]
fn build_subagent_headers_sets_other_subagent_label() {
    let client = test_model_client(SessionSource::SubAgent(SubAgentSource::Other(
        "memory_consolidation".to_string(),
    )));
    let headers = client.build_subagent_headers();
    let value = headers
        .get(X_OPENAI_SUBAGENT_HEADER)
        .and_then(|value| value.to_str().ok());
    assert_eq!(value, Some("memory_consolidation"));
}

#[test]
fn build_subagent_headers_sets_internal_memory_consolidation_label() {
    let client = test_model_client(SessionSource::Internal(
        InternalSessionSource::MemoryConsolidation,
    ));
    let headers = client.build_subagent_headers();
    let value = headers
        .get(X_OPENAI_SUBAGENT_HEADER)
        .and_then(|value| value.to_str().ok());
    assert_eq!(value, Some("memory_consolidation"));
}

#[test]
fn build_ws_client_metadata_includes_window_lineage_and_turn_metadata() {
    let parent_thread_id = ThreadId::new();
    let client = test_model_client(SessionSource::SubAgent(SubAgentSource::ThreadSpawn {
        parent_thread_id,
        depth: 2,
        agent_path: None,
        agent_nickname: None,
        agent_role: None,
    }));

    let thread_id = client.state.thread_id.to_string();
    let expected_window_id = format!("{thread_id}:1");
    let responses_metadata = test_responses_metadata_for_client(
        &client,
        Some("turn-123"),
        expected_window_id.clone(),
        Some(parent_thread_id),
        TestCodexResponsesRequestKind::Turn,
    );
    let client_metadata =
        client.build_ws_client_metadata(&responses_metadata, /*use_responses_lite*/ false);
    let parent_thread_id = parent_thread_id.to_string();
    let turn_metadata: serde_json::Value = serde_json::from_str(
        client_metadata
            .get(X_CODEX_TURN_METADATA_HEADER)
            .expect("turn metadata"),
    )
    .expect("valid turn metadata");
    for (client_key, metadata_key, expected) in [
        (
            X_CODEX_INSTALLATION_ID_HEADER,
            "installation_id",
            "11111111-1111-4111-8111-111111111111",
        ),
        ("session_id", "session_id", thread_id.as_str()),
        ("thread_id", "thread_id", thread_id.as_str()),
        ("turn_id", "turn_id", "turn-123"),
        (
            X_CODEX_WINDOW_ID_HEADER,
            "window_id",
            expected_window_id.as_str(),
        ),
        (
            X_CODEX_PARENT_THREAD_ID_HEADER,
            "parent_thread_id",
            parent_thread_id.as_str(),
        ),
    ] {
        assert_eq!(
            client_metadata.get(client_key).map(String::as_str),
            Some(expected)
        );
        assert_eq!(turn_metadata[metadata_key].as_str(), Some(expected));
    }
    assert_eq!(
        client_metadata
            .get(X_OPENAI_SUBAGENT_HEADER)
            .map(String::as_str),
        Some("collab_spawn")
    );
}

#[tokio::test]
async fn summarize_memories_returns_empty_for_empty_input() {
    let client = test_model_client(SessionSource::Cli);
    let model_info = test_model_info();
    let session_telemetry = test_session_telemetry();

    let output = client
        .summarize_memories(
            Vec::new(),
            &model_info,
            /*effort*/ None,
            &session_telemetry,
        )
        .await
        .expect("empty summarize request should succeed");
    assert_eq!(output.len(), 0);
}

#[tokio::test]
async fn dropped_response_stream_traces_cancelled_partial_output() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let attempt = started_inference_attempt(&temp)?;

    // The provider has produced one complete output item, but no terminal
    // response.completed event. The harness has enough information to keep this
    // item in history, so the trace should preserve it when the stream is
    // abandoned.
    let item = output_message("msg-1", "partial answer");
    let api_stream = futures::stream::iter([Ok(ResponseEvent::OutputItemDone(item))])
        .chain(futures::stream::pending());
    let (mut stream, _) = super::map_response_events(
        /*upstream_request_id*/ None,
        api_stream,
        test_session_telemetry(),
        attempt,
        None,
    );

    let observed = stream
        .next()
        .await
        .expect("mapped stream should yield output item")?;
    assert!(matches!(observed, ResponseEvent::OutputItemDone(_)));

    // Dropping the consumer is how turn interruption/preemption stops polling
    // the provider stream. The mapper task observes that drop asynchronously
    // and records cancellation using the output items it has already seen.
    drop(stream);

    // Cancellation is recorded by the mapper task after Drop wakes it, so the
    // replay may need a short wait before the terminal event appears on disk.
    let rollout = replay_until_cancelled(&temp).await?;
    let inference = rollout
        .inference_calls
        .values()
        .next()
        .expect("inference should be reduced");

    assert_eq!(inference.execution.status, ExecutionStatus::Cancelled);
    assert_eq!(inference.response_item_ids.len(), 1);
    assert_eq!(rollout.raw_payloads.len(), 2);

    Ok(())
}

#[tokio::test]
async fn response_stream_records_last_model_feedback_ids() {
    let tags = Arc::new(Mutex::new(BTreeMap::new()));
    let _guard = tracing_subscriber::registry()
        .with(TagCollectorLayer { tags: tags.clone() })
        .set_default();

    let api_stream = futures::stream::iter([
        Ok(ResponseEvent::Created),
        Ok(ResponseEvent::Completed {
            response_id: "resp-123".to_string(),
            token_usage: None,
            end_turn: Some(true),
        }),
    ]);
    let (mut stream, _) = super::map_response_events(
        Some("req-123".to_string()),
        api_stream,
        test_session_telemetry(),
        InferenceTraceAttempt::disabled(),
        None,
    );

    while stream.next().await.is_some() {}

    let tags = tags.lock().unwrap().clone();
    assert_eq!(
        tags.get("last_model_request_id").map(String::as_str),
        Some("\"req-123\"")
    );
    assert_eq!(
        tags.get("last_model_response_id").map(String::as_str),
        Some("\"resp-123\"")
    );
}

#[tokio::test]
async fn dropped_backpressured_response_stream_traces_cancelled_partial_output()
-> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let attempt = started_inference_attempt(&temp)?;
    let backpressured_item_yielded = Arc::new(Notify::new());
    let mut events = VecDeque::new();
    for _ in 0..super::RESPONSE_STREAM_CHANNEL_CAPACITY {
        events.push_back(ResponseEvent::Created);
    }
    events.push_back(ResponseEvent::OutputItemDone(output_message(
        "msg-1",
        "partial answer",
    )));
    let api_stream = NotifyAfterEventStream {
        events,
        yielded: 0,
        notify_after: super::RESPONSE_STREAM_CHANNEL_CAPACITY + 1,
        notify: Arc::clone(&backpressured_item_yielded),
    };

    let (stream, _) = super::map_response_events(
        /*upstream_request_id*/ None,
        api_stream,
        test_session_telemetry(),
        attempt,
        None,
    );

    // Fill the mapper channel with non-terminal events, then yield one output
    // item. The mapper has observed that item and is blocked trying to send it
    // downstream, so dropping the consumer covers the send-failure path rather
    // than the `consumer_dropped` select branch.
    backpressured_item_yielded.notified().await;
    drop(stream);

    let rollout = replay_until_cancelled(&temp).await?;
    let inference = rollout
        .inference_calls
        .values()
        .next()
        .expect("inference should be reduced");

    assert_eq!(inference.execution.status, ExecutionStatus::Cancelled);
    assert_eq!(inference.response_item_ids.len(), 1);
    assert_eq!(rollout.raw_payloads.len(), 2);

    Ok(())
}

#[test]
fn auth_request_telemetry_context_tracks_attached_auth_and_retry_phase() {
    let auth_context = AuthRequestTelemetryContext::new(
        Some(AuthMode::Chatgpt),
        &BearerAuthProvider::for_test(Some("access-token"), Some("workspace-123")),
        PendingUnauthorizedRetry::from_recovery(UnauthorizedRecoveryExecution {
            mode: "managed",
            phase: "refresh_token",
        }),
    );

    assert_eq!(auth_context.auth_mode, Some("Chatgpt"));
    assert!(auth_context.auth_header_attached);
    assert_eq!(auth_context.auth_header_name, Some("authorization"));
    assert!(auth_context.retry_after_unauthorized);
    assert_eq!(auth_context.recovery_mode, Some("managed"));
    assert_eq!(auth_context.recovery_phase, Some("refresh_token"));
}

fn model_client_with_counting_attestation(
    include_attestation: bool,
) -> (ModelClient, Arc<AtomicUsize>) {
    #[derive(Debug)]
    struct CountingAttestationProvider {
        calls: Arc<AtomicUsize>,
    }

    impl AttestationProvider for CountingAttestationProvider {
        fn header_for_request(
            &self,
            _context: AttestationContext,
        ) -> GenerateAttestationFuture<'_> {
            let calls = self.calls.clone();
            Box::pin(async move {
                let call = calls.fetch_add(1, Ordering::Relaxed) + 1;
                Some(http::HeaderValue::from_bytes(format!("v1.header-{call}").as_bytes()).unwrap())
            })
        }
    }

    let attestation_calls = Arc::new(AtomicUsize::new(0));
    let (auth_manager, provider) = if include_attestation {
        (
            Some(AuthManager::from_auth_for_testing(
                CodexAuth::create_dummy_chatgpt_auth_for_testing(),
            )),
            ModelProviderInfo::create_openai_provider(Some(CHATGPT_CODEX_BASE_URL.to_string())),
        )
    } else {
        (
            None,
            create_oss_provider_with_base_url("https://example.com/v1", WireApi::Responses),
        )
    };
    let model_client = ModelClient::new(
        auth_manager,
        ThreadId::new(),
        provider,
        SessionSource::Exec,
        /*model_verbosity*/ None,
        /*enable_request_compression*/ false,
        /*include_timing_metrics*/ false,
        /*beta_features_header*/ None,
        Some(Arc::new(CountingAttestationProvider {
            calls: attestation_calls.clone(),
        })),
        crate::config::ModelOffloadConfig::default(),
    );
    (model_client, attestation_calls)
}

#[tokio::test]
async fn websocket_handshake_includes_attestation_for_chatgpt_codex_responses() {
    let (model_client, attestation_calls) =
        model_client_with_counting_attestation(/*include_attestation*/ true);
    let responses_metadata = test_responses_metadata_for_client(
        &model_client,
        /*turn_id*/ None,
        format!("{}:0", model_client.state.thread_id),
        /*parent_thread_id*/ None,
        TestCodexResponsesRequestKind::WebsocketConnection,
    );

    let headers = model_client
        .build_websocket_headers(&responses_metadata)
        .await;

    assert_eq!(
        headers
            .get(crate::attestation::X_OAI_ATTESTATION_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some("v1.header-1"),
    );
    assert_eq!(attestation_calls.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn non_chatgpt_codex_endpoints_omit_attestation_generation() {
    let (model_client, attestation_calls) =
        model_client_with_counting_attestation(/*include_attestation*/ false);
    let mut response_headers = http::HeaderMap::new();

    if let Some(header_value) = model_client.generate_attestation_header_for().await {
        response_headers.insert(crate::attestation::X_OAI_ATTESTATION_HEADER, header_value);
    }
    let mut compaction_headers = http::HeaderMap::new();
    if let Some(header_value) = model_client.generate_attestation_header_for().await {
        compaction_headers.insert(crate::attestation::X_OAI_ATTESTATION_HEADER, header_value);
    }
    let mut realtime_headers = http::HeaderMap::new();
    if let Some(header_value) = model_client.generate_attestation_header_for().await {
        realtime_headers.insert(crate::attestation::X_OAI_ATTESTATION_HEADER, header_value);
    }

    assert_eq!(
        response_headers.get(crate::attestation::X_OAI_ATTESTATION_HEADER),
        None,
    );
    assert_eq!(
        compaction_headers.get(crate::attestation::X_OAI_ATTESTATION_HEADER),
        None,
    );
    assert_eq!(
        realtime_headers.get(crate::attestation::X_OAI_ATTESTATION_HEADER),
        None,
    );
    assert_eq!(attestation_calls.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn local_offload_responses_request_omits_codex_control_plane_metadata() {
    let primary_provider =
        ModelProviderInfo::create_openai_provider(Some(CHATGPT_CODEX_BASE_URL.to_string()));
    let local_provider =
        create_oss_provider_with_base_url("http://127.0.0.1:11434/v1", WireApi::Responses);
    let (attestation_client, attestation_calls) =
        model_client_with_counting_attestation(/*include_attestation*/ true);
    let client = ModelClient::new(
        Some(AuthManager::from_auth_for_testing(
            CodexAuth::create_dummy_chatgpt_auth_for_testing(),
        )),
        ThreadId::new(),
        primary_provider,
        SessionSource::Exec,
        /*model_verbosity*/ None,
        /*enable_request_compression*/ false,
        /*include_timing_metrics*/ false,
        Some("beta-feature".to_string()),
        attestation_client.state.attestation_provider.clone(),
        ModelOffloadConfig {
            enabled: true,
            runtime_override: None,
            compaction_runtime_override: None,
            provider_id: Some("local".to_string()),
            provider: Some(local_provider),
            model: Some("local-responses-model".to_string()),
            compaction_policy: ModelOffloadCompactionPolicy::Local,
            compaction_recovery: crate::config::ModelOffloadCompactionRecoveryConfig::default(),
            context: Default::default(),
        },
    );
    let client_session = client.new_session();
    let responses_metadata = test_responses_metadata_for_client(
        &client,
        Some("turn-1"),
        format!("{}:0", client.state.thread_id),
        Some(ThreadId::new()),
        TestCodexResponsesRequestKind::Turn,
    );

    let route = client.route_for_responses_request(&responses_metadata);
    assert!(route.is_local_offload());
    let client_setup = client
        .current_client_setup_for_route(route)
        .await
        .expect("local client setup resolves");
    let prompt = Prompt {
        input: vec![ResponseItem::FunctionCall {
            id: None,
            name: "run".to_string(),
            namespace: Some("web".to_string()),
            arguments: "{\"search_query\":[{\"q\":\"codex\"}]}".to_string(),
            call_id: "call_web".to_string(),
            metadata: None,
        }],
        tools: vec![
            test_web_run_namespace_tool(),
            ToolSpec::WebSearch {
                external_web_access: Some(true),
                filters: None,
                user_location: None,
                search_context_size: None,
                search_content_types: None,
            },
        ],
        ..Prompt::default()
    };
    let options = client_session
        .build_responses_options(
            &responses_metadata,
            Compression::None,
            /*use_responses_lite*/ false,
            !route.is_local_offload(),
        )
        .await;
    let request = client
        .build_responses_request(
            &client_setup.api_provider,
            client_setup.model_provider.info(),
            client_setup.model_override.as_deref(),
            &prompt,
            &test_model_info(),
            /*effort*/ None,
            codex_protocol::config_types::ReasoningSummary::Auto,
            /*service_tier*/ None,
            &responses_metadata,
            !route.is_local_offload(),
        )
        .expect("local responses request builds");
    let mut request = request;
    transform_request_for_local_offload(&mut request, &prompt.tools)
        .expect("local request transform succeeds");

    assert_eq!(request.model, "local-responses-model");
    assert!(request.client_metadata.is_none());
    assert_eq!(request.tools.len(), 1);
    assert_eq!(request.tools[0]["name"], "ns__web__run");
    assert_eq!(
        request.input,
        vec![ResponseItem::FunctionCall {
            id: None,
            name: "ns__web__run".to_string(),
            namespace: None,
            arguments: "{\"search_query\":[{\"q\":\"codex\"}]}".to_string(),
            call_id: "call_web".to_string(),
            metadata: None,
        }]
    );
    assert!(options.session_id.is_none());
    assert!(options.thread_id.is_none());
    assert!(options.session_source.is_none());
    assert!(options.turn_state.is_none());
    assert!(
        options
            .extra_headers
            .get(http::header::AUTHORIZATION)
            .is_none()
    );
    assert!(
        options
            .extra_headers
            .get(crate::attestation::X_OAI_ATTESTATION_HEADER)
            .is_none()
    );
    assert!(
        options
            .extra_headers
            .get(X_CODEX_INSTALLATION_ID_HEADER)
            .is_none()
    );
    assert!(
        options
            .extra_headers
            .get(X_CODEX_WINDOW_ID_HEADER)
            .is_none()
    );
    assert!(
        options
            .extra_headers
            .get(X_CODEX_TURN_METADATA_HEADER)
            .is_none()
    );
    assert!(
        options
            .extra_headers
            .get(X_CODEX_PARENT_THREAD_ID_HEADER)
            .is_none()
    );
    assert!(
        options
            .extra_headers
            .get(X_OPENAI_SUBAGENT_HEADER)
            .is_none()
    );
    assert_eq!(attestation_calls.load(Ordering::Relaxed), 0);
}

#[test]
fn no_offload_config_preserves_primary_turn_routing() {
    let client = test_model_client(SessionSource::Exec);
    let responses_metadata = test_responses_metadata_for_client(
        &client,
        Some("turn-1"),
        format!("{}:0", client.state.thread_id),
        None,
        TestCodexResponsesRequestKind::Turn,
    );

    assert!(
        !client
            .route_for_responses_request(&responses_metadata)
            .is_local_offload()
    );
    assert!(
        !client
            .new_session()
            .mark_offload_used_for_responses_request(&responses_metadata)
    );
    assert!(!client.offload_ever_used());
}

#[test]
fn internal_and_subagent_sources_stay_primary_with_offload_configured() {
    for session_source in [
        SessionSource::Internal(InternalSessionSource::MemoryConsolidation),
        SessionSource::SubAgent(SubAgentSource::Review),
        SessionSource::SubAgent(SubAgentSource::Other("guardian".to_string())),
    ] {
        let client = test_model_client_with_local_offload(session_source.clone());
        let responses_metadata = test_responses_metadata_for_client(
            &client,
            Some("turn-1"),
            format!("{}:0", client.state.thread_id),
            None,
            TestCodexResponsesRequestKind::Turn,
        );

        assert!(
            !client
                .route_for_responses_request(&responses_metadata)
                .is_local_offload(),
            "expected {session_source:?} to stay on the primary route",
        );
        assert!(
            !client
                .new_session()
                .mark_offload_used_for_responses_request(&responses_metadata)
        );
        assert!(!client.offload_ever_used());
    }
}

async fn request_model_for_metadata(
    client: &ModelClient,
    responses_metadata: &CodexResponsesMetadata,
) -> String {
    let route = client.route_for_responses_request(responses_metadata);
    let client_setup = client
        .current_client_setup_for_request(route, Some(responses_metadata))
        .await
        .expect("client setup resolves");
    client
        .build_responses_request(
            &client_setup.api_provider,
            client_setup.model_provider.info(),
            client_setup.model_override.as_deref(),
            &Prompt::default(),
            &test_model_info(),
            /*effort*/ None,
            codex_protocol::config_types::ReasoningSummary::Auto,
            /*service_tier*/ None,
            responses_metadata,
            !route.is_local_offload(),
        )
        .expect("responses request builds")
        .model
}

#[tokio::test]
async fn offload_primary_compaction_uses_current_primary_model() {
    let client = test_model_client_with_local_offload_config(
        SessionSource::Exec,
        ModelOffloadCompactionPolicy::Primary,
    );

    for implementation in [
        CompactionImplementation::ResponsesCompact,
        CompactionImplementation::ResponsesCompactionV2,
    ] {
        let responses_metadata = compaction_responses_metadata_for_client(&client, implementation);
        assert!(
            !client
                .route_for_responses_request(&responses_metadata)
                .is_local_offload()
        );
        assert_eq!(
            request_model_for_metadata(&client, &responses_metadata).await,
            "gpt-test"
        );
    }
}

#[tokio::test]
async fn runtime_force_off_routes_eligible_turn_primary() {
    let client = test_model_client_with_local_offload_config(
        SessionSource::Exec,
        ModelOffloadCompactionPolicy::Primary,
    );
    client
        .set_model_offload_runtime_override(Some(ModelOffloadRuntimeOverride::ForceOff))
        .unwrap();
    let responses_metadata = test_responses_metadata_for_client(
        &client,
        Some("turn-1"),
        format!("{}:0", client.state.thread_id),
        None,
        TestCodexResponsesRequestKind::Turn,
    );

    assert!(
        !client
            .route_for_responses_request(&responses_metadata)
            .is_local_offload()
    );
    assert_eq!(
        request_model_for_metadata(&client, &responses_metadata).await,
        "gpt-test"
    );
}

#[tokio::test]
async fn compaction_recovery_stays_primary_with_offload_configured() {
    let client = test_model_client_with_local_offload(SessionSource::Exec);
    client.seed_offload_ever_used(true);
    let mut responses_metadata = test_responses_metadata_for_client(
        &client,
        Some("turn-1"),
        format!("{}:0", client.state.thread_id),
        None,
        TestCodexResponsesRequestKind::Turn,
    );
    responses_metadata.request_kind = Some(CodexResponsesRequestKind::CompactionRecovery);

    let route = client.route_for_responses_request(&responses_metadata);

    assert!(!route.is_local_offload());
    assert_eq!(
        request_model_for_metadata(&client, &responses_metadata).await,
        "gpt-test"
    );
}

#[tokio::test]
async fn runtime_force_off_does_not_clear_offload_ever_used() {
    let client = test_model_client_with_local_offload(SessionSource::Exec);
    client.seed_offload_ever_used(true);

    client
        .set_model_offload_runtime_override(Some(ModelOffloadRuntimeOverride::ForceOff))
        .unwrap();

    assert!(!client.local_offload_enabled_for_turns());
    assert!(client.offload_ever_used());
}

#[tokio::test]
async fn runtime_force_on_requires_resolved_offload_provider() {
    let client = test_model_client(SessionSource::Exec);

    let err = client
        .set_model_offload_runtime_override(Some(ModelOffloadRuntimeOverride::ForceOn))
        .expect_err("force_on without a local provider must fail");
    assert!(
        err.to_string()
            .contains("model_offload.provider is not configured or invalid"),
        "unexpected error: {err}"
    );

    client
        .set_model_offload_runtime_override(Some(ModelOffloadRuntimeOverride::ForceOff))
        .expect("force_off remains allowed");
    client
        .set_model_offload_runtime_override(None)
        .expect("clearing override remains allowed");
}

#[tokio::test]
async fn runtime_force_on_routes_eligible_turn_local() {
    let client = test_model_client_with_local_offload_config(
        SessionSource::Exec,
        ModelOffloadCompactionPolicy::Primary,
    );
    client
        .set_model_offload_runtime_override(Some(ModelOffloadRuntimeOverride::ForceOn))
        .unwrap();
    let responses_metadata = test_responses_metadata_for_client(
        &client,
        Some("turn-1"),
        format!("{}:0", client.state.thread_id),
        None,
        TestCodexResponsesRequestKind::Turn,
    );

    assert!(
        client
            .route_for_responses_request(&responses_metadata)
            .is_local_offload()
    );
    assert_eq!(
        request_model_for_metadata(&client, &responses_metadata).await,
        "local-responses-model"
    );
}

#[tokio::test]
async fn local_turn_uses_offload_model() {
    let client = test_model_client_with_local_offload_config(
        SessionSource::Exec,
        ModelOffloadCompactionPolicy::Primary,
    );
    let responses_metadata = test_responses_metadata_for_client(
        &client,
        Some("turn-1"),
        format!("{}:0", client.state.thread_id),
        None,
        TestCodexResponsesRequestKind::Turn,
    );

    assert!(
        client
            .route_for_responses_request(&responses_metadata)
            .is_local_offload()
    );
    assert_eq!(
        request_model_for_metadata(&client, &responses_metadata).await,
        "local-responses-model"
    );
}

#[test]
fn turn_session_force_primary_disables_local_offload_for_turn_only() {
    let client = test_model_client_with_local_offload_config(
        SessionSource::Exec,
        ModelOffloadCompactionPolicy::Primary,
    );
    let client_session = client.new_session();
    let responses_metadata = test_responses_metadata_for_client(
        &client,
        Some("turn-1"),
        format!("{}:0", client.state.thread_id),
        None,
        TestCodexResponsesRequestKind::Turn,
    );

    assert!(client_session.local_offload_enabled_for_turns());
    assert!(
        client_session
            .route_for_responses_request(&responses_metadata)
            .is_local_offload()
    );

    client_session.force_primary_for_responses_requests();

    assert!(!client_session.local_offload_enabled_for_turns());
    assert!(
        !client_session
            .route_for_responses_request(&responses_metadata)
            .is_local_offload()
    );
    assert!(client.local_offload_enabled_for_turns());
}

#[test]
fn turn_session_force_primary_does_not_mark_offload_used() {
    let client = test_model_client_with_local_offload_config(
        SessionSource::Exec,
        ModelOffloadCompactionPolicy::Primary,
    );
    let client_session = client.new_session();
    let responses_metadata = test_responses_metadata_for_client(
        &client,
        Some("turn-1"),
        format!("{}:0", client.state.thread_id),
        None,
        TestCodexResponsesRequestKind::Turn,
    );

    client_session.force_primary_for_responses_requests();

    assert!(!client_session.mark_offload_used_for_responses_request(&responses_metadata));
    assert!(!client.offload_ever_used());
}

#[tokio::test]
async fn offload_local_compaction_policy_uses_local_only_when_effectively_enabled() {
    let client = test_model_client_with_local_offload_config(
        SessionSource::Exec,
        ModelOffloadCompactionPolicy::Local,
    );
    client.seed_offload_ever_used(true);
    let responses_metadata = compaction_responses_metadata_for_client(
        &client,
        CompactionImplementation::ResponsesCompactionV2,
    );

    assert!(
        client
            .route_for_responses_request(&responses_metadata)
            .is_local_offload()
    );
    assert_eq!(
        request_model_for_metadata(&client, &responses_metadata).await,
        "local-responses-model"
    );

    client
        .set_model_offload_runtime_override(Some(ModelOffloadRuntimeOverride::ForceOff))
        .unwrap();
    assert!(
        !client
            .route_for_responses_request(&responses_metadata)
            .is_local_offload()
    );
}

#[tokio::test]
async fn compaction_runtime_override_updates_effective_policy_with_offload_guard() {
    let client = test_model_client_with_local_offload_config(
        SessionSource::Exec,
        ModelOffloadCompactionPolicy::Local,
    );

    assert_eq!(
        client.effective_model_offload_compaction_policy(),
        ModelOffloadCompactionPolicy::Primary
    );

    client.seed_offload_ever_used(true);
    assert_eq!(
        client.effective_model_offload_compaction_policy(),
        ModelOffloadCompactionPolicy::Local
    );

    client
        .set_model_offload_compaction_runtime_override(Some(
            ModelOffloadCompactionRuntimeOverride::Primary,
        ))
        .unwrap();
    assert_eq!(
        client.effective_model_offload_compaction_policy(),
        ModelOffloadCompactionPolicy::Primary
    );

    client
        .set_model_offload_compaction_runtime_override(Some(
            ModelOffloadCompactionRuntimeOverride::Local,
        ))
        .unwrap();
    assert_eq!(
        client.effective_model_offload_compaction_policy(),
        ModelOffloadCompactionPolicy::Local
    );

    client
        .set_model_offload_runtime_override(Some(ModelOffloadRuntimeOverride::ForceOff))
        .unwrap();
    assert_eq!(
        client.effective_model_offload_compaction_policy(),
        ModelOffloadCompactionPolicy::Primary
    );
}
