use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use serde::{Deserialize, Serialize};

use crate::content_label::ContentLabel;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct AuxiliaryContentMeta {
    #[serde(
        default,
        skip_serializing_if = "crate::skip_if_default",
        deserialize_with = "crate::try_deserialize"
    )]
    pub applied_overrides: AuxAppliedOverrides,
    #[serde(
        default,
        skip_serializing_if = "crate::skip_if_default",
        deserialize_with = "crate::try_deserialize"
    )]
    pub disabled_children: AuxDisabledChildren,
    /// The user's category and highlight colour for this item.
    #[serde(
        default,
        skip_serializing_if = "crate::skip_if_default",
        deserialize_with = "crate::try_deserialize"
    )]
    pub label: ContentLabel,
    /// Labels for a modpack's own files, keyed by [`crate::content_label::child_label_keys`].
    #[serde(
        default,
        skip_serializing_if = "BTreeMap::is_empty",
        deserialize_with = "crate::try_deserialize"
    )]
    pub child_labels: BTreeMap<Arc<str>, ContentLabel>,
}

impl AuxiliaryContentMeta {
    /// Whether there is nothing worth keeping a file for.
    pub fn is_empty(&self) -> bool {
        self.applied_overrides == AuxAppliedOverrides::default()
            && self.disabled_children == AuxDisabledChildren::default()
            && self.label.is_empty()
            && self.child_labels.is_empty()
    }
}

#[derive(Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuxAppliedOverrides {
    pub filename_to_hash: BTreeMap<Arc<str>, Arc<str>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuxDisabledChildren {
    // For files that are enabled by default
    pub disabled_ids: BTreeSet<Arc<str>>,
    pub disabled_names: BTreeSet<Arc<str>>,
    pub disabled_filenames: BTreeSet<Arc<str>>,
    #[serde(default, skip_serializing_if = "crate::skip_if_default")]
    pub deleted_filenames: BTreeSet<Arc<str>>,
    // For files that are disabled by default
    pub enabled_ids: BTreeSet<Arc<str>>,
    pub enabled_names: BTreeSet<Arc<str>>,
    pub enabled_filenames: BTreeSet<Arc<str>>,
}

impl AuxDisabledChildren {
    pub fn is_enabled(&self, disabled_default: bool, id: Option<&str>, name: Option<&str>, filename: &str) -> bool {
        if self.deleted_filenames.contains(filename) {
            return false;
        }

        if disabled_default {
            if let Some(id) = id
                && self.enabled_ids.contains(id)
            {
                return true;
            }
            if let Some(name) = name
                && self.enabled_names.contains(name)
            {
                return true;
            }
            self.enabled_filenames.contains(filename)
        } else {
            if let Some(id) = id
                && self.disabled_ids.contains(id)
            {
                return false;
            }
            if let Some(name) = name
                && self.disabled_names.contains(name)
            {
                return false;
            }
            !self.disabled_filenames.contains(filename)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_label::HexColor;

    #[test]
    fn files_from_before_labels_still_load() {
        let json = r#"{"disabled_children":{"disabled_ids":["sodium"],"disabled_names":[],"disabled_filenames":[],
            "enabled_ids":[],"enabled_names":[],"enabled_filenames":[]}}"#;
        let aux: AuxiliaryContentMeta = serde_json::from_str(json).unwrap();
        assert!(aux.disabled_children.disabled_ids.contains("sodium"));
        assert!(aux.label.is_empty());
        assert!(aux.child_labels.is_empty());
        assert!(!aux.is_empty());
    }

    #[test]
    fn label_only_file_round_trips() {
        let mut aux = AuxiliaryContentMeta::default();
        assert!(aux.is_empty());

        aux.label.category = Some("optimization".into());
        aux.child_labels.insert(
            "id:sodium".into(),
            ContentLabel {
                category: None,
                color: Some(HexColor::new(0xf59e0b)),
            },
        );
        assert!(!aux.is_empty());

        let json = serde_json::to_string(&aux).unwrap();
        assert_eq!(
            json,
            r##"{"label":{"category":"optimization"},"child_labels":{"id:sodium":{"color":"#f59e0b"}}}"##
        );

        let read: AuxiliaryContentMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(read.label, aux.label);
        assert_eq!(read.child_labels, aux.child_labels);
    }

    #[test]
    fn corrupt_label_does_not_lose_disabled_children() {
        let json = r#"{"label":42,"disabled_children":{"disabled_ids":["sodium"],"disabled_names":[],
            "disabled_filenames":[],"enabled_ids":[],"enabled_names":[],"enabled_filenames":[]}}"#;
        let aux: AuxiliaryContentMeta = serde_json::from_str(json).unwrap();
        assert!(aux.label.is_empty());
        assert!(aux.disabled_children.disabled_ids.contains("sodium"));
    }
}
