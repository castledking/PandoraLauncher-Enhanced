//! Exact-cell rendering for Unicode Block Elements (`U+2580..=U+259F`) and Box Drawing
//! (`U+2500..=U+257F`) in the monospaced log view.
//!
//! The bundled `Roboto Mono` font doesn't include box-drawing or block-element glyphs, so without
//! custom rendering GPUI falls back to a system font (often proportional, like DejaVu Sans on
//! Linux) whose advance widths don't match the monospace grid and whose glyphs leave horizontal
//! and vertical seams between rows. Drawing these characters directly as pixel-snapped quads
//! inside each monospace cell keeps ASCII/Unicode banners aligned and seamless on every platform.

use gpui::{Bounds, Hsla, Pixels, Window, fill, point, px};

/// A block-element or box-drawing glyph to draw over a space placeholder in a [`gpui::ShapedLine`].
#[derive(Clone, Copy, Debug)]
pub struct BoxCell {
    /// Byte index of the single-byte `' '` placeholder in the shaped line's text.
    pub byte_ix: usize,
    /// Original Unicode character to draw.
    pub ch: char,
    /// Foreground colour resolved for this cell.
    pub color: Hsla,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stroke {
    None,
    Light,
    Heavy,
    Double,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BoxArms {
    left: Stroke,
    right: Stroke,
    up: Stroke,
    down: Stroke,
}

impl BoxArms {
    const fn new(left: Stroke, right: Stroke, up: Stroke, down: Stroke) -> Self {
        Self { left, right, up, down }
    }
}

/// Returns `true` for characters rendered directly into the monospace cell grid.
pub fn is_supported(ch: char) -> bool {
    is_block_element(ch) || box_arms(ch).is_some()
}

fn is_block_element(ch: char) -> bool {
    ('\u{2580}'..='\u{259F}').contains(&ch)
}

/// Paints a single block-element or box-drawing glyph inside the cell `[x0, x1) x [y0, y1)`.
///
/// Adjacent cells must pass matching edges (`prev.x1 == next.x0` and `prev.y1 == next.y0`) so
/// consecutive blocks and box borders connect with zero subpixel seams.
pub fn paint_cell(x0: Pixels, x1: Pixels, y0: Pixels, y1: Pixels, ch: char, color: Hsla, window: &mut Window) {
    let x0 = x0.floor();
    let x1 = x1.floor();
    let y0 = y0.floor();
    let y1 = y1.floor();
    if x1 <= x0 || y1 <= y0 {
        return;
    }

    if is_block_element(ch) {
        paint_block_element(x0, x1, y0, y1, ch, color, window);
    } else if let Some(arms) = box_arms(ch) {
        paint_box_arms(x0, x1, y0, y1, arms, color, window);
    }
}

fn quad(x0: Pixels, y0: Pixels, x1: Pixels, y1: Pixels, color: Hsla, window: &mut Window) {
    if x1 > x0 && y1 > y0 {
        window.paint_quad(fill(Bounds::from_corners(point(x0, y0), point(x1, y1)), color));
    }
}

fn frac_x(x0: Pixels, x1: Pixels, num: u32, den: u32) -> Pixels {
    let w: f32 = (x1 - x0).into();
    let base: f32 = x0.into();
    px((base + w * (num as f32 / den as f32)).round())
}

fn frac_y(y0: Pixels, y1: Pixels, num: u32, den: u32) -> Pixels {
    let h: f32 = (y1 - y0).into();
    let base: f32 = y0.into();
    px((base + h * (num as f32 / den as f32)).round())
}

fn paint_block_element(x0: Pixels, x1: Pixels, y0: Pixels, y1: Pixels, ch: char, color: Hsla, window: &mut Window) {
    match ch {
        // Upper half block
        '\u{2580}' => quad(x0, y0, x1, frac_y(y0, y1, 1, 2), color, window),
        // Lower 1/8 .. 7/8 blocks
        '\u{2581}'..='\u{2587}' => {
            let eighths = (ch as u32) - 0x2580;
            let top = frac_y(y0, y1, 8 - eighths, 8);
            quad(x0, top, x1, y1, color, window);
        },
        // Full block
        '\u{2588}' => quad(x0, y0, x1, y1, color, window),
        // Left 7/8 .. 1/8 blocks
        '\u{2589}'..='\u{258F}' => {
            let eighths = 0x2590 - (ch as u32);
            let right = frac_x(x0, x1, eighths, 8);
            quad(x0, y0, right, y1, color, window);
        },
        // Right half block
        '\u{2590}' => quad(frac_x(x0, x1, 1, 2), y0, x1, y1, color, window),
        // Light / medium / dark shade
        '\u{2591}' => quad(x0, y0, x1, y1, color.opacity(0.25), window),
        '\u{2592}' => quad(x0, y0, x1, y1, color.opacity(0.50), window),
        '\u{2593}' => quad(x0, y0, x1, y1, color.opacity(0.75), window),
        // Upper 1/8 block
        '\u{2594}' => quad(x0, y0, x1, frac_y(y0, y1, 1, 8), color, window),
        // Right 1/8 block
        '\u{2595}' => quad(frac_x(x0, x1, 7, 8), y0, x1, y1, color, window),
        // Quadrants: bits = [UL=1, UR=2, LL=4, LR=8]
        '\u{2596}'..='\u{259F}' => {
            let mask: u8 = match ch {
                '\u{2596}' => 0b0100, // ▖
                '\u{2597}' => 0b1000, // ▗
                '\u{2598}' => 0b0001, // ▘
                '\u{2599}' => 0b1101, // ▙
                '\u{259A}' => 0b1001, // ▚
                '\u{259B}' => 0b0111, // ▛
                '\u{259C}' => 0b1011, // ▜
                '\u{259D}' => 0b0010, // ▝
                '\u{259E}' => 0b0110, // ▞
                '\u{259F}' => 0b1110, // ▟
                _ => 0,
            };
            let xm = frac_x(x0, x1, 1, 2);
            let ym = frac_y(y0, y1, 1, 2);
            if mask & 0b0001 != 0 {
                quad(x0, y0, xm, ym, color, window);
            }
            if mask & 0b0010 != 0 {
                quad(xm, y0, x1, ym, color, window);
            }
            if mask & 0b0100 != 0 {
                quad(x0, ym, xm, y1, color, window);
            }
            if mask & 0b1000 != 0 {
                quad(xm, ym, x1, y1, color, window);
            }
        },
        _ => {},
    }
}

fn paint_box_arms(x0: Pixels, x1: Pixels, y0: Pixels, y1: Pixels, arms: BoxArms, color: Hsla, window: &mut Window) {
    let w: f32 = (x1 - x0).into();
    let light = (w * 0.14).round().max(1.0);
    let heavy = (light * 2.0).max(2.0);
    let gap = light.max(1.0);

    let x0f: f32 = x0.into();
    let x1f: f32 = x1.into();
    let y0f: f32 = y0.into();
    let y1f: f32 = y1.into();

    let cx = ((x0f + x1f) * 0.5).floor();
    let cy = ((y0f + y1f) * 0.5).floor();

    let v_center_left = |stroke: Stroke| -> f32 {
        match stroke {
            Stroke::Heavy => cx - (heavy * 0.5).floor(),
            _ => cx - (light * 0.5).floor(),
        }
    };
    let h_center_top = |stroke: Stroke| -> f32 {
        match stroke {
            Stroke::Heavy => cy - (heavy * 0.5).floor(),
            _ => cy - (light * 0.5).floor(),
        }
    };

    // Outer bounds of the center junction so perpendicular arms meet cleanly with no corner notches
    let v_span_left = match (arms.up, arms.down) {
        (Stroke::Double, _) | (_, Stroke::Double) => cx - gap - light,
        (Stroke::Heavy, _) | (_, Stroke::Heavy) => v_center_left(Stroke::Heavy),
        (Stroke::Light, _) | (_, Stroke::Light) => v_center_left(Stroke::Light),
        (Stroke::None, Stroke::None) => cx - (light * 0.5).floor(),
    };
    let v_span_right = match (arms.up, arms.down) {
        (Stroke::Double, _) | (_, Stroke::Double) => cx + gap + light,
        (Stroke::Heavy, _) | (_, Stroke::Heavy) => v_center_left(Stroke::Heavy) + heavy,
        (Stroke::Light, _) | (_, Stroke::Light) => v_center_left(Stroke::Light) + light,
        (Stroke::None, Stroke::None) => cx - (light * 0.5).floor() + light,
    };
    let h_span_top = match (arms.left, arms.right) {
        (Stroke::Double, _) | (_, Stroke::Double) => cy - gap - light,
        (Stroke::Heavy, _) | (_, Stroke::Heavy) => h_center_top(Stroke::Heavy),
        (Stroke::Light, _) | (_, Stroke::Light) => h_center_top(Stroke::Light),
        (Stroke::None, Stroke::None) => cy - (light * 0.5).floor(),
    };
    let h_span_bottom = match (arms.left, arms.right) {
        (Stroke::Double, _) | (_, Stroke::Double) => cy + gap + light,
        (Stroke::Heavy, _) | (_, Stroke::Heavy) => h_center_top(Stroke::Heavy) + heavy,
        (Stroke::Light, _) | (_, Stroke::Light) => h_center_top(Stroke::Light) + light,
        (Stroke::None, Stroke::None) => cy - (light * 0.5).floor() + light,
    };

    // Parallel offsets for double strokes
    let dx_l = cx - gap - light;
    let dx_r = cx + gap;
    let dy_t = cy - gap - light;
    let dy_b = cy + gap;

    // Horizontal arms
    match arms.left {
        Stroke::None => {},
        Stroke::Light => {
            let t = h_center_top(Stroke::Light);
            let end = if arms.right == Stroke::None { v_span_right } else { cx };
            quad(px(x0f), px(t), px(end), px(t + light), color, window);
        },
        Stroke::Heavy => {
            let t = h_center_top(Stroke::Heavy);
            let end = if arms.right == Stroke::None { v_span_right } else { cx };
            quad(px(x0f), px(t), px(end), px(t + heavy), color, window);
        },
        Stroke::Double => {
            let end_top = if arms.up == Stroke::Double {
                dx_l + light
            } else if arms.right == Stroke::None {
                v_span_right
            } else {
                cx
            };
            let end_bottom = if arms.down == Stroke::Double {
                dx_l + light
            } else if arms.right == Stroke::None {
                v_span_right
            } else {
                cx
            };
            quad(px(x0f), px(dy_t), px(end_top), px(dy_t + light), color, window);
            quad(px(x0f), px(dy_b), px(end_bottom), px(dy_b + light), color, window);
        },
    }

    match arms.right {
        Stroke::None => {},
        Stroke::Light => {
            let t = h_center_top(Stroke::Light);
            let start = if arms.left == Stroke::None { v_span_left } else { cx };
            quad(px(start), px(t), px(x1f), px(t + light), color, window);
        },
        Stroke::Heavy => {
            let t = h_center_top(Stroke::Heavy);
            let start = if arms.left == Stroke::None { v_span_left } else { cx };
            quad(px(start), px(t), px(x1f), px(t + heavy), color, window);
        },
        Stroke::Double => {
            let start_top = if arms.up == Stroke::Double {
                dx_r
            } else if arms.left == Stroke::None {
                v_span_left
            } else {
                cx
            };
            let start_bottom = if arms.down == Stroke::Double {
                dx_r
            } else if arms.left == Stroke::None {
                v_span_left
            } else {
                cx
            };
            quad(px(start_top), px(dy_t), px(x1f), px(dy_t + light), color, window);
            quad(px(start_bottom), px(dy_b), px(x1f), px(dy_b + light), color, window);
        },
    }

    // Vertical arms
    match arms.up {
        Stroke::None => {},
        Stroke::Light => {
            let l = v_center_left(Stroke::Light);
            let end = if arms.down == Stroke::None { h_span_bottom } else { cy };
            quad(px(l), px(y0f), px(l + light), px(end), color, window);
        },
        Stroke::Heavy => {
            let l = v_center_left(Stroke::Heavy);
            let end = if arms.down == Stroke::None { h_span_bottom } else { cy };
            quad(px(l), px(y0f), px(l + heavy), px(end), color, window);
        },
        Stroke::Double => {
            let end_left = if arms.left == Stroke::Double {
                dy_t + light
            } else if arms.down == Stroke::None {
                h_span_bottom
            } else {
                cy
            };
            let end_right = if arms.right == Stroke::Double {
                dy_t + light
            } else if arms.down == Stroke::None {
                h_span_bottom
            } else {
                cy
            };
            quad(px(dx_l), px(y0f), px(dx_l + light), px(end_left), color, window);
            quad(px(dx_r), px(y0f), px(dx_r + light), px(end_right), color, window);
        },
    }

    match arms.down {
        Stroke::None => {},
        Stroke::Light => {
            let l = v_center_left(Stroke::Light);
            let start = if arms.up == Stroke::None { h_span_top } else { cy };
            quad(px(l), px(start), px(l + light), px(y1f), color, window);
        },
        Stroke::Heavy => {
            let l = v_center_left(Stroke::Heavy);
            let start = if arms.up == Stroke::None { h_span_top } else { cy };
            quad(px(l), px(start), px(l + heavy), px(y1f), color, window);
        },
        Stroke::Double => {
            let start_left = if arms.left == Stroke::Double {
                dy_b
            } else if arms.up == Stroke::None {
                h_span_top
            } else {
                cy
            };
            let start_right = if arms.right == Stroke::Double {
                dy_b
            } else if arms.up == Stroke::None {
                h_span_top
            } else {
                cy
            };
            quad(px(dx_l), px(start_left), px(dx_l + light), px(y1f), color, window);
            quad(px(dx_r), px(start_right), px(dx_r + light), px(y1f), color, window);
        },
    }
}

const fn box_arms(ch: char) -> Option<BoxArms> {
    use Stroke::{Double as D, Heavy as H, Light as L, None as N};
    let arms = match ch {
        '─' => BoxArms::new(L, L, N, N),
        '━' => BoxArms::new(H, H, N, N),
        '│' => BoxArms::new(N, N, L, L),
        '┃' => BoxArms::new(N, N, H, H),
        '┌' | '╭' => BoxArms::new(N, L, N, L),
        '┍' => BoxArms::new(N, H, N, L),
        '┎' => BoxArms::new(N, L, N, H),
        '┏' => BoxArms::new(N, H, N, H),
        '┐' | '╮' => BoxArms::new(L, N, N, L),
        '┑' => BoxArms::new(H, N, N, L),
        '┒' => BoxArms::new(L, N, N, H),
        '┓' => BoxArms::new(H, N, N, H),
        '└' | '╰' => BoxArms::new(N, L, L, N),
        '┕' => BoxArms::new(N, H, L, N),
        '┖' => BoxArms::new(N, L, H, N),
        '┗' => BoxArms::new(N, H, H, N),
        '┘' | '╯' => BoxArms::new(L, N, L, N),
        '┙' => BoxArms::new(H, N, L, N),
        '┚' => BoxArms::new(L, N, H, N),
        '┛' => BoxArms::new(H, N, H, N),
        '├' => BoxArms::new(N, L, L, L),
        '┝' => BoxArms::new(N, H, L, L),
        '┞' => BoxArms::new(N, L, H, L),
        '┟' => BoxArms::new(N, L, L, H),
        '┠' => BoxArms::new(N, L, H, H),
        '┡' => BoxArms::new(N, H, H, L),
        '┢' => BoxArms::new(N, H, L, H),
        '┣' => BoxArms::new(N, H, H, H),
        '┤' => BoxArms::new(L, N, L, L),
        '┥' => BoxArms::new(H, N, L, L),
        '┦' => BoxArms::new(L, N, H, L),
        '┧' => BoxArms::new(L, N, L, H),
        '┨' => BoxArms::new(L, N, H, H),
        '┩' => BoxArms::new(H, N, H, L),
        '┪' => BoxArms::new(H, N, L, H),
        '┫' => BoxArms::new(H, N, H, H),
        '┬' => BoxArms::new(L, L, N, L),
        '┭' => BoxArms::new(H, L, N, L),
        '┮' => BoxArms::new(L, H, N, L),
        '┯' => BoxArms::new(H, H, N, L),
        '┰' => BoxArms::new(L, L, N, H),
        '┱' => BoxArms::new(H, L, N, H),
        '┲' => BoxArms::new(L, H, N, H),
        '┳' => BoxArms::new(H, H, N, H),
        '┴' => BoxArms::new(L, L, L, N),
        '┵' => BoxArms::new(H, L, L, N),
        '┶' => BoxArms::new(L, H, L, N),
        '┷' => BoxArms::new(H, H, L, N),
        '┸' => BoxArms::new(L, L, H, N),
        '┹' => BoxArms::new(H, L, H, N),
        '┺' => BoxArms::new(L, H, H, N),
        '┻' => BoxArms::new(H, H, H, N),
        '┼' => BoxArms::new(L, L, L, L),
        '┽' => BoxArms::new(H, L, L, L),
        '┾' => BoxArms::new(L, H, L, L),
        '┿' => BoxArms::new(H, H, L, L),
        '╀' => BoxArms::new(L, L, H, L),
        '╁' => BoxArms::new(L, L, L, H),
        '╂' => BoxArms::new(L, L, H, H),
        '╃' => BoxArms::new(H, L, H, L),
        '╄' => BoxArms::new(L, H, H, L),
        '╅' => BoxArms::new(H, L, L, H),
        '╆' => BoxArms::new(L, H, L, H),
        '╇' => BoxArms::new(H, H, H, L),
        '╈' => BoxArms::new(H, H, L, H),
        '╉' => BoxArms::new(H, L, H, H),
        '╊' => BoxArms::new(L, H, H, H),
        '╋' => BoxArms::new(H, H, H, H),
        '═' => BoxArms::new(D, D, N, N),
        '║' => BoxArms::new(N, N, D, D),
        '╒' => BoxArms::new(N, D, N, L),
        '╓' => BoxArms::new(N, L, N, D),
        '╔' => BoxArms::new(N, D, N, D),
        '╕' => BoxArms::new(D, N, N, L),
        '╖' => BoxArms::new(L, N, N, D),
        '╗' => BoxArms::new(D, N, N, D),
        '╘' => BoxArms::new(N, D, L, N),
        '╙' => BoxArms::new(N, L, D, N),
        '╚' => BoxArms::new(N, D, D, N),
        '╛' => BoxArms::new(D, N, L, N),
        '╜' => BoxArms::new(L, N, D, N),
        '╝' => BoxArms::new(D, N, D, N),
        '╞' => BoxArms::new(N, D, L, L),
        '╟' => BoxArms::new(N, L, D, D),
        '╠' => BoxArms::new(N, D, D, D),
        '╡' => BoxArms::new(D, N, L, L),
        '╢' => BoxArms::new(L, N, D, D),
        '╣' => BoxArms::new(D, N, D, D),
        '╤' => BoxArms::new(D, D, N, L),
        '╥' => BoxArms::new(L, L, N, D),
        '╦' => BoxArms::new(D, D, N, D),
        '╧' => BoxArms::new(D, D, L, N),
        '╨' => BoxArms::new(L, L, D, N),
        '╩' => BoxArms::new(D, D, D, N),
        '╪' => BoxArms::new(D, D, L, L),
        '╫' => BoxArms::new(L, L, D, D),
        '╬' => BoxArms::new(D, D, D, D),
        '╴' => BoxArms::new(L, N, N, N),
        '╵' => BoxArms::new(N, N, L, N),
        '╶' => BoxArms::new(N, L, N, N),
        '╷' => BoxArms::new(N, N, N, L),
        '╸' => BoxArms::new(H, N, N, N),
        '╹' => BoxArms::new(N, N, H, N),
        '╺' => BoxArms::new(N, H, N, N),
        '╻' => BoxArms::new(N, N, N, H),
        '╼' => BoxArms::new(L, H, N, N),
        '╽' => BoxArms::new(N, N, L, H),
        '╾' => BoxArms::new(H, L, N, N),
        '╿' => BoxArms::new(N, N, H, L),
        _ => return None,
    };
    Some(arms)
}

#[cfg(test)]
mod tests {
    use super::is_supported;

    #[test]
    fn recognizes_all_block_elements_and_standard_box_drawing() {
        for ch in ['█', '▄', '▀', '▓', '▒', '░', '▌', '▐', '▖', '▟', '╔', '═', '╗', '║', '╚', '╝', '─', '│', '┌', '┘'] {
            assert!(is_supported(ch), "expected {ch:?} to be supported");
        }
        for ch in [' ', 'A', '0', '(', ')', '-', '|', '_'] {
            assert!(!is_supported(ch), "expected {ch:?} not to be treated as a box char");
        }
    }
}
