//! Drives the real server flow end to end without the GUI: create a server, start it, talk to its
//! console, edit server.properties, then stop it. Downloads a real server jar and Java runtime, so
//! it is an example rather than a test.
//!
//! cargo run -p backend --example server_smoke -- [paper|purpur|vanilla|fabric] [minecraft version]

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use bridge::{
    instance::{InstanceID, InstanceStatus},
    message::{MessageToBackend, MessageToFrontend},
    modal_action::ModalAction,
    quit::QuitCoordinator,
};
use schema::server::{ServerPlatform, ServerProperty};

#[derive(Default)]
struct Seen {
    server_id: Option<InstanceID>,
    ids_by_name: std::collections::HashMap<String, InstanceID>,
    status: Option<InstanceStatus>,
    errors: Vec<String>,
}

fn main() {
    let mut args = std::env::args().skip(1);
    let first = args.next();
    let sync_mode = first.as_deref() == Some("sync");
    let platform = if sync_mode {
        ServerPlatform::Fabric
    } else {
        first.and_then(|name| ServerPlatform::from_name(&name)).unwrap_or(ServerPlatform::Paper)
    };
    let version = args.next().unwrap_or_else(|| "1.21.1".into());

    // Sync mode never starts a server, so it gets its own folder and can run alongside the others
    let launcher_dir = std::env::temp_dir().join(if sync_mode {
        "pandora_server_smoke_sync"
    } else {
        "pandora_server_smoke"
    });
    _ = std::fs::create_dir_all(&launcher_dir);
    println!("launcher dir: {}", launcher_dir.display());

    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    let helper = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();

    let (backend_recv, backend_handle, mut frontend_recv, frontend_handle) = bridge::handle::create_pair();
    let quit = QuitCoordinator::new(Box::new(|| {}));

    backend::start(runtime, launcher_dir.clone(), frontend_handle, backend_handle.clone(), backend_recv, quit);

    let name = if sync_mode {
        format!("smoke-generated-{version}")
    } else {
        format!("smoke-{}-{}", platform.pretty_name().to_lowercase(), version)
    };
    let seen = Arc::new(Mutex::new(Seen::default()));

    // The backend talks to the frontend over a bounded channel in debug builds, so it has to be
    // drained or the backend stalls
    {
        let seen = seen.clone();
        let name = name.clone();
        helper.spawn(async move {
            while let Some(message) = frontend_recv.recv().await {
                match message {
                    MessageToFrontend::InstanceAdded { id, name: added, .. } => {
                        let mut seen = seen.lock().unwrap();
                        seen.ids_by_name.insert(added.to_string(), id);
                        if added.as_str() == name {
                            seen.server_id = Some(id);
                        }
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

    if sync_mode {
        run_sync_scenario(&backend_handle, &seen, &launcher_dir, &name, &version, &helper);
        println!("\nALL OK");
        std::process::exit(0);
    }

    step(&format!("create {} {} server '{}'", platform.pretty_name(), version, name));
    let modal = ModalAction::default();
    backend_handle.send(MessageToBackend::CreateServerInstance {
        name: name.as_str().into(),
        version: version.as_str().into(),
        platform,
        eula_accepted: true,
        group: None,
        generate_from: None,
        modal_action: modal.clone(),
    });
    wait_modal("create", &modal, Duration::from_secs(30));
    wait_for("instance added", Duration::from_secs(10), {
        let seen = seen.clone();
        Box::new(move || seen.lock().unwrap().server_id.is_some())
    });
    let id = seen.lock().unwrap().server_id.unwrap();

    let server_dir: PathBuf = launcher_dir.join("instances").join(&name).join(".minecraft");
    assert!(server_dir.join("eula.txt").is_file(), "eula.txt should exist once the EULA is accepted");
    println!("  ok: eula.txt written");

    // Picks a port nothing is using, then holds it so the first start is guaranteed to clash
    let blocker = std::net::TcpListener::bind("0.0.0.0:0").expect("free port");
    let port = blocker.local_addr().unwrap().port();
    println!("  using port {port}");

    step("set server-port before the first start");
    backend_handle.send(MessageToBackend::SetServerProperties {
        id,
        properties: vec![ServerProperty {
            key: "server-port".into(),
            value: port.to_string().into(),
        }]
        .into(),
    });
    wait_for("server.properties seeded", Duration::from_secs(10), {
        let path = server_dir.join("server.properties");
        Box::new(move || {
            std::fs::read_to_string(&path)
                .map(|text| text.contains(&format!("server-port={port}")))
                .unwrap_or(false)
        })
    });

    step("start while the port is taken (downloads server jar and Java on first run)");
    let modal = ModalAction::default();
    backend_handle.send(MessageToBackend::StartServer {
        id,
        modal_action: modal.clone(),
    });
    wait_modal("start", &modal, Duration::from_secs(900));
    wait_for("port clash reported to the user", Duration::from_secs(300), {
        let seen = seen.clone();
        Box::new(move || seen.lock().unwrap().errors.iter().any(|error| error.contains("already in use")))
    });
    wait_for("server exited after the clash", Duration::from_secs(120), {
        let seen = seen.clone();
        Box::new(move || seen.lock().unwrap().status == Some(InstanceStatus::NotRunning))
    });
    seen.lock().unwrap().errors.clear();
    drop(blocker);

    step("start again with the port free");
    let modal = ModalAction::default();
    backend_handle.send(MessageToBackend::StartServer {
        id,
        modal_action: modal.clone(),
    });
    wait_modal("start", &modal, Duration::from_secs(300));

    step("attach console and wait for the server to finish starting");
    let (send, recv) = tokio::sync::oneshot::channel();
    backend_handle.send(MessageToBackend::SubscribeServerConsole { id, channel: send });
    let (history, mut live) = helper.block_on(recv).expect("console subscription");
    println!("  scrollback lines at attach: {}", history.len());

    let lines = Arc::new(Mutex::new(Vec::<String>::new()));
    for message in history {
        lines.lock().unwrap().extend(message.text.iter().map(|line| line.to_string()));
    }
    let console_closed = Arc::new(Mutex::new(false));
    {
        let lines = lines.clone();
        let console_closed = console_closed.clone();
        helper.spawn(async move {
            while let Some(message) = live.recv().await {
                for line in message.text.iter() {
                    lines.lock().unwrap().push(line.to_string());
                }
            }
            *console_closed.lock().unwrap() = true;
        });
    }
    let console_contains = |needle: &'static str| {
        let lines = lines.clone();
        Box::new(move || lines.lock().unwrap().iter().any(|line| line.contains(needle))) as Box<dyn FnMut() -> bool>
    };
    wait_for("'Done (' in console", Duration::from_secs(600), console_contains("Done ("));

    step("send a command");
    backend_handle.send(MessageToBackend::SendServerCommand {
        id,
        command: "list".into(),
    });
    wait_for("command echoed", Duration::from_secs(10), console_contains("> list"));
    wait_for("server answered 'list'", Duration::from_secs(20), console_contains("players online"));

    step("read and edit server.properties");
    let (send, recv) = tokio::sync::oneshot::channel();
    backend_handle.send(MessageToBackend::GetServerProperties { id, channel: send });
    let mut properties: Vec<ServerProperty> = helper.block_on(recv).unwrap();
    println!("  {} properties", properties.len());
    assert!(properties.iter().any(|property| &*property.key == "max-players"));
    properties.iter_mut().find(|property| &*property.key == "motd").unwrap().value = "Pandora smoke test".into();
    backend_handle.send(MessageToBackend::SetServerProperties {
        id,
        properties: properties.into(),
    });
    wait_for("motd written to disk", Duration::from_secs(10), {
        let path = server_dir.join("server.properties");
        Box::new(move || {
            std::fs::read_to_string(&path)
                .map(|text| text.contains("motd=Pandora smoke test"))
                .unwrap_or(false)
        })
    });

    step("stop");
    backend_handle.send(MessageToBackend::StopServer { id });
    wait_for("console closed after stop", Duration::from_secs(120), {
        let console_closed = console_closed.clone();
        Box::new(move || *console_closed.lock().unwrap())
    });
    wait_for("status back to NotRunning", Duration::from_secs(30), {
        let seen = seen.clone();
        Box::new(move || seen.lock().unwrap().status == Some(InstanceStatus::NotRunning))
    });
    assert!(
        lines
            .lock()
            .unwrap()
            .iter()
            .any(|line| line.contains("Stopping the server") || line.contains("Stopping server")),
        "the server should have shut down cleanly via the stop command"
    );
    println!("  ok: clean shutdown logged");

    let errors = seen.lock().unwrap().errors.clone();
    assert!(errors.is_empty(), "backend reported errors: {errors:?}");

    println!("\nALL OK");
    std::process::exit(0);
}

/// Generates a Fabric server from a Fabric client instance and checks that shared mods are copied,
/// client-only and disabled mods are left behind, and the pairing is recorded.
fn run_sync_scenario(
    backend_handle: &bridge::handle::BackendHandle,
    seen: &Arc<Mutex<Seen>>,
    launcher_dir: &std::path::Path,
    server_name: &str,
    version: &str,
    helper: &tokio::runtime::Runtime,
) {
    use std::io::Write;

    let wait = |what: &str, timeout: Duration, mut done: Box<dyn FnMut() -> bool>| {
        let start = Instant::now();
        while !done() {
            if start.elapsed() > timeout {
                panic!("timed out waiting for {what}");
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        println!("  ok: {what} ({:.1}s)", start.elapsed().as_secs_f32());
    };
    let id_of = |name: &str| seen.lock().unwrap().ids_by_name.get(name).copied();

    let client_name = format!("smoke-client-{version}");
    println!("\n== create client instance '{client_name}'");
    backend_handle.send(MessageToBackend::CreateInstance {
        name: client_name.as_str().into(),
        version: version.into(),
        loader: schema::loader::Loader::Fabric,
        icon: None,
        group: None,
    });
    wait("client instance added", Duration::from_secs(15), {
        let seen = seen.clone();
        let client_name = client_name.clone();
        Box::new(move || seen.lock().unwrap().ids_by_name.contains_key(&client_name))
    });
    let client_id = id_of(&client_name).unwrap();

    let client_mods = launcher_dir.join("instances").join(&client_name).join(".minecraft").join("mods");
    std::fs::create_dir_all(&client_mods).unwrap();
    let make_jar = |file: &str, environment: &str| {
        let mut zip = zip::ZipWriter::new(std::fs::File::create(client_mods.join(file)).unwrap());
        zip.start_file("fabric.mod.json", zip::write::SimpleFileOptions::default()).unwrap();
        write!(zip, r#"{{"id":"{file}","version":"1","environment":"{environment}"}}"#).unwrap();
        zip.finish().unwrap();
    };
    make_jar("shared-mod.jar", "*");
    make_jar("client-mod.jar", "client");
    make_jar("switched-off.jar.disabled", "*");
    println!("  client mods: shared-mod.jar (both), client-mod.jar (client), switched-off.jar.disabled");

    println!("\n== generate server '{server_name}' from it");
    let modal = ModalAction::default();
    backend_handle.send(MessageToBackend::CreateServerInstance {
        name: server_name.into(),
        version: version.into(),
        platform: ServerPlatform::Fabric,
        eula_accepted: true,
        group: None,
        generate_from: Some(client_id),
        modal_action: modal.clone(),
    });
    wait("create and sync finished", Duration::from_secs(30), {
        let modal = modal.clone();
        Box::new(move || modal.get_finished_at().is_some())
    });
    if let Some(error) = modal.get_error_message() {
        panic!("create failed: {error}");
    }

    let server_root = launcher_dir.join("instances").join(server_name);
    let server_mods = server_root.join(".minecraft").join("mods");
    assert!(server_mods.join("shared-mod.jar").is_file(), "the shared mod should be copied");
    assert!(!server_mods.join("client-mod.jar").exists(), "the client-only mod should be left out");
    assert!(
        !server_mods.join("switched-off.jar.disabled").exists(),
        "disabled mods should be left out"
    );
    println!("  ok: shared mod copied, client-only and disabled mods left out");

    let info = std::fs::read_to_string(server_root.join("info_v1.json")).unwrap();
    assert!(
        info.contains(&format!("\"linked_instance\":\"{client_name}\"")),
        "pairing not recorded: {info}"
    );
    println!("  ok: linked_instance recorded");

    println!("\n== plan again and check the review list");
    let server_id = id_of(server_name).expect("server loaded");
    let (send, recv) = tokio::sync::oneshot::channel();
    backend_handle.send(MessageToBackend::PlanServerSync {
        server_id,
        client_id,
        channel: send,
    });
    let plan = helper.block_on(recv).unwrap().expect("plan");
    for entry in plan.mods.iter() {
        println!(
            "  {:<22} {:<18} selected={} on_server={} only_on_server={}",
            entry.filename,
            entry.side.label(),
            entry.selected,
            entry.already_on_server,
            entry.only_on_server
        );
    }
    let client_mod = plan.mods.iter().find(|entry| &*entry.filename == "client-mod.jar").unwrap();
    assert!(!client_mod.selected && !client_mod.already_on_server);
    let shared = plan.mods.iter().find(|entry| &*entry.filename == "shared-mod.jar").unwrap();
    assert!(shared.selected && shared.already_on_server);
    assert!(plan.version_mismatch.is_none() && plan.loader_mismatch.is_none());
    println!("  ok: plan matches");
}
