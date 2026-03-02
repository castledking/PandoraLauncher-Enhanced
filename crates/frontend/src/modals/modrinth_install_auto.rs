use std::{cmp::Ordering, sync::Arc};

use bridge::{
    install::{ContentDownload, ContentInstall, ContentInstallFile, InstallTarget},
    instance::{ContentType, InstanceID},
    message::MessageToBackend,
    meta::MetadataRequest,
    modal_action::ModalAction,
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
    notification::{Notification, NotificationType},
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
use uuid::Uuid;

use crate::{
    component::{error_alert::ErrorAlert, instance_dropdown::InstanceDropdown},
    entity::{
        DataEntities,
        instance::InstanceEntry,
        metadata::{AsMetadataResult, FrontendMetadata, FrontendMetadataResult, FrontendMetadataState},
    },
    root,
};

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
        let loader_override = loader_override;
        move |project_versions, window, cx| {
            handle_project_versions(
                &data,
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
        .autohide(false);

    window.push_notification(notification, cx);
}

fn handle_project_versions(
    data: &DataEntities,
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
    let result: FrontendMetadataResult<ModrinthProjectVersionsResult> = project_versions.read(cx).result();
    match result {
        FrontendMetadataResult::Loading => {
            return false;
        },
        FrontendMetadataResult::Loaded(project_versions) => {
            let Some(instance) = data.instances.read(cx).entries.get(&install_for) else {
                return true;
            };
            let configuration = instance.read(cx).configuration.clone();
            let effective_loader = loader_override.unwrap_or(configuration.loader);
            let modrinth_loader = effective_loader.as_modrinth_loader();
            let is_mod = project_type == ModrinthProjectType::Mod || project_type == ModrinthProjectType::Modpack;
            let allow_all_versions =
                project_type == ModrinthProjectType::Resourcepack || project_type == ModrinthProjectType::Shader;
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

            let install_file = version.files.iter().find(|file| file.primary).unwrap_or(version.files.first().unwrap());

            let path = match project_type {
                ModrinthProjectType::Mod => RelativePath::new("mods").join(&*install_file.filename),
                ModrinthProjectType::Modpack => RelativePath::new("mods").join(&*install_file.filename),
                ModrinthProjectType::Resourcepack => RelativePath::new("resourcepacks").join(&*install_file.filename),
                ModrinthProjectType::Shader => RelativePath::new("shaderpacks").join(&*install_file.filename),
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

            let required_dependencies = version.dependencies.as_ref().map(|deps| {
                deps.iter()
                    .filter(|dep| dep.project_id.is_some() && dep.dependency_type == ModrinthDependencyType::Required)
                    .cloned()
                    .collect::<Vec<_>>()
            });

            if let Some(required_dependencies) = required_dependencies {
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
        },
        FrontendMetadataResult::Error(error) => {
            push_error(
                title.clone(),
                key,
                format!("Error loading project versions from Modrinth:\n{error}").into(),
                window,
                cx,
            );
            return true;
        },
    }
}

fn push_error(title: SharedString, key: Uuid, message: SharedString, window: &mut Window, cx: &mut App) {
    let notification = Notification::error(message)
        .id1::<AutoInstallNotificationType>(key)
        .title(title)
        .autohide(false);

    window.push_notification(notification, cx);
}
