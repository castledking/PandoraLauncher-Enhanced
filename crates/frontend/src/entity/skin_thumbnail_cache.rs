use std::sync::Arc;

use gpui::{App, Entity, EventEmitter, RenderImage};

#[derive(Default, Clone)]
pub struct SkinThumbnailCache {
    pub thumbnails: std::collections::HashMap<Arc<str>, (Arc<RenderImage>, Arc<RenderImage>)>,
    pub cape_thumbnails: std::collections::HashMap<Arc<str>, (Arc<RenderImage>, Arc<RenderImage>)>,
    /// Capeless skin back view with same framing as cape thumbnails (for None card)
    pub none_card_thumbnails: std::collections::HashMap<Arc<str>, Arc<RenderImage>>,
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

    /// Cache key combines skin URL and cape URL so thumbnails refresh when skin changes.
    fn cape_key(skin_url: &str, cape_url: &str) -> Arc<str> {
        format!("{}\0{}", skin_url, cape_url).into()
    }

    pub fn insert_cape(
        &mut self,
        skin_url: Arc<str>,
        cape_url: Arc<str>,
        front: Arc<RenderImage>,
        back: Arc<RenderImage>,
    ) {
        let key = Self::cape_key(&skin_url, &cape_url);
        self.cape_thumbnails.insert(key, (front, back));
    }

    pub fn get_cape(
        &self,
        skin_url: &str,
        cape_url: &str,
    ) -> Option<(Arc<RenderImage>, Arc<RenderImage>)> {
        let key = Self::cape_key(skin_url, cape_url);
        self.cape_thumbnails.get(&key).cloned()
    }

    pub fn contains_cape(&self, skin_url: &str, cape_url: &str) -> bool {
        let key = Self::cape_key(skin_url, cape_url);
        self.cape_thumbnails.contains_key(&key)
    }

    pub fn insert_none_card(&mut self, skin_url: Arc<str>, back: Arc<RenderImage>) {
        self.none_card_thumbnails.insert(skin_url, back);
    }

    pub fn get_none_card(&self, skin_url: &str) -> Option<Arc<RenderImage>> {
        self.none_card_thumbnails.get(skin_url).cloned()
    }

    pub fn contains_none_card(&self, skin_url: &str) -> bool {
        self.none_card_thumbnails.contains_key(skin_url)
    }
}
