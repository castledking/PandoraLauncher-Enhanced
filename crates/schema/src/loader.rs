use enumset::EnumSetType;
use serde::{Deserialize, Serialize};

use crate::{curseforge::CurseforgeModLoaderType, modrinth::ModrinthLoader};

#[derive(EnumSetType, Serialize, Deserialize, Debug, Hash, strum::EnumIter)]
#[serde(rename_all = "lowercase")]
pub enum Loader {
    #[serde(alias = "Fabric")]
    Fabric,
    #[serde(alias = "Forge")]
    Forge,
    #[serde(alias = "NeoForge")]
    NeoForge,
    /// Bukkit-style plugins, for Paper (and Purpur) servers. Never offered for client instances.
    #[serde(alias = "Paper")]
    Paper,
    #[serde(other)]
    #[serde(alias = "Vanilla")]
    Vanilla,
}

impl Loader {
    pub fn pretty_name(self) -> &'static str {
        match self {
            Loader::Vanilla => "Vanilla",
            Loader::Fabric => "Fabric",
            Loader::Forge => "Forge",
            Loader::NeoForge => "NeoForge",
            Loader::Paper => "Paper",
        }
    }

    /// Whether a client instance can use this loader. Paper only exists server side.
    pub fn is_client_loader(self) -> bool {
        !matches!(self, Loader::Paper)
    }

    pub fn from_name(str: &str) -> Option<Self> {
        match str {
            "Vanilla" | "vanilla" => Some(Self::Vanilla),
            "Fabric" | "fabric" => Some(Self::Fabric),
            "Forge" | "forge" => Some(Self::Forge),
            "NeoForge" | "neoforge" => Some(Self::NeoForge),
            "Paper" | "paper" => Some(Self::Paper),
            _ => None,
        }
    }

    pub fn as_modrinth_loader(self) -> ModrinthLoader {
        match self {
            Loader::Vanilla => ModrinthLoader::Unknown,
            Loader::Fabric => ModrinthLoader::Fabric,
            Loader::Forge => ModrinthLoader::Forge,
            Loader::NeoForge => ModrinthLoader::NeoForge,
            Loader::Paper => ModrinthLoader::Paper,
        }
    }

    /// Every Modrinth loader whose builds this loader can run. For mod loaders that's just the
    /// one; a Paper server also runs plugins published only for Spigot, Bukkit or Purpur.
    pub fn compatible_modrinth_loaders(self) -> &'static [ModrinthLoader] {
        match self {
            Loader::Vanilla => &[],
            Loader::Fabric => &[ModrinthLoader::Fabric],
            Loader::Forge => &[ModrinthLoader::Forge],
            Loader::NeoForge => &[ModrinthLoader::NeoForge],
            Loader::Paper => &[
                ModrinthLoader::Paper,
                ModrinthLoader::Spigot,
                ModrinthLoader::Bukkit,
                ModrinthLoader::Purpur,
            ],
        }
    }

    pub fn as_curseforge_loader(&self) -> CurseforgeModLoaderType {
        match self {
            Loader::Vanilla => CurseforgeModLoaderType::Any,
            Loader::Fabric => CurseforgeModLoaderType::Fabric,
            Loader::Forge => CurseforgeModLoaderType::Forge,
            Loader::NeoForge => CurseforgeModLoaderType::NeoForge,
            // CurseForge files Bukkit plugins under their own class rather than a loader
            Loader::Paper => CurseforgeModLoaderType::Any,
        }
    }
}

/// A mod that lets one loader run mods made for another, such as Sinytra Connector running Fabric
/// mods on NeoForge. With one installed, content for the loader it bridges counts as compatible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoaderBridge {
    pub name: &'static str,
    /// The id the mod declares, which is how an installed copy is recognised.
    pub mod_id: &'static str,
    pub modrinth_project_id: &'static str,
    pub curseforge_project_id: u32,
    hosts: &'static [Loader],
}

pub const SINYTRA_CONNECTOR: LoaderBridge = LoaderBridge {
    name: "Sinytra Connector",
    mod_id: "connector",
    modrinth_project_id: "u58R1TMW",
    curseforge_project_id: 890127,
    hosts: &[Loader::Forge, Loader::NeoForge],
};

pub const KILT: LoaderBridge = LoaderBridge {
    name: "Kilt",
    mod_id: "kilt",
    modrinth_project_id: "t87XZEqv",
    curseforge_project_id: 1295823,
    hosts: &[Loader::Fabric],
};

pub const LOADER_BRIDGES: &[LoaderBridge] = &[SINYTRA_CONNECTOR, KILT];

/// Fabric mods depend on Fabric API, which under Sinytra Connector is provided by Forgified Fabric
/// API instead: the real one is a Fabric mod and can't load there.
pub const FABRIC_API_MODRINTH_ID: &str = "P7dR8mSH";
pub const FORGIFIED_FABRIC_API_MODRINTH_ID: &str = "Aqlf1Shp";
pub const FABRIC_API_CURSEFORGE_ID: u32 = 306612;
pub const FORGIFIED_FABRIC_API_CURSEFORGE_ID: u32 = 889079;

impl LoaderBridge {
    /// The loader whose mods this bridge runs on `host` for `minecraft_version`, or none if it
    /// doesn't install on `host` at all.
    pub fn bridged_loader(&self, host: Loader, minecraft_version: &str) -> Option<Loader> {
        if !self.hosts.contains(&host) {
            return None;
        }
        if *self == SINYTRA_CONNECTOR {
            Some(Loader::Fabric)
        } else if is_forge_era(minecraft_version) {
            // Kilt follows the loader mods were being made for: Forge up to 1.20.1, NeoForge after
            Some(Loader::Forge)
        } else {
            Some(Loader::NeoForge)
        }
    }

    /// Whether a mod with this id is this bridge.
    pub fn is_mod(&self, mod_id: &str) -> bool {
        mod_id.eq_ignore_ascii_case(self.mod_id)
    }
}

impl Loader {
    /// The loaders other than this one whose mods it can also run, because a bridge mod for them
    /// is among the installed `mod_ids`.
    pub fn bridged_loaders<'a>(
        self,
        minecraft_version: &str,
        mod_ids: impl IntoIterator<Item = &'a str>,
    ) -> enumset::EnumSet<Loader> {
        let mut bridged = enumset::EnumSet::empty();
        for mod_id in mod_ids {
            for bridge in LOADER_BRIDGES {
                if bridge.is_mod(mod_id)
                    && let Some(loader) = bridge.bridged_loader(self, minecraft_version)
                {
                    bridged.insert(loader);
                }
            }
        }
        bridged.remove(self);
        bridged
    }

    /// The bridge that lets this loader run `other`'s mods, for naming it to the user.
    pub fn bridge_to(self, other: Loader, minecraft_version: &str) -> Option<&'static LoaderBridge> {
        LOADER_BRIDGES
            .iter()
            .find(|bridge| bridge.bridged_loader(self, minecraft_version) == Some(other))
    }
}

/// The Modrinth project to install in place of a dependency on `project_id`, when that dependency
/// can't load through the bridges in `bridged` on `host`.
pub fn bridged_modrinth_dependency(
    project_id: &str,
    host: Loader,
    bridged: enumset::EnumSet<Loader>,
) -> Option<&'static str> {
    let connector = matches!(host, Loader::Forge | Loader::NeoForge) && bridged.contains(Loader::Fabric);
    (connector && project_id == FABRIC_API_MODRINTH_ID).then_some(FORGIFIED_FABRIC_API_MODRINTH_ID)
}

/// As [`bridged_modrinth_dependency`], for CurseForge projects.
pub fn bridged_curseforge_dependency(project_id: u32, host: Loader, bridged: enumset::EnumSet<Loader>) -> Option<u32> {
    let connector = matches!(host, Loader::Forge | Loader::NeoForge) && bridged.contains(Loader::Fabric);
    (connector && project_id == FABRIC_API_CURSEFORGE_ID).then_some(FORGIFIED_FABRIC_API_CURSEFORGE_ID)
}

/// Whether `version` is a release no newer than 1.20.1, the last one modded mostly on Forge rather
/// than NeoForge.
fn is_forge_era(version: &str) -> bool {
    let mut parts = version.split('.').map(|part| part.parse::<u32>().ok());
    match (parts.next(), parts.next(), parts.next()) {
        (Some(Some(1)), Some(Some(minor)), patch) => {
            minor < 20 || (minor == 20 && matches!(patch, None | Some(Some(0 | 1))))
        },
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use enumset::EnumSet;

    use super::*;

    #[test]
    fn connector_lets_neoforge_and_forge_run_fabric_mods() {
        assert_eq!(
            Loader::NeoForge.bridged_loaders("1.21.1", ["sodium", "connector"]),
            EnumSet::only(Loader::Fabric)
        );
        assert_eq!(Loader::Forge.bridged_loaders("1.20.1", ["connector"]), EnumSet::only(Loader::Fabric));
        // Connector is no use on Fabric itself
        assert_eq!(Loader::Fabric.bridged_loaders("1.21.1", ["connector"]), EnumSet::empty());
    }

    #[test]
    fn kilt_runs_forge_mods_on_old_versions_and_neoforge_mods_after() {
        assert_eq!(Loader::Fabric.bridged_loaders("1.20.1", ["kilt"]), EnumSet::only(Loader::Forge));
        assert_eq!(Loader::Fabric.bridged_loaders("1.21.1", ["kilt"]), EnumSet::only(Loader::NeoForge));
        assert_eq!(Loader::Fabric.bridged_loaders("26.1", ["Kilt"]), EnumSet::only(Loader::NeoForge));
        assert_eq!(Loader::NeoForge.bridged_loaders("1.21.1", ["kilt"]), EnumSet::empty());
    }

    #[test]
    fn nothing_is_bridged_without_a_bridge() {
        assert_eq!(Loader::NeoForge.bridged_loaders("1.21.1", ["sodium", "jei"]), EnumSet::empty());
        assert_eq!(Loader::Vanilla.bridged_loaders("1.21.1", ["connector", "kilt"]), EnumSet::empty());
    }

    #[test]
    fn bridges_are_named_for_the_loader_they_run() {
        assert_eq!(Loader::NeoForge.bridge_to(Loader::Fabric, "1.21.1"), Some(&SINYTRA_CONNECTOR));
        assert_eq!(Loader::Fabric.bridge_to(Loader::NeoForge, "1.21.1"), Some(&KILT));
        assert_eq!(Loader::Fabric.bridge_to(Loader::Forge, "1.21.1"), None);
    }

    #[test]
    fn fabric_api_becomes_forgified_fabric_api_under_connector() {
        let connector = EnumSet::only(Loader::Fabric);
        assert_eq!(
            bridged_modrinth_dependency(FABRIC_API_MODRINTH_ID, Loader::NeoForge, connector),
            Some(FORGIFIED_FABRIC_API_MODRINTH_ID)
        );
        assert_eq!(bridged_modrinth_dependency("AANobbMI", Loader::NeoForge, connector), None);
        assert_eq!(bridged_modrinth_dependency(FABRIC_API_MODRINTH_ID, Loader::Fabric, EnumSet::empty()), None);
        assert_eq!(
            bridged_curseforge_dependency(FABRIC_API_CURSEFORGE_ID, Loader::Forge, connector),
            Some(FORGIFIED_FABRIC_API_CURSEFORGE_ID)
        );
    }

    #[test]
    fn forge_era_ends_at_1_20_1() {
        assert!(is_forge_era("1.12.2"));
        assert!(is_forge_era("1.20"));
        assert!(is_forge_era("1.20.1"));
        assert!(!is_forge_era("1.20.2"));
        assert!(!is_forge_era("1.21.1"));
        assert!(!is_forge_era("26.1"));
        assert!(!is_forge_era("24w14a"));
    }
}
