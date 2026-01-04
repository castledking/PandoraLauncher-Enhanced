use std::{ffi::OsString, io::Write, path::{Path, PathBuf}, sync::{atomic::AtomicBool, Arc}};

use bridge::{
    install::{ContentDownload, ContentInstall, ContentInstallFile, ContentInstallPath}, instance::{LoaderSpecificModSummary, ModSummary}, message::MessageToFrontend, modal_action::{ModalAction, ProgressTracker, ProgressTrackerFinishType}, safe_path::SafePath
};
use reqwest::StatusCode;
use schema::content::ContentSource;
use sha1::{Digest, Sha1};
use tokio::io::AsyncWriteExt;

use crate::{metadata::items::MinecraftVersionManifestMetadataItem, BackendState};

#[derive(thiserror::Error, Debug)]
pub enum ContentInstallError {
    #[error("Failed to download remote content")]
    Reqwest(#[from] reqwest::Error),
    #[error("Remote server returned non-200 status code: {0}")]
    NotOK(StatusCode),
    #[error("Downloaded file had the wrong size")]
    WrongFilesize,
    #[error("Downloaded file had the wrong hash")]
    WrongHash,
    #[error("Hash isn't a valid sha1 hash:\n{0}")]
    InvalidHash(Arc<str>),
    #[error("Failed to perform I/O operation:\n{0}")]
    IoError(#[from] std::io::Error),
    #[error("Invalid filename:\n{0}")]
    InvalidPath(Arc<Path>),
}

struct InstallFromContentLibrary {
    from: PathBuf,
    replace: Option<Arc<Path>>,
    hash: [u8; 20],
    content_file: ContentInstallFile,
    mod_summary: Option<Arc<ModSummary>>,
}

impl BackendState {
    pub async fn install_content(&self, content: ContentInstall, modal_action: ModalAction) {
        let semaphore = tokio::sync::Semaphore::new(8);

        let mut tasks = Vec::new();

        for content_file in content.files.iter() {
            tasks.push(async {
                match content_file.download {
                    bridge::install::ContentDownload::Url { ref url, ref sha1, size } => {
                        let (path, hash, mod_summary) = self.download_file_into_library(&modal_action,
                            content_file.path.clone(), url, sha1, size, &semaphore).await?;

                        return Ok(InstallFromContentLibrary {
                            from: path,
                            replace: content_file.replace_old.clone(),
                            hash,
                            content_file: content_file.clone(),
                            mod_summary
                        });
                    },
                    bridge::install::ContentDownload::File { path: ref copy_path } => {
                        let title = format!("Copying {}", copy_path.file_name().unwrap().to_string_lossy());
                        let tracker = ProgressTracker::new(title.into(), self.send.clone());
                        modal_action.trackers.push(tracker.clone());

                        tracker.set_total(3);
                        tracker.notify();

                        let data = tokio::fs::read(copy_path).await?;

                        tracker.set_count(1);
                        tracker.notify();

                        let mut hasher = Sha1::new();
                        hasher.update(&data);
                        let hash: [u8; 20] = hasher.finalize().into();

                        let hash_as_str = hex::encode(hash);

                        let hash_folder = self.directories.content_library_dir.join(&hash_as_str[..2]);
                        let _ = tokio::fs::create_dir_all(&hash_folder).await;
                        let mut path = hash_folder.join(hash_as_str);

                        if let Some(extension) = content_file.path.extension() {
                            path.set_extension(extension);
                        }

                        let valid_hash_on_disk = {
                            let path = path.clone();
                            tokio::task::spawn_blocking(move || {
                                crate::check_sha1_hash(&path, hash).unwrap_or(false)
                            }).await.unwrap()
                        };

                        tracker.set_count(2);
                        tracker.notify();

                        if !valid_hash_on_disk {
                            tokio::fs::write(&path, &data).await?;
                        }

                        tracker.set_count(3);
                        tracker.notify();
                        return Ok(InstallFromContentLibrary {
                            from: path,
                            replace: content_file.replace_old.clone(),
                            hash: hash.into(),
                            content_file: content_file.clone(),
                            mod_summary: self.mod_metadata_manager.get_bytes(&data)
                        });
                    },
                }
            });
        }

        let result: Result<Vec<InstallFromContentLibrary>, ContentInstallError> = futures::future::try_join_all(tasks).await;
        match result {
            Ok(files) => {
                let mut instance_dir = None;

                match content.target {
                    bridge::install::InstallTarget::Instance(instance_id) => {
                        if let Some(instance) = self.instance_state.read().instances.get(instance_id) {
                            instance_dir = Some(instance.dot_minecraft_path.clone());
                        }
                    },
                    bridge::install::InstallTarget::Library => {},
                    bridge::install::InstallTarget::NewInstance { loader, name, mut minecraft_version } => {
                        if minecraft_version.is_none() {
                            if let Ok(meta) = self.meta.fetch(&MinecraftVersionManifestMetadataItem).await {
                                minecraft_version = Some(meta.latest.release.into());
                            }
                        }

                        if let Some(minecraft_version) = minecraft_version {
                            instance_dir = self.create_instance_sanitized(&name, &minecraft_version, loader).await
                                .map(|v| v.join(".minecraft").into());
                        }
                    },
                }

                let sources = files.iter()
                    .filter_map(|install| {
                        if install.content_file.content_source != ContentSource::Manual {
                            Some((install.hash.clone(), install.content_file.content_source))
                        } else {
                            None
                        }
                    });
                self.mod_metadata_manager.set_content_sources(sources);

                if let Some(instance_dir) = instance_dir {
                    for install in files {
                        let target_path = match install.content_file.path {
                            ContentInstallPath::Raw(path) => instance_dir.join(&path),
                            ContentInstallPath::Safe(safe_path) => safe_path.to_path(&instance_dir),
                        };

                        let _ = std::fs::create_dir_all(target_path.parent().unwrap());

                        if let Some(replace) = install.replace {
                            let _ = std::fs::remove_file(replace);
                        }
                        let _ = std::fs::hard_link(install.from, target_path);
                    }
                }
            },
            Err(error) => {
                modal_action.set_error_message(Arc::from(format!("{}", error).as_str()));
            },
        }
    }

    async fn download_file_into_library(&self, modal_action: &ModalAction, content_path: ContentInstallPath, url: &Arc<str>, sha1: &Arc<str>, size: usize, semaphore: &tokio::sync::Semaphore) -> Result<(PathBuf, [u8; 20], Option<Arc<ModSummary>>), ContentInstallError> {
        let mut result = self.download_file_into_library_inner(modal_action, content_path, url, sha1, size, semaphore).await?;

        if let Some(summary) = &result.2 {
            if let LoaderSpecificModSummary::ModrinthModpack { downloads, .. } = &summary.extra {
                let mut tasks = Vec::new();

                for download in downloads.iter() {
                    let Some(path) = SafePath::new(&download.path) else {
                        continue;
                    };

                    tasks.push(self.download_file_into_library_inner(modal_action, ContentInstallPath::Safe(path),
                        &download.downloads[0], &download.hashes.sha1, download.file_size, semaphore));
                }

                _ = futures::future::try_join_all(tasks).await;
            }
            result.2 = self.mod_metadata_manager.get_path(&result.0);
        }

        Ok(result)
    }

    async fn download_file_into_library_inner(&self, modal_action: &ModalAction, content_path: ContentInstallPath, url: &Arc<str>, sha1: &Arc<str>, size: usize, semaphore: &tokio::sync::Semaphore) -> Result<(PathBuf, [u8; 20], Option<Arc<ModSummary>>), ContentInstallError> {
        let _permit = semaphore.acquire().await.unwrap();

        let mut expected_hash = [0u8; 20];
        let Ok(_) = hex::decode_to_slice(&**sha1, &mut expected_hash) else {
            eprintln!("Content install has invalid sha1: {}", sha1);
            return Err(ContentInstallError::InvalidHash(sha1.clone()));
        };

        // Re-encode as hex just in case the given sha1 was uppercase
        let hash_as_str = hex::encode(expected_hash);

        let hash_folder = self.directories.content_library_dir.join(&hash_as_str[..2]);
        let _ = tokio::fs::create_dir_all(&hash_folder).await;
        let mut path = hash_folder.join(hash_as_str);

        if let Some(extension) = content_path.extension() {
            path.set_extension(extension);
        }

        let file_name = match &content_path {
            ContentInstallPath::Raw(path) => path.file_name(),
            ContentInstallPath::Safe(safe_path) => safe_path.file_name().map(std::ffi::OsStr::new),
        };

        let title = format!("Downloading {}", file_name.map(|s| s.to_string_lossy()).unwrap_or(std::borrow::Cow::Borrowed("???")));
        let tracker = ProgressTracker::new(title.into(), self.send.clone());
        modal_action.trackers.push(tracker.clone());

        tracker.set_total(size);
        tracker.notify();

        let valid_hash_on_disk = {
            let path = path.clone();
            tokio::task::spawn_blocking(move || {
                crate::check_sha1_hash(&path, expected_hash).unwrap_or(false)
            }).await.unwrap()
        };

        if valid_hash_on_disk {
            tracker.set_count(size);
            tracker.set_finished(ProgressTrackerFinishType::Fast);
            tracker.notify();
            let summary = self.mod_metadata_manager.get_path(&path);
            return Ok((path, expected_hash, summary));
        }

        let response = self.redirecting_http_client.get(&**url).send().await?;

        if response.status() != StatusCode::OK {
            return Err(ContentInstallError::NotOK(response.status()));
        }

        // Tokio doesn't have lock, so we use std temporarily to lock it
        let file = std::fs::File::create(&path)?;
        _ = file.lock();

        let mut file = tokio::fs::File::from_std(file);

        use futures::StreamExt;
        let mut stream = response.bytes_stream();

        let mut total_bytes = 0;

        let mut hasher = Sha1::new();
        while let Some(item) = stream.next().await {
            let item = item?;

            total_bytes += item.len();
            tracker.add_count(item.len());
            tracker.notify();

            hasher.write_all(&item)?;
            file.write_all(&item).await?;
        }

        tracker.set_finished(ProgressTrackerFinishType::Fast);

        let actual_hash = hasher.finalize();

        let wrong_hash = *actual_hash != expected_hash;
        let wrong_size = total_bytes != size;

        if wrong_hash || wrong_size {
            let _ = file.set_len(0).await;
            drop(file);
            let _ = tokio::fs::remove_file(&path).await;

            if wrong_hash {
                return Err(ContentInstallError::WrongHash);
            } else if wrong_size {
                return Err(ContentInstallError::WrongFilesize);
            } else {
                unreachable!();
            }
        }

        let summary = self.mod_metadata_manager.get_path(&path);
        Ok((path, expected_hash, summary))
    }
}
