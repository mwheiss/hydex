use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::HashSet;

use codex_api::ResponsesApiRequest;
use codex_protocol::models::ResponseItem;
use codex_tools::ResponsesApiNamespaceTool;
use codex_tools::ToolName;
use codex_tools::ToolSpec;
use codex_utils_string::approx_token_count;
use serde_json::Value;
use serde_json::value::RawValue;
use serde_json::value::to_raw_value;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

#[derive(Debug, Clone, Default)]
pub(crate) struct LocalOffloadToolNameMap {
    flattened_to_canonical: HashMap<String, ToolName>,
    canonical_to_flattened: HashMap<ToolName, String>,
    local_wire_tool_names: HashSet<String>,
    request_metrics: LocalOffloadRequestToolMetrics,
    call_metrics: Arc<LocalOffloadCallMetrics>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct LocalOffloadRequestToolMetrics {
    ordinary_direct_tools: usize,
    namespace_tools_before_flattening: usize,
    flattened_functions: usize,
    special_hosted_specs_removed_locally: usize,
    collision_renamed_tools: usize,
}

#[derive(Debug, Default)]
struct LocalOffloadCallMetrics {
    unknown_tool_calls: AtomicUsize,
    malformed_argument_calls: AtomicUsize,
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
                encrypted_function_args,
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
                        encrypted_function_args,
                        internal_chat_message_metadata_passthrough,
                    }
                } else {
                    ResponseItem::FunctionCall {
                        id,
                        name,
                        namespace,
                        arguments,
                        call_id,
                        encrypted_function_args,
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
                encrypted_function_args,
                internal_chat_message_metadata_passthrough,
            } if namespace.is_none() => {
                if let Some(canonical) = self.flattened_to_canonical.get(&name) {
                    ResponseItem::FunctionCall {
                        id,
                        name: canonical.name.clone(),
                        namespace: canonical.namespace.clone(),
                        arguments,
                        call_id,
                        encrypted_function_args,
                        internal_chat_message_metadata_passthrough,
                    }
                } else {
                    ResponseItem::FunctionCall {
                        id,
                        name,
                        namespace,
                        arguments,
                        call_id,
                        encrypted_function_args,
                        internal_chat_message_metadata_passthrough,
                    }
                }
            }
            item => item,
        }
    }

    pub(crate) fn unflatten_response_item_with_telemetry(
        &self,
        item: ResponseItem,
    ) -> ResponseItem {
        if let ResponseItem::FunctionCall {
            name,
            namespace,
            arguments,
            ..
        } = &item
        {
            let unknown_tool_call =
                namespace.is_some() || !self.local_wire_tool_names.contains(name);
            let malformed_argument_call = serde_json::from_str::<Value>(arguments)
                .map(|arguments| !arguments.is_object())
                .unwrap_or(true);
            if unknown_tool_call {
                self.call_metrics
                    .unknown_tool_calls
                    .fetch_add(1, Ordering::Relaxed);
            }
            if malformed_argument_call {
                self.call_metrics
                    .malformed_argument_calls
                    .fetch_add(1, Ordering::Relaxed);
            }
            tracing::trace!(
                tool_name = name,
                unknown_tool_call,
                malformed_argument_call,
                "observed local offload tool call"
            );
        }
        self.unflatten_response_item(item)
    }

    pub(crate) fn trace_response_call_summary(&self) {
        tracing::debug!(
            unknown_tool_calls = self.call_metrics.unknown_tool_calls.load(Ordering::Relaxed),
            malformed_argument_calls = self
                .call_metrics
                .malformed_argument_calls
                .load(Ordering::Relaxed),
            "completed local offload tool-call telemetry"
        );
    }
}

#[cfg(test)]
pub(crate) fn create_tools_json_for_local_offload(
    tools: &[ToolSpec],
) -> Result<(Vec<Value>, LocalOffloadToolNameMap), serde_json::Error> {
    create_tools_json_for_local_offload_with_input(tools, &[])
}

fn create_tools_json_for_local_offload_with_input(
    tools: &[ToolSpec],
    input: &[ResponseItem],
) -> Result<(Vec<Value>, LocalOffloadToolNameMap), serde_json::Error> {
    let mut flattened_to_canonical = HashMap::new();
    let mut canonical_to_flattened = HashMap::new();
    let mut used_names = tools
        .iter()
        .filter_map(|tool| match tool {
            ToolSpec::Function(function) => Some(function.name.clone()),
            ToolSpec::Namespace(_)
            | ToolSpec::ToolSearch { .. }
            | ToolSpec::WebSearch { .. }
            | ToolSpec::Freeform(_) => None,
        })
        .collect::<HashSet<_>>();
    used_names.extend(input.iter().filter_map(|item| match item {
        ResponseItem::FunctionCall {
            name,
            namespace: None,
            ..
        } => Some(name.clone()),
        _ => None,
    }));
    let mut canonical_namespace_names = BTreeSet::new();
    for tool in tools {
        if let ToolSpec::Namespace(namespace) = tool {
            for namespace_tool in &namespace.tools {
                let ResponsesApiNamespaceTool::Function(function) = namespace_tool else {
                    continue;
                };
                canonical_namespace_names.insert((namespace.name.clone(), function.name.clone()));
            }
        }
    }
    let current_namespace_tool_names = canonical_namespace_names
        .iter()
        .map(|(namespace, name)| ToolName::namespaced(namespace.clone(), name.clone()))
        .collect::<HashSet<_>>();
    canonical_namespace_names.extend(input.iter().filter_map(|item| match item {
        ResponseItem::FunctionCall {
            name,
            namespace: Some(namespace),
            ..
        } => Some((namespace.clone(), name.clone())),
        _ => None,
    }));
    for (namespace, name) in canonical_namespace_names {
        register_namespaced_tool_name(
            &namespace,
            &name,
            &mut used_names,
            &mut flattened_to_canonical,
            &mut canonical_to_flattened,
        );
    }
    let mut local_tools = Vec::new();
    let mut ordinary_direct_tools = 0usize;
    let mut namespace_tools_before_flattening = 0usize;
    let mut special_hosted_specs_removed_locally = 0usize;

    for tool in tools {
        match tool {
            ToolSpec::Function(function) => {
                ordinary_direct_tools += 1;
                local_tools.push(ToolSpec::Function(function.clone()));
            }
            ToolSpec::Namespace(namespace) => {
                for namespace_tool in &namespace.tools {
                    let ResponsesApiNamespaceTool::Function(function) = namespace_tool else {
                        continue;
                    };
                    namespace_tools_before_flattening += 1;
                    let flattened_name = register_namespaced_tool_name(
                        &namespace.name,
                        &function.name,
                        &mut used_names,
                        &mut flattened_to_canonical,
                        &mut canonical_to_flattened,
                    );
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
            ToolSpec::ToolSearch { .. } | ToolSpec::WebSearch { .. } | ToolSpec::Freeform(_) => {
                special_hosted_specs_removed_locally += 1;
            }
        }
    }

    let local_wire_tool_names = local_tools
        .iter()
        .filter_map(|tool| match tool {
            ToolSpec::Function(function) => Some(function.name.clone()),
            ToolSpec::Namespace(_)
            | ToolSpec::ToolSearch { .. }
            | ToolSpec::WebSearch { .. }
            | ToolSpec::Freeform(_) => None,
        })
        .collect();
    let collision_renamed_tools = canonical_to_flattened
        .iter()
        .filter(|(canonical, flattened)| {
            current_namespace_tool_names.contains(*canonical)
                && flattened.as_str()
                    != format!(
                        "ns__{}__{}",
                        canonical.namespace.as_deref().unwrap_or_default(),
                        canonical.name
                    )
        })
        .count();
    let flattened_functions = local_tools.len();
    let tools_json = local_tools
        .iter()
        .map(serde_json::to_value)
        .collect::<Result<Vec<_>, _>>()?;
    Ok((
        tools_json,
        LocalOffloadToolNameMap {
            flattened_to_canonical,
            canonical_to_flattened,
            local_wire_tool_names,
            request_metrics: LocalOffloadRequestToolMetrics {
                ordinary_direct_tools,
                namespace_tools_before_flattening,
                flattened_functions,
                special_hosted_specs_removed_locally,
                collision_renamed_tools,
            },
            call_metrics: Arc::default(),
        },
    ))
}

pub(crate) fn transform_request_for_local_offload(
    request: &mut ResponsesApiRequest,
    tools: &[ToolSpec],
) -> Result<LocalOffloadToolNameMap, serde_json::Error> {
    let (local_tools, tool_names) =
        create_tools_json_for_local_offload_with_input(tools, &request.input)?;
    let local_tools = Arc::<RawValue>::from(to_raw_value(&local_tools)?);
    let serialized_tool_schema_bytes = local_tools.get().len();
    let approximate_tool_schema_tokens = approx_token_count(local_tools.get());
    let metrics = tool_names.request_metrics;
    tracing::debug!(
        ordinary_direct_tools = metrics.ordinary_direct_tools,
        namespace_tools_before_flattening = metrics.namespace_tools_before_flattening,
        flattened_functions = metrics.flattened_functions,
        special_hosted_specs_removed_locally = metrics.special_hosted_specs_removed_locally,
        serialized_tool_schema_bytes,
        approximate_tool_schema_tokens,
        collision_renamed_tools = metrics.collision_renamed_tools,
        unknown_tool_calls = 0,
        malformed_argument_calls = 0,
        "serialized local offload tool surface"
    );
    request.tools = Some(local_tools.into());
    request.input = request
        .input
        .drain(..)
        .map(|item| tool_names.flatten_response_item(item))
        .collect();
    Ok(tool_names)
}

fn register_namespaced_tool_name(
    namespace: &str,
    name: &str,
    used_names: &mut HashSet<String>,
    flattened_to_canonical: &mut HashMap<String, ToolName>,
    canonical_to_flattened: &mut HashMap<ToolName, String>,
) -> String {
    let canonical = ToolName::namespaced(namespace.to_string(), name.to_string());
    if let Some(flattened_name) = canonical_to_flattened.get(&canonical) {
        return flattened_name.clone();
    }

    let flattened_name = unique_flattened_name(namespace, name, used_names);
    flattened_to_canonical.insert(flattened_name.clone(), canonical.clone());
    canonical_to_flattened.insert(canonical, flattened_name.clone());
    flattened_name
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
#[path = "local_offload_tests.rs"]
mod tests;
