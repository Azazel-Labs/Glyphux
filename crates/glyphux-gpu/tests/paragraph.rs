//! Paragraph layout: breaking, wrapping, mixed sizes, fallback with bidi, alignment.
#![cfg(feature = "shaping")]
use std::sync::Arc;

use glyphux_gpu::paragraph::{ParagraphStyle, Paragraphs, Span, layout};
use glyphux_gpu::text::Align;
use glyphux_gpu::{Shaper, ShapingFont};

fn font(id: u32, name: &str) -> ShapingFont {
    let path = format!("{}/../../examples/web-demo/assets/{name}", env!("CARGO_MANIFEST_DIR"));
    ShapingFont::new(id, Arc::new(std::fs::read(path).unwrap()), 0).unwrap()
}

fn lay(text: &str, spans: &[Span], style: ParagraphStyle) -> Paragraphs {
    let latin = font(0, "InterVariable-Latin.ttf");
    let arabic = font(1, "NotoSansArabic-Subset.ttf");
    let list: &[&ShapingFont] = &[&latin, &arabic];
    layout(&mut Shaper::new(), text, spans, &[list], &style, ShapingFont::has_glyph)
}

fn one(text: &str, size: f32) -> Vec<Span> {
    vec![Span {
        range: 0..text.len(),
        fonts: 0,
        size,
        style: 0,
    }]
}

#[test]
fn hard_breaks_make_lines() {
    let text = "first\n\nthird";
    let p = lay(text, &one(text, 20.0), ParagraphStyle::default());
    assert_eq!(p.lines.len(), 3);
    assert_eq!(p.lines[1].glyphs.len(), 0, "the empty line has no glyphs");
    assert!(p.lines[2].baseline > p.lines[1].baseline && p.lines[1].baseline > p.lines[0].baseline);
}

#[test]
fn wraps_at_spaces_within_the_width() {
    let text = "Sphinx of black quartz, judge my vow.";
    let style = ParagraphStyle {
        width: Some(120.0),
        ..ParagraphStyle::default()
    };
    let p = lay(text, &one(text, 20.0), style);
    assert!(p.lines.len() > 2, "{} lines", p.lines.len());
    let mut joined = String::new();
    for line in &p.lines {
        assert!(line.width <= 120.0 + 1e-3, "{} wide", line.width);
        let s = &text[line.range.clone()];
        assert!(!s.starts_with(' '), "line starts with a space: {s:?}");
        joined.push_str(s);
    }
    assert_eq!(joined, text, "lines cover the text");
}

#[test]
fn breaks_long_words_between_clusters() {
    let text = "Supercalifragilisticexpialidocious";
    let style = ParagraphStyle {
        width: Some(60.0),
        ..ParagraphStyle::default()
    };
    let p = lay(text, &one(text, 20.0), style);
    assert!(p.lines.len() > 3);
    assert!(p.lines.iter().all(|l| l.width <= 60.0 + 1e-3));
}

#[test]
fn mixed_sizes_share_a_line() {
    let text = "small BIG";
    let spans = [
        Span {
            range: 0..6,
            fonts: 0,
            size: 10.0,
            style: 1,
        },
        Span {
            range: 6..9,
            fonts: 0,
            size: 40.0,
            style: 2,
        },
    ];
    let p = lay(text, &spans, ParagraphStyle::default());
    assert_eq!(p.lines.len(), 1);
    let big = p.glyphs.iter().filter(|g| g.style == 2).count();
    assert_eq!(big, 3);
    assert!(
        p.glyphs
            .iter()
            .all(|g| g.size == if g.style == 2 { 40.0 } else { 10.0 })
    );
    // The line is as tall as its biggest text needs.
    assert!(p.lines[0].height >= 40.0);
    // Glyphs advance left to right.
    assert!(p.glyphs.windows(2).all(|w| w[1].x >= w[0].x));
}

#[test]
fn fallback_and_bidi_order() {
    let text = "abc تكبير def";
    let p = lay(text, &one(text, 20.0), ParagraphStyle::default());
    assert!(!p.lines[0].rtl);
    let arabic: Vec<_> = p.glyphs.iter().filter(|g| g.font == 1).collect();
    assert!(!arabic.is_empty(), "Arabic falls back to the second font");
    let latin_x = |c: usize| p.glyphs.iter().find(|g| g.cluster == c).unwrap().x;
    let (abc, def) = (latin_x(0), latin_x(text.find("def").unwrap()));
    assert!(
        arabic.iter().all(|g| g.x > abc && g.x < def),
        "Arabic between abc and def"
    );
    // Right to left inside: later clusters further left.
    assert!(arabic.windows(2).all(|w| w[0].cluster >= w[1].cluster));
}

#[test]
fn centers_lines() {
    let text = "centered";
    let style = ParagraphStyle {
        width: Some(300.0),
        align: Align::Center,
        ..ParagraphStyle::default()
    };
    let p = lay(text, &one(text, 20.0), style);
    let l = &p.lines[0];
    assert!((l.x - 0.5 * (300.0 - l.width)).abs() < 1e-3);
    assert_eq!(p.width, 300.0);
}

#[test]
fn aligns_without_wrapping() {
    let text = "Sphinx of black quartz, judge my vow.";
    let style = ParagraphStyle {
        width: Some(100.0),
        wrap: false,
        align: Align::End,
        ..ParagraphStyle::default()
    };
    let p = lay(text, &one(text, 20.0), style);
    assert_eq!(p.lines.len(), 1);
    let l = &p.lines[0];
    assert!(
        (l.x + l.width - 100.0).abs() < 1e-3,
        "right-aligned past the box's left edge"
    );
}
