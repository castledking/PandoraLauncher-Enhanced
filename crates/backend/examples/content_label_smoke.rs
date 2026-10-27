//! Drives content labels end to end against a real Fabric server, without the GUI: a label lands
//! in the mod's auxiliary file and comes back in the content list, can still be changed while the
//! server runs (when the live mods folder is a throwaway copy and the real one waits in
//! `original_mods/`), survives the server stopping, and leaves no file behind once cleared.
//! Downloads a real server jar and Java runtime on first run, so it is an example rather than a
//! test.
//!
//! cargo run -p backend --example content_label_smoke -- <a Fabric mod jar> [minecraft version]

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use bridge::{
    instance::{ContentFolder, InstanceContentSummary, InstanceID, InstanceStatus},
    message::{BridgeDataLoadState, MessageToBackend, MessageToFrontend},
    modal_action::ModalAction,
    quit::QuitCoordinator,
};
use schema::{
    content_label::{ContentLabel, ContentLabelEdit, HexColor},
    server::{ServerPlatform, ServerProperty},
};

#[derive(Default)]
struct Seen {
    server_id: Option<InstanceID>,
    mods_load_state: Option<BridgeDataLoadState>,
    status: Option<InstanceStatus>,
    mods: Option<Arc<[InstanceContentSummary]>>,
    errors: Vec<String>,
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mod_jar = PathBuf::from(args.next().expect("usage: content_label_smoke <a Fabric mod jar> [version]"));
    let version = args.next().unwrap_or_else(|| "1.21.1".into());
    let jar_name = mod_jar.file_name().unwrap().to_string_lossy().into_owned();

    let launcher_dir = std::env::temp_dir().join("pandora_content_label_smoke");
    _ = std::fs::create_dir_all(&launcher_dir);
    println!("launcher dir: {}", launcher_dir.display());

    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    let helper = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();

    let (backend_recv, backend_handle, mut frontend_recv, frontend_handle) = bridge::handle::create_pair();
    let quit = QuitCoordinator::new(Box::new(|| {}));
    backend::start(runtime, launcher_dir.clone(), frontend_handle, backend_handle.clone(), backend_recv, quit);

    let name = format!("smoke-labels-{version}");
    let seen = Arc::new(Mutex::new(Seen::default()));

    // The backend talks to the frontend over a bounded channel in debug builds, so it has to be
    // drained or the backend stalls
    {
        let seen = seen.clone();
        let name = name.clone();
        helper.spawn(async move {
            while let Some(message) = frontend_recv.recv().await {
                match message {
                    MessageToFrontend::InstanceAdded {
                        id,
                        name: added,
                        content_states,
                        ..
                    } if added.as_str() == name => {
                        let mut seen = seen.lock().unwrap();
                        seen.server_id = Some(id);
                        seen.mods_load_state = Some(content_states[ContentFolder::Mods].clone());
                    },
                    MessageToFrontend::InstanceModified { id, status, .. } => {
                        let mut seen = seen.lock().unwrap();
                        if seen.server_id == Some(id) {
                            if seen.status != Some(status) {
                                println!("  status -> {status:?}");
                            }
                            seen.status = Some(status);
                        }
                    },
                    MessageToFrontend::InstanceContentUpdated {
                        id,
                        content_folder: ContentFolder::Mods,
                        content,
                    } => {
                        let mut seen = seen.lock().unwrap();
                        if seen.server_id == Some(id) {
                            seen.mods = Some(content);
                        }
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

    let step = |label: &str| println!("\n== {label}");
    // What the content page does on every render while it's open: the backend only reloads a
    // folder that changed on disk while somebody is looking at it
    let observe_mods = {
        let seen = seen.clone();
        let backend_handle = backend_handle.clone();
        move || {
            let seen = seen.lock().unwrap();
            if let (Some(id), Some(load_state)) = (seen.server_id, &seen.mods_load_state) {
                load_state.set_observed();
                if load_state.should_load() {
                    backend_handle.send(MessageToBackend::RequestLoadContentFolder {
                        id,
                        content_folder: ContentFolder::Mods,
                    });
                }
            }
        }
    };
    let wait_for = |what: &str, timeout: Duration, mut done: Box<dyn FnMut() -> bool>| {
        let start = Instant::now();
        while !done() {
            if start.elapsed() > timeout {
                panic!("timed out waiting for {what}");
            }
            observe_mods();
            std::thread::sleep(Duration::from_millis(100));
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
    // The mod as the content list currently has it
    let listed_mod = {
        let seen = seen.clone();
        let jar_name = jar_name.clone();
        move || -> Option<InstanceContentSummary> {
            let seen = seen.lock().unwrap();
            seen.mods.as_ref()?.iter().find(|summary| summary.filename.as_ref() == jar_name).cloned()
        }
    };
    let wait_for_listed_label = |what: &str, expected: ContentLabel| {
        let listed_mod = listed_mod.clone();
        wait_for(
            what,
            Duration::from_secs(10),
            Box::new(move || listed_mod().is_some_and(|summary| summary.label == expected)),
        );
    };
    let read_aux = |path: &Path| -> Option<serde_json::Value> {
        std::fs::read(path).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok())
    };

    step(&format!("create Fabric {version} server '{name}'"));
    let modal = ModalAction::default();
    backend_handle.send(MessageToBackend::CreateServerInstance {
        name: name.as_str().into(),
        version: version.as_str().into(),
        platform: ServerPlatform::Fabric,
        eula_accepted: true,
        group: None,
        generate_from: None,
        custom_jar: None,
        modal_action: modal.clone(),
    });
    wait_modal("create", &modal, Duration::from_secs(30));
    wait_for("instance added", Duration::from_secs(10), {
        let seen = seen.clone();
        Box::new(move || seen.lock().unwrap().server_id.is_some())
    });
    let id = seen.lock().unwrap().server_id.unwrap();

    let instance_dir = launcher_dir.join("instances").join(&name);
    let mods_dir = instance_dir.join(".minecraft").join("mods");
    let backup_dir = instance_dir.join("original_mods");
    std::fs::create_dir_all(&mods_dir).unwrap();
    std::fs::copy(&mod_jar, mods_dir.join(&jar_name)).unwrap();

    step("list the mods");
    wait_for("mod listed", Duration::from_secs(20), {
        let listed_mod = listed_mod.clone();
        Box::new(move || listed_mod().is_some())
    });
    let summary = listed_mod().unwrap();
    let mod_id = summary.content_summary.id.clone().expect("the jar should have a mod id");
    let aux_name = format!(".{mod_id}.aux.json");
    println!("  {jar_name} is mod '{mod_id}', labelled in {aux_name}");
    assert!(summary.label.is_empty(), "a new mod shouldn't have a label");

    step("put it in a category while the server is stopped");
    backend_handle.send(MessageToBackend::SetContentLabel {
        id,
        content_ids: vec![summary.id],
        edit: ContentLabelEdit::SetCategory(Some("library".into())),
    });
    let library = ContentLabel {
        category: Some("library".into()),
        color: None,
        ..Default::default()
    };
    wait_for_listed_label("content list shows the category", library.clone());
    let aux = read_aux(&mods_dir.join(&aux_name)).expect("aux file in mods/");
    assert_eq!(aux["label"]["category"], "library");
    println!("  ok: {aux}");

    // Picks a port nothing is using, so the server can actually come up
    let port = std::net::TcpListener::bind("0.0.0.0:0").unwrap().local_addr().unwrap().port();
    backend_handle.send(MessageToBackend::SetServerProperties {
        id,
        properties: vec![ServerProperty {
            key: "server-port".into(),
            value: port.to_string().into(),
        }]
        .into(),
    });

    step("start the server (downloads the server and Java on first run)");
    let modal = ModalAction::default();
    backend_handle.send(MessageToBackend::StartServer {
        id,
        modal_action: modal.clone(),
    });
    wait_modal("start", &modal, Duration::from_secs(900));
    wait_for("server running with its mods folder frozen", Duration::from_secs(60), {
        let seen = seen.clone();
        let backup_dir = backup_dir.clone();
        Box::new(move || seen.lock().unwrap().status == Some(InstanceStatus::Running) && backup_dir.is_dir())
    });
    assert!(
        !mods_dir.join(&aux_name).exists(),
        "the live mods folder is a copy of the mods alone, without their aux files"
    );

    step("give it a highlight colour while the server runs");
    let summary = listed_mod().unwrap();
    backend_handle.send(MessageToBackend::SetContentLabel {
        id,
        content_ids: vec![summary.id],
        edit: ContentLabelEdit::SetColor(Some(HexColor::new(0xef4444))),
    });
    let library_red = ContentLabel {
        category: Some("library".into()),
        color: Some(HexColor::new(0xef4444)),
        ..Default::default()
    };
    wait_for_listed_label("content list shows the colour while running", library_red.clone());
    let aux = read_aux(&backup_dir.join(&aux_name)).expect("aux file in original_mods/");
    assert_eq!(aux["label"]["category"], "library");
    assert_eq!(aux["label"]["color"], "#ef4444");
    println!("  ok: written to original_mods/: {aux}");

    step("stop the server");
    backend_handle.send(MessageToBackend::StopServer { id });
    wait_for("server stopped and mods folder restored", Duration::from_secs(180), {
        let seen = seen.clone();
        let backup_dir = backup_dir.clone();
        Box::new(move || seen.lock().unwrap().status == Some(InstanceStatus::NotRunning) && !backup_dir.exists())
    });
    let aux = read_aux(&mods_dir.join(&aux_name)).expect("aux file back in mods/");
    assert_eq!(aux["label"]["color"], "#ef4444", "the change made while running was lost: {aux}");
    println!("  ok: survived the stop: {aux}");
    wait_for_listed_label("content list still shows both after the stop", library_red);

    step("clear the label");
    let summary = listed_mod().unwrap();
    backend_handle.send(MessageToBackend::SetContentLabel {
        id,
        content_ids: vec![summary.id],
        edit: ContentLabelEdit::Clear,
    });
    wait_for_listed_label(
        "content list shows an explicit local removal",
        ContentLabel {
            category_override: true,
            color_override: true,
            ..Default::default()
        },
    );
    assert!(
        mods_dir.join(&aux_name).exists(),
        "a local removal must survive future global assignments"
    );
    backend_handle.send(MessageToBackend::SetContentLabel {
        id,
        content_ids: vec![summary.id],
        edit: ContentLabelEdit::UseGlobal,
    });
    wait_for_listed_label("content list uses global defaults again", ContentLabel::default());
    assert!(!mods_dir.join(&aux_name).exists(), "an empty aux file should be removed");
    println!("  ok: aux file removed");

    let errors = seen.lock().unwrap().errors.clone();
    assert!(errors.is_empty(), "backend reported errors: {errors:?}");

    println!("\nALL OK");
    std::process::exit(0);
}
