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
    server::{ServerPlatform, ServerProperty, parse_server_properties, write_server_properties},
};
use tokio::sync::mpsc::UnboundedSender;

use crate::{BackendState, log_reader, metadata::items::MinecraftVersionManifestMetadataItem};

/// How many console lines are kept per server so the console still has scrollback after the user
/// navigates away from the page and back.
const CONSOLE_HISTORY_LINES: usize = 2000;

/// A server the launcher has started, and the pipes needed to talk to it.
pub struct RunningServer {
    /// The server's stdin, which is how commands are delivered. `None` once the pipe is closed.
    stdin: Option<std::io::PipeWriter>,
    history: std::collections::VecDeque<GameOutputMsg>,
    subscribers: Vec<UnboundedSender<GameOutputMsg>>,
    /// Set while a stop was asked for, so the exit isn't reported as a crash.
    stopping: bool,
}

#[derive(Default)]
pub struct ServerRegistry {
    servers: Mutex<FxHashMap<InstanceID, RunningServer>>,
}

impl ServerRegistry {
    fn with<R>(&self, id: InstanceID, f: impl FnOnce(&mut RunningServer) -> R) -> Option<R> {
        self.servers.lock().get_mut(&id).map(f)
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

        let java = match self.resolve_server_java_binary(&configuration).await {
            Some(java) => java,
            None => {
                launch_tracker.set_finished(ProgressTrackerFinishType::from_err(true));
                self.undo_server_prelaunch(id);
                modal_action.set_finished_with_error("Unable to find a Java runtime to run this server".into());
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
            .map(|stdout| log_reader::start_game_output(stdout, child.stderr.take()));

        self.servers.servers.lock().insert(
            id,
            RunningServer {
                stdin,
                history: std::collections::VecDeque::with_capacity(64),
                subscribers: Vec::new(),
                stopping: false,
            },
        );

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
                let mut reported_bind_failure = false;
                while let Some(message) = output.recv().await {
                    let message = normalize_server_line(message);

                    // A port clash is the most common reason a new server won't come up, and the
                    // server just shuts down, so say what happened and what to change
                    if !reported_bind_failure && message.text.iter().any(|line| line.contains("FAILED TO BIND TO PORT"))
                    {
                        reported_bind_failure = true;
                        let port = this.configured_server_port(id);
                        this.send.send_error(format!(
                            "Port {port} is already in use, so the server can't start. Another server may be \
                             running; change server-port in this server's Properties tab."
                        ));
                    }

                    this.servers.with(id, |server| {
                        if server.history.len() >= CONSOLE_HISTORY_LINES {
                            server.history.pop_front();
                        }
                        server.history.push_back(message.clone());
                        server.subscribers.retain(|subscriber| subscriber.send(message.clone()).is_ok());
                    });
                }

                // stdout closing means the server has exited
                this.on_server_exited(id);
            });
        }
    }

    /// Waits for the filesystem watcher to load a newly created instance folder, loading it directly
    /// only if the watcher never does.
    async fn wait_for_new_instance(&self, instance_dir: &Path) -> Option<InstanceID> {
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

    fn on_server_exited(self: &Arc<Self>, id: InstanceID) {
        let was_stopping = self.servers.servers.lock().remove(&id).map(|server| server.stopping).unwrap_or(false);

        let auto_restart = self
            .instance_state
            .write()
            .instances
            .get_mut(id)
            .and_then(|instance| instance.configuration.get().server.as_ref().map(|server| server.auto_restart))
            .unwrap_or(false);

        self.send.send(MessageToFrontend::Refresh);

        if auto_restart && !was_stopping {
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

    pub async fn restart_server(self: &Arc<Self>, id: InstanceID, modal_action: ModalAction) {
        if self.is_server_running(id) {
            self.stop_server(id);

            // Give the server time to shut down on its own before starting it again
            for _ in 0..120 {
                if !self.is_server_running(id) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }

            if self.is_server_running(id) {
                modal_action.set_finished_with_error("The server did not shut down in time".into());
                return;
            }
        }

        self.start_server(id, modal_action).await;
    }

    pub fn send_server_command(self: &Arc<Self>, id: InstanceID, command: &str) {
        let command = command.trim();
        if command.is_empty() {
            return;
        }

        // The console shows what was typed, since the server only echoes commands it recognises
        self.servers.with(id, |server| {
            let echo = GameOutputMsg {
                time: chrono::Utc::now().timestamp_millis(),
                level: bridge::game_output::GameOutputLogLevel::Info,
                text: Arc::new([format!("> {command}").into()]),
            };
            if server.history.len() >= CONSOLE_HISTORY_LINES {
                server.history.pop_front();
            }
            server.history.push_back(echo.clone());
            server.subscribers.retain(|subscriber| subscriber.send(echo.clone()).is_ok());

            write_command(&mut server.stdin, command)
        });
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
        let history = self
            .servers
            .with(id, |server| {
                server.subscribers.push(sender);
                server.history.iter().cloned().collect::<Vec<_>>()
            })
            .unwrap_or_default();

        _ = channel.send((history, receiver));
    }

    pub fn read_server_properties(self: &Arc<Self>, id: InstanceID) -> Vec<ServerProperty> {
        let Some(server_dir) = self.server_dir(id) else {
            return Vec::new();
        };

        let contents = std::fs::read_to_string(server_dir.join("server.properties")).unwrap_or_default();
        parse_server_properties(&contents)
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

    /// Picks the Java runtime for a server. Honours an explicit `jvm_binary` override, otherwise
    /// falls back to the managed runtime for the server's Minecraft version.
    pub(crate) async fn resolve_server_java_binary(
        self: &Arc<Self>,
        configuration: &InstanceConfiguration,
    ) -> Option<PathBuf> {
        if let Some(jvm_binary) = &configuration.jvm_binary
            && jvm_binary.enabled
            && let Some(path) = &jvm_binary.path
            && let Some(binary) = crate::launch::Launcher::search_for_java_binary(path)
        {
            return Some(binary);
        }

        let manifest = self.meta.fetch(MinecraftVersionManifestMetadataItem).await.ok()?;
        let link = manifest.versions.iter().find(|link| link.id == configuration.minecraft_version)?;
        let version_info = self.meta.fetch(crate::metadata::items::MinecraftVersionMetadataItem(link)).await.ok()?;

        self.launcher
            .load_java_binary_for_server(&self.meta, &self.http_client_provider.client(), configuration, &version_info)
            .await
    }
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
        modal_action: ModalAction,
    ) {
        let loader = platform.loader();

        let mut server = schema::server::ServerConfiguration::new(platform);
        server.eula_accepted = eula_accepted;
        if let Some(generate_from) = generate_from {
            server.linked_instance = self
                .instance_state
                .read()
                .instances
                .get(generate_from)
                .map(|instance| instance.name.as_str().into());
        }

        let Some(instance_dir) = self.create_instance_configured(&name, &version, loader, None, |configuration| {
            configuration.group = group;
            configuration.server = Some(server);
        }) else {
            // create_instance_configured already reported why
            modal_action.set_finished();
            return;
        };

        // The server directory doubles as the working directory, so make sure it exists before
        // anything tries to write a jar or server.properties into it
        _ = std::fs::create_dir_all(instance_dir.join(".minecraft"));

        if eula_accepted {
            write_eula(&instance_dir.join(".minecraft"));
        }

        let Some(generate_from) = generate_from else {
            modal_action.set_finished();
            return;
        };

        // The new folder is loaded by the filesystem watcher, the same as a client instance, and
        // the watcher is serviced by this same handler loop, so the mod copy has to wait for it
        // from a separate task rather than here
        let this = self.clone();
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
    }
}

#[cfg(test)]
mod tests {
    use bridge::game_output::GameOutputLogLevel;

    use super::split_server_log_prefix;

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
}
