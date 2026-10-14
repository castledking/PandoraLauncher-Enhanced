//! Moves an instance onto another Minecraft version by swapping each piece of its content for the
//! build made for that version. Used on a fresh duplicate, so the original is never touched and
//! anything without a build for the new version can simply be switched off in the copy.
//!
//! The lookups mirror the update check in `backend_handler.rs`. They're kept apart from it so that
//! upstream changes to the update check keep merging cleanly.

use std::sync::Arc;

use bridge::{
    install::{ContentDownload, ContentInstall, ContentInstallFile, ContentInstallPath, InstallTarget},
    instance::{ContentFolder, ContentSummary, ContentType, InstanceContentSummary, InstanceID},
    message::MessageToFrontend,
    modal_action::{ModalAction, ProgressTrackerFinishType},
};
use futures::StreamExt;
use schema::{
    content::{ContentInstallReason, ContentSource},
    curseforge::CurseforgeGetModFilesRequest,
    instance::UpdateChannel,
    loader::Loader,
    modrinth::ModrinthLoader,
};
use strum::IntoEnumIterator;
use ustr::Ustr;

use crate::{
    BackendState,
    instance::Instance,
    metadata::{
        items::{
            CurseforgeGetModFilesMetadataItem, ModrinthV3VersionUpdateMetadataItem, ModrinthVersionUpdateMetadataItem,
            VersionUpdateParameters, VersionV3LoaderFields, VersionV3UpdateParameters,
        },
        manager::{MetaLoadError, MetadataManager},
    },
};

/// How many lookups or downloads run at once.
const CONCURRENCY: usize = 8;

/// How many names a notification lists before summarising the rest.
const LISTED_NAMES: usize = 8;

/// What one piece of content needs in order to run on the new version.
enum Retarget {
    /// Swap it for this file.
    Replace(ContentInstallFile),
    /// The file already supports the new version.
    Keep,
    /// Nothing was published for the new version.
    Missing,
    /// The lookup itself failed, so it isn't known either way.
    Failed,
}

impl BackendState {
    /// Points instance `id` at `version` and replaces its content with builds for that version.
    ///
    /// Mods with no build for the new version are disabled, since leaving them on would most likely
    /// stop the game from starting. Resource packs and shaders without one are left as they are,
    /// since they usually still work.
    pub(crate) async fn move_instance_to_version(
        self: &Arc<Self>,
        id: InstanceID,
        version: Ustr,
        modal_action: &ModalAction,
    ) {
        let Some((name, loader, update_channel)) = self.instance_state.write().instances.get_mut(id).map(|instance| {
            let configuration = instance.configuration.get();
            (instance.name, configuration.loader, configuration.update_channel)
        }) else {
            modal_action.set_finished_with_error("Unable to find the duplicated instance".into());
            return;
        };

        let mut content = Vec::new();
        for folder in ContentFolder::iter() {
            if let Some(summaries) = Instance::load_content(self.clone(), id, folder).await {
                content.extend(
                    summaries
                        .iter()
                        // Folder resource packs can't be swapped for a download, and unreadable
                        // files have no hash to look up
                        .filter(|summary| summary.can_toggle && !ContentSummary::is_unknown(&summary.content_summary))
                        .map(|summary| (folder, summary.clone())),
                );
            }
        }

        let lookup_tracker = modal_action.push_tracker(format!("Finding content for Minecraft {version}").into());
        lookup_tracker.set_total(content.len());

        let results: Vec<(ContentFolder, InstanceContentSummary, Retarget)> = {
            let meta = &*self.meta;
            let lookup_tracker = &lookup_tracker;
            futures::stream::iter(content)
                .map(|(folder, summary)| async move {
                    let retarget = find_for_version(meta, &summary, loader, version, update_channel)
                        .await
                        .unwrap_or_else(|err| {
                            log::warn!("Unable to look up {} for Minecraft {version}: {err:?}", summary.filename);
                            Retarget::Failed
                        });
                    lookup_tracker.add_count(1);
                    (folder, summary, retarget)
                })
                .buffer_unordered(CONCURRENCY)
                .collect()
                .await
        };

        if modal_action.has_requested_cancel() {
            lookup_tracker.set_finished(ProgressTrackerFinishType::Fast);
            return;
        }

        // With no answers at all the launcher is most likely offline, and switching every mod off
        // would leave a copy that's worse than a plain one
        if !results.is_empty() && results.iter().all(|(_, _, retarget)| matches!(retarget, Retarget::Failed)) {
            lookup_tracker.set_finished(ProgressTrackerFinishType::Error);
            modal_action.set_finished_with_error(
                format!(
                    "The instance was duplicated, but its content couldn't be looked up for Minecraft {version}, \
                     so the copy was left on its original version. Check your connection and try again."
                )
                .into(),
            );
            return;
        }
        lookup_tracker.set_finished(ProgressTrackerFinishType::Normal);

        if let Some(instance) = self.instance_state.write().instances.get_mut(id) {
            instance.configuration.modify(|configuration| {
                configuration.minecraft_version = version;
                // Loader builds are tied to a Minecraft version, so let the launcher pick one that
                // matches the new version
                configuration.preferred_loader_version = None;
            });
            self.send.send(instance.create_modify_message());
        }

        let mut replacements = Vec::new();
        let mut turned_off = Vec::new();
        for (folder, summary, retarget) in results {
            match retarget {
                Retarget::Replace(file) => replacements.push((folder, summary, file)),
                Retarget::Keep => {},
                Retarget::Missing | Retarget::Failed => {
                    if folder == ContentFolder::Mods && summary.enabled {
                        turned_off.push(summary);
                    }
                },
            }
        }

        let install_tracker = modal_action.push_tracker(format!("Downloading content for Minecraft {version}").into());
        install_tracker.set_total(replacements.len());

        let failed: Vec<(ContentFolder, InstanceContentSummary, Arc<str>)> = {
            let install_tracker = &install_tracker;
            futures::stream::iter(replacements)
                .map(|(folder, summary, file)| async move {
                    // Each file gets its own action, so one failed download doesn't stop the rest
                    let action = ModalAction::default();
                    self.install_content(
                        ContentInstall {
                            target: InstallTarget::Instance(id),
                            loader,
                            minecraft_version: version,
                            files: [file].into(),
                        },
                        action.clone(),
                    )
                    .await;
                    install_tracker.add_count(1);
                    action.get_error_message().map(|error| (folder, summary, error))
                })
                .buffer_unordered(CONCURRENCY)
                .filter_map(|failed| async move { failed })
                .collect()
                .await
        };
        install_tracker.set_finished(ProgressTrackerFinishType::from_err(!failed.is_empty()));

        for (folder, summary, error) in failed {
            log::warn!("Unable to download {} for Minecraft {version}: {error}", summary.filename);
            if folder == ContentFolder::Mods && summary.enabled {
                turned_off.push(summary);
            }
        }

        for summary in &turned_off {
            let mut disabled = summary.path.to_path_buf();
            disabled.add_extension("disabled");
            if let Err(err) = std::fs::rename(&summary.path, &disabled) {
                log::error!("Unable to disable {:?}: {err}", summary.path);
            }
        }

        self.send.send(MessageToFrontend::Refresh);

        if turned_off.is_empty() {
            self.send.send_success(format!("'{name}' is now on Minecraft {version}"));
        } else {
            let mut names: Vec<&str> = turned_off
                .iter()
                .map(|summary| summary.content_summary.name.as_deref().unwrap_or(&summary.filename))
                .collect();
            names.sort_unstable_by_key(|name| name.to_lowercase());
            let mut list = names.iter().take(LISTED_NAMES).copied().collect::<Vec<_>>().join(", ");
            if names.len() > LISTED_NAMES {
                list.push_str(&format!(" and {} more", names.len() - LISTED_NAMES));
            }
            let detail = match names.len() {
                1 => format!("No {version} version was found for {list}, so it was turned off"),
                count => format!("No {version} version was found for {count} mods, so they were turned off: {list}"),
            };
            self.send.send_warning(format!("'{name}' is now on Minecraft {version}. {detail}"));
        }
    }
}

async fn find_for_version(
    meta: &MetadataManager,
    summary: &InstanceContentSummary,
    loader: Loader,
    version: Ustr,
    update_channel: UpdateChannel,
) -> Result<Retarget, MetaLoadError> {
    let content = &summary.content_summary;

    match &summary.content_source {
        // Content added by hand is looked up by hash as well. Modrinth knows most files however they
        // were installed, and the user asked for the copy to be moved.
        ContentSource::Manual | ContentSource::ModrinthUnknown | ContentSource::ModrinthProject { .. } => {
            let sha1: Arc<str> = hex::encode(content.hash).into();

            let mut found = None;
            for &version_types in update_channel.modrinth_version_types_with_fallback() {
                let result = match &content.extra {
                    ContentType::ModrinthModpack { .. } => {
                        meta.fetch(ModrinthV3VersionUpdateMetadataItem {
                            sha1: sha1.clone(),
                            params: VersionV3UpdateParameters {
                                loaders: ["mrpack".into()].into(),
                                loader_fields: VersionV3LoaderFields {
                                    mrpack_loaders: [loader.as_modrinth_loader()].into(),
                                    game_versions: [version].into(),
                                },
                                version_types,
                            },
                        })
                        .await
                    },
                    extra => {
                        let loaders = extra.modrinth_loaders_for(loader);
                        // A plain jar on a vanilla instance has no loader to search with
                        if loaders.contains(&ModrinthLoader::Unknown) {
                            return Ok(Retarget::Missing);
                        }
                        meta.fetch(ModrinthVersionUpdateMetadataItem {
                            sha1: sha1.clone(),
                            params: VersionUpdateParameters {
                                loaders,
                                game_versions: [version].into(),
                                version_types,
                            },
                        })
                        .await
                    },
                };

                match result {
                    Err(MetaLoadError::NonOK(404)) => continue,
                    result => {
                        found = Some(result?);
                        break;
                    },
                }
            }

            let Some(found) = found else {
                return Ok(Retarget::Missing);
            };
            let found = &found.0;

            if let ContentSource::ModrinthProject { project_id } = &summary.content_source
                && found.project_id != *project_id
            {
                log::error!(
                    "Not replacing {}, Modrinth answered with project {} instead of {project_id}",
                    summary.filename,
                    found.project_id
                );
                return Ok(Retarget::Missing);
            }

            let Some(file) = found.files.iter().find(|file| file.primary).or(found.files.first()) else {
                return Ok(Retarget::Missing);
            };
            let mut hash = [0u8; 20];
            if hex::decode_to_slice(&*file.hashes.sha1, &mut hash).is_err() {
                return Ok(Retarget::Missing);
            }
            if hash == content.hash {
                return Ok(Retarget::Keep);
            }

            Ok(replacement(
                summary,
                &file.filename,
                ContentDownload::Url {
                    url: file.url.clone(),
                    sha1: hash,
                    size: file.size,
                },
                ContentSource::ModrinthProject {
                    project_id: found.project_id.clone(),
                },
            ))
        },
        ContentSource::CurseforgeProject { project_id } => {
            let project_id = *project_id;
            let mod_loader_type = content.extra.curseforge_loader_for(loader).map(|loader| loader as u32);

            let mut found = None;
            for &release_types in update_channel.curseforge_release_types_with_fallback() {
                let request = CurseforgeGetModFilesRequest {
                    mod_id: project_id,
                    game_version: Some(version),
                    mod_loader_type,
                    release_types: Some(release_types),
                    page_size: Some(1),
                };
                match meta.fetch(CurseforgeGetModFilesMetadataItem(&request)).await {
                    Err(MetaLoadError::NonOK(404)) => continue,
                    Ok(files) if files.data.is_empty() => continue,
                    result => {
                        found = Some(result?);
                        break;
                    },
                }
            }

            let Some(file) = found.as_ref().and_then(|files| files.data.first()) else {
                return Ok(Retarget::Missing);
            };

            if file.mod_id != project_id {
                log::error!(
                    "Not replacing {}, CurseForge answered with project {} instead of {project_id}",
                    summary.filename,
                    file.mod_id
                );
                return Ok(Retarget::Missing);
            }

            let Some(sha1) = file.hashes.iter().find(|hash| hash.algo == 1) else {
                return Ok(Retarget::Missing);
            };
            let mut hash = [0u8; 20];
            if hex::decode_to_slice(&*sha1.value, &mut hash).is_err() {
                return Ok(Retarget::Missing);
            }
            if hash == content.hash {
                return Ok(Retarget::Keep);
            }

            // Authors can block downloads from third-party launchers
            let Some(url) = file.download_url.clone() else {
                return Ok(Retarget::Missing);
            };

            Ok(replacement(
                summary,
                &file.file_name,
                ContentDownload::Url {
                    url,
                    sha1: hash,
                    size: file.file_length as usize,
                },
                ContentSource::CurseforgeProject { project_id },
            ))
        },
    }
}

/// Installs `filename` next to the file it replaces, keeping it switched off if the old one was.
fn replacement(
    summary: &InstanceContentSummary,
    filename: &str,
    download: ContentDownload,
    content_source: ContentSource,
) -> Retarget {
    if !crate::fs::is_single_component_path_str(filename) {
        log::error!("Not replacing {}, the new file has an invalid name: {filename}", summary.filename);
        return Retarget::Missing;
    }

    let mut path = summary.path.with_file_name(filename);
    if !summary.enabled {
        path.add_extension("disabled");
    }

    Retarget::Replace(ContentInstallFile {
        replace_old: Some(summary.path.clone()),
        path: ContentInstallPath::Raw(path.into()),
        download,
        content_source,
        reason: ContentInstallReason::Update,
    })
}
