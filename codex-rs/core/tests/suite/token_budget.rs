use anyhow::Result;
use codex_core::config::SessionTokenBudgetConfig;
use codex_features::Feature;
use codex_model_provider_info::built_in_model_providers;
use codex_protocol::protocol::AgentStatus;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::TurnAbortReason;
use codex_protocol::user_input::UserInput;
use core_test_support::PathBufExt;
use core_test_support::context_snapshot;
use core_test_support::context_snapshot::ContextSnapshotOptions;
use core_test_support::responses::ResponsesRequest;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_completed_with_tokens;
use core_test_support::responses::ev_function_call;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_sse_once;
use core_test_support::responses::mount_sse_once_match;
use core_test_support::responses::mount_sse_sequence;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::local;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use std::time::Duration;
use tokio::time::sleep;
use tokio::time::timeout;

const CONFIGURED_CONTEXT_WINDOW: i64 = 128_000;
const EFFECTIVE_CONTEXT_WINDOW: i64 = CONFIGURED_CONTEXT_WINDOW * 95 / 100;
const SESSION_TOKEN_BUDGET: SessionTokenBudgetConfig = SessionTokenBudgetConfig {
    limit_tokens: 100,
    reminder_interval_tokens: 25,
};

fn token_budget_texts(request: &ResponsesRequest) -> Vec<String> {
    request
        .message_input_texts("developer")
        .into_iter()
        .filter(|text| text.starts_with("<token_budget>"))
        .collect()
}

fn session_token_budget_texts(request: &ResponsesRequest) -> Vec<String> {
    request
        .message_input_texts("developer")
        .into_iter()
        .filter(|text| text.starts_with("<session_token_budget>"))
        .collect()
}

fn tool_names(request: &ResponsesRequest) -> Vec<String> {
    request
        .body_json()
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|tool| tool.get("name").and_then(Value::as_str).map(str::to_string))
        .collect()
}

fn wire_request_contains(request: &wiremock::Request, text: &str) -> bool {
    std::str::from_utf8(&request.body).is_ok_and(|body| body.contains(text))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_token_budget_adds_stable_declaration_and_periodic_reminders() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let responses = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-1"),
                ev_completed_with_tokens("resp-1", /*total_tokens*/ 30),
            ]),
            sse(vec![ev_response_created("resp-2"), ev_completed("resp-2")]),
        ],
    )
    .await;
    let test = test_codex()
        .with_config(|config| {
            config.model_context_window = Some(CONFIGURED_CONTEXT_WINDOW);
            config
                .features
                .enable(Feature::TokenBudget)
                .expect("test config should allow token budget");
            config.session_token_budget = Some(SESSION_TOKEN_BUDGET);
        })
        .build(&server)
        .await?;

    test.submit_turn("first turn").await?;
    test.submit_turn("second turn").await?;

    let requests = responses.requests();
    let declaration = "<session_token_budget>\nThis session has a shared token budget of 100 tokens across all threads.\n</session_token_budget>".to_string();
    let initial_remaining = "<session_token_budget>\nYou have 100 tokens left in the shared session token budget.\n</session_token_budget>".to_string();
    let remaining = "<session_token_budget>\nYou have 70 tokens left in the shared session token budget.\n</session_token_budget>".to_string();
    assert_eq!(
        session_token_budget_texts(&requests[0]),
        vec![declaration.clone(), initial_remaining.clone()]
    );
    let initial_developer_group = requests[0]
        .message_input_text_groups("developer")
        .into_iter()
        .find(|group| group.contains(&declaration))
        .expect("session token budget declaration should be model-visible");
    let declaration_index = initial_developer_group
        .iter()
        .position(|text| text == &declaration)
        .expect("developer group should contain the declaration");
    let context_budget_index = initial_developer_group
        .iter()
        .position(|text| text.starts_with("<token_budget>"))
        .expect("developer group should contain the context budget");
    assert!(
        declaration_index < context_budget_index,
        "the stable session declaration should precede changing context-window metadata"
    );
    assert_eq!(
        session_token_budget_texts(&requests[1]),
        vec![declaration, initial_remaining, remaining]
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_token_budget_exhaustion_uses_existing_interrupt_path() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let response = mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("resp-1"),
            ev_completed_with_tokens("resp-1", /*total_tokens*/ 30),
        ]),
    )
    .await;
    let test = test_codex()
        .with_config(|config| {
            config.session_token_budget = Some(SessionTokenBudgetConfig {
                limit_tokens: 30,
                reminder_interval_tokens: 10,
            });
        })
        .build(&server)
        .await?;

    test.codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "use the budget".to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: Default::default(),
        })
        .await?;

    let event = wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnAborted(_))
    })
    .await;
    let EventMsg::TurnAborted(event) = event else {
        unreachable!("event filter only accepts TurnAborted")
    };
    assert_eq!(event.reason, TurnAbortReason::Interrupted);
    response.single_request();

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subagent_usage_draws_from_the_shared_session_token_budget() -> Result<()> {
    skip_if_no_network!(Ok(()));

    const ROOT_PROMPT: &str = "spawn a budget worker";
    const CHILD_PROMPT: &str = "consume child budget";
    const FOLLOW_UP_PROMPT: &str = "report the shared budget";
    const SPAWN_CALL_ID: &str = "spawn-budget-worker";

    let server = start_mock_server().await;
    let spawn_args = json!({
        "message": CHILD_PROMPT,
        "task_name": "budget_worker",
    })
    .to_string();
    mount_sse_once_match(
        &server,
        |request: &wiremock::Request| wire_request_contains(request, ROOT_PROMPT),
        sse(vec![
            ev_response_created("root-1"),
            ev_function_call(SPAWN_CALL_ID, "spawn_agent", &spawn_args),
            ev_completed_with_tokens("root-1", /*total_tokens*/ 10),
        ]),
    )
    .await;
    mount_sse_once_match(
        &server,
        |request: &wiremock::Request| {
            wire_request_contains(request, CHILD_PROMPT)
                && !wire_request_contains(request, SPAWN_CALL_ID)
        },
        sse(vec![
            ev_response_created("child-1"),
            ev_completed_with_tokens("child-1", /*total_tokens*/ 30),
        ]),
    )
    .await;
    mount_sse_once_match(
        &server,
        |request: &wiremock::Request| wire_request_contains(request, SPAWN_CALL_ID),
        sse(vec![
            ev_response_created("root-2"),
            ev_completed_with_tokens("root-2", /*total_tokens*/ 10),
        ]),
    )
    .await;
    let follow_up = mount_sse_once_match(
        &server,
        |request: &wiremock::Request| wire_request_contains(request, FOLLOW_UP_PROMPT),
        sse(vec![ev_response_created("root-3"), ev_completed("root-3")]),
    )
    .await;

    let test = test_codex()
        .with_config(|config| {
            config
                .features
                .enable(Feature::Collab)
                .expect("test config should allow multi-agent tools");
            config
                .features
                .enable(Feature::MultiAgentV2)
                .expect("test config should allow multi-agent v2");
            config.session_token_budget = Some(SESSION_TOKEN_BUDGET);
        })
        .build(&server)
        .await?;

    let mut created_threads = test.thread_manager.subscribe_thread_created();
    test.submit_turn(ROOT_PROMPT).await?;
    let child_thread_id = timeout(Duration::from_secs(10), created_threads.recv()).await??;
    let child_thread = test.thread_manager.get_thread(child_thread_id).await?;
    timeout(Duration::from_secs(10), async {
        while !matches!(child_thread.agent_status().await, AgentStatus::Completed(_)) {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    test.submit_turn(FOLLOW_UP_PROMPT).await?;

    let request = follow_up.single_request();
    assert_eq!(
        session_token_budget_texts(&request).last(),
        Some(
            &"<session_token_budget>\nYou have 50 tokens left in the shared session token budget.\n</session_token_budget>"
                .to_string()
        )
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn token_budget_context_is_only_emitted_with_full_context() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let responses = mount_sse_sequence(
        &server,
        vec![
            sse(vec![ev_response_created("resp-1"), ev_completed("resp-1")]),
            sse(vec![ev_response_created("resp-2"), ev_completed("resp-2")]),
        ],
    )
    .await;
    let test = test_codex()
        .with_config(|config| {
            config.model_context_window = Some(CONFIGURED_CONTEXT_WINDOW);
            config
                .features
                .enable(Feature::TokenBudget)
                .expect("test config should allow token budget");
        })
        .build(&server)
        .await?;

    test.submit_turn("first turn").await?;

    let second_cwd = test.workspace_path("second-cwd");
    std::fs::create_dir_all(&second_cwd)?;
    test.submit_turn_with_environments("second turn", Some(vec![local(second_cwd.abs())]))
        .await?;

    let requests = responses.requests();
    assert_eq!(requests.len(), 2);

    let thread_id = test.session_configured.thread_id;
    let expected = vec![format!(
        "<token_budget>\nThread id {thread_id}.\nCurrent context window 0.\nYou have {EFFECTIVE_CONTEXT_WINDOW} tokens left in this context window.\n</token_budget>"
    )];
    assert_eq!(
        token_budget_texts(&requests[0]),
        expected,
        "initial full context should report context window 0"
    );
    assert_eq!(
        token_budget_texts(&requests[1]),
        expected,
        "steady-state context update should not advance the context window"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn token_budget_remaining_context_emits_on_first_threshold_crossing() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let responses = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-1"),
                ev_completed_with_tokens("resp-1", /*total_tokens*/ 2_500),
            ]),
            sse(vec![
                ev_response_created("resp-2"),
                ev_completed_with_tokens("resp-2", /*total_tokens*/ 3_000),
            ]),
            sse(vec![
                ev_response_created("resp-3"),
                ev_completed_with_tokens("resp-3", /*total_tokens*/ 5_000),
            ]),
            sse(vec![
                ev_response_created("resp-4"),
                ev_completed_with_tokens("resp-4", /*total_tokens*/ 8_000),
            ]),
            sse(vec![ev_response_created("resp-5"), ev_completed("resp-5")]),
        ],
    )
    .await;
    let test = test_codex()
        .with_config(|config| {
            config.model_context_window = Some(10_000);
            config
                .features
                .enable(Feature::TokenBudget)
                .expect("test config should allow token budget");
        })
        .build(&server)
        .await?;

    for turn in 1..=5 {
        test.submit_turn(&format!("turn {turn}")).await?;
    }

    let requests = responses.requests();
    assert_eq!(requests.len(), 5);

    let thread_id = test.session_configured.thread_id;
    let full_context = format!(
        "<token_budget>\nThread id {thread_id}.\nCurrent context window 0.\nYou have 9500 tokens left in this context window.\n</token_budget>"
    );
    let threshold_25 =
        "<token_budget>\nYou have 7000 tokens left in this context window.\n</token_budget>"
            .to_string();
    let threshold_50 =
        "<token_budget>\nYou have 4500 tokens left in this context window.\n</token_budget>"
            .to_string();
    let threshold_75 =
        "<token_budget>\nYou have 1500 tokens left in this context window.\n</token_budget>"
            .to_string();

    assert_eq!(token_budget_texts(&requests[0]), vec![full_context.clone()]);
    assert_eq!(
        token_budget_texts(&requests[1]),
        vec![full_context.clone(), threshold_25.clone()]
    );
    assert_eq!(
        token_budget_texts(&requests[2]),
        vec![full_context.clone(), threshold_25.clone()]
    );
    assert_eq!(
        token_budget_texts(&requests[3]),
        vec![
            full_context.clone(),
            threshold_25.clone(),
            threshold_50.clone()
        ]
    );
    assert_eq!(
        token_budget_texts(&requests[4]),
        vec![full_context, threshold_25, threshold_50, threshold_75]
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_context_remaining_returns_token_budget_remaining_fragment() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let call_id = "remaining-call";
    let responses = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-1"),
                ev_assistant_message("msg-1", "noted"),
                ev_completed_with_tokens("resp-1", /*total_tokens*/ 2_500),
            ]),
            sse(vec![
                ev_response_created("resp-2"),
                ev_function_call(call_id, "get_context_remaining", "{}"),
                ev_completed_with_tokens("resp-2", /*total_tokens*/ 2_500),
            ]),
            sse(vec![
                ev_response_created("resp-3"),
                ev_assistant_message("msg-3", "done"),
                ev_completed("resp-3"),
            ]),
        ],
    )
    .await;
    let test = test_codex()
        .with_config(|config| {
            config.model_context_window = Some(10_000);
            config
                .features
                .enable(Feature::TokenBudget)
                .expect("test config should allow token budget");
        })
        .build(&server)
        .await?;

    test.submit_turn("spend some tokens").await?;
    test.submit_turn("check remaining context").await?;

    let requests = responses.requests();
    assert_eq!(requests.len(), 3);
    assert!(
        tool_names(&requests[1])
            .iter()
            .any(|name| name == "get_context_remaining"),
        "get_context_remaining should be exposed when token budget is enabled"
    );

    let thread_id = test.session_configured.thread_id;
    let full_context = format!(
        "<token_budget>\nThread id {thread_id}.\nCurrent context window 0.\nYou have 9500 tokens left in this context window.\n</token_budget>"
    );
    let remaining_context =
        "<token_budget>\nYou have 7000 tokens left in this context window.\n</token_budget>"
            .to_string();
    assert_eq!(
        token_budget_texts(&requests[1]),
        vec![full_context, remaining_context.clone()]
    );
    assert_eq!(
        requests[2].function_call_output_content_and_success(call_id),
        Some((Some(remaining_context), None))
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_context_remaining_returns_unknown_when_window_is_unavailable() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let call_id = "remaining-call";
    let responses = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-1"),
                ev_function_call(call_id, "get_context_remaining", "{}"),
                ev_completed("resp-1"),
            ]),
            sse(vec![
                ev_response_created("resp-2"),
                ev_assistant_message("msg-2", "done"),
                ev_completed("resp-2"),
            ]),
        ],
    )
    .await;
    let test = test_codex()
        .with_model_info_override("gpt-5.2", |model_info| {
            model_info.context_window = None;
            model_info.max_context_window = None;
        })
        .with_config(|config| {
            config.model_context_window = None;
            config
                .features
                .enable(Feature::TokenBudget)
                .expect("test config should allow token budget");
        })
        .build(&server)
        .await?;

    test.submit_turn("check remaining context").await?;

    let requests = responses.requests();
    assert_eq!(requests.len(), 2);
    assert!(
        tool_names(&requests[0])
            .iter()
            .any(|name| name == "get_context_remaining"),
        "get_context_remaining should be exposed when token budget is enabled"
    );

    assert_eq!(token_budget_texts(&requests[0]), Vec::<String>::new());
    assert_eq!(
        requests[1].function_call_output_content_and_success(call_id),
        Some((
            Some(
                "<token_budget>\nYou have unknown tokens left in this context window.\n</token_budget>"
                    .to_string()
            ),
            None,
        ))
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn token_budget_context_uses_new_window_after_compaction() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let responses = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-1"),
                ev_completed_with_tokens("resp-1", /*total_tokens*/ 20),
            ]),
            sse(vec![
                ev_response_created("resp-compact"),
                ev_assistant_message("msg-compact", "compact summary"),
                ev_completed_with_tokens("resp-compact", /*total_tokens*/ 10),
            ]),
            sse(vec![ev_response_created("resp-2"), ev_completed("resp-2")]),
        ],
    )
    .await;

    let mut model_provider = built_in_model_providers(/*openai_base_url*/ None)["openai"].clone();
    model_provider.name = "OpenAI-compatible test provider".to_string();
    model_provider.base_url = Some(format!("{}/v1", server.uri()));
    model_provider.supports_websockets = false;

    let test = test_codex()
        .with_config(move |config| {
            config.model_provider = model_provider;
            config.model_context_window = Some(CONFIGURED_CONTEXT_WINDOW);
            config
                .features
                .enable(Feature::TokenBudget)
                .expect("test config should allow token budget");
            config.session_token_budget = Some(SESSION_TOKEN_BUDGET);
        })
        .build(&server)
        .await?;

    test.submit_turn("before compact").await?;
    test.codex.submit(Op::Compact).await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    test.submit_turn("after compact").await?;

    let requests = responses.requests();
    assert_eq!(requests.len(), 3);

    let thread_id = test.session_configured.thread_id;
    assert_eq!(
        token_budget_texts(&requests[2]),
        vec![format!(
            "<token_budget>\nThread id {thread_id}.\nCurrent context window 1.\nYou have {EFFECTIVE_CONTEXT_WINDOW} tokens left in this context window.\n</token_budget>"
        )],
        "post-compaction full context should report context window 1"
    );
    assert_eq!(
        session_token_budget_texts(&requests[2]),
        vec![
            "<session_token_budget>\nThis session has a shared token budget of 100 tokens across all threads.\n</session_token_budget>".to_string(),
            "<session_token_budget>\nYou have 70 tokens left in the shared session token budget.\n</session_token_budget>".to_string(),
        ],
        "a new context window should restate the stable declaration and current remainder"
    );
    let request_body = requests[2].body_json().to_string();
    let summary_position = request_body
        .find("compact summary")
        .expect("post-compaction request should contain the summary");
    let reminder_position = request_body
        .find("You have 70 tokens left in the shared session token budget.")
        .expect("post-compaction request should contain the current remainder");
    assert!(
        summary_position < reminder_position,
        "the current remainder should follow the compaction summary"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn new_context_tool_starts_new_window_before_follow_up() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let call_id = "new-window-call";
    let continue_call_id = "continue-call";
    let continue_args = json!({
        "plan": [
            {"step": "Continue in the new context window", "status": "in_progress"}
        ],
    })
    .to_string();
    let responses = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-1"),
                ev_function_call(call_id, "new_context", "{}"),
                ev_completed("resp-1"),
            ]),
            sse(vec![
                ev_response_created("resp-2"),
                ev_function_call(continue_call_id, "update_plan", &continue_args),
                ev_completed("resp-2"),
            ]),
            sse(vec![
                ev_response_created("resp-3"),
                ev_assistant_message("msg-3", "done"),
                ev_completed("resp-3"),
            ]),
        ],
    )
    .await;
    let test = test_codex()
        .with_config(|config| {
            config.model_context_window = Some(CONFIGURED_CONTEXT_WINDOW);
            config
                .features
                .enable(Feature::TokenBudget)
                .expect("test config should allow token budget");
        })
        .build(&server)
        .await?;

    test.submit_turn("request new context window").await?;

    let requests = responses.requests();
    assert_eq!(requests.len(), 3);
    assert!(
        tool_names(&requests[0])
            .iter()
            .any(|name| name == "new_context"),
        "new_context should be exposed when token budget is enabled"
    );
    let thread_id = test.session_configured.thread_id;
    assert_eq!(
        token_budget_texts(&requests[2]),
        vec![format!(
            "<token_budget>\nThread id {thread_id}.\nCurrent context window 1.\nYou have {EFFECTIVE_CONTEXT_WINDOW} tokens left in this context window.\n</token_budget>"
        )]
    );
    assert!(
        !requests[2].body_contains_text("request new context window"),
        "new_context should drop the prior window history before continuing the turn"
    );
    assert_eq!(
        requests[2].function_call_output_text(continue_call_id),
        Some("Plan updated".to_string())
    );
    let snapshot = context_snapshot::format_labeled_requests_snapshot(
        "New context window tool installs fresh full context before the next follow-up request.",
        &[("Final Follow-Up Request", &requests[2])],
        &ContextSnapshotOptions::default(),
    );
    let snapshot = snapshot.replace(&thread_id.to_string(), "<THREAD_ID>");
    insta::assert_snapshot!(
        "token_budget_new_context_window_tool_full_context",
        snapshot
    );

    Ok(())
}
