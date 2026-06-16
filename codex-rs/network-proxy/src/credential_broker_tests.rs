use super::*;

use pretty_assertions::assert_eq;

#[test]
fn virtualize_child_env_replaces_supported_credentials() {
    let broker = CredentialBroker::new(/*enabled*/ true);
    let mut env = HashMap::from([
        ("GH_TOKEN".to_string(), "ghp-real".to_string()),
        ("OPENAI_API_KEY".to_string(), "sk-real".to_string()),
    ]);

    broker.virtualize_child_env(&mut env);

    assert_eq!(
        env.get("GH_TOKEN"),
        Some(&"ghp_codex_dummy_0000".to_string())
    );
    assert_eq!(
        env.get("OPENAI_API_KEY"),
        Some(&"sk-codex-dummy-0001".to_string())
    );
    assert_eq!(
        env.get(CREDENTIAL_BROKER_ACTIVE_ENV_KEY),
        Some(&"1".to_string())
    );
}

#[test]
fn virtualize_child_env_preserves_live_dummy_mappings() {
    let broker = CredentialBroker::new(/*enabled*/ true);
    let mut first_env = HashMap::from([("GH_TOKEN".to_string(), "ghp-real-one".to_string())]);
    let mut second_env = HashMap::from([("GH_TOKEN".to_string(), "ghp-real-two".to_string())]);

    broker.virtualize_child_env(&mut first_env);
    broker.virtualize_child_env(&mut second_env);
    let first_dummy = first_env.get("GH_TOKEN").expect("first dummy token");
    let second_dummy = second_env.get("GH_TOKEN").expect("second dummy token");
    let mut first_headers = HeaderMap::new();
    first_headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {first_dummy}")).expect("valid first dummy header"),
    );
    let mut second_headers = HeaderMap::new();
    second_headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {second_dummy}"))
            .expect("valid second dummy header"),
    );

    broker.inject_request_headers("api.github.com", &mut first_headers);
    broker.inject_request_headers("api.github.com", &mut second_headers);

    assert_eq!(
        first_headers.get(AUTHORIZATION),
        Some(&HeaderValue::from_static("Bearer ghp-real-one"))
    );
    assert_eq!(
        second_headers.get(AUTHORIZATION),
        Some(&HeaderValue::from_static("Bearer ghp-real-two"))
    );
}

#[test]
fn virtualize_child_env_replaces_unbound_enterprise_token_and_injects_with_dummy() {
    let broker = CredentialBroker::new(/*enabled*/ true);
    let mut env = HashMap::from([(
        "GH_ENTERPRISE_TOKEN".to_string(),
        "ghp-enterprise-real".to_string(),
    )]);

    broker.virtualize_child_env(&mut env);
    let enterprise_token = env
        .get("GH_ENTERPRISE_TOKEN")
        .expect("dummy enterprise token");
    let mut headers = HeaderMap::new();
    broker.inject_request_headers("github.example.com", &mut headers);

    assert_eq!(
        env.get("GH_ENTERPRISE_TOKEN"),
        Some(&"ghp_codex_dummy_0000".to_string())
    );
    assert_eq!(headers.get(AUTHORIZATION), None);
    assert!(broker.host_requires_mitm("github.example.com"));

    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {enterprise_token}"))
            .expect("valid dummy enterprise header"),
    );
    broker.inject_request_headers("github.example.com", &mut headers);

    assert_eq!(
        headers.get(AUTHORIZATION),
        Some(&HeaderValue::from_static("Bearer ghp-enterprise-real"))
    );
}

#[test]
fn inject_request_headers_uses_dummy_to_select_ambiguous_github_credential() {
    let broker = CredentialBroker::new(/*enabled*/ true);
    let mut env = HashMap::from([
        ("GH_TOKEN".to_string(), "ghp-real-one".to_string()),
        ("GITHUB_TOKEN".to_string(), "ghp-real-two".to_string()),
    ]);
    broker.virtualize_child_env(&mut env);
    let github_token = env.get("GITHUB_TOKEN").expect("dummy github token");
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {github_token}")).expect("valid dummy header"),
    );

    broker.inject_request_headers("api.github.com", &mut headers);

    assert_eq!(
        headers.get(AUTHORIZATION),
        Some(&HeaderValue::from_static("Bearer ghp-real-two"))
    );
}

#[test]
fn inject_request_headers_skips_ambiguous_github_credential_without_dummy() {
    let broker = CredentialBroker::new(/*enabled*/ true);
    let mut env = HashMap::from([
        ("GH_TOKEN".to_string(), "ghp-real-one".to_string()),
        ("GITHUB_TOKEN".to_string(), "ghp-real-two".to_string()),
    ]);
    broker.virtualize_child_env(&mut env);
    let mut headers = HeaderMap::new();

    broker.inject_request_headers("api.github.com", &mut headers);

    assert_eq!(headers.get(AUTHORIZATION), None);
}

#[test]
fn inject_request_headers_uses_duplicate_real_github_credential_without_dummy() {
    let broker = CredentialBroker::new(/*enabled*/ true);
    let mut env = HashMap::from([
        ("GH_TOKEN".to_string(), "ghp-real".to_string()),
        ("GITHUB_TOKEN".to_string(), "ghp-real".to_string()),
    ]);
    broker.virtualize_child_env(&mut env);
    let mut headers = HeaderMap::new();

    broker.inject_request_headers("api.github.com", &mut headers);

    assert_eq!(
        headers.get(AUTHORIZATION),
        Some(&HeaderValue::from_static("Bearer ghp-real"))
    );
}

#[test]
fn inject_request_headers_uses_unique_openai_api_key_without_dummy_header() {
    let broker = CredentialBroker::new(/*enabled*/ true);
    let mut env = HashMap::from([("OPENAI_API_KEY".to_string(), "sk-real".to_string())]);
    broker.virtualize_child_env(&mut env);
    let mut headers = HeaderMap::new();

    broker.inject_request_headers("api.openai.com", &mut headers);

    assert_eq!(
        headers.get(AUTHORIZATION),
        Some(&HeaderValue::from_static("Bearer sk-real"))
    );
}

#[test]
fn inject_request_headers_preserves_explicit_authorization() {
    let broker = CredentialBroker::new(/*enabled*/ true);
    let mut env = HashMap::from([("OPENAI_API_KEY".to_string(), "sk-real".to_string())]);
    broker.virtualize_child_env(&mut env);
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_static("Bearer sk-explicit"),
    );

    broker.inject_request_headers("api.openai.com", &mut headers);

    assert_eq!(
        headers.get(AUTHORIZATION),
        Some(&HeaderValue::from_static("Bearer sk-explicit"))
    );
}

#[test]
fn github_cloud_credentials_match_ghe_com_host_hint() {
    let broker = CredentialBroker::new(/*enabled*/ true);
    let mut env = HashMap::from([
        ("GH_HOST".to_string(), "astemu.ghe.com".to_string()),
        ("GH_TOKEN".to_string(), "ghp-real".to_string()),
    ]);
    broker.virtualize_child_env(&mut env);
    let mut headers = HeaderMap::new();

    broker.inject_request_headers("api.astemu.ghe.com", &mut headers);

    assert_eq!(
        headers.get(AUTHORIZATION),
        Some(&HeaderValue::from_static("Bearer ghp-real"))
    );
}

#[test]
fn github_cloud_credentials_do_not_bind_to_ghes_host_hint() {
    let broker = CredentialBroker::new(/*enabled*/ true);
    let mut env = HashMap::from([
        ("GH_HOST".to_string(), "github.example.com".to_string()),
        ("GH_TOKEN".to_string(), "ghp-real".to_string()),
    ]);
    broker.virtualize_child_env(&mut env);
    let github_token = env.get("GH_TOKEN").expect("dummy github token");
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {github_token}")).expect("valid dummy header"),
    );

    broker.inject_request_headers("github.example.com", &mut headers);

    assert_eq!(
        headers.get(AUTHORIZATION),
        Some(
            &HeaderValue::from_str(&format!("Bearer {github_token}")).expect("valid dummy header")
        )
    );
    assert!(!broker.host_requires_mitm("github.example.com"));
    assert!(broker.host_requires_mitm("api.github.com"));
}

#[test]
fn github_enterprise_credentials_bind_to_gh_host() {
    let broker = CredentialBroker::new(/*enabled*/ true);
    let mut env = HashMap::from([
        ("GH_HOST".to_string(), "github.example.com".to_string()),
        (
            "GH_ENTERPRISE_TOKEN".to_string(),
            "ghp-enterprise-real".to_string(),
        ),
    ]);
    broker.virtualize_child_env(&mut env);
    let mut headers = HeaderMap::new();

    broker.inject_request_headers("github.example.com", &mut headers);

    assert_eq!(
        headers.get(AUTHORIZATION),
        Some(&HeaderValue::from_static("Bearer ghp-enterprise-real"))
    );
    assert!(broker.host_requires_mitm("github.example.com"));
    assert!(!broker.host_requires_mitm("api.github.com"));
}
