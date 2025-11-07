use std::{collections::HashMap, sync::Arc};

use bridge::{instance::{InstanceID, InstanceModSummary, InstanceServerSummary, InstanceStatus, InstanceWorldSummary}, message::AtomicBridgeDataLoadState};
use gpui::{App, AppContext, Context, Entity, EventEmitter, SharedString, Window};
use indexmap::IndexMap;
use schema::loader::Loader;

pub struct InstanceEntries {
    pub entries: IndexMap<InstanceID, Entity<InstanceEntry>>,
}

impl InstanceEntries {
    pub fn add<C: AppContext>(entity: &Entity<Self>, id: InstanceID, name: SharedString, version: SharedString, loader: Loader,
        worlds_state: Arc<AtomicBridgeDataLoadState>, servers_state: Arc<AtomicBridgeDataLoadState>, mods_state: Arc<AtomicBridgeDataLoadState>, cx: &mut C
    ) {
        entity.update(cx, |entries, cx| {
            let instance = InstanceEntry {
                id,
                name,
                version,
                loader,
                status: InstanceStatus::NotRunning,
                worlds_state,
                worlds: cx.new(|_| [].into()),
                servers_state,
                servers: cx.new(|_| [].into()),
                mods_state,
                mods: cx.new(|_| [].into()),
            };
            
            entries.entries.insert(id, cx.new(|_| instance.clone()));
            cx.emit(InstanceAddedEvent {
                instance
            });
        });
    }

    pub fn remove<C: AppContext>(entity: &Entity<Self>, id: InstanceID, cx: &mut C) {
        entity.update(cx, |entries, cx| {
            if let Some(_) = entries.entries.shift_remove(&id) {
                cx.emit(InstanceRemovedEvent { id });
            }
        });
    }
    
    pub fn modify<C: AppContext>(entity: &Entity<Self>, id: InstanceID, name: SharedString, version: SharedString, loader: Loader, status: InstanceStatus, cx: &mut C) {
        entity.update(cx, |entries, cx| {
            if let Some(instance) = entries.entries.get_mut(&id) {
                let cloned = instance.update(cx, |instance, cx| {
                    instance.name = name.clone();
                    instance.version = version.clone();
                    instance.loader = loader;
                    instance.status = status;
                    cx.notify();
                    
                    instance.clone()
                });
                
                cx.emit(InstanceModifiedEvent { 
                    instance: cloned
                });
            }
        });
    }

    pub fn set_worlds<C: AppContext>(entity: &Entity<Self>, id: InstanceID, worlds: Arc<[InstanceWorldSummary]>, cx: &mut C) {
        entity.update(cx, |entries, cx| {
             if let Some(instance) = entries.entries.get_mut(&id) {
                 instance.update(cx, |instance, cx| {
                     instance.worlds.update(cx, |existing_worlds, cx| {
                         *existing_worlds = worlds;
                         cx.notify();
                     })
                 });
             }
        });
    }
    
    pub fn set_servers<C: AppContext>(entity: &Entity<Self>, id: InstanceID, servers: Arc<[InstanceServerSummary]>, cx: &mut C) {
        entity.update(cx, |entries, cx| {
             if let Some(instance) = entries.entries.get_mut(&id) {
                 instance.update(cx, |instance, cx| {
                     instance.servers.update(cx, |existing_servers, cx| {
                         *existing_servers = servers;
                         cx.notify();
                     })
                 });
             }
        });
    }
    
    pub fn set_mods<C: AppContext>(entity: &Entity<Self>, id: InstanceID, mods: Arc<[InstanceModSummary]>, cx: &mut C) {
        entity.update(cx, |entries, cx| {
             if let Some(instance) = entries.entries.get_mut(&id) {
                 instance.update(cx, |instance, cx| {
                     instance.mods.update(cx, |existing_mods, cx| {
                         *existing_mods = mods;
                         cx.notify();
                     })
                 });
             }
        });
    }
}

#[derive(Clone)]
pub struct InstanceEntry {
    pub id: InstanceID,
    pub name: SharedString,
    pub version: SharedString,
    pub loader: Loader,
    pub status: InstanceStatus,
    pub worlds_state: Arc<AtomicBridgeDataLoadState>,
    pub worlds: Entity<Arc<[InstanceWorldSummary]>>,
    pub servers_state: Arc<AtomicBridgeDataLoadState>,
    pub servers: Entity<Arc<[InstanceServerSummary]>>,
    pub mods_state: Arc<AtomicBridgeDataLoadState>,
    pub mods: Entity<Arc<[InstanceModSummary]>>,
}

impl InstanceEntry {
    pub fn title(&self) -> String {
        if self.name == self.version {
            if self.loader == Loader::Vanilla {
                format!("{}", self.name)
            } else {
                format!("{} ({:?})", self.name, self.loader)
            }
        } else {
            if self.loader == Loader::Vanilla {
                format!("{} ({})", self.name, self.version)
            } else {
                format!("{} ({:?} {})", self.name, self.loader, self.version)
            }
        }
        
    }
}

impl EventEmitter<InstanceAddedEvent> for InstanceEntries {}

pub struct InstanceAddedEvent {
    pub instance: InstanceEntry
}

impl EventEmitter<InstanceModifiedEvent> for InstanceEntries {}

pub struct InstanceModifiedEvent {
    pub instance: InstanceEntry,
}

impl EventEmitter<InstanceRemovedEvent> for InstanceEntries {}

pub struct InstanceRemovedEvent {
    pub id: InstanceID,
}
