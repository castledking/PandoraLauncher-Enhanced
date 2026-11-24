use std::{path::Path, sync::Arc};

use bridge::modal_action::{ModalAction, ProgressTracker, ProgressTrackerFinishType};
use futures::StreamExt;
use rustc_hash::FxHashSet;
use schema::{
    forge::{ForgeMavenManifest, NeoforgeMavenManifest},
    instance::InstanceConfiguration,
    loader::Loader,
    server::ServerPlatform,
    version_manifest::MinecraftVersionType,
};
use ustr::Ustr;

use crate::{BackendState, metadata::items::MinecraftVersionManifestMetadataItem};

/// A server jar that is ready to be launched, plus how to launch it.
pub struct ProvisionedServer {
    /// Jar to pass to `java -jar`, relative to the server directory.
    pub jar: Arc<str>,
    /// Extra arguments the platform needs before `nogui`, e.g. Forge's generated arg file.
    pub extra_args: Vec<Arc<str>>,
    /// Build that was resolved, to be written back onto the configuration.
    pub build: Option<Ustr>,
}

#[derive(Debug)]
pub enum ProvisionError {
    Message(Arc<str>),
}

impl std::fmt::Display for ProvisionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProvisionError::Message(message) => f.write_str(message),
        }
    }
}

impl From<String> for ProvisionError {
    fn from(value: String) -> Self {
        ProvisionError::Message(value.into())
    }
}

impl From<&str> for ProvisionError {
    fn from(value: &str) -> Self {
        ProvisionError::Message(value.into())
    }
}

impl BackendState {
    /// Makes sure the server directory contains a launchable server for the configured platform,
    /// downloading it if it isn't there yet.
    ///
    /// Jars are named after the platform, version and build, so changing any of those downloads a
    /// new one and leaves the old one in place rather than overwriting it. That keeps a failed
    /// download from destroying a server that currently works.
    pub(crate) async fn provision_server(
        self: &Arc<Self>,
        server_dir: &Path,
        configuration: &InstanceConfiguration,
        modal_action: &ModalAction,
    ) -> Result<ProvisionedServer, ProvisionError> {
        let Some(server) = &configuration.server else {
            return Err("Instance is not a server".into());
        };

        // A jar the user supplied replaces whatever the platform would download. The build is
        // passed through untouched so it's still there if they switch back.
        if let Some(jar) = server.active_custom_jar() {
            if !crate::fs::is_single_component_path_str(jar) || !server_dir.join(jar).is_file() {
                return Err(format!(
                    "The custom server jar {jar} is missing from the server folder. Pick it again in this server's \
                     Settings tab."
                )
                .into());
            }
            return Ok(ProvisionedServer {
                jar: jar.into(),
                extra_args: Vec::new(),
                build: server.build,
            });
        }

        let version = configuration.minecraft_version;
        let tracker = modal_action.push_tracker(format!("Preparing {} server", server.platform.pretty_name()).into());
        tracker.set_total(1);

        let result = match server.platform {
            ServerPlatform::Vanilla => self.provision_vanilla(server_dir, version, &tracker).await,
            ServerPlatform::Paper => self.provision_papermc("paper", server_dir, version, &tracker).await,
            ServerPlatform::Purpur => self.provision_purpur(server_dir, version, &tracker).await,
            ServerPlatform::Fabric => self.provision_fabric(server_dir, version, &tracker).await,
            ServerPlatform::Forge | ServerPlatform::NeoForge => {
                self.provision_forge_like(server.platform, server_dir, version, configuration, &tracker)
                    .await
            },
        };

        tracker.set_count(1);
        tracker.set_finished(ProgressTrackerFinishType::from_err(result.is_err()));
        result
    }

    /// The Minecraft releases `platform` publishes servers for, newest first, in the order of
    /// Mojang's own manifest. Each platform's list comes from the same API its jars are downloaded
    /// from, so what is offered here is what [`Self::provision_server`] can then fetch. When that
    /// list can't be had every release is returned, and the download says whether it exists.
    pub(crate) async fn server_platform_versions(self: &Arc<Self>, platform: ServerPlatform) -> Option<Arc<[Ustr]>> {
        let manifest = self.meta.fetch(MinecraftVersionManifestMetadataItem).await.ok()?;
        let releases: Vec<Ustr> = manifest
            .versions
            .iter()
            .filter(|version| matches!(version.r#type, MinecraftVersionType::Release))
            .map(|version| version.id)
            .collect();

        let supported = match self.platform_version_filter(platform).await {
            Ok(supported) => supported,
            Err(err) => {
                log::warn!("Unable to list the versions {} supports: {err}", platform.pretty_name());
                return Some(releases.into());
            },
        };

        Some(releases.into_iter().filter(|version| supported.supports(version)).collect())
    }

    async fn platform_version_filter(self: &Arc<Self>, platform: ServerPlatform) -> Result<PlatformVersions, String> {
        let client = self.http_client_provider.client();
        match platform {
            ServerPlatform::Vanilla => Ok(PlatformVersions::All),
            ServerPlatform::Paper => {
                #[derive(serde::Deserialize)]
                struct Project {
                    versions: std::collections::HashMap<String, Vec<String>>,
                }
                let project: Project = client
                    .get("https://fill.papermc.io/v3/projects/paper")
                    .send()
                    .await
                    .map_err(|err| err.to_string())?
                    .json()
                    .await
                    .map_err(|err| err.to_string())?;
                Ok(PlatformVersions::Listed(project.versions.into_values().flatten().collect()))
            },
            ServerPlatform::Purpur => {
                #[derive(serde::Deserialize)]
                struct Project {
                    versions: Vec<String>,
                }
                let project: Project = client
                    .get("https://api.purpurmc.org/v2/purpur")
                    .send()
                    .await
                    .map_err(|err| err.to_string())?
                    .json()
                    .await
                    .map_err(|err| err.to_string())?;
                Ok(PlatformVersions::Listed(project.versions.into_iter().collect()))
            },
            ServerPlatform::Fabric => {
                #[derive(serde::Deserialize)]
                struct GameVersion {
                    version: String,
                }
                let versions: Vec<GameVersion> = client
                    .get("https://meta.fabricmc.net/v2/versions/game")
                    .send()
                    .await
                    .map_err(|err| err.to_string())?
                    .json()
                    .await
                    .map_err(|err| err.to_string())?;
                Ok(PlatformVersions::Listed(versions.into_iter().map(|version| version.version).collect()))
            },
            ServerPlatform::Forge => {
                let manifest = self
                    .meta
                    .fetch(crate::metadata::items::ForgeInstallerMavenMetadataItem)
                    .await
                    .map_err(|err| format!("{err:?}"))?;
                Ok(PlatformVersions::Listed(forge_minecraft_versions(&manifest)))
            },
            ServerPlatform::NeoForge => {
                let manifest = self
                    .meta
                    .fetch(crate::metadata::items::NeoforgeInstallerMavenMetadataItem)
                    .await
                    .map_err(|err| format!("{err:?}"))?;
                Ok(PlatformVersions::NeoForge(manifest))
            },
        }
    }

    async fn provision_vanilla(
        self: &Arc<Self>,
        server_dir: &Path,
        version: Ustr,
        tracker: &ProgressTracker,
    ) -> Result<ProvisionedServer, ProvisionError> {
        let manifest = self
            .meta
            .fetch(MinecraftVersionManifestMetadataItem)
            .await
            .map_err(|err| format!("Unable to load the Minecraft version manifest: {err:?}"))?;

        let link = manifest
            .versions
            .iter()
            .find(|candidate| candidate.id == version)
            .ok_or_else(|| format!("Unknown Minecraft version {version}"))?;

        let version_info = self
            .meta
            .fetch(crate::metadata::items::MinecraftVersionMetadataItem(link))
            .await
            .map_err(|err| format!("Unable to load Minecraft {version}: {err:?}"))?;

        let download = version_info
            .downloads
            .server
            .as_ref()
            .ok_or_else(|| format!("Minecraft {version} does not publish a server jar"))?;

        let jar: Arc<str> = format!("vanilla-{version}.jar").into();
        self.download_server_file(&download.url, &server_dir.join(&*jar), None, tracker).await?;

        Ok(ProvisionedServer {
            jar,
            extra_args: Vec::new(),
            build: None,
        })
    }

    /// PaperMC's Fill API. Its older v2 API has been shut down and now answers 410 Gone.
    async fn provision_papermc(
        self: &Arc<Self>,
        project: &str,
        server_dir: &Path,
        version: Ustr,
        tracker: &ProgressTracker,
    ) -> Result<ProvisionedServer, ProvisionError> {
        #[derive(serde::Deserialize)]
        struct Build {
            id: u32,
            downloads: std::collections::HashMap<String, Download>,
        }
        #[derive(serde::Deserialize)]
        struct Download {
            name: String,
            url: String,
            checksums: Checksums,
        }
        #[derive(serde::Deserialize)]
        struct Checksums {
            sha256: Option<String>,
        }

        let response = self
            .http_client_provider
            .client()
            .get(format!("https://fill.papermc.io/v3/projects/{project}/versions/{version}/builds/latest"))
            .send()
            .await
            .map_err(|err| format!("Unable to reach the PaperMC API: {err}"))?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(format!("{project} does not have a build for Minecraft {version}").into());
        }

        let build: Build = response
            .json()
            .await
            .map_err(|_| format!("{project} does not have a build for Minecraft {version}"))?;

        let download = build
            .downloads
            .get("server:default")
            .ok_or_else(|| format!("{project} build {} has no server download", build.id))?;

        let jar: Arc<str> = download.name.as_str().into();
        self.download_server_file(
            &download.url,
            &server_dir.join(&*jar),
            download.checksums.sha256.as_deref(),
            tracker,
        )
        .await?;

        Ok(ProvisionedServer {
            jar,
            extra_args: Vec::new(),
            build: Some(build.id.to_string().into()),
        })
    }

    async fn provision_purpur(
        self: &Arc<Self>,
        server_dir: &Path,
        version: Ustr,
        tracker: &ProgressTracker,
    ) -> Result<ProvisionedServer, ProvisionError> {
        #[derive(serde::Deserialize)]
        struct Latest {
            build: String,
        }

        let latest: Latest = self
            .http_client_provider
            .client()
            .get(format!("https://api.purpurmc.org/v2/purpur/{version}/latest"))
            .send()
            .await
            .map_err(|err| format!("Unable to reach the Purpur API: {err}"))?
            .json()
            .await
            .map_err(|_| format!("Purpur does not have a build for Minecraft {version}"))?;

        let jar: Arc<str> = format!("purpur-{version}-{}.jar", latest.build).into();
        let url = format!("https://api.purpurmc.org/v2/purpur/{version}/{}/download", latest.build);
        self.download_server_file(&url, &server_dir.join(&*jar), None, tracker).await?;

        Ok(ProvisionedServer {
            jar,
            extra_args: Vec::new(),
            build: Some(latest.build.into()),
        })
    }

    /// Fabric publishes a self-contained server launcher, so there is no installer step.
    async fn provision_fabric(
        self: &Arc<Self>,
        server_dir: &Path,
        version: Ustr,
        tracker: &ProgressTracker,
    ) -> Result<ProvisionedServer, ProvisionError> {
        #[derive(serde::Deserialize)]
        struct LoaderEntry {
            loader: LoaderVersion,
        }
        #[derive(serde::Deserialize)]
        struct LoaderVersion {
            version: String,
        }

        let loaders: Vec<LoaderEntry> = self
            .http_client_provider
            .client()
            .get(format!("https://meta.fabricmc.net/v2/versions/loader/{version}"))
            .send()
            .await
            .map_err(|err| format!("Unable to reach the Fabric API: {err}"))?
            .json()
            .await
            .map_err(|_| format!("Fabric does not support Minecraft {version}"))?;

        let loader = loaders
            .into_iter()
            .next()
            .ok_or_else(|| format!("Fabric does not support Minecraft {version}"))?
            .loader
            .version;

        #[derive(serde::Deserialize)]
        struct Installer {
            version: String,
            stable: bool,
        }

        let installers: Vec<Installer> = self
            .http_client_provider
            .client()
            .get("https://meta.fabricmc.net/v2/versions/installer")
            .send()
            .await
            .map_err(|err| format!("Unable to reach the Fabric API: {err}"))?
            .json()
            .await
            .map_err(|_| "Unable to read the list of Fabric installers")?;

        let installer = installers
            .iter()
            .find(|installer| installer.stable)
            .or(installers.first())
            .ok_or("Fabric did not list any installers")?
            .version
            .clone();

        let jar: Arc<str> = format!("fabric-{version}-{loader}.jar").into();
        let url = format!("https://meta.fabricmc.net/v2/versions/loader/{version}/{loader}/{installer}/server/jar");
        self.download_server_file(&url, &server_dir.join(&*jar), None, tracker).await?;

        Ok(ProvisionedServer {
            jar,
            extra_args: Vec::new(),
            build: Some(loader.into()),
        })
    }

    /// Forge and NeoForge ship an installer that has to be run once to lay out the server. Since
    /// 1.17 it produces an argument file rather than a runnable jar, which is what
    /// `extra_args` carries.
    async fn provision_forge_like(
        self: &Arc<Self>,
        platform: ServerPlatform,
        server_dir: &Path,
        version: Ustr,
        configuration: &InstanceConfiguration,
        tracker: &ProgressTracker,
    ) -> Result<ProvisionedServer, ProvisionError> {
        let loader_version = self
            .resolve_forge_like_version(platform, configuration)
            .await
            .ok_or_else(|| format!("{} does not support Minecraft {version}", platform.pretty_name()))?;

        let (installer_url, installer_name) = match platform {
            ServerPlatform::Forge => (
                format!(
                    "https://maven.minecraftforge.net/net/minecraftforge/forge/{loader_version}/forge-{loader_version}-installer.jar"
                ),
                format!("forge-{loader_version}-installer.jar"),
            ),
            _ => (
                format!(
                    "https://maven.neoforged.net/releases/net/neoforged/neoforge/{loader_version}/neoforge-{loader_version}-installer.jar"
                ),
                format!("neoforge-{loader_version}-installer.jar"),
            ),
        };

        let installer_path = server_dir.join(&installer_name);
        self.download_server_file(&installer_url, &installer_path, None, tracker).await?;

        if let Some(provisioned) = find_installed_forge_like(platform, server_dir, &loader_version) {
            return Ok(provisioned);
        }

        self.run_forge_installer(server_dir, &installer_path, configuration, tracker).await?;

        find_installed_forge_like(platform, server_dir, &loader_version).ok_or_else(|| {
            format!(
                "The {} installer didn't finish setting up the server. Its log is at {}",
                platform.pretty_name(),
                installer_path.with_extension("jar.log").display()
            )
            .into()
        })
    }

    async fn resolve_forge_like_version(
        self: &Arc<Self>,
        platform: ServerPlatform,
        configuration: &InstanceConfiguration,
    ) -> Option<String> {
        if platform == ServerPlatform::Forge {
            let manifest = self.meta.fetch(crate::metadata::items::ForgeInstallerMavenMetadataItem).await.ok()?;
            configuration.determine_forge_loader_version(&manifest).map(|version| version.to_string())
        } else {
            let manifest = self.meta.fetch(crate::metadata::items::NeoforgeInstallerMavenMetadataItem).await.ok()?;
            configuration
                .determine_neoforge_loader_version(&manifest)
                .map(|version| version.to_string())
        }
    }

    async fn run_forge_installer(
        self: &Arc<Self>,
        server_dir: &Path,
        installer_path: &Path,
        configuration: &InstanceConfiguration,
        tracker: &ProgressTracker,
    ) -> Result<(), ProvisionError> {
        tracker.set_title("Running the server installer".into());

        let java = self.resolve_server_java_binary(configuration, None, None).await?;

        let mut command = command::PandoraCommand::new(java.into_os_string());
        command.current_dir(server_dir);
        command.arg(std::ffi::OsString::from("-jar"));
        command.arg(installer_path.as_os_str().to_os_string());
        command.arg(std::ffi::OsString::from("--installServer"));
        // The installer writes its own log next to the jar. Its stdout is just as verbose, and
        // piping it without reading it would fill the pipe and hang the installer partway through
        command.stdout(command::PandoraStdioReadMode::Null);
        command.stderr(command::PandoraStdioReadMode::Null);

        let child = command
            .spawn()
            .await
            .map_err(|err| format!("Unable to run the server installer: {err}"))?;

        let status = tokio::task::spawn_blocking(move || child.process.wait())
            .await
            .map_err(|err| format!("Unable to wait for the server installer: {err}"))?
            .map_err(|err| format!("Unable to wait for the server installer: {err}"))?;

        // Whether the installer actually worked is decided by the caller checking for the files it
        // should have produced, which is more reliable than its exit code
        log::info!("Server installer finished with {}", status);

        Ok(())
    }

    async fn download_server_file(
        self: &Arc<Self>,
        url: &str,
        target: &Path,
        expected_sha256: Option<&str>,
        tracker: &ProgressTracker,
    ) -> Result<(), ProvisionError> {
        if target.is_file() {
            return Ok(());
        }

        if let Some(parent) = target.parent() {
            _ = std::fs::create_dir_all(parent);
        }

        let response = self
            .http_client_provider
            .redirecting()
            .get(url)
            .send()
            .await
            .map_err(|err| format!("Unable to download the server: {err}"))?;

        if !response.status().is_success() {
            return Err(format!("Unable to download the server: the server returned {}", response.status()).into());
        }

        let total = response.content_length().unwrap_or(0) as usize;
        tracker.set_total(total.max(1));
        tracker.set_count(0);

        // Downloaded to a temporary name so an interrupted download is never mistaken for a
        // complete one on the next launch
        let partial = target.with_extension("partial");
        let mut file = std::fs::File::create(&partial).map_err(|err| format!("Unable to write the server: {err}"))?;

        let mut hasher = <sha2::Sha256 as sha2::Digest>::new();
        let mut stream = response.bytes_stream();
        let mut downloaded = 0usize;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|err| format!("Unable to download the server: {err}"))?;
            sha2::Digest::update(&mut hasher, &chunk);
            std::io::Write::write_all(&mut file, &chunk).map_err(|err| format!("Unable to write the server: {err}"))?;
            downloaded += chunk.len();
            if total > 0 {
                tracker.set_count(downloaded.min(total));
            }
        }

        drop(file);

        if let Some(expected) = expected_sha256 {
            let actual = hex::encode(sha2::Digest::finalize(hasher));
            if !actual.eq_ignore_ascii_case(expected) {
                _ = std::fs::remove_file(&partial);
                return Err("The downloaded server failed its checksum, please try again".into());
            }
        }
        std::fs::rename(&partial, target).map_err(|err| format!("Unable to write the server: {err}"))?;

        Ok(())
    }
}

/// Checks whether a Forge or NeoForge server installer has finished laying out `server_dir`.
///
/// The arg file appears early during installation, so we also verify the final patched server jar:
/// Forge and NeoForge 1.20-1.21 write `{artifact}-{version}-server.jar` next to the arg file, while
/// NeoForge 26.x+ writes `libraries/net/neoforged/minecraft-server-patched/{version}/minecraft-server-patched-{version}.jar`.
fn find_installed_forge_like(
    platform: ServerPlatform,
    server_dir: &Path,
    loader_version: &str,
) -> Option<ProvisionedServer> {
    let library_dir = match platform {
        ServerPlatform::Forge => format!("libraries/net/minecraftforge/forge/{loader_version}"),
        _ => format!("libraries/net/neoforged/neoforge/{loader_version}"),
    };
    // The arg file's classpath uses the platform separator, so only the native one will do
    let args_file = format!("{library_dir}/{}", if cfg!(windows) { "win_args.txt" } else { "unix_args.txt" });
    let patched_server_jars: Vec<String> = match platform {
        ServerPlatform::Forge => vec![format!("{library_dir}/forge-{loader_version}-server.jar")],
        _ => vec![
            format!("{library_dir}/neoforge-{loader_version}-server.jar"),
            format!(
                "libraries/net/neoforged/minecraft-server-patched/{loader_version}/minecraft-server-patched-{loader_version}.jar"
            ),
        ],
    };
    // Before 1.17 the installer produced a runnable jar instead
    let legacy_jar = format!("forge-{loader_version}.jar");

    if server_dir.join(&args_file).is_file() && patched_server_jars.iter().any(|jar| server_dir.join(jar).is_file()) {
        Some(ProvisionedServer {
            jar: "".into(),
            extra_args: vec![format!("@{args_file}").into()],
            build: Some(loader_version.into()),
        })
    } else if server_dir.join(&legacy_jar).is_file() {
        Some(ProvisionedServer {
            jar: legacy_jar.as_str().into(),
            extra_args: Vec::new(),
            build: Some(loader_version.into()),
        })
    } else {
        None
    }
}

/// Which Minecraft versions a platform has servers for.
enum PlatformVersions {
    All,
    Listed(FxHashSet<String>),
    /// NeoForge versions encode the Minecraft version in their own scheme, so they are matched
    /// the same way the installer is picked rather than listed up front.
    NeoForge(Arc<NeoforgeMavenManifest>),
}

impl PlatformVersions {
    fn supports(&self, minecraft_version: &Ustr) -> bool {
        match self {
            PlatformVersions::All => true,
            PlatformVersions::Listed(versions) => versions.contains(minecraft_version.as_str()),
            PlatformVersions::NeoForge(manifest) => InstanceConfiguration::new(*minecraft_version, Loader::NeoForge)
                .determine_neoforge_loader_version(manifest)
                .is_some(),
        }
    }
}

/// Forge versions are `{minecraft}-{forge}`.
fn forge_minecraft_versions(manifest: &ForgeMavenManifest) -> FxHashSet<String> {
    manifest
        .0
        .iter()
        .filter_map(|version| version.split_once('-').map(|(minecraft, _)| minecraft.to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use schema::forge::{ForgeMavenManifest, NeoforgeMavenManifest};

    use super::{PlatformVersions, forge_minecraft_versions};

    #[test]
    fn forge_versions_are_read_from_the_prefix() {
        let manifest = ForgeMavenManifest(vec![
            "1.20.1-47.3.0".into(),
            "1.20.1-47.2.0".into(),
            "1.7.10-10.13.4.1614-1.7.10".into(),
        ]);
        let versions = PlatformVersions::Listed(forge_minecraft_versions(&manifest));
        assert!(versions.supports(&"1.20.1".into()));
        assert!(versions.supports(&"1.7.10".into()));
        assert!(!versions.supports(&"1.21.1".into()));
    }

    #[test]
    fn neoforge_versions_match_the_way_the_installer_is_picked() {
        let manifest = NeoforgeMavenManifest(vec!["21.1.77".into(), "20.4.237".into(), "21.0.167".into()]);
        let versions = PlatformVersions::NeoForge(manifest.into());
        assert!(versions.supports(&"1.21.1".into()));
        assert!(versions.supports(&"1.21".into()));
        assert!(versions.supports(&"1.20.4".into()));
        assert!(!versions.supports(&"1.20.1".into()));
        assert!(!versions.supports(&"1.21.4".into()));
    }

    #[test]
    fn recognizes_neoforge_26_patched_server_jar_layout() {
        use schema::server::ServerPlatform;

        use super::find_installed_forge_like;

        let dir = std::env::temp_dir().join(format!("pandora_neoforge26_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let args_name = if cfg!(windows) { "win_args.txt" } else { "unix_args.txt" };
        let args_path = dir.join("libraries/net/neoforged/neoforge/26.1.2.130").join(args_name);
        std::fs::create_dir_all(args_path.parent().unwrap()).unwrap();
        std::fs::write(&args_path, "").unwrap();

        // Arg file alone isn't enough until the patched server jar exists
        assert!(find_installed_forge_like(ServerPlatform::NeoForge, &dir, "26.1.2.130").is_none());

        let patched_jar = dir.join(
            "libraries/net/neoforged/minecraft-server-patched/26.1.2.130/minecraft-server-patched-26.1.2.130.jar",
        );
        std::fs::create_dir_all(patched_jar.parent().unwrap()).unwrap();
        std::fs::write(&patched_jar, "").unwrap();

        let provisioned = find_installed_forge_like(ServerPlatform::NeoForge, &dir, "26.1.2.130")
            .expect("NeoForge 26.x layout should be recognized");
        assert_eq!(&*provisioned.jar, "");
        assert_eq!(provisioned.extra_args.len(), 1);
        assert!(provisioned.extra_args[0].starts_with("@libraries/net/neoforged/neoforge/26.1.2.130/"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
