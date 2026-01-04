use std::{collections::HashMap, path::PathBuf, sync::Arc};

use chrono::{DateTime, Utc};
use relative_path::RelativePathBuf;
use serde::{Deserialize, Deserializer};
use ustr::Ustr;

use crate::version_manifest::MinecraftVersionType;

#[derive(Deserialize, Clone, Debug)]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct MinecraftVersion {
    pub arguments: Option<LaunchArguments>,
    pub asset_index: AssetIndexLink,
    pub assets: Ustr,
    pub compliance_level: Option<u32>,
    pub downloads: GameDownloads,
    pub id: Ustr,
    pub java_version: Option<JavaVersion>,
    pub libraries: Vec<GameLibrary>,
    pub logging: Option<GameLogging>,
    pub main_class: Ustr,
    /// Used in 1.12.2 and below instead of `arguments`
    pub minecraft_arguments: Option<Ustr>,
    pub minimum_launcher_version: u32,
    pub release_time: DateTime<Utc>,
    pub time: DateTime<Utc>,
    pub r#type: MinecraftVersionType,
}

#[derive(Deserialize, Clone, Debug)]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
pub struct LaunchArguments {
    pub game: Arc<[LaunchArgument]>,
    pub jvm: Arc<[LaunchArgument]>,
}

#[derive(Clone, Debug)]
pub enum LaunchArgument {
    Single(LaunchArgumentValue),
    Ruled(LaunchArgumentRuled),
}

impl<'de> Deserialize<'de> for LaunchArgument {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        serde_untagged::UntaggedEnumVisitor::new()
            .string(|single| Ok(LaunchArgument::Single(LaunchArgumentValue::Single(Ustr::from(single)))))
            .seq(|seq| seq.deserialize().map(LaunchArgument::Single))
            .map(|map| map.deserialize().map(LaunchArgument::Ruled))
            .deserialize(deserializer)
    }
}

#[derive(Deserialize, Clone, Debug)]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
pub struct LaunchArgumentRuled {
    pub rules: Arc<[Rule]>,
    pub value: LaunchArgumentValue,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(untagged)]
pub enum LaunchArgumentValue {
    Single(Ustr),
    Multiple(Arc<[Ustr]>),
}

#[derive(Deserialize, Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
pub struct Rule {
    pub action: RuleAction,
    pub features: Option<RuleFeatures>,
    pub os: Option<RuleOs>,
}

#[derive(Deserialize, Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum RuleAction {
    Allow,
    Disallow,
}

#[derive(Deserialize, Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
pub struct RuleFeatures {
    #[serde(default)]
    pub is_demo_user: bool,
    #[serde(default)]
    pub has_custom_resolution: bool,
    #[serde(default)]
    pub has_quick_plays_support: bool,
    #[serde(default)]
    pub is_quick_play_singleplayer: bool,
    #[serde(default)]
    pub is_quick_play_multiplayer: bool,
    #[serde(default)]
    pub is_quick_play_realms: bool,
}

#[derive(Deserialize, Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
pub struct RuleOs {
    pub name: Option<OsName>,
    pub arch: Option<OsArch>,
    /// Regex for OS version, only used in 23w17a and below
    pub version: Option<Ustr>,
}

#[derive(Deserialize, Copy, Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum OsName {
    Linux,
    Osx,
    Windows,
}

#[derive(Deserialize, Copy, Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum OsArch {
    Arm64,
    X86,
}

#[derive(Deserialize, Clone, Debug)]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct AssetIndexLink {
    pub id: Ustr,
    pub sha1: Ustr,
    pub size: u32,
    pub total_size: u32,
    pub url: Ustr,
}

#[derive(Deserialize, Clone, Debug)]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
pub struct GameDownloads {
    pub client: VersionDownloadLink,
    pub client_mappings: Option<VersionDownloadLink>,
    pub server: Option<VersionDownloadLink>,
    pub server_mappings: Option<VersionDownloadLink>,
    /// Only present in 16w04a and below
    pub windows_server: Option<VersionDownloadLink>,
}

#[derive(Deserialize, Clone, Debug)]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
pub struct VersionDownloadLink {
    pub sha1: Ustr,
    pub size: u32,
    pub url: Ustr,
}

#[derive(Deserialize, Clone, Debug)]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
#[serde(rename_all = "camelCase")]
pub struct JavaVersion {
    pub component: Ustr,
    pub major_version: u32,
}

#[derive(Deserialize, Clone, Debug)]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
pub struct GameLibrary {
    pub downloads: GameLibraryDownloads,
    pub name: Ustr,
    pub rules: Option<Arc<[Rule]>>,

    /// Natives for a specific OS version, only used in 22w19a and below
    /// Refers to an artifact in `GameLibraryDownloads::classifiers`
    pub natives: Option<HashMap<OsName, Ustr>>,

    /// Options that modify the extraction of natives, only used in 22w17a and below
    pub extract: Option<GameLibraryExtractOptions>,
}

#[derive(Deserialize, Clone, Debug)]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
pub struct GameLibraryDownloads {
    pub artifact: Option<GameLibraryArtifact>,

    /// Named artifacts, only used in 22w19a and below
    pub classifiers: Option<HashMap<Ustr, GameLibraryArtifact>>,
}

#[derive(Deserialize, Clone, Debug)]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
pub struct GameLibraryArtifact {
    pub path: Ustr,
    pub sha1: Option<Ustr>,
    pub size: Option<u32>,
    pub url: Ustr,
}

#[derive(Deserialize, Clone, Debug)]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
pub struct GameLibraryExtractOptions {
    pub exclude: Option<Arc<[RelativePathBuf]>>,
}

#[derive(Deserialize, Clone, Debug)]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
pub struct GameLogging {
    pub client: GameLoggingTarget,
}

#[derive(Deserialize, Clone, Debug)]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
pub struct GameLoggingTarget {
    pub argument: Ustr,
    pub file: GameLoggingFile,
    pub r#type: GameLoggingType,
}

#[derive(Deserialize, Clone, Debug)]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
pub struct GameLoggingFile {
    pub id: Ustr,
    pub sha1: Ustr,
    pub size: u32,
    pub url: Ustr,
}

#[derive(Deserialize, Clone, Debug)]
#[cfg_attr(debug_assertions, serde(deny_unknown_fields))]
pub enum GameLoggingType {
    #[serde(rename = "log4j2-xml")]
    Log4j2Xml,
}
