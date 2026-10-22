//! Distance field evaluation: prepared shape, edge selection, contour combining and the
//! parallel per-texel driver.
//!
//! Selection semantics follow msdfgen's (per channel: nearest edge by true distance with
//! orthogonality tie-break, converted to a pseudo-distance, plus perpendicular distances from
//! corner extensions), but the search is accelerated:
//! * each edge carries precomputed geometry (power-basis coefficients, curve samples,
//!   normalized tangents and corner bisectors),
//! * edges are rejected with conservative lower bounds (bounding box distance for the true
//!   distance, scaled endpoint distance for corner perpendiculars) against the current best
//!   distance, which is exact - it never changes the result,
//! * the nearest edges of the previous texel are evaluated first (spatial coherence), so the
//!   bounds are tight from the start,
//! * rows are processed in parallel.

use crate::edge::{CubicData, LineData, QuadData, Segment, SignedDistance, pseudo_extend};
use crate::math::{Vec2, median};
use crate::shape::Shape;

pub(crate) const NONE: u32 = u32::MAX;

/// Relative margin applied to squared culling thresholds.
const CULL_MARGIN: f64 = 1.0 + 1e-9;

/// An edge with everything needed for fast distance queries.
#[derive(Clone, Debug)]
pub(crate) struct PEdge {
    pub geom: GeomData,
    pub color: u8,
    pub p: [Vec2; 4],
    pub start: Vec2,
    pub end: Vec2,
    /// Point at t = 0.5 (used for tile upper bounds).
    pub mid: Vec2,
    pub bmin: Vec2,
    pub bmax: Vec2,
    /// Normalized tangents at start and end (zero if degenerate).
    pub dir0: Vec2,
    pub dir1: Vec2,
    /// Normalized bisectors of the corners at the start and end (zero at a perfect cusp).
    pub bis_a: Vec2,
    pub bis_b: Vec2,
    /// Lower-bound factors for corner perpendicular candidates: a candidate from the start
    /// corner has |distance| >= |p - start| * cos_a (0 means the corner never produces one).
    pub cos_a: f64,
    pub cos_b: f64,
    /// Squares of the above, or +inf where the corner never produces candidates.
    pub cos_a2: f64,
    pub cos_b2: f64,
}

#[derive(Clone, Debug)]
pub(crate) enum GeomData {
    Line(LineData),
    /// Closed-form nearest point (cubic equation).
    Quad(QuadData),
    /// Bracketed-descent nearest point search.
    Curve(Box<CubicData>),
}

impl PEdge {
    /// The edge's segment (from its control points).
    fn segment(&self) -> Segment {
        match self.geom {
            GeomData::Line(_) => Segment::Linear([self.p[0], self.p[1]]),
            GeomData::Quad(_) => Segment::Quadratic([self.p[0], self.p[1], self.p[2]]),
            GeomData::Curve(_) => Segment::Cubic(self.p),
        }
    }

    /// Like [`Self::signed_distance`], with a per-edge warm-start parameter for curves.
    #[inline(always)]
    pub fn signed_distance_hint(&self, p: Vec2, hint: &mut f64) -> (SignedDistance, f64) {
        match &self.geom {
            GeomData::Line(l) => l.signed_distance(self.p[0], self.p[1], p),
            GeomData::Quad(q) => q.signed_distance_warm(&[self.p[0], self.p[1], self.p[2]], p, hint),
            GeomData::Curve(c) => c.signed_distance_hint(p, hint),
        }
    }

    /// Converts a true distance obtained from this edge into a pseudo-distance.
    #[inline(always)]
    pub fn to_pseudo(&self, sd: &mut SignedDistance, p: Vec2, param: f64) {
        if param < 0.0 {
            pseudo_extend(sd, p - self.start, self.dir0, false);
        } else if param > 1.0 {
            pseudo_extend(sd, p - self.end, self.dir1, true);
        }
    }
}

fn sq_or_inf(c: f64) -> f64 {
    if c > 0.0 { c * c } else { f64::INFINITY }
}

/// The data needed to cull an edge, packed densely for cache efficiency.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Hot {
    pub bmin: Vec2,
    pub bmax: Vec2,
    pub start: Vec2,
    pub end: Vec2,
    pub cos_a2: f64,
    pub cos_b2: f64,
    pub color: u8,
}

impl Hot {
    #[inline(always)]
    fn bbox_dist2(&self, p: Vec2) -> f64 {
        let dx = (self.bmin.x - p.x).max(p.x - self.bmax.x).max(0.0);
        let dy = (self.bmin.y - p.y).max(p.y - self.bmax.y).max(0.0);
        dx * dx + dy * dy
    }
}

/// A contour as a range of prepared edges.
#[derive(Clone, Debug)]
pub(crate) struct PContour {
    pub start: u32,
    pub end: u32,
    pub winding: i32,
}

/// A shape preprocessed for fast distance queries.
#[derive(Clone, Debug)]
pub struct PreparedShape {
    pub(crate) edges: Vec<PEdge>,
    pub(crate) hot: Vec<Hot>,
    pub(crate) contours: Vec<PContour>,
    pub(crate) inverse_y_axis: bool,
}

impl PreparedShape {
    pub fn new(shape: &Shape) -> Self {
        let mut edges = Vec::with_capacity(shape.edge_count());
        let mut contours = Vec::with_capacity(shape.contours.len());
        for c in &shape.contours {
            if c.edges.is_empty() {
                continue;
            }
            let start = edges.len() as u32;
            let n = c.edges.len();
            for (i, e) in c.edges.iter().enumerate() {
                let prev = &c.edges[(i + n - 1) % n];
                let next = &c.edges[(i + 1) % n];
                let dir0 = e.direction(0.0).normalize_or_zero();
                let dir1 = e.direction(1.0).normalize_or_zero();
                let prev_dir = prev.direction(1.0).normalize_or_zero();
                let next_dir = next.direction(0.0).normalize_or_zero();
                let bis_a = (prev_dir + dir0).normalize_or_zero();
                let bis_b = (dir1 + next_dir).normalize_or_zero();
                let (bmin, bmax) = e.segment.bounds();
                let mut p = [Vec2::ZERO; 4];
                let pts = e.segment.control_points();
                p[..pts.len()].copy_from_slice(pts);
                let geom = match e.segment {
                    Segment::Linear([a, b]) => GeomData::Line(LineData::new(a, b)),
                    Segment::Quadratic(q) => GeomData::Quad(QuadData::new(&q)),
                    Segment::Cubic(cp) => GeomData::Curve(Box::new(CubicData::new(&cp))),
                };
                edges.push(PEdge {
                    geom,
                    color: e.color.bits() & 7,
                    p,
                    start: e.start(),
                    end: e.end(),
                    mid: e.point(0.5),
                    bmin,
                    bmax,
                    dir0,
                    dir1,
                    bis_a,
                    bis_b,
                    cos_a: dir0.dot(bis_a).max(0.0),
                    cos_b: dir1.dot(bis_b).max(0.0),
                    cos_a2: sq_or_inf(dir0.dot(bis_a)),
                    cos_b2: sq_or_inf(dir1.dot(bis_b)),
                });
            }
            contours.push(PContour {
                start,
                end: edges.len() as u32,
                winding: c.winding(),
            });
        }
        let hot = edges
            .iter()
            .map(|e: &PEdge| Hot {
                bmin: e.bmin,
                bmax: e.bmax,
                start: e.start,
                end: e.end,
                cos_a2: e.cos_a2,
                cos_b2: e.cos_b2,
                color: e.color,
            })
            .collect();
        PreparedShape {
            edges,
            hot,
            contours,
            inverse_y_axis: shape.inverse_y_axis,
        }
    }

    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// Whether the source shape was y-down (texel rows are then stored bottom to top).
    pub fn inverse_y_axis(&self) -> bool {
        self.inverse_y_axis
    }

    /// Winding number of the shape around `p`, as [`crate::Shape::winding_number`]: exact,
    /// unlike the sign of a distance, which the overlapping contour combiner can get wrong
    /// inside partly overlapping contours.
    pub fn winding_number(&self, p: Vec2) -> i32 {
        // Only the crossings right of `p` count, in any order (no sort), and only edges whose
        // bounds straddle the line can cross it.
        let mut crossings = Vec::new();
        for e in self.edges.iter().filter(|e| e.bmin.y <= p.y && p.y <= e.bmax.y) {
            e.segment().scanline_crossings(p.y, &mut crossings);
        }
        // Clockwise contours (filled) cross downward on the right of an interior point.
        -crossings.iter().filter(|(x, _)| *x > p.x).map(|(_, d)| *d).sum::<i32>()
    }

    /// The outline's crossings of the horizontal line at height `y`, as
    /// [`crate::Shape::scanline`]: `(x, dir)` sorted by x; the winding number at x is
    /// `-sum(dir for crossings with x' > x)`.
    pub fn scanline(&self, y: f64, out: &mut Vec<(f64, i32)>) {
        out.clear();
        for e in &self.edges {
            e.segment().scanline_crossings(y, out);
        }
        out.sort_by(|a, b| a.0.total_cmp(&b.0));
    }
}

/// Per-channel selection state (nearest edge by true distance + corner perpendiculars).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Chan {
    pub sd: SignedDistance,
    pub near: u32,
    pub param: f64,
    pub neg: f64,
    pub pos: f64,
}

impl Chan {
    pub const EMPTY: Chan = Chan {
        sd: SignedDistance::INFINITE,
        near: NONE,
        param: 0.0,
        neg: -f64::MAX,
        pos: f64::MAX,
    };

    #[inline(always)]
    fn add_perp(&mut self, d: f64) {
        if d <= 0.0 && d > self.neg {
            self.neg = d;
        }
        if d >= 0.0 && d < self.pos {
            self.pos = d;
        }
    }

    /// Closest perpendicular candidate on the side of the nearest true distance.
    #[inline(always)]
    fn bucket(&self) -> f64 {
        if self.sd.distance < 0.0 { self.neg } else { self.pos }
    }

    /// Pseudo-distance to the nearest edge, or +inf if there is none.
    #[inline(always)]
    fn near_pseudo(&self, edges: &[PEdge], p: Vec2) -> f64 {
        if self.near == NONE {
            return f64::INFINITY;
        }
        let mut sd = self.sd;
        edges[self.near as usize].to_pseudo(&mut sd, p, self.param);
        sd.distance
    }

    #[inline(always)]
    fn resolve(&self, edges: &[PEdge], p: Vec2) -> f64 {
        let mut d = if self.sd.distance < 0.0 { self.neg } else { self.pos };
        if self.near != NONE {
            let mut sd = self.sd;
            edges[self.near as usize].to_pseudo(&mut sd, p, self.param);
            if sd.distance.abs() < d.abs() {
                d = sd.distance;
            }
        }
        d
    }
}

/// Selection state of one contour (or of the whole shape).
/// `NC` pseudo-distance channels (0, 1 or 3) plus an optional true distance (`TD`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct State<const NC: usize> {
    pub ch: [Chan; NC],
    pub tsd: SignedDistance,
    pub tnear: u32,
    /// Squared culling threshold (with margin) for each channel mask: the largest current
    /// best distance among the selectors an edge with that mask feeds.
    pub t2: [f64; 8],
}

impl<const NC: usize> State<NC> {
    pub const EMPTY: Self = State {
        ch: [Chan::EMPTY; NC],
        tsd: SignedDistance::INFINITE,
        tnear: NONE,
        t2: [f64::INFINITY; 8],
    };

    #[inline(always)]
    fn reset(&mut self) {
        self.ch.fill(Chan::EMPTY);
        self.tsd = SignedDistance::INFINITE;
        self.tnear = NONE;
        self.t2 = [f64::INFINITY; 8];
    }

    /// Recomputes the threshold table after a selector improved.
    #[inline(always)]
    fn update_thresholds<const TD: bool>(&mut self) {
        let mut b = [0.0f64; 3];
        for c in 0..NC {
            let d = self.ch[c].sd.distance;
            b[c] = d * d;
        }
        let base = if TD {
            let d = self.tsd.distance;
            d * d
        } else {
            0.0
        };
        match NC {
            0 => self.t2[0] = base * CULL_MARGIN,
            1 => self.t2[1] = base.max(b[0]) * CULL_MARGIN,
            _ => {
                // t2[m] = max(base, b[c] for c in m), unrolled.
                let m1 = base.max(b[0]);
                let m2 = base.max(b[1]);
                let m3 = m1.max(b[1]);
                self.t2 = [
                    base * CULL_MARGIN,
                    m1 * CULL_MARGIN,
                    m2 * CULL_MARGIN,
                    m3 * CULL_MARGIN,
                    base.max(b[2]) * CULL_MARGIN,
                    m1.max(b[2]) * CULL_MARGIN,
                    m2.max(b[2]) * CULL_MARGIN,
                    m3.max(b[2]) * CULL_MARGIN,
                ];
            }
        }
    }

    /// Final distances: `NC` channels followed by the true distance.
    #[inline(always)]
    fn resolve(&self, edges: &[PEdge], p: Vec2) -> Dist {
        let mut d = [0.0; 4];
        for c in 0..NC {
            d[c] = self.ch[c].resolve(edges, p);
        }
        d[3] = self.tsd.distance;
        d
    }
}

/// Final channel value: the nearest edge's pseudo-distance unless a corner perpendicular is
/// strictly shorter.
#[inline(always)]
fn pick(bucket: f64, near_pseudo: f64) -> f64 {
    if near_pseudo.abs() < bucket.abs() {
        near_pseudo
    } else {
        bucket
    }
}

/// Merge accumulator remembering which contour owns each channel's nearest edge.
#[derive(Clone, Copy)]
struct Acc<const NC: usize> {
    sd: [SignedDistance; NC],
    owner: [usize; NC],
    neg: [f64; NC],
    pos: [f64; NC],
    tsd: SignedDistance,
}

impl<const NC: usize> Acc<NC> {
    const EMPTY: Self = Acc {
        sd: [SignedDistance::INFINITE; NC],
        owner: [usize::MAX; NC],
        neg: [-f64::MAX; NC],
        pos: [f64::MAX; NC],
        tsd: SignedDistance::INFINITE,
    };

    #[inline(always)]
    fn merge(&mut self, st: &State<NC>, i: usize) {
        for c in 0..NC {
            let ch = &st.ch[c];
            if ch.sd.closer_than(&self.sd[c]) {
                self.sd[c] = ch.sd;
                self.owner[c] = if ch.near == NONE { usize::MAX } else { i };
            }
            if ch.neg > self.neg[c] {
                self.neg[c] = ch.neg;
            }
            if ch.pos < self.pos[c] {
                self.pos[c] = ch.pos;
            }
        }
        if st.tsd.closer_than(&self.tsd) {
            self.tsd = st.tsd;
        }
    }

    #[inline(always)]
    fn resolve(&self, pseudo: &[[f64; 3]]) -> Dist {
        let mut d = [0.0; 4];
        for c in 0..NC {
            let bucket = if self.sd[c].distance < 0.0 {
                self.neg[c]
            } else {
                self.pos[c]
            };
            let ps = if self.owner[c] == usize::MAX {
                f64::INFINITY
            } else {
                pseudo[self.owner[c]][c]
            };
            d[c] = pick(bucket, ps);
        }
        d[3] = self.tsd.distance;
        d
    }
}

/// Up to 3 pseudo-distance channels + true distance in slot 3.
pub(crate) type Dist = [f64; 4];

/// Scalar used to compare combined distances: median of channels, the single channel, or the
/// true distance (when there are no pseudo channels).
#[inline(always)]
fn scalar<const NC: usize>(d: &Dist) -> f64 {
    match NC {
        3 => median(d[0], d[1], d[2]),
        1 => d[0],
        _ => d[3],
    }
}

/// Reusable per-thread evaluator.
pub(crate) struct Evaluator<'a, const NC: usize, const TD: bool> {
    shape: &'a PreparedShape,
    overlap: bool,
    states: Vec<State<NC>>,
    seeds: Vec<[u32; 4]>,
    /// Edge index range of each group (each contour with overlap support, else one group).
    groups: Vec<(u32, u32)>,
    /// All edge indices (used when no tile is active).
    all: Vec<u32>,
    /// Candidate edges of the current tile, per group.
    cand: Vec<u32>,
    cand_range: Vec<(u32, u32)>,
    tiled: bool,
    /// Visit stamps (per edge) used to skip seeds already evaluated for the current query.
    stamp: Vec<u32>,
    counter: u32,
    /// Warm-start parameters for curve distance queries (per edge).
    hints: Vec<f64>,
    /// Scratch for the overlapping contour combiner.
    dists: Vec<Dist>,
    pseudo: Vec<[f64; 3]>,
}

impl<'a, const NC: usize, const TD: bool> Evaluator<'a, NC, TD> {
    pub fn new(shape: &'a PreparedShape, overlap: bool) -> Self {
        // A single contour is identical under both combiners; skip the overlap bookkeeping.
        let overlap = overlap && shape.contours.len() > 1;
        let groups: Vec<(u32, u32)> = if overlap {
            shape.contours.iter().map(|c| (c.start, c.end)).collect()
        } else {
            vec![(0, shape.edges.len() as u32)]
        };
        let n = groups.len();
        Evaluator {
            shape,
            overlap,
            states: vec![State::EMPTY; n],
            seeds: vec![[NONE; 4]; n],
            all: (0..shape.edges.len() as u32).collect(),
            cand: Vec::with_capacity(shape.edges.len()),
            cand_range: vec![(0, 0); n],
            groups,
            tiled: false,
            stamp: vec![0; shape.edges.len()],
            counter: 0,
            hints: vec![0.5; shape.edges.len()],
            dists: vec![[0.0; 4]; shape.contours.len()],
            pseudo: vec![[0.0; 3]; shape.contours.len()],
        }
    }

    /// Restricts subsequent queries to points inside the rectangle `[lo, hi]` (shape units):
    /// edges that provably cannot affect any distance inside it are skipped. Exact.
    pub fn set_tile(&mut self, lo: Vec2, hi: Vec2) {
        let edges = &self.shape.edges;
        self.cand.clear();
        for (g, &(s, e)) in self.groups.iter().enumerate() {
            // Upper bounds of the nearest true distance per channel (slot 3 = any edge) over
            // the tile: the farthest tile corner from a point on the edge.
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
                if d < ub[3] {
                    ub[3] = d;
                }
            }
            let start = self.cand.len() as u32;
            for i in s..e {
                let e = &edges[i as usize];
                let mask: u8 = match NC {
                    3 => e.color,
                    1 => 1,
                    _ => 0,
                };
                let mut thr2: f64 = if TD || NC == 1 { ub[3] } else { 0.0 };
                if NC == 3 {
                    for c in 0..3 {
                        if mask & (1 << c) != 0 {
                            thr2 = thr2.max(ub[c]);
                        }
                    }
                }
                let t2 = thr2 * CULL_MARGIN;
                let keep = rect_bbox_dist2(lo, hi, e.bmin, e.bmax) <= t2
                    || (NC > 0
                        && ((e.cos_a > 0.0 && min_dist2(lo, hi, e.start) * e.cos_a * e.cos_a < t2)
                            || (e.cos_b > 0.0 && min_dist2(lo, hi, e.end) * e.cos_b * e.cos_b < t2)));
                if keep {
                    self.cand.push(i);
                }
            }
            self.cand_range[g] = (start, self.cand.len() as u32);
        }
        self.tiled = true;
    }

    /// Distances at `p`: `NC` channels then the true distance in slot 3 (when `TD`).
    #[inline]
    pub fn distance(&mut self, p: Vec2) -> Dist {
        let edges = &self.shape.edges;
        let hot = &self.shape.hot;
        for g in 0..self.groups.len() {
            let list = if self.tiled {
                let (a, b) = self.cand_range[g];
                &self.cand[a as usize..b as usize]
            } else {
                let (a, b) = self.groups[g];
                &self.all[a as usize..b as usize]
            };
            self.counter = self.counter.wrapping_add(1);
            if self.counter == 0 {
                self.stamp.iter_mut().for_each(|s| *s = 0);
                self.counter = 1;
            }
            let st = &mut self.states[g];
            st.reset();
            scan::<NC, TD>(
                edges,
                hot,
                list,
                p,
                st,
                &mut self.seeds[g],
                &mut self.stamp,
                self.counter,
                &mut self.hints,
            );
        }
        if !self.overlap {
            return self.states[0].resolve(edges, p);
        }
        combine_overlapping::<NC>(
            &self.states,
            &self.shape.contours,
            edges,
            &mut self.dists,
            &mut self.pseudo,
            p,
        )
    }
}

/// Combines per-contour results so that overlapping contours produce the distance to the
/// outline of their union (heuristic equivalent to msdfgen's overlapping contour combiner).
fn combine_overlapping<const NC: usize>(
    states: &[State<NC>],
    contours: &[PContour],
    edges: &[PEdge],
    dists: &mut [Dist],
    pseudo: &mut [[f64; 3]],
    p: Vec2,
) -> Dist {
    {
        let n = contours.len();
        let mut shape_acc = Acc::<NC>::EMPTY;
        let mut inner_acc = Acc::<NC>::EMPTY;
        let mut outer_acc = Acc::<NC>::EMPTY;
        for i in 0..n {
            let st = &states[i];
            let mut d = [0.0; 4];
            for c in 0..NC {
                let ch = &st.ch[c];
                let ps = ch.near_pseudo(edges, p);
                pseudo[i][c] = ps;
                d[c] = pick(ch.bucket(), ps);
            }
            d[3] = st.tsd.distance;
            dists[i] = d;
            shape_acc.merge(st, i);
            let s = scalar::<NC>(&d);
            let w = contours[i].winding;
            if w > 0 && s >= 0.0 {
                inner_acc.merge(st, i);
            }
            if w < 0 && s <= 0.0 {
                outer_acc.merge(st, i);
            }
        }
        let shape_d = shape_acc.resolve(pseudo);
        let inner_d = inner_acc.resolve(pseudo);
        let outer_d = outer_acc.resolve(pseudo);
        let inner_s = scalar::<NC>(&inner_d);
        let outer_s = scalar::<NC>(&outer_d);
        let mut d;
        let winding;
        if inner_s >= 0.0 && inner_s.abs() <= outer_s.abs() {
            d = inner_d;
            winding = 1;
            for i in 0..n {
                if contours[i].winding > 0 {
                    let s = scalar::<NC>(&dists[i]);
                    if s.abs() < outer_s.abs() && s > scalar::<NC>(&d) {
                        d = dists[i];
                    }
                }
            }
        } else if outer_s <= 0.0 && outer_s.abs() < inner_s.abs() {
            d = outer_d;
            winding = -1;
            for i in 0..n {
                if contours[i].winding < 0 {
                    let s = scalar::<NC>(&dists[i]);
                    if s.abs() < inner_s.abs() && s < scalar::<NC>(&d) {
                        d = dists[i];
                    }
                }
            }
        } else {
            return shape_d;
        }
        for i in 0..n {
            if contours[i].winding != winding {
                let s = scalar::<NC>(&dists[i]);
                let ds = scalar::<NC>(&d);
                if s * ds >= 0.0 && s.abs() < ds.abs() {
                    d = dists[i];
                }
            }
        }
        if scalar::<NC>(&d) == scalar::<NC>(&shape_d) {
            d = shape_d;
        }
        d
    }
}

/// Scans the listed edges and accumulates the selection state for point `p`.
/// The edges in `seeds` (nearest edges at the previous query point) are evaluated first so
/// the culling bounds are tight; afterwards `seeds` is updated for the next query.
#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn scan<const NC: usize, const TD: bool>(
    edges: &[PEdge],
    hot: &[Hot],
    list: &[u32],
    p: Vec2,
    st: &mut State<NC>,
    seeds: &mut [u32; 4],
    stamp: &mut [u32],
    counter: u32,
    hints: &mut [f64],
) {
    for &s in seeds.iter() {
        if s != NONE && stamp[s as usize] != counter {
            stamp[s as usize] = counter;
            visit::<NC, TD>(edges, hot, s, p, st, hints);
        }
    }
    for &i in list {
        if stamp[i as usize] != counter {
            visit::<NC, TD>(edges, hot, i, p, st, hints);
        }
    }
    // Remember nearest edges for the next texel.
    let mut k = 0;
    for c in 0..NC {
        seeds[k] = st.ch[c].near;
        k += 1;
    }
    if TD {
        seeds[k] = st.tnear;
        k += 1;
    }
    while k < 4 {
        seeds[k] = NONE;
        k += 1;
    }
}

/// Squared distance from the rectangle `[lo, hi]` to point `p` (0 inside).
#[inline(always)]
fn min_dist2(lo: Vec2, hi: Vec2, p: Vec2) -> f64 {
    let dx = (lo.x - p.x).max(p.x - hi.x).max(0.0);
    let dy = (lo.y - p.y).max(p.y - hi.y).max(0.0);
    dx * dx + dy * dy
}

/// Squared distance from the farthest point of the rectangle `[lo, hi]` to `p`.
#[inline(always)]
fn max_dist2(lo: Vec2, hi: Vec2, p: Vec2) -> f64 {
    let dx = (p.x - lo.x).abs().max((hi.x - p.x).abs());
    let dy = (p.y - lo.y).abs().max((hi.y - p.y).abs());
    dx * dx + dy * dy
}

/// Squared distance between two axis-aligned rectangles.
#[inline(always)]
fn rect_bbox_dist2(lo: Vec2, hi: Vec2, blo: Vec2, bhi: Vec2) -> f64 {
    let dx = (blo.x - hi.x).max(lo.x - bhi.x).max(0.0);
    let dy = (blo.y - hi.y).max(lo.y - bhi.y).max(0.0);
    dx * dx + dy * dy
}

#[inline(always)]
fn visit<const NC: usize, const TD: bool>(
    edges: &[PEdge],
    hot: &[Hot],
    i: u32,
    p: Vec2,
    st: &mut State<NC>,
    hints: &mut [f64],
) {
    let e = &hot[i as usize];
    let mask: u8 = match NC {
        3 => e.color,
        1 => 1,
        _ => 0,
    };
    // Largest current distance among the selectors this edge feeds: if the edge cannot get
    // closer than this, it cannot change any of them. The table includes a small relative
    // margin so that exact ties (e.g. two edges meeting at the nearest corner) are never
    // culled by rounding - ties are decided by the orthogonality tie-break.
    let t2 = st.t2[mask as usize];
    let lb2 = e.bbox_dist2(p);
    if lb2 > t2 {
        if NC > 0 {
            // The true distance can't improve, but a corner extension still might yield a
            // shorter perpendicular distance. Cheap lower bound first.
            let ap2 = (p - e.start).length_squared() * e.cos_a2;
            let bp2 = (p - e.end).length_squared() * e.cos_b2;
            if ap2 < t2 || bp2 < t2 {
                perpendiculars::<NC>(&edges[i as usize], p, t2, mask, st);
            }
        }
        return;
    }
    let (sd, param) = edges[i as usize].signed_distance_hint(p, &mut hints[i as usize]);
    let mut improved = false;
    if TD && sd.closer_than(&st.tsd) {
        st.tsd = sd;
        st.tnear = i;
        improved = true;
    }
    if NC > 0 {
        for c in 0..NC {
            // Branch-free update (the outcome is hard to predict).
            let ch = &mut st.ch[c];
            let take = (mask & (1 << c) != 0) & sd.closer_than(&ch.sd);
            ch.sd.distance = if take { sd.distance } else { ch.sd.distance };
            ch.sd.dot = if take { sd.dot } else { ch.sd.dot };
            ch.near = if take { i } else { ch.near };
            ch.param = if take { param } else { ch.param };
            improved |= take;
        }
        perpendiculars::<NC>(&edges[i as usize], p, sd.distance * sd.distance, mask, st);
    }
    if improved {
        st.update_thresholds::<TD>();
    }
}

/// Adds perpendicular distances to the extensions of the edge beyond its corners, for points
/// inside the corner's outer domain. `limit2` bounds the (squared) candidates of interest.
/// Branch-free: whether a corner applies is data dependent and hard to predict.
#[inline(always)]
fn perpendiculars<const NC: usize>(e: &PEdge, p: Vec2, limit2: f64, mask: u8, st: &mut State<NC>) {
    let ap = p - e.start;
    let pa = ap.cross(e.dir0);
    let va = (ap.dot(e.bis_a) > 0.0) & (ap.dot(e.dir0) < 0.0) & (pa * pa < limit2);
    let bp = p - e.end;
    let pb = bp.cross(e.dir1);
    let vb = (bp.dot(e.bis_b) < 0.0) & (bp.dot(e.dir1) > 0.0) & (pb * pb < limit2);
    if !(va | vb) {
        return;
    }
    for c in 0..NC {
        if mask & (1 << c) != 0 {
            let ch = &mut st.ch[c];
            if va {
                ch.add_perp(pa);
            }
            if vb {
                ch.add_perp(pb);
            }
        }
    }
}
