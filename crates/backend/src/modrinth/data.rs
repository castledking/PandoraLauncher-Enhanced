use std::{cell::RefCell, collections::{HashMap, VecDeque}, sync::{atomic::AtomicBool, Arc, Mutex}, time::{Duration, Instant}};

use bridge::{handle::FrontendHandle, keep_alive::{KeepAlive, KeepAliveHandle}, message::MessageToFrontend};
use chrono::{DateTime, Utc};
use reqwest::StatusCode;
use schema::modrinth::{ModrinthError, ModrinthRequest, ModrinthResult, ModrinthSearchRequest, ModrinthSearchResult};
use serde::{Deserialize, Serialize};
use tokio::task::JoinHandle;

const DATA_TTL: Duration = Duration::from_secs(5 * 60);

enum ModrinthEntry {
    Loading(Option<JoinHandle<LoadedModrinthResult>>),
    Loaded(LoadedModrinthResult),
}

struct LoadedModrinthResult {
    value: Result<ModrinthResult, ModrinthError>,
    expiry: Instant,
    _alive: KeepAlive,
}

pub struct ModrinthData {
    client: reqwest::Client,
    frontend_handle: FrontendHandle,
    search: HashMap<ModrinthRequest, ModrinthEntry>,
    expiring: VecDeque<ModrinthRequest>,
}

impl ModrinthData {
    pub fn new(client: reqwest::Client, frontend_handle: FrontendHandle) -> Self {
        Self {
            client,
            frontend_handle,
            search: HashMap::new(),
            expiring: VecDeque::new(),
        }
    }
    
    pub async fn expire(&mut self) {
        let now = Instant::now();
        
        while let Some(request) = self.expiring.front_mut() {
            let entry = self.search.get_mut(&request).unwrap();
            match &mut *entry {
                ModrinthEntry::Loading(join_handle) => {
                    if join_handle.as_ref().unwrap().is_finished() {
                        let result = join_handle.take().unwrap().await.unwrap();
                        *entry = ModrinthEntry::Loaded(result);
                        break;
                    } else {
                        return;
                    }
                },
                ModrinthEntry::Loaded(result) => {
                    if now > result.expiry {
                        self.search.remove(&request);
                        self.expiring.pop_front();
                        
                        continue;
                    }
                },
            }
            return;
        }
    }
    
    pub async fn frontend_request(&mut self, modrinth_request: ModrinthRequest) {
        if self.search.contains_key(&modrinth_request) {
            return;
        }
        
        match modrinth_request {
            ModrinthRequest::Search(ref modrinth_search_request) => {
                let request = self.client.get("https://api.modrinth.com/v2/search")
                    .query(modrinth_search_request)
                    .send();
                
                let frontend_handle = self.frontend_handle.clone();
                let request_copy = modrinth_request.clone();
                let future = tokio::task::spawn(async move {
                    let result = async move {
                        let response = request.await.map_err(|e| {
                            eprintln!("Error making request to modrinth: {:?}", e);
                            ModrinthError::ClientRequestError
                        })?;
                        
                        let status = response.status();
                        let bytes = response.bytes().await.map_err(|e| {
                            eprintln!("Error downloading response from modrinth: {:?}", e);
                            ModrinthError::ClientRequestError
                        })?;
                        
                        if status == StatusCode::OK {
                            serde_json::from_slice(&*bytes).map_err(|e| {
                                eprintln!("Error deserializing response from modrinth: {:?}", e);
                                ModrinthError::DeserializeError
                            }).map(ModrinthResult::Search)
                        } else if let Ok(error_response) = serde_json::from_slice(&*bytes) {
                            return Err(ModrinthError::ModrinthResponse(error_response));
                        } else {
                            return Err(ModrinthError::NonOK(status.as_u16()))
                        }
                    }.await;
                    
                    let keep_alive = KeepAlive::new();
                    frontend_handle.send(MessageToFrontend::ModrinthDataUpdated {
                        request: request_copy,
                        result: result.clone(),
                        alive_handle: keep_alive.create_handle(),
                    }).await;
                    LoadedModrinthResult {
                        value: result,
                        expiry: Instant::now() + DATA_TTL,
                        _alive: keep_alive,
                    }
                });
                let entry = ModrinthEntry::Loading(Some(future));
                self.search.insert(modrinth_request.clone(), entry);
                self.expiring.push_back(modrinth_request);
            },
        }
    }
}
