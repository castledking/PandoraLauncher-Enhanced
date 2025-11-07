#![deny(unused_must_use)]

mod backend;
use std::path::Path;

pub use backend::*;
use sha1::{Digest, Sha1};

mod backend_filesystem;
mod backend_handler;

mod metadata;
mod instance;
mod launch;
mod launch_wrapper;
mod directories;
mod log_reader;
mod mod_metadata;
mod modrinth;
mod account;

pub(crate) fn is_single_component_path(path: &str) -> bool {
    let path = std::path::Path::new(path);
    let mut components = path.components().peekable();

    if let Some(first) = components.peek() {
        if !matches!(first, std::path::Component::Normal(_)) {
            return false;
        }
    }

    components.count() == 1
}

pub(crate) fn check_sha1_hash(path: &Path, expected_hash: [u8; 20]) -> std::io::Result<bool> {
    let mut file = std::fs::File::open(&path)?;
    let mut hasher = Sha1::new();
    let _ = std::io::copy(&mut file, &mut hasher)?;
    
    let actual_hash = hasher.finalize();

    Ok(expected_hash == *actual_hash)
}
