//! Moving datapacks from an instance's datapacks folder into its worlds.
//!
//! Vanilla only reads datapacks from inside a world, so a client instance's `datapacks/` folder is
//! a library: nothing loads from it until a datapack is copied into a world. A world picks a new
//! datapack up the next time it's opened.

use std::path::Path;

use bridge::instance::{ContentFolder, InstanceID};

use crate::BackendState;

impl BackendState {
    pub fn add_datapack_to_world(&self, id: InstanceID, datapack: &Path, world: &Path) {
        let Some((datapacks_dir, saves_dir)) = self.instance_state.read().instances.get(id).map(|instance| {
            (instance.content_state[ContentFolder::Datapacks].path.clone(), instance.saves_path.clone())
        }) else {
            return;
        };

        // Only ever from this instance's datapacks into one of its own worlds
        if datapack.parent() != Some(&*datapacks_dir) || world.parent() != Some(&*saves_dir) || !world.is_dir() {
            log::warn!("Refusing to copy {datapack:?} into {world:?}: not this instance's datapack and world");
            return;
        }
        let Some(file_name) = datapack.file_name().and_then(|name| name.to_str()) else {
            return;
        };
        // A datapack switched off in the library still goes in switched on
        let target_name = file_name.strip_suffix(".disabled").unwrap_or(file_name);
        let target = world.join("datapacks").join(target_name);
        let world_name = world.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();

        let result = std::fs::create_dir_all(world.join("datapacks")).and_then(|_| {
            if datapack.is_dir() {
                copy_dir(datapack, &target)
            } else {
                // A copy of its own, so the world keeps working if the library's is updated or removed
                crate::fs::fastcopy(datapack, &target, true, false)
            }
        });

        match result {
            Ok(()) => self.send.send_success(format!("Added {target_name} to {world_name}")),
            Err(err) => {
                log::error!("Unable to copy {datapack:?} into {target:?}: {err}");
                self.send.send_error(format!("Unable to add {target_name} to {world_name}: {err}"));
            },
        }
    }
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    for entry in walkdir::WalkDir::new(from) {
        let entry = entry.map_err(std::io::Error::other)?;
        let Ok(relative) = entry.path().strip_prefix(from) else {
            continue;
        };
        let target = to.join(relative);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&target)?;
        } else if entry.file_type().is_file() {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}
