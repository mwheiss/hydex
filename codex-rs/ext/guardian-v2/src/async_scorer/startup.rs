//! Resolves the classifier's own model budget and transport configuration.
//! Parent-model context overrides do not change the classifier allowance.

use std::sync::Arc;

use codex_core::ThreadManager;
use codex_core::config::Config;
use codex_core::cyber_access_program::ApiKeyCyberAccessPrograms;
use codex_extension_api::ExtensionMetrics;
use codex_extension_api::ThreadOriginator;
use codex_extension_api::ThreadStartInput;
use codex_features::Feature;
use codex_login::AgentIdentityAuthPolicy;
use codex_login::AuthManager;
use codex_login::CodexAuth;
use codex_model_provider::create_model_provider;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result as CodexResult;
use codex_protocol::protocol::SessionSource;

use super::decisions::DecisionsError;
use super::decisions::DecisionsSampler;
use super::decisions::URL;
use super::sampler::LunaSamplerConfig;
use super::sampler::MODEL;

// Decisions setup failures are telemetry only; they never affect the baseline sampler.
pub(super) fn decisions_sampler(
    input: &ThreadStartInput<'_, Config>,
    auth_manager: &AuthManager,
) -> Option<DecisionsSampler> {
    if input.config.model_offload.full_local_enabled() {
        return None;
    }
    use codex_http_client::ClientRouteClass;
    let result = decisions_api_key(
        &input.config.model_provider_id,
        input.config.model_provider.base_url.as_deref(),
        auth_manager,
    )
    .and_then(|key| {
        let client = input
            .config
            .http_client_factory()
            .build_client_without_request_logging(URL, ClientRouteClass::Api)
            .map_err(|_| DecisionsError::ClientSetup)?;
        DecisionsSampler::new(client, key, URL.to_owned())
    });
    match result {
        Ok(sampler) => Some(sampler),
        Err(error) => {
            if let Some(metrics) = input.extension_metrics.as_deref() {
                metrics.counter(
                    "codex.guardian_v2.decisions_comparison.setup_failure",
                    /*inc*/ 1,
                    &[("reason", super::metrics::decisions_failure_reason(error))],
                );
            }
            None
        }
    }
}

pub(super) fn decisions_api_key(
    provider_id: &str,
    base_url: Option<&str>,
    auth_manager: &AuthManager,
) -> Result<String, DecisionsError> {
    match std::env::var("CODEX_GUARDIAN_DECISIONS_API_KEY") {
        Ok(key) => Ok(key),
        Err(std::env::VarError::NotPresent) if provider_id == "openai" && base_url.is_none() => {
            auth_manager
                .auth_cached()
                .as_ref()
                .and_then(CodexAuth::api_key)
                .map(str::to_owned)
                .or_else(codex_login::read_openai_api_key_from_env)
                .ok_or(DecisionsError::Credentials)
        }
        Err(std::env::VarError::NotPresent | std::env::VarError::NotUnicode(_)) => {
            Err(DecisionsError::Credentials)
        }
    }
}

pub(super) async fn sampler_config(
    input: &ThreadStartInput<'_, Config>,
    auth_manager: Arc<AuthManager>,
    manager: Option<Arc<ThreadManager>>,
) -> CodexResult<LunaSamplerConfig> {
    if input.config.model_offload.full_local_enabled() {
        let mut local_config = input.config.clone();
        if local_config.model.is_none() {
            local_config.model = input
                .thread_store
                .get::<codex_protocol::openai_models::ModelInfo>()
                .map(|model| model.slug.clone());
        }
        return full_local_sampler_config(
            &local_config,
            input.session_source.clone(),
            input.session_store.level_id().to_string(),
            input.thread_store.level_id().to_string(),
            input.extension_metrics.clone(),
        );
    }
    let luna_model = if let Some(thread_manager) = manager {
        let mut model_config = input.config.to_models_manager_config();
        model_config.model_context_window = None;
        Some(
            thread_manager
                .get_models_manager()
                .get_model_info(MODEL, &model_config)
                .await,
        )
    } else {
        None
    };
    let max_input_tokens =
        luna_model
            .as_ref()
            .map_or(codex_guardian_context::DEFAULT_MAX_INPUT_TOKENS, |model| {
                codex_guardian_context::effective_input_token_limit(
                    model, /*configured_window*/ None,
                )
            });
    let luna_compaction_hash = luna_model.and_then(|model| model.comp_hash);
    Ok(LunaSamplerConfig {
        local_model: None,
        api_key_cyber_access_programs: ApiKeyCyberAccessPrograms::from_config(input.config),
        workspace_routing: input
            .thread_store
            .get_or_init(|| input.config.workspace_routing_context())
            .as_ref()
            .clone(),
        provider: create_model_provider(input.config.model_provider.clone(), Some(auth_manager)),
        http_client_factory: input.config.http_client_factory(),
        agent_identity_policy: if input.config.features.enabled(Feature::UseAgentIdentity) {
            AgentIdentityAuthPolicy::ChatGptAuth
        } else {
            AgentIdentityAuthPolicy::JwtOnly
        },
        session_source: input.session_source.clone(),
        session_id: input.session_store.level_id().to_string(),
        thread_id: input.thread_store.level_id().to_string(),
        originator: input
            .thread_store
            .get::<ThreadOriginator>()
            .map(|originator| originator.0.clone()),
        service_tier: input.config.service_tier.clone(),
        luna_compaction_hash,
        max_input_tokens,
        metrics: input.extension_metrics.clone(),
    })
}

/// Resolve only the explicitly configured local provider, without hosted credentials or routing.
pub(super) fn full_local_sampler_config(
    config: &Config,
    session_source: SessionSource,
    session_id: String,
    thread_id: String,
    metrics: Option<Arc<dyn ExtensionMetrics>>,
) -> CodexResult<LunaSamplerConfig> {
    let provider = config.model_offload.provider.clone().ok_or_else(|| {
        CodexErr::InvalidRequest("Full local review requires a configured local provider.".into())
    })?;
    let max_input_tokens = config
        .model_offload
        .context
        .context_window
        .and_then(|window| usize::try_from(window).ok())
        .filter(|window| *window > 0)
        .unwrap_or(codex_guardian_context::DEFAULT_MAX_INPUT_TOKENS);
    Ok(LunaSamplerConfig {
        local_model: Some(
            config
                .model_offload
                .model
                .clone()
                .or_else(|| config.model.clone())
                .ok_or_else(|| {
                    CodexErr::InvalidRequest("Full local review requires a selected model.".into())
                })?,
        ),
        provider: create_model_provider(provider, None),
        api_key_cyber_access_programs: ApiKeyCyberAccessPrograms::Disabled,
        workspace_routing: config.workspace_routing_context(),
        http_client_factory: config.http_client_factory(),
        agent_identity_policy: AgentIdentityAuthPolicy::JwtOnly,
        session_source,
        session_id,
        thread_id,
        originator: None,
        service_tier: None,
        luna_compaction_hash: None,
        max_input_tokens,
        metrics,
    })
}
