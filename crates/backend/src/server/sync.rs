use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use bridge::{
    instance::{ContentFolder, InstanceID},
    message::MessageToFrontend,
    modal_action::ModalAction,
    server_sync::{ServerSyncPlan, ServerSyncSide, SyncableMod},
};

use crate::BackendState;

/// Mods that are client-only but don't say so in their metadata. Forge in particular has no
/// reliable side declaration, so without a list like this a generated server would be handed
/// rendering mods it can't load.
const KNOWN_CLIENT_ONLY: &[&str] = &[
    "sodium",
    "iris",
    "oculus",
    "optifine",
    "embeddium",
    "rubidium",
    "indium",
    "lithium-fabric-client",
    "entityculling",
    "modernfix-client",
    "immediatelyfast",
    "reeses-sodium-options",
    "sodium-extra",
    "moreculling",
    "betterf3",
    "zoomify",
    "lambdynamiclights",
    "continuity",
    "iceberg",
    "controlling",
    "modmenu",
    "dynamic-fps",
    "enhanced-block-entities",
    "cull-less-leaves",
    "ferrite-core-client",
    "borderless-mining",
    "fabrishot",
    "capes",
    "skinlayers3d",
    "3dskinlayers",
    "notenoughanimations",
    "eating-animation",
    "visuality",
    "particle-rain",
    "ambientsounds",
    "soundphysics",
    "presencefootsteps",
    "drippyloadingscreen",
    "legendarytooltips",
    "appleskin-client",
    "jei-client",
    "wthit-client",
    "emi",
    "xaeros_minimap",
    "xaerobetterpvp",
    "journeymap",
    "replaymod",
    "distanthorizons",
];

impl BackendState {
    /// Works out what a server's `mods/` folder would need in order to match a client instance.
    ///
    /// Nothing is copied here. The plan is handed to the UI so the user can see what was detected
    /// and untick anything they disagree with, because side detection can only ever be a good
    /// guess: Fabric mods declare their side, Forge mods generally don't.
    pub fn plan_server_sync(self: &Arc<Self>, server_id: InstanceID, client_id: InstanceID) -> Option<ServerSyncPlan> {
        let mut state = self.instance_state.write();

        let (client_mods_dir, client_name, client_version, client_loader) = {
            let client = state.instances.get_mut(client_id)?;
            let configuration = client.configuration.get();
            (
                client.content_state[ContentFolder::Mods].path.clone(),
                client.name,
                configuration.minecraft_version,
                configuration.loader,
            )
        };

        let (server_mods_dir, server_name, server_version, server_loader) = {
            let server = state.instances.get_mut(server_id)?;
            let configuration = server.configuration.get();
            (
                server.content_state[ContentFolder::Mods].path.clone(),
                server.name,
                configuration.minecraft_version,
                configuration.loader,
            )
        };

        drop(state);

        let client_mods = read_mod_dir(&client_mods_dir);
        let server_mods = read_mod_dir(&server_mods_dir);

        let mut mods = Vec::new();

        for (filename, path) in &client_mods {
            let side = detect_mod_side(path);
            mods.push(SyncableMod {
                filename: filename.clone(),
                side,
                // Client-only mods start unticked; the user can override
                selected: side != ServerSyncSide::ClientOnly,
                already_on_server: server_mods.contains_key(filename),
                only_on_server: false,
            });
        }

        // Mods the server has that the client doesn't, which syncing would remove
        for filename in server_mods.keys() {
            if !client_mods.contains_key(filename) {
                mods.push(SyncableMod {
                    filename: filename.clone(),
                    side: ServerSyncSide::Unknown,
                    selected: true,
                    already_on_server: true,
                    only_on_server: true,
                });
            }
        }

        mods.sort_by(|a, b| a.filename.cmp(&b.filename));

        Some(ServerSyncPlan {
            server_id,
            client_id,
            server_name: server_name.as_str().into(),
            client_name: client_name.as_str().into(),
            version_mismatch: (client_version != server_version)
                .then(|| format!("{client_version} -> {server_version}").into()),
            loader_mismatch: (client_loader != server_loader)
                .then(|| format!("{} -> {}", client_loader.pretty_name(), server_loader.pretty_name()).into()),
            mods: mods.into(),
        })
    }

    /// Applies a sync plan: copies in the selected mods the server is missing, and removes the
    /// selected mods that only exist on the server.
    ///
    /// Copies go through `fastcopy`, so a mod shared between a client instance and its server
    /// costs no extra disk space on filesystems that support reflinks or hard links.
    pub async fn apply_server_sync(self: &Arc<Self>, plan: ServerSyncPlan, modal_action: ModalAction) {
        let state = self.instance_state.read();
        let Some(server) = state.instances.get(plan.server_id) else {
            modal_action.set_finished_with_error("Unable to find that server".into());
            return;
        };
        let Some(client) = state.instances.get(plan.client_id) else {
            modal_action.set_finished_with_error("Unable to find that instance".into());
            return;
        };

        if !server.processes.is_empty() {
            modal_action.set_finished_with_error("Stop the server before syncing its mods".into());
            return;
        }

        let server_mods_dir = server.content_state[ContentFolder::Mods].path.to_path_buf();
        let client_mods_dir = client.content_state[ContentFolder::Mods].path.to_path_buf();
        drop(state);

        _ = std::fs::create_dir_all(&server_mods_dir);

        let tracker = modal_action.push_tracker("Syncing mods".into());
        tracker.set_total(plan.mods.len());

        let mut copied = 0usize;
        let mut removed = 0usize;

        for entry in plan.mods.iter() {
            tracker.add_count(1);

            if !entry.selected {
                continue;
            }

            let target = server_mods_dir.join(&*entry.filename);

            if entry.only_on_server {
                if std::fs::remove_file(&target).is_ok() {
                    removed += 1;
                }
                continue;
            }

            let source = client_mods_dir.join(&*entry.filename);
            if !source.is_file() {
                continue;
            }

            match crate::fs::fastcopy(&source, &target, true, true) {
                Ok(()) => copied += 1,
                Err(err) => log::error!("Unable to sync {:?}: {err}", entry.filename),
            }
        }

        // Record the pairing so the server page can offer to sync again later
        if let Some(instance) = self.instance_state.write().instances.get_mut(plan.server_id) {
            let client_name = plan.client_name.clone();
            instance.configuration.modify(|cfg| {
                if let Some(server) = &mut cfg.server {
                    server.linked_instance = Some(client_name.clone());
                }
            });
            self.send.send(instance.create_modify_message());
        }

        tracker.set_finished(bridge::modal_action::ProgressTrackerFinishType::Normal);
        modal_action.set_finished();

        self.send.send_success(format!("Synced mods: {copied} copied, {removed} removed"));
        self.send.send(MessageToFrontend::Refresh);

        tokio::task::spawn(crate::instance::Instance::load_content(self.clone(), plan.server_id, ContentFolder::Mods));
    }
}

fn read_mod_dir(dir: &Path) -> BTreeMap<Arc<str>, PathBuf> {
    let mut mods = BTreeMap::new();

    let Ok(entries) = std::fs::read_dir(dir) else {
        return mods;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        // `.disabled` mods are deliberately switched off, so they aren't part of the mod set
        if path.extension().is_none_or(|extension| extension != "jar") {
            continue;
        }
        let Some(filename) = path.file_name().map(|name| name.to_string_lossy().into_owned()) else {
            continue;
        };
        mods.insert(filename.into(), path);
    }

    mods
}

/// Guesses which side a mod jar belongs on.
///
/// Fabric mods declare this in `fabric.mod.json`, which is authoritative. Forge and NeoForge have
/// no equivalent, so the only signal left is the name, which is why the result is presented to the
/// user for confirmation rather than acted on silently.
fn detect_mod_side(path: &Path) -> ServerSyncSide {
    if let Some(side) = read_fabric_environment(path) {
        return side;
    }

    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let normalized = stem.replace(['_', ' '], "-");

    for known in KNOWN_CLIENT_ONLY {
        // Matches "sodium-fabric-0.5.11" for "sodium" without matching unrelated names that
        // merely contain the word
        if normalized == *known
            || normalized.starts_with(&format!("{known}-"))
            || normalized.starts_with(&format!("{known}+"))
        {
            return ServerSyncSide::ClientOnly;
        }
    }

    if crate::KNOWN_SHADER_MODS.iter().any(|shader_mod| normalized.starts_with(shader_mod)) {
        return ServerSyncSide::ClientOnly;
    }

    ServerSyncSide::Unknown
}

fn read_fabric_environment(path: &Path) -> Option<ServerSyncSide> {
    #[derive(serde::Deserialize)]
    struct Environment {
        environment: Option<String>,
    }

    let file = std::fs::File::open(path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    let entry = archive.by_name("fabric.mod.json").ok()?;
    let parsed: Environment = serde_json::from_reader(entry).ok()?;

    match parsed.environment?.as_str() {
        "client" => Some(ServerSyncSide::ClientOnly),
        "server" => Some(ServerSyncSide::ServerOnly),
        "*" => Some(ServerSyncSide::Both),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::{io::Write, path::PathBuf};

    use bridge::server_sync::ServerSyncSide;

    use super::{detect_mod_side, read_mod_dir};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("pandora_server_sync_{name}"));
            _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn jar(&self, name: &str, fabric_mod_json: Option<&str>) -> PathBuf {
            let path = self.0.join(name);
            let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
            let options = zip::write::SimpleFileOptions::default();
            if let Some(json) = fabric_mod_json {
                zip.start_file("fabric.mod.json", options).unwrap();
                zip.write_all(json.as_bytes()).unwrap();
            } else {
                zip.start_file("META-INF/MANIFEST.MF", options).unwrap();
                zip.write_all(b"Manifest-Version: 1.0\n").unwrap();
            }
            zip.finish().unwrap();
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn fabric_environment_is_authoritative() {
        let dir = TempDir::new("environment");
        let client = dir.jar("a.jar", Some(r#"{"id":"a","version":"1","environment":"client"}"#));
        let server = dir.jar("b.jar", Some(r#"{"id":"b","version":"1","environment":"server"}"#));
        let both = dir.jar("c.jar", Some(r#"{"id":"c","version":"1","environment":"*"}"#));
        // Declared as both, even though the name looks like a known client mod
        let named = dir.jar("sodium-fabric-0.6.0.jar", Some(r#"{"id":"sodium","version":"1","environment":"*"}"#));

        assert_eq!(detect_mod_side(&client), ServerSyncSide::ClientOnly);
        assert_eq!(detect_mod_side(&server), ServerSyncSide::ServerOnly);
        assert_eq!(detect_mod_side(&both), ServerSyncSide::Both);
        assert_eq!(detect_mod_side(&named), ServerSyncSide::Both);
    }

    #[test]
    fn falls_back_to_known_client_mod_names() {
        let dir = TempDir::new("names");
        let sodium = dir.jar("sodium-neoforge-0.6.0.jar", None);
        let iris = dir.jar("iris-1.8.0+1.21.1.jar", None);
        let jei = dir.jar("jei-1.21.1-neoforge-19.0.0.jar", None);
        // Must not match just because the name starts with a known mod's name
        let lookalike = dir.jar("sodiumcore-1.0.jar", None);

        assert_eq!(detect_mod_side(&sodium), ServerSyncSide::ClientOnly);
        assert_eq!(detect_mod_side(&iris), ServerSyncSide::ClientOnly);
        assert_eq!(detect_mod_side(&jei), ServerSyncSide::Unknown);
        assert_eq!(detect_mod_side(&lookalike), ServerSyncSide::Unknown);
    }

    #[test]
    fn disabled_mods_are_not_part_of_the_mod_set() {
        let dir = TempDir::new("disabled");
        dir.jar("enabled.jar", None);
        dir.jar("switched-off.jar.disabled", None);
        std::fs::write(dir.0.join("notes.txt"), b"not a mod").unwrap();

        let mods = read_mod_dir(&dir.0);
        assert_eq!(mods.keys().map(|name| &**name).collect::<Vec<_>>(), vec!["enabled.jar"]);
    }
}
