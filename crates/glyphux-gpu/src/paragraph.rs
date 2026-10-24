//! Paragraph layout: text in styled spans (each with its fonts and size), wrapped to a width
//! and in visual order, ready for placing glyphs from the atlas. What a label or a rich text
//! box needs between its text and its quads.
//!
//! * Hard breaks (`\n`) end paragraphs; each paragraph gets the Unicode bidi algorithm.
//! * Each span's text goes to the first of its fonts that can draw each character (font
//!   fallback, [`crate::shaping::split_runs_with`]), and is shaped in script runs.
//! * Lines break at the opportunities of UAX #14 (`unicode-linebreak`) where they would grow
//!   past the width; a word wider than the line breaks between its clusters. Trailing spaces
//!   hang past the line's end.
//! * Each line is shaped again on its own (shaping never joins across a break) and put in
//!   visual order (bidi rules L1 and L2), then aligned.

use std::ops::Range;

use unicode_bidi::BidiInfo;
use unicode_linebreak::{BreakOpportunity, linebreaks};

use crate::shaping::{ShapedGlyph, Shaper, ShapingFont};
use crate::text::Align;

/// A styled range of the text.
#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    /// Byte range in the text. Spans should cover the text; uncovered bytes use the first.
    pub range: Range<usize>,
    /// Index of the font list (primary font, then fallbacks) in [`layout`]'s `fonts`.
    pub fonts: usize,
    /// Pixels per em.
    pub size: f32,
    /// Carried to the span's glyphs: the caller's style (colors, effects, links...).
    pub style: u32,
}

/// How the paragraphs are set.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParagraphStyle {
    /// Width of the box (pixels) the lines are aligned in and, with `wrap`, wrapped at;
    /// `None`: the widest line's.
    pub width: Option<f32>,
    /// Whether lines wrap at `width`.
    pub wrap: bool,
    /// Alignment within `width` (or the widest line without one).
    pub align: Align,
    /// Distance between lines in multiples of their fonts' line height.
    pub line_spacing: f32,
    /// Paragraph direction: `None` detects it from each paragraph's first strong character.
    pub base_rtl: Option<bool>,
}

impl Default for ParagraphStyle {
    fn default() -> Self {
        ParagraphStyle {
            width: None,
            wrap: true,
            align: Align::Start,
            line_spacing: 1.0,
            base_rtl: None,
        }
    }
}

/// A glyph of the laid out text.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LaidGlyph {
    /// The font: index of the font list and of the font in it.
    pub fonts: usize,
    pub font: usize,
    pub glyph: u32,
    /// Where its origin goes (pixels, y down, from the top-left of the text box): the
    /// baseline of its line.
    pub x: f32,
    pub y: f32,
    /// Advance (pixels) and size (pixels per em).
    pub advance: f32,
    pub size: f32,
    /// Its span's style.
    pub style: u32,
    /// Byte offset in the text of its cluster.
    pub cluster: usize,
}

/// A laid out line.
#[derive(Clone, Debug, PartialEq)]
pub struct LaidLine {
    /// Byte range of the text.
    pub range: Range<usize>,
    /// Its glyphs in [`Paragraphs::glyphs`], in visual order.
    pub glyphs: Range<usize>,
    /// Left edge and width (pixels) of its ink advance, without trailing spaces.
    pub x: f32,
    pub width: f32,
    /// Top of the line box and its baseline (pixels, y down).
    pub top: f32,
    pub baseline: f32,
    /// Height of the line box.
    pub height: f32,
    pub rtl: bool,
}

/// Laid out text.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Paragraphs {
    pub glyphs: Vec<LaidGlyph>,
    pub lines: Vec<LaidLine>,
    /// Size of the box around the lines: the wrap width (or the widest line) by the lines'
    /// total height.
    pub width: f32,
    pub height: f32,
}

/// A piece of a line to shape: one span, one font, one direction, one script.
struct Item {
    range: Range<usize>,
    span: usize,
    font: usize,
}

/// Lays out `text` in `spans`. `fonts` are font lists (primary, then fallbacks) that spans
/// refer to; `can_draw(font, c)` says whether a font can draw a character (e.g. also
/// whether its color glyph is supported), for fallback.
pub fn layout(
    shaper: &mut Shaper,
    text: &str,
    spans: &[Span],
    fonts: &[&[&ShapingFont]],
    style: &ParagraphStyle,
    mut can_draw: impl FnMut(&ShapingFont, char) -> bool,
) -> Paragraphs {
    let mut out = Paragraphs::default();
    if spans.is_empty() || fonts.is_empty() {
        return out;
    }
    let mut top = 0.0f32;
    let mut start = 0;
    for para in text.split('\n') {
        let range = start..start + para.len();
        start = range.end + 1;
        top = layout_paragraph(shaper, text, range, spans, fonts, style, &mut can_draw, top, &mut out);
    }
    out.height = top;
    let widest = out.lines.iter().map(|l| l.width).fold(0.0, f32::max);
    out.width = style.width.unwrap_or(widest);
    // Alignment once the box is known.
    for line in &mut out.lines {
        let shift = match (style.align, line.rtl) {
            (Align::Start, false) | (Align::End, true) => 0.0,
            (Align::Start, true) | (Align::End, false) => out.width - line.width,
            (Align::Center, _) => 0.5 * (out.width - line.width),
        } - line.x;
        for g in &mut out.glyphs[line.glyphs.clone()] {
            g.x += shift;
        }
        line.x += shift;
    }
    out
}

/// The span that styles byte `i`.
fn span_at(spans: &[Span], i: usize) -> usize {
    spans.iter().position(|s| s.range.contains(&i)).unwrap_or(0)
}

#[allow(clippy::too_many_arguments)]
fn layout_paragraph(
    shaper: &mut Shaper,
    text: &str,
    para: Range<usize>,
    spans: &[Span],
    fonts: &[&[&ShapingFont]],
    style: &ParagraphStyle,
    can_draw: &mut impl FnMut(&ShapingFont, char) -> bool,
    mut top: f32,
    out: &mut Paragraphs,
) -> f32 {
    let t = &text[para.clone()];
    if t.is_empty() {
        // An empty paragraph still takes a line, in its span's size.
        let span = &spans[span_at(spans, para.start.min(text.len().saturating_sub(1)))];
        let font = fonts[span.fonts.min(fonts.len() - 1)][0];
        let (ascent, height) = metrics(font, span.size, style.line_spacing);
        let n = out.glyphs.len();
        out.lines.push(LaidLine {
            range: para.clone(),
            glyphs: n..n,
            x: 0.0,
            width: 0.0,
            top,
            baseline: top + ascent,
            height,
            rtl: false,
        });
        return top + height;
    }
    // Items in logical order: span, then font fallback within it.
    let mut items: Vec<Item> = Vec::new();
    let mut i = para.start;
    while i < para.end {
        let s = span_at(spans, i);
        let end = if spans[s].range.contains(&i) {
            spans[s].range.end.min(para.end)
        } else {
            // Uncovered bytes: up to the next span.
            spans
                .iter()
                .map(|sp| sp.range.start)
                .filter(|&b| b > i)
                .min()
                .unwrap_or(para.end)
                .min(para.end)
        };
        let list = fonts[spans[s].fonts.min(fonts.len() - 1)];
        let runs = crate::shaping::split_runs_with(list.len(), &text[i..end], |f, c| can_draw(list[f], c));
        for (f, r) in runs {
            items.push(Item {
                range: i + r.start..i + r.end,
                span: s,
                font: f,
            });
        }
        i = end;
    }
    let base = style.base_rtl.map(|r| {
        if r {
            unicode_bidi::Level::rtl()
        } else {
            unicode_bidi::Level::ltr()
        }
    });
    let bidi = BidiInfo::new(t, base);
    let Some(bpara) = bidi.paragraphs.first() else {
        return top;
    };
    let rtl = bpara.level.is_rtl();
    // Advance (pixels) per byte offset, measured on the whole paragraph shaped item by item.
    let mut advance = vec![0.0f32; t.len() + 1];
    let mut glyphs: Vec<ShapedGlyph> = Vec::new();
    for item in &items {
        let font = fonts[spans[item.span].fonts.min(fonts.len() - 1)][item.font];
        let size = spans[item.span].size;
        let local = item.range.start - para.start..item.range.end - para.start;
        for (piece, script) in shaper.script_runs(t, local) {
            let level_rtl = bidi.levels[piece.start].is_rtl();
            glyphs.clear();
            shaper.shape_piece(font, t, piece, 0..t.len(), level_rtl, script, 0.0, &mut glyphs);
            for g in &glyphs {
                advance[g.cluster as usize] += g.advance * size;
            }
        }
    }
    // Greedy line breaking.
    let width = style.width.filter(|_| style.wrap).unwrap_or(f32::INFINITY);
    let mut lines: Vec<Range<usize>> = Vec::new();
    let mut line_start = 0;
    let mut line_width = 0.0f32;
    let mut prev = 0;
    let seg_width = |r: Range<usize>| -> f32 { advance[r].iter().sum() };
    let trailing_space = |r: Range<usize>| -> f32 {
        let s = &t[r.clone()];
        let trimmed = s.trim_end_matches(char::is_whitespace);
        seg_width(r.start + trimmed.len()..r.end)
    };
    for (bo, kind) in linebreaks(t) {
        let seg = prev..bo;
        let w = seg_width(seg.clone());
        let visible = w - trailing_space(seg.clone());
        if line_width + visible > width && line_width > 0.0 {
            lines.push(line_start..prev);
            line_start = prev;
            line_width = 0.0;
        }
        if visible > width && line_width == 0.0 {
            // A word wider than the line: break between its clusters.
            let mut x = 0.0;
            let mut cut = seg.start;
            for (o, _) in t[seg.clone()].char_indices() {
                let o = seg.start + o;
                if x + advance[o] > width && o > cut {
                    lines.push(line_start..o);
                    line_start = o;
                    cut = o;
                    x = 0.0;
                }
                x += advance[o];
            }
            line_width = x;
        } else {
            line_width += w;
        }
        prev = bo;
        if kind == BreakOpportunity::Mandatory && bo < t.len() {
            lines.push(line_start..bo);
            line_start = bo;
            line_width = 0.0;
        }
    }
    if line_start < t.len() || lines.is_empty() {
        lines.push(line_start..t.len());
    }
    // Shape and order each line.
    for line in lines {
        let (levels, runs) = bidi.visual_runs(bpara, line.clone());
        let first = out.glyphs.len();
        let mut pen = 0.0f32;
        let (mut ascent, mut descent, mut height) = (0.0f32, 0.0f32, 0.0f32);
        for run in runs {
            let run_rtl = levels[run.start].is_rtl();
            let mut pieces: Vec<(Range<usize>, usize)> = items
                .iter()
                .enumerate()
                .filter_map(|(k, item)| {
                    let a = (item.range.start - para.start).max(run.start);
                    let b = (item.range.end - para.start).min(run.end);
                    (a < b).then_some((a..b, k))
                })
                .collect();
            if run_rtl {
                pieces.reverse();
            }
            for (piece, k) in pieces {
                let item = &items[k];
                let span = &spans[item.span];
                let font = fonts[span.fonts.min(fonts.len() - 1)][item.font];
                let (a, d, h) = line_metrics(font, span.size, style.line_spacing);
                ascent = ascent.max(a);
                descent = descent.max(d);
                height = height.max(h);
                let mut scripts = shaper.script_runs(t, piece);
                if run_rtl {
                    scripts.reverse();
                }
                for (r, script) in scripts {
                    glyphs.clear();
                    shaper.shape_piece(font, t, r, line.clone(), run_rtl, script, 0.0, &mut glyphs);
                    for g in &glyphs {
                        out.glyphs.push(LaidGlyph {
                            fonts: span.fonts.min(fonts.len() - 1),
                            font: item.font,
                            glyph: g.glyph,
                            x: pen + g.x * span.size,
                            y: -g.y * span.size,
                            advance: g.advance * span.size,
                            size: span.size,
                            style: span.style,
                            cluster: para.start + g.cluster as usize,
                        });
                    }
                    pen += glyphs.iter().map(|g| g.advance).sum::<f32>() * span.size;
                }
            }
        }
        // Trailing spaces hang: they sit at the logical end (visually right of a
        // left-to-right line, left of a right-to-left one).
        let hang = trailing_space(line.clone());
        let line_rtl = rtl;
        let (x, w) = if line_rtl {
            (hang, pen - hang)
        } else {
            (0.0, pen - hang)
        };
        let height = height.max(ascent + descent);
        let leading = 0.5 * (height - ascent - descent);
        let baseline = top + leading + ascent;
        let n = out.glyphs.len();
        for g in &mut out.glyphs[first..n] {
            g.y += baseline;
        }
        out.lines.push(LaidLine {
            range: para.start + line.start..para.start + line.end,
            glyphs: first..n,
            x,
            width: w.max(0.0),
            top,
            baseline,
            height,
            rtl: line_rtl,
        });
        top += height;
    }
    top
}

/// Ascent, descent (positive, below the baseline) and line box height of a font at `size`.
fn line_metrics(font: &ShapingFont, size: f32, spacing: f32) -> (f32, f32, f32) {
    let m = font.metrics();
    (
        m.ascender as f32 * size,
        -m.descender as f32 * size,
        m.line_height as f32 * size * spacing,
    )
}

fn metrics(font: &ShapingFont, size: f32, spacing: f32) -> (f32, f32) {
    let (a, d, h) = line_metrics(font, size, spacing);
    let h = h.max(a + d);
    (0.5 * (h - a - d) + a, h)
}
