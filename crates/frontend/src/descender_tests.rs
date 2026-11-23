//! Regression tests for descenders being cut off (the tails of g, j, p, q, y).
//!
//! It has happened three times: button labels, checkbox labels, and names in the settings lists.
//! Each time the cause was the same: an element clips to its own box, and the text in it has a line
//! box shorter than its glyphs. GPUI centres the glyphs on the line box, and Inter's ascent plus
//! descent is 1.21em, so in a 1em line box the glyphs reach 0.1em past the top and bottom.
//! `truncate()` clips, and the settings lists use a 16px line box for 16px text.
//!
//! The fix for `truncate()` is in GPUI's clip calculation (patches/gpui/truncated-text-clip.patch).
//! These tests check it against the real metrics of the fonts the launcher ships, and check that
//! every vendored fix is still applied after `cargo vendor` regenerates vendor/. The first
//! descender fix shipped without effect because of that.

use std::path::{Path, PathBuf};

use gpui::{Bounds, Div, Pixels, Refineable, Style, Styled, div, point, px, size};

/// Font sizes from text_xs to text_3xl.
const FONT_SIZES: [f32; 7] = [12.0, 14.0, 16.0, 18.0, 20.0, 24.0, 30.0];
const REM: f32 = 16.0;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn bundled_fonts() -> Vec<(String, Vec<u8>)> {
    let mut fonts = Vec::new();
    let mut dirs = vec![repo_root().join("assets/fonts")];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
            } else if path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("ttf") || ext.eq_ignore_ascii_case("otf"))
            {
                fonts.push((path.file_name().unwrap().to_string_lossy().into_owned(), std::fs::read(&path).unwrap()));
            }
        }
    }
    assert!(
        fonts.iter().any(|(name, _)| name == "Inter-Regular.ttf"),
        "Inter should be among {fonts:?}",
        fonts = fonts.iter().map(|(name, _)| name).collect::<Vec<_>>()
    );
    fonts
}

/// A font's vertical metrics in em, ascent plus descent.
struct Extents {
    /// What text is laid out with: hhea, or OS/2 typo when the font asks for it. GPUI centres
    /// this on the line box.
    layout: f32,
    /// The tallest any table claims, OS/2 win included, which also covers accented capitals.
    tallest: f32,
}

fn extents(font: &[u8]) -> Extents {
    let u16_at = |offset: usize| u16::from_be_bytes([font[offset], font[offset + 1]]);
    let i16_at = |offset: usize| u16_at(offset) as i16 as f32;
    let u32_at = |offset: usize| u32::from_be_bytes(font[offset..offset + 4].try_into().unwrap());

    let table = |tag: &[u8; 4]| {
        (0..u16_at(4) as usize)
            .map(|index| 12 + index * 16)
            .find(|record| &font[*record..*record + 4] == tag)
            .map(|record| u32_at(record + 8) as usize)
    };

    let head = table(b"head").expect("head table");
    let units_per_em = u16_at(head + 18) as f32;

    let hhea = table(b"hhea").expect("hhea table");
    let mut layout = i16_at(hhea + 4) - i16_at(hhea + 6);
    let mut tallest = layout;
    if let Some(os2) = table(b"OS/2") {
        let typo = i16_at(os2 + 68) - i16_at(os2 + 70);
        let win = u16_at(os2 + 74) as f32 + u16_at(os2 + 76) as f32;
        let use_typo_metrics = u16_at(os2 + 62) & 0x80 != 0;
        if use_typo_metrics {
            layout = typo;
        }
        tallest = tallest.max(typo).max(win);
    }
    Extents {
        layout: layout / units_per_em,
        tallest: tallest / units_per_em,
    }
}

fn style_of(mut element: Div) -> Style {
    let mut style = Style::default();
    style.refine(element.style());
    style
}

#[test]
fn inter_glyphs_are_taller_than_one_em() {
    // The premise of this file: if this ever fails, the bundled font changed and the line heights
    // patched in for it (1.25em) need another look
    let inter = bundled_fonts().into_iter().find(|(name, _)| name == "Inter-Regular.ttf").unwrap().1;
    let extent = extents(&inter).layout;
    assert!((1.15..1.25).contains(&extent), "Inter's glyphs are {extent}em tall");
}

#[test]
fn truncated_text_keeps_its_descenders() {
    let style = style_of(div().truncate());

    for (font, bytes) in bundled_fonts() {
        // The tallest metric, so accented capitals are covered as well as descenders
        let extent_em = extents(&bytes).tallest;
        for font_size in FONT_SIZES {
            // A line box one em tall, as relative(1.0) gives, and one rem tall, as the settings
            // lists use whatever the text size is
            for line_height in [font_size, REM] {
                let bounds = Bounds::new(point(px(40.0), px(100.0)), size(px(120.0), px(line_height)));
                let mask = style.overflow_mask(bounds, px(REM)).expect("truncated text still clips").bounds;

                // Centred on the line box, as GPUI lays glyphs out
                let glyphs = font_size * extent_em;
                let glyph_top = 100.0 + (line_height - glyphs) / 2.0;
                let glyph_bottom = glyph_top + glyphs;

                let what = format!("{font} at {font_size}px in a {line_height}px line box");
                assert!(f32::from(mask.top()) <= glyph_top, "ascenders clipped: {what}");
                assert!(f32::from(mask.bottom()) >= glyph_bottom, "descenders clipped: {what}");
                // Still clipped across, or the ellipsis would have nothing to do
                assert_eq!((mask.left(), mask.right()), (bounds.left(), bounds.right()), "{what}");
            }
        }
    }
}

#[test]
fn the_settings_player_name_keeps_its_descenders() {
    // The report: "ggpots" in Settings > Servers > Default players, 16px Inter in the 16px line
    // box every settings group sets, inside a `truncate()`
    let inter = bundled_fonts().into_iter().find(|(name, _)| name == "Inter-Regular.ttf").unwrap().1;
    let glyphs = 16.0 * extents(&inter).tallest;
    let bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(200.0), px(16.0)));
    let mask = style_of(div().truncate()).overflow_mask(bounds, px(REM)).unwrap().bounds;
    assert!(f32::from(mask.bottom()) >= 8.0 + glyphs / 2.0);
}

#[test]
fn plain_clipping_still_clips_to_the_box() {
    // Only ellipsis-truncated text gets slack; a box that hides its overflow still does
    let bounds: Bounds<Pixels> = Bounds::new(point(px(40.0), px(100.0)), size(px(120.0), px(16.0)));
    let mask = style_of(div().overflow_hidden()).overflow_mask(bounds, px(REM)).unwrap().bounds;
    assert_eq!(mask, bounds);
}

/// The added lines of every hunk in every vendored fix under patches/, with the file each belongs
/// to inside its crate.
fn patch_additions(patch: &str) -> Vec<(String, String)> {
    let mut additions = Vec::new();
    let mut target = None;
    for line in patch.lines() {
        if let Some(file) = line.strip_prefix("+++ b/") {
            target = Some(file.trim().to_string());
        } else if let Some(added) = line.strip_prefix('+')
            && let Some(target) = &target
            && !added.trim().is_empty()
        {
            additions.push((target.clone(), added.to_string()));
        }
    }
    additions
}

#[test]
fn vendored_fixes_are_applied() {
    let patches = repo_root().join("patches");
    let mut checked = 0;
    for crate_dir in std::fs::read_dir(&patches).unwrap() {
        let crate_dir = crate_dir.unwrap().path();
        let Ok(entries) = std::fs::read_dir(&crate_dir) else {
            continue;
        };
        for patch in entries {
            let patch = patch.unwrap().path();
            if patch.extension().is_none_or(|ext| ext != "patch") {
                continue;
            }
            let vendored = repo_root().join("vendor").join(crate_dir.file_name().unwrap());
            for (file, added) in patch_additions(&std::fs::read_to_string(&patch).unwrap()) {
                let contents = std::fs::read_to_string(vendored.join(&file)).unwrap();
                assert!(
                    contents.lines().any(|line| line == added),
                    "{} isn't applied to vendor/{}/{file} (run scripts/patch-vendor.py): missing {added:?}",
                    patch.display(),
                    crate_dir.file_name().unwrap().to_string_lossy(),
                );
                checked += 1;
            }
        }
    }
    assert!(checked > 0, "no patches found under {}", patches.display());
}
