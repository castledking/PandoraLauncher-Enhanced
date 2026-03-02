use std::{cmp::Ordering, sync::Arc};

use bridge::{
    install::{ContentDownload, ContentInstall, ContentInstallFile, InstallTarget},
    instance::{ContentType, InstanceID, InstanceWorldSummary},
    message::MessageToBackend,
    meta::MetadataRequest,
    modal_action::ModalAction,
    safe_path::SafePath,
};
use enumset::EnumSet;
use gpui::{prelude::*, *};
use gpui_component::{
    Selectable, Sizable, WindowExt,
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    dialog::Dialog,
    h_flex,
    notification::{Notification, NotificationType},
    spinner::Spinner,
    v_flex,
};
use super::modrinth_install;
use relative_path::RelativePath;
use rustc_hash::FxHashMap;
use schema::{
    content::ContentSource,
    instance::InstanceConfiguration,
    loader::Loader,
    modrinth::{
        ModrinthDependency, ModrinthDependencyType, ModrinthFile, ModrinthLoader, ModrinthProjectType, ModrinthProjectVersion,
        ModrinthProjectVersionsRequest, ModrinthProjectVersionsResult, ModrinthVersionStatus, ModrinthVersionType,
    },
};
use uuid::Uuid;

use crate::{
    component::{error_alert::ErrorAlert, instance_dropdown::InstanceDropdown},
    interface_config::InterfaceConfig,
    png_render_cache,
    ts,
    entity::{
        DataEntities,
        instance::InstanceEntry,
        metadata::{AsMetadataResult, FrontendMetadata, FrontendMetadataResult, FrontendMetadataState},
    },
    root,
};

struct WorldSelectState {
    selected_idx: usize,
    worlds: Vec<InstanceWorldSummary>,
}

// struct VersionMatrixLoaders {
//     loaders: EnumSet<ModrinthLoader>,
//     same_loaders_for_all_versions: bool,
// }

// struct InstallNotification {
//     title: SharedString,
//     name: SharedString,

//     project_versions: Arc<[ModrinthProjectVersion]>,
//     data: DataEntities,
//     project_type: ModrinthProjectType,

//     version_matrix: FxHashMap<&'static str, VersionMatrixLoaders>,
//     instances: Option<Entity<SelectState<InstanceDropdown>>>,
//     unsupported_instances: usize,

//     target: Option<InstallTarget>,

//     last_selected_minecraft_version: Option<SharedString>,
//     last_selected_loader: Option<SharedString>,

//     fixed_minecraft_version: Option<&'static str>,
//     minecraft_version_select_state: Option<Entity<SelectState<SearchableVec<SharedString>>>>,

//     fixed_loader: Option<ModrinthLoader>,
//     loader_select_state: Option<Entity<SelectState<Vec<SharedString>>>>,
//     skip_loader_check_for_mod_version: bool,
//     install_dependencies: bool,

//     mod_version_select_state: Option<Entity<SelectState<SearchableVec<ModVersionItem>>>>,
// }

struct AutoInstallNotificationType;

pub fn open(
    name: &str,
    project_id: Arc<str>,
    project_type: ModrinthProjectType,
    install_for: InstanceID,
    data: &DataEntities,
    window: &mut Window,
    cx: &mut App,
    loader_override: Option<Loader>,
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

    let key = Uuid::new_v4();
    let title = SharedString::new(format!("Install {}", name));

    if handle_project_versions(
        data,
        name,
        title.clone(),
        key,
        project_id.clone(),
        project_type,
        install_for,
        &project_versions,
        window,
        cx,
        loader_override,
    ) {
        return;
    }

    let _subscription = window.observe(&project_versions, cx, {
        let title = title.clone();
        let data = data.clone();
        let name = name.to_string();
        let loader_override = loader_override;
        move |project_versions, window, cx| {
            handle_project_versions(
                &data,
                &name,
                title.clone(),
                key,
                project_id.clone(),
                project_type,
                install_for,
                &project_versions,
                window,
                cx,
                loader_override,
            );
        }
    });

    let notification = Notification::new()
        .id1::<AutoInstallNotificationType>(key)
        .title(title)
        .content(move |_, _, _| {
            _ = &_subscription;

            h_flex()
                .gap_2()
                .child("Loading project versions from Modrinth...")
                .child(Spinner::new())
                .into_any_element()
        })
        .autohide(true);

    window.push_notification(notification, cx);
}

fn handle_project_versions(
    data: &DataEntities,
    name: &str,
    title: SharedString,
    key: Uuid,
    project_id: Arc<str>,
    project_type: ModrinthProjectType,
    install_for: InstanceID,
    project_versions: &Entity<FrontendMetadataState>,
    window: &mut Window,
    cx: &mut App,
    loader_override: Option<Loader>,
) -> bool {
    let project_versions_owned = match crate::entity::metadata::AsMetadataResult::<ModrinthProjectVersionsResult>::result(
        &*project_versions.read(cx),
    ) {
        FrontendMetadataResult::Loading => {
            return false;
        },
        FrontendMetadataResult::Loaded(v) => v.clone(),
        FrontendMetadataResult::Error(e) => {
            push_error(title.clone(), key, (*e).clone().into(), window, cx);
            return true;
        },
    };
    let project_versions = &project_versions_owned;
    {
            let Some(instance) = data.instances.read(cx).entries.get(&install_for) else {
                return true;
            };
            let configuration = instance.read(cx).configuration.clone();
            let effective_loader = loader_override.unwrap_or(configuration.loader);
            let modrinth_loader = effective_loader.as_modrinth_loader();
            let is_mod = project_type == ModrinthProjectType::Mod || project_type == ModrinthProjectType::Modpack;
            let is_datapack = project_type == ModrinthProjectType::Datapack;
            let allow_all_versions = matches!(
                project_type,
                ModrinthProjectType::Resourcepack | ModrinthProjectType::Shader | ModrinthProjectType::Datapack
            );
            let matching_versions = project_versions
                .0
                .iter()
                .filter(|version| {
                    let Some(loaders) = version.loaders.clone() else {
                        return false;
                    };
                    let Some(game_versions) = &version.game_versions else {
                        return false;
                    };
                    if version.files.is_empty() {
                        return false;
                    }
                    if let Some(status) = version.status
                        && !matches!(status, ModrinthVersionStatus::Listed | ModrinthVersionStatus::Archived)
                    {
                        return false;
                    }
                    if !allow_all_versions && !game_versions.contains(&configuration.minecraft_version) {
                        return false;
                    }
                    if is_mod && effective_loader != Loader::Vanilla && !loaders.contains(&modrinth_loader) {
                        return false;
                    }
                    if is_datapack && !loaders.contains(&ModrinthLoader::Datapack) && !loaders.contains(&ModrinthLoader::Minecraft) {
                        return false;
                    }
                    true
                })
                .collect::<Vec<_>>();

            let mut highest_release = None;
            let mut highest_beta = None;
            let mut highest_alpha = None;

            for (index, version) in matching_versions.iter().enumerate() {
                match version.version_type {
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
            let Some(highest) = highest else {
                push_error(title.clone(), key, "Unable to find matching version of project".into(), window, cx);
                return true;
            };

            let version = matching_versions[highest];

            let required_dependencies: Option<Vec<ModrinthDependency>> = version.dependencies.as_ref().map(|deps| {
                deps.iter()
                    .filter(|dep| dep.project_id.is_some() && dep.dependency_type == ModrinthDependencyType::Required)
                    .cloned()
                    .collect()
            });

            let install_file = version.files.iter().find(|file| file.primary).unwrap_or(version.files.first().unwrap());

            let (datapack_world, show_datapack_world_modal) = if project_type == ModrinthProjectType::Datapack {
                let instance_read = instance.read(cx);
                let config_key = instance_read.dot_minecraft_folder.to_string_lossy().to_string();
                let saved_world = InterfaceConfig::get(cx).datapack_world_by_instance.get(&config_key).cloned();
                let worlds_list: Vec<_> = instance_read.worlds.read(cx).iter()
                    .filter_map(|w| w.level_path.file_name().map(|n| n.to_string_lossy().into_owned()))
                    .collect();
                let saved_still_exists = saved_world.as_ref().map_or(false, |sw| worlds_list.iter().any(|w| w == sw));
                // Always show modal for datapacks so user can choose/confirm world (or prime)
                let show_modal = true;
                let world_for_install = if saved_still_exists { saved_world } else {
                    worlds_list.first().cloned()
                };
                (world_for_install, show_modal)
            } else {
                (None, false)
            };

            if project_type == ModrinthProjectType::Datapack && show_datapack_world_modal {
                let instance_read = instance.read(cx);
                let config_key = instance_read.dot_minecraft_folder.to_string_lossy().to_string();
                let worlds: Vec<InstanceWorldSummary> = instance_read.worlds.read(cx).to_vec();
                data.backend_handle.send(MessageToBackend::RequestLoadWorlds { id: install_for });
                let install_file = install_file.clone();
                let required_deps = Arc::new(required_dependencies.as_deref().unwrap_or(&[]).to_vec());
                let version_hint = Some(configuration.minecraft_version.to_string());
                let name = name.to_string();
                let data = data.clone();
                if worlds.is_empty() {
                    window.open_dialog(cx, move |modal, _window, _cx| {
                        let config_key = config_key.clone();
                        let data = data.clone();
                        let version_hint = version_hint.clone();
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
                                                    .on_click({
                                                        let install_file = install_file.clone();
                                                        let required_deps = required_deps.clone();
                                                        let project_id = project_id.clone();
                                                        let name = name.clone();
                                                        let data = data.clone();
                                                        let version_hint = version_hint.clone();
                                                        move |_, window, cx| {
                                                            InterfaceConfig::get_mut(cx)
                                                                .datapack_world_by_instance
                                                                .insert(config_key.clone(), "World".to_string());
                                                            window.close_dialog(cx);
                                                            modrinth_install::perform_datapack_install(
                                                                "World".to_string(),
                                                                &install_file,
                                                                InstallTarget::Instance(install_for),
                                                                Some(effective_loader),
                                                                &None,
                                                                &version_hint.as_ref().map(|s| s.clone().into()),
                                                                true,
                                                                required_deps.as_ref(),
                                                                project_id.clone(),
                                                                &name,
                                                                &data,
                                                                window,
                                                                cx,
                                                            );
                                                        }
                                                    }),
                                            )
                                            .child(
                                                Button::new("prime_cancel")
                                                    .label(ts!("instance.content.install.datapack.prime.cancel"))
                                                    .on_click(|_, window, cx| {
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
                    window.open_dialog(cx, move |modal, window, cx| {
                        let state_entity = state.clone();
                        let version_hint = version_hint.clone();
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
                                                            let config_key = config_key.clone();
                                                            let world_folders = world_folders.clone();
                                                            let install_file = install_file.clone();
                                                            let required_deps = required_deps.clone();
                                                            let data = data.clone();
                                                            let project_id = project_id.clone();
                                                            let name = name.clone();
                                                            let version_hint = version_hint.clone();
                                                            let state_entity = state_entity.clone();
                                                            move |_, window, cx| {
                                                                let idx = state_entity.read(cx).selected_idx;
                                                                let world_folder = world_folders.get(idx).cloned();
                                                                if let Some(world) = world_folder {
                                                                    InterfaceConfig::get_mut(cx)
                                                                        .datapack_world_by_instance
                                                                        .insert(config_key.clone(), world.clone());
                                                                    window.close_dialog(cx);
                                                                    modrinth_install::perform_datapack_install(
                                                                        world,
                                                                        &install_file,
                                                                        InstallTarget::Instance(install_for),
                                                                        Some(effective_loader),
                                                                        &None,
                                                                        &version_hint.as_ref().map(|s| s.clone().into()),
                                                                        true,
                                                                        required_deps.as_ref(),
                                                                        project_id.clone(),
                                                                        &name,
                                                                        &data,
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
                                                            let config_key = config_key.clone();
                                                            let install_file = install_file.clone();
                                                            let required_deps = required_deps.clone();
                                                            let data = data.clone();
                                                            let project_id = project_id.clone();
                                                            let name = name.clone();
                                                            let version_hint = version_hint.clone();
                                                            move |_, window, cx| {
                                                                InterfaceConfig::get_mut(cx)
                                                                    .datapack_world_by_instance
                                                                    .insert(config_key.clone(), "World".to_string());
                                                                window.close_dialog(cx);
                                                                modrinth_install::perform_datapack_install(
                                                                    "World".to_string(),
                                                                    &install_file,
                                                                    InstallTarget::Instance(install_for),
                                                                    Some(effective_loader),
                                                                    &None,
                                                                    &version_hint.as_ref().map(|s| s.clone().into()),
                                                                    true,
                                                                    required_deps.as_ref(),
                                                                    project_id.clone(),
                                                                    &name,
                                                                    &data,
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
                return true;
            }
            let datapack_world = datapack_world.unwrap_or_default();

            let path = match project_type {
                ModrinthProjectType::Mod => RelativePath::new("mods").join(&*install_file.filename),
                ModrinthProjectType::Modpack => RelativePath::new("mods").join(&*install_file.filename),
                ModrinthProjectType::Resourcepack => RelativePath::new("resourcepacks").join(&*install_file.filename),
                ModrinthProjectType::Shader => RelativePath::new("shaderpacks").join(&*install_file.filename),
                ModrinthProjectType::Datapack => {
                    let world = &datapack_world;
                    RelativePath::new("saves")
                        .join(&world)
                        .join("datapacks")
                        .join(&*install_file.filename)
                },
                ModrinthProjectType::Other => {
                    push_error(title.clone(), key, "Unable to install 'other' project type".into(), window, cx);
                    return true;
                },
            };

            let Some(path) = SafePath::from_relative_path(&path) else {
                push_error(title.clone(), key, "Invalid/dangerous filename".into(), window, cx);
                return true;
            };

            let replace_old = (project_type == ModrinthProjectType::Modpack).then(|| {
                let mods = instance.read(cx).mods.read(cx);
                mods.iter()
                    .find(|mod_summary| {
                        matches!(&mod_summary.content_source, ContentSource::ModrinthProject { project } if project.as_ref() == project_id.as_ref())
                            && matches!(&mod_summary.content_summary.extra, ContentType::ModrinthModpack { .. })
                    })
                    .map(|mod_summary| mod_summary.path.clone())
            }).flatten();

            let mut files = Vec::new();

            if let Some(ref required_dependencies) = required_dependencies {
                for dep in required_dependencies.iter() {
                    files.push(ContentInstallFile {
                        replace_old: None,
                        path: bridge::install::ContentInstallPath::Automatic,
                        download: ContentDownload::Modrinth {
                            project_id: dep.project_id.clone().unwrap(),
                            version_id: dep.version_id.clone(),
                        },
                        content_source: ContentSource::ModrinthProject {
                            project: dep.project_id.clone().unwrap(),
                        },
                    })
                }
            }

            files.push(ContentInstallFile {
                replace_old,
                path: bridge::install::ContentInstallPath::Safe(path),
                download: ContentDownload::Url {
                    url: install_file.url.clone(),
                    sha1: install_file.hashes.sha1.clone(),
                    size: install_file.size,
                },
                content_source: ContentSource::ModrinthProject { project: project_id },
            });

            let content_install = ContentInstall {
                target: InstallTarget::Instance(install_for),
                loader_hint: effective_loader,
                version_hint: Some(configuration.minecraft_version.into()),
                files: files.into(),
            };
            let modal_action = ModalAction::default();

            data.backend_handle.send(MessageToBackend::InstallContent {
                content: content_install.clone(),
                modal_action: modal_action.clone(),
            });

            crate::modals::generic::show_notification_with_note(
                window,
                cx,
                "Error installing content".into(),
                modal_action,
                Notification::new().id1::<AutoInstallNotificationType>(key),
            );

            return true;
        }
}

fn do_datapack_install(
    world: String,
    version: Arc<ModrinthProjectVersion>,
    install_file: Arc<ModrinthFile>,
    project_id: Arc<str>,
    install_for: InstanceID,
    configuration: InstanceConfiguration,
    effective_loader: Loader,
    title: SharedString,
    key: Uuid,
    required_dependencies: &[ModrinthDependency],
    data: DataEntities,
    window: &mut Window,
    cx: &mut App,
) {
    let path = RelativePath::new("saves")
        .join(&world)
        .join("datapacks")
        .join(&*install_file.filename);
    let Some(path) = SafePath::from_relative_path(&path) else {
        push_error(title, key, "Invalid/dangerous filename".into(), window, cx);
        return;
    };
    let mut files = Vec::new();
    for dep in required_dependencies {
        files.push(ContentInstallFile {
            replace_old: None,
            path: bridge::install::ContentInstallPath::Automatic,
            download: ContentDownload::Modrinth {
                project_id: dep.project_id.clone().unwrap(),
                version_id: dep.version_id.clone(),
            },
            content_source: ContentSource::ModrinthProject {
                project: dep.project_id.clone().unwrap(),
            },
        });
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
        target: InstallTarget::Instance(install_for),
        loader_hint: effective_loader,
        version_hint: Some(configuration.minecraft_version.into()),
        files: files.into(),
    };
    let modal_action = ModalAction::default();
    data.backend_handle.send(MessageToBackend::InstallContent {
        content: content_install.clone(),
        modal_action: modal_action.clone(),
    });
    crate::modals::generic::show_notification_with_note(
        window,
        cx,
        "Error installing content".into(),
        modal_action,
        Notification::new().id1::<AutoInstallNotificationType>(key),
    );
}

fn push_error(title: SharedString, key: Uuid, message: SharedString, window: &mut Window, cx: &mut App) {
    let notification = Notification::error(message)
        .id1::<AutoInstallNotificationType>(key)
        .title(title)
        .autohide(false);

    window.push_notification(notification, cx);
}
