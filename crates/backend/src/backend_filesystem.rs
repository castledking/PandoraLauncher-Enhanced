use std::{collections::{HashMap, HashSet}, path::Path, sync::Arc};

use bridge::instance::InstanceID;
use notify::{event::{CreateKind, DataChange, ModifyKind, RemoveKind, RenameMode}, EventKind};

use crate::{BackendState, WatchTarget};

#[derive(Debug)]
enum FilesystemEvent {
    Changed {
        path: Arc<Path>,
        maybe_is_file: bool,
        maybe_is_folder: bool,
    },
    Remove(Arc<Path>),
    Rename(Arc<Path>, Arc<Path>),
}

impl FilesystemEvent {
    pub fn change_or_remove_path(&self) -> Option<&Arc<Path>> {
        match self {
            FilesystemEvent::Changed { path, .. } => Some(path),
            FilesystemEvent::Remove(path) => Some(path),
            FilesystemEvent::Rename(..) => None,
        }
    }
}

struct AfterDebounceEffects {
    reload_mods: HashSet<InstanceID>,
}

impl BackendState {
    pub async fn handle_filesystem(&mut self, result: notify_debouncer_full::DebounceEventResult) {
        match result {
            Ok(events) => {
                let mut after_debounce_effects = AfterDebounceEffects {
                    reload_mods: HashSet::new(),
                };
                
                let mut last_event: Option<FilesystemEvent> = None;
                for event in events {
                    let Some(next_event) = get_simple_event(event.event) else {
                        continue;
                    };
                    
                    if let Some(last_event) = last_event.take() {
                        if last_event.change_or_remove_path() != next_event.change_or_remove_path() {
                            self.handle_filesystem_event(last_event, &mut after_debounce_effects).await;
                        }
                    }
                    
                    last_event = Some(next_event);
                }
                if let Some(last_event) = last_event.take() {
                    self.handle_filesystem_event(last_event, &mut after_debounce_effects).await;
                }
                for id in after_debounce_effects.reload_mods {
                    if let Some(instance) = self.instances.get_mut(id.index) {
                        if instance.id == id {
                            instance.start_load_mods(&self.notify_tick, &self.mod_metadata_manager);
                        }
                    }
                }
            },
            Err(_) => {
                eprintln!("An error occurred while watching the filesystem! The launcher might be out-of-sync with your files!");
                self.send.send_error("An error occurred while watching the filesystem! The launcher might be out-of-sync with your files!").await;
            },
        }
    }
    
    async fn handle_filesystem_event(&mut self, event: FilesystemEvent, after_debounce_effects: &mut AfterDebounceEffects) {
        match event {
            FilesystemEvent::Changed { path, maybe_is_file, maybe_is_folder } => {
                if let Some(watch_target) = self.watching.get(&path) {
                    match watch_target {
                        WatchTarget::ServersDat { id } => {
                            if let Some(instance) = self.instances.get_mut(id.index) {
                                if instance.id == *id {
                                    instance.mark_server_state_dirty();
                                }
                            }
                            return;
                        },
                        _ => {}
                    }
                }
                
                let Some(parent_path) = path.parent() else {
                    return;
                };
                
                let Some(parent_watch_target) = self.watching.get(parent_path) else {
                    return;
                };
                
                match parent_watch_target {
                    WatchTarget::InstancesDir => {
                        if maybe_is_folder {
                            let success = self.load_instance_from_path(&path, false, true).await;
                            if !success {
                                self.watch_filesystem(&path, WatchTarget::InvalidInstanceDir).await;
                            }
                        }
                    },
                    WatchTarget::InstanceDir { .. } | WatchTarget::InvalidInstanceDir => {
                        if maybe_is_file {
                            let Some(file_name) = path.file_name() else {
                                return;
                            };
                            if file_name == "info_v1.json" {
                                self.load_instance_from_path(parent_path, true, true).await;
                            }
                        }
                    },
                    WatchTarget::InstanceLevelDir { id } => {
                        if let Some(instance) = self.instances.get_mut(id.index) {
                            if instance.id == *id && instance.dirty_worlds.insert(parent_path.into()) {
                                instance.mark_world_state_dirty();
                            }
                        }
                    },
                    WatchTarget::InstanceSavesDir { id } => {
                        if let Some(instance) = self.instances.get_mut(id.index) {
                            if instance.id == *id && instance.dirty_worlds.insert(path.into()) {
                                instance.mark_world_state_dirty();
                            }
                        }
                    },
                    WatchTarget::ServersDat { .. } => {},
                    WatchTarget::InstanceModsDir { id } => {
                        if let Some(instance) = self.instances.get_mut(id.index) {
                            if instance.id == *id && instance.dirty_mods.insert(path.into()) {
                                instance.mark_mods_state_dirty();
                                if let Some(reload_immediately) = self.reload_mods_immediately.take(&instance.id) {
                                    after_debounce_effects.reload_mods.insert(reload_immediately);
                                }
                            }
                        }
                    }
                }
            },
            FilesystemEvent::Remove(path) => {
                if let Some(watch_target) = self.watching.remove(&path) {
                    match watch_target {
                        WatchTarget::InstancesDir => {
                            self.send.send_error("Instances folder has been removed! What?!").await;
                        },
                        WatchTarget::InstanceDir { id } => {
                            self.remove_instance(id).await;
                        },
                        WatchTarget::InvalidInstanceDir => {},
                        WatchTarget::InstanceLevelDir { id } => {
                            if let Some(instance) = self.instances.get_mut(id.index) {
                                if instance.id == id && instance.dirty_worlds.insert(path.into()) {
                                    instance.mark_world_state_dirty();
                                }
                            }
                        },
                        WatchTarget::InstanceSavesDir { id: _ } => {
                            // Saves dir deleted... umm...
                        },
                        WatchTarget::ServersDat { ref id } => {
                            if let Some(instance) = self.instances.get_mut(id.index) {
                                if instance.id == *id {
                                    instance.mark_server_state_dirty();
                                }
                            }
                            // Minecraft moves the servers.dat to servers.dat_old and then back,
                            // so lets just re-listen immediately
                            if self.watcher.watch(&path, notify::RecursiveMode::NonRecursive).is_ok() {
                                self.watching.insert(path, watch_target);
                            }
                        },
                        WatchTarget::InstanceModsDir { id } => {
                            // Mods dir deleted... umm...
                        },
                    }
                } else {
                    let Some(parent_path) = path.parent() else {
                        return;
                    };
                    
                    let Some(parent_watch_target) = self.watching.get(parent_path) else {
                        return;
                    };
                    
                    match parent_watch_target {
                        WatchTarget::InstanceDir { id } => {
                            let Some(file_name) = path.file_name() else {
                                return;
                            };
                            if file_name == "info_v1.json" {
                                self.remove_instance(*id).await;
                                self.watch_filesystem(parent_path, WatchTarget::InvalidInstanceDir).await;
                            }
                        },
                        WatchTarget::InstanceLevelDir { id } => {
                            if let Some(instance) = self.instances.get_mut(id.index) {
                                if instance.id == *id && instance.dirty_worlds.insert(parent_path.into()) {
                                    instance.mark_world_state_dirty();
                                }
                            }
                        },
                        WatchTarget::InstanceSavesDir { id } => {
                            if let Some(instance) = self.instances.get_mut(id.index) {
                                if instance.id == *id && instance.dirty_worlds.insert(path.clone()) {
                                    instance.mark_world_state_dirty();
                                }
                            }
                        },
                        WatchTarget::InstanceModsDir { id } => {
                            if let Some(instance) = self.instances.get_mut(id.index) {
                                if instance.id == *id && instance.dirty_mods.insert(path.into()) {
                                    instance.mark_mods_state_dirty();
                                    if let Some(reload_immediately) = self.reload_mods_immediately.take(&instance.id) {
                                        after_debounce_effects.reload_mods.insert(reload_immediately);
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            },
            FilesystemEvent::Rename(from, to) => {
                if let Some(watch_target) = self.watching.remove(&from) {
                    match watch_target {
                        WatchTarget::InstancesDir => {
                            self.send.send_error("Instances folder has been removed! What?!").await;
                        },
                        WatchTarget::InstanceDir { id } => {
                            if let Some(instance) = self.instances.get_mut(id.index) {
                                if instance.id == id {
                                    if !parent_is_instances_dir(&self.watching, &to) {
                                        self.remove_instance(id).await;
                                        return;
                                    }
                                    
                                    let old_name = instance.name.clone();
                                    instance.name = to.file_name().unwrap().to_string_lossy().into_owned().into();
                                    
                                    self.watching.insert(to, WatchTarget::InstanceDir { id });
                                    
                                    self.send.send_info(format!("Instance '{}' renamed to '{}'", old_name, instance.name)).await;
                                    self.send.send(instance.create_modify_message()).await;
                                }
                            }
                        },
                        WatchTarget::InvalidInstanceDir => {
                            if parent_is_instances_dir(&self.watching, &to) && !self.watching.contains_key(&to) {
                                self.watch_filesystem(&to, WatchTarget::InvalidInstanceDir).await;
                            }
                        },
                        WatchTarget::InstanceLevelDir { id } => {
                            if let Some(instance) = self.instances.get_mut(id.index) {
                                if instance.id == id {
                                    instance.dirty_worlds.insert(from.clone());
                                    if to.parent() == from.parent() {
                                        instance.dirty_worlds.insert(to.clone());
                                    }
                                    instance.mark_world_state_dirty();
                                }
                            }
                        },
                        WatchTarget::InstanceSavesDir { id: _ } => {
                            // Saves dir renamed... um...
                        },
                        WatchTarget::ServersDat { .. } => {},
                        WatchTarget::InstanceModsDir { id: _ } => {
                            // Mods dir renamed... um...
                        }
                    }
                } else {
                    if let Some(from_parent_path) = from.parent() && let Some(parent_watch_target) = self.watching.get(from_parent_path) {
                        match parent_watch_target {
                            WatchTarget::InstanceModsDir { id } => {
                                if let Some(instance) = self.instances.get_mut(id.index) {
                                    if instance.id == *id && instance.dirty_mods.insert(from.into()) {
                                        instance.mark_mods_state_dirty();
                                        if let Some(reload_immediately) = self.reload_mods_immediately.take(&instance.id) {
                                            after_debounce_effects.reload_mods.insert(reload_immediately);
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    if let Some(to_parent_path) = to.parent() && let Some(parent_watch_target) = self.watching.get(to_parent_path) {
                        match parent_watch_target {
                            WatchTarget::InstanceModsDir { id } => {
                                if let Some(instance) = self.instances.get_mut(id.index) {
                                    if instance.id == *id && instance.dirty_mods.insert(to.into()) {
                                        instance.mark_mods_state_dirty();
                                        if let Some(reload_immediately) = self.reload_mods_immediately.take(&instance.id) {
                                            after_debounce_effects.reload_mods.insert(reload_immediately);
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
                
            },
        }
    }
    
}

fn get_simple_event(event: notify::Event) -> Option<FilesystemEvent> {
    match event.kind {
        EventKind::Create(create_kind) => {
            if create_kind == CreateKind::Other {
                return None;
            }
            return Some(FilesystemEvent::Changed {
                path: event.paths[0].clone().into(),
                maybe_is_file: create_kind == CreateKind::File || create_kind == CreateKind::Any,
                maybe_is_folder: create_kind == CreateKind::Folder || create_kind == CreateKind::Any
            });
        },
        EventKind::Modify(modify_kind) => {
            match modify_kind {
                ModifyKind::Any => {
                    return Some(FilesystemEvent::Changed {
                        path: event.paths[0].clone().into(),
                        maybe_is_file: true,
                        maybe_is_folder: true,
                    });
                },
                ModifyKind::Data(data_change) => {
                    if data_change == DataChange::Any || data_change == DataChange::Content {
                        return Some(FilesystemEvent::Changed {
                            path: event.paths[0].clone().into(),
                            maybe_is_file: true,
                            maybe_is_folder: false,
                        });
                    } else {
                        return None;
                    }
                },
                ModifyKind::Metadata(_) => return None,
                ModifyKind::Name(rename_mode) => {
                    match rename_mode {
                        RenameMode::Any => return None,
                        RenameMode::To => {
                            return Some(FilesystemEvent::Changed {
                                path: event.paths[0].clone().into(),
                                maybe_is_file: true,
                                maybe_is_folder: true,
                            });
                        },
                        RenameMode::From => {
                            return Some(FilesystemEvent::Remove(event.paths[0].clone().into()));
                        },
                        RenameMode::Both => {
                            return Some(FilesystemEvent::Rename(event.paths[0].clone().into(), event.paths[1].clone().into()));
                        },
                        RenameMode::Other => return None,
                    }
                },
                ModifyKind::Other => return None,
            }
        },
        EventKind::Remove(remove_kind) => {
            if remove_kind == RemoveKind::Other {
                return None;
            }
    
            return Some(FilesystemEvent::Remove(event.paths[0].clone().into()));
        },
        EventKind::Any => return None,
        EventKind::Access(_) => return None,
        EventKind::Other => return None,
    };
}

fn parent_is_instances_dir(watching: &HashMap<Arc<Path>, WatchTarget>, path: &Path) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    if let Some(WatchTarget::InstancesDir) = watching.get(parent) {
        true
    } else {
        false
    }
}
