//! # textra-atlas
//!
//! Font atlas generation on top of [`textra`]: select glyphs with a [`Charset`], compute glyph
//! boxes, pack them (tightly with MaxRects or in a uniform grid), generate hard/soft masks,
//! SDF, PSDF, MSDF or MTSDF tiles in parallel and export the layout as JSON or CSV in the
//! format of Viktor Chlumský's msdf-atlas-gen.
//!
//! ```no_run
//! use textra_atlas::*;
//!
//! let data = std::fs::read("font.ttf").unwrap();
//! let font = FontGeometry::load(&data, 0, &[], &GlyphSelection::Charset(Charset::ascii()), &GlyphOptions::default()).unwrap();
//! let config = AtlasConfig { size: Some(32.0), ..AtlasConfig::default() };
//! let atlas = generate(&font, &config).unwrap();
//! atlas.image.save("atlas.png", textra::io::ImageFormat::Png).unwrap();
//! std::fs::write("atlas.json", layout::to_json(&font, &atlas.layout, &config)).unwrap();
//! ```

pub mod charset;
pub mod layout;
pub mod pack;

use std::path::Path;

pub use charset::Charset;
pub use pack::DimensionsConstraint;

use textra::io::font::{Font, FontError, FontScaling};
use textra::io::image::{ImageFormat, save};
use textra::{
    Bitmap, Bounds, ColoringStrategy, DEFAULT_ANGLE_THRESHOLD, FillRule, GeneratorConfig, Projection, Range, Shape,
    Transform, Vec2, edge_coloring, generate_msdf, generate_mtsdf, generate_psdf, generate_sdf, io, par_map, rasterize,
};

/// Kind of atlas image.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AtlasType {
    /// Binary coverage (pixel centers inside the shape).
    HardMask,
    /// Anti-aliased coverage.
    SoftMask,
    /// True signed distance field.
    Sdf,
    /// Signed pseudo-distance field.
    Psdf,
    /// Multi-channel signed distance field.
    #[default]
    Msdf,
    /// MSDF with the true SDF in alpha.
    Mtsdf,
}

impl AtlasType {
    pub fn channels(self) -> usize {
        match self {
            AtlasType::Msdf => 3,
            AtlasType::Mtsdf => 4,
            _ => 1,
        }
    }
    /// Name used in the JSON layout.
    pub fn name(self) -> &'static str {
        match self {
            AtlasType::HardMask => "hardmask",
            AtlasType::SoftMask => "softmask",
            AtlasType::Sdf => "sdf",
            AtlasType::Psdf => "psdf",
            AtlasType::Msdf => "msdf",
            AtlasType::Mtsdf => "mtsdf",
        }
    }
}

/// Vertical direction of atlas and plane coordinates in the layout.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum YOrigin {
    /// y grows upward from the bottom of the atlas (OpenGL convention).
    #[default]
    Bottom,
    /// y grows downward from the top of the atlas (image convention).
    Top,
}

/// Which glyphs to load.
#[derive(Clone, Debug)]
pub enum GlyphSelection {
    /// Unicode characters.
    Charset(Charset),
    /// Glyph indices.
    Indices(Vec<u16>),
    /// Every glyph in the font.
    All,
}

/// Shape preparation options.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlyphOptions {
    /// Scale applied to glyph geometry (1 = em units).
    pub font_scale: f64,
    pub coloring: ColoringStrategy,
    pub angle_threshold: f64,
    pub seed: u64,
    /// Reorient contours by the non-zero fill rule.
    pub orient: bool,
    /// Compute kerning pairs (from the `kern` table).
    pub kerning: bool,
}

impl Default for GlyphOptions {
    fn default() -> Self {
        GlyphOptions {
            font_scale: 1.0,
            coloring: ColoringStrategy::InkTrap,
            angle_threshold: DEFAULT_ANGLE_THRESHOLD,
            seed: 0,
            orient: true,
            kerning: true,
        }
    }
}

/// One glyph's prepared geometry.
#[derive(Clone, Debug)]
pub struct GlyphGeometry {
    pub glyph_index: u16,
    pub codepoint: Option<u32>,
    /// Normalized, oriented and colored outline, in ems times `font_scale`.
    pub shape: Shape,
    pub bounds: Bounds,
    pub advance: f64,
}

impl GlyphGeometry {
    /// Whitespace and other glyphs without an outline.
    pub fn is_empty(&self) -> bool {
        self.shape.contours.is_empty() || self.bounds.is_empty()
    }
}

/// Font-wide metrics in ems times `font_scale`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    pub em_size: f64,
    pub line_height: f64,
    pub ascender: f64,
    pub descender: f64,
    pub underline_y: f64,
    pub underline_thickness: f64,
}

/// All glyphs of a font selected for an atlas.
#[derive(Clone, Debug)]
pub struct FontGeometry {
    pub glyphs: Vec<GlyphGeometry>,
    pub metrics: Metrics,
    /// Kerning pairs as indices into `glyphs` and the advance adjustment.
    pub kerning: Vec<(usize, usize, f64)>,
    /// Requested code points that the font doesn't contain.
    pub missing: Vec<u32>,
    /// The font's class-based `kern` subtables were left out of `kerning`: they can only be
    /// queried pair by pair, which is skipped beyond [`CLASS_KERNING_LIMIT`] glyphs.
    pub kerning_incomplete: bool,
}

/// Up to this many glyphs, class-based `kern` subtables are queried for every pair of glyphs.
pub const CLASS_KERNING_LIMIT: usize = 2048;

impl FontGeometry {
    /// Loads glyphs from font data (face `index`), with optional variation axis values.
    pub fn load(
        data: &[u8],
        index: u32,
        variations: &[(String, f32)],
        selection: &GlyphSelection,
        opts: &GlyphOptions,
    ) -> Result<FontGeometry, FontError> {
        let mut font = Font::from_bytes(data, index, FontScaling::EmNormalized)?;
        for (axis, value) in variations {
            if !font.set_variation(axis, *value) {
                return Err(FontError::UnknownAxis(axis.clone()));
            }
        }
        let k = opts.font_scale;
        let fm = font.metrics();
        let metrics = Metrics {
            em_size: fm.em_size * k,
            line_height: fm.line_height * k,
            ascender: fm.ascender * k,
            descender: fm.descender * k,
            underline_y: fm.underline_y * k,
            underline_thickness: fm.underline_thickness * k,
        };
        // (glyph index, code point) pairs
        let mut wanted: Vec<(u16, Option<u32>)> = Vec::new();
        let mut missing = Vec::new();
        match selection {
            GlyphSelection::Charset(cs) => {
                for cp in cs.iter() {
                    match char::from_u32(cp).and_then(|c| font.glyph_index(c)) {
                        Some(g) => wanted.push((g, Some(cp))),
                        None => missing.push(cp),
                    }
                }
            }
            GlyphSelection::Indices(ids) => wanted.extend(ids.iter().map(|&g| (g, None))),
            GlyphSelection::All => {
                wanted.extend((0..font.glyph_count()).map(|g| (g, None)));
            }
        }
        let glyphs: Vec<GlyphGeometry> = par_map(&wanted, 0, |&(gid, cp)| {
            let g = font.glyph_by_index(gid).unwrap_or_else(|_| io::font::Glyph {
                shape: Shape::new(),
                advance: 0.0,
                glyph_id: gid,
            });
            let mut shape = g.shape;
            if k != 1.0 {
                for c in &mut shape.contours {
                    for e in &mut c.edges {
                        for p in e.segment.control_points_mut() {
                            *p *= k;
                        }
                    }
                }
            }
            shape.normalize();
            if opts.orient {
                shape.orient_contours(FillRule::NonZero);
            }
            edge_coloring(&mut shape, opts.coloring, opts.angle_threshold, opts.seed);
            let bounds = shape.bounds();
            GlyphGeometry {
                glyph_index: gid,
                codepoint: cp,
                shape,
                bounds,
                advance: g.advance * k,
            }
        });
        let mut kerning = Vec::new();
        let mut kerning_incomplete = false;
        if opts.kerning {
            let (pairs, class_based) = font.kerning_pairs();
            kerning_incomplete = class_based && glyphs.len() > CLASS_KERNING_LIMIT;
            let mut slot = std::collections::HashMap::new();
            for (i, g) in glyphs.iter().enumerate() {
                slot.entry(g.glyph_index).or_insert(i);
            }
            if class_based && !kerning_incomplete {
                // Class-based subtables can only be queried per pair.
                let rows: Vec<usize> = (0..glyphs.len()).collect();
                let found = par_map(&rows, 0, |&i| {
                    let mut v = Vec::new();
                    for (j, b) in glyphs.iter().enumerate() {
                        let k = font.kerning(glyphs[i].glyph_index, b.glyph_index);
                        if k != 0.0 {
                            v.push((i, j, k * opts.font_scale));
                        }
                    }
                    v
                });
                kerning = found.into_iter().flatten().collect();
            } else {
                // Format-0 pairs: first listed value for a pair wins (as in a lookup).
                let mut seen = std::collections::HashSet::new();
                for (l, r, v) in pairs {
                    if let (Some(&i), Some(&j)) = (slot.get(&l), slot.get(&r)) {
                        if seen.insert((i, j)) {
                            kerning.push((i, j, v * opts.font_scale));
                        }
                    }
                }
                kerning.sort_by_key(|&(i, j, _)| (i, j));
            }
        }
        Ok(FontGeometry {
            glyphs,
            metrics,
            kerning,
            missing,
            kerning_incomplete,
        })
    }
}

/// Uniform grid layout options.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GridConfig {
    /// Number of columns (default: near-square grid).
    pub columns: Option<u32>,
    /// Cell size in pixels (default: the union of all glyph boxes).
    pub cell: Option<(u32, u32)>,
}

/// Distance range specification.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RangeSpec {
    /// In atlas pixels (scales with the glyph size).
    Pixels(Range),
    /// In ems (times `font_scale`).
    Em(Range),
}

/// Atlas generation options.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AtlasConfig {
    pub atlas_type: AtlasType,
    /// Glyph scale in pixels per em. If `None` with fixed `dimensions`, the largest scale that
    /// fits is used (at least `min_size`); otherwise `min_size` or 32.
    pub size: Option<f64>,
    pub min_size: Option<f64>,
    /// Fixed atlas dimensions; chosen automatically under `constraint` if `None`.
    pub dimensions: Option<(u32, u32)>,
    pub constraint: DimensionsConstraint,
    pub range: RangeSpec,
    /// Extra padding around each glyph box, in pixels and in ems.
    pub px_padding: f64,
    pub em_padding: f64,
    /// Empty pixels between packed glyph boxes.
    pub spacing: u32,
    /// Align glyph origins to the pixel grid.
    pub px_align: bool,
    /// Room for the miters of sharp corners, as a multiple of half the distance range
    /// (0 disables; msdf-atlas-gen's default is 1).
    pub miter_limit: f64,
    pub y_origin: YOrigin,
    /// Uniform grid instead of tight packing.
    pub grid: Option<GridConfig>,
    /// Distance field generation options (error correction, overlap support, ...).
    pub generator: GeneratorConfig,
    /// Worker threads for glyph generation (0 = all cores).
    pub threads: usize,
}

impl Default for AtlasConfig {
    fn default() -> Self {
        AtlasConfig {
            atlas_type: AtlasType::Msdf,
            size: None,
            min_size: None,
            dimensions: None,
            constraint: DimensionsConstraint::default(),
            range: RangeSpec::Pixels(Range::symmetric(2.0)),
            px_padding: 0.0,
            em_padding: 0.0,
            spacing: 0,
            px_align: true,
            miter_limit: 1.0,
            y_origin: YOrigin::Bottom,
            grid: None,
            generator: GeneratorConfig::default(),
            threads: 0,
        }
    }
}

/// Atlas generation error.
#[derive(Clone, Debug, PartialEq)]
pub enum AtlasError {
    /// The glyphs don't fit into the requested dimensions.
    DoesNotFit {
        width: u32,
        height: u32,
    },
    /// No glyph has an outline.
    NoGlyphs,
    InvalidConfig(String),
}

impl std::fmt::Display for AtlasError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AtlasError::DoesNotFit { width, height } => {
                write!(f, "glyphs do not fit into a {width}x{height} atlas")
            }
            AtlasError::NoGlyphs => write!(f, "no glyph has an outline"),
            AtlasError::InvalidConfig(m) => write!(f, "invalid configuration: {m}"),
        }
    }
}

impl std::error::Error for AtlasError {}

/// A glyph's box in the atlas.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlyphPlacement {
    /// Position and size in atlas pixels (rows from the top).
    pub rect: pack::Rect,
    /// Shape-to-box transform (the box's own bottom-left is texel origin).
    pub transform: Transform,
}

/// Result of laying out the atlas.
#[derive(Clone, Debug, PartialEq)]
pub struct AtlasLayout {
    pub width: u32,
    pub height: u32,
    /// Pixels per em.
    pub scale: f64,
    /// Distance range in ems.
    pub range: Range,
    /// Per glyph (same order as `FontGeometry::glyphs`); `None` for empty glyphs.
    pub placements: Vec<Option<GlyphPlacement>>,
}

/// Bounds rectangle used in layouts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoundsRect {
    pub left: f64,
    pub bottom: f64,
    pub right: f64,
    pub top: f64,
}

impl AtlasLayout {
    /// The quad to draw for a glyph, in ems relative to the glyph origin (texel-center inset),
    /// following the layout's y origin.
    pub fn plane_bounds(&self, i: usize, y_origin: YOrigin) -> Option<BoundsRect> {
        let p = self.placements[i]?;
        let s = p.transform.projection.scale.x;
        let t = p.transform.projection.translate;
        let (w, h) = (p.rect.w as f64, p.rect.h as f64);
        let (l, r) = (0.5 / s - t.x, (w - 0.5) / s - t.x);
        let (b, top) = (0.5 / s - t.y, (h - 0.5) / s - t.y);
        Some(match y_origin {
            YOrigin::Bottom => BoundsRect {
                left: l,
                bottom: b,
                right: r,
                top,
            },
            YOrigin::Top => BoundsRect {
                left: l,
                bottom: -b,
                right: r,
                top: -top,
            },
        })
    }

    /// The glyph's texel-center inset rectangle in atlas pixels.
    pub fn atlas_bounds(&self, i: usize, y_origin: YOrigin) -> Option<BoundsRect> {
        let p = self.placements[i]?;
        let r = p.rect;
        let (l, right) = (r.x as f64 + 0.5, (r.x + r.w) as f64 - 0.5);
        Some(match y_origin {
            YOrigin::Bottom => {
                let bottom = (self.height - r.y - r.h) as f64 + 0.5;
                let top = (self.height - r.y) as f64 - 0.5;
                BoundsRect {
                    left: l,
                    bottom,
                    right,
                    top,
                }
            }
            YOrigin::Top => BoundsRect {
                left: l,
                bottom: (r.y + r.h) as f64 - 0.5,
                right,
                top: r.y as f64 + 0.5,
            },
        })
    }
}

/// Area a glyph's tile must cover, in shape units: the outline expanded by half the
/// distance range, plus the miters of sharp convex corners (limited by `miter_limit`).
fn glyph_extent(g: &GlyphGeometry, range: Range, miter_limit: f64) -> Bounds {
    let half = -range.lower;
    let b = &g.bounds;
    let mut out = Bounds {
        left: b.left - half,
        bottom: b.bottom - half,
        right: b.right + half,
        top: b.top + half,
    };
    if miter_limit > 0.0 {
        g.shape.bound_miters(&mut out, half, miter_limit, 1);
    }
    out
}

/// Box covering `extent` (plus padding) at a given scale: size in pixels and translation in
/// shape units.
fn glyph_box(extent: &Bounds, scale: f64, px_padding: f64, em_padding: f64, px_align: bool) -> (u32, u32, Vec2) {
    let pad = em_padding + px_padding / scale;
    let (l, b, r, t) = (
        extent.left - pad,
        extent.bottom - pad,
        extent.right + pad,
        extent.top + pad,
    );
    if px_align {
        // Half a texel of margin so that the quad between texel centers still spans the
        // whole extent.
        let (x0, x1) = ((l * scale - 0.5).floor(), (r * scale + 0.5).ceil());
        let (y0, y1) = ((b * scale - 0.5).floor(), (t * scale + 0.5).ceil());
        let w = (x1 - x0).max(1.0) as u32;
        let h = (y1 - y0).max(1.0) as u32;
        (w, h, Vec2::new(-x0 / scale, -y0 / scale))
    } else {
        let (fw, fh) = ((r - l) * scale, (t - b) * scale);
        let w = (fw + 1.0).ceil() as u32;
        let h = (fh + 1.0).ceil() as u32;
        let tx = -l + 0.5 * (w as f64 - fw) / scale;
        let ty = -b + 0.5 * (h as f64 - fh) / scale;
        (w, h, Vec2::new(tx, ty))
    }
}

fn range_at(spec: RangeSpec, scale: f64) -> Range {
    match spec {
        RangeSpec::Pixels(r) => r.px_to_units(scale),
        RangeSpec::Em(r) => r,
    }
}

/// Tries to lay out all glyphs at `scale`. With fixed dimensions, returns `None` if they
/// don't fit.
fn layout_at(font: &FontGeometry, cfg: &AtlasConfig, scale: f64) -> Option<AtlasLayout> {
    let range = range_at(cfg.range, scale);
    let n = font.glyphs.len();
    let extents: Vec<Option<Bounds>> = font
        .glyphs
        .iter()
        .map(|g| (!g.is_empty()).then(|| glyph_extent(g, range, cfg.miter_limit)))
        .collect();
    let boxes: Vec<Option<(u32, u32, Vec2)>> = extents
        .iter()
        .map(|e| e.map(|e| glyph_box(&e, scale, cfg.px_padding, cfg.em_padding, cfg.px_align)))
        .collect();
    let mut placements = vec![None; n];
    let make = |rect: pack::Rect, translate: Vec2| GlyphPlacement {
        rect,
        transform: Transform::new(Projection::uniform(scale, translate), range),
    };
    let (width, height);
    if let Some(grid) = cfg.grid {
        // Uniform cells sharing one origin: the union of all glyph boxes.
        let mut u = Bounds::EMPTY;
        for e in extents.iter().flatten() {
            u.include(Vec2::new(e.left, e.bottom), Vec2::new(e.right, e.top));
        }
        if u.is_empty() {
            return Some(AtlasLayout {
                width: 1,
                height: 1,
                scale,
                range,
                placements,
            });
        }
        let (mut cw, mut ch, mut tr) = glyph_box(&u, scale, cfg.px_padding, cfg.em_padding, cfg.px_align);
        if let Some((fw, fh)) = grid.cell {
            if fw < cw || fh < ch {
                return None;
            }
            // Center the union box in the requested cell.
            tr += Vec2::new((fw - cw) as f64 * 0.5 / scale, (fh - ch) as f64 * 0.5 / scale);
            cw = fw;
            ch = fh;
        }
        let count = boxes.iter().filter(|b| b.is_some()).count() as u32;
        let cols = grid
            .columns
            .unwrap_or_else(|| (count as f64).sqrt().ceil() as u32)
            .max(1);
        let rows = count.div_ceil(cols).max(1);
        let sp = cfg.spacing;
        // Sizes come from the configuration: too large a grid doesn't fit (rather than wrap).
        let extent = |n: u32, cell: u32| n.checked_mul(cell.checked_add(sp)?).map(|v| v - sp);
        let need = (extent(cols, cw)?, extent(rows, ch)?);
        (width, height) = match cfg.dimensions {
            Some((w, h)) if w < need.0 || h < need.1 => return None,
            Some(d) => d,
            None => need,
        };
        let mut k = 0;
        for (i, b) in boxes.iter().enumerate() {
            if b.is_some() {
                let rect = pack::Rect {
                    x: (k % cols) * (cw + sp),
                    y: (k / cols) * (ch + sp),
                    w: cw,
                    h: ch,
                };
                placements[i] = Some(make(rect, tr));
                k += 1;
            }
        }
    } else {
        let sp = cfg.spacing;
        let sizes: Vec<(u32, u32)> = boxes
            .iter()
            .map(|b| b.map_or(Some((0, 0)), |(w, h, _)| Some((w.checked_add(sp)?, h.checked_add(sp)?))))
            .collect::<Option<_>>()?;
        let positions;
        (width, height, positions) = if let Some((w, h)) = cfg.dimensions {
            // The spacing after the last column/row is not needed.
            (w, h, pack::pack(&sizes, w.checked_add(sp)?, h.checked_add(sp)?)?)
        } else {
            pack::pack_auto(&sizes, cfg.constraint)
        };
        for (i, b) in boxes.iter().enumerate() {
            if let Some((w, h, tr)) = *b {
                let (x, y) = positions[i];
                placements[i] = Some(make(pack::Rect { x, y, w, h }, tr));
            }
        }
    }
    Some(AtlasLayout {
        width,
        height,
        scale,
        range,
        placements,
    })
}

/// Computes the atlas layout (glyph scale, dimensions and placements).
pub fn layout(font: &FontGeometry, cfg: &AtlasConfig) -> Result<AtlasLayout, AtlasError> {
    if font.glyphs.iter().all(GlyphGeometry::is_empty) {
        return Err(AtlasError::NoGlyphs);
    }
    match (cfg.size, cfg.dimensions) {
        (Some(s), d) => {
            if s.is_nan() || s <= 0.0 {
                return Err(AtlasError::InvalidConfig("size must be positive".into()));
            }
            layout_at(font, cfg, s).ok_or_else(|| {
                let (width, height) = d.unwrap_or((0, 0));
                AtlasError::DoesNotFit { width, height }
            })
        }
        (None, Some((w, h))) => {
            // Largest scale that fits, by bisection.
            let min = cfg.min_size.unwrap_or(1.0 / 64.0).max(1e-6);
            let Some(mut best) = layout_at(font, cfg, min) else {
                return Err(AtlasError::DoesNotFit { width: w, height: h });
            };
            let (mut lo, mut hi) = (min, min * 2.0);
            while let Some(l) = layout_at(font, cfg, hi) {
                best = l;
                lo = hi;
                hi *= 2.0;
                if hi > 1e6 {
                    break;
                }
            }
            for _ in 0..40 {
                let mid = f64::midpoint(lo, hi);
                match layout_at(font, cfg, mid) {
                    Some(l) => {
                        best = l;
                        lo = mid;
                    }
                    None => hi = mid,
                }
                if hi - lo < 1e-4 * lo {
                    break;
                }
            }
            Ok(best)
        }
        (None, None) => {
            let s = cfg.min_size.unwrap_or(32.0);
            layout_at(font, cfg, s).ok_or(AtlasError::DoesNotFit { width: 0, height: 0 })
        }
    }
}

/// An atlas image with the channel count of its type.
#[derive(Clone, Debug, PartialEq)]
pub enum AtlasImage {
    One(Bitmap<1>),
    Three(Bitmap<3>),
    Four(Bitmap<4>),
}

impl AtlasImage {
    pub fn width(&self) -> u32 {
        match self {
            AtlasImage::One(b) => b.width(),
            AtlasImage::Three(b) => b.width(),
            AtlasImage::Four(b) => b.width(),
        }
    }
    pub fn height(&self) -> u32 {
        match self {
            AtlasImage::One(b) => b.height(),
            AtlasImage::Three(b) => b.height(),
            AtlasImage::Four(b) => b.height(),
        }
    }
    /// Saves the image (rows top-to-bottom; FL32 is bottom-to-top as in msdfgen).
    pub fn save(&self, path: impl AsRef<Path>, format: ImageFormat) -> std::io::Result<()> {
        match self {
            AtlasImage::One(b) => save(b, path, format),
            AtlasImage::Three(b) => save(b, path, format),
            AtlasImage::Four(b) => save(b, path, format),
        }
    }
}

/// A generated atlas.
#[derive(Clone, Debug)]
pub struct Atlas {
    pub layout: AtlasLayout,
    pub image: AtlasImage,
}

fn tile<const N: usize>(
    g: &GlyphGeometry,
    p: &GlyphPlacement,
    cfg: &AtlasConfig,
    generator: &GeneratorConfig,
) -> Bitmap<N> {
    let (w, h) = (p.rect.w, p.rect.h);
    let t = &p.transform;
    // Single-channel results in an `N`-channel tile (moved as is when `N` is 1).
    let widen = |b: Bitmap<1>| {
        if N == 1 {
            Bitmap::from_data(w, h, b.into_data())
        } else {
            Bitmap::from_data(w, h, b.into_data().into_iter().flat_map(|v| [v; N]).collect())
        }
    };
    match cfg.atlas_type {
        AtlasType::HardMask | AtlasType::SoftMask => {
            let samples = if cfg.atlas_type == AtlasType::HardMask { 1 } else { 4 };
            widen(rasterize(&g.shape, w, h, &t.projection, FillRule::NonZero, samples))
        }
        AtlasType::Sdf => {
            let mut b = Bitmap::<1>::new(w, h);
            generate_sdf(&mut b, &g.shape, t, generator);
            widen(b)
        }
        AtlasType::Psdf => {
            let mut b = Bitmap::<1>::new(w, h);
            generate_psdf(&mut b, &g.shape, t, generator);
            widen(b)
        }
        // `render` picks `N` from the atlas type, so these always match.
        AtlasType::Msdf if N == 3 => {
            let mut m = Bitmap::<3>::new(w, h);
            generate_msdf(&mut m, &g.shape, t, generator);
            Bitmap::from_data(w, h, m.into_data())
        }
        AtlasType::Mtsdf if N == 4 => {
            let mut m = Bitmap::<4>::new(w, h);
            generate_mtsdf(&mut m, &g.shape, t, generator);
            Bitmap::from_data(w, h, m.into_data())
        }
        AtlasType::Msdf | AtlasType::Mtsdf => Bitmap::new(w, h),
    }
}

fn render_into<const N: usize>(font: &FontGeometry, layout: &AtlasLayout, cfg: &AtlasConfig) -> Bitmap<N> {
    // Glyphs are distributed over threads; each glyph is generated single-threaded.
    let generator = cfg.generator.with_threads(1);
    let jobs: Vec<usize> = (0..font.glyphs.len())
        .filter(|&i| layout.placements[i].is_some())
        .collect();
    // Background: "outside" for fields (value 0 = far outside), empty for masks.
    let atlas = std::sync::Mutex::new(Bitmap::<N>::new(layout.width, layout.height));
    // Each tile is copied in as soon as it is done (a quick copy under the lock), so only the
    // tiles being generated are in memory besides the atlas.
    par_map(&jobs, cfg.threads, |&i| {
        let p = layout.placements[i].unwrap();
        let t = tile::<N>(&font.glyphs[i], &p, cfg, &generator);
        let r = p.rect;
        let mut atlas = atlas.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        for y in 0..r.h {
            let dst = ((r.y + y) as usize * layout.width as usize + r.x as usize) * N;
            atlas.data_mut()[dst..dst + r.w as usize * N].copy_from_slice(t.row(y));
        }
    });
    atlas.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Generates the atlas image for a layout.
pub fn render(font: &FontGeometry, layout: &AtlasLayout, cfg: &AtlasConfig) -> AtlasImage {
    match cfg.atlas_type.channels() {
        3 => AtlasImage::Three(render_into::<3>(font, layout, cfg)),
        4 => AtlasImage::Four(render_into::<4>(font, layout, cfg)),
        _ => AtlasImage::One(render_into::<1>(font, layout, cfg)),
    }
}

/// Lays out and renders the atlas.
pub fn generate(font: &FontGeometry, cfg: &AtlasConfig) -> Result<Atlas, AtlasError> {
    let layout = layout(font, cfg)?;
    let image = render(font, &layout, cfg);
    Ok(Atlas { layout, image })
}

#[cfg(test)]
mod tests {
    use super::*;
    use textra::{edge_coloring_simple, median};

    fn square_font() -> FontGeometry {
        let mut glyphs = Vec::new();
        for (i, s) in [0.5, 1.0, 0.75].iter().enumerate() {
            let mut shape = Shape::from_description(&format!("{{ 0,0; 0,{s}; {s},{s}; {s},0; # }}")).unwrap();
            shape.normalize();
            edge_coloring_simple(&mut shape, DEFAULT_ANGLE_THRESHOLD, 0);
            let bounds = shape.bounds();
            glyphs.push(GlyphGeometry {
                glyph_index: i as u16,
                codepoint: Some(65 + i as u32),
                shape,
                bounds,
                advance: 1.0,
            });
        }
        glyphs.push(GlyphGeometry {
            glyph_index: 9,
            codepoint: Some(32),
            shape: Shape::new(),
            bounds: Bounds::EMPTY,
            advance: 0.3,
        });
        FontGeometry {
            glyphs,
            metrics: Metrics {
                em_size: 1.0,
                line_height: 1.2,
                ascender: 0.9,
                descender: -0.3,
                underline_y: -0.1,
                underline_thickness: 0.05,
            },
            kerning: vec![],
            missing: vec![],
            kerning_incomplete: false,
        }
    }

    #[test]
    fn plane_bounds_cover_glyph_and_map_to_atlas() {
        let font = square_font();
        for px_align in [true, false] {
            let cfg = AtlasConfig {
                size: Some(20.0),
                px_align,
                ..Default::default()
            };
            let l = layout(&font, &cfg).unwrap();
            for (i, g) in font.glyphs.iter().enumerate() {
                let Some(pb) = l.plane_bounds(i, YOrigin::Bottom) else {
                    assert!(g.is_empty());
                    continue;
                };
                let ab = l.atlas_bounds(i, YOrigin::Bottom).unwrap();
                // The quad (between texel centers) covers the glyph plus half the distance range.
                let half = -l.range.lower;
                assert!(pb.left <= g.bounds.left - half + 1e-9 && pb.right >= g.bounds.right + half - 1e-9);
                assert!(pb.bottom <= g.bounds.bottom - half + 1e-9 && pb.top >= g.bounds.top + half - 1e-9);
                // Quad and atlas rectangle have the same size in pixels.
                assert!(((pb.right - pb.left) * l.scale - (ab.right - ab.left)).abs() < 1e-9);
                assert!(((pb.top - pb.bottom) * l.scale - (ab.top - ab.bottom)).abs() < 1e-9);
            }
        }
    }

    #[test]
    fn fixed_dimensions_maximize_scale() {
        let font = square_font();
        let cfg = AtlasConfig {
            dimensions: Some((64, 64)),
            ..Default::default()
        };
        let l = layout(&font, &cfg).unwrap();
        assert_eq!((l.width, l.height), (64, 64));
        assert!(l.scale > 20.0, "scale {}", l.scale);
        let bigger = AtlasConfig {
            size: Some(l.scale * 1.05),
            ..cfg
        };
        assert!(layout(&font, &bigger).is_err());
    }

    #[test]
    fn rendered_atlas_reproduces_glyphs() {
        let font = square_font();
        for grid in [None, Some(GridConfig::default())] {
            let cfg = AtlasConfig {
                size: Some(24.0),
                grid,
                spacing: 1,
                ..Default::default()
            };
            let atlas = generate(&font, &cfg).unwrap();
            let AtlasImage::Three(img) = &atlas.image else { panic!() };
            for (i, g) in font.glyphs.iter().enumerate() {
                let Some(p) = atlas.layout.placements[i] else {
                    continue;
                };
                // The texel at the center of the glyph is inside.
                let c = Vec2::new(
                    f64::midpoint(g.bounds.left, g.bounds.right),
                    f64::midpoint(g.bounds.bottom, g.bounds.top),
                );
                let px = p.transform.projection.project(c);
                let (x, y) = (p.rect.x + px.x as u32, p.rect.y + p.rect.h - 1 - px.y as u32);
                let v = img.pixel(x, y);
                assert!(median(v[0], v[1], v[2]) > 0.5);
            }
        }
    }
}
