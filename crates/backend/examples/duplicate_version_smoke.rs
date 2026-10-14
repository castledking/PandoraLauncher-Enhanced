//! Drives "duplicate to another Minecraft version" end to end without the GUI: builds a Fabric
//! instance with mods from Modrinth and CurseForge, one dropped in by hand, and one made up, then
//! duplicates it onto another version and checks the copy's mods were swapped for that version,
//! the made-up one was turned off, and the original wasn't touched. Talks to the real Modrinth and CurseForge APIs, so it is
//! an example rather than a test.
//!
//! cargo run -p backend --example duplicate_version_smoke -- [from version] [to version]

use std::{
    collections::BTreeMap,
    io::Write,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use bridge::{
    install::{ContentDownload, ContentInstall, ContentInstallFile, ContentInstallPath, InstallTarget},
    instance::InstanceID,
    message::{MessageToBackend, MessageToFrontend},
    modal_action::ModalAction,
    quit::QuitCoordinator,
};
use schema::{
    content::{ContentInstallReason, ContentSource},
    loader::Loader,
};
use sha1::Digest;

#[derive(Default)]
struct Seen {
    ids_by_name: std::collections::HashMap<String, InstanceID>,
    warnings: Vec<String>,
    errors: Vec<String>,
}

const MODRINTH_PROJECTS: &[(&str, &str)] = &[("Fabric API", "P7dR8mSH"), ("Sodium", "AANobbMI")];
/// Lithium, downloaded straight from Modrinth so the launcher has no record of where it came from
const MANUAL_MODRINTH_PROJECT: &str = "gvQqBUqZ";
/// Just Enough Items, which publishes Fabric builds on CurseForge for both versions
const CURSEFORGE_PROJECT: u32 = 238222;

fn main() {
    let mut args = std::env::args().skip(1);
    let from_version = args.next().unwrap_or_else(|| "1.20.1".into());
    let to_version = args.next().unwrap_or_else(|| "1.21.1".into());

    let launcher_dir = std::env::temp_dir().join("pandora_duplicate_version_smoke");
    _ = std::fs::remove_dir_all(launcher_dir.join("instances"));
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
                    MessageToFrontend::AddNotification {
                        notification_type,
                        message,
                    } => {
                        println!("  notification ({notification_type:?}): {message}");
                        let kind = format!("{notification_type:?}");
                        if kind.contains("Error") {
                            seen.lock().unwrap().errors.push(message.to_string());
                        } else if kind.contains("Warning") {
                            seen.lock().unwrap().warnings.push(message.to_string());
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
    let wait_modal = |what: &str, modal: &ModalAction, timeout: Duration| {
        let start = Instant::now();
        while modal.get_finished_at().is_none() {
            if start.elapsed() > timeout {
                panic!("timed out waiting for {what}");
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        if let Some(error) = modal.get_error_message() {
            panic!("{what} failed: {error}");
        }
        println!("  ok: {what} ({:.1}s)", start.elapsed().as_secs_f32());
    };

    let source_name = format!("dup-source-{from_version}");
    let copy_name = format!("dup-copy-{to_version}");
    let instances_dir = launcher_dir.join("instances");
    let source_mods = instances_dir.join(&source_name).join(".minecraft").join("mods");
    let copy_mods = instances_dir.join(&copy_name).join(".minecraft").join("mods");

    println!("\n== create Fabric {from_version} instance '{source_name}'");
    backend_handle.send(MessageToBackend::CreateInstance {
        name: source_name.as_str().into(),
        version: from_version.as_str().into(),
        loader: Loader::Fabric,
        icon: None,
        group: None,
    });
    wait_for("instance added", Duration::from_secs(15), {
        let seen = seen.clone();
        let source_name = source_name.clone();
        Box::new(move || seen.lock().unwrap().ids_by_name.contains_key(&source_name))
    });
    let source_id = seen.lock().unwrap().ids_by_name[&source_name];

    println!("\n== install mods for {from_version}");
    let mut files: Vec<ContentInstallFile> = MODRINTH_PROJECTS
        .iter()
        .map(|(_, project_id)| ContentInstallFile {
            replace_old: None,
            path: ContentInstallPath::Automatic,
            download: ContentDownload::Modrinth {
                project_id: (*project_id).into(),
                version_id: None,
                install_dependencies: false,
            },
            content_source: ContentSource::ModrinthProject {
                project_id: (*project_id).into(),
            },
            reason: ContentInstallReason::Standalone,
        })
        .collect();
    files.push(ContentInstallFile {
        replace_old: None,
        path: ContentInstallPath::Automatic,
        download: ContentDownload::Curseforge {
            project_id: CURSEFORGE_PROJECT,
            install_dependencies: false,
        },
        content_source: ContentSource::CurseforgeProject {
            project_id: CURSEFORGE_PROJECT,
        },
        reason: ContentInstallReason::Standalone,
    });
    let modal = ModalAction::default();
    backend_handle.send(MessageToBackend::InstallContent {
        content: ContentInstall {
            target: InstallTarget::Instance(source_id),
            loader: Loader::Fabric,
            minecraft_version: from_version.as_str().into(),
            files: files.into(),
        },
        modal_action: modal.clone(),
    });
    wait_modal("install", &modal, Duration::from_secs(300));

    // Unknown to Modrinth, so there is nothing to move it to
    {
        let mut zip = zip::ZipWriter::new(std::fs::File::create(source_mods.join("handmade-test-mod.jar")).unwrap());
        zip.start_file("fabric.mod.json", zip::write::SimpleFileOptions::default()).unwrap();
        write!(
            zip,
            r#"{{"schemaVersion":1,"id":"handmade_test_mod","name":"Handmade Test Mod","version":"1","depends":{{"minecraft":"{from_version}"}}}}"#
        )
        .unwrap();
        zip.finish().unwrap();
    }

    // Known to Modrinth, but dropped in by hand, so it can only be found by its hash
    helper.block_on(async {
        let url = format!(
            "https://api.modrinth.com/v2/project/{MANUAL_MODRINTH_PROJECT}/version?loaders=%5B%22fabric%22%5D&game_versions=%5B%22{from_version}%22%5D"
        );
        let versions: serde_json::Value = reqwest::get(url).await.unwrap().json().await.unwrap();
        let file_url = versions[0]["files"][0]["url"].as_str().expect("a file for the manual mod");
        let bytes = reqwest::get(file_url).await.unwrap().bytes().await.unwrap();
        std::fs::write(source_mods.join("dropped-in-by-hand.jar"), bytes).unwrap();
    });

    let source_before = listing(&source_mods);
    print_listing("source mods", &source_before);
    assert_eq!(source_before.len(), 5, "expected three installed mods and two added by hand");

    println!("\n== duplicate '{source_name}' as '{copy_name}' on {to_version}");
    let modal = ModalAction::default();
    backend_handle.send(MessageToBackend::DuplicateInstance {
        id: source_id,
        name: copy_name.as_str().into(),
        version: Some(to_version.as_str().into()),
        modal_action: modal.clone(),
    });
    wait_modal("duplicate", &modal, Duration::from_secs(600));

    // The handmade mod is disabled last, so once it is the copy is complete
    wait_for("handmade mod turned off in the copy", Duration::from_secs(30), {
        let disabled = copy_mods.join("handmade-test-mod.jar.disabled");
        Box::new(move || disabled.is_file())
    });

    let copy_after = listing(&copy_mods);
    print_listing("copy mods", &copy_after);

    println!("\n== check the copy");
    let copy_info = read_info(&instances_dir.join(&copy_name));
    assert_eq!(copy_info["minecraft_version"], to_version, "the copy should be on the new version");
    assert!(copy_info.get("preferred_loader_version").is_none(), "the loader version should be reset");
    println!("  ok: info_v1.json is on {to_version}");

    let enabled: Vec<&String> = copy_after.keys().filter(|name| name.ends_with(".jar")).collect();
    assert_eq!(
        enabled.len(),
        4,
        "the four mods Modrinth or CurseForge know should have been replaced: {enabled:?}"
    );
    assert!(
        !copy_after.contains_key("dropped-in-by-hand.jar"),
        "the mod added by hand should have been found on Modrinth by its hash and replaced"
    );
    for name in &enabled {
        assert!(
            !source_before.contains_key(*name),
            "{name} is still the {from_version} file, it should have been replaced"
        );
        let hash = &copy_after[*name];
        assert!(
            !source_before.values().any(|source_hash| source_hash == hash),
            "{name} has the same contents as a {from_version} mod"
        );
    }
    println!("  ok: every downloaded mod was swapped for a different file");

    let unversioned: Vec<_> = enabled.iter().filter(|name| !name.contains(to_version.as_str())).collect();
    if !unversioned.is_empty() {
        println!("  note: these replacements don't name {to_version} in their file name: {unversioned:?}");
    }

    let warnings = seen.lock().unwrap().warnings.clone();
    assert!(
        warnings.iter().any(|warning| warning.contains("Handmade Test Mod")),
        "the user should be told which mods were turned off: {warnings:?}"
    );
    println!("  ok: turned-off mods were reported");

    println!("\n== check the original");
    assert_eq!(listing(&source_mods), source_before, "the original's mods must not change");
    let source_info = read_info(&instances_dir.join(&source_name));
    assert_eq!(source_info["minecraft_version"], from_version, "the original must stay on its version");
    println!("  ok: original untouched");

    let errors = seen.lock().unwrap().errors.clone();
    assert!(errors.is_empty(), "backend reported errors: {errors:?}");

    println!("\nALL OK");
    std::process::exit(0);
}

/// File name to sha1 of everything in a mods folder.
fn listing(dir: &Path) -> BTreeMap<String, String> {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|entry| {
            let entry = entry.ok()?;
            if !entry.file_type().ok()?.is_file() {
                return None;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            // Pandora's own sidecar files aren't content
            if name.starts_with('.') {
                return None;
            }
            let bytes = std::fs::read(entry.path()).ok()?;
            Some((name, hex::encode(sha1::Sha1::digest(&bytes))))
        })
        .collect()
}

fn print_listing(what: &str, listing: &BTreeMap<String, String>) {
    println!("  {what}:");
    for (name, hash) in listing {
        println!("    {name} ({})", &hash[..8]);
    }
}

fn read_info(instance_dir: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(instance_dir.join("info_v1.json")).unwrap()).unwrap()
}
