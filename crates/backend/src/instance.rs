use std::{collections::HashSet, io::Read, path::Path, process::Child, sync::{atomic::{AtomicBool, Ordering}, Arc}, time::Instant};

use anyhow::Context;
use base64::Engine;
use bridge::{instance::{InstanceID, InstanceModID, InstanceModSummary, InstanceServerSummary, InstanceStatus, InstanceWorldSummary}, message::{AtomicBridgeDataLoadState, BridgeDataLoadState, MessageToFrontend}};
use schema::loader::Loader;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::task::JoinHandle;

use ustr::Ustr;

use crate::mod_metadata::ModMetadataManager;

#[derive(Debug)]
pub struct Instance {
    pub id: InstanceID,
    pub root_path: Arc<Path>,
    pub server_dat_path: Arc<Path>,
    pub saves_path: Arc<Path>,
    pub mods_path: Arc<Path>,
    pub name: Ustr,
    pub version: Ustr,
    pub loader: Loader,
    
    pub child: Option<Child>,
    
    pub worlds_state: Arc<AtomicBridgeDataLoadState>,
    pub dirty_worlds: HashSet<Arc<Path>>,
    worlds_loading: Option<(Arc<AtomicBool>, JoinHandle<Arc<[InstanceWorldSummary]>>)>,
    worlds: Option<Arc<[InstanceWorldSummary]>>,
    
    pub servers_state: Arc<AtomicBridgeDataLoadState>,
    pub dirty_servers: bool,
    servers_loading: Option<(Arc<AtomicBool>, JoinHandle<Arc<[InstanceServerSummary]>>)>,
    servers: Option<Arc<[InstanceServerSummary]>>,
    
    pub mods_state: Arc<AtomicBridgeDataLoadState>,
    pub dirty_mods: HashSet<Arc<Path>>,
    mods_generation: usize,
    mods_loading: Option<(Arc<AtomicBool>, JoinHandle<Vec<InstanceModSummary>>)>,
    mods: Option<Arc<[InstanceModSummary]>>,
}

#[derive(Error, Debug)]
pub enum InstanceLoadError {
    #[error("Not a directory")]
    NotADirectory,
    #[error("An I/O error occured while trying to read the instance")]
    IoError(#[from] std::io::Error),
    #[error("A serialization error occured while trying to read the instance")]
    SerdeError(#[from] serde_json::Error)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartLoadResult {
    Initial,
    Reload,
    None
}

impl Instance {
    pub fn try_get_mod(&self, id: InstanceModID) -> Option<&InstanceModSummary> {
        if id.generation != self.mods_generation {
            return None;
        }
        self.mods.as_ref().and_then(|mods| mods.get(id.index))
    }
    
    pub async fn finish_loading_worlds(&mut self) -> Option<Arc<[InstanceWorldSummary]>> {
        let Some((finished, _)) = &self.worlds_loading else {
            return None;
        };
        
        if !finished.load(Ordering::SeqCst) {
            return None;
        }
        
        let Some((_, join_handle)) = self.worlds_loading.take() else {
            unreachable!();
        };
        
        // Note: load state is only updated by backend, so no race condition
        let new_state = match self.worlds_state.load(std::sync::atomic::Ordering::SeqCst) {
            BridgeDataLoadState::LoadingDirty => BridgeDataLoadState::LoadedDirty,
            BridgeDataLoadState::Loading => BridgeDataLoadState::Loaded,
            _ => unreachable!(),
        };
        self.worlds_state.store(new_state, std::sync::atomic::Ordering::SeqCst);
        
        let result = join_handle.await.unwrap();
        self.worlds = Some(result.clone());
        Some(result)
    }
    
    pub async fn finish_loading_servers(&mut self) -> Option<Arc<[InstanceServerSummary]>> {
        let Some((finished, _)) = &self.servers_loading else {
            return None;
        };
        
        if !finished.load(Ordering::SeqCst) {
            return None;
        }
        
        let Some((_, join_handle)) = self.servers_loading.take() else {
            unreachable!();
        };
        
        // Note: load state is only updated by backend, so no race condition
        let new_state = match self.servers_state.load(std::sync::atomic::Ordering::SeqCst) {
            BridgeDataLoadState::LoadingDirty => BridgeDataLoadState::LoadedDirty,
            BridgeDataLoadState::Loading => BridgeDataLoadState::Loaded,
            _ => unreachable!(),
        };
        self.servers_state.store(new_state, std::sync::atomic::Ordering::SeqCst);
        
        let result = join_handle.await.unwrap();
        self.servers = Some(result.clone());
        Some(result)
    }
    
    pub async fn finish_loading_mods(&mut self) -> Option<Arc<[InstanceModSummary]>> {
        let Some((finished, _)) = &self.mods_loading else {
            return None;
        };
        
        if !finished.load(Ordering::SeqCst) {
            return None;
        }
        
        let Some((_, join_handle)) = self.mods_loading.take() else {
            unreachable!();
        };
        
        // Note: load state is only updated by backend, so no race condition
        let new_state = match self.mods_state.load(std::sync::atomic::Ordering::SeqCst) {
            BridgeDataLoadState::LoadingDirty => BridgeDataLoadState::LoadedDirty,
            BridgeDataLoadState::Loading => BridgeDataLoadState::Loaded,
            _ => unreachable!(),
        };
        self.mods_state.store(new_state, std::sync::atomic::Ordering::SeqCst);
        
        let mut result = join_handle.await.unwrap();
        
        self.mods_generation = self.mods_generation.wrapping_add(1);
        for (index, summary) in result.iter_mut().enumerate() {
            summary.id = InstanceModID {
                index,
                generation: self.mods_generation,
            };
        }
        
        let result: Arc<[InstanceModSummary]> = result.into();
        self.mods = Some(result.clone());
        Some(result)
    }
    
    pub fn start_load_worlds(&mut self, notify_tick: &Arc<tokio::sync::Notify>) -> StartLoadResult {
        if self.worlds_loading.is_some() {
            return StartLoadResult::None;
        }
        
        let Some(previous) = &self.worlds else {
            self.load_worlds_initial(Arc::clone(notify_tick));
            return StartLoadResult::Initial;
        };
        
        if !self.dirty_worlds.is_empty() {
            self.load_worlds_dirty(Arc::clone(notify_tick), Arc::clone(previous));
            return StartLoadResult::Reload;
        }
        
        StartLoadResult::None
    }
    
    fn load_worlds_initial(&mut self, notify_tick: Arc<tokio::sync::Notify>) {
        self.worlds_state.store(BridgeDataLoadState::Loading, std::sync::atomic::Ordering::SeqCst);
        
        let saves = self.saves_path.clone();
        
        let finished = Arc::new(AtomicBool::new(false));
        let finished2 = Arc::clone(&finished);
        let task = tokio::task::spawn_blocking(move || {
            let mut count = 0;
            let mut summaries = Vec::with_capacity(64);
            
            for entry in std::fs::read_dir(&saves).unwrap() {
                if count >= 64 {
                    break;
                }
                
                let Ok(entry) = entry else {
                    eprintln!("Error reading directory in saves folder: {:?}", entry.unwrap_err());
                    continue;
                };
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                
                count += 1;
                
                match load_world_summary(&path) {
                    Ok(summary) => {
                        summaries.push(summary);
                    },
                    Err(err) => {
                        eprintln!("Error loading world summary: {:?}", err);
                    },
                }
            }
            
            summaries.sort_by_key(|s| -s.last_played);
            
            summaries.shrink_to_fit();
            
            finished.store(true, Ordering::SeqCst);
            notify_tick.notify_one();
            
            summaries.into()
        });
        self.worlds_loading = Some((finished2, task));
    }
    
    fn load_worlds_dirty(&mut self, notify_tick: Arc<tokio::sync::Notify>, last: Arc<[InstanceWorldSummary]>) {
        self.worlds_state.store(BridgeDataLoadState::Loading, std::sync::atomic::Ordering::SeqCst);
        
        let dirty = std::mem::take(&mut self.dirty_worlds);
        
        let finished = Arc::new(AtomicBool::new(false));
        let finished2 = Arc::clone(&finished);
        let task = tokio::task::spawn_blocking(move || {
            let mut summaries = Vec::with_capacity(64);
            
            let mut count = 0;
            
            for path in dirty.iter() {
                if count >= 64 {
                    break;
                }
                
                if !path.is_dir() {
                    continue;
                }
                
                count += 1;
                
                match load_world_summary(&path) {
                    Ok(summary) => {
                        summaries.push(summary);
                    },
                    Err(err) => {
                        eprintln!("Error loading world summary: {:?}", err);
                    },
                }
            }
            
            for old_summary in &*last {
                if !dirty.contains(&old_summary.level_path) && old_summary.level_path.exists() {
                    summaries.push(old_summary.clone());
                }
            }
            
            summaries.sort_by_key(|s| -s.last_played);
            
            if summaries.len() > 64 {
                summaries.truncate(64);
            }
            summaries.shrink_to_fit();
            
            finished.store(true, Ordering::SeqCst);
            notify_tick.notify_one();
            
            summaries.into()
        });
        self.worlds_loading = Some((finished2, task));
    }
    
    pub fn start_load_servers(&mut self, notify_tick: &Arc<tokio::sync::Notify>) -> StartLoadResult {
        if self.servers_loading.is_some() {
            return StartLoadResult::None;
        }
        
        let Some(_previous) = &self.servers else {
            self.load_servers(Arc::clone(notify_tick));
            return StartLoadResult::Initial;
        };
        
        if self.dirty_servers {
            self.load_servers(Arc::clone(notify_tick));
            return StartLoadResult::Reload;
        }
        
        StartLoadResult::None
    }
    
    fn load_servers(&mut self, notify_tick: Arc<tokio::sync::Notify>) {
        self.servers_state.store(BridgeDataLoadState::Loading, std::sync::atomic::Ordering::SeqCst);
        
        self.dirty_servers = false;
        let server_dat_path = self.server_dat_path.clone();
        
        let finished = Arc::new(AtomicBool::new(false));
        let finished2 = Arc::clone(&finished);
        let task = tokio::task::spawn_blocking(move || {
            if !server_dat_path.is_file() {
                return Arc::from([]);
            }
            
            let result = match load_servers_summary(&server_dat_path) {
                Ok(mut summaries) => {
                    summaries.shrink_to_fit();
                    summaries.into()
                },
                Err(err) => {
                    eprintln!("Error loading servers: {:?}", err);
                    Arc::from([])
                },
            };
            
            finished.store(true, Ordering::SeqCst);
            notify_tick.notify_one();
            
            result
        });
        self.servers_loading = Some((finished2, task));
    }
    
    pub fn start_load_mods(&mut self, notify_tick: &Arc<tokio::sync::Notify>, mod_metadata_manager: &Arc<ModMetadataManager>) -> StartLoadResult {
        if self.mods_loading.is_some() {
            return StartLoadResult::None;
        }
        
        let Some(previous) = &self.mods else {
            self.load_mods_initial(Arc::clone(notify_tick), Arc::clone(mod_metadata_manager));
            return StartLoadResult::Initial;
        };
        
        if !self.dirty_mods.is_empty() {
            self.load_mods_dirty(Arc::clone(notify_tick), Arc::clone(mod_metadata_manager), Arc::clone(previous));
            return StartLoadResult::Reload;
        }
        
        StartLoadResult::None
    }
    
    fn load_mods_initial(&mut self, notify_tick: Arc<tokio::sync::Notify>, mod_metadata_manager: Arc<ModMetadataManager>) {
        self.mods_state.store(BridgeDataLoadState::Loading, std::sync::atomic::Ordering::SeqCst);
        
        let mods = self.mods_path.clone();
        
        let finished = Arc::new(AtomicBool::new(false));
        let finished2 = Arc::clone(&finished);
        let task = tokio::task::spawn_blocking(move || {
            let mut summaries = Vec::with_capacity(32);
            
            for entry in std::fs::read_dir(&mods).unwrap() {
                let Ok(entry) = entry else {
                    eprintln!("Error reading file in mods folder: {:?}", entry.unwrap_err());
                    continue;
                };
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                let Some(file_name) = path.file_name().and_then(|s| s.to_str()) else {
                    continue;
                };
                let enabled = if file_name.ends_with(".jar.disabled") {
                    false
                } else if file_name.ends_with(".jar") {
                    true
                } else {
                    continue;
                };
                let Ok(mut file) = std::fs::File::open(&path) else {
                    continue;
                };
                
                if let Some(summary) = mod_metadata_manager.get(&mut file) {
                    summaries.push(InstanceModSummary {
                        mod_summary: summary,
                        id: InstanceModID::dangling(),
                        file_name: file_name.into(),
                        path: path.into(),
                        enabled,
                    });
                }
            }
            
            summaries.sort_by_key(|s| Arc::clone(&s.mod_summary.id));
            
            summaries.shrink_to_fit();
            
            finished.store(true, Ordering::SeqCst);
            notify_tick.notify_one();
            
            summaries
        });
        self.mods_loading = Some((finished2, task));
    }
    
    fn load_mods_dirty(&mut self, notify_tick: Arc<tokio::sync::Notify>, mod_metadata_manager: Arc<ModMetadataManager>, last: Arc<[InstanceModSummary]>) {
        self.mods_state.store(BridgeDataLoadState::Loading, std::sync::atomic::Ordering::SeqCst);
        
        let dirty = std::mem::take(&mut self.dirty_mods);
        
        let finished = Arc::new(AtomicBool::new(false));
        let finished2 = Arc::clone(&finished);
        let task = tokio::task::spawn_blocking(move || {
            let mut summaries = Vec::with_capacity(32);
            
            for path in dirty.iter() {
                if !path.is_file() {
                    continue;
                }
                let Some(file_name) = path.file_name().and_then(|s| s.to_str()) else {
                    continue;
                };
                let enabled = if file_name.ends_with(".jar.disabled") {
                    false
                } else if file_name.ends_with(".jar") {
                    true
                } else {
                    continue;
                };
                let Ok(mut file) = std::fs::File::open(&path) else {
                    continue;
                };
                
                if let Some(summary) = mod_metadata_manager.get(&mut file) {
                    summaries.push(InstanceModSummary {
                        mod_summary: summary,
                        id: InstanceModID::dangling(),
                        file_name: file_name.into(),
                        path: path.clone(),
                        enabled,
                    });
                }
            }
            
            for old_summary in &*last {
                if !dirty.contains(&old_summary.path) && old_summary.path.exists() {
                    summaries.push(old_summary.clone());
                }
            }
            
            summaries.sort_by_key(|s| Arc::clone(&s.mod_summary.id));
            
            summaries.shrink_to_fit();
            
            finished.store(true, Ordering::SeqCst);
            notify_tick.notify_one();
            
            summaries
        });
        self.mods_loading = Some((finished2, task));
    }
    
    pub async fn load_from_folder(path: impl AsRef<Path>) -> Result<Self, InstanceLoadError> {
        let path = path.as_ref();
        if !path.is_dir() {
            return Err(InstanceLoadError::NotADirectory);
        }
        
        let info_path = path.join("info_v1.json");
        if !info_path.exists() {
        }
        
        let file = tokio::fs::read(&info_path).await?;
        
        let instance_info: InstanceInfo = serde_json::from_slice(&file)?;
        
        let mut saves_path = path.to_owned();
        saves_path.push(".minecraft");
        saves_path.push("saves");
        
        let mut mods_path = path.to_owned();
        mods_path.push(".minecraft");
        mods_path.push("mods");
        
        let mut server_dat_path = path.to_owned();
        server_dat_path.push(".minecraft");
        server_dat_path.push("servers.dat");
        
        Ok(Self {
            id: InstanceID::dangling(),
            root_path: path.into(),
            server_dat_path: server_dat_path.into(),
            saves_path: saves_path.into(),
            mods_path: mods_path.into(),
            name: path.file_name().unwrap().to_string_lossy().into_owned().into(),
            version: instance_info.minecraft_version,
            loader: instance_info.loader,
            
            child: None,
            
            worlds_state: Arc::new(AtomicBridgeDataLoadState::new(BridgeDataLoadState::Unloaded)),
            dirty_worlds: HashSet::new(),
            worlds_loading: None,
            worlds: None,
            
            servers_state: Arc::new(AtomicBridgeDataLoadState::new(BridgeDataLoadState::Unloaded)),
            dirty_servers: false,
            servers_loading: None,
            servers: None,
            
            mods_state: Arc::new(AtomicBridgeDataLoadState::new(BridgeDataLoadState::Unloaded)),
            dirty_mods: HashSet::new(),
            mods_generation: 0,
            mods_loading: None,
            mods: None,
        })
    }
    
    pub fn mark_world_state_dirty(&mut self) {
        let state = self.worlds_state.load(Ordering::SeqCst);
        match state {
            BridgeDataLoadState::Loading => {
                self.worlds_state.store(BridgeDataLoadState::LoadingDirty, Ordering::SeqCst);
            },
            BridgeDataLoadState::Loaded => {
                self.worlds_state.store(BridgeDataLoadState::LoadedDirty, Ordering::SeqCst);
            },
            _ => {}
        }
    }
    
    pub fn mark_server_state_dirty(&mut self) {
        self.dirty_servers = true;
        
        let state = self.servers_state.load(Ordering::SeqCst);
        match state {
            BridgeDataLoadState::Loading => {
                self.servers_state.store(BridgeDataLoadState::LoadingDirty, Ordering::SeqCst);
            },
            BridgeDataLoadState::Loaded => {
                self.servers_state.store(BridgeDataLoadState::LoadedDirty, Ordering::SeqCst);
            },
            _ => {}
        }
    }
    
    pub fn mark_mods_state_dirty(&mut self) {
        let state = self.mods_state.load(Ordering::SeqCst);
        match state {
            BridgeDataLoadState::Loading => {
                self.mods_state.store(BridgeDataLoadState::LoadingDirty, Ordering::SeqCst);
            },
            BridgeDataLoadState::Loaded => {
                self.mods_state.store(BridgeDataLoadState::LoadedDirty, Ordering::SeqCst);
            },
            _ => {}
        }
    }
    
    pub fn copy_basic_attributes_from(&mut self, new: Self) {
        assert_eq!(new.id, InstanceID::dangling());
        
        self.root_path = new.root_path;
        self.name = new.name;
        self.version = new.version;
        self.loader = new.loader;
    }

    pub fn status(&self) -> InstanceStatus {
        if self.child.is_some() {
            InstanceStatus::Running
        } else {
            InstanceStatus::NotRunning
        }
    }
    
    pub fn create_modify_message(&self) -> MessageToFrontend {
        self.create_modify_message_with_status(self.status())
    }
    
    pub fn create_modify_message_with_status(&self, status: InstanceStatus) -> MessageToFrontend {
        MessageToFrontend::InstanceModified {
            id: self.id,
            name: self.name.clone(),
            version: self.version.clone(),
            loader: self.loader,
            status
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum LoaderInfo {
    Vanilla,
    Fabric,
    Forge,
    NeoForge
}

#[derive(Serialize, Deserialize)]
pub struct InstanceInfo {
    pub minecraft_version: Ustr,
    pub loader: Loader,
}

fn load_world_summary(path: &Path) -> anyhow::Result<InstanceWorldSummary> {
    let level_dat_path = path.join("level.dat");
    if !level_dat_path.is_file() {
        anyhow::bail!("level.dat doesn't exist");
    }
    
    let compressed = std::fs::read(&level_dat_path)?;
    
    let mut decoder = flate2::bufread::GzDecoder::new(compressed.as_slice());
    
    let mut decompressed = Vec::new();
    decoder.read_to_end(&mut decompressed)?;
    
    let mut nbt_data = decompressed.as_slice();
    let result = nbt::decode::read_named(&mut nbt_data)?;
    
    let root = result.as_compound().context("Unable to get root compound")?;
    let data = root.find_compound("Data").context("Unable to get Data")?;
    let last_played: i64 = data.find_numeric("LastPlayed").context("Unable to get LastPlayed")?;
    let level_name = data.find_string("LevelName").cloned().unwrap_or_default();
    
    let folder = path.file_name().context("Unable to get filename")?.to_string_lossy();
    
    let subtitle = if let Some(date_time) = chrono::DateTime::from_timestamp_millis(last_played) && last_played > 0 {
        let date_time = date_time.with_timezone(&chrono::Local);
        format!("{} ({})", folder, date_time.format("%d/%m/%Y %H:%M")).into()
    } else {
        format!("{}", folder).into()
    };
    
    let title = if level_name.is_empty() {
        folder.into_owned().into()
    } else {
        level_name.into()
    };
    
    let icon_path = path.join("icon.png");
    let icon = if icon_path.is_file() {
        std::fs::read(icon_path).map(|v| Arc::from(v)).ok()
    } else {
        None
    };
    
    Ok(InstanceWorldSummary {
        title,
        subtitle,
        level_path: path.into(),
        last_played,
        png_icon: icon.into(),
    })
}

fn load_servers_summary(server_dat_path: &Path) -> anyhow::Result<Vec<InstanceServerSummary>> {
    let raw = std::fs::read(&server_dat_path)?;
    
    let mut nbt_data = raw.as_slice();
    let result = nbt::decode::read_named(&mut nbt_data)?;
    
    let root = result.as_compound().context("Unable to get root compound")?;
    let servers = root.find_list("servers", nbt::TAG_COMPOUND_ID).context("Unable to get servers")?;
    
    let mut summaries = Vec::with_capacity(servers.len());
    
    for server in servers.iter() {
        let server = server.as_compound().unwrap();
        
        if let Some(hidden) = server.find_byte("hidden") {
            if *hidden != 0 {
                continue;
            }
        }
        
        let Some(ip) = server.find_string("ip") else {
            continue;
        };
        
        let name: Arc<str> = server.find_string("name")
            .map(|v| Arc::from(v.as_str()))
            .unwrap_or_else(|| Arc::from("<unnamed>"));
        
        let icon = server.find_string("icon")
            .and_then(|v| base64::engine::general_purpose::STANDARD.decode(v).map(Arc::from).ok());
        
        summaries.push(InstanceServerSummary {
            name,
            ip: Arc::from(ip.as_str()),
            png_icon: icon,
        });
    }
    
    Ok(summaries)
}
