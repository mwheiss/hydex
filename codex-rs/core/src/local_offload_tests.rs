use codex_tools::ResponsesApiNamespace;
use codex_tools::ResponsesApiTool;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

fn function(name: &str) -> ResponsesApiTool {
    ResponsesApiTool {
        name: name.to_string(),
        description: format!("{name} description"),
        strict: false,
        defer_loading: None,
        parameters: serde_json::from_value(json!({"type": "object"})).expect("valid object schema"),
        output_schema: None,
    }
}

fn request_with_input(input: Vec<ResponseItem>) -> ResponsesApiRequest {
    ResponsesApiRequest {
        model: "local-model".to_string(),
        input,
        tools: None,
        tool_choice: "auto".to_string(),
        parallel_tool_calls: false,
        temperature: None,
        reasoning: None,
        store: false,
        stream: true,
        stream_options: None,
        include: Vec::new(),
        service_tier: None,
        prompt_cache_key: None,
        text: None,
        client_metadata: None,
        access_programs: None,
    }
}

#[test]
fn local_offload_projects_backend_controls_and_agent_messages_without_mutating_history() {
    let canonical = vec![
        ResponseItem::AdditionalTools { id: None, role: "developer".into(), tools: Arc::from(vec![json!({"type":"function","name":"old-tool"})]) },
        ResponseItem::ConfigurationUpdate { reasoning: codex_protocol::models::ConfigurationReasoning { effort: codex_protocol::openai_models::ReasoningEffort::Low } },
        ResponseItem::CompactionTrigger {},
        serde_json::from_value(json!({"type":"message","role":"user","content":[{"type":"input_text","text":"Use the current tool."}]})).unwrap(),
        ResponseItem::AgentMessage { id: None, author: "/root/peer".into(), recipient: "/root".into(),
            content: vec![AgentMessageInputContent::InputText { text: "Peer evidence.".into() }],
            internal_chat_message_metadata_passthrough: None },
    ];
    let original = canonical.clone();
    let mut request = request_with_input(canonical.clone());
    request.include = vec!["reasoning.encrypted_content".into()];
    request.service_tier = Some("priority".into());
    transform_request_for_local_offload(
        &mut request,
        &[ToolSpec::Function(function("current-tool"))],
    )
    .unwrap();
    assert_eq!(canonical, original);
    assert_eq!(request.input.len(), 2);
    assert_eq!(request.input[0], original[3]);
    let ResponseItem::Message { role, content, .. } = &request.input[1] else {
        panic!("agent data must be a portable message")
    };
    assert_eq!(role, "assistant");
    let serialized = serde_json::to_string(content).unwrap();
    assert!(serialized.contains("/root/peer") && serialized.contains("Peer evidence."));
    let tools = serde_json::to_value(request.tools.unwrap()).unwrap();
    assert_eq!(tools[0]["name"], "current-tool");
    assert_eq!(tools.as_array().unwrap().len(), 1);
    assert!(request.include.is_empty());
    assert!(request.service_tier.is_none());
}

#[test]
fn local_offload_rejects_encrypted_agent_messages_instead_of_losing_context() {
    let canonical = ResponseItem::AgentMessage {
        id: None,
        author: "/root/peer".into(),
        recipient: "/root".into(),
        content: vec![AgentMessageInputContent::EncryptedContent {
            encrypted_content: "opaque".into(),
        }],
        internal_chat_message_metadata_passthrough: None,
    };
    let mut request = request_with_input(vec![canonical.clone()]);
    let error = transform_request_for_local_offload(&mut request, &[]).unwrap_err();
    assert!(error.to_string().contains("encrypted agent message"));
    assert!(matches!(canonical, ResponseItem::AgentMessage { .. }));
}

#[test]
fn local_offload_preserves_readable_reasoning_summary_without_replaying_ciphertext() {
    let canonical = ResponseItem::Reasoning {
        id: None,
        summary: vec![
            codex_protocol::models::ReasoningItemReasoningSummary::SummaryText {
                text: "Prior check completed.".into(),
            },
        ],
        content: None,
        encrypted_content: Some("other-provider-ciphertext".into()),
        internal_chat_message_metadata_passthrough: None,
    };
    let mut request = request_with_input(vec![canonical.clone()]);
    transform_request_for_local_offload(&mut request, &[]).unwrap();
    let body = serde_json::to_string(&request.input).unwrap();
    assert!(body.contains("Prior check completed."));
    assert!(!body.contains("other-provider-ciphertext"));
    assert!(matches!(request.input[0], ResponseItem::Message { .. }));
    assert!(matches!(
        canonical,
        ResponseItem::Reasoning {
            encrypted_content: Some(_),
            ..
        }
    ));
}

#[test]
fn flattens_namespace_tools_for_local_wire_only() {
    let tools = vec![ToolSpec::Namespace(ResponsesApiNamespace {
        name: "web".to_string(),
        description: "Web tools.".to_string(),
        tools: vec![ResponsesApiNamespaceTool::Function(function("run"))],
    })];

    let (wire_tools, names) =
        create_tools_json_for_local_offload(&tools).expect("local tools serialize");

    assert_eq!(
        wire_tools,
        vec![json!({
            "type": "function",
            "name": "ns__web__run",
            "description": "Web tools.\n\nrun description",
            "strict": false,
            "parameters": {"type": "object"}
        })]
    );

    let item = ResponseItem::FunctionCall {
        encrypted_content: Some("opaque-provider-content".to_string()),
        status: Some("completed".to_string()),
        id: None,
        name: "ns__web__run".to_string(),
        namespace: None,
        arguments: "{}".to_string(),
        call_id: "call_1".to_string(),
        internal_chat_message_metadata_passthrough: None,
        encrypted_function_args: Some(vec!["local-encrypted-args".to_string()]),
    };
    assert_eq!(
        names.unflatten_response_item(item),
        ResponseItem::FunctionCall {
            encrypted_content: Some("opaque-provider-content".to_string()),
            status: Some("completed".to_string()),
            id: None,
            name: "run".to_string(),
            namespace: Some("web".to_string()),
            arguments: "{}".to_string(),
            call_id: "call_1".to_string(),
            internal_chat_message_metadata_passthrough: None,
            encrypted_function_args: Some(vec!["local-encrypted-args".to_string()]),
        }
    );

    let canonical_item = ResponseItem::FunctionCall {
        encrypted_content: Some("opaque-provider-content".to_string()),
        status: Some("completed".to_string()),
        id: None,
        name: "run".to_string(),
        namespace: Some("web".to_string()),
        arguments: "{}".to_string(),
        call_id: "call_2".to_string(),
        internal_chat_message_metadata_passthrough: None,
        encrypted_function_args: Some(vec!["primary-encrypted-args".to_string()]),
    };
    assert_eq!(
        names.flatten_response_item(canonical_item),
        ResponseItem::FunctionCall {
            encrypted_content: Some("opaque-provider-content".to_string()),
            status: Some("completed".to_string()),
            id: None,
            name: "ns__web__run".to_string(),
            namespace: None,
            arguments: "{}".to_string(),
            call_id: "call_2".to_string(),
            internal_chat_message_metadata_passthrough: None,
            encrypted_function_args: Some(vec!["primary-encrypted-args".to_string()]),
        }
    );
}

#[test]
fn flattens_mcp_namespace_without_delimiter_decoding() {
    let namespace = "mcp__codex_apps__google_calendar";
    let tools = vec![ToolSpec::Namespace(ResponsesApiNamespace {
        name: namespace.to_string(),
        description: String::new(),
        tools: vec![ResponsesApiNamespaceTool::Function(function(
            "search_events",
        ))],
    })];

    let (wire_tools, names) =
        create_tools_json_for_local_offload(&tools).expect("local tools serialize");
    let flattened = "ns__mcp__codex_apps__google_calendar__search_events";

    assert_eq!(wire_tools[0]["name"], flattened);
    assert_eq!(
        names.unflatten_response_item(ResponseItem::FunctionCall {
            encrypted_content: None,
            status: None,

            id: None,
            name: flattened.to_string(),
            namespace: None,
            arguments: "{}".to_string(),
            call_id: "call_mcp".to_string(),
            internal_chat_message_metadata_passthrough: None,
            encrypted_function_args: None,
        }),
        ResponseItem::FunctionCall {
            encrypted_content: None,
            status: None,

            id: None,
            name: "search_events".to_string(),
            namespace: Some(namespace.to_string()),
            arguments: "{}".to_string(),
            call_id: "call_mcp".to_string(),
            internal_chat_message_metadata_passthrough: None,
            encrypted_function_args: None,
        }
    );
}

#[test]
fn flattened_name_collision_suffix_is_deterministic() {
    let tools = vec![
        ToolSpec::Function(function("ns__web__run")),
        ToolSpec::Namespace(ResponsesApiNamespace {
            name: "web".to_string(),
            description: String::new(),
            tools: vec![ResponsesApiNamespaceTool::Function(function("run"))],
        }),
    ];

    let (wire_tools, names) =
        create_tools_json_for_local_offload(&tools).expect("local tools serialize");

    assert_eq!(wire_tools[0]["name"], "ns__web__run");
    assert_eq!(wire_tools[1]["name"], "ns__web__run__2");
    assert_eq!(names.request_metrics.collision_renamed_tools, 1);
}

#[test]
fn ordinary_function_keeps_colliding_name_when_namespace_tool_comes_first() {
    let tools = vec![
        ToolSpec::Namespace(ResponsesApiNamespace {
            name: "web".to_string(),
            description: String::new(),
            tools: vec![ResponsesApiNamespaceTool::Function(function("run"))],
        }),
        ToolSpec::Function(function("ns__web__run")),
    ];

    let (wire_tools, names) =
        create_tools_json_for_local_offload(&tools).expect("local tools serialize");

    assert_eq!(wire_tools[0]["name"], "ns__web__run__2");
    assert_eq!(wire_tools[1]["name"], "ns__web__run");
    assert_eq!(
        names.unflatten_response_item(ResponseItem::FunctionCall {
            encrypted_content: None,
            status: None,

            id: None,
            name: "ns__web__run__2".to_string(),
            namespace: None,
            arguments: "{}".to_string(),
            call_id: "call_namespace".to_string(),
            internal_chat_message_metadata_passthrough: None,
            encrypted_function_args: None,
        }),
        ResponseItem::FunctionCall {
            encrypted_content: None,
            status: None,

            id: None,
            name: "run".to_string(),
            namespace: Some("web".to_string()),
            arguments: "{}".to_string(),
            call_id: "call_namespace".to_string(),
            internal_chat_message_metadata_passthrough: None,
            encrypted_function_args: None,
        }
    );
    assert_eq!(
        names.unflatten_response_item(ResponseItem::FunctionCall {
            encrypted_content: None,
            status: None,

            id: None,
            name: "ns__web__run".to_string(),
            namespace: None,
            arguments: "{}".to_string(),
            call_id: "call_ordinary".to_string(),
            internal_chat_message_metadata_passthrough: None,
            encrypted_function_args: None,
        }),
        ResponseItem::FunctionCall {
            encrypted_content: None,
            status: None,

            id: None,
            name: "ns__web__run".to_string(),
            namespace: None,
            arguments: "{}".to_string(),
            call_id: "call_ordinary".to_string(),
            internal_chat_message_metadata_passthrough: None,
            encrypted_function_args: None,
        }
    );
}

#[test]
fn namespace_collision_assignment_is_independent_of_tool_order() {
    let first = ToolSpec::Namespace(ResponsesApiNamespace {
        name: "a__b".to_string(),
        description: String::new(),
        tools: vec![ResponsesApiNamespaceTool::Function(function("c"))],
    });
    let second = ToolSpec::Namespace(ResponsesApiNamespace {
        name: "a".to_string(),
        description: String::new(),
        tools: vec![ResponsesApiNamespaceTool::Function(function("b__c"))],
    });

    let (_, forward) = create_tools_json_for_local_offload(&[first.clone(), second.clone()])
        .expect("forward local tools serialize");
    let (_, reversed) = create_tools_json_for_local_offload(&[second, first])
        .expect("reversed local tools serialize");

    assert_eq!(
        forward.canonical_to_flattened,
        reversed.canonical_to_flattened
    );
    assert_eq!(
        forward.canonical_to_flattened,
        HashMap::from([
            (
                ToolName::namespaced("a".to_string(), "b__c".to_string()),
                "ns__a__b__c".to_string(),
            ),
            (
                ToolName::namespaced("a__b".to_string(), "c".to_string()),
                "ns__a__b__c__2".to_string(),
            ),
        ])
    );
}

#[test]
fn flattens_historical_namespace_call_when_tool_is_not_currently_advertised() {
    let canonical_call = ResponseItem::FunctionCall {
        encrypted_content: None,
        status: None,

        id: None,
        name: "run".to_string(),
        namespace: Some("web".to_string()),
        arguments: "{}".to_string(),
        call_id: "call_historical".to_string(),
        internal_chat_message_metadata_passthrough: None,
        encrypted_function_args: None,
    };
    let mut request = request_with_input(vec![canonical_call.clone()]);

    let names = transform_request_for_local_offload(&mut request, &[])
        .expect("historical local request transforms");

    assert_eq!(
        request.input,
        vec![ResponseItem::FunctionCall {
            encrypted_content: None,
            status: None,

            id: None,
            name: "ns__web__run".to_string(),
            namespace: None,
            arguments: "{}".to_string(),
            call_id: "call_historical".to_string(),
            internal_chat_message_metadata_passthrough: None,
            encrypted_function_args: None,
        }]
    );
    assert_eq!(
        serde_json::to_value(request.tools.expect("local tools should be present"))
            .expect("local tools serialize"),
        json!([])
    );
    assert_eq!(
        names.unflatten_response_item(request.input[0].clone()),
        canonical_call
    );
}

#[test]
fn historical_ordinary_call_reserves_name_before_historical_namespace_flattening() {
    let ordinary_call = ResponseItem::FunctionCall {
        encrypted_content: None,
        status: None,

        id: None,
        name: "ns__web__run".to_string(),
        namespace: None,
        arguments: "{}".to_string(),
        call_id: "call_ordinary_history".to_string(),
        internal_chat_message_metadata_passthrough: None,
        encrypted_function_args: None,
    };
    let canonical_namespace_call = ResponseItem::FunctionCall {
        encrypted_content: None,
        status: None,

        id: None,
        name: "run".to_string(),
        namespace: Some("web".to_string()),
        arguments: "{}".to_string(),
        call_id: "call_namespace_history".to_string(),
        internal_chat_message_metadata_passthrough: None,
        encrypted_function_args: None,
    };
    let mut request = request_with_input(vec![
        ordinary_call.clone(),
        canonical_namespace_call.clone(),
    ]);

    let names = transform_request_for_local_offload(&mut request, &[])
        .expect("historical local request transforms");

    assert_eq!(
        request.input,
        vec![
            ordinary_call,
            ResponseItem::FunctionCall {
                encrypted_content: None,
                status: None,

                id: None,
                name: "ns__web__run__2".to_string(),
                namespace: None,
                arguments: "{}".to_string(),
                call_id: "call_namespace_history".to_string(),
                internal_chat_message_metadata_passthrough: None,
                encrypted_function_args: None,
            },
        ]
    );
    assert_eq!(
        names.unflatten_response_item(request.input[1].clone()),
        canonical_namespace_call
    );
}

#[test]
fn drops_hosted_tool_specs_for_local_wire() {
    let tools = vec![
        ToolSpec::Function(function("plain")),
        ToolSpec::ToolSearch {
            execution: "client".to_string(),
            description: "search".to_string(),
            parameters: serde_json::from_value(json!({"type": "object"}))
                .expect("valid object schema"),
        },
        ToolSpec::WebSearch {
            external_web_access: Some(true),
            filters: None,
            user_location: None,
            search_context_size: None,
            search_content_types: None,
            indexed_web_access: None,
        },
    ];

    let (wire_tools, names) =
        create_tools_json_for_local_offload(&tools).expect("local tools serialize");

    assert_eq!(wire_tools.len(), 1);
    assert_eq!(wire_tools[0]["name"], "plain");
    assert_eq!(
        names.request_metrics,
        LocalOffloadRequestToolMetrics {
            ordinary_direct_tools: 1,
            namespace_tools_before_flattening: 0,
            flattened_functions: 1,
            special_hosted_specs_removed_locally: 2,
            collision_renamed_tools: 0,
        }
    );
}

#[test]
fn records_unknown_and_malformed_local_tool_calls() {
    let (_, names) = create_tools_json_for_local_offload(&[ToolSpec::Function(function("known"))])
        .expect("local tools serialize");
    let known = ResponseItem::FunctionCall {
        encrypted_content: None,
        status: None,

        id: None,
        name: "known".to_string(),
        namespace: None,
        arguments: "{}".to_string(),
        call_id: "call_known".to_string(),
        internal_chat_message_metadata_passthrough: None,
        encrypted_function_args: None,
    };
    let unknown_malformed = ResponseItem::FunctionCall {
        encrypted_content: None,
        status: None,

        id: None,
        name: "missing".to_string(),
        namespace: None,
        arguments: "{".to_string(),
        call_id: "call_unknown".to_string(),
        internal_chat_message_metadata_passthrough: None,
        encrypted_function_args: None,
    };

    assert_eq!(
        names.unflatten_response_item_with_telemetry(known.clone()),
        known
    );
    assert_eq!(
        names.unflatten_response_item_with_telemetry(unknown_malformed.clone()),
        unknown_malformed
    );
    assert_eq!(
        names
            .call_metrics
            .unknown_tool_calls
            .load(Ordering::Relaxed),
        1
    );
    assert_eq!(
        names
            .call_metrics
            .malformed_argument_calls
            .load(Ordering::Relaxed),
        1
    );
}
