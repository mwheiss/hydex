use crate::app_event::PluginLocation;
use crate::bottom_pane::SelectionItem;
use codex_app_server_protocol::PluginAuthPolicy;
use codex_app_server_protocol::PluginAvailability;
use codex_app_server_protocol::PluginDetail;
use codex_app_server_protocol::PluginInstallPolicy;
use codex_app_server_protocol::PluginMarketplaceEntry;
use codex_app_server_protocol::PluginShareContext;
use codex_app_server_protocol::PluginShareDiscoverability;
use codex_app_server_protocol::PluginSharePrincipal;
use codex_app_server_protocol::PluginSource;
use codex_app_server_protocol::PluginSummary;
use codex_utils_absolute_path::AbsolutePathBuf;

use super::entries::marketplace_product_label_from_name;

#[derive(Debug, Clone)]
pub(in super::super) struct PreferredLocalPluginSource {
    remote_plugin_id: String,
    marketplace_path: AbsolutePathBuf,
    plugin_name: String,
    installed: bool,
}

pub(in super::super) fn plugin_display_name(plugin: &PluginSummary) -> String {
    plugin
        .interface
        .as_ref()
        .and_then(|interface| interface.display_name.as_deref())
        .map(str::trim)
        .filter(|display_name| !display_name.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| plugin.name.clone())
}

pub(in super::super) fn plugin_brief_description(
    plugin: &PluginSummary,
    marketplace_label: &str,
    status_label_width: usize,
) -> String {
    let status_label = plugin_status_label(plugin);
    let status_label = format!("{status_label:<status_label_width$}");
    match plugin_description(plugin) {
        Some(description) => format!("{status_label} · {marketplace_label} · {description}"),
        None => format!("{status_label} · {marketplace_label}"),
    }
}

pub(in super::super) fn plugin_brief_description_without_marketplace(
    plugin: &PluginSummary,
    status_label_width: usize,
) -> String {
    let status_label = plugin_status_label(plugin);
    let status_label = format!("{status_label:<status_label_width$}");
    match plugin_description(plugin) {
        Some(description) => format!("{status_label} · {description}"),
        None => status_label,
    }
}

pub(in super::super) fn plugin_status_label(plugin: &PluginSummary) -> &'static str {
    if plugin.availability == PluginAvailability::DisabledByAdmin {
        return "Disabled by admin";
    }
    if plugin.installed {
        if plugin.enabled {
            "Installed"
        } else {
            "Disabled"
        }
    } else {
        match plugin.install_policy {
            PluginInstallPolicy::NotAvailable => "Not installable",
            PluginInstallPolicy::Available => "Available",
            PluginInstallPolicy::InstalledByDefault => "Available by default",
        }
    }
}

pub(in super::super) fn plugin_detail_status_label(plugin: &PluginSummary) -> &'static str {
    if plugin.availability == PluginAvailability::DisabledByAdmin {
        return "Disabled by admin";
    }
    if plugin.installed {
        if plugin.enabled {
            "Installed"
        } else {
            "Disabled"
        }
    } else {
        match plugin.install_policy {
            PluginInstallPolicy::NotAvailable => "Not installable",
            PluginInstallPolicy::Available => "Can be installed",
            PluginInstallPolicy::InstalledByDefault => "Available by default",
        }
    }
}

pub(in super::super) fn plugin_detail_location(plugin: &PluginDetail) -> Option<PluginLocation> {
    if let Some(marketplace_path) = plugin.marketplace_path.clone() {
        return Some(PluginLocation::Local { marketplace_path });
    }
    plugin_remote_identity(&plugin.summary).map(|_| PluginLocation::Remote {
        marketplace_name: plugin.marketplace_name.clone(),
    })
}

pub(in super::super) fn plugin_detail_request_for_entry(
    marketplace: &PluginMarketplaceEntry,
    plugin: &PluginSummary,
    preferred_local_sources: &[PreferredLocalPluginSource],
) -> Option<(PluginLocation, String)> {
    if matches!(&plugin.source, PluginSource::Remote)
        && let Some(remote_plugin_id) = plugin_remote_identity(plugin)
        && let Some(preferred_source) = preferred_local_sources.iter().find(|source| {
            source.remote_plugin_id == remote_plugin_id && source.installed == plugin.installed
        })
    {
        return Some((
            PluginLocation::Local {
                marketplace_path: preferred_source.marketplace_path.clone(),
            },
            preferred_source.plugin_name.clone(),
        ));
    }

    plugin_location_for_marketplace(marketplace, plugin)
        .map(|location| (location, plugin_request_name(plugin)))
}

pub(in super::super) fn preferred_local_plugin_sources(
    marketplaces: &[&PluginMarketplaceEntry],
) -> Vec<PreferredLocalPluginSource> {
    let mut sources: Vec<PreferredLocalPluginSource> = Vec::new();
    let mut seen_remote_plugin_ids = std::collections::HashSet::new();
    for marketplace in marketplaces {
        let Some(marketplace_path) = marketplace.path.clone() else {
            continue;
        };
        for plugin in &marketplace.plugins {
            if matches!(&plugin.source, PluginSource::Remote) {
                continue;
            }
            let Some(remote_plugin_id) = plugin
                .share_context
                .as_ref()
                .map(|context| context.remote_plugin_id.clone())
            else {
                continue;
            };
            if !seen_remote_plugin_ids.insert(remote_plugin_id.clone()) {
                continue;
            }
            sources.push(PreferredLocalPluginSource {
                remote_plugin_id,
                marketplace_path: marketplace_path.clone(),
                plugin_name: plugin.name.clone(),
                installed: plugin.installed,
            });
        }
    }
    sources
}

pub(in super::super) fn plugin_request_name(plugin: &PluginSummary) -> String {
    if matches!(&plugin.source, PluginSource::Remote)
        && let Some(remote_plugin_id) = plugin_remote_identity(plugin)
    {
        return remote_plugin_id;
    }
    plugin.name.clone()
}

pub(in super::super) fn plugin_remote_identity(plugin: &PluginSummary) -> Option<String> {
    plugin
        .share_context
        .as_ref()
        .map(|context| context.remote_plugin_id.clone())
        .or_else(|| plugin.remote_plugin_id.clone())
}

pub(in super::super) fn plugin_uninstall_id(plugin: &PluginSummary) -> Option<String> {
    if matches!(&plugin.source, PluginSource::Remote) {
        return plugin_remote_identity(plugin);
    }
    Some(plugin.id.clone())
}

pub(in super::super) fn plugin_metadata_items(plugin: &PluginDetail) -> Vec<SelectionItem> {
    let mut items = Vec::new();
    items.push(SelectionItem {
        name: "Source".to_string(),
        description: Some(plugin_source_summary(plugin)),
        is_disabled: true,
        ..Default::default()
    });
    items.push(SelectionItem {
        name: "Auth".to_string(),
        description: Some(plugin_auth_policy_summary(plugin.summary.auth_policy)),
        is_disabled: true,
        ..Default::default()
    });
    if let Some(version) = plugin_version_summary(&plugin.summary) {
        items.push(SelectionItem {
            name: "Version".to_string(),
            description: Some(version),
            is_disabled: true,
            ..Default::default()
        });
    }
    if let Some(share_context) = &plugin.summary.share_context {
        items.push(SelectionItem {
            name: "Sharing".to_string(),
            description: Some(plugin_share_context_summary(share_context)),
            is_disabled: true,
            ..Default::default()
        });
    }
    items
}

pub(in super::super) fn plugin_description(plugin: &PluginSummary) -> Option<String> {
    plugin
        .interface
        .as_ref()
        .and_then(|interface| {
            interface
                .short_description
                .as_deref()
                .or(interface.long_description.as_deref())
        })
        .map(str::trim)
        .filter(|description| !description.is_empty())
        .map(str::to_string)
}

pub(in super::super) fn plugin_detail_description(plugin: &PluginDetail) -> Option<String> {
    plugin
        .description
        .as_deref()
        .or_else(|| {
            plugin
                .summary
                .interface
                .as_ref()
                .and_then(|interface| interface.long_description.as_deref())
        })
        .or_else(|| {
            plugin
                .summary
                .interface
                .as_ref()
                .and_then(|interface| interface.short_description.as_deref())
        })
        .map(str::trim)
        .filter(|description| !description.is_empty())
        .map(str::to_string)
}

fn plugin_location_for_marketplace(
    marketplace: &PluginMarketplaceEntry,
    plugin: &PluginSummary,
) -> Option<PluginLocation> {
    if let Some(marketplace_path) = marketplace.path.clone() {
        return Some(PluginLocation::Local { marketplace_path });
    }
    plugin_remote_identity(plugin).map(|_| PluginLocation::Remote {
        marketplace_name: marketplace.name.clone(),
    })
}

fn plugin_source_summary(plugin: &PluginDetail) -> String {
    match &plugin.summary.source {
        PluginSource::Local { .. } => "Local".to_string(),
        PluginSource::Git { url, ref_name, .. } => match ref_name {
            Some(ref_name) => format!("Git · {url}@{ref_name}"),
            None => format!("Git · {url}"),
        },
        PluginSource::Remote => {
            let marketplace_label = marketplace_product_label_from_name(&plugin.marketplace_name)
                .unwrap_or(plugin.marketplace_name.as_str());
            format!("Remote · {marketplace_label}")
        }
    }
}

fn plugin_auth_policy_summary(auth_policy: PluginAuthPolicy) -> String {
    match auth_policy {
        PluginAuthPolicy::OnInstall => "Auth on install".to_string(),
        PluginAuthPolicy::OnUse => "Auth on use".to_string(),
    }
}

fn plugin_version_summary(plugin: &PluginSummary) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(local_version) = plugin.local_version.as_deref() {
        parts.push(format!("local {local_version}"));
    }
    if let Some(remote_version) = plugin
        .share_context
        .as_ref()
        .and_then(|context| context.remote_version.as_deref())
    {
        parts.push(format!("remote {remote_version}"));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

fn plugin_share_context_summary(context: &PluginShareContext) -> String {
    let mut parts = Vec::new();
    if let Some(discoverability) = context.discoverability {
        parts.push(plugin_share_discoverability_label(discoverability).to_string());
    }
    if let Some(creator_summary) = plugin_share_creator_summary(context) {
        parts.push(creator_summary);
    }
    if let Some(principals) = context.share_principals.as_ref() {
        parts.push(plugin_share_principals_summary(principals));
    }
    if let Some(share_url) = context
        .share_url
        .as_deref()
        .filter(|url| !url.trim().is_empty())
    {
        parts.push(share_url.to_string());
    }
    if parts.is_empty() {
        format!("Remote ID {}", context.remote_plugin_id)
    } else {
        parts.join(" · ")
    }
}

fn plugin_share_discoverability_label(discoverability: PluginShareDiscoverability) -> &'static str {
    match discoverability {
        PluginShareDiscoverability::Listed => "Listed",
        PluginShareDiscoverability::Unlisted => "Workspace link",
        PluginShareDiscoverability::Private => "Private",
    }
}

fn plugin_share_creator_summary(context: &PluginShareContext) -> Option<String> {
    match (
        context.creator_name.as_deref(),
        context.creator_account_user_id.as_deref(),
    ) {
        (Some(name), Some(account_id)) => Some(format!("creator {name} ({account_id})")),
        (Some(name), None) => Some(format!("creator {name}")),
        (None, Some(account_id)) => Some(format!("creator account {account_id}")),
        (None, None) => None,
    }
}

fn plugin_share_principals_summary(principals: &[PluginSharePrincipal]) -> String {
    match principals.len() {
        0 => "No explicit principals".to_string(),
        1 => format!("1 principal: {}", principals[0].name),
        count => format!("{count} principals"),
    }
}
