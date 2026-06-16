use super::*;
use pretty_assertions::assert_eq;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;

#[test]
fn build_remote_marketplace_preserves_directory_order_and_appends_installed_only_plugins() {
    let directory_plugins = vec![
        directory_plugin("plugin-z", "zulu"),
        directory_plugin("plugin-m", "mike"),
    ];
    let installed_plugins = vec![RemotePluginInstalledItem {
        plugin: directory_plugin("plugin-a", "alpha"),
        enabled: true,
        disabled_skill_names: Vec::new(),
    }];

    let marketplace = build_remote_marketplace(
        "marketplace",
        "Marketplace",
        directory_plugins,
        installed_plugins,
        /*include_installed_only*/ true,
    )
    .expect("marketplace should be valid")
    .expect("marketplace should not be empty");

    assert_eq!(
        marketplace
            .plugins
            .into_iter()
            .map(|plugin| plugin.remote_plugin_id)
            .collect::<Vec<_>>(),
        vec!["plugin-z", "plugin-m", "plugin-a"]
    );
}

fn directory_plugin(id: &str, name: &str) -> RemotePluginDirectoryItem {
    RemotePluginDirectoryItem {
        id: id.to_string(),
        name: name.to_string(),
        scope: RemotePluginScope::Global,
        discoverability: None,
        creator_account_user_id: None,
        creator_name: None,
        share_url: None,
        share_principals: None,
        installation_policy: PluginInstallPolicy::Available,
        authentication_policy: PluginAuthPolicy::OnUse,
        availability: PluginAvailability::Available,
        release: RemotePluginReleaseResponse {
            version: None,
            display_name: name.to_string(),
            description: String::new(),
            bundle_download_url: None,
            app_ids: Vec::new(),
            app_manifest: None,
            app_templates: Vec::new(),
            keywords: Vec::new(),
            interface: RemotePluginReleaseInterfaceResponse {
                short_description: None,
                long_description: None,
                developer_name: None,
                category: None,
                capabilities: Vec::new(),
                website_url: None,
                privacy_policy_url: None,
                terms_of_service_url: None,
                brand_color: None,
                default_prompt: None,
                default_prompts: None,
                composer_icon_url: None,
                logo_url: None,
                screenshot_urls: Vec::new(),
            },
            skills: Vec::new(),
            mcp_servers: Vec::new(),
        },
    }
}

#[tokio::test]
async fn plugin_service_request_does_not_add_preview_cookie_when_disabled() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let response = send_plugin_service_request_with_preview(
        reqwest::Client::new().get(server.uri()),
        /*preview_enabled*/ false,
    )
    .await
    .expect("request should succeed");
    assert!(response.status().is_success());

    let requests = server
        .received_requests()
        .await
        .expect("request recording should be available");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].headers.get("cookie"), None);
}

#[tokio::test]
async fn plugin_service_request_sanitizes_and_preserves_caller_cookies() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let response = send_plugin_service_request_with_preview(
        reqwest::Client::new().get(server.uri()).header(
            COOKIE,
            "session=abc; oai-chat-plugin-service-preview=false; theme=dark",
        ),
        /*preview_enabled*/ true,
    )
    .await
    .expect("request should succeed");
    assert!(response.status().is_success());

    let requests = server
        .received_requests()
        .await
        .expect("request recording should be available");
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0]
            .headers
            .get("cookie")
            .and_then(|value| value.to_str().ok()),
        Some("session=abc; theme=dark; oai-chat-plugin-service-preview=true"),
    );
}
