use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ustr::Ustr;

#[derive(thiserror::Error, Debug, Clone)]
pub enum ModrinthError {
    #[error("Error connecting to modrinth")]
    ClientRequestError,
    #[error("Error deserializing result from modrinth")]
    DeserializeError,
    #[error("Descriptive error from modrinth")]
    ModrinthResponse(ModrinthErrorResponse),
    #[error("Non-OK response from modrinth")]
    NonOK(u16),
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModrinthErrorResponse {
    pub error: Arc<str>,
    pub description: Arc<str>,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum ModrinthRequest {
    Search(ModrinthSearchRequest),
}

#[derive(Debug, Clone)]
pub enum ModrinthResult {
    Search(ModrinthSearchResult),
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct ModrinthSearchRequest {
    pub query: Option<Arc<str>>,
    pub facets: Option<Arc<str>>,
    pub index: ModrinthSearchIndex,
    pub offset: usize,
    pub limit: usize,
}


#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ModrinthSearchIndex {
    Relevance,
    Downloads,
    Follows,
    Newest,
    Updated,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModrinthSearchResult {
    pub hits: Arc<[ModrinthHit]>,
    pub offset: usize,
    pub limit: usize,
    pub total_hits: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModrinthHit {
    // pub slug: Option<Arc<str>>,
    pub title: Option<Arc<str>>,
    pub description: Option<Arc<str>>,
    // pub categories: Option<Arc<[Arc<str>]>>,
    pub client_side: Option<ModrinthSideRequirement>,
    pub server_side: Option<ModrinthSideRequirement>,
    pub project_type: ModrinthProjectType,
    pub downloads: usize,
    pub icon_url: Option<Arc<str>>,
    // pub color: Option<u32>,
    // pub thread_id: Option<Arc<str>>,
    // pub monetization_status: Option<ModrinthMonetizationStatus>,
    pub project_id: Arc<str>,
    pub author: Arc<str>,
    pub display_categories: Option<Arc<[Ustr]>>,
    // pub versions: Arc<[Arc<str>]>,
    // pub follows: usize,
    // pub date_created: DateTime<Utc>,
    pub date_modified: DateTime<Utc>,
    // pub latest_version: Option<Arc<str>>,
    // pub license: Arc<str>,
    // pub gallery: Option<Arc<[Arc<str>]>>,
    // pub featured_gallery: Option<Arc<str>>,
}

#[derive(Debug, Copy, Clone, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModrinthSideRequirement {
    Required,
    Optional,
    Unsupported,
    Unknown,
}

#[derive(Debug, Copy, Clone, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModrinthProjectType {
    Mod,
    ModPack,
    ResourcePack,
    Shader,
}
