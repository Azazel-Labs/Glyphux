//! What the labels draw: glyphs placed in layout units, each with its own style, turned into
//! quads and the glyph records the plugin's materials read (`GlyphuxMaterial.ush`), the same
//! records as the Godot addon's.
//!
//! * Monochrome glyphs: a record of [`RECORD_TEXELS`] float texels each (its
//!   `GlyphInstance`, its atlas page, its animation), `RECORDS_PER_ROW` per row of the
//!   records texture. Underlines and strikethroughs are records too, with no atlas field:
//!   the material takes the exact distance to their box.
//! * Color glyphs: four texels each in the tables texture (`TABLES_WIDTH` per row), then
//!   their tables.
//! * Quads: [`QUAD_FLOATS`] floats each, in drawing order: every monochrome glyph's shadow and
//!   glow, then outlines, then fills, then color glyphs.
//!
//! Lengths stay in layout units (the materials turn them into pixels), so a zoom only
//! changes which atlas fields are used: [`TextDraw::draw`] returns nothing when no record
//! changed. The atlas pages of the glyphs shown are retained, so they are never recycled
//! while shown.

use std::ops::Range;
use std::rc::Rc;

use glyphux_gpu::units::px;
use glyphux_gpu::{AtlasGlyph, GlyphInstance, GlyphKey, TextStyle, Units};

use crate::server::{ColorGlyph, FontChoice, Server};

pub use crate::records::{QUAD_FLOATS, RECORD_TEXELS};
/// Texels per color glyph record.
const COLOR_TEXELS: usize = 4;

/// Passes of monochrome glyphs (glyphux's `text_pass`): every glyph's shadow and glow, then
/// every outline, then every fill, so that no glyph's effects cover a neighbor. Color glyphs
/// are a pass of their own.
const PASS_EFFECTS: f32 = 0.0;
const PASS_OUTLINE: f32 = 1.0;
const PASS_FILL: f32 = 2.0;
const PASS_COLOR: f32 = crate::records::PASS_COLOR as f32;

/// A linear RGBA color.
pub type Rgba = [f32; 4];

/// How glyphs look, in layout units.
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphStyle {
    pub color: Rgba,
    /// Units added to the outline (> 0 bolder).
    pub weight: f32,
    /// Color and width.
    pub outline: Option<(Rgba, f32)>,
    /// Color and radius.
    pub glow: Option<(Rgba, f32)>,
    /// Color, offset and blur.
    pub shadow: Option<(Rgba, [f32; 2], f32)>,
    /// Slant of synthetic italics (x offset per unit above the baseline).
    pub slant: f32,
    pub underline: bool,
    pub strike: bool,
    /// Amplitude (units) and frequency (Hz) of a vertical wave.
    pub wave: Option<(f32, f32)>,
    /// Amplitude (units) and rate (jumps per second).
    pub shake: Option<(f32, f32)>,
    /// Speed (cycles per second), saturation, value and spatial frequency of a rainbow fill.
    pub rainbow: Option<(f32, f32, f32, f32)>,
}

impl Default for GlyphStyle {
    fn default() -> Self {
        GlyphStyle {
            color: [1.0; 4],
            weight: 0.0,
            outline: None,
            glow: None,
            shadow: None,
            slant: 0.0,
            underline: false,
            strike: false,
            wave: None,
            shake: None,
            rainbow: None,
        }
    }
}

impl GlyphStyle {
    /// glyphux's style at `size` units per em, `extra_weight` em bolder (weight the font
    /// instance lacks).
    fn text_style(&self, size: f32, extra_weight: f32) -> TextStyle {
        let mut style = TextStyle::new(px(size.max(0.01)), self.color).weight(px(self.weight + extra_weight * size));
        if let Some((c, w)) = self.outline.filter(|(c, w)| *w > 0.0 && c[3] > 0.0) {
            style = style.outline(c, px(w));
        }
        if let Some((c, r)) = self.glow.filter(|(c, r)| *r > 0.0 && c[3] > 0.0) {
            style = style.glow(c, px(r));
        }
        if let Some((c, o, b)) = self.shadow.filter(|(c, ..)| c[3] > 0.0) {
            style = style.shadow(c, [px(o[0]), px(o[1])], px(b));
        }
        style
    }

    fn animation(&self) -> [[f32; 4]; 2] {
        let (wa, wf) = self.wave.unwrap_or((0.0, 0.0));
        let (sa, sr) = self.shake.unwrap_or((0.0, 0.0));
        let (rs, rsat, rv, rf) = self.rainbow.unwrap_or((0.0, 0.0, 0.0, 0.0));
        [[wa, wf, sa, sr], [rs, rsat, rv, rf]]
    }

    fn animated(&self) -> bool {
        self.wave.is_some() || self.shake.is_some() || self.rainbow.is_some()
    }

    /// The foreground color COLR glyphs may use, as sRGB bytes.
    pub fn foreground(&self) -> [u8; 4] {
        ferreal::LinearColor::new(self.color[0], self.color[1], self.color[2], self.color[3])
            .to_srgb()
            .map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
    }
}

/// One glyph to draw.
pub struct Glyph<'a> {
    pub font: &'a FontChoice,
    /// Its id in its font.
    pub id: u32,
    /// Its origin (baseline, pen position) in layout units, y down.
    pub origin: [f32; 2],
    /// Its pen position and advance along its line.
    pub pen: f32,
    pub advance: f32,
    /// Units per em.
    pub size: f32,
    pub style: &'a GlyphStyle,
}

/// An underline or a strikethrough along a run of glyphs. It is drawn in their style, as a
/// piece under each glyph that moves with it (with wave and shake).
pub struct Decoration<'a> {
    /// The glyphs (indices in [`TextDraw::draw`]'s).
    pub glyphs: Range<usize>,
    /// Where it starts and ends along the line.
    pub x: [f32; 2],
    /// Its center below the baseline (y down) and its thickness.
    pub y: f32,
    pub thickness: f32,
    /// Drawn over the glyphs (strikethrough) instead of under them (underline).
    pub over: bool,
    pub style: &'a GlyphStyle,
}

/// A monochrome glyph: its record, its quad's corners and its animation.
#[derive(Clone, PartialEq)]
struct Mono {
    record: [[f32; 4]; RECORD_TEXELS],
    corners: [[f32; 2]; 4],
}

/// A color glyph: its compiled glyph, record and corners.
#[derive(Clone)]
struct ColorQuad {
    glyph: Rc<ColorGlyph>,
    record: [[f32; 4]; COLOR_TEXELS],
    corners: [[f32; 2]; 4],
}

impl PartialEq for ColorQuad {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.glyph, &other.glyph) && self.record == other.record && self.corners == other.corners
    }
}

/// What C++ draws: see the module documentation for the layouts.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DrawData {
    pub quads: Vec<f32>,
    pub records: Vec<f32>,
    pub tables: Vec<f32>,
    /// Some glyph moves or changes color with time (the quads need updating every frame).
    pub animated: bool,
}

/// The corners of `rect` (min x, min y, max x, max y) slanted about the baseline at
/// `baseline_y`.
fn corners(rect: [f32; 4], baseline_y: f32, slant: f32) -> [[f32; 2]; 4] {
    let [x0, y0, x1, y1] = rect;
    [[x0, y0], [x1, y0], [x0, y1], [x1, y1]].map(|[x, y]| [x + slant * (baseline_y - y), y])
}

/// The monochrome records as floats.
fn mono_floats(mono: &[Mono]) -> Vec<f32> {
    mono.iter().flat_map(|m| m.record.iter().flatten().copied()).collect()
}

/// The color glyph records, then each distinct glyph's tables, as floats.
fn color_floats(colors: &[ColorQuad]) -> Vec<f32> {
    let n = colors.len();
    let mut records: Vec<[f32; 4]> = Vec::with_capacity(COLOR_TEXELS * n);
    let mut tables: Vec<[f32; 4]> = Vec::new();
    let mut placed: Vec<(*const ColorGlyph, [f32; 4])> = Vec::new();
    for q in colors {
        let ptr = Rc::as_ptr(&q.glyph);
        let bases = if let Some((_, b)) = placed.iter().find(|(p, _)| *p == ptr) {
            *b
        } else {
            let at = |tables: &Vec<[f32; 4]>| (COLOR_TEXELS * n + tables.len()) as f32;
            let g = &q.glyph;
            let tiles = at(&tables);
            tables.extend_from_slice(&g.tiles);
            let paints = at(&tables);
            tables.extend_from_slice(&g.paints);
            let stops = at(&tables);
            tables.extend_from_slice(&g.stops);
            // Recipe words: four per texel.
            let recipes = at(&tables) * 4.0;
            tables.extend_from_slice(&g.recipes);
            let b = [tiles, paints, stops, recipes];
            placed.push((ptr, b));
            b
        };
        records.push(bases);
        records.extend_from_slice(&q.record[1..]);
    }
    records.extend(tables);
    records.into_iter().flatten().collect()
}

fn push_quad(
    out: &mut Vec<f32>,
    corners: &[[f32; 2]; 4],
    uv: [f32; 4],
    glyph: usize,
    pass: f32,
    anim: [f32; 4],
    phase: [f32; 2],
) {
    out.extend(corners.iter().flatten());
    out.extend(uv);
    out.extend([glyph as f32, pass]);
    out.extend(anim);
    out.extend(phase);
}

/// The quads: monochrome records in passes (those with a shadow or glow, those with an
/// outline, then all), then color glyphs.
fn quads(mono: &[Mono], colors: &[ColorQuad]) -> Vec<f32> {
    let mut q = Vec::with_capacity(QUAD_FLOATS * (mono.len() * 2 + colors.len()));
    let effects = |m: &Mono| (m.record[5][3] > 0.0 && m.record[4][2] > 0.0) || m.record[7][3] > 0.0;
    let outline = |m: &Mono| m.record[3][3] > 0.0 && m.record[4][1] > 0.0;
    for (pass, wanted) in [
        (PASS_EFFECTS, &effects as &dyn Fn(&Mono) -> bool),
        (PASS_OUTLINE, &outline),
        (PASS_FILL, &|_: &Mono| true),
    ] {
        for (i, m) in mono.iter().enumerate().filter(|(_, m)| wanted(m)) {
            // Record texel 9: layer, pen position (phase), index (seed); texel 10: animation.
            push_quad(
                &mut q,
                &m.corners,
                m.record[1],
                i,
                pass,
                m.record[10],
                [m.record[9][1], m.record[9][2]],
            );
        }
    }
    for (i, c) in colors.iter().enumerate() {
        // Record texel 2: page, opacity, pen position, index; texel 3: animation.
        push_quad(
            &mut q,
            &c.corners,
            [-1.0, -1.0, 1.0, 1.0],
            i,
            PASS_COLOR,
            c.record[3],
            [c.record[2][2], c.record[2][3]],
        );
    }
    q
}

/// The records of decoration `d`'s pieces, one per glyph, appended to `mono`.
///
/// A decoration has no field in the atlas: the material computes the exact distance to its
/// box (record texel 6: the box's half size; texel 9: layer -1). Its texture coordinates are
/// layout units from the box's center, and its distance range is wide enough for its
/// effects, so it takes the glyphs' outline, glow, shadow and animation as they are.
fn decoration_pieces(d: &Decoration<'_>, glyphs: &[Glyph<'_>], scale: f32, mono: &mut Vec<Mono>) {
    let size = glyphs[d.glyphs.clone()].iter().map(|g| g.size).fold(0.0, f32::max);
    let rs = d.style.text_style(size, 0.0).resolve(&Units::new(1.0));
    // How far the effects reach, and a pixel to antialias the edge.
    let pad = rs.reach() + 1.0 / scale.max(1e-6);
    let field = AtlasGlyph {
        layer: 0,
        uv_min: [0.0; 2],
        uv_max: [0.0; 2],
        plane_min: [0.0; 2],
        plane_max: [0.0; 2],
        level: 0,
        px_range: 2.0 * (rs.reach() + 2.0),
    };
    let q = rs.instance(&field, [0.0, 0.0]);
    let half = [0.5 * (d.x[1] - d.x[0]), 0.5 * d.thickness];
    let center = f32::midpoint(d.x[0], d.x[1]);
    let animation = d.style.animation();
    for index in d.glyphs.clone() {
        let g = &glyphs[index];
        // Its stretch of the line (the line's ends padded for the effects).
        let from = if g.pen <= d.x[0] + 1e-4 { d.x[0] - pad } else { g.pen };
        let to = if g.pen + g.advance >= d.x[1] - 1e-4 {
            d.x[1] + pad
        } else {
            g.pen + g.advance
        };
        if to <= from {
            continue;
        }
        let (top, bottom) = (-half[1] - pad, half[1] + pad);
        let [ox, oy] = g.origin;
        let rect = [ox + from - g.pen, oy + d.y + top, ox + to - g.pen, oy + d.y + bottom];
        mono.push(Mono {
            record: [
                rect,
                [from - center, top, to - center, bottom],
                q.color,
                q.outline,
                q.params,
                q.glow,
                [half[0], half[1], 0.0, 0.0],
                q.shadow,
                q.shadow_params,
                [-1.0, ox, index as f32, 0.0],
                animation[0],
                animation[1],
            ],
            corners: corners(rect, oy, 0.0),
        });
    }
}

/// What a label shows.
#[derive(Default)]
pub struct TextDraw {
    /// What is drawn now.
    drawn: Option<(Vec<Mono>, Vec<ColorQuad>)>,
    /// Atlas pages retained for the glyphs shown.
    retained: Vec<u32>,
}

impl TextDraw {
    /// Releases the retained pages (the label is hidden or gone) and forgets what it drew.
    pub fn release(&mut self, s: &mut Server) {
        for l in std::mem::take(&mut self.retained) {
            s.atlas.release_layer(l);
        }
        self.drawn = None;
    }

    /// Requests the glyphs at `scale` screen pixels per unit. Returns what to draw if it
    /// changed, and the monochrome glyphs shown (to be told when their fields improve).
    pub fn draw(
        &mut self,
        s: &mut Server,
        glyphs: &[Glyph<'_>],
        decorations: &[Decoration<'_>],
        scale: f32,
    ) -> (Option<DrawData>, Vec<GlyphKey>) {
        let local = Units::new(1.0);
        let mut mono: Vec<Mono> = Vec::new();
        let mut colors: Vec<ColorQuad> = Vec::new();
        let mut keys = Vec::new();
        let mut layers: Vec<u32> = Vec::new();
        let mut animated = false;
        for d in decorations.iter().filter(|d| !d.over) {
            decoration_pieces(d, glyphs, scale, &mut mono);
        }
        for (index, g) in glyphs.iter().enumerate() {
            animated |= g.style.animated();
            let [ox, oy] = g.origin;
            let animation = g.style.animation();
            if let Some(color) = s.color_glyph(&g.font.font, g.id, g.style.foreground()) {
                let [min, max] = color.plane;
                let rect = [
                    ox + min[0] * g.size,
                    oy - max[1] * g.size,
                    ox + max[0] * g.size,
                    oy - min[1] * g.size,
                ];
                // Too small on screen for its tiles.
                if (rect[2] - rect[0]) * scale < color.config[3] {
                    continue;
                }
                colors.push(ColorQuad {
                    record: [
                        [0.0; 4],
                        color.config,
                        [color.page as f32, g.style.color[3], ox, index as f32],
                        animation[0],
                    ],
                    corners: corners(rect, oy, g.style.slant),
                    glyph: color,
                });
                continue;
            }
            let key = GlyphKey {
                font: g.font.font.id,
                glyph: g.id,
            };
            let rs = g
                .style
                .text_style(g.size, 0.0001 * g.font.missing_weight)
                .resolve(&local);
            let (size, reach) = (f64::from(g.size * scale), f64::from(rs.reach() * scale));
            let Some(placed) = s.atlas.get_with_reach(key, size, reach, || g.font.font.outline(g.id)) else {
                continue;
            };
            if !placed.is_visible() {
                continue;
            }
            keys.push(key);
            if !layers.contains(&placed.layer) {
                layers.push(placed.layer);
            }
            let q: GlyphInstance = rs.instance(&placed, g.origin);
            mono.push(Mono {
                record: [
                    q.rect,
                    q.uv,
                    q.color,
                    q.outline,
                    q.params,
                    q.glow,
                    [0.0; 4],
                    q.shadow,
                    q.shadow_params,
                    [q.layer as f32, ox, index as f32, 0.0],
                    animation[0],
                    animation[1],
                ],
                corners: corners(q.rect, oy, g.style.slant),
            });
        }
        for d in decorations.iter().filter(|d| d.over) {
            decoration_pieces(d, glyphs, scale, &mut mono);
        }
        // Retain before releasing: shared pages stay retained throughout.
        for &l in &layers {
            s.atlas.retain_layer(l);
        }
        for l in std::mem::replace(&mut self.retained, layers) {
            s.atlas.release_layer(l);
        }
        let unchanged = self.drawn.as_ref().is_some_and(|(m, c)| *m == mono && *c == colors);
        let data = (!unchanged).then(|| DrawData {
            quads: quads(&mono, &colors),
            records: mono_floats(&mono),
            tables: color_floats(&colors),
            animated,
        });
        self.drawn = Some((mono, colors));
        (data, keys)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quad_layout() {
        let m = Mono {
            record: [[1.0; 4]; RECORD_TEXELS],
            corners: [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]],
        };
        let q = quads(&[m.clone(), m], &[]);
        // Both glyphs have effects and outlines (all ones): three passes of two.
        assert_eq!(q.len(), 6 * QUAD_FLOATS);
        assert_eq!(q[12], 0.0, "first quad: glyph 0");
        assert_eq!(q[13], PASS_EFFECTS);
        assert_eq!(q[5 * QUAD_FLOATS + 12], 1.0, "last quad: glyph 1");
        assert_eq!(q[5 * QUAD_FLOATS + 13], PASS_FILL);
    }
}
