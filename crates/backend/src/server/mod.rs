mod list;
mod provision;
mod sync;

use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};

use bridge::{
    instance::InstanceID,
    message::{GameOutputMsg, MessageToFrontend},
    modal_action::{ModalAction, ProgressTrackerFinishType},
};
use command::{PandoraCommand, PandoraStdioReadMode, PandoraStdioWriteMode};
use parking_lot::Mutex;
use rustc_hash::FxHashMap;
use schema::{
    instance::InstanceConfiguration,
    server::{
        ServerCustomJar, ServerPlatform, ServerProperty, merge_server_properties, parse_server_properties,
        write_server_properties,
    },
};
use tokio::sync::mpsc::UnboundedSender;

use crate::{BackendState, log_reader, metadata::items::MinecraftVersionManifestMetadataItem};

/// How many console lines are kept per server so the console still has scrollback after the user
/// navigates away from the page and back.
const CONSOLE_HISTORY_LINES: usize = 2000;

/// A server the launcher has started, and the pipe needed to talk to it.
pub struct RunningServer {
    /// The server's stdin, which is how commands are delivered. `None` once the pipe is closed.
    stdin: Option<std::io::PipeWriter>,
    /// Set while a stop was asked for, so the exit isn't reported as a crash.
    stopping: bool,
}

/// What a server has printed, and who is watching it.
///
/// Kept apart from [`RunningServer`] so that it outlives the process: a console that is open
/// across a restart stays subscribed and simply carries on with the next session, instead of
/// being left holding a channel that closed with the old process.
#[derive(Default)]
struct ServerConsole {
    history: std::collections::VecDeque<GameOutputMsg>,
    subscribers: Vec<UnboundedSender<GameOutputMsg>>,
}

impl ServerConsole {
    fn push(&mut self, message: GameOutputMsg) {
        if self.history.len() >= CONSOLE_HISTORY_LINES {
            self.history.pop_front();
        }
        self.subscribers.retain(|subscriber| subscriber.send(message.clone()).is_ok());
        self.history.push_back(message);
    }
}

#[derive(Default)]
pub struct ServerRegistry {
    servers: Mutex<FxHashMap<InstanceID, RunningServer>>,
    consoles: Mutex<FxHashMap<InstanceID, ServerConsole>>,
    pending_restarts: Mutex<FxHashMap<InstanceID, u64>>,
    next_restart_token: std::sync::atomic::AtomicU64,
}

impl ServerRegistry {
    fn with<R>(&self, id: InstanceID, f: impl FnOnce(&mut RunningServer) -> R) -> Option<R> {
        self.servers.lock().get_mut(&id).map(f)
    }

    fn console<R>(&self, id: InstanceID, f: impl FnOnce(&mut ServerConsole) -> R) -> R {
        f(self.consoles.lock().entry(id).or_default())
    }

    /// Marks a running server as intentionally stopping and cancels any pending manual restart, so
    /// killing it bypasses the restart-on-crash setting and never brings it back up.
    pub(crate) fn mark_stopping(&self, id: InstanceID) {
        if let Some(server) = self.servers.lock().get_mut(&id) {
            server.stopping = true;
        }
        self.pending_restarts.lock().remove(&id);
    }

    fn begin_restart(&self, id: InstanceID) -> u64 {
        let token = self.next_restart_token.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        self.pending_restarts.lock().insert(id, token);
        token
    }

    fn take_restart_if_current(&self, id: InstanceID, token: u64) -> bool {
        let mut guard = self.pending_restarts.lock();
        if guard.get(&id).copied() == Some(token) {
            guard.remove(&id);
            true
        } else {
            false
        }
    }
}

impl BackendState {
    pub fn is_server_running(&self, id: InstanceID) -> bool {
        self.servers.servers.lock().contains_key(&id)
    }

    /// Starts a server instance: makes sure the jar is present, the EULA is accepted, and then
    /// spawns it with its stdio piped so the launcher can show the console and send commands.
    pub async fn start_server(self: &Arc<Self>, id: InstanceID, modal_action: ModalAction) {
        if self.is_server_running(id) {
            self.send.send_warning("That server is already running");
            modal_action.set_finished();
            return;
        }

        let Some((server_dir, configuration, name)) =
            self.instance_state.write().instances.get_mut(id).map(|instance| {
                (instance.dot_minecraft_path.clone(), instance.configuration.get().clone(), instance.name)
            })
        else {
            modal_action.set_finished_with_error("Unable to find that server".into());
            return;
        };

        let Some(server) = configuration.server.clone() else {
            modal_action.set_finished_with_error("That instance is not a server".into());
            return;
        };

        // The global defaults from the Java settings page apply to servers just as they do to
        // client instances, unless the server overrides them
        let mut configuration = configuration;
        crate::launch::apply_global_launch_defaults(&mut configuration, self.config.lock().get());

        if !server.eula_accepted {
            modal_action
                .set_finished_with_error("The Minecraft EULA has to be accepted before this server can start".into());
            return;
        }

        _ = std::fs::create_dir_all(&server_dir);
        write_eula(&server_dir);

        let provisioned = match self.provision_server(&server_dir, &configuration, &modal_action).await {
            Ok(provisioned) => provisioned,
            Err(err) => {
                modal_action.set_finished_with_error(format!("{err}").into());
                return;
            },
        };

        // Remember the resolved build so the UI can show it and the same jar is reused next time
        if provisioned.build != server.build
            && let Some(instance) = self.instance_state.write().instances.get_mut(id)
        {
            instance.configuration.modify(|cfg| {
                if let Some(server) = &mut cfg.server {
                    server.build = provisioned.build;
                }
            });
            self.send.send(instance.create_modify_message());
        }

        // Expands any modpacks in mods/ into their individual mods, exactly as a client launch
        // does. File syncing is deliberately skipped: it carries options and client state that a
        // server has no use for.
        self.prelaunch_setup_mods(id, &modal_action).await;

        let launch_tracker = modal_action.push_tracker("Starting server".into());

        let jar_path = (!provisioned.jar.is_empty()).then(|| server_dir.join(&*provisioned.jar));
        let java = match self
            .resolve_server_java_binary(&configuration, jar_path.as_deref(), Some(&modal_action))
            .await
        {
            Ok(java) => java,
            Err(err) => {
                launch_tracker.set_finished(ProgressTrackerFinishType::from_err(true));
                self.undo_server_prelaunch(id);
                modal_action.set_finished_with_error(err.into());
                return;
            },
        };

        let mut command = PandoraCommand::new(java.into_os_string());
        command.current_dir(&server_dir);

        if let Some(memory) = &configuration.memory
            && memory.enabled
        {
            command.arg(std::ffi::OsString::from(format!("-Xms{}M", memory.min)));
            command.arg(std::ffi::OsString::from(format!("-Xmx{}M", memory.max)));
        }

        if let Some(jvm_flags) = &configuration.jvm_flags
            && jvm_flags.enabled
        {
            let split = shell_words::split(&jvm_flags.flags)
                .unwrap_or_else(|_| jvm_flags.flags.split_whitespace().map(|flag| flag.to_string()).collect());
            for flag in split {
                command.arg(std::ffi::OsString::from(flag));
            }
        }

        for arg in &provisioned.extra_args {
            command.arg(std::ffi::OsString::from(&**arg));
        }

        if !provisioned.jar.is_empty() {
            command.arg(std::ffi::OsString::from("-jar"));
            command.arg(std::ffi::OsString::from(&*provisioned.jar));
        }

        // Servers have no use for the built-in GUI, and it costs memory
        command.arg(std::ffi::OsString::from("nogui"));

        // The console draws colours, but a server only prints them when its environment says
        // something will: Paper strips a plugin's colours unless COLORTERM is set, which it
        // only is when the launcher itself was started from a terminal
        command.env("COLORTERM", "truecolor");

        command.stdin(PandoraStdioWriteMode::Pipe);
        command.stdout(PandoraStdioReadMode::Pipe);
        command.stderr(PandoraStdioReadMode::Pipe);

        let mut child = match command.spawn().await {
            Ok(child) => child,
            Err(err) => {
                log::error!("Unable to start server {name}: {err}");
                launch_tracker.set_finished(ProgressTrackerFinishType::from_err(true));
                self.undo_server_prelaunch(id);
                modal_action.set_finished_with_error(format!("Unable to start the server: {err}").into());
                return;
            },
        };

        let stdin = child.stdin.take();
        let output = child
            .stdout
            .take()
            .map(|stdout| log_reader::start_plain_output(stdout, child.stderr.take()));

        // The scrollback starts over with the new session. Whoever is watching stays subscribed,
        // so a console left open through a restart goes straight on to the new output.
        self.servers.console(id, |console| console.history.clear());
        self.servers.servers.lock().insert(id, RunningServer { stdin, stopping: false });

        if let Some(instance) = self.instance_state.write().instances.get_mut(id) {
            instance.processes.push(child.process);
            instance.update_session();
            self.quit_coordinator.set_can_quit(false);
        }

        launch_tracker.set_finished(ProgressTrackerFinishType::Normal);
        modal_action.set_finished();
        self.send.send(MessageToFrontend::Refresh);

        if let Some(mut output) = output {
            let this = self.clone();
            tokio::task::spawn(async move {
                let mut reported_startup_error = false;
                let mut startup_done = false;
                let mut first_error_line: Option<String> = None;
                while let Some(message) = output.recv().await {
                    let message = normalize_server_line(message);

                    if !startup_done {
                        for line in message.text.iter() {
                            if let Some(detected) = parse_server_runtime_line(line) {
                                this.apply_detected_server_runtime(id, detected);
                            }
                            if !reported_startup_error
                                && let Some(startup_error) =
                                    parse_server_startup_error(line, || this.configured_server_port(id))
                            {
                                reported_startup_error = true;
                                this.send.send_error(startup_error);
                            }
                            if first_error_line.is_none()
                                && matches!(
                                    message.level,
                                    bridge::game_output::GameOutputLogLevel::Error
                                        | bridge::game_output::GameOutputLogLevel::Fatal
                                )
                            {
                                let clean = strip_ansi_escapes(line);
                                let trimmed = clean.trim();
                                if !trimmed.is_empty()
                                    && !trimmed.starts_with("at ")
                                    && !trimmed.starts_with("Caused by:")
                                {
                                    first_error_line = Some(trimmed.chars().take(240).collect());
                                }
                            }
                            if line.contains("Done (") && line.contains("For help, type") {
                                startup_done = true;
                                break;
                            }
                        }
                    } else if !reported_startup_error
                        && message.text.iter().any(|line| line.contains("FAILED TO BIND TO PORT"))
                    {
                        reported_startup_error = true;
                        let port = this.configured_server_port(id);
                        this.send.send_error(format!(
                            "Port {port} is already in use, so the server can't start. Another server may be \
                             running; change server-port in this server's Properties tab."
                        ));
                    }

                    this.servers.console(id, |console| console.push(message));
                }

                // stdout closing means the server has exited
                this.on_server_exited(id, !startup_done, reported_startup_error, first_error_line.as_deref());
            });
        }
    }

    /// Waits for the filesystem watcher to load a newly created instance folder, loading it directly
    /// only if the watcher never does.
    pub(crate) async fn wait_for_new_instance(&self, instance_dir: &Path) -> Option<InstanceID> {
        for _ in 0..100 {
            let found = self
                .instance_state
                .read()
                .instances
                .iter()
                .find(|instance| &*instance.root_path == instance_dir)
                .map(|instance| instance.id);
            if found.is_some() {
                return found;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        self.load_instance_from_path(instance_dir, true, false)
    }

    /// Puts the real mods folder back after a start that failed before any process existed, since
    /// nothing else would notice it was ever swapped out.
    fn undo_server_prelaunch(&self, id: InstanceID) {
        if let Some(instance) = self.instance_state.write().instances.get_mut(id) {
            self.restore_mods_folder_if_stopped(instance);
        }
    }

    fn on_server_exited(
        self: &Arc<Self>,
        id: InstanceID,
        exited_during_startup: bool,
        reported_startup_error: bool,
        first_error_line: Option<&str>,
    ) {
        let was_stopping = self.servers.servers.lock().remove(&id).map(|server| server.stopping).unwrap_or(false);

        let (server_name, auto_restart) = self
            .instance_state
            .write()
            .instances
            .get_mut(id)
            .map(|instance| {
                let auto = instance
                    .configuration
                    .get()
                    .server
                    .as_ref()
                    .map(|server| server.auto_restart)
                    .unwrap_or(false);
                (instance.name, auto)
            })
            .unwrap_or_else(|| (ustr::Ustr::from("Server"), false));

        self.send.send(MessageToFrontend::Refresh);

        if exited_during_startup && !was_stopping && !reported_startup_error {
            let message = match first_error_line {
                Some(line) => format!("Server '{server_name}' failed to start: {line}"),
                None => format!(
                    "Server '{server_name}' stopped before finishing startup. Check the Console tab for details."
                ),
            };
            self.send.send_error(message);
        }

        if auto_restart && !was_stopping && !reported_startup_error {
            log::info!("Restarting server {:?} because it stopped and auto restart is on", id);
            self.send.send_info("The server stopped unexpectedly, restarting it");
            // Goes back through the handler because starting a server resolves a Java runtime,
            // which isn't `Send` and so can't run on this task
            self.self_handle.send(bridge::message::MessageToBackend::StartServer {
                id,
                modal_action: ModalAction::default(),
            });
        }
    }

    /// Asks the server to shut down cleanly by sending it the `stop` command, which lets it save
    /// its worlds. Killing the process would risk losing whatever hasn't been written yet.
    pub fn stop_server(self: &Arc<Self>, id: InstanceID) {
        let sent = self
            .servers
            .with(id, |server| {
                server.stopping = true;
                write_command(&mut server.stdin, "stop")
            })
            .unwrap_or(false);

        if !sent {
            self.send.send_warning("Unable to send the stop command to that server");
        }
    }

    /// Stops the server and starts it again once it has gone.
    ///
    /// The wait happens on its own task. Waiting here would hold up the whole message loop for
    /// as long as the server takes to save and exit, and with it the check that notices the old
    /// process has ended, so the UI never saw the server stop and the console was left attached
    /// to the old session.
    pub async fn restart_server(self: &Arc<Self>, id: InstanceID, modal_action: ModalAction) {
        if !self.is_server_running(id) {
            self.start_server(id, modal_action).await;
            return;
        }

        let restart_token = self.servers.begin_restart(id);
        self.stop_server(id);

        let this = self.clone();
        tokio::task::spawn(async move {
            // Give the server time to shut down on its own before starting it again
            for _ in 0..120 {
                if !this.is_server_running(id) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }

            if this.is_server_running(id) {
                this.servers.take_restart_if_current(id, restart_token);
                modal_action.set_finished_with_error("The server did not shut down in time".into());
                return;
            }

            if !this.servers.take_restart_if_current(id, restart_token) {
                modal_action.set_finished();
                return;
            }

            // Back through the handler, since starting a server resolves a Java runtime, which
            // isn't `Send` and so can't run on this task
            this.self_handle.send(bridge::message::MessageToBackend::StartServer { id, modal_action });
        });
    }

    pub fn send_server_command(self: &Arc<Self>, id: InstanceID, command: &str) {
        let command = command.trim();
        if command.is_empty() {
            return;
        }

        if !self.is_server_running(id) {
            return;
        }

        // The console shows what was typed, since the server only echoes commands it recognises
        self.servers.console(id, |console| {
            console.push(GameOutputMsg {
                time: chrono::Utc::now().timestamp_millis(),
                level: bridge::game_output::GameOutputLogLevel::Info,
                text: Arc::new([format!("> {command}").into()]),
            })
        });

        self.servers.with(id, |server| write_command(&mut server.stdin, command));
    }

    /// Hands the frontend the console scrollback plus a channel for everything printed from now
    /// on, so opening the console page mid-session shows the whole session.
    pub fn subscribe_server_console(
        self: &Arc<Self>,
        id: InstanceID,
        channel: tokio::sync::oneshot::Sender<(
            Vec<GameOutputMsg>,
            tokio::sync::mpsc::UnboundedReceiver<GameOutputMsg>,
        )>,
    ) {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let history = self.servers.console(id, |console| {
            console.subscribers.push(sender);
            console.history.iter().cloned().collect::<Vec<_>>()
        });

        _ = channel.send((history, receiver));
    }

    pub fn read_server_properties(self: &Arc<Self>, id: InstanceID) -> Vec<ServerProperty> {
        let Some(server_dir) = self.server_dir(id) else {
            return Vec::new();
        };

        let contents = std::fs::read_to_string(server_dir.join("server.properties")).unwrap_or_default();
        merge_server_properties(&contents)
    }

    pub fn write_server_properties(self: &Arc<Self>, id: InstanceID, properties: &[ServerProperty]) {
        let Some(server_dir) = self.server_dir(id) else {
            return;
        };

        let path = server_dir.join("server.properties");
        let original = std::fs::read_to_string(&path).unwrap_or_default();
        let contents = write_server_properties(&original, properties);

        if let Err(err) = crate::fs::write_safe(&path, contents.as_bytes()) {
            log::error!("Unable to write server.properties: {err}");
            self.send.send_error("Unable to save server.properties");
            return;
        }

        if self.is_server_running(id) {
            self.send.send_info("server.properties saved, restart the server to apply it");
        }
    }

    pub fn accept_server_eula(self: &Arc<Self>, id: InstanceID, accepted: bool) {
        let server_dir = {
            let mut state = self.instance_state.write();
            let Some(instance) = state.instances.get_mut(id) else {
                return;
            };

            instance.configuration.modify(|cfg| {
                if let Some(server) = &mut cfg.server {
                    server.eula_accepted = accepted;
                }
            });
            self.send.send(instance.create_modify_message());
            instance.dot_minecraft_path.clone()
        };

        if accepted {
            _ = std::fs::create_dir_all(&server_dir);
            write_eula(&server_dir);
        }
    }

    /// Copies `jar` into the server directory and runs it from then on instead of the platform's
    /// own jar.
    pub async fn set_server_custom_jar(self: &Arc<Self>, id: InstanceID, jar: &Path) {
        let Some(server_dir) = self.server_dir(id) else {
            return;
        };

        let file = match import_custom_jar(&server_dir, jar).await {
            Ok(file) => file,
            Err(err) => {
                self.send.send_error(err);
                return;
            },
        };

        if let Some(instance) = self.instance_state.write().instances.get_mut(id) {
            instance.configuration.modify(|cfg| {
                if let Some(server) = &mut cfg.server {
                    server.custom_jar = Some(ServerCustomJar { enabled: true, file });
                    server.detected_platform = None;
                }
            });
            self.send.send(instance.create_modify_message());
        }

        if self.is_server_running(id) {
            self.send.send_info("Custom jar set, restart the server to use it");
        }
    }

    /// Switches between the picked custom jar and the platform's own jar.
    pub fn set_server_custom_jar_enabled(self: &Arc<Self>, id: InstanceID, enabled: bool) {
        let mut state = self.instance_state.write();
        let Some(instance) = state.instances.get_mut(id) else {
            return;
        };
        instance.configuration.modify(|cfg| {
            if let Some(server) = cfg.server.as_mut() {
                if let Some(custom_jar) = server.custom_jar.as_mut() {
                    custom_jar.enabled = enabled;
                }
                server.detected_platform = None;
            }
        });
        self.send.send(instance.create_modify_message());
    }

    /// Moves a stopped server to `platform` running Minecraft `version`. The new server is fetched
    /// before anything is saved, so a platform that has no build for that version, or a download
    /// that fails, leaves the server exactly as it was. Worlds, plugins and mods are left where
    /// they are; only what runs them changes.
    pub async fn set_server_software(
        self: &Arc<Self>,
        id: InstanceID,
        platform: ServerPlatform,
        version: ustr::Ustr,
        modal_action: ModalAction,
    ) {
        if self.is_server_running(id) {
            modal_action.set_finished_with_error("Stop the server before changing its software".into());
            return;
        }

        let Some((server_dir, current)) = self
            .instance_state
            .write()
            .instances
            .get_mut(id)
            .map(|instance| (instance.dot_minecraft_path.clone(), instance.configuration.get().clone()))
        else {
            modal_action.set_finished_with_error("Unable to find that server".into());
            return;
        };
        if current.server.is_none() {
            modal_action.set_finished_with_error("That instance is not a server".into());
            return;
        }

        let target = software_switched(&current, platform, version);

        // Provisioned the way a start would, global Java defaults included, since Forge and
        // NeoForge run their installer with that Java
        let mut provision_configuration = target.clone();
        crate::launch::apply_global_launch_defaults(&mut provision_configuration, self.config.lock().get());

        _ = std::fs::create_dir_all(&server_dir);
        let provisioned = match self.provision_server(&server_dir, &provision_configuration, &modal_action).await {
            Ok(provisioned) => provisioned,
            Err(err) => {
                modal_action.set_finished_with_error(format!("{err}").into());
                return;
            },
        };

        if let Some(instance) = self.instance_state.write().instances.get_mut(id) {
            // Only what the switch decides is written, so settings changed meanwhile are kept
            instance.configuration.modify(|cfg| {
                let mut switched = software_switched(cfg, platform, version);
                if let Some(server) = &mut switched.server {
                    server.build = provisioned.build;
                }
                *cfg = switched;
            });
            self.send.send(instance.create_modify_message());
        }

        self.send.send_success(format!("Switched to {} {version}", platform.pretty_name()));
        modal_action.set_finished();
    }

    /// Turns auto restart on or off for a server.
    pub fn set_server_auto_restart(self: &Arc<Self>, id: InstanceID, auto_restart: bool) {
        let mut state = self.instance_state.write();
        let Some(instance) = state.instances.get_mut(id) else {
            return;
        };
        instance.configuration.modify(|cfg| {
            if let Some(server) = &mut cfg.server {
                server.auto_restart = auto_restart;
            }
        });
        self.send.send(instance.create_modify_message());
    }

    /// The port the server is configured to listen on, falling back to Minecraft's default.
    fn configured_server_port(&self, id: InstanceID) -> u16 {
        let Some(server_dir) = self.server_dir(id) else {
            return 25565;
        };
        let contents = std::fs::read_to_string(server_dir.join("server.properties")).unwrap_or_default();
        parse_server_properties(&contents)
            .iter()
            .find(|property| &*property.key == "server-port")
            .and_then(|property| property.value.parse().ok())
            .unwrap_or(25565)
    }

    fn server_dir(&self, id: InstanceID) -> Option<Arc<Path>> {
        self.instance_state
            .read()
            .instances
            .get(id)
            .map(|instance| instance.dot_minecraft_path.clone())
    }

    /// Picks the Java runtime for a server. Honours an explicit `jvm_binary` override; otherwise
    /// determines the required Java version from the server jar's own `version.json` / bytecode
    /// (falling back to Mojang's version manifest and Minecraft version rules), uses Mojang's
    /// managed runtime, and falls back to a matching Java installed on the PC.
    pub(crate) async fn resolve_server_java_binary(
        self: &Arc<Self>,
        configuration: &InstanceConfiguration,
        jar_path: Option<&Path>,
        modal_action: Option<&ModalAction>,
    ) -> Result<PathBuf, String> {
        if let Some(jvm_binary) = &configuration.jvm_binary
            && jvm_binary.enabled
            && let Some(path) = &jvm_binary.path
        {
            return crate::launch::Launcher::search_for_java_binary(path).ok_or_else(|| {
                format!(
                    "The configured Java binary ({}) could not be found. Update it in Server Settings -> Java.",
                    path.display()
                )
            });
        }

        let jar_req = match jar_path {
            Some(path) => {
                let path = path.to_path_buf();
                tokio::task::spawn_blocking(move || inspect_server_jar_java(&path)).await.ok().flatten()
            },
            None => None,
        };

        let mut required_component = jar_req.as_ref().and_then(|req| req.java_component);
        let mut required_major = jar_req.as_ref().and_then(|req| req.java_major);
        let lookup_version = jar_req.as_ref().and_then(|req| req.mc_version).unwrap_or(configuration.minecraft_version);

        if required_component.is_none() || required_major.is_none() {
            if let Ok(manifest) = self.meta.fetch(MinecraftVersionManifestMetadataItem).await
                && let Some(link) = manifest.versions.iter().find(|link| link.id == lookup_version)
                && let Ok(version_info) =
                    self.meta.fetch(crate::metadata::items::MinecraftVersionMetadataItem(link)).await
                && let Some(java_version) = &version_info.java_version
            {
                required_component.get_or_insert(java_version.component);
                required_major.get_or_insert(java_version.major_version);
            }
        }

        let (fallback_component, fallback_major) = default_java_for_minecraft_version(lookup_version.as_str());
        let final_major = required_major.unwrap_or(fallback_major);
        let final_component = required_component
            .or_else(|| component_for_java_major(final_major))
            .unwrap_or(fallback_component);

        self.launcher
            .load_java_binary_for_server(
                &self.meta,
                &self.http_client_provider.client(),
                configuration,
                final_component,
                final_major,
                modal_action,
            )
            .await
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct ServerJarJavaRequirement {
    mc_version: Option<ustr::Ustr>,
    java_component: Option<ustr::Ustr>,
    java_major: Option<u32>,
}

fn component_for_java_major(major: u32) -> Option<ustr::Ustr> {
    Some(ustr::Ustr::from(match major {
        ..=8 => "jre-legacy",
        9..=16 => "java-runtime-alpha",
        17..=20 => "java-runtime-gamma",
        21..=24 => "java-runtime-delta",
        25.. => "java-runtime-epsilon",
    }))
}

/// Maps a Minecraft version string to Mojang's required managed Java component and major version
/// when neither `version.json` nor Mojang's version manifest is available.
fn default_java_for_minecraft_version(version: &str) -> (ustr::Ustr, u32) {
    let mut parts = version.split(|c: char| !c.is_ascii_digit()).filter(|part| !part.is_empty());
    let major: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(1);
    let minor: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let patch: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);

    // 26.x+ (new year-based numbering) requires Java 25; 1.20.5+ requires Java 21;
    // 1.18+ requires Java 17; 1.17 requires Java 16; older requires Java 8.
    let java_major = if major >= 26 {
        25
    } else if major > 1 || (major == 1 && (minor > 20 || (minor == 20 && patch >= 5))) {
        21
    } else if major == 1 && minor >= 18 {
        17
    } else if major == 1 && minor == 17 {
        16
    } else {
        8
    };
    (
        component_for_java_major(java_major).unwrap_or_else(|| "java-runtime-delta".into()),
        java_major,
    )
}

/// Inspects a server jar's `version.json` (present in Vanilla, Paper, Purpur, Folia, Canvas, etc.)
/// and falls back to its `Main-Class` bytecode major version to determine the required Java runtime
/// before spawning the process.
fn inspect_server_jar_java(jar_path: &Path) -> Option<ServerJarJavaRequirement> {
    let file = std::fs::File::open(jar_path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    let mut req = ServerJarJavaRequirement::default();

    if let Ok(mut entry) = archive.by_name("version.json") {
        #[derive(serde::Deserialize)]
        struct JarVersionJson {
            id: Option<String>,
            java_component: Option<String>,
            java_version: Option<u32>,
        }
        let mut buf = Vec::new();
        if std::io::Read::read_to_end(&mut entry, &mut buf).is_ok()
            && let Ok(parsed) = serde_json::from_slice::<JarVersionJson>(&buf)
        {
            if let Some(id) = parsed.id
                && is_valid_mc_version(id.trim())
            {
                req.mc_version = Some(ustr::Ustr::from(id.trim()));
            }
            if let Some(component) = parsed.java_component
                && !component.trim().is_empty()
            {
                req.java_component = Some(ustr::Ustr::from(component.trim()));
            }
            if let Some(major) = parsed.java_version
                && major >= 8
            {
                req.java_major = Some(major);
            }
        }
    }

    // If version.json didn't declare a Java version, read the Main-Class bytecode header (bytes 6..8)
    if req.java_major.is_none() {
        let class_path = {
            let mut manifest_text = String::new();
            let ok = archive
                .by_name("META-INF/MANIFEST.MF")
                .ok()
                .is_some_and(|mut manifest| std::io::Read::read_to_string(&mut manifest, &mut manifest_text).is_ok());
            ok.then(|| {
                manifest_text
                    .lines()
                    .find_map(|line| line.strip_prefix("Main-Class:").map(str::trim))
                    .map(|main_class| format!("{}.class", main_class.replace('.', "/")))
            })
            .flatten()
        };
        if let Some(class_path) = class_path
            && let Ok(mut class_entry) = archive.by_name(&class_path)
        {
            let mut header = [0u8; 8];
            if std::io::Read::read_exact(&mut class_entry, &mut header).is_ok()
                && header[0..4] == [0xCA, 0xFE, 0xBA, 0xBE]
            {
                let class_major = u16::from_be_bytes([header[6], header[7]]) as u32;
                if class_major >= 52 {
                    req.java_major = Some(class_major - 44);
                }
            }
        }
    }

    (req.mc_version.is_some() || req.java_component.is_some() || req.java_major.is_some()).then_some(req)
}

fn parse_class_version_java_major(text: &str) -> Option<u32> {
    let digits: String = text.chars().take_while(|c| c.is_ascii_digit()).collect();
    let class_major: u32 = digits.parse().ok()?;
    (class_major >= 52).then_some(class_major - 44)
}

/// Recognizes fatal JVM/server startup errors so they surface immediately as bottom-right
/// notifications instead of failing silently in the console.
fn parse_server_startup_error(line: &str, configured_port: impl FnOnce() -> u16) -> Option<String> {
    let clean = strip_ansi_escapes(line);
    let line = clean.trim();

    if line.contains("FAILED TO BIND TO PORT") {
        let port = configured_port();
        return Some(format!(
            "Port {port} is already in use, so the server can't start. Another server may be running; change \
             server-port in this server's Properties tab."
        ));
    }

    if line.contains("UnsupportedClassVersionError") {
        let required = line
            .split_once("class file version ")
            .and_then(|(_, rest)| parse_class_version_java_major(rest));
        let actual = line
            .split_once("class file versions up to ")
            .and_then(|(_, rest)| parse_class_version_java_major(rest));
        return Some(match (required, actual) {
            (Some(req), Some(act)) => format!(
                "This server requires Java {req}, but started with Java {act}. Set Java to Automatic or select Java \
                 {req} in Server Settings -> Java."
            ),
            (Some(req), None) => format!(
                "This server requires Java {req}. Set Java to Automatic or select Java {req} in Server Settings -> \
                 Java."
            ),
            _ => "This server requires a newer Java version than the one it started with. Set Java to Automatic or \
                  pick a newer runtime in Server Settings -> Java."
                .to_string(),
        });
    }

    if line.contains("Unsupported Java detected")
        || line.contains("requires Java ")
        || line.contains("Cannot run with Java")
    {
        return Some(format!("{line} Change the Java runtime in Server Settings -> Java."));
    }

    if line.contains("Invalid or corrupt jarfile") || line.contains("Unable to access jarfile") {
        return Some(line.to_string());
    }

    if line.contains("Unrecognized VM option") || line.contains("Could not create the Java Virtual Machine") {
        return Some(format!("{line}. Check JVM flags in Server Settings -> Java."));
    }

    if line.contains("Invalid maximum heap size")
        || line.contains("Initial heap size set to a larger value")
        || line.contains("Could not reserve enough space")
    {
        return Some(format!("{line}. Check memory settings in Server Settings -> Memory."));
    }

    None
}

/// `configuration` moved to `platform` and `version`, with everything that belonged to the old
/// software cleared: the resolved build, a preferred loader version (Forge's are tied to one
/// Minecraft version) and, if the loader changed, the client instance mods were synced from.
/// A custom jar is switched off but kept, so it is still there to turn back on.
fn software_switched(
    configuration: &InstanceConfiguration,
    platform: ServerPlatform,
    version: ustr::Ustr,
) -> InstanceConfiguration {
    let mut switched = configuration.clone();
    let loader = platform.loader();
    let changed = switched.minecraft_version != version
        || switched.loader != loader
        || switched.server.as_ref().map(|server| server.platform) != Some(platform);

    if switched.loader != loader
        && let Some(server) = &mut switched.server
    {
        server.linked_instance = None;
    }
    switched.minecraft_version = version;
    switched.loader = loader;
    if changed {
        switched.preferred_loader_version = None;
    }
    if let Some(server) = &mut switched.server {
        if changed {
            server.build = None;
        }
        server.platform = platform;
        server.detected_platform = None;
        if let Some(custom_jar) = &mut server.custom_jar {
            custom_jar.enabled = false;
        }
    }
    switched
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DetectedSoftwareKind {
    Known(ServerPlatform),
    PluginFork,
    ModFork(ServerPlatform),
}

#[derive(Debug, Default, PartialEq, Eq)]
struct DetectedServerRuntime {
    software: Option<(String, DetectedSoftwareKind)>,
    version: Option<String>,
}

fn strip_ansi_escapes(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.as_bytes().contains(&0x1b) {
        return std::borrow::Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            for c in chars.by_ref() {
                if ('@'..='~').contains(&c) {
                    break;
                }
            }
        } else {
            out.push(ch);
        }
    }
    std::borrow::Cow::Owned(out)
}

fn is_valid_software_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name.as_bytes()[0].is_ascii_alphabetic()
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '+'))
}

fn is_valid_mc_version(ver: &str) -> bool {
    !ver.is_empty()
        && ver.len() <= 32
        && ver.as_bytes()[0].is_ascii_digit()
        && ver.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

fn classify_plugin_software(raw: &str) -> Option<(String, DetectedSoftwareKind)> {
    let trimmed = raw.trim();
    if !is_valid_software_name(trimmed) {
        return None;
    }
    if let Some(known) = ServerPlatform::from_name(trimmed) {
        Some((known.pretty_name().to_string(), DetectedSoftwareKind::Known(known)))
    } else {
        Some((trimmed.to_string(), DetectedSoftwareKind::PluginFork))
    }
}

/// Extracts the server's actual software and/or Minecraft version from a startup console line.
fn parse_server_runtime_line(line: &str) -> Option<DetectedServerRuntime> {
    let clean = strip_ansi_escapes(line);
    let line = clean.trim();
    let mut detected = DetectedServerRuntime::default();

    // Paper, Purpur, Folia, Canvas, Leaf, Pufferfish, etc. (PaperBootstrap):
    // "[bootstrap] Loading Canvas 26.3-1046-HEAD@23ea235 (...) for Minecraft 26.3"
    if let Some((_, rest)) = line.split_once("[bootstrap] Loading ") {
        if let Some(name) = rest.split_whitespace().next() {
            detected.software = classify_plugin_software(name);
        }
        if let Some((_, after_mc)) = rest.split_once(" for Minecraft ")
            && let Some(ver) = after_mc.split_whitespace().next()
        {
            let ver = ver.trim_end_matches(|c: char| matches!(c, ',' | ';' | ')' | '.'));
            if is_valid_mc_version(ver) {
                detected.version = Some(ver.to_string());
            }
        }
    }

    // CraftBukkit / Spigot / Paper / Purpur / Folia / Canvas (CraftServer):
    // "This server is running Canvas version 26.3-1046-HEAD@23ea235 ... (Implementing API ...)"
    // "This server is running Paper version git-Paper-196 (MC: 1.20.1) ..."
    // "This server is running CraftBukkit version 4344-Spigot-... (MC: 1.21.3) ..."
    if detected.software.is_none()
        && let Some((_, rest)) = line.split_once("This server is running ")
        && let Some((raw_software, version_rest)) = rest.split_once(" version ")
    {
        let raw_software = raw_software.trim();
        let effective_name = if raw_software == "CraftBukkit" && version_rest.contains("-Spigot-") {
            "Spigot"
        } else {
            raw_software
        };
        detected.software = classify_plugin_software(effective_name);
        if detected.version.is_none()
            && let Some((_, after_mc)) = version_rest.split_once("(MC: ")
            && let Some((ver, _)) = after_mc.split_once(')')
        {
            let ver = ver.trim();
            if is_valid_mc_version(ver) {
                detected.version = Some(ver.to_string());
            }
        }
    }

    // Fabric & Quilt loader banner:
    // "Loading Minecraft 1.21.1 with Fabric Loader 0.16.10"
    if let Some((_, rest)) = line.split_once("Loading Minecraft ")
        && let Some((ver, loader_rest)) = rest.split_once(" with ")
    {
        let ver = ver.trim();
        if detected.version.is_none() && is_valid_mc_version(ver) {
            detected.version = Some(ver.to_string());
        }
        if detected.software.is_none() {
            if loader_rest.starts_with("Fabric Loader") {
                detected.software = Some(("Fabric".to_string(), DetectedSoftwareKind::Known(ServerPlatform::Fabric)));
            } else if loader_rest.starts_with("Quilt Loader") {
                detected.software = Some(("Quilt".to_string(), DetectedSoftwareKind::ModFork(ServerPlatform::Fabric)));
            }
        }
    }

    // NeoForge & Forge startup lines
    if detected.software.is_none() {
        if line.contains("--fml.neoForgeVersion") || line.contains("net.neoforged") {
            detected.software = Some(("NeoForge".to_string(), DetectedSoftwareKind::Known(ServerPlatform::NeoForge)));
        } else if line.contains("--fml.forgeVersion") || line.contains("MinecraftForge v") {
            detected.software = Some(("Forge".to_string(), DetectedSoftwareKind::Known(ServerPlatform::Forge)));
        }
    }

    // Universal Minecraft DedicatedServer version line (Vanilla and every modded/plugin server):
    // "Starting minecraft server version 26.3"
    if detected.version.is_none()
        && let Some((_, rest)) = line.split_once("Starting minecraft server version ")
        && let Some(ver) = rest.split_whitespace().next()
    {
        let ver = ver.trim_end_matches(|c: char| matches!(c, ',' | ';' | ')' | '.'));
        if is_valid_mc_version(ver) {
            detected.version = Some(ver.to_string());
        }
    }

    (detected.software.is_some() || detected.version.is_some()).then_some(detected)
}

fn apply_detected_runtime_to_config(cfg: &mut InstanceConfiguration, detected: &DetectedServerRuntime) -> (bool, bool) {
    let Some(server) = &mut cfg.server else {
        return (false, false);
    };
    let mut changed = false;
    let mut version_or_loader_changed = false;

    if let Some(version) = &detected.version
        && cfg.minecraft_version.as_str() != version.as_str()
    {
        cfg.minecraft_version = ustr::Ustr::from(version);
        server.build = None;
        changed = true;
        version_or_loader_changed = true;
    }

    if let Some((name, kind)) = &detected.software {
        match *kind {
            DetectedSoftwareKind::Known(platform) => {
                if server.platform != platform {
                    server.platform = platform;
                    server.build = None;
                    changed = true;
                }
                let loader = platform.loader();
                if cfg.loader != loader {
                    cfg.loader = loader;
                    changed = true;
                    version_or_loader_changed = true;
                }
                if server.detected_platform.is_some() {
                    server.detected_platform = None;
                    changed = true;
                }
            },
            DetectedSoftwareKind::PluginFork => {
                if server.detected_platform.as_deref() != Some(name.as_str()) {
                    server.detected_platform = Some(Arc::from(name.as_str()));
                    changed = true;
                }
                if !server.platform.uses_plugins() {
                    server.platform = ServerPlatform::Paper;
                    server.build = None;
                    cfg.loader = ServerPlatform::Paper.loader();
                    changed = true;
                    version_or_loader_changed = true;
                }
            },
            DetectedSoftwareKind::ModFork(base_platform) => {
                if server.detected_platform.as_deref() != Some(name.as_str()) {
                    server.detected_platform = Some(Arc::from(name.as_str()));
                    changed = true;
                }
                if !server.platform.uses_mods() {
                    server.platform = base_platform;
                    server.build = None;
                    cfg.loader = base_platform.loader();
                    changed = true;
                    version_or_loader_changed = true;
                }
            },
        }
    }

    (changed, version_or_loader_changed)
}

impl BackendState {
    fn apply_detected_server_runtime(self: &Arc<Self>, id: InstanceID, detected: DetectedServerRuntime) {
        let mut guard = self.instance_state.write();
        let Some(instance) = guard.instances.get_mut(id) else {
            return;
        };

        let mut changed = false;
        let mut version_or_loader_changed = false;
        instance.configuration.modify(|cfg| {
            let (c, vl) = apply_detected_runtime_to_config(cfg, &detected);
            changed = c;
            version_or_loader_changed = vl;
        });

        if changed {
            if version_or_loader_changed {
                for folder in [
                    bridge::instance::ContentFolder::Plugins,
                    bridge::instance::ContentFolder::Mods,
                    bridge::instance::ContentFolder::Datapacks,
                ] {
                    instance.mark_content_dirty(self, folder, crate::fs::FolderChanges::all_dirty(), false);
                }
            }
            self.send.send(instance.create_modify_message());
        }
    }
}

/// The name a user-supplied jar will have inside the server directory, once it's checked to be a jar
/// that exists.
fn custom_jar_file_name(jar: &Path) -> Result<Arc<str>, String> {
    let Some(file_name) = jar.file_name().and_then(|name| name.to_str()) else {
        return Err(format!("{} can't be used as a server jar", jar.display()));
    };
    if !jar.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("jar")) {
        return Err(format!("{file_name} isn't a .jar file"));
    }
    if !jar.is_file() {
        return Err(format!("{} doesn't exist", jar.display()));
    }
    Ok(file_name.into())
}

/// Copies a user-supplied jar into the server directory and returns its name there. Keeping a copy
/// means the server still starts if the original is moved, and travels with the server when it's
/// duplicated or exported.
async fn import_custom_jar(server_dir: &Path, jar: &Path) -> Result<Arc<str>, String> {
    let file_name = custom_jar_file_name(jar)?;
    let target = server_dir.join(&*file_name);

    // A jar picked from the server's own folder is already where it needs to be
    let already_there = matches!((jar.canonicalize(), target.canonicalize()), (Ok(a), Ok(b)) if a == b);
    if !already_there {
        _ = tokio::fs::create_dir_all(server_dir).await;
        // Copied under a temporary name so a failed copy never replaces a jar that works
        let partial = target.with_extension("jar.partial");
        let copied = match tokio::fs::copy(jar, &partial).await {
            Ok(_) => tokio::fs::rename(&partial, &target).await,
            Err(err) => Err(err),
        };
        if let Err(err) = copied {
            _ = tokio::fs::remove_file(&partial).await;
            return Err(format!("Unable to copy {file_name} into the server: {err}"));
        }
    }

    Ok(file_name)
}

/// Mojang's EULA has to be accepted in `eula.txt` or the server refuses to start. Only written
/// once the user has ticked the box in the UI, which is their acceptance.
fn write_eula(server_dir: &Path) {
    let path = server_dir.join("eula.txt");
    let contents = "# Accepted through Pandora Launcher\n# https://aka.ms/MinecraftEULA\neula=true\n";
    if let Err(err) = crate::fs::write_safe(&path, contents.as_bytes()) {
        log::error!("Unable to write eula.txt: {err}");
    }
}

/// Server consoles print plain text carrying their own `[time LEVEL]` prefix, rather than the
/// XML events the client log reader is built around. Left alone, that prefix duplicates the
/// console's own time and level columns and every line shows as INFO, so it is stripped and its
/// level used instead.
fn normalize_server_line(message: GameOutputMsg) -> GameOutputMsg {
    let Some((level, rest)) = message.text.first().and_then(|first| split_server_log_prefix(first)) else {
        return message;
    };
    let mut lines: Vec<Arc<str>> = message.text.to_vec();
    lines[0] = rest.into();
    GameOutputMsg {
        time: message.time,
        level,
        text: lines.into(),
    }
}

fn split_server_log_prefix(line: &str) -> Option<(bridge::game_output::GameOutputLogLevel, &str)> {
    let is_clock = |text: &str| {
        let bytes = text.as_bytes();
        bytes.len() == 8
            && bytes[2] == b':'
            && bytes[5] == b':'
            && bytes
                .iter()
                .enumerate()
                .all(|(index, byte)| index == 2 || index == 5 || byte.is_ascii_digit())
    };
    let after_bracket = line.strip_prefix('[')?;
    let (head, rest) = after_bracket.split_once(']')?;

    // Paper and Spigot: "[13:25:05 INFO]: message"
    if let Some((time, level)) = head.split_once(' ')
        && is_clock(time)
    {
        let rest = rest.strip_prefix(": ").or_else(|| rest.strip_prefix(':')).unwrap_or(rest).trim_start();
        return Some((parse_log_level(level)?, rest));
    }

    // log4j's default, used by vanilla, Fabric and Forge: "[13:25:05] [Server thread/INFO]: message"
    if is_clock(head) {
        let rest = rest.trim_start().strip_prefix('[')?;
        let (thread_and_level, rest) = rest.split_once(']')?;
        let level = thread_and_level.rsplit_once('/').map_or(thread_and_level, |(_, level)| level);
        let rest = rest.strip_prefix(": ").or_else(|| rest.strip_prefix(':')).unwrap_or(rest).trim_start();
        return Some((parse_log_level(level)?, rest));
    }

    None
}

fn parse_log_level(level: &str) -> Option<bridge::game_output::GameOutputLogLevel> {
    use bridge::game_output::GameOutputLogLevel;
    Some(match level.trim() {
        "FATAL" => GameOutputLogLevel::Fatal,
        "ERROR" | "SEVERE" => GameOutputLogLevel::Error,
        "WARN" | "WARNING" => GameOutputLogLevel::Warn,
        "INFO" => GameOutputLogLevel::Info,
        "DEBUG" => GameOutputLogLevel::Debug,
        "TRACE" => GameOutputLogLevel::Trace,
        _ => return None,
    })
}

fn write_command(stdin: &mut Option<std::io::PipeWriter>, command: &str) -> bool {
    let Some(writer) = stdin else {
        return false;
    };

    if writeln!(writer, "{command}").and_then(|_| writer.flush()).is_err() {
        // The pipe is gone, so the server is on its way out
        *stdin = None;
        return false;
    }

    true
}

impl BackendState {
    /// Creates a server instance, optionally seeded from an existing client instance.
    ///
    /// Server instances are ordinary instances carrying a [`schema::server::ServerConfiguration`],
    /// which is what makes them show up on the Servers page instead of the Instances page. Doing
    /// it this way means content installation, groups, icons and settings all work on servers
    /// without a second implementation of any of it.
    pub(crate) async fn create_server_instance(
        self: &Arc<Self>,
        name: ustr::Ustr,
        version: ustr::Ustr,
        platform: ServerPlatform,
        eula_accepted: bool,
        group: Option<Arc<str>>,
        generate_from: Option<InstanceID>,
        custom_jar: Option<PathBuf>,
        modal_action: ModalAction,
    ) -> Option<PathBuf> {
        let loader = platform.loader();

        let mut server = schema::server::ServerConfiguration::new(platform);
        server.eula_accepted = eula_accepted;
        if let Some(jar) = &custom_jar {
            // Checked before anything is created, so a bad pick doesn't leave a half-made server
            match custom_jar_file_name(jar) {
                Ok(file) => server.custom_jar = Some(ServerCustomJar { enabled: true, file }),
                Err(err) => {
                    modal_action.set_finished_with_error(err.into());
                    return None;
                },
            }
        }
        if let Some(generate_from) = generate_from {
            server.linked_instance = self
                .instance_state
                .read()
                .instances
                .get(generate_from)
                .map(|instance| instance.name.as_str().into());
        }

        let Some(instance_dir) = self.create_instance_configured_inner(
            &self.directories.servers_dir,
            &name,
            &version,
            loader,
            None,
            true,
            |configuration| {
                configuration.group = group;
                configuration.server = Some(server);
            },
        ) else {
            // create_instance_configured already reported why
            modal_action.set_finished();
            return None;
        };

        // The server directory doubles as the working directory, so make sure it exists before
        // anything tries to write a jar or server.properties into it
        _ = std::fs::create_dir_all(instance_dir.join(".minecraft"));

        if eula_accepted {
            write_eula(&instance_dir.join(".minecraft"));
        }

        if let Some(jar) = &custom_jar
            && let Err(err) = import_custom_jar(&instance_dir.join(".minecraft"), jar).await
        {
            // The server itself is fine; the jar can be picked again from its settings
            self.send.send_error(err);
        }

        // The players from Settings > Servers are opped and whitelisted before it first starts
        self.apply_server_defaults(&instance_dir.join(".minecraft")).await;

        let Some(generate_from) = generate_from else {
            modal_action.set_finished();
            return Some(instance_dir);
        };

        // The new folder is loaded by the filesystem watcher, the same as a client instance, and
        // the watcher is serviced by this same handler loop, so the mod copy has to wait for it
        // from a separate task rather than here
        let this = self.clone();
        let created = instance_dir.clone();
        tokio::task::spawn(async move {
            match this.wait_for_new_instance(&instance_dir).await {
                Some(id) => match this.plan_server_sync(id, generate_from) {
                    // Generated servers take the detected defaults; the list can be reviewed and
                    // re-synced from the server's settings
                    Some(plan) => this.apply_server_sync(plan, modal_action).await,
                    None => {
                        modal_action.set_finished();
                        this.send.send_warning("Unable to copy mods from that instance");
                    },
                },
                None => modal_action.set_finished_with_error("Unable to load the new server".into()),
            }
        });
        Some(created)
    }
}

#[cfg(test)]
mod tests {
    use bridge::game_output::GameOutputLogLevel;

    use schema::{
        instance::InstanceConfiguration,
        loader::Loader,
        server::{ServerConfiguration, ServerCustomJar, ServerPlatform},
    };

    use super::{software_switched, split_server_log_prefix};

    fn paper_server() -> InstanceConfiguration {
        let mut configuration = InstanceConfiguration::new("1.20.1".into(), Loader::Paper);
        let mut server = ServerConfiguration::new(ServerPlatform::Paper);
        server.build = Some("196".into());
        server.eula_accepted = true;
        server.auto_restart = true;
        server.custom_jar = Some(ServerCustomJar {
            enabled: true,
            file: "spigot.jar".into(),
        });
        configuration.server = Some(server);
        configuration
    }

    #[test]
    fn switching_software_moves_loader_and_version_and_clears_the_old_build() {
        let switched = software_switched(&paper_server(), ServerPlatform::Fabric, "1.21.1".into());
        let server = switched.server.as_ref().unwrap();
        assert_eq!(switched.minecraft_version, "1.21.1");
        assert_eq!(switched.loader, Loader::Fabric);
        assert_eq!(server.platform, ServerPlatform::Fabric);
        assert_eq!(server.build, None);
        // The rest of the server is untouched, and the custom jar is kept but no longer runs
        assert!(server.eula_accepted && server.auto_restart);
        assert_eq!(server.active_custom_jar(), None);
        assert_eq!(server.custom_jar.as_ref().map(|jar| &*jar.file), Some("spigot.jar"));
    }

    #[test]
    fn switching_between_plugin_platforms_keeps_the_loader() {
        let mut configuration = paper_server();
        configuration.server.as_mut().unwrap().linked_instance = Some("Pack".into());
        let switched = software_switched(&configuration, ServerPlatform::Purpur, "1.20.1".into());
        assert_eq!(switched.loader, Loader::Paper);
        assert_eq!(switched.server.as_ref().unwrap().platform, ServerPlatform::Purpur);
        assert_eq!(switched.server.as_ref().unwrap().build, None);
        assert_eq!(switched.server.as_ref().unwrap().linked_instance.as_deref(), Some("Pack"));
    }

    #[test]
    fn switching_loader_forgets_the_synced_instance_and_loader_version() {
        let mut configuration = InstanceConfiguration::new("1.20.1".into(), Loader::Forge);
        configuration.preferred_loader_version = Some("1.20.1-47.3.0".into());
        let mut server = ServerConfiguration::new(ServerPlatform::Forge);
        server.linked_instance = Some("Pack".into());
        configuration.server = Some(server);

        let switched = software_switched(&configuration, ServerPlatform::NeoForge, "1.20.1".into());
        assert_eq!(switched.loader, Loader::NeoForge);
        assert_eq!(switched.preferred_loader_version, None);
        assert_eq!(switched.server.as_ref().unwrap().linked_instance, None);
    }

    #[test]
    fn reapplying_the_same_software_keeps_the_build() {
        let switched = software_switched(&paper_server(), ServerPlatform::Paper, "1.20.1".into());
        assert_eq!(switched.server.as_ref().unwrap().build.as_deref(), Some("196"));
    }

    #[test]
    fn strips_paper_prefix() {
        assert_eq!(
            split_server_log_prefix("[13:25:06 INFO]: Done (6.073s)! For help, type \"help\""),
            Some((GameOutputLogLevel::Info, "Done (6.073s)! For help, type \"help\""))
        );
        assert_eq!(
            split_server_log_prefix("[13:19:31 WARN]: **** FAILED TO BIND TO PORT!"),
            Some((GameOutputLogLevel::Warn, "**** FAILED TO BIND TO PORT!"))
        );
    }

    #[test]
    fn strips_log4j_default_prefix() {
        assert_eq!(
            split_server_log_prefix("[13:19:32] [Server thread/ERROR]: Encountered an unexpected exception"),
            Some((GameOutputLogLevel::Error, "Encountered an unexpected exception"))
        );
        // Forge adds a logger tag, which is kept
        assert_eq!(
            split_server_log_prefix("[13:19:32] [main/INFO] [cpw.mods.modlauncher.Launcher/MODLAUNCHER]: Launching"),
            Some((GameOutputLogLevel::Info, "[cpw.mods.modlauncher.Launcher/MODLAUNCHER]: Launching"))
        );
    }

    #[test]
    fn leaves_other_lines_alone() {
        assert_eq!(split_server_log_prefix("> list"), None);
        assert_eq!(split_server_log_prefix("\tat java.base/java.lang.Thread.run(Thread.java:1583)"), None);
        assert_eq!(split_server_log_prefix("[spark] Starting background profiler..."), None);
        assert_eq!(split_server_log_prefix("[13:19:32 NOTALEVEL]: hello"), None);
    }

    #[test]
    fn mark_stopping_prevents_auto_restart_and_cancels_pending_restart() {
        use super::{RunningServer, ServerRegistry};
        use bridge::instance::InstanceID;

        let registry = ServerRegistry::default();
        let id = InstanceID::dangling();
        registry.servers.lock().insert(
            id,
            RunningServer {
                stdin: None,
                stopping: false,
            },
        );
        let token = registry.begin_restart(id);

        registry.mark_stopping(id);

        let was_stopping = registry.servers.lock().remove(&id).map(|server| server.stopping).unwrap_or(false);
        assert!(was_stopping, "killing a server must mark it as intentionally stopping");
        assert!(
            !registry.take_restart_if_current(id, token),
            "killing a server must cancel any pending manual restart"
        );
    }

    #[test]
    fn detects_canvas_and_paper_forks_from_startup_lines() {
        use super::{DetectedSoftwareKind, apply_detected_runtime_to_config, parse_server_runtime_line};

        let parsed = parse_server_runtime_line(
            "[bootstrap] Loading Canvas 26.3-1046-HEAD@23ea235 (2026-11-22T16:29:14Z) for Minecraft 26.3",
        )
        .unwrap();
        assert_eq!(parsed.software, Some(("Canvas".to_string(), DetectedSoftwareKind::PluginFork)));
        assert_eq!(parsed.version.as_deref(), Some("26.3"));

        let mut cfg = paper_server();
        let (changed, vl_changed) = apply_detected_runtime_to_config(&mut cfg, &parsed);
        assert!(changed && vl_changed);
        assert_eq!(cfg.minecraft_version, "26.3");
        let server = cfg.server.as_ref().unwrap();
        assert_eq!(server.platform, ServerPlatform::Paper);
        assert_eq!(server.display_platform(), "Canvas");
        assert_eq!(server.build, None);

        // Colored CraftServer line with MC version in parentheses
        let spigot = parse_server_runtime_line(
            "\x1b[32mThis server is running CraftBukkit version 4344-Spigot-a759b62 (MC: 1.21.3)\x1b[0m",
        )
        .unwrap();
        assert_eq!(spigot.software, Some(("Spigot".to_string(), DetectedSoftwareKind::PluginFork)));
        assert_eq!(spigot.version.as_deref(), Some("1.21.3"));
    }

    #[test]
    fn detects_fabric_quilt_forge_neoforge_and_vanilla_version() {
        use super::{DetectedSoftwareKind, parse_server_runtime_line};

        let fabric = parse_server_runtime_line("Loading Minecraft 1.21.1 with Fabric Loader 0.16.10").unwrap();
        assert_eq!(
            fabric.software,
            Some(("Fabric".to_string(), DetectedSoftwareKind::Known(ServerPlatform::Fabric)))
        );
        assert_eq!(fabric.version.as_deref(), Some("1.21.1"));

        let quilt = parse_server_runtime_line("Loading Minecraft 1.21.1 with Quilt Loader 0.26.4").unwrap();
        assert_eq!(
            quilt.software,
            Some(("Quilt".to_string(), DetectedSoftwareKind::ModFork(ServerPlatform::Fabric)))
        );
        assert_eq!(quilt.version.as_deref(), Some("1.21.1"));

        let neoforge =
            parse_server_runtime_line("ModLauncher running: args [--fml.neoForgeVersion, 21.4.123]").unwrap();
        assert_eq!(
            neoforge.software,
            Some(("NeoForge".to_string(), DetectedSoftwareKind::Known(ServerPlatform::NeoForge)))
        );

        let forge = parse_server_runtime_line("MinecraftForge v47.3.0 Initialized").unwrap();
        assert_eq!(
            forge.software,
            Some(("Forge".to_string(), DetectedSoftwareKind::Known(ServerPlatform::Forge)))
        );

        let vanilla = parse_server_runtime_line("Starting minecraft server version 26.3").unwrap();
        assert_eq!(vanilla.software, None);
        assert_eq!(vanilla.version.as_deref(), Some("26.3"));
    }

    #[test]
    fn maps_minecraft_versions_to_expected_java_versions() {
        use super::default_java_for_minecraft_version;

        assert_eq!(default_java_for_minecraft_version("1.12.2"), ("jre-legacy".into(), 8));
        assert_eq!(default_java_for_minecraft_version("1.16.5"), ("jre-legacy".into(), 8));
        assert_eq!(default_java_for_minecraft_version("1.17.1"), ("java-runtime-alpha".into(), 16));
        assert_eq!(default_java_for_minecraft_version("1.20.1"), ("java-runtime-gamma".into(), 17));
        assert_eq!(default_java_for_minecraft_version("1.20.4"), ("java-runtime-gamma".into(), 17));
        assert_eq!(default_java_for_minecraft_version("1.20.5"), ("java-runtime-delta".into(), 21));
        assert_eq!(default_java_for_minecraft_version("1.21.6"), ("java-runtime-delta".into(), 21));
        assert_eq!(default_java_for_minecraft_version("26.1"), ("java-runtime-epsilon".into(), 25));
        assert_eq!(default_java_for_minecraft_version("26.3"), ("java-runtime-epsilon".into(), 25));
    }

    #[test]
    fn inspects_server_jar_version_json_and_class_bytecode() {
        use super::inspect_server_jar_java;
        use std::io::Write;

        let dir = std::env::temp_dir().join(format!("pandora_jar_java_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);

        // Jar with version.json (Canvas 26.3 -> Java 25)
        let json_jar = dir.join("canvas.jar");
        {
            let mut zip = zip::ZipWriter::new(std::fs::File::create(&json_jar).unwrap());
            zip.start_file("version.json", zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(br#"{"id":"26.3","java_component":"java-runtime-epsilon","java_version":25}"#)
                .unwrap();
            zip.finish().unwrap();
        }
        let req = inspect_server_jar_java(&json_jar).unwrap();
        assert_eq!(req.mc_version.as_deref(), Some("26.3"));
        assert_eq!(req.java_component.as_deref(), Some("java-runtime-epsilon"));
        assert_eq!(req.java_major, Some(25));

        // Jar without version.json, using Main-Class bytecode major version 69 (Java 25)
        let class_jar = dir.join("custom.jar");
        {
            let mut zip = zip::ZipWriter::new(std::fs::File::create(&class_jar).unwrap());
            zip.start_file("META-INF/MANIFEST.MF", zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(b"Manifest-Version: 1.0\nMain-Class: com.example.Main\n").unwrap();
            zip.start_file("com/example/Main.class", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(&[0xCA, 0xFE, 0xBA, 0xBE, 0x00, 0x00, 0x00, 69]).unwrap();
            zip.finish().unwrap();
        }
        let req = inspect_server_jar_java(&class_jar).unwrap();
        assert_eq!(req.java_major, Some(25));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn formats_clear_startup_error_notifications() {
        use super::parse_server_startup_error;

        let mismatch = parse_server_startup_error(
            "java.lang.UnsupportedClassVersionError: net/minecraft/server/Main has been compiled by a more recent version of the Java Runtime (class file version 69.0), this version of the Java Runtime only recognizes class file versions up to 65.0",
            || 25565,
        )
        .unwrap();
        assert!(mismatch.contains("requires Java 25") && mismatch.contains("started with Java 21"));

        let port = parse_server_startup_error("**** FAILED TO BIND TO PORT!", || 25566).unwrap();
        assert!(port.contains("Port 25566"));

        let flags = parse_server_startup_error("Unrecognized VM option 'UseZGCBad'", || 25565).unwrap();
        assert!(flags.contains("JVM flags"));
    }
}
