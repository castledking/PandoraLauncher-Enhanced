use std::{path::Path, sync::Arc};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct InstanceID {
    pub index: usize,
    pub generation: usize,
}

impl InstanceID {
    pub fn dangling() -> Self {
        Self { index: usize::MAX, generation: usize::MAX }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct InstanceModID {
    pub index: usize,
    pub generation: usize,
}

impl InstanceModID {
    pub fn dangling() -> Self {
        Self { index: usize::MAX, generation: usize::MAX }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstanceStatus {
    NotRunning,
    Launching,
    Running
}

#[derive(Debug, Clone)]
pub struct InstanceWorldSummary {
    pub title: Arc<str>,
    pub subtitle: Arc<str>,
    pub level_path: Arc<Path>,
    pub last_played: i64,
    pub png_icon: Option<Arc<[u8]>>,
}

#[derive(Debug, Clone)]
pub struct InstanceServerSummary {
    pub name: Arc<str>,
    pub ip: Arc<str>,
    pub png_icon: Option<Arc<[u8]>>,
}

#[derive(Debug, Clone)]
pub struct InstanceModSummary {
    pub mod_summary: Arc<ModSummary>,
    pub id: InstanceModID,
    pub file_name: Arc<str>,
    pub path: Arc<Path>,
    pub enabled: bool,
}

#[derive(Debug, Clone)]
pub struct ModSummary {
    pub id: Arc<str>,
    pub name: Arc<str>,
    pub version_str: Arc<str>,
    pub authors: Arc<str>,
    pub png_icon: Option<Arc<[u8]>>,
}
