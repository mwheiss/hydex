use crate::policy::normalize_host;
use rama_http::HeaderMap;
use rama_http::HeaderValue;
use rama_http::header::AUTHORIZATION;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;

const GH_HOST_ENV_VAR: &str = "GH_HOST";
const GITHUB_CLOUD_TOKEN_ENV_VARS: &[&str] = &["GH_TOKEN", "GITHUB_TOKEN"];
const GITHUB_ENTERPRISE_TOKEN_ENV_VARS: &[&str] =
    &["GH_ENTERPRISE_TOKEN", "GITHUB_ENTERPRISE_TOKEN"];
const OPENAI_API_KEY_ENV_VARS: &[&str] = &["OPENAI_API_KEY"];
pub const CREDENTIAL_BROKER_ACTIVE_ENV_KEY: &str = "CODEX_NETWORK_PROXY_CREDENTIAL_BROKER_ACTIVE";
pub(crate) const CREDENTIAL_BROKER_ENV_KEYS: &[&str] = &[
    GH_HOST_ENV_VAR,
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GH_ENTERPRISE_TOKEN",
    "GITHUB_ENTERPRISE_TOKEN",
    "OPENAI_API_KEY",
];

#[derive(Clone)]
pub(crate) struct CredentialBroker {
    state: Arc<RwLock<CredentialBrokerState>>,
}

#[derive(Default)]
struct CredentialBrokerState {
    enabled: bool,
    next_credential_id: usize,
    credentials: Vec<CredentialRecord>,
}

struct CredentialRecord {
    env_var: String,
    kind: CredentialKind,
    host_binding: CredentialHostBinding,
    real_value: String,
    dummy_value: String,
}

struct CredentialCandidate {
    env_var: &'static str,
    kind: CredentialKind,
    host_binding: CredentialHostBinding,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CredentialKind {
    GitHub,
    OpenAiApiKey,
}

#[derive(Clone, PartialEq, Eq)]
enum CredentialHostBinding {
    GitHubCloud,
    ExactHost(String),
    OpenAiApi,
}

impl CredentialBroker {
    pub(crate) fn new(enabled: bool) -> Self {
        Self {
            state: Arc::new(RwLock::new(CredentialBrokerState {
                enabled,
                ..CredentialBrokerState::default()
            })),
        }
    }

    pub(crate) fn set_enabled(&self, enabled: bool) {
        let mut state = self.write_state();
        state.enabled = enabled;
        if !enabled {
            state.credentials.clear();
            state.next_credential_id = 0;
        }
    }

    pub(crate) fn virtualize_child_env(&self, env: &mut HashMap<String, String>) {
        let mut state = self.write_state();
        if !state.enabled {
            env.remove(CREDENTIAL_BROKER_ACTIVE_ENV_KEY);
            return;
        }
        env.insert(
            CREDENTIAL_BROKER_ACTIVE_ENV_KEY.to_string(),
            "1".to_string(),
        );

        for candidate in virtualizable_credential_candidates(env) {
            virtualize_env_var(
                env,
                &mut state,
                candidate.env_var,
                candidate.kind,
                candidate.host_binding,
            );
        }
    }

    pub(crate) fn host_requires_mitm(&self, host: &str) -> bool {
        let normalized_host = normalize_host(host);
        let state = self.read_state();
        state.enabled
            && state
                .credentials
                .iter()
                .any(|credential| credential.matches_host(&normalized_host))
    }

    pub(crate) fn inject_request_headers(&self, host: &str, headers: &mut HeaderMap) {
        let normalized_host = normalize_host(host);
        let state = self.read_state();
        if !state.enabled {
            return;
        }

        let matching_credentials = state
            .credentials
            .iter()
            .filter(|credential| credential.matches_host(&normalized_host))
            .collect::<Vec<_>>();
        let Some(credential) = select_credential(headers, &matching_credentials) else {
            return;
        };
        let Some(header_value) = credential
            .kind
            .authorization_header_value(&credential.real_value)
        else {
            return;
        };
        headers.insert(AUTHORIZATION, header_value);
    }

    fn read_state(&self) -> std::sync::RwLockReadGuard<'_, CredentialBrokerState> {
        self.state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn write_state(&self) -> std::sync::RwLockWriteGuard<'_, CredentialBrokerState> {
        self.state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn virtualizable_credential_candidates(env: &HashMap<String, String>) -> Vec<CredentialCandidate> {
    let mut candidates = GITHUB_CLOUD_TOKEN_ENV_VARS
        .iter()
        .map(|env_var| CredentialCandidate {
            env_var,
            kind: CredentialKind::GitHub,
            host_binding: CredentialHostBinding::GitHubCloud,
        })
        .collect::<Vec<_>>();

    if let Some(host_binding) = github_host_hint(env)
        .filter(|host| !github_cloud_host(host))
        .map(CredentialHostBinding::ExactHost)
    {
        candidates.extend(GITHUB_ENTERPRISE_TOKEN_ENV_VARS.iter().map(|env_var| {
            CredentialCandidate {
                env_var,
                kind: CredentialKind::GitHub,
                host_binding: host_binding.clone(),
            }
        }));
    }

    candidates.extend(
        OPENAI_API_KEY_ENV_VARS
            .iter()
            .map(|env_var| CredentialCandidate {
                env_var,
                kind: CredentialKind::OpenAiApiKey,
                host_binding: CredentialHostBinding::OpenAiApi,
            }),
    );
    candidates.retain(|candidate| {
        brokerable_credential_value(env, candidate.env_var, candidate.kind).is_some()
    });
    candidates
}

fn virtualize_env_var(
    env: &mut HashMap<String, String>,
    state: &mut CredentialBrokerState,
    env_var: &str,
    kind: CredentialKind,
    host_binding: CredentialHostBinding,
) {
    let Some(real_value) = brokerable_credential_value(env, env_var, kind) else {
        return;
    };

    let dummy_value = state.register(env_var, kind, host_binding, real_value);
    env.insert(env_var.to_string(), dummy_value);
}

fn brokerable_credential_value<'a>(
    env: &'a HashMap<String, String>,
    env_var: &str,
    kind: CredentialKind,
) -> Option<&'a str> {
    let real_value = env.get(env_var)?.trim();
    (!real_value.is_empty()
        && !kind.is_dummy_value(real_value)
        && kind.authorization_header_value(real_value).is_some())
    .then_some(real_value)
}

impl CredentialBrokerState {
    fn register(
        &mut self,
        env_var: &str,
        kind: CredentialKind,
        host_binding: CredentialHostBinding,
        real_value: &str,
    ) -> String {
        if let Some(existing) = self.credentials.iter().find(|credential| {
            credential.env_var == env_var
                && credential.kind == kind
                && credential.host_binding == host_binding
                && credential.real_value == real_value
        }) {
            return existing.dummy_value.clone();
        }

        let dummy_value = kind.dummy_value(self.next_credential_id);
        self.next_credential_id += 1;
        self.credentials.push(CredentialRecord {
            env_var: env_var.to_string(),
            kind,
            host_binding,
            real_value: real_value.to_string(),
            dummy_value: dummy_value.clone(),
        });
        dummy_value
    }
}

impl CredentialRecord {
    fn matches_host(&self, host: &str) -> bool {
        self.host_binding.matches_host(host)
    }
}

impl CredentialKind {
    fn dummy_value(self, credential_id: usize) -> String {
        match self {
            Self::GitHub => format!("ghp_codex_dummy_{credential_id:04}"),
            Self::OpenAiApiKey => format!("sk-codex-dummy-{credential_id:04}"),
        }
    }

    fn is_dummy_value(self, value: &str) -> bool {
        match self {
            Self::GitHub => value.starts_with("ghp_codex_dummy_"),
            Self::OpenAiApiKey => value.starts_with("sk-codex-dummy-"),
        }
    }

    fn authorization_header_value(self, value: &str) -> Option<HeaderValue> {
        HeaderValue::from_str(&format!("Bearer {value}")).ok()
    }
}

impl CredentialHostBinding {
    fn matches_host(&self, host: &str) -> bool {
        match self {
            Self::GitHubCloud => github_cloud_host(host),
            Self::ExactHost(expected_host) => host == expected_host,
            Self::OpenAiApi => host == "api.openai.com",
        }
    }
}

fn github_cloud_host(host: &str) -> bool {
    matches!(host, "api.github.com" | "github.com") || host.ends_with(".ghe.com")
}

fn github_host_hint(env: &HashMap<String, String>) -> Option<String> {
    env.get(GH_HOST_ENV_VAR)
        .map(String::as_str)
        .map(normalize_host)
        .filter(|host| !host.is_empty())
}

fn select_credential<'a>(
    headers: &HeaderMap,
    matching_credentials: &[&'a CredentialRecord],
) -> Option<&'a CredentialRecord> {
    match headers.get(AUTHORIZATION) {
        Some(authorization) => authorization.to_str().ok().and_then(|authorization| {
            dummy_authorization_match(authorization, matching_credentials)
        }),
        None => missing_dummy_fallback(matching_credentials),
    }
}

fn dummy_authorization_match<'a>(
    authorization: &str,
    matching_credentials: &[&'a CredentialRecord],
) -> Option<&'a CredentialRecord> {
    let dummy_matches = matching_credentials
        .iter()
        .copied()
        .filter(|credential| authorization.contains(&credential.dummy_value))
        .collect::<Vec<_>>();
    if dummy_matches.len() == 1 {
        dummy_matches.into_iter().next()
    } else {
        None
    }
}

fn missing_dummy_fallback<'a>(
    matching_credentials: &[&'a CredentialRecord],
) -> Option<&'a CredentialRecord> {
    let credential = *matching_credentials.first()?;
    matching_credentials
        .iter()
        .all(|candidate| {
            candidate.kind == credential.kind && candidate.real_value == credential.real_value
        })
        .then_some(credential)
}

pub(crate) fn is_dummy_credential_value(value: &str) -> bool {
    value.starts_with("ghp_codex_dummy_") || value.starts_with("sk-codex-dummy-")
}

#[cfg(test)]
#[path = "credential_broker_tests.rs"]
mod tests;
