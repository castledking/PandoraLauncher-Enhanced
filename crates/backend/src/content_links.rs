//! Links content added to a server by hand to the Modrinth or CurseForge project it came from,
//! by looking its file up by hash, and gives linked content that doesn't ship an icon the icon of
//! its project.

use std::{path::Path, sync::Arc};

use bridge::instance::{ContentFolder, ContentSummary, InstanceContentSummary, InstanceID};
use futures::StreamExt;
use rustc_hash::FxHashMap;
use schema::{
    content::ContentSource,
    curseforge::CurseforgeFingerprintRequest,
    modrinth::{ModrinthProjectsRequest, ModrinthVersionsFromHashesRequest},
};

use crate::{
    BackendState,
    fs::FolderChanges,
    metadata::items::{
        CurseforgeFingerprintMetadataItem, CurseforgeProjectItem, ModrinthProjectsMetadataItem,
        ModrinthVersionsFromHashesMetadataItem,
    },
    mod_metadata::remote_icon_key,
};

/// A file that isn't linked to a project yet. Copies of the same file share one entry.
struct Unlinked {
    hash: [u8; 20],
    source: ContentSource,
    has_icon: bool,
    paths: Vec<Arc<Path>>,
}

/// A project whose icon is missing, with the files that should show it
struct MissingIcon {
    source: ContentSource,
    paths: Vec<Arc<Path>>,
}

/// Called after a content folder loads. Every file and icon is only looked up once a session, and
/// the folder reloads afterwards to show whatever was found.
pub(crate) fn after_content_loaded(
    backend: &Arc<BackendState>,
    id: InstanceID,
    folder: ContentFolder,
    is_server: bool,
    content: &[InstanceContentSummary],
) {
    let manager = &backend.mod_metadata_manager;

    let mut unlinked: FxHashMap<[u8; 20], Unlinked> = FxHashMap::default();
    let mut missing_icons: FxHashMap<Arc<str>, MissingIcon> = FxHashMap::default();

    for summary in content {
        if !summary.can_toggle || ContentSummary::is_unknown(&summary.content_summary) {
            continue;
        }
        let hash = summary.content_summary.hash;
        let has_icon = summary.content_summary.png_icon.is_some();

        match &summary.content_source {
            // Only servers get looked up. On client instances, content added by hand stays the
            // player's to manage, and isn't touched by update checks.
            ContentSource::Manual | ContentSource::ModrinthUnknown => {
                if !is_server {
                    continue;
                }
                if let Some(entry) = unlinked.get_mut(&hash) {
                    entry.paths.push(summary.path.clone());
                } else if manager.claim_identify_attempt(hash) {
                    unlinked.insert(
                        hash,
                        Unlinked {
                            hash,
                            source: summary.content_source.clone(),
                            has_icon,
                            paths: vec![summary.path.clone()],
                        },
                    );
                }
            },
            source => {
                if has_icon {
                    continue;
                }
                let Some(key) = remote_icon_key(source) else {
                    continue;
                };
                if let Some(entry) = missing_icons.get_mut(&key) {
                    entry.paths.push(summary.path.clone());
                } else if manager.claim_remote_icon_attempt(&key) {
                    missing_icons.insert(
                        key,
                        MissingIcon {
                            source: source.clone(),
                            paths: vec![summary.path.clone()],
                        },
                    );
                }
            },
        }
    }

    if unlinked.is_empty() && missing_icons.is_empty() {
        return;
    }

    let unlinked = unlinked.into_values().collect();
    tokio::task::spawn(link_content(backend.clone(), id, folder, unlinked, missing_icons));
}

async fn link_content(
    backend: Arc<BackendState>,
    id: InstanceID,
    folder: ContentFolder,
    unlinked: Vec<Unlinked>,
    mut missing_icons: FxHashMap<Arc<str>, MissingIcon>,
) {
    let manager = &backend.mod_metadata_manager;
    let mut changes = FolderChanges::no_changes();

    for (file, source) in identify(&backend, unlinked).await {
        // Content may have been reinstalled from a project while the lookup was running
        if !manager.read_content_sources().get(&file.hash).should_replace_with(&source) {
            continue;
        }
        manager.set_content_source(file.hash, source.clone());
        for path in &file.paths {
            changes.dirty_path(path.clone());
        }

        if file.has_icon {
            continue;
        }
        let Some(key) = remote_icon_key(&source) else {
            continue;
        };
        if let Some(entry) = missing_icons.get_mut(&key) {
            entry.paths.extend(file.paths);
        } else if manager.claim_remote_icon_attempt(&key) {
            missing_icons.insert(
                key,
                MissingIcon {
                    source,
                    paths: file.paths,
                },
            );
        }
    }

    for path in fetch_icons(&backend, missing_icons).await {
        changes.dirty_path(path);
    }

    if changes.is_empty() {
        return;
    }
    if let Some(instance) = backend.instance_state.write().instances.get_mut(id) {
        instance.mark_content_dirty(&backend, folder, changes, true);
    }
}

/// Looks files up on Modrinth by SHA-1, then the ones Modrinth doesn't know on CurseForge by
/// fingerprint
async fn identify(backend: &BackendState, unlinked: Vec<Unlinked>) -> Vec<(Unlinked, ContentSource)> {
    let mut linked = Vec::new();
    if unlinked.is_empty() {
        return linked;
    }
    let manager = &backend.mod_metadata_manager;

    let hashes: Arc<[Arc<str>]> = unlinked.iter().map(|file| Arc::from(hex::encode(file.hash))).collect();
    let request = ModrinthVersionsFromHashesRequest {
        hashes: hashes.clone(),
        algorithm: "sha1".into(),
    };
    let versions = match backend.meta.fetch(ModrinthVersionsFromHashesMetadataItem(&request)).await {
        Ok(versions) => versions,
        Err(error) => {
            log::warn!("Unable to look up content on Modrinth: {error}");
            manager.release_identify_attempts(unlinked.iter().map(|file| file.hash));
            return linked;
        },
    };

    let mut remaining = Vec::new();
    for (file, hash) in unlinked.into_iter().zip(hashes.iter()) {
        match versions.0.get(hash) {
            Some(Some(version)) => {
                let project_id = version.project_id.clone();
                linked.push((file, ContentSource::ModrinthProject { project_id }));
            },
            // Files that came from a Modrinth modpack can't be on CurseForge
            _ if file.source == ContentSource::Manual => remaining.push(file),
            _ => {},
        }
    }
    if remaining.is_empty() {
        return linked;
    }

    let paths: Vec<Arc<Path>> = remaining.iter().map(|file| file.paths[0].clone()).collect();
    let fingerprints = tokio::task::spawn_blocking(move || {
        paths
            .iter()
            .map(|path| std::fs::read(path).ok().map(|bytes| schema::curseforge::fingerprint(&bytes)))
            .collect::<Vec<_>>()
    })
    .await
    .unwrap_or_default();

    let mut by_fingerprint = FxHashMap::default();
    for (file, fingerprint) in remaining.into_iter().zip(fingerprints) {
        if let Some(fingerprint) = fingerprint {
            by_fingerprint.insert(fingerprint, file);
        }
    }
    if by_fingerprint.is_empty() {
        return linked;
    }

    let request = CurseforgeFingerprintRequest {
        fingerprints: by_fingerprint.keys().copied().collect(),
    };
    match backend.meta.fetch(CurseforgeFingerprintMetadataItem(&request)).await {
        Ok(response) => {
            for exact_match in response.data.exact_matches.iter() {
                if let Some(file) = by_fingerprint.remove(&exact_match.file.file_fingerprint) {
                    let project_id = exact_match.file.mod_id;
                    linked.push((file, ContentSource::CurseforgeProject { project_id }));
                }
            }
        },
        Err(error) => {
            log::warn!("Unable to look up content on CurseForge: {error}");
            manager.release_identify_attempts(by_fingerprint.values().map(|file| file.hash));
        },
    }

    linked
}

/// Downloads project icons, returning the files whose icon is now available
async fn fetch_icons(backend: &BackendState, missing_icons: FxHashMap<Arc<str>, MissingIcon>) -> Vec<Arc<Path>> {
    if missing_icons.is_empty() {
        return Vec::new();
    }
    let manager = &backend.mod_metadata_manager;

    let modrinth_ids: Arc<[Arc<str>]> = missing_icons
        .values()
        .filter_map(|missing| match &missing.source {
            ContentSource::ModrinthProject { project_id } => Some(project_id.clone()),
            _ => None,
        })
        .collect();
    let mut modrinth_icon_urls: FxHashMap<Arc<str>, Arc<str>> = FxHashMap::default();
    if !modrinth_ids.is_empty() {
        let request = ModrinthProjectsRequest { ids: modrinth_ids };
        match backend.meta.fetch(ModrinthProjectsMetadataItem(&request)).await {
            Ok(projects) => {
                for project in projects.0.iter() {
                    if let Some(icon_url) = &project.icon_url {
                        modrinth_icon_urls.insert(project.id.clone(), icon_url.clone());
                    }
                }
            },
            Err(error) => {
                log::warn!("Unable to fetch Modrinth projects for icons: {error}");
                for (key, missing) in &missing_icons {
                    if matches!(missing.source, ContentSource::ModrinthProject { .. }) {
                        manager.release_remote_icon_attempt(key);
                    }
                }
            },
        }
    }

    let client = backend.http_client_provider.client();
    let modrinth_icon_urls = &modrinth_icon_urls;
    futures::stream::iter(missing_icons)
        .map(|(key, missing)| {
            let client = client.clone();
            async move {
                let icon_url = match &missing.source {
                    ContentSource::ModrinthProject { project_id } => modrinth_icon_urls.get(project_id).cloned(),
                    ContentSource::CurseforgeProject { project_id } => {
                        match backend
                            .meta
                            .fetch(CurseforgeProjectItem {
                                project_id: *project_id,
                            })
                            .await
                        {
                            Ok(project) => project.logo.as_ref().map(|logo| logo.thumbnail_url.clone()),
                            Err(error) => {
                                log::warn!("Unable to fetch CurseForge project {project_id} for its icon: {error}");
                                manager.release_remote_icon_attempt(&key);
                                None
                            },
                        }
                    },
                    _ => None,
                };
                let icon_url = icon_url.filter(|url| !url.is_empty())?;

                let response = client.get(&*icon_url).send().await.and_then(|response| response.error_for_status());
                let bytes = match response {
                    Ok(response) => response.bytes().await,
                    Err(error) => Err(error),
                };
                let bytes = match bytes {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        log::warn!("Unable to download icon {icon_url}: {error}");
                        manager.release_remote_icon_attempt(&key);
                        return None;
                    },
                };

                let manager = manager.clone();
                let stored = tokio::task::spawn_blocking(move || manager.store_remote_icon(key, &bytes))
                    .await
                    .unwrap_or(false);
                stored.then_some(missing.paths)
            }
        })
        .buffer_unordered(4)
        .filter_map(std::future::ready)
        .flat_map(futures::stream::iter)
        .collect()
        .await
}
