use std::{ops::Deref, sync::{atomic::{AtomicBool, AtomicU32, Ordering}, Arc, RwLock}, time::Instant};

use atomic_time::AtomicOptionInstant;

use crate::{handle::FrontendHandle, message::MessageToFrontend};

#[derive(Default, Clone, Debug)]
pub struct ModalAction {
    inner: Arc<ModalActionInner>,
}

impl Deref for ModalAction {
    type Target = ModalActionInner;

    fn deref(&self) -> &Self::Target {
        &*self.inner
    }
}

pub struct ModalActionVisitUrl {
    pub message: Arc<str>,
    pub url: Arc<str>,
}

#[derive(Default)]
pub struct ModalActionInner {
    pub finished_at: AtomicOptionInstant,
    pub error: RwLock<Option<Arc<str>>>,
    pub visit_url: RwLock<Option<ModalActionVisitUrl>>,
    pub trackers: ProgressTrackers,
}

impl ModalActionInner {
    pub fn set_finished(&self) {
        let _ = self.finished_at.compare_exchange(None, Some(Instant::now()), Ordering::SeqCst, Ordering::Relaxed);
    }
    
    pub fn get_finished_at(&self) -> Option<Instant> {
        self.finished_at.load(Ordering::SeqCst)
    }
    
    pub fn set_error_message(&self, error: Arc<str>) {
        *self.error.write().unwrap() = Some(error);
    }
    
    pub fn set_visit_url(&self, visit_url: ModalActionVisitUrl) {
        *self.visit_url.write().unwrap() = Some(visit_url);
    }
    
    pub fn unset_visit_url(&self) {
        *self.visit_url.write().unwrap() = None;
    }
}

impl std::fmt::Debug for ModalActionInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModalActionInner")
            .field("finished_at", &self.finished_at.load(Ordering::Relaxed))
            .field("error", &self.error)
            .field("trackers", &self.trackers)
            .finish()
    }
}

#[derive(Default, Clone, Debug)]
pub struct ProgressTrackers {
    pub trackers: Arc<RwLock<Vec<ProgressTracker>>>,
}

impl ProgressTrackers {
    pub fn push(&self, tracker: ProgressTracker) {
        self.trackers.write().unwrap().push(tracker);
    }
    
    pub fn clear(&self) {
        self.trackers.write().unwrap().clear();
    }
}

#[derive(Clone, Debug)]
pub struct ProgressTracker {
    inner: Arc<ProgressTrackerInner>,
    sender: FrontendHandle
}

struct ProgressTrackerInner {
    count: AtomicU32,
    total: AtomicU32,
    finished_at: AtomicOptionInstant,
    finished_with_error: AtomicBool,
    title: RwLock<Arc<str>>,
}

impl std::fmt::Debug for ProgressTrackerInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProgressTrackerInner")
            .field("count", &self.count)
            .field("total", &self.total)
            .field("finished_at", &self.finished_at.load(Ordering::Relaxed))
            .finish()
    }
}

impl ProgressTracker {
    pub fn new(title: Arc<str>, sender: FrontendHandle) -> Self {
        Self {
            inner: Arc::new(ProgressTrackerInner {
                count: AtomicU32::new(0),
                total: AtomicU32::new(0),
                finished_at: AtomicOptionInstant::none(),
                finished_with_error: AtomicBool::new(false),
                title: RwLock::new(title),
            }),
            sender
        }
    }

    pub fn get_title(&self) -> Arc<str> {
        self.inner.title.read().unwrap().clone()
    }

    pub fn set_title(&self, title: Arc<str>) {
        *self.inner.title.write().unwrap() = title;
    }

    pub fn get_float(&self) -> Option<f32> {
        let (count, total) = self.get();
        if total == 0 {
            None
        } else {
            Some((count as f32 / total as f32).clamp(0.0, 1.0))
        }
    }

    pub fn get(&self) -> (u32, u32) {
        (
            self.inner.count.load(Ordering::SeqCst),
            self.inner.total.load(Ordering::SeqCst)
        )
    }

    pub fn set_finished(&self, error: bool) {
        if error {
            self.inner.finished_with_error.store(true, Ordering::SeqCst);
        }
        let _ = self.inner.finished_at.compare_exchange(None, Some(Instant::now()), Ordering::SeqCst, Ordering::Relaxed);
    }

    pub fn get_finished_at(&self) -> Option<Instant> {
        self.inner.finished_at.load(Ordering::SeqCst)
    }

    pub fn is_error(&self) -> bool {
        self.inner.finished_with_error.load(Ordering::SeqCst)
    }

    pub fn add_count(&self, count: u32) {
        self.inner.count.fetch_add(count, Ordering::SeqCst);
    }

    pub fn set_count(&self, count: u32) {
        self.inner.count.store(count, Ordering::SeqCst);
    }

    pub fn add_total(&self, total: u32) {
        self.inner.total.fetch_add(total, Ordering::SeqCst);
    }

    pub fn set_total(&self, total: u32) {
        self.inner.total.store(total, Ordering::SeqCst);
    }

    pub async fn notify(&self) {
        let _ = self.sender.send(MessageToFrontend::Refresh).await;
    }
}
