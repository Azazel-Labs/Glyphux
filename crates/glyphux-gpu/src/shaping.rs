//! OpenType text shaping with [HarfRust](https://github.com/harfbuzz/harfrust) (HarfBuzz's
//! Rust port) and the Unicode bidirectional algorithm ([`unicode_bidi`]).
//!
//! A line is split into bidi runs in visual order, each run into script runs, and each piece
//! is shaped with its own direction and script (with the surrounding text as context, so
//! Arabic joins across pieces). The result is positioned glyph ids in em units, which
//! [`crate::text::layout_shaped`] turns into quads from the glyph atlas. Glyph outlines come
//! from the same font file through glyphux's font loader, so glyph ids always agree.

use std::sync::Arc;

use glyphux::Shape;
use glyphux::io::font::{Font as OutlineFont, FontMetrics, FontScaling};
use harfrust::{Buffer, BufferFlags, Direction, Feature, ShapeOptions, ShapePlan, ShapePlanKey, ShaperFont};
use unicode_bidi::BidiInfo;

pub use harfrust;

/// A font prepared for shaping (and for loading the outlines of the glyphs it shapes to).
pub struct ShapingFont {
    /// Id used in the atlas' [`crate::GlyphKey`]s and to find [`Shaper`]'s cached plans;
    /// unique per font *and* variation instance.
    pub id: u32,
    data: Arc<Vec<u8>>,
    index: u32,
    variations: Vec<([u8; 4], f32)>,
    instance: harfrust::Font,
    units_per_em: f32,
    metrics: FontMetrics,
}

impl ShapingFont {
    /// Parses a font (index `index` of a collection). `None` if it is not a font.
    pub fn new(id: u32, data: Arc<Vec<u8>>, index: u32) -> Option<Self> {
        let metrics = OutlineFont::from_bytes(&data, index, FontScaling::EmNormalized)
            .ok()?
            .metrics();
        let blob: Arc<dyn AsRef<[u8]> + Send + Sync> = data.clone();
        let instance = harfrust::Font::new(harfrust::font::Blob::from(blob), index)?;
        let units_per_em = instance.units_per_em().max(1) as f32;
        Some(ShapingFont {
            id,
            data,
            index,
            variations: Vec::new(),
            instance,
            units_per_em,
            metrics,
        })
    }

    /// The same font at a point of its design space (variable fonts), e.g.
    /// `[("wght", 700.0)]`. Give the instance its own `id`: its glyphs differ.
    pub fn with_variations(mut self, id: u32, variations: &[(&str, f32)]) -> Self {
        self.id = id;
        self.variations = variations
            .iter()
            .filter_map(|&(t, v)| Some((<[u8; 4]>::try_from(t.as_bytes()).ok()?, v)))
            .collect();
        self.instance = self
            .instance
            .instance_builder()
            .variations(variations.iter().map(|&(t, v)| (t, v)))
            .build();
        if let Some(f) = self.outlines() {
            self.metrics = f.metrics();
        }
        self
    }

    /// The outline loader: parses the font's tables again (a few microseconds), which is
    /// fine for loading the outline of a glyph new to the atlas, not for per-character work.
    fn outlines(&self) -> Option<OutlineFont<'_>> {
        let mut f = OutlineFont::from_bytes(&self.data, self.index, FontScaling::EmNormalized).ok()?;
        for (tag, v) in &self.variations {
            f.set_variation(std::str::from_utf8(tag).ok()?, *v);
        }
        Some(f)
    }

    pub fn units_per_em(&self) -> f32 {
        self.units_per_em
    }

    /// Outline of a glyph in em units (y up), or `None`.
    pub fn outline(&self, glyph: u32) -> Option<Shape> {
        let g = self.outlines()?.glyph_by_index(u16::try_from(glyph).ok()?).ok()?;
        Some(g.shape)
    }

    /// COLR artwork for a shaped glyph ID, including variation selectors and ligatures
    /// already resolved by the shaper. Bitmap-only and SVG glyphs return an error.
    pub fn color_glyph(
        &self,
        glyph: u32,
        palette: u16,
        foreground: [u8; 4],
    ) -> Result<glyphux::color::ColorGlyph, glyphux::color::ColorError> {
        let font = self
            .outlines()
            .ok_or(glyphux::color::ColorError::Invalid("font face"))?;
        font.color_glyph_by_index(
            u16::try_from(glyph).map_err(|_| glyphux::color::ColorError::Invalid("glyph index"))?,
            palette,
            foreground,
        )
    }

    /// Whether the font maps `c` to a glyph (for choosing a fallback font per line), through
    /// the shaper's own (cached) character map.
    pub fn has_glyph(&self, c: char) -> bool {
        self.instance.charmap().map_unicode(c).is_some()
    }

    /// Ascender, descender, line height etc. in em units.
    pub fn metrics(&self) -> FontMetrics {
        self.metrics
    }
}

/// A positioned glyph of a shaped line (em units, relative to the line's origin, y up).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapedGlyph {
    pub glyph: u32,
    /// Byte offset in the source text of the cluster the glyph belongs to.
    pub cluster: u32,
    /// Where the glyph's origin goes (pen position plus the shaper's offset).
    pub x: f32,
    pub y: f32,
    pub advance: f32,
}

/// A shaped line: glyphs in visual (left to right) order.
#[derive(Clone, Debug, Default)]
pub struct ShapedLine {
    pub glyphs: Vec<ShapedGlyph>,
    /// Total advance in em.
    pub width: f32,
    /// Whether the paragraph's base direction is right to left.
    pub rtl: bool,
}

/// Shape plans kept by a [`Shaper`]; a line typically needs one or two.
const PLAN_CACHE: usize = 32;

/// Reusable shaping state (buffers, compiled shape plans), and the features to apply.
pub struct Shaper {
    buffer: Option<Buffer>,
    probe: Option<Buffer>,
    /// Compiled plans by font id, most recently used last. Building a plan (looking up and
    /// compiling the font's lookups for a script, direction and feature set) costs far more
    /// than shaping a short line with it.
    plans: Vec<(u32, ShapePlan)>,
    /// OpenType features for every run, e.g. `"smcp".parse()`, `"-liga".parse()`,
    /// `"ss01=1".parse()`. Defaults (kerning, ligatures, marks, contextual forms…) apply
    /// without listing them.
    pub features: Vec<Feature>,
    /// Paragraph direction: `None` detects it from the first strong character.
    pub base_rtl: Option<bool>,
}

impl Default for Shaper {
    fn default() -> Self {
        Self::new()
    }
}

impl Shaper {
    pub fn new() -> Self {
        Shaper {
            buffer: Some(Buffer::new()),
            probe: Some(Buffer::new()),
            plans: Vec::new(),
            features: Vec::new(),
            base_rtl: None,
        }
    }

    /// Script of a character as the shaper sees it (`None` for common / inherited ones such
    /// as spaces, digits, punctuation and combining marks, which join their neighbors' run).
    fn script_of(&mut self, c: char) -> Option<harfrust::Script> {
        if c.is_ascii() {
            return c.is_ascii_alphabetic().then_some(harfrust::Script::LATIN);
        }
        let mut b = self.probe.take().unwrap_or_default();
        b.clear();
        b.push(c as u32, 0);
        b.guess_segment_properties();
        let s = b.script();
        self.probe = Some(b);
        s
    }

    /// Index in `self.plans` of the plan for `font` and the buffer's properties, compiled on
    /// first use.
    fn plan(&mut self, font: &ShapingFont, buf: &Buffer) -> usize {
        let key = ShapePlanKey::new(&font.instance, buf.script(), buf.direction()).features(&self.features);
        let found = self.plans.iter().rposition(|(id, p)| *id == font.id && key.matches(p));
        let plan = if let Some(i) = found {
            self.plans.remove(i)
        } else {
            if self.plans.len() >= PLAN_CACHE {
                self.plans.remove(0);
            }
            let plan = ShapePlan::new(&font.instance, buf.direction(), buf.script(), None, &self.features);
            (font.id, plan)
        };
        self.plans.push(plan);
        self.plans.len() - 1
    }

    /// Shapes one line of text (no line breaks; a `\n` ends the line).
    pub fn shape_line(&mut self, font: &ShapingFont, text: &str) -> ShapedLine {
        let text = text.split('\n').next().unwrap_or("");
        let mut out = ShapedLine::default();
        if text.is_empty() {
            return out;
        }
        let base = self.base_rtl.map(|r| {
            if r {
                unicode_bidi::Level::rtl()
            } else {
                unicode_bidi::Level::ltr()
            }
        });
        let bidi = BidiInfo::new(text, base);
        let Some(para) = bidi.paragraphs.first() else {
            return out;
        };
        out.rtl = para.level.is_rtl();
        let (levels, runs) = bidi.visual_runs(para, para.range.clone());
        let scale = 1.0 / font.units_per_em;
        let shaper_font = ShaperFont::new(&font.instance);
        let mut pen = 0.0f32;
        for run in runs {
            let rtl = levels[run.start].is_rtl();
            // Script runs inside the bidi run, in logical order.
            let mut pieces: Vec<(usize, usize, Option<harfrust::Script>)> = Vec::new();
            let mut start = run.start;
            let mut script = None;
            for (i, c) in text[run.clone()].char_indices() {
                let i = i + run.start;
                let s = self.script_of(c);
                if let (Some(s), Some(cur)) = (s, script)
                    && s != cur
                {
                    pieces.push((start, i, script));
                    start = i;
                }
                if script.is_none() {
                    script = s;
                } else if let Some(s) = s {
                    script = Some(s);
                }
            }
            pieces.push((start, run.end, script));
            if rtl {
                pieces.reverse();
            }
            for (a, b, script) in pieces {
                let mut buf = self.buffer.take().unwrap_or_default();
                buf.clear();
                buf.push_str(&text[a..b]);
                buf.set_pre_context(&text[..a]);
                buf.set_post_context(&text[b..]);
                buf.set_direction(if rtl {
                    Direction::RightToLeft
                } else {
                    Direction::LeftToRight
                });
                buf.set_script(script);
                let mut flags = BufferFlags::empty();
                if a == 0 {
                    flags |= BufferFlags::BEGINNING_OF_TEXT;
                }
                if b == text.len() {
                    flags |= BufferFlags::END_OF_TEXT;
                }
                buf.set_flags(flags);
                buf.guess_segment_properties();
                let i = self.plan(font, &buf);
                let plan = &self.plans[i].1;
                let options = ShapeOptions::new().plan(Some(plan)).features(&self.features);
                if harfrust::shape(&shaper_font, &mut buf, options).is_ok() {
                    for (info, pos) in buf.glyph_infos().iter().zip(buf.glyph_positions()) {
                        out.glyphs.push(ShapedGlyph {
                            glyph: info.glyph_id,
                            cluster: info.cluster + a as u32,
                            x: pen + pos.x_offset as f32 * scale,
                            y: pos.y_offset as f32 * scale,
                            advance: pos.x_advance as f32 * scale,
                        });
                        pen += pos.x_advance as f32 * scale;
                    }
                }
                self.buffer = Some(buf);
            }
        }
        out.width = pen;
        out
    }
}
