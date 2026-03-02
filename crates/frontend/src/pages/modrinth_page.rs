use std::{cell::RefCell, ops::Range, rc::Rc, sync::{atomic::AtomicBool, Arc}, time::Duration};

use bridge::{
    instance::{ContentType, ContentUpdateContext, ContentUpdateStatus, InstanceContentID, InstanceContentSummary, InstanceID},
    message::{AtomicBridgeDataLoadState, MessageToBackend},
    meta::MetadataRequest,
    modal_action::ModalAction,
    serial::AtomicOptionSerial,
};
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme, Icon, IconName, Selectable, Sizable, StyledExt, WindowExt,
    breadcrumb::Breadcrumb,
    button::{Button, ButtonGroup, ButtonVariant, ButtonVariants},
    checkbox::Checkbox,
    h_flex,
    input::{Input, InputEvent, InputState},
    label::Label,
    notification::NotificationType,
    scroll::{ScrollableElement, Scrollbar},
    skeleton::Skeleton,
    tooltip::Tooltip,
    v_flex,
};
use rustc_hash::{FxHashMap, FxHashSet};
use schema::{content::ContentSource, loader::Loader, modrinth::{
    ModrinthHit, ModrinthProjectType, ModrinthSearchRequest, ModrinthSearchResult, ModrinthSideRequirement
}};
use ustr::Ustr;

use crate::{
    component::{error_alert::ErrorAlert, page::Page, page_path::PagePath}, entity::{
        DataEntities, metadata::{AsMetadataResult, FrontendMetadata, FrontendMetadataResult}
    }, icon::PandoraIcon, interface_config::InterfaceConfig, ts, ts_short
};

fn show_vanilla_change_to_fabric_modal(
    install_for_id: InstanceID,
    backend_handle: bridge::handle::BackendHandle,
    on_yes: impl FnOnce(&mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let on_yes = Rc::new(RefCell::new(Some(on_yes)));
    let on_yes_for_button = on_yes.clone();
    window.open_dialog(cx, move |dialog, _window, _cx| {
        let on_yes_for_button = on_yes_for_button.clone();
        let backend_handle = backend_handle.clone();
        dialog
            .title(ts!("instance.content.install.vanilla_change_to_fabric.title"))
            .child(
                v_flex()
                    .gap_2()
                    .child(ts!("instance.content.install.vanilla_change_to_fabric.message"))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("yes")
                                    .label(ts!("instance.content.install.vanilla_change_to_fabric.yes"))
                                    .success()
                                    .on_click(move |_, window, cx| {
                                        window.close_all_dialogs(cx);
                                        backend_handle.send(MessageToBackend::SetInstanceLoader {
                                            id: install_for_id,
                                            loader: Loader::Fabric,
                                        });
                                        if let Some(f) = on_yes_for_button.borrow_mut().take() {
                                            f(window, cx);
                                        }
                                    }),
                            )
                            .child(
                                Button::new("no")
                                    .label(ts!("instance.content.install.vanilla_change_to_fabric.no"))
                                    .on_click(|_, window, cx| {
                                        window.close_all_dialogs(cx);
                                    }),
                            ),
                    ),
            )
    });
}

pub struct ModrinthSearchPage {
    data: DataEntities,
    hits: Vec<ModrinthHit>,
    page_path: PagePath,
    install_for: Option<InstanceID>,
    filter_version: Option<Ustr>,
    loading: Option<Subscription>,
    pending_clear: bool,
    total_hits: usize,
    search_state: Entity<InputState>,
    _search_input_subscription: Subscription,
    _delayed_clear_task: Task<()>,
    filter_project_type: ModrinthProjectType,
    filter_loaders: FxHashSet<Loader>,
    filter_categories: FxHashSet<&'static str>,
    show_categories: Arc<AtomicBool>,
    can_install_latest: bool,
    installed_mods_by_project: FxHashMap<Arc<str>, Vec<InstalledContent>>,
    installed_mods_by_hash: FxHashMap<[u8; 20], InstalledContent>,
    installed_mods_by_filename_prefix: FxHashMap<Arc<str>, InstalledContent>,
    installed_mods_by_mod_id: FxHashMap<Arc<str>, InstalledContent>,
    installed_modpacks_by_project: FxHashMap<Arc<str>, Vec<InstalledContent>>,
    installed_resourcepacks_by_project: FxHashMap<Arc<str>, Vec<InstalledContent>>,
    installed_resourcepacks_by_hash: FxHashMap<[u8; 20], Arc<str>>,
    installed_shaders_by_project: FxHashMap<Arc<str>, Vec<InstalledContent>>,
    installed_shaders_by_filename_prefix: FxHashMap<Arc<str>, InstalledContent>,
    installed_datapacks_by_project: FxHashMap<Arc<str>, Vec<InstalledContent>>,
    installed_datapacks_by_filename_prefix: FxHashMap<Arc<str>, InstalledContent>,
    last_search: Arc<str>,
    scroll_handle: UniformListScrollHandle,
    search_error: Option<SharedString>,
    image_cache: Entity<RetainAllImageCache>,
    mods_load_state: Option<(Arc<AtomicBridgeDataLoadState>, AtomicOptionSerial)>,
    _instance_mods_subscription: Option<Subscription>,
    _instance_resourcepacks_subscription: Option<Subscription>,
    _instance_worlds_subscription: Option<Subscription>,
    _shader_refresh_task: Option<Task<()>>,
}

struct InstalledContent {
    content_id: InstanceContentID,
    status: ContentUpdateContext,
    mod_id: Option<Arc<str>>,
}

impl Clone for InstalledContent {
    fn clone(&self) -> Self {
        Self {
            content_id: self.content_id,
            status: self.status,
            mod_id: self.mod_id.clone(),
        }
    }
}

impl ModrinthSearchPage {
    pub fn new(install_for: Option<InstanceID>, project_type: Option<ModrinthProjectType>, page_path: PagePath, data: &DataEntities, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search_state = cx.new(|cx| InputState::new(window, cx).placeholder(ts!("instance.content.search.mod")).clean_on_escape());

        let mut can_install_latest = false;
        let mut installed_mods_by_project: FxHashMap<Arc<str>, Vec<InstalledContent>> = FxHashMap::default();
        let mut installed_mods_by_hash: FxHashMap<[u8; 20], InstalledContent> = FxHashMap::default();
        let mut installed_mods_by_filename_prefix: FxHashMap<Arc<str>, InstalledContent> = FxHashMap::default();
        let mut installed_mods_by_mod_id: FxHashMap<Arc<str>, InstalledContent> = FxHashMap::default();
        let mut installed_modpacks_by_project: FxHashMap<Arc<str>, Vec<InstalledContent>> = FxHashMap::default();
        let mut installed_resourcepacks_by_project: FxHashMap<Arc<str>, Vec<InstalledContent>> = FxHashMap::default();
        let mut installed_resourcepacks_by_hash: FxHashMap<[u8; 20], Arc<str>> = FxHashMap::default();
        let mut installed_shaders_by_project: FxHashMap<Arc<str>, Vec<InstalledContent>> = FxHashMap::default();
        let mut installed_shaders_by_filename_prefix: FxHashMap<Arc<str>, InstalledContent> = FxHashMap::default();
        let mut installed_datapacks_by_project: FxHashMap<Arc<str>, Vec<InstalledContent>> = FxHashMap::default();
        let mut installed_datapacks_by_filename_prefix: FxHashMap<Arc<str>, InstalledContent> = FxHashMap::default();
        let mut filter_version = None;

        let mut mods_load_state = None;
        let mut _instance_mods_subscription = None;
        let mut _instance_resourcepacks_subscription = None;
        let mut _instance_worlds_subscription = None;
        let mut _shader_refresh_task = None;

        if let Some(install_for) = install_for {
            if let Some(entry) = data.instances.read(cx).entries.get(&install_for) {
                let instance = entry.read(cx);
                can_install_latest = true;
                filter_version = Some(instance.configuration.minecraft_version);

                data.backend_handle.send(MessageToBackend::RequestLoadWorlds { id: install_for });

                mods_load_state = Some((instance.mods_state.clone(), AtomicOptionSerial::default()));

                let mods_entity = instance.mods.clone();
                let resource_packs_entity = instance.resource_packs.clone();
                let worlds_entity = instance.worlds.clone();

                _instance_mods_subscription = Some(cx.observe(&mods_entity, |page, _mods, cx| {
                    page.refill_installed_content_from_instance(cx);
                    cx.notify();
                }));

                _instance_resourcepacks_subscription = Some(cx.observe(&resource_packs_entity, |page, _resource_packs, cx| {
                    page.refill_installed_content_from_instance(cx);
                    cx.notify();
                }));

                _instance_worlds_subscription = Some(cx.observe(&worlds_entity, |page, _worlds, cx| {
                    page.refill_installed_content_from_instance(cx);
                    cx.notify();
                }));

                _shader_refresh_task = Some(cx.spawn(async move |page, cx| {
                    loop {
                        cx.background_executor().timer(Duration::from_secs(2)).await;
                        let _ = page.update(cx, |page, cx| {
                            page.refill_installed_content_from_instance(cx);
                            cx.notify();
                        });
                    }
                }));
            }
        }

        let _search_input_subscription = cx.subscribe_in(&search_state, window, Self::on_search_input_event);

        let mut filter_project_type = if let Some(project_type) = project_type {
            InterfaceConfig::get_mut(cx).modrinth_page_project_type = project_type;
            project_type
        } else {
            InterfaceConfig::get(cx).modrinth_page_project_type
        };
        if filter_project_type == ModrinthProjectType::Other {
            filter_project_type = ModrinthProjectType::Mod;
        }

        let mut page = Self {
            data: data.clone(),
            hits: Vec::new(),
            page_path,
            install_for,
            filter_version,
            loading: None,
            pending_clear: false,
            total_hits: 1,
            search_state,
            _search_input_subscription,
            _delayed_clear_task: Task::ready(()),
            filter_project_type,
            filter_loaders: FxHashSet::default(),
            filter_categories: FxHashSet::default(),
            show_categories: Arc::new(AtomicBool::new(false)),
            can_install_latest,
            installed_mods_by_project,
            installed_mods_by_hash,
            installed_mods_by_filename_prefix,
            installed_mods_by_mod_id,
            installed_modpacks_by_project,
            installed_resourcepacks_by_project,
            installed_resourcepacks_by_hash,
            installed_shaders_by_project,
            installed_shaders_by_filename_prefix,
            installed_datapacks_by_project,
            installed_datapacks_by_filename_prefix,
            last_search: Arc::from(""),
            scroll_handle: UniformListScrollHandle::new(),
            search_error: None,
            image_cache: RetainAllImageCache::new(cx),
            mods_load_state,
            _instance_mods_subscription,
            _instance_resourcepacks_subscription,
            _instance_worlds_subscription,
            _shader_refresh_task,
        };
        page.refill_installed_content_from_instance(cx);
        page.load_more(cx);
        page
    }

    fn on_search_input_event(
        &mut self,
        state: &Entity<InputState>,
        event: &InputEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let InputEvent::Change = event else {
            return;
        };

        let search = state.read(cx).text().to_string();
        let search = search.trim();

        if &*self.last_search == search {
            return;
        }

        let search: Arc<str> = Arc::from(search);
        self.last_search = search.clone();
        self.reload(cx);
    }

    fn refill_installed_content_from_instance(&mut self, cx: &App) {
        self.installed_mods_by_project.clear();
        self.installed_mods_by_hash.clear();
        self.installed_mods_by_filename_prefix.clear();
        self.installed_mods_by_mod_id.clear();
        self.installed_modpacks_by_project.clear();
        self.installed_resourcepacks_by_project.clear();
        self.installed_resourcepacks_by_hash.clear();
        self.installed_shaders_by_project.clear();
        self.installed_shaders_by_filename_prefix.clear();
        self.installed_datapacks_by_project.clear();
        self.installed_datapacks_by_filename_prefix.clear();

        if let Some(install_for) = self.install_for {
            if let Some(entry) = self.data.instances.read(cx).entries.get(&install_for) {
                let instance = entry.read(cx);
                let instance_loader = instance.configuration.loader;
                let instance_version = instance.configuration.minecraft_version;

                let instance_loader = instance.configuration.loader;
                let instance_version = instance.configuration.minecraft_version;

                let mods = instance.mods.read(cx);
                for summary in mods.iter() {
                    let content = InstalledContent {
                        content_id: summary.id,
                        status: summary.update,
                        mod_id: summary.content_summary.id.clone(),
                    };

                    if let ContentSource::ModrinthProject { project } = &summary.content_source {
                        let project_key: Arc<str> = Arc::from(project.to_lowercase().as_str());
                        let installed = self.installed_mods_by_project.entry(project_key).or_default();
                        installed.push(content.clone());
                    }

                    self.installed_mods_by_hash.insert(summary.content_summary.hash, content.clone());

                    if let Some(filename_prefix) = extract_modrinth_project_id_from_filename(&summary.filename) {
                        self.installed_mods_by_filename_prefix
                            .entry(filename_prefix.clone())
                            .or_insert(content.clone());
                    }

                    if let Some(ref mod_id) = summary.content_summary.id {
                        let mod_id_key: Arc<str> = Arc::from(mod_id.to_lowercase().as_str());
                        self.installed_mods_by_mod_id.entry(mod_id_key.clone()).or_insert(content.clone());
                    }

                    if let ContentType::ModrinthModpack { downloads, summaries, .. } = &summary.content_summary.extra {
                        let modpack_project = match &summary.content_source {
                            ContentSource::ModrinthProject { project } => Some(project.clone()),
                            _ => None,
                        };
                        let deleted_filenames = &summary.disabled_children.deleted_filenames;

                        for (index, bundled_summary) in summaries.iter().enumerate() {
                            if let Some(bundled) = bundled_summary {
                                if let Some(download) = downloads.get(index) {
                                    if deleted_filenames.contains(&*download.path) {
                                        continue;
                                    }
                                }

                                let bundled_content = InstalledContent {
                                    content_id: summary.id,
                                    status: ContentUpdateContext::new(ContentUpdateStatus::Unknown, instance_loader, instance_version),
                                    mod_id: bundled.id.clone(),
                                };
                                self.installed_mods_by_hash.insert(bundled.hash, bundled_content.clone());

                                if let Some(ref mod_id) = bundled.id {
                                    let mod_id_key: Arc<str> = Arc::from(mod_id.to_lowercase().as_str());
                                    self.installed_mods_by_mod_id.entry(mod_id_key.clone()).or_insert(bundled_content.clone());
                                    self.installed_mods_by_filename_prefix.entry(mod_id_key).or_insert(bundled_content.clone());
                                }

                                if let Some(ref name) = bundled.name {
                                    let name_key: Arc<str> = Arc::from(name.to_lowercase().as_str());
                                    self.installed_mods_by_filename_prefix.entry(name_key).or_insert(bundled_content.clone());
                                }

                                if let Some(download) = downloads.get(index) {
                                    if let Some(filename) = download.path.rsplit('/').next() {
                                        if let Some(project_id) = extract_modrinth_project_id_from_filename(filename) {
                                            self.installed_mods_by_filename_prefix.entry(project_id).or_insert(bundled_content.clone());
                                        }
                                    }
                                    for download_url in download.downloads.iter() {
                                        if let Some(project_id) = extract_modrinth_project_id_from_url(download_url) {
                                            self.installed_mods_by_filename_prefix.entry(project_id).or_insert(bundled_content.clone());
                                        }
                                    }
                                }

                                if let Some(ref project) = modpack_project {
                                    let project_key: Arc<str> = Arc::from(project.to_lowercase().as_str());
                                    self.installed_resourcepacks_by_hash.insert(bundled.hash, project_key);
                                }
                            }
                        }

                        if let Some(project) = modpack_project {
                            let project_key: Arc<str> = Arc::from(project.to_lowercase().as_str());
                            let installed = self.installed_modpacks_by_project.entry(project_key).or_default();
                            installed.push(content);
                        }
                    }
                }

                let resource_packs = instance.resource_packs.read(cx);
                for summary in resource_packs.iter() {
                    match &summary.content_source {
                        ContentSource::ModrinthProject { project } => {
                            let project_key: Arc<str> = Arc::from(project.to_lowercase().as_str());
                            let installed = self.installed_resourcepacks_by_project.entry(project_key).or_default();
                            installed.push(InstalledContent {
                                content_id: summary.id,
                                status: summary.update,
                                mod_id: summary.content_summary.id.clone(),
                            });
                        }
                        ContentSource::ModrinthUnknown => {
                            if let Some(project) = self.installed_resourcepacks_by_hash.get(&summary.content_summary.hash) {
                                let project_key: Arc<str> = Arc::from(project.to_lowercase().as_str());
                                let installed = self.installed_resourcepacks_by_project.entry(project_key).or_default();
                                installed.push(InstalledContent {
                                    content_id: summary.id,
                                    status: summary.update,
                                    mod_id: summary.content_summary.id.clone(),
                                });
                            }
                        }
                        ContentSource::Manual => {}
                    }
                }

                let shaderpacks_path = instance.dot_minecraft_folder.join("shaderpacks");
                if shaderpacks_path.exists() {
                    if let Ok(entries) = std::fs::read_dir(&shaderpacks_path) {
                        for entry in entries.flatten() {
                            let path = entry.path();
                            if path.is_file() {
                                if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
                                    let shader_name = filename.trim_end_matches(".zip").trim_end_matches(".disabled");
                                    if !shader_name.is_empty() {
                                        let filename_prefix: Arc<str> = Arc::from(shader_name.to_lowercase().as_str());
                                        let content = InstalledContent {
                                            content_id: InstanceContentID::dangling(),
                                            status: ContentUpdateContext::new(ContentUpdateStatus::ManualInstall, instance_loader, instance_version),
                                            mod_id: None,
                                        };
                                        self.installed_shaders_by_filename_prefix.entry(filename_prefix).or_insert(content);
                                    }
                                }
                            }
                        }
                    }
                }

                let saves_path = instance.dot_minecraft_folder.join("saves");
                if saves_path.exists() {
                    if let Ok(world_dirs) = std::fs::read_dir(&saves_path) {
                        for world_dir in world_dirs.flatten() {
                            let datapacks_path = world_dir.path().join("datapacks");
                            if datapacks_path.is_dir() {
                                if let Ok(entries) = std::fs::read_dir(&datapacks_path) {
                                    for entry in entries.flatten() {
                                        let path = entry.path();
                                        if path.is_file() {
                                            if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
                                                let dp_name = filename.trim_end_matches(".zip").trim_end_matches(".disabled");
                                                if !dp_name.is_empty() {
                                                    let content = InstalledContent {
                                                        content_id: InstanceContentID::dangling(),
                                                        status: ContentUpdateContext::new(ContentUpdateStatus::ManualInstall, instance_loader, instance_version),
                                                        mod_id: None,
                                                    };
                                                    let filename_prefix: Arc<str> = Arc::from(dp_name.to_lowercase().as_str());
                                                    self.installed_datapacks_by_filename_prefix.entry(filename_prefix.clone()).or_insert(content.clone());
                                                    if let Some(project_prefix) = extract_modrinth_project_id_from_filename(filename) {
                                                        let project_key: Arc<str> = project_prefix.to_lowercase().into();
                                                        self.installed_datapacks_by_filename_prefix.entry(project_key.clone()).or_insert(content.clone());
                                                        let installed = self.installed_datapacks_by_project.entry(project_key).or_default();
                                                        installed.push(content);
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    fn set_project_type(&mut self, project_type: ModrinthProjectType, window: &mut Window, cx: &mut Context<Self>) {
        if self.filter_project_type == project_type {
            return;
        }
        InterfaceConfig::get_mut(cx).modrinth_page_project_type = project_type;
        self.filter_project_type = project_type;
        self.filter_categories.clear();
        self.search_state.update(cx, |state, cx| {
            let placeholder = match project_type {
                ModrinthProjectType::Mod => ts!("instance.content.search.mod"),
                ModrinthProjectType::Modpack => ts!("instance.content.search.modpack"),
                ModrinthProjectType::Resourcepack => ts!("instance.content.search.resourcepack"),
                ModrinthProjectType::Shader => ts!("instance.content.search.shader"),
                ModrinthProjectType::Datapack => ts!("instance.content.search.datapack"),
                ModrinthProjectType::Other => ts!("instance.content.search.file"),
            };
            state.set_placeholder(placeholder, window, cx)
        });
        self.reload(cx);
    }

    fn set_filter_loaders(&mut self, loaders: FxHashSet<Loader>, _window: &mut Window, cx: &mut Context<Self>) {
        if self.filter_loaders == loaders {
            return;
        }
        self.filter_loaders = loaders;
        self.reload(cx);
    }

    fn set_filter_categories(&mut self, categories: FxHashSet<&'static str>, _window: &mut Window, cx: &mut Context<Self>) {
        if self.filter_categories == categories {
            return;
        }
        self.filter_categories = categories;
        self.reload(cx);
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        self.pending_clear = true;
        self.loading = None;

        self._delayed_clear_task = cx.spawn(async |page, cx| {
            cx.background_executor().timer(Duration::from_millis(300)).await;
            let _ = page.update(cx, |page, cx| {
                if page.pending_clear {
                    page.pending_clear = false;
                    page.hits.clear();
                    page.total_hits = 1;
                    cx.notify();
                }
            });
        });

        self.load_more(cx);
    }

    fn load_more(&mut self, cx: &mut Context<Self>) {
        if self.loading.is_some() {
            return;
        }
        self.search_error = None;

        let query = if self.last_search.is_empty() {
            None
        } else {
            Some(self.last_search.clone())
        };

        let project_type = match self.filter_project_type {
            ModrinthProjectType::Mod | ModrinthProjectType::Other => "mod",
            ModrinthProjectType::Modpack => "modpack",
            ModrinthProjectType::Resourcepack => "resourcepack",
            ModrinthProjectType::Shader => "shader",
            ModrinthProjectType::Datapack => "datapack",
        };

        let offset = if self.pending_clear { 0 } else { self.hits.len() };

        let mut facets = format!("[[\"project_type={}\"]", project_type);

        let is_mod = self.filter_project_type == ModrinthProjectType::Mod || self.filter_project_type == ModrinthProjectType::Modpack;
        let applies_version_filter = matches!(
            self.filter_project_type,
            ModrinthProjectType::Mod | ModrinthProjectType::Modpack | ModrinthProjectType::Shader
        );
        let filter_by_instance = self.install_for.is_some() && InterfaceConfig::get(cx).modrinth_filter_version;
        if filter_by_instance && applies_version_filter && let Some(filter_version) = &self.filter_version {
            facets.push_str(",[\"versions=");
            facets.push_str(filter_version);
            facets.push_str("\"]");
        }

        if !self.filter_loaders.is_empty() && is_mod {
            facets.push_str(",[");

            let mut first = true;
            for loader in &self.filter_loaders {
                if first {
                    first = false;
                } else {
                    facets.push(',');
                }
                facets.push_str("\"categories:");
                facets.push_str(loader.as_modrinth_loader().id());
                facets.push('"');
            }
            facets.push(']');
        }

        if !self.filter_categories.is_empty() {
            facets.push_str(",[");

            let mut first = true;
            for category in &self.filter_categories {
                if first {
                    first = false;
                } else {
                    facets.push(',');
                }
                facets.push_str("\"categories:");
                facets.push_str(*category);
                facets.push('"');
            }
            facets.push(']');
        }

        facets.push(']');

        let request = ModrinthSearchRequest {
            query,
            facets: Some(facets.into()),
            index: schema::modrinth::ModrinthSearchIndex::Relevance,
            offset,
            limit: 20,
        };

        let data = FrontendMetadata::request(&self.data.metadata, MetadataRequest::ModrinthSearch(request), cx);

        let result: FrontendMetadataResult<ModrinthSearchResult> = data.read(cx).result();
        match result {
            FrontendMetadataResult::Loading => {
                let subscription = cx.observe(&data, |page, data, cx| {
                    let result: FrontendMetadataResult<ModrinthSearchResult> = data.read(cx).result();
                    match result {
                        FrontendMetadataResult::Loading => {},
                        FrontendMetadataResult::Loaded(result) => {
                            page.apply_search_data(result);
                            page.loading = None;
                            cx.notify();
                        },
                        FrontendMetadataResult::Error(shared_string) => {
                            page.search_error = Some(shared_string);
                            page.loading = None;
                            cx.notify();
                        },
                    }
                });
                self.loading = Some(subscription);
            },
            FrontendMetadataResult::Loaded(result) => {
                self.apply_search_data(result);
            },
            FrontendMetadataResult::Error(shared_string) => {
                self.search_error = Some(shared_string);
            },
        }
    }

    fn apply_search_data(&mut self, search_result: &ModrinthSearchResult) {
        if self.pending_clear {
            self.pending_clear = false;
            self.hits.clear();
            self.total_hits = 1;
            self._delayed_clear_task = Task::ready(());
        }

        self.hits.extend(search_result.hits.iter().map(|hit| {
            let mut hit = hit.clone();
            if let Some(description) = hit.description {
                hit.description = Some(description.replace("\n", " ").into());
            }
            hit
        }));
        self.total_hits = search_result.total_hits;
    }

    fn render_items(&mut self, visible_range: Range<usize>, _window: &mut Window, cx: &mut Context<Self>) -> Vec<Div> {
        let theme = cx.theme();
        let mut should_load_more = false;
        let items = visible_range
            .map(|index| {
                let Some(hit) = self.hits.get(index) else {
                    if let Some(search_error) = self.search_error.clone() {
                        return div()
                            .pl_3()
                            .pt_3()
                            .child(ErrorAlert::new("search_error", ts!("instance.content.requesting_from_modrinth_error"), search_error));
                    } else {
                        should_load_more = true;
                        return div()
                            .pl_3()
                            .pt_3()
                            .child(Skeleton::new().w_full().h(px(28.0 * 4.0)).rounded_lg());
                    }
                };

                let image = if let Some(icon_url) = &hit.icon_url
                    && !icon_url.is_empty()
                {
                    gpui::img(SharedUri::from(icon_url))
                        .with_fallback(|| {
                            gpui::img(ImageSource::Resource(Resource::Embedded(
                                "images/default_mod.png".into(),
                            ))).rounded_lg().size_16().min_w_16().min_h_16().into_any_element()
                        })
                } else {
                    gpui::img(ImageSource::Resource(Resource::Embedded(
                        "images/default_mod.png".into(),
                    ))).rounded_lg().size_16().min_w_16().min_h_16()
                };

                let name = hit
                    .title
                    .as_ref()
                    .map(Arc::clone)
                    .map(SharedString::new)
                    .unwrap_or(ts!("instance.content.unnamed"));
                let author = ts!("instance.content.by", name = hit.author.clone());
                let description = hit
                    .description
                    .as_ref()
                    .map(Arc::clone)
                    .map(SharedString::new)
                    .unwrap_or(ts!("instance.content.no_description"));

                let author_line = div().text_color(cx.theme().muted_foreground).text_sm().pb_px().child(author);

                let client_side = hit.client_side.unwrap_or(ModrinthSideRequirement::Unknown);
                let server_side = hit.server_side.unwrap_or(ModrinthSideRequirement::Unknown);

                let (env_icon, env_name) = match (client_side, server_side) {
                    (ModrinthSideRequirement::Required, ModrinthSideRequirement::Required) => {
                        (PandoraIcon::Globe, ts!("modrinth.environment.client_and_server"))
                    },
                    (ModrinthSideRequirement::Required, ModrinthSideRequirement::Unsupported) => {
                        (PandoraIcon::Computer, ts!("modrinth.environment.client_only"))
                    },
                    (ModrinthSideRequirement::Required, ModrinthSideRequirement::Optional) => {
                        (PandoraIcon::Computer, ts!("modrinth.environment.client_only_server_optional"))
                    },
                    (ModrinthSideRequirement::Unsupported, ModrinthSideRequirement::Required) => {
                        (PandoraIcon::Router, ts!("modrinth.environment.server_only"))
                    },
                    (ModrinthSideRequirement::Optional, ModrinthSideRequirement::Required) => {
                        (PandoraIcon::Router, ts!("modrinth.environment.server_only_client_optional"))
                    },
                    (ModrinthSideRequirement::Optional, ModrinthSideRequirement::Optional) => {
                        (PandoraIcon::Globe, ts!("modrinth.environment.client_or_server"))
                    },
                    _ => (PandoraIcon::Cpu, ts!("modrinth.environment.unknown_environment")),
                };

                let environment = h_flex().gap_1().font_bold().child(env_icon).child(env_name);

                let categories = hit.display_categories.iter().flat_map(|categories| {
                    categories.iter().filter_map(|category| {
                        if category == "minecraft" {
                            return None;
                        }

                        let icon = icon_for(category).unwrap_or("icons/diamond.svg");
                        let icon = Icon::empty().path(icon);
                        let translated_category = ts!(format!("modrinth.category.{}", category));
                        Some(h_flex().gap_0p5().child(icon).child(translated_category))
                    })
                });

                let downloads = h_flex()
                    .gap_0p5()
                    .child(PandoraIcon::Download)
                    .child(format_downloads(hit.downloads));

                let (main_action, nub_action) = self.get_primary_action(hit, cx);

                let primary_button_label = match &main_action {
                    PrimaryAction::InstallLatest => ts!("instance.content.install.label"),
                    _ => main_action.text(),
                };

                let primary_button = Button::new(("install", index))
                    .label(primary_button_label)
                    .icon(main_action.icon())
                    .with_variant(main_action.button_variant())
                    .on_click({
                        let data = self.data.clone();
                        let name = name.clone();
                        let project_id = hit.project_id.clone();
                        let install_for = self.install_for.clone();
                        let project_type = hit.project_type;
                        let filter_project_type = self.filter_project_type;
                        let main_action = main_action.clone();

                        move |_, window, cx| {
                            let effective_type = if filter_project_type == ModrinthProjectType::Datapack {
                                ModrinthProjectType::Datapack
                            } else {
                                project_type
                            };
                            if project_type != ModrinthProjectType::Other {
                                match main_action {
                                    PrimaryAction::Install | PrimaryAction::Reinstall => {
                                        if let Some(install_for_id) = install_for {
                                            let is_vanilla = data.instances.read(cx).entries.get(&install_for_id)
                                                .map(|e| e.read(cx).configuration.loader == Loader::Vanilla)
                                                .unwrap_or(false);
                                            let is_mod_or_modpack = matches!(filter_project_type, ModrinthProjectType::Mod | ModrinthProjectType::Modpack);
                                            let is_datapack = filter_project_type == ModrinthProjectType::Datapack;
                                            if is_vanilla && is_mod_or_modpack && !is_datapack {
                                                let name = name.clone();
                                                let project_id = project_id.clone();
                                                let data = data.clone();
                                                show_vanilla_change_to_fabric_modal(
                                                    install_for_id,
                                                    data.backend_handle.clone(),
                                                    move |window, cx| {
                                                        crate::modals::modrinth_install::open_with_version(
                                                            name.as_str(),
                                                            project_id.clone(),
                                                            effective_type,
                                                            Some(install_for_id),
                                                            &data,
                                                            window,
                                                            cx,
                                                            None,
                                                            Some(Loader::Fabric),
                                                        );
                                                    },
                                                    window,
                                                    cx,
                                                );
                                            } else {
                                                crate::modals::modrinth_install::open(
                                                    name.as_str(),
                                                    project_id.clone(),
                                                    effective_type,
                                                    install_for,
                                                    &data,
                                                    window,
                                                    cx
                                                );
                                            }
                                        } else {
                                            crate::modals::modrinth_install::open(
                                                name.as_str(),
                                                project_id.clone(),
                                                effective_type,
                                                install_for,
                                                &data,
                                                window,
                                                cx
                                            );
                                        }
                                    },
                                    PrimaryAction::InstallLatest => {
                                        if let Some(install_for_id) = install_for {
                                            let is_vanilla = data.instances.read(cx).entries.get(&install_for_id)
                                                .map(|e| e.read(cx).configuration.loader == Loader::Vanilla)
                                                .unwrap_or(false);
                                            let is_mod_or_modpack = matches!(filter_project_type, ModrinthProjectType::Mod | ModrinthProjectType::Modpack);
                                            let is_datapack = filter_project_type == ModrinthProjectType::Datapack;
                                            if is_vanilla && is_mod_or_modpack && !is_datapack {
                                                let name = name.clone();
                                                let project_id = project_id.clone();
                                                let data = data.clone();
                                                show_vanilla_change_to_fabric_modal(
                                                    install_for_id,
                                                    data.backend_handle.clone(),
                                                    move |window, cx| {
                                                        crate::modals::modrinth_install_auto::open(
                                                            name.as_str(),
                                                            project_id.clone(),
                                                            project_type,
                                                            install_for_id,
                                                            &data,
                                                            window,
                                                            cx,
                                                            Some(Loader::Fabric),
                                                        );
                                                    },
                                                    window,
                                                    cx,
                                                );
                                            } else if is_datapack {
                                                crate::modals::modrinth_install_auto::open(
                                                    name.as_str(),
                                                    project_id.clone(),
                                                    ModrinthProjectType::Datapack,
                                                    install_for_id,
                                                    &data,
                                                    window,
                                                    cx,
                                                    None,
                                                );
                                            } else {
                                                crate::modals::modrinth_install_auto::open(
                                                    name.as_str(),
                                                    project_id.clone(),
                                                    project_type,
                                                    install_for_id,
                                                    &data,
                                                    window,
                                                    cx,
                                                    None,
                                                );
                                            }
                                        }
                                    },
                                    PrimaryAction::Installed => {
                                        crate::modals::modrinth_install::open(
                                            name.as_str(),
                                            project_id.clone(),
                                            effective_type,
                                            install_for,
                                            &data,
                                            window,
                                            cx,
                                        );
                                    },
                                    PrimaryAction::Update(ref ids) => {
                                        for id in ids {
                                            let modal_action = ModalAction::default();
                                            data.backend_handle.send(MessageToBackend::UpdateContent {
                                                instance: install_for.unwrap(),
                                                content_id: *id,
                                                modal_action: modal_action.clone()
                                            });
                                            crate::modals::generic::show_notification(window, cx,
                                                ts!("instance.content.update.error"), modal_action);
                                        }
                                    },
                                }
                            } else {
                                window.push_notification(
                                    (
                                        NotificationType::Error,
                                        ts!("instance.content.install.unknown_type"),
                                    ),
                                    cx,
                                );
                            }
                        }
                    });

                let nub_button = nub_action.as_ref().map(|nub| {
                    let data = self.data.clone();
                    let install_for = self.install_for;
                    let nub = nub.clone();
                    Button::new(("nub", index))
                        .icon(nub.icon())
                        .with_variant(nub.button_variant())
                        .compact()
                        .small()
                        .on_click(move |_, window, cx| {
                            match &nub {
                                NubAction::CheckForUpdates => {
                                    let modal_action = ModalAction::default();
                                    data.backend_handle.send(MessageToBackend::UpdateCheck {
                                        instance: install_for.unwrap(),
                                        modal_action: modal_action.clone()
                                    });
                                    crate::modals::generic::show_notification(window, cx,
                                        ts!("instance.content.update.check.error"), modal_action);
                                },
                                NubAction::ErrorCheckingForUpdates => {},
                                NubAction::UpToDate => {},
                                NubAction::Update(ids) => {
                                    for id in ids {
                                        let modal_action = ModalAction::default();
                                        data.backend_handle.send(MessageToBackend::UpdateContent {
                                            instance: install_for.unwrap(),
                                            content_id: *id,
                                            modal_action: modal_action.clone()
                                        });
                                        crate::modals::generic::show_notification(window, cx,
                                            ts!("instance.content.update.error"), modal_action);
                                    }
                                }
                            }
                        })
                });

                let open_page_button = Button::new(("open", index))
                    .label(ts!("instance.content.open_page"))
                    .icon(PandoraIcon::Globe)
                    .info()
                    .on_click({
                        let project_type = hit.project_type.as_str();
                        let project_id = hit.project_id.clone();
                        move |_, _, cx| {
                            cx.open_url(&format!(
                                "https://modrinth.com/{}/{}",
                                project_type, project_id
                            ));
                        }
                    });

                let left_slot_w = px(40.0);
                let button_w = px(118.0);
                let first_row = h_flex()
                    .gap_1()
                    .items_center()
                    .child(div().min_w(left_slot_w).w(left_slot_w).flex_shrink())
                    .child(div().w(button_w).min_w(button_w).flex_shrink().child(primary_button));
                let second_row_left = if let Some(nub) = nub_button {
                    div().min_w(left_slot_w).w(left_slot_w).flex_shrink().child(
                        h_flex().items_center().justify_center().size_full().child(nub),
                    )
                } else {
                    div().min_w(left_slot_w).w(left_slot_w).flex_shrink()
                };
                let second_row = h_flex()
                    .gap_1()
                    .items_center()
                    .child(second_row_left)
                    .child(div().w(button_w).min_w(button_w).flex_shrink().child(open_page_button));
                let buttons = v_flex()
                    .gap_2()
                    .child(first_row)
                    .child(second_row);

                let item = h_flex()
                    .rounded_lg()
                    .px_4()
                    .py_2()
                    .gap_4()
                    .h_32()
                    .bg(theme.background)
                    .border_color(theme.border)
                    .border_1()
                    .size_full()
                    .child(image.rounded_lg().size_16().min_w_16().min_h_16())
                    .child(
                        v_flex()
                            .h(px(104.0))
                            .flex_grow()
                            .gap_1()
                            .overflow_hidden()
                            .child(
                                h_flex()
                                    .gap_1()
                                    .items_end()
                                    .line_clamp(1)
                                    .text_lg()
                                    .child(name)
                                    .child(author_line),
                            )
                            .child(
                                div()
                                    .flex_auto()
                                    .line_height(px(20.0))
                                    .line_clamp(2)
                                    .child(description),
                            )
                            .child(
                                h_flex()
                                    .gap_2p5()
                                    .children(std::iter::once(environment).chain(categories)),
                            ),
                    )
                    .child(v_flex().items_end().gap_2().child(downloads).child(buttons));

                div().pl_3().pt_3().child(item)
            })
            .collect();

        if should_load_more {
            self.load_more(cx);
        }

        items
    }

    fn get_primary_action(&self, hit: &ModrinthHit, cx: &App) -> (PrimaryAction, Option<NubAction>) {
        let project_id = &hit.project_id;
        let title = hit.title.as_deref().unwrap_or("");
        let install_latest = self.can_install_latest && !InterfaceConfig::get(cx).modrinth_install_normally;

        let project_type = self.filter_project_type;
        let is_resourcepack = project_type == ModrinthProjectType::Resourcepack;
        let is_modpack = project_type == ModrinthProjectType::Modpack;
        let is_shader = project_type == ModrinthProjectType::Shader;
        let is_datapack = project_type == ModrinthProjectType::Datapack;

        let project_id_key: Arc<str> = project_id.to_lowercase().into();

        let installed = if is_resourcepack {
            self.installed_resourcepacks_by_project.get(&project_id_key)
        } else if is_modpack {
            self.installed_modpacks_by_project.get(&project_id_key)
        } else if is_shader {
            self.installed_shaders_by_project.get(&project_id_key)
        } else if is_datapack {
            self.installed_datapacks_by_project.get(&project_id_key)
        } else {
            self.installed_mods_by_project.get(&project_id_key)
        };

        if let Some(installed) = installed && !installed.is_empty() {
            if !install_latest {
                return (PrimaryAction::Reinstall, None);
            }

            let (loader, version) = self.install_for.and_then(|id| {
                self.data.instances.read(cx).entries.get(&id).map(|e| {
                    let cfg = e.read(cx).configuration.clone();
                    (cfg.loader, cfg.minecraft_version)
                })
            }).unwrap_or((Loader::Vanilla, Ustr::from("")));

            let mut nub_action = NubAction::CheckForUpdates;
            for installed_content in installed {
                let status = installed_content.status.status_if_matches(loader, version);
                match status {
                    ContentUpdateStatus::Unknown => {}
                    ContentUpdateStatus::AlreadyUpToDate => {
                        if !matches!(nub_action, NubAction::Update(..)) {
                            nub_action = NubAction::UpToDate;
                        }
                    }
                    ContentUpdateStatus::Modrinth => {
                        if let NubAction::Update(ref mut vec) = nub_action {
                            vec.push(installed_content.content_id);
                        } else {
                            nub_action = NubAction::Update(vec![installed_content.content_id]);
                        }
                    }
                    _ => {
                        if nub_action == NubAction::CheckForUpdates {
                            nub_action = NubAction::ErrorCheckingForUpdates;
                        }
                    }
                };
            }
            let main_action = match &nub_action {
                NubAction::Update(ids) => PrimaryAction::Update(ids.clone()),
                _ => PrimaryAction::Installed,
            };
            return (main_action, Some(nub_action));
        }

        if !is_resourcepack && !is_modpack && !is_datapack {
            if let Some(slug) = &hit.slug {
                let slug_key: Arc<str> = slug.to_lowercase().into();
                if self.installed_mods_by_mod_id.contains_key(&slug_key) {
                    return (PrimaryAction::Installed, Some(NubAction::UpToDate));
                }
            }

            if self.installed_mods_by_filename_prefix.contains_key(&project_id_key) {
                return (PrimaryAction::Installed, Some(NubAction::UpToDate));
            }
        }

        if is_shader {
            if self.installed_shaders_by_filename_prefix.contains_key(&project_id_key) {
                return (PrimaryAction::Installed, Some(NubAction::UpToDate));
            }

            if !title.is_empty() {
                let title_words: Vec<String> = title
                    .to_lowercase()
                    .split(|c: char| !c.is_alphanumeric())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .collect();

                for (filename_prefix, _) in &self.installed_shaders_by_filename_prefix {
                    let fp_words: Vec<String> = filename_prefix
                        .to_string()
                        .split(|c: char| !c.is_alphanumeric())
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string())
                        .collect();

                    for title_word in &title_words {
                        for fp_word in &fp_words {
                            if title_word.len() > 2 && fp_word.len() > 2 {
                                if title_word == fp_word || fp_word.contains(title_word) || title_word.contains(fp_word) {
                                    return (PrimaryAction::Installed, Some(NubAction::UpToDate));
                                }
                            }
                        }
                    }
                }
            }
        }

        if is_datapack {
            if self.installed_datapacks_by_filename_prefix.contains_key(&project_id_key) {
                return (PrimaryAction::Installed, Some(NubAction::UpToDate));
            }
            if let Some(slug) = &hit.slug {
                let slug_key: Arc<str> = slug.to_lowercase().into();
                if self.installed_datapacks_by_filename_prefix.contains_key(&slug_key) {
                    return (PrimaryAction::Installed, Some(NubAction::UpToDate));
                }
            }
        }

        if install_latest {
            (PrimaryAction::InstallLatest, None)
        } else {
            (PrimaryAction::Install, None)
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
enum PrimaryAction {
    Install,
    Reinstall,
    InstallLatest,
    Installed,
    Update(Vec<InstanceContentID>),
}

/// Small update-check nub shown to the left of Open Page when content is installed.
#[derive(Clone, PartialEq, Eq)]
enum NubAction {
    CheckForUpdates,
    ErrorCheckingForUpdates,
    UpToDate,
    Update(Vec<InstanceContentID>),
}

impl NubAction {
    fn icon(&self) -> PandoraIcon {
        match self {
            NubAction::CheckForUpdates => PandoraIcon::RefreshCcw,
            NubAction::ErrorCheckingForUpdates => PandoraIcon::TriangleAlert,
            NubAction::UpToDate => PandoraIcon::Check,
            NubAction::Update(..) => PandoraIcon::Download,
        }
    }

    fn button_variant(&self) -> ButtonVariant {
        match self {
            NubAction::CheckForUpdates => ButtonVariant::Warning,
            NubAction::ErrorCheckingForUpdates => ButtonVariant::Danger,
            NubAction::UpToDate => ButtonVariant::Secondary,
            NubAction::Update(..) => ButtonVariant::Success,
        }
    }
}

impl PrimaryAction {
    pub fn text(&self) -> SharedString {
        match self {
            PrimaryAction::Install => ts!("instance.content.install.label"),
            PrimaryAction::Reinstall => ts!("instance.content.install.reinstall"),
            PrimaryAction::InstallLatest => ts!("instance.content.install.latest"),
            PrimaryAction::Installed => ts!("instance.content.installed"),
            PrimaryAction::Update(..) => ts!("instance.content.update.label"),
        }
    }

    pub fn icon(&self) -> PandoraIcon {
        match self {
            PrimaryAction::Install => PandoraIcon::Download,
            PrimaryAction::Reinstall => PandoraIcon::Download,
            PrimaryAction::InstallLatest => PandoraIcon::Download,
            PrimaryAction::Installed => PandoraIcon::Check,
            PrimaryAction::Update(..) => PandoraIcon::Download,
        }
    }

    pub fn button_variant(&self) -> ButtonVariant {
        match self {
            PrimaryAction::Install => ButtonVariant::Success,
            PrimaryAction::Reinstall => ButtonVariant::Success,
            PrimaryAction::InstallLatest => ButtonVariant::Success,
            PrimaryAction::Installed => ButtonVariant::Secondary,
            PrimaryAction::Update(..) => ButtonVariant::Success,
        }
    }
}

impl Render for ModrinthSearchPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let can_load_more = self.total_hits > self.hits.len();
        let scroll_handle = self.scroll_handle.clone();

        let item_count = self.hits.len() + if can_load_more || self.search_error.is_some() { 1 } else { 0 };

        if let Some((mods_state, load_serial)) = &self.mods_load_state
            && let Some(install_for) = self.install_for
        {
            let state = mods_state.load(std::sync::atomic::Ordering::SeqCst);
            if state.should_send_load_request() {
                self.data.backend_handle.send_with_serial(MessageToBackend::RequestLoadMods { id: install_for }, load_serial);
            }
        }

        let list = h_flex()
            .image_cache(self.image_cache.clone())
            .size_full()
            .overflow_y_hidden()
            .child(
                uniform_list(
                    "uniform-list",
                    item_count,
                    cx.processor(Self::render_items),
                )
                .size_full()
                .track_scroll(&scroll_handle),
            )
            .child(
                div()
                    .w_3()
                    .h_full()
                    .py_3()
                    .child(Scrollbar::vertical(&scroll_handle)),
            );

        let mut top_bar = h_flex()
            .w_full()
            .gap_3()
            .child(Input::new(&self.search_state));


        if self.can_install_latest {
            let tooltip = |window: &mut Window, cx: &mut App| {
                Tooltip::new(ts!("instance.content.install.always_latest")).build(window, cx)
            };

            let install_latest = !InterfaceConfig::get(cx).modrinth_install_normally;
            top_bar = top_bar.child(Checkbox::new("install-latest")
                .label(ts!("instance.content.install.latest"))
                .tooltip(tooltip)
                .checked(install_latest)
                .on_click({
                    move |value, _, cx| {
                        InterfaceConfig::get_mut(cx).modrinth_install_normally = !*value;
                    }
                })
            );
        }

        let theme = cx.theme();
        let content = v_flex()
            .size_full()
            .gap_3()
            .p_3()
            .pl_0()
            .child(top_bar)
            .child(div().size_full().rounded_lg().border_1().border_color(theme.border).child(list));

        let type_button_group = ButtonGroup::new("type")
            .layout(Axis::Vertical)
            .outline()
            .child(Button::new("mods").label(ts!("instance.content.mods")).selected(self.filter_project_type == ModrinthProjectType::Mod))
            .child(
                Button::new("modpacks")
                    .label(ts!("instance.content.modpacks"))
                    .selected(self.filter_project_type == ModrinthProjectType::Modpack),
            )
            .child(
                Button::new("resourcepacks")
                    .label(ts!("instance.content.resourcepacks"))
                    .selected(self.filter_project_type == ModrinthProjectType::Resourcepack),
            )
            .child(Button::new("shaders").label(ts!("instance.content.shaders")).selected(self.filter_project_type == ModrinthProjectType::Shader))
            .child(Button::new("datapacks").label(ts!("instance.content.datapacks")).selected(self.filter_project_type == ModrinthProjectType::Datapack))
            .on_click(cx.listener(|page, clicked: &Vec<usize>, window, cx| match clicked[0] {
                0 => page.set_project_type(ModrinthProjectType::Mod, window, cx),
                1 => page.set_project_type(ModrinthProjectType::Modpack, window, cx),
                2 => page.set_project_type(ModrinthProjectType::Resourcepack, window, cx),
                3 => page.set_project_type(ModrinthProjectType::Shader, window, cx),
                4 => page.set_project_type(ModrinthProjectType::Datapack, window, cx),
                _ => {},
            }));

        let loader_button_group = if self.filter_project_type == ModrinthProjectType::Mod || self.filter_project_type == ModrinthProjectType::Modpack {
            Some(ButtonGroup::new("loader_group")
                .layout(Axis::Vertical)
                .outline()
                .multiple(true)
                .child(Button::new("fabric").label(ts!("modrinth.category.fabric")).selected(self.filter_loaders.contains(&Loader::Fabric)))
                .child(Button::new("forge").label(ts!("modrinth.category.forge")).selected(self.filter_loaders.contains(&Loader::Forge)))
                .child(Button::new("neoforge").label(ts!("modrinth.category.neoforge")).selected(self.filter_loaders.contains(&Loader::NeoForge)))
                .on_click(cx.listener(|page, clicked: &Vec<usize>, window, cx| {
                    page.set_filter_loaders(clicked.iter().filter_map(|index| match index {
                        0 => Some(Loader::Fabric),
                        1 => Some(Loader::Forge),
                        2 => Some(Loader::NeoForge),
                        _ => None
                    }).collect(), window, cx);
                })))
        } else {
            None
        };

        let categories = match self.filter_project_type {
            ModrinthProjectType::Mod => FILTER_MOD_CATEGORIES,
            ModrinthProjectType::Modpack => FILTER_MODPACK_CATEGORIES,
            ModrinthProjectType::Resourcepack => FILTER_RESOURCEPACK_CATEGORIES,
            ModrinthProjectType::Shader => FILTER_SHADERPACK_CATEGORIES,
            ModrinthProjectType::Datapack => FILTER_DATAPACK_CATEGORIES,
            ModrinthProjectType::Other => &[],
        };

        let is_shown = self.show_categories.load(std::sync::atomic::Ordering::Relaxed);
        let show_categories = self.show_categories.clone();

        let category = v_flex()
            .gap_1()
            .child(
                Button::new("toggle-categories")
                    .label(ts!("instance.content.categories"))
                    .icon(if is_shown { PandoraIcon::ChevronDown } else { PandoraIcon::ChevronRight })
                    .when(!is_shown, |this| this.outline())
                    .on_click(move |_, _, _| {
                        show_categories.store(!is_shown, std::sync::atomic::Ordering::Relaxed);
                    })
            )
            .child(
                ButtonGroup::new("category_group")
                    .layout(Axis::Vertical)
                    .outline()
                    .multiple(true)
                    .children(categories.iter().map(|id| {
                        Button::new(*id)
                            .child(
                                h_flex().w_full().justify_start().gap_2()
                                .when_some(icon_for(id), |this, icon| {
                                    this.child(Icon::empty().path(icon))
                                })
                                .child(Label::new(ts_short!(format!("modrinth.category.{}", id)))))
                            .selected(self.filter_categories.contains(id))
                    }))
                    .on_click(cx.listener(|page, clicked: &Vec<usize>, window, cx| {
                        page.set_filter_categories(clicked.iter()
                            .filter_map(|index| categories.get(*index).map(|s| *s))
                            .collect(), window, cx);
                    }))
                    .when(!is_shown, |this| this.invisible().h_0())
            )
            .into_any_element();

        let shows_version_filter = matches!(
            self.filter_project_type,
            ModrinthProjectType::Mod | ModrinthProjectType::Modpack | ModrinthProjectType::Shader
        );
        let filter_version_toggle = if shows_version_filter && let Some(filter_version) = self.filter_version {
            let title = format!("{}: {}", ts!("instance.version"), filter_version);
            Some(Button::new("filter_version").label(title)
                .outline()
                .selected(InterfaceConfig::get(cx).modrinth_filter_version)
                .on_click(cx.listener(|page, _, _, cx| {
                    let cfg = InterfaceConfig::get_mut(cx);
                    cfg.modrinth_filter_version = !cfg.modrinth_filter_version;
                    page.reload(cx);
                })))
        } else {
            None
        };

        let parameters = v_flex()
            .h_full()
            .overflow_y_scrollbar()
            .w_auto()
            .min_w(px(170.0))
            .p_3()
            .gap_3()
            .child(type_button_group)
            .when_some(loader_button_group, |this, group| this.child(group))
            .when_some(filter_version_toggle, |this, button| this.child(button))
            .child(category);

        Page::new(self.page_path.create_breadcrumb(&self.data, cx))
            .child(h_flex().flex_1().min_h_0().size_full().child(parameters).child(content))
    }
}

fn extract_modrinth_project_id_from_filename(filename: &str) -> Option<Arc<str>> {
    let filename = filename.strip_suffix(".disabled").unwrap_or(filename);
    let filename = filename.strip_suffix(".jar").unwrap_or(filename);
    let filename = filename.strip_suffix(".zip").unwrap_or(filename);

    if let Some(last_dash_pos) = filename.rfind('-') {
        let after_last_dash = &filename[last_dash_pos + 1..];
        if after_last_dash.is_empty() {
            return None;
        }

        let first_char = after_last_dash.chars().next().unwrap();
        if first_char.is_ascii_digit() {
            let result = filename[..last_dash_pos].to_lowercase();
            return Some(result.into());
        }

        if let Some(second_dash_pos) = filename[..last_dash_pos].rfind('-') {
            let potential_version = &filename[second_dash_pos + 1..last_dash_pos];
            if !potential_version.is_empty() && potential_version.chars().next()?.is_ascii_digit() {
                let result = filename[..second_dash_pos].to_lowercase();
                return Some(result.into());
            }
        }
    }

    None
}

fn extract_modrinth_project_id_from_url(url: &str) -> Option<Arc<str>> {
    if let Some(data_pos) = url.find("/data/") {
        let after_data = &url[data_pos + 6..];
        if let Some(slash_pos) = after_data.find('/') {
            let project_id = &after_data[..slash_pos];
            if !project_id.is_empty() {
                return Some(project_id.to_lowercase().into());
            }
        }
    }
    None
}

fn format_downloads(downloads: usize) -> SharedString {
    if downloads >= 1_000_000_000 {
        ts!("instance.content.downloads", num = format!("{}B", (downloads / 10_000_000) as f64 / 100.0))
    } else if downloads >= 1_000_000 {
        ts!("instance.content.downloads", num = format!("{}M", (downloads / 10_000) as f64 / 100.0))
    } else if downloads >= 10_000 {
        ts!("instance.content.downloads", num = format!("{}K", (downloads / 10) as f64 / 100.0))
    } else {
        ts!("instance.content.downloads", num = downloads)
    }
}

fn icon_for(str: &str) -> Option<&'static str> {
    match str {
        "forge" => Some("icons/anvil.svg"),
        "fabric" => Some("icons/scroll.svg"),
        "neoforge" => Some("icons/cat.svg"),
        "quilt" => Some("icons/grid-2x2.svg"),
        "adventure" => Some("icons/compass.svg"),
        "cursed" => Some("icons/bug.svg"),
        "decoration" => Some("icons/house.svg"),
        "economy" => Some("icons/dollar-sign.svg"),
        "equipment" | "combat" => Some("icons/swords.svg"),
        "food" => Some("icons/carrot.svg"),
        "game-mechanics" => Some("icons/sliders-vertical.svg"),
        "library" | "items" => Some("icons/book.svg"),
        "magic" => Some("icons/wand.svg"),
        "management" => Some("icons/server.svg"),
        "minigame" => Some("icons/award.svg"),
        "mobs" | "entities" => Some("icons/cat.svg"),
        "optimization" => Some("icons/zap.svg"),
        "social" => Some("icons/message-circle.svg"),
        "storage" => Some("icons/archive.svg"),
        "technology" => Some("icons/hard-drive.svg"),
        "transportation" => Some("icons/truck.svg"),
        "utility" => Some("icons/briefcase.svg"),
        "worldgen" | "locale" => Some("icons/globe.svg"),
        "audio" => Some("icons/headphones.svg"),
        "blocks" | "rift" => Some("icons/box.svg"),
        "core-shaders" => Some("icons/cpu.svg"),
        "fonts" => Some("icons/type.svg"),
        "gui" => Some("icons/panels-top-left.svg"),
        "models" => Some("icons/layers.svg"),
        "cartoon" => Some("icons/brush.svg"),
        "fantasy" => Some("icons/wand-sparkles.svg"),
        "realistic" => Some("icons/camera.svg"),
        "semi-realistic" => Some("icons/film.svg"),
        "vanilla-like" => Some("icons/ice-cream-cone.svg"),
        "atmosphere" => Some("icons/cloud-sun-rain.svg"),
        "colored-lighting" => Some("icons/palette.svg"),
        "foliage" => Some("icons/tree-pine.svg"),
        "path-tracing" => Some("icons/waypoints.svg"),
        "pbr" => Some("icons/lightbulb.svg"),
        "reflections" => Some("icons/flip-horizontal-2.svg"),
        "shadows" => Some("icons/mountain.svg"),
        "challenging" => Some("icons/chart-no-axes-combined.svg"),
        "kitchen-sink" => Some("icons/bath.svg"),
        "lightweight" | "liteloader" => Some("icons/feather.svg"),
        "multiplayer" => Some("icons/users.svg"),
        "quests" => Some("icons/network.svg"),
        "modded" => Some("icons/puzzle.svg"),
        "simplistic" => Some("icons/box.svg"),
        "themed" => Some("icons/palette.svg"),
        "tweaks" => Some("icons/sliders-vertical.svg"),
        _ => None,
    }
}

const FILTER_MOD_CATEGORIES: &[&'static str] = &[
    "adventure",
    "cursed",
    "decoration",
    "economy",
    "equipment",
    "food",
    "library",
    "magic",
    "management",
    "minigame",
    "mobs",
    "optimization",
    "social",
    "storage",
    "technology",
    "transportation",
    "utility",
    "worldgen"
];

const FILTER_MODPACK_CATEGORIES: &[&'static str] = &[
    "adventure",
    "challenging",
    "combat",
    "kitchen-sink",
    "lightweight",
    "magic",
    "multiplayer",
    "optimization",
    "quests",
    "technology",
];

const FILTER_RESOURCEPACK_CATEGORIES: &[&'static str] = &[
    "combat",
    "cursed",
    "decoration",
    "modded",
    "realistic",
    "simplistic",
    "themed",
    "tweaks",
    "utility",
    "vanilla-like",
];

const FILTER_DATAPACK_CATEGORIES: &[&'static str] = &[
    "adventure",
    "decoration",
    "game-mechanics",
    "mobs",
    "worldgen",
];

const FILTER_SHADERPACK_CATEGORIES: &[&'static str] = &[
    "cartoon",
    "cursed",
    "fantasy",
    "realistic",
    "semi-realistic",
    "vanilla-like",
];
