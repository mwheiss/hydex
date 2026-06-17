use crate::app_event::PluginRemoteSectionError;
use crate::bottom_pane::SelectionItem;
use crate::bottom_pane::SelectionTab;
use crate::render::renderable::ColumnRenderable;
use crate::render::renderable::Renderable;
use codex_app_server_protocol::MarketplaceLoadErrorInfo;
use codex_app_server_protocol::PluginMarketplaceEntry;
use codex_core_plugins::remote::REMOTE_WORKSPACE_MARKETPLACE_NAME;
use codex_core_plugins::remote::REMOTE_WORKSPACE_SHARED_WITH_ME_MARKETPLACE_NAME;
use codex_core_plugins::remote::REMOTE_WORKSPACE_SHARED_WITH_ME_PRIVATE_MARKETPLACE_NAME;
use codex_core_plugins::remote::REMOTE_WORKSPACE_SHARED_WITH_ME_UNLISTED_MARKETPLACE_NAME;
use ratatui::style::Stylize;
use ratatui::text::Line;

const WORKSPACE_SECTION_MARKETPLACE_NAMES: &[&str] = &[REMOTE_WORKSPACE_MARKETPLACE_NAME];
const SHARED_WITH_ME_SECTION_MARKETPLACE_NAMES: &[&str] = &[
    REMOTE_WORKSPACE_SHARED_WITH_ME_MARKETPLACE_NAME,
    REMOTE_WORKSPACE_SHARED_WITH_ME_PRIVATE_MARKETPLACE_NAME,
    REMOTE_WORKSPACE_SHARED_WITH_ME_UNLISTED_MARKETPLACE_NAME,
];
const WORKSPACE_SECTION_FALLBACK_TAB_ORDER: u8 = 5;
const SHARED_WITH_ME_SECTION_FALLBACK_TAB_ORDER: u8 = 6;

#[derive(Debug, Clone, Copy)]
pub(in super::super) enum RemoteMarketplaceSection {
    Workspace,
    SharedWithMe,
}

impl RemoteMarketplaceSection {
    pub(in super::super) fn fallback_tab(
        self,
        marketplaces: &[&PluginMarketplaceEntry],
        remote_sections_loading: bool,
        remote_sections_loaded: bool,
        section_errors: &[PluginRemoteSectionError],
    ) -> Option<(u8, SelectionTab)> {
        if self.marketplace_names().iter().any(|marketplace_name| {
            marketplaces
                .iter()
                .any(|marketplace| marketplace.name.as_str() == *marketplace_name)
        }) {
            return None;
        }

        let tab = if remote_sections_loading {
            remote_section_loading_tab(self.loading_tab_id(), self.label())
        } else if remote_sections_loaded {
            if let Some(section_error) = plugin_remote_section_error(section_errors, self.id()) {
                remote_section_error_tab(section_error)
            } else {
                remote_section_empty_tab(
                    self.id(),
                    self.label(),
                    self.empty_item_name(),
                    self.empty_item_description(),
                )
            }
        } else {
            return None;
        };

        Some((self.fallback_tab_order(), tab))
    }

    fn id(self) -> &'static str {
        match self {
            Self::Workspace => "workspace",
            Self::SharedWithMe => "shared-with-me",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Workspace => "Workspace",
            Self::SharedWithMe => "Shared with me",
        }
    }

    fn marketplace_names(self) -> &'static [&'static str] {
        match self {
            Self::Workspace => WORKSPACE_SECTION_MARKETPLACE_NAMES,
            Self::SharedWithMe => SHARED_WITH_ME_SECTION_MARKETPLACE_NAMES,
        }
    }

    fn loading_tab_id(self) -> &'static str {
        match self {
            Self::Workspace => "workspace-loading",
            Self::SharedWithMe => "shared-with-me-loading",
        }
    }

    fn empty_item_name(self) -> &'static str {
        match self {
            Self::Workspace => "No workspace plugins available",
            Self::SharedWithMe => "No shared plugins available",
        }
    }

    fn empty_item_description(self) -> &'static str {
        match self {
            Self::Workspace => "No workspace directory plugins are available.",
            Self::SharedWithMe => "No plugins have been shared with you.",
        }
    }

    fn fallback_tab_order(self) -> u8 {
        match self {
            Self::Workspace => WORKSPACE_SECTION_FALLBACK_TAB_ORDER,
            Self::SharedWithMe => SHARED_WITH_ME_SECTION_FALLBACK_TAB_ORDER,
        }
    }
}

pub(in super::super) fn plugins_header(
    subtitle: String,
    count_line: String,
) -> Box<dyn Renderable> {
    let mut header = ColumnRenderable::new();
    header.push(Line::from("Plugins".bold()));
    header.push(Line::from(subtitle.dim()));
    header.push(Line::from(count_line.dim()));
    Box::new(header)
}

pub(in super::super) fn remote_section_loading_item(label: &str) -> SelectionItem {
    SelectionItem {
        name: format!("Loading {label} plugins..."),
        description: Some("This section updates when app-server returns it.".to_string()),
        is_disabled: true,
        ..Default::default()
    }
}

pub(in super::super) fn remote_section_error_item(label: &str, message: &str) -> SelectionItem {
    SelectionItem {
        name: format!("{label} unavailable"),
        description: Some(message.to_string()),
        is_disabled: true,
        ..Default::default()
    }
}

pub(in super::super) fn plugin_remote_section_error<'a>(
    section_errors: &'a [PluginRemoteSectionError],
    section_id: &str,
) -> Option<&'a PluginRemoteSectionError> {
    section_errors
        .iter()
        .find(|section_error| section_error.section_id == section_id)
}

pub(in super::super) fn append_marketplace_load_error_items(
    items: &mut Vec<SelectionItem>,
    load_errors: &[MarketplaceLoadErrorInfo],
) {
    for load_error in load_errors {
        let marketplace_path = load_error.marketplace_path.as_path().display();
        let description = format!("{marketplace_path}: {}", load_error.message);
        items.push(SelectionItem {
            name: "Marketplace unavailable".to_string(),
            description: Some(description.clone()),
            selected_description: Some(description.clone()),
            search_value: Some(description),
            is_disabled: true,
            ..Default::default()
        });
    }
}

fn remote_section_loading_tab(id: &str, label: &str) -> SelectionTab {
    SelectionTab {
        id: format!("remote-loading:{id}"),
        label: label.to_string(),
        header: plugins_header(
            format!("Loading {label} plugins."),
            "Local plugin functionality is already available.".to_string(),
        ),
        items: vec![remote_section_loading_item(label)],
    }
}

fn remote_section_empty_tab(
    id: &str,
    label: &str,
    item_name: &str,
    item_description: &str,
) -> SelectionTab {
    SelectionTab {
        id: format!("remote-empty:{id}"),
        label: label.to_string(),
        header: plugins_header(
            format!("{label}."),
            "This section loaded successfully.".to_string(),
        ),
        items: vec![SelectionItem {
            name: item_name.to_string(),
            description: Some(item_description.to_string()),
            is_disabled: true,
            ..Default::default()
        }],
    }
}

fn remote_section_error_tab(section_error: &PluginRemoteSectionError) -> SelectionTab {
    SelectionTab {
        id: format!("remote-error:{}", section_error.section_id),
        label: section_error.label.clone(),
        header: plugins_header(
            format!("{} unavailable.", section_error.label),
            "Local plugin functionality is still available.".to_string(),
        ),
        items: vec![remote_section_error_item(
            &section_error.label,
            &section_error.message,
        )],
    }
}
