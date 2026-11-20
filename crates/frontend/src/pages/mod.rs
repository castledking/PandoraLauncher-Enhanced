pub mod changelog_page;
pub mod curseforge_page;
pub mod curseforge_project_page;
pub mod import;
pub mod instance;
pub mod instances_page;
pub mod modrinth_page;
pub mod modrinth_project_page;
pub mod page;
pub mod quickplay;
pub mod server;
pub mod servers_page;
pub mod skins_page;
pub mod syncing_page;

use bridge::instance::InstanceID;
use gpui::{App, SharedString, Window};
use schema::content::ContentSource;

use crate::{entity::DataEntities, interface_config::InterfaceConfig, ui::PageType};

/// Opens the in-launcher page of the project `source` points at, on top of the current page so
/// Back returns to it. `install_for` is the instance its install button works on. Returns whether
/// there was a page to open: content that isn't linked to a project has none.
pub fn open_project_page(
    source: &ContentSource,
    title: SharedString,
    install_for: Option<InstanceID>,
    data: &DataEntities,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    let install_for =
        install_for.and_then(|id| crate::entity::instance::InstanceEntries::find_name_by_id(&data.instances, id, cx));
    let page = match source {
        ContentSource::ModrinthProject { project_id } => PageType::ModrinthProject {
            project_id: SharedString::new(project_id.clone()),
            project_title: title,
            install_for,
        },
        ContentSource::CurseforgeProject { project_id } => PageType::CurseforgeProject {
            project_id: *project_id,
            project_title: title,
            install_for,
        },
        ContentSource::Manual | ContentSource::ModrinthUnknown => return false,
    };

    let config = InterfaceConfig::get(cx);
    let mut path: Vec<PageType> = config.page_path.to_vec();
    path.push(config.main_page.clone());
    crate::root::switch_page(page, &path, window, cx);
    true
}
