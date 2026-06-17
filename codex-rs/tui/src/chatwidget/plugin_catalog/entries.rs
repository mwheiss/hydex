use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;

use super::details::plugin_display_name;
use super::details::plugin_remote_identity;
use crate::bottom_pane::SelectionTab;
use codex_app_server_protocol::PluginListResponse;
use codex_app_server_protocol::PluginMarketplaceEntry;
use codex_app_server_protocol::PluginSource;
use codex_app_server_protocol::PluginSummary;
use codex_core_plugins::is_openai_curated_marketplace_name;
use codex_core_plugins::remote::REMOTE_GLOBAL_MARKETPLACE_NAME;
use codex_core_plugins::remote::REMOTE_WORKSPACE_MARKETPLACE_NAME;
use codex_core_plugins::remote::REMOTE_WORKSPACE_SHARED_WITH_ME_MARKETPLACE_NAME;
use codex_core_plugins::remote::REMOTE_WORKSPACE_SHARED_WITH_ME_PRIVATE_MARKETPLACE_NAME;
use codex_core_plugins::remote::REMOTE_WORKSPACE_SHARED_WITH_ME_UNLISTED_MARKETPLACE_NAME;
use codex_utils_absolute_path::AbsolutePathBuf;

const MARKETPLACE_TAB_ID_PREFIX: &str = "marketplace:";
const PERSONAL_MARKETPLACE_RELATIVE_PATH: &str = ".agents/plugins/marketplace.json";
const WORKSPACE_SECTION_MARKETPLACE_NAMES: &[&str] = &[REMOTE_WORKSPACE_MARKETPLACE_NAME];
const SHARED_WITH_ME_SECTION_MARKETPLACE_NAMES: &[&str] = &[
    REMOTE_WORKSPACE_SHARED_WITH_ME_MARKETPLACE_NAME,
    REMOTE_WORKSPACE_SHARED_WITH_ME_PRIVATE_MARKETPLACE_NAME,
    REMOTE_WORKSPACE_SHARED_WITH_ME_UNLISTED_MARKETPLACE_NAME,
];
const WORKSPACE_SECTION_TAB_IDS: &[&str] = &[
    "marketplace:workspace-directory",
    "remote-loading:workspace-loading",
    "remote-empty:workspace",
    "remote-error:workspace",
];
const SHARED_WITH_ME_SECTION_TAB_IDS: &[&str] = &[
    "marketplace:workspace-shared-with-me",
    "marketplace:workspace-shared-with-me-private",
    "marketplace:workspace-shared-with-me-unlisted",
    "remote-loading:shared-with-me-loading",
    "remote-empty:shared-with-me",
    "remote-error:shared-with-me",
];
const WORKSPACE_SECTION_TAB_ORDER: u8 = 0;
const SHARED_WITH_ME_SECTION_TAB_ORDER: u8 = 1;
const SHARED_WITH_ME_LINK_SECTION_TAB_ORDER: u8 = 2;
const LOCAL_MARKETPLACE_TAB_ORDER: u8 = 3;
const OTHER_MARKETPLACE_TAB_ORDER: u8 = 4;

#[derive(Debug, Clone, Copy)]
enum MarketplaceProduct {
    OpenAiCurated,
    Workspace,
    SharedWithMe,
    SharedWithMeLink,
    Local,
    Other,
}

impl MarketplaceProduct {
    fn from_marketplace(marketplace: &PluginMarketplaceEntry) -> Self {
        if marketplace
            .path
            .as_ref()
            .is_some_and(is_personal_marketplace_path)
        {
            return Self::Local;
        }

        Self::from_marketplace_name(&marketplace.name)
    }

    fn from_marketplace_name(marketplace_name: &str) -> Self {
        if is_openai_curated_marketplace_name(marketplace_name)
            || marketplace_name == REMOTE_GLOBAL_MARKETPLACE_NAME
        {
            return Self::OpenAiCurated;
        }

        match marketplace_name {
            REMOTE_WORKSPACE_MARKETPLACE_NAME => Self::Workspace,
            REMOTE_WORKSPACE_SHARED_WITH_ME_MARKETPLACE_NAME
            | REMOTE_WORKSPACE_SHARED_WITH_ME_PRIVATE_MARKETPLACE_NAME => Self::SharedWithMe,
            REMOTE_WORKSPACE_SHARED_WITH_ME_UNLISTED_MARKETPLACE_NAME => Self::SharedWithMeLink,
            _ => Self::Other,
        }
    }

    fn label(self) -> Option<&'static str> {
        match self {
            Self::OpenAiCurated => Some("OpenAI Curated"),
            Self::Workspace => Some("Workspace"),
            Self::SharedWithMe => Some("Shared with me"),
            Self::SharedWithMeLink => Some("Shared with me (link)"),
            Self::Local => Some("Local"),
            Self::Other => None,
        }
    }

    fn tab_order(self) -> u8 {
        match self {
            Self::Workspace => WORKSPACE_SECTION_TAB_ORDER,
            Self::SharedWithMe => SHARED_WITH_ME_SECTION_TAB_ORDER,
            Self::SharedWithMeLink => SHARED_WITH_ME_LINK_SECTION_TAB_ORDER,
            Self::Local => LOCAL_MARKETPLACE_TAB_ORDER,
            Self::OpenAiCurated | Self::Other => OTHER_MARKETPLACE_TAB_ORDER,
        }
    }

    fn is_by_openai(self) -> bool {
        matches!(self, Self::OpenAiCurated)
    }
}

pub(in super::super) fn plugin_entries_for_marketplaces<'a>(
    marketplaces: impl IntoIterator<Item = &'a PluginMarketplaceEntry>,
) -> Vec<(&'a PluginMarketplaceEntry, &'a PluginSummary, String)> {
    let entries = marketplaces
        .into_iter()
        .flat_map(|marketplace| {
            marketplace
                .plugins
                .iter()
                .map(move |plugin| (marketplace, plugin, plugin_display_name(plugin)))
        })
        .collect::<Vec<_>>();
    dedupe_plugin_entries(entries)
}

fn dedupe_plugin_entries<'a>(
    entries: Vec<(&'a PluginMarketplaceEntry, &'a PluginSummary, String)>,
) -> Vec<(&'a PluginMarketplaceEntry, &'a PluginSummary, String)> {
    // App-server should eventually normalize local/remote duplicates. Keep this
    // display-only pass narrow so shared plugins do not appear twice meanwhile.
    let mut deduped: Vec<(&PluginMarketplaceEntry, &PluginSummary, String)> = Vec::new();
    let mut remote_entry_indexes = HashMap::new();
    for entry in entries {
        let Some(remote_plugin_id) = plugin_remote_identity(entry.1) else {
            deduped.push(entry);
            continue;
        };
        if let Some(existing_index) = remote_entry_indexes.get(&remote_plugin_id).copied() {
            if plugin_entry_preferred(&entry, &deduped[existing_index]) {
                deduped[existing_index] = entry;
            }
        } else {
            remote_entry_indexes.insert(remote_plugin_id, deduped.len());
            deduped.push(entry);
        }
    }
    deduped
}

fn plugin_entry_preferred(
    candidate: &(&PluginMarketplaceEntry, &PluginSummary, String),
    existing: &(&PluginMarketplaceEntry, &PluginSummary, String),
) -> bool {
    if candidate.1.installed != existing.1.installed {
        return candidate.1.installed;
    }

    let candidate_is_local_share =
        candidate.1.share_context.is_some() && !matches!(&candidate.1.source, PluginSource::Remote);
    let existing_is_local_share =
        existing.1.share_context.is_some() && !matches!(&existing.1.source, PluginSource::Remote);
    if candidate_is_local_share != existing_is_local_share {
        return candidate_is_local_share;
    }

    !matches!(&candidate.1.source, PluginSource::Remote)
        && matches!(&existing.1.source, PluginSource::Remote)
}

pub(in super::super) fn sort_plugin_entries(
    entries: &mut [(&PluginMarketplaceEntry, &PluginSummary, String)],
) {
    entries.sort_by(|left, right| {
        right
            .1
            .installed
            .cmp(&left.1.installed)
            .then_with(|| {
                left.2
                    .to_ascii_lowercase()
                    .cmp(&right.2.to_ascii_lowercase())
            })
            .then_with(|| left.2.cmp(&right.2))
            .then_with(|| left.1.name.cmp(&right.1.name))
            .then_with(|| left.1.id.cmp(&right.1.id))
    });
}

pub(in super::super) fn marketplace_tab_id(marketplace: &PluginMarketplaceEntry) -> String {
    match marketplace.path.as_ref() {
        Some(path) => marketplace_tab_id_from_path(path.as_path()),
        None => format!("marketplace:{}", marketplace.name),
    }
}

pub(in super::super) fn marketplace_tab_id_from_path(path: &Path) -> String {
    format!("{MARKETPLACE_TAB_ID_PREFIX}{}", path.display())
}

pub(in super::super) fn marketplace_tab_id_matching_saved_id(
    saved_tab_id: &str,
    marketplaces: &[PluginMarketplaceEntry],
) -> Option<String> {
    if let Some(tab_id) = remote_section_marketplace_tab_id(saved_tab_id, marketplaces) {
        return Some(tab_id);
    }

    if let Some(tab_id) = marketplaces.iter().find_map(|marketplace| {
        let tab_id = marketplace_tab_id(marketplace);
        (tab_id == saved_tab_id).then_some(tab_id)
    }) {
        return Some(tab_id);
    }

    let root = saved_tab_id.strip_prefix(MARKETPLACE_TAB_ID_PREFIX)?;
    if root.is_empty() {
        return None;
    }
    let root = Path::new(root);
    marketplaces.iter().find_map(|marketplace| {
        marketplace
            .path
            .as_ref()
            .is_some_and(|path| path.as_path().starts_with(root))
            .then(|| marketplace_tab_id(marketplace))
    })
}

fn remote_section_marketplace_tab_id(
    saved_tab_id: &str,
    marketplaces: &[PluginMarketplaceEntry],
) -> Option<String> {
    let marketplace_name_matches = match saved_tab_id {
        "remote-loading:workspace-loading"
        | "remote-empty:workspace"
        | "remote-error:workspace" => WORKSPACE_SECTION_MARKETPLACE_NAMES,
        "remote-loading:shared-with-me-loading"
        | "remote-empty:shared-with-me"
        | "remote-error:shared-with-me" => SHARED_WITH_ME_SECTION_MARKETPLACE_NAMES,
        _ => return None,
    };

    marketplace_name_matches
        .iter()
        .find_map(|marketplace_name| {
            marketplaces
                .iter()
                .find(|marketplace| marketplace.name.as_str() == *marketplace_name)
                .map(marketplace_tab_id)
        })
}

pub(in super::super) fn plugin_tab_id_matching_saved_id(
    saved_tab_id: &str,
    tabs: &[SelectionTab],
) -> Option<String> {
    if let Some(tab_id) = tabs
        .iter()
        .find(|tab| tab.id.as_str() == saved_tab_id)
        .map(|tab| tab.id.clone())
    {
        return Some(tab_id);
    }

    let candidate_tab_ids = match saved_tab_id {
        "remote-loading:workspace-loading"
        | "remote-empty:workspace"
        | "remote-error:workspace"
        | "marketplace:workspace-directory" => WORKSPACE_SECTION_TAB_IDS,
        "remote-loading:shared-with-me-loading"
        | "remote-empty:shared-with-me"
        | "remote-error:shared-with-me"
        | "marketplace:workspace-shared-with-me"
        | "marketplace:workspace-shared-with-me-private"
        | "marketplace:workspace-shared-with-me-unlisted" => SHARED_WITH_ME_SECTION_TAB_IDS,
        _ => return None,
    };

    tabs.iter()
        .find(|tab| candidate_tab_ids.contains(&tab.id.as_str()))
        .map(|tab| tab.id.clone())
}

pub(in super::super) fn merge_remote_marketplaces(
    response: &mut PluginListResponse,
    remote_marketplaces: Vec<PluginMarketplaceEntry>,
) {
    let remote_names = remote_marketplaces
        .iter()
        .map(|marketplace| marketplace.name.clone())
        .collect::<HashSet<_>>();
    response.marketplaces.retain(|marketplace| {
        marketplace.path.is_some()
            || !remote_marketplace_is_remote_section(marketplace)
                && !remote_names.contains(marketplace.name.as_str())
    });
    response.marketplaces.extend(remote_marketplaces);
}

fn remote_marketplace_is_remote_section(marketplace: &PluginMarketplaceEntry) -> bool {
    matches!(
        marketplace.name.as_str(),
        REMOTE_WORKSPACE_MARKETPLACE_NAME
            | REMOTE_WORKSPACE_SHARED_WITH_ME_MARKETPLACE_NAME
            | REMOTE_WORKSPACE_SHARED_WITH_ME_PRIVATE_MARKETPLACE_NAME
            | REMOTE_WORKSPACE_SHARED_WITH_ME_UNLISTED_MARKETPLACE_NAME
    )
}

pub(in super::super) fn disambiguate_duplicate_tab_labels(labels: Vec<String>) -> Vec<String> {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for label in &labels {
        if let Some((_, count)) = counts.iter_mut().find(|(existing, _)| existing == label) {
            *count += 1;
        } else {
            counts.push((label.clone(), 1));
        }
    }

    let mut seen: Vec<(String, usize)> = Vec::new();
    labels
        .into_iter()
        .map(|label| {
            let total = counts
                .iter()
                .find(|(existing, _)| existing == &label)
                .map(|(_, count)| *count)
                .unwrap_or(1);
            if total == 1 {
                return label;
            }

            let current = if let Some((_, seen_count)) =
                seen.iter_mut().find(|(existing, _)| existing == &label)
            {
                *seen_count += 1;
                *seen_count
            } else {
                seen.push((label.clone(), 1));
                1
            };
            format!("{label} ({current}/{total})")
        })
        .collect()
}

pub(in super::super) fn marketplace_display_name(marketplace: &PluginMarketplaceEntry) -> String {
    if let Some(label) = MarketplaceProduct::from_marketplace(marketplace).label() {
        return label.to_string();
    }
    marketplace
        .interface
        .as_ref()
        .and_then(|interface| interface.display_name.as_deref())
        .map(str::trim)
        .filter(|display_name| !display_name.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| marketplace.name.clone())
}

pub(in super::super) fn marketplace_product_tab_order(marketplace: &PluginMarketplaceEntry) -> u8 {
    MarketplaceProduct::from_marketplace(marketplace).tab_order()
}

pub(in super::super) fn marketplace_product_label_from_name(
    marketplace_name: &str,
) -> Option<&str> {
    MarketplaceProduct::from_marketplace_name(marketplace_name).label()
}

pub(in super::super) fn marketplace_is_by_openai(marketplace: &PluginMarketplaceEntry) -> bool {
    MarketplaceProduct::from_marketplace(marketplace).is_by_openai()
}

fn is_personal_marketplace_path(marketplace_path: &AbsolutePathBuf) -> bool {
    dirs::home_dir()
        .and_then(|home| personal_marketplace_path_from_home(home.as_path()))
        .is_some_and(|personal_path| personal_path.as_path() == marketplace_path.as_path())
}

fn personal_marketplace_path_from_home(home: &Path) -> Option<AbsolutePathBuf> {
    AbsolutePathBuf::try_from(home.join(PERSONAL_MARKETPLACE_RELATIVE_PATH)).ok()
}
