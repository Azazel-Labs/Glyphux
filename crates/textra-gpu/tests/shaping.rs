//! Shaping with the committed demo fonts (no GPU needed).
use std::sync::Arc;

use textra_gpu::{ShapedLine, Shaper, ShapingFont};

fn font(id: u32, file: &str) -> ShapingFont {
    let path = format!("{}/../../examples/web-demo/assets/{file}", env!("CARGO_MANIFEST_DIR"));
    ShapingFont::new(id, Arc::new(std::fs::read(path).unwrap()), 0).unwrap()
}

fn glyphs(line: &ShapedLine) -> Vec<(u32, u32, i32, i32)> {
    let q = |v: f32| (v * 1e4).round() as i32;
    line.glyphs
        .iter()
        .map(|g| (g.glyph, g.cluster, q(g.x), q(g.y)))
        .collect()
}

/// A shaper reusing its compiled plans across fonts, scripts and feature changes shapes
/// exactly like a fresh one.
#[test]
fn reused_plans_shape_like_fresh_ones() {
    let latin = font(0, "Inter-Regular.otf");
    let arabic = font(1, "NotoSansArabic-Subset.ttf");
    let bold = font(2, "InterVariable-Latin.ttf").with_variations(3, &[("wght", 800.0)]);
    let lines: [(&ShapingFont, &str); 5] = [
        (&latin, "Ta->b office AVATAR"),
        (&arabic, "تكبير وانزلاق"),
        (&latin, "Mixed 123 text"),
        (&bold, "Ta->b office AVATAR"),
        (&latin, "Ta->b office AVATAR"),
    ];
    let mut warm = Shaper::new();
    for features in [&[][..], &["-calt".parse().unwrap(), "-kern".parse().unwrap()][..]] {
        warm.features = features.to_vec();
        for round in 0..2 {
            for (f, text) in lines {
                let mut fresh = Shaper::new();
                fresh.features = features.to_vec();
                let expected = glyphs(&fresh.shape_line(f, text));
                assert_eq!(glyphs(&warm.shape_line(f, text)), expected, "{text:?}, round {round}");
            }
        }
    }
    // The features matter (so a stale plan would show): "->" is one glyph only with calt.
    warm.features.clear();
    let with = warm.shape_line(&latin, "->").glyphs.len();
    warm.features = vec!["-calt".parse().unwrap()];
    assert_eq!((with, warm.shape_line(&latin, "->").glyphs.len()), (1, 2));
}

#[test]
fn has_glyph_and_metrics() {
    let latin = font(0, "Inter-Regular.otf");
    let arabic = font(1, "NotoSansArabic-Subset.ttf");
    assert!(latin.has_glyph('A') && latin.has_glyph('é') && !latin.has_glyph('ت'));
    assert!(arabic.has_glyph('ت') && !arabic.has_glyph('永'));
    let m = latin.metrics();
    assert!(m.line_height > 1.0 && m.line_height < 1.5, "{m:?}");
}
