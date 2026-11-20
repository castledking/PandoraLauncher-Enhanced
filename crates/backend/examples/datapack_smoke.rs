//! Installs datapacks from Modrinth the ways the launcher can, against the real API, and checks
//! where each one lands:
//!
//! - Veinminer, which publishes both a datapack and a Fabric mod, installed into a Fabric instance
//!   with the datapack picked, goes to `datapacks/` rather than `mods/`
//! - the same project with no loader picked still gets the instance's own (Fabric) build, as before
//! - Arboria, which only publishes a datapack, goes to `datapacks/` and not `resourcepacks/`
//! - on a server, a datapack goes into the world, `world/datapacks/`
//! - a client datapack can be copied from the library into a world
//!
//! cargo run -p backend --example datapack_smoke -- [minecraft version]

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use bridge::{
    install::{ContentDownload, ContentInstall, ContentInstallFile, ContentInstallPath, InstallTarget},
    instance::{ContentFolder, ContentType, InstanceContentSummary, InstanceID},
    message::{MessageToBackend, MessageToFrontend},
    modal_action::ModalAction,
    quit::QuitCoordinator,
};
use schema::{
    content::{ContentInstallReason, ContentSource},
    loader::Loader,
    modrinth::ModrinthLoader,
    server::ServerPlatform,
};

const VEINMINER: &str = "OhduvhIc";
const ARBORIA: &str = "IuVnevHX";

#[derive(Default)]
struct Seen {
    ids_by_name: std::collections::HashMap<String, InstanceID>,
    content: std::collections::HashMap<(InstanceID, ContentFolder), Arc<[InstanceContentSummary]>>,
    errors: Vec<String>,
}

fn main() {
    let version = std::env::args().nth(1).unwrap_or_else(|| "1.21.1".into());

    let launcher_dir = std::env::temp_dir().join("pandora_datapack_smoke");
    _ = std::fs::remove_dir_all(launcher_dir.join("instances"));
    _ = std::fs::remove_dir_all(launcher_dir.join("servers"));
    _ = std::fs::create_dir_all(&launcher_dir);
    println!("launcher dir: {}", launcher_dir.display());

    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    let helper = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();

    let (backend_recv, backend_handle, mut frontend_recv, frontend_handle) = bridge::handle::create_pair();
    let quit = QuitCoordinator::new(Box::new(|| {}));
    backend::start(runtime, launcher_dir.clone(), frontend_handle, backend_handle.clone(), backend_recv, quit);

    let seen = Arc::new(Mutex::new(Seen::default()));
    {
        let seen = seen.clone();
        helper.spawn(async move {
            while let Some(message) = frontend_recv.recv().await {
                match message {
                    MessageToFrontend::InstanceAdded { id, name, .. } => {
                        seen.lock().unwrap().ids_by_name.insert(name.to_string(), id);
                    },
                    MessageToFrontend::InstanceContentUpdated {
                        id,
                        content_folder,
                        content,
                    } => {
                        seen.lock().unwrap().content.insert((id, content_folder), content);
                    },
                    MessageToFrontend::AddNotification {
                        notification_type,
                        message,
                    } => {
                        println!("  notification ({notification_type:?}): {message}");
                        if format!("{notification_type:?}").contains("Error") {
                            seen.lock().unwrap().errors.push(message.to_string());
                        }
                    },
                    _ => {},
                }
            }
        });
    }

    let wait_for = |what: &str, timeout: Duration, mut done: Box<dyn FnMut() -> bool>| {
        let start = Instant::now();
        while !done() {
            if start.elapsed() > timeout {
                panic!("timed out waiting for {what}");
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        println!("  ok: {what} ({:.1}s)", start.elapsed().as_secs_f32());
    };
    let wait_modal = |what: &str, modal: &ModalAction| {
        let start = Instant::now();
        while modal.get_finished_at().is_none() {
            if start.elapsed() > Duration::from_secs(120) {
                panic!("timed out waiting for {what}");
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        if let Some(error) = modal.get_error_message() {
            panic!("{what} failed: {error}");
        }
        println!("  ok: {what} ({:.1}s)", start.elapsed().as_secs_f32());
    };
    let instance_id = |name: &str| {
        let start = Instant::now();
        loop {
            if let Some(id) = seen.lock().unwrap().ids_by_name.get(name) {
                return *id;
            }
            assert!(start.elapsed() < Duration::from_secs(20), "{name} was never added");
            std::thread::sleep(Duration::from_millis(200));
        }
    };
    let install = |what: &str, id: InstanceID, project: &str, only_loaders: Option<ModrinthLoader>| {
        let modal = ModalAction::default();
        backend_handle.send(MessageToBackend::InstallContent {
            content: ContentInstall {
                target: InstallTarget::Instance(id),
                loader: Loader::Fabric,
                minecraft_version: version.as_str().into(),
                files: [ContentInstallFile {
                    replace_old: None,
                    path: ContentInstallPath::Automatic,
                    download: ContentDownload::Modrinth {
                        project_id: project.into(),
                        version_id: None,
                        install_dependencies: false,
                        only_loaders: only_loaders.map(|loader| Arc::from([loader])),
                    },
                    content_source: ContentSource::ModrinthProject {
                        project_id: project.into(),
                    },
                    reason: ContentInstallReason::Standalone,
                }]
                .into(),
            },
            modal_action: modal.clone(),
        });
        wait_modal(what, &modal);
    };
    let files_in = |dir: &Path| -> Vec<String> {
        let mut files: Vec<String> = std::fs::read_dir(dir)
            .map(|entries| entries.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect())
            .unwrap_or_default();
        files.sort();
        files
    };

    println!("\n== create Fabric {version} instance");
    backend_handle.send(MessageToBackend::CreateInstance {
        name: "dp-client".into(),
        version: version.as_str().into(),
        loader: Loader::Fabric,
        icon: None,
        group: None,
    });
    let client = instance_id("dp-client");
    let client_dir: PathBuf = launcher_dir.join("instances").join("dp-client").join(".minecraft");

    println!("\n== Veinminer with the datapack picked");
    install("install", client, VEINMINER, Some(ModrinthLoader::Datapack));
    let datapacks = files_in(&client_dir.join("datapacks"));
    println!("  datapacks/: {datapacks:?}");
    assert!(
        datapacks.iter().any(|file| file.to_lowercase().starts_with("veinminer") && file.ends_with(".zip")),
        "the datapack build should be in datapacks/"
    );
    assert!(files_in(&client_dir.join("mods")).is_empty(), "no mod should have been installed: {:?}", files_in(&client_dir.join("mods")));

    println!("\n== Veinminer with no loader picked (the instance's own)");
    install("install", client, VEINMINER, None);
    let mods = files_in(&client_dir.join("mods"));
    println!("  mods/: {mods:?}");
    assert!(mods.iter().any(|file| file.contains("fabric") && file.ends_with(".jar")), "the Fabric build should be in mods/");

    println!("\n== Arboria, which is only a datapack");
    install("install", client, ARBORIA, None);
    let datapacks = files_in(&client_dir.join("datapacks"));
    println!("  datapacks/: {datapacks:?}");
    assert!(datapacks.iter().any(|file| file.starts_with("arboria")), "Arboria should be in datapacks/");
    assert!(
        !files_in(&client_dir.join("resourcepacks")).iter().any(|file| file.starts_with("arboria")),
        "Arboria must not be installed as a resource pack"
    );

    println!("\n== the datapacks folder loads as datapacks");
    backend_handle.send(MessageToBackend::RequestLoadContentFolder {
        id: client,
        content_folder: ContentFolder::Datapacks,
    });
    wait_for("datapacks folder loaded", Duration::from_secs(20), {
        let seen = seen.clone();
        Box::new(move || {
            seen.lock().unwrap().content.get(&(client, ContentFolder::Datapacks)).is_some_and(|content| content.len() >= 2)
        })
    });
    let content = seen.lock().unwrap().content[&(client, ContentFolder::Datapacks)].clone();
    for summary in content.iter() {
        println!(
            "  {} -> {:?}, source {:?}",
            summary.filename,
            match summary.content_summary.extra {
                ContentType::Datapack => "datapack",
                ContentType::ResourcePack => "resource pack",
                _ => "other",
            },
            summary.content_source
        );
    }
    let arboria = content.iter().find(|summary| summary.filename.starts_with("arboria")).expect("Arboria listed");
    assert!(matches!(arboria.content_summary.extra, ContentType::Datapack), "Arboria should read as a datapack");

    println!("\n== add a datapack to a world");
    let world = client_dir.join("saves").join("Test World");
    std::fs::create_dir_all(&world).unwrap();
    std::fs::write(world.join("level.dat"), b"").unwrap();
    backend_handle.send(MessageToBackend::AddDatapackToWorld {
        id: client,
        datapack: arboria.path.clone(),
        world: world.clone().into(),
    });
    wait_for("copied into the world", Duration::from_secs(10), {
        let target = world.join("datapacks").join(&*arboria.filename);
        Box::new(move || target.is_file())
    });
    println!("\n== a datapack from outside the library is refused");
    let outside = launcher_dir.join("not-a-datapack.zip");
    std::fs::write(&outside, b"nope").unwrap();
    backend_handle.send(MessageToBackend::AddDatapackToWorld {
        id: client,
        datapack: outside.clone().into(),
        world: world.clone().into(),
    });
    std::thread::sleep(Duration::from_secs(1));
    assert!(!world.join("datapacks").join("not-a-datapack.zip").exists(), "a file from elsewhere was copied");
    println!("  ok: refused");

    println!("\n== a Fabric server puts datapacks in its world");
    let modal = ModalAction::default();
    backend_handle.send(MessageToBackend::CreateServerInstance {
        name: "dp-server".into(),
        version: version.as_str().into(),
        platform: ServerPlatform::Fabric,
        eula_accepted: true,
        group: None,
        generate_from: None,
        custom_jar: None,
        modal_action: modal.clone(),
    });
    wait_modal("create server", &modal);
    let server = instance_id("dp-server");
    install("install", server, VEINMINER, Some(ModrinthLoader::Datapack));
    let server_dir = launcher_dir.join("servers").join("dp-server").join(".minecraft");
    let world_datapacks = files_in(&server_dir.join("world").join("datapacks"));
    println!("  world/datapacks/: {world_datapacks:?}");
    assert!(world_datapacks.iter().any(|file| file.to_lowercase().starts_with("veinminer")), "the datapack should be in the world");
    assert!(!server_dir.join("datapacks").exists(), "a server has no datapacks/ of its own");

    let errors = seen.lock().unwrap().errors.clone();
    assert!(errors.is_empty(), "backend reported errors: {errors:?}");
    println!("\nALL OK");
    std::process::exit(0);
}
