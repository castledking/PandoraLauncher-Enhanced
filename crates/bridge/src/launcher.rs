//! What the running launcher was started from, which isn't always where its code is loaded from.
//!
//! An AppImage runs its binary out of a temporary mount that's gone once it exits, and a macOS
//! app runs one from inside its bundle, so anything that starts the launcher again (shortcuts,
//! restarting into an update) or replaces it goes through [`current`] rather than
//! [`std::env::current_exe`].

use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::OnceLock,
    time::SystemTime,
};

/// Set on a launcher started by [`spawn_restart`], which waits for the running one to exit
/// instead of handing over to it.
pub const RESTART_ENV: &str = "PANDORA_RESTART";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LauncherKind {
    AppImage,
    MacApp,
    Executable,
}

#[derive(Debug)]
pub struct LauncherInstall {
    pub kind: LauncherKind,
    /// What gets installed and replaced as one: the AppImage, the `.app` bundle or the executable.
    pub path: PathBuf,
    /// What gets run to start the launcher: the AppImage, the bundle's binary or the executable.
    pub executable: PathBuf,
    stamp: Option<FileStamp>,
}

impl LauncherInstall {
    /// Whether [`Self::executable`] has been swapped for another file since the launcher started,
    /// by an update or a reinstall. Starting it again would then run the new version.
    pub fn has_been_replaced(&self) -> bool {
        match (self.stamp, FileStamp::of(&self.executable)) {
            (Some(started), Some(now)) => started != now,
            _ => false,
        }
    }
}

static INSTALL: OnceLock<LauncherInstall> = OnceLock::new();

/// Records what the launcher was started from and, inside an AppImage, takes the AppImage's
/// changes back out of the environment.
///
/// # Safety
///
/// Edits the process environment, so it must be called before any other threads exist.
pub unsafe fn init() {
    INSTALL.get_or_init(detect);

    #[cfg(target_os = "linux")]
    unsafe {
        restore_environment_outside_appimage();
    }
}

pub fn current() -> &'static LauncherInstall {
    INSTALL.get_or_init(detect)
}

fn detect() -> LauncherInstall {
    let executable = std::env::current_exe().unwrap_or_default();

    let appimage = own_appimage(
        &executable,
        std::env::var_os("APPIMAGE").as_deref(),
        std::env::var_os("APPDIR").as_deref(),
    );
    if let Some(appimage) = appimage {
        return LauncherInstall {
            kind: LauncherKind::AppImage,
            stamp: FileStamp::of(&appimage),
            path: appimage.clone(),
            executable: appimage,
        };
    }

    let stamp = FileStamp::of(&executable);

    if cfg!(target_os = "macos")
        && let Some(app) = macos_app_bundle(&executable)
    {
        return LauncherInstall {
            kind: LauncherKind::MacApp,
            path: app.to_path_buf(),
            executable,
            stamp,
        };
    }

    LauncherInstall {
        kind: LauncherKind::Executable,
        path: executable.clone(),
        executable,
        stamp,
    }
}

/// The `Foo.app` an executable at `Foo.app/Contents/MacOS/foo` belongs to.
fn macos_app_bundle(executable: &Path) -> Option<&Path> {
    let macos = executable.parent()?;
    let contents = macos.parent()?;
    let app = contents.parent()?;

    let is_bundle = macos.file_name()? == "MacOS" && contents.file_name()? == "Contents" && app.extension()? == "app";
    is_bundle.then_some(app)
}

/// Starts the launcher at `executable`, which waits for this one to exit before it opens.
pub fn spawn_restart(executable: &Path) -> std::io::Result<()> {
    let mut command = Command::new(executable);
    command
        .env(RESTART_ENV, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    // Keep it out of the terminal's job control, so stopping the old launcher there leaves it be
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);

    command.spawn().map(drop)
}

/// The AppImage the launcher is running from, going by the variables the AppImage runtime sets.
/// They're only the launcher's own when it runs from inside the AppImage's mount. A launcher
/// started from a terminal in some other AppImage inherits that one's, and taking them at their
/// word would have updating write the launcher over that other app.
fn own_appimage(executable: &Path, appimage: Option<&OsStr>, appdir: Option<&OsStr>) -> Option<PathBuf> {
    let appimage = appimage.filter(|appimage| !appimage.is_empty())?;
    let appdir = Path::new(appdir.filter(|appdir| !appdir.is_empty())?);
    let appdir = std::fs::canonicalize(appdir).unwrap_or_else(|_| appdir.to_path_buf());
    executable.starts_with(appdir).then(|| PathBuf::from(appimage))
}

/// Enough of a file's metadata to tell it apart from a file put in its place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileStamp {
    len: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    inode: (u64, u64),
}

impl FileStamp {
    fn of(path: &Path) -> Option<Self> {
        let metadata = std::fs::metadata(path).ok()?;

        Some(Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            #[cfg(unix)]
            inode: {
                use std::os::unix::fs::MetadataExt;
                (metadata.dev(), metadata.ino())
            },
        })
    }
}

/// An AppImage's `AppRun` puts the AppImage's own folders first in `LD_LIBRARY_PATH`, `PATH`,
/// `XDG_DATA_DIRS` and friends. The launcher's own libraries are already loaded by then, so all
/// that's left for those variables to do is leak into everything the launcher starts: the file
/// manager, the browser and the game then pick up the bundled libraries (an older OpenSSL, for
/// one) and fail to start.
#[cfg(target_os = "linux")]
unsafe fn restore_environment_outside_appimage() {
    use std::os::unix::ffi::OsStrExt;

    let Some(appdir) = std::env::var_os("APPDIR").filter(|appdir| !appdir.is_empty()) else {
        return;
    };
    let appdir = PathBuf::from(appdir);

    for (key, value) in std::env::vars_os() {
        let mentions_appdir = value
            .as_bytes()
            .windows(appdir.as_os_str().len())
            .any(|window| window == appdir.as_os_str().as_bytes());
        if !mentions_appdir {
            continue;
        }

        // SAFETY: the caller guarantees no other threads exist yet
        unsafe {
            match without_appdir_entries(&value, &appdir) {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }

    // Set by the AppImage runtime for whatever it runs, which isn't what we start
    for key in ["APPDIR", "APPIMAGE", "ARGV0", "OWD"] {
        // SAFETY: the caller guarantees no other threads exist yet
        unsafe { std::env::remove_var(key) };
    }
}

/// A `:`-separated list of paths without the ones inside `appdir`, or `None` if nothing is left.
#[cfg(target_os = "linux")]
fn without_appdir_entries(value: &std::ffi::OsStr, appdir: &Path) -> Option<std::ffi::OsString> {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};

    let kept: Vec<&[u8]> = value
        .as_bytes()
        .split(|byte| *byte == b':')
        .filter(|entry| !entry.is_empty() && !Path::new(std::ffi::OsStr::from_bytes(entry)).starts_with(appdir))
        .collect();

    if kept.is_empty() {
        None
    } else {
        Some(std::ffi::OsString::from_vec(kept.join(&b':')))
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{macos_app_bundle, own_appimage};

    #[test]
    fn only_takes_the_appimage_it_runs_from() {
        use std::ffi::OsStr;

        let appimage = Some(OsStr::new("/home/me/Applications/Pandora.AppImage"));
        let appdir = Some(OsStr::new("/tmp/.mount_PandorXyz"));

        assert_eq!(
            own_appimage(Path::new("/tmp/.mount_PandorXyz/usr/bin/PandoraLauncher"), appimage, appdir),
            Some(Path::new("/home/me/Applications/Pandora.AppImage").to_path_buf())
        );
        // Started from a terminal inside another AppImage, whose variables it inherited
        assert_eq!(
            own_appimage(
                Path::new("/home/me/pandora_launcher"),
                Some(OsStr::new("/home/me/Applications/Editor.AppImage")),
                Some(OsStr::new("/tmp/.mount_EditorAbc"))
            ),
            None
        );
        assert_eq!(
            own_appimage(Path::new("/tmp/.mount_PandorXyz/usr/bin/PandoraLauncher"), appimage, None),
            None
        );
        assert_eq!(
            own_appimage(Path::new("/tmp/.mount_PandorXyz/usr/bin/PandoraLauncher"), Some(OsStr::new("")), appdir),
            None
        );
    }

    #[test]
    fn finds_the_bundle_an_executable_is_in() {
        assert_eq!(
            macos_app_bundle(Path::new("/Applications/Pandora Launcher.app/Contents/MacOS/pandora_launcher")),
            Some(Path::new("/Applications/Pandora Launcher.app"))
        );
        assert_eq!(macos_app_bundle(Path::new("/usr/bin/pandora_launcher")), None);
        assert_eq!(macos_app_bundle(Path::new("/Applications/Foo/Contents/MacOS/pandora_launcher")), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn drops_only_the_appimage_entries() {
        use std::ffi::OsStr;

        use super::without_appdir_entries;

        let appdir = Path::new("/tmp/.mount_PandorXyz");

        // What AppRun makes of an LD_LIBRARY_PATH that was empty before
        assert_eq!(
            without_appdir_entries(
                OsStr::new("/tmp/.mount_PandorXyz/usr/lib/:/tmp/.mount_PandorXyz/usr/lib64/:"),
                appdir
            ),
            None
        );

        assert_eq!(
            without_appdir_entries(
                OsStr::new("/tmp/.mount_PandorXyz/usr/bin/:/tmp/.mount_PandorXyz/bin/:/usr/local/bin:/usr/bin"),
                appdir
            )
            .as_deref(),
            Some(OsStr::new("/usr/local/bin:/usr/bin"))
        );

        // A neighbouring folder that merely shares the prefix stays
        assert_eq!(
            without_appdir_entries(OsStr::new("/tmp/.mount_PandorXyzOther/usr/lib"), appdir).as_deref(),
            Some(OsStr::new("/tmp/.mount_PandorXyzOther/usr/lib"))
        );
    }
}
