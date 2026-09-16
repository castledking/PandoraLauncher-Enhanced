use std::{
    ffi::{OsStr, OsString},
    io::Cursor,
    path::{Path, PathBuf},
    sync::Arc,
};

use base64::Engine;
use bridge::{handle::FrontendHandle, message::MessageToFrontend, modal_action::ModalAction};
use rand::RngCore;
use reqwest::StatusCode;
use schema::pandora_update::{UpdateInstallType, UpdateManifest, UpdatePrompt};
use sha1::{Digest, Sha1};

use crate::directories::LauncherDirectories;

pub async fn check_for_updates(http_client: reqwest::Client, send: FrontendHandle) {
    if option_env!("PANDORA_UPDATE_PUBKEY").is_none() {
        return;
    }

    let Some(version) = option_env!("PANDORA_RELEASE_VERSION") else {
        log::warn!("Skipping update check because PANDORA_RELEASE_VERSION isn't set");

        #[cfg(not(debug_assertions))] // Don't show error in non-release builds
        send.send_warning("Unable to check for updates, missing PANDORA_RELEASE_VERSION");
        return;
    };

    let Some(repository_url) = option_env!("GITHUB_REPOSITORY_URL") else {
        log::warn!("Skipping update check because GITHUB_REPOSITORY_URL isn't set");

        #[cfg(not(debug_assertions))] // Don't show error in non-release builds
        send.send_warning("Unable to check for updates, missing GITHUB_REPOSITORY_URL");
        return;
    };

    let current_version = schema::forge::VersionFragment::string_to_parts(version);

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
                send.send_error("Unable to fetch Pandora update manifest, see logs for more details");
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
                send.send_error("Unable to download Pandora update manifest, see logs for more details");
                return;
            },
        };
        break;
    }

    let Some(manifest_bytes) = manifest_bytes else {
        send.send_error(format!(
            "Unable to fetch Pandora update manifest, non-200 status code: {}",
            last_status.unwrap_or(StatusCode::NOT_FOUND)
        ));
        return;
    };

    let manifest = match serde_json::from_slice::<UpdateManifest>(&manifest_bytes) {
        Ok(manifest) => manifest,
        Err(err) => {
            log::error!("Error while parsing update manifest: {}", err);
            send.send_error("Unable to parse update manifest, see logs for more details");
            return;
        },
    };

    let update_version = schema::forge::VersionFragment::string_to_parts(&manifest.version);

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

fn determine_update_install_type() -> Option<UpdateInstallType> {
    if let Some(appimage) = std::env::var_os("APPIMAGE") {
        return Some(UpdateInstallType::AppImage(appimage.into()));
    }

    let current_exe = std::env::current_exe().ok()?;

    if cfg!(target_os = "macos")
        && let Some(app) = determine_macos_app_path(&current_exe)
    {
        return Some(UpdateInstallType::App(app.to_path_buf()));
    }

    return Some(UpdateInstallType::Executable);
}

fn determine_macos_app_path(current_exe: &Path) -> Option<&Path> {
    let parent = current_exe.parent()?;

    if parent.file_name()? != OsStr::new("MacOS") {
        return None;
    }

    let parent2 = parent.parent()?;

    if parent2.file_name()? != OsStr::new("Contents") {
        return None;
    }

    let parent3 = parent2.parent()?;

    if parent3.extension()? != OsStr::new("app") {
        return None;
    }

    Some(parent3)
}

pub async fn install_update(
    http_client: reqwest::Client,
    dirs: Arc<LauncherDirectories>,
    send: FrontendHandle,
    update: UpdatePrompt,
    modal_action: ModalAction,
) {
    if let Err(error) = install_update_inner(http_client, &dirs, send.clone(), update, modal_action.clone()).await {
        modal_action.set_finished_with_error(error);
    }

    modal_action.set_finished();
    send.send(MessageToFrontend::Refresh);
}

async fn install_update_inner(
    http_client: reqwest::Client,
    dirs: &LauncherDirectories,
    send: FrontendHandle,
    update: UpdatePrompt,
    modal_action: ModalAction,
) -> Result<(), Arc<str>> {
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

    let pubkey = base64::engine::general_purpose::STANDARD.decode(pubkey).unwrap();
    let sig = base64::engine::general_purpose::STANDARD.decode(&*update.exe.sig).unwrap();

    let pk = minisign_verify::PublicKey::decode(std::str::from_utf8(&pubkey).unwrap()).unwrap();
    let signature = minisign_verify::Signature::decode(std::str::from_utf8(&sig).unwrap()).unwrap();

    match pk.verify(&bytes, &signature, false) {
        Err(minisign_verify::Error::InvalidSignature) => {
            return Err("Invalid signature, file was not properly signed".into());
        },
        Err(err) => {
            return Err(format!("Error while validating signature: {:?}", err).into());
        },
        Ok(_) => {},
    }

    match update.install_type {
        UpdateInstallType::AppImage(appimage) => {
            let Some(filename) = appimage.file_name() else {
                return Err("Appimage path has no filename".into());
            };

            // This is temporary to address pre-3.3.0 including the version inside the filename
            // This should be removed at some point in the near future
            let new_filename = replace_os_str(filename, &format!("-{}", &update.old_version), "");
            let new_appimage = appimage.with_file_name(new_filename);

            replace_exe(appimage.clone(), new_appimage.clone(), &bytes, dirs)?;

            // AppImageLauncher keys its generated desktop entries on the AppImage's path, so
            // updating (and especially moving) the AppImage leaves the old entry behind and the
            // launcher shows up several times in menus like rofi.
            cleanup_appimage_desktop_entries(Some(&new_appimage));
        },
        UpdateInstallType::Executable => {
            let Ok(current_exe) = std::env::current_exe() else {
                return Err("Unable to determine current exe path".into());
            };

            let Some(filename) = current_exe.file_name() else {
                return Err("Current exe path has no filename".into());
            };

            // This is temporary to address pre-3.3.0 including the version inside the filename
            // This should be removed at some point in the near future
            let new_filename = replace_os_str(filename, &format!("-{}", &update.old_version), "");
            let new_exe = current_exe.with_file_name(new_filename);

            replace_exe(current_exe, new_exe, &bytes, dirs)?;
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

            let result = install_app_update(current_app_folder, &bytes, &temp_extract, &temp_backup);

            _ = std::fs::remove_dir_all(temp_backup);
            _ = std::fs::remove_dir_all(temp_extract);

            if let Err(err) = result {
                return Err(err);
            }
        },
    }

    send.send_success("Pandora update successful. Restart to apply changes");

    Ok(())
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
/// `current` is the AppImage to treat as the running one, defaulting to `$APPIMAGE`.
pub fn cleanup_appimage_desktop_entries(current: Option<&Path>) {
    let current = match current {
        Some(current) => current.to_path_buf(),
        None => match std::env::var_os("APPIMAGE") {
            Some(appimage) => PathBuf::from(appimage),
            // Not running as an AppImage, so there is nothing AppImageLauncher could have made
            None => return,
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

fn add_new_extension(path: &Path) -> PathBuf {
    let mut new_exe_data = path.with_added_extension(format!("{}.new", rand::thread_rng().next_u64()));
    while new_exe_data.exists() {
        log::warn!(
            "Randomly generated new_exe_data file exists... what are the chances? ({:?})",
            new_exe_data
        );
        new_exe_data = path.with_added_extension(format!("{}.new", rand::thread_rng().next_u64()));
    }
    return new_exe_data;
}

fn write_new_exe_temp(new_exe: &Path, data: &[u8], dirs: &LauncherDirectories) -> Result<PathBuf, String> {
    let new_exe_data = add_new_extension(new_exe);

    let Err(err) = std::fs::write(&new_exe_data, data) else {
        return Ok(new_exe_data);
    };

    if err.kind() != std::io::ErrorKind::PermissionDenied {
        log::error!("Error while writing new executable: {}", err);
        return Err("Error while writing new executable, see logs for more details".into());
    }

    let new_exe_data = add_new_extension(&dirs.temp_dir.join("new_exe_data"));

    if let Err(err) = std::fs::write(&new_exe_data, data) {
        log::error!("Error while writing new executable: {}", err);
        return Err("Error while writing new executable, see logs for more details".into());
    }

    Ok(new_exe_data)
}

fn replace_exe(old_exe: PathBuf, new_exe: PathBuf, data: &[u8], dirs: &LauncherDirectories) -> Result<(), String> {
    let new_exe_temp = write_new_exe_temp(&new_exe, data, dirs)?;

    #[cfg(unix)]
    {
        let result = move_new_exe_into(old_exe, new_exe, &new_exe_temp);
        _ = std::fs::remove_file(new_exe_temp);
        return result;
    }
    #[cfg(windows)]
    {
        launch_update_helper(old_exe, new_exe, &new_exe_temp);
        return Ok(());
    }
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

// Windows doesn't like replacing currently running executables, so we spawn a powershell script that waits for the program to exit
#[cfg(windows)]
fn launch_update_helper(old_exe_path: PathBuf, new_exe_path: PathBuf, new_exe_data: &Path) {
    let mut ps_arguments: Vec<&OsStr> = Vec::new();

    let id_string = format!("{}", std::process::id());

    ps_arguments.push(OsStr::new("Write-Host"));
    ps_arguments.push(OsStr::new("Waiting for launcher to close..."));
    ps_arguments.push(OsStr::new(";"));

    ps_arguments.push(OsStr::new("Wait-Process"));
    ps_arguments.push(OsStr::new("-Id"));
    ps_arguments.push(OsStr::new(&id_string));
    ps_arguments.push(OsStr::new("-ErrorAction"));
    ps_arguments.push(OsStr::new("SilentlyContinue;"));

    ps_arguments.push(OsStr::new("if"));
    ps_arguments.push(OsStr::new("($?)"));
    ps_arguments.push(OsStr::new("{"));

    ps_arguments.push(OsStr::new("Move-Item"));
    ps_arguments.push(OsStr::new("-Path"));
    ps_arguments.push(new_exe_data.as_os_str());
    ps_arguments.push(OsStr::new("-Destination"));
    ps_arguments.push(new_exe_path.as_os_str());

    if old_exe_path == new_exe_path {
        ps_arguments.push(OsStr::new("-Force"));
    } else {
        ps_arguments.push(OsStr::new("-Force;"));
        ps_arguments.push(OsStr::new("if"));
        ps_arguments.push(OsStr::new("($?)"));
        ps_arguments.push(OsStr::new("{"));
        ps_arguments.push(OsStr::new("Remove-Item"));
        ps_arguments.push(OsStr::new("-Path"));
        ps_arguments.push(old_exe_path.as_os_str());
        ps_arguments.push(OsStr::new("-Force"));
        ps_arguments.push(OsStr::new("}"));
    }

    ps_arguments.push(OsStr::new("}"));

    let ps_command = crate::join_windows_shell_os(&ps_arguments);

    log::info!("Running with powershell.exe: {}", ps_command.to_string_lossy());

    std::process::Command::new("powershell.exe")
        .arg("-Command")
        .arg(ps_command)
        .spawn()
        .unwrap();
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
        let encoded = crate::join_windows_shell_os(&[OsStr::new("-Command"), script])
            .encode_wide()
            .chain(OsStr::new("\0").encode_wide())
            .collect::<Vec<_>>();

        sei.fMask = windows::Win32::UI::Shell::SEE_MASK_NOASYNC | windows::Win32::UI::Shell::SEE_MASK_NOCLOSEPROCESS;
        sei.cbSize = std::mem::size_of::<windows::Win32::UI::Shell::SHELLEXECUTEINFOW>() as _;
        sei.lpVerb = windows::core::w!("runas");
        sei.lpFile = windows::core::w!("powershell.exe");
        sei.lpParameters = windows::core::PCWSTR::from_raw(encoded.as_ptr());
        sei.nShow = windows::Win32::UI::WindowsAndMessaging::SW_NORMAL.0;

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

    use super::{cleanup_appimage_desktop_entries_in, undisambiguated_name};

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
    fn undisambiguated_name_strips_only_counter_suffixes() {
        assert_eq!(undisambiguated_name("Pandora Launcher"), "Pandora Launcher");
        assert_eq!(undisambiguated_name("Pandora Launcher (1)"), "Pandora Launcher");
        assert_eq!(undisambiguated_name("Pandora Launcher (12)"), "Pandora Launcher");
        assert_eq!(undisambiguated_name("Pandora Launcher (beta)"), "Pandora Launcher (beta)");
        assert_eq!(undisambiguated_name("Pandora Launcher ()"), "Pandora Launcher ()");
    }
}
