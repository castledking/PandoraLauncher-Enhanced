use std::sync::Arc;

use serde::{Deserialize, Serialize};
use strum::EnumIter;
use ustr::Ustr;

use crate::loader::Loader;

pub const CURSEFORGE_SEARCH_URL: &str = "https://api.curseforge.com/v1/mods/search";
pub const CURSEFORGE_API_KEY: &str = "$2a$10$YXf6dyJfJZM4zeChdr.RDOvWN.L48AN0dQShQO8/cVc5ho1wA8ZbS";
pub const MINECRAFT_GAME_ID: u32 = 432;

#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CurseforgeSearchRequest {
    pub class_id: u32,
    #[serde(skip_serializing_if = "crate::skip_if_none")]
    pub category_ids: Option<Arc<str>>,
    #[serde(skip_serializing_if = "crate::skip_if_none")]
    pub game_version: Option<Ustr>,
    #[serde(skip_serializing_if = "crate::skip_if_none")]
    pub search_filter: Option<Arc<str>>,
    #[serde(skip_serializing_if = "crate::skip_if_none")]
    pub mod_loader_types: Option<Arc<str>>,
    pub sort_field: u32,
    pub index: u32,
    pub page_size: u32,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CurseforgeGetModFilesRequest {
    pub mod_id: u32,
    #[serde(skip_serializing_if = "crate::skip_if_none")]
    pub game_version: Option<Ustr>,
    #[serde(skip_serializing_if = "crate::skip_if_none")]
    pub mod_loader_type: Option<u32>,
    #[serde(default, skip_serializing_if = "crate::skip_if_none")]
    pub release_types: Option<&'static [CurseforgeReleaseType]>,
    #[serde(skip_serializing_if = "crate::skip_if_none")]
    pub page_size: Option<u32>,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CurseforgeGetFilesRequest {
    pub file_ids: Vec<u32>,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CurseforgeChangelogRequest {
    pub mod_id: u32,
    pub file_id: u32,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CurseforgeFingerprintRequest {
    pub fingerprints: Vec<u32>,
}

#[derive(Debug, Deserialize)]
pub struct CurseforgeSearchResult {
    pub data: Arc<[CurseforgeHit]>,
    pub pagination: CurseforgePagination,
}

#[derive(Debug, Deserialize)]
pub struct CurseforgeGetModFilesResult {
    pub data: Arc<[CurseforgeFile]>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct CurseforgeChangelogResult {
    pub data: Option<Arc<str>>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct CurseforgeFingerprintResponse {
    pub data: CurseforgeFingerprintData,
}

#[derive(Debug, Deserialize, Clone)]
pub struct CurseforgeProjectResponse {
    pub data: CurseforgeProject,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CurseforgeFingerprintData {
    pub exact_matches: Arc<[CurseforgeFingerprintMatch]>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct CurseforgeFingerprintMatch {
    pub file: CurseforgeFingerprintFile,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CurseforgeFingerprintFile {
    pub id: u32,
    pub mod_id: u32,
    pub file_fingerprint: u32,
    pub file_name: Arc<str>,
    pub download_url: Option<Arc<str>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CurseforgePagination {
    pub index: u32,
    pub page_size: u32,
    pub result_count: u32,
    pub total_count: u64,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CurseforgeHit {
    pub id: u32,
    pub game_id: u32,
    pub name: Arc<str>,
    pub slug: Arc<str>,
    pub summary: Arc<str>,
    pub download_count: u64,
    pub class_id: Option<u32>,
    pub logo: Option<CurseforgeModAsset>,
    pub authors: Arc<[CurseforgeModAuthor]>,
    pub categories: Arc<[CurseforgeCategory]>,
    pub latest_files_indexes: Arc<[FileIndex]>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FileIndex {
    pub game_version: Ustr,
    pub file_id: u32,
    pub mod_loader: Option<u32>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CurseforgeModAsset {
    pub thumbnail_url: Arc<str>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CurseforgeModAuthor {
    pub id: u32,
    pub name: Arc<str>,
    pub url: Arc<str>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CurseforgeCategory {
    pub name: Arc<str>,
    pub is_class: bool,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CurseforgeFile {
    pub id: u32,
    pub mod_id: u32,
    pub file_name: Arc<str>,
    pub file_date: Option<Arc<str>>,
    pub release_type: u32,
    pub file_length: u64,
    pub game_versions: Option<Arc<[Ustr]>>,
    pub hashes: Arc<[CurseforgeFileHash]>,
    pub download_url: Option<Arc<str>>,
    pub dependencies: Arc<[CurseforgeFileDependency]>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CurseforgeProjectLinks {
    pub website_url: Arc<str>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CurseforgeProject {
    pub name: Arc<str>,
    pub links: CurseforgeProjectLinks,
    #[serde(default)]
    pub logo: Option<CurseforgeModAsset>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CurseforgeFileDependency {
    pub mod_id: u32,
    pub relation_type: u32,
}

pub const CURSEFORGE_RELATION_TYPE_REQUIRED_DEPENDENCY: u32 = 3;

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CurseforgeFileHash {
    pub value: Arc<str>,
    pub algo: u32,
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(into = "u32")]
#[repr(u32)]
pub enum CurseforgeReleaseType {
    Release = 1,
    Beta = 2,
    Alpha = 3,
    #[default]
    Other = 0,
}

impl CurseforgeReleaseType {
    pub fn from_u32(value: u32) -> Self {
        match value {
            1 => Self::Release,
            2 => Self::Beta,
            3 => Self::Alpha,
            _ => Self::Other,
        }
    }
}

impl From<CurseforgeReleaseType> for u32 {
    fn from(v: CurseforgeReleaseType) -> Self {
        v as u32
    }
}

#[derive(enumset::EnumSetType, Default, Debug, Hash, PartialOrd, Ord)]
#[repr(u32)]
pub enum CurseforgeModLoaderType {
    Forge = 1,
    Cauldron = 2,
    LiteLoader = 3,
    Fabric = 4,
    Quilt = 5,
    NeoForge = 6,
    #[default]
    Any = 0,
}

impl CurseforgeModLoaderType {
    pub fn from_u32(value: u32) -> Self {
        match value {
            1 => Self::Forge,
            2 => Self::Cauldron,
            3 => Self::LiteLoader,
            4 => Self::Fabric,
            5 => Self::Quilt,
            6 => Self::NeoForge,
            _ => Self::Any,
        }
    }

    pub fn pretty_name(self) -> &'static str {
        match self {
            Self::Forge => "Forge",
            Self::Cauldron => "Cauldron",
            Self::LiteLoader => "LiteLoader",
            Self::Fabric => "Fabric",
            Self::Quilt => "Quilt",
            Self::NeoForge => "NeoForge",
            Self::Any => "Any",
        }
    }

    pub fn from_name(str: &str) -> Self {
        match str {
            "Forge" | "forge" => Self::Forge,
            "Cauldron" | "cauldron" => Self::Cauldron,
            "LiteLoader" | "liteloader" => Self::LiteLoader,
            "Fabric" | "fabric" => Self::Fabric,
            "Quilt" | "quilt" => Self::Quilt,
            "NeoForge" | "neoforge" => Self::NeoForge,
            _ => Self::Any,
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        if id.starts_with("forge-") {
            Some(Self::Forge)
        } else if id.starts_with("neoforge-") {
            Some(Self::NeoForge)
        } else if id.starts_with("fabric-") {
            Some(Self::Fabric)
        } else {
            None
        }
    }

    pub fn as_pandora(self) -> Option<Loader> {
        match self {
            CurseforgeModLoaderType::Forge => Some(Loader::Forge),
            CurseforgeModLoaderType::Cauldron => None,
            CurseforgeModLoaderType::LiteLoader => None,
            CurseforgeModLoaderType::Fabric => Some(Loader::Fabric),
            CurseforgeModLoaderType::Quilt => None,
            CurseforgeModLoaderType::NeoForge => Some(Loader::NeoForge),
            CurseforgeModLoaderType::Any => None,
        }
    }
}

#[derive(Default, Debug, Copy, Clone, Hash, PartialEq, Eq, PartialOrd, Ord, EnumIter)]
#[repr(u32)]
pub enum CurseforgeSortField {
    #[default]
    Popularity = 2,
    Downloads = 6,
    LastUpdated = 3,
    Name = 4,
    Author = 5,
}

impl CurseforgeSortField {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Popularity => "popularity",
            Self::Downloads => "downloads",
            Self::LastUpdated => "updated",
            Self::Name => "name",
            Self::Author => "author",
        }
    }
}

#[derive(Default, Serialize, Deserialize, PartialEq, Eq, Clone, Copy, Debug)]
#[serde(rename_all = "lowercase")]
#[repr(u32)]
pub enum CurseforgeClassId {
    BukkitPlugin = 5,
    Mod = 6,
    Resourcepack = 12,
    World = 17,
    Modpack = 4471,
    Customization = 4546,
    BedrockAddon = 4559,
    Shader = 6552,
    Datapack = 6945,
    #[default]
    #[serde(other)]
    Other = 0,
}

impl CurseforgeClassId {
    pub fn from_u32(value: u32) -> Self {
        match value {
            5 => Self::BukkitPlugin,
            6 => Self::Mod,
            12 => Self::Resourcepack,
            17 => Self::World,
            4471 => Self::Modpack,
            4546 => Self::Customization,
            4559 => Self::BedrockAddon,
            6552 => Self::Shader,
            6945 => Self::Datapack,
            _ => Self::Other,
        }
    }

    pub fn mod_or_modpack(self) -> bool {
        match self {
            CurseforgeClassId::Mod | CurseforgeClassId::Modpack => true,
            _ => false,
        }
    }

    /// Bukkit plugins are a class of their own on CurseForge rather than a loader, but they are
    /// still tied to a game version, so the version filter applies to them.
    pub fn has_loader_and_version(self) -> bool {
        match self {
            Self::Mod | Self::Modpack | Self::BukkitPlugin => true,
            _ => false,
        }
    }

    /// CurseForge rejects `modLoaderType` on the Bukkit plugin class with a 400, since the class
    /// already implies it. Every other class can be narrowed by loader.
    pub fn supports_loader_filter(self) -> bool {
        match self {
            Self::Mod | Self::Modpack => true,
            _ => false,
        }
    }
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CurseforgeModpackManifestJson {
    pub minecraft: CurseforgeModpackMinecraft,
    pub version: Arc<str>,
    pub name: Option<Arc<str>>,
    #[serde(default)]
    pub files: Arc<[CurseforgeModpackFile]>,
    pub author: Option<Arc<str>>,
    pub overrides: Option<Arc<str>>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CurseforgeModpackMinecraft {
    pub version: Option<Arc<str>>,
    pub mod_loaders: Arc<[CurseforgeModpackModLoader]>,
    pub recommended_ram: Option<u32>,
}

impl CurseforgeModpackMinecraft {
    pub fn get_loader(&self) -> Option<Loader> {
        self.mod_loaders
            .iter()
            .find(|loader| loader.primary)
            .or_else(|| self.mod_loaders.first())
            .and_then(|loader| {
                CurseforgeModLoaderType::from_id(&loader.id).and_then(CurseforgeModLoaderType::as_pandora)
            })
    }
}

#[derive(Deserialize, Debug, Clone)]
pub struct CurseforgeModpackModLoader {
    pub id: Arc<str>,
    pub primary: bool,
}

#[derive(Deserialize, Debug, Clone)]
pub struct CurseforgeModpackFile {
    #[serde(rename = "projectID")]
    pub project_id: u32,
    #[serde(rename = "fileID")]
    pub file_id: u32,
    pub required: bool,
}

#[derive(Clone, Debug)]
pub struct CachedCurseforgeFileInfo {
    pub hash: [u8; 20],
    pub filename: Arc<str>,
    pub disabled_third_party_downloads: bool,
}

/// CurseForge's file fingerprint, used to look files up by their contents: 32-bit MurmurHash2
/// with a seed of 1, taken over the file with every tab, newline, carriage return and space removed.
pub fn fingerprint(bytes: &[u8]) -> u32 {
    const M: u32 = 0x5bd1_e995;
    const R: u32 = 24;
    const SEED: u32 = 1;

    let is_kept = |byte: &&u8| !matches!(**byte, b'\t' | b'\n' | b'\r' | b' ');
    let length = bytes.iter().filter(is_kept).count() as u32;

    let mut hash = SEED ^ length;
    let mut block = [0_u8; 4];
    let mut filled = 0;
    for &byte in bytes.iter().filter(is_kept) {
        block[filled] = byte;
        filled += 1;
        if filled == block.len() {
            let mut k = u32::from_le_bytes(block);
            k = k.wrapping_mul(M);
            k ^= k >> R;
            k = k.wrapping_mul(M);
            hash = hash.wrapping_mul(M) ^ k;
            filled = 0;
        }
    }

    if filled > 0 {
        for (index, &byte) in block[..filled].iter().enumerate() {
            hash ^= (byte as u32) << (8 * index);
        }
        hash = hash.wrapping_mul(M);
    }

    hash ^= hash >> 13;
    hash = hash.wrapping_mul(M);
    hash ^= hash >> 15;
    hash
}

#[cfg(test)]
mod tests {
    use super::fingerprint;

    // Reference values from an implementation that reproduces the fingerprints CurseForge reports
    // for uploaded files
    #[test]
    fn fingerprint_matches_curseforge() {
        assert_eq!(fingerprint(b""), 1540447798);
        assert_eq!(fingerprint(b"PandoraLauncher"), 2647682328);
        assert_eq!(fingerprint(b"PandoraLauncherEnhanced"), 3361054667);
    }

    #[test]
    fn fingerprint_ignores_whitespace() {
        assert_eq!(fingerprint(b"Pandora Launcher\r\n\tEnhanced"), fingerprint(b"PandoraLauncherEnhanced"));
    }
}
