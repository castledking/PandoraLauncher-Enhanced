use std::{collections::HashMap, fs::File, io::{Cursor, Read}, sync::Arc};

use bridge::instance::ModSummary;
use image::imageops::FilterType;
use serde::Deserialize;
use sha1::{Digest, Sha1};
use zip::read::ZipFile;
use std::sync::RwLock;

#[derive(Default)]
pub struct ModMetadataManager {
    by_hash: RwLock<HashMap<[u8; 20], Option<Arc<ModSummary>>>>,
}

impl ModMetadataManager {
    pub fn get(&self, file: &mut std::fs::File) -> Option<Arc<ModSummary>> {
        let mut hasher = Sha1::new();
        let _ = std::io::copy(file, &mut hasher).ok()?;
        let actual_hash: [u8; 20] = hasher.finalize().into();
        
        // todo: cache on disk! (but only when we need to store additional metadata like source)
        
        if let Some(summary) = self.by_hash.read().unwrap().get(&actual_hash) {
            return summary.clone();
        }
        
        let summary = Self::load(file);
        
        self.by_hash.write().unwrap().insert(actual_hash, summary.clone());
        
        summary
    }
    
    pub fn load(file: &mut std::fs::File) -> Option<Arc<ModSummary>> {
        let mut archive = zip::ZipArchive::new(file).ok()?;
        let file = match archive.by_name("fabric.mod.json") {
            Ok(file) => file,
            Err(..) => {
                return None;
            }
        };
        
        let fabric_mod_json: FabricModJson = serde_json::from_reader(file).unwrap();
        
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
        if let Some(icon) = icon {
            if let Ok(icon_file) = archive.by_name(&*icon) {
                png_icon = load_icon(icon_file);
            }
        }
        
        let authors = if let Some(authors) = fabric_mod_json.authors && !authors.is_empty() {
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
            authors_string.into()
        } else {
            "".into()
        };
        
        Some(Arc::new(ModSummary {
            id: fabric_mod_json.id,
            name,
            authors,
            version_str: format!("v{}", fabric_mod_json.version).into(),
            png_icon
        }))
    }
}

fn load_icon(mut icon_file: ZipFile<'_, &mut File>) -> Option<Arc<[u8]>> {
    let mut icon_bytes = Vec::with_capacity(icon_file.size() as usize);
    let Ok(_) = icon_file.read_to_end(&mut icon_bytes) else {
        return None;
    };
    
    let Ok(image) = image::load_from_memory(&*icon_bytes) else {
        return None;
    };
    
    let width = image.width();
    let height = image.height();
    if image.width() != 64 || image.height() != 64 {
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
    
    icon_bytes.shrink_to_fit();
    return Some(icon_bytes.into());
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct FabricModJson {
    id: Arc<str>,
    version: Arc<str>,
    name: Option<Arc<str>>,
    description: Option<Arc<str>>,
    authors: Option<Vec<Person>>,
    icon: Option<Icon>,
    #[serde(alias = "requires")]
    depends: Option<HashMap<Arc<str>, Dependency>>,
    breaks: Option<HashMap<Arc<str>, Dependency>>,
}

#[derive(Deserialize, Debug)]
#[serde(untagged)]
enum Dependency {
    Single(Arc<str>),
    Multiple(Vec<Arc<str>>)
}

#[derive(Deserialize, Debug)]
#[serde(untagged)]
enum Icon {
    Single(Arc<str>),
    Sizes(HashMap<usize, Arc<str>>)
}

#[derive(Deserialize, Debug)]
#[serde(untagged)]
enum Person {
    Name(Arc<str>),
    NameAndContact {
        name: Arc<str>,
    }
}

impl Person {
    pub fn name(&self) -> &str {
        match self {
            Person::Name(name) => name,
            Person::NameAndContact { name, .. } => name,
        }
    }
}
