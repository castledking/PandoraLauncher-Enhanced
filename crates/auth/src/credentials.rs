use std::sync::Arc;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::models::{MinecraftAccessToken, TokenWithExpiry, XstsToken};

#[derive(Default, Deserialize, Serialize)]
pub struct AccountCredentials {
    pub msa_refresh: Option<Arc<str>>,
    pub msa_access: Option<TokenWithExpiry>,
    pub xbl: Option<TokenWithExpiry>,
    pub xsts: Option<XstsToken>,
    pub access_token: Option<TokenWithExpiry>,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[repr(u8)]
pub enum AuthStage {
    Initial,
    MsaRefresh,
    MsaAccess,
    XboxLive,
    XboxSecure,
    AccessToken,
}
pub const AUTH_STAGE_COUNT: u8 = 6;

pub enum AuthStageWithData {
    Initial,
    MsaRefresh(Arc<str>),
    MsaAccess(Arc<str>),
    XboxLive(Arc<str>),
    XboxSecure {
        xsts: Arc<str>,
        userhash: Arc<str>,
    },
    AccessToken(MinecraftAccessToken),
}

impl AuthStageWithData {
    pub fn stage(&self) -> AuthStage {
        match self {
            AuthStageWithData::Initial => AuthStage::Initial,
            AuthStageWithData::MsaRefresh(..) => AuthStage::MsaRefresh,
            AuthStageWithData::MsaAccess(..) => AuthStage::MsaAccess,
            AuthStageWithData::XboxLive(..) => AuthStage::XboxLive,
            AuthStageWithData::XboxSecure { .. } => AuthStage::XboxSecure,
            AuthStageWithData::AccessToken(..) => AuthStage::AccessToken,
        }
    }
}

impl AccountCredentials {
    pub fn stage(&mut self) -> AuthStageWithData {
        let now = Utc::now();
        
        // Try returning access token
        if let Some(access_token) = &self.access_token && now < access_token.expiry {
            return AuthStageWithData::AccessToken(MinecraftAccessToken(Arc::clone(&access_token.token)));
        }
        self.access_token = None;
        
        // Try returning XboxSecure
        if let Some(xsts) = &self.xsts && now < xsts.expiry {
            return AuthStageWithData::XboxSecure {
                xsts: Arc::clone(&xsts.token),
                userhash: Arc::clone(&xsts.userhash),
            };
        }
        self.xsts = None;
        
        // Try returning XboxLive
        if let Some(xbl) = &self.xbl && now < xbl.expiry {
            return AuthStageWithData::XboxLive(Arc::clone(&xbl.token));
        }
        self.xbl = None;
        
        // Try returning MsaAccess
        if let Some(msa_access) = &self.msa_access && now < msa_access.expiry {
            return AuthStageWithData::MsaAccess(Arc::clone(&msa_access.token));
        }
        self.msa_access = None;
        
        // Try returning MsaRefresh
        if let Some(msa_refresh) = &self.msa_refresh {
            return AuthStageWithData::MsaRefresh(Arc::clone(msa_refresh));
        }
        
        // No valid stage, return initial stage
        return AuthStageWithData::Initial;
    }
}
