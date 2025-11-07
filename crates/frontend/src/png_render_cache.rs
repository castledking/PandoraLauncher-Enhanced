use std::{rc::Rc, sync::{atomic::Ordering, Arc, Mutex}, time::{Duration, Instant}};

use atomic_time::AtomicInstant;
use gpui::{App, RenderImage, Window};
use image::Frame;
use intrusive_collections::{intrusive_adapter, LinkedList, LinkedListLink};
use rustc_hash::FxHashMap;

struct CacheEntry {
    link: LinkedListLink,
    ptrs: Mutex<Vec<usize>>,
    source: Arc<[u8]>,
    expiring: AtomicInstant,
    value: Option<Arc<RenderImage>>,
}

intrusive_adapter!(CacheEntryAdapter = Rc<CacheEntry>: CacheEntry { link: LinkedListLink });

#[derive(Default)]
struct PngRenderCache {
    by_ptr: FxHashMap<usize, Rc<CacheEntry>>,
    by_arc: FxHashMap<Arc<[u8]>, Rc<CacheEntry>>,
    expiring: LinkedList<CacheEntryAdapter>,
    submitted_cleanup: bool,
}

impl gpui::Global for PngRenderCache {}

const EXPIRY_SECONDS: u64 = 1;

pub fn render(image: Arc<[u8]>, cx: &mut App) -> gpui::Img {
    let cache = cx.default_global::<PngRenderCache>();
    
    let result = if let Some(result) = cache.get_or_create(image) {
        gpui::img(result)
    } else {
        gpui::img(gpui::ImageSource::Resource(gpui::Resource::Embedded("images/missing.png".into())))
    };
    
    if !cache.submitted_cleanup {
        cache.submitted_cleanup = true;
        cx.spawn(async |cx| {
            let _ = cx.update_global(|cache: &mut PngRenderCache, cx| {
                let now = Instant::now();
                let mut cursor = cache.expiring.front_mut();
                while let Some(entry) = cursor.get() {
                    if now > entry.expiring.load(Ordering::Relaxed) {
                        let entry = cursor.remove().expect("present");
                        for ptr in entry.ptrs.lock().unwrap().iter() {
                            cache.by_ptr.remove(ptr).expect("present");
                        }
                        cache.by_arc.remove(&entry.source).expect("present");
                        
                        debug_assert_eq!(Rc::strong_count(&entry), 1);
                        
                        if let Some(image) = &entry.value {
                            cx.drop_image(image.clone(), None);
                        }
                    } else {
                        break;
                    }
                }
                cache.submitted_cleanup = false;
            });
        }).detach();
    }
    
    result
}

impl PngRenderCache {
    fn get_or_create(&mut self, image: Arc<[u8]>) -> Option<Arc<RenderImage>> {
        let ptr = Arc::as_ptr(&image).addr();
        
        if let Some(result) = self.by_ptr.get(&ptr) {
            // Update expiry
            result.expiring.store(Instant::now() + Duration::from_secs(EXPIRY_SECONDS), Ordering::Relaxed);
            unsafe {
                self.expiring.cursor_mut_from_ptr(Rc::as_ptr(result)).remove();
            }
            self.expiring.push_back(result.clone());
            
            return result.value.clone();
        }
        
        if let Some(result) = self.by_arc.get(&image) {
            // Update expiry
            result.expiring.store(Instant::now() + Duration::from_secs(EXPIRY_SECONDS), Ordering::Relaxed);
            unsafe {
                self.expiring.cursor_mut_from_ptr(Rc::as_ptr(result)).remove();
            }
            self.expiring.push_back(result.clone());
            
            // Add ptr
            result.ptrs.lock().unwrap().push(ptr);
            self.by_ptr.insert(ptr, result.clone());
            
            return result.value.clone();
        }
        
        let result = image::load_from_memory_with_format(&*image, image::ImageFormat::Png).map(|data| {
            let mut data = data.into_rgba8();
            
            // Convert from RGBA to BGRA.
            for pixel in data.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
            
            RenderImage::new([Frame::new(data)])
        });
        
        let render_image = match result {
            Ok(render_image) => {
                Some(Arc::new(render_image))
            },
            Err(error) => {
                eprintln!("Error loading png: {error:?}");
                None
            },
        };
        
        let entry = Rc::new(CacheEntry {
            link: LinkedListLink::new(),
            ptrs: Mutex::new(vec![ptr]),
            source: Arc::clone(&image),
            expiring: AtomicInstant::new(Instant::now() + Duration::from_secs(EXPIRY_SECONDS)),
            value: render_image.clone(),
        });
        self.by_ptr.insert(ptr, entry.clone());
        self.by_arc.insert(Arc::clone(&image), entry.clone());
        self.expiring.push_back(entry);
        
        render_image
    }
}
