use std::{io::Cursor, sync::Arc};

use auth::models::{MinecraftAccessToken, MinecraftProfileResponse};
use bridge::{account::Account, handle::BackendHandle, message::MessageToFrontend};
use image::imageops::FilterType;
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub struct MinecraftLoginInfo {
    pub uuid: Uuid,
    pub username: Arc<str>,
    pub access_token: MinecraftAccessToken,
}

#[derive(Default, Debug, Serialize, Deserialize)]
pub struct BackendAccountInfo {
    pub accounts: FxHashMap<Uuid, BackendAccount>,
    pub selected_account: Option<Uuid>,
}

impl BackendAccountInfo {
    pub fn create_update_message(&self) -> MessageToFrontend {
        let mut accounts = Vec::with_capacity(self.accounts.len());
        for (uuid, account) in &self.accounts {
            accounts.push(Account {
                uuid: *uuid,
                username: account.username.clone(),
                head: account.head_32x.clone(),
            });
        }
        accounts.sort_by(|a, b| {
            lexical_sort::natural_lexical_cmp(&a.username, &b.username)
        });
        MessageToFrontend::AccountsUpdated {
            accounts: accounts.into(),
            selected_account: self.selected_account,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BackendAccount {
    pub username: Arc<str>,
    pub head: Option<Arc<[u8]>>,
    
    #[serde(skip)]
    pub head_32x: Option<Arc<[u8]>>,
    #[serde(skip)]
    pub downloaded_head: bool,
}

impl BackendAccount {
    pub fn new_from_profile(profile: &MinecraftProfileResponse) -> Self {
        Self {
            username: profile.name.clone(),
            head_32x: None,
            head: None,
            downloaded_head: false,
        }
    }
    
    pub fn try_load_head_32x_from_head(&mut self) {
        let Some(head) = &self.head else {
            return;
        };
        let head = Arc::clone(head);
        
        let Ok(image) = image::load_from_memory(&*head) else {
            return;
        };
        
        if image.width() != 32 || image.height() != 32 {
            let resized = image.resize_exact(32, 32, FilterType::Nearest);
            
            let mut head_png = Vec::new();
            let mut cursor = Cursor::new(&mut head_png);
            if resized.write_to(&mut cursor, image::ImageFormat::Png).is_ok() {
                head_png.shrink_to_fit();
                self.head_32x = Some(head_png.into());
            } else {
                self.head_32x = Some(head);
            }
        } else {
            self.head_32x = Some(head);
        };
    }
}
