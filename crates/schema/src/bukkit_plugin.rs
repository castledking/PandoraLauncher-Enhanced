//! Minimal reader for Bukkit/Paper plugin descriptors.
//!
//! `plugin.yml` and `paper-plugin.yml` are small, mostly-scalar YAML documents. Rather than pull
//! in a full YAML parser (and risk choking on the arbitrary user-authored config that ships inside
//! these jars), this walks the lines and picks out the handful of top-level keys the content list
//! actually displays. Anything nested or malformed is ignored.

use once_cell::sync::Lazy;
use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BukkitPluginDescriptor {
    pub name: Option<String>,
    pub version: Option<String>,
    pub prefix: Option<String>,
    pub description: Option<String>,
    pub main: Option<String>,
    pub author: Option<String>,
    pub authors: Option<AuthorList>,
    pub website: Option<String>,
}

/// `authors` accepts either a single name or a list, and plugins in the wild use both forms.
#[derive(Debug, Default)]
pub struct AuthorList(pub Vec<String>);

impl<'de> Deserialize<'de> for AuthorList {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum OneOrMany {
            One(String),
            Many(Vec<String>),
        }

        Ok(match OneOrMany::deserialize(deserializer)? {
            OneOrMany::One(name) => AuthorList(vec![name]),
            OneOrMany::Many(names) => AuthorList(names),
        })
    }
}

impl BukkitPluginDescriptor {
    /// The authors to show, joined the same way the other content types format them.
    pub fn authors_string(&self) -> Option<String> {
        let names: Vec<&str> = match &self.authors {
            Some(AuthorList(names)) if !names.is_empty() => names.iter().map(String::as_str).collect(),
            _ => match &self.author {
                Some(author) if !author.is_empty() => vec![author.as_str()],
                _ => return None,
            },
        };

        Some(format!("By {}", names.join(", ")))
    }
}

/// Parses the top-level scalar keys out of a plugin descriptor.
///
/// Deliberately line-based: block mappings (`commands:` followed by indented children) and flow
/// sequences other than `authors` are skipped rather than interpreted.
pub fn parse_bukkit_plugin_descriptor(source: &str) -> Option<BukkitPluginDescriptor> {
    let mut descriptor = BukkitPluginDescriptor::default();

    for raw_line in source.lines() {
        let line = strip_comment(raw_line).trim_end();
        // Indented lines belong to a nested block, not the top level.
        if line.is_empty() || line.starts_with(char::is_whitespace) || line.starts_with('-') {
            continue;
        }

        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        if value.is_empty() {
            continue;
        }

        match key.trim() {
            "name" => descriptor.name = Some(unquote(value)),
            "version" => descriptor.version = Some(unquote(value)),
            "prefix" => descriptor.prefix = Some(unquote(value)),
            "description" => descriptor.description = Some(unquote(value)),
            "main" => descriptor.main = Some(unquote(value)),
            "author" => descriptor.author = Some(unquote(value)),
            "website" => descriptor.website = Some(unquote(value)),
            "authors" => {
                if let Some(names) = parse_flow_sequence(value) {
                    descriptor.authors = Some(AuthorList(names));
                } else {
                    descriptor.authors = Some(AuthorList(vec![unquote(value)]));
                }
            },
            _ => {},
        }
    }

    // `name` is the one field every descriptor is required to have, so its absence means this
    // isn't a plugin descriptor we understand.
    descriptor.name.is_some().then_some(descriptor)
}

/// Removes a trailing `#` comment, respecting quoted values so URLs like `https://x/#y` survive.
fn strip_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut quote: Option<u8> = None;

    for (index, byte) in bytes.iter().enumerate() {
        match quote {
            Some(q) if *byte == q => quote = None,
            Some(_) => {},
            None if *byte == b'\'' || *byte == b'"' => quote = Some(*byte),
            None if *byte == b'#' => {
                // Only a comment when preceded by whitespace or at the start, so `a#b` stays whole.
                return if index == 0 || bytes[index - 1].is_ascii_whitespace() {
                    &line[..index]
                } else {
                    line
                };
            },
            None => {},
        }
    }

    line
}

/// Parses `[a, b, "c d"]` into its entries. Returns `None` if the value isn't a flow sequence.
fn parse_flow_sequence(value: &str) -> Option<Vec<String>> {
    let inner = value.strip_prefix('[')?.strip_suffix(']')?;
    let mut items = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;

    for ch in inner.chars() {
        match quote {
            Some(q) if ch == q => quote = None,
            Some(_) => current.push(ch),
            None if ch == '\'' || ch == '"' => quote = Some(ch),
            None if ch == ',' => {
                let item = unquote(current.trim());
                if !item.is_empty() {
                    items.push(item);
                }
                current.clear();
            },
            None => current.push(ch),
        }
    }

    let item = unquote(current.trim());
    if !item.is_empty() {
        items.push(item);
    }

    Some(items)
}

fn unquote(value: &str) -> String {
    let trimmed = value.trim();
    for quote in ['\'', '"'] {
        if trimmed.len() >= 2 && trimmed.starts_with(quote) && trimmed.ends_with(quote) {
            return trimmed[1..trimmed.len() - 1].to_owned();
        }
    }
    trimmed.to_owned()
}

/// File names searched, in order, for a plugin's bundled icon. Bukkit has no standard key for
/// this, so these are the conventional ones.
pub static PLUGIN_ICON_NAMES: Lazy<[&'static str; 5]> =
    Lazy::new(|| ["plugin.png", "icon.png", "logo.png", "plugin_icon.png", "banner.png"]);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_top_level_keys() {
        let source = "\
name: GriefPrevention3D
prefix: GriefPrevention3D
main: me.ryanhamshire.GriefPrevention.GriefPrevention
version: \"18.4.9\"
description: The self-service anti-griefing plugin
website: https://castled.codes
authors: [castledking, RoboMWM, BigScary]
api-version: \"1.13\"
commands:
  claim:
    description: Command to manage a claim
";
        let descriptor = parse_bukkit_plugin_descriptor(source).expect("parsed");

        assert_eq!(descriptor.name.as_deref(), Some("GriefPrevention3D"));
        assert_eq!(descriptor.version.as_deref(), Some("18.4.9"));
        assert_eq!(
            descriptor.authors_string().as_deref(),
            Some("By castledking, RoboMWM, BigScary")
        );
    }

    #[test]
    fn reads_single_author() {
        let descriptor = parse_bukkit_plugin_descriptor("name: CraftServerManager\nauthor: CraftServerManager Team\n")
            .expect("parsed");
        assert_eq!(
            descriptor.authors_string().as_deref(),
            Some("By CraftServerManager Team")
        );
    }

    #[test]
    fn single_entry_author_list_is_not_wrapped_in_brackets() {
        let descriptor =
            parse_bukkit_plugin_descriptor("name: Solo\nauthors: [nisovin]\n").expect("parsed");
        assert_eq!(descriptor.authors_string().as_deref(), Some("By nisovin"));
    }

    #[test]
    fn keeps_urls_containing_hashes() {
        let descriptor =
            parse_bukkit_plugin_descriptor("name: Site\nwebsite: https://example.com/page#anchor\n")
                .expect("parsed");
        assert_eq!(descriptor.website.as_deref(), Some("https://example.com/page#anchor"));
    }

    #[test]
    fn strips_trailing_comments() {
        let descriptor =
            parse_bukkit_plugin_descriptor("name: Commented # trailing note\n").expect("parsed");
        assert_eq!(descriptor.name.as_deref(), Some("Commented"));
    }

    #[test]
    fn rejects_documents_without_a_name() {
        assert!(parse_bukkit_plugin_descriptor("version: 1.0\nmain: com.example.Main\n").is_none());
    }

    #[test]
    fn nested_values_do_not_override_top_level_keys() {
        let source = "\
name: Real
commands:
  name: Nested
";
        let descriptor = parse_bukkit_plugin_descriptor(source).expect("parsed");
        assert_eq!(descriptor.name.as_deref(), Some("Real"));
    }
}