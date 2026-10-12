use std::sync::Arc;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// One of the files a Minecraft server keeps its lists in, each in the server's own
/// directory. Which fields an entry carries depends on the file and on how new the
/// server is, so everything here is read leniently and only the fields the launcher
/// writes back are guaranteed to be there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerListFile {
    /// `banned-players.json`
    BannedPlayers,
    /// `banned-ips.json`
    BannedIps,
    /// `whitelist.json`
    Whitelist,
    /// `ops.json`
    Operators,
    /// `usercache.json`
    UserCache,
}

impl ServerListFile {
    pub fn file_name(self) -> &'static str {
        match self {
            ServerListFile::BannedPlayers => "banned-players.json",
            ServerListFile::BannedIps => "banned-ips.json",
            ServerListFile::Whitelist => "whitelist.json",
            ServerListFile::Operators => "ops.json",
            ServerListFile::UserCache => "usercache.json",
        }
    }

    /// Whether entries in this file name a player, and so can be shown with a skin.
    ///
    /// A banned address is just an address. The user cache is written by the server as it
    /// sees players rather than being asked to, so it is shown but not edited.
    pub fn describes_players(self) -> bool {
        matches!(
            self,
            ServerListFile::BannedPlayers
                | ServerListFile::Whitelist
                | ServerListFile::Operators
                | ServerListFile::UserCache
        )
    }

    /// Whether the launcher will add entries to this file.
    pub fn is_editable(self) -> bool {
        !matches!(self, ServerListFile::UserCache)
    }
}

/// A ban record. Servers have written these with a varying set of fields over the years,
/// so everything but the name and reason is optional and only the fields the launcher
/// writes are written back.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BannedPlayer {
    pub name: Arc<str>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uuid: Option<Uuid>,
    /// When the ban ends. Absent for a permanent one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_on: Option<Arc<str>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<Arc<str>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Arc<str>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub banned_by: Option<Arc<str>>,
    /// The reason shown to the player. This is what the server calls the reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<Arc<str>>,
    /// Only present on the newer split-format file, where it identifies the profile the
    /// ban came from. Left as written so a file the launcher didn't create round-trips.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<Arc<str>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_updated: Option<Arc<str>>,
    /// Whatever else was in the file. A server from a newer Minecraft writes fields this
    /// launcher has no name for, and those are left exactly as they were found.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// A banned address, which has no player behind it to show.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BannedIp {
    pub ip: Arc<str>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_on: Option<Arc<str>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<Arc<str>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Arc<str>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<Arc<str>>,
    /// Whatever else was in the file, kept as it was found.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// A whitelisted player.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct WhitelistedPlayer {
    pub name: Arc<str>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uuid: Option<Uuid>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// An operator, with the permission level it holds.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Operator {
    pub name: Arc<str>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uuid: Option<Uuid>,
    /// 0 to 4, where 4 is the highest a server grants.
    pub level: i32,
    /// Whether the operator is exempt from the player limit.
    #[serde(default)]
    pub bypasses_player_limit: bool,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// A player the server has seen, as it remembers them between runs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CachedUser {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uuid: Option<Uuid>,
    pub name: Arc<str>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_on: Option<Arc<str>>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// One entry from any of the lists, flattened for the UI. Fields the file in question
/// doesn't have are left empty rather than being optional at every use site.
#[derive(Debug, Clone, PartialEq)]
pub enum ServerListEntry {
    BannedPlayer(BannedPlayer),
    BannedIp(BannedIp),
    Whitelisted(WhitelistedPlayer),
    Operator(Operator),
    CachedUser(CachedUser),
}

impl ServerListEntry {
    pub fn file(&self) -> ServerListFile {
        match self {
            ServerListEntry::BannedPlayer(_) => ServerListFile::BannedPlayers,
            ServerListEntry::BannedIp(_) => ServerListFile::BannedIps,
            ServerListEntry::Whitelisted(_) => ServerListFile::Whitelist,
            ServerListEntry::Operator(_) => ServerListFile::Operators,
            ServerListEntry::CachedUser(_) => ServerListFile::UserCache,
        }
    }

    /// The name to show, which for a banned address is the address itself.
    pub fn name(&self) -> Arc<str> {
        match self {
            ServerListEntry::BannedPlayer(entry) => entry.name.clone(),
            ServerListEntry::BannedIp(entry) => entry.ip.clone(),
            ServerListEntry::Whitelisted(entry) => entry.name.clone(),
            ServerListEntry::Operator(entry) => entry.name.clone(),
            ServerListEntry::CachedUser(entry) => entry.name.clone(),
        }
    }

    pub fn uuid(&self) -> Option<Uuid> {
        match self {
            ServerListEntry::BannedPlayer(entry) => entry.uuid,
            ServerListEntry::BannedIp(_) => None,
            ServerListEntry::Whitelisted(entry) => entry.uuid,
            ServerListEntry::Operator(entry) => entry.uuid,
            ServerListEntry::CachedUser(entry) => entry.uuid,
        }
    }

    /// The reason a player was banned, which is the only one of these the server calls a
    /// reason and the only one worth showing beside a name.
    pub fn reason(&self) -> Option<Arc<str>> {
        match self {
            ServerListEntry::BannedPlayer(entry) => entry.reason.clone(),
            ServerListEntry::BannedIp(entry) => entry.reason.clone(),
            _ => None,
        }
    }

    /// Whether the entry has enough of a name to be told apart from another with the same
    /// one, which is what the remove action identifies it by.
    pub fn matches(&self, name: &str, uuid: Option<Uuid>) -> bool {
        if let Some(uuid) = uuid
            && self.uuid().is_some_and(|entry_uuid| entry_uuid == uuid)
        {
            return true;
        }
        &*self.name() == name
    }
}

/// Reading an entry back out of the flat list, so each file can be written from the entries
/// belonging to it alone.
macro_rules! entry_of {
    ($name:ident, $variant:ident) => {
        impl $name {
            pub fn of(entry: &ServerListEntry) -> Option<&Self> {
                match entry {
                    ServerListEntry::$variant(value) => Some(value),
                    _ => None,
                }
            }
        }
    };
}

entry_of!(BannedPlayer, BannedPlayer);
entry_of!(BannedIp, BannedIp);
entry_of!(WhitelistedPlayer, Whitelisted);
entry_of!(Operator, Operator);
entry_of!(CachedUser, CachedUser);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_ban_the_way_a_server_writes_it() {
        let raw = r#"[
            {
                "uuid": "29bd6441-0f08-41e9-bd56-919c40b71b5c",
                "name": "Anthony_24681012",
                "expiresOn": "2026-08-23 10:35:17 +0200",
                "created": "2026-08-23 10:35:17 +0200",
                "source": "Server",
                "bannedBy": "e6b95f38d-c1a1-4e2c-8c9e-f3f6b15b8bb7",
                "reason": "roblox kid",
                "uid": "b68921ca",
                "lastUpdated": "2026-08-05 16:41:01 +0200"
            }
        ]"#;

        let parsed: Vec<BannedPlayer> = serde_json::from_str(raw).expect("parsed");
        assert_eq!(parsed.len(), 1);
        assert_eq!(&*parsed[0].name, "Anthony_24681012");
        assert_eq!(&*parsed[0].reason.clone().unwrap(), "roblox kid");
        assert_eq!(&*parsed[0].uid.clone().unwrap(), "b68921ca");
        assert!(parsed[0].uuid.is_some());
    }

    #[test]
    fn reads_a_ban_that_only_names_a_player() {
        // Older servers, and anything written by hand, carry far less than the above
        let raw = r#"[{"name": "Steve", "reason": "griefing"}]"#;
        let parsed: Vec<BannedPlayer> = serde_json::from_str(raw).expect("parsed");
        assert_eq!(&*parsed[0].name, "Steve");
        assert_eq!(&*parsed[0].reason.clone().unwrap(), "griefing");
        assert_eq!(parsed[0].uuid, None);
    }

    #[test]
    fn an_empty_list_is_an_empty_list() {
        assert!(serde_json::from_str::<Vec<BannedPlayer>>("[]").expect("parsed").is_empty());
        assert!(serde_json::from_str::<Vec<WhitelistedPlayer>>("[]").expect("parsed").is_empty());
    }

    #[test]
    fn keeps_fields_it_didnt_understand() {
        // A server from a newer version writes fields this launcher has no name for. It has
        // to leave them alone rather than drop them on the next save.
        let raw = r#"[{"name": "Steve", "someFutureField": 7}]"#;
        let parsed: Vec<serde_json::Value> = serde_json::from_str(raw).expect("parsed");
        assert_eq!(parsed[0]["someFutureField"], 7);
    }

    #[test]
    fn reads_a_whitelist_entry() {
        let raw = r#"[{"uuid": "710f96df-8b04-4c91-8828-b2b5afc45cd3", "name": "ggpots"}]"#;
        let parsed: Vec<WhitelistedPlayer> = serde_json::from_str(raw).expect("parsed");
        assert_eq!(&*parsed[0].name, "ggpots");
        assert!(parsed[0].uuid.is_some());
    }

    #[test]
    fn reads_an_operator_with_its_level() {
        let raw = r#"[
            {
                "uuid": "710f96df-8b04-4c91-8828-b2b5afc45cd3",
                "name": "ggpots",
                "level": 4,
                "bypassesPlayerLimit": false
            }
        ]"#;
        let parsed: Vec<Operator> = serde_json::from_str(raw).expect("parsed");
        assert_eq!(parsed[0].level, 4);
        assert!(!parsed[0].bypasses_player_limit);
    }

    #[test]
    fn reads_a_cached_user() {
        let raw = r#"[{"name":"ggpots","uuid":"710f96df-8b04-4c91-8828-b2b5afc45cd3","expiresOn":"2026-11-11 03:14:50 -0800"}]"#;
        let parsed: Vec<CachedUser> = serde_json::from_str(raw).expect("parsed");
        assert_eq!(&*parsed[0].name, "ggpots");
        assert_eq!(&*parsed[0].expires_on.clone().unwrap(), "2026-11-11 03:14:50 -0800");
    }

    #[test]
    fn a_banned_address_has_no_player_to_show() {
        let entry = ServerListEntry::BannedIp(BannedIp {
            ip: "203.0.113.7".into(),
            ..Default::default()
        });
        assert_eq!(&*entry.name(), "203.0.113.7");
        assert_eq!(entry.uuid(), None);
        assert_eq!(entry.file(), ServerListFile::BannedIps);
    }

    #[test]
    fn the_user_cache_is_not_the_launchers_to_edit() {
        // It is written by the server as it sees players, so adding to it by hand would
        // only be undone the next time somebody joined
        assert!(!ServerListFile::UserCache.is_editable());
        assert!(ServerListFile::Whitelist.is_editable());
        assert!(ServerListFile::BannedPlayers.is_editable());
        assert!(ServerListFile::UserCache.describes_players());
        assert!(!ServerListFile::BannedIps.describes_players());
    }

    #[test]
    fn entries_match_on_either_name_or_uuid() {
        let entry = ServerListEntry::Whitelisted(WhitelistedPlayer {
            name: "ggpots".into(),
            uuid: Uuid::parse_str("710f96df-8b04-4c91-8828-b2b5afc45cd3").ok(),
            extra: serde_json::Map::new(),
        });

        assert!(entry.matches("ggpots", None));
        assert!(!entry.matches("someone", None));
        assert!(entry.matches("renamed", Uuid::parse_str("710f96df-8b04-4c91-8828-b2b5afc45cd3").ok()));
    }
}

/// What came of adding a batch of names to a list.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ServerListAddOutcome {
    /// The names that were written.
    pub added: Vec<Arc<str>>,
    /// The names Mojang has no account for. A server matches a ban on the name alone until
    /// it sees the player, so these are written without a UUID.
    pub unresolved: Vec<Arc<str>>,
    /// The names that could not be looked up at all, because the API was rate limiting or
    /// unreachable. Kept apart from `unresolved` since nothing was learned about these
    /// ones, and saying "no such account" about a real account would be a lie.
    pub lookup_failed: Vec<Arc<str>>,
    /// Whether this went through the server's console rather than its files, which is what
    /// happens when the server is up. The files are left alone in that case.
    pub via_console: bool,
}

/// The timestamp format Minecraft writes into these files, which is not ISO 8601 but
/// `2026-11-11 03:14:50 -0800`.
pub fn now_timestamp() -> Arc<str> {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S %z").to_string().into()
}

#[cfg(test)]
mod unknown_field_checks {
    use super::*;

    /// Each entry type has to hold on to fields it has no name for. A server from a newer
    /// Minecraft writes fields this launcher has never heard of, and dropping them on the
    /// next save would quietly edit the operator's server.
    fn round_trip<T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug>(
        value: serde_json::Value,
    ) -> T {
        let parsed: T = serde_json::from_value(value.clone()).expect("parsed");
        let written = serde_json::to_value(&parsed).expect("written");
        assert_eq!(written, value, "a field was lost on the way through");
        parsed
    }

    #[test]
    fn a_ban_keeps_a_field_it_didnt_understand() {
        round_trip::<BannedPlayer>(serde_json::json!({
            "name": "Steve",
            "reason": "griefing",
            "someFutureField": 7,
        }));
    }

    #[test]
    fn a_banned_address_keeps_a_field_it_didnt_understand() {
        round_trip::<BannedIp>(serde_json::json!({
            "ip": "10.0.0.1",
            "someFutureField": {"nested": true},
        }));
    }

    #[test]
    fn a_whitelisted_player_keeps_a_field_it_didnt_understand() {
        round_trip::<WhitelistedPlayer>(serde_json::json!({
            "name": "ggpots",
            "someFutureField": "hello",
        }));
    }

    #[test]
    fn an_operator_keeps_a_field_it_didnt_understand() {
        round_trip::<Operator>(serde_json::json!({
            "name": "ggpots",
            "level": 4,
            "bypassesPlayerLimit": true,
            "someFutureField": [1, 2, 3],
        }));
    }

    #[test]
    fn a_cached_user_keeps_a_field_it_didnt_understand() {
        round_trip::<CachedUser>(serde_json::json!({
            "name": "ggpots",
            "expiresOn": "2026-01-01T00:00:00Z",
            "someFutureField": 1.5,
        }));
    }
}

#[cfg(test)]
mod mojang_id_format {
    #[test]
    fn tmp_unhyphenated_id_parses() {
        let raw = "b5a7864d41d441f59e6b64f934b3ad4e";
        println!("parse_str -> {:?}", uuid::Uuid::parse_str(raw));
        assert!(uuid::Uuid::parse_str(raw).is_ok(), "Mojang returns ids with no hyphens");
    }
}
