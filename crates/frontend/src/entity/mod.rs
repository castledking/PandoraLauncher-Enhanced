use std::{path::Path, sync::Arc};

use bridge::handle::BackendHandle;
use gpui::Entity;
use parking_lot::RwLock;

use crate::entity::{
    account::AccountEntries, instance::InstanceEntries, metadata::FrontendMetadata,
    minecraft_profile::MinecraftProfileEntries,
};

pub mod account;
pub mod instance;
pub mod metadata;
pub mod minecraft_profile;

#[derive(Clone)]
pub struct DataEntities {
    pub instances: Entity<InstanceEntries>,
    pub metadata: Entity<FrontendMetadata>,
    pub accounts: Entity<AccountEntries>,
    pub minecraft_profile: Entity<MinecraftProfileEntries>,
    pub backend_handle: BackendHandle,
    pub theme_folder: Arc<Path>,
    pub panic_messages: Arc<PanicMessages>,
}

pub struct PanicMessages {
    pub panic_message: Arc<RwLock<Option<String>>>,
    pub deadlock_message: Arc<RwLock<Option<String>>>,
}
