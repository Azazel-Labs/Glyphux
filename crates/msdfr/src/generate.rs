//! Public distance field generation API.

use std::sync::Mutex;

use crate::bitmap::Bitmap;
use crate::correction::{DistanceCheckMode, ErrorCorrectionConfig, correct_msdf};
use crate::eval::{Dist, Evaluator, PreparedShape};
use crate::math::{Vec2, median};
use crate::shape::{FillRule, Shape};
use crate::transform::{DistanceMapping, Transform};

/// Options shared by all generators.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GeneratorConfig {
    /// Correctly handle shapes made of overlapping contours (e.g. variable fonts).
    /// Has no cost for single-contour shapes.
    pub overlap_support: bool,
    /// MSDF/MTSDF artifact correction.
    pub error_correction: ErrorCorrectionConfig,
    /// If set, a scanline pass fixes the sign of every texel according to this fill rule
    /// (robust against self-intersecting or inconsistently oriented contours).
    pub sign_correction: Option<FillRule>,
    /// Worker threads; 0 = automatic (single-threaded for small jobs).
    pub threads: usize,
}

impl Default for GeneratorConfig {
    fn default() -> Self {
        GeneratorConfig {
            overlap_support: true,
            error_correction: ErrorCorrectionConfig::default(),
            sign_correction: None,
            threads: 0,
        }
    }
}

impl GeneratorConfig {
    pub fn with_threads(mut self, threads: usize) -> Self {
        self.threads = threads;
        self
    }
    pub fn with_error_correction(mut self, ec: ErrorCorrectionConfig) -> Self {
        self.error_correction = ec;
        self
    }
    pub fn with_overlap_support(mut self, on: bool) -> Self {
        self.overlap_support = on;
        self
    }
    pub fn with_sign_correction(mut self, rule: Option<FillRule>) -> Self {
        self.sign_correction = rule;
        self
    }
}

/// Number of threads to use for a job of `cost` (texels x edges).
pub(crate) fn resolve_threads(requested: usize, rows: usize, cost: usize) -> usize {
    let t = if requested == 0 {
        // Spawning threads costs tens of microseconds; only worth it for larger jobs.
        if cost < 400_000 {
            1
        } else {
            crate::batch::available_cores()
        }
    } else {
        requested
    };
    t.clamp(1, rows.max(1))
}

/// Runs `f(row_index, state, row)` over all rows, in parallel when `threads > 1`.
/// Each worker gets its own state from `init`. Rows are handed out in small consecutive
/// blocks so each worker benefits from spatial coherence.
pub(crate) fn par_rows<T: Send, S>(
    rows: Vec<&mut [T]>,
    threads: usize,
    init: impl Fn() -> S + Sync,
    f: impl Fn(usize, &mut S, &mut [T]) + Sync,
) {
    let n = rows.len();
    if threads <= 1 || n <= 1 {
        let mut s = init();
        for (i, row) in rows.into_iter().enumerate() {
            f(i, &mut s, row);
        }
        return;
    }
    let block = (n / (threads * 8)).clamp(1, 8);
    // One queue of rows, taken a block at a time (one lock per block).
    let queue = Mutex::new(rows.into_iter().enumerate());
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                let mut s = init();
                let mut taken = Vec::with_capacity(block);
                loop {
                    taken.extend(
                        queue
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .by_ref()
                            .take(block),
                    );
                    if taken.is_empty() {
                        break;
                    }
                    for (i, row) in taken.drain(..) {
                        f(i, &mut s, row);
                    }
                }
            });
        }
    });
}

#[inline(always)]
fn store<const N: usize, const NC: usize>(px: &mut [f32], d: &Dist, m: &DistanceMapping) {
    match (N, NC) {
        (1, 0) => px[0] = m.map(d[3]) as f32,
        (1, _) => px[0] = m.map(d[0]) as f32,
        (3, _) => {
            px[0] = m.map(d[0]) as f32;
            px[1] = m.map(d[1]) as f32;
            px[2] = m.map(d[2]) as f32;
        }
        (4, _) => {
            px[0] = m.map(d[0]) as f32;
            px[1] = m.map(d[1]) as f32;
            px[2] = m.map(d[2]) as f32;
            px[3] = m.map(d[3]) as f32;
        }
        _ => unreachable!(),
    }
}

/// Side of the square texel tiles used for edge culling.
const TILE: usize = 8;

fn run<const N: usize, const NC: usize, const TD: bool>(
    out: &mut Bitmap<N>,
    shape: &PreparedShape,
    transform: &Transform,
    overlap: bool,
    threads: usize,
) {
    let (w, h) = (out.width() as usize, out.height() as usize);
    if w == 0 || h == 0 {
        return;
    }
    let mapping = transform.mapping();
    let proj = transform.projection;
    let inverse_y = shape.inverse_y_axis;
    let bands = h.div_ceil(TILE);
    let threads = resolve_threads(threads, bands, w * h * shape.edge_count().max(1));
    let chunks: Vec<&mut [f32]> = out.data_mut().chunks_mut(w * N * TILE).collect();
    par_rows(
        chunks,
        threads,
        || Evaluator::<NC, TD>::new(shape, overlap),
        |band, ev, chunk| {
            let r0 = band * TILE;
            let rows = chunk.len() / (w * N);
            // Texel-space y of the band's rows (row 0 = top).
            let ty = |r: usize| if inverse_y { r } else { h - 1 - r } as f64 + 0.5;
            let (ya, yb) = (ty(r0), ty(r0 + rows - 1));
            for (ti, x0) in (0..w).step_by(TILE).enumerate() {
                let x1 = (x0 + TILE).min(w);
                // Tile rectangle spanned by the texel centers, in shape units.
                let pa = proj.unproject(Vec2::new(x0 as f64 + 0.5, ya));
                let pb = proj.unproject(Vec2::new((x1 - 1) as f64 + 0.5, yb));
                ev.set_tile(pa.min(pb), pa.max(pb));
                // Serpentine order inside the tile keeps consecutive queries adjacent.
                for rr in 0..rows {
                    let y = ty(r0 + rr);
                    let row = &mut chunk[rr * w * N..(rr + 1) * w * N];
                    if (rr + ti) % 2 == 0 {
                        for x in x0..x1 {
                            let d = ev.distance(proj.unproject(Vec2::new(x as f64 + 0.5, y)));
                            store::<N, NC>(&mut row[x * N..x * N + N], &d, &mapping);
                        }
                    } else {
                        for x in (x0..x1).rev() {
                            let d = ev.distance(proj.unproject(Vec2::new(x as f64 + 0.5, y)));
                            store::<N, NC>(&mut row[x * N..x * N + N], &d, &mapping);
                        }
                    }
                }
            }
        },
    );
}

/// Generates a conventional (true) signed distance field.
pub fn generate_sdf(out: &mut Bitmap<1>, shape: &Shape, transform: &Transform, config: &GeneratorConfig) {
    let ps = PreparedShape::new(shape);
    generate_sdf_prepared(out, &ps, shape, transform, config);
}

/// [`generate_sdf`] for an already prepared shape (`shape` is used for the optional scanline pass).
pub fn generate_sdf_prepared(
    out: &mut Bitmap<1>,
    ps: &PreparedShape,
    shape: &Shape,
    transform: &Transform,
    config: &GeneratorConfig,
) {
    run::<1, 0, true>(out, ps, transform, config.overlap_support, config.threads);
    if let Some(rule) = config.sign_correction {
        distance_sign_correction(out, shape, transform, rule);
    }
}

/// Generates a single-channel signed pseudo-distance field (sharp corners, like a single
/// channel of an MSDF where all edges have the same color).
pub fn generate_psdf(out: &mut Bitmap<1>, shape: &Shape, transform: &Transform, config: &GeneratorConfig) {
    let ps = PreparedShape::new(shape);
    generate_psdf_prepared(out, &ps, shape, transform, config);
}

pub fn generate_psdf_prepared(
    out: &mut Bitmap<1>,
    ps: &PreparedShape,
    shape: &Shape,
    transform: &Transform,
    config: &GeneratorConfig,
) {
    run::<1, 1, false>(out, ps, transform, config.overlap_support, config.threads);
    if let Some(rule) = config.sign_correction {
        distance_sign_correction(out, shape, transform, rule);
    }
}

/// Generates a multi-channel signed distance field. The shape must be colored
/// (see [`crate::edge_coloring_simple`]).
pub fn generate_msdf(out: &mut Bitmap<3>, shape: &Shape, transform: &Transform, config: &GeneratorConfig) {
    let ps = PreparedShape::new(shape);
    generate_msdf_prepared(out, &ps, shape, transform, config);
}

pub fn generate_msdf_prepared(
    out: &mut Bitmap<3>,
    ps: &PreparedShape,
    shape: &Shape,
    transform: &Transform,
    config: &GeneratorConfig,
) {
    run::<3, 3, false>(out, ps, transform, config.overlap_support, config.threads);
    post_process(out, ps, shape, transform, config);
}

/// Generates a multi-channel signed distance field with the true distance in the alpha channel.
pub fn generate_mtsdf(out: &mut Bitmap<4>, shape: &Shape, transform: &Transform, config: &GeneratorConfig) {
    let ps = PreparedShape::new(shape);
    generate_mtsdf_prepared(out, &ps, shape, transform, config);
}

pub fn generate_mtsdf_prepared(
    out: &mut Bitmap<4>,
    ps: &PreparedShape,
    shape: &Shape,
    transform: &Transform,
    config: &GeneratorConfig,
) {
    run::<4, 3, true>(out, ps, transform, config.overlap_support, config.threads);
    post_process(out, ps, shape, transform, config);
}

fn post_process<const N: usize>(
    out: &mut Bitmap<N>,
    ps: &PreparedShape,
    shape: &Shape,
    transform: &Transform,
    config: &GeneratorConfig,
) {
    let mut ec = config.error_correction;
    if let Some(rule) = config.sign_correction {
        let changed = distance_sign_correction(out, shape, transform, rule);
        // If signs had to be fixed, the exact distances disagree with the field there and
        // distance-checked correction would undo the fix.
        if changed > 0 {
            ec.distance_check = DistanceCheckMode::DoNotCheck;
        }
    }
    // Error correction costs roughly the same per texel regardless of the shape.
    let threads = resolve_threads(
        config.threads,
        out.height() as usize,
        (out.width() * out.height()) as usize * 20,
    );
    correct_msdf(out, ps, transform, &ec, config.overlap_support, threads);
}

/// Fixes the sign of each texel according to `fill_rule` evaluated with exact scanline
/// intersections of the shape. Returns the number of texels changed.
pub fn distance_sign_correction<const N: usize>(
    sdf: &mut Bitmap<N>,
    shape: &Shape,
    transform: &Transform,
    fill_rule: FillRule,
) -> usize {
    let (w, h) = (sdf.width() as usize, sdf.height() as usize);
    let z = transform.zero_value();
    let proj = transform.projection;
    let mut changed = 0;
    let mut crossings = Vec::new();
    // match: 1 = sign agrees, -1 = inverted, 0 = ambiguous (exactly on the boundary)
    let mut matches = vec![0i8; w * h];
    let mut ambiguous = false;
    for r in 0..h {
        let ty = if shape.inverse_y_axis { r } else { h - 1 - r } as f64 + 0.5;
        let y = proj.unproject(Vec2::new(0.0, ty)).y;
        shape.scanline(y, &mut crossings);
        let total: i32 = crossings.iter().map(|c| c.1).sum();
        let mut left = 0i32;
        let mut k = 0;
        for x in 0..w {
            let sx = proj.unproject(Vec2::new(x as f64 + 0.5, ty)).x;
            while k < crossings.len() && crossings[k].0 <= sx {
                left += crossings[k].1;
                k += 1;
            }
            let winding = -(total - left);
            let fill = fill_rule.is_filled(winding);
            let px = sdf.pixel_mut(x as u32, r as u32);
            let sd = if N >= 3 { median(px[0], px[1], px[2]) } else { px[0] };
            let m = &mut matches[r * w + x];
            if sd == z {
                ambiguous = true;
            } else if (sd > z) != fill {
                for c in px.iter_mut().take(if N >= 3 { 3 } else { 1 }) {
                    *c = 2.0 * z - *c;
                }
                *m = -1;
                changed += 1;
            } else {
                *m = 1;
            }
            if N >= 4 && (px[3] > z) != fill {
                px[3] = 2.0 * z - px[3];
            }
        }
    }
    if ambiguous {
        for r in 0..h {
            for x in 0..w {
                if matches[r * w + x] != 0 {
                    continue;
                }
                let mut s = 0i32;
                if x > 0 {
                    s += matches[r * w + x - 1] as i32;
                }
                if x + 1 < w {
                    s += matches[r * w + x + 1] as i32;
                }
                if r > 0 {
                    s += matches[(r - 1) * w + x] as i32;
                }
                if r + 1 < h {
                    s += matches[(r + 1) * w + x] as i32;
                }
                if s < 0 {
                    let px = sdf.pixel_mut(x as u32, r as u32);
                    for c in px.iter_mut().take(if N >= 3 { 3 } else { 1 }) {
                        *c = 2.0 * z - *c;
                    }
                    changed += 1;
                }
            }
        }
    }
    changed
}

/// Signed pseudo-distance (in shape units) from `p` to the shape - useful for testing and
/// for custom evaluation.
pub fn shape_distance(ps: &PreparedShape, p: Vec2, overlap: bool) -> f64 {
    let mut ev = Evaluator::<1, false>::new(ps, overlap);
    ev.distance(p)[0]
}

/// Exact true signed distance (in shape units) from `p` to the shape.
pub fn shape_true_distance(ps: &PreparedShape, p: Vec2, overlap: bool) -> f64 {
    let mut ev = Evaluator::<0, true>::new(ps, overlap);
    ev.distance(p)[3]
}
