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
