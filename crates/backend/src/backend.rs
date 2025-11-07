use std::{collections::{HashMap, HashSet}, io::Cursor, path::{Path, PathBuf}, str::FromStr, sync::{atomic::AtomicBool, Arc}, time::{Duration, SystemTime}};

use auth::{authenticator::{Authenticator, MsaAuthorizationError, XboxAuthenticateError}, credentials::{AccountCredentials, AUTH_STAGE_COUNT}, models::{MinecraftAccessToken, MinecraftProfileResponse, SkinState, TokenWithExpiry}, secret::PlatformSecretStorage, serve_redirect::{self, ProcessAuthorizationError}};
use bridge::{handle::{BackendHandle, FrontendHandle}, instance::InstanceID, message::{MessageToBackend, MessageToFrontend}, modal_action::{ModalAction, ModalActionVisitUrl, ProgressTracker}};
use image::imageops::FilterType;
use reqwest::{redirect::Policy, StatusCode};
use rustc_hash::FxHashMap;
use schema::modrinth::{ModrinthRequest, ModrinthSearchRequest};
use slab::Slab;
use tokio::sync::{mpsc::{Receiver, Sender}, OnceCell};
use uuid::Uuid;

use crate::{account::{BackendAccount, BackendAccountInfo, MinecraftLoginInfo}, directories::LauncherDirectories, instance::Instance, launch::Launcher, metadata::manager::{MetadataManager, MinecraftVersionManifestMetadata}, mod_metadata::ModMetadataManager, modrinth::data::ModrinthData};

pub fn start(send: FrontendHandle, self_handle: BackendHandle, recv: Receiver<MessageToBackend>) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("Failed to initialize Tokio runtime");

    let http_client = reqwest::ClientBuilder::new()
        // .connect_timeout(Duration::from_secs(5))
        .redirect(Policy::none())
        .use_rustls_tls()
        .user_agent("PandoraLauncher/0.1.0 (https://github.com/Moulberry/PandoraLauncher)")
        .build().unwrap();

    let base_dirs = directories::BaseDirs::new().unwrap();
    let data_dir = base_dirs.data_dir();
    let launcher_dir = data_dir.join("PandoraLauncher");
    let directories = Arc::new(LauncherDirectories::new(launcher_dir));

    let meta = Arc::new(MetadataManager::new(http_client.clone(), runtime.handle().clone(), directories.metadata_dir.clone(), send.clone()));
    
    let (watcher_tx, watcher_rx) = tokio::sync::mpsc::channel::<notify_debouncer_full::DebounceEventResult>(64);
    let watcher = notify_debouncer_full::new_debouncer(Duration::from_millis(100), None, move |event| {
        let _ = watcher_tx.blocking_send(event);
    }).unwrap();
    
    let modrinth_data = ModrinthData::new(http_client.clone(), send.clone());
    
    let mut account_info = load_accounts_json(&directories);
    for (_, account) in &mut account_info.accounts {
        account.try_load_head_32x_from_head();
    }
    
    let state = BackendState {
        recv,
        self_handle,
        send: send.clone(),
        watcher,
        watcher_rx,
        http_client,
        meta: Arc::clone(&meta),
        watching: HashMap::new(),
        instances: Slab::new(),
        instance_by_path: HashMap::new(),
        instances_generation: 0,
        directories: Arc::clone(&directories),
        launcher: Launcher::new(meta, directories, send),
        mod_metadata_manager: Arc::new(ModMetadataManager::default()),
        // todo: replace notify tick with a call to self_handle
        notify_tick: Arc::new(tokio::sync::Notify::new()),
        reload_mods_immediately: HashSet::new(),
        modrinth_data,
        account_info,
        secret_storage: OnceCell::new(),
    };

    runtime.spawn(state.start());

    std::mem::forget(runtime);
}

pub enum WatchTarget {
    InstancesDir,
    InstanceDir {
        id: InstanceID,
    },
    InvalidInstanceDir,
    InstanceLevelDir {
        id: InstanceID,
    },
    InstanceSavesDir {
        id: InstanceID
    },
    ServersDat {
        id: InstanceID
    },
    InstanceModsDir {
        id: InstanceID
    },
}

pub struct BackendState {
    pub recv: Receiver<MessageToBackend>,
    pub self_handle: BackendHandle,
    pub send: FrontendHandle,
    pub watcher: notify_debouncer_full::Debouncer<notify::RecommendedWatcher, notify_debouncer_full::RecommendedCache>,
    pub watcher_rx: Receiver<notify_debouncer_full::DebounceEventResult>,
    pub http_client: reqwest::Client,
    pub meta: Arc<MetadataManager>,
    pub watching: HashMap<Arc<Path>, WatchTarget>,
    pub instances: Slab<Instance>,
    pub instance_by_path: HashMap<PathBuf, InstanceID>,
    pub instances_generation: usize,
    pub directories: Arc<LauncherDirectories>,
    pub launcher: Launcher,
    pub mod_metadata_manager: Arc<ModMetadataManager>,
    pub notify_tick: Arc<tokio::sync::Notify>,
    pub reload_mods_immediately: HashSet<InstanceID>,
    pub modrinth_data: ModrinthData,
    pub account_info: BackendAccountInfo,
    pub secret_storage: OnceCell<PlatformSecretStorage>,
}

impl BackendState {
    async fn start(mut self) {
        // Pre-fetch version manifest
        self.meta.load(&MinecraftVersionManifestMetadata).await;
        
        self.send.send(self.account_info.create_update_message()).await;
        
        let _ = std::fs::create_dir_all(&self.directories.instances_dir);
        
        self.watch_filesystem(&self.directories.instances_dir.clone(), WatchTarget::InstancesDir).await;
        
        let mut paths_with_time = Vec::new();
        
        for entry in std::fs::read_dir(&self.directories.instances_dir).unwrap() {
            let Ok(entry) = entry else {
                eprintln!("Error reading directory in instances folder: {:?}", entry.unwrap_err());
                continue;
            };
            
            let path = entry.path();
            
            let mut time = SystemTime::UNIX_EPOCH;
            if let Ok(metadata) = path.metadata() {
                if let Ok(created) = metadata.created() {
                    time = time.max(created);
                }
                if let Ok(created) = metadata.modified() {
                    time = time.max(created);
                }
            }
            
            // options.txt exists in every minecraft version, so we use its
            // modified time to determine the latest instance as well
            let mut options_txt = path.join(".minecraft");
            options_txt.push("options.txt");
            if let Ok(metadata) = options_txt.metadata() {
                if let Ok(created) = metadata.created() {
                    time = time.max(created);
                }
                if let Ok(created) = metadata.modified() {
                    time = time.max(created);
                }
            }
            
            paths_with_time.push((path, time));
        }
        
        paths_with_time.sort_by_key(|(_, time)| *time);
        for (path, _) in paths_with_time {
            let success = self.load_instance_from_path(&path, true, false).await;
            if !success {
                self.watch_filesystem(&path, WatchTarget::InvalidInstanceDir).await;
            }
        }
        
        self.handle().await;
    }
    
    pub async fn watch_filesystem(&mut self, path: &Path, target: WatchTarget) {
        if self.watcher.watch(path, notify::RecursiveMode::NonRecursive).is_err() {
            self.send.send_error(format!("Unable to watch directory {:?}, launcher may be out of sync with files!", path)).await;
            return;
        }
        self.watching.insert(path.into(), target);
    }
    
    pub async fn remove_instance(&mut self, id: InstanceID) {
        if let Some(instance) = self.instances.get(id.index) {
            if instance.id == id {
                let instance = self.instances.remove(id.index);
                self.send.send(MessageToFrontend::InstanceRemoved { id }).await;
                self.send.send_info(format!("Instance '{}' removed", instance.name)).await;
            }
        }
    }
    
    pub async fn load_instance_from_path(&mut self, path: &Path, mut show_errors: bool, show_success: bool) -> bool {
        let instance = Instance::load_from_folder(&path).await;
        let Ok(mut instance) = instance else {
            if let Some(existing) = self.instance_by_path.get(path) {
                if let Some(existing_instance) = self.instances.get(existing.index) {
                    if existing_instance.id == *existing {
                        let instance = self.instances.remove(existing.index);
                        self.send.send(MessageToFrontend::InstanceRemoved { id: instance.id}).await;
                        show_errors = true;
                    }
                }
            }
            
            if show_errors {
                let error = instance.unwrap_err();
                self.send.send_error(format!("Unable to load instance from {:?}:\n{}", &path, &error)).await;
                eprintln!("Error loading instance: {:?}", &error);
            }
            
            return false;
        };
        
        if let Some(existing) = self.instance_by_path.get(path) {
            if let Some(existing_instance) = self.instances.get_mut(existing.index) {
                if existing_instance.id == *existing {
                    existing_instance.copy_basic_attributes_from(instance);
                    
                    let _ = self.send.send(existing_instance.create_modify_message()).await;
                    
                    if show_success {
                        self.send.send_info(format!("Instance '{}' updated", existing_instance.name)).await;
                    }
                    
                    return true;
                }
            }
        }
        
        let vacant = self.instances.vacant_entry();
        let instance_id = InstanceID {
            index: vacant.key(),
            generation: self.instances_generation,
        };
        self.instances_generation = self.instances_generation.wrapping_add(1);
        
        if show_success {
            self.send.send_success(format!("Instance '{}' created", instance.name)).await;
        }
        let message = MessageToFrontend::InstanceAdded {
            id: instance_id,
            name: instance.name.clone(),
            version: instance.version.clone(),
            loader: instance.loader,
            worlds_state: Arc::clone(&instance.worlds_state),
            servers_state: Arc::clone(&instance.servers_state),
            mods_state: Arc::clone(&instance.mods_state),
        };
        instance.id = instance_id;
        vacant.insert(instance);
        let _ = self.send.send(message).await;
        
        self.instance_by_path.insert(path.to_owned(), instance_id);
        
        self.watch_filesystem(path, WatchTarget::InstanceDir { id: instance_id }).await;
        return true;
    }
    
    async fn handle(mut self) {
        let mut interval = tokio::time::interval(Duration::from_millis(1000));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        tokio::pin!(interval);
        
        loop {
            tokio::select! {
                message = self.recv.recv() => {
                    if let Some(message) = message {
                        self.handle_message(message).await;
                    } else {
                        eprintln!("Backend receiver has shut down");
                        break;
                    }
                },
                instance_change = self.watcher_rx.recv() => {
                    if let Some(instance_change) = instance_change {
                        self.handle_filesystem(instance_change).await;
                    } else {
                        eprintln!("Backend filesystem has shut down");
                        break;
                    }
                },
                _ = self.notify_tick.notified() => {
                    self.handle_notify_tick().await;
                },
                _ = interval.tick() => {
                    self.handle_tick().await;
                }
            }
        }
    }
    
    async fn handle_tick(&mut self) {
        self.modrinth_data.expire().await;
        
        for (_, instance) in &mut self.instances {
            if let Some(child) = &mut instance.child {
                if !matches!(child.try_wait(), Ok(None)) {
                    instance.child = None;
                    self.send.send(instance.create_modify_message()).await;
                }
            }
        }
    }
    
    async fn handle_notify_tick(&mut self) {
        for (_, instance) in &mut self.instances {
            if let Some(summaries) = instance.finish_loading_worlds().await {
                self.send.send(MessageToFrontend::InstanceWorldsUpdated {
                    id: instance.id,
                    worlds: Arc::clone(&summaries)
                }).await;
                
                for summary in summaries.iter() {
                    if self.watcher.watch(&summary.level_path, notify::RecursiveMode::NonRecursive).is_ok() {
                        self.watching.insert(summary.level_path.clone(), WatchTarget::InstanceLevelDir {
                            id: instance.id,
                        });
                    }
                }
            }
            if let Some(summaries) = instance.finish_loading_servers().await {
                self.send.send(MessageToFrontend::InstanceServersUpdated {
                    id: instance.id,
                    servers: Arc::clone(&summaries)
                }).await;
            }
            if let Some(summaries) = instance.finish_loading_mods().await {
                self.send.send(MessageToFrontend::InstanceModsUpdated {
                    id: instance.id,
                    mods: Arc::clone(&summaries)
                }).await;
            }
        }
    }
    
    pub async fn login(
        &mut self,
        credentials: &mut AccountCredentials,
        login_tracker: &ProgressTracker,
        modal_action: &ModalAction
    ) -> Result<(MinecraftProfileResponse, MinecraftAccessToken), LoginError> {
        let mut authenticator = Authenticator::new(self.http_client.clone());
        
        login_tracker.set_total(AUTH_STAGE_COUNT as u32 + 1);
        login_tracker.notify().await;
        
        let mut last_auth_stage = None;
        let mut allow_backwards = true;
        loop {
            let stage_with_data = credentials.stage();
            let stage = stage_with_data.stage();
            
            login_tracker.set_count(stage as u32 + 1);
            login_tracker.notify().await;
            
            if let Some(last_stage) = last_auth_stage {
                if stage > last_stage {
                    allow_backwards = false;
                } else if stage < last_stage && !allow_backwards {
                    eprintln!("Stage {:?} went backwards from {:?} when going backwards isn't allowed. This is most likely a bug with the auth flow!", stage, last_stage);
                    return Err(LoginError::LoginStageErrorBackwards);
                } else if stage == last_stage {
                    eprintln!("Stage {:?} didn't change. This is most likely a bug with the auth flow!", stage);
                    return Err(LoginError::LoginStageErrorDidntChange);
                }
            }
            last_auth_stage = Some(stage);
            
            match credentials.stage() {
                auth::credentials::AuthStageWithData::Initial => {
                    let pending = authenticator.create_authorization();
                    modal_action.set_visit_url(ModalActionVisitUrl {
                        message: "Login with Microsoft".into(),
                        url: pending.url.as_str().into(),
                    });
                    self.send.send(MessageToFrontend::Refresh).await;
                    
                    let finished = tokio::task::spawn_blocking(|| {
                        serve_redirect::start_server(pending, Arc::new(AtomicBool::new(false)))
                    }).await.unwrap()?;
                    
                    modal_action.unset_visit_url();
                    self.send.send(MessageToFrontend::Refresh).await;
                    
                    let msa_tokens = authenticator.finish_authorization(finished).await?;
                    
                    credentials.msa_access = Some(msa_tokens.access);
                    credentials.msa_refresh = msa_tokens.refresh;
                },
                auth::credentials::AuthStageWithData::MsaRefresh(refresh) => {
                    match authenticator.refresh_msa(&refresh).await {
                        Ok(Some(msa_tokens)) => {
                            credentials.msa_access = Some(msa_tokens.access);
                            credentials.msa_refresh = msa_tokens.refresh;
                        },
                        Ok(None) => {
                            if !allow_backwards {
                                return Err(MsaAuthorizationError::InvalidGrant.into());
                            }
                            credentials.msa_refresh = None;
                        },
                        Err(error) => {
                            if !allow_backwards || error.is_connection_error() {
                                return Err(error.into());
                            }
                            if !matches!(error, MsaAuthorizationError::InvalidGrant) {
                                eprintln!("Error using msa refresh to get msa access: {:?}", error);
                            }
                            credentials.msa_refresh = None;
                        },
                    }
                },
                auth::credentials::AuthStageWithData::MsaAccess(access) => {
                    match authenticator.authenticate_xbox(&access).await {
                        Ok(xbl) => {
                            credentials.xbl = Some(xbl);
                        },
                        Err(error) => {
                            if !allow_backwards || error.is_connection_error() {
                                return Err(error.into());
                            }
                            if !matches!(error, XboxAuthenticateError::NonOkHttpStatus(StatusCode::UNAUTHORIZED)) {
                                eprintln!("Error using msa access to get xbl token: {:?}", error);
                            }
                            credentials.msa_access = None;
                        },
                    }
                },
                auth::credentials::AuthStageWithData::XboxLive(xbl) => {
                    match authenticator.obtain_xsts(&xbl).await {
                        Ok(xsts) => {
                            credentials.xsts = Some(xsts);
                        },
                        Err(error) => {
                            if !allow_backwards || error.is_connection_error() {
                                return Err(error.into());
                            }
                            if !matches!(error, XboxAuthenticateError::NonOkHttpStatus(StatusCode::UNAUTHORIZED)) {
                                eprintln!("Error using xbl to get xsts: {:?}", error);
                            }
                            credentials.xbl = None;
                        },
                    }
                },
                auth::credentials::AuthStageWithData::XboxSecure { xsts, userhash } => {
                    match authenticator.authenticate_minecraft(&xsts, &userhash).await {
                        Ok(token) => {
                            credentials.access_token = Some(token);
                        },
                        Err(error) => {
                            if !allow_backwards || error.is_connection_error() {
                                return Err(error.into());
                            }
                            if !matches!(error, XboxAuthenticateError::NonOkHttpStatus(StatusCode::UNAUTHORIZED)) {
                                eprintln!("Error using xsts to get minecraft access token: {:?}", error);
                            }
                            credentials.xsts = None;
                        },
                    }
                },
                auth::credentials::AuthStageWithData::AccessToken(access_token) => {
                    match authenticator.get_minecraft_profile(&access_token).await {
                        Ok(profile) => {
                            login_tracker.set_count(AUTH_STAGE_COUNT as u32 + 1);
                            login_tracker.notify().await;
                            
                            return Ok((profile, access_token));
                        },
                        Err(error) => {
                            if !allow_backwards || error.is_connection_error() {
                                return Err(error.into());
                            }
                            if !matches!(error, XboxAuthenticateError::NonOkHttpStatus(StatusCode::UNAUTHORIZED)) {
                                eprintln!("Error using access token to get profile: {:?}", error);
                            }
                            credentials.access_token = None;
                        },
                    }
                },
            }
        }
    }
    
    pub async fn write_account_info(&mut self) {
        // Check that accounts_json can be loaded before backing it up
        if let Ok(file) = std::fs::File::open(&self.directories.accounts_json) {
            if let Ok(_) = serde_json::from_reader::<_, BackendAccountInfo>(file) {
                let _ = std::fs::rename(&self.directories.accounts_json, &self.directories.accounts_json_backup);
            }
        }
        
        let Ok(file) = std::fs::File::create(&self.directories.accounts_json) else {
            return;
        };
        
        if serde_json::to_writer(file, &self.account_info).is_err() {
            self.send.send_error("Failed to save accounts.json").await;
        }
    }
    
    pub fn update_profile_head(&self, profile: &MinecraftProfileResponse) {
        let Some(skin) = profile.skins.iter().filter(|skin| skin.state == SkinState::Active).cloned().next() else {
            return;
        };
        
        let handle = self.self_handle.clone();
        let http_client = self.http_client.clone();
        let uuid = profile.id;
        tokio::task::spawn(async move {
            let Ok(response) = http_client.get(&*skin.url).send().await else {
                return;
            };
            let Ok(bytes) = response.bytes().await else {
                return;
            };
            let Ok(mut image) = image::load_from_memory(&*bytes) else {
                return;
            };
            
            let head = image.crop(8, 8, 8, 8);
            
            let mut head_bytes = Vec::new();
            let mut cursor = Cursor::new(&mut head_bytes);
            if head.write_to(&mut cursor, image::ImageFormat::Png).is_err() {
                return;
            }
            
            head_bytes.shrink_to_fit();
            let head_png: Arc<[u8]> = Arc::from(head_bytes);
            
            let head_png_32x = if head.width() != 32 || head.height() != 32 {
                let resized = head.resize_exact(32, 32, FilterType::Nearest);
                
                let mut head_png_32x = Vec::new();
                let mut cursor = Cursor::new(&mut head_png_32x);
                if resized.write_to(&mut cursor, image::ImageFormat::Png).is_ok() {
                    head_png_32x.shrink_to_fit();
                    head_png_32x.into()
                } else {
                    head_png.clone()
                }
            } else {
                head_png.clone()
            };
            
            handle.send(MessageToBackend::UpdateAccountHeadPng {
                uuid,
                head_png,
                head_png_32x,
            }).await;
        });
    }
}

fn load_accounts_json(directories: &LauncherDirectories) -> BackendAccountInfo {
    if let Ok(file) = std::fs::File::open(&directories.accounts_json) {
        if let Ok(account_info) = serde_json::from_reader(file) {
            return account_info;
        }
    }
    
    if let Ok(file) = std::fs::File::open(&directories.accounts_json_backup) {
        if let Ok(account_info) = serde_json::from_reader(file) {
            return account_info;
        }
    }
    
    BackendAccountInfo::default()
}

#[derive(thiserror::Error, Debug)]
pub enum LoginError {
    #[error("Login stage error: Backwards")]
    LoginStageErrorBackwards,
    #[error("Login stage error: Didn't change")]
    LoginStageErrorDidntChange,
    #[error("Process authorization error: {0}")]
    ProcessAuthorizationError(#[from] ProcessAuthorizationError),
    #[error("Microsoft authorization error: {0}")]
    MsaAuthorizationError(#[from] MsaAuthorizationError),
    #[error("XboxLive authentication error: {0}")]
    XboxAuthenticateError(#[from] XboxAuthenticateError),
}
