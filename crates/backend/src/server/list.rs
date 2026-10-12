use std::sync::Arc;

use bridge::instance::InstanceID;
use bridge::message::PlayerLookupResult;
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
    /// Whether a server is up right now, which decides whether these files can be written
    /// directly or have to go through the console.
    fn server_is_running(&self, id: InstanceID) -> bool {
        self.servers.with(id, |_| ()).is_some()
    }

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

        // A running server keeps these files in memory and rewrites them itself, so
        // writing one behind its back is undone the moment it next saves. Ask it over the
        // console instead, which also means the server does the name resolution and
        // doesn't need the Mojang API at all.
        if self.server_is_running(id) {
            for name in &wanted {
                if let Some(command) = console_command(file, name, reason.as_deref(), Direction::Add) {
                    self.send_server_command(id, &command);
                }
            }

            outcome.added = wanted;
            outcome.via_console = true;
            return outcome;
        }

        let uuids = self.resolve_player_names(&wanted).await;
        let created = schema::server_list::now_timestamp();

        let mut updated: Vec<ServerListEntry> = existing.to_vec();
        for (name, lookup) in wanted.iter().zip(uuids) {
            let uuid = match lookup {
                PlayerLookupResult::Found(uuid) => Some(uuid),
                // Mojang says there is no such account. The entry is still written, since a
                // server matches a ban on the name alone until it sees the player, which is
                // the case that matters: banning someone before they have ever joined is
                // the whole reason to do this without starting the server. This is also
                // every name on an offline-mode server.
                PlayerLookupResult::NoSuchPlayer => {
                    outcome.unresolved.push(name.clone());
                    None
                },
                // The API wouldn't answer. Nothing is known about this name, so it is
                // written without a UUID too, but it's kept apart from the ones above
                // rather than reported as though the account didn't exist.
                PlayerLookupResult::Unavailable => {
                    outcome.lookup_failed.push(name.clone());
                    None
                },
            };

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

        // Same as adding: a running server would rewrite the file over the top of this
        if self.server_is_running(id) {
            if let Some(command) = console_command(file, &name, None, Direction::Remove) {
                self.send_server_command(id, &command);
            }
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

/// Whether a name is going onto a list or coming off it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Direction {
    Add,
    Remove,
}

/// The console command a running server would do this with.
///
/// Whitelist and ban-ip take a verb to say which way, while bans, operators and addresses
/// have a different command for each direction. A reason is only passed where the command
/// takes one, since the others have nowhere to put it and would read it as a second name.
fn console_command(file: ServerListFile, name: &str, reason: Option<&str>, direction: Direction) -> Option<String> {
    // A reason only goes on a command that has somewhere to put it
    let reason = reason.map(str::trim).filter(|reason| !reason.is_empty());

    Some(match (file, direction) {
        // A ban is lifted with pardon, which takes no reason
        (ServerListFile::BannedPlayers, Direction::Add) => match reason {
            Some(reason) => format!("/ban {name} {reason}"),
            None => format!("/ban {name}"),
        },
        (ServerListFile::BannedPlayers, Direction::Remove) => format!("/pardon {name}"),

        (ServerListFile::BannedIps, Direction::Add) => match reason {
            Some(reason) => format!("/ban-ip {name} {reason}"),
            None => format!("/ban-ip {name}"),
        },
        (ServerListFile::BannedIps, Direction::Remove) => format!("/ban-ip remove {name}"),

        (ServerListFile::Whitelist, Direction::Add) => format!("/whitelist add {name}"),
        (ServerListFile::Whitelist, Direction::Remove) => format!("/whitelist remove {name}"),

        (ServerListFile::Operators, Direction::Add) => format!("/op {name}"),
        (ServerListFile::Operators, Direction::Remove) => format!("/deop {name}"),

        // The server writes this one itself as it sees players
        (ServerListFile::UserCache, _) => return None,
    })
}

#[cfg(test)]
mod console_commands {
    use super::*;

    fn command(file: ServerListFile, name: &str, reason: Option<&str>, direction: Direction) -> String {
        console_command(file, name, reason, direction).expect("this file has a command")
    }

    #[test]
    fn a_ban_carries_its_reason() {
        assert_eq!(
            command(ServerListFile::BannedPlayers, "steve", Some("griefing"), Direction::Add),
            "/ban steve griefing"
        );
    }

    #[test]
    fn a_ban_without_a_reason_stops_at_the_name() {
        assert_eq!(command(ServerListFile::BannedPlayers, "steve", None, Direction::Add), "/ban steve");
        // A blank reason is the same as none, rather than a trailing space
        assert_eq!(command(ServerListFile::BannedPlayers, "steve", Some("   "), Direction::Add), "/ban steve");
    }

    #[test]
    fn a_ban_is_lifted_with_pardon() {
        assert_eq!(
            command(ServerListFile::BannedPlayers, "steve", Some("griefing"), Direction::Remove),
            "/pardon steve"
        );
    }

    #[test]
    fn the_whitelist_takes_a_verb() {
        assert_eq!(command(ServerListFile::Whitelist, "steve", None, Direction::Add), "/whitelist add steve");
        assert_eq!(
            command(ServerListFile::Whitelist, "steve", None, Direction::Remove),
            "/whitelist remove steve"
        );
    }

    #[test]
    fn a_banned_address_is_removed_by_verb_too() {
        assert_eq!(command(ServerListFile::BannedIps, "10.0.0.1", None, Direction::Add), "/ban-ip 10.0.0.1");
        assert_eq!(
            command(ServerListFile::BannedIps, "10.0.0.1", None, Direction::Remove),
            "/ban-ip remove 10.0.0.1"
        );
    }

    #[test]
    fn an_address_can_carry_a_reason() {
        assert_eq!(
            command(ServerListFile::BannedIps, "10.0.0.1", Some("flooding"), Direction::Add),
            "/ban-ip 10.0.0.1 flooding"
        );
    }

    #[test]
    fn an_operator_is_promoted_and_demoted() {
        assert_eq!(command(ServerListFile::Operators, "steve", None, Direction::Add), "/op steve");
        assert_eq!(command(ServerListFile::Operators, "steve", None, Direction::Remove), "/deop steve");
    }

    #[test]
    fn a_reason_is_not_passed_where_there_is_nowhere_for_it() {
        // The whitelist command would read a reason as a second player name
        assert_eq!(
            command(ServerListFile::Whitelist, "steve", Some("a reason"), Direction::Add),
            "/whitelist add steve"
        );
    }

    #[test]
    fn the_user_cache_has_no_command() {
        assert!(console_command(ServerListFile::UserCache, "steve", None, Direction::Add).is_none());
    }
}
