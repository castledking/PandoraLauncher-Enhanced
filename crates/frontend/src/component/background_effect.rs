//! The animated backgrounds behind the quickplay page, and the snow that can fall over the whole
//! launcher.
//!
//! Every effect is a pure function of time: particle `i` is wherever its hashed speed and phase put
//! it at `time`, and nothing is simulated from one frame to the next. Pausing an effect is just not
//! advancing its clock, and the settings window can draw a still of each one for its picker.

use std::{
    cell::Cell,
    f32::consts::{PI, TAU},
    rc::Rc,
    time::{Duration, Instant},
};

use gpui::{prelude::*, *};
use gpui_component::{Colorize, Theme};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, strum::EnumIter)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundEffect {
    #[default]
    MatrixRain,
    EnchantmentTable,
    NebulaWaves,
    NebulaParticles,
    NebulaGrid,
    NebulaVoxels,
    NebulaLightning,
    LiquidChrome,
    RetroGrid,
    PlainColor,
}

impl BackgroundEffect {
    pub fn name(self) -> &'static str {
        use t::settings::appearance::background::effects;
        match self {
            BackgroundEffect::MatrixRain => effects::matrix_rain(),
            BackgroundEffect::EnchantmentTable => effects::enchantment_table(),
            BackgroundEffect::NebulaWaves => effects::nebula_waves(),
            BackgroundEffect::NebulaParticles => effects::nebula_particles(),
            BackgroundEffect::NebulaGrid => effects::nebula_grid(),
            BackgroundEffect::NebulaVoxels => effects::nebula_voxels(),
            BackgroundEffect::NebulaLightning => effects::nebula_lightning(),
            BackgroundEffect::LiquidChrome => effects::liquid_chrome(),
            BackgroundEffect::RetroGrid => effects::retro_grid(),
            BackgroundEffect::PlainColor => effects::plain_color(),
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            BackgroundEffect::MatrixRain => "icons/code-xml.svg",
            BackgroundEffect::EnchantmentTable => "icons/wand-sparkles.svg",
            BackgroundEffect::NebulaWaves => "icons/waypoints.svg",
            BackgroundEffect::NebulaParticles => "icons/star.svg",
            BackgroundEffect::NebulaGrid => "icons/grid-2x2.svg",
            BackgroundEffect::NebulaVoxels => "icons/box.svg",
            BackgroundEffect::NebulaLightning => "icons/zap.svg",
            BackgroundEffect::LiquidChrome => "icons/diamond.svg",
            BackgroundEffect::RetroGrid => "icons/layout-dashboard.svg",
            BackgroundEffect::PlainColor => "icons/palette.svg",
        }
    }

    /// Whether there is anything to animate. A plain colour never needs another frame.
    pub fn moves(self) -> bool {
        self != BackgroundEffect::PlainColor
    }

    /// A moment that shows the effect off in a still, for the settings picker.
    pub fn preview_time(self) -> f64 {
        const DEFAULT: f64 = 9.0;
        match self {
            // Most of the time there's no strike, so find the first bright one
            BackgroundEffect::NebulaLightning => (0..2000)
                .map(|step| DEFAULT + step as f64 * 0.02)
                .find(|time| lightning_strike(0, *time as f32).is_some_and(|(_, intensity)| intensity > 0.8))
                .unwrap_or(DEFAULT),
            _ => DEFAULT,
        }
    }
}

/// Trades how busy an effect is, and how often it redraws, against CPU time.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, strum::EnumIter)]
#[serde(rename_all = "lowercase")]
pub enum EffectQuality {
    Low,
    #[default]
    Medium,
    High,
}

impl EffectQuality {
    pub fn fps(self) -> f32 {
        match self {
            EffectQuality::Low => 24.0,
            EffectQuality::Medium => 40.0,
            EffectQuality::High => 60.0,
        }
    }

    /// Scales how many particles, columns and shapes an effect draws.
    pub fn density(self) -> f32 {
        match self {
            EffectQuality::Low => 0.45,
            EffectQuality::Medium => 0.7,
            EffectQuality::High => 1.0,
        }
    }

    pub fn from_index(index: usize) -> Self {
        match index {
            0 => EffectQuality::Low,
            1 => EffectQuality::Medium,
            _ => EffectQuality::High,
        }
    }

    pub fn index(self) -> usize {
        match self {
            EffectQuality::Low => 0,
            EffectQuality::Medium => 1,
            EffectQuality::High => 2,
        }
    }
}

/// The colours effects are drawn in, taken from the theme so they sit with the rest of the UI.
#[derive(Clone, Copy, Debug)]
pub struct EffectPalette {
    pub base_top: Hsla,
    pub base_bottom: Hsla,
    pub accent: Hsla,
    pub highlight: Hsla,
}

impl EffectPalette {
    pub fn from_theme(theme: &Theme) -> Self {
        let accent = theme.blue;
        let background = theme.background;
        Self {
            base_top: background.mix_oklab(accent, 0.9),
            base_bottom: background.darken(0.3),
            accent,
            highlight: accent.mix_oklab(gpui::white(), 0.45),
        }
    }

    /// The accent turned around the colour wheel, for effects that want a second colour.
    fn shifted(&self, turns: f32) -> Hsla {
        Hsla {
            h: (self.accent.h + turns).rem_euclid(1.0),
            ..self.accent
        }
    }
}

/// Time for an animation that only moves while it is meant to.
#[derive(Debug, Default)]
pub struct AnimationClock {
    elapsed: f64,
    last_tick: Option<Instant>,
}

impl AnimationClock {
    /// Longest step a single tick takes, so a stalled frame doesn't make everything jump.
    const MAX_STEP: f64 = 0.25;

    pub fn starting_at(elapsed: f64) -> Self {
        Self {
            elapsed,
            last_tick: None,
        }
    }

    /// Advances by the time since the last running tick. A paused clock resumes from where it
    /// stopped rather than skipping ahead by however long it was paused.
    pub fn tick(&mut self, running: bool) -> f64 {
        let now = Instant::now();
        if running {
            if let Some(last_tick) = self.last_tick {
                self.elapsed += (now - last_tick).as_secs_f64().min(Self::MAX_STEP);
            }
            self.last_tick = Some(now);
        } else {
            self.last_tick = None;
        }
        self.elapsed
    }

    pub fn elapsed(&self) -> f64 {
        self.elapsed
    }
}

/// Re-renders a view at a capped frame rate, for as long as its render keeps asking.
///
/// Asking from render means a view that stops being drawn, such as a page that has been
/// navigated away from, stops ticking on its own.
#[derive(Default)]
pub struct FrameTicker {
    pending: Rc<Cell<bool>>,
    task: Option<Task<()>>,
}

impl FrameTicker {
    pub fn request<T: 'static>(&mut self, fps: f32, cx: &mut Context<T>) {
        if self.pending.get() {
            return;
        }
        self.pending.set(true);
        let pending = self.pending.clone();
        let interval = Duration::from_secs_f32(1.0 / fps.max(1.0));
        self.task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(interval).await;
            pending.set(false);
            _ = this.update(cx, |_, cx| cx.notify());
        }));
    }
}

/// An element that fills its bounds with `effect` as it looks at `time` seconds.
///
/// `paint_base` draws the effect's own backdrop; leave it off to draw over a custom image.
pub fn effect_canvas(
    effect: BackgroundEffect,
    time: f64,
    quality: EffectQuality,
    palette: EffectPalette,
    paint_base: bool,
) -> Canvas<()> {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, cx| {
            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                if paint_base {
                    window.paint_quad(fill(
                        bounds,
                        linear_gradient(
                            180.0,
                            linear_color_stop(palette.base_top, 0.0),
                            linear_color_stop(palette.base_bottom, 1.0),
                        ),
                    ));
                }
                let mut painter = Painter::new(bounds, time, quality.density(), palette, window, cx);
                match effect {
                    BackgroundEffect::MatrixRain => painter.matrix_rain(),
                    BackgroundEffect::EnchantmentTable => painter.enchantment_table(),
                    BackgroundEffect::NebulaWaves => painter.nebula_waves(),
                    BackgroundEffect::NebulaParticles => painter.nebula_particles(),
                    BackgroundEffect::NebulaGrid => painter.nebula_grid(),
                    BackgroundEffect::NebulaVoxels => painter.nebula_voxels(),
                    BackgroundEffect::NebulaLightning => painter.nebula_lightning(),
                    BackgroundEffect::LiquidChrome => painter.liquid_chrome(),
                    BackgroundEffect::RetroGrid => painter.retro_grid(),
                    BackgroundEffect::PlainColor => {},
                }
            });
        },
    )
}

/// Snow falling across the given bounds at `time` seconds. Draws nothing else, so it can sit over
/// any content.
pub fn snow_canvas(time: f64, quality: EffectQuality) -> Canvas<()> {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, cx| {
            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                let palette = EffectPalette {
                    base_top: gpui::transparent_black(),
                    base_bottom: gpui::transparent_black(),
                    accent: gpui::white(),
                    highlight: gpui::white(),
                };
                Painter::new(bounds, time, quality.density(), palette, window, cx).snow();
            });
        },
    )
}

/// Snow over the whole launcher. Unlike the background it keeps falling while the launcher isn't
/// the focused window.
#[derive(Default)]
pub struct SnowOverlay {
    clock: AnimationClock,
    ticker: FrameTicker,
}

impl Render for SnowOverlay {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let quality = crate::interface_config::InterfaceConfig::get(cx).background_quality;
        let time = self.clock.tick(true);
        self.ticker.request(quality.fps(), cx);
        // Pinned, since an absolute element without insets sits wherever it would have flowed to
        snow_canvas(time, quality).absolute().top_0().left_0().size_full()
    }
}

/// A uniformly distributed value in `0.0..1.0`, the same every time for the same inputs.
pub fn hash(a: u32, b: u32) -> f32 {
    let mut x = a.wrapping_mul(0x9E37_79B1) ^ b.wrapping_mul(0x85EB_CA77).rotate_left(13) ^ 0x2545_F491;
    x ^= x >> 16;
    x = x.wrapping_mul(0x7FEB_352D);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846C_A68B);
    x ^= x >> 16;
    (x >> 8) as f32 / (1u32 << 24) as f32
}

fn hash3(a: u32, b: u32, c: u32) -> f32 {
    hash(a, b.wrapping_mul(0x27D4_EB2F) ^ c.wrapping_mul(0x1656_67B1))
}

fn pick<T: Copy>(items: &[T], value: f32) -> T {
    items[((value * items.len() as f32) as usize).min(items.len() - 1)]
}

const LIGHTNING_SLOTS: u32 = 3;

/// Whether lightning slot `slot` is striking at `time`, and if so which strike it is and how bright.
fn lightning_strike(slot: u32, time: f32) -> Option<(u32, f32)> {
    let period = 2.6 + hash(slot, 1) * 3.2;
    let shifted = time + hash(slot, 2) * period;
    let cycle = (shifted / period).floor() as u32;
    let local = shifted - cycle as f32 * period;
    let duration = 0.45;
    if local > duration || hash3(slot, cycle, 3) < 0.3 {
        return None;
    }
    Some((cycle, (1.0 - local / duration) * (0.65 + 0.35 * (local * 55.0).sin().abs())))
}

const MATRIX_GLYPHS: &[&str] = &[
    "0", "1", "2", "3", "4", "5", "7", "8", "9", "A", "B", "C", "E", "F", "K", "M", "N", "R", "T", "X", "Z", ":", "=",
    "+", "*", "<", ">", "ｱ", "ｲ", "ｳ", "ｴ", "ｵ", "ｶ", "ｷ", "ｸ", "ｹ", "ｺ", "ｻ", "ｼ", "ｽ", "ｾ", "ｿ", "ﾀ", "ﾁ", "ﾂ", "ﾃ",
    "ﾄ", "ﾅ", "ﾆ", "ﾇ", "ﾈ", "ﾊ", "ﾋ", "ﾌ", "ﾍ", "ﾎ", "ﾏ", "ﾐ", "ﾑ", "ﾒ", "ﾓ", "ﾔ", "ﾕ", "ﾖ", "ﾗ", "ﾘ", "ﾜ",
];

/// Lookalikes for the Standard Galactic Alphabet the enchanting table writes in.
const RUNE_GLYPHS: &[&str] = &[
    "ᔑ", "ʖ", "ᓵ", "↸", "ᒷ", "⎓", "⊣", "⍑", "╎", "⋮", "ꖌ", "ꖎ", "ᒲ", "リ", "ᑑ", "∷", "ᓭ", "ℸ", "⚍", "⍊", "∴", "⨅",
];

struct Painter<'a> {
    bounds: Bounds<Pixels>,
    w: f32,
    h: f32,
    t: f32,
    density: f32,
    palette: EffectPalette,
    window: &'a mut Window,
    cx: &'a mut App,
}

impl<'a> Painter<'a> {
    fn new(
        bounds: Bounds<Pixels>,
        time: f64,
        density: f32,
        palette: EffectPalette,
        window: &'a mut Window,
        cx: &'a mut App,
    ) -> Self {
        Self {
            bounds,
            w: bounds.size.width.as_f32(),
            h: bounds.size.height.as_f32(),
            // Wrapped so very long sessions don't lose precision; nothing repeats over that span
            t: (time % 100_000.0) as f32,
            density,
            palette,
            window,
            cx,
        }
    }

    fn at(&self, x: f32, y: f32) -> Point<Pixels> {
        point(self.bounds.origin.x + px(x), self.bounds.origin.y + px(y))
    }

    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, background: impl Into<Background>) {
        let bounds = Bounds::new(self.at(x, y), size(px(w), px(h)));
        self.window.paint_quad(fill(bounds, background));
    }

    fn dot(&mut self, x: f32, y: f32, radius: f32, color: Hsla) {
        let bounds = Bounds::new(self.at(x - radius, y - radius), size(px(radius * 2.0), px(radius * 2.0)));
        self.window.paint_quad(fill(bounds, color).corner_radii(px(radius)));
    }

    /// A soft round glow, drawn as nothing but the blurred shadow of a circle.
    fn glow(&mut self, x: f32, y: f32, radius: f32, blur: f32, color: Hsla) {
        let bounds = Bounds::new(self.at(x - radius, y - radius), size(px(radius * 2.0), px(radius * 2.0)));
        self.window.paint_drop_shadows(
            bounds,
            Corners::all(px(radius)),
            &[BoxShadow {
                color,
                offset: point(px(0.0), px(0.0)),
                blur_radius: px(blur),
                spread_radius: px(0.0),
                inset: false,
            }],
        );
    }

    fn polyline(&mut self, points: &[(f32, f32)], width: f32, color: impl Into<Background>) {
        let Some(&(x, y)) = points.first() else {
            return;
        };
        let mut builder = PathBuilder::stroke(px(width));
        builder.move_to(self.at(x, y));
        for &(x, y) in &points[1..] {
            builder.line_to(self.at(x, y));
        }
        if let Ok(path) = builder.build() {
            self.window.paint_path(path, color);
        }
    }

    /// Many separate straight lines in one style, drawn as a single path.
    fn segments(&mut self, lines: &[((f32, f32), (f32, f32))], width: f32, color: impl Into<Background>) {
        if lines.is_empty() {
            return;
        }
        let mut builder = PathBuilder::stroke(px(width));
        for &((x1, y1), (x2, y2)) in lines {
            builder.move_to(self.at(x1, y1));
            builder.line_to(self.at(x2, y2));
        }
        if let Ok(path) = builder.build() {
            self.window.paint_path(path, color);
        }
    }

    fn polygon(&mut self, points: &[(f32, f32)], color: impl Into<Background>) {
        let Some(&(x, y)) = points.first() else {
            return;
        };
        let mut builder = PathBuilder::fill();
        builder.move_to(self.at(x, y));
        for &(x, y) in &points[1..] {
            builder.line_to(self.at(x, y));
        }
        builder.close();
        if let Ok(path) = builder.build() {
            self.window.paint_path(path, color);
        }
    }

    fn glyph(&mut self, text: &'static str, x: f32, y: f32, font_size: f32, font: &Font, color: Hsla) {
        let run = TextRun {
            len: text.len(),
            font: font.clone(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let line = self
            .window
            .text_system()
            .shape_line(SharedString::new_static(text), px(font_size), &[run], None);
        _ = line.paint(self.at(x, y), px(font_size * 1.2), TextAlign::Left, None, self.window, self.cx);
    }

    fn count(&self, full: f32) -> u32 {
        (full * self.density).round().max(1.0) as u32
    }

    fn matrix_rain(&mut self) {
        let font = font(gpui_component::ActiveTheme::theme(self.cx).mono_font_family.clone());
        let font_size = 15.0;
        let cell = 18.0;
        let columns = (self.w / cell).ceil() as u32 + 1;
        let rows = (self.h / cell).ceil() as i32 + 1;
        let streams = if self.density < 0.5 { 1 } else { 2 };
        let accent = self.palette.accent;
        let head = self.palette.highlight.mix(gpui::white(), 0.6);

        for column in 0..columns {
            if hash(column, 1) > 0.3 + 0.7 * self.density {
                continue;
            }
            for stream in 0..streams {
                let seed = column * 7 + stream;
                let speed = 4.0 + hash(seed, 2) * 9.0;
                let length = 6 + (hash(seed, 3) * 18.0) as i32;
                let cycle = (rows + length) as f32 * (1.0 + hash(seed, 4) * 0.8);
                let head_row = ((self.t * speed + hash(seed, 5) * cycle) % cycle) as i32;

                for k in 0..length {
                    let row = head_row - k;
                    if row < 0 || row >= rows {
                        continue;
                    }
                    let flicker = (self.t * (0.8 + hash3(column, row as u32, 6) * 2.5)) as u32;
                    let text = pick(MATRIX_GLYPHS, hash3(column, row as u32, flicker));
                    let color = if k == 0 {
                        head.opacity(0.95)
                    } else {
                        let fade = 1.0 - k as f32 / length as f32;
                        accent.opacity(fade.powf(1.6) * 0.8)
                    };
                    self.glyph(text, column as f32 * cell + 2.0, row as f32 * cell, font_size, &font, color);
                }
            }
        }
    }

    fn enchantment_table(&mut self) {
        let font = font(gpui_component::ActiveTheme::theme(self.cx).font_family.clone());
        let violet = self.palette.shifted(0.12).mix(gpui::white(), 0.55);
        let highlight = self.palette.highlight;

        // A faint glow low in the middle, where the table would be
        let (w, h) = (self.w, self.h);
        self.glow(w * 0.5, h * 0.82, w.min(h) * 0.12, w.min(h) * 0.35, self.palette.accent.opacity(0.18));

        for i in 0..self.count(150.0) {
            let period = 5.0 + hash(i, 1) * 6.0;
            let shifted = self.t + hash(i, 2) * period;
            let cycle = (shifted / period).floor() as u32;
            let progress = shifted / period - cycle as f32;

            let start_x = hash3(i, cycle, 3) * self.w;
            let start_y = self.h * (0.2 + 0.9 * hash3(i, cycle, 4));
            let rise = 60.0 + hash3(i, cycle, 5) * 110.0;
            let x = start_x + (self.t * (0.6 + hash(i, 6)) + i as f32).sin() * 12.0;
            let y = start_y - progress * rise;

            let alpha = (PI * progress).sin().powf(1.3) * 0.85;
            let font_size = 16.0 + hash3(i, cycle, 7) * 16.0;
            let color = if hash(i, 8) > 0.5 { violet } else { highlight };
            let text = pick(RUNE_GLYPHS, hash3(i, cycle, 9));
            self.glyph(text, x, y, font_size, &font, color.opacity(alpha));
        }
    }

    fn nebula_waves(&mut self) {
        let layers = self.count(8.0).max(3);
        let step = 6.0;
        for layer in 0..layers {
            let depth = layer as f32 / (layers - 1).max(1) as f32;
            let amplitude = 18.0 + hash(layer, 1) * 42.0;
            let frequency = 0.004 + hash(layer, 2) * 0.006;
            let speed = 0.25 + hash(layer, 3) * 0.5;
            let phase = hash(layer, 4) * TAU;
            let baseline = self.h * (0.32 + 0.42 * depth);

            let mut points = Vec::with_capacity((self.w / step) as usize + 2);
            let mut x = -step;
            while x <= self.w + step {
                let y = baseline
                    + (x * frequency + self.t * speed + phase).sin() * amplitude
                    + (x * frequency * 2.3 - self.t * speed * 0.7 + phase).sin() * amplitude * 0.35;
                points.push((x, y));
                x += step;
            }

            let color = self.palette.accent.mix(self.palette.shifted(0.08), 1.0 - depth);
            if layer % 3 == 0 {
                let mut area = points.clone();
                area.push((self.w + step, self.h));
                area.push((-step, self.h));
                self.polygon(
                    &area,
                    linear_gradient(
                        180.0,
                        linear_color_stop(color.opacity(0.10), 0.0),
                        linear_color_stop(color.opacity(0.0), 1.0),
                    ),
                );
            }
            self.polyline(&points, 1.2 + hash(layer, 5) * 1.4, color.opacity(0.15 + 0.3 * hash(layer, 6)));
        }
    }

    fn nebula_particles(&mut self) {
        let count = self.count(120.0);
        let link_distance = 110.0;
        let mut particles = Vec::with_capacity(count as usize);
        for i in 0..count {
            let drift_x = (self.t * (0.05 + hash(i, 1) * 0.15) + hash(i, 2) * TAU).sin() * 60.0;
            let drift_y = (self.t * (0.04 + hash(i, 3) * 0.12) + hash(i, 4) * TAU).cos() * 60.0;
            let x = (hash(i, 5) * (self.w + 120.0) + drift_x).rem_euclid(self.w + 120.0) - 60.0;
            let y = (hash(i, 6) * (self.h + 120.0) + drift_y - self.t * (4.0 + hash(i, 7) * 8.0))
                .rem_euclid(self.h + 120.0)
                - 60.0;
            particles.push((x, y));
        }

        for a in 0..particles.len() {
            for b in a + 1..particles.len() {
                let (ax, ay) = particles[a];
                let (bx, by) = particles[b];
                let distance = ((ax - bx).powi(2) + (ay - by).powi(2)).sqrt();
                if distance < link_distance {
                    let alpha = (1.0 - distance / link_distance) * 0.22;
                    self.polyline(&[(ax, ay), (bx, by)], 1.0, self.palette.accent.opacity(alpha));
                }
            }
        }

        for (i, &(x, y)) in particles.iter().enumerate() {
            let i = i as u32;
            let twinkle = 0.55 + 0.45 * (self.t * (1.0 + hash(i, 8) * 2.0) + hash(i, 9) * TAU).sin();
            let radius = 0.8 + hash(i, 10).powi(3) * 2.4;
            if radius > 2.4 {
                self.glow(x, y, radius, 10.0, self.palette.highlight.opacity(0.35 * twinkle));
            }
            self.dot(x, y, radius, self.palette.highlight.opacity(0.35 + 0.55 * twinkle));
        }
    }

    fn nebula_grid(&mut self) {
        let spacing = 44.0;
        let line = self.palette.accent.opacity(0.08);
        let columns = (self.w / spacing).ceil() as u32 + 1;
        let rows = (self.h / spacing).ceil() as u32 + 1;
        for column in 0..columns {
            self.rect(column as f32 * spacing, 0.0, 1.0, self.h, line);
        }
        for row in 0..rows {
            self.rect(0.0, row as f32 * spacing, self.w, 1.0, line);
        }

        let trail = 90.0;
        for i in 0..self.count(26.0) {
            let horizontal = hash(i, 1) > 0.5;
            let span = if horizontal { self.w } else { self.h };
            let speed = 110.0 + hash(i, 2) * 160.0;
            let period = (span + trail * 2.0) / speed + hash(i, 3) * 2.0;
            let shifted = self.t + hash(i, 4) * period;
            let cycle = (shifted / period).floor() as u32;
            let head = (shifted - cycle as f32 * period) * speed - trail;
            let forwards = hash3(i, cycle, 5) > 0.5;
            let head = if forwards { head } else { span - head };
            let lane = if horizontal { rows } else { columns };
            let offset = (hash3(i, cycle, 6) * lane as f32).floor() * spacing;

            let tail = if forwards { head - trail } else { head + trail };
            let (start, end) = (head.min(tail), head.max(tail));
            let (dim, bright) = (self.palette.accent.opacity(0.0), self.palette.highlight.opacity(0.9));
            let (from, to) = if forwards { (dim, bright) } else { (bright, dim) };
            let (angle, x, y, w, h) = if horizontal {
                (90.0, start, offset - 0.5, end - start, 2.0)
            } else {
                (180.0, offset - 0.5, start, 2.0, end - start)
            };
            self.rect(
                x,
                y,
                w,
                h,
                linear_gradient(angle, linear_color_stop(from, 0.0), linear_color_stop(to, 1.0)),
            );

            let (hx, hy) = if horizontal {
                (head, offset + 0.5)
            } else {
                (offset + 0.5, head)
            };
            self.glow(hx, hy, 2.0, 12.0, self.palette.highlight.opacity(0.6));
            self.dot(hx, hy, 2.0, self.palette.highlight);
        }
    }

    fn nebula_voxels(&mut self) {
        const CORNERS: [(f32, f32, f32); 8] = [
            (-1.0, -1.0, -1.0),
            (1.0, -1.0, -1.0),
            (1.0, 1.0, -1.0),
            (-1.0, 1.0, -1.0),
            (-1.0, -1.0, 1.0),
            (1.0, -1.0, 1.0),
            (1.0, 1.0, 1.0),
            (-1.0, 1.0, 1.0),
        ];
        // Wound so the normal from the first three corners points out of the cube
        const FACES: [[usize; 4]; 6] = [
            [0, 3, 2, 1],
            [4, 5, 6, 7],
            [0, 1, 5, 4],
            [3, 7, 6, 2],
            [0, 4, 7, 3],
            [1, 2, 6, 5],
        ];

        for i in 0..self.count(18.0) {
            let half = 11.0 + hash(i, 1).powi(2) * 34.0;
            let speed = 8.0 + hash(i, 2) * 16.0;
            let travel = self.h + half * 4.0;
            let cx = hash(i, 3) * self.w;
            let cy = self.h + half * 2.0 - ((self.t * speed + hash(i, 4) * travel) % travel);
            let (ax, ay) = (self.t * (0.2 + hash(i, 5) * 0.4) + hash(i, 6) * TAU, self.t * (0.15 + hash(i, 7) * 0.35));
            let (sx, cx_) = ax.sin_cos();
            let (sy, cy_) = ay.sin_cos();

            let rotate = |(x, y, z): (f32, f32, f32)| {
                let (x, z) = (x * cy_ + z * sy, -x * sy + z * cy_);
                let (y, z) = (y * cx_ - z * sx, y * sx + z * cx_);
                (x, y, z)
            };
            let corners = CORNERS.map(rotate);

            let alpha = 0.5 + 0.5 * (half / 45.0);
            for face in FACES {
                let [a, b, c, _] = face.map(|index| corners[index]);
                let normal_z = (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
                if normal_z <= 0.0 {
                    continue;
                }
                let facing = (normal_z / 4.0).clamp(0.0, 1.0);
                let points = face.map(|index| (cx + corners[index].0 * half, cy + corners[index].1 * half));
                self.polygon(&points, self.palette.accent.opacity((0.04 + 0.1 * facing) * alpha));
                let mut outline = points.to_vec();
                outline.push(points[0]);
                self.polyline(&outline, 1.0, self.palette.highlight.opacity(0.28 * alpha));
            }
        }
    }

    fn nebula_lightning(&mut self) {
        let (w, h) = (self.w, self.h);
        for i in 0..4 {
            let x = w * (0.15 + 0.7 * hash(i, 1)) + (self.t * 0.05 + i as f32).sin() * 40.0;
            self.glow(x, -h * 0.05, w * 0.18, w * 0.2, self.palette.accent.opacity(0.10));
        }

        for slot in 0..LIGHTNING_SLOTS {
            let Some((cycle, intensity)) = lightning_strike(slot, self.t) else {
                continue;
            };

            let mut bolt = vec![(w * (0.12 + 0.76 * hash3(slot, cycle, 4)), -10.0)];
            let segments = 14;
            let bottom = h * (0.5 + 0.4 * hash3(slot, cycle, 5));
            for s in 1..=segments {
                let (px_, _) = bolt[bolt.len() - 1];
                let jitter = (hash3(slot, cycle, 10 + s) - 0.5) * 70.0;
                bolt.push((px_ + jitter, bottom * s as f32 / segments as f32));
            }

            let mut branches = Vec::new();
            for b in 0..2 {
                let from = 3 + (hash3(slot, cycle, 40 + b) * 7.0) as usize;
                let side = if hash3(slot, cycle, 50 + b) > 0.5 { 1.0 } else { -1.0 };
                let mut branch = vec![bolt[from]];
                for s in 1..6 {
                    let (px_, py) = branch[branch.len() - 1];
                    let jitter = hash3(slot, cycle, 60 + b * 10 + s) * 30.0;
                    branch.push((px_ + side * (10.0 + jitter), py + 18.0 + jitter * 0.6));
                }
                branches.push(branch);
            }

            self.rect(0.0, 0.0, w, h, self.palette.accent.opacity(0.06 * intensity));
            for path in std::iter::once(&bolt).chain(branches.iter()) {
                self.polyline(path, 8.0, self.palette.accent.opacity(0.12 * intensity));
                self.polyline(path, 3.0, self.palette.highlight.opacity(0.35 * intensity));
                self.polyline(path, 1.3, gpui::white().opacity(0.9 * intensity));
            }
        }
    }

    fn liquid_chrome(&mut self) {
        let (w, h) = (self.w, self.h);
        let colors = [
            self.palette.accent.opacity(0.7),
            self.palette.shifted(0.3).saturation(0.7).opacity(0.55),
            self.palette.shifted(-0.18).saturation(0.65).opacity(0.6),
            self.palette.highlight.opacity(0.55),
            hsla(0.0, 0.0, 0.85, 0.45),
            self.palette.shifted(0.12).opacity(0.6),
        ];
        for (i, color) in colors.into_iter().enumerate() {
            let i = i as u32;
            let x = w * (0.5 + 0.4 * (self.t * (0.07 + hash(i, 1) * 0.08) + hash(i, 2) * TAU).sin());
            let y = h * (0.5 + 0.38 * (self.t * (0.05 + hash(i, 3) * 0.07) + hash(i, 4) * TAU).sin());
            let radius = w.min(h) * (0.14 + hash(i, 5) * 0.14);
            self.glow(x, y, radius, radius * 0.9, color);
        }

        // Folds of polished metal flowing over the colour, lit along their upper edge
        let step = 8.0;
        for ribbon in 0..3u32 {
            let baseline = h * (0.25 + 0.25 * ribbon as f32);
            let amplitude = h * (0.08 + 0.06 * hash(ribbon, 6));
            let frequency = 0.003 + hash(ribbon, 7) * 0.003;
            let speed = 0.2 + hash(ribbon, 8) * 0.25;
            let thickness = 40.0 + hash(ribbon, 9) * 60.0;
            let wave = |x: f32, t: f32| {
                baseline
                    + (x * frequency + t * speed + ribbon as f32).sin() * amplitude
                    + (x * frequency * 0.5 - t * speed * 0.6).sin() * amplitude * 0.6
            };

            let mut top = Vec::new();
            let mut x = -step;
            while x <= w + step {
                top.push((x, wave(x, self.t)));
                x += step;
            }
            let mut band = top.clone();
            band.extend(top.iter().rev().map(|&(x, y)| (x, y + thickness)));
            self.polygon(
                &band,
                linear_gradient(
                    180.0,
                    linear_color_stop(gpui::white().opacity(0.16), 0.0),
                    linear_color_stop(gpui::white().opacity(0.0), 1.0),
                ),
            );
            self.polyline(&top, 1.5, gpui::white().opacity(0.35));
        }
    }

    fn retro_grid(&mut self) {
        // A corridor seen from inside: a gridded ceiling and floor running away to a far wall, with
        // the space between them left open for the skin. One-point perspective onto the centre.
        const NEAR: f32 = 1.0;
        /// Puts the far wall's seams about a fifth of the way in from the top and bottom
        const FAR: f32 = 1.75;
        /// Depth between the lines that cross the corridor, and width between those running along it
        const ROW_SPACING: f32 = 0.08;
        const LANE_SPACING: f32 = 0.28;
        /// How fast the crossing lines rush towards the viewer, in depth per second
        const SPEED: f32 = 0.06;
        /// Lines run on past the edge of the screen, to this depth
        const BEYOND_SCREEN: f32 = 0.45;

        let (w, h) = (self.w, self.h);
        let (cx, cy) = (w * 0.5, h * 0.5);
        // A plane one unit off the eye line reaches the edge of the screen at the near depth
        let focal = h * 0.5;
        let accent = self.palette.accent;
        let highlight = self.palette.highlight;
        let to_x = |lane: f32, depth: f32| cx + lane * LANE_SPACING * focal / depth;

        for side in [1.0f32, -1.0] {
            let to_y = |depth: f32| cy + side * focal / depth;
            let seam = to_y(FAR);

            // Light spilling off the seam into the open middle
            let spill = h * 0.07;
            let (lit, clear) = (accent.opacity(0.12), accent.opacity(0.0));
            let (top, angle) = if side > 0.0 { (seam - spill, 0.0) } else { (seam, 180.0) };
            self.rect(
                0.0,
                top,
                w,
                spill,
                linear_gradient(angle, linear_color_stop(lit, 0.0), linear_color_stop(clear, 1.0)),
            );

            // Lanes along the corridor, from the far wall to past the edge of the screen
            let lanes = (cx / (LANE_SPACING * focal / FAR)).ceil() as i32 + 1;
            let lane_lines: Vec<_> = (-lanes..=lanes)
                .map(|lane| {
                    let lane = lane as f32;
                    ((to_x(lane, FAR), seam), (to_x(lane, BEYOND_SCREEN), to_y(BEYOND_SCREEN)))
                })
                .collect();
            self.segments(&lane_lines, 3.0, accent.opacity(0.14));
            self.segments(&lane_lines, 1.2, accent.opacity(0.5));

            // Lines across it, appearing at the far wall and rushing out past the viewer
            let scroll = (self.t * SPEED) % ROW_SPACING;
            let mut depth = FAR - scroll;
            while depth > BEYOND_SCREEN {
                // Fades in as it leaves the seam, rather than popping into existence there
                let arrival = ((FAR - depth) / ROW_SPACING).min(1.0);
                let nearness = (NEAR / depth).min(1.0);
                let alpha = (0.32 + 0.22 * nearness) * arrival;
                let y = to_y(depth);
                self.rect(0.0, y - 1.5, w, 3.0, accent.opacity(0.14 * arrival));
                self.rect(0.0, y - 0.6, w, 1.2, accent.opacity(alpha));
                depth -= ROW_SPACING;
            }

            // The seam itself, brightest of all
            self.rect(0.0, seam - 2.0, w, 4.0, highlight.opacity(0.12));
            self.rect(0.0, seam - 0.75, w, 1.5, highlight.opacity(0.6));
        }
    }

    fn snow(&mut self) {
        let count = self.count(140.0);
        let fall = self.h + 20.0;
        for i in 0..count {
            let radius = 0.8 + hash(i, 1).powi(2) * 2.2;
            let speed = 16.0 + radius * 14.0 + hash(i, 2) * 14.0;
            let sway = (self.t * (0.3 + hash(i, 3) * 0.7) + hash(i, 4) * TAU).sin() * (10.0 + hash(i, 5) * 22.0);
            let x = (hash(i, 6) * (self.w + 40.0) + sway).rem_euclid(self.w + 40.0) - 20.0;
            let y = ((self.t * speed + hash(i, 7) * fall) % fall) - 10.0;
            let alpha = 0.35 + hash(i, 8) * 0.5;

            if i % 11 == 0 {
                // An occasional proper flake, slowly turning as it falls
                let arm = 3.0 + hash(i, 9) * 3.0;
                let spin = self.t * (0.4 + hash(i, 10)) + hash(i, 11) * TAU;
                for k in 0..3 {
                    let angle = spin + k as f32 * PI / 3.0;
                    let (s, c) = angle.sin_cos();
                    self.polyline(
                        &[(x - c * arm, y - s * arm), (x + c * arm, y + s * arm)],
                        1.0,
                        gpui::white().opacity(alpha),
                    );
                }
            } else {
                self.dot(x, y, radius, gpui::white().opacity(alpha));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    // Not a glob: gpui exports its own `test` macro, which would shadow the standard one
    use std::time::Duration;

    use super::{AnimationClock, BackgroundEffect, EffectQuality, hash, lightning_strike};

    #[test]
    fn hash_stays_in_range() {
        for a in 0..200 {
            for b in 0..50 {
                let value = hash(a, b);
                assert!((0.0..1.0).contains(&value), "hash({a}, {b}) = {value}");
            }
        }
    }

    #[test]
    fn hash_is_spread_out() {
        let mean = (0..10_000).map(|i| hash(i, 3)).sum::<f32>() / 10_000.0;
        assert!((mean - 0.5).abs() < 0.02, "mean {mean}");
    }

    #[test]
    fn a_paused_clock_does_not_skip_ahead() {
        let mut clock = AnimationClock::starting_at(5.0);
        clock.tick(true);
        clock.tick(false);
        std::thread::sleep(Duration::from_millis(30));
        // The first running tick after a pause only re-anchors the clock
        assert_eq!(clock.tick(true), clock.elapsed());
        assert!(clock.elapsed() < 5.02, "elapsed {}", clock.elapsed());
    }

    #[test]
    fn a_running_clock_advances() {
        let mut clock = AnimationClock::default();
        clock.tick(true);
        std::thread::sleep(Duration::from_millis(20));
        assert!(clock.tick(true) >= 0.019);
    }

    #[test]
    fn the_lightning_preview_catches_a_strike() {
        let time = BackgroundEffect::NebulaLightning.preview_time() as f32;
        assert!(lightning_strike(0, time).is_some());
    }

    #[test]
    fn quality_round_trips_through_its_slider_index() {
        for quality in [EffectQuality::Low, EffectQuality::Medium, EffectQuality::High] {
            assert_eq!(EffectQuality::from_index(quality.index()), quality);
        }
    }
}
