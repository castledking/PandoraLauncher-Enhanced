use std::{
    ffi::{OsStr, OsString},
    io::Cursor,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use base64::Engine;
use bridge::{handle::FrontendHandle, launcher::LauncherKind, message::MessageToFrontend, modal_action::ModalAction};
use rand::RngCore;
use reqwest::StatusCode;
use schema::{
    forge::VersionFragment,
    pandora_update::{ReleaseNotes, UpdateInstallType, UpdateManifest, UpdatePrompt},
};
use serde::Deserialize;
use sha1::{Digest, Sha1};

use crate::{HttpClientProvider, directories::LauncherDirectories};

/// How long to wait between looking for updates. A release can become available while the launcher
/// is open, including after a temporary 404 while its assets are still being published.
const UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// Set once an update is in place, after which there's nothing newer to find until the restart.
static UPDATE_INSTALLED: AtomicBool = AtomicBool::new(false);

pub async fn check_for_updates_periodically(http_client: HttpClientProvider, send: FrontendHandle) {
    // Problems are only reported for the check at startup. A later one failing because the
    // connection dropped for a moment isn't worth interrupting a game for
    let mut quiet = false;
    loop {
        if !UPDATE_INSTALLED.load(Ordering::Relaxed) {
            check_for_updates(http_client.redirecting(), send.clone(), quiet).await;
        }
        quiet = true;
        tokio::time::sleep(UPDATE_CHECK_INTERVAL).await;
    }
}

pub async fn check_for_updates(http_client: reqwest::Client, send: FrontendHandle, quiet: bool) {
    if option_env!("PANDORA_UPDATE_PUBKEY").is_none() {
        return;
    }

    let Some(version) = option_env!("PANDORA_RELEASE_VERSION") else {
        log::warn!("Skipping update check because PANDORA_RELEASE_VERSION isn't set");

        #[cfg(not(debug_assertions))] // Don't show error in non-release builds
        if !quiet {
            send.send_warning("Unable to check for updates, missing PANDORA_RELEASE_VERSION");
        }
        return;
    };

    let Some(repository_url) = option_env!("GITHUB_REPOSITORY_URL") else {
        log::warn!("Skipping update check because GITHUB_REPOSITORY_URL isn't set");

        #[cfg(not(debug_assertions))] // Don't show error in non-release builds
        if !quiet {
            send.send_warning("Unable to check for updates, missing GITHUB_REPOSITORY_URL");
        }
        return;
    };

    let report_error = |message: String| {
        if !quiet {
            send.send_error(message);
        }
    };

    let current_version = VersionFragment::string_to_parts(version);

    let manifest_names = [
        format!("update_{}.json", std::env::consts::OS),
        format!("update_manifest_{}.json", std::env::consts::OS),
    ];
    let mut last_status = None;
    let mut manifest_bytes = None;

    for manifest_name in manifest_names {
        let url = format!("{repository_url}/releases/latest/download/{manifest_name}");
        let response = http_client.get(url).send().await;

        let response = match response {
            Ok(response) => response,
            Err(err) => {
                log::error!("Error while requesting update manifest: {}", err);
                report_error("Unable to fetch Pandora update manifest, see logs for more details".into());
                return;
            },
        };

        if response.status() != StatusCode::OK {
            last_status = Some(response.status());
            continue;
        }

        manifest_bytes = match response.bytes().await {
            Ok(manifest_bytes) => Some(manifest_bytes),
            Err(err) => {
                log::error!("Error while downloading update manifest: {}", err);
                report_error("Unable to download Pandora update manifest, see logs for more details".into());
                return;
            },
        };
        break;
    }

    let Some(manifest_bytes) = manifest_bytes else {
        log::error!("Unable to fetch update manifest: {:?}", last_status);
        report_error(format!(
            "Unable to fetch Pandora update manifest, non-200 status code: {}",
            last_status.unwrap_or(StatusCode::NOT_FOUND)
        ));
        return;
    };

    let manifest = match serde_json::from_slice::<UpdateManifest>(&manifest_bytes) {
        Ok(manifest) => manifest,
        Err(err) => {
            log::error!("Error while parsing update manifest: {}", err);
            report_error("Unable to parse update manifest, see logs for more details".into());
            return;
        },
    };

    let update_version = VersionFragment::string_to_parts(&manifest.version);

    if current_version >= update_version {
        log::info!("Pandora is up-to-date");
        return;
    }

    let exes = if let Some(universal) = manifest.downloads.archs.get("universal") {
        universal
    } else if let Some(exes) = manifest.downloads.archs.get(std::env::consts::ARCH) {
        exes
    } else {
        log::warn!(
            "Unable to update, can't find arch \"{}\" in {:?}",
            std::env::consts::ARCH,
            manifest.downloads.archs.keys()
        );
        return;
    };

    let Some(install_type) = determine_update_install_type() else {
        log::warn!("Unable to update, can't determine installation type");
        return;
    };

    let install_type_key = install_type.key();
    let Some(executable) = exes.exes.get(install_type_key) else {
        log::warn!(
            "Unable to update, installation type \"{}\" not in {:?}",
            install_type_key,
            exes.exes.keys()
        );
        return;
    };

    send.send(MessageToFrontend::UpdateAvailable {
        update: UpdatePrompt {
            old_version: version.into(),
            new_version: manifest.version.clone(),
            install_type,
            exe: executable.clone(),
        },
    });
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: Arc<str>,
    name: Option<Arc<str>>,
    body: Option<Arc<str>>,
    published_at: Option<Arc<str>>,
    html_url: Arc<str>,
    #[serde(default)]
    draft: bool,
}

/// The notes of every release, newest first. They come from the GitHub releases, the same place
/// the updates themselves do.
pub async fn fetch_release_notes(http_client: reqwest::Client) -> Result<Arc<[ReleaseNotes]>, Arc<str>> {
    let Some(repository_url) = option_env!("GITHUB_REPOSITORY_URL") else {
        return Err("This build doesn't know where its releases are published".into());
    };
    let Some(repository) = repository_url.strip_prefix("https://github.com/") else {
        return Err(format!("Release notes can only be fetched from GitHub, not {repository_url}").into());
    };

    // GitHub hands them out 100 at a time
    let mut releases: Vec<GithubRelease> = Vec::new();
    for page in 1..=5 {
        let url = format!("https://api.github.com/repos/{repository}/releases?per_page=100&page={page}");
        let response = http_client
            .get(url)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|err| {
                log::error!("Error while requesting release notes: {err}");
                Arc::<str>::from(format!("Unable to fetch the release notes: {err}"))
            })?;
        let page: Vec<GithubRelease> = response.json().await.map_err(|err| {
            log::error!("Error while reading release notes: {err}");
            Arc::<str>::from(format!("Unable to read the release notes: {err}"))
        })?;

        let last_page = page.len() < 100;
        releases.extend(page);
        if last_page {
            break;
        }
    }

    let mut notes: Vec<(Vec<VersionFragment>, ReleaseNotes)> = releases
        .into_iter()
        .filter(|release| !release.draft)
        .map(|release| {
            let version = release.tag_name.trim_start_matches('v');
            let notes = ReleaseNotes {
                version: version.into(),
                title: release.name.filter(|name| !name.trim().is_empty()).unwrap_or_else(|| version.into()),
                published: release.published_at.map(|date| date.get(..10).unwrap_or(&date).into()),
                url: release.html_url,
                body: release.body.unwrap_or_default(),
            };
            (VersionFragment::string_to_parts(version), notes)
        })
        .collect();

    if notes.is_empty() {
        return Err("No releases were found".into());
    }

    notes.sort_by(|(a, _), (b, _)| b.cmp(a));
    Ok(notes.into_iter().map(|(_, notes)| notes).collect())
}

fn determine_update_install_type() -> Option<UpdateInstallType> {
    let install = bridge::launcher::current();

    if install.executable.as_os_str().is_empty() {
        return None;
    }

    Some(match install.kind {
        LauncherKind::AppImage => UpdateInstallType::AppImage(install.path.clone()),
        LauncherKind::MacApp => UpdateInstallType::App(install.path.clone()),
        LauncherKind::Executable => UpdateInstallType::Executable,
    })
}

pub async fn install_update(
    http_client: reqwest::Client,
    dirs: Arc<LauncherDirectories>,
    send: FrontendHandle,
    update: UpdatePrompt,
    modal_action: ModalAction,
) {
    let version = update.new_version.clone();

    match install_update_inner(http_client, dirs, update, modal_action.clone()).await {
        Ok(relaunch) => {
            UPDATE_INSTALLED.store(true, Ordering::Relaxed);
            send.send(MessageToFrontend::UpdateInstalled { version, relaunch });
        },
        Err(error) => modal_action.set_finished_with_error(error),
    }

    modal_action.set_finished();
    send.send(MessageToFrontend::Refresh);
}

/// Downloads and installs the update, returning what to run to start the new version.
async fn install_update_inner(
    http_client: reqwest::Client,
    dirs: Arc<LauncherDirectories>,
    update: UpdatePrompt,
    modal_action: ModalAction,
) -> Result<PathBuf, Arc<str>> {
    let title = format!("Downloading Pandora {}", update.new_version);
    let tracker = modal_action.push_tracker(title.into());

    let mut expected_hash = [0u8; 20];
    let Ok(_) = hex::decode_to_slice(&*update.exe.sha1, &mut expected_hash) else {
        return Err("Unable to decode sha1 hash".into());
    };

    let Ok(response) = http_client.get(&*update.exe.download).send().await else {
        return Err("Error making download request".into());
    };

    if response.status() != StatusCode::OK {
        return Err("Download URL returned non-200 status code".into());
    }

    tracker.set_total(update.exe.size);

    use futures::StreamExt;
    let mut stream = response.bytes_stream();

    let mut bytes = Vec::new();

    while let Some(item) = stream.next().await {
        let Ok(item) = item else {
            return Err("Error while downloading update".into());
        };

        bytes.extend_from_slice(&*item);
        tracker.add_count(item.len());
    }

    let mut hasher = Sha1::new();
    hasher.update(&bytes);
    let actual_hash = hasher.finalize();

    if expected_hash != *actual_hash {
        return Err("Hash of downloaded file does not match".into());
    }

    let Some(pubkey) = option_env!("PANDORA_UPDATE_PUBKEY") else {
        return Err("Unable to update, missing PANDORA_UPDATE_PUBKEY at compile time".into());
    };

    let pk = base64::engine::general_purpose::STANDARD
        .decode(pubkey)
        .ok()
        .and_then(|pubkey| String::from_utf8(pubkey).ok())
        .and_then(|pubkey| minisign_verify::PublicKey::decode(&pubkey).ok());
    let Some(pk) = pk else {
        return Err("Unable to update, PANDORA_UPDATE_PUBKEY is not a valid public key".into());
    };

    let signature = base64::engine::general_purpose::STANDARD
        .decode(&*update.exe.sig)
        .ok()
        .and_then(|sig| String::from_utf8(sig).ok())
        .and_then(|sig| minisign_verify::Signature::decode(&sig).ok());
    let Some(signature) = signature else {
        return Err("Invalid signature, unable to decode it".into());
    };

    match pk.verify(&bytes, &signature, false) {
        Err(minisign_verify::Error::InvalidSignature) => {
            return Err("Invalid signature, file was not properly signed".into());
        },
        Err(err) => {
            return Err(format!("Error while validating signature: {:?}", err).into());
        },
        Ok(_) => {},
    }

    // Replacing the launcher can wait on a password prompt, which mustn't hold up the backend
    match tokio::task::spawn_blocking(move || replace_launcher(update, &bytes, &dirs)).await {
        Ok(result) => result,
        Err(err) => {
            log::error!("Update install task failed: {err}");
            Err("Error while installing update, see logs for more details".into())
        },
    }
}

/// Puts the verified update in place of the running launcher, returning what to run to start it.
fn replace_launcher(update: UpdatePrompt, bytes: &[u8], dirs: &LauncherDirectories) -> Result<PathBuf, Arc<str>> {
    match update.install_type {
        UpdateInstallType::AppImage(appimage) => {
            let Some(filename) = appimage.file_name() else {
                return Err("Appimage path has no filename".into());
            };

            // This is temporary to address pre-3.3.0 including the version inside the filename
            // This should be removed at some point in the near future
            let new_filename = replace_os_str(filename, &format!("-{}", &update.old_version), "");
            let new_appimage = appimage.with_file_name(new_filename);

            replace_exe(appimage.clone(), new_appimage.clone(), bytes, dirs)?;

            // AppImageLauncher keys its generated desktop entries on the AppImage's path, so
            // updating (and especially moving) the AppImage leaves the old entry behind and the
            // launcher shows up several times in menus like rofi.
            cleanup_appimage_desktop_entries(Some(&new_appimage));

            Ok(new_appimage)
        },
        UpdateInstallType::Executable => {
            let current_exe = bridge::launcher::current().executable.clone();

            let Some(filename) = current_exe.file_name() else {
                return Err("Current exe path has no filename".into());
            };

            // This is temporary to address pre-3.3.0 including the version inside the filename
            // This should be removed at some point in the near future
            let new_filename = replace_os_str(filename, &format!("-{}", &update.old_version), "");
            let new_exe = current_exe.with_file_name(new_filename);

            replace_exe(current_exe, new_exe.clone(), bytes, dirs)?;

            Ok(new_exe)
        },
        UpdateInstallType::App(current_app_folder) => {
            let mut temp_extract = dirs.temp_dir.join(format!("app_unpack_{}", rand::thread_rng().next_u64()));
            while temp_extract.exists() {
                log::warn!(
                    "Randomly generated app_unpack folder exists... what are the chances? ({:?})",
                    temp_extract
                );
                temp_extract = dirs.temp_dir.join(format!("app_unpack_{}", rand::thread_rng().next_u64()));
            }

            let mut temp_backup = dirs.temp_dir.join(format!("app_backup_{}", rand::thread_rng().next_u64()));
            while temp_backup.exists() {
                log::warn!("Randomly generated app_backup folder exists... what are the chances? ({:?})", temp_backup);
                temp_backup = dirs.temp_dir.join(format!("app_backup_{}", rand::thread_rng().next_u64()));
            }

            let result = install_app_update(current_app_folder, bytes, &temp_extract, &temp_backup);

            _ = std::fs::remove_dir_all(temp_backup);
            _ = std::fs::remove_dir_all(temp_extract);

            result?;

            // The bundle was replaced where it was, so its executable is where ours was
            Ok(bridge::launcher::current().executable.clone())
        },
    }
}

/// Removes redundant AppImageLauncher-generated desktop entries for this launcher.
///
/// AppImageLauncher writes one `appimagekit_<md5-of-path>-<Name>.desktop` file per *path* an
/// AppImage is integrated from. A launcher that self-updates in place, or that is reachable
/// through both a symlink and its real filename, therefore accumulates several entries that all
/// start the same program, which is what shows up as duplicate listings in rofi and friends.
///
/// Only files generated by AppImageLauncher are ever deleted; hand-written `.desktop` files are
/// left alone, and are treated as the entry the user actually wants.
///
/// `current` is the AppImage to treat as the running one, defaulting to the one we were started from.
pub fn cleanup_appimage_desktop_entries(current: Option<&Path>) {
    let current = match current {
        Some(current) => current.to_path_buf(),
        None => {
            let install = bridge::launcher::current();
            // Not running as an AppImage, so there is nothing AppImageLauncher could have made
            if install.kind != LauncherKind::AppImage {
                return;
            }
            install.path.clone()
        },
    };

    let Some(dir) = desktop_entry_dir() else {
        return;
    };

    cleanup_appimage_desktop_entries_in(&dir, &current);
}

fn cleanup_appimage_desktop_entries_in(dir: &Path, current: &Path) {
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return;
    };

    let canonical_current = try_canonicalize(current).unwrap_or_else(|| current.to_path_buf());

    let mut generated = Vec::new();
    let mut handwritten_names = Vec::new();

    for entry in read_dir.flatten() {
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();

        if !file_name.ends_with(".desktop") {
            continue;
        }

        let Some(desktop_entry) = read_desktop_entry(&entry.path()) else {
            continue;
        };

        if file_name.starts_with("appimagekit_") {
            generated.push((file_name.into_owned(), desktop_entry));
        } else {
            handwritten_names.push(undisambiguated_name(&desktop_entry.name).to_owned());
        }
    }

    // Prefer keeping the entry that points at the exact path we're running from, then fall back to
    // a stable order so repeated runs don't keep swapping which entry survives
    generated.sort_by(|(a_file, a), (b_file, b)| {
        (b.exec == current).cmp(&(a.exec == current)).then_with(|| a_file.cmp(b_file))
    });

    let mut kept: Vec<PathBuf> = Vec::new();

    for (file_name, desktop_entry) in generated {
        if !is_pandora_appimage(&desktop_entry) {
            continue;
        }

        let path = dir.join(&file_name);

        if !desktop_entry.exec.exists() {
            remove_desktop_entry(&path, "it points at an AppImage that no longer exists");
            continue;
        }

        let canonical = try_canonicalize(&desktop_entry.exec).unwrap_or_else(|| desktop_entry.exec.clone());

        if kept.contains(&canonical) {
            remove_desktop_entry(&path, "another entry already starts the same AppImage");
            continue;
        }

        // The user maintains their own entry for this launcher (which is why AppImageLauncher had
        // to disambiguate the name in the first place), so ours is just a duplicate listing
        if canonical == canonical_current
            && handwritten_names.iter().any(|name| name == undisambiguated_name(&desktop_entry.name))
        {
            remove_desktop_entry(&path, "a hand-written desktop entry with the same name exists");
            continue;
        }

        kept.push(canonical);
    }
}

fn remove_desktop_entry(path: &Path, reason: &str) {
    match std::fs::remove_file(path) {
        Ok(()) => log::info!("Removed duplicate desktop entry {:?} because {}", path, reason),
        Err(err) => log::warn!("Unable to remove duplicate desktop entry {:?}: {}", path, err),
    }
}

fn desktop_entry_dir() -> Option<PathBuf> {
    if let Some(data_home) = std::env::var_os("XDG_DATA_HOME")
        && !data_home.is_empty()
    {
        return Some(PathBuf::from(data_home).join("applications"));
    }

    Some(PathBuf::from(std::env::var_os("HOME")?).join(".local/share/applications"))
}

struct DesktopEntry {
    name: String,
    exec: PathBuf,
}

/// Reads `Name` and the program part of `Exec` from the `[Desktop Entry]` group, ignoring the
/// `[Desktop Action ...]` groups AppImageLauncher appends (they have their own `Exec` lines).
fn read_desktop_entry(path: &Path) -> Option<DesktopEntry> {
    let content = std::fs::read_to_string(path).ok()?;

    let mut in_desktop_entry = false;
    let mut name = None;
    let mut exec = None;

    for line in content.lines() {
        let line = line.trim();

        if let Some(group) = line.strip_prefix('[').and_then(|line| line.strip_suffix(']')) {
            if in_desktop_entry {
                break;
            }
            in_desktop_entry = group == "Desktop Entry";
            continue;
        }

        if !in_desktop_entry {
            continue;
        }

        if let Some(value) = line.strip_prefix("Name=") {
            name.get_or_insert_with(|| value.trim().to_owned());
        } else if let Some(value) = line.strip_prefix("Exec=") {
            exec.get_or_insert_with(|| exec_program(value.trim()));
        }
    }

    Some(DesktopEntry {
        name: name?,
        exec: PathBuf::from(exec?),
    })
}

/// Extracts the program from an `Exec` value, dropping arguments and field codes like `%U`.
fn exec_program(exec: &str) -> String {
    if let Some(rest) = exec.strip_prefix('"') {
        return rest.split('"').next().unwrap_or(rest).to_owned();
    }

    exec.split(' ').next().unwrap_or(exec).to_owned()
}

/// Strips the ` (2)` style suffix desktop environments add when two entries share a name.
fn undisambiguated_name(name: &str) -> &str {
    let Some(rest) = name.strip_suffix(')') else {
        return name;
    };

    let Some((base, counter)) = rest.rsplit_once(" (") else {
        return name;
    };

    if counter.is_empty() || !counter.bytes().all(|byte| byte.is_ascii_digit()) {
        return name;
    }

    base
}

fn is_pandora_appimage(desktop_entry: &DesktopEntry) -> bool {
    let matches_filename = desktop_entry
        .exec
        .file_name()
        .is_some_and(|name| name.to_string_lossy().contains("PandoraLauncher"));

    matches_filename || undisambiguated_name(&desktop_entry.name) == "Pandora Launcher"
}

/// `path` with `.<random number>.<extension>` added, picking a name that isn't taken.
fn add_random_extension(path: &Path, extension: &str) -> PathBuf {
    loop {
        let candidate = path.with_added_extension(format!("{}.{extension}", rand::thread_rng().next_u64()));
        if !candidate.exists() {
            return candidate;
        }
        log::warn!("Randomly generated file name exists... what are the chances? ({:?})", candidate);
    }
}

/// Deletes what an earlier update left beside the launcher: on Windows the old executable that
/// was moved aside to make room, and anywhere a downloaded update that never made it into place.
pub fn cleanup_previous_update() {
    let executable = &bridge::launcher::current().executable;
    let (Some(dir), Some(name)) = (executable.parent(), executable.file_name()) else {
        return;
    };
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in read_dir.flatten() {
        if !is_left_over_from_update(name, &entry.file_name()) {
            continue;
        }

        match std::fs::remove_file(entry.path()) {
            Ok(()) => log::info!("Removed {:?} left over from an earlier update", entry.path()),
            // Most likely an older launcher that hasn't exited yet
            Err(err) => log::debug!("Unable to remove {:?} left over from an earlier update: {err}", entry.path()),
        }
    }
}

/// Whether `file_name` is one of the `<executable>.<random number>.new`/`.old` files
/// [`add_random_extension`] names.
fn is_left_over_from_update(executable_name: &OsStr, file_name: &OsStr) -> bool {
    let (Some(executable_name), Some(file_name)) = (executable_name.to_str(), file_name.to_str()) else {
        return false;
    };
    let Some(rest) = file_name.strip_prefix(executable_name).and_then(|rest| rest.strip_prefix('.')) else {
        return false;
    };
    let Some(random) = rest.strip_suffix(".new").or_else(|| rest.strip_suffix(".old")) else {
        return false;
    };

    !random.is_empty() && random.bytes().all(|byte| byte.is_ascii_digit())
}

fn write_new_exe_temp(new_exe: &Path, data: &[u8], dirs: &LauncherDirectories) -> Result<PathBuf, String> {
    let new_exe_data = add_random_extension(new_exe, "new");

    let Err(err) = std::fs::write(&new_exe_data, data) else {
        return Ok(new_exe_data);
    };

    if err.kind() != std::io::ErrorKind::PermissionDenied {
        log::error!("Error while writing new executable: {}", err);
        return Err("Error while writing new executable, see logs for more details".into());
    }

    let new_exe_data = add_random_extension(&dirs.temp_dir.join("new_exe_data"), "new");

    if let Err(err) = std::fs::write(&new_exe_data, data) {
        log::error!("Error while writing new executable: {}", err);
        return Err("Error while writing new executable, see logs for more details".into());
    }

    Ok(new_exe_data)
}

fn replace_exe(old_exe: PathBuf, new_exe: PathBuf, data: &[u8], dirs: &LauncherDirectories) -> Result<(), String> {
    let new_exe_temp = write_new_exe_temp(&new_exe, data, dirs)?;

    #[cfg(unix)]
    let result = move_new_exe_into(old_exe, new_exe, &new_exe_temp);
    #[cfg(windows)]
    let result = swap_exe_into_place(&old_exe, &new_exe, &new_exe_temp);

    // Already moved into place unless something went wrong
    _ = std::fs::remove_file(new_exe_temp);
    result
}

fn try_canonicalize(path: &Path) -> Option<PathBuf> {
    let canonical = path.canonicalize().ok()?;

    if cfg!(windows) {
        let path_bytes = path.as_os_str().as_encoded_bytes();
        let canonical_bytes = canonical.as_os_str().as_encoded_bytes();
        if canonical_bytes.len() == path_bytes.len() + 4
            && &canonical_bytes[..4] == b"\\\\?\\"
            && &canonical_bytes[4..] == path_bytes
        {
            None
        } else {
            Some(canonical)
        }
    } else {
        Some(canonical)
    }
}

/// Windows won't let a running executable be overwritten or deleted, but it will let it be
/// renamed. So the running one is moved aside and the update takes its name straight away, ready
/// for the next start, rather than waiting on a helper to do it once the launcher has exited. The
/// copy moved aside is deleted on the next start by [`cleanup_previous_update`].
#[cfg(windows)]
fn swap_exe_into_place(old_exe: &Path, new_exe: &Path, new_exe_temp: &Path) -> Result<(), String> {
    let aside = add_random_extension(old_exe, "old");

    if let Err(err) = std::fs::rename(old_exe, &aside) {
        if err.kind() == std::io::ErrorKind::PermissionDenied {
            log::info!("Permission denied while moving the running executable aside, need to elevate");
            return swap_exe_into_place_elevated(old_exe, new_exe, new_exe_temp, &aside);
        }

        log::error!("Unable to move the running executable {:?} aside: {}", old_exe, err);
        return Err("Error while replacing executable, see logs for more details".into());
    }

    if let Err(err) = rename_retrying(new_exe_temp, new_exe) {
        log::error!("Unable to move the update {:?} into place at {:?}: {}", new_exe_temp, new_exe, err);
        if let Err(err) = std::fs::rename(&aside, old_exe) {
            log::error!("Unable to put the running executable back from {:?}: {}", aside, err);
        }
        return Err("Error while replacing executable, see logs for more details".into());
    }

    Ok(())
}

/// Renames, retrying for a few seconds: antivirus software tends to hold on to an executable
/// that was just written while it scans it.
#[cfg(windows)]
fn rename_retrying(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut attempts = 0;
    loop {
        match std::fs::rename(from, to) {
            Err(err) if attempts < 10 && err.kind() != std::io::ErrorKind::NotFound => {
                log::warn!("Unable to rename {:?} to {:?}, retrying: {}", from, to, err);
                attempts += 1;
                std::thread::sleep(std::time::Duration::from_millis(500));
            },
            result => return result,
        }
    }
}

/// [`swap_exe_into_place`] for a launcher installed somewhere only an administrator can change,
/// such as Program Files.
#[cfg(windows)]
fn swap_exe_into_place_elevated(
    old_exe: &Path,
    new_exe: &Path,
    new_exe_temp: &Path,
    aside: &Path,
) -> Result<(), String> {
    let script = format!(
        "$ErrorActionPreference = 'Stop'; \
         Move-Item -LiteralPath {old} -Destination {aside}; \
         try {{ Move-Item -LiteralPath {temp} -Destination {new} -Force }} \
         catch {{ Move-Item -LiteralPath {aside} -Destination {old}; exit 1 }}",
        old = powershell_quote(old_exe),
        aside = powershell_quote(aside),
        temp = powershell_quote(new_exe_temp),
        new = powershell_quote(new_exe),
    );

    log::info!("Running with elevated powershell.exe: {}", script);
    let status = run_admin_powershell(OsStr::new(&script));

    if status.success() {
        Ok(())
    } else {
        log::error!("Error completing elevated executable install: {}", status);
        Err("Error completing elevated executable installation, see logs for more details".into())
    }
}

/// `path` as a single-quoted PowerShell string, which takes everything literally but quotes.
#[cfg(windows)]
fn powershell_quote(path: &Path) -> String {
    let mut quoted = String::from("'");
    for char in path.to_string_lossy().chars() {
        // PowerShell treats the typographic single quotes as quotes too
        if matches!(char, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
            quoted.push(char);
        }
        quoted.push(char);
    }
    quoted.push('\'');
    quoted
}

#[cfg(unix)]
fn move_new_exe_into(old_exe_path: PathBuf, new_exe_path: PathBuf, new_exe_data: &Path) -> Result<(), String> {
    let old_exe_path = try_canonicalize(&old_exe_path).unwrap_or(old_exe_path);
    let new_exe_path = try_canonicalize(&new_exe_path).unwrap_or(new_exe_path);

    if let Err(err) = std::fs::rename(&new_exe_data, &new_exe_path) {
        if err.kind() == std::io::ErrorKind::PermissionDenied {
            log::info!("Permission denied while trying to update executable, need to elevate");

            let mut command = OsString::new();
            command.push("mv -f '");
            command.push(new_exe_data.as_os_str());
            command.push("' '");
            command.push(new_exe_path.as_os_str());
            command.push("' && chmod +x '");
            command.push(new_exe_path.as_os_str());

            if old_exe_path == new_exe_path {
                command.push("'");
            } else {
                command.push("' && rm -f '");
                command.push(old_exe_path.as_os_str());
                command.push("'");
            }

            // todo: replace runas with workspace command crate
            let result = if cfg!(target_os = "linux") {
                log::info!("Running with pkexec: {}", command.to_string_lossy());
                std::process::Command::new("pkexec").arg("sh").arg("-c").arg(command).status()
            } else {
                log::info!("Running with runas: {}", command.to_string_lossy());
                runas::Command::new("sh").arg("-c").arg(command).gui(true).status()
            };

            match result {
                Ok(status) if status.success() => return Ok(()),
                Ok(status) => {
                    log::error!("Error completing elevated executable install: {}", status);
                    return Err("Error completing elevated executable installation, see logs for more details".into());
                },
                Err(err) => {
                    log::error!("Error completing elevated executable install: {}", err);
                    return Err("Error completing elevated executable installation, see logs for more details".into());
                },
            }
        }

        return Err(format!("Error while updating executable file: {:?}", err).into());
    }

    if old_exe_path != new_exe_path {
        _ = std::fs::remove_file(&old_exe_path);
    }

    use std::os::unix::fs::PermissionsExt;
    _ = std::fs::set_permissions(&new_exe_path, std::fs::Permissions::from_mode(0o755));

    Ok(())
}

fn install_app_update(
    current_app_folder: PathBuf,
    bytes: &[u8],
    temp_extract: &Path,
    temp_backup: &Path,
) -> Result<(), Arc<str>> {
    // An app that was opened without being moved out of Downloads runs from a read-only copy macOS
    // makes of it (App Translocation), or straight off the disk image, and replacing either of
    // those wouldn't update the app the user actually opens
    if current_app_folder.to_string_lossy().contains("/AppTranslocation/") || current_app_folder.starts_with("/Volumes")
    {
        return Err("Move Pandora Launcher into your Applications folder, open it from there and update again".into());
    }

    let gz_decoder = flate2::bufread::GzDecoder::new(Cursor::new(bytes));
    let mut archive = tar::Archive::new(gz_decoder);

    if let Err(err) = archive.unpack(&temp_extract) {
        log::error!("Unable to unpack .app.tar.gz: {}", err);
        return Err("Error while unpacking .app.tar.gz archive, see logs for more details".into());
    }

    let app_dir = match find_child_with_extension(&temp_extract, OsStr::new("app")) {
        Ok(None) => {
            return Err("Unable to find .app folder in extracted archive".into());
        },
        Err(err) => {
            log::error!("Unable to find .app folder: {}", err);
            return Err("I/O error while finding .app folder, see logs for more details".into());
        },
        Ok(Some(app_dir)) => app_dir,
    };

    // Backup current .app folder
    let needs_authorization = match std::fs::rename(&current_app_folder, &temp_backup) {
        Ok(_) => false,
        Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => true,
        Err(err) => {
            log::error!("Unable to backup current .app: {}", err);
            return Err("I/O error while backing up current .app, see logs for more details".into());
        },
    };

    if needs_authorization && temp_backup.exists() {
        _ = std::fs::rename(&temp_backup, &current_app_folder);
        return Err("Rename from current .app to temp backup errored, but then succeeded".into());
    }

    if needs_authorization {
        // Move current -> backup, then app -> current in single elevated command
        let mut command = OsString::new();
        command.push("mv -f '");
        command.push(current_app_folder.as_os_str());
        command.push("' '");
        command.push(temp_backup.as_os_str());
        command.push("' && mv -f '");
        command.push(app_dir.as_os_str());
        command.push("' '");
        command.push(current_app_folder.as_os_str());
        command.push("'");

        let result = runas::Command::new("sh").arg("-c").arg(command).gui(true).status();

        let success = match result {
            Ok(status) if status.success() => true,
            Ok(status) => {
                log::error!("Error completing elevated .app install: {}", status);
                false
            },
            Err(err) => {
                log::error!("Error completing elevated .app install: {}", err);
                false
            },
        };

        if !success {
            if temp_backup.exists() {
                let mut command = OsString::new();
                command.push("mv -f '");
                command.push(temp_backup.as_os_str());
                command.push("' '");
                command.push(current_app_folder.as_os_str());
                command.push("'");

                _ = runas::Command::new("sh").arg("-c").arg(command).gui(true).status();
            }

            return Err("Error completing elevated .app installation, see logs for more details".into());
        }
    } else {
        if let Err(err) = std::fs::rename(&app_dir, &current_app_folder) {
            _ = std::fs::rename(&temp_backup, &current_app_folder);
            log::error!("Error renaming new .app to old .app: {}", err);
            return Err("Error completing elevated .app installation, see logs for more details".into());
        }
    }

    Ok(())
}

#[cfg(windows)]
fn run_admin_powershell(script: &OsStr) -> std::process::ExitStatus {
    unsafe {
        let mut sei: windows::Win32::UI::Shell::SHELLEXECUTEINFOW = std::mem::zeroed();
        _ = windows::Win32::System::Com::CoInitializeEx(
            None,
            windows::Win32::System::Com::COINIT_APARTMENTTHREADED | windows::Win32::System::Com::COINIT_DISABLE_OLE1DDE,
        );

        use std::os::windows::ffi::OsStrExt;
        let encoded = crate::join_windows_shell_os(&[OsStr::new("-NoProfile"), OsStr::new("-Command"), script])
            .encode_wide()
            .chain(OsStr::new("\0").encode_wide())
            .collect::<Vec<_>>();

        sei.fMask = windows::Win32::UI::Shell::SEE_MASK_NOASYNC | windows::Win32::UI::Shell::SEE_MASK_NOCLOSEPROCESS;
        sei.cbSize = std::mem::size_of::<windows::Win32::UI::Shell::SHELLEXECUTEINFOW>() as _;
        sei.lpVerb = windows::core::w!("runas");
        sei.lpFile = windows::core::w!("powershell.exe");
        sei.lpParameters = windows::core::PCWSTR::from_raw(encoded.as_ptr());
        sei.nShow = windows::Win32::UI::WindowsAndMessaging::SW_HIDE.0;

        if windows::Win32::UI::Shell::ShellExecuteExW(&mut sei).is_err() || sei.hProcess.is_invalid() {
            return std::mem::transmute(!0);
        }

        windows::Win32::System::Threading::WaitForSingleObject(
            sei.hProcess,
            windows::Win32::System::Threading::INFINITE,
        );

        let mut code = 0;
        if windows::Win32::System::Threading::GetExitCodeProcess(sei.hProcess, &mut code).is_err() {
            std::mem::transmute(!0)
        } else {
            std::mem::transmute(code)
        }
    }
}

fn replace_os_str(input: &OsStr, from: &str, to: &str) -> OsString {
    let encoded = input.as_encoded_bytes();

    let from_bytes = from.as_bytes();
    let to_bytes = to.as_bytes();

    let mut new_encoded = Vec::new();
    let mut index = 0;
    while index < encoded.len() {
        if encoded[index..].starts_with(from_bytes) {
            new_encoded.extend_from_slice(to_bytes);
            index += from_bytes.len();
        } else {
            new_encoded.push(encoded[index]);
            index += 1;
        }
    }

    // SAFETY: We construct new_encoded from a mixture of encoded and valid utf-8
    unsafe { OsString::from_encoded_bytes_unchecked(new_encoded) }
}

fn find_child_with_extension(folder: &Path, extension: &OsStr) -> std::io::Result<Option<PathBuf>> {
    let read_dir = std::fs::read_dir(folder)?;

    for entry in read_dir {
        let entry = entry?;
        let path = entry.path();
        if path.extension() == Some(extension) {
            return Ok(Some(path));
        }
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{cleanup_appimage_desktop_entries_in, is_left_over_from_update, undisambiguated_name};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("pandora_desktop_entries_{name}"));
            _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn write(&self, name: &str, content: &str) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, content).unwrap();
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn appimagekit_entry(name: &str, exec: &Path) -> String {
        format!(
            "[Desktop Entry]\nCategories=\nExec={exec}\nName={name}\nTerminal=false\nType=Application\n\n\
             TryExec={exec}\nActions=AppImageLauncher-Remove-AppImage;\n\n\
             [Desktop Action AppImageLauncher-Remove-AppImage]\nName=Delete this AppImage\n\
             Exec=/usr/lib/appimagelauncher/remove \"{exec}\"\n",
            exec = exec.display()
        )
    }

    #[test]
    fn removes_entry_for_second_path_to_the_same_appimage() {
        let dir = TempDir::new("same_appimage");

        let appimage = dir.path().join("PandoraLauncher-Linux-x86_64_abc.AppImage");
        std::fs::write(&appimage, b"appimage").unwrap();
        let symlink = dir.path().join("PandoraLauncher.AppImage");
        std::os::unix::fs::symlink(&appimage, &symlink).unwrap();

        let real_entry = dir.write(
            "appimagekit_1111-Pandora_Launcher.desktop",
            &appimagekit_entry("Pandora Launcher", &appimage),
        );
        let symlink_entry = dir.write(
            "appimagekit_2222-Pandora_Launcher.desktop",
            &appimagekit_entry("Pandora Launcher", &symlink),
        );

        cleanup_appimage_desktop_entries_in(dir.path(), &appimage);

        assert!(real_entry.exists(), "the entry for the running path should be kept");
        assert!(!symlink_entry.exists(), "the entry for the symlink should be removed");
    }

    #[test]
    fn removes_entry_pointing_at_a_deleted_appimage() {
        let dir = TempDir::new("deleted_appimage");

        let appimage = dir.path().join("PandoraLauncher.AppImage");
        std::fs::write(&appimage, b"appimage").unwrap();

        let kept = dir.write(
            "appimagekit_1111-Pandora_Launcher.desktop",
            &appimagekit_entry("Pandora Launcher", &appimage),
        );
        let stale = dir.write(
            "appimagekit_2222-Pandora_Launcher.desktop",
            &appimagekit_entry("Pandora Launcher (1)", &dir.path().join("PandoraLauncher-old.AppImage")),
        );

        cleanup_appimage_desktop_entries_in(dir.path(), &appimage);

        assert!(kept.exists());
        assert!(!stale.exists(), "the entry for the removed AppImage should be removed");
    }

    #[test]
    fn defers_to_a_hand_written_entry() {
        let dir = TempDir::new("hand_written");

        let appimage = dir.path().join("PandoraLauncher.AppImage");
        std::fs::write(&appimage, b"appimage").unwrap();

        let handwritten = dir.write(
            "pandora-launcher.desktop",
            "[Desktop Entry]\nName=Pandora Launcher\nExec=/home/user/.local/bin/pandora %U\n\
             Type=Application\n",
        );
        let generated = dir.write(
            "appimagekit_1111-Pandora_Launcher.desktop",
            &appimagekit_entry("Pandora Launcher (1)", &appimage),
        );

        cleanup_appimage_desktop_entries_in(dir.path(), &appimage);

        assert!(handwritten.exists(), "hand-written entries must never be removed");
        assert!(!generated.exists(), "the generated duplicate should be removed");
    }

    #[test]
    fn keeps_entries_for_other_applications() {
        let dir = TempDir::new("other_apps");

        let appimage = dir.path().join("PandoraLauncher.AppImage");
        std::fs::write(&appimage, b"appimage").unwrap();
        let other = dir.path().join("SomethingElse.AppImage");
        std::fs::write(&other, b"appimage").unwrap();

        let ours = dir.write(
            "appimagekit_1111-Pandora_Launcher.desktop",
            &appimagekit_entry("Pandora Launcher", &appimage),
        );
        let theirs = dir.write("appimagekit_2222-Something_Else.desktop", &appimagekit_entry("Something Else", &other));
        let theirs_stale = dir.write(
            "appimagekit_3333-Something_Else.desktop",
            &appimagekit_entry("Something Else", &dir.path().join("Gone.AppImage")),
        );

        cleanup_appimage_desktop_entries_in(dir.path(), &appimage);

        assert!(ours.exists());
        assert!(theirs.exists(), "other applications must not be touched");
        assert!(theirs_stale.exists(), "other applications must not be touched even when stale");
    }

    #[test]
    fn keeps_a_second_independent_pandora_appimage() {
        let dir = TempDir::new("two_appimages");

        let appimage = dir.path().join("PandoraLauncher.AppImage");
        std::fs::write(&appimage, b"appimage").unwrap();
        let beta = dir.path().join("PandoraLauncher-beta.AppImage");
        std::fs::write(&beta, b"beta").unwrap();

        dir.write(
            "pandora-launcher.desktop",
            "[Desktop Entry]\nName=Pandora Launcher\nExec=/home/user/.local/bin/pandora %U\nType=Application\n",
        );
        let running = dir.write(
            "appimagekit_1111-Pandora_Launcher.desktop",
            &appimagekit_entry("Pandora Launcher (1)", &appimage),
        );
        let beta_entry = dir.write(
            "appimagekit_2222-Pandora_Launcher.desktop",
            &appimagekit_entry("Pandora Launcher (2)", &beta),
        );

        cleanup_appimage_desktop_entries_in(dir.path(), &appimage);

        assert!(!running.exists(), "the redundant entry for the running AppImage should go");
        assert!(beta_entry.exists(), "a different AppImage keeps its own entry");
    }

    #[test]
    fn recognises_only_files_an_update_leaves_behind() {
        use std::ffi::OsStr;

        let exe = OsStr::new("PandoraLauncher-Windows-x86_64.exe");
        let left_over = |name: &str| is_left_over_from_update(exe, OsStr::new(name));

        assert!(left_over("PandoraLauncher-Windows-x86_64.exe.8213490713.old"));
        assert!(left_over("PandoraLauncher-Windows-x86_64.exe.17.new"));

        assert!(!left_over("PandoraLauncher-Windows-x86_64.exe"));
        assert!(!left_over("PandoraLauncher-Windows-x86_64.exe.old"));
        assert!(!left_over("PandoraLauncher-Windows-x86_64.exe.backup.old"));
        assert!(!left_over("PandoraLauncher-Windows-x86_64.exe.17.new.txt"));
        assert!(!left_over("Other.exe.17.old"));
    }

    #[test]
    fn undisambiguated_name_strips_only_counter_suffixes() {
        assert_eq!(undisambiguated_name("Pandora Launcher"), "Pandora Launcher");
        assert_eq!(undisambiguated_name("Pandora Launcher (1)"), "Pandora Launcher");
        assert_eq!(undisambiguated_name("Pandora Launcher (12)"), "Pandora Launcher");
        assert_eq!(undisambiguated_name("Pandora Launcher (beta)"), "Pandora Launcher (beta)");
        assert_eq!(undisambiguated_name("Pandora Launcher ()"), "Pandora Launcher ()");
    }
}
