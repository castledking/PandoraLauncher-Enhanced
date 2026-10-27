//! Stable identities used by the global content-label settings, shared with modpack children.

use std::sync::Arc;

use schema::content::ContentSource;

use crate::instance::{ContentFolder, ContentSummary, ContentType, ModpackFile};

/// Project IDs and native mod/plugin IDs survive filename and version changes. Names and exact
/// filenames are only fallbacks for manual files without an identity; never guess by stripping
/// version-looking digits, which could match an unrelated file.
pub fn global_content_keys(
    folder: ContentFolder,
    source: &ContentSource,
    summary: Option<&ContentSummary>,
    filename: &str,
) -> Vec<Arc<str>> {
    let folder = folder.folder_name();
    let mut keys = Vec::new();
    match source {
        ContentSource::ModrinthProject { project_id } => keys.push(format!("{folder}:modrinth:{project_id}").into()),
        ContentSource::CurseforgeProject { project_id } => {
            keys.push(format!("{folder}:curseforge:{project_id}").into())
        },
        _ => {},
    }
    if let Some(id) = summary
        .and_then(|summary| summary.id.as_deref())
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        keys.push(format!("{folder}:id:{}", id.to_lowercase()).into());
    }
    if keys.is_empty() {
        if let Some(name) = summary
            .and_then(|summary| summary.name.as_deref())
            .map(str::trim)
            .filter(|name| !name.is_empty())
        {
            keys.push(format!("{folder}:name:{}", name.to_lowercase()).into());
        } else {
            let filename = filename.strip_suffix(".disabled").unwrap_or(filename);
            let filename = filename.rsplit('/').next().unwrap_or(filename);
            keys.push(format!("{folder}:file:{filename}").into());
        }
    }
    keys
}

pub fn modpack_file_source(parent: &ContentSummary, file: &ModpackFile) -> ContentSource {
    match (&parent.extra, &file.project_id) {
        (ContentType::ModrinthModpack { .. }, Some(project_id)) => ContentSource::ModrinthProject {
            project_id: project_id.clone(),
        },
        (ContentType::CurseforgeModpack { .. }, Some(project_id)) => project_id
            .parse()
            .ok()
            .map(|project_id| ContentSource::CurseforgeProject { project_id })
            .unwrap_or_default(),
        _ => ContentSource::Manual,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        instance::{ModpackFilePath, ModpackFileSource, UNKNOWN_CONTENT_SUMMARY},
        safe_path::SafePath,
    };

    #[test]
    fn identity_survives_versions_disabled_state_and_manual_installation() {
        let mut old = (**UNKNOWN_CONTENT_SUMMARY).clone();
        old.id = Some("sodium".into());
        old.version_str = "0.6".into();
        let mut new = old.clone();
        new.version_str = "0.7".into();
        let source = ContentSource::ModrinthProject {
            project_id: "AANobbMI".into(),
        };
        let a = global_content_keys(ContentFolder::Mods, &source, Some(&old), "sodium-0.6.jar");
        let b = global_content_keys(ContentFolder::Mods, &source, Some(&new), "sodium-0.7.jar.disabled");
        assert_eq!(a, b);
        let manual = global_content_keys(ContentFolder::Mods, &ContentSource::Manual, Some(&new), "renamed.jar");
        assert!(manual.iter().any(|key| a.contains(key)));
        let plugin = global_content_keys(ContentFolder::Plugins, &source, Some(&new), "sodium.jar");
        assert!(plugin.iter().all(|key| !a.contains(key)));
    }

    #[test]
    fn modpack_resource_pack_matches_standalone_project_before_download() {
        let mut pack = (**UNKNOWN_CONTENT_SUMMARY).clone();
        pack.extra = ContentType::ModrinthModpack {
            files: Arc::new([]),
            dependencies: Default::default(),
        };
        let file = ModpackFile {
            source: ModpackFileSource::DownloadUrl {
                url: "https://example.com/pack.zip".into(),
                size: 1,
            },
            path: ModpackFilePath::Path(SafePath::new("resourcepacks/pack-1.zip").unwrap()),
            hash: [0; 20],
            summary: None,
            default_disabled: false,
            disabled_third_party_downloads: false,
            project_id: Some("pack-project".into()),
        };
        let child = global_content_keys(
            ContentFolder::ResourcePacks,
            &modpack_file_source(&pack, &file),
            None,
            file.path.as_str(),
        );
        let standalone = global_content_keys(
            ContentFolder::ResourcePacks,
            &ContentSource::ModrinthProject {
                project_id: "pack-project".into(),
            },
            None,
            "pack-2.zip.disabled",
        );
        assert_eq!(child, standalone);
    }

    #[test]
    fn unidentified_files_use_exact_names_without_guessing_versions() {
        let keys = |filename: &str| global_content_keys(ContentFolder::Shaders, &ContentSource::Manual, None, filename);
        assert_eq!(keys("shaderpacks/Shader2.zip"), keys("Shader2.zip.disabled"));
        assert_ne!(keys("Shader2.zip"), keys("Shader3.zip"));
        assert_ne!(
            keys("Shader2.zip"),
            global_content_keys(ContentFolder::ResourcePacks, &ContentSource::Manual, None, "Shader2.zip")
        );
    }
}
