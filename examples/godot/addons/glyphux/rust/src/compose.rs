//! What the two labels share between their properties and [`TextDraw`]: the laid out text
//! (glyphux-gpu's paragraph layout, kept until the text, fonts or box change), the styles
//! of its spans, and drawing it at the node's current scale, in lines or along a curve.

use std::collections::HashMap;
use std::ops::Range;

use glyphux_wgpu::ShapingFont;
use glyphux_wgpu::glyphux_gpu::paragraph::{self, ParagraphStyle, Paragraphs, Span};
use godot::classes::{CanvasItem, Curve2D, Font};
use godot::prelude::*;
use godot::tools::try_load;

use crate::bbcode::{self, Attrs};
use crate::server::{self, FontChoice, Server};
use crate::text_draw::{Decoration, Glyph, GlyphStyle, TextDraw};

/// Where text goes along a curve.
pub struct CurvePlacement {
    pub curve: Gd<Curve2D>,
    /// Distance along the curve where the text box starts.
    pub offset: f32,
    /// Wraps around the curve's end.
    pub wrap: bool,
}

/// A label's text, laid out, and its meshes.
#[derive(Default)]
pub struct LabelCore {
    pub draw: TextDraw,
    pub paragraphs: Paragraphs,
    fonts: Vec<Vec<FontChoice>>,
    pub styles: Vec<GlyphStyle>,
    /// The node's own scale when last drawn (transform changes that keep it need nothing).
    pub node_scale: f32,
}

/// Screen pixels per unit of `node`: its global scale times its canvas' scale.
pub fn screen_scale(node: &Gd<CanvasItem>) -> (f32, f32) {
    let t = node.get_global_transform();
    let own = t.a.length().max(t.b.length()).max(1e-6);
    let canvas = server::canvas_scale(node);
    (own, own * canvas)
}

impl LabelCore {
    /// A label drawing with `draw` (e.g. into a 3D mesh).
    pub fn with_draw(draw: TextDraw) -> Self {
        LabelCore {
            draw,
            ..LabelCore::default()
        }
    }

    /// Lays out `text` in `spans`; `fonts` are font lists the spans refer to, `styles`
    /// what the spans' `style` indexes.
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
        let c = spans
            .first()
            .and_then(|sp| styles.get(sp.style as usize))
            .map_or(Color::WHITE, |st| st.color);
        let foreground = [c.r, c.g, c.b, c.a].map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
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

    /// Draws the laid out text of a 2D label in `node`, its top-left at `offset` or along
    /// `curve`, and registers it with the server.
    pub fn render(&mut self, s: &mut Server, node: &Gd<CanvasItem>, offset: Vector2, curve: Option<&CurvePlacement>) {
        let (own, scale) = screen_scale(node);
        self.node_scale = own;
        let keys = self.draw_at(s, scale, offset, curve);
        s.register(node.instance_id(), node.get_canvas(), scale / own, keys);
    }

    /// Draws the laid out text at `scale` screen pixels per unit, its top-left at `offset`
    /// or along `curve`. Returns the glyphs shown.
    pub fn draw_at(
        &mut self,
        s: &mut Server,
        scale: f32,
        offset: Vector2,
        curve: Option<&CurvePlacement>,
    ) -> Vec<glyphux_wgpu::GlyphKey> {
        let default = GlyphStyle::default();
        let first_baseline = self.paragraphs.lines.first().map_or(0.0, |l| l.baseline);
        let length = curve.map_or(0.0, |c| c.curve.get_baked_length());
        let mut glyphs = Vec::with_capacity(self.paragraphs.glyphs.len());
        // Each glyph's line and style, for the decorations.
        let mut placed: Vec<(usize, u32)> = Vec::with_capacity(self.paragraphs.glyphs.len());
        for (line_index, line) in self.paragraphs.lines.iter().enumerate() {
            for g in &self.paragraphs.glyphs[line.glyphs.clone()] {
                let Some(font) = self.fonts.get(g.fonts).and_then(|l| l.get(g.font)) else {
                    continue;
                };
                let style = self.styles.get(g.style as usize).unwrap_or(&default);
                let (origin, rotation) = match curve {
                    None => ([offset.x + g.x, offset.y + g.y], None),
                    Some(c) => {
                        // The glyph's baseline midpoint sits on the curve; the quad turns about
                        // it. Later lines follow a line height further out.
                        let mid = c.offset + offset.x + g.x + 0.5 * g.advance;
                        let at = if c.wrap { mid.rem_euclid(length.max(1e-3)) } else { mid };
                        if !(0.0..=length).contains(&at) {
                            continue;
                        }
                        let t = c.curve.sample_baked_with_rotation_ex().offset(at).done();
                        let p = [t.origin.x, t.origin.y];
                        let below = g.y - first_baseline;
                        ([p[0] - 0.5 * g.advance, p[1] + below], Some((t.rotation(), p)))
                    }
                };
                glyphs.push(Glyph {
                    font,
                    id: g.glyph,
                    origin,
                    pen: g.x,
                    advance: g.advance,
                    size: g.size,
                    rotation,
                    style,
                });
                placed.push((line_index, g.style));
            }
        }
        let decorations = decorations(&glyphs, &placed);
        self.draw.draw(s, &glyphs, &decorations, scale)
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

/// The fonts of rich text: the normal one, and those of `[b]`, `[i]`, both and `[code]`
/// (without one, the normal font at weight 700, slanted, or as it is).
#[derive(Clone, Default)]
pub struct RichFonts {
    pub normal: Option<Gd<Font>>,
    pub bold: Option<Gd<Font>>,
    pub italics: Option<Gd<Font>>,
    pub bold_italics: Option<Gd<Font>>,
    pub mono: Option<Gd<Font>>,
}

/// Slant of synthetic italics.
const SLANT: f32 = 0.2;

impl RichFonts {
    /// The font, weight and slant of a run; `loaded` caches `[font=...]` resources.
    fn of(&self, a: &Attrs, loaded: &mut HashMap<String, Option<Gd<Font>>>) -> (Option<Gd<Font>>, Option<f32>, f32) {
        let slant = |italic: bool| if italic { SLANT } else { 0.0 };
        if let Some(path) = &a.font {
            let font = loaded
                .entry(path.clone())
                .or_insert_with(|| {
                    try_load::<Font>(path.as_str())
                        .inspect_err(|e| {
                            godot_warn!("glyphux: [font={path}]: not a font ({e}); using the label's");
                        })
                        .ok()
                })
                .clone();
            if font.is_some() {
                return (font, a.bold.then_some(700.0), slant(a.italic));
            }
        }
        if a.mono && self.mono.is_some() {
            return (self.mono.clone(), a.bold.then_some(700.0), 0.0);
        }
        match (a.bold, a.italic) {
            (true, true) if self.bold_italics.is_some() => (self.bold_italics.clone(), None, 0.0),
            (true, _) if self.bold.is_some() => (self.bold.clone(), None, slant(a.italic)),
            (_, true) if self.italics.is_some() => (self.italics.clone(), a.bold.then_some(700.0), 0.0),
            (bold, italic) => (self.normal.clone(), bold.then_some(700.0), slant(italic)),
        }
    }
}

/// Rich text ready for [`LabelCore::layout`].
pub struct RichContent {
    pub text: String,
    pub spans: Vec<Span>,
    /// Font lists to resolve ([`resolve_fonts`]): a font and a weight (bits of `wght`).
    pub fonts: Vec<(Gd<Font>, Option<u32>)>,
    pub styles: Vec<GlyphStyle>,
    /// Each style's link.
    pub metas: Vec<Option<String>>,
    pub aligns: Vec<(Range<usize>, bbcode::Align)>,
}

fn named_color(name: Option<&String>, default: Color) -> Color {
    name.and_then(|n| Color::from_string(n.as_str())).unwrap_or(default)
}

/// Turns parsed BBCode into spans and styles: `base` is the style of untagged text, `size`
/// its size; `[font_size]`, `[color]` and the effect tags override them per run.
pub fn rich_content(
    parsed: bbcode::Parsed,
    fonts: &RichFonts,
    base: &GlyphStyle,
    size: f32,
    underline_links: bool,
    loaded: &mut HashMap<String, Option<Gd<Font>>>,
) -> RichContent {
    let mut out = RichContent {
        text: parsed.text,
        spans: Vec::new(),
        fonts: Vec::new(),
        styles: Vec::new(),
        metas: Vec::new(),
        aligns: parsed.aligns,
    };
    for (range, a) in parsed.runs {
        let (font, wght, slant) = fonts.of(&a, loaded);
        let Some(font) = font.or_else(|| fonts.normal.clone()) else {
            continue;
        };
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
                .map(|w| (named_color(a.outline_color.as_ref(), Color::BLACK), w))
                .or(base.outline),
            glow: a
                .glow
                .as_ref()
                .map(|(c, r)| (named_color(c.as_ref(), color), *r))
                .or(base.glow),
            shadow: a
                .shadow
                .as_ref()
                .map(|(c, o, b)| {
                    let shadow = named_color(c.as_ref(), Color::from_rgba(0.0, 0.0, 0.0, 0.6));
                    (shadow, Vector2::new(o[0], o[1]), *b)
                })
                .or(base.shadow),
            slant: slant + base.slant,
            underline: a.underline || (a.url.is_some() && underline_links),
            strike: a.strike,
            // As Godot's tags: wave and shake amplitudes in tenths of a pixel, wave
            // frequency in radians per second.
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

/// The font lists of [`RichContent::fonts`]; `None` if one has no usable font.
pub fn resolve_fonts(s: &mut Server, keys: &[(Gd<Font>, Option<u32>)]) -> Option<Vec<Vec<FontChoice>>> {
    let fonts: Vec<_> = keys.iter().map(|(f, w)| s.fonts(f, w.map(f32::from_bits))).collect();
    (!fonts.is_empty() && !fonts.iter().any(Vec::is_empty)).then_some(fonts)
}

impl LabelCore {
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
}
