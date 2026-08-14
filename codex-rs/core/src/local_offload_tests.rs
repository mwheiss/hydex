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
        instructions: String::new(),
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
    }
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
            id: None,
            name: flattened.to_string(),
            namespace: None,
            arguments: "{}".to_string(),
            call_id: "call_mcp".to_string(),
            internal_chat_message_metadata_passthrough: None,
            encrypted_function_args: None,
        }),
        ResponseItem::FunctionCall {
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

    let (wire_tools, _) =
        create_tools_json_for_local_offload(&tools).expect("local tools serialize");

    assert_eq!(wire_tools[0]["name"], "ns__web__run");
    assert_eq!(wire_tools[1]["name"], "ns__web__run__2");
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
            id: None,
            name: "ns__web__run__2".to_string(),
            namespace: None,
            arguments: "{}".to_string(),
            call_id: "call_namespace".to_string(),
            internal_chat_message_metadata_passthrough: None,
            encrypted_function_args: None,
        }),
        ResponseItem::FunctionCall {
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
            id: None,
            name: "ns__web__run".to_string(),
            namespace: None,
            arguments: "{}".to_string(),
            call_id: "call_ordinary".to_string(),
            internal_chat_message_metadata_passthrough: None,
            encrypted_function_args: None,
        }),
        ResponseItem::FunctionCall {
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
        id: None,
        name: "ns__web__run".to_string(),
        namespace: None,
        arguments: "{}".to_string(),
        call_id: "call_ordinary_history".to_string(),
        internal_chat_message_metadata_passthrough: None,
        encrypted_function_args: None,
    };
    let canonical_namespace_call = ResponseItem::FunctionCall {
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

    let (wire_tools, _) =
        create_tools_json_for_local_offload(&tools).expect("local tools serialize");

    assert_eq!(wire_tools.len(), 1);
    assert_eq!(wire_tools[0]["name"], "plain");
}
