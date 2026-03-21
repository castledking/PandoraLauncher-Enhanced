use std::{cmp::Ordering, sync::Arc};

use crate::ts;
use bridge::{
    install::{ContentDownload, ContentInstall, ContentInstallFile, InstallTarget},
    instance::{InstanceID, InstanceWorldSummary},
    message::MessageToBackend,
    meta::MetadataRequest,
    safe_path::SafePath,
};
use enumset::EnumSet;
use gpui::{prelude::*, *};
use gpui_component::{
    IndexPath, Selectable, Sizable, WindowExt,
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    dialog::Dialog,
    h_flex,
    notification::NotificationType,
    select::{SearchableVec, Select, SelectItem, SelectState},
    spinner::Spinner,
    v_flex,
};
use relative_path::RelativePath;
use rustc_hash::FxHashMap;
use schema::{
    content::ContentSource,
    loader::Loader,
    modrinth::{
        ModrinthDependency, ModrinthDependencyType, ModrinthLoader, ModrinthProjectType, ModrinthProjectVersion,
        ModrinthProjectVersionsRequest, ModrinthProjectVersionsResult, ModrinthVersionStatus, ModrinthVersionType,
    },
};

use crate::{
    component::{error_alert::ErrorAlert, instance_dropdown::InstanceDropdown},
    png_render_cache,
    entity::{
        DataEntities,
        instance::InstanceEntry,
        metadata::{AsMetadataResult, FrontendMetadata, FrontendMetadataResult, FrontendMetadataState},
    },
    interface_config::InterfaceConfig,
    root,
};

pub fn perform_datapack_install(
    world: String,
    install_file: &schema::modrinth::ModrinthFile,
    target: InstallTarget,
    loader_override: Option<Loader>,
    selected_loader: &Option<SharedString>,
    selected_minecraft_version: &Option<SharedString>,
    install_dependencies: bool,
    required_dependencies: &[ModrinthDependency],
    project_id: Arc<str>,
    name: &str,
    data: &DataEntities,
    window: &mut Window,
    cx: &mut App,
) {
    let path = RelativePath::new("saves")
        .join(&world)
        .join("datapacks")
        .join(&*install_file.filename);
    let Some(path) = SafePath::from_relative_path(&path) else {
        window.push_notification((NotificationType::Error, "Invalid/dangerous filename"), cx);
        return;
    };
    let mut loader_hint = Loader::Unknown;
    if let Some(override_loader) = loader_override {
        loader_hint = override_loader;
    } else if let Some(selected_loader) = selected_loader {
        let modrinth_loader = ModrinthLoader::from_name(selected_loader);
        match modrinth_loader {
            ModrinthLoader::Fabric => loader_hint = Loader::Fabric,
            ModrinthLoader::Forge => loader_hint = Loader::Forge,
            ModrinthLoader::NeoForge => loader_hint = Loader::NeoForge,
            _ => {},
        }
    }
    let mut version_hint = None;
    if let Some(selected_minecraft_version) = selected_minecraft_version {
        version_hint = Some(selected_minecraft_version.as_str().into());
    }
    let mut target = target;
    if let InstallTarget::NewInstance { name: ref mut instance_name } = target {
        *instance_name = Some(name.into());
    }
    let mut files = Vec::new();
    if install_dependencies {
        for dep in required_dependencies {
                files.push(ContentInstallFile {
                    replace_old: None,
                    path: bridge::install::ContentInstallPath::Automatic,
                    download: ContentDownload::Modrinth {
                        project_id: dep.project_id.clone().unwrap(),
                        version_id: dep.version_id.clone(),
                        install_dependencies: true,
                    },
                    content_source: ContentSource::ModrinthProject {
                        project: dep.project_id.clone().unwrap(),
                    },
            });
        }
    }
    files.push(ContentInstallFile {
        replace_old: None,
        path: bridge::install::ContentInstallPath::Safe(path),
        download: ContentDownload::Url {
            url: install_file.url.clone(),
            sha1: install_file.hashes.sha1.clone(),
            size: install_file.size,
        },
        content_source: ContentSource::ModrinthProject { project: project_id },
    });
    let content_install = ContentInstall {
        target,
        loader_hint,
        version_hint,
        datapack_world: Some(world.clone()),
        files: files.into(),
    };
    window.close_dialog(cx);
    root::start_install(content_install, &data.backend_handle, window, cx);
}

struct WorldSelectState {
    selected_idx: usize,
    worlds: Vec<InstanceWorldSummary>,
}

struct VersionMatrixLoaders {
    loaders: EnumSet<ModrinthLoader>,
    same_loaders_for_all_versions: bool,
}

struct InstallDialog {
    title: SharedString,
    name: SharedString,

    project_versions: Arc<[ModrinthProjectVersion]>,
    data: DataEntities,
    project_type: ModrinthProjectType,
    project_id: Arc<str>,

    version_matrix: FxHashMap<&'static str, VersionMatrixLoaders>,
    instances: Option<Entity<SelectState<InstanceDropdown>>>,
    unsupported_instances: usize,

    target: Option<InstallTarget>,

    last_selected_minecraft_version: Option<SharedString>,
    last_selected_loader: Option<SharedString>,

    fixed_minecraft_version: Option<&'static str>,
    minecraft_version_select_state: Option<Entity<SelectState<SearchableVec<SharedString>>>>,

    fixed_loader: Option<ModrinthLoader>,
    loader_select_state: Option<Entity<SelectState<Vec<SharedString>>>>,
    skip_loader_check_for_mod_version: bool,
    install_dependencies: bool,

    mod_version_select_state: Option<Entity<SelectState<SearchableVec<ModVersionItem>>>>,

    /// When set, the modal will pre-select this version (by id) in the version dropdown.
    selected_version_id: Option<Arc<str>>,

    /// Override loader for content install (e.g. when user just switched vanilla->fabric).
    loader_override: Option<Loader>,
}

pub fn open(
    name: &str,
    project_id: Arc<str>,
    project_type: ModrinthProjectType,
    install_for: Option<InstanceID>,
    data: &DataEntities,
    window: &mut Window,
    cx: &mut App,
) {
    open_with_version(name, project_id, project_type, install_for, data, window, cx, None, None);
}

pub fn open_with_version(
    name: &str,
    project_id: Arc<str>,
    project_type: ModrinthProjectType,
    install_for: Option<InstanceID>,
    data: &DataEntities,
    window: &mut Window,
    cx: &mut App,
    selected_version_id: Option<Arc<str>>,
    loader_override: Option<Loader>,
) {
    let loaders = if project_type == ModrinthProjectType::Datapack {
        Some(Arc::from([ModrinthLoader::Datapack, ModrinthLoader::Minecraft]))
    } else {
        None
    };
    let project_versions = FrontendMetadata::request(
        &data.metadata,
        MetadataRequest::ModrinthProjectVersions(ModrinthProjectVersionsRequest {
            project_id: project_id.clone(),
            game_versions: None,
            loaders,
        }),
        cx,
    );

    open_from_entity(
        SharedString::new(name),
        project_versions,
        project_id,
        project_type,
        install_for,
        data.clone(),
        window,
        cx,
        selected_version_id,
        loader_override,
    );
}

fn open_from_entity(
    name: SharedString,
    project_versions: Entity<FrontendMetadataState>,
    project_id: Arc<str>,
    project_type: ModrinthProjectType,
    install_for: Option<InstanceID>,
    data: DataEntities,
    window: &mut Window,
    cx: &mut App,
    selected_version_id: Option<Arc<str>>,
    loader_override: Option<Loader>,
) {
    let title = SharedString::new(format!("Install {}", name));

    let result: FrontendMetadataResult<ModrinthProjectVersionsResult> = project_versions.read(cx).result();
    match result {
        FrontendMetadataResult::Loading => {
            let loader_override = loader_override;
            let _subscription = window.observe(&project_versions, cx, move |project_versions, window, cx| {
                window.close_all_dialogs(cx);
                open_from_entity(
                    name.clone(),
                    project_versions,
                    project_id.clone(),
                    project_type,
                    install_for,
                    data.clone(),
                    window,
                    cx,
                    selected_version_id.clone(),
                    loader_override,
                );
            });
            window.open_dialog(cx, move |dialog, _, _| {
                let _ = &_subscription;
                dialog
                    .title(title.clone())
                    .child(h_flex().gap_2().child("Loading mod versions...").child(Spinner::new()))
            });
        },
        FrontendMetadataResult::Loaded(versions) => {
            let mut valid_project_versions = Vec::with_capacity(versions.0.len());

            let mut version_matrix: FxHashMap<&'static str, VersionMatrixLoaders> = FxHashMap::default();
            for version in versions.0.iter() {
                let Some(loaders) = version.loaders.clone() else {
                    continue;
                };
                let Some(game_versions) = &version.game_versions else {
                    continue;
                };
                if version.files.is_empty() {
                    continue;
                }
                if let Some(status) = version.status
                    && !matches!(status, ModrinthVersionStatus::Listed | ModrinthVersionStatus::Archived)
                {
                    continue;
                }

                let mut loaders = EnumSet::from_iter(loaders.iter().copied());
                loaders.remove(ModrinthLoader::Unknown);
                if loaders.is_empty() {
                    continue;
                }

                valid_project_versions.push(version.clone());

                for game_version in game_versions.iter() {
                    match version_matrix.entry(game_version.as_str()) {
                        std::collections::hash_map::Entry::Occupied(mut occupied_entry) => {
                            occupied_entry.get_mut().same_loaders_for_all_versions &=
                                occupied_entry.get().loaders == loaders;
                            occupied_entry.get_mut().loaders |= loaders;
                        },
                        std::collections::hash_map::Entry::Vacant(vacant_entry) => {
                            vacant_entry.insert(VersionMatrixLoaders {
                                loaders,
                                same_loaders_for_all_versions: true,
                            });
                        },
                    }
                }
            }

            if version_matrix.is_empty() {
                open_error_dialog(title.clone(), "No mod versions found".into(), window, cx);
                return;
            }
            if let Some(install_for) = install_for {
                let Some(instance) = data.instances.read(cx).entries.get(&install_for) else {
                    open_error_dialog(title.clone(), "Unable to find instance".into(), window, cx);
                    return;
                };

                let instance = instance.read(cx);

                let minecraft_version = instance.configuration.minecraft_version.as_str();
                let instance_loader = instance.configuration.loader;
                let allow_all_versions = matches!(
                    project_type,
                    ModrinthProjectType::Resourcepack | ModrinthProjectType::Shader | ModrinthProjectType::Datapack
                );

                let fixed_minecraft_version = if allow_all_versions {
                    None
                } else {
                    let Some(loaders) = version_matrix.get(minecraft_version) else {
                        let error_message =
                            SharedString::from(&format!("No mod versions found for {}", minecraft_version));
                        open_error_dialog(title.clone(), error_message, window, cx);
                        return;
                    };
                    let valid_loader = project_type != ModrinthProjectType::Mod
                        && project_type != ModrinthProjectType::Modpack
                        || instance_loader == Loader::Vanilla
                        || loaders.loaders.contains(instance_loader.as_modrinth_loader());
                    if !valid_loader {
                        let error_message = SharedString::from(&format!(
                            "No mod versions found for {} {}",
                            instance_loader.name(),
                            minecraft_version
                        ));
                        open_error_dialog(title.clone(), error_message, window, cx);
                        return;
                    }
                    Some(minecraft_version)
                };

                let title = title.clone();
                let instance_id = instance.id;
                let fixed_loader = if let Some(override_loader) = loader_override {
                    Some(override_loader.as_modrinth_loader())
                } else if (project_type == ModrinthProjectType::Mod
                    || project_type == ModrinthProjectType::Modpack)
                    && instance_loader != Loader::Vanilla
                {
                    Some(instance_loader.as_modrinth_loader())
                } else if project_type == ModrinthProjectType::Datapack {
                    Some(ModrinthLoader::Datapack)
                } else {
                    None
                };
                let install_dialog = InstallDialog {
                    title,
                    name: name.into(),
                    project_versions: valid_project_versions.into(),
                    data,
                    project_type,
                    project_id,
                    version_matrix,
                    instances: None,
                    unsupported_instances: 0,
                    target: Some(InstallTarget::Instance(instance_id)),
                    fixed_minecraft_version,
                    minecraft_version_select_state: None,
                    fixed_loader,
                    loader_select_state: None,
                    last_selected_minecraft_version: None,
                    skip_loader_check_for_mod_version: false,
                    install_dependencies: true,
                    mod_version_select_state: None,
                    last_selected_loader: None,
                    selected_version_id: selected_version_id.clone(),
                    loader_override,
                };
                install_dialog.show(window, cx);
            } else {
                let instance_entries = data.instances.clone();

                let entries: Arc<[InstanceEntry]> = instance_entries
                    .read(cx)
                    .entries
                    .iter()
                    .filter_map(|(_, instance)| {
                        let instance = instance.read(cx);

                        let minecraft_version = instance.configuration.minecraft_version.as_str();
                        let instance_loader = instance.configuration.loader;

                        if let Some(loaders) = version_matrix.get(minecraft_version) {
                            let mut valid_loader = true;
                            if project_type == ModrinthProjectType::Mod || project_type == ModrinthProjectType::Modpack
                            {
                                valid_loader = instance_loader == Loader::Vanilla
                                    || loaders.loaders.contains(instance_loader.as_modrinth_loader());
                            }
                            if valid_loader {
                                return Some(instance.clone());
                            }
                        }

                        None
                    })
                    .collect();

                let unsupported_instances = instance_entries.read(cx).entries.len().saturating_sub(entries.len());
                let instances = if !entries.is_empty() {
                    let dropdown = InstanceDropdown::create(entries, window, cx);
                    dropdown
                        .update(cx, |dropdown, cx| dropdown.set_selected_index(Some(IndexPath::default()), window, cx));
                    Some(dropdown)
                } else {
                    None
                };

                let install_dialog = InstallDialog {
                    title,
                    name: name.into(),
                    project_versions: valid_project_versions.into(),
                    data,
                    project_type,
                    project_id,
                    version_matrix,
                    instances,
                    unsupported_instances,
                    target: None,
                    fixed_minecraft_version: None,
                    minecraft_version_select_state: None,
                    fixed_loader: None,
                    loader_select_state: None,
                    last_selected_minecraft_version: None,
                    skip_loader_check_for_mod_version: false,
                    install_dependencies: true,
                    mod_version_select_state: None,
                    last_selected_loader: None,
                    selected_version_id: selected_version_id.clone(),
                    loader_override: None,
                };
                install_dialog.show(window, cx);
            }
        },
        FrontendMetadataResult::Error(message) => {
            window.open_dialog(cx, move |modal, _, _| {
                modal.title(title.clone()).child(ErrorAlert::new(
                    "Error requesting from Modrinth".into(),
                    message.clone(),
                ))
            });
        },
    }
}

fn open_error_dialog(title: SharedString, text: SharedString, window: &mut Window, cx: &mut App) {
    window.open_dialog(cx, move |modal, _, _| modal.title(title.clone()).child(text.clone()));
}

impl InstallDialog {
    fn show(self, window: &mut Window, cx: &mut App) {
        let install_dialog = cx.new(|_| self);
        window.open_dialog(cx, move |modal, window, cx| {
            install_dialog.update(cx, |this, cx| this.render(modal, window, cx))
        });
    }

    fn render(&mut self, modal: Dialog, window: &mut Window, cx: &mut Context<Self>) -> Dialog {
        let modal = modal.title(self.title.clone());

        if self.target.is_none() {
            let create_instance_label = match self.project_type {
                ModrinthProjectType::Mod => "Create new instance with this mod",
                ModrinthProjectType::Modpack => "Create new instance with this modpack",
                ModrinthProjectType::Resourcepack => "Create new instance with this resourcepack",
                ModrinthProjectType::Shader => "Create new instance with this shader",
                ModrinthProjectType::Datapack => "Create new instance with this datapack",
                ModrinthProjectType::Other => "Create new instance with this file",
            };

            let content = v_flex()
                .gap_2()
                .text_center()
                .when_some(self.instances.as_ref(), |content, instances| {
                    let read_instances = instances.read(cx);
                    let selected_instance: Option<InstanceEntry> = read_instances.selected_value().cloned();

                    let button_and_dropdown = h_flex()
                        .gap_2()
                        .child(
                            v_flex()
                                .w_full()
                                .gap_0p5()
                                .child(
                                    Select::new(instances).placeholder("Select an instance").title_prefix("Instance: "),
                                )
                                .when(self.unsupported_instances > 0, |content| {
                                    content
                                        .child(format!("({} instances were incompatible)", self.unsupported_instances))
                                }),
                        )
                        .when_some(selected_instance, |dialog, instance| {
                            dialog.child(Button::new("instance").success().h_full().label("Add to instance").on_click(
                                cx.listener(move |this, _, _, _| {
                                    this.target = Some(InstallTarget::Instance(instance.id));
                                    this.fixed_minecraft_version =
                                        Some(instance.configuration.minecraft_version.as_str());
                                    if (this.project_type == ModrinthProjectType::Mod
                                        || this.project_type == ModrinthProjectType::Modpack)
                                        && instance.configuration.loader != Loader::Vanilla
                                    {
                                        this.fixed_loader = Some(instance.configuration.loader.as_modrinth_loader());
                                    }
                                }),
                            ))
                        });

                    content.child(button_and_dropdown).child("— OR —")
                })
                .child(Button::new("create").success().label(create_instance_label).on_click(cx.listener(
                    |this, _, _, _| {
                        this.target = Some(InstallTarget::NewInstance { name: None });
                    },
                )));

            return modal.child(content);
        }

        if self.minecraft_version_select_state.is_none() {
            if let Some(minecraft_version) = self.fixed_minecraft_version.clone() {
                self.minecraft_version_select_state = Some(cx.new(|cx| {
                    let mut select_state = SelectState::new(
                        SearchableVec::new(vec![SharedString::new_static(minecraft_version)]),
                        None,
                        window,
                        cx,
                    )
                    .searchable(true);
                    select_state.set_selected_index(Some(IndexPath::default()), window, cx);
                    select_state
                }));
            } else {
                let mut keys: Vec<SharedString> =
                    self.version_matrix.keys().cloned().map(SharedString::new_static).collect();
                keys.sort_by(|a, b| {
                    let a_is_snapshot = a.contains("w") || a.contains("pre") || a.contains("rc");
                    let b_is_snapshot = b.contains("w") || b.contains("pre") || b.contains("rc");
                    if a_is_snapshot != b_is_snapshot {
                        if a_is_snapshot {
                            Ordering::Greater
                        } else {
                            Ordering::Less
                        }
                    } else {
                        lexical_sort::natural_lexical_cmp(a, b).reverse()
                    }
                });
                self.minecraft_version_select_state = Some(cx.new(|cx| {
                    let mut select_state =
                        SelectState::new(SearchableVec::new(keys), None, window, cx).searchable(true);
                    select_state.set_selected_index(Some(IndexPath::default()), window, cx);
                    select_state
                }));
            }
        }

        let selected_minecraft_version = self
            .minecraft_version_select_state
            .as_ref()
            .and_then(|v| v.read(cx).selected_value())
            .cloned();
        let game_version_changed = self.last_selected_minecraft_version != selected_minecraft_version;
        self.last_selected_minecraft_version = selected_minecraft_version.clone();

        if self.loader_select_state.is_none() || game_version_changed {
            self.last_selected_minecraft_version = selected_minecraft_version.clone();
            self.skip_loader_check_for_mod_version = false;

            if let Some(loader) = self.fixed_loader {
                let loader = SharedString::new_static(loader.pretty_name());
                self.loader_select_state = Some(cx.new(|cx| {
                    let mut select_state = SelectState::new(vec![loader], None, window, cx);
                    select_state.set_selected_index(Some(IndexPath::default()), window, cx);
                    select_state
                }));
            } else if let Some(selected_minecraft_version) = selected_minecraft_version.clone()
                && let Some(loaders) = self.version_matrix.get(selected_minecraft_version.as_str())
            {
                if loaders.same_loaders_for_all_versions {
                    let single_loader = if loaders.loaders.len() == 1 {
                        SharedString::new_static(loaders.loaders.iter().next().unwrap().pretty_name())
                    } else {
                        let mut string = String::new();
                        let mut first = true;
                        for loader in loaders.loaders.iter() {
                            if first {
                                first = false;
                            } else {
                                string.push_str(" / ");
                            }
                            string.push_str(loader.pretty_name());
                        }
                        SharedString::new(string)
                    };

                    self.skip_loader_check_for_mod_version = true;
                    self.loader_select_state = Some(cx.new(|cx| {
                        let mut select_state = SelectState::new(vec![single_loader], None, window, cx);
                        select_state.set_selected_index(Some(IndexPath::default()), window, cx);
                        select_state
                    }));
                } else {
                    let keys: Vec<SharedString> = loaders
                        .loaders
                        .iter()
                        .map(ModrinthLoader::pretty_name)
                        .map(SharedString::new_static)
                        .collect();

                    let previous = self
                        .loader_select_state
                        .as_ref()
                        .and_then(|state| state.read(cx).selected_value().cloned());
                    self.loader_select_state = Some(cx.new(|cx| {
                        let mut select_state = SelectState::new(keys, None, window, cx);
                        if let Some(previous) = previous {
                            select_state.set_selected_value(&previous, window, cx);
                        }
                        if select_state.selected_index(cx).is_none() {
                            select_state.set_selected_index(Some(IndexPath::default()), window, cx);
                        }
                        select_state
                    }));
                }
            }
            if self.loader_select_state.is_none() {
                self.loader_select_state = Some(cx.new(|cx| {
                    let mut select_state = SelectState::new(Vec::new(), None, window, cx);
                    select_state.set_selected_index(Some(IndexPath::default()), window, cx);
                    select_state
                }));
            }
        }

        let selected_loader = self.loader_select_state.as_ref().and_then(|v| v.read(cx).selected_value()).cloned();
        let loader_changed = self.last_selected_loader != selected_loader;
        self.last_selected_loader = selected_loader.clone();

        if (self.mod_version_select_state.is_none() || game_version_changed || loader_changed)
            && let Some(selected_game_version) = selected_minecraft_version.clone()
            && let Some(selected_loader) = self.last_selected_loader.clone()
        {
            let selected_game_version = selected_game_version.as_str();

            let selected_loader = if self.skip_loader_check_for_mod_version {
                None
            } else {
                Some(ModrinthLoader::from_name(selected_loader.as_str()))
            };

            let mod_versions: Vec<ModVersionItem> = self
                .project_versions
                .iter()
                .filter_map(|version| {
                    let Some(game_versions) = &version.game_versions else {
                        return None;
                    };
                    let Some(loaders) = &version.loaders else {
                        return None;
                    };
                    if version.files.is_empty() {
                        return None;
                    }
                    let matches_game_version = self.project_type == ModrinthProjectType::Datapack
                        || game_versions.iter().any(|v| v.as_str() == selected_game_version);
                    let matches_loader = if let Some(selected_loader) = selected_loader {
                        loaders.contains(&selected_loader)
                    } else {
                        true
                    };
                    if matches_game_version && matches_loader {
                        let name = version
                            .version_number
                            .clone()
                            .unwrap_or(version.name.clone().unwrap_or(version.id.clone()));
                        let mut name = SharedString::new(name);

                        match version.version_type {
                            Some(ModrinthVersionType::Beta) => name = format!("{} (Beta)", name).into(),
                            Some(ModrinthVersionType::Alpha) => name = format!("{} (Alpha)", name).into(),
                            _ => {},
                        }

                        Some(ModVersionItem {
                            name,
                            version: version.clone(),
                        })
                    } else {
                        None
                    }
                })
                .collect();

            let mut highest_release = None;
            let mut highest_beta = None;
            let mut highest_alpha = None;

            for (index, version) in mod_versions.iter().enumerate() {
                match version.version.version_type {
                    Some(ModrinthVersionType::Release) => {
                        highest_release = Some(index);
                        break;
                    },
                    Some(ModrinthVersionType::Beta) => {
                        if highest_beta.is_none() {
                            highest_beta = Some(index);
                        }
                    },
                    Some(ModrinthVersionType::Alpha) => {
                        if highest_alpha.is_none() {
                            highest_alpha = Some(index);
                        }
                    },
                    _ => {},
                }
            }

            let highest = highest_release.or(highest_beta).or(highest_alpha);

            let default_index = self
                .selected_version_id
                .as_ref()
                .and_then(|id| mod_versions.iter().position(|item| item.version.id.as_ref() == id.as_ref()))
                .or(highest);

            self.mod_version_select_state = Some(cx.new(|cx| {
                let mut select_state =
                    SelectState::new(SearchableVec::new(mod_versions), None, window, cx).searchable(true);
                if let Some(index) = default_index {
                    select_state.set_selected_index(Some(IndexPath::default().row(index)), window, cx);
                }
                select_state
            }));
        }

        let selected_mod_version = self
            .mod_version_select_state
            .as_ref()
            .and_then(|state| state.read(cx).selected_value())
            .cloned();

        let mod_version_prefix = match self.project_type {
            ModrinthProjectType::Mod => "Mod Version: ",
            ModrinthProjectType::Modpack => "Modpack version: ",
            ModrinthProjectType::Resourcepack => "Pack version: ",
            ModrinthProjectType::Shader => "Shader version: ",
            ModrinthProjectType::Datapack => "Datapack version: ",
            ModrinthProjectType::Other => "File version: ",
        };

        let required_dependencies = selected_mod_version
            .as_ref()
            .and_then(|version| {
                version.dependencies.as_ref().map(|deps| {
                    deps.iter()
                        .filter(|dep| {
                            dep.project_id.is_some() && dep.dependency_type == ModrinthDependencyType::Required
                        })
                        .cloned()
                        .collect::<Arc<[_]>>()
                })
            })
            .unwrap_or_default();

        let content = v_flex()
            .gap_2()
            .child(
                Select::new(self.minecraft_version_select_state.as_ref().unwrap())
                    .disabled(self.fixed_minecraft_version.is_some())
                    .title_prefix("Game Version: "),
            )
            .child(
                Select::new(self.loader_select_state.as_ref().unwrap())
                    .disabled(self.fixed_loader.is_some() || self.skip_loader_check_for_mod_version)
                    .title_prefix("Loader: "),
            )
            .when_some(self.mod_version_select_state.as_ref(), |modal, mod_versions| {
                modal
                    .child(Select::new(mod_versions).title_prefix(mod_version_prefix))
                    .when(!required_dependencies.is_empty(), |modal| {
                        modal.child(
                            Checkbox::new("install_deps")
                                .checked(self.install_dependencies)
                                .label(if required_dependencies.len() == 1 {
                                    SharedString::new_static("Install 1 dependency")
                                } else {
                                    SharedString::new(format!("Install {} dependencies", required_dependencies.len()))
                                })
                                .on_click(cx.listener(|dialog, value, _, _| {
                                    dialog.install_dependencies = *value;
                                })),
                        )
                    })
                    .child(Button::new("install").success().label("Install").on_click(cx.listener(
                        move |this, _, window, cx| {
                            let Some(selected_mod_version) = selected_mod_version.as_ref() else {
                                window.push_notification((NotificationType::Error, ts!("instance.content.install.no_mod_version_selected")), cx);
                                return;
                            };

                            let install_file = selected_mod_version
                                .files
                                .iter()
                                .find(|file| file.primary)
                                .unwrap_or(selected_mod_version.files.first().unwrap());

                            let path = if this.project_type != ModrinthProjectType::Datapack {
                                Some(match this.project_type {
                                    ModrinthProjectType::Mod => RelativePath::new("mods").join(&*install_file.filename),
                                    ModrinthProjectType::Modpack => RelativePath::new("mods").join(&*install_file.filename),
                                    ModrinthProjectType::Resourcepack => {
                                        RelativePath::new("resourcepacks").join(&*install_file.filename)
                                    },
                                    ModrinthProjectType::Shader => {
                                        RelativePath::new("shaderpacks").join(&*install_file.filename)
                                    },
                                    ModrinthProjectType::Datapack => unreachable!(),
                                    ModrinthProjectType::Other => {
                                        window.push_notification(
                                            (NotificationType::Error, "Unable to install 'other' project type"),
                                            cx,
                                        );
                                        return;
                                    },
                                })
                            } else {
                                None
                            };

                            if this.project_type == ModrinthProjectType::Datapack {
                                let target = this.target.clone().unwrap();
                                match &target {
                                    InstallTarget::NewInstance { .. } | InstallTarget::Library => {
                                        perform_datapack_install(
                                            "World".to_string(),
                                            install_file,
                                            this.target.clone().unwrap(),
                                            this.loader_override,
                                            &selected_loader,
                                            &selected_minecraft_version,
                                            this.install_dependencies,
                                            &required_dependencies,
                                            this.project_id.clone(),
                                            this.name.as_str(),
                                            &this.data,
                                            window,
                                            cx,
                                        );
                                        return;
                                    },
                                    InstallTarget::Instance(instance_id) => {
                                        let Some(entry) = this.data.instances.read(cx).entries.get(instance_id) else {
                                            window.push_notification((NotificationType::Error, ts!("instance.unable_to_find")), cx);
                                            return;
                                        };
                                        let instance = entry.read(cx);
                                        let config_key = instance.dot_minecraft_folder.to_string_lossy().to_string();
                                        let worlds: Vec<InstanceWorldSummary> = instance.worlds.read(cx).to_vec();
                                        let world_folders: Vec<String> = worlds.iter()
                                            .filter_map(|w| w.level_path.file_name().map(|n| n.to_string_lossy().into_owned()))
                                            .collect();
                                        // Always show world/prime modal for datapacks so user can choose
                                        this.data.backend_handle.send(MessageToBackend::RequestLoadWorlds {
                                            id: *instance_id,
                                        });
                                        let install_file = Arc::new(install_file.clone());
                                        let target = Arc::new(this.target.clone().unwrap());
                                        let loader_override = this.loader_override;
                                        let install_dependencies = this.install_dependencies;
                                        let project_id = Arc::new(this.project_id.clone());
                                        let name = Arc::new(this.name.to_string());
                                        let data = Arc::new(this.data.clone());
                                        let config_key = Arc::new(config_key.clone());
                                        let selected_loader = Arc::new(selected_loader.clone());
                                        let selected_minecraft_version = Arc::new(selected_minecraft_version.clone());
                                        let required_deps = Arc::clone(&required_dependencies);
                                        window.close_dialog(cx);
                                        if worlds.is_empty() {
                                            window.open_dialog(cx, move |modal, window, cx| {
                                                let config_key = Arc::clone(&config_key);
                                                let install_file = Arc::clone(&install_file);
                                                let target = Arc::clone(&target);
                                                let selected_loader = Arc::clone(&selected_loader);
                                                let selected_minecraft_version = Arc::clone(&selected_minecraft_version);
                                                let required_deps = Arc::clone(&required_deps);
                                                let project_id = Arc::clone(&project_id);
                                                let name = Arc::clone(&name);
                                                let data = Arc::clone(&data);
                                                modal
                                                    .title(ts!("instance.content.install.datapack.prime.title"))
                                                    .child(
                                                        v_flex()
                                                            .gap_3()
                                                            .child(ts!("instance.content.install.datapack.prime.message"))
                                                            .child(
                                                                h_flex()
                                                                    .gap_2()
                                                                    .child(
                                                                        Button::new("prime_yes")
                                                                            .success()
                                                                            .label(ts!("instance.content.install.datapack.prime.yes"))
                                                                            .on_click(move |_, window, cx| {
                                                                                InterfaceConfig::get_mut(cx)
                                                                                    .datapack_world_by_instance
                                                                                    .insert((*config_key).clone(), "World".to_string());
                                                                                window.close_dialog(cx);
                                                                                perform_datapack_install(
                                                                                    "World".to_string(),
                                                                                    install_file.as_ref(),
                                                                                    (*target).clone(),
                                                                                    loader_override,
                                                                                    selected_loader.as_ref(),
                                                                                    selected_minecraft_version.as_ref(),
                                                                                    install_dependencies,
                                                                                    required_deps.as_ref(),
                                                                                    Arc::clone(&project_id),
                                                                                    name.as_str(),
                                                                                    data.as_ref(),
                                                                                    window,
                                                                                    cx,
                                                                                );
                                                                            }),
                                                                    )
                                                                    .child(
                                                                        Button::new("prime_cancel")
                                                                            .label(ts!("instance.content.install.datapack.prime.cancel"))
                                                                            .on_click(move |_, window, cx| {
                                                                                window.close_dialog(cx);
                                                                            }),
                                                                    ),
                                                            ),
                                                    )
                                            });
                                        } else {
                                            let world_folders: Vec<String> = worlds
                                                .iter()
                                                .map(|w| {
                                                    w.level_path
                                                        .file_name()
                                                        .map(|n| n.to_string_lossy().to_string())
                                                        .unwrap_or_default()
                                                })
                                                .collect();
                                            let state = cx.new(|cx| WorldSelectState {
                                                selected_idx: 0,
                                                worlds: worlds.clone(),
                                            });
                                            let required_deps_world = Arc::clone(&required_deps);
                                            window.open_dialog(cx, move |modal, window, cx| {
                                                let state_entity = state.clone();
                                                cx.update_entity(&state_entity, |state, cx| {
                                                    let world_buttons = state
                                                        .worlds
                                                        .iter()
                                                        .enumerate()
                                                        .map(|(i, w)| {
                                                            let folder_name = w
                                                                .level_path
                                                                .file_name()
                                                                .map(|n| n.to_string_lossy().to_string())
                                                                .unwrap_or_else(|| "World".to_string());
                                                            let icon = if let Some(png_icon) = w.png_icon.as_ref() {
                                                                png_render_cache::render(Arc::clone(png_icon), cx)
                                                            } else {
                                                                gpui::img(ImageSource::Resource(Resource::Embedded("images/default_world.png".into())))
                                                            };
                                                            Button::new(("world_select", i))
                                                                .outline()
                                                                .selected(state.selected_idx == i)
                                                                .w_full()
                                                                .min_h(px(72.0))
                                                                .on_click({
                                                                    let state_entity = state_entity.clone();
                                                                    move |_, _, cx| {
                                                                        cx.update_entity(&state_entity, |s, cx| {
                                                                            s.selected_idx = i;
                                                                            cx.notify();
                                                                        });
                                                                    }
                                                                })
                                                                .child(
                                                                    h_flex()
                                                                        .gap_3()
                                                                        .items_center()
                                                                        .w_full()
                                                                        .child(icon.w(px(64.0)).h(px(64.0)))
                                                                        .child(SharedString::from(folder_name)),
                                                                )
                                                        })
                                                        .collect::<Vec<_>>();
                                                    modal
                                                        .title(ts!("instance.content.install.datapack.select_world.title"))
                                                        .child(
                                                            v_flex()
                                                                .gap_3()
                                                                .child(
                                                                    v_flex()
                                                                        .gap_2()
                                                                        .children(world_buttons),
                                                                )
                                                                .child(
                                                                    h_flex()
                                                                        .gap_2()
                                                                        .child(
                                                                            Button::new("world_install")
                                                                                .success()
                                                                                .label("Install")
                                                                                .on_click({
                                                                                    let config_key = Arc::clone(&config_key);
                                                                                    let world_folders = world_folders.clone();
                                                                                    let install_file = Arc::clone(&install_file);
                                                                                    let target = Arc::clone(&target);
                                                                                    let required_deps = Arc::clone(&required_deps_world);
                                                                                    let selected_loader = Arc::clone(&selected_loader);
                                                                                    let selected_minecraft_version = Arc::clone(&selected_minecraft_version);
                                                                                    let project_id = Arc::clone(&project_id);
                                                                                    let name = Arc::clone(&name);
                                                                                    let data = Arc::clone(&data);
                                                                                    let state_entity = state_entity.clone();
                                                                                    move |_, window, cx| {
                                                                                        let idx = state_entity.read(cx).selected_idx;
                                                                                        let world_folder = world_folders.get(idx).cloned();
                                                                                        if let Some(world) = world_folder {
                                                                                            InterfaceConfig::get_mut(cx)
                                                                                                .datapack_world_by_instance
                                                                                                .insert((*config_key).clone(), world.clone());
                                                                                            window.close_dialog(cx);
                                                                                            perform_datapack_install(
                                                                                                world,
                                                                                                install_file.as_ref(),
                                                                                                (*target).clone(),
                                                                                                loader_override,
                                                                                                selected_loader.as_ref(),
                                                                                                selected_minecraft_version.as_ref(),
                                                                                                install_dependencies,
                                                                                                required_deps.as_ref(),
                                                                                                Arc::clone(&project_id),
                                                                                                name.as_str(),
                                                                                                data.as_ref(),
                                                                                                window,
                                                                                                cx,
                                                                                            );
                                                                                        }
                                                                                    }
                                                                                }),
                                                                        )
                                                                        .child(
                                                                            Button::new("world_prime")
                                                                                .outline()
                                                                                .label(ts!("instance.content.install.datapack.select_world.prime_for_next"))
                                                                                .on_click({
                                                                                    let config_key = Arc::clone(&config_key);
                                                                                    let install_file = Arc::clone(&install_file);
                                                                                    let target = Arc::clone(&target);
                                                                                    let required_deps = Arc::clone(&required_deps_world);
                                                                                    let selected_loader = Arc::clone(&selected_loader);
                                                                                    let selected_minecraft_version = Arc::clone(&selected_minecraft_version);
                                                                                    let project_id = Arc::clone(&project_id);
                                                                                    let name = Arc::clone(&name);
                                                                                    let data = Arc::clone(&data);
                                                                                    move |_, window, cx| {
                                                                                        InterfaceConfig::get_mut(cx)
                                                                                            .datapack_world_by_instance
                                                                                            .insert((*config_key).clone(), "World".to_string());
                                                                                        window.close_dialog(cx);
                                                                                        perform_datapack_install(
                                                                                            "World".to_string(),
                                                                                            install_file.as_ref(),
                                                                                            (*target).clone(),
                                                                                            loader_override,
                                                                                            selected_loader.as_ref(),
                                                                                            selected_minecraft_version.as_ref(),
                                                                                            install_dependencies,
                                                                                            required_deps.as_ref(),
                                                                                            Arc::clone(&project_id),
                                                                                            name.as_str(),
                                                                                            data.as_ref(),
                                                                                            window,
                                                                                            cx,
                                                                                        );
                                                                                    }
                                                                                }),
                                                                        )
                                                                        .child(
                                                                            Button::new("world_cancel")
                                                                                .label(ts!("instance.content.install.datapack.prime.cancel"))
                                                                                .on_click(move |_, window, cx| {
                                                                                    window.close_dialog(cx);
                                                                                }),
                                                                        ),
                                                                ),
                                                        )
                                                })
                                            });
                                        }
                                        return;
                                    },
                                };
                            } else if let Some(path) = path {
                                let Some(path) = SafePath::from_relative_path(&path) else {
                                    window.push_notification((NotificationType::Error, "Invalid/dangerous filename"), cx);
                                    return;
                                };
                                let mut target = this.target.clone().unwrap();
                                let mut loader_hint = Loader::Unknown;
                                if let Some(override_loader) = this.loader_override {
                                    loader_hint = override_loader;
                                } else if let Some(selected_loader) = &selected_loader {
                                    let modrinth_loader = ModrinthLoader::from_name(selected_loader);
                                    match modrinth_loader {
                                        ModrinthLoader::Fabric => loader_hint = Loader::Fabric,
                                        ModrinthLoader::Forge => loader_hint = Loader::Forge,
                                        ModrinthLoader::NeoForge => loader_hint = Loader::NeoForge,
                                        _ => {},
                                    }
                                }

                                let mut version_hint = None;
                                if let Some(selected_minecraft_version) = &selected_minecraft_version {
                                    version_hint = Some(selected_minecraft_version.as_str().into());
                                }
                                if let InstallTarget::NewInstance { name } = &mut target {
                                    *name = Some(this.name.as_str().into());
                                }
                                let mut files = Vec::new();
                                if this.install_dependencies {
                                    for dep in required_dependencies.iter() {
                                        files.push(ContentInstallFile {
                                            replace_old: None,
                                            path: bridge::install::ContentInstallPath::Automatic,
                                            download: ContentDownload::Modrinth {
                                                project_id: dep.project_id.clone().unwrap(),
                                                version_id: dep.version_id.clone(),
                                                install_dependencies: true,
                                            },
                                            content_source: ContentSource::ModrinthProject {
                                                project: dep.project_id.clone().unwrap(),
                                            },
                                        })
                                    }
                                }
                                files.push(ContentInstallFile {
                                    replace_old: None,
                                    path: bridge::install::ContentInstallPath::Safe(path),
                                    download: ContentDownload::Url {
                                        url: install_file.url.clone(),
                                        sha1: install_file.hashes.sha1.clone(),
                                        size: install_file.size,
                                    },
                                    content_source: ContentSource::ModrinthProject {
                                        project: this.project_id.clone(),
                                    },
                                });
                                let content_install = ContentInstall {
                                    target,
                                    loader_hint,
                                    version_hint,
                                    datapack_world: None,
                                    files: files.into(),
                                };
                                window.close_dialog(cx);
                                root::start_install(content_install, &this.data.backend_handle, window, cx);
                            }
                        },
                    )))
            });

        modal.child(content)
    }
}

#[derive(Clone)]
struct ModVersionItem {
    name: SharedString,
    version: ModrinthProjectVersion,
}

impl SelectItem for ModVersionItem {
    type Value = ModrinthProjectVersion;

    fn title(&self) -> SharedString {
        self.name.clone()
    }

    fn value(&self) -> &Self::Value {
        &self.version
    }
}
