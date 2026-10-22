//! Edge segments (linear, quadratic and cubic Bézier curves), edge colors and signed distances.

use crate::math::{Vec2, mix, non_zero_sign};
use crate::solver::{solve_cubic_unit_normed, solve_quadratic};

/// Color channels an edge contributes to (bit set of R=1, G=2, B=4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct EdgeColor(pub u8);

impl EdgeColor {
    pub const BLACK: EdgeColor = EdgeColor(0);
    pub const RED: EdgeColor = EdgeColor(1);
    pub const GREEN: EdgeColor = EdgeColor(2);
    pub const YELLOW: EdgeColor = EdgeColor(3);
    pub const BLUE: EdgeColor = EdgeColor(4);
    pub const MAGENTA: EdgeColor = EdgeColor(5);
    pub const CYAN: EdgeColor = EdgeColor(6);
    pub const WHITE: EdgeColor = EdgeColor(7);

    #[inline(always)]
    pub fn has(self, channel_mask: u8) -> bool {
        self.0 & channel_mask != 0
    }
    #[inline(always)]
    pub fn bits(self) -> u8 {
        self.0
    }
    /// Number of channels set.
    #[inline(always)]
    pub fn channel_count(self) -> u32 {
        (self.0 & 7).count_ones()
    }
}

impl core::ops::BitAnd for EdgeColor {
    type Output = EdgeColor;
    fn bitand(self, o: EdgeColor) -> EdgeColor {
        EdgeColor(self.0 & o.0)
    }
}
impl core::ops::BitOr for EdgeColor {
    type Output = EdgeColor;
    fn bitor(self, o: EdgeColor) -> EdgeColor {
        EdgeColor(self.0 | o.0)
    }
}
impl core::ops::BitXor for EdgeColor {
    type Output = EdgeColor;
    fn bitxor(self, o: EdgeColor) -> EdgeColor {
        EdgeColor(self.0 ^ o.0)
    }
}

/// A signed distance together with an orthogonality measure used to break ties between
/// equally distant edges (smaller `dot` = the query point is more perpendicular to the edge).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SignedDistance {
    pub distance: f64,
    pub dot: f64,
}

impl SignedDistance {
    /// The "no distance yet" value.
    pub const INFINITE: SignedDistance = SignedDistance {
        distance: -f64::MAX,
        dot: 0.0,
    };

    #[inline(always)]
    pub fn new(distance: f64, dot: f64) -> Self {
        SignedDistance { distance, dot }
    }

    /// Strict ordering by absolute distance, then by `dot`.
    #[inline(always)]
    pub fn closer_than(&self, o: &SignedDistance) -> bool {
        let a = self.distance.abs();
        let b = o.distance.abs();
        a < b || (a == b && self.dot < o.dot)
    }
}

impl Default for SignedDistance {
    fn default() -> Self {
        SignedDistance::INFINITE
    }
}

/// Geometry of a single edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Segment {
    Linear([Vec2; 2]),
    Quadratic([Vec2; 3]),
    Cubic([Vec2; 4]),
}

/// An edge segment of a contour together with its color.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Edge {
    pub segment: Segment,
    pub color: EdgeColor,
}

impl Edge {
    /// A straight line from `a` to `b`.
    pub fn line(a: Vec2, b: Vec2) -> Edge {
        Edge {
            segment: Segment::Linear([a, b]),
            color: EdgeColor::WHITE,
        }
    }

    /// A quadratic Bézier curve. A control point coinciding with an endpoint is moved to the
    /// middle of the chord to avoid a degenerate tangent.
    pub fn quadratic(a: Vec2, mut c: Vec2, b: Vec2) -> Edge {
        if c == a || c == b {
            c = (a + b) * 0.5;
        }
        Edge {
            segment: Segment::Quadratic([a, c, b]),
            color: EdgeColor::WHITE,
        }
    }

    /// A cubic Bézier curve. Fully degenerate control points are spread along the chord.
    pub fn cubic(a: Vec2, mut c1: Vec2, mut c2: Vec2, b: Vec2) -> Edge {
        if (c1 == a || c1 == b) && (c2 == a || c2 == b) {
            c1 = mix(a, b, 1.0 / 3.0);
            c2 = mix(a, b, 2.0 / 3.0);
        }
        Edge {
            segment: Segment::Cubic([a, c1, c2, b]),
            color: EdgeColor::WHITE,
        }
    }

    pub fn with_color(mut self, color: EdgeColor) -> Edge {
        self.color = color;
        self
    }

    #[inline]
    pub fn point(&self, t: f64) -> Vec2 {
        self.segment.point(t)
    }
    #[inline]
    pub fn direction(&self, t: f64) -> Vec2 {
        self.segment.direction(t)
    }
    #[inline]
    pub fn start(&self) -> Vec2 {
        self.segment.start()
    }
    #[inline]
    pub fn end(&self) -> Vec2 {
        self.segment.end()
    }
}

impl Segment {
    pub fn control_points(&self) -> &[Vec2] {
        match self {
            Segment::Linear(p) => p,
            Segment::Quadratic(p) => p,
            Segment::Cubic(p) => p,
        }
    }

    pub fn control_points_mut(&mut self) -> &mut [Vec2] {
        match self {
            Segment::Linear(p) => p,
            Segment::Quadratic(p) => p,
            Segment::Cubic(p) => p,
        }
    }

    #[inline]
    pub fn start(&self) -> Vec2 {
        self.control_points()[0]
    }

    #[inline]
    pub fn end(&self) -> Vec2 {
        *self.control_points().last().unwrap()
    }

    /// Point at parameter `t` in `[0, 1]`.
    pub fn point(&self, t: f64) -> Vec2 {
        match *self {
            Segment::Linear([a, b]) => mix(a, b, t),
            Segment::Quadratic([a, b, c]) => mix(mix(a, b, t), mix(b, c, t), t),
            Segment::Cubic([a, b, c, d]) => {
                let bc = mix(b, c, t);
                mix(mix(mix(a, b, t), bc, t), mix(bc, mix(c, d, t), t), t)
            }
        }
    }

    /// Tangent direction (not normalized) at `t`. Degenerate tangents fall back to a chord direction.
    pub fn direction(&self, t: f64) -> Vec2 {
        match *self {
            Segment::Linear([a, b]) => b - a,
            Segment::Quadratic([a, b, c]) => {
                let tan = mix(b - a, c - b, t);
                if tan.is_zero() { c - a } else { tan }
            }
            Segment::Cubic([a, b, c, d]) => {
                let tan = mix(mix(b - a, c - b, t), mix(c - b, d - c, t), t);
                if tan.is_zero() {
                    if t == 0.0 {
                        let v = c - a;
                        if !v.is_zero() {
                            return v;
                        }
                    } else if t == 1.0 {
                        let v = d - b;
                        if !v.is_zero() {
                            return v;
                        }
                    }
                    let v = d - a;
                    if !v.is_zero() {
                        return v;
                    }
                }
                tan
            }
        }
    }

    /// Change of direction (proportional to the second derivative) at `t`.
    pub fn direction_change(&self, t: f64) -> Vec2 {
        match *self {
            Segment::Linear(_) => Vec2::ZERO,
            Segment::Quadratic([a, b, c]) => (c - b) - (b - a),
            Segment::Cubic([a, b, c, d]) => mix((c - b) - (b - a), (d - c) - (c - b), t),
        }
    }

    /// Reverses the direction of the segment.
    pub fn reverse(&mut self) {
        self.control_points_mut().reverse();
    }

    /// Approximate arc length.
    pub fn length(&self) -> f64 {
        if let Segment::Linear([a, b]) = *self {
            (b - a).length()
        } else {
            const N: usize = 16;
            let mut len = 0.0;
            let mut prev = self.start();
            for i in 1..=N {
                let cur = self.point(i as f64 / N as f64);
                len += (cur - prev).length();
                prev = cur;
            }
            len
        }
    }

    /// True if all control points coincide.
    pub fn is_degenerate(&self) -> bool {
        let pts = self.control_points();
        pts.iter().all(|p| *p == pts[0])
    }

    /// Splits the segment at parameter `t`.
    pub fn split(&self, t: f64) -> (Segment, Segment) {
        match *self {
            Segment::Linear([a, b]) => {
                let m = mix(a, b, t);
                (Segment::Linear([a, m]), Segment::Linear([m, b]))
            }
            Segment::Quadratic([a, b, c]) => {
                let ab = mix(a, b, t);
                let bc = mix(b, c, t);
                let m = mix(ab, bc, t);
                (Segment::Quadratic([a, ab, m]), Segment::Quadratic([m, bc, c]))
            }
            Segment::Cubic([a, b, c, d]) => {
                let ab = mix(a, b, t);
                let bc = mix(b, c, t);
                let cd = mix(c, d, t);
                let abc = mix(ab, bc, t);
                let bcd = mix(bc, cd, t);
                let m = mix(abc, bcd, t);
                (Segment::Cubic([a, ab, abc, m]), Segment::Cubic([m, bcd, cd, d]))
            }
        }
    }

    /// Splits the segment into three parts of equal parameter length.
    pub fn split_in_thirds(&self) -> [Segment; 3] {
        let (s0, rest) = self.split(1.0 / 3.0);
        let (s1, s2) = rest.split(0.5);
        [s0, s1, s2]
    }

    /// Exact conversion of a quadratic segment to a cubic one (identity for cubics).
    pub fn to_cubic(&self) -> Segment {
        match *self {
            Segment::Linear([a, b]) => Segment::Cubic([a, mix(a, b, 1.0 / 3.0), mix(a, b, 2.0 / 3.0), b]),
            Segment::Quadratic([a, b, c]) => Segment::Cubic([a, mix(a, b, 2.0 / 3.0), mix(c, b, 2.0 / 3.0), c]),
            s @ Segment::Cubic(_) => s,
        }
    }

    /// Tight axis-aligned bounding box `(min, max)`.
    pub fn bounds(&self) -> (Vec2, Vec2) {
        let pts = self.control_points();
        let first = pts[0];
        let last = *pts.last().unwrap();
        let mut lo = first.min(last);
        let mut hi = first.max(last);
        let include = |t: f64, lo: &mut Vec2, hi: &mut Vec2| {
            if t > 0.0 && t < 1.0 {
                let p = self.point(t);
                *lo = lo.min(p);
                *hi = hi.max(p);
            }
        };
        match *self {
            Segment::Linear(_) => {}
            Segment::Quadratic([a, b, c]) => {
                // Derivative is linear: zero at t = (a - b) / (a - 2b + c).
                let den = a - 2.0 * b + c;
                if den.x != 0.0 {
                    include((a.x - b.x) / den.x, &mut lo, &mut hi);
                }
                if den.y != 0.0 {
                    include((a.y - b.y) / den.y, &mut lo, &mut hi);
                }
            }
            Segment::Cubic([a, b, c, d]) => {
                // Derivative / 3 = (b-a)(1-t)^2 + 2(c-b)t(1-t) + (d-c)t^2
                let a0 = b - a;
                let a1 = 2.0 * (c - b - a0);
                let a2 = d - 3.0 * c + 3.0 * b - a;
                for r in solve_quadratic(a2.x, a1.x, a0.x).as_slice() {
                    include(*r, &mut lo, &mut hi);
                }
                for r in solve_quadratic(a2.y, a1.y, a0.y).as_slice() {
                    include(*r, &mut lo, &mut hi);
                }
            }
        }
        (lo, hi)
    }

    /// Signed distance from `p` to the segment and the parameter of the nearest point.
    ///
    /// The parameter is in `[0, 1]` when the nearest point is on the segment proper, below 0
    /// if `p` lies "behind" the start point (relative to the start tangent), and above 1 if
    /// it lies beyond the end point. Positive distance = right-hand side of the direction of
    /// travel (the filled side under this crate's orientation convention).
    ///
    /// Each call prepares the curve from scratch (for a cubic that is dozens of
    /// transcendental evaluations); for many queries, prepare a shape once
    /// ([`crate::PreparedShape`]) or generate a field.
    pub fn signed_distance(&self, p: Vec2) -> (SignedDistance, f64) {
        match *self {
            Segment::Linear([a, b]) => line_signed_distance(a, b, p),
            Segment::Quadratic(pts) => QuadData::new(&pts).signed_distance(&pts, p),
            Segment::Cubic(pts) => CubicData::new(&pts).signed_distance(p),
        }
    }

    /// Converts a true signed distance to the perpendicular ("pseudo") distance to the
    /// extension of the segment beyond its endpoints, when applicable.
    pub fn to_pseudo_distance(&self, sd: &mut SignedDistance, p: Vec2, param: f64) {
        if param < 0.0 {
            let dir = self.direction(0.0).normalize_or_zero();
            pseudo_extend(sd, p - self.start(), dir, false);
        } else if param > 1.0 {
            let dir = self.direction(1.0).normalize_or_zero();
            pseudo_extend(sd, p - self.end(), dir, true);
        }
    }

    /// Appends the intersections of the segment with the horizontal line at `y` as
    /// `(x, dir)` pairs, where `dir` is +1 when the segment crosses upward and -1 when downward.
    /// Uses a half-open convention so shared vertices are counted consistently.
    pub fn scanline_crossings(&self, y: f64, out: &mut Vec<(f64, i32)>) {
        if let Segment::Linear([a, b]) = *self {
            if a.y != b.y {
                let (lo, hi) = if a.y < b.y { (a.y, b.y) } else { (b.y, a.y) };
                if y >= lo && y < hi {
                    let t = (y - a.y) / (b.y - a.y);
                    out.push((a.x + (b.x - a.x) * t, if b.y > a.y { 1 } else { -1 }));
                }
            }
        } else {
            // Cheap rejection with the control points (the curve lies in their hull, so
            // this implies the exact test below).
            let pts = self.control_points();
            let (mut cmin, mut cmax) = (pts[0].y, pts[0].y);
            for q in &pts[1..] {
                cmin = cmin.min(q.y);
                cmax = cmax.max(q.y);
            }
            if y < cmin || y >= cmax {
                return;
            }
            let (lo, hi) = self.bounds();
            if y < lo.y || y >= hi.y {
                // A curve can still touch y at its max; the half-open rule excludes it anyway.
                return;
            }
            let mut ts = [0.0f64; 4];
            let mut n = 0;
            ts[n] = 0.0;
            n += 1;
            match *self {
                Segment::Quadratic([a, b, c]) => {
                    let den = a.y - 2.0 * b.y + c.y;
                    if den != 0.0 {
                        let t = (a.y - b.y) / den;
                        if t > 0.0 && t < 1.0 {
                            ts[n] = t;
                            n += 1;
                        }
                    }
                }
                Segment::Cubic([a, b, c, d]) => {
                    let a0 = b.y - a.y;
                    let a1 = 2.0 * (c.y - b.y - a0);
                    let a2 = d.y - 3.0 * c.y + 3.0 * b.y - a.y;
                    let r = solve_quadratic(a2, a1, a0);
                    let mut rs = [0.0; 2];
                    let mut m = 0;
                    for &t in r.as_slice() {
                        if t > 0.0 && t < 1.0 {
                            rs[m] = t;
                            m += 1;
                        }
                    }
                    if m == 2 && rs[0] > rs[1] {
                        rs.swap(0, 1);
                    }
                    for &t in &rs[..m] {
                        ts[n] = t;
                        n += 1;
                    }
                }
                Segment::Linear(_) => unreachable!(),
            }
            ts[n] = 1.0;
            n += 1;
            for i in 0..n - 1 {
                let (t0, t1) = (ts[i], ts[i + 1]);
                if t1 <= t0 {
                    continue;
                }
                let y0 = self.point(t0).y;
                let y1 = self.point(t1).y;
                if y0 == y1 {
                    continue;
                }
                let (ylo, yhi) = if y0 < y1 { (y0, y1) } else { (y1, y0) };
                if y >= ylo && y < yhi {
                    let t = solve_monotone(self, t0, t1, y0, y1, y);
                    out.push((self.point(t).x, if y1 > y0 { 1 } else { -1 }));
                }
            }
        }
    }
}

/// Finds `t` in `[t0, t1]` where the segment's y equals `target`, given that y is monotone on
/// the interval (`y0`, `y1`: its values at the ends): Newton steps inside the shrinking
/// bracket, bisection when a step would leave it.
fn solve_monotone(seg: &Segment, t0: f64, t1: f64, y0: f64, y1: f64, target: f64) -> f64 {
    let increasing = y1 > y0;
    let (mut lo, mut hi) = (t0, t1);
    let mut t = t0 + (target - y0) * (t1 - t0) / (y1 - y0);
    if let Segment::Quadratic([a, b, c]) = *seg {
        // Closed form start (the root of the quadratic inside the piece), polished below.
        let qa = a.y - 2.0 * b.y + c.y;
        let qb = 2.0 * (b.y - a.y);
        let qc = a.y - target;
        for &r in solve_quadratic(qa, qb, qc).as_slice() {
            if r >= lo && r <= hi {
                t = r;
            }
        }
    }
    if !(t > lo && t < hi) {
        t = f64::midpoint(lo, hi);
    }
    for _ in 0..100 {
        let ft = seg.point(t).y - target;
        if ft == 0.0 {
            return t;
        }
        if (ft < 0.0) == increasing {
            lo = t;
        } else {
            hi = t;
        }
        // `direction` is the derivative divided by the degree.
        let deg = match seg {
            Segment::Linear(_) => 1.0,
            Segment::Quadratic(_) => 2.0,
            Segment::Cubic(_) => 3.0,
        };
        let d = deg * seg.direction(t).y;
        let mut nt = t - ft / d;
        if !(nt > lo && nt < hi) {
            nt = f64::midpoint(lo, hi);
        }
        if (nt - t).abs() <= 1e-15 || hi - lo <= 1e-15 {
            return nt;
        }
        t = nt;
    }
    t
}

#[inline(always)]
pub(crate) fn pseudo_extend(sd: &mut SignedDistance, ep: Vec2, dir: Vec2, forward: bool) {
    let ts = ep.dot(dir);
    if (forward && ts > 0.0) || (!forward && ts < 0.0) {
        let pd = ep.cross(dir);
        if pd.abs() <= sd.distance.abs() {
            sd.distance = pd;
            sd.dot = 0.0;
        }
    }
}

#[inline(always)]
pub(crate) fn line_signed_distance(a: Vec2, b: Vec2, p: Vec2) -> (SignedDistance, f64) {
    let aq = p - a;
    let ab = b - a;
    let len2 = ab.length_squared();
    let param = if len2 > 0.0 { aq.dot(ab) / len2 } else { 0.0 };
    let eq = if param > 0.5 { b - p } else { a - p };
    let ep_dist = eq.length();
    let c = aq.cross(ab);
    if param > 0.0 && param < 1.0 {
        let ortho = c / len2.sqrt();
        if ortho.abs() < ep_dist {
            return (SignedDistance::new(ortho, 0.0), param);
        }
    }
    let dot = if ep_dist > 0.0 && len2 > 0.0 {
        (ab.dot(eq) / (len2.sqrt() * ep_dist)).abs()
    } else {
        0.0
    };
    (SignedDistance::new(non_zero_sign(c) * ep_dist, dot), param)
}

/// Precomputed data for line segment distance queries.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LineData {
    ab: Vec2,
    /// 1/|ab|^2 and 1/|ab| (0 for a degenerate segment).
    inv_len2: f64,
    inv_len: f64,
}

impl LineData {
    pub(crate) fn new(a: Vec2, b: Vec2) -> Self {
        let ab = b - a;
        let len2 = ab.length_squared();
        LineData {
            ab,
            inv_len2: if len2 > 0.0 { 1.0 / len2 } else { 0.0 },
            inv_len: if len2 > 0.0 { 1.0 / len2.sqrt() } else { 0.0 },
        }
    }

    /// [`line_signed_distance`] without divisions, and without a square root when the
    /// nearest point is inside the segment.
    #[inline(always)]
    pub(crate) fn signed_distance(&self, a: Vec2, b: Vec2, p: Vec2) -> (SignedDistance, f64) {
        let aq = p - a;
        let ab = self.ab;
        let param = aq.dot(ab) * self.inv_len2;
        let eq = if param > 0.5 { b - p } else { a - p };
        let c = aq.cross(ab);
        let ep2 = eq.length_squared();
        if param > 0.0 && param < 1.0 {
            let ortho = c * self.inv_len;
            if ortho * ortho < ep2 {
                return (SignedDistance::new(ortho, 0.0), param);
            }
        }
        let ep_dist = ep2.sqrt();
        let dot = if ep_dist > 0.0 && self.inv_len > 0.0 {
            (ab.dot(eq) * self.inv_len / ep_dist).abs()
        } else {
            0.0
        };
        (SignedDistance::new(non_zero_sign(c) * ep_dist, dot), param)
    }
}

/// Builds the signed distance result for a nearest point at an endpoint, from the edge's
/// direction there, its unit vector and inverse squared length (precomputed per edge).
#[inline(always)]
pub(crate) fn endpoint_distance_pre(
    p: Vec2,
    ep: Vec2,
    dir: Vec2,
    unit: Vec2,
    inv_len2: f64,
    at_end: bool,
    dist2: f64,
) -> (SignedDistance, f64) {
    let pe = p - ep;
    let dist = dist2.sqrt();
    let dot = if dist > 0.0 { (unit.dot(pe) / dist).abs() } else { 0.0 };
    let sd = SignedDistance::new(non_zero_sign(pe.cross(dir)) * dist, dot);
    let proj = pe.dot(dir) * inv_len2;
    let param = if at_end { 1.0 + proj.max(0.0) } else { proj.min(0.0) };
    (sd, param)
}

/// Finds the root of `f` in `(lo, hi)` given `f(lo) < 0 < f(hi)` and `f` increasing there:
/// Newton steps safeguarded by bisection, starting from the secant estimate.
#[cfg(test)]
pub(crate) fn solve_increasing(
    f: &impl Fn(f64) -> f64,
    df: &impl Fn(f64) -> f64,
    mut lo: f64,
    mut hi: f64,
    flo: f64,
    fhi: f64,
) -> f64 {
    let mut t = lo - flo * (hi - lo) / (fhi - flo);
    if !(t > lo && t < hi) {
        t = f64::midpoint(lo, hi);
    }
    for _ in 0..64 {
        let ft = f(t);
        if ft == 0.0 {
            return t;
        }
        if ft < 0.0 {
            lo = t;
        } else {
            hi = t;
        }
        let d = df(t);
        let mut nt = if d > 0.0 { t - ft / d } else { f64::NAN };
        if !(nt > lo && nt < hi) {
            nt = f64::midpoint(lo, hi);
        }
        if (nt - t).abs() <= 4.0 * f64::EPSILON * t.abs().max(1e-3) || hi - lo <= 4.0 * f64::EPSILON {
            return nt;
        }
        t = nt;
    }
    t
}

/// Safeguarded Newton iteration (Newton steps inside the shrinking bracket `[lo, hi]`, with
/// `f(lo) < 0 < f(hi)`, bisection otherwise) for nearest-point parameters: stops as soon as a
/// Newton step is below `T_TOL` (after applying it). At a distance minimum the squared distance is stationary, so
/// an error `e` in the parameter only changes it by `O(e^2)`; with quadratic convergence the
/// final step leaves an error far below that anyway. Saves the confirming iterations.
#[inline]
pub(crate) fn rtsafe(f: &impl Fn(f64) -> (f64, f64), mut lo: f64, mut hi: f64, flo: f64, fhi: f64, start: f64) -> f64 {
    const T_TOL: f64 = 1e-9;
    let mut t = if start > lo && start < hi {
        start
    } else {
        lo - flo * (hi - lo) / (fhi - flo)
    };
    if !(t > lo && t < hi) {
        t = f64::midpoint(lo, hi);
    }
    let mut dxold = hi - lo;
    let mut dx = dxold;
    let (mut ft, mut dft) = f(t);
    for _ in 0..100 {
        if ft == 0.0 {
            return t;
        }
        if ft < 0.0 {
            lo = t;
        } else {
            hi = t;
        }
        let nt = t - ft / dft;
        let newton_ok = dft > 0.0 && nt > lo && nt < hi && (2.0 * ft).abs() <= (dxold * dft).abs();
        dxold = dx;
        if newton_ok {
            dx = t - nt;
            t = nt;
            if dx.abs() <= T_TOL {
                return t;
            }
        } else {
            dx = 0.5 * (hi - lo);
            t = lo + dx;
            if t == lo || t == hi {
                return t;
            }
        }
        let r = f(t);
        ft = r.0;
        dft = r.1;
    }
    t
}

/// Precomputed data for quadratic segment distance queries.
#[derive(Clone, Copy, Debug)]
pub(crate) struct QuadData {
    ab: Vec2,
    br: Vec2,
    ca: f64,
    cb: f64,
    cc0: f64,
    /// 1/ca, cb/ca, cc0/ca (when the cubic term is significant).
    inv_ca: f64,
    cb_n: f64,
    cc0_n: f64,
    dir0: Vec2,
    dir1: Vec2,
    unit0: Vec2,
    unit1: Vec2,
    inv_len2_0: f64,
    inv_len2_1: f64,
}

impl QuadData {
    /// `ca`, `cb`, `cc0` (see [`crate::gpu`]).
    #[cfg(feature = "gpu")]
    pub(crate) fn gpu_coeffs(&self) -> [f64; 3] {
        [self.ca, self.cb, self.cc0]
    }

    pub(crate) fn new(p: &[Vec2; 3]) -> Self {
        let ab = p[1] - p[0];
        let br = p[2] - p[1] - ab;
        let seg = Segment::Quadratic(*p);
        let ca = br.dot(br);
        let cb = 3.0 * ab.dot(br);
        let cc0 = 2.0 * ab.dot(ab);
        let dir0 = seg.direction(0.0);
        let dir1 = seg.direction(1.0);
        let inv = |v: Vec2| {
            if v.is_zero() { 0.0 } else { 1.0 / v.length_squared() }
        };
        // Below this ratio the cubic formula is used; beyond it the cubic term is numerically
        // irrelevant and the equation is solved as a quadratic.
        let cubic = ca != 0.0 && (cb / ca).abs() < 1e6;
        QuadData {
            ab,
            br,
            ca,
            cb,
            cc0,
            inv_ca: if cubic { 1.0 / ca } else { 0.0 },
            cb_n: if cubic { cb / ca } else { 0.0 },
            cc0_n: if cubic { cc0 / ca } else { 0.0 },
            dir0,
            dir1,
            unit0: dir0.normalize_or_zero(),
            unit1: dir1.normalize_or_zero(),
            inv_len2_0: inv(dir0),
            inv_len2_1: inv(dir1),
        }
    }

    /// Nearest point with a warm start: like the cold-start Newton solve (kept in the tests as
    /// a reference) but the iteration starts at `hint` (the parameter found at a nearby point), which typically
    /// converges in one or two steps. `hint` is updated.
    #[inline]
    pub(crate) fn signed_distance_warm(&self, pts: &[Vec2; 3], p: Vec2, hint: &mut f64) -> (SignedDistance, f64) {
        let qa = pts[0] - p;
        let (ca, cb) = (self.ca, self.cb);
        let cc = self.cc0 + qa.dot(self.br);
        let cd = qa.dot(self.ab);
        let ca3 = 3.0 * ca;
        let cb2 = 2.0 * cb;
        let fd = |t: f64| (((ca * t + cb) * t + cc) * t + cd, (ca3 * t + cb2) * t + cc);
        let d0 = qa.length_squared();
        let d1 = (pts[2] - p).length_squared();
        let (mut best, mut best_t, mut best_kind) = if d1 < d0 { (d1, 1.0, 2u8) } else { (d0, 0.0, 0u8) };
        // Critical points of f split [0, 1] into intervals where f is monotone.
        let mut bp = [0.0f64; 4];
        let mut n = 1;
        let crit = solve_quadratic(ca3, cb2, cc);
        let (mut c0, mut c1) = (2.0, 2.0);
        if crit.count >= 1 {
            c0 = crit.roots[0];
        }
        if crit.count >= 2 {
            c1 = crit.roots[1];
            if c1 < c0 {
                core::mem::swap(&mut c0, &mut c1);
            }
        }
        if c0 > 0.0 && c0 < 1.0 {
            bp[n] = c0;
            n += 1;
        }
        if c1 > 0.0 && c1 < 1.0 && c1 != c0 {
            bp[n] = c1;
            n += 1;
        }
        bp[n] = 1.0;
        n += 1;
        let mut flo = cd;
        for k in 0..n - 1 {
            let (lo, hi) = (bp[k], bp[k + 1]);
            let fhi = if k + 2 == n { ((ca + cb) + cc) + cd } else { fd(hi).0 };
            if flo < 0.0 && fhi > 0.0 {
                let t = rtsafe(&fd, lo, hi, flo, fhi, *hint);
                if t > 0.0 && t < 1.0 {
                    let q = qa + (2.0 * t) * self.ab + (t * t) * self.br;
                    let d = q.length_squared();
                    if d <= best {
                        best = d;
                        best_t = t;
                        best_kind = 1;
                    }
                }
            }
            flo = fhi;
        }
        match best_kind {
            0 => endpoint_distance_pre(p, pts[0], self.dir0, self.unit0, self.inv_len2_0, false, best),
            2 => endpoint_distance_pre(p, pts[2], self.dir1, self.unit1, self.inv_len2_1, true, best),
            _ => {
                let t = best_t;
                *hint = t;
                let q = qa + (2.0 * t) * self.ab + (t * t) * self.br;
                let mut dir = self.ab + t * self.br;
                if dir.is_zero() {
                    dir = pts[2] - pts[0];
                }
                let s = non_zero_sign((-q).cross(dir));
                (SignedDistance::new(s * best.sqrt(), 0.0), t)
            }
        }
    }

    /// Alternative (testing) nearest point via the local minima of |B(t)-p|^2: the derivative is a cubic in t whose
    /// increasing zero crossings are bracketed using its critical points and then solved by
    /// safeguarded Newton iteration. Avoids the trigonometric cubic formula (several times
    /// faster) while being at least as accurate.
    /// The cold-start Newton solve, a reference for the tests.
    #[cfg(test)]
    pub(crate) fn signed_distance_newton(&self, pts: &[Vec2; 3], p: Vec2) -> (SignedDistance, f64) {
        let qa = pts[0] - p;
        // f(t) = d/dt |B(t)-p|^2 / 4 = ca t^3 + cb t^2 + cc t + cd
        let (ca, cb) = (self.ca, self.cb);
        let cc = self.cc0 + qa.dot(self.br);
        let cd = qa.dot(self.ab);
        let f = |t: f64| ((ca * t + cb) * t + cc) * t + cd;
        let df = |t: f64| (3.0 * ca * t + 2.0 * cb) * t + cc;
        let d0 = qa.length_squared();
        let d1 = (pts[2] - p).length_squared();
        let (mut best, mut best_t, mut best_kind) = if d1 < d0 { (d1, 1.0, 2u8) } else { (d0, 0.0, 0u8) };
        // Breakpoints: 0, critical points of f inside (0, 1), 1.
        let mut bp = [0.0f64; 4];
        let mut n = 1;
        let crit = solve_quadratic(3.0 * ca, 2.0 * cb, cc);
        let (mut c0, mut c1) = (2.0, 2.0);
        if crit.count >= 1 {
            c0 = crit.roots[0];
        }
        if crit.count >= 2 {
            c1 = crit.roots[1];
            if c1 < c0 {
                core::mem::swap(&mut c0, &mut c1);
            }
        }
        if c0 > 0.0 && c0 < 1.0 {
            bp[n] = c0;
            n += 1;
        }
        if c1 > 0.0 && c1 < 1.0 && c1 != c0 {
            bp[n] = c1;
            n += 1;
        }
        bp[n] = 1.0;
        n += 1;
        let mut flo = cd; // f(0)
        for k in 0..n - 1 {
            let (lo, hi) = (bp[k], bp[k + 1]);
            let fhi = if k + 2 == n { ((ca + cb) + cc) + cd } else { f(hi) };
            // A local minimum of the distance: f crosses from negative to positive.
            if flo < 0.0 && fhi > 0.0 {
                let t = solve_increasing(&f, &df, lo, hi, flo, fhi);
                if t > 0.0 && t < 1.0 {
                    let q = qa + (2.0 * t) * self.ab + (t * t) * self.br;
                    let d = q.length_squared();
                    if d <= best {
                        best = d;
                        best_t = t;
                        best_kind = 1;
                    }
                }
            }
            flo = fhi;
        }
        match best_kind {
            0 => endpoint_distance_pre(p, pts[0], self.dir0, self.unit0, self.inv_len2_0, false, best),
            2 => endpoint_distance_pre(p, pts[2], self.dir1, self.unit1, self.inv_len2_1, true, best),
            _ => {
                let t = best_t;
                let q = qa + (2.0 * t) * self.ab + (t * t) * self.br; // B(t) - p
                let mut dir = self.ab + t * self.br;
                if dir.is_zero() {
                    dir = pts[2] - pts[0];
                }
                let s = non_zero_sign((-q).cross(dir));
                (SignedDistance::new(s * best.sqrt(), 0.0), t)
            }
        }
    }

    /// Closed-form nearest point (cubic formula with a single `sin_cos`).
    #[inline]
    pub(crate) fn signed_distance(&self, pts: &[Vec2; 3], p: Vec2) -> (SignedDistance, f64) {
        let qa = pts[0] - p;
        let cc = self.cc0 + qa.dot(self.br);
        let cd = qa.dot(self.ab);
        let roots = if self.inv_ca == 0.0 {
            solve_quadratic(self.cb, cc, cd).into()
        } else {
            solve_cubic_unit_normed(
                self.cb_n,
                self.cc0_n + qa.dot(self.br) * self.inv_ca,
                cd * self.inv_ca,
                [self.ca, self.cb, cc, cd],
            )
        };
        let d0 = qa.length_squared();
        let d1 = (pts[2] - p).length_squared();
        let (mut best, mut best_t, mut best_kind) = if d1 < d0 { (d1, 1.0, 2u8) } else { (d0, 0.0, 0u8) };
        for &t in roots.as_slice() {
            if t > 0.0 && t < 1.0 {
                let q = qa + (2.0 * t) * self.ab + (t * t) * self.br;
                let d = q.length_squared();
                if d <= best {
                    best = d;
                    best_t = t;
                    best_kind = 1;
                }
            }
        }
        match best_kind {
            0 => endpoint_distance_pre(p, pts[0], self.dir0, self.unit0, self.inv_len2_0, false, best),
            2 => endpoint_distance_pre(p, pts[2], self.dir1, self.unit1, self.inv_len2_1, true, best),
            _ => {
                let t = best_t;
                let q = qa + (2.0 * t) * self.ab + (t * t) * self.br;
                let mut dir = self.ab + t * self.br;
                if dir.is_zero() {
                    dir = pts[2] - pts[0];
                }
                let s = non_zero_sign((-q).cross(dir));
                (SignedDistance::new(s * best.sqrt(), 0.0), t)
            }
        }
    }
}

/// Maximum number of samples used to bracket the nearest point on a cubic curve.
pub(crate) const CUBIC_MAX_SAMPLES: usize = 25;
/// Maximum tangent turning (radians) between consecutive samples.
const CUBIC_SAMPLE_TURN: f64 = core::f64::consts::PI / 6.0;
/// Maximum parameter gap between consecutive samples.
const CUBIC_SAMPLE_GAP: f64 = 0.25;

/// Precomputed data for exact cubic (and quadratic) nearest point queries.
///
/// The squared distance g(t) = |B(t)-p|^2 is minimized exactly:
/// * samples placed every <= 30 degrees of tangent turning give an initial best distance,
/// * each sample interval is pruned with a rigorous lower bound (distance to the chord minus
///   the interpolation error bound `h^2/8 * max|B''|`),
/// * on the remaining intervals the derivative f = g'/2 (a degree-5 polynomial) is converted
///   to Bernstein form; by Descartes' rule of signs, no sign change proves there is no
///   interior extremum, a single sign change isolates exactly one root (refined by
///   safeguarded Newton), and otherwise the interval is subdivided.
///
/// Unlike Newton iteration from a handful of fixed starting points (msdfgen's approach), this
/// cannot miss the global minimum of a tight loop or converge to a local maximum.
///
/// About 5 KB (per-interval data for the subdivision): not `Copy`, so it is never copied by
/// accident; prepared shapes box it.
#[derive(Clone, Debug)]
pub(crate) struct CubicData {
    p0: Vec2,
    a: Vec2,
    b: Vec2,
    c: Vec2,
    /// p-independent coefficients of f = (B-p).B': t^1 (partial), t^2 (partial), t^3, t^4, t^5.
    k1: f64,
    k2: f64,
    k3: f64,
    k4: f64,
    k5: f64,
    n: usize,
    ts: [f64; CUBIC_MAX_SAMPLES],
    pts: [Vec2; CUBIC_MAX_SAMPLES],
    /// |B''| at each sample (B'' is linear in t, so its maximum on an interval is at an end).
    dd: [f64; CUBIC_MAX_SAMPLES],
    /// Precomputed data of the intervals between consecutive samples.
    iv: [CubicInterval; CUBIC_MAX_SAMPLES - 1],
    dir0: Vec2,
    dir1: Vec2,
    unit0: Vec2,
    unit1: Vec2,
    inv_len2_0: f64,
    inv_len2_1: f64,
    start: Vec2,
    end: Vec2,
}

/// Running minimum of a nearest-point search.
struct Search {
    best: f64,
    best_t: f64,
    /// sqrt(best).
    sb: f64,
}

/// One sample interval `[t0, t1]` of a [`CubicData`].
///
/// f(t) = (B(t)-p).B'(t) is affine in p: with q = p0 - p,
/// f = F(t) + q.x Gx(t) + q.y Gy(t). The Bernstein coefficients of F, Gx and Gy on the
/// interval are precomputed, so a query only needs 12 multiply-adds to get those of f.
#[derive(Clone, Copy, Debug, Default)]
struct CubicInterval {
    /// Chord `pts[i+1] - pts[i]` and its inverse squared length (0 if degenerate).
    ch: Vec2,
    inv_l2: f64,
    /// Interpolation error bound of the chord: h^2/8 * max|B''|.
    sag: f64,
    bf: [f64; 6],
    bx: [f64; 6],
    by: [f64; 6],
}

/// Bernstein coefficients on `[a, a + h]` of the degree <= 5 polynomial with power basis
/// coefficients `coef` (Taylor shift to `a`, scaling by `h`, basis conversion).
fn bernstein5(coef: [f64; 6], a: f64, h: f64) -> [f64; 6] {
    let mut t5 = coef;
    // Synthetic division: t5[k] becomes the k-th Taylor coefficient at a.
    for k in 0..5 {
        for j in (k..5).rev() {
            t5[j] += a * t5[j + 1];
        }
    }
    let mut dk = [0.0f64; 6];
    let mut hp = 1.0;
    for k in 0..6 {
        dk[k] = t5[k] * hp;
        hp *= h;
    }
    let mut bern = [0.0f64; 6];
    for j in 0..6 {
        let row = &BERN5[j];
        let mut v = 0.0;
        for k in 0..=j {
            v += row[k] * dk[k];
        }
        bern[j] = v;
    }
    bern
}

/// Bernstein conversion: b_j = sum_{k<=j} C(j,k)/C(5,k) d_k.
const BERN5: [[f64; 6]; 6] = [
    [1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
    [1.0, 0.2, 0.0, 0.0, 0.0, 0.0],
    [1.0, 0.4, 0.1, 0.0, 0.0, 0.0],
    [1.0, 0.6, 0.3, 0.1, 0.0, 0.0],
    [1.0, 0.8, 0.6, 0.4, 0.2, 0.0],
    [1.0, 1.0, 1.0, 1.0, 1.0, 1.0],
];

impl CubicData {
    pub(crate) fn new(p: &[Vec2; 4]) -> Self {
        let a = 3.0 * (p[1] - p[0]);
        let b = 3.0 * (p[2] - 2.0 * p[1] + p[0]);
        let c = p[3] - 3.0 * p[2] + 3.0 * p[1] - p[0];
        Self::build(Segment::Cubic(*p), p[0], a, b, c, p[3])
    }

    fn build(seg: Segment, p0: Vec2, a: Vec2, b: Vec2, c: Vec2, p3: Vec2) -> Self {
        let dir0 = seg.direction(0.0);
        let dir1 = seg.direction(1.0);
        let inv = |v: Vec2| {
            if v.is_zero() { 0.0 } else { 1.0 / v.length_squared() }
        };
        let mut d = CubicData {
            p0,
            a,
            b,
            c,
            k1: a.dot(a),
            k2: 3.0 * a.dot(b),
            k3: 4.0 * a.dot(c) + 2.0 * b.dot(b),
            k4: 5.0 * b.dot(c),
            k5: 3.0 * c.dot(c),
            n: 0,
            ts: [0.0; CUBIC_MAX_SAMPLES],
            pts: [Vec2::ZERO; CUBIC_MAX_SAMPLES],
            dd: [0.0; CUBIC_MAX_SAMPLES],
            iv: [CubicInterval::default(); CUBIC_MAX_SAMPLES - 1],
            dir0,
            dir1,
            unit0: dir0.normalize_or_zero(),
            unit1: dir1.normalize_or_zero(),
            inv_len2_0: inv(dir0),
            inv_len2_1: inv(dir1),
            start: p0,
            end: p3,
        };
        // Choose sample parameters by accumulated tangent turning on a fine grid.
        const GRID: usize = 128;
        let mut ts = Vec::with_capacity(CUBIC_MAX_SAMPLES);
        ts.push(0.0);
        let mut prev_angle: Option<f64> = None;
        let mut turn = 0.0;
        let mut last_t = 0.0;
        for k in 1..GRID {
            let t = k as f64 / GRID as f64;
            let der = d.d1(t);
            if !der.is_zero() {
                let ang = der.y.atan2(der.x);
                if let Some(pa) = prev_angle {
                    let mut da = ang - pa;
                    while da > core::f64::consts::PI {
                        da -= core::f64::consts::TAU;
                    }
                    while da < -core::f64::consts::PI {
                        da += core::f64::consts::TAU;
                    }
                    turn += da.abs();
                }
                prev_angle = Some(ang);
            }
            if (turn >= CUBIC_SAMPLE_TURN || t - last_t >= CUBIC_SAMPLE_GAP - 1e-12) && ts.len() < CUBIC_MAX_SAMPLES - 1
            {
                ts.push(t);
                last_t = t;
                turn = 0.0;
            }
        }
        ts.push(1.0);
        d.n = ts.len();
        for (i, &t) in ts.iter().enumerate() {
            d.ts[i] = t;
            d.pts[i] = d.eval(t);
            d.dd[i] = d.d2(t).length();
        }
        d.pts[0] = p0;
        d.pts[d.n - 1] = p3;
        let fc = [0.0, d.k1, d.k2, d.k3, d.k4, d.k5];
        let gx = [a.x, 2.0 * b.x, 3.0 * c.x, 0.0, 0.0, 0.0];
        let gy = [a.y, 2.0 * b.y, 3.0 * c.y, 0.0, 0.0, 0.0];
        for i in 0..d.n - 1 {
            let (t0, t1) = (d.ts[i], d.ts[i + 1]);
            let h = t1 - t0;
            let ch = d.pts[i + 1] - d.pts[i];
            let l2 = ch.length_squared();
            d.iv[i] = CubicInterval {
                ch,
                inv_l2: if l2 > 0.0 { 1.0 / l2 } else { 0.0 },
                sag: 0.125 * h * h * d.dd[i].max(d.dd[i + 1]),
                bf: bernstein5(fc, t0, h),
                bx: bernstein5(gx, t0, h),
                by: bernstein5(gy, t0, h),
            };
        }
        d
    }

    #[inline(always)]
    fn eval(&self, t: f64) -> Vec2 {
        self.p0 + t * (self.a + t * (self.b + t * self.c))
    }
    #[inline(always)]
    fn d1(&self, t: f64) -> Vec2 {
        self.a + t * (2.0 * self.b + (3.0 * t) * self.c)
    }
    #[inline(always)]
    fn d2(&self, t: f64) -> Vec2 {
        2.0 * self.b + (6.0 * t) * self.c
    }

    /// Lower bound of the distance from `p` to the curve piece over `[a, b]` with end points
    /// `pa`, `pb` and `max|B''| <= dd`, compared against `lim` (sqrt of the current best):
    /// returns true if the piece cannot contain a closer point.
    #[inline(always)]
    fn piece_is_far(p: Vec2, pa: Vec2, pb: Vec2, h: f64, dd: f64, lim: f64) -> bool {
        let ch = pb - pa;
        let ap = p - pa;
        let l2 = ch.length_squared();
        let u = if l2 > 0.0 {
            (ap.dot(ch) / l2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let d2 = (ap - ch * u).length_squared();
        let r = lim + 0.125 * h * h * dd;
        d2 > r * r
    }

    /// Handles one piece `[a, b]` given the inner Bernstein coefficients of f (the end ones
    /// are replaced by exact values): no sign change - nothing; one - the root is solved if it
    /// is a minimum. Returns true if the piece has several sign changes (needs subdivision).
    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    fn solve_piece(
        &self,
        p: Vec2,
        fd: &impl Fn(f64) -> (f64, f64),
        bern: &mut [f64; 6],
        a: f64,
        b: f64,
        pa: Vec2,
        pb: Vec2,
        hint: &mut f64,
        st: &mut Search,
    ) -> bool {
        let fa = fd(a).0;
        let fb = fd(b).0;
        bern[0] = fa;
        bern[5] = fb;
        let mut changes = 0;
        let mut prev = 0.0f64;
        for &v in bern.iter() {
            if v != 0.0 {
                if prev != 0.0 && (v > 0.0) != (prev > 0.0) {
                    changes += 1;
                }
                prev = v;
            }
        }
        if changes == 0 {
            return false;
        }
        if changes > 1 {
            return true;
        }
        // Exactly one root inside: a minimum if f goes from negative to positive (the last
        // non-zero coefficient is positive). Note f(a) or f(b) may be exactly zero, e.g. at
        // an end with a vanishing derivative (coincident control points).
        if prev > 0.0 {
            let start = if *hint > a && *hint < b {
                *hint
            } else {
                let pm = p - pa;
                let ch = pb - pa;
                let l2 = ch.length_squared();
                let u = if l2 > 0.0 {
                    (pm.dot(ch) / l2).clamp(0.0, 1.0)
                } else {
                    0.5
                };
                a + (b - a) * u
            };
            let t = rtsafe(fd, a, b, fa, fb, start);
            let g = (self.eval(t) - p).length_squared();
            if g < st.best {
                st.best = g;
                st.best_t = t;
                st.sb = g.sqrt();
            }
        }
        false
    }

    /// Depth-first subdivision of a piece with several sign changes of f.
    #[cold]
    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    fn subdivide(
        &self,
        p: Vec2,
        coef: &[f64; 6],
        fd: &impl Fn(f64) -> (f64, f64),
        a: f64,
        b: f64,
        pa: Vec2,
        pb: Vec2,
        hint: &mut f64,
        st: &mut Search,
    ) {
        let mut stack: [(f64, f64, Vec2, Vec2, f64); 64] = [(0.0, 0.0, Vec2::ZERO, Vec2::ZERO, 0.0); 64];
        let mut sp = 0;
        // Split the initial piece.
        let split = |a: f64,
                     b: f64,
                     pa: Vec2,
                     pb: Vec2,
                     stack: &mut [(f64, f64, Vec2, Vec2, f64); 64],
                     sp: &mut usize,
                     st: &mut Search| {
            let m = f64::midpoint(a, b);
            let pm = self.eval(m);
            if b - a < 1e-12 || *sp + 2 > stack.len() {
                let g = (pm - p).length_squared();
                if g < st.best {
                    st.best = g;
                    st.best_t = m;
                    st.sb = g.sqrt();
                }
                return;
            }
            let ddm = self.d2(m).length();
            stack[*sp] = (m, b, pm, pb, ddm.max(self.d2(b).length()));
            stack[*sp + 1] = (a, m, pa, pm, ddm.max(self.d2(a).length()));
            *sp += 2;
        };
        split(a, b, pa, pb, &mut stack, &mut sp, st);
        while sp > 0 {
            sp -= 1;
            let (a, b, pa, pb, dd) = stack[sp];
            let h = b - a;
            if Self::piece_is_far(p, pa, pb, h, dd, st.sb) {
                continue;
            }
            let mut bern = bernstein5(*coef, a, h);
            if self.solve_piece(p, fd, &mut bern, a, b, pa, pb, hint, st) {
                split(a, b, pa, pb, &mut stack, &mut sp, st);
            }
        }
    }

    /// Appends the GPU form of this curve (layout documented in [`crate::gpu`]).
    #[cfg(feature = "gpu")]
    pub(crate) fn gpu_write(&self, out: &mut Vec<f32>) {
        let n = self.n;
        out.push(n as f32);
        for v in [self.p0, self.a, self.b, self.c] {
            out.push(v.x as f32);
            out.push(v.y as f32);
        }
        out.extend([self.k1, self.k2, self.k3, self.k4, self.k5].map(|k| k as f32));
        for i in 0..n {
            out.extend([self.ts[i] as f32, self.pts[i].x as f32, self.pts[i].y as f32]);
        }
        for iv in &self.iv[..n - 1] {
            out.extend([iv.ch.x as f32, iv.ch.y as f32, iv.inv_l2 as f32, iv.sag as f32]);
            out.extend(iv.bf.map(|v| v as f32));
            out.extend(iv.bx.map(|v| v as f32));
            out.extend(iv.by.map(|v| v as f32));
        }
    }

    /// [`Self::piece_is_far`] for a precomputed sample interval.
    #[inline(always)]
    fn interval_is_far(p: Vec2, pa: Vec2, iv: &CubicInterval, lim: f64) -> bool {
        let ap = p - pa;
        let u = (ap.dot(iv.ch) * iv.inv_l2).clamp(0.0, 1.0);
        let d2 = (ap - iv.ch * u).length_squared();
        let r = lim + iv.sag;
        d2 > r * r
    }

    #[inline]
    pub(crate) fn signed_distance(&self, p: Vec2) -> (SignedDistance, f64) {
        let mut hint = -1.0;
        self.signed_distance_hint(p, &mut hint)
    }

    /// Like [`Self::signed_distance`]; `hint` (the parameter found at a nearby point, or any
    /// value outside [0, 1]) is used as the Newton starting point when it falls into the
    /// interval being solved, and is updated.
    #[inline]
    pub(crate) fn signed_distance_hint(&self, p: Vec2, hint: &mut f64) -> (SignedDistance, f64) {
        let n = self.n;
        let last = n - 1;
        let mut best = f64::MAX;
        let mut best_t = 0.0;
        for i in 0..n {
            let g = (self.pts[i] - p).length_squared();
            if g < best {
                best = g;
                best_t = self.ts[i];
            }
        }
        // f(t) = (B(t)-p).B'(t) in power basis.
        let q = self.p0 - p;
        let c0 = q.dot(self.a);
        let c1 = 2.0 * q.dot(self.b) + self.k1;
        let c2 = 3.0 * q.dot(self.c) + self.k2;
        let coef = [c0, c1, c2, self.k3, self.k4, self.k5];
        let f = |t: f64| ((((coef[5] * t + coef[4]) * t + coef[3]) * t + coef[2]) * t + coef[1]) * t + coef[0];
        let fd = |t: f64| {
            let v = f(t);
            let dv = (((5.0 * coef[5] * t + 4.0 * coef[4]) * t + 3.0 * coef[3]) * t + 2.0 * coef[2]) * t + coef[1];
            (v, dv)
        };
        let mut st = Search {
            best,
            best_t,
            sb: best.sqrt(),
        };
        // Sample intervals in order (precomputed data); the rare intervals with several sign
        // changes are subdivided depth-first.
        for i in 0..last {
            let iv = &self.iv[i];
            let pa = self.pts[i];
            if Self::interval_is_far(p, pa, iv, st.sb) {
                continue;
            }
            let mut bern = [0.0f64; 6];
            for j in 1..5 {
                bern[j] = iv.bf[j] + q.x * iv.bx[j] + q.y * iv.by[j];
            }
            let (a, b) = (self.ts[i], self.ts[i + 1]);
            let pb = self.pts[i + 1];
            if self.solve_piece(p, &fd, &mut bern, a, b, pa, pb, hint, &mut st) {
                self.subdivide(p, &coef, &fd, a, b, pa, pb, hint, &mut st);
            }
        }
        let (best, best_t) = (st.best, st.best_t);
        if best_t <= 0.0 {
            endpoint_distance_pre(p, self.start, self.dir0, self.unit0, self.inv_len2_0, false, best)
        } else if best_t >= 1.0 {
            endpoint_distance_pre(p, self.end, self.dir1, self.unit1, self.inv_len2_1, true, best)
        } else {
            let q = self.eval(best_t) - p;
            let mut dir = self.d1(best_t);
            if dir.is_zero() {
                dir = self.d2(best_t);
                if dir.is_zero() {
                    dir = self.end - self.start;
                }
            }
            let s = non_zero_sign((-q).cross(dir));
            *hint = best_t;
            (SignedDistance::new(s * best.sqrt(), 0.0), best_t)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Brute-force nearest distance by dense sampling.
    fn brute(seg: &Segment, p: Vec2) -> f64 {
        let n = 200_000;
        let mut best = f64::MAX;
        for i in 0..=n {
            let q = seg.point(i as f64 / n as f64);
            best = best.min((q - p).length());
        }
        best
    }

    fn lcg(seed: &mut u64) -> f64 {
        *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((*seed >> 11) as f64) / ((1u64 << 53) as f64)
    }

    #[test]
    fn distances_match_brute_force() {
        let mut seed = 12345u64;
        let mut r = || lcg(&mut seed) * 4.0 - 2.0;
        for _ in 0..4000 {
            let pts: Vec<Vec2> = (0..4).map(|_| Vec2::new(r(), r())).collect();
            let segs = [
                Segment::Linear([pts[0], pts[1]]),
                Segment::Quadratic([pts[0], pts[1], pts[2]]),
                Segment::Cubic([pts[0], pts[1], pts[2], pts[3]]),
            ];
            let p = Vec2::new(r(), r());
            for seg in &segs {
                let (sd, _) = seg.signed_distance(p);
                let bf = brute(seg, p);
                assert!(
                    (sd.distance.abs() - bf).abs() < 2e-7 && sd.distance.abs() <= bf + 1e-12,
                    "{seg:?} p={p:?} got {} expected {bf}",
                    sd.distance.abs()
                );
            }
        }
    }

    #[test]
    fn quadratic_fast_matches_closed_form() {
        let mut seed = 777u64;
        let mut r = || lcg(&mut seed) * 4.0 - 2.0;
        for _ in 0..4000 {
            let pts = [Vec2::new(r(), r()), Vec2::new(r(), r()), Vec2::new(r(), r())];
            let q = QuadData::new(&pts);
            let p = Vec2::new(r(), r());
            let (a, _) = q.signed_distance_newton(&pts, p);
            let (b, _) = q.signed_distance(&pts, p);
            assert!(
                (a.distance - b.distance).abs() < 1e-12 * (1.0 + b.distance.abs()),
                "{pts:?} {p:?}: {a:?} vs {b:?}"
            );
            let mut hint = r() * 0.5 + 0.25;
            let (c, _) = q.signed_distance_warm(&pts, p, &mut hint);
            assert!(
                (c.distance - b.distance).abs() < 1e-12 * (1.0 + b.distance.abs()),
                "{pts:?} {p:?}: {c:?} vs {b:?}"
            );
        }
    }

    #[test]
    fn sign_convention() {
        // Right-hand side of the direction of travel is positive.
        let seg = Segment::Linear([Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0)]);
        assert!(seg.signed_distance(Vec2::new(0.5, -1.0)).0.distance > 0.0);
        assert!(seg.signed_distance(Vec2::new(0.5, 1.0)).0.distance < 0.0);
        let q = Segment::Quadratic([Vec2::new(0.0, 0.0), Vec2::new(0.5, 0.0), Vec2::new(1.0, 0.0)]);
        assert!(q.signed_distance(Vec2::new(0.5, -1.0)).0.distance > 0.0);
        let c = Segment::Cubic([
            Vec2::new(0.0, 0.0),
            Vec2::new(0.3, 0.0),
            Vec2::new(0.6, 0.0),
            Vec2::new(1.0, 0.0),
        ]);
        assert!(c.signed_distance(Vec2::new(0.5, -1.0)).0.distance > 0.0);
        assert!(c.signed_distance(Vec2::new(0.5, 1.0)).0.distance < 0.0);
    }

    #[test]
    fn bounds_contain_curve() {
        let mut seed = 99u64;
        let mut r = || lcg(&mut seed) * 4.0 - 2.0;
        for _ in 0..100 {
            let seg = Segment::Cubic([
                Vec2::new(r(), r()),
                Vec2::new(r(), r()),
                Vec2::new(r(), r()),
                Vec2::new(r(), r()),
            ]);
            let (lo, hi) = seg.bounds();
            for i in 0..=1000 {
                let p = seg.point(i as f64 / 1000.0);
                assert!(p.x >= lo.x - 1e-9 && p.y >= lo.y - 1e-9 && p.x <= hi.x + 1e-9 && p.y <= hi.y + 1e-9);
            }
        }
    }

    #[test]
    fn split_preserves_curve() {
        let seg = Segment::Cubic([
            Vec2::new(0.0, 0.0),
            Vec2::new(1.0, 2.0),
            Vec2::new(2.0, -1.0),
            Vec2::new(3.0, 1.0),
        ]);
        let [a, b, c] = seg.split_in_thirds();
        assert!((a.point(1.0) - seg.point(1.0 / 3.0)).length() < 1e-12);
        assert!((b.point(0.5) - seg.point(0.5)).length() < 1e-12);
        assert!((c.point(1.0) - seg.point(1.0)).length() < 1e-12);
    }

    #[test]
    fn cubic_zero_derivative_end() {
        // Start control point coincides with the start: f(0) = 0 exactly, and the nearest
        // point is inside the first sample interval.
        let pts = [
            Vec2::new(0.29, 0.047),
            Vec2::new(0.29, 0.047),
            Vec2::new(0.201, 0.047),
            Vec2::new(0.201, 0.06),
        ];
        let p = Vec2::new(0.27895, -0.1305);
        let (sd, t) = Segment::Cubic(pts).signed_distance(p);
        let seg = Segment::Cubic(pts);
        let brute = (0..=100_000)
            .map(|i| (seg.point(i as f64 / 100_000.0) - p).length())
            .fold(f64::MAX, f64::min);
        assert!(sd.distance.abs() <= brute + 1e-12, "{} vs {brute} (t={t})", sd.distance);
        assert!(t > 0.1 && t < 0.25, "{t}");
    }

    fn brute_cubic(pts: [Vec2; 4], p: Vec2) -> f64 {
        let seg = Segment::Cubic(pts);
        let g = |t: f64| (seg.point(t) - p).length_squared();
        let n = 4000;
        let (mut best, mut best_t) = (f64::MAX, 0.0);
        for i in 0..=n {
            let t = i as f64 / n as f64;
            if g(t) < best {
                (best, best_t) = (g(t), t);
            }
        }
        // Golden-section refinement around the best sample.
        let (mut a, mut b) = ((best_t - 1.0 / n as f64).max(0.0), (best_t + 1.0 / n as f64).min(1.0));
        for _ in 0..100 {
            let m1 = a + (b - a) * 0.381966;
            let m2 = a + (b - a) * 0.618034;
            if g(m1) < g(m2) {
                b = m2;
            } else {
                a = m1;
            }
        }
        g(f64::midpoint(a, b)).min(best).sqrt()
    }

    /// The exact cubic nearest-point kernel is never farther than brute force, on a grid of points
    /// around every cubic edge of the font's printable ASCII glyphs (CFF fonts have cubics).
    #[cfg(feature = "font")]
    #[test]
    #[ignore = "slow; run with --release -- --ignored"]
    fn cubic_kernel_matches_brute_force() {
        let path = std::env::var("MSDFR_TEST_FONT").unwrap_or_else(|_| {
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../examples/web-demo/assets/Inter-Regular.otf"
            )
            .into()
        });
        let data = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let font = crate::io::font::Font::from_bytes(&data, 0, crate::io::font::FontScaling::EmNormalized).unwrap();
        let (mut checked, mut bad) = (0u64, Vec::new());
        for c in 33u8..127 {
            let Ok(g) = font.glyph(c as char) else {
                continue;
            };
            let b = g.shape.bounds();
            if b.is_empty() {
                continue;
            }
            let pad = 0.05;
            for e in g.shape.contours.iter().flat_map(|c| &c.edges) {
                let Segment::Cubic(pts) = e.segment else {
                    continue;
                };
                let cubic = CubicData::new(&pts);
                let mut hint = -1.0;
                let steps = 16;
                for iy in 0..=steps {
                    for ix in 0..=steps {
                        let p = Vec2::new(
                            b.left - pad + (b.width() + 2.0 * pad) * ix as f64 / steps as f64,
                            b.bottom - pad + (b.height() + 2.0 * pad) * iy as f64 / steps as f64,
                        );
                        let d = cubic.signed_distance_hint(p, &mut hint).0.distance;
                        let bd = brute_cubic(pts, p);
                        checked += 1;
                        if d.abs() > bd + 1e-9 {
                            bad.push(format!("{:?} p={p:?}: kernel {} brute {bd}", c as char, d.abs()));
                        }
                    }
                }
            }
        }
        assert!(checked > 0, "the font has no cubic edges");
        let shown = bad.len().min(10);
        assert!(
            bad.is_empty(),
            "{} of {checked} queries worse than brute force:\n{}",
            bad.len(),
            bad[..shown].join("\n")
        );
    }
}
