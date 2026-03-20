use std::{path::Path, sync::Arc};

use bridge::handle::BackendHandle;
use gpui::{App, Entity};
use parking_lot::RwLock;

use crate::entity::{
    account::AccountEntries, instance::InstanceEntries, metadata::FrontendMetadata,
    minecraft_profile::MinecraftProfileEntries, skin_thumbnail_cache::SkinThumbnailCache,
};

pub mod account;
pub mod instance;
pub mod metadata;
pub mod minecraft_profile;
pub mod skin_thumbnail_cache;

/// Incremented when content is installed; Modrinth page observes this to refill installed content.
#[derive(Clone, Default)]
pub struct RefreshTrigger(pub u64);

#[derive(Clone)]
pub struct DataEntities {
    pub instances: Entity<InstanceEntries>,
    pub metadata: Entity<FrontendMetadata>,
    pub accounts: Entity<AccountEntries>,
    pub minecraft_profile: Entity<MinecraftProfileEntries>,
    pub skin_thumbnail_cache: Entity<SkinThumbnailCache>,
    pub refresh_trigger: Entity<RefreshTrigger>,
    pub backend_handle: BackendHandle,
    pub theme_folder: Arc<Path>,
    pub launcher_dir: Arc<Path>,
    pub panic_messages: Arc<PanicMessages>,
}

pub struct PanicMessages {
    pub panic_message: Arc<RwLock<Option<String>>>,
    pub deadlock_message: Arc<RwLock<Option<String>>>,
}

impl DataEntities {
}
