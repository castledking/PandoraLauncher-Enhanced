//! Drives what happens to files dropped onto the launcher, against the real Modrinth and
//! CurseForge APIs: identifying them, resolving a project someone typed in, installing into a new
//! instance and a new server, falling back to the tab's folder for a file that can't be told, and
//! linking a file to a project by hand.
//!
//! cargo run -p backend --example dropped_files_smoke

use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use bridge::{
    dropped::{DropTarget, DroppedFile},
    instance::{ContentFolder, InstanceContentSummary, InstanceID},
    message::{MessageToBackend, MessageToFrontend},
    modal_action::ModalAction,
    quit::QuitCoordinator,
};
use schema::{content::ContentSource, loader::Loader, server::ServerPlatform};

#[derive(Default)]
struct Seen {
    ids_by_name: std::collections::HashMap<String, InstanceID>,
    content: std::collections::HashMap<(InstanceID, ContentFolder), Arc<[InstanceContentSummary]>>,
    errors: Vec<String>,
}

fn main() {
    let launcher_dir = std::env::temp_dir().join("pandora_dropped_files_smoke");
    _ = std::fs::remove_dir_all(&launcher_dir);
    std::fs::create_dir_all(launcher_dir.join("downloads")).unwrap();
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

    let wait_modal = |what: &str, modal: &ModalAction| {
        let start = Instant::now();
        while modal.get_finished_at().is_none() {
            assert!(start.elapsed() < Duration::from_secs(120), "timed out waiting for {what}");
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
    let download = |url: &str, name: &str| -> PathBuf {
        let path = launcher_dir.join("downloads").join(name);
        let bytes = helper.block_on(async { reqwest::get(url).await.unwrap().bytes().await.unwrap() });
        std::fs::write(&path, bytes).unwrap();
        path
    };

    println!("\n== fetch files the way someone would before dropping them");
    let sodium = download(
        &helper.block_on(modrinth_file_url("AANobbMI", "1.21.1", "fabric")),
        "sodium-from-browser.jar",
    );
    let unknown = launcher_dir.join("downloads").join("my-own-mod.jar");
    write_fabric_jar(&unknown, "my_own_mod", "My Own Mod", "0.1.0");
    let mystery = launcher_dir.join("downloads").join("mystery.zip");
    write_zip(&mystery, &[("readme.txt", "nothing that says what this is")]);
    println!("  ok: Sodium from Modrinth, a mod nobody published, and a zip that's neither");

    println!("\n== inspect: Modrinth knows Sodium by its hash");
    let info = ask(&backend_handle, &helper, Box::new({
        let path: Arc<Path> = sodium.clone().into();
        move |channel| MessageToBackend::InspectDroppedFile { path, channel }
    }))
    .expect("inspect sodium");
    println!(
        "  {} -> {:?}, version {:?}, versions {:?}, loaders {:?}, folder {:?}",
        info.filename,
        info.project.as_ref().map(|project| (&project.title, &project.source, project.icon_url.is_some())),
        info.version_name,
        info.game_versions,
        info.loaders,
        info.content_folder
    );
    let project = info.project.expect("Sodium should be identified");
    assert_eq!(project.source, ContentSource::ModrinthProject { project_id: "AANobbMI".into() });
    assert!(project.icon_url.is_some(), "Sodium has an icon");
    assert!(info.game_versions.iter().any(|version| version.as_str() == "1.21.1"));
    assert_eq!(&*info.loaders, &[Loader::Fabric]);
    assert_eq!(info.content_folder, Some(ContentFolder::Mods));

    println!("\n== inspect: an unpublished mod is read but not identified");
    let info = ask(&backend_handle, &helper, Box::new({
        let path: Arc<Path> = unknown.clone().into();
        move |channel| MessageToBackend::InspectDroppedFile { path, channel }
    }))
    .expect("inspect unknown");
    println!("  {} -> name {:?}, version {:?}, project {:?}", info.filename, info.summary.name, info.summary.version_str, info.project.is_some());
    assert!(info.project.is_none());
    assert_eq!(info.summary.name.as_deref(), Some("My Own Mod"));
    assert_eq!(info.content_folder, Some(ContentFolder::Mods));

    println!("\n== resolve what someone types");
    for (input, expected) in [
        ("https://modrinth.com/mod/lithium", Some(ContentSource::ModrinthProject { project_id: "gvQqBUqZ".into() })),
        ("https://www.curseforge.com/minecraft/mc-mods/jei", Some(ContentSource::CurseforgeProject { project_id: 238222 })),
        ("238222", Some(ContentSource::CurseforgeProject { project_id: 238222 })),
        ("definitely-not-a-real-project-1x9q", None),
    ] {
        let result = ask(&backend_handle, &helper, Box::new(move |channel| MessageToBackend::ResolveProject { input: input.into(), channel }));
        match (&result, &expected) {
            (Ok(project), Some(expected)) => {
                println!("  {input} -> {} {:?} icon={}", project.title, project.source, project.icon_url.is_some());
                assert_eq!(&project.source, expected);
            },
            (Err(error), None) => println!("  {input} -> refused: {error}"),
            _ => panic!("{input}: unexpected {result:?}"),
        }
    }

    println!("\n== search projects across Modrinth and CurseForge");
    let results = ask(
        &backend_handle,
        &helper,
        Box::new(|channel| MessageToBackend::SearchProjects {
            query: "Allium".into(),
            channel,
        }),
    )
    .expect("search Allium");
    for project in results.iter() {
        let site = match project.source {
            ContentSource::ModrinthProject { .. } => "Modrinth",
            ContentSource::CurseforgeProject { .. } => "CurseForge",
            _ => "Other",
        };
        println!("  [{}] {} icon={}", site, project.title, project.icon_url.is_some());
    }
    assert!(!results.is_empty(), "Allium search should return results");
    assert!(
        results.iter().any(|p| matches!(p.source, ContentSource::ModrinthProject { .. })),
        "Allium search should include Modrinth results"
    );
    assert!(
        results.iter().any(|p| matches!(p.source, ContentSource::CurseforgeProject { .. })),
        "Allium search should include CurseForge results"
    );

    println!("\n== drop two files into a new Fabric instance, one linked to Lithium by hand");
    let modal = ModalAction::default();
    backend_handle.send(MessageToBackend::InstallDroppedFiles {
        files: Arc::from([
            DroppedFile {
                path: sodium.clone().into(),
                source: ContentSource::ModrinthProject { project_id: "AANobbMI".into() },
                fallback_folder: None,
            },
            DroppedFile {
                path: unknown.clone().into(),
                source: ContentSource::ModrinthProject { project_id: "gvQqBUqZ".into() },
                fallback_folder: None,
            },
        ]),
        target: DropTarget::NewInstance {
            name: "Dropped".into(),
            minecraft_version: "1.21.1".into(),
            loader: Loader::Fabric,
        },
        modal_action: modal.clone(),
    });
    wait_modal("install into a new instance", &modal);
    let dropped = instance_id("Dropped");
    let mods_dir = launcher_dir.join("instances").join("Dropped").join(".minecraft").join("mods");
    let mods = files_in(&mods_dir);
    println!("  mods/: {mods:?}");
    assert!(mods.contains(&"sodium-from-browser.jar".to_string()) && mods.contains(&"my-own-mod.jar".to_string()));
    let mods_content = load(&backend_handle, &seen, dropped, ContentFolder::Mods, 2);
    let own = mods_content.iter().find(|summary| &*summary.filename == "my-own-mod.jar").unwrap();
    assert_eq!(own.content_source, ContentSource::ModrinthProject { project_id: "gvQqBUqZ".into() }, "the source given on drop is kept");
    println!("  ok: my-own-mod.jar is recorded as Lithium");

    println!("\n== set the project of a file by hand afterwards");
    backend_handle.send(MessageToBackend::SetContentProject {
        id: dropped,
        content_folder: ContentFolder::Mods,
        hashes: Arc::from([own.content_summary.hash]),
        source: ContentSource::CurseforgeProject { project_id: 238222 },
    });
    // An open tab keeps its folder loaded and sees the reload; here it's asked for again
    std::thread::sleep(Duration::from_millis(300));
    backend_handle.send(MessageToBackend::RequestLoadContentFolder { id: dropped, content_folder: ContentFolder::Mods });
    let start = Instant::now();
    loop {
        let relinked = seen.lock().unwrap().content.get(&(dropped, ContentFolder::Mods)).and_then(|content| {
            content.iter().find(|summary| &*summary.filename == "my-own-mod.jar").map(|summary| summary.content_source.clone())
        });
        if relinked == Some(ContentSource::CurseforgeProject { project_id: 238222 }) {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(20), "the new project never showed: {relinked:?}");
        std::thread::sleep(Duration::from_millis(200));
    }
    println!("  ok: my-own-mod.jar is now recorded as JEI");

    println!("\n== a file that can't be told goes to the tab it was dropped on");
    let modal = ModalAction::default();
    backend_handle.send(MessageToBackend::InstallDroppedFiles {
        files: Arc::from([DroppedFile {
            path: mystery.clone().into(),
            source: ContentSource::Manual,
            fallback_folder: Some(ContentFolder::ResourcePacks),
        }]),
        target: DropTarget::Instance(dropped),
        modal_action: modal.clone(),
    });
    wait_modal("install the mystery zip", &modal);
    let packs = files_in(&launcher_dir.join("instances").join("Dropped").join(".minecraft").join("resourcepacks"));
    println!("  resourcepacks/: {packs:?}");
    assert!(packs.contains(&"mystery.zip".to_string()));

    println!("\n== drop a file into a new Fabric server");
    let modal = ModalAction::default();
    backend_handle.send(MessageToBackend::InstallDroppedFiles {
        files: Arc::from([DroppedFile {
            path: unknown.clone().into(),
            source: ContentSource::Manual,
            fallback_folder: None,
        }]),
        target: DropTarget::NewServer {
            name: "Dropped Server".into(),
            minecraft_version: "1.21.1".into(),
            platform: ServerPlatform::Fabric,
            eula_accepted: true,
        },
        modal_action: modal.clone(),
    });
    wait_modal("install into a new server", &modal);
    let server_mods = files_in(&launcher_dir.join("servers").join("Dropped Server").join(".minecraft").join("mods"));
    println!("  server mods/: {server_mods:?}");
    assert!(server_mods.contains(&"my-own-mod.jar".to_string()));
    assert!(launcher_dir.join("servers").join("Dropped Server").join(".minecraft").join("eula.txt").is_file());

    let errors = seen.lock().unwrap().errors.clone();
    assert!(errors.is_empty(), "backend reported errors: {errors:?}");
    println!("\nALL OK");
    std::process::exit(0);
}

fn ask<T>(
    backend_handle: &bridge::handle::BackendHandle,
    helper: &tokio::runtime::Runtime,
    message: Box<dyn FnOnce(tokio::sync::oneshot::Sender<T>) -> MessageToBackend>,
) -> T {
    let (send, recv) = tokio::sync::oneshot::channel();
    backend_handle.send(message(send));
    helper.block_on(recv).unwrap()
}

fn load(
    backend_handle: &bridge::handle::BackendHandle,
    seen: &Arc<Mutex<Seen>>,
    id: InstanceID,
    folder: ContentFolder,
    at_least: usize,
) -> Arc<[InstanceContentSummary]> {
    backend_handle.send(MessageToBackend::RequestLoadContentFolder { id, content_folder: folder });
    let start = Instant::now();
    loop {
        if let Some(content) = seen.lock().unwrap().content.get(&(id, folder))
            && content.len() >= at_least
        {
            return content.clone();
        }
        assert!(start.elapsed() < Duration::from_secs(20), "the folder never loaded");
        std::thread::sleep(Duration::from_millis(200));
    }
}

async fn modrinth_file_url(project: &str, version: &str, loader: &str) -> String {
    let url = format!(
        "https://api.modrinth.com/v2/project/{project}/version?game_versions=[\"{version}\"]&loaders=[\"{loader}\"]"
    );
    let versions: serde_json::Value =
        reqwest::Client::new().get(url).header("User-Agent", "PandoraLauncher-smoke").send().await.unwrap().json().await.unwrap();
    versions[0]["files"][0]["url"].as_str().unwrap().to_string()
}

fn files_in(dir: &Path) -> Vec<String> {
    let mut files: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| entries.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    files.sort();
    files
}

fn write_zip(path: &Path, files: &[(&str, &str)]) {
    let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    for (name, contents) in files {
        zip.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(contents.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
}

fn write_fabric_jar(path: &Path, id: &str, name: &str, version: &str) {
    let manifest = format!(
        r#"{{"schemaVersion":1,"id":"{id}","name":"{name}","version":"{version}","environment":"*","depends":{{"minecraft":"1.21.1"}}}}"#
    );
    write_zip(path, &[("fabric.mod.json", &manifest)]);
}
