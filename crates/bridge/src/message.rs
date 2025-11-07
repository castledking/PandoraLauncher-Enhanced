use std::{error::Error, ffi::OsString, sync::Arc};

use schema::{loader::Loader, modrinth::{ModrinthError, ModrinthRequest, ModrinthResult}, version_manifest::MinecraftVersionManifest};
use ustr::Ustr;
use uuid::Uuid;

use crate::{account::Account, game_output::GameOutputLogLevel, instance::{InstanceID, InstanceModID, InstanceModSummary, InstanceServerSummary, InstanceStatus, InstanceWorldSummary}, keep_alive::{KeepAlive, KeepAliveHandle}, modal_action::ModalAction};

#[derive(Debug)]
pub enum MessageToBackend {
    LoadVersionManifest {
        reload: bool
    },
    CreateInstance {
        name: Ustr,
        version: Ustr,
        loader: Loader,
    },
    KillInstance {
        id: InstanceID,
    },
    StartInstance {
        id: InstanceID,
        quick_play: Option<QuickPlayLaunch>,
        modal_action: ModalAction,
    },
    RequestLoadWorlds {
        id: InstanceID
    },
    RequestLoadServers {
        id: InstanceID
    },
    RequestLoadMods {
        id: InstanceID
    },
    SetModEnabled {
        id: InstanceID,
        mod_id: InstanceModID,
        enabled: bool,
    },
    RequestModrinth {
        request: ModrinthRequest,
    },
    UpdateAccountHeadPng {
        uuid: Uuid,
        head_png: Arc<[u8]>,
        head_png_32x: Arc<[u8]>,
    },
    DownloadAllMetadata
}

#[derive(Debug)]
pub enum MessageToFrontend {
    VersionManifestUpdated(Result<Arc<MinecraftVersionManifest>, Arc<str>>),
    InstanceAdded {
        id: InstanceID,
        name: Ustr,
        version: Ustr,
        loader: Loader,
        worlds_state: Arc<AtomicBridgeDataLoadState>,
        servers_state: Arc<AtomicBridgeDataLoadState>,
        mods_state: Arc<AtomicBridgeDataLoadState>,
    },
    InstanceRemoved {
        id: InstanceID,
    },
    InstanceModified {
        id: InstanceID,
        name: Ustr,
        version: Ustr,
        loader: Loader,
        status: InstanceStatus
    },
    InstanceWorldsUpdated {
        id: InstanceID,
        worlds: Arc<[InstanceWorldSummary]>,
    },
    InstanceServersUpdated {
        id: InstanceID,
        servers: Arc<[InstanceServerSummary]>,
    },
    InstanceModsUpdated {
        id: InstanceID,
        mods: Arc<[InstanceModSummary]>,
    },
    CreateGameOutputWindow {
        id: usize,
        keep_alive: KeepAlive,
    },
    AddGameOutput {
        id: usize,
        time: i64,
        thread: Arc<str>,
        level: GameOutputLogLevel,
        text: Arc<[Arc<str>]>,
    },
    AddNotification {
        notification_type: BridgeNotificationType,
        message: Arc<str>,
    },
    ModrinthDataUpdated {
        request: ModrinthRequest,
        result: Result<ModrinthResult, ModrinthError>,
        alive_handle: KeepAliveHandle,
    },
    AccountsUpdated {
        accounts: Arc<[Account]>,
        selected_account: Option<Uuid>,
    },
    Refresh
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BridgeNotificationType {
    Success,
    Info,
    Error,
    Warning
}

#[atomic_enum::atomic_enum]
#[derive(PartialEq, Eq)]
pub enum BridgeDataLoadState {
    Unloaded,
    LoadingDirty,
    LoadedDirty,
    Loading,
    Loaded
}

impl BridgeDataLoadState {
    pub fn should_send_load_request(self) -> bool {
        match self {
            BridgeDataLoadState::Unloaded => true,
            BridgeDataLoadState::LoadingDirty => false,
            BridgeDataLoadState::LoadedDirty => true,
            BridgeDataLoadState::Loading => false,
            BridgeDataLoadState::Loaded => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuickPlayLaunch {
    Singleplayer(OsString),
    Multiplayer(OsString),
    Realms(OsString),
}
