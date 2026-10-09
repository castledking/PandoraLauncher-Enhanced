use std::sync::Arc;

use crate::instance::InstanceID;

/// Which side of the game a mod is for, as far as the launcher can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerSyncSide {
    /// `fabric.mod.json` says `"environment": "*"`.
    Both,
    /// `fabric.mod.json` says `"environment": "client"`, or the name matches a known client mod.
    ClientOnly,
    /// `fabric.mod.json` says `"environment": "server"`.
    ServerOnly,
    /// Nothing in the jar says, which is the normal case for Forge and NeoForge.
    Unknown,
}

impl ServerSyncSide {
    pub fn label(self) -> &'static str {
        match self {
            ServerSyncSide::Both => "Client & server",
            ServerSyncSide::ClientOnly => "Client only",
            ServerSyncSide::ServerOnly => "Server only",
            ServerSyncSide::Unknown => "Side not declared",
        }
    }
}

/// One mod in a sync plan.
#[derive(Debug, Clone)]
pub struct SyncableMod {
    pub filename: Arc<str>,
    pub side: ServerSyncSide,
    /// Whether applying the plan should act on this mod. Pre-set from `side`, then up to the user.
    pub selected: bool,
    pub already_on_server: bool,
    /// Set for mods the server has and the client doesn't, which syncing would delete.
    pub only_on_server: bool,
}

/// What syncing a server against a client instance would do, for the user to review first.
#[derive(Debug, Clone)]
pub struct ServerSyncPlan {
    pub server_id: InstanceID,
    pub client_id: InstanceID,
    pub server_name: Arc<str>,
    pub client_name: Arc<str>,
    /// Set when the two instances are on different Minecraft versions, which usually means the
    /// mods won't be interchangeable.
    pub version_mismatch: Option<Arc<str>>,
    /// Set when the two instances use different mod loaders.
    pub loader_mismatch: Option<Arc<str>>,
    pub mods: Arc<[SyncableMod]>,
}
