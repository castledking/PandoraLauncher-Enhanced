//! Colour codes in log lines. A server console prints ANSI escapes (Paper turns a plugin's
//! colours into 24-bit ones), while Minecraft's own § codes turn up as they are in plain server
//! output and in the game's chat log. Either way the codes are taken out of the text, so search,
//! selection and copying all see what is on screen, and kept as spans to draw it with.

use std::{ops::Range, sync::Arc};

use gpui::{Hsla, Rgba, rgb};

/// How a stretch of a line is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SpanStyle {
    /// `None` is the log's own text colour.
    pub color: Option<Rgba>,
    pub underline: bool,
    pub strikethrough: bool,
}

/// A stretch of a line drawn other than plainly, in bytes of the line with its codes taken out.
#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    pub range: Range<usize>,
    pub style: SpanStyle,
}

/// A message's lines with their codes taken out, and the spans each one is drawn with.
pub struct Formatted {
    pub text: Arc<[Arc<str>]>,
    /// One per line, or empty when no line had any codes at all.
    pub spans: Box<[Box<[Span]>]>,
}

/// Minecraft's sixteen colours, in code order (§0 to §f). ANSI's sixteen are mapped onto the
/// same ones, the way Paper maps them the other way.
const PALETTE: [u32; 16] = [
    0x000000, 0x0000AA, 0x00AA00, 0x00AAAA, 0xAA0000, 0xAA00AA, 0xFFAA00, 0xAAAAAA, 0x555555, 0x5555FF, 0x55FF55,
    0x55FFFF, 0xFF5555, 0xFF55FF, 0xFFFF55, 0xFFFFFF,
];

/// ANSI's colours 0 to 15 as indices into `PALETTE`: black, red, green, yellow, blue, magenta,
/// cyan, white, then the bright ones.
const ANSI_TO_PALETTE: [usize; 16] = [0, 4, 2, 6, 1, 5, 3, 7, 8, 12, 10, 14, 9, 13, 11, 15];

const ESC: char = '\x1b';
const SECTION: char = '§';

/// Takes the codes out of a message's lines. A colour runs on from one line to the next, as it
/// would in a terminal, but not into the next message.
pub fn format_lines(lines: &Arc<[Arc<str>]>) -> Formatted {
    if !lines.iter().any(|line| line.contains([ESC, SECTION])) {
        return Formatted {
            text: lines.clone(),
            spans: Box::new([]),
        };
    }

    let mut style = SpanStyle::default();
    let mut text = Vec::with_capacity(lines.len());
    let mut spans = Vec::with_capacity(lines.len());
    for line in lines.iter() {
        let (stripped, line_spans) = format_line(line, &mut style);
        text.push(stripped);
        spans.push(line_spans);
    }

    Formatted {
        text: text.into(),
        spans: spans.into(),
    }
}

fn format_line(line: &str, style: &mut SpanStyle) -> (Arc<str>, Box<[Span]>) {
    let mut out = String::with_capacity(line.len());
    let mut spans: Vec<Span> = Vec::new();

    let mut push_text = |out: &mut String, text: &str, style: SpanStyle| {
        if text.is_empty() {
            return;
        }
        let start = out.len();
        out.push_str(text);
        if style == SpanStyle::default() {
            return;
        }
        match spans.last_mut() {
            Some(last) if last.range.end == start && last.style == style => last.range.end = out.len(),
            _ => spans.push(Span {
                range: start..out.len(),
                style,
            }),
        }
    };

    let mut rest = line;
    while let Some(at) = rest.find([ESC, SECTION]) {
        push_text(&mut out, &rest[..at], *style);
        let code = &rest[at..];

        rest = if let Some(after) = code.strip_prefix(ESC) {
            skip_escape(after, style)
        } else {
            let after = &code[SECTION.len_utf8()..];
            match apply_section_code(after, style) {
                Some(after) => after,
                // A § that isn't a code is just a character
                None => {
                    push_text(&mut out, &code[..SECTION.len_utf8()], *style);
                    after
                },
            }
        };
    }
    push_text(&mut out, rest, *style);

    (out.into(), spans.into())
}

/// Steps over an escape sequence, `after` being what follows the ESC, applying it if it sets
/// colours and dropping it if it does anything else, which a log view has no use for.
fn skip_escape<'a>(after: &'a str, style: &mut SpanStyle) -> &'a str {
    let bytes = after.as_bytes();
    match bytes.first() {
        // CSI: parameters, then intermediates, then the byte saying what it is
        Some(b'[') => {
            let body = &after[1..];
            let params_end = body.bytes().position(|byte| !(0x30..=0x3F).contains(&byte)).unwrap_or(body.len());
            let final_at = params_end
                + body[params_end..]
                    .bytes()
                    .position(|byte| !(0x20..=0x2F).contains(&byte))
                    .unwrap_or(body.len() - params_end);
            match body.as_bytes().get(final_at) {
                Some(&final_byte) if (0x40..=0x7E).contains(&final_byte) => {
                    if final_byte == b'm' {
                        apply_sgr(&body[..params_end], style);
                    }
                    &body[final_at + 1..]
                },
                // Cut off, or not an escape after all
                _ => &body[final_at..],
            }
        },
        // OSC, such as a window title: runs to a BEL or to ESC \
        Some(b']') => {
            let body = &after[1..];
            match body.find(['\x07', ESC]) {
                Some(end) if body.as_bytes()[end] == 0x07 => &body[end + 1..],
                Some(end) => body[end + 1..].strip_prefix('\\').unwrap_or(&body[end + 1..]),
                None => "",
            }
        },
        // Picking a character set takes one more character
        Some(b'(' | b')') => after.get(2..).unwrap_or(""),
        Some(byte) if byte.is_ascii() => &after[1..],
        _ => after,
    }
}

/// Applies a Select Graphic Rendition sequence's parameters, the part between `ESC [` and `m`.
fn apply_sgr(params: &str, style: &mut SpanStyle) {
    // An empty parameter is a zero, so a bare `ESC [ m` resets
    let mut params = params.split(';').map(|param| {
        if param.is_empty() {
            Some(0)
        } else {
            param.parse::<u16>().ok()
        }
    });

    while let Some(param) = params.next() {
        let Some(param) = param else {
            continue;
        };
        match param {
            0 => *style = SpanStyle::default(),
            4 | 21 => style.underline = true,
            24 => style.underline = false,
            9 => style.strikethrough = true,
            29 => style.strikethrough = false,
            30..=37 => style.color = Some(rgb(PALETTE[ANSI_TO_PALETTE[(param - 30) as usize]])),
            90..=97 => style.color = Some(rgb(PALETTE[ANSI_TO_PALETTE[(param - 90 + 8) as usize]])),
            39 => style.color = None,
            38 => {
                if let Some(color) = extended_color(&mut params) {
                    style.color = Some(color);
                }
            },
            // Backgrounds aren't drawn, but their parameters still have to be stepped over
            48 => _ = extended_color(&mut params),
            _ => {},
        }
    }
}

/// Reads the rest of a 38 or 48: `5;n` from the 256 colour palette, or `2;r;g;b`.
fn extended_color(params: &mut impl Iterator<Item = Option<u16>>) -> Option<Rgba> {
    let mut next = || params.next().flatten();
    match next()? {
        5 => Some(rgb(ansi_256(next()?))),
        2 => {
            let (r, g, b) = (next()?, next()?, next()?);
            Some(rgb(((r.min(255) as u32) << 16) | ((g.min(255) as u32) << 8) | b.min(255) as u32))
        },
        _ => None,
    }
}

fn ansi_256(index: u16) -> u32 {
    match index {
        0..=15 => PALETTE[ANSI_TO_PALETTE[index as usize]],
        16..=231 => {
            let index = index as u32 - 16;
            let level = |value: u32| if value == 0 { 0 } else { 55 + value * 40 };
            (level(index / 36) << 16) | (level(index / 6 % 6) << 8) | level(index % 6)
        },
        _ => {
            let gray = 8 + (index.min(255) as u32 - 232) * 10;
            (gray << 16) | (gray << 8) | gray
        },
    }
}

/// Applies the § code that `after` starts with, returning what follows it, or `None` if it
/// isn't one.
fn apply_section_code<'a>(after: &'a str, style: &mut SpanStyle) -> Option<&'a str> {
    let code = after.chars().next()?.to_ascii_lowercase();
    let rest = &after[code.len_utf8()..];

    match code {
        // As in the game, a colour also ends any underlining or striking through
        '0'..='9' | 'a'..='f' => {
            *style = SpanStyle {
                color: Some(rgb(PALETTE[code.to_digit(16)? as usize])),
                ..Default::default()
            };
        },
        'x' => {
            // §x§r§r§g§g§b§b, as Spigot writes an RGB colour
            let mut hex = 0;
            let mut digits = rest;
            for _ in 0..6 {
                let digit = digits.strip_prefix(SECTION)?.chars().next()?;
                hex = hex << 4 | digit.to_digit(16)?;
                digits = &digits[SECTION.len_utf8() + 1..];
            }
            *style = SpanStyle {
                color: Some(rgb(hex)),
                ..Default::default()
            };
            return Some(digits);
        },
        'n' => style.underline = true,
        'm' => style.strikethrough = true,
        'r' => *style = SpanStyle::default(),
        // Obfuscated, bold and italic. The log has the one font, in the one weight, so these are
        // dropped rather than drawn in a different one that throws out the columns
        'k' | 'l' | 'o' => {},
        _ => return None,
    }
    Some(rest)
}

/// The colour to draw a span in, nudged lighter or darker until it stands out from the log's
/// background. A server has no idea what that is, so black text is as likely as white.
///
/// `foreground` is the log's own text colour, which is light on a dark background and dark on a
/// light one.
pub fn readable(color: Rgba, foreground: Hsla) -> Hsla {
    // Relative luminance a span has to reach (or stay under), which is roughly 3.5:1 against the
    // near black or near white the themes use
    const MIN_ON_DARK: f32 = 0.2;
    const MAX_ON_LIGHT: f32 = 0.24;

    let dark_background = foreground.l > 0.5;
    let mut hsla = Hsla::from(color);
    for _ in 0..20 {
        let luminance = luminance(hsla.to_rgb());
        if dark_background && luminance >= MIN_ON_DARK || !dark_background && luminance <= MAX_ON_LIGHT {
            break;
        }
        hsla.l = if dark_background {
            (hsla.l + 0.05).min(1.0)
        } else {
            (hsla.l - 0.05).max(0.0)
        };
    }
    hsla
}

fn luminance(color: Rgba) -> f32 {
    let linear = |channel: f32| {
        if channel <= 0.04045 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use gpui::{hsla, rgb};

    use super::{Formatted, Span, SpanStyle, format_lines, luminance, readable};

    fn format(line: &str) -> (String, Vec<Span>) {
        let Formatted { text, spans } = format_lines(&Arc::from([Arc::from(line)]));
        (text[0].to_string(), spans.first().map(|spans| spans.to_vec()).unwrap_or_default())
    }

    fn colored(range: std::ops::Range<usize>, hex: u32) -> Span {
        Span {
            range,
            style: SpanStyle {
                color: Some(rgb(hex)),
                ..Default::default()
            },
        }
    }

    #[test]
    fn plain_lines_are_left_alone() {
        let lines: Arc<[Arc<str>]> = Arc::from([Arc::from("Done (4.867s)! For help, type \"help\"")]);
        let formatted = format_lines(&lines);
        assert!(Arc::ptr_eq(&formatted.text, &lines));
        assert!(formatted.spans.is_empty());
    }

    #[test]
    fn a_24_bit_colour_runs_to_the_reset() {
        // What Paper printed for GriefPrevention3D's startup banner
        let (text, spans) = format("\x1b[38;2;85;255;85m ___ \x1b[38;2;85;85;255m___\x1b[0m done");
        assert_eq!(text, " ___ ___ done");
        assert_eq!(spans, vec![colored(0..5, 0x55FF55), colored(5..8, 0x5555FF)]);
    }

    #[test]
    fn the_sixteen_ansi_colours_are_minecrafts() {
        let (text, spans) = format("\x1b[33mgold\x1b[39m \x1b[1;92mgreen\x1b[m");
        assert_eq!(text, "gold green");
        assert_eq!(spans, vec![colored(0..4, 0xFFAA00), colored(5..10, 0x55FF55)]);
    }

    #[test]
    fn the_256_colour_palette() {
        let (_, spans) = format("\x1b[38;5;196mred\x1b[38;5;244mgray\x1b[38;5;12mblue");
        assert_eq!(
            spans,
            vec![
                colored(0..3, 0xFF0000),
                colored(3..7, 0x808080),
                colored(7..11, 0x5555FF)
            ]
        );
    }

    #[test]
    fn a_background_is_stepped_over() {
        let (text, spans) = format("\x1b[48;2;1;2;3;31mred");
        assert_eq!(text, "red");
        assert_eq!(spans, vec![colored(0..3, 0xAA0000)]);
    }

    #[test]
    fn other_escapes_are_dropped() {
        let (text, spans) = format("\x1b]0;title\x07\x1b[2K\x1b(Bloading\x1b[?25h done\x1b");
        assert_eq!(text, "loading done");
        assert!(spans.is_empty());
    }

    #[test]
    fn section_codes() {
        let (text, spans) = format("§aGreen §lstill green§r plain §x§f§f§a§a§0§0hex");
        assert_eq!(text, "Green still green plain hex");
        assert_eq!(spans, vec![colored(0..17, 0x55FF55), colored(24..27, 0xFFAA00)]);
    }

    #[test]
    fn a_colour_code_ends_underlining() {
        let (_, spans) = format("§nunder§cred");
        assert_eq!(
            spans,
            vec![
                Span {
                    range: 0..5,
                    style: SpanStyle {
                        underline: true,
                        ..Default::default()
                    },
                },
                colored(5..8, 0xFF5555),
            ]
        );
    }

    #[test]
    fn a_section_sign_that_isnt_a_code_stays() {
        let (text, spans) = format("costs 5§ or §zso §");
        assert_eq!(text, "costs 5§ or §zso §");
        assert!(spans.is_empty());
    }

    #[test]
    fn a_colour_carries_onto_the_next_line_of_a_message() {
        let lines: Arc<[Arc<str>]> = Arc::from([Arc::from("\x1b[31mfirst"), Arc::from("second\x1b[0m third")]);
        let formatted = format_lines(&lines);
        assert_eq!(&*formatted.text[1], "second third");
        assert_eq!(formatted.spans[1].to_vec(), vec![colored(0..6, 0xAA0000)]);
    }

    #[test]
    fn colours_are_made_to_stand_out_from_the_background() {
        let dark_theme_text = hsla(0.0, 0.0, 0.9, 1.0);
        let light_theme_text = hsla(0.0, 0.0, 0.1, 1.0);

        assert!(luminance(readable(rgb(0x000000), dark_theme_text).to_rgb()) >= 0.2);
        assert!(luminance(readable(rgb(0x0000AA), dark_theme_text).to_rgb()) >= 0.2);
        assert!(luminance(readable(rgb(0xFFFFFF), light_theme_text).to_rgb()) <= 0.24);
        assert!(luminance(readable(rgb(0xFFFF55), light_theme_text).to_rgb()) <= 0.24);

        // Ones that already do are left as they are
        let green = readable(rgb(0x55FF55), dark_theme_text).to_rgb();
        assert!((green.g - 1.0).abs() < 0.01 && (green.r - 85.0 / 255.0).abs() < 0.01);
    }
}
