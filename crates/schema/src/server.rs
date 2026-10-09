use std::sync::Arc;

use serde::{Deserialize, Serialize};
use ustr::Ustr;

use crate::{loader::Loader, modrinth::ModrinthLoader};

/// What a server instance runs. Mirrors [`Loader`] for the modded platforms, and adds the
/// plugin-based platforms that have no client-side equivalent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, strum::EnumIter, Default)]
#[serde(rename_all = "lowercase")]
pub enum ServerPlatform {
    #[serde(alias = "Paper")]
    Paper,
    #[serde(alias = "Purpur")]
    Purpur,
    #[serde(alias = "Fabric")]
    Fabric,
    #[serde(alias = "Forge")]
    Forge,
    #[serde(alias = "NeoForge")]
    NeoForge,
    #[default]
    #[serde(other, alias = "Vanilla")]
    Vanilla,
}

impl ServerPlatform {
    pub fn pretty_name(self) -> &'static str {
        match self {
            ServerPlatform::Vanilla => "Vanilla",
            ServerPlatform::Paper => "Paper",
            ServerPlatform::Purpur => "Purpur",
            ServerPlatform::Fabric => "Fabric",
            ServerPlatform::Forge => "Forge",
            ServerPlatform::NeoForge => "NeoForge",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "Vanilla" | "vanilla" => Some(Self::Vanilla),
            "Paper" | "paper" => Some(Self::Paper),
            "Purpur" | "purpur" => Some(Self::Purpur),
            "Fabric" | "fabric" => Some(Self::Fabric),
            "Forge" | "forge" => Some(Self::Forge),
            "NeoForge" | "neoforge" => Some(Self::NeoForge),
            _ => None,
        }
    }

    /// The loader the instance's content should be resolved against. Paper and Purpur both report
    /// [`Loader::Paper`], since Purpur runs Paper plugins.
    pub fn loader(self) -> Loader {
        match self {
            ServerPlatform::Vanilla => Loader::Vanilla,
            ServerPlatform::Paper | ServerPlatform::Purpur => Loader::Paper,
            ServerPlatform::Fabric => Loader::Fabric,
            ServerPlatform::Forge => Loader::Forge,
            ServerPlatform::NeoForge => Loader::NeoForge,
        }
    }

    /// Plugins go in `plugins/`, mods in `mods/`. Vanilla takes neither.
    pub fn uses_plugins(self) -> bool {
        matches!(self, ServerPlatform::Paper | ServerPlatform::Purpur)
    }

    pub fn uses_mods(self) -> bool {
        matches!(self, ServerPlatform::Fabric | ServerPlatform::Forge | ServerPlatform::NeoForge)
    }

    /// The loader to search Modrinth with. Paper and Purpur both accept Paper plugins, and Purpur
    /// is a Paper fork, so Purpur searches as Paper.
    pub fn modrinth_loader(self) -> ModrinthLoader {
        match self {
            ServerPlatform::Vanilla => ModrinthLoader::Unknown,
            ServerPlatform::Paper | ServerPlatform::Purpur => ModrinthLoader::Paper,
            ServerPlatform::Fabric => ModrinthLoader::Fabric,
            ServerPlatform::Forge => ModrinthLoader::Forge,
            ServerPlatform::NeoForge => ModrinthLoader::NeoForge,
        }
    }

    /// Whether a build number has to be resolved from the platform's API before the server jar
    /// can be downloaded.
    pub fn needs_build(self) -> bool {
        matches!(self, ServerPlatform::Paper | ServerPlatform::Purpur)
    }

    /// Forge and NeoForge ship an installer that has to be run to produce the launchable server.
    pub fn needs_installer(self) -> bool {
        matches!(self, ServerPlatform::Forge | ServerPlatform::NeoForge)
    }
}

/// Everything about an instance that only applies when it is a server. Its presence on an
/// [`crate::instance::InstanceConfiguration`] is what makes that instance a server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerConfiguration {
    #[serde(default, deserialize_with = "crate::try_deserialize")]
    pub platform: ServerPlatform,

    /// Resolved build of the server jar, for platforms that have one. Kept so the same jar is
    /// reused across launches and so the UI can show what is installed.
    #[serde(
        default,
        skip_serializing_if = "crate::skip_if_none",
        deserialize_with = "crate::try_deserialize"
    )]
    pub build: Option<Ustr>,

    /// Mojang requires the EULA to be accepted before a server will start. The launcher only
    /// writes `eula.txt` once the user has ticked this.
    #[serde(
        default,
        skip_serializing_if = "crate::skip_if_default",
        deserialize_with = "crate::try_deserialize"
    )]
    pub eula_accepted: bool,

    #[serde(
        default,
        skip_serializing_if = "crate::skip_if_default",
        deserialize_with = "crate::try_deserialize"
    )]
    pub auto_restart: bool,

    /// Client instance this server's mods were last synced from, by name, so the server page can
    /// offer to sync from it again.
    #[serde(
        default,
        skip_serializing_if = "crate::skip_if_none",
        deserialize_with = "crate::try_deserialize"
    )]
    pub linked_instance: Option<Arc<str>>,
}

impl ServerConfiguration {
    pub fn new(platform: ServerPlatform) -> Self {
        Self {
            platform,
            build: None,
            eula_accepted: false,
            auto_restart: false,
            linked_instance: None,
        }
    }
}

/// A single `server.properties` entry, kept in file order so the editor can round-trip the file
/// without reordering or dropping anything it doesn't recognise.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerProperty {
    pub key: Arc<str>,
    pub value: Arc<str>,
}

/// Which widget the properties editor should use for a key, and the choices for a dropdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerPropertyKind {
    Bool,
    Integer { min: i64, max: i64 },
    Choice(&'static [&'static str]),
    Text,
}

/// Shape of the well-known `server.properties` keys, so they can be presented as switches,
/// number fields and dropdowns instead of raw text. Anything not listed here still shows up as a
/// text field, which is what keeps forks and plugins that add their own keys working.
pub fn server_property_kind(key: &str) -> ServerPropertyKind {
    const DIFFICULTY: &[&str] = &["peaceful", "easy", "normal", "hard"];
    const GAMEMODE: &[&str] = &["survival", "creative", "adventure", "spectator"];
    const LEVEL_TYPE: &[&str] = &[
        "minecraft:normal",
        "minecraft:flat",
        "minecraft:large_biomes",
        "minecraft:amplified",
        "minecraft:single_biome_surface",
    ];

    match key {
        "allow-flight"
        | "allow-nether"
        | "broadcast-console-to-ops"
        | "broadcast-rcon-to-ops"
        | "enable-command-block"
        | "enable-jmx-monitoring"
        | "enable-query"
        | "enable-rcon"
        | "enable-status"
        | "enforce-secure-profile"
        | "enforce-whitelist"
        | "force-gamemode"
        | "generate-structures"
        | "hardcore"
        | "hide-online-players"
        | "log-ips"
        | "online-mode"
        | "prevent-proxy-connections"
        | "pvp"
        | "require-resource-pack"
        | "spawn-monsters"
        | "sync-chunk-writes"
        | "use-native-transport"
        | "white-list" => ServerPropertyKind::Bool,

        "difficulty" => ServerPropertyKind::Choice(DIFFICULTY),
        "gamemode" => ServerPropertyKind::Choice(GAMEMODE),
        "level-type" => ServerPropertyKind::Choice(LEVEL_TYPE),

        "max-players" => ServerPropertyKind::Integer { min: 0, max: 1_000_000 },
        "server-port" | "query.port" | "rcon.port" => ServerPropertyKind::Integer { min: 1, max: 65_535 },
        "view-distance" | "simulation-distance" => ServerPropertyKind::Integer { min: 2, max: 32 },
        "spawn-protection" => ServerPropertyKind::Integer { min: 0, max: 16_384 },
        "op-permission-level" | "function-permission-level" => ServerPropertyKind::Integer { min: 0, max: 4 },
        "player-idle-timeout" => ServerPropertyKind::Integer { min: 0, max: 525_600 },
        "max-world-size" => ServerPropertyKind::Integer {
            min: 1,
            max: 29_999_984,
        },
        "entity-broadcast-range-percentage" => ServerPropertyKind::Integer { min: 10, max: 1000 },
        "rate-limit" | "network-compression-threshold" | "max-chained-neighbor-updates" | "max-tick-time" => {
            ServerPropertyKind::Integer { min: -1, max: i64::MAX }
        },

        _ => ServerPropertyKind::Text,
    }
}

/// Parses `server.properties`, keeping comments and unknown keys so the file can be written back
/// without losing anything.
pub fn parse_server_properties(contents: &str) -> Vec<ServerProperty> {
    let mut properties = Vec::new();

    for line in contents.lines() {
        let trimmed = line.trim_ascii();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('!') {
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        properties.push(ServerProperty {
            key: key.trim_ascii().into(),
            value: value.trim_ascii().into(),
        });
    }

    properties
}

/// Rewrites `server.properties`, replacing the values of keys that already appear in the file and
/// appending the rest. Comments and key order in the original file are preserved.
pub fn write_server_properties(original: &str, properties: &[ServerProperty]) -> String {
    let mut written = vec![false; properties.len()];
    let mut out = String::with_capacity(original.len() + 64);

    for line in original.lines() {
        let trimmed = line.trim_ascii();
        let key = (!trimmed.is_empty() && !trimmed.starts_with('#') && !trimmed.starts_with('!'))
            .then(|| trimmed.split_once('='))
            .flatten()
            .map(|(key, _)| key.trim_ascii());

        match key.and_then(|key| properties.iter().position(|property| &*property.key == key)) {
            Some(index) => {
                written[index] = true;
                out.push_str(&properties[index].key);
                out.push('=');
                out.push_str(&properties[index].value);
                out.push('\n');
            },
            None => {
                out.push_str(line);
                out.push('\n');
            },
        }
    }

    for (property, written) in properties.iter().zip(written) {
        if !written {
            out.push_str(&property.key);
            out.push('=');
            out.push_str(&property.value);
            out.push('\n');
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instance::InstanceConfiguration;

    #[test]
    fn properties_round_trip_keeps_comments_order_and_unknown_keys() {
        let original = "#Minecraft server properties\n#Mon Oct 06 12:00:00 2026\nmotd=A Minecraft Server\npvp=true\nsome-plugin-key=keep me\n\nmax-players=20\n";

        let mut properties = parse_server_properties(original);
        assert_eq!(properties.len(), 4);
        assert_eq!(&*properties[0].key, "motd");
        assert_eq!(&*properties[2].value, "keep me");

        properties.iter_mut().find(|p| &*p.key == "pvp").unwrap().value = "false".into();
        properties.push(ServerProperty {
            key: "difficulty".into(),
            value: "hard".into(),
        });

        let written = write_server_properties(original, &properties);
        assert_eq!(
            written,
            "#Minecraft server properties\n#Mon Oct 06 12:00:00 2026\nmotd=A Minecraft Server\npvp=false\nsome-plugin-key=keep me\n\nmax-players=20\ndifficulty=hard\n"
        );
    }

    #[test]
    fn properties_values_may_contain_equals_signs() {
        let properties = parse_server_properties("motd=a=b=c\n");
        assert_eq!(&*properties[0].value, "a=b=c");
    }

    #[test]
    fn property_kinds_cover_the_common_keys() {
        assert_eq!(server_property_kind("pvp"), ServerPropertyKind::Bool);
        assert_eq!(server_property_kind("server-port"), ServerPropertyKind::Integer { min: 1, max: 65_535 });
        assert!(
            matches!(server_property_kind("difficulty"), ServerPropertyKind::Choice(choices) if choices.contains(&"hard"))
        );
        assert_eq!(server_property_kind("motd"), ServerPropertyKind::Text);
        assert_eq!(server_property_kind("not-a-real-key"), ServerPropertyKind::Text);
    }

    #[test]
    fn platforms_map_to_the_right_content() {
        assert_eq!(ServerPlatform::Paper.loader(), Loader::Paper);
        assert_eq!(ServerPlatform::Purpur.loader(), Loader::Paper);
        assert_eq!(ServerPlatform::Fabric.loader(), Loader::Fabric);
        assert_eq!(ServerPlatform::Vanilla.loader(), Loader::Vanilla);
        assert!(ServerPlatform::Paper.uses_plugins() && !ServerPlatform::Paper.uses_mods());
        assert!(ServerPlatform::NeoForge.uses_mods() && !ServerPlatform::NeoForge.uses_plugins());
        assert!(!ServerPlatform::Vanilla.uses_mods() && !ServerPlatform::Vanilla.uses_plugins());
        assert!(!Loader::Paper.is_client_loader());
    }

    #[test]
    fn existing_instance_configs_still_load_as_clients() {
        let json = r#"{"minecraft_version":"1.21.1","loader":"fabric"}"#;
        let configuration: InstanceConfiguration = serde_json::from_str(json).unwrap();
        assert!(configuration.server.is_none());

        // and a client config doesn't grow a server key when saved
        let saved = serde_json::to_string(&configuration).unwrap();
        assert!(!saved.contains("\"server\""));
    }

    #[test]
    fn server_configs_round_trip() {
        let mut configuration = InstanceConfiguration::new("1.21.1".into(), Loader::Paper);
        let mut server = ServerConfiguration::new(ServerPlatform::Purpur);
        server.eula_accepted = true;
        server.auto_restart = true;
        server.linked_instance = Some("My Pack".into());
        configuration.server = Some(server.clone());

        let saved = serde_json::to_string(&configuration).unwrap();
        let loaded: InstanceConfiguration = serde_json::from_str(&saved).unwrap();
        assert_eq!(loaded.server, Some(server));
        assert_eq!(loaded.loader, Loader::Paper);
    }

    #[test]
    fn unknown_platforms_fall_back_to_vanilla() {
        let server: ServerConfiguration = serde_json::from_str(r#"{"platform":"folia"}"#).unwrap();
        assert_eq!(server.platform, ServerPlatform::Vanilla);
    }
}
