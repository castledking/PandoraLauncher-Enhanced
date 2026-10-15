use std::sync::Arc;

use bridge::{
    install::{ContentDownload, ContentInstall, ContentInstallFile, InstallTarget},
    instance::InstanceID,
    meta::MetadataRequest,
    safe_path::SafePath,
};
use enumset::EnumSet;
use gpui::{prelude::*, *};
use gpui_component::{
    IndexPath, WindowExt,
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
use rustc_hash::{FxHashMap, FxHashSet};
use schema::{
    content::{ContentInstallReason, ContentSource},
    loader::{Loader, bridged_modrinth_dependency},
    modrinth::{
        ModrinthDependencyType, ModrinthLoader, ModrinthProjectType, ModrinthProjectVersion,
        ModrinthProjectVersionsRequest, ModrinthProjectVersionsResult, ModrinthVersionStatus, ModrinthVersionType,
    },
};
use strum::IntoEnumIterator;
use ustr::Ustr;

use crate::{
    component::{error_alert::ErrorAlert, instance_dropdown::InstanceDropdown},
    entity::{
        DataEntities,
        instance::InstanceEntry,
        metadata::{AsMetadataResult, FrontendMetadata, FrontendMetadataResult},
    },
    modals::install_compat::{self, InstanceCompat, VersionChoice},
    root,
};

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
    /// When the instance's own version has nothing, the other versions offered instead.
    minecraft_version_choices: Option<Vec<&'static str>>,
    preferred_minecraft_version: Option<&'static str>,
    minecraft_version_select_state: Option<Entity<SelectState<SearchableVec<SharedString>>>>,
    /// What the existing instance being installed into can take.
    instance: Option<InstanceCompat>,

    force_target_loader: bool,
    target_loader: Option<Loader>,
    loader_select_state: Option<Entity<SelectState<Vec<SharedString>>>>,
    /// The loader choices offered for an instance, and the Modrinth loaders each one matches.
    loader_options: Vec<(SharedString, EnumSet<ModrinthLoader>)>,
    single_loader_set: Option<EnumSet<ModrinthLoader>>,
    install_dependencies: bool,

    mod_version_select_state: Option<Entity<SelectState<SearchableVec<ModVersionItem>>>>,
}

/// `game_version_hint` is the Minecraft version the user was browsing, which is picked when the
/// instance's own version has nothing.
pub fn open(
    name: SharedString,
    project_id: Arc<str>,
    project_type: ModrinthProjectType,
    install_for: Option<InstanceID>,
    game_version_hint: Option<Ustr>,
    data: &DataEntities,
    window: &mut Window,
    cx: &mut App,
) {
    let project_versions = FrontendMetadata::request(
        &data.metadata,
        MetadataRequest::ModrinthProjectVersions(ModrinthProjectVersionsRequest {
            project_id: project_id.clone(),
            game_versions: None,
            loaders: None,
        }),
        cx,
    );

    let title: SharedString = t::instance::content::install::title(&name).into();

    let result: FrontendMetadataResult<ModrinthProjectVersionsResult> = project_versions.read(cx).result();
    match result {
        FrontendMetadataResult::Loading => {
            let data = data.clone();
            let name = name.clone();
            let _subscription = window.observe(&project_versions, cx, move |_, window, cx| {
                window.close_all_dialogs(cx);
                open(
                    name.clone(),
                    project_id.clone(),
                    project_type,
                    install_for,
                    game_version_hint,
                    &data,
                    window,
                    cx,
                );
            });
            window.open_dialog(cx, move |dialog, _, _| {
                let _ = &_subscription;
                dialog.title(title.clone()).child(
                    h_flex()
                        .gap_2()
                        .child(t::instance::content::load::versions::title())
                        .child(Spinner::new()),
                )
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
                open_error_dialog(title.clone(), t::instance::content::load::versions::not_found().into(), window, cx);
                return;
            }
            if let Some(install_for) = install_for {
                let Some(instance) = data.instances.read(cx).entries.get(&install_for) else {
                    open_error_dialog(title.clone(), t::instance::unable_to_find().into(), window, cx);
                    return;
                };

                let instance = instance.read(cx);
                let compat = InstanceCompat::of(instance, cx);
                let minecraft_version = compat.minecraft_version;
                let instance_loader = compat.loader;

                // What the instance can load: its own loader's builds, and those of any loader it
                // runs through a bridge mod
                let usable_loaders = usable_modrinth_loaders(&compat);
                let checks_loader = project_type.has_loader_and_version() && instance_loader != Loader::Vanilla;
                let usable_versions = version_matrix
                    .iter()
                    .filter(|(_, loaders)| !checks_loader || !loaders.loaders.is_disjoint(usable_loaders))
                    .map(|(version, _)| *version)
                    .collect::<Vec<_>>();

                let (fixed_minecraft_version, minecraft_version_choices, preferred_minecraft_version) =
                    match VersionChoice::new(minecraft_version, usable_versions, game_version_hint) {
                        VersionChoice::Exact => (Some(minecraft_version), None, None),
                        VersionChoice::Other { choices, preferred } => (None, Some(choices), preferred),
                        VersionChoice::Nothing => {
                            let error_message = if version_matrix.contains_key(minecraft_version) {
                                t::instance::content::load::versions::not_found_for_loader(
                                    instance_loader.pretty_name(),
                                    minecraft_version,
                                )
                            } else {
                                t::instance::content::load::versions::not_found_for(minecraft_version)
                            };
                            open_error_dialog(title.clone(), error_message.into(), window, cx);
                            return;
                        },
                    };

                let title = title.clone();
                let instance_id = instance.id;
                let force_target_loader = checks_loader;
                let install_dialog = InstallDialog {
                    title,
                    name: name.into(),
                    project_versions: valid_project_versions.into(),
                    data: data.clone(),
                    project_type,
                    project_id,
                    version_matrix,
                    instances: None,
                    unsupported_instances: 0,
                    target: Some(InstallTarget::Instance(instance_id)),
                    fixed_minecraft_version,
                    minecraft_version_choices,
                    preferred_minecraft_version,
                    minecraft_version_select_state: None,
                    instance: Some(compat),
                    force_target_loader,
                    target_loader: Some(instance_loader),
                    loader_select_state: None,
                    loader_options: Vec::new(),
                    last_selected_minecraft_version: None,
                    single_loader_set: None,
                    install_dependencies: true,
                    mod_version_select_state: None,
                    last_selected_loader: None,
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
                        let compat = InstanceCompat::of(instance, cx);

                        if let Some(loaders) = version_matrix.get(compat.minecraft_version) {
                            let mut valid_loader = true;
                            if project_type.has_loader_and_version() {
                                valid_loader = compat.loader == Loader::Vanilla
                                    || !loaders.loaders.is_disjoint(usable_modrinth_loaders(&compat));
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
                    data: data.clone(),
                    project_type,
                    project_id,
                    version_matrix,
                    instances,
                    unsupported_instances,
                    target: None,
                    fixed_minecraft_version: None,
                    minecraft_version_choices: None,
                    preferred_minecraft_version: None,
                    minecraft_version_select_state: None,
                    instance: None,
                    force_target_loader: false,
                    target_loader: None,
                    loader_select_state: None,
                    loader_options: Vec::new(),
                    last_selected_minecraft_version: None,
                    single_loader_set: None,
                    install_dependencies: true,
                    mod_version_select_state: None,
                    last_selected_loader: None,
                };
                install_dialog.show(window, cx);
            }
        },
        FrontendMetadataResult::Error(error, alive) => {
            let _task = if let Some(alive) = alive {
                let data = data.clone();
                let name = name.clone();
                window.spawn(cx, async move |cx| {
                    alive.await_notification().await;
                    _ = cx.update(|window, cx| {
                        window.close_all_dialogs(cx);
                        open(
                            name.clone(),
                            project_id.clone(),
                            project_type,
                            install_for,
                            game_version_hint,
                            &data,
                            window,
                            cx,
                        );
                    });
                })
            } else {
                Task::ready(())
            };

            window.open_dialog(cx, move |modal, _, _| {
                let _ = &_task;
                modal.title(title.clone()).child(ErrorAlert::new(
                    t::instance::content::requesting_from_error("Modrinth").into(),
                    error.clone(),
                ))
            });
        },
    }
}

/// The Modrinth loaders whose builds an instance can load.
fn usable_modrinth_loaders(compat: &InstanceCompat) -> EnumSet<ModrinthLoader> {
    compat
        .loader
        .compatible_modrinth_loaders()
        .iter()
        .copied()
        .chain(compat.bridged.iter().map(Loader::as_modrinth_loader))
        .collect()
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

        let Some(install_target) = self.target.clone() else {
            return modal.child(self.render_select_target(window, cx));
        };

        let mut content = v_flex().gap_2();

        content = content.child(self.render_select_minecraft_version(window, cx));

        let selected_minecraft_version = self
            .minecraft_version_select_state
            .as_ref()
            .and_then(|v| v.read(cx).selected_value())
            .cloned();

        if self.last_selected_minecraft_version != selected_minecraft_version {
            self.last_selected_minecraft_version = selected_minecraft_version.clone();
            self.loader_select_state = None;
            self.mod_version_select_state = None;
        }

        let Some(selected_minecraft_version) = selected_minecraft_version else {
            return modal.child(content);
        };

        if let Some(instance) = self.instance
            && selected_minecraft_version.as_str() != instance.minecraft_version
        {
            content = content.child(install_compat::other_version_warning(
                &selected_minecraft_version,
                instance.minecraft_version,
                cx,
            ));
        }

        content = content.child(self.render_select_loader(&selected_minecraft_version, window, cx));

        let selected_loader_string =
            self.loader_select_state.as_ref().and_then(|v| v.read(cx).selected_value()).cloned();

        if self.last_selected_loader != selected_loader_string {
            self.last_selected_loader = selected_loader_string.clone();
            self.mod_version_select_state = None;
        }

        let Some(selected_loader_string) = selected_loader_string else {
            return modal.child(content);
        };

        content = content.child(self.render_select_mod_version(
            &selected_minecraft_version,
            &selected_loader_string,
            window,
            cx,
        ));

        let selected_mod_version = self
            .mod_version_select_state
            .as_ref()
            .and_then(|state| state.read(cx).selected_value())
            .cloned();

        let Some(selected_mod_version) = selected_mod_version else {
            return modal.child(content);
        };
        let selected_mod_version = selected_mod_version.version;

        let required_dependencies = selected_mod_version
            .dependencies
            .as_ref()
            .map(|deps| {
                let mut required = deps
                    .iter()
                    .filter(|dep| dep.project_id.is_some() && dep.dependency_type == ModrinthDependencyType::Required)
                    .cloned()
                    .collect::<Vec<_>>();

                // A dependency that can't load through the instance's bridge is swapped for the one
                // that stands in for it there, such as Forgified Fabric API for Fabric API under
                // Sinytra Connector
                if let Some(instance) = self.instance {
                    for dep in &mut required {
                        let replacement = dep
                            .project_id
                            .as_deref()
                            .and_then(|project_id| bridged_modrinth_dependency(project_id, instance.loader, instance.bridged));
                        if let Some(replacement) = replacement {
                            dep.project_id = Some(replacement.into());
                            dep.version_id = None;
                        }
                    }
                }

                // Ignore projects that are already installed
                if !required.is_empty()
                    && let InstallTarget::Instance(instance_id) = &install_target
                    && let Some(instance) = self.data.instances.read(cx).entries.get(instance_id)
                {
                    let mut existing_projects = FxHashSet::default();

                    for existing_content in instance.read(cx).content.values() {
                        if let Some(existing_content) = existing_content.read(cx) {
                            for summary in existing_content.iter() {
                                let ContentSource::ModrinthProject { project_id } = &summary.content_source else {
                                    continue;
                                };
                                existing_projects.insert(project_id.clone());
                            }
                        }
                    }

                    required.retain(|dep| !existing_projects.contains(dep.project_id.as_ref().unwrap()));
                }

                required
            })
            .unwrap_or_default();

        let content = content
            .when(!required_dependencies.is_empty(), |modal| {
                modal.child(
                    Checkbox::new("install_deps")
                        .checked(self.install_dependencies)
                        .label(if required_dependencies.len() == 1 {
                            SharedString::new_static(t::instance::content::install::install_dependency())
                        } else {
                            t::instance::content::install::install_dependencies(required_dependencies.len()).into()
                        })
                        .on_click(cx.listener(|dialog, value, _, _| {
                            dialog.install_dependencies = *value;
                        })),
                )
            })
            .child(Button::new("install").success().label(t::instance::content::install::label()).on_click(
                cx.listener(move |this, _, window, cx| {
                    let install_file = selected_mod_version
                        .files
                        .iter()
                        .find(|file| file.primary)
                        .unwrap_or(selected_mod_version.files.first().unwrap());

                    // Modrinth labels plugins as mods in search results, so the folder is decided by
                    // what the jar is going into: on a Paper server it's a plugin, on a modded
                    // client the same project's jar is a mod.
                    let onto_plugin_server = this.target_loader == Some(Loader::Paper);
                    let path = match this.project_type {
                        ModrinthProjectType::Mod | ModrinthProjectType::Plugin if onto_plugin_server => {
                            RelativePath::new("plugins").join(&*install_file.filename)
                        },
                        ModrinthProjectType::Mod => RelativePath::new("mods").join(&*install_file.filename),
                        ModrinthProjectType::Plugin => RelativePath::new("mods").join(&*install_file.filename),
                        ModrinthProjectType::Modpack => RelativePath::new("mods").join(&*install_file.filename),
                        ModrinthProjectType::Resourcepack => {
                            RelativePath::new("resourcepacks").join(&*install_file.filename)
                        },
                        ModrinthProjectType::Shader => RelativePath::new("shaderpacks").join(&*install_file.filename),
                        ModrinthProjectType::Datapack => {
                            RelativePath::new("saves").join("World").join("datapacks").join(&*install_file.filename)
                        },
                        ModrinthProjectType::Other => {
                            window.push_notification(
                                (NotificationType::Error, t::instance::content::install::unable_install_other()),
                                cx,
                            );
                            return;
                        },
                    };

                    let Some(path) = SafePath::from_relative_path(&path) else {
                        window.push_notification(
                            (NotificationType::Error, t::instance::content::install::invalid_filename()),
                            cx,
                        );
                        return;
                    };

                    let loader = if let Some(loader) = this.target_loader {
                        loader
                    } else if let Some(single_loader_set) = this.single_loader_set {
                        Loader::iter()
                            .filter(|loader| *loader != Loader::Vanilla && loader.is_client_loader())
                            .find(|loader| single_loader_set.contains(loader.as_modrinth_loader()))
                            .unwrap_or(Loader::Vanilla)
                    } else {
                        ModrinthLoader::from_name(&selected_loader_string).as_pandora().unwrap_or(Loader::Vanilla)
                    };

                    let mut target = install_target.clone();
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
                                    project_id: dep.project_id.clone().unwrap(),
                                },
                                reason: ContentInstallReason::Dependency,
                            })
                        }
                    }

                    let mut hash = [0u8; 20];
                    let Ok(_) = hex::decode_to_slice(&*install_file.hashes.sha1, &mut hash) else {
                        let warning = t::instance::content::install::file_invalid_sha1(
                            &install_file.filename,
                            &install_file.hashes.sha1,
                        );
                        window.push_notification((NotificationType::Error, SharedString::new(warning)), cx);
                        return;
                    };

                    files.push(ContentInstallFile {
                        replace_old: None,
                        path: bridge::install::ContentInstallPath::Safe(path),
                        download: ContentDownload::Url {
                            url: install_file.url.clone(),
                            sha1: hash,
                            size: install_file.size,
                        },
                        content_source: ContentSource::ModrinthProject {
                            project_id: this.project_id.clone(),
                        },
                        reason: ContentInstallReason::Standalone,
                    });

                    // An existing instance stays on its own version, which its dependencies are looked
                    // up for, even when this file was made for another
                    let minecraft_version = match this.instance {
                        Some(instance) => instance.minecraft_version,
                        None => selected_minecraft_version.as_str(),
                    };
                    let content_install = ContentInstall {
                        target,
                        loader,
                        minecraft_version: minecraft_version.into(),
                        files: files.into(),
                    };

                    window.close_dialog(cx);
                    root::start_install(content_install, &this.data.backend_handle, window, cx);
                }),
            ));

        modal.child(content)
    }

    fn render_select_target(&self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let create_instance_label = match self.project_type {
            ModrinthProjectType::Mod => t::instance::content::install::new_instance_with::mod_(),
            ModrinthProjectType::Plugin => t::instance::content::install::new_instance_with::plugin(),
            ModrinthProjectType::Modpack => t::instance::content::install::new_instance_with::modpack(),
            ModrinthProjectType::Resourcepack => t::instance::content::install::new_instance_with::resourcepack(),
            ModrinthProjectType::Shader => t::instance::content::install::new_instance_with::shader(),
            ModrinthProjectType::Datapack => "Create new instance with this datapack",
            ModrinthProjectType::Other => t::instance::content::install::new_instance_with::file(),
        };

        v_flex()
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
                                Select::new(instances)
                                    .placeholder(t::instance::none_selected())
                                    .title_prefix(format!("{}: ", t::instance::label()))
                                    .search_placeholder(t::common::search()),
                            )
                            .when(self.unsupported_instances > 0, |content| {
                                content.child(t::instance::incompatible(self.unsupported_instances))
                            }),
                    )
                    .when_some(selected_instance, |dialog, instance| {
                        dialog.child(
                            Button::new("instance")
                                .success()
                                .h_full()
                                .label(t::instance::content::install::add_to_instance())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.target = Some(InstallTarget::Instance(instance.id));
                                    this.fixed_minecraft_version =
                                        Some(instance.configuration.minecraft_version.as_str());
                                    this.force_target_loader = this.project_type.has_loader_and_version()
                                        && instance.configuration.loader != Loader::Vanilla;
                                    this.target_loader = Some(instance.configuration.loader);
                                    this.instance = Some(InstanceCompat::of(&instance, cx));
                                })),
                        )
                    });

                content.child(button_and_dropdown).child(format!("— {} —", t::common::or_upper()))
            })
            .child(Button::new("create").success().label(create_instance_label).on_click(cx.listener(
                |this, _, _, _| {
                    this.target = Some(InstallTarget::NewInstance { name: None });
                    this.fixed_minecraft_version = None;
                    this.force_target_loader = false;
                    this.target_loader = None;
                    this.instance = None;
                },
            )))
            .into_any_element()
    }

    fn render_select_minecraft_version(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let select_state = self.minecraft_version_select_state.get_or_insert_with(|| {
            if let Some(minecraft_version) = self.fixed_minecraft_version.clone() {
                cx.new(|cx| {
                    let mut select_state = SelectState::new(
                        SearchableVec::new(vec![SharedString::new_static(minecraft_version)]),
                        None,
                        window,
                        cx,
                    )
                    .searchable(true);
                    select_state.set_selected_index(Some(IndexPath::default()), window, cx);
                    select_state
                })
            } else {
                let mut keys: Vec<SharedString> = match &self.minecraft_version_choices {
                    Some(choices) => choices.iter().copied().map(SharedString::new_static).collect(),
                    None => self.version_matrix.keys().copied().map(SharedString::new_static).collect(),
                };
                install_compat::sort_game_versions(&mut keys);
                let preferred = self
                    .preferred_minecraft_version
                    .and_then(|preferred| keys.iter().position(|key| key.as_str() == preferred))
                    .unwrap_or(0);
                cx.new(|cx| {
                    let mut select_state =
                        SelectState::new(SearchableVec::new(keys), None, window, cx).searchable(true);
                    select_state.set_selected_index(Some(IndexPath::default().row(preferred)), window, cx);
                    select_state
                })
            }
        });

        Select::new(select_state)
            .disabled(self.fixed_minecraft_version.is_some())
            .title_prefix(format!("{}: ", t::instance::game_version()))
            .search_placeholder(t::common::search())
            .into_any_element()
    }

    fn render_select_loader(
        &mut self,
        selected_minecraft_version: &SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let loader_select_state = self.loader_select_state.get_or_insert_with(|| {
            self.single_loader_set = None;

            self.loader_options.clear();

            if let Some(loader) = self.target_loader
                && self.force_target_loader
            {
                // The instance's own loader, then any it runs through a bridge mod, of those this
                // version has builds for
                let available = self
                    .version_matrix
                    .get(selected_minecraft_version.as_str())
                    .map(|loaders| loaders.loaders)
                    .unwrap_or_default();
                let own: EnumSet<ModrinthLoader> = loader.compatible_modrinth_loaders().iter().copied().collect();
                let mut options = vec![(SharedString::new_static(loader.as_modrinth_loader().pretty_name()), own)];
                if let Some(instance) = self.instance {
                    for bridged in instance.bridged {
                        options.push((instance.bridged_label(bridged), EnumSet::only(bridged.as_modrinth_loader())));
                    }
                }
                let mut usable: Vec<_> =
                    options.iter().filter(|(_, matches)| !matches.is_disjoint(available)).cloned().collect();
                if usable.is_empty() {
                    usable.push(options.swap_remove(0));
                }
                self.loader_options = usable;

                let labels = self.loader_options.iter().map(|(label, _)| label.clone()).collect::<Vec<_>>();
                cx.new(|cx| {
                    let mut select_state = SelectState::new(labels, None, window, cx);
                    select_state.set_selected_index(Some(IndexPath::default()), window, cx);
                    select_state
                })
            } else if let Some(loaders) = self.version_matrix.get(selected_minecraft_version.as_str()) {
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

                    self.single_loader_set = Some(loaders.loaders);

                    cx.new(|cx| {
                        let mut select_state = SelectState::new(vec![single_loader], None, window, cx);
                        select_state.set_selected_index(Some(IndexPath::default()), window, cx);
                        select_state
                    })
                } else {
                    let keys: Vec<SharedString> = loaders
                        .loaders
                        .iter()
                        .map(ModrinthLoader::pretty_name)
                        .map(SharedString::new_static)
                        .collect();

                    cx.new(|cx| {
                        let mut select_state = SelectState::new(keys, None, window, cx);
                        if let Some(previous) = &self.last_selected_loader {
                            select_state.set_selected_value(previous, window, cx);
                        }
                        if select_state.selected_index(cx).is_none() {
                            select_state.set_selected_index(Some(IndexPath::default()), window, cx);
                        }
                        select_state
                    })
                }
            } else {
                cx.new(|cx| {
                    let mut select_state = SelectState::new(Vec::new(), None, window, cx);
                    select_state.set_selected_index(Some(IndexPath::default()), window, cx);
                    select_state
                })
            }
        });

        // An instance's loader is fixed, unless it runs another's mods through a bridge too
        let fixed_loader = self.force_target_loader && self.loader_options.len() <= 1;
        Select::new(loader_select_state)
            .disabled(fixed_loader || self.single_loader_set.is_some())
            .title_prefix(format!("{}: ", t::instance::loader()))
            .into_any_element()
    }

    fn render_select_mod_version(
        &mut self,
        selected_minecraft_version: &SharedString,
        selected_loader_string: &SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mod_version_select_state = self.mod_version_select_state.get_or_insert_with(|| {
            let selected_game_version = selected_minecraft_version.as_str();

            let selected_loaders = if self.single_loader_set.is_some() {
                None
            } else if let Some((_, matches)) =
                self.loader_options.iter().find(|(label, _)| label == selected_loader_string)
            {
                Some(*matches)
            } else {
                Some(EnumSet::only(ModrinthLoader::from_name(selected_loader_string.as_str())))
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
                    let matches_game_version = game_versions.iter().any(|v| v.as_str() == selected_game_version);
                    let matches_loader = if let Some(selected_loaders) = selected_loaders {
                        loaders.iter().any(|loader| selected_loaders.contains(*loader))
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
                            Some(ModrinthVersionType::Beta) => name = t::modrinth::versions::beta(&name).into(),
                            Some(ModrinthVersionType::Alpha) => name = t::modrinth::versions::alpha(&name).into(),
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

            cx.new(|cx| {
                let mut select_state =
                    SelectState::new(SearchableVec::new(mod_versions), None, window, cx).searchable(true);
                if let Some(index) = highest {
                    select_state.set_selected_index(Some(IndexPath::default().row(index)), window, cx);
                }
                select_state
            })
        });

        let mod_version_prefix = match self.project_type {
            ModrinthProjectType::Mod => format!("{}: ", t::instance::content::version::mod_()),
            ModrinthProjectType::Plugin => format!("{}: ", t::instance::content::version::plugin()),
            ModrinthProjectType::Modpack => format!("{}: ", t::instance::content::version::modpack()),
            ModrinthProjectType::Resourcepack => format!("{}: ", t::instance::content::version::resourcepack()),
            ModrinthProjectType::Shader => format!("{}: ", t::instance::content::version::shader()),
            ModrinthProjectType::Datapack => "Datapack version: ".to_string(),
            ModrinthProjectType::Other => format!("{}: ", t::instance::content::version::file()),
        };

        Select::new(mod_version_select_state)
            .title_prefix(mod_version_prefix)
            .search_placeholder(t::common::search())
            .into_any_element()
    }
}

#[derive(Clone)]
struct ModVersionItem {
    name: SharedString,
    version: ModrinthProjectVersion,
}

impl PartialEq for ModVersionItem {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl SelectItem for ModVersionItem {
    type Value = Self;

    fn title(&self) -> SharedString {
        self.name.clone()
    }

    fn value(&self) -> &Self::Value {
        self
    }
}
