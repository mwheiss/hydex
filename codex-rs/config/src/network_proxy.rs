use crate::permissions_toml::NetworkDomainPermissionToml;
use crate::permissions_toml::NetworkDomainPermissionsToml;
use crate::permissions_toml::NetworkToml;
use crate::permissions_toml::NetworkUnixSocketPermissionToml;
use crate::permissions_toml::NetworkUnixSocketPermissionsToml;
use codex_features::NetworkProxyConfigToml;
use codex_features::NetworkProxyDomainPermissionToml;
use codex_features::NetworkProxyModeToml;
use codex_features::NetworkProxyUnixSocketPermissionToml;
use codex_network_proxy::NetworkMode;
use codex_network_proxy::NetworkProxyConfig;

pub fn apply_network_proxy_feature_config(
    config: &mut NetworkProxyConfig,
    feature_config: &NetworkProxyConfigToml,
) {
    NetworkToml {
        enabled: feature_config.enabled,
        proxy_url: feature_config.proxy_url.clone(),
        enable_socks5: feature_config.enable_socks5,
        socks_url: feature_config.socks_url.clone(),
        enable_socks5_udp: feature_config.enable_socks5_udp,
        allow_upstream_proxy: feature_config.allow_upstream_proxy,
        dangerously_allow_non_loopback_proxy: feature_config.dangerously_allow_non_loopback_proxy,
        dangerously_allow_all_unix_sockets: feature_config.dangerously_allow_all_unix_sockets,
        mode: feature_config.mode.map(|mode| match mode {
            NetworkProxyModeToml::Limited => NetworkMode::Limited,
            NetworkProxyModeToml::Full => NetworkMode::Full,
        }),
        domains: feature_config
            .domains
            .as_ref()
            .map(|domains| NetworkDomainPermissionsToml {
                entries: domains
                    .iter()
                    .map(|(pattern, permission)| {
                        let permission = match permission {
                            NetworkProxyDomainPermissionToml::Allow => {
                                NetworkDomainPermissionToml::Allow
                            }
                            NetworkProxyDomainPermissionToml::Deny => {
                                NetworkDomainPermissionToml::Deny
                            }
                        };
                        (pattern.clone(), permission)
                    })
                    .collect(),
            }),
        unix_sockets: feature_config.unix_sockets.as_ref().map(|unix_sockets| {
            NetworkUnixSocketPermissionsToml {
                entries: unix_sockets
                    .iter()
                    .map(|(path, permission)| {
                        let permission = match permission {
                            NetworkProxyUnixSocketPermissionToml::Allow => {
                                NetworkUnixSocketPermissionToml::Allow
                            }
                            NetworkProxyUnixSocketPermissionToml::Deny => {
                                NetworkUnixSocketPermissionToml::Deny
                            }
                        };
                        (path.clone(), permission)
                    })
                    .collect(),
            }
        }),
        allow_local_binding: feature_config.allow_local_binding,
        mitm: None,
    }
    .apply_to_network_proxy_config(config);
    if let Some(credential_broker) = feature_config.credential_broker {
        config.set_credential_broker_enabled(credential_broker);
    }
}

#[cfg(test)]
#[path = "network_proxy_tests.rs"]
mod tests;
