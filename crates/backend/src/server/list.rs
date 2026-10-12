use std::sync::Arc;

use bridge::instance::InstanceID;
use schema::server_list::{
    BannedIp, BannedPlayer, CachedUser, Operator, ServerListAddOutcome, ServerListEntry, ServerListFile,
    WhitelistedPlayer,
};
use uuid::Uuid;

use crate::BackendState;

impl BackendState {
    /// Reads one of a server's own list files.
    ///
    /// A file that isn't there yet reads as an empty list and is written out empty, so a
    /// server that has never run still has the file when it first starts. One that won't
    /// parse is reported and left alone, since overwriting it would throw away real bans
    /// over a stray comma.
    pub fn read_server_list(self: &Arc<Self>, id: InstanceID, file: ServerListFile) -> Arc<[ServerListEntry]> {
        let Some(server_dir) = self.server_dir(id) else {
            return Arc::new([]);
        };
        let path = server_dir.join(file.file_name());

        let Ok(contents) = std::fs::read_to_string(&path) else {
            write_server_list_file(&path, file, &[]);
            return Arc::new([]);
        };

        let parsed: Result<Vec<ServerListEntry>, _> = match file {
            ServerListFile::BannedPlayers => serde_json::from_str::<Vec<BannedPlayer>>(&contents)
                .map(|v| v.into_iter().map(ServerListEntry::BannedPlayer).collect()),
            ServerListFile::BannedIps => serde_json::from_str::<Vec<BannedIp>>(&contents)
                .map(|v| v.into_iter().map(ServerListEntry::BannedIp).collect()),
            ServerListFile::Whitelist => serde_json::from_str::<Vec<WhitelistedPlayer>>(&contents)
                .map(|v| v.into_iter().map(ServerListEntry::Whitelisted).collect()),
            ServerListFile::Operators => serde_json::from_str::<Vec<Operator>>(&contents)
                .map(|v| v.into_iter().map(ServerListEntry::Operator).collect()),
            ServerListFile::UserCache => serde_json::from_str::<Vec<CachedUser>>(&contents)
                .map(|v| v.into_iter().map(ServerListEntry::CachedUser).collect()),
        };

        match parsed {
            Ok(entries) => Arc::from(entries),
            Err(err) => {
                log::error!("Unable to read {}: {err}", file.file_name());
                self.send.send_error(format!("Unable to read {}, leaving it as it is", file.file_name()));
                Arc::new([])
            },
        }
    }

    /// Writes a whole list back in one go.
    pub fn write_server_list(self: &Arc<Self>, id: InstanceID, file: ServerListFile, entries: &[ServerListEntry]) {
        if let Some(server_dir) = self.server_dir(id) {
            write_server_list_file(&server_dir.join(file.file_name()), file, entries);
        }
    }

    /// Adds named players to one of a server's list files.
    ///
    /// A server matches a ban or a whitelist entry on the UUID, so a name no account answers
    /// for can't be written meaningfully. Those come back in the outcome rather than being
    /// written with a missing UUID, which would never match anybody.
    pub async fn add_server_list_entries(
        self: &Arc<Self>,
        id: InstanceID,
        file: ServerListFile,
        names: Arc<[Arc<str>]>,
        reason: Option<Arc<str>>,
    ) -> ServerListAddOutcome {
        let mut outcome = ServerListAddOutcome::default();

        if !file.is_editable() {
            return outcome;
        }

        let existing = self.read_server_list(id, file);

        // Names already present, or repeated in what was typed, are dropped before
        // resolving, so a typo twice doesn't cost two lookups
        let mut wanted: Vec<Arc<str>> = Vec::new();
        for name in names.iter() {
            let name: Arc<str> = Arc::from(name.trim());
            if name.is_empty() || existing.iter().any(|entry| entry.matches(&name, None)) {
                continue;
            }
            if !wanted.contains(&name) {
                wanted.push(name);
            }
        }
        if wanted.is_empty() {
            return outcome;
        }

        let uuids = self.resolve_player_names(&wanted).await;
        let created = schema::server_list::now_timestamp();

        let mut updated: Vec<ServerListEntry> = existing.to_vec();
        for (name, uuid) in wanted.iter().zip(uuids) {
            let entry = match file {
                ServerListFile::BannedPlayers => ServerListEntry::BannedPlayer(BannedPlayer {
                    name: name.clone(),
                    uuid,
                    expires_on: None,
                    created: Some(created.clone()),
                    source: Some("Server".into()),
                    banned_by: None,
                    reason: reason.clone(),
                    uid: None,
                    last_updated: None,
                    extra: Default::default(),
                }),
                ServerListFile::Whitelist => ServerListEntry::Whitelisted(WhitelistedPlayer {
                    name: name.clone(),
                    uuid,
                    extra: Default::default(),
                }),
                ServerListFile::Operators => ServerListEntry::Operator(Operator {
                    name: name.clone(),
                    uuid,
                    // The level a server grants by default when it promotes someone
                    level: 4,
                    bypasses_player_limit: false,
                    extra: Default::default(),
                }),
                // Caught by is_editable above; nothing to add a player to
                ServerListFile::BannedIps | ServerListFile::UserCache => continue,
            };

            outcome.added.push(name.clone());

            // Still written without a UUID. The server matches a ban on name alone until the
            // player is seen, which is the case that matters here: banning someone before
            // they have ever joined is the whole reason to do this without starting the
            // server. It can't be resolved for a name Mojang has no account for, such as
            // anyone on an offline-mode server.
            if entry.uuid().is_none() {
                outcome.unresolved.push(name.clone());
            }

            updated.push(entry);
        }

        if !outcome.added.is_empty() {
            self.write_server_list(id, file, &updated);
        }
        outcome
    }

    /// Takes one entry back out, matched on the name it was shown under or the UUID behind it.
    pub fn remove_server_list_entry(
        self: &Arc<Self>,
        id: InstanceID,
        file: ServerListFile,
        name: Arc<str>,
        uuid: Option<Uuid>,
    ) {
        if !file.is_editable() {
            return;
        }

        let existing = self.read_server_list(id, file);
        let kept: Vec<ServerListEntry> = existing.iter().filter(|entry| !entry.matches(&name, uuid)).cloned().collect();

        if kept.len() != existing.len() {
            self.write_server_list(id, file, &kept);
        }
    }

    /// Reads a list file exactly as it is on disk, for the raw view.
    pub fn read_server_list_raw(self: &Arc<Self>, id: InstanceID, file: ServerListFile) -> Arc<str> {
        let Some(server_dir) = self.server_dir(id) else {
            return Arc::from("");
        };

        std::fs::read_to_string(server_dir.join(file.file_name())).unwrap_or_default().into()
    }

    /// Writes a list file back exactly as given. The file is checked for being a list of
    /// something first, so a raw view that lost its brackets doesn't leave the server unable
    /// to start.
    pub fn write_server_list_raw(self: &Arc<Self>, id: InstanceID, file: ServerListFile, contents: Arc<str>) -> bool {
        let Some(server_dir) = self.server_dir(id) else {
            return false;
        };

        if serde_json::from_str::<serde_json::Value>(&contents).is_err() {
            return false;
        }

        let path = server_dir.join(file.file_name());
        if let Err(err) = crate::fs::write_safe(&path, contents.as_bytes()) {
            log::error!("Unable to write {}: {err}", file.file_name());
            self.send.send_error(format!("Unable to save {}", file.file_name()));
            return false;
        }
        true
    }
}

/// Pretty-prints a list, ending in a newline, the way a server writes one so the two are
/// hard to tell apart. An empty list is written as `[]` rather than as a blank array.
fn serialise<T: serde::Serialize>(entries: &[T]) -> Option<String> {
    if entries.is_empty() {
        return Some("[]\n".into());
    }
    match serde_json::to_string_pretty(entries) {
        Ok(contents) => Some(format!("{contents}\n")),
        Err(err) => {
            log::error!("Unable to serialise a server list: {err}");
            None
        },
    }
}

/// Writes a whole list file, formatted the way a server writes one so the two are hard to
/// tell apart: indented, and ending in a newline.
fn write_server_list_file(path: &std::path::Path, file: ServerListFile, entries: &[ServerListEntry]) {
    // Only the entries belonging to this file are written, so a list built from the wrong
    // one can't end up with another file's shape in it
    let serialised = match file {
        ServerListFile::BannedPlayers => {
            let players: Vec<&BannedPlayer> = entries.iter().filter_map(BannedPlayer::of).collect();
            serialise(&players)
        },
        ServerListFile::BannedIps => {
            let ips: Vec<&BannedIp> = entries.iter().filter_map(BannedIp::of).collect();
            serialise(&ips)
        },
        ServerListFile::Whitelist => {
            let players: Vec<&WhitelistedPlayer> = entries.iter().filter_map(WhitelistedPlayer::of).collect();
            serialise(&players)
        },
        ServerListFile::Operators => {
            let operators: Vec<&Operator> = entries.iter().filter_map(Operator::of).collect();
            serialise(&operators)
        },
        ServerListFile::UserCache => {
            let users: Vec<&CachedUser> = entries.iter().filter_map(CachedUser::of).collect();
            serialise(&users)
        },
    };

    let contents = match serialised {
        Some(contents) => contents,
        None => return,
    };

    if let Err(err) = crate::fs::write_safe(path, contents.as_bytes()) {
        log::error!("Unable to write {}: {err}", file.file_name());
    }
}
