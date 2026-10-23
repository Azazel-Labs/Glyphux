//! Font glyph loading via the pure-Rust `ttf-parser` crate (TrueType, OpenType/CFF, CFF2,
//! variable fonts).

use crate::edge::Edge;
use crate::math::Vec2;
use crate::shape::{Contour, Shape};

mod kerning;

/// The font parser, for what this module doesn't wrap (names, tables…) through
/// [`Font::face`]. Its types are part of this crate's API: a new major version of
/// `ttf-parser` is a breaking change here too.
pub use ttf_parser;

/// How glyph coordinates are scaled.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum FontScaling {
    /// Raw font units.
    None,
    /// 1 unit = 1 em.
    #[default]
    EmNormalized,
    /// Font units / 64 (msdfgen's legacy default, for command line compatibility).
    Legacy,
}

/// Error while loading a font or glyph.
#[derive(Clone, Debug, PartialEq)]
pub enum FontError {
    /// The data is not a font (or not face `index` of a collection).
    Parse(String),
    MissingGlyph,
    /// A variation axis the font doesn't have.
    UnknownAxis(String),
}

impl core::fmt::Display for FontError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FontError::Parse(e) => write!(f, "failed to parse font: {e}"),
            FontError::MissingGlyph => write!(f, "glyph not found in font"),
            FontError::UnknownAxis(axis) => write!(f, "font has no variation axis '{axis}'"),
        }
    }
}

impl std::error::Error for FontError {}

/// Font-wide metrics in the selected scaling.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontMetrics {
    pub em_size: f64,
    pub ascender: f64,
    pub descender: f64,
    pub line_height: f64,
    /// Center of the underline.
    pub underline_y: f64,
    pub underline_thickness: f64,
}

/// A glyph outline plus its advance width.
#[derive(Clone, Debug)]
pub struct Glyph {
    pub shape: Shape,
    pub advance: f64,
    pub glyph_id: u16,
}

/// A parsed font face.
pub struct Font<'a> {
    face: ttf_parser::Face<'a>,
    scale: f64,
}

impl<'a> Font<'a> {
    /// Parses face `index` of a font file.
    pub fn from_bytes(data: &'a [u8], index: u32, scaling: FontScaling) -> Result<Self, FontError> {
        let face = ttf_parser::Face::parse(data, index).map_err(|e| FontError::Parse(e.to_string()))?;
        let upem = face.units_per_em() as f64;
        let scale = match scaling {
            FontScaling::None => 1.0,
            FontScaling::EmNormalized => 1.0 / upem,
            FontScaling::Legacy => 1.0 / 64.0,
        };
        Ok(Font { face, scale })
    }

    /// Sets a variation axis (e.g. `"wght"`, 700.0) for variable fonts.
    pub fn set_variation(&mut self, axis: &str, value: f32) -> bool {
        let b = axis.as_bytes();
        if b.len() != 4 {
            return false;
        }
        self.face
            .set_variation(ttf_parser::Tag::from_bytes(&[b[0], b[1], b[2], b[3]]), value)
            .is_some()
    }

    /// The underlying `ttf-parser` face.
    pub fn face(&self) -> &ttf_parser::Face<'a> {
        &self.face
    }

    /// Number of glyphs in the font (glyph ids are `0..glyph_count()`).
    pub fn glyph_count(&self) -> u16 {
        self.face.number_of_glyphs()
    }

    pub fn metrics(&self) -> FontMetrics {
        let f = &self.face;
        let s = self.scale;
        let (uy, ut) = f
            .underline_metrics()
            // The `post` table stores the top of the underline; report its center (as FreeType does).
            .map_or((0.0, 0.0), |m| {
                (
                    m.position as f64 - 0.5 * m.thickness as f64,
                    m.thickness as f64,
                )
            });
        FontMetrics {
            em_size: f.units_per_em() as f64 * s,
            ascender: f.ascender() as f64 * s,
            descender: f.descender() as f64 * s,
            line_height: (f.ascender() as f64 - f.descender() as f64 + f.line_gap() as f64) * s,
            underline_y: uy * s,
            underline_thickness: ut * s,
        }
    }

    pub fn glyph_index(&self, c: char) -> Option<u16> {
        self.face.glyph_index(c).map(|g| g.0)
    }

    /// Loads the glyph for a Unicode character.
    pub fn glyph(&self, c: char) -> Result<Glyph, FontError> {
        let id = self.face.glyph_index(c).ok_or(FontError::MissingGlyph)?;
        self.glyph_by_index(id.0)
    }

    /// Horizontal advance of a glyph (without loading its outline).
    pub fn advance(&self, id: u16) -> f64 {
        self.face.glyph_hor_advance(ttf_parser::GlyphId(id)).unwrap_or(0) as f64 * self.scale
    }

    /// Loads a glyph by index. Glyphs without an outline (e.g. space) yield an empty shape.
    pub fn glyph_by_index(&self, id: u16) -> Result<Glyph, FontError> {
        if id >= self.face.number_of_glyphs() {
            return Err(FontError::MissingGlyph);
        }
        let gid = ttf_parser::GlyphId(id);
        let mut b = Builder {
            shape: Shape::new(),
            cur: Contour::new(),
            start: Vec2::ZERO,
            pos: Vec2::ZERO,
            s: self.scale,
        };
        self.face.outline_glyph(gid, &mut b);
        b.finish();
        let advance = self.face.glyph_hor_advance(gid).unwrap_or(0) as f64 * self.scale;
        Ok(Glyph {
            shape: b.shape,
            advance,
            glyph_id: id,
        })
    }

    /// Combined, nonzero format-0 `kern` pairs as `(left, right, adjustment)`.
    /// The flag indicates class-based subtables: query [`Self::kerning`] per pair to
    /// include those too. Vertical, cross-stream, minimum-distance, variable and
    /// contextual subtables are excluded. This is not OpenType GPOS shaping.
    pub fn kerning_pairs(&self) -> (Vec<(u16, u16, f64)>, bool) {
        let mut pairs = std::collections::BTreeMap::new();
        let mut other = false;
        for (format, replace) in kerning::subtables(self.kern_data()) {
            match format {
                ttf_parser::kern::Format::Format0(f0) => {
                    for p in f0.pairs {
                        let value = pairs.entry((p.left().0, p.right().0)).or_insert(0.0);
                        *value = if replace { 0.0 } else { *value } + f64::from(p.value);
                    }
                }
                _ => other = true,
            }
        }
        let out = pairs
            .into_iter()
            .filter_map(|((l, r), v)| (v != 0.0).then_some((l, r, v * self.scale)))
            .collect();
        (out, other)
    }

    /// Horizontal advance adjustment from legacy `kern`, in the selected scaling.
    /// Additive and override subtables are combined in order. This excludes cross-stream,
    /// minimum-distance, variable and contextual kerning. Use a text shaper for GPOS
    /// kerning, language-specific positioning, ligatures and combining marks.
    pub fn kerning(&self, left: u16, right: u16) -> f64 {
        let mut value = 0.0;
        for (format, replace) in kerning::subtables(self.kern_data()) {
            let l = ttf_parser::GlyphId(left);
            let r = ttf_parser::GlyphId(right);
            // Format1 has different fields when another dependency enables apple-layout.
            #[allow(clippy::match_wildcard_for_single_variants)]
            let adjustment = match format {
                ttf_parser::kern::Format::Format0(st) => st.glyphs_kerning(l, r),
                ttf_parser::kern::Format::Format2(st) => st.glyphs_kerning(l, r),
                ttf_parser::kern::Format::Format3(st) => st.glyphs_kerning(l, r),
                _ => None,
            };
            if let Some(v) = adjustment {
                value = if replace { 0.0 } else { value } + f64::from(v);
            }
        }
        value * self.scale
    }

    fn kern_data(&self) -> &[u8] {
        self.face
            .raw_face()
            .table(ttf_parser::Tag::from_bytes(b"kern"))
            .unwrap_or(&[])
    }
}

struct Builder {
    shape: Shape,
    cur: Contour,
    start: Vec2,
    pos: Vec2,
    s: f64,
}

impl Builder {
    fn p(&self, x: f32, y: f32) -> Vec2 {
        Vec2::new(x as f64 * self.s, y as f64 * self.s)
    }
    fn finish(&mut self) {
        if !self.cur.edges.is_empty() {
            if self.pos != self.start {
                self.cur.add_edge(Edge::line(self.pos, self.start));
            }
            self.shape.add_contour(std::mem::take(&mut self.cur));
        }
        self.pos = self.start;
    }
}

impl ttf_parser::OutlineBuilder for Builder {
    fn move_to(&mut self, x: f32, y: f32) {
        self.finish();
        self.start = self.p(x, y);
        self.pos = self.start;
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let p = self.p(x, y);
        if p != self.pos {
            self.cur.add_edge(Edge::line(self.pos, p));
            self.pos = p;
        }
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let c = self.p(x1, y1);
        let p = self.p(x, y);
        if c == self.pos || c == p {
            self.line_to(x, y);
            return;
        }
        self.cur.add_edge(Edge::quadratic(self.pos, c, p));
        self.pos = p;
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let c1 = self.p(x1, y1);
        let c2 = self.p(x2, y2);
        let p = self.p(x, y);
        if p == self.pos && c1 == p && c2 == p {
            return;
        }
        self.cur.add_edge(Edge::cubic(self.pos, c1, c2, p));
        self.pos = p;
    }
    fn close(&mut self) {
        self.finish();
    }
}
