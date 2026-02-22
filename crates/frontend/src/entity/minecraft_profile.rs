use std::sync::Arc;

use bridge::message::{MinecraftCapeInfo, MinecraftProfileInfo, MinecraftSkinInfo};
use gpui::{App, Entity, EventEmitter};

#[derive(Default, Clone)]
pub struct MinecraftProfileEntries {
    pub profile: Option<MinecraftProfileInfo>,
}

#[derive(Clone)]
pub struct ProfileUpdated;

impl EventEmitter<ProfileUpdated> for MinecraftProfileEntries {}

impl MinecraftProfileEntries {
    pub fn set_profile(entity: &Entity<Self>, profile: MinecraftProfileInfo, cx: &mut App) {
        entity.update(cx, |entries, cx| {
            entries.profile = Some(profile);
            cx.notify();
        });
    }
}
