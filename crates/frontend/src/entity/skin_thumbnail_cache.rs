use std::sync::Arc;

use gpui::{App, Entity, EventEmitter, RenderImage};

#[derive(Default, Clone)]
pub struct SkinThumbnailCache {
    pub thumbnails: std::collections::HashMap<Arc<str>, (Arc<RenderImage>, Arc<RenderImage>)>,
}

#[derive(Clone)]
pub struct ThumbnailUpdated;

impl EventEmitter<ThumbnailUpdated> for SkinThumbnailCache {}

impl SkinThumbnailCache {
    pub fn insert(&mut self, url: Arc<str>, front: Arc<RenderImage>, back: Arc<RenderImage>) {
        self.thumbnails.insert(url, (front, back));
    }

    pub fn get(&self, url: &str) -> Option<(Arc<RenderImage>, Arc<RenderImage>)> {
        self.thumbnails.get(url).cloned()
    }

    pub fn contains(&self, url: &str) -> bool {
        self.thumbnails.contains_key(url)
    }
}
