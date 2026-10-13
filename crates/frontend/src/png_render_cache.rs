use std::{
    cell::Cell,
    rc::Rc,
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};

use atomic_time::AtomicInstant;
use gpui::{AnyElement, App, Element, IntoElement, RenderImage, StyleRefinement, Styled, Window, prelude::*};
use gpui_component::StyledExt;
use image::{
    AnimationDecoder, DynamicImage, Frame,
    imageops::{self, FilterType},
};
use intrusive_collections::{LinkedList, LinkedListLink, intrusive_adapter};
use rustc_hash::FxHashMap;
use schema::unique_bytes::UniqueBytes;

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
pub enum ImageTransformation {
    None,
    Resize {
        width: u32,
        height: u32,
    },
    ResizeToWidth {
        width: u32,
    },
    CropAndScale {
        min_x: u32,
        min_y: u32,
        width: u32,
        height: u32,
        scale: u32,
    },
}

/// A decoded icon, still or moving. GIFs keep every frame so playback can step
/// through them; everything else is a single image.
#[derive(Clone)]
enum Decoded {
    Still(Arc<RenderImage>),
    Animated(Arc<Animation>),
}

impl Decoded {
    fn images(&self) -> std::slice::Iter<'_, Arc<RenderImage>> {
        match self {
            Decoded::Still(image) => std::slice::from_ref(image).iter(),
            Decoded::Animated(animation) => animation.frames.iter(),
        }
    }
}

struct Animation {
    frames: Vec<Arc<RenderImage>>,
    /// How long each frame is held, in the same order as `frames`.
    delays: Vec<Duration>,
    total: Duration,
    started: Cell<Instant>,
}

impl Animation {
    /// The frame that should be showing at `now`. Wraps, so a clip loops.
    fn frame_at(&self, now: Instant) -> usize {
        let total = self.total.as_nanos();
        if total == 0 {
            return 0;
        }

        let elapsed = now.saturating_duration_since(self.started.get()).as_nanos() % total;
        let mut taken: u128 = 0;
        for (index, delay) in self.delays.iter().enumerate() {
            taken += delay.as_nanos();
            if elapsed < taken {
                return index;
            }
        }
        0
    }
}

struct CacheEntry {
    link: LinkedListLink,
    source: UniqueBytes,
    transform: ImageTransformation,
    expiring: AtomicInstant,
    value: Option<Decoded>,
}

intrusive_adapter!(CacheEntryAdapter = Rc<CacheEntry>: CacheEntry { link: LinkedListLink });

#[derive(Default)]
struct PngRenderCache {
    map: FxHashMap<(UniqueBytes, ImageTransformation), Rc<CacheEntry>>,
    expiring: LinkedList<CacheEntryAdapter>,
    submitted_cleanup: bool,
}

impl gpui::Global for PngRenderCache {}

const EXPIRY_SECONDS: u64 = 1;

/// Every frame of a GIF is decoded and kept, so a file with thousands of frames
/// would cost its size multiplied by that. Anything past this plays as far as it
/// gets rather than risking the launcher.
const MAX_ANIMATION_FRAMES: usize = 200;

/// An icon the caller can style exactly like a `gpui::img`.
pub struct CachedImage {
    content: ImageContent,
    style: StyleRefinement,
    grayscale: bool,
}

enum ImageContent {
    Still(Arc<RenderImage>),
    Animated(Arc<Animation>),
    /// One of the launcher's own assets, left for gpui to load and cache.
    Embedded(gpui::SharedString),
}

impl CachedImage {
    /// A still image, styled and ready.
    pub fn still(image: Arc<RenderImage>) -> Self {
        Self {
            content: ImageContent::Still(image),
            style: StyleRefinement::default(),
            grayscale: false,
        }
    }

    /// An image that plays. Used for GIF icons.
    pub fn animated(animation: Arc<Animation>) -> Self {
        Self {
            content: ImageContent::Animated(animation),
            style: StyleRefinement::default(),
            grayscale: false,
        }
    }

    /// One of the launcher's own images, by its asset path.
    pub fn embedded(path: impl Into<gpui::SharedString>) -> Self {
        Self {
            content: ImageContent::Embedded(path.into()),
            style: StyleRefinement::default(),
            grayscale: false,
        }
    }

    /// Mirrors `gpui::img(..).grayscale(..)`.
    pub fn grayscale(mut self, grayscale: bool) -> Self {
        self.grayscale = grayscale;
        self
    }
}

impl Styled for CachedImage {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl IntoElement for CachedImage {
    type Element = gpui::Div;

    fn into_element(self) -> Self::Element {
        let grayscale = self.grayscale;
        let content = match self.content {
            ImageContent::Still(image) => gpui::img(image).size_full().into_any_element(),
            ImageContent::Animated(animation) => AnimatedImage { animation, grayscale }.into_any_element(),
            ImageContent::Embedded(path) => gpui::img(gpui::ImageSource::Resource(gpui::Resource::Embedded(path)))
                .size_full()
                .into_any_element(),
        };

        // The caller's styling goes on a box around the image, not on the image:
        // an animated one is repainted from scratch every frame.
        gpui::div().refine_style(&self.style).child(content)
    }
}

/// Paints whichever animation frame is due, and asks for the next one so the
/// clip keeps moving while it is on screen.
struct AnimatedImage {
    animation: Arc<Animation>,
    grayscale: bool,
}

impl IntoElement for AnimatedImage {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for AnimatedImage {
    type RequestLayoutState = gpui::ImgLayoutState;
    type PrepaintState = ();

    fn id(&self) -> Option<gpui::ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        id: Option<&gpui::GlobalElementId>,
        inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (gpui::LayoutId, Self::RequestLayoutState) {
        gpui::img(self.animation.frames[0].clone()).request_layout(id, inspector_id, window, cx)
    }

    fn prepaint(
        &mut self,
        _id: Option<&gpui::GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        _bounds: gpui::Bounds<gpui::Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
    }

    fn paint(
        &mut self,
        _id: Option<&gpui::GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: gpui::Bounds<gpui::Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        _cx: &mut App,
    ) {
        let frame = self.animation.frames[self.animation.frame_at(Instant::now())].clone();

        let viewport = window.viewport_size();
        if viewport.width > gpui::Pixels::ZERO && viewport.height > gpui::Pixels::ZERO {
            window.request_animation_frame();
        }

        _ = window.paint_image(bounds, bounds, gpui::Corners::default(), frame, 0, self.grayscale);
    }
}

/// The stand-in shown when an icon can't be decoded. Same type as a real icon,
/// so callers can mix it into an `if` without the arms disagreeing.
pub fn missing_image() -> CachedImage {
    CachedImage::embedded("images/missing.png")
}

/// An embedded asset such as `images/default_head.png`, so a caller can choose
/// between one of these and a cached icon without boxing the branch.
pub fn embedded_image(path: impl Into<gpui::SharedString>) -> CachedImage {
    CachedImage::embedded(path)
}

pub fn render(image: UniqueBytes, cx: &mut App) -> CachedImage {
    render_with_transform(image, ImageTransformation::None, cx)
}

pub fn render_with_transform(image: UniqueBytes, transform: ImageTransformation, cx: &mut App) -> CachedImage {
    let cache = cx.default_global::<PngRenderCache>();

    let result = match cache.get_or_create(image, transform) {
        Some(Decoded::Still(image)) => CachedImage::still(image),
        Some(Decoded::Animated(animation)) => CachedImage::animated(animation),
        None => missing_image(),
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
                        cache.map.remove(&(entry.source.clone(), entry.transform)).expect("present");

                        debug_assert_eq!(Rc::strong_count(&entry), 1);

                        if let Some(decoded) = &entry.value {
                            for image in decoded.images() {
                                cx.drop_image(image.clone(), None);
                            }
                        }
                    } else {
                        break;
                    }
                }
                cache.submitted_cleanup = false;
            });
        })
        .detach();
    }

    result
}

impl PngRenderCache {
    fn get_or_create(&mut self, image: UniqueBytes, transform: ImageTransformation) -> Option<Decoded> {
        let key = (image, transform);

        if let Some(result) = self.map.get(&key) {
            result
                .expiring
                .store(Instant::now() + Duration::from_secs(EXPIRY_SECONDS), Ordering::Relaxed);
            unsafe {
                self.expiring.cursor_mut_from_ptr(Rc::as_ptr(result)).remove();
            }
            self.expiring.push_back(result.clone());

            return result.value.clone();
        }

        let decoded = decode(&key.0, transform);

        let entry = Rc::new(CacheEntry {
            link: LinkedListLink::new(),
            source: key.0.clone(),
            transform,
            expiring: AtomicInstant::new(Instant::now() + Duration::from_secs(EXPIRY_SECONDS)),
            value: decoded.clone(),
        });
        self.map.insert(key, entry.clone());
        self.expiring.push_back(entry);

        decoded
    }
}

/// Applies the caller's transform to an already-decoded frame.
fn transform_frame(image: DynamicImage, transform: ImageTransformation) -> DynamicImage {
    match transform {
        ImageTransformation::None => image,
        ImageTransformation::Resize { width, height } => {
            if image.width() != width || image.height() != height {
                let filter = if image.width() > width || image.height() > height {
                    FilterType::Lanczos3
                } else {
                    FilterType::Nearest
                };
                image.resize(width, height, filter)
            } else {
                image
            }
        },
        ImageTransformation::ResizeToWidth { width } => {
            let ratio = width as f64 / image.width() as f64;
            let height = ((image.height() as f64 * ratio).round() as u32).max(1);
            if image.width() != width || image.height() != height {
                let filter = if image.width() > width || image.height() > height {
                    FilterType::Lanczos3
                } else {
                    FilterType::Nearest
                };
                image.resize_exact(width, height, filter)
            } else {
                image
            }
        },
        ImageTransformation::CropAndScale {
            min_x,
            min_y,
            width,
            height,
            scale,
        } => DynamicImage::ImageRgba8(imageops::crop_imm(&image, min_x, min_y, width, height).to_image()).resize_exact(
            width * scale,
            height * scale,
            FilterType::Nearest,
        ),
    }
}

/// gpui wants BGRA and the image crate hands over RGBA.
fn to_render_image(image: DynamicImage) -> Arc<RenderImage> {
    let mut buffer = image.into_rgba8();
    for pixel in buffer.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Arc::new(RenderImage::new([Frame::new(buffer)]))
}

/// Decodes an icon. PNG is the usual case; a GIF keeps its frames so it can
/// play. Anything else is left to the caller to report.
fn decode(bytes: &[u8], transform: ImageTransformation) -> Option<Decoded> {
    let format = match image::guess_format(bytes) {
        Ok(format) => format,
        Err(error) => {
            log::warn!("Unable to guess icon format: {error:?}");
            return None;
        },
    };

    match format {
        image::ImageFormat::Png => {
            let image = match image::load_from_memory_with_format(bytes, image::ImageFormat::Png) {
                Ok(image) => image,
                Err(error) => {
                    log::warn!("Error loading png: {error:?}");
                    return None;
                },
            };
            Some(Decoded::Still(to_render_image(transform_frame(image, transform))))
        },
        image::ImageFormat::Gif => decode_gif(bytes, transform),
        format => {
            log::warn!("Unsupported icon format: {format:?}");
            None
        },
    }
}

fn decode_gif(bytes: &[u8], transform: ImageTransformation) -> Option<Decoded> {
    let decoder = match image::codecs::gif::GifDecoder::new(std::io::Cursor::new(bytes)) {
        Ok(decoder) => decoder,
        Err(error) => {
            log::warn!("Error opening gif: {error:?}");
            return None;
        },
    };

    let mut frames = Vec::new();
    let mut delays = Vec::new();
    for frame in decoder.into_frames() {
        if frames.len() >= MAX_ANIMATION_FRAMES {
            log::warn!("Gif had more than {MAX_ANIMATION_FRAMES} frames, playing what fits");
            break;
        }

        let frame = match frame {
            Ok(frame) => frame,
            Err(error) => {
                log::warn!("Error reading gif frame: {error:?}");
                return None;
            },
        };

        // Plenty of GIFs say nothing about their own timing; viewers fall back
        // to a tenth of a second in that case, so this does too.
        let (numer, denom) = frame.delay().numer_denom_ms();
        let delay = if denom == 0 {
            Duration::from_millis(100)
        } else {
            Duration::from_millis(u64::from(numer / denom).max(1))
        };

        frames.push(to_render_image(transform_frame(DynamicImage::ImageRgba8(frame.into_buffer()), transform)));
        delays.push(delay);
    }

    if frames.is_empty() {
        log::warn!("Gif held no frames");
        return None;
    }

    // A single-frame GIF is just a still image wearing a costume.
    if frames.len() == 1 {
        return Some(Decoded::Still(frames.remove(0)));
    }

    let total = delays.iter().sum();
    Some(Decoded::Animated(Arc::new(Animation {
        frames,
        delays,
        total,
        started: Cell::new(Instant::now()),
    })))
}
