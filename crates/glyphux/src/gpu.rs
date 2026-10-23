//! Flat, GPU-friendly forms of a [`PreparedShape`] and its tile culling, for evaluating
//! distance fields on a GPU (used by the `glyphux-gpu` crate).
//!
//! All records are `#[repr(C)]` plain data made of 32-bit words with no padding, laid out to
//! match the WGSL structs of `glyphux-gpu`; they derive [`bytemuck::Pod`] (which checks that at
//! compile time), so `bytemuck::cast_slice` gives their bytes for uploading.
//!
//! # Cubic curve data
//!
//! A cubic edge's `curve` field is the offset (in `f32`s) of its record in
//! [`GpuShape::curves`]: `n` (sample count), `p0, a, b, c` (power basis
//! `B(t) = p0 + a t + b t^2 + c t^3`), `k1..k5` (point-independent coefficients of
//! `f = (B - p).B'`), `n` samples `(t, x, y)`, then `n - 1` intervals of 22 values: chord
//! `(x, y)`, its inverse squared length, the chord sag bound, and the Bernstein coefficients
//! (6 each) of the parts `F`, `Gx`, `Gy` of `f = F + q.x Gx + q.y Gy` with `q = p0 - p`.

use bytemuck::{Pod, Zeroable};

use crate::eval::{GeomData, PreparedShape};
use crate::math::Vec2;
use crate::transform::Transform;

/// Edge kinds in [`GpuEdge::kind`] (low byte).
pub const EDGE_LINE: u32 = 0;
pub const EDGE_QUAD: u32 = 1;
pub const EDGE_CUBIC: u32 = 2;

/// Distances standing in for "infinite" (squares stay finite in `f32`).
pub const BIG: f32 = 1e18;

/// One edge with the data the GPU kernels need (128 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct GpuEdge {
    /// Kind in bits 0-7, color mask (red 1, green 2, blue 4) in bits 8-15.
    pub kind: u32,
    /// Offset of the curve record in [`GpuShape::curves`] (cubics only).
    pub curve: u32,
    /// Squared lower-bound factors for corner perpendicular candidates ([`BIG`] if none).
    pub cos_a2: f32,
    pub cos_b2: f32,
    /// Control points (unused ones zero).
    pub p: [[f32; 2]; 4],
    pub bmin: [f32; 2],
    pub bmax: [f32; 2],
    /// Unit tangents at the start and end.
    pub dir0: [f32; 2],
    pub dir1: [f32; 2],
    /// Unit bisectors of the corners at the start and end.
    pub bis_a: [f32; 2],
    pub bis_b: [f32; 2],
    /// Kernel constants: lines `1/|ab|^2, 1/|ab|`; quadratics `ca, cb, cc0`.
    pub k: [f32; 4],
    /// End point.
    pub end: [f32; 2],
    /// Index of the contour.
    pub contour: u32,
    pub _pad: u32,
}

/// A contour: a range of edges and its winding (128-bit aligned).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct GpuContour {
    pub start: u32,
    pub end: u32,
    pub winding: i32,
    pub _pad: u32,
}

/// The GPU form of a prepared shape.
#[derive(Clone, Debug, Default)]
pub struct GpuShape {
    pub edges: Vec<GpuEdge>,
    pub contours: Vec<GpuContour>,
    pub curves: Vec<f32>,
}

fn v2(v: Vec2) -> [f32; 2] {
    [v.x as f32, v.y as f32]
}

fn big(x: f64) -> f32 {
    if x.is_finite() && x < BIG as f64 { x as f32 } else { BIG }
}

impl GpuShape {
    pub fn new(shape: &PreparedShape) -> Self {
        let mut out = GpuShape::default();
        for (ci, c) in shape.contours.iter().enumerate() {
            out.contours.push(GpuContour {
                start: c.start,
                end: c.end,
                winding: c.winding,
                _pad: 0,
            });
            for e in &shape.edges[c.start as usize..c.end as usize] {
                let mut g = GpuEdge {
                    kind: (e.color as u32) << 8,
                    cos_a2: big(e.cos_a2),
                    cos_b2: big(e.cos_b2),
                    bmin: v2(e.bmin),
                    bmax: v2(e.bmax),
                    dir0: v2(e.dir0),
                    dir1: v2(e.dir1),
                    bis_a: v2(e.bis_a),
                    bis_b: v2(e.bis_b),
                    end: v2(e.end),
                    contour: ci as u32,
                    ..Default::default()
                };
                for (i, p) in e.p.iter().enumerate() {
                    g.p[i] = v2(*p);
                }
                match &e.geom {
                    GeomData::Line(_) => {
                        let ab = e.p[1] - e.p[0];
                        let l2 = ab.length_squared();
                        g.kind |= EDGE_LINE;
                        if l2 > 0.0 {
                            g.k = [(1.0 / l2) as f32, (1.0 / l2.sqrt()) as f32, 0.0, 0.0];
                        }
                    }
                    GeomData::Quad(q) => {
                        g.kind |= EDGE_QUAD;
                        let [a, b, c] = q.gpu_coeffs();
                        g.k = [a as f32, b as f32, c as f32, 0.0];
                    }
                    GeomData::Curve(cd) => {
                        g.kind |= EDGE_CUBIC;
                        g.curve = out.curves.len() as u32;
                        cd.gpu_write(&mut out.curves);
                    }
                }
                out.edges.push(g);
            }
        }
        if out.curves.is_empty() {
            out.curves.push(0.0); // never bind an empty buffer
        }
        out
    }
}

/// Which quantities a field needs: `channels` pseudo-distance channels (0, 1 or 3) and/or
/// the true distance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FieldKind {
    pub channels: u32,
    pub true_distance: bool,
}

impl FieldKind {
    pub const SDF: FieldKind = FieldKind {
        channels: 0,
        true_distance: true,
    };
    pub const PSDF: FieldKind = FieldKind {
        channels: 1,
        true_distance: false,
    };
    pub const MSDF: FieldKind = FieldKind {
        channels: 3,
        true_distance: false,
    };
    pub const MTSDF: FieldKind = FieldKind {
        channels: 3,
        true_distance: true,
    };
}

/// Candidate edges of one group (a contour, or the whole shape) in one tile (32 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct GpuTileGroup {
    /// Range in [`TileBins::indices`] / [`TileBins::keys`].
    pub offset: u32,
    pub count: u32,
    pub _pad: [u32; 2],
    /// Squared upper bounds of the nearest distance per channel (slot 3: true distance) over
    /// the tile, already widened for `f32` evaluation: valid initial culling thresholds.
    pub ub: [f32; 4],
}

/// Per-tile candidate lists, like the CPU evaluator's tile culling.
#[derive(Clone, Debug, Default)]
pub struct TileBins {
    pub tile: u32,
    pub tiles_x: u32,
    pub tiles_y: u32,
    /// Groups per tile: the contours with overlap support (and more than one contour), else 1.
    pub groups: u32,
    /// `tiles_x * tiles_y * groups` records, tile-major (row of tiles, then column).
    pub ranges: Vec<GpuTileGroup>,
    /// Edge indices, each group's sorted by key.
    pub indices: Vec<u32>,
    /// Squared lower bound (over the tile) of anything the edge can contribute.
    pub keys: Vec<f32>,
    /// Per tile: the smallest key of any edge (squared), i.e. a lower bound of the distance
    /// from the tile to the shape (used for narrow-band evaluation).
    pub min_key: Vec<f32>,
}

/// Relative widening of bounds converted to `f32` (covers `f32` evaluation error).
const F32_MARGIN: f64 = 1e-4;

/// Squared distance from the rectangle `[lo, hi]` to `p` (0 inside).
fn min_dist2(lo: Vec2, hi: Vec2, p: Vec2) -> f64 {
    let dx = (lo.x - p.x).max(p.x - hi.x).max(0.0);
    let dy = (lo.y - p.y).max(p.y - hi.y).max(0.0);
    dx * dx + dy * dy
}

/// Squared distance from the farthest point of the rectangle `[lo, hi]` to `p`.
fn max_dist2(lo: Vec2, hi: Vec2, p: Vec2) -> f64 {
    let dx = (p.x - lo.x).abs().max((hi.x - p.x).abs());
    let dy = (p.y - lo.y).abs().max((hi.y - p.y).abs());
    dx * dx + dy * dy
}

/// Squared distance between two axis-aligned rectangles.
fn rect_dist2(lo: Vec2, hi: Vec2, blo: Vec2, bhi: Vec2) -> f64 {
    let dx = (blo.x - hi.x).max(lo.x - bhi.x).max(0.0);
    let dy = (blo.y - hi.y).max(lo.y - bhi.y).max(0.0);
    dx * dx + dy * dy
}

/// Builds per-tile candidate lists for a `width` x `height` field of `shape` under
/// `transform` (rows top to bottom, as [`crate::Bitmap`]), with `tile` x `tile` texel tiles.
pub fn bin_tiles(
    shape: &PreparedShape,
    transform: &Transform,
    width: u32,
    height: u32,
    tile: u32,
    kind: FieldKind,
    overlap: bool,
) -> TileBins {
    let edges = &shape.edges;
    let groups: Vec<(u32, u32)> = if overlap && shape.contours.len() > 1 {
        shape.contours.iter().map(|c| (c.start, c.end)).collect()
    } else {
        vec![(0, edges.len() as u32)]
    };
    let tiles_x = width.div_ceil(tile);
    let tiles_y = height.div_ceil(tile);
    let mut bins = TileBins {
        tile,
        tiles_x,
        tiles_y,
        groups: groups.len() as u32,
        ..Default::default()
    };
    let proj = transform.projection;
    let (w, h) = (width as usize, height as usize);
    let ty = |r: usize| if shape.inverse_y_axis { r } else { h - 1 - r } as f64 + 0.5;
    let nc = kind.channels;
    let mut buf: Vec<(f64, u32)> = Vec::new();
    for tyi in 0..tiles_y as usize {
        let r0 = tyi * tile as usize;
        let r1 = (r0 + tile as usize).min(h);
        for txi in 0..tiles_x as usize {
            let x0 = txi * tile as usize;
            let x1 = (x0 + tile as usize).min(w);
            let pa = proj.unproject(Vec2::new(x0 as f64 + 0.5, ty(r0)));
            let pb = proj.unproject(Vec2::new((x1 - 1) as f64 + 0.5, ty(r1 - 1)));
            let (lo, hi) = (pa.min(pb), pa.max(pb));
            let mut tile_min = f64::INFINITY;
            for &(s, e) in &groups {
                // Upper bounds of the nearest true distance per channel over the tile.
                let mut ub = [f64::MAX; 4];
                for e in &edges[s as usize..e as usize] {
                    let d = max_dist2(lo, hi, e.start)
                        .min(max_dist2(lo, hi, e.mid))
                        .min(max_dist2(lo, hi, e.end));
                    for c in 0..3 {
                        if e.color & (1 << c) != 0 && d < ub[c] {
                            ub[c] = d;
                        }
                    }
                    ub[3] = ub[3].min(d);
                }
                buf.clear();
                for i in s..e {
                    let e = &edges[i as usize];
                    let mask: u8 = match nc {
                        3 => e.color,
                        1 => 1,
                        _ => 0,
                    };
                    let mut thr2: f64 = if kind.true_distance || nc == 1 { ub[3] } else { 0.0 };
                    if nc == 3 {
                        for c in 0..3 {
                            if mask & (1 << c) != 0 {
                                thr2 = thr2.max(ub[c]);
                            }
                        }
                    }
                    let mut key = rect_dist2(lo, hi, e.bmin, e.bmax);
                    if nc > 0 {
                        if e.cos_a > 0.0 {
                            key = key.min(min_dist2(lo, hi, e.start) * e.cos_a * e.cos_a);
                        }
                        if e.cos_b > 0.0 {
                            key = key.min(min_dist2(lo, hi, e.end) * e.cos_b * e.cos_b);
                        }
                    }
                    tile_min = tile_min.min(key);
                    if key <= thr2 * (1.0 + F32_MARGIN) {
                        buf.push((key, i));
                    }
                }
                buf.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
                let offset = bins.indices.len() as u32;
                for &(k, i) in &buf {
                    bins.indices.push(i);
                    bins.keys.push((k * (1.0 - F32_MARGIN)) as f32);
                }
                let ubf = |x: f64| {
                    if x >= f64::MAX {
                        BIG * BIG
                    } else {
                        ((x * (1.0 + F32_MARGIN)) as f32).min(BIG * BIG)
                    }
                };
                // Thresholds for channels this group has no edge of stay "infinite".
                bins.ranges.push(GpuTileGroup {
                    offset,
                    count: buf.len() as u32,
                    _pad: [0; 2],
                    ub: [ubf(ub[0]), ubf(ub[1]), ubf(ub[2]), ubf(ub[3])],
                });
            }
            bins.min_key.push(if tile_min.is_finite() {
                (tile_min * (1.0 - F32_MARGIN)) as f32
            } else {
                BIG * BIG
            });
        }
    }
    if bins.indices.is_empty() {
        bins.indices.push(0);
        bins.keys.push(0.0);
    }
    bins
}

/// Row-space texel indices (`row * width + x`) of the texels around every color-changing
/// corner, which MSDF error correction protects (as [`crate::correction::correct_msdf`] does).
/// May contain duplicates.
pub fn corner_texels(shape: &PreparedShape, transform: &Transform, width: u32, height: u32) -> Vec<u32> {
    let (w, h) = (width as i64, height as i64);
    let mut out = Vec::new();
    for c in &shape.contours {
        let (s, e) = (c.start as usize, c.end as usize);
        if e <= s {
            continue;
        }
        let mut prev = &shape.edges[e - 1];
        for edge in &shape.edges[s..e] {
            let common = prev.color & edge.color;
            if common & common.wrapping_sub(1) == 0 {
                let p = transform.projection.project(edge.start);
                let l = (p.x - 0.5).floor() as i64;
                let b = (p.y - 0.5).floor() as i64;
                for (tx, ty) in [(l, b), (l + 1, b), (l, b + 1), (l + 1, b + 1)] {
                    if tx >= 0 && ty >= 0 && tx < w && ty < h {
                        let r = if shape.inverse_y_axis { ty } else { h - 1 - ty };
                        out.push((r * w + tx) as u32);
                    }
                }
            }
            prev = edge;
        }
    }
    out
}

/// Error correction parameters of one field, as the GPU pass uses them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GpuCorrection {
    /// `min_deviation_ratio` times the value change per texel step (horizontal, vertical,
    /// diagonal) of a field with unit gradient.
    pub spans: [f32; 3],
    /// Edge protection radii (same directions).
    pub radii: [f32; 3],
    /// Output value of the shape boundary.
    pub zero: f32,
    pub min_improve_ratio: f32,
    /// Bit flags (see `ec.wgsl` in glyphux-gpu): 1 enabled, 2 protect corners and edges,
    /// 4 protect all, 8 field-only search, 16 distance check, 32 check only flagged texels.
    pub flags: u32,
}

impl GpuCorrection {
    pub fn new(transform: &Transform, cfg: &crate::ErrorCorrectionConfig) -> Self {
        use crate::{DistanceCheckMode as D, ErrorCorrectionMode as M};
        let m = transform.mapping();
        let span = |dx: f64, dy: f64| {
            transform
                .projection
                .unproject_vector(Vec2::new(m.map_delta(dx), m.map_delta(dy)))
                .length()
        };
        let s = [span(1.0, 0.0), span(0.0, 1.0), span(1.0, 1.0)];
        let k = cfg.min_deviation_ratio;
        const PROTECTION_RADIUS_TOLERANCE: f64 = 1.001;
        let mut flags = 0;
        if cfg.mode != M::Disabled {
            flags |= 1;
            match cfg.mode {
                M::EdgePriority => flags |= 2,
                M::EdgeOnly => flags |= 4,
                _ => {}
            }
            let two_phase = cfg.distance_check == D::CheckAtEdge && cfg.mode != M::EdgeOnly;
            if cfg.distance_check == D::DoNotCheck || two_phase {
                flags |= 8;
            }
            if cfg.distance_check != D::DoNotCheck {
                flags |= 16;
            }
            if two_phase {
                flags |= 32;
            }
        }
        GpuCorrection {
            spans: [(k * s[0]) as f32, (k * s[1]) as f32, (k * s[2]) as f32],
            radii: s.map(|v| (PROTECTION_RADIUS_TOLERANCE * v) as f32),
            zero: transform.zero_value(),
            min_improve_ratio: cfg.min_improve_ratio as f32,
            flags,
        }
    }

    pub const DISABLED: GpuCorrection = GpuCorrection {
        spans: [0.0; 3],
        radii: [0.0; 3],
        zero: 0.5,
        min_improve_ratio: 1.0,
        flags: 0,
    };
}

#[cfg(all(test, feature = "font"))]
mod tests {
    use super::*;
    use crate::io::font::{Font, FontScaling};
    use crate::{DEFAULT_ANGLE_THRESHOLD, FillRule, edge_coloring_simple};

    /// Every tile's `min_key` is a lower bound of the distance from its texels to the outline
    /// (the narrow band fills tiles beyond it with one sign). Inter's variable font at heavy
    /// weights has overlapping contours.
    #[test]
    fn min_key_is_a_lower_bound() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/web-demo/assets/InterVariable-Latin.ttf"
        );
        let data = std::fs::read(path).unwrap();
        let (size, tile) = (64u32, 8u32);
        let mut bad = Vec::new();
        for wght in [100.0, 400.0, 900.0] {
            let mut font = Font::from_bytes(&data, 0, FontScaling::EmNormalized).unwrap();
            font.set_variation("wght", wght);
            for c in (33u8..127).map(char::from) {
                let Ok(g) = font.glyph(c) else { continue };
                let mut shape = g.shape;
                if shape.contours.is_empty() {
                    continue;
                }
                shape.normalize();
                shape.orient_contours(FillRule::NonZero);
                edge_coloring_simple(&mut shape, DEFAULT_ANGLE_THRESHOLD, 0);
                let t = Transform::autoframe(shape.bounds(), size, size, 4.0).unwrap();
                let prepared = PreparedShape::new(&shape);
                let bins = bin_tiles(&prepared, &t, size, size, tile, FieldKind::MTSDF, true);
                let ty = |r: u32| if prepared.inverse_y_axis { r } else { size - 1 - r } as f64 + 0.5;
                for ti in 0..bins.tiles_x * bins.tiles_y {
                    let (tx, tr) = (ti % bins.tiles_x, ti / bins.tiles_x);
                    let mut nearest = (f64::INFINITY, 0, 0);
                    for r in tr * tile..((tr + 1) * tile).min(size) {
                        for x in tx * tile..((tx + 1) * tile).min(size) {
                            let p = t.projection.unproject(Vec2::new(x as f64 + 0.5, ty(r)));
                            for (ci, contour) in shape.contours.iter().enumerate() {
                                for (ei, e) in contour.edges.iter().enumerate() {
                                    let d = e.segment.signed_distance(p).0.distance.abs();
                                    if d < nearest.0 {
                                        nearest = (d, ci, ei);
                                    }
                                }
                            }
                        }
                    }
                    let key = f64::from(bins.min_key[ti as usize]);
                    if key > nearest.0 * nearest.0 * (1.0 + 1e-3) + 1e-12 {
                        bad.push(format!(
                            "wght {wght} {c:?} tile ({tx},{tr}): min_key {:.5} > nearest {:.5} (contour {} edge {})",
                            key.sqrt(),
                            nearest.0,
                            nearest.1,
                            nearest.2
                        ));
                    }
                }
            }
        }
        assert!(
            bad.is_empty(),
            "{} tiles:\n{}",
            bad.len(),
            bad[..bad.len().min(15)].join("\n")
        );
    }
}
