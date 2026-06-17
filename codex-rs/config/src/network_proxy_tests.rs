use super::*;

use pretty_assertions::assert_eq;

#[test]
fn feature_config_enables_credential_broker_and_mitm() {
    let mut config = NetworkProxyConfig::default();
    let mut expected = NetworkProxyConfig::default();
    expected.network.enabled = true;
    expected.network.credential_broker = true;
    expected.network.mitm = true;

    apply_network_proxy_feature_config(
        &mut config,
        &NetworkProxyConfigToml {
            enabled: Some(true),
            credential_broker: Some(true),
            ..Default::default()
        },
    );

    assert_eq!(config, expected);
}
