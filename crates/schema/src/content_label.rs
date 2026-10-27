//! Labels the user puts on single pieces of content, such as a mod, a resource pack or a plugin, so
//! that a long list can be read at a glance: a category, and a colour to tint its row with.
//!
//! Local overrides are kept in the content's auxiliary file (see [`crate::auxiliary`]), which
//! follows the item through updates. Global defaults are kept in the interface configuration.

use std::{collections::BTreeMap, fmt, sync::Arc};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};

/// An opaque RGB colour, written as `#rrggbb`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HexColor(u32);

impl HexColor {
    pub const fn new(rgb: u32) -> Self {
        Self(rgb & 0x00FF_FFFF)
    }

    pub const fn rgb(self) -> u32 {
        self.0
    }

    /// Reads `#rgb` or `#rrggbb`, with or without the `#` and surrounding whitespace.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let hex = text.strip_prefix('#').unwrap_or(text);
        // from_str_radix would also take a leading sign
        if !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }

        let value = u32::from_str_radix(hex, 16).ok()?;
        match hex.len() {
            3 => {
                let (r, g, b) = ((value >> 8) & 0xF, (value >> 4) & 0xF, value & 0xF);
                Some(Self::new((r * 0x11) << 16 | (g * 0x11) << 8 | (b * 0x11)))
            },
            6 => Some(Self::new(value)),
            _ => None,
        }
    }
}

impl fmt::Display for HexColor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:06x}", self.0)
    }
}

impl Serialize for HexColor {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for HexColor {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        HexColor::parse(&text).ok_or_else(|| D::Error::custom(format!("invalid colour {text:?}")))
    }
}

/// How the user has marked one piece of content.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentLabel {
    /// The category's id: a Modrinth category such as `library`, or a name the user made up.
    #[serde(
        default,
        skip_serializing_if = "crate::skip_if_none",
        deserialize_with = "crate::try_deserialize"
    )]
    pub category: Option<Arc<str>>,
    /// A colour picked for this item alone, shown instead of its category's colour.
    #[serde(
        default,
        skip_serializing_if = "crate::skip_if_none",
        deserialize_with = "crate::try_deserialize"
    )]
    pub color: Option<HexColor>,
    /// An explicit local removal must hide a global default, even though the value is `None`.
    #[serde(
        default,
        skip_serializing_if = "crate::skip_if_default",
        deserialize_with = "crate::try_deserialize"
    )]
    pub category_override: bool,
    #[serde(
        default,
        skip_serializing_if = "crate::skip_if_default",
        deserialize_with = "crate::try_deserialize"
    )]
    pub color_override: bool,
}

impl ContentLabel {
    pub fn is_empty(&self) -> bool {
        self.category.is_none() && self.color.is_none() && !self.category_override && !self.color_override
    }

    /// Resolve each field separately. Existing local labels also override defaults, without
    /// requiring a migration of their auxiliary files.
    pub fn resolve(&self, global: &Self) -> Self {
        let category_override = self.category.is_some() || self.category_override;
        let color_override = self.color.is_some() || self.color_override;
        Self {
            category: if category_override {
                self.category.clone()
            } else {
                global.category.clone()
            },
            color: if color_override { self.color } else { global.color },
            category_override,
            color_override,
        }
    }
}

/// A version-independent default shared by all instances and servers. Keys include the content
/// folder and identity type, so a plugin, mod and resource pack cannot collide by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlobalContentLabel {
    pub keys: Vec<Arc<str>>,
    pub name: Arc<str>,
    pub label: ContentLabel,
}

pub fn find_global_label<'a>(labels: &'a [GlobalContentLabel], keys: &[Arc<str>]) -> Option<&'a GlobalContentLabel> {
    keys.iter().find_map(|key| labels.iter().find(|label| label.keys.contains(key)))
}

/// Changes only the global default, never any instance's auxiliary files. Retain identity aliases
/// learned from installed builds, so a rule made for a manual mod also matches its project ID.
pub fn edit_global_label(
    labels: &mut Vec<GlobalContentLabel>,
    keys: &[Arc<str>],
    name: Arc<str>,
    edit: &ContentLabelEdit,
) {
    if keys.is_empty() {
        return;
    }
    let index = keys.iter().find_map(|key| labels.iter().position(|label| label.keys.contains(key)));
    let mut rule = index.map(|index| labels.remove(index)).unwrap_or_else(|| GlobalContentLabel {
        keys: Vec::new(),
        name,
        label: ContentLabel::default(),
    });
    for key in keys {
        if !rule.keys.contains(key) {
            rule.keys.push(key.clone());
        }
    }
    edit.apply(&mut rule.label);
    if !rule.label.is_empty() {
        labels.insert(index.unwrap_or(labels.len()), rule);
    }
}

/// A change to the labels of one or more items.
///
/// The category and the colour are changed separately, so that picking one for several items at
/// once leaves each item's other half as it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentLabelEdit {
    SetCategory(Option<Arc<str>>),
    SetColor(Option<HexColor>),
    Clear,
    UseGlobal,
}

impl ContentLabelEdit {
    /// Applies the edit, returning whether the label changed.
    pub fn apply(&self, label: &mut ContentLabel) -> bool {
        let before = label.clone();
        match self {
            ContentLabelEdit::SetCategory(category) => {
                label.category =
                    category.as_deref().map(str::trim).filter(|category| !category.is_empty()).map(Arc::from);
            },
            ContentLabelEdit::SetColor(color) => label.color = *color,
            ContentLabelEdit::Clear | ContentLabelEdit::UseGlobal => *label = ContentLabel::default(),
        }
        *label != before
    }

    /// Local edits distinguish removing a value from inheriting it. Positive values from older
    /// auxiliary files already count as overrides; only explicit removals need extra storage.
    pub fn apply_local(&self, label: &mut ContentLabel) -> bool {
        let before = label.clone();
        self.apply(label);
        match self {
            Self::SetCategory(_) => label.category_override = label.category.is_none(),
            Self::SetColor(_) => label.color_override = label.color.is_none(),
            Self::Clear => {
                label.category_override = true;
                label.color_override = true;
            },
            Self::UseGlobal => {},
        }
        *label != before
    }
}

/// The keys a modpack child's label can be filed under in its modpack's auxiliary file, best first.
///
/// Like the record of disabled children, this prefers the child's mod id, then its name, then its
/// path, so that a label survives the modpack moving the child onto a newer file. A child that
/// hasn't been downloaded yet only has its path, so the label is looked up under every key the
/// child has and moved to the best one the next time it's changed. The prefixes keep an id from
/// ever being mistaken for a path.
pub fn child_label_keys(id: Option<&str>, name: Option<&str>, path: &str) -> Vec<Arc<str>> {
    let id = id.filter(|id| !id.trim().is_empty()).map(|id| format!("id:{id}").into());
    let name = name.filter(|name| !name.trim().is_empty()).map(|name| format!("name:{name}").into());
    id.into_iter().chain(name).chain(std::iter::once(format!("file:{path}").into())).collect()
}

/// The label filed under the first of `keys` that has one.
pub fn find_child_label<'a>(
    labels: &'a BTreeMap<Arc<str>, ContentLabel>,
    keys: &[Arc<str>],
) -> Option<&'a ContentLabel> {
    keys.iter().find_map(|key| labels.get(key))
}

/// Applies `edit` to the label of the child known by `keys` (see [`child_label_keys`]), filing the
/// result under the best key. Returns whether `labels` changed.
pub fn edit_child_label(
    labels: &mut BTreeMap<Arc<str>, ContentLabel>,
    keys: &[Arc<str>],
    edit: &ContentLabelEdit,
) -> bool {
    let Some((best, others)) = keys.split_first() else {
        return false;
    };

    let mut label = find_child_label(labels, keys).cloned().unwrap_or_default();
    let edited = edit.apply_local(&mut label);

    let mut moved = false;
    for key in others {
        moved |= labels.remove(key).is_some();
    }
    if label.is_empty() {
        moved |= labels.remove(best).is_some();
    } else if labels.get(best) != Some(&label) {
        labels.insert(best.clone(), label);
        moved = true;
    }

    edited || moved
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_long_and_short_hex() {
        assert_eq!(HexColor::parse("#3b82f6"), Some(HexColor::new(0x3b82f6)));
        assert_eq!(HexColor::parse("3B82F6"), Some(HexColor::new(0x3b82f6)));
        assert_eq!(HexColor::parse("  #fa0 "), Some(HexColor::new(0xffaa00)));
        assert_eq!(HexColor::parse("000"), Some(HexColor::new(0)));
    }

    #[test]
    fn rejects_malformed_hex() {
        for text in [
            "", "#", "#12", "#1234", "#12345", "#1234567", "#gggggg", "+12345", "#-12345", "red",
        ] {
            assert_eq!(HexColor::parse(text), None, "{text:?} should not parse");
        }
    }

    #[test]
    fn displays_as_lowercase_hash_hex() {
        assert_eq!(HexColor::new(0x3B82F6).to_string(), "#3b82f6");
        assert_eq!(HexColor::new(0x0000ff).to_string(), "#0000ff");
        // Anything above 24 bits is dropped rather than printed
        assert_eq!(HexColor::new(0xff123456).to_string(), "#123456");
    }

    #[test]
    fn label_round_trips_through_json() {
        let label = ContentLabel {
            category: Some("library".into()),
            color: Some(HexColor::new(0xef4444)),
            ..Default::default()
        };
        let json = serde_json::to_string(&label).unwrap();
        assert_eq!(json, r##"{"category":"library","color":"#ef4444"}"##);
        assert_eq!(serde_json::from_str::<ContentLabel>(&json).unwrap(), label);

        assert_eq!(serde_json::to_string(&ContentLabel::default()).unwrap(), "{}");
    }

    #[test]
    fn bad_colour_in_file_is_dropped_not_fatal() {
        let label: ContentLabel = serde_json::from_str(r#"{"category":"utility","color":"not a colour"}"#).unwrap();
        assert_eq!(label.category.as_deref(), Some("utility"));
        assert_eq!(label.color, None);
    }

    #[test]
    fn edits_change_only_their_half() {
        let mut label = ContentLabel {
            category: Some("combat".into()),
            color: Some(HexColor::new(0x22c55e)),
            ..Default::default()
        };

        assert!(ContentLabelEdit::SetColor(Some(HexColor::new(0xef4444))).apply(&mut label));
        assert_eq!(label.category.as_deref(), Some("combat"));
        assert_eq!(label.color, Some(HexColor::new(0xef4444)));

        assert!(ContentLabelEdit::SetCategory(Some("worldgen".into())).apply(&mut label));
        assert_eq!(label.category.as_deref(), Some("worldgen"));
        assert_eq!(label.color, Some(HexColor::new(0xef4444)));

        assert!(!ContentLabelEdit::SetCategory(Some("worldgen".into())).apply(&mut label));

        assert!(ContentLabelEdit::Clear.apply(&mut label));
        assert!(label.is_empty());
        assert!(!ContentLabelEdit::Clear.apply(&mut label));
    }

    #[test]
    fn blank_category_means_none() {
        let mut label = ContentLabel {
            category: Some("utility".into()),
            color: None,
            ..Default::default()
        };
        assert!(ContentLabelEdit::SetCategory(Some("   ".into())).apply(&mut label));
        assert_eq!(label.category, None);

        assert!(ContentLabelEdit::SetCategory(Some("  QoL ".into())).apply(&mut label));
        assert_eq!(label.category.as_deref(), Some("QoL"));
    }

    fn keys(keys: &[Arc<str>]) -> Vec<&str> {
        keys.iter().map(|key| &**key).collect()
    }

    #[test]
    fn child_keys_prefer_id_then_name_then_path() {
        assert_eq!(
            keys(&child_label_keys(Some("sodium"), Some("Sodium"), "mods/sodium.jar")),
            ["id:sodium", "name:Sodium", "file:mods/sodium.jar"]
        );
        assert_eq!(
            keys(&child_label_keys(None, Some("Sodium"), "mods/sodium.jar")),
            ["name:Sodium", "file:mods/sodium.jar"]
        );
        assert_eq!(keys(&child_label_keys(Some(" "), None, "mods/sodium.jar")), ["file:mods/sodium.jar"]);
    }

    #[test]
    fn child_label_follows_a_child_once_it_has_an_id() {
        let red = ContentLabelEdit::SetColor(Some(HexColor::new(0xef4444)));
        let mut labels = BTreeMap::new();

        // Labelled before the modpack's files were downloaded, so only the path is known
        let before = child_label_keys(None, None, "mods/sodium.jar");
        assert!(edit_child_label(&mut labels, &before, &red));
        assert_eq!(keys(&labels.keys().cloned().collect::<Vec<_>>()), ["file:mods/sodium.jar"]);

        // Downloaded since: still found by its path
        let after = child_label_keys(Some("sodium"), Some("Sodium"), "mods/sodium.jar");
        assert_eq!(find_child_label(&labels, &after).unwrap().color, Some(HexColor::new(0xef4444)));

        // and moved to its id on the next change, keeping the colour
        assert!(edit_child_label(
            &mut labels,
            &after,
            &ContentLabelEdit::SetCategory(Some("optimization".into()))
        ));
        assert_eq!(keys(&labels.keys().cloned().collect::<Vec<_>>()), ["id:sodium"]);
        let label = find_child_label(&labels, &after).unwrap();
        assert_eq!(label.category.as_deref(), Some("optimization"));
        assert_eq!(label.color, Some(HexColor::new(0xef4444)));

        // A local removal is retained, so a global default cannot put the label back.
        assert!(edit_child_label(&mut labels, &after, &ContentLabelEdit::Clear));
        let label = find_child_label(&labels, &after).unwrap();
        assert_eq!(label.category, None);
        assert!(label.category_override && label.color_override);
        assert!(!edit_child_label(&mut labels, &after, &ContentLabelEdit::Clear));

        // Returning to global defaults removes the local record.
        assert!(edit_child_label(&mut labels, &after, &ContentLabelEdit::UseGlobal));
        assert!(labels.is_empty());
    }

    #[test]
    fn child_label_edit_without_keys_does_nothing() {
        let mut labels = BTreeMap::new();
        assert!(!edit_child_label(&mut labels, &[], &ContentLabelEdit::SetColor(Some(HexColor::new(1)))));
        assert!(labels.is_empty());
    }

    #[test]
    fn global_defaults_resolve_per_field_and_keep_legacy_local_labels() {
        let global = ContentLabel {
            category: Some("library".into()),
            color: Some(HexColor::new(0x112233)),
            ..Default::default()
        };
        let inherited = ContentLabel::default().resolve(&global);
        assert_eq!(inherited.category, global.category);
        assert_eq!(inherited.color, global.color);
        assert!(!inherited.category_override && !inherited.color_override);

        // Legacy labels have no flags, but their assigned fields still take priority.
        let local: ContentLabel = serde_json::from_str(r#"{"category":"combat"}"#).unwrap();
        let resolved = local.resolve(&global);
        assert_eq!(resolved.category.as_deref(), Some("combat"));
        assert_eq!(resolved.color, global.color);
        assert!(resolved.category_override && !resolved.color_override);

        let local: ContentLabel = serde_json::from_str(r##"{"color":"#abcdef"}"##).unwrap();
        let resolved = local.resolve(&global);
        assert_eq!(resolved.category, global.category);
        assert_eq!(resolved.color, Some(HexColor::new(0xabcdef)));
    }

    #[test]
    fn local_removals_survive_restart_and_can_return_to_inheritance() {
        let mut global = ContentLabel {
            category: Some("utility".into()),
            color: Some(HexColor::new(0x112233)),
            ..Default::default()
        };
        let mut local = ContentLabel::default();
        ContentLabelEdit::SetCategory(None).apply_local(&mut local);
        let local: ContentLabel = serde_json::from_str(&serde_json::to_string(&local).unwrap()).unwrap();
        assert_eq!(local.resolve(&global).category, None);
        assert_eq!(local.resolve(&global).color, global.color);

        let mut local = local;
        ContentLabelEdit::Clear.apply_local(&mut local);
        let mut local: ContentLabel = serde_json::from_str(&serde_json::to_string(&local).unwrap()).unwrap();
        global.category = Some("combat".into());
        global.color = Some(HexColor::new(0xffffff));
        let resolved = local.resolve(&global);
        assert_eq!(resolved.category, None);
        assert_eq!(resolved.color, None);
        assert!(!local.is_empty());

        ContentLabelEdit::UseGlobal.apply_local(&mut local);
        assert!(local.is_empty());
        assert_eq!(local.resolve(&global).category, global.category);
        assert_eq!(local.resolve(&global).color, global.color);
    }

    #[test]
    fn global_assignment_updates_aliases_without_editing_local_labels() {
        let keys: Vec<Arc<str>> = vec!["mods:modrinth:sodium-project".into(), "mods:id:sodium".into()];
        let manual_keys = vec!["mods:id:sodium".into()];
        let mut rules = Vec::new();
        edit_global_label(
            &mut rules,
            &manual_keys,
            "Sodium".into(),
            &ContentLabelEdit::SetCategory(Some("optimization".into())),
        );
        edit_global_label(
            &mut rules,
            &keys,
            "Sodium".into(),
            &ContentLabelEdit::SetColor(Some(HexColor::new(0xff0000))),
        );
        assert_eq!(rules.len(), 1);
        let mut rules: Vec<GlobalContentLabel> = serde_json::from_str(&serde_json::to_string(&rules).unwrap()).unwrap();
        assert_eq!(find_global_label(&rules, &keys[..1]), find_global_label(&rules, &manual_keys));
        let global = &find_global_label(&rules, &keys).unwrap().label;
        let mut local = ContentLabel::default();
        ContentLabelEdit::SetCategory(Some("combat".into())).apply_local(&mut local);
        assert_eq!(local.resolve(global).category.as_deref(), Some("combat"));
        assert_eq!(ContentLabel::default().resolve(global).category.as_deref(), Some("optimization"));
        assert!(find_global_label(&rules, &["plugins:id:sodium".into()]).is_none());
        assert!(find_global_label(&rules, &["mods:id:other-mod".into()]).is_none());
        edit_global_label(&mut rules, &keys, "Sodium".into(), &ContentLabelEdit::Clear);
        assert!(rules.is_empty());
        assert_eq!(local.category.as_deref(), Some("combat"));
    }
}
