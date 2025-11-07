use bridge::handle::BackendHandle;
use gpui::Entity;

use crate::entity::{account::AccountEntries, instance::InstanceEntries, modrinth::FrontendModrinthData, version::VersionEntries};

pub mod instance;
pub mod version;
pub mod modrinth;
pub mod account;

#[derive(Clone)]
pub struct DataEntities {
    pub instances: Entity<InstanceEntries>,
    pub versions: Entity<VersionEntries>,
    pub modrinth: Entity<FrontendModrinthData>,
    pub accounts: Entity<AccountEntries>,
    pub backend_handle: BackendHandle
}
