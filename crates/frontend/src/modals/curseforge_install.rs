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
    v_flex,
};
use relative_path::RelativePath;
use rustc_hash::{FxHashMap, FxHashSet};
use schema::{
    content::{ContentInstallReason, ContentSource},
    curseforge::{
        CURSEFORGE_RELATION_TYPE_REQUIRED_DEPENDENCY, CurseforgeClassId, CurseforgeFile, CurseforgeGetModFilesRequest,
        CurseforgeGetModFilesResult, CurseforgeHit, CurseforgeModLoaderType, CurseforgeReleaseType,
    },
    loader::{Loader, bridged_curseforge_dependency},
};
use strum::IntoEnumIterator;
use ustr::Ustr;

use crate::{
    component::instance_dropdown::InstanceDropdown,
    entity::{
        DataEntities,
        instance::InstanceEntry,
        metadata::{AsMetadataResult, FrontendMetadata, FrontendMetadataResult},
    },
    modals::install_compat::{self, InstanceCompat, VersionChoice},
    root,
};

struct VersionMatrixLoaders {
    loaders: EnumSet<CurseforgeModLoaderType>,
    same_loaders_for_all_versions: bool,
}

struct InstallDialog {
    title: SharedString,
    name: SharedString,

    data: DataEntities,
    project_type: CurseforgeClassId,
    project_id: u32,

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
    /// The loader choices offered for an instance, and the CurseForge loader each one is.
    loader_options: Vec<(SharedString, CurseforgeModLoaderType)>,
    single_loader_set: Option<EnumSet<CurseforgeModLoaderType>>,
    install_dependencies: bool,

    mod_version_select_state: Option<Entity<SelectState<SearchableVec<ModVersionItem>>>>,
    mod_versions_retry_task: Task<()>,
}

/// `game_version_hint` is the Minecraft version the user was browsing, which is picked when the
/// instance's own version has nothing.
pub fn open(
    hit: CurseforgeHit,
    install_for: Option<InstanceID>,
    game_version_hint: Option<Ustr>,
    data: &DataEntities,
    window: &mut Window,
    cx: &mut App,
) {
    let name = SharedString::new(hit.name.clone());
    let title: SharedString = t::instance::content::install::title(&hit.name).into();
    let project_type = hit.class_id.map(CurseforgeClassId::from_u32).unwrap_or_default();

    let mut version_matrix: FxHashMap<&'static str, VersionMatrixLoaders> = FxHashMap::default();
    for version in hit.latest_files_indexes.iter() {
        let mod_loader = version
            .mod_loader
            .map(CurseforgeModLoaderType::from_u32)
            .unwrap_or(CurseforgeModLoaderType::Any);

        let loaders = EnumSet::only(mod_loader);

        match version_matrix.entry(version.game_version.as_str()) {
            std::collections::hash_map::Entry::Occupied(mut occupied_entry) => {
                occupied_entry.get_mut().same_loaders_for_all_versions &= occupied_entry.get().loaders == loaders;
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

        // What the instance can load: its own loader's files, and those of any loader it runs
        // through a bridge mod. Plugin files on CurseForge carry no mod loader, so only mods and
        // modpacks can be checked.
        let usable_loaders = usable_curseforge_loaders(&compat);
        let checks_loader = project_type.supports_loader_filter() && instance_loader != Loader::Vanilla;
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
        let force_target_loader = project_type.mod_or_modpack() && instance_loader != Loader::Vanilla;
        let install_dialog = InstallDialog {
            title,
            name: name.into(),
            data: data.clone(),
            project_type,
            project_id: hit.id,
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
            mod_versions_retry_task: Task::ready(()),
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
                    if project_type.supports_loader_filter() {
                        valid_loader = compat.loader == Loader::Vanilla
                            || !loaders.loaders.is_disjoint(usable_curseforge_loaders(&compat));
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
            dropdown.update(cx, |dropdown, cx| dropdown.set_selected_index(Some(IndexPath::default()), window, cx));
            Some(dropdown)
        } else {
            None
        };

        let install_dialog = InstallDialog {
            title,
            name: name.into(),
            data: data.clone(),
            project_type,
            project_id: hit.id,
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
            mod_versions_retry_task: Task::ready(()),
        };
        install_dialog.show(window, cx);
    }
}

/// The CurseForge loaders whose files an instance can load.
fn usable_curseforge_loaders(compat: &InstanceCompat) -> EnumSet<CurseforgeModLoaderType> {
    std::iter::once(compat.loader)
        .chain(compat.bridged)
        .map(|loader| loader.as_curseforge_loader())
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

        let selected_file = self
            .mod_version_select_state
            .as_ref()
            .and_then(|state| state.read(cx).selected_value())
            .cloned();

        let Some(selected_file) = selected_file else {
            return modal.child(content);
        };
        let selected_file = selected_file.file;

        let mut required_dependencies = selected_file
            .dependencies
            .iter()
            .filter(|dep| dep.relation_type == CURSEFORGE_RELATION_TYPE_REQUIRED_DEPENDENCY)
            .cloned()
            .collect::<Vec<_>>();

        // A dependency that can't load through the instance's bridge is swapped for the one that
        // stands in for it there, such as Forgified Fabric API for Fabric API under Sinytra Connector
        if let Some(instance) = self.instance {
            for dep in &mut required_dependencies {
                if let Some(replacement) = bridged_curseforge_dependency(dep.mod_id, instance.loader, instance.bridged) {
                    dep.mod_id = replacement;
                }
            }
        }

        // Ignore projects that are already installed
        if !required_dependencies.is_empty()
            && let InstallTarget::Instance(instance_id) = install_target
            && let Some(instance) = self.data.instances.read(cx).entries.get(&instance_id)
        {
            let mut existing_projects = FxHashSet::default();

            for existing_content in instance.read(cx).content.values() {
                if let Some(existing_content) = existing_content.read(cx) {
                    for summary in existing_content.iter() {
                        let ContentSource::CurseforgeProject { project_id: project } = &summary.content_source else {
                            continue;
                        };
                        existing_projects.insert(project.clone());
                    }
                }
            }

            required_dependencies.retain(|dep| !existing_projects.contains(&dep.mod_id));
        }

        content = content
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
                    let path = match this.project_type {
                        CurseforgeClassId::Mod => RelativePath::new("mods").join(&*selected_file.file_name),
                        CurseforgeClassId::Modpack => RelativePath::new("mods").join(&*selected_file.file_name),
                        CurseforgeClassId::Resourcepack => {
                            RelativePath::new("resourcepacks").join(&*selected_file.file_name)
                        },
                        CurseforgeClassId::Shader => RelativePath::new("shaderpacks").join(&*selected_file.file_name),
                        CurseforgeClassId::BukkitPlugin => RelativePath::new("plugins").join(&*selected_file.file_name),
                        _ => {
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
                            .find(|loader| single_loader_set.contains(loader.as_curseforge_loader()))
                            .unwrap_or(Loader::Vanilla)
                    } else {
                        CurseforgeModLoaderType::from_name(&selected_loader_string)
                            .as_pandora()
                            .unwrap_or(Loader::Vanilla)
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
                                download: ContentDownload::Curseforge {
                                    project_id: dep.mod_id,
                                    install_dependencies: true,
                                },
                                content_source: ContentSource::CurseforgeProject { project_id: dep.mod_id },
                                reason: ContentInstallReason::Dependency,
                            })
                        }
                    }

                    let sha1 = selected_file.hashes.iter().find(|hash| hash.algo == 1).map(|hash| hash.value.clone());

                    let Some(sha1) = sha1 else {
                        window.push_notification(
                            (NotificationType::Error, t::instance::content::install::missing_sha1_hash()),
                            cx,
                        );
                        return;
                    };

                    let mut hash = [0u8; 20];
                    let Ok(_) = hex::decode_to_slice(&*sha1, &mut hash) else {
                        let warning = t::instance::content::install::file_invalid_sha1(&selected_file.file_name, &sha1);
                        window.push_notification((NotificationType::Error, SharedString::new(warning)), cx);
                        return;
                    };

                    let Some(download_url) = selected_file.download_url.clone() else {
                        window.push_notification(
                            (NotificationType::Error, t::instance::content::install::no_third_party_downloads()),
                            cx,
                        );
                        return;
                    };

                    files.push(ContentInstallFile {
                        replace_old: None,
                        path: bridge::install::ContentInstallPath::Safe(path),
                        download: ContentDownload::Url {
                            url: download_url,
                            sha1: hash,
                            size: selected_file.file_length as usize,
                        },
                        content_source: ContentSource::CurseforgeProject {
                            project_id: this.project_id,
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
            CurseforgeClassId::Mod => t::instance::content::install::new_instance_with::mod_(),
            CurseforgeClassId::Modpack => t::instance::content::install::new_instance_with::modpack(),
            CurseforgeClassId::Resourcepack => t::instance::content::install::new_instance_with::resourcepack(),
            CurseforgeClassId::Shader => t::instance::content::install::new_instance_with::shader(),
            _ => t::instance::content::install::new_instance_with::file(),
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
                                    this.force_target_loader = this.project_type.mod_or_modpack()
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
                // version has files for
                let available = self
                    .version_matrix
                    .get(selected_minecraft_version.as_str())
                    .map(|loaders| loaders.loaders)
                    .unwrap_or_default();
                let own = loader.as_curseforge_loader();
                let mut options = vec![(SharedString::new_static(own.pretty_name()), own)];
                if let Some(instance) = self.instance {
                    for bridged in instance.bridged {
                        options.push((instance.bridged_label(bridged), bridged.as_curseforge_loader()));
                    }
                }
                let mut usable: Vec<_> =
                    options.iter().filter(|(_, loader)| available.contains(*loader)).cloned().collect();
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
                        .map(CurseforgeModLoaderType::pretty_name)
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
        if self.mod_version_select_state.is_none() {
            let selected_game_version: Ustr = selected_minecraft_version.as_str().into();

            let mod_loader_type = if self.single_loader_set.is_some() {
                None
            } else if let Some((_, loader)) =
                self.loader_options.iter().find(|(label, _)| label == selected_loader_string)
            {
                Some(*loader as u32)
            } else {
                Some(CurseforgeModLoaderType::from_name(selected_loader_string.as_str()) as u32)
            };

            let request = FrontendMetadata::request(
                &self.data.metadata,
                MetadataRequest::CurseforgeGetModFiles(CurseforgeGetModFilesRequest {
                    mod_id: self.project_id,
                    game_version: Some(selected_game_version),
                    mod_loader_type,
                    release_types: None,
                    page_size: None,
                }),
                cx,
            );

            let result: FrontendMetadataResult<CurseforgeGetModFilesResult> = request.read(cx).result();

            match result {
                FrontendMetadataResult::Loading => {
                    return t::instance::content::install::loading_files().into_any_element();
                },
                FrontendMetadataResult::Loaded(result) => {
                    let mod_versions: Vec<ModVersionItem> = result
                        .data
                        .iter()
                        .map(|file| ModVersionItem {
                            name: file.file_name.clone().into(),
                            file: file.clone(),
                        })
                        .collect();

                    let mut highest_release = None;
                    let mut highest_beta = None;
                    let mut highest_alpha = None;

                    for (index, version) in mod_versions.iter().enumerate() {
                        match CurseforgeReleaseType::from_u32(version.file.release_type) {
                            CurseforgeReleaseType::Release => {
                                highest_release = Some(index);
                                break;
                            },
                            CurseforgeReleaseType::Beta => {
                                if highest_beta.is_none() {
                                    highest_beta = Some(index);
                                }
                            },
                            _ => {
                                if highest_alpha.is_none() {
                                    highest_alpha = Some(index);
                                }
                            },
                        }
                    }

                    let highest = highest_release.or(highest_beta).or(highest_alpha);

                    self.mod_versions_retry_task = Task::ready(());
                    self.mod_version_select_state = Some(cx.new(|cx| {
                        let mut select_state =
                            SelectState::new(SearchableVec::new(mod_versions), None, window, cx).searchable(true);
                        if let Some(index) = highest {
                            select_state.set_selected_index(Some(IndexPath::default().row(index)), window, cx);
                        }
                        select_state
                    }));
                },
                FrontendMetadataResult::Error(shared_string, alive) => {
                    if let Some(alive) = alive {
                        self.mod_versions_retry_task = cx.spawn(async move |page, cx| {
                            alive.await_notification().await;
                            _ = page.update(cx, |page, cx| {
                                page.mod_version_select_state = None;
                                page.mod_versions_retry_task = Task::ready(());
                                cx.notify();
                            });
                        });
                    }

                    return t::instance::content::install::error_loading_files(&shared_string).into_any_element();
                },
            }
        }

        Select::new(self.mod_version_select_state.as_ref().unwrap())
            .title_prefix(t::instance::content::filename_prefix())
            .search_placeholder(t::common::search())
            .into_any_element()
    }
}

#[derive(Clone)]
struct ModVersionItem {
    name: SharedString,
    file: CurseforgeFile,
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
