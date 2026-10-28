//! What both classes share between their properties and [`TextDraw`]: the laid out text
//! (glyphux-gpu's paragraph layout, kept until the text, fonts or box change), the styles of
//! its spans, and drawing it at a scale.

use std::collections::HashMap;
use std::ops::Range;

use glyphux_gpu::ShapingFont;
use glyphux_gpu::paragraph::{self, ParagraphStyle, Paragraphs, Span};
use rivet::Obj;

use crate::FontFace;
use crate::bbcode::{self, Attrs};
use crate::draw::{Decoration, DrawData, Glyph, GlyphStyle, Rgba, TextDraw};
use crate::server::{FontChoice, Server};

/// A label's text, laid out, and what it draws.
#[derive(Default)]
pub struct LabelCore {
    pub draw: TextDraw,
    pub paragraphs: Paragraphs,
    fonts: Vec<Vec<FontChoice>>,
    pub styles: Vec<GlyphStyle>,
    /// Each style's link.
    pub metas: Vec<Option<String>>,
}

impl LabelCore {
    /// Lays out `text` in `spans`; `fonts` are font lists the spans refer to, `styles` what the
    /// spans' `style` indexes.
    pub fn layout(
        &mut self,
        s: &mut Server,
        text: &str,
        spans: &[Span],
        fonts: Vec<Vec<FontChoice>>,
        styles: Vec<GlyphStyle>,
        style: &ParagraphStyle,
    ) {
        // Fallback skips fonts whose color glyph for a character can't be compiled (checked
        // with the first span's color, which COLR layers may use).
        let foreground = spans
            .first()
            .and_then(|sp| styles.get(sp.style as usize))
            .map_or([255; 4], GlyphStyle::foreground);
        let lists: Vec<Vec<&ShapingFont>> = fonts.iter().map(|l| l.iter().map(|f| &*f.font).collect()).collect();
        let refs: Vec<&[&ShapingFont]> = lists.iter().map(Vec::as_slice).collect();
        // The shaper out of the server for the call: the fallback check uses the rest of it.
        let mut shaper = std::mem::take(&mut s.shaper);
        self.paragraphs = paragraph::layout(&mut shaper, text, spans, &refs, style, |font, c| {
            s.can_draw(font, c, foreground)
        });
        s.shaper = shaper;
        self.fonts = fonts;
        self.styles = styles;
    }

    /// Draws the laid out text at `scale` screen pixels per unit, its top-left at `offset`.
    pub fn draw_at(
        &mut self,
        s: &mut Server,
        scale: f32,
        offset: [f32; 2],
    ) -> (Option<DrawData>, Vec<glyphux_gpu::GlyphKey>) {
        let default = GlyphStyle::default();
        let mut glyphs = Vec::with_capacity(self.paragraphs.glyphs.len());
        // Each glyph's line and style, for the decorations.
        let mut placed: Vec<(usize, u32)> = Vec::with_capacity(self.paragraphs.glyphs.len());
        for (line_index, line) in self.paragraphs.lines.iter().enumerate() {
            for g in &self.paragraphs.glyphs[line.glyphs.clone()] {
                let Some(font) = self.fonts.get(g.fonts).and_then(|l| l.get(g.font)) else {
                    continue;
                };
                glyphs.push(Glyph {
                    font,
                    id: g.glyph,
                    origin: [offset[0] + g.x, offset[1] + g.y],
                    pen: g.x,
                    advance: g.advance,
                    size: g.size,
                    style: self.styles.get(g.style as usize).unwrap_or(&default),
                });
                placed.push((line_index, g.style));
            }
        }
        let decorations = decorations(&glyphs, &placed);
        self.draw.draw(s, &glyphs, &decorations, scale)
    }

    /// Moves the lines of paragraphs inside alignment tags.
    pub fn apply_aligns(&mut self, aligns: &[(Range<usize>, bbcode::Align)]) {
        let box_width = self.paragraphs.width;
        for line in &mut self.paragraphs.lines {
            let Some((_, a)) = aligns.iter().find(|(r, _)| r.contains(&line.range.start)) else {
                continue;
            };
            let x = match a {
                bbcode::Align::Left | bbcode::Align::Fill => 0.0,
                bbcode::Align::Center => 0.5 * (box_width - line.width),
                bbcode::Align::Right => box_width - line.width,
            };
            let shift = x - line.x;
            for g in &mut self.paragraphs.glyphs[line.glyphs.clone()] {
                g.x += shift;
            }
            line.x = x;
        }
    }

    /// The link under `position` (layout units, from the text's top-left).
    pub fn link_at(&self, position: [f32; 2]) -> Option<String> {
        let p = &self.paragraphs;
        let line = p
            .lines
            .iter()
            .find(|l| position[1] >= l.top && position[1] < l.top + l.height)?;
        let glyph = p.glyphs[line.glyphs.clone()]
            .iter()
            .find(|g| position[0] >= g.x && position[0] < g.x + g.advance)?;
        self.metas.get(glyph.style as usize).cloned().flatten()
    }
}

/// Underlines and strikethroughs: one per run of glyphs of one style in a line (`placed`:
/// each glyph's line and style).
fn decorations<'a>(glyphs: &[Glyph<'a>], placed: &[(usize, u32)]) -> Vec<Decoration<'a>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < glyphs.len() {
        let mut j = i + 1;
        while j < glyphs.len() && placed[j] == placed[i] {
            j += 1;
        }
        let run = &glyphs[i..j];
        let style = run[0].style;
        if style.underline || style.strike {
            let x0 = run.iter().map(|g| g.pen).fold(f32::INFINITY, f32::min);
            let x1 = run.iter().map(|g| g.pen + g.advance).fold(f32::NEG_INFINITY, f32::max);
            let size = run.iter().map(|g| g.size).fold(0.0, f32::max);
            let metrics = run[0].font.font.metrics();
            let under = metrics.underline_y as f32;
            let thickness = ((metrics.underline_thickness as f32).max(0.04) * size).max(1.0);
            let mut line = |y: f32, over: bool| {
                out.push(Decoration {
                    glyphs: i..j,
                    x: [x0, x1],
                    y,
                    thickness,
                    over,
                    style,
                });
            };
            if style.underline {
                line(-under * size, false);
            }
            if style.strike {
                line(-0.28 * size, true);
            }
        }
        i = j;
    }
    out
}

/// The fonts of a label: the normal one with its fallbacks, and those of `[b]`, `[i]`, both and
/// `[code]` (without one, the normal font at weight 700, slanted, or as it is).
#[derive(Clone, Default)]
pub struct Fonts {
    pub normal: Option<Obj<FontFace>>,
    pub fallbacks: Vec<Obj<FontFace>>,
    pub bold: Option<Obj<FontFace>>,
    pub italics: Option<Obj<FontFace>>,
    pub bold_italics: Option<Obj<FontFace>>,
    pub mono: Option<Obj<FontFace>>,
}

/// Slant of synthetic italics.
const SLANT: f32 = 0.2;

impl Fonts {
    /// The font, weight and slant of a run; `loaded` caches `[font=...]` assets.
    fn of(
        &self,
        s: &Server,
        a: &Attrs,
        loaded: &mut HashMap<String, Option<Obj<FontFace>>>,
    ) -> (Option<Obj<FontFace>>, Option<f32>, f32) {
        let slant = |italic: bool| if italic { SLANT } else { 0.0 };
        if let Some(path) = &a.font {
            let font = *loaded.entry(path.clone()).or_insert_with(|| {
                let f = s.load_font(path);
                if f.is_none() {
                    rivet::warn!("glyphux: [font={path}]: not a font face asset; using the label's");
                }
                f
            });
            if font.is_some() {
                return (font, a.bold.then_some(700.0), slant(a.italic));
            }
        }
        if a.mono && self.mono.is_some() {
            return (self.mono, a.bold.then_some(700.0), 0.0);
        }
        match (a.bold, a.italic) {
            (true, true) if self.bold_italics.is_some() => (self.bold_italics, None, 0.0),
            (true, _) if self.bold.is_some() => (self.bold, None, slant(a.italic)),
            (_, true) if self.italics.is_some() => (self.italics, a.bold.then_some(700.0), 0.0),
            (bold, italic) => (self.normal, bold.then_some(700.0), slant(italic)),
        }
    }
}

/// Text ready for [`LabelCore::layout`].
pub struct Content {
    pub text: String,
    pub spans: Vec<Span>,
    /// Font lists to resolve ([`resolve_fonts`]): a font and a weight (bits of `wght`).
    pub fonts: Vec<(Obj<FontFace>, Option<u32>)>,
    pub styles: Vec<GlyphStyle>,
    pub metas: Vec<Option<String>>,
    pub aligns: Vec<(Range<usize>, bbcode::Align)>,
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// A color named in markup, as BBCode writes them: a name or `#rgb`, `#rgba`, `#rrggbb`,
/// `#rrggbbaa` (sRGB), as a linear color.
pub fn parse_color(s: &str) -> Option<Rgba> {
    let s = s.trim();
    let srgb: [f32; 4] = if let Some(hex) = s.strip_prefix('#') {
        let digits: Vec<u32> = hex.chars().map(|c| c.to_digit(16)).collect::<Option<_>>()?;
        let byte = |i: usize| (digits[i] * 16 + digits[i + 1]) as f32 / 255.0;
        match digits.len() {
            3 | 4 => {
                let n = |i: usize| digits[i] as f32 / 15.0;
                [n(0), n(1), n(2), if digits.len() == 4 { n(3) } else { 1.0 }]
            }
            6 => [byte(0), byte(2), byte(4), 1.0],
            8 => [byte(0), byte(2), byte(4), byte(6)],
            _ => return None,
        }
    } else {
        let rgb: [u8; 3] = match s.to_ascii_lowercase().as_str() {
            "transparent" => return Some([0.0; 4]),
            "white" => [255, 255, 255],
            "black" => [0, 0, 0],
            "red" => [255, 0, 0],
            "green" => [0, 255, 0],
            "blue" => [0, 0, 255],
            "yellow" => [255, 255, 0],
            "cyan" | "aqua" => [0, 255, 255],
            "magenta" | "fuchsia" => [255, 0, 255],
            "gray" | "grey" => [190, 190, 190],
            "orange" => [255, 165, 0],
            "purple" => [160, 32, 240],
            "pink" => [255, 192, 203],
            "brown" => [165, 42, 42],
            "lime" => [50, 205, 50],
            "gold" => [255, 215, 0],
            "silver" => [192, 192, 192],
            "navy" => [0, 0, 128],
            "teal" => [0, 128, 128],
            _ => return None,
        };
        [
            f32::from(rgb[0]) / 255.0,
            f32::from(rgb[1]) / 255.0,
            f32::from(rgb[2]) / 255.0,
            1.0,
        ]
    };
    Some([
        srgb_to_linear(srgb[0]),
        srgb_to_linear(srgb[1]),
        srgb_to_linear(srgb[2]),
        srgb[3],
    ])
}

fn named_color(name: Option<&String>, default: Rgba) -> Rgba {
    name.and_then(|n| parse_color(n)).unwrap_or(default)
}

/// Turns parsed BBCode into spans and styles: `base` is the style of untagged text, `size` its
/// size and `wght` its weight (`wght` units); the tags override them per run.
#[allow(clippy::too_many_arguments)]
pub fn rich_content(
    s: &Server,
    parsed: bbcode::Parsed,
    fonts: &Fonts,
    base: &GlyphStyle,
    size: f32,
    wght: Option<f32>,
    underline_links: bool,
    loaded: &mut HashMap<String, Option<Obj<FontFace>>>,
) -> Content {
    let mut out = Content {
        text: parsed.text,
        spans: Vec::new(),
        fonts: Vec::new(),
        styles: Vec::new(),
        metas: Vec::new(),
        aligns: parsed.aligns,
    };
    for (range, a) in parsed.runs {
        let (font, run_wght, slant) = fonts.of(s, &a, loaded);
        let Some(font) = font.or(fonts.normal) else {
            continue;
        };
        let wght = run_wght.or(wght);
        let key = (font, wght.map(f32::to_bits));
        let set = out.fonts.iter().position(|k| *k == key).unwrap_or_else(|| {
            out.fonts.push(key);
            out.fonts.len() - 1
        });
        let color = named_color(a.color.as_ref(), base.color);
        out.styles.push(GlyphStyle {
            color,
            outline: a
                .outline_size
                .map(|w| (named_color(a.outline_color.as_ref(), [0.0, 0.0, 0.0, 1.0]), w))
                .or(base.outline),
            glow: a
                .glow
                .as_ref()
                .map(|(c, r)| (named_color(c.as_ref(), color), *r))
                .or(base.glow),
            shadow: a
                .shadow
                .as_ref()
                .map(|(c, o, b)| (named_color(c.as_ref(), [0.0, 0.0, 0.0, 0.6]), *o, *b))
                .or(base.shadow),
            slant: slant + base.slant,
            underline: a.underline || (a.url.is_some() && underline_links),
            strike: a.strike,
            // As Godot's tags: wave and shake amplitudes in tenths of a pixel, wave frequency
            // in radians per second.
            wave: a.wave.map(|(amp, freq)| (amp / 10.0, freq / std::f32::consts::TAU)),
            shake: a.shake.map(|(rate, level)| (level / 10.0, rate)),
            rainbow: a.rainbow.map(|(freq, sat, val, speed)| (speed, sat, val, freq)),
            weight: base.weight,
        });
        out.metas.push(a.url.clone());
        out.spans.push(Span {
            range,
            fonts: set,
            size: a.size.unwrap_or(size).max(0.01),
            style: (out.styles.len() - 1) as u32,
        });
    }
    out
}

/// Plain text as content: one span in the label's font and style; `wght` its weight.
pub fn plain_content(text: &str, font: Obj<FontFace>, wght: Option<f32>, style: &GlyphStyle, size: f32) -> Content {
    Content {
        text: text.to_string(),
        spans: vec![Span {
            range: 0..text.len(),
            fonts: 0,
            size: size.max(0.01),
            style: 0,
        }],
        fonts: vec![(font, wght.map(f32::to_bits))],
        styles: vec![style.clone()],
        metas: vec![None],
        aligns: Vec::new(),
    }
}

/// The font lists of [`Content::fonts`] (each font followed by the label's fallbacks); `None`
/// if one has no usable font.
pub fn resolve_fonts(
    s: &mut Server,
    keys: &[(Obj<FontFace>, Option<u32>)],
    fallbacks: &[Obj<FontFace>],
) -> Option<Vec<Vec<FontChoice>>> {
    let fonts: Vec<_> = keys
        .iter()
        .map(|(f, w)| s.fonts(*f, fallbacks, w.map(f32::from_bits)))
        .collect();
    (!fonts.is_empty() && !fonts.iter().any(Vec::is_empty)).then_some(fonts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors() {
        assert_eq!(parse_color("white"), Some([1.0; 4]));
        assert_eq!(parse_color("#000"), Some([0.0, 0.0, 0.0, 1.0]));
        let c = parse_color("#ff800080").unwrap();
        assert!((c[1] - srgb_to_linear(128.0 / 255.0)).abs() < 1e-6);
        assert!((c[3] - 128.0 / 255.0).abs() < 1e-6);
        assert_eq!(parse_color("nope"), None);
        assert_eq!(parse_color("#12345"), None);
    }
}
