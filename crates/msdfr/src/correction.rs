//! MSDF error correction.
//!
//! Bilinear interpolation of a multi-channel field can create points where the median of the
//! interpolated channels differs wildly from the median of the texels - visible as specks or
//! notches. This pass finds texels that cause such artifacts when interpolated with any of
//! their 8 neighbors and flattens them (sets all channels to their median), which locally
//! degrades to a plain SDF. Texels that carry sharp corners or edges are protected unless an
//! exact distance check proves the correction is an improvement.

use crate::bitmap::Bitmap;
use crate::eval::{Evaluator, PreparedShape};
use crate::math::{Vec2, median};
use crate::transform::Transform;

/// Which texels may be corrected.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ErrorCorrectionMode {
    /// No correction.
    Disabled,
    /// Correct all detected artifacts, including at edges and corners.
    Indiscriminate,
    /// Correct artifacts, but avoid texels that are important for edges and corners.
    #[default]
    EdgePriority,
    /// Only correct artifacts at edges (inversions verified by exact distance).
    EdgeOnly,
}

/// When to verify artifact candidates against the exact shape distance.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum DistanceCheckMode {
    /// Never - fastest, purely based on the field's contents.
    DoNotCheck,
    /// Only for texels near edges (protected texels).
    #[default]
    CheckAtEdge,
    /// For all candidates - slowest, most thorough.
    AlwaysCheck,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ErrorCorrectionConfig {
    pub mode: ErrorCorrectionMode,
    pub distance_check: DistanceCheckMode,
    /// Minimum ratio between the actual and the maximum expected distance delta to be
    /// considered an artifact.
    pub min_deviation_ratio: f64,
    /// Minimum ratio between the pre-correction and post-correction distance error for a
    /// correction to be applied (only with distance checks).
    pub min_improve_ratio: f64,
}

impl Default for ErrorCorrectionConfig {
    fn default() -> Self {
        ErrorCorrectionConfig {
            mode: ErrorCorrectionMode::EdgePriority,
            distance_check: DistanceCheckMode::CheckAtEdge,
            min_deviation_ratio: DEFAULT_MIN_DEVIATION_RATIO,
            min_improve_ratio: DEFAULT_MIN_IMPROVE_RATIO,
        }
    }
}

impl ErrorCorrectionConfig {
    pub const DISABLED: ErrorCorrectionConfig = ErrorCorrectionConfig {
        mode: ErrorCorrectionMode::Disabled,
        distance_check: DistanceCheckMode::DoNotCheck,
        min_deviation_ratio: DEFAULT_MIN_DEVIATION_RATIO,
        min_improve_ratio: DEFAULT_MIN_IMPROVE_RATIO,
    };
}

pub const DEFAULT_MIN_DEVIATION_RATIO: f64 = 10.0 / 9.0;
pub const DEFAULT_MIN_IMPROVE_RATIO: f64 = 10.0 / 9.0;

const ERROR: u8 = 1;
const PROTECTED: u8 = 2;
/// Set during the base scan when the texel has sign-inversion candidates that only an exact
/// distance check can decide.
const NEEDS_CHECK: u8 = 4;
const ARTIFACT_T_EPSILON: f64 = 0.01;
const PROTECTION_RADIUS_TOLERANCE: f64 = 1.001;

const CANDIDATE: u8 = 1;
const ARTIFACT: u8 = 2;
const INVERSION: u8 = 4;

/// Decides whether a candidate point found between a texel and a neighbor is an artifact.
trait Classifier {
    /// `dir`: direction to the neighbor in row space; `t`: position along it; `xm`: the
    /// interpolated median; `flags`: result of [`range_test`].
    fn evaluate(&mut self, dir: (f64, f64), t: f64, xm: f32, flags: u8) -> bool;
}

/// Classifies purely by the field contents; remembers whether inversion candidates were seen.
struct BaseClassifier {
    inversion_candidate: bool,
}

impl Classifier for BaseClassifier {
    #[inline(always)]
    fn evaluate(&mut self, _: (f64, f64), _: f64, _: f32, flags: u8) -> bool {
        if flags & INVERSION != 0 {
            self.inversion_candidate = true;
        }
        flags & ARTIFACT != 0
    }
}

/// Read-only view of the field with precomputed medians, plus structure-of-arrays copies of
/// the per-texel quantities used by the vectorized prefilters.
struct Field<'a> {
    data: &'a [f32],
    n: usize,
    w: i64,
    h: i64,
    med: Vec<f32>,
    /// Channel differences `c1-c0`, `c2-c1`, `c0-c2` (the pairs examined for crossings).
    diff: [Vec<f32>; 3],
    /// `|median - zero|`.
    dist: Vec<f32>,
    /// Bits 0-2: channel k above zero; bits 3-5: channel k below zero.
    side: Vec<u8>,
}

impl<'a> Field<'a> {
    fn new<const N: usize>(sdf: &'a Bitmap<N>, z: f32) -> Self {
        let data = sdf.data();
        let len = data.len() / N;
        let mut med = vec![0f32; len];
        let mut diff = [vec![0f32; len], vec![0f32; len], vec![0f32; len]];
        let mut dist = vec![0f32; len];
        let mut side = vec![0u8; len];
        {
            let [d0, d1, d2] = &mut diff;
            for (i, p) in data.chunks_exact(N).enumerate() {
                let m = median(p[0], p[1], p[2]);
                med[i] = m;
                d0[i] = p[1] - p[0];
                d1[i] = p[2] - p[1];
                d2[i] = p[0] - p[2];
                dist[i] = (m - z).abs();
                let mut sb = 0u8;
                for k in 0..3 {
                    sb |= ((p[k] > z) as u8) << k | ((p[k] < z) as u8) << (k + 3);
                }
                side[i] = sb;
            }
        }
        Field {
            data,
            n: N,
            w: sdf.width() as i64,
            h: sdf.height() as i64,
            med,
            diff,
            dist,
            side,
        }
    }
    #[inline(always)]
    fn px(&self, x: i64, r: i64) -> [f32; 3] {
        let i = (r * self.w + x) as usize * self.n;
        [self.data[i], self.data[i + 1], self.data[i + 2]]
    }
    #[inline(always)]
    fn m(&self, x: i64, r: i64) -> f32 {
        self.med[(r * self.w + x) as usize]
    }
    /// Bilinear sample in row space (texel centers at half-integers), first 3 channels.
    #[inline]
    fn sample(&self, x: f64, y: f64) -> [f32; 3] {
        let fx = x - 0.5;
        let fy = y - 0.5;
        let x0 = fx.floor();
        let y0 = fy.floor();
        let tx = (fx - x0) as f32;
        let ty = (fy - y0) as f32;
        let cx = |v: i64| v.clamp(0, self.w - 1);
        let cy = |v: i64| v.clamp(0, self.h - 1);
        let (x0, y0) = (x0 as i64, y0 as i64);
        let a = self.px(cx(x0), cy(y0));
        let b = self.px(cx(x0 + 1), cy(y0));
        let c = self.px(cx(x0), cy(y0 + 1));
        let d = self.px(cx(x0 + 1), cy(y0 + 1));
        let mut out = [0.0; 3];
        for i in 0..3 {
            let top = a[i] + (b[i] - a[i]) * tx;
            let bot = c[i] + (d[i] - c[i]) * tx;
            out[i] = top + (bot - top) * ty;
        }
        out
    }
}

/// Error correction state for one bitmap.
struct Corrector<'a> {
    f: Field<'a>,
    stencil: Vec<u8>,
    /// Per texel: bits `3k..3k+3` - the channel pairs that may produce an artifact candidate
    /// with neighbor direction `k` (in [`texel_has_artifact`] order, see [`artifact_gates`]);
    /// empty until computed.
    dirs: Vec<u32>,
    transform: Transform,
    inverse_y: bool,
    zero: f32,
    min_deviation_ratio: f64,
    min_improve_ratio: f64,
}

impl<'a> Corrector<'a> {
    fn new<const N: usize>(
        sdf: &'a Bitmap<N>,
        transform: &Transform,
        inverse_y: bool,
        cfg: &ErrorCorrectionConfig,
    ) -> Self {
        Corrector {
            f: Field::new(sdf, transform.zero_value()),
            dirs: Vec::new(),
            stencil: vec![0; sdf.width() as usize * sdf.height() as usize],
            transform: *transform,
            inverse_y,
            zero: transform.zero_value(),
            min_deviation_ratio: cfg.min_deviation_ratio,
            min_improve_ratio: cfg.min_improve_ratio,
        }
    }

    #[inline]
    fn st(&mut self, x: i64, r: i64) -> &mut u8 {
        let w = self.f.w;
        &mut self.stencil[(r * w + x) as usize]
    }

    /// Row index of texel-space y (y-up).
    #[inline]
    fn row_of(&self, ty: i64) -> i64 {
        if self.inverse_y { ty } else { self.f.h - 1 - ty }
    }

    /// Value change per texel step along an axis for a field with unit gradient.
    fn span(&self, dx: f64, dy: f64) -> f64 {
        let m = self.transform.mapping();
        let v = self
            .transform
            .projection
            .unproject_vector(Vec2::new(m.map_delta(dx), m.map_delta(dy)));
        v.length()
    }

    fn spans(&self) -> [f64; 3] {
        let k = self.min_deviation_ratio;
        [
            k * self.span(1.0, 0.0),
            k * self.span(0.0, 1.0),
            k * self.span(1.0, 1.0),
        ]
    }

    fn protect_all(&mut self) {
        for s in &mut self.stencil {
            *s |= PROTECTED;
        }
    }

    /// Protects the four texels around every color-changing corner.
    fn protect_corners(&mut self, shape: &PreparedShape) {
        let (w, h) = (self.f.w, self.f.h);
        for c in &shape.contours {
            let (s, e) = (c.start as usize, c.end as usize);
            if e <= s {
                continue;
            }
            let mut prev = &shape.edges[e - 1];
            for i in s..e {
                let edge = &shape.edges[i];
                let common = prev.color & edge.color;
                if common & common.wrapping_sub(1) == 0 {
                    let p = self.transform.projection.project(edge.start);
                    let l = (p.x - 0.5).floor() as i64;
                    let b = (p.y - 0.5).floor() as i64;
                    for (tx, ty) in [(l, b), (l + 1, b), (l, b + 1), (l + 1, b + 1)] {
                        if tx >= 0 && ty >= 0 && tx < w && ty < h {
                            let r = self.row_of(ty);
                            *self.st(tx, r) |= PROTECTED;
                        }
                    }
                }
                prev = edge;
            }
        }
    }

    /// Protects texels whose extreme (non-median) channels form an edge with a neighbor.
    /// Each texel pair is examined once and may protect both texels. A vectorized prefilter
    /// selects the (few) pairs close enough to the edge first.
    fn protect_edges(&mut self) {
        let (w, h) = (self.f.w as usize, self.f.h as usize);
        let n = w * h;
        if n == 0 {
            return;
        }
        let z = self.zero;
        let hr = (PROTECTION_RADIUS_TOLERANCE * self.span(1.0, 0.0)) as f32;
        let vr = (PROTECTION_RADIUS_TOLERANCE * self.span(0.0, 1.0)) as f32;
        let dr = (PROTECTION_RADIUS_TOLERANCE * self.span(1.0, 1.0)) as f32;
        let near = near_pairs(&self.f, [hr, vr, dr]);
        // Second texel offsets of the pairs: right, down, down-right, down-left.
        let offs = [1usize, w, w + 1, w - 1];
        for (i, &bits) in near.iter().enumerate() {
            if bits == 0 {
                continue;
            }
            for (k, &o) in offs.iter().enumerate() {
                if bits & (1 << k) == 0 {
                    continue;
                }
                let j = i + o;
                let (ax, ar) = ((i % w) as i64, (i / w) as i64);
                let (bx, br) = ((j % w) as i64, (j / w) as i64);
                let a = self.f.px(ax, ar);
                let b = self.f.px(bx, br);
                let mask = edge_between_texels(&a, &b, z);
                if protect_extreme(&a, self.f.med[i], mask) {
                    self.stencil[i] |= PROTECTED;
                }
                if protect_extreme(&b, self.f.med[j], mask) {
                    self.stencil[j] |= PROTECTED;
                }
            }
        }
    }

    /// Finds artifacts based on the field contents alone. Texels that are not artifacts but
    /// have inversion candidates are marked `NEEDS_CHECK` for a later distance check.
    fn find_errors(&mut self, threads: usize) {
        let w = self.f.w;
        let spans = self.spans();
        let z = self.zero;
        self.ensure_gates();
        let field = &self.f;
        let dirs = &self.dirs;
        // Each texel only reads its own flags and writes its own entry: rows are independent.
        let rows: Vec<&mut [u8]> = self.stencil.chunks_mut(w as usize).collect();
        crate::generate::par_rows(
            rows,
            threads,
            || (),
            |r, (), row| {
                let dirs = &dirs[r * w as usize..(r + 1) * w as usize];
                for x in 0..w {
                    let d = dirs[x as usize];
                    if d == 0 {
                        continue;
                    }
                    let st = &mut row[x as usize];
                    let prot = *st & PROTECTED != 0;
                    let mut cls = BaseClassifier {
                        inversion_candidate: false,
                    };
                    if texel_has_artifact(field, x, r as i64, d, &spans, prot, z, &mut cls) {
                        *st |= ERROR;
                    } else if cls.inversion_candidate {
                        *st |= NEEDS_CHECK;
                    }
                }
            },
        );
    }

    fn ensure_gates(&mut self) {
        if self.dirs.is_empty() {
            self.dirs = artifact_gates(&self.f);
        }
    }

    /// Verifies candidates against the exact shape distance. With `only_flagged`, only
    /// texels marked `NEEDS_CHECK` by [`Self::find_errors`] are examined (all texels are
    /// treated as protected, i.e. only sign inversions are considered).
    fn find_errors_with_distance(&mut self, shape: &PreparedShape, overlap: bool, threads: usize, only_flagged: bool) {
        let (w, h) = (self.f.w, self.f.h);
        self.ensure_gates();
        let dirs = &self.dirs;
        let todo: Vec<i64> = (0..h)
            .filter(|&r| {
                let a = (r * w) as usize..((r + 1) * w) as usize;
                self.stencil[a.clone()]
                    .iter()
                    .zip(&dirs[a])
                    .any(|(s, &d)| d != 0 && s & ERROR == 0 && (!only_flagged || s & NEEDS_CHECK != 0))
            })
            .collect();
        if todo.is_empty() {
            return;
        }
        let spans = self.spans();
        let mapping = self.transform.mapping();
        let proj = self.transform.projection;
        let inverse_y = self.inverse_y;
        let zero = self.zero;
        let improve = self.min_improve_ratio;
        let field = &self.f;
        let stencil = &self.stencil;
        let mut results: Vec<Vec<u8>> = todo.iter().map(|_| vec![0u8; w as usize]).collect();
        let rows: Vec<&mut [u8]> = results.iter_mut().map(Vec::as_mut_slice).collect();
        crate::generate::par_rows(
            rows,
            threads,
            || Evaluator::<1, false>::new(shape, overlap),
            |k, ev, out| {
                let r = todo[k];
                let ty = if inverse_y { r } else { h - 1 - r };
                for x in 0..w {
                    let flags = stencil[(r * w + x) as usize];
                    let d = dirs[(r * w + x) as usize];
                    if d == 0 || flags & ERROR != 0 || (only_flagged && flags & NEEDS_CHECK == 0) {
                        continue;
                    }
                    let prot = only_flagged || flags & PROTECTED != 0;
                    let c = field.px(x, r);
                    let cm = field.m(x, r);
                    let mut cls = DistanceClassifier {
                        field,
                        ev,
                        c,
                        cm,
                        base_x: x as f64 + 0.5,
                        base_row: r as f64 + 0.5,
                        base_ty: ty as f64 + 0.5,
                        inverse_y,
                        proj,
                        mapping,
                        improve,
                    };
                    if texel_has_artifact(field, x, r, d, &spans, prot, zero, &mut cls) {
                        out[x as usize] = 1;
                    }
                }
            },
        );
        for (k, &r) in todo.iter().enumerate() {
            for x in 0..w as usize {
                if results[k][x] != 0 {
                    self.stencil[r as usize * w as usize + x] |= ERROR;
                }
            }
        }
    }

    fn error_count(&self) -> usize {
        self.stencil.iter().filter(|s| *s & ERROR != 0).count()
    }
}

/// Classifier that confirms candidates by comparing the interpolated value before and after
/// the correction with the exact (pseudo) distance of the shape.
struct DistanceClassifier<'f, 'e, 's> {
    field: &'f Field<'f>,
    ev: &'e mut Evaluator<'s, 1, false>,
    c: [f32; 3],
    cm: f32,
    base_x: f64,
    base_row: f64,
    base_ty: f64,
    inverse_y: bool,
    proj: crate::transform::Projection,
    mapping: crate::transform::DistanceMapping,
    improve: f64,
}

impl Classifier for DistanceClassifier<'_, '_, '_> {
    #[inline]
    fn evaluate(&mut self, dir: (f64, f64), t: f64, _xm: f32, flags: u8) -> bool {
        if flags & CANDIDATE == 0 {
            return false;
        }
        if flags & ARTIFACT != 0 {
            return true;
        }
        let (tx, tr) = (t * dir.0, t * dir.1);
        let old = self.field.sample(self.base_x + tx, self.base_row + tr);
        // Weight of this texel in the bilinear interpolation at that point.
        let wa = ((1.0 - tx.abs()) * (1.0 - tr.abs())) as f32;
        let (c, cm) = (self.c, self.cm);
        let new = [
            old[0] + wa * (cm - c[0]),
            old[1] + wa * (cm - c[1]),
            old[2] + wa * (cm - c[2]),
        ];
        let old_m = median(old[0], old[1], old[2]);
        let new_m = median(new[0], new[1], new[2]);
        let ty = self.base_ty + if self.inverse_y { tr } else { -tr };
        let sp = self.proj.unproject(Vec2::new(self.base_x + tx, ty));
        let refd = self.mapping.map(self.ev.distance(sp)[0]) as f32;
        self.improve * ((new_m - refd).abs() as f64) < (old_m - refd).abs() as f64
    }
}

// ---------------------------------------------------------------------------------------
// Prefilters
//
// Most texel/neighbor combinations cannot produce an artifact candidate: the channel pairs
// don't cross between the two texels, or the texel is not the one blamed. The gates below
// test necessary conditions for every path in `linear_artifact` / `diagonal_artifact` that
// reaches the classifier - conservatively (with slack, so rounding differences can only let
// more through) - for whole rows at once, without branches, so they compile to 8-wide
// vector code. Each texel pair / 2x2 cell is examined once and shared by the texels using it.
// The scalar code then only visits the (few) directions that passed, with unchanged results.

/// Widened artifact interval: (GATE_E0, 1 - GATE_E0).
const GATE_E0: f32 = 0.99 * ARTIFACT_T_EPSILON as f32;

/// Necessary condition for `t = da / (da - db)` to lie inside (EPS, 1 - EPS) (symmetric in
/// `da`, `db`, so it also covers the pair seen from the other texel).
#[inline(always)]
fn axis_cross(da: f32, db: f32) -> bool {
    let k = GATE_E0 * (da - db).abs();
    (((da < 0.0) & (db > 0.0)) | ((da > 0.0) & (db < 0.0))) & (da.abs() > k) & (db.abs() > k)
}

/// Necessary condition for the crossing equation of `diagonal_artifact` with Bernstein
/// coefficients `(a, bc / 2, d)` to have a root inside (EPS, 1 - EPS) (symmetric under
/// reversal of the diagonal).
#[inline(always)]
fn diag_cross(a: f32, bc: f32, d: f32) -> bool {
    let same = ((a > 0.0) & (bc > 0.0) & (d > 0.0)) | ((a < 0.0) & (bc < 0.0) & (d < 0.0));
    let m = 0.5 * bc;
    let mag = a.abs() + m.abs() + d.abs();
    let tol = 1e-5 * mag;
    const U: f32 = 1.0 - GATE_E0;
    // p(t) = a (1-t)^2 + 2 m t (1-t) + d t^2 at both ends of the widened interval.
    let p0 = a * (U * U) + m * (2.0 * GATE_E0 * U) + d * (GATE_E0 * GATE_E0);
    let p1 = a * (GATE_E0 * GATE_E0) + m * (2.0 * GATE_E0 * U) + d * (U * U);
    let change = ((p0 <= tol) & (p1 >= -tol)) | ((p0 >= -tol) & (p1 <= tol));
    // Two roots inside: real roots and the vertex inside [0, 1].
    let real = m * m - a * d >= -1e-4 * (m * m + (a * d).abs());
    let vertex = ((a >= m) & (d >= m)) | ((a <= m) & (d <= m));
    !same & (mag > 0.0) & (change | (real & vertex))
}

#[inline(always)]
fn gate_axis_row(out: &mut [u8], c: [&[f32]; 3], n: [&[f32]; 3]) {
    let len = out.len();
    let (c0, c1, c2) = (&c[0][..len], &c[1][..len], &c[2][..len]);
    let (n0, n1, n2) = (&n[0][..len], &n[1][..len], &n[2][..len]);
    for x in 0..len {
        out[x] = axis_cross(c0[x], n0[x]) as u8
            | (axis_cross(c1[x], n1[x]) as u8) << 1
            | (axis_cross(c2[x], n2[x]) as u8) << 2;
    }
}

#[inline(always)]
fn gate_diag_row(out: &mut [u8], a: [&[f32]; 3], b: [&[f32]; 3], c: [&[f32]; 3], d: [&[f32]; 3]) {
    let len = out.len();
    for k in 0..3 {
        let (a, b, c, d) = (&a[k][..len], &b[k][..len], &c[k][..len], &d[k][..len]);
        for x in 0..len {
            out[x] |= (diag_cross(a[x], b[x] + c[x], d[x]) as u8) << k;
        }
    }
}

/// `out |= (pair bits if the texel is blamed) << (3 * dir)`.
#[inline(always)]
fn dir_pass(out: &mut [u32], hit: &[u8], cd: &[f32], nd: &[f32], dir: u32) {
    let len = out.len();
    let (hit, cd, nd) = (&hit[..len], &cd[..len], &nd[..len]);
    for x in 0..len {
        out[x] |= ((hit[x] as u32) * (!(cd[x] < nd[x]) as u32)) << (3 * dir);
    }
}

#[inline(always)]
fn artifact_gates_impl(f: &Field<'_>) -> Vec<u32> {
    let (w, h) = (f.w as usize, f.h as usize);
    let n = w * h;
    let mut dirs = vec![0u32; n];
    if n == 0 {
        return dirs;
    }
    // The whole image is processed as flat arrays (long, vectorizable loops); pairs that
    // wrap around a row end are cleared at the end.
    let d = |a: usize, b: usize| -> [&[f32]; 3] { [&f.diff[0][a..b], &f.diff[1][a..b], &f.diff[2][a..b]] };
    let dist = &f.dist[..];
    // Pair / cell hits indexed by the top-left texel: horizontal pair (i, i+1), vertical
    // pair (i, i+w), diagonals tl-br and tr-bl of the cell with top-left i.
    let mut hp = vec![0u8; n];
    let mut vp = vec![0u8; n];
    let mut d1 = vec![0u8; n];
    let mut d2 = vec![0u8; n];
    gate_axis_row(&mut hp[..n - 1], d(0, n - 1), d(1, n));
    if h > 1 {
        let m = n - w;
        gate_axis_row(&mut vp[..m], d(0, m), d(w, n));
        if w > 1 {
            let m = n - w - 1;
            let (tl, tr, bl, br) = (d(0, m), d(1, m + 1), d(w, m + w), d(w + 1, n));
            gate_diag_row(&mut d1[..m], tl, tr, bl, br);
            gate_diag_row(&mut d2[..m], tr, tl, br, bl);
        }
    }
    // Per texel direction bits, in `texel_has_artifact` order: axis (-1,0), (0,1), (1,0),
    // (0,-1); diagonal (-1,-1), (1,-1), (-1,1), (1,1) (row space). A direction counts only if
    // the texel is blamed (not closer to the edge than the neighbor).
    dir_pass(&mut dirs[1..], &hp[..n - 1], &dist[1..], &dist[..n - 1], 0);
    dir_pass(&mut dirs[..n - 1], &hp[..n - 1], &dist[..n - 1], &dist[1..], 2);
    if h > 1 {
        let m = n - w;
        dir_pass(&mut dirs[..m], &vp[..m], &dist[..m], &dist[w..], 1);
        dir_pass(&mut dirs[w..], &vp[..m], &dist[w..], &dist[..m], 3);
        if w > 1 {
            let m = n - w - 1;
            dir_pass(&mut dirs[..m], &d1[..m], &dist[..m], &dist[w + 1..], 7);
            dir_pass(&mut dirs[w + 1..], &d1[..m], &dist[w + 1..], &dist[..m], 4);
            dir_pass(&mut dirs[1..=m], &d2[..m], &dist[1..=m], &dist[w..m + w], 6);
            dir_pass(&mut dirs[w..m + w], &d2[..m], &dist[w..m + w], &dist[1..=m], 5);
        }
    }
    // Directions leaving the row.
    for r in 0..h {
        dirs[r * w] &= !(7 | 7 << 12 | 7 << 18);
        dirs[r * w + w - 1] &= !(7 << 6 | 7 << 15 | 7 << 21);
    }
    dirs
}

#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn near_pass(out: &mut [u8], a: &[f32], b: &[f32], sa: &[u8], sb: &[u8], radius: f32, bit: u32) {
    let len = out.len();
    let (a, b, sa, sb) = (&a[..len], &b[..len], &sa[..len], &sb[..len]);
    for x in 0..len {
        // Some channel must cross zero strictly between the texels for
        // `edge_between_texels` to report it.
        let cross = (((sa[x] >> 3) & sb[x]) | (sa[x] & (sb[x] >> 3))) & 7 != 0;
        out[x] |= (((a[x] + b[x] < radius) & cross) as u8) << bit;
    }
}

#[inline(always)]
fn near_pairs_impl(dist: &[f32], side: &[u8], w: usize, h: usize, radii: [f32; 3]) -> Vec<u8> {
    let n = w * h;
    let mut out = vec![0u8; n];
    let (d, s) = (dist, side);
    // Bits: 0 = (i, i+1), 1 = (i, i+w), 2 = (i, i+w+1), 3 = (i, i+w-1).
    near_pass(
        &mut out[..n - 1],
        &d[..n - 1],
        &d[1..],
        &s[..n - 1],
        &s[1..],
        radii[0],
        0,
    );
    if h > 1 {
        let m = n - w;
        near_pass(&mut out[..m], &d[..m], &d[w..], &s[..m], &s[w..], radii[1], 1);
        if w > 1 {
            let (a, b) = (..m - 1, w + 1..);
            near_pass(&mut out[a], &d[a], &d[b.clone()], &s[a], &s[b], radii[2], 2);
            let (a, b) = (1..m, w..n - 1);
            near_pass(
                &mut out[a.clone()],
                &d[a.clone()],
                &d[b.clone()],
                &s[a],
                &s[b],
                radii[2],
                3,
            );
        }
    }
    // Pairs wrapping around a row end.
    for r in 0..h {
        out[r * w] &= !8;
        out[r * w + w - 1] &= !(1 | 4);
    }
    out
}

/// Pairs of neighboring texels whose distances to the edge sum to less than the radius of
/// their direction (`radii`: horizontal, vertical, diagonal) and that have a channel crossing
/// zero between them, as bits per first texel.
fn near_pairs(f: &Field<'_>, radii: [f32; 3]) -> Vec<u8> {
    near_pairs_impl(&f.dist, &f.side, f.w as usize, f.h as usize, radii)
}

/// Direction masks of all texels (see [`Corrector::dirs`]).
fn artifact_gates(f: &Field<'_>) -> Vec<u32> {
    artifact_gates_impl(f)
}

/// Checks the texel against its 8 neighbors.
/// Only the directions whose bit is set in `dirs` are examined (bits in the order of the
/// loops below; see [`artifact_gates`]).
#[inline]
#[allow(clippy::too_many_arguments)]
fn texel_has_artifact<C: Classifier>(
    f: &Field<'_>,
    x: i64,
    r: i64,
    dirs: u32,
    spans: &[f64; 3],
    prot: bool,
    z: f32,
    cls: &mut C,
) -> bool {
    let (w, h) = (f.w, f.h);
    let c = f.px(x, r);
    let cm = f.m(x, r);
    let cdist = (cm - z).abs();
    // Axis neighbors.
    for (k, (dx, dr)) in [(-1i64, 0i64), (0, 1), (1, 0), (0, -1)].into_iter().enumerate() {
        let pairs = (dirs >> (3 * k)) & 7;
        if pairs == 0 {
            continue;
        }
        let (nx, nr) = (x + dx, r + dr);
        if nx < 0 || nr < 0 || nx >= w || nr >= h {
            continue;
        }
        let nm = f.m(nx, nr);
        // Only blame the texel that is farther from the edge.
        if cdist < (nm - z).abs() {
            continue;
        }
        let span = if dx != 0 { spans[0] } else { spans[1] };
        let n = f.px(nx, nr);
        let dir = (dx as f64, dr as f64);
        if linear_artifact(&c, cm, &n, nm, pairs, span, prot, z, dir, cls) {
            return true;
        }
    }
    // Diagonal neighbors (bilinear interpolation across the 2x2 cell).
    for (k, (dx, dr)) in [(-1i64, -1i64), (1, -1), (-1, 1), (1, 1)].into_iter().enumerate() {
        let pairs = (dirs >> (3 * (4 + k))) & 7;
        if pairs == 0 {
            continue;
        }
        let (nx, nr) = (x + dx, r + dr);
        if nx < 0 || nr < 0 || nx >= w || nr >= h {
            continue;
        }
        let dm = f.m(nx, nr);
        if cdist < (dm - z).abs() {
            continue;
        }
        let b = f.px(nx, r);
        let cc = f.px(x, nr);
        let d = f.px(nx, nr);
        if diagonal_artifact(
            &c,
            cm,
            &b,
            &cc,
            &d,
            dm,
            pairs,
            spans[2],
            prot,
            z,
            (dx as f64, dr as f64),
            cls,
        ) {
            return true;
        }
    }
    false
}

/// Tests whether median `xm` at `xt` (between `at` with median `am` and `bt` with `bm`)
/// indicates an artifact. For protected texels only sign inversions are candidates.
#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn range_test(at: f64, bt: f64, xt: f64, am: f32, bm: f32, xm: f32, span: f64, prot: bool, z: f32) -> u8 {
    let inversion = (am > z && bm > z && xm <= z) || (am < z && bm < z && xm >= z);
    if inversion || (!prot && median(am, bm, xm) != xm) {
        let ax = ((xt - at) * span) as f32;
        let bx = ((bt - xt) * span) as f32;
        let inv = if inversion { INVERSION } else { 0 };
        // The true field can change by at most `span` per unit of t.
        if !(xm >= am - ax && xm <= am + ax && xm >= bm - bx && xm <= bm + bx) {
            return CANDIDATE | ARTIFACT | inv;
        }
        return CANDIDATE | inv;
    }
    0
}

#[inline(always)]
fn lerp_median(a: &[f32; 3], b: &[f32; 3], t: f64) -> f32 {
    let t = t as f32;
    median(
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    )
}

#[inline]
#[allow(clippy::too_many_arguments)]
fn linear_artifact<C: Classifier>(
    a: &[f32; 3],
    am: f32,
    b: &[f32; 3],
    bm: f32,
    pairs: u32,
    span: f64,
    prot: bool,
    z: f32,
    dir: (f64, f64),
    cls: &mut C,
) -> bool {
    for (k, (i, j)) in [(1usize, 0usize), (2, 1), (0, 2)].into_iter().enumerate() {
        if pairs & (1 << k) == 0 {
            continue;
        }
        let da = (a[i] - a[j]) as f64;
        let db = (b[i] - b[j]) as f64;
        if da == db {
            continue;
        }
        // Where the two channels cross, the median is extreme.
        let t = da / (da - db);
        if t > ARTIFACT_T_EPSILON && t < 1.0 - ARTIFACT_T_EPSILON {
            let xm = lerp_median(a, b, t);
            let f = range_test(0.0, 1.0, t, am, bm, xm, span, prot, z);
            if f != 0 && cls.evaluate(dir, t, xm, f) {
                return true;
            }
        }
    }
    false
}

#[inline(always)]
fn quad_median(a: &[f32; 3], l: &[f32; 3], q: &[f32; 3], t: f64) -> f32 {
    let v = |k: usize| (t * (t * q[k] as f64 + l[k] as f64) + a[k] as f64) as f32;
    median(v(0), v(1), v(2))
}

#[inline]
#[allow(clippy::too_many_arguments)]
fn diagonal_artifact<C: Classifier>(
    a: &[f32; 3],
    am: f32,
    b: &[f32; 3],
    c: &[f32; 3],
    d: &[f32; 3],
    dm: f32,
    pairs: u32,
    span: f64,
    prot: bool,
    z: f32,
    dir: (f64, f64),
    cls: &mut C,
) -> bool {
    // Along the diagonal: v(t) = a + l t + q t^2.
    let mut l = [0f32; 3];
    let mut q = [0f32; 3];
    for k in 0..3 {
        let abc = a[k] - b[k] - c[k];
        l[k] = -a[k] - abc;
        q[k] = d[k] + abc;
    }
    // Extremum of channel k along the diagonal (only needed once a crossing is found).
    let tex = |k: usize| -0.5 * l[k] as f64 / q[k] as f64;
    for (k, (i, j)) in [(1usize, 0usize), (2, 1), (0, 2)].into_iter().enumerate() {
        if pairs & (1 << k) == 0 {
            continue;
        }
        let da = (a[i] - a[j]) as f64;
        let dbc = ((b[i] - b[j]) + (c[i] - c[j])) as f64;
        let dd = (d[i] - d[j]) as f64;
        // Bernstein coefficients (da, dbc/2, dd) of the same sign: no crossing in [0, 1].
        if (da > 0.0 && dbc > 0.0 && dd > 0.0) || (da < 0.0 && dbc < 0.0 && dd < 0.0) {
            continue;
        }
        let roots = crate::solver::solve_quadratic(dd - dbc + da, dbc - da - da, da);
        for &t in roots.as_slice() {
            if !(t > ARTIFACT_T_EPSILON && t < 1.0 - ARTIFACT_T_EPSILON) {
                continue;
            }
            let xm = quad_median(a, &l, &q, t);
            let mut f = range_test(0.0, 1.0, t, am, dm, xm, span, prot, z);
            // Also compare against the medians at the channels' local extremes.
            for te in [tex(j), tex(i)] {
                if te > 0.0 && te < 1.0 {
                    let mut tend = [0.0, 1.0];
                    let mut em = [am, dm];
                    let k = (te > t) as usize;
                    tend[k] = te;
                    em[k] = quad_median(a, &l, &q, te);
                    f |= range_test(tend[0], tend[1], t, em[0], em[1], xm, span, prot, z);
                }
            }
            if f != 0 && cls.evaluate(dir, t, xm, f) {
                return true;
            }
        }
    }
    false
}

/// Bit mask of channels that form an edge between texels `a` and `b`.
fn edge_between_texels(a: &[f32; 3], b: &[f32; 3], z: f32) -> u8 {
    let mut mask = 0;
    for ch in 0..3 {
        let den = a[ch] - b[ch];
        if den == 0.0 {
            continue;
        }
        let t = (a[ch] - z) as f64 / den as f64;
        if t > 0.0 && t < 1.0 {
            let tf = t as f32;
            let v = [
                a[0] + (b[0] - a[0]) * tf,
                a[1] + (b[1] - a[1]) * tf,
                a[2] + (b[2] - a[2]) * tf,
            ];
            if median(v[0], v[1], v[2]) == v[ch] {
                mask |= 1 << ch;
            }
        }
    }
    mask
}

#[inline]
fn protect_extreme(px: &[f32; 3], m: f32, mask: u8) -> bool {
    (mask & 1 != 0 && px[0] != m) || (mask & 2 != 0 && px[1] != m) || (mask & 4 != 0 && px[2] != m)
}

/// Flattens all texels marked as errors to their median.
fn apply<const N: usize>(stencil: &[u8], out: &mut Bitmap<N>) {
    for (px, s) in out.data_mut().chunks_exact_mut(N).zip(stencil) {
        if s & ERROR != 0 {
            let m = median(px[0], px[1], px[2]);
            px[0] = m;
            px[1] = m;
            px[2] = m;
        }
    }
}

/// Runs MSDF error correction on a generated 3- or 4-channel field.
/// Returns the number of corrected texels.
pub fn correct_msdf<const N: usize>(
    sdf: &mut Bitmap<N>,
    shape: &PreparedShape,
    transform: &Transform,
    cfg: &ErrorCorrectionConfig,
    overlap: bool,
    threads: usize,
) -> usize {
    const { assert!(N >= 3, "error correction needs a multi-channel field") };
    if cfg.mode == ErrorCorrectionMode::Disabled || sdf.width() == 0 || sdf.height() == 0 {
        return 0;
    }
    let (stencil, n) = {
        let mut ec = Corrector::new(&*sdf, transform, shape.inverse_y_axis, cfg);
        match cfg.mode {
            ErrorCorrectionMode::EdgePriority => {
                ec.protect_corners(shape);
                ec.protect_edges();
            }
            ErrorCorrectionMode::EdgeOnly => ec.protect_all(),
            _ => {}
        }
        let check = cfg.distance_check;
        let two_phase = check == DistanceCheckMode::CheckAtEdge && cfg.mode != ErrorCorrectionMode::EdgeOnly;
        if check == DistanceCheckMode::DoNotCheck || two_phase {
            ec.find_errors(threads);
        }
        if check != DistanceCheckMode::DoNotCheck {
            // In the two-phase mode every texel counts as protected for the distance check,
            // so only those with inversion candidates (flagged above) need to be examined.
            ec.find_errors_with_distance(shape, overlap, threads, two_phase);
        }
        (std::mem::take(&mut ec.stencil), ec.error_count())
    };
    apply(&stencil, sdf);
    n
}

/// Error correction without the shape (no corner protection, no distance checks).
pub fn correct_msdf_fast<const N: usize>(
    sdf: &mut Bitmap<N>,
    transform: &Transform,
    inverse_y: bool,
    min_deviation_ratio: f64,
) -> usize {
    const { assert!(N >= 3, "error correction needs a multi-channel field") };
    if sdf.width() == 0 || sdf.height() == 0 {
        return 0;
    }
    let cfg = ErrorCorrectionConfig {
        min_deviation_ratio,
        ..Default::default()
    };
    let (stencil, n) = {
        let mut ec = Corrector::new(&*sdf, transform, inverse_y, &cfg);
        ec.find_errors(1);
        (std::mem::take(&mut ec.stencil), ec.error_count())
    };
    apply(&stencil, sdf);
    n
}
