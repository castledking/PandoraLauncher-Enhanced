use std::{path::Path, sync::Arc};

use bridge::instance::InstanceID;
use bridge::message::PlayerLookupResult;
use schema::{
    backend_config::{ServerDefaultPlayer, ServerDefaults},
    server::{ServerProperty, write_server_properties},
    server_list::{
        BannedIp, BannedPlayer, CachedUser, Operator, ServerListAddOutcome, ServerListEntry, ServerListFile,
        WhitelistedPlayer,
    },
};
use uuid::Uuid;

use crate::BackendState;

impl BackendState {
    /// Saves the players every new server starts with. Names typed in without a UUID are looked
    /// up afterwards, in the background, so that creating a server never has to wait on Mojang.
    pub fn set_server_defaults(self: &Arc<Self>, mut defaults: ServerDefaults) {
        {
            let mut config = self.config.lock();
            // A UUID already found for a name is kept when the name comes back without one
            let current = config.get().server_defaults.clone();
            for player in &mut defaults.players {
                if player.uuid.is_none() {
                    player.uuid = current.player(&player.name).and_then(|known| known.uuid);
                }
            }
            config.modify(|config| config.server_defaults = defaults.clone());
        }

        if defaults.players.iter().all(|player| player.uuid.is_some()) {
            return;
        }

        let this = self.clone();
        tokio::task::spawn(async move {
            let resolved = this.resolve_default_players(&defaults.players).await;
            this.remember_default_player_uuids(&resolved);
        });
    }

    /// Writes the default players into a server that has just been created, into its operator
    /// list and whitelist, and sets whether its whitelist is on. The server has never run, so the
    /// files are written directly rather than through its console.
    pub(super) async fn apply_server_defaults(self: &Arc<Self>, server_dir: &Path) {
        let defaults = self.config.lock().get().server_defaults.clone();

        // Written whichever way it's set. Newer Minecraft versions (26.x) turn the whitelist on unless
        // server.properties says otherwise, so leaving it out wouldn't mean off
        let path = server_dir.join("server.properties");
        let original = std::fs::read_to_string(&path).unwrap_or_default();
        let contents = with_whitelist(&original, defaults.enable_whitelist);
        if let Err(err) = crate::fs::write_safe(&path, contents.as_bytes()) {
            log::error!("Unable to write server.properties: {err}");
        }

        if defaults.players.is_empty() {
            return;
        }

        let players = self.resolve_default_players(&defaults.players).await;
        self.remember_default_player_uuids(&players);

        let operators: Vec<ServerListEntry> = players
            .iter()
            .filter(|player| player.operator)
            .map(|player| {
                ServerListEntry::Operator(Operator {
                    name: player.name.clone(),
                    uuid: player.uuid,
                    // The level a server grants by default when it promotes someone
                    level: 4,
                    bypasses_player_limit: false,
                    extra: Default::default(),
                })
            })
            .collect();
        let whitelist: Vec<ServerListEntry> = players
            .iter()
            .filter(|player| player.whitelisted)
            .map(|player| {
                ServerListEntry::Whitelisted(WhitelistedPlayer {
                    name: player.name.clone(),
                    uuid: player.uuid,
                    extra: Default::default(),
                })
            })
            .collect();

        for (file, entries) in [
            (ServerListFile::Operators, operators),
            (ServerListFile::Whitelist, whitelist),
        ] {
            if !entries.is_empty() {
                write_server_list_file(&server_dir.join(file.file_name()), file, &entries);
            }
        }
    }

    /// Fills in the UUID of every default player that doesn't have one yet, since that is what a
    /// server matches operators and whitelisted players on.
    ///
    /// A name Mojang has no account for can only ever join an offline-mode server, so it gets the
    /// UUID such a server would give it. A name the API wouldn't answer for is left without one,
    /// to be tried again next time.
    async fn resolve_default_players(&self, players: &[ServerDefaultPlayer]) -> Vec<ServerDefaultPlayer> {
        let missing: Vec<Arc<str>> = players
            .iter()
            .filter(|player| player.uuid.is_none())
            .map(|player| player.name.clone())
            .collect();
        let mut resolved = players.to_vec();
        if missing.is_empty() {
            return resolved;
        }

        let lookups = self.resolve_player_names(&missing).await;
        for (name, lookup) in missing.iter().zip(lookups) {
            let uuid = match lookup {
                PlayerLookupResult::Found(uuid) => Some(uuid),
                PlayerLookupResult::NoSuchPlayer => Some(offline_player_uuid(name)),
                PlayerLookupResult::Unavailable => None,
            };
            for player in resolved.iter_mut().filter(|player| player.uuid.is_none() && player.name == *name) {
                player.uuid = uuid;
            }
        }
        resolved
    }

    /// Saves UUIDs that were looked up, onto whichever of the stored players still lack one.
    fn remember_default_player_uuids(&self, resolved: &[ServerDefaultPlayer]) {
        let mut config = self.config.lock();
        let needs_saving = config.get().server_defaults.players.iter().any(|player| {
            player.uuid.is_none()
                && resolved
                    .iter()
                    .any(|found| found.uuid.is_some() && found.name.eq_ignore_ascii_case(&player.name))
        });
        if !needs_saving {
            return;
        }
        config.modify(|config| {
            for player in &mut config.server_defaults.players {
                if player.uuid.is_none()
                    && let Some(found) = resolved
                        .iter()
                        .find(|found| found.uuid.is_some() && found.name.eq_ignore_ascii_case(&player.name))
                {
                    player.uuid = found.uuid;
                }
            }
        });
    }
}

/// `server.properties` with the whitelist turned on or off. Enforcing it goes with it, so that
/// turning it on also removes anyone already online who isn't on the list. Every other property is
/// left for the server to fill in the first time it starts.
fn with_whitelist(original: &str, enabled: bool) -> String {
    let value: Arc<str> = if enabled { "true" } else { "false" }.into();
    write_server_properties(
        original,
        &[
            ServerProperty {
                key: "white-list".into(),
                value: value.clone(),
            },
            ServerProperty {
                key: "enforce-whitelist".into(),
                value,
            },
        ],
    )
}

/// The UUID an offline-mode server gives a player, which is Java's `UUID.nameUUIDFromBytes` of
/// `OfflinePlayer:<name>`.
fn offline_player_uuid(name: &str) -> Uuid {
    use md5::{Digest, Md5};
    let digest: [u8; 16] = Md5::digest(format!("OfflinePlayer:{name}").as_bytes()).into();
    uuid::Builder::from_md5_bytes(digest).into_uuid()
}

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

        match parse_server_list(file, &contents) {
            Ok(entries) => Arc::from(entries),
            Err(err) => {
                log::error!("Unable to read {}: {err}", file.file_name());
                self.send.send_error(format!("Unable to read {}, leaving it as it is", file.file_name()));
                Arc::new([])
            },
        }
    }

    /// Waits for a running server to save a change asked of it over the console, which it
    /// does in its own time (an online server looks the player up with Mojang first), and gives
    /// up after a while in case it turned the command down.
    ///
    /// Without this, whatever re-read the list straight after asking still found it unchanged.
    pub async fn wait_for_server_list(
        self: &Arc<Self>,
        id: InstanceID,
        file: ServerListFile,
        changed: impl Fn(&[ServerListEntry]) -> bool,
    ) {
        let Some(server_dir) = self.server_dir(id) else {
            return;
        };
        let path = server_dir.join(file.file_name());
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);

        while tokio::time::Instant::now() < deadline {
            // Quietly, unlike read_server_list: the server may be halfway through writing it
            if let Ok(contents) = tokio::fs::read_to_string(&path).await
                && let Ok(entries) = parse_server_list(file, &contents)
                && changed(&entries)
            {
                return;
            }

            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
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
    ///
    /// Returns whether it went to a running server's console, which saves it in its own time.
    pub fn remove_server_list_entry(
        self: &Arc<Self>,
        id: InstanceID,
        file: ServerListFile,
        name: Arc<str>,
        uuid: Option<Uuid>,
    ) -> bool {
        if !file.is_editable() {
            return false;
        }

        // Same as adding: a running server would rewrite the file over the top of this
        if self.server_is_running(id) {
            if let Some(command) = console_command(file, &name, None, Direction::Remove) {
                self.send_server_command(id, &command);
            }
            return true;
        }

        let existing = self.read_server_list(id, file);
        let kept: Vec<ServerListEntry> = existing.iter().filter(|entry| !entry.matches(&name, uuid)).cloned().collect();

        if kept.len() != existing.len() {
            self.write_server_list(id, file, &kept);
        }
        false
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

/// Reads one of the list files, as entries of the kind that file holds.
fn parse_server_list(file: ServerListFile, contents: &str) -> serde_json::Result<Vec<ServerListEntry>> {
    match file {
        ServerListFile::BannedPlayers => serde_json::from_str::<Vec<BannedPlayer>>(contents)
            .map(|v| v.into_iter().map(ServerListEntry::BannedPlayer).collect()),
        ServerListFile::BannedIps => serde_json::from_str::<Vec<BannedIp>>(contents)
            .map(|v| v.into_iter().map(ServerListEntry::BannedIp).collect()),
        ServerListFile::Whitelist => serde_json::from_str::<Vec<WhitelistedPlayer>>(contents)
            .map(|v| v.into_iter().map(ServerListEntry::Whitelisted).collect()),
        ServerListFile::Operators => serde_json::from_str::<Vec<Operator>>(contents)
            .map(|v| v.into_iter().map(ServerListEntry::Operator).collect()),
        ServerListFile::UserCache => serde_json::from_str::<Vec<CachedUser>>(contents)
            .map(|v| v.into_iter().map(ServerListEntry::CachedUser).collect()),
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
/// The whitelist takes a verb to say which way, while bans, operators and addresses have a
/// different command for each direction. A reason is only passed where the command takes one,
/// since the others have nowhere to put it and would read it as a second name.
///
/// Without a leading slash: vanilla's console drops one, but Paper's reads it as part of the
/// command's name and turns the command down as unknown.
fn console_command(file: ServerListFile, name: &str, reason: Option<&str>, direction: Direction) -> Option<String> {
    // A reason only goes on a command that has somewhere to put it
    let reason = reason.map(str::trim).filter(|reason| !reason.is_empty());

    Some(match (file, direction) {
        // A ban is lifted with pardon, which takes no reason, and an address ban with pardon-ip
        (ServerListFile::BannedPlayers, Direction::Add) => match reason {
            Some(reason) => format!("ban {name} {reason}"),
            None => format!("ban {name}"),
        },
        (ServerListFile::BannedPlayers, Direction::Remove) => format!("pardon {name}"),

        (ServerListFile::BannedIps, Direction::Add) => match reason {
            Some(reason) => format!("ban-ip {name} {reason}"),
            None => format!("ban-ip {name}"),
        },
        (ServerListFile::BannedIps, Direction::Remove) => format!("pardon-ip {name}"),

        (ServerListFile::Whitelist, Direction::Add) => format!("whitelist add {name}"),
        (ServerListFile::Whitelist, Direction::Remove) => format!("whitelist remove {name}"),

        (ServerListFile::Operators, Direction::Add) => format!("op {name}"),
        (ServerListFile::Operators, Direction::Remove) => format!("deop {name}"),

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
            "ban steve griefing"
        );
    }

    #[test]
    fn a_ban_without_a_reason_stops_at_the_name() {
        assert_eq!(command(ServerListFile::BannedPlayers, "steve", None, Direction::Add), "ban steve");
        // A blank reason is the same as none, rather than a trailing space
        assert_eq!(command(ServerListFile::BannedPlayers, "steve", Some("   "), Direction::Add), "ban steve");
    }

    #[test]
    fn a_ban_is_lifted_with_pardon() {
        assert_eq!(
            command(ServerListFile::BannedPlayers, "steve", Some("griefing"), Direction::Remove),
            "pardon steve"
        );
    }

    #[test]
    fn the_whitelist_takes_a_verb() {
        assert_eq!(command(ServerListFile::Whitelist, "steve", None, Direction::Add), "whitelist add steve");
        assert_eq!(
            command(ServerListFile::Whitelist, "steve", None, Direction::Remove),
            "whitelist remove steve"
        );
    }

    #[test]
    fn a_banned_address_is_lifted_with_pardon_ip() {
        assert_eq!(command(ServerListFile::BannedIps, "10.0.0.1", None, Direction::Add), "ban-ip 10.0.0.1");
        assert_eq!(
            command(ServerListFile::BannedIps, "10.0.0.1", None, Direction::Remove),
            "pardon-ip 10.0.0.1"
        );
    }

    #[test]
    fn an_address_can_carry_a_reason() {
        assert_eq!(
            command(ServerListFile::BannedIps, "10.0.0.1", Some("flooding"), Direction::Add),
            "ban-ip 10.0.0.1 flooding"
        );
    }

    #[test]
    fn an_operator_is_promoted_and_demoted() {
        assert_eq!(command(ServerListFile::Operators, "steve", None, Direction::Add), "op steve");
        assert_eq!(command(ServerListFile::Operators, "steve", None, Direction::Remove), "deop steve");
    }

    #[test]
    fn a_reason_is_not_passed_where_there_is_nowhere_for_it() {
        // The whitelist command would read a reason as a second player name
        assert_eq!(
            command(ServerListFile::Whitelist, "steve", Some("a reason"), Direction::Add),
            "whitelist add steve"
        );
    }

    #[test]
    fn the_user_cache_has_no_command() {
        assert!(console_command(ServerListFile::UserCache, "steve", None, Direction::Add).is_none());
    }
}

#[cfg(test)]
mod offline_uuids {
    #[test]
    fn matches_what_an_offline_server_gives_a_player() {
        assert_eq!(super::offline_player_uuid("Notch").to_string(), "b50ad385-829d-3141-a216-7e7d7539ba7f");
    }
}

#[cfg(test)]
mod whitelist_property {
    use super::with_whitelist;

    #[test]
    fn off_is_written_out_rather_than_left_to_the_server() {
        assert_eq!(with_whitelist("", false), "white-list=false\nenforce-whitelist=false\n");
        assert_eq!(with_whitelist("", true), "white-list=true\nenforce-whitelist=true\n");
    }

    #[test]
    fn an_existing_value_is_replaced_in_place() {
        let original = "#Minecraft server properties\nmotd=hi\nwhite-list=true\n";
        assert_eq!(
            with_whitelist(original, false),
            "#Minecraft server properties\nmotd=hi\nwhite-list=false\nenforce-whitelist=false\n"
        );
    }
}
