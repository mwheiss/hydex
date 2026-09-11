use std::time::Duration;

use codex_api::AuthProvider;
use codex_api::HttpTransport;
use codex_api::Provider;
use codex_api::Request;
use codex_api::ReqwestTransport;
use http::Method;
use serde_json::Value;
use url::Url;

use crate::config::ModelOffloadContextConfig;

const LOCAL_CONTEXT_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct AdvertisedLocalOffloadContext {
    pub context_window: Option<i64>,
    pub effective_context_window_percent: Option<i64>,
    pub auto_compact_token_limit: Option<i64>,
}

impl AdvertisedLocalOffloadContext {
    fn has_context_window(self) -> bool {
        self.context_window.is_some()
    }

    pub(crate) fn is_useful(self) -> bool {
        self.context_window.is_some()
            || self.effective_context_window_percent.is_some()
            || self.auto_compact_token_limit.is_some()
    }

    pub(crate) fn apply_over(
        self,
        fallback: ModelOffloadContextConfig,
    ) -> ModelOffloadContextConfig {
        ModelOffloadContextConfig {
            context_window: self.context_window.or(fallback.context_window),
            effective_context_window_percent: self
                .effective_context_window_percent
                .unwrap_or(fallback.effective_context_window_percent),
            auto_compact_token_limit: self
                .auto_compact_token_limit
                .or(fallback.auto_compact_token_limit),
        }
    }
}

pub(crate) async fn discover_local_offload_context(
    provider: &Provider,
    auth: &dyn AuthProvider,
    transport: &ReqwestTransport,
    configured_model: Option<&str>,
) -> Result<AdvertisedLocalOffloadContext, String> {
    tokio::time::timeout(
        LOCAL_CONTEXT_DISCOVERY_TIMEOUT,
        discover_local_offload_context_inner(provider, auth, transport, configured_model),
    )
    .await
    .map_err(|_| "local provider context discovery timed out".to_string())?
}

async fn discover_local_offload_context_inner(
    provider: &Provider,
    auth: &dyn AuthProvider,
    transport: &ReqwestTransport,
    configured_model: Option<&str>,
) -> Result<AdvertisedLocalOffloadContext, String> {
    let models = execute_json(
        provider.build_request(Method::GET, "models"),
        auth,
        transport,
    )
    .await;
    let mut advertised = models
        .as_ref()
        .ok()
        .and_then(|models| context_from_models(models, configured_model))
        .unwrap_or_default();
    if advertised.has_context_window() {
        return Ok(advertised);
    }

    let probes = [
        ("props", ContextProbe::LlamaCppProps),
        ("api/ps", ContextProbe::OllamaProcesses),
        ("api/v1/models", ContextProbe::LmStudioModels),
        ("model_info", ContextProbe::SglangModelInfo),
        ("get_model_info", ContextProbe::SglangModelInfo),
        ("info", ContextProbe::TgiInfo),
    ];
    let mut errors = Vec::new();
    if let Err(err) = models {
        errors.push(format!("/models: {err}"));
    }
    for (path, probe) in probes {
        let mut request = provider.build_request(Method::GET, "");
        let query_model = if matches!(probe, ContextProbe::LlamaCppProps) {
            configured_model
        } else {
            None
        };
        request.url = root_endpoint_url(&request.url, path, query_model)?;
        match execute_json(request, auth, transport).await {
            Ok(response) => {
                let discovered = probe.parse(&response, configured_model);
                advertised = AdvertisedLocalOffloadContext {
                    context_window: advertised.context_window.or(discovered.context_window),
                    effective_context_window_percent: advertised
                        .effective_context_window_percent
                        .or(discovered.effective_context_window_percent),
                    auto_compact_token_limit: advertised
                        .auto_compact_token_limit
                        .or(discovered.auto_compact_token_limit),
                };
                if advertised.has_context_window() {
                    return Ok(advertised);
                }
            }
            Err(err) => errors.push(format!("/{path}: {err}")),
        }
    }
    if advertised.is_useful() {
        Ok(advertised)
    } else {
        Err(format!(
            "local provider did not advertise usable context metadata ({})",
            errors.join("; ")
        ))
    }
}

#[derive(Debug, Clone, Copy)]
enum ContextProbe {
    LlamaCppProps,
    OllamaProcesses,
    LmStudioModels,
    SglangModelInfo,
    TgiInfo,
}

impl ContextProbe {
    fn parse(
        self,
        response: &Value,
        configured_model: Option<&str>,
    ) -> AdvertisedLocalOffloadContext {
        match self {
            Self::LlamaCppProps => context_from_props(response).unwrap_or_default(),
            Self::OllamaProcesses => {
                context_from_ollama_processes(response, configured_model).unwrap_or_default()
            }
            Self::LmStudioModels => {
                context_from_lm_studio_models(response, configured_model).unwrap_or_default()
            }
            Self::SglangModelInfo => context_from_named_limit(response, "context_length"),
            Self::TgiInfo => context_from_named_limit(response, "max_total_tokens"),
        }
    }
}

async fn execute_json(
    mut request: Request,
    auth: &dyn AuthProvider,
    transport: &ReqwestTransport,
) -> Result<Value, String> {
    request.timeout = Some(LOCAL_CONTEXT_DISCOVERY_TIMEOUT);
    let request = auth
        .apply_auth(request)
        .await
        .map_err(|err| err.to_string())?;
    let response = transport
        .execute(request)
        .await
        .map_err(|err| err.to_string())?;
    serde_json::from_slice(&response.body).map_err(|err| err.to_string())
}

fn root_endpoint_url(
    base_url: &str,
    endpoint: &str,
    configured_model: Option<&str>,
) -> Result<String, String> {
    let mut url = Url::parse(base_url).map_err(|err| err.to_string())?;
    let path = url.path().trim_end_matches('/');
    let root = path.strip_suffix("/v1").unwrap_or(path);
    url.set_path(&format!("{root}/{}", endpoint.trim_start_matches('/')));
    if let Some(configured_model) = configured_model
        && !configured_model.is_empty()
        && !url.query_pairs().any(|(key, _)| key == "model")
    {
        url.query_pairs_mut().append_pair("model", configured_model);
    }
    Ok(url.to_string())
}

fn context_from_models(
    response: &Value,
    configured_model: Option<&str>,
) -> Option<AdvertisedLocalOffloadContext> {
    let codex_models = response.get("models").and_then(Value::as_array);
    if let Some(model) = select_model(codex_models, configured_model, "slug") {
        let context = context_from_model_card(model);
        if context.is_useful() {
            return Some(context);
        }
    }

    let openai_models = response.get("data").and_then(Value::as_array);
    let model = select_model(openai_models, configured_model, "id")?;
    let context = context_from_model_card(model);
    if context.is_useful() {
        return Some(context);
    }
    model
        .get("parent")
        .and_then(Value::as_str)
        .and_then(|parent| select_model(openai_models, Some(parent), "id"))
        .map(context_from_model_card)
}

fn select_model<'a>(
    models: Option<&'a Vec<Value>>,
    configured_model: Option<&str>,
    id_field: &str,
) -> Option<&'a Value> {
    let models = models?;
    configured_model
        .and_then(|configured_model| {
            models
                .iter()
                .find(|model| model.get(id_field).and_then(Value::as_str) == Some(configured_model))
        })
        .or_else(|| (models.len() == 1).then(|| &models[0]))
}

fn context_from_model_card(model: &Value) -> AdvertisedLocalOffloadContext {
    let token_truncation_limit = model
        .get("truncation_policy")
        .filter(|policy| policy.get("mode").and_then(Value::as_str) == Some("tokens"))
        .and_then(|policy| positive_i64(policy.get("limit")));
    AdvertisedLocalOffloadContext {
        context_window: positive_i64(model.get("context_window"))
            .or_else(|| positive_i64(model.get("max_context_window")))
            .or_else(|| positive_i64(model.get("max_model_len")))
            .or_else(|| positive_i64(model.get("context_length")))
            .or_else(|| positive_i64(model.get("context_size")))
            .or(token_truncation_limit),
        effective_context_window_percent: positive_i64(
            model.get("effective_context_window_percent"),
        )
        .filter(|percent| *percent <= 100),
        auto_compact_token_limit: positive_i64(model.get("auto_compact_token_limit")),
    }
}

fn context_from_ollama_processes(
    response: &Value,
    configured_model: Option<&str>,
) -> Option<AdvertisedLocalOffloadContext> {
    let models = response.get("models").and_then(Value::as_array)?;
    let model = select_model_by_fields(models, configured_model, &["model", "name"])?;
    Some(context_from_named_limit(model, "context_length"))
}

fn context_from_lm_studio_models(
    response: &Value,
    configured_model: Option<&str>,
) -> Option<AdvertisedLocalOffloadContext> {
    let models = response.get("models").and_then(Value::as_array)?;
    let model = select_model_by_fields(models, configured_model, &["key", "id"])?;
    let loaded_instances = model.get("loaded_instances").and_then(Value::as_array)?;
    let loaded = select_model_by_fields(loaded_instances, configured_model, &["id"])?;
    let context_window = loaded
        .get("config")
        .and_then(|config| positive_i64(config.get("context_length")))?;
    Some(AdvertisedLocalOffloadContext {
        context_window: Some(context_window),
        ..Default::default()
    })
}

fn select_model_by_fields<'a>(
    models: &'a [Value],
    configured_model: Option<&str>,
    id_fields: &[&str],
) -> Option<&'a Value> {
    configured_model
        .and_then(|configured_model| {
            models.iter().find(|model| {
                id_fields
                    .iter()
                    .any(|field| model.get(field).and_then(Value::as_str) == Some(configured_model))
            })
        })
        .or_else(|| (models.len() == 1).then(|| &models[0]))
}

fn context_from_named_limit(response: &Value, field: &str) -> AdvertisedLocalOffloadContext {
    AdvertisedLocalOffloadContext {
        context_window: positive_i64(response.get(field)),
        ..Default::default()
    }
}

fn context_from_props(props: &Value) -> Option<AdvertisedLocalOffloadContext> {
    let context_window = props
        .get("default_generation_settings")
        .and_then(|settings| positive_i64(settings.get("n_ctx")))
        .or_else(|| positive_i64(props.get("n_ctx")))?;
    Some(AdvertisedLocalOffloadContext {
        context_window: Some(context_window),
        ..Default::default()
    })
}

fn positive_i64(value: Option<&Value>) -> Option<i64> {
    value.and_then(Value::as_i64).filter(|value| *value > 0)
}

#[cfg(test)]
#[path = "local_offload_context_tests.rs"]
mod tests;
