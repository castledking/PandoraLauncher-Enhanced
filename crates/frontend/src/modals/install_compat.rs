//! What an existing instance can take, shared by the Modrinth and CurseForge install dialogs: its
//! own loader plus any it runs through a bridge mod such as Sinytra Connector, and its own
//! Minecraft version or, when a project has nothing for that, a neighbouring release the user
//! picks knowing it might not work.

use std::cmp::Ordering;

use enumset::EnumSet;
use gpui::{prelude::*, *};
use gpui_component::{ActiveTheme, Icon, h_flex};
use schema::{game_version, loader::Loader};
use ustr::Ustr;

use crate::{entity::instance::InstanceEntry, icon::PandoraIcon};

#[derive(Clone, Copy)]
pub struct InstanceCompat {
    pub minecraft_version: &'static str,
    pub loader: Loader,
    /// Loaders other than its own whose mods it runs through a bridge mod.
    pub bridged: EnumSet<Loader>,
}

impl InstanceCompat {
    pub fn of(instance: &InstanceEntry, cx: &App) -> Self {
        Self {
            minecraft_version: instance.configuration.minecraft_version.as_str(),
            loader: instance.configuration.loader,
            bridged: instance.bridged_loaders(cx),
        }
    }

    /// How a loader the instance only runs through a bridge is offered, naming the bridge.
    pub fn bridged_label(&self, loader: Loader) -> SharedString {
        match self.loader.bridge_to(loader, self.minecraft_version) {
            Some(bridge) => t::instance::content::install::through_bridge(loader.pretty_name(), bridge.name).into(),
            None => SharedString::new_static(loader.pretty_name()),
        }
    }
}

/// Which Minecraft versions an install dialog for an existing instance offers.
pub enum VersionChoice {
    /// The project has something for the instance's own version, which is all that's offered.
    Exact,
    /// It doesn't, but it has something the instance can load for these other versions.
    Other {
        choices: Vec<&'static str>,
        preferred: Option<&'static str>,
    },
    Nothing,
}

impl VersionChoice {
    /// `usable` is every game version the project has something the instance can load for, and
    /// `hint` the version the user was browsing, which is picked if it's among them.
    pub fn new(instance_version: &'static str, usable: Vec<&'static str>, hint: Option<Ustr>) -> Self {
        if usable.contains(&instance_version) {
            return Self::Exact;
        }
        if usable.is_empty() {
            return Self::Nothing;
        }

        let preferred = hint
            .and_then(|hint| usable.iter().copied().find(|version| *version == hint.as_str()))
            .or_else(|| game_version::closest_earlier_release(instance_version, usable.iter().copied()));
        let mut choices = usable;
        sort_game_versions(&mut choices);
        Self::Other { choices, preferred }
    }
}

/// Releases newest first, then everything else.
pub fn sort_game_versions<S: AsRef<str>>(versions: &mut [S]) {
    versions.sort_by(|a, b| {
        let (a, b) = (a.as_ref(), b.as_ref());
        let a_is_snapshot = a.contains('w') || a.contains("pre") || a.contains("rc");
        let b_is_snapshot = b.contains('w') || b.contains("pre") || b.contains("rc");
        if a_is_snapshot != b_is_snapshot {
            if a_is_snapshot { Ordering::Greater } else { Ordering::Less }
        } else {
            lexical_sort::natural_lexical_cmp(a, b).reverse()
        }
    });
}

/// Shown while a version made for another Minecraft release is picked.
pub fn other_version_warning(selected: &str, instance_version: &str, cx: &App) -> Div {
    let warning = cx.theme().warning;
    h_flex()
        .gap_2()
        .text_sm()
        .text_color(warning)
        .child(Icon::new(PandoraIcon::TriangleAlert).flex_none().text_color(warning))
        .child(t::instance::content::install::other_game_version(selected, instance_version))
}

#[cfg(test)]
mod tests {
    // Not a glob import: that would bring in gpui's `test` attribute in place of the built-in one
    use ustr::Ustr;

    use super::{VersionChoice, sort_game_versions};

    #[test]
    fn the_instance_version_is_used_when_there_is_one() {
        assert!(matches!(
            VersionChoice::new("1.21.1", vec!["1.21", "1.21.1"], None),
            VersionChoice::Exact
        ));
    }

    #[test]
    fn otherwise_the_browsed_version_is_picked_or_the_nearest_earlier_release() {
        let VersionChoice::Other { choices, preferred } =
            VersionChoice::new("1.21.1", vec!["1.20.1", "1.21", "1.21.4"], Some(Ustr::from("1.20.1")))
        else {
            panic!("expected other versions");
        };
        assert_eq!(choices, ["1.21.4", "1.21", "1.20.1"]);
        assert_eq!(preferred, Some("1.20.1"));

        let VersionChoice::Other { preferred, .. } = VersionChoice::new("1.21.1", vec!["1.20.1", "1.21"], None) else {
            panic!("expected other versions");
        };
        assert_eq!(preferred, Some("1.21"));

        // Nothing earlier in the same release to fall back on, so the user picks
        let VersionChoice::Other { preferred, .. } = VersionChoice::new("1.21.1", vec!["1.20.1"], None) else {
            panic!("expected other versions");
        };
        assert_eq!(preferred, None);
    }

    #[test]
    fn nothing_usable_is_nothing() {
        assert!(matches!(VersionChoice::new("1.21.1", Vec::new(), None), VersionChoice::Nothing));
    }

    #[test]
    fn releases_sort_newest_first_ahead_of_snapshots() {
        let mut versions = vec!["1.20.1", "24w14a", "1.21.1", "1.21", "1.21-rc1"];
        sort_game_versions(&mut versions);
        assert_eq!(versions, ["1.21.1", "1.21", "1.20.1", "24w14a", "1.21-rc1"]);
    }
}
