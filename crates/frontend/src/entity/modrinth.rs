use std::{collections::HashMap, error::Error, sync::{atomic::{AtomicBool, Ordering}, Arc}};

use bridge::{handle::BackendHandle, keep_alive::KeepAliveHandle, message::MessageToBackend};
use gpui::{App, AppContext, Entity};
use schema::{modrinth::{ModrinthError, ModrinthRequest, ModrinthResult}, version_manifest::MinecraftVersionManifest};

pub enum FrontendModrinthDataState {
    Loading,
    Loaded {
        result: Result<ModrinthResult, ModrinthError>,
        keep_alive: KeepAliveHandle,
    }
}

pub struct FrontendModrinthData {
    pub data: HashMap<ModrinthRequest, Entity<FrontendModrinthDataState>>,
    pub backend_handle: BackendHandle,
}

impl FrontendModrinthData {
    pub fn new(backend_handle: BackendHandle) -> Self {
        Self {
            data: HashMap::new(),
            backend_handle,
        }
    }

    pub fn request(entity: &Entity<Self>, request: ModrinthRequest, cx: &mut App) -> Entity<FrontendModrinthDataState> {
        entity.update(cx, |this, cx| {
            if let Some(existing) = this.data.get(&request) {
                if let FrontendModrinthDataState::Loaded { keep_alive, .. } = existing.read(cx) {
                    if keep_alive.is_alive() {
                        return existing.clone();
                    }
                } else {
                    return existing.clone();
                }
            }
            
            let loading = cx.new(|_| FrontendModrinthDataState::Loading);
            this.backend_handle.blocking_send(MessageToBackend::RequestModrinth {
                request: request.clone()
            });
            this.data.insert(request, loading.clone());
            loading
        })
    }
    
    pub fn set<C: AppContext>(entity: &Entity<Self>, request: ModrinthRequest, result: Result<ModrinthResult, ModrinthError>, keep_alive: KeepAliveHandle, cx: &mut C) {
        entity.update(cx, |this, cx| {
            this.data.get(&request).unwrap().update(cx, |value, cx| {
                *value = FrontendModrinthDataState::Loaded {
                    result,
                    keep_alive
                };
                cx.notify();
            });
        });
    }
}
