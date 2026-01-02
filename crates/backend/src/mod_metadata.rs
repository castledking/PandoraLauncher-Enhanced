use std::{
    collections::HashMap, fs::File, io::{Cursor, Read, Seek, Write}, path::{Path, PathBuf}, sync::{atomic::AtomicBool, Arc}
};

use bridge::instance::{AtomicContentUpdateStatus, ContentUpdateStatus, LoaderSpecificModSummary, ModSummary};
use image::imageops::FilterType;
use indexmap::IndexMap;
use parking_lot::{RwLock, RwLockReadGuard};
use rustc_hash::FxHashMap;
use schema::{content::ContentSource, modification::ModrinthModpackFileDownload, modrinth::{ModrinthFile, ModrinthSideRequirement}};
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, DeserializeAs, SerializeAs};
use sha1::{Digest, Sha1};
use zip::{read::ZipFile, ZipArchive};

#[derive(Clone)]
pub enum ModUpdateAction {
    ErrorNotFound,
    ErrorInvalidHash,
    AlreadyUpToDate,
    ManualInstall,
    Modrinth(ModrinthFile),
}

impl ModUpdateAction {
    pub fn to_status(&self) -> ContentUpdateStatus {
        match self {
            ModUpdateAction::ErrorNotFound => ContentUpdateStatus::ErrorNotFound,
            ModUpdateAction::ErrorInvalidHash => ContentUpdateStatus::ErrorInvalidHash,
            ModUpdateAction::AlreadyUpToDate => ContentUpdateStatus::AlreadyUpToDate,
            ModUpdateAction::ManualInstall => ContentUpdateStatus::ManualInstall,
            ModUpdateAction::Modrinth(_) => ContentUpdateStatus::Modrinth,
        }
    }
}

pub struct ModMetadataManager {
    content_library_dir: Arc<Path>,
    sources_json: PathBuf,
    by_hash: RwLock<FxHashMap<[u8; 20], Option<Arc<ModSummary>>>>,
    content_sources: RwLock<FxHashMap<[u8; 20], ContentSource>>,
    parents_by_missing_child: RwLock<FxHashMap<[u8; 20], Vec<[u8; 20]>>>,
    pub updates: RwLock<FxHashMap<[u8; 20], ModUpdateAction>>,
}

impl ModMetadataManager {
    pub fn load(content_meta_dir: Arc<Path>, content_library_dir: Arc<Path>) -> Self {
        let sources_json = content_meta_dir.join("sources.json");

        let content_sources = if let Ok(data) = std::fs::read(&sources_json) {
            let content_sources = serde_json::from_slice(&data);
            content_sources.map(|v: DeserializedContentSources| v.0).unwrap_or_default()
        } else {
            Default::default()
        };

        Self {
            content_library_dir,
            sources_json,
            by_hash: Default::default(),
            content_sources: RwLock::new(content_sources),
            parents_by_missing_child: Default::default(),
            updates: Default::default(),
        }
    }

    pub fn read_content_sources(&self) -> RwLockReadGuard<'_, FxHashMap<[u8; 20], ContentSource>> {
        self.content_sources.read()
    }

    pub fn set_content_sources(&self, sources: impl Iterator<Item = ([u8; 20], ContentSource)>) {
        let mut content_sources = self.content_sources.write();

        let mut changed = false;
        for (hash, source) in sources {
            let old = content_sources.insert(hash, source);
            if old != Some(source) {
                changed = true;
            }
        }

        if changed {
            let serialized = SerializedContentSources(&content_sources);
            if let Ok(content) = serde_json::to_vec(&serialized) {
                let _ = crate::write_safe(&self.sources_json, &content);
            }
        }
    }

    pub fn get_path(self: &Arc<Self>, path: &Path) -> Option<Arc<ModSummary>> {
        let mut file = std::fs::File::open(path).ok()?;
        self.get_file(&mut file)
    }

    pub fn get_file(self: &Arc<Self>, file: &mut std::fs::File) -> Option<Arc<ModSummary>> {
        let mut hasher = Sha1::new();
        let _ = std::io::copy(file, &mut hasher).ok()?;
        let actual_hash: [u8; 20] = hasher.finalize().into();

        if let Some(summary) = self.by_hash.read().get(&actual_hash) {
            return summary.clone();
        }

        let summary = self.load_mod_summary(actual_hash, file, true);

        self.put(actual_hash, summary.clone());

        summary
    }

    pub fn get_bytes(self: &Arc<Self>, bytes: &[u8]) -> Option<Arc<ModSummary>> {
        let mut hasher = Sha1::new();
        hasher.write_all(bytes).ok()?;
        let actual_hash: [u8; 20] = hasher.finalize().into();

        if let Some(summary) = self.by_hash.read().get(&actual_hash) {
            return summary.clone();
        }

        let summary = self.load_mod_summary(actual_hash, &mut Cursor::new(bytes), true);

        self.put(actual_hash, summary.clone());

        summary
    }

    fn put(self: &Arc<Self>, hash: [u8; 20], summary: Option<Arc<ModSummary>>) {
        self.by_hash.write().insert(hash, summary.clone());

        if let Some(parents) = self.parents_by_missing_child.write().remove(&hash) {
            // Remove cached summary of parent, so it can be recalculated next time it is requested
            let mut by_hash = self.by_hash.write();
            for parent in parents {
                by_hash.remove(&parent);
            }
        }
    }

    fn load_mod_summary<R: Read + Seek>(self: &Arc<Self>, hash: [u8; 20], file: &mut R, allow_children: bool) -> Option<Arc<ModSummary>> {
        let archive = zip::ZipArchive::new(file).ok()?;

        if archive.index_for_name("fabric.mod.json").is_some() {
            Self::load_fabric_mod(hash, archive)
        } else if allow_children && archive.index_for_name("modrinth.index.json").is_some() {
            self.load_modrinth_modpack(hash, archive)
        } else {
            None
        }
    }

    fn load_fabric_mod<R: Read + Seek>(hash: [u8; 20], mut archive: ZipArchive<&mut R>) -> Option<Arc<ModSummary>> {
        let mut file = match archive.by_name("fabric.mod.json") {
            Ok(file) => file,
            Err(..) => {
                return None;
            },
        };

        let mut file_content = String::with_capacity(file.size() as usize);
        file.read_to_string(&mut file_content).ok()?;

        // Some mods violate the JSON spec by using raw newline characters inside strings (e.g. BetterGrassify)
        file_content = file_content.replace("\n", " ");

        let fabric_mod_json: FabricModJson = serde_json::from_str(&file_content).inspect_err(|e| {
            eprintln!("Error parsing fabric.mod.json: {e}");
        }).ok()?;

        drop(file);

        let name = fabric_mod_json.name.unwrap_or_else(|| Arc::clone(&fabric_mod_json.id));

        let icon = match fabric_mod_json.icon {
            Some(icon) => match icon {
                Icon::Single(icon) => Some(icon),
                Icon::Sizes(hash_map) => {
                    const DESIRED_SIZE: usize = 64;
                    hash_map.iter().min_by_key(|size| size.0.abs_diff(DESIRED_SIZE)).map(|e| Arc::clone(e.1))
                },
            },
            None => None,
        };

        let mut png_icon: Option<Arc<[u8]>> = None;
        if let Some(icon) = icon && let Ok(icon_file) = archive.by_name(&icon) {
            png_icon = load_icon(icon_file);
        }

        let authors = if let Some(authors) = fabric_mod_json.authors && let Some(authors) = create_authors_string(&authors) {
            authors.into()
        } else {
            "".into()
        };

        let mut lowercase_search_key = fabric_mod_json.id.to_lowercase();
        lowercase_search_key.push_str("$$");
        lowercase_search_key.push_str(&name.to_lowercase());

        Some(Arc::new(ModSummary {
            id: fabric_mod_json.id,
            hash,
            name,
            lowercase_search_key: lowercase_search_key.into(),
            authors,
            version_str: format!("v{}", fabric_mod_json.version).into(),
            png_icon,
            update_status: Arc::new(AtomicContentUpdateStatus::new(ContentUpdateStatus::Unknown)),
            extra: LoaderSpecificModSummary::Fabric
        }))
    }

    fn load_modrinth_modpack<R: Read + Seek>(self: &Arc<Self>, hash: [u8; 20], mut archive: ZipArchive<&mut R>) -> Option<Arc<ModSummary>> {
        let file = match archive.by_name("modrinth.index.json") {
            Ok(file) => file,
            Err(..) => {
                return None;
            },
        };

        let modrinth_index_json: ModrinthIndexJson = serde_json::from_reader(file).unwrap();

        let mut overrides: IndexMap<Arc<Path>, Arc<[u8]>> = IndexMap::new();

        for i in 0..archive.len() {
            let mut file = archive.by_index(i).unwrap();
            let Some(enclosed) = file.enclosed_name() else {
                continue;
            };
            if !file.is_file() {
                continue;
            }

            let (prioritize, path) = if let Ok(path) = enclosed.strip_prefix("overrides") {
                (false, path)
            } else if let Ok(path) = enclosed.strip_prefix("client-overrides") {
                (true, path)
            } else {
                continue;
            };

            let path = path.into();

            if !prioritize && overrides.contains_key(&path) {
                continue;
            }

            let mut data = Vec::with_capacity(file.size() as usize);
            if file.read_to_end(&mut data).is_err() {
                continue;
            }
            overrides.insert(path, data.into());
        }

        let lowercase_search_key = modrinth_index_json.name.to_lowercase();

        let summaries_fut = modrinth_index_json.files.iter().cloned().map(|download| {
            async move {
                let this = self.clone();
                tokio::task::spawn_blocking(move || {
                    if let Some(env) = download.env {
                        if env.client == ModrinthSideRequirement::Unsupported {
                            return None;
                        }
                    }

                    let mut file_hash = [0u8; 20];
                    let Ok(_) = hex::decode_to_slice(&*download.hashes.sha1, &mut file_hash) else {
                        return None;
                    };

                    if let Some(cached) = this.by_hash.read().get(&file_hash).cloned() {
                        return cached;
                    }

                    let file_hash_as_str = hex::encode(file_hash);

                    let mut file = this.content_library_dir.join(&file_hash_as_str[..2]);
                    file.push(&file_hash_as_str);
                    if let Some(extension) = typed_path::Utf8UnixPath::new(&*download.path).extension() {
                        file.set_extension(extension);
                    }

                    if let Ok(mut file) = std::fs::File::open(file) {
                        let summary = this.load_mod_summary(file_hash, &mut file, false);
                        this.put(file_hash, summary.clone());
                        return summary;
                    }

                    this.parents_by_missing_child.write().entry(file_hash).or_default().push(hash);

                    None
                }).await.ok().flatten()
            }
        });
        let summaries = futures::executor::block_on(futures::future::join_all(summaries_fut));

        let mut png_icon = None;

        if let Ok(icon) = archive.by_name("icon.png") {
            png_icon = load_icon(icon);
        }

        let authors = if let Some(authors) = modrinth_index_json.authors && let Some(authors) = create_authors_string(&authors) {
            authors.into()
        } else if let Some(author) = modrinth_index_json.author {
            format!("By {}", author.name()).into()
        } else {
            "".into()
        };

        Some(Arc::new(ModSummary {
            id: "".into(),
            hash,
            name: modrinth_index_json.name,
            lowercase_search_key: lowercase_search_key.into(),
            authors,
            version_str: format!("v{}", modrinth_index_json.version_id).into(),
            png_icon,
            update_status: Arc::new(AtomicContentUpdateStatus::new(ContentUpdateStatus::Unknown)),
            extra: LoaderSpecificModSummary::ModrinthModpack {
                downloads: modrinth_index_json.files,
                summaries: summaries.into(),
                overrides: overrides.into_iter().collect(),
            }
        }))
    }
}

fn load_icon<R: Read + Seek>(mut icon_file: ZipFile<'_, &mut R>) -> Option<Arc<[u8]>> {
    let mut icon_bytes = Vec::with_capacity(icon_file.size() as usize);
    let Ok(_) = icon_file.read_to_end(&mut icon_bytes) else {
        return None;
    };

    let Ok(image) = image::load_from_memory(&icon_bytes) else {
        return None;
    };

    let width = image.width();
    let height = image.height();
    if width != 64 || height != 64 {
        let filter = if width > 64 || height > 64 {
            FilterType::Lanczos3
        } else {
            FilterType::Nearest
        };
        let resized = image.resize_exact(64, 64, filter);

        icon_bytes.clear();
        let mut cursor = Cursor::new(&mut icon_bytes);
        if resized.write_to(&mut cursor, image::ImageFormat::Png).is_err() {
            return None;
        }
    }

    Some(icon_bytes.into())
}

fn create_authors_string(authors: &[Person]) -> Option<String> {
    if !authors.is_empty() {
        let mut authors_string = "By ".to_owned();
        let mut first = true;
        for author in authors {
            if first {
                first = false;
            } else {
                authors_string.push_str(", ");
            }
            authors_string.push_str(author.name());
        }
        Some(authors_string.into())
    } else {
        None
    }
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct FabricModJson {
    id: Arc<str>,
    version: Arc<str>,
    name: Option<Arc<str>>,
    // description: Option<Arc<str>>,
    authors: Option<Vec<Person>>,
    icon: Option<Icon>,
    // #[serde(alias = "requires")]
    // depends: Option<HashMap<Arc<str>, Dependency>>,
    // breaks: Option<HashMap<Arc<str>, Dependency>>,
}

// #[derive(Deserialize, Debug)]
// #[serde(untagged)]
// enum Dependency {
//     Single(Arc<str>),
//     Multiple(Vec<Arc<str>>)
// }

#[derive(Deserialize, Debug)]
#[serde(untagged)]
enum Icon {
    Single(Arc<str>),
    Sizes(HashMap<usize, Arc<str>>),
}

#[derive(Deserialize, Debug)]
#[serde(untagged)]
enum Person {
    Name(Arc<str>),
    NameAndContact { name: Arc<str> },
}

impl Person {
    pub fn name(&self) -> &str {
        match self {
            Person::Name(name) => name,
            Person::NameAndContact { name, .. } => name,
        }
    }
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct ModrinthIndexJson {
    version_id: Arc<str>,
    name: Arc<str>,
    files: Arc<[ModrinthModpackFileDownload]>,

    // Unofficial
    #[serde(default)]
    #[serde(deserialize_with = "skip_if_deserialization_fails")]
    authors: Option<Vec<Person>>,
    #[serde(default)]
    #[serde(deserialize_with = "skip_if_deserialization_fails")]
    author: Option<Person>,
}

#[serde_as]
#[derive(Serialize)]
struct SerializedContentSources<'a>(
    #[serde_as(as = "FxHashMap<SerializeAsHex, _>")]
    &'a FxHashMap<[u8; 20], ContentSource>
);

struct SerializeAsHex {}

impl SerializeAs<[u8; 20]> for SerializeAsHex {
    fn serialize_as<S>(source: &[u8; 20], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer {
        hex::serde::serialize(source, serializer)
    }
}

#[serde_as]
#[derive(Deserialize)]
struct DeserializedContentSources(
    #[serde_as(as = "FxHashMap<DeserializeAsHex, _>")]
    FxHashMap<[u8; 20], ContentSource>
);

struct DeserializeAsHex {}

impl<'de> DeserializeAs<'de, [u8; 20]> for DeserializeAsHex {
    fn deserialize_as<D>(deserializer: D) -> Result<[u8; 20], D::Error>
    where
        D: serde::Deserializer<'de> {
        hex::serde::deserialize(deserializer)
    }
}

fn skip_if_deserialization_fails<'de, T, D>(deserializer: D) -> Result<Option<T>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    match T::deserialize(deserializer) {
        Ok(value) => Ok(Some(value)),
        Err(_) => Ok(None),
    }
}
