//! Font glyph loading via the pure-Rust `ttf-parser` crate (TrueType, OpenType/CFF, CFF2,
//! variable fonts).

use crate::edge::Edge;
use crate::math::Vec2;
use crate::shape::{Contour, Shape};

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
    Parse(String),
    MissingGlyph,
}

impl core::fmt::Display for FontError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FontError::Parse(e) => write!(f, "failed to parse font: {e}"),
            FontError::MissingGlyph => write!(f, "glyph not found in font"),
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

    pub fn face(&self) -> &ttf_parser::Face<'a> {
        &self.face
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

    /// All kerning pairs listed in format-0 `kern` subtables as `(left, right, adjustment)`,
    /// plus a flag telling whether other (class-based) subtables exist, for which
    /// [`Self::kerning`] must be queried per pair.
    pub fn kerning_pairs(&self) -> (Vec<(u16, u16, f64)>, bool) {
        let mut out = Vec::new();
        let mut other = false;
        let Some(kern) = self.face.tables().kern else {
            return (out, false);
        };
        for st in kern.subtables {
            if !st.horizontal || st.variable {
                continue;
            }
            match st.format {
                ttf_parser::kern::Format::Format0(f0) => {
                    for p in f0.pairs {
                        if p.value != 0 {
                            out.push((p.left().0, p.right().0, p.value as f64 * self.scale));
                        }
                    }
                }
                _ => other = true,
            }
        }
        (out, other)
    }

    /// Kerning adjustment between two glyphs (from the `kern` table), in the selected scaling.
    pub fn kerning(&self, left: u16, right: u16) -> f64 {
        let Some(kern) = self.face.tables().kern else {
            return 0.0;
        };
        for st in kern.subtables {
            if st.horizontal && !st.variable {
                if let Some(v) = st.glyphs_kerning(ttf_parser::GlyphId(left), ttf_parser::GlyphId(right)) {
                    return v as f64 * self.scale;
                }
            }
        }
        0.0
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
