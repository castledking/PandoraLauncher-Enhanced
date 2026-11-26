//! Files dropped onto the launcher from outside it: working out what each one is and which project
//! it's a build of, installing them into an instance or server (a new one, if asked), and linking
//! content added by hand to its project.

use std::{ffi::OsString, path::Path, sync::Arc};

use bridge::{
    dropped::{DropTarget, DroppedFile, DroppedFileInfo, ResolvedProject},
    install::{ContentDownload, ContentInstall, ContentInstallFile, ContentInstallPath, InstallTarget},
    instance::{ContentFolder, ContentType, InstanceID},
    modal_action::ModalAction,
};
use schema::{
    content::{ContentInstallReason, ContentSource},
    curseforge::{CURSEFORGE_API_KEY, CurseforgeFingerprintRequest, MINECRAFT_GAME_ID},
    modrinth::{
        ModrinthLoader, ModrinthProjectRequest, ModrinthSearchIndex, ModrinthSearchRequest,
        ModrinthVersionsFromHashesRequest,
    },
};
use serde::Deserialize;
use sha1::{Digest, Sha1};

use crate::{
    BackendState,
    fs::FolderChanges,
    metadata::{
        items::{
            CurseforgeFingerprintMetadataItem, CurseforgeProjectItem, ModrinthProjectMetadataItem,
            ModrinthSearchMetadataItem, ModrinthVersionsFromHashesMetadataItem,
        },
        manager::MetaLoadError,
    },
};

impl BackendState {
    pub async fn inspect_dropped_file(self: &Arc<Self>, path: Arc<Path>) -> Result<DroppedFileInfo, Arc<str>> {
        let filename: Arc<str> = path.file_name().map(|name| name.to_string_lossy().into()).unwrap_or_default();
        if path.is_dir() {
            return Err(format!("{filename} is a folder. Drop the .zip or .jar itself.").into());
        }
        let bytes = tokio::fs::read(&path).await.map_err(|err| format!("Unable to read {filename}: {err}"))?;
        let hash: [u8; 20] = Sha1::digest(&bytes).into();

        let manager = self.mod_metadata_manager.clone();
        let extension = path.extension().map(OsString::from);
        let (summary, bytes) =
            tokio::task::spawn_blocking(move || (manager.get_bytes(&bytes, extension.as_deref()), bytes))
                .await
                .map_err(|err| format!("Unable to read {filename}: {err}"))?;

        let mut info = DroppedFileInfo {
            path,
            filename,
            hash,
            content_folder: content_folder_of(&summary.extra),
            summary,
            project: None,
            version_name: None,
            game_versions: Arc::from([]),
            loaders: Arc::from([]),
        };

        // A published build: Modrinth knows it by SHA-1, CurseForge by fingerprint
        let sha1: Arc<str> = hex::encode(hash).into();
        let request = ModrinthVersionsFromHashesRequest {
            hashes: Arc::from([sha1.clone()]),
            algorithm: "sha1".into(),
        };
        if let Ok(versions) = self.meta.fetch(ModrinthVersionsFromHashesMetadataItem(&request)).await
            && let Some(Some(version)) = versions.0.get(&sha1)
        {
            info.version_name = version.version_number.clone().or_else(|| version.name.clone());
            info.game_versions = version.game_versions.clone().unwrap_or_else(|| Arc::from([]));
            let loaders = version.loaders.clone().unwrap_or_else(|| Arc::from([]));
            info.loaders = loaders.iter().filter_map(|loader| loader.as_pandora()).collect();
            if loaders.contains(&ModrinthLoader::Datapack) && info.summary.extra.mod_loader().is_none() {
                info.content_folder = Some(ContentFolder::Datapacks);
            }
            info.project = self.modrinth_project(&version.project_id).await.ok();
            return Ok(info);
        }

        let fingerprint = schema::curseforge::fingerprint(&bytes);
        let request = CurseforgeFingerprintRequest {
            fingerprints: vec![fingerprint],
        };
        if let Ok(response) = self.meta.fetch(CurseforgeFingerprintMetadataItem(&request)).await
            && let Some(found) = response.data.exact_matches.first()
        {
            info.project = self.curseforge_project(found.file.mod_id).await.ok();
            return Ok(info);
        }

        // Neither site has it, but it may have been linked to a project by hand before
        let source = self.mod_metadata_manager.read_content_sources().get(&hash);
        info.project = match source {
            ContentSource::ModrinthProject { project_id } => self.modrinth_project(&project_id).await.ok(),
            ContentSource::CurseforgeProject { project_id } => self.curseforge_project(project_id).await.ok(),
            ContentSource::Manual | ContentSource::ModrinthUnknown => None,
        };
        Ok(info)
    }

    pub async fn resolve_project_input(self: &Arc<Self>, input: &str) -> Result<ResolvedProject, Arc<str>> {
        match parse_project_input(input) {
            ProjectInput::Modrinth(id) => self.modrinth_project(&id).await,
            ProjectInput::CurseforgeId(id) => self.curseforge_project(id).await,
            ProjectInput::CurseforgeSlug(slug) => {
                let id = self.curseforge_id_for_slug(&slug).await?;
                self.curseforge_project(id).await
            },
            ProjectInput::Empty => Err("Enter a project ID or a Modrinth or CurseForge link".into()),
            ProjectInput::Invalid => Err("That isn't a Modrinth or CurseForge link, or a project ID".into()),
        }
    }

    async fn modrinth_project(&self, id: &str) -> Result<ResolvedProject, Arc<str>> {
        let request = ModrinthProjectRequest { project_id: id.into() };
        let project = self.meta.fetch(ModrinthProjectMetadataItem(&request)).await.map_err(|err| match err {
            MetaLoadError::NonOK(404) => format!("Modrinth has no project called {id}"),
            err => format!("Unable to look {id} up on Modrinth: {err}"),
        })?;
        Ok(ResolvedProject {
            source: ContentSource::ModrinthProject {
                project_id: project.id.clone(),
            },
            title: project.title.clone().unwrap_or_else(|| project.id.clone()),
            icon_url: project.icon_url.clone().filter(|url| !url.is_empty()),
        })
    }

    async fn curseforge_project(&self, id: u32) -> Result<ResolvedProject, Arc<str>> {
        let project = self.meta.fetch(CurseforgeProjectItem { project_id: id }).await.map_err(|err| match err {
            MetaLoadError::NonOK(404) => format!("CurseForge has no project {id}"),
            err => format!("Unable to look {id} up on CurseForge: {err}"),
        })?;
        Ok(ResolvedProject {
            source: ContentSource::CurseforgeProject { project_id: id },
            title: project.name.clone(),
            icon_url: project.logo.as_ref().map(|logo| logo.thumbnail_url.clone()).filter(|url| !url.is_empty()),
        })
    }

    /// Searches Modrinth and CurseForge for projects matching a name, link, or project ID.
    pub async fn search_projects(self: &Arc<Self>, query: &str) -> Result<Arc<[ResolvedProject]>, Arc<str>> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(Arc::from([]));
        }

        // Direct Modrinth/CurseForge URL or numeric CurseForge ID: resolve directly
        let parsed = parse_project_input(query);
        let is_direct_url =
            query.contains('/') && matches!(parsed, ProjectInput::Modrinth(_) | ProjectInput::CurseforgeSlug(_));
        if is_direct_url || matches!(parsed, ProjectInput::CurseforgeId(_)) {
            let project = self.resolve_project_input(query).await?;
            return Ok(Arc::from([project]));
        }

        let exact_modrinth_fut = async {
            if let ProjectInput::Modrinth(ref slug) = parsed {
                self.modrinth_project(slug).await.ok()
            } else {
                None
            }
        };

        let modrinth_fut = async {
            let request = ModrinthSearchRequest {
                query: Some(query.into()),
                facets: None,
                index: ModrinthSearchIndex::Relevance,
                offset: 0,
                limit: 8,
            };
            self.meta
                .fetch(ModrinthSearchMetadataItem(&request))
                .await
                .map(|result| {
                    result
                        .hits
                        .iter()
                        .map(|hit| ResolvedProject {
                            source: ContentSource::ModrinthProject {
                                project_id: hit.project_id.clone(),
                            },
                            title: hit.title.clone().unwrap_or_else(|| hit.project_id.clone()),
                            icon_url: hit.icon_url.clone().filter(|url| !url.is_empty()),
                        })
                        .collect::<Vec<_>>()
                })
                .map_err(|err| Arc::<str>::from(format!("Modrinth search failed: {err}")))
        };

        let curseforge_fut = self.search_curseforge_projects(query, 8);

        let (exact_modrinth, modrinth_res, curseforge_res) =
            tokio::join!(exact_modrinth_fut, modrinth_fut, curseforge_fut);

        let modrinth_hits = match modrinth_res {
            Ok(hits) => hits,
            Err(err) => {
                if exact_modrinth.is_none() && curseforge_res.is_err() {
                    return Err(err);
                }
                Vec::new()
            },
        };
        let curseforge_hits = curseforge_res.unwrap_or_default();

        let query_lower = query.to_ascii_lowercase();
        let mut results: Vec<ResolvedProject> = Vec::with_capacity(12);
        let mut push_unique = |project: ResolvedProject| {
            if results.len() < 12 && !results.iter().any(|existing| existing.source == project.source) {
                results.push(project);
            }
        };

        if let Some(exact) = exact_modrinth {
            push_unique(exact);
        }

        // Partition each platform into exact title matches, prefix matches, and remaining hits
        let partition = |hits: Vec<ResolvedProject>| {
            let mut exact = Vec::new();
            let mut prefix = Vec::new();
            let mut rest = Vec::new();
            for hit in hits {
                let title_lower = hit.title.to_ascii_lowercase();
                if title_lower == query_lower {
                    exact.push(hit);
                } else if title_lower.starts_with(&query_lower) {
                    prefix.push(hit);
                } else {
                    rest.push(hit);
                }
            }
            (exact, prefix, rest)
        };

        let (mr_exact, mr_prefix, mr_rest) = partition(modrinth_hits);
        let (cf_exact, cf_prefix, cf_rest) = partition(curseforge_hits);

        // Interleave Modrinth and CurseForge within each relevance tier
        for (mr_tier, cf_tier) in [(mr_exact, cf_exact), (mr_prefix, cf_prefix), (mr_rest, cf_rest)] {
            let mut mr = mr_tier.into_iter();
            let mut cf = cf_tier.into_iter();
            loop {
                let a = mr.next();
                let b = cf.next();
                if a.is_none() && b.is_none() {
                    break;
                }
                if let Some(hit) = a {
                    push_unique(hit);
                }
                if let Some(hit) = b {
                    push_unique(hit);
                }
            }
        }

        Ok(results.into())
    }

    async fn search_curseforge_projects(&self, query: &str, limit: usize) -> Result<Vec<ResolvedProject>, Arc<str>> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Logo {
            thumbnail_url: Arc<str>,
        }
        #[derive(Deserialize)]
        struct Hit {
            id: u32,
            name: Arc<str>,
            logo: Option<Logo>,
        }
        #[derive(Deserialize)]
        struct Search {
            data: Vec<Hit>,
        }

        let response = self
            .http_client_provider
            .client()
            .get("https://api.curseforge.com/v1/mods/search")
            .header("x-api-key", CURSEFORGE_API_KEY)
            .query(&[
                ("gameId", MINECRAFT_GAME_ID.to_string().as_str()),
                ("searchFilter", query),
                ("sortField", "2"),
                ("sortOrder", "desc"),
                ("pageSize", &limit.to_string()),
            ])
            .send()
            .await
            .and_then(|response| response.error_for_status())
            .map_err(|err| Arc::<str>::from(format!("CurseForge search failed: {err}")))?;
        let search: Search = response
            .json()
            .await
            .map_err(|err| Arc::<str>::from(format!("CurseForge search failed: {err}")))?;
        Ok(search
            .data
            .into_iter()
            .map(|hit| ResolvedProject {
                source: ContentSource::CurseforgeProject { project_id: hit.id },
                title: hit.name,
                icon_url: hit.logo.map(|logo| logo.thumbnail_url).filter(|url| !url.is_empty()),
            })
            .collect())
    }

    /// CurseForge page links carry the project's slug, which only its search turns into an ID.
    async fn curseforge_id_for_slug(&self, slug: &str) -> Result<u32, Arc<str>> {
        #[derive(Deserialize)]
        struct Hit {
            id: u32,
        }
        #[derive(Deserialize)]
        struct Search {
            data: Vec<Hit>,
        }

        let response = self
            .http_client_provider
            .client()
            .get("https://api.curseforge.com/v1/mods/search")
            .header("x-api-key", CURSEFORGE_API_KEY)
            .query(&[("gameId", MINECRAFT_GAME_ID.to_string().as_str()), ("slug", slug)])
            .send()
            .await
            .and_then(|response| response.error_for_status())
            .map_err(|err| format!("Unable to look {slug} up on CurseForge: {err}"))?;
        let search: Search = response
            .json()
            .await
            .map_err(|err| format!("Unable to look {slug} up on CurseForge: {err}"))?;
        search
            .data
            .first()
            .map(|hit| hit.id)
            .ok_or_else(|| format!("CurseForge has no project called {slug}").into())
    }

    /// Runs on the message loop: creating a server has to, and the install then goes on its own
    /// task like any other.
    pub async fn install_dropped_files(
        self: &Arc<Self>,
        files: Arc<[DroppedFile]>,
        target: DropTarget,
        modal_action: ModalAction,
    ) {
        let (existing_id, loader, minecraft_version, new_dir) = match target {
            DropTarget::Instance(id) => {
                let Some((loader, version)) = self.instance_state.write().instances.get_mut(id).map(|instance| {
                    let configuration = instance.configuration.get();
                    (configuration.loader, configuration.minecraft_version)
                }) else {
                    modal_action.set_finished_with_error("Unable to find that instance".into());
                    return;
                };
                (Some(id), loader, version, None)
            },
            DropTarget::NewInstance {
                name,
                minecraft_version,
                loader,
            } => {
                let Some(dir) = self.create_instance_sanitized(&name, &minecraft_version, loader, None, None) else {
                    modal_action.set_finished_with_error("Unable to create the instance".into());
                    return;
                };
                (None, loader, minecraft_version, Some(dir))
            },
            DropTarget::NewServer {
                name,
                minecraft_version,
                platform,
                eula_accepted,
            } => {
                let creating = ModalAction::default();
                let created = self
                    .create_server_instance(
                        (&*name).into(),
                        minecraft_version,
                        platform,
                        eula_accepted,
                        None,
                        None,
                        None,
                        creating.clone(),
                    )
                    .await;
                let Some(dir) = created else {
                    let error = creating.get_error_message().unwrap_or_else(|| "Unable to create the server".into());
                    modal_action.set_finished_with_error(error);
                    return;
                };
                (None, platform.loader(), minecraft_version, Some(dir))
            },
        };

        let this = self.clone();
        tokio::task::spawn(async move {
            // A new instance or server has to be loaded by the folder watcher before installing
            let target_id = match (existing_id, new_dir) {
                (Some(id), _) => id,
                (None, Some(dir)) => match this.wait_for_new_instance(&dir).await {
                    Some(id) => id,
                    None => {
                        modal_action.set_finished_with_error("Unable to load the new instance".into());
                        return;
                    },
                },
                (None, None) => return,
            };

            let mut install_files = Vec::with_capacity(files.len());
            let mut installed_targets: Vec<(ContentFolder, Arc<str>)> = Vec::new();
            for file in files.iter() {
                let (path, folder, filename) = this.dropped_file_path(file).await;
                let mut content_source = file.source.clone();
                let mut replace_old = file.replace_old.clone();

                // When replacing an existing file of the same name in an instance/server, keep the
                // previously linked Modrinth/CurseForge project even if the frontend didn't pass it
                // (e.g. multi-file drop or an unloaded folder).
                if let (Some(folder), Some(filename)) = (folder, &filename) {
                    if !matches!(
                        content_source,
                        ContentSource::ModrinthProject { .. } | ContentSource::CurseforgeProject { .. }
                    ) || replace_old.is_none()
                    {
                        if let Some((existing_path, existing_source)) =
                            this.existing_same_name_content(target_id, folder, filename).await
                        {
                            if replace_old.is_none() {
                                replace_old = Some(existing_path);
                            }
                            if !matches!(
                                content_source,
                                ContentSource::ModrinthProject { .. } | ContentSource::CurseforgeProject { .. }
                            ) && let Some(existing_source) = existing_source
                            {
                                content_source = existing_source;
                            }
                        }
                    }
                    installed_targets.push((folder, filename.clone()));
                }
                install_files.push(ContentInstallFile {
                    replace_old,
                    path,
                    download: ContentDownload::File {
                        path: file.path.to_path_buf(),
                    },
                    content_source,
                    reason: ContentInstallReason::Standalone,
                });
            }

            let content = ContentInstall {
                target: InstallTarget::Instance(target_id),
                loader,
                minecraft_version,
                files: install_files.into(),
            };
            this.install_content(content, modal_action.clone()).await;
            let had_error = modal_action.get_error_message().is_some();
            modal_action.set_finished();

            if !had_error && let Some((reveal_folder, _)) = installed_targets.first() {
                let reveal_folder = *reveal_folder;
                let filenames: Arc<[Arc<str>]> = installed_targets
                    .into_iter()
                    .filter_map(|(folder, name)| (folder == reveal_folder).then_some(name))
                    .collect();
                if !filenames.is_empty() {
                    if let Some(instance) = this.instance_state.write().instances.get_mut(target_id) {
                        instance.mark_content_dirty(&this, reveal_folder, FolderChanges::all_dirty(), true);
                    }
                    this.send.send(bridge::message::MessageToFrontend::RevealInstalledContent {
                        id: target_id,
                        content_folder: reveal_folder,
                        filenames,
                    });
                }
            }
            this.send.send(bridge::message::MessageToFrontend::Refresh);
        });
    }

    /// Where a dropped file goes: decided by what it is, or, for a file that can't be told, the
    /// folder of the tab it was dropped on. Also returns the resolved folder and filename so the
    /// frontend can reveal the file after installation.
    async fn dropped_file_path(
        &self,
        file: &DroppedFile,
    ) -> (ContentInstallPath, Option<ContentFolder>, Option<Arc<str>>) {
        let filename: Option<Arc<str>> = file.path.file_name().map(|name| Arc::from(name.to_string_lossy()));
        let manager = self.mod_metadata_manager.clone();
        let path = file.path.clone();
        let detected_folder = tokio::task::spawn_blocking(move || {
            let bytes = std::fs::read(&path).ok()?;
            let summary = manager.get_bytes(&bytes, path.extension());
            content_folder_of(&summary.extra)
        })
        .await
        .ok()
        .flatten();

        if let Some(folder) = detected_folder {
            return (ContentInstallPath::Automatic, Some(folder), filename);
        }

        if let Some(fallback) = file.fallback_folder
            && let Some(name) = file.path.file_name()
        {
            return (
                ContentInstallPath::Raw(Path::new(fallback.folder_name()).join(name).into()),
                Some(fallback),
                filename,
            );
        }

        (ContentInstallPath::Automatic, None, filename)
    }

    /// Finds an already-installed file with the same filename (enabled or `.disabled`) in
    /// `folder`, along with any Modrinth/CurseForge project previously linked to its hash.
    async fn existing_same_name_content(
        &self,
        id: InstanceID,
        folder: ContentFolder,
        filename: &str,
    ) -> Option<(Arc<Path>, Option<ContentSource>)> {
        let folder_dir = {
            let guard = self.instance_state.read();
            guard.instances.get(id)?.content_state[folder].path.clone()
        };
        let filename_owned = filename.to_string();
        let manager = self.mod_metadata_manager.clone();
        tokio::task::spawn_blocking(move || {
            let direct = folder_dir.join(&filename_owned);
            let disabled = folder_dir.join(format!("{filename_owned}.disabled"));
            let existing = if direct.is_file() {
                direct
            } else if disabled.is_file() {
                disabled
            } else {
                return None;
            };
            let source = std::fs::read(&existing).ok().and_then(|bytes| {
                let hash: [u8; 20] = Sha1::digest(&bytes).into();
                match manager.read_content_sources().get(&hash) {
                    s @ (ContentSource::ModrinthProject { .. } | ContentSource::CurseforgeProject { .. }) => Some(s),
                    _ => None,
                }
            });
            Some((existing.into(), source))
        })
        .await
        .ok()
        .flatten()
    }

    /// Records files as builds of `source`, overriding whatever they were linked to, and reloads
    /// the folder so they pick up the project's icon and update checks.
    pub fn set_content_project(
        self: &Arc<Self>,
        id: InstanceID,
        content_folder: ContentFolder,
        hashes: &[[u8; 20]],
        source: ContentSource,
    ) {
        if source == ContentSource::Manual {
            return;
        }
        for hash in hashes {
            self.mod_metadata_manager.set_content_source(*hash, source.clone());
        }
        self.mod_metadata_manager.invalidate_remote_icon(&source);
        if let Some(instance) = self.instance_state.write().instances.get_mut(id) {
            instance.mark_content_dirty(self, content_folder, FolderChanges::all_dirty(), true);
        }
    }
}

fn content_folder_of(content_type: &ContentType) -> Option<ContentFolder> {
    Some(match content_type.content_folder()? {
        "mods" => ContentFolder::Mods,
        "resourcepacks" => ContentFolder::ResourcePacks,
        "shaderpacks" => ContentFolder::Shaders,
        "plugins" => ContentFolder::Plugins,
        "datapacks" => ContentFolder::Datapacks,
        _ => return None,
    })
}

#[derive(Debug, PartialEq, Eq)]
enum ProjectInput {
    Modrinth(String),
    CurseforgeId(u32),
    CurseforgeSlug(String),
    Empty,
    Invalid,
}

/// Makes sense of what someone pasted to say which project a file is: a Modrinth page link, a
/// CurseForge page link, a Modrinth slug or ID, or a CurseForge project ID (which is a number).
fn parse_project_input(input: &str) -> ProjectInput {
    let input = input.trim();
    if input.is_empty() {
        return ProjectInput::Empty;
    }
    let is_slug = |text: &str| {
        !text.is_empty() && text.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    };

    let without_scheme = input.strip_prefix("https://").or_else(|| input.strip_prefix("http://")).unwrap_or(input);
    let without_query = without_scheme.split(['?', '#']).next().unwrap_or_default();
    let mut segments = without_query.split('/').filter(|segment| !segment.is_empty());
    let host = segments.next().unwrap_or_default().to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);

    match host {
        // modrinth.com/<type>/<slug>
        "modrinth.com" => match (segments.next(), segments.next()) {
            (Some(_), Some(slug)) if is_slug(slug) => ProjectInput::Modrinth(slug.to_string()),
            _ => ProjectInput::Invalid,
        },
        // curseforge.com/minecraft/<class>/<slug>, on the old site too
        "curseforge.com" | "legacy.curseforge.com" => match (segments.next(), segments.next(), segments.next()) {
            (Some(_game), Some(_class), Some(slug)) if is_slug(slug) => ProjectInput::CurseforgeSlug(slug.to_string()),
            _ => ProjectInput::Invalid,
        },
        _ if !input.contains('/') && input.chars().all(|c| c.is_ascii_digit()) => {
            input.parse().map(ProjectInput::CurseforgeId).unwrap_or(ProjectInput::Invalid)
        },
        _ if !input.contains('/') && is_slug(input) => ProjectInput::Modrinth(input.to_string()),
        _ => ProjectInput::Invalid,
    }
}

#[cfg(test)]
mod tests {
    use super::{ProjectInput, parse_project_input};

    #[test]
    fn reads_modrinth_links_and_ids() {
        assert_eq!(
            parse_project_input("https://modrinth.com/mod/sodium"),
            ProjectInput::Modrinth("sodium".into())
        );
        assert_eq!(
            parse_project_input("modrinth.com/datapack/arboria/versions?l=datapack"),
            ProjectInput::Modrinth("arboria".into())
        );
        assert_eq!(parse_project_input("  AANobbMI "), ProjectInput::Modrinth("AANobbMI".into()));
        assert_eq!(parse_project_input("fabric-api"), ProjectInput::Modrinth("fabric-api".into()));
    }

    #[test]
    fn reads_curseforge_links_and_ids() {
        assert_eq!(
            parse_project_input("https://www.curseforge.com/minecraft/mc-mods/jei"),
            ProjectInput::CurseforgeSlug("jei".into())
        );
        assert_eq!(parse_project_input("238222"), ProjectInput::CurseforgeId(238222));
    }

    #[test]
    fn turns_down_anything_else() {
        assert_eq!(parse_project_input(""), ProjectInput::Empty);
        assert_eq!(parse_project_input("https://example.com/mod/sodium"), ProjectInput::Invalid);
        assert_eq!(parse_project_input("https://modrinth.com/mod"), ProjectInput::Invalid);
        assert_eq!(parse_project_input("two words"), ProjectInput::Invalid);
    }
}
