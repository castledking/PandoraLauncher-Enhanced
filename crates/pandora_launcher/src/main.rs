#![deny(unused_must_use)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(windows)]
use std::ffi::OsStr;
use std::ffi::OsString;
use std::fmt::Write;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use bridge::handle::{BackendHandle, FrontendHandle};
use bridge::message::{MessageToBackend, MessageToFrontend};
use bridge::quit::QuitCoordinator;
use clap::Parser;
use fern::colors::ColoredLevelConfig;
use native_dialog::DialogBuilder;
use parking_lot::RwLock;

#[derive(Parser, Debug)]
#[command()]
struct Cli {
    /// Instance to launch, instead of opening the launcher
    #[arg(long)]
    run_instance: Option<String>,
    /// Internal function to set traversable ACLs in an elevated context
    #[cfg(windows)]
    #[arg(long, hide = false, num_args = 2..)]
    internal_set_traverse_acls: Option<Vec<std::ffi::OsString>>,
}

pub mod panic;

/// How long a launcher that's restarting into an update waits for the old one to exit.
const RESTART_WAIT: Duration = Duration::from_secs(60);
/// How long to keep trying to reach a running launcher that holds the lock: it may still be
/// starting up and not listening yet, or be shutting down and about to let go of the lock.
const HAND_OVER_WAIT: Duration = Duration::from_secs(10);

fn main() {
    // SAFETY: nothing else is running yet
    unsafe { bridge::launcher::init() };

    let restarting = std::env::var_os(bridge::launcher::RESTART_ENV).is_some();
    // SAFETY: nothing else is running yet. Removed so the games we start don't inherit it
    unsafe { std::env::remove_var(bridge::launcher::RESTART_ENV) };

    let cli = Cli::parse();

    #[cfg(windows)]
    if let Some(internal_set_traverse_acls) = cli.internal_set_traverse_acls {
        if let Err(err) = command::set_traverse_acls(internal_set_traverse_acls) {
            eprintln!("Unable to set traverse ACLs: {err}");
            std::process::exit(1);
        } else {
            std::process::exit(0);
        }
    }

    let data_dir = if let Some(portable_dir) = get_portable_dir() {
        portable_dir
    } else {
        let base_dirs = directories::BaseDirs::new().unwrap();
        base_dirs.data_dir().into()
    };

    let launcher_dir = data_dir.join("PandoraLauncher");
    _ = std::fs::create_dir_all(&launcher_dir);
    _ = std::env::set_current_dir(&launcher_dir);

    let endpoint = Endpoint::new(&launcher_dir);

    let lockfile_path = launcher_dir.join("launcher.lock");
    let lockfile = match OpenOptions::new().read(true).write(true).create(true).open(&lockfile_path) {
        Ok(lockfile) => lockfile,
        Err(err) => {
            show_error_eprintln(format!("Unable open launcher.lock file: {err}"));
            return;
        },
    };

    // The lock is held for as long as a launcher runs, so whoever holds it is either up and
    // listening on the endpoint or about to be (starting up, or shutting down and about to exit).
    // Hand over to it, or take its place once it's gone, and never give up on it so quickly that
    // two launchers end up running.
    let restart_deadline = Instant::now() + if restarting { RESTART_WAIT } else { Duration::ZERO };
    let hand_over_deadline = restart_deadline + HAND_OVER_WAIT;
    let mut locking_unsupported = None;

    loop {
        match lockfile.try_lock() {
            Ok(()) => break,
            Err(TryLockError::WouldBlock) => {},
            Err(TryLockError::Error(err)) => {
                // Some network and cloud-synced folders can't be locked at all. Start anyway rather
                // than refusing to, at the cost of not stopping a second launcher alongside.
                locking_unsupported = Some(err);
                break;
            },
        }

        let now = Instant::now();

        // A launcher restarting into an update would otherwise just hand over to the old one
        if now >= restart_deadline {
            match endpoint.hand_over(std::env::args_os()) {
                Ok(()) => return,
                Err(err) if now >= hand_over_deadline => {
                    show_error_eprintln(format!(
                        "Pandora Launcher is already running but isn't responding ({err}). If it doesn't close \
                         by itself, end it from your task manager and start it again."
                    ));
                    return;
                },
                Err(_) => {},
            }
        }

        std::thread::sleep(Duration::from_millis(100));
    }

    run_launcher(cli, launcher_dir, endpoint, lockfile, locking_unsupported);
}

fn run_launcher(
    cli: Cli,
    launcher_dir: PathBuf,
    endpoint: Endpoint,
    lockfile: File,
    locking_unsupported: Option<std::io::Error>,
) {
    setup_launcher_logging(&launcher_dir);

    if let Some(err) = locking_unsupported {
        log::warn!("Unable to lock launcher.lock, so another launcher could start alongside this one: {err}");
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .expect("Failed to initialize Tokio runtime");

    let enter_guard = runtime.enter();
    let listeners = match endpoint.listen() {
        Ok(listeners) => listeners,
        Err(err) => {
            show_error(format!("Unable to start listener: {err}"));
            return;
        },
    };
    drop(enter_guard);

    let panic_message = Arc::new(RwLock::new(None));
    let deadlock_message = Arc::new(RwLock::new(None));

    let (backend_recv, backend_handle, frontend_recv, frontend_handle) = bridge::handle::create_pair();

    crate::panic::install_hook(panic_message.clone(), frontend_handle.clone());
    start_deadlock_detection(&deadlock_message, &frontend_handle);

    let listen_cancel = tokio_util::sync::CancellationToken::new();

    // note: there are many possible race conditions with the whole single-process architecture
    // it's possible for a command to be sent to the main process while it is shutting down
    // most of these are covered by the lock being held until the process exits, and by a second
    // launcher retrying for a while when the running one doesn't answer

    for listener in listeners {
        runtime.spawn(listen(listener, frontend_handle.clone(), backend_handle.clone(), listen_cancel.clone()));
    }

    let quit_handler = {
        let backend_handle = backend_handle.clone();
        QuitCoordinator::new(Box::new(move || {
            listen_cancel.cancel();
            backend_handle.send(MessageToBackend::Quit);
            // backend will send Quit to frontend when done
            // when frontend is done, frontend::start will be unblocked and program will exit
        }))
    };

    run_cli(cli, &frontend_handle, &backend_handle);

    backend::start(
        runtime,
        launcher_dir.clone(),
        frontend_handle.clone(),
        backend_handle.clone(),
        backend_recv,
        quit_handler.fork(),
    );
    frontend::start(
        launcher_dir.clone(),
        panic_message,
        deadlock_message,
        backend_handle,
        frontend_handle,
        frontend_recv,
        quit_handler,
    );
    log::info!("Quitting...");

    // Only now, once everything is saved, can another launcher take over
    drop(lockfile);
}

/// Runs the commands other launchers started while this one runs hand over.
async fn listen(
    mut listener: PlatformListener,
    frontend_handle: FrontendHandle,
    backend_handle: BackendHandle,
    cancel: tokio_util::sync::CancellationToken,
) {
    let mut args = Vec::new();

    'listen: loop {
        tokio::select! {
            conn = listener.accept() => {
                let conn = match conn {
                    Ok(conn) => conn,
                    Err(err) => {
                        log::error!("An error occurred trying to handle an incoming connection: {err}");
                        continue;
                    },
                };
                let mut conn = tokio::io::BufReader::new(conn);

                use tokio::io::AsyncReadExt;
                use tokio::io::AsyncBufReadExt;

                let mut argc = [0; 1];
                if let Err(err) = conn.read_exact(&mut argc).await {
                    log::error!("Error reading data from listener: {err}");
                    continue;
                }

                args.clear();
                for _ in 0..argc[0] {
                    let mut buf = Vec::new();
                    if let Err(err) = conn.read_until(b'\0', &mut buf).await {
                        log::error!("Error reading data from listener: {err}");
                        continue 'listen;
                    }

                    if buf.last().copied() != Some(0) {
                        log::error!("Error reading data from listener: expected last byte to be NUL byte");
                        continue 'listen;
                    }

                    buf.truncate(buf.len() - 1);
                    args.push(unsafe { OsString::from_encoded_bytes_unchecked(buf) });
                }

                match Cli::try_parse_from(&args) {
                    Ok(cli) => run_cli(cli, &frontend_handle, &backend_handle),
                    Err(err) => {
                        log::error!("Error while parsing received arguments: {err}");
                        continue 'listen;
                    },
                }
            },
            _ = cancel.cancelled() => {
                break;
            }
        }
    }

    listener.close();
}

/// Where the running launcher listens for launchers started after it: a socket in the launcher
/// folder, or on Windows a named pipe.
struct Endpoint {
    #[cfg(unix)]
    socket: PathBuf,
    #[cfg(windows)]
    pipe: OsString,
}

/// The pipe launchers before 4.0.1 used. Pipe names are shared by every user on the machine, so
/// whichever user opened the launcher second couldn't start one at all.
#[cfg(windows)]
const LEGACY_PIPE: &str = r"\\.\pipe\pandora-launcher-socket";

impl Endpoint {
    fn new(launcher_dir: &Path) -> Self {
        #[cfg(unix)]
        return Self {
            socket: launcher_dir.join("launcher.sock"),
        };

        #[cfg(windows)]
        {
            // Paths are case-insensitive on Windows, so `C:\Games` and `c:\games` get one pipe
            let folder = launcher_dir.as_os_str().as_encoded_bytes().to_ascii_lowercase();
            return Self {
                pipe: format!(r"\\.\pipe\pandora-launcher-{:016x}", fnv1a(&folder)).into(),
            };
        }
    }

    /// Passes `args` on to the launcher that's running.
    fn hand_over(&self, args: impl ExactSizeIterator<Item = OsString>) -> std::io::Result<()> {
        if args.len() >= u8::MAX as usize {
            return Err(std::io::Error::other("too many arguments"));
        }

        let mut bytes = vec![args.len() as u8];
        for arg in args {
            bytes.extend(arg.as_encoded_bytes());
            bytes.push(0);
        }

        #[cfg(unix)]
        let mut conn = std::os::unix::net::UnixStream::connect(&self.socket)?;

        // Launchers before 4.0.1 only listen on the old name
        #[cfg(windows)]
        let mut conn = open_pipe(&self.pipe).or_else(|_| open_pipe(OsStr::new(LEGACY_PIPE)))?;

        conn.write_all(&bytes)
    }

    fn listen(&self) -> std::io::Result<Vec<PlatformListener>> {
        #[cfg(unix)]
        {
            _ = std::fs::remove_file(&self.socket);
            log::info!("Starting local socket: {:?}", self.socket);
            return Ok(vec![PlatformListener::bind(&self.socket)?]);
        }

        #[cfg(windows)]
        {
            log::info!("Starting local pipe: {:?}", self.pipe);
            let mut listeners = vec![PlatformListener::bind(&self.pipe)?];

            // So a launcher from before 4.0.1 that's started now hands over instead of failing.
            // Another user's launcher may have it already, which is fine.
            match PlatformListener::bind(OsStr::new(LEGACY_PIPE)) {
                Ok(listener) => listeners.push(listener),
                Err(err) => log::debug!("Not listening on {LEGACY_PIPE}: {err}"),
            }

            return Ok(listeners);
        }
    }
}

#[cfg(windows)]
fn open_pipe(name: &OsStr) -> std::io::Result<File> {
    // Every instance of the pipe being busy only lasts as long as the launcher takes to accept
    // the previous connection
    let mut attempts = 0;
    loop {
        match OpenOptions::new().write(true).open(name) {
            Err(err) if err.raw_os_error() == Some(231) && attempts < 20 => {
                // ERROR_PIPE_BUSY
                attempts += 1;
                std::thread::sleep(Duration::from_millis(50));
            },
            result => return result,
        }
    }
}

/// 64-bit FNV-1a, which unlike `std`'s hashers stays the same across builds.
#[cfg(windows)]
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

struct PlatformListener {
    #[cfg(unix)]
    listener: tokio::net::UnixListener,
    #[cfg(unix)]
    path: PathBuf,
    #[cfg(windows)]
    pipe_name: std::ffi::OsString,
    #[cfg(windows)]
    pipe: tokio::net::windows::named_pipe::NamedPipeServer,
}

struct PlatformServerStream {
    #[cfg(unix)]
    stream: tokio::net::UnixStream,
    #[cfg(windows)]
    server: tokio::net::windows::named_pipe::NamedPipeServer,
}

#[cfg(unix)]
impl PlatformListener {
    fn bind(local_path: &Path) -> std::io::Result<Self> {
        Ok(Self {
            listener: tokio::net::UnixListener::bind(local_path)?,
            path: local_path.to_path_buf(),
        })
    }

    async fn accept(&mut self) -> std::io::Result<PlatformServerStream> {
        let (stream, _) = self.listener.accept().await?;
        Ok(PlatformServerStream { stream })
    }

    fn close(self) {
        drop(self.listener);
        _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(windows)]
impl PlatformListener {
    fn bind(pipe_name: &OsStr) -> std::io::Result<Self> {
        let pipe = tokio::net::windows::named_pipe::ServerOptions::new()
            .access_outbound(false)
            .first_pipe_instance(true)
            .create(pipe_name)?;

        Ok(Self {
            pipe_name: pipe_name.to_os_string(),
            pipe,
        })
    }

    async fn accept(&mut self) -> std::io::Result<PlatformServerStream> {
        let connected = self.pipe.connect().await;

        // Whether or not that worked, this instance is spent: one whose client went away before it
        // was accepted fails every connect from then on, and nobody else could get through
        let next = tokio::net::windows::named_pipe::ServerOptions::new()
            .access_outbound(false)
            .create(&self.pipe_name)?;
        let pipe = std::mem::replace(&mut self.pipe, next);

        connected?;
        Ok(PlatformServerStream { server: pipe })
    }

    fn close(self) {}
}

impl PlatformServerStream {
    #[cfg(unix)]
    fn project(self: std::pin::Pin<&mut Self>) -> std::pin::Pin<&mut tokio::net::UnixStream> {
        unsafe { self.map_unchecked_mut(|s| &mut s.stream) }
    }

    #[cfg(windows)]
    fn project(self: std::pin::Pin<&mut Self>) -> std::pin::Pin<&mut tokio::net::windows::named_pipe::NamedPipeServer> {
        unsafe { self.map_unchecked_mut(|s| &mut s.server) }
    }
}

impl tokio::io::AsyncRead for PlatformServerStream {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        tokio::io::AsyncRead::poll_read(self.project(), cx, buf)
    }
}

fn run_cli(cli: Cli, frontend: &FrontendHandle, backend: &BackendHandle) {
    frontend.send(MessageToFrontend::OpenOrFocusMainWindow);

    if let Some(run_instance) = cli.run_instance {
        backend.send(bridge::message::MessageToBackend::StartInstanceByName {
            name: run_instance,
            quick_play: None,
        });
    }
}

fn setup_launcher_logging(launcher_dir: &Path) {
    let log_file = launcher_dir.join("launcher.log");
    if log_file.exists() {
        let old_log_file = launcher_dir.join("launcher.log.old");
        _ = std::fs::rename(&log_file, old_log_file);
    }

    if let Err(error) = init_logging(log::LevelFilter::Debug, &log_file) {
        eprintln!("Unable to enable logging: {error:?}");
    }

    log::debug!("DEBUG logging enabled");
    log::trace!("TRACE logging enabled");

    panic::install_logging_hook();
}

fn show_error(error: String) {
    log::error!("{}", error);
    _ = DialogBuilder::message()
        .set_level(native_dialog::MessageLevel::Error)
        .set_title("An error occurred")
        .set_text(error)
        .alert()
        .show();
}

fn show_error_eprintln(error: String) {
    eprintln!("{}", error);
    _ = DialogBuilder::message()
        .set_level(native_dialog::MessageLevel::Error)
        .set_title("An error occurred")
        .set_text(error)
        .alert()
        .show();
}

fn start_deadlock_detection(
    deadlock_message: &Arc<parking_lot::lock_api::RwLock<parking_lot::RawRwLock, Option<String>>>,
    frontend_handle: &bridge::handle::FrontendHandle,
) {
    std::thread::spawn({
        let deadlock_message = deadlock_message.clone();
        let frontend_handle = frontend_handle.clone();
        move || {
            loop {
                std::thread::sleep(std::time::Duration::from_secs(10));
                let deadlocks = parking_lot::deadlock::check_deadlock();
                if deadlocks.is_empty() {
                    continue;
                }

                let mut message = String::new();
                _ = writeln!(&mut message, "{} deadlock(s) detected", deadlocks.len());
                for (i, threads) in deadlocks.iter().enumerate() {
                    _ = writeln!(&mut message, "==== Deadlock #{} ({} threads) ====", i, threads.len());
                    for (thread_index, t) in threads.iter().enumerate() {
                        _ = writeln!(&mut message, "== Thread #{} ({:?}) ==", thread_index, t.thread_id());
                        _ = writeln!(&mut message, "{:#?}", t.backtrace());
                    }
                }

                log::error!("{}", message);
                *deadlock_message.write() = Some(message);
                frontend_handle.send(MessageToFrontend::Refresh);
                return;
            }
        }
    });
}

fn init_logging(level: log::LevelFilter, log_file: &Path) -> Result<(), fern::InitError> {
    let base_config = fern::Dispatch::new()
        .level_for("pandora_launcher", level)
        .level_for("auth", level)
        .level_for("backend", level)
        .level_for("frontend", level)
        .level_for("bridge", level)
        .level_for("command", level)
        .level_for("gpui_component::text", log::LevelFilter::Off)
        .level(log::LevelFilter::Warn);

    let colors_line = ColoredLevelConfig::new().info(fern::colors::Color::BrightWhite);

    let file_config = fern::Dispatch::new()
        .format(|out, message, record| {
            out.finish(format_args!(
                "[{time} {level} {target}] {message}",
                time = humantime::format_rfc3339_seconds(SystemTime::now()),
                level = record.level(),
                target = record.target(),
                message = message
            ))
        })
        .chain(fern::log_file(log_file)?);

    let stdout_config = fern::Dispatch::new()
        .format(move |out, message, record| {
            out.finish(format_args!(
                "{color_line}[{time} {level} {target}{color_line}] {message}\x1B[0m",
                color_line = format_args!("\x1B[{}m", colors_line.get_color(&record.level()).to_fg_str()),
                time = humantime::format_rfc3339_seconds(SystemTime::now()),
                level = record.level(),
                target = record.target(),
                message = message
            ))
        })
        .chain(std::io::stdout());

    base_config.chain(file_config).chain(stdout_config).apply()?;

    Ok(())
}

fn get_portable_dir() -> Option<PathBuf> {
    let current_exe = std::env::current_exe().ok()?;
    let file_name = current_exe.file_name()?;
    let file_name = file_name.to_string_lossy();
    if file_name.to_lowercase().contains("portable") {
        Some(current_exe.parent()?.into())
    } else {
        None
    }
}
