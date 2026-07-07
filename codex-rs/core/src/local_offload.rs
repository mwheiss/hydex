use std::collections::HashMap;
use std::collections::HashSet;

use codex_api::ResponsesApiRequest;
use codex_protocol::models::ResponseItem;
use codex_tools::ResponsesApiNamespaceTool;
use codex_tools::ToolName;
use codex_tools::ToolSpec;
use serde_json::Value;

#[derive(Debug, Clone, Default)]
pub(crate) struct LocalOffloadToolNameMap {
    flattened_to_canonical: HashMap<String, ToolName>,
    canonical_to_flattened: HashMap<ToolName, String>,
}

impl LocalOffloadToolNameMap {
    pub(crate) fn flatten_response_item(&self, item: ResponseItem) -> ResponseItem {
        match item {
            ResponseItem::FunctionCall {
                id,
                name,
                namespace,
                arguments,
                call_id,
                internal_chat_message_metadata_passthrough,
            } => {
                let canonical = ToolName::new(namespace.clone(), name.clone());
                if let Some(flattened) = self.canonical_to_flattened.get(&canonical) {
                    ResponseItem::FunctionCall {
                        id,
                        name: flattened.clone(),
                        namespace: None,
                        arguments,
                        call_id,
                        internal_chat_message_metadata_passthrough,
                    }
                } else {
                    ResponseItem::FunctionCall {
                        id,
                        name,
                        namespace,
                        arguments,
                        call_id,
                        internal_chat_message_metadata_passthrough,
                    }
                }
            }
            item => item,
        }
    }

    pub(crate) fn unflatten_response_item(&self, item: ResponseItem) -> ResponseItem {
        match item {
            ResponseItem::FunctionCall {
                id,
                name,
                namespace,
                arguments,
                call_id,
                internal_chat_message_metadata_passthrough,
            } if namespace.is_none() => {
                if let Some(canonical) = self.flattened_to_canonical.get(&name) {
                    ResponseItem::FunctionCall {
                        id,
                        name: canonical.name.clone(),
                        namespace: canonical.namespace.clone(),
                        arguments,
                        call_id,
                        internal_chat_message_metadata_passthrough,
                    }
                } else {
                    ResponseItem::FunctionCall {
                        id,
                        name,
                        namespace,
                        arguments,
                        call_id,
                        internal_chat_message_metadata_passthrough,
                    }
                }
            }
            item => item,
        }
    }
}

pub(crate) fn create_tools_json_for_local_offload(
    tools: &[ToolSpec],
) -> Result<(Vec<Value>, LocalOffloadToolNameMap), serde_json::Error> {
    let mut flattened_to_canonical = HashMap::new();
    let mut canonical_to_flattened = HashMap::new();
    let mut used_names = HashSet::new();
    let mut local_tools = Vec::new();

    for tool in tools {
        match tool {
            ToolSpec::Function(function) => {
                used_names.insert(function.name.clone());
                local_tools.push(ToolSpec::Function(function.clone()));
            }
            ToolSpec::Namespace(namespace) => {
                for namespace_tool in &namespace.tools {
                    let ResponsesApiNamespaceTool::Function(function) = namespace_tool;
                    let flattened_name =
                        unique_flattened_name(&namespace.name, &function.name, &mut used_names);
                    let canonical =
                        ToolName::namespaced(namespace.name.clone(), function.name.clone());
                    flattened_to_canonical.insert(flattened_name.clone(), canonical.clone());
                    canonical_to_flattened.insert(canonical, flattened_name.clone());
                    let mut flattened_function = function.clone();
                    flattened_function.name = flattened_name;
                    if !namespace.description.trim().is_empty() {
                        flattened_function.description = format!(
                            "{}\n\n{}",
                            namespace.description, flattened_function.description
                        );
                    }
                    local_tools.push(ToolSpec::Function(flattened_function));
                }
            }
            ToolSpec::ToolSearch { .. }
            | ToolSpec::ImageGeneration { .. }
            | ToolSpec::WebSearch { .. }
            | ToolSpec::Freeform(_) => {}
        }
    }

    let tools_json = local_tools
        .iter()
        .map(serde_json::to_value)
        .collect::<Result<Vec<_>, _>>()?;
    Ok((
        tools_json,
        LocalOffloadToolNameMap {
            flattened_to_canonical,
            canonical_to_flattened,
        },
    ))
}

pub(crate) fn transform_request_for_local_offload(
    request: &mut ResponsesApiRequest,
    tools: &[ToolSpec],
) -> Result<LocalOffloadToolNameMap, serde_json::Error> {
    let (local_tools, tool_names) = create_tools_json_for_local_offload(tools)?;
    request.tools = Some(local_tools);
    request.input = request
        .input
        .drain(..)
        .map(|item| tool_names.flatten_response_item(item))
        .collect();
    Ok(tool_names)
}

fn unique_flattened_name(namespace: &str, name: &str, used_names: &mut HashSet<String>) -> String {
    let base = format!("ns__{namespace}__{name}");
    if used_names.insert(base.clone()) {
        return base;
    }
    for index in 2usize.. {
        let candidate = format!("{base}__{index}");
        if used_names.insert(candidate.clone()) {
            return candidate;
        }
    }
    unreachable!("unbounded suffix search should always find a free name")
}

#[cfg(test)]
mod tests {
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
            parameters: serde_json::from_value(json!({"type": "object"}))
                .expect("valid object schema"),
            output_schema: None,
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
            }
        );

        let canonical_item = ResponseItem::FunctionCall {
            id: None,
            name: "run".to_string(),
            namespace: Some("web".to_string()),
            arguments: "{}".to_string(),
            call_id: "call_2".to_string(),
            internal_chat_message_metadata_passthrough: None,
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
            }),
            ResponseItem::FunctionCall {
                id: None,
                name: "search_events".to_string(),
                namespace: Some(namespace.to_string()),
                arguments: "{}".to_string(),
                call_id: "call_mcp".to_string(),
                internal_chat_message_metadata_passthrough: None,
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
    fn drops_hosted_tool_specs_for_local_wire() {
        let tools = vec![
            ToolSpec::Function(function("plain")),
            ToolSpec::ToolSearch {
                execution: "client".to_string(),
                description: "search".to_string(),
                parameters: serde_json::from_value(json!({"type": "object"}))
                    .expect("valid object schema"),
            },
            ToolSpec::ImageGeneration {
                output_format: "png".to_string(),
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
}
