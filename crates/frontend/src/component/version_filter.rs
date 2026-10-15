//! The Minecraft version a content search is narrowed to. Browsing for an instance starts on the
//! instance's own version, but any release can be picked instead, since content made for a
//! neighbouring release (a 1.21 mod for a 1.21.1 instance) often runs anyway.

use gpui::{prelude::*, *};
use gpui_component::{
    IndexPath,
    select::{Select, SelectDelegate, SelectEvent, SelectItem, SelectState},
};
use schema::version_manifest::{MinecraftVersionManifest, MinecraftVersionType};
use ustr::Ustr;

use crate::entity::metadata::{AsMetadataResult, FrontendMetadata, FrontendMetadataResult, FrontendMetadataState};

#[derive(Clone, PartialEq)]
pub struct VersionFilterItem {
    label: SharedString,
    version: Option<Ustr>,
}

impl VersionFilterItem {
    fn new(label: impl Into<SharedString>, version: Option<Ustr>) -> Self {
        Self {
            label: label.into(),
            version,
        }
    }
}

impl SelectItem for VersionFilterItem {
    type Value = Option<Ustr>;

    fn title(&self) -> SharedString {
        self.label.clone()
    }

    /// The narrow filter column only has room for the version itself on the dropdown's button.
    fn display_title(&self) -> Option<AnyElement> {
        let version = match self.version {
            Some(version) => version.as_str(),
            None => t::instance::content::version_filter::any_short(),
        };
        Some(format!("{}: {version}", t::instance::version()).into_any_element())
    }

    fn value(&self) -> &Self::Value {
        &self.version
    }
}

#[derive(Default)]
pub struct VersionFilterList {
    items: Vec<VersionFilterItem>,
    matched: Vec<VersionFilterItem>,
}

impl SelectDelegate for VersionFilterList {
    type Item = VersionFilterItem;

    fn items_count(&self, _section: usize) -> usize {
        self.matched.len()
    }

    fn item(&self, ix: IndexPath) -> Option<&Self::Item> {
        self.matched.get(ix.row)
    }

    fn position<V>(&self, value: &V) -> Option<IndexPath>
    where
        Self::Item: SelectItem<Value = V>,
        V: PartialEq,
    {
        self.matched
            .iter()
            .position(|item| item.value() == value)
            .map(|ix| IndexPath::default().row(ix))
    }

    fn perform_search(&mut self, query: &str, _window: &mut Window, _cx: &mut App) -> Task<()> {
        let query = query.trim().to_lowercase();
        self.matched = self
            .items
            .iter()
            .filter(|item| query.is_empty() || item.label.to_lowercase().contains(&query))
            .cloned()
            .collect();
        Task::ready(())
    }
}

/// A searchable dropdown of "Any version", the instance's version and every release.
pub struct VersionFilter {
    pub select: Entity<SelectState<VersionFilterList>>,
    versions: Entity<FrontendMetadataState>,
    instance_version: Option<Ustr>,
    _subscriptions: [Subscription; 2],
}

impl VersionFilter {
    /// `on_change` is called with the newly picked version, or none for any version.
    pub fn new<P: 'static>(
        selected: Option<Ustr>,
        instance_version: Option<Ustr>,
        metadata: &Entity<FrontendMetadata>,
        window: &mut Window,
        cx: &mut Context<P>,
        on_change: impl Fn(&mut P, Option<Ustr>, &mut Window, &mut Context<P>) + 'static,
    ) -> Self {
        let select = cx.new(|cx| SelectState::new(VersionFilterList::default(), None, window, cx).searchable(true));
        let versions = FrontendMetadata::request(metadata, bridge::meta::MetadataRequest::MinecraftVersionManifest, cx);

        let select_subscription =
            cx.subscribe_in(&select, window, move |page, _, event: &SelectEvent<VersionFilterList>, window, cx| {
                if let SelectEvent::Confirm(Some(version)) = event {
                    on_change(page, *version, window, cx);
                }
            });

        let versions_subscription = cx.observe_in(&versions, window, {
            let select = select.clone();
            let versions = versions.clone();
            move |_, _, window, cx| {
                let selected = select.read(cx).selected_value().copied().flatten();
                let items = Self::items(&versions, instance_version, selected, cx);
                select.update(cx, |state, cx| Self::set_items(state, items, selected, window, cx));
            }
        });

        let items = Self::items(&versions, instance_version, selected, cx);
        select.update(cx, |state, cx| Self::set_items(state, items, selected, window, cx));

        Self {
            select,
            versions,
            instance_version,
            _subscriptions: [select_subscription, versions_subscription],
        }
    }

    /// Picks `version` without anyone having chosen it, such as when the filters reset.
    pub fn set_selected(&self, version: Option<Ustr>, window: &mut Window, cx: &mut App) {
        let items = Self::items(&self.versions, self.instance_version, version, cx);
        self.select.update(cx, |state, cx| Self::set_items(state, items, version, window, cx));
    }

    pub fn render(&self) -> Select<VersionFilterList> {
        Select::new(&self.select).search_placeholder(t::common::search())
    }

    fn items(
        versions: &Entity<FrontendMetadataState>,
        instance_version: Option<Ustr>,
        selected: Option<Ustr>,
        cx: &App,
    ) -> Vec<VersionFilterItem> {
        let mut items = vec![VersionFilterItem::new(t::instance::content::version_filter::any(), None)];
        if let Some(instance_version) = instance_version {
            items.push(VersionFilterItem::new(
                t::instance::content::version_filter::instance(&instance_version),
                Some(instance_version),
            ));
        }

        let result: FrontendMetadataResult<MinecraftVersionManifest> = versions.read(cx).result();
        if let FrontendMetadataResult::Loaded(manifest) = result {
            for version in &manifest.versions {
                if Some(version.id) == instance_version {
                    continue;
                }
                // Snapshots would bury the releases, and content is rarely made for them
                if matches!(version.r#type, MinecraftVersionType::Release) || Some(version.id) == selected {
                    items.push(VersionFilterItem::new(version.id.as_str(), Some(version.id)));
                }
            }
        }

        // A version picked before the list loaded, or one the list doesn't have, stays offered
        if let Some(selected) = selected
            && !items.iter().any(|item| item.version == Some(selected))
        {
            items.push(VersionFilterItem::new(selected.as_str(), Some(selected)));
        }
        items
    }

    fn set_items(
        state: &mut SelectState<VersionFilterList>,
        items: Vec<VersionFilterItem>,
        selected: Option<Ustr>,
        window: &mut Window,
        cx: &mut Context<SelectState<VersionFilterList>>,
    ) {
        state.set_items(
            VersionFilterList {
                matched: items.clone(),
                items,
            },
            window,
            cx,
        );
        state.set_selected_value(&selected, window, cx);
    }
}
