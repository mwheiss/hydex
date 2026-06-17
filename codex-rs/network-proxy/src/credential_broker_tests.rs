use super::*;

use pretty_assertions::assert_eq;

fn env_map<const N: usize>(entries: [(&str, &str); N]) -> HashMap<String, String> {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

fn headers_with_bearer(value: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {value}")).expect("valid bearer header"),
    );
    headers
}

fn authorization(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
}

#[test]
fn virtualize_child_env_replaces_supported_credentials() {
    let broker = CredentialBroker::new(/*enabled*/ true);
    let mut env = env_map([("GH_TOKEN", "ghp-real"), ("OPENAI_API_KEY", "sk-real")]);

    broker.virtualize_child_env(&mut env);

    assert_eq!(
        env,
        env_map([
            ("GH_TOKEN", "ghp_codex_dummy_0000"),
            ("OPENAI_API_KEY", "sk-codex-dummy-0001"),
            (CREDENTIAL_BROKER_ACTIVE_ENV_KEY, "1",),
        ])
    );
}

#[test]
fn virtualize_child_env_preserves_live_dummy_mappings() {
    let broker = CredentialBroker::new(/*enabled*/ true);
    let mut first_env = env_map([("GH_TOKEN", "ghp-real-one")]);
    let mut second_env = env_map([("GH_TOKEN", "ghp-real-two")]);

    broker.virtualize_child_env(&mut first_env);
    broker.virtualize_child_env(&mut second_env);
    let first_dummy = first_env.get("GH_TOKEN").expect("first dummy token");
    let second_dummy = second_env.get("GH_TOKEN").expect("second dummy token");
    let mut first_headers = headers_with_bearer(first_dummy);
    let mut second_headers = headers_with_bearer(second_dummy);

    broker.inject_request_headers("api.github.com", &mut first_headers);
    broker.inject_request_headers("api.github.com", &mut second_headers);

    assert_eq!(authorization(&first_headers), Some("Bearer ghp-real-one"));
    assert_eq!(authorization(&second_headers), Some("Bearer ghp-real-two"));
}

#[test]
fn virtualize_child_env_preserves_unbound_enterprise_token() {
    let broker = CredentialBroker::new(/*enabled*/ true);
    let mut env = env_map([("GH_ENTERPRISE_TOKEN", "ghp-enterprise-real")]);

    broker.virtualize_child_env(&mut env);
    let mut headers = headers_with_bearer("ghp_codex_dummy_0000");
    broker.inject_request_headers("attacker.example", &mut headers);

    assert_eq!(
        env,
        env_map([
            ("GH_ENTERPRISE_TOKEN", "ghp-enterprise-real"),
            (CREDENTIAL_BROKER_ACTIVE_ENV_KEY, "1"),
        ])
    );
    assert_eq!(authorization(&headers), Some("Bearer ghp_codex_dummy_0000"));
    assert!(!broker.host_requires_mitm("attacker.example"));
}

#[test]
fn inject_request_headers_requires_dummy_to_select_ambiguous_github_credential() {
    let broker = CredentialBroker::new(/*enabled*/ true);
    let mut env = env_map([
        ("GH_TOKEN", "ghp-real-one"),
        ("GITHUB_TOKEN", "ghp-real-two"),
    ]);
    broker.virtualize_child_env(&mut env);
    let github_token = env.get("GITHUB_TOKEN").expect("dummy github token");
    let mut headers = HeaderMap::new();

    broker.inject_request_headers("api.github.com", &mut headers);
    assert_eq!(authorization(&headers), None);

    headers = headers_with_bearer(github_token);

    broker.inject_request_headers("api.github.com", &mut headers);

    assert_eq!(authorization(&headers), Some("Bearer ghp-real-two"));
}

#[test]
fn inject_request_headers_uses_unique_key_and_preserves_explicit_authorization() {
    let broker = CredentialBroker::new(/*enabled*/ true);
    let mut env = env_map([("OPENAI_API_KEY", "sk-real")]);
    broker.virtualize_child_env(&mut env);
    let mut headers = HeaderMap::new();

    broker.inject_request_headers("api.openai.com", &mut headers);
    assert_eq!(authorization(&headers), Some("Bearer sk-real"));

    let mut explicit_headers = headers_with_bearer("sk-explicit");
    broker.inject_request_headers("api.openai.com", &mut explicit_headers);

    assert_eq!(authorization(&explicit_headers), Some("Bearer sk-explicit"));
}

#[test]
fn github_cloud_credentials_match_ghe_com_host_hint() {
    let broker = CredentialBroker::new(/*enabled*/ true);
    let mut env = env_map([("GH_HOST", "astemu.ghe.com"), ("GH_TOKEN", "ghp-real")]);
    broker.virtualize_child_env(&mut env);
    let mut headers = HeaderMap::new();

    broker.inject_request_headers("api.astemu.ghe.com", &mut headers);

    assert_eq!(authorization(&headers), Some("Bearer ghp-real"));
}

#[test]
fn github_cloud_credentials_do_not_bind_to_ghes_host_hint() {
    let broker = CredentialBroker::new(/*enabled*/ true);
    let mut env = env_map([("GH_HOST", "github.example.com"), ("GH_TOKEN", "ghp-real")]);
    broker.virtualize_child_env(&mut env);
    let github_token = env.get("GH_TOKEN").expect("dummy github token");
    let expected_authorization = format!("Bearer {github_token}");
    let mut headers = headers_with_bearer(github_token);

    broker.inject_request_headers("github.example.com", &mut headers);

    assert_eq!(
        authorization(&headers),
        Some(expected_authorization.as_str())
    );
    assert!(!broker.host_requires_mitm("github.example.com"));
    assert!(broker.host_requires_mitm("api.github.com"));
}

#[test]
fn github_enterprise_credentials_bind_to_gh_host() {
    let broker = CredentialBroker::new(/*enabled*/ true);
    let mut env = env_map([
        ("GH_HOST", "github.example.com"),
        ("GH_ENTERPRISE_TOKEN", "ghp-enterprise-real"),
    ]);
    broker.virtualize_child_env(&mut env);
    let mut headers = HeaderMap::new();

    broker.inject_request_headers("github.example.com", &mut headers);

    assert_eq!(authorization(&headers), Some("Bearer ghp-enterprise-real"));
    assert!(broker.host_requires_mitm("github.example.com"));
    assert!(!broker.host_requires_mitm("api.github.com"));
}
