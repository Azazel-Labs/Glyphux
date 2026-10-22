//! Contours and shapes.

use crate::edge::{Edge, Segment};
use crate::math::Vec2;

/// Fill rule used to decide which regions of a (possibly self-overlapping) shape are inside.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FillRule {
    /// Inside if the winding number is non-zero (default for fonts and SVG).
    #[default]
    NonZero,
    /// Inside if the winding number is odd.
    EvenOdd,
    /// Inside if the winding number is positive.
    Positive,
    /// Inside if the winding number is negative.
    Negative,
}

impl FillRule {
    #[inline]
    pub fn is_filled(self, winding: i32) -> bool {
        match self {
            FillRule::NonZero => winding != 0,
            FillRule::EvenOdd => winding & 1 != 0,
            FillRule::Positive => winding > 0,
            FillRule::Negative => winding < 0,
        }
    }
}

/// Axis-aligned bounding box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub left: f64,
    pub bottom: f64,
    pub right: f64,
    pub top: f64,
}

impl Bounds {
    pub const EMPTY: Bounds = Bounds {
        left: f64::MAX,
        bottom: f64::MAX,
        right: -f64::MAX,
        top: -f64::MAX,
    };

    pub fn is_empty(&self) -> bool {
        !(self.left <= self.right && self.bottom <= self.top)
    }
    pub fn include(&mut self, lo: Vec2, hi: Vec2) {
        self.left = self.left.min(lo.x);
        self.bottom = self.bottom.min(lo.y);
        self.right = self.right.max(hi.x);
        self.top = self.top.max(hi.y);
    }
    pub fn width(&self) -> f64 {
        self.right - self.left
    }
    pub fn height(&self) -> f64 {
        self.top - self.bottom
    }
}

/// A closed sequence of edges.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Contour {
    pub edges: Vec<Edge>,
}

impl Contour {
    pub fn new() -> Self {
        Contour { edges: Vec::new() }
    }

    pub fn add_edge(&mut self, edge: Edge) {
        self.edges.push(edge);
    }

    /// Signed area (positive = clockwise in a y-up coordinate system, which is the
    /// "filled" orientation under this crate's convention). Exact for Bézier segments.
    pub fn signed_area(&self) -> f64 {
        // Integral of x dy over the closed contour (Green's theorem), negated so that clockwise
        // contours are positive. Gauss-Legendre with 3 points is exact for the degree-5 integrand.
        const GL: [(f64, f64); 3] = [
            (0.1127016653792583, 0.2777777777777778),
            (0.5, 0.4444444444444444),
            (0.8872983346207417, 0.2777777777777778),
        ];
        let mut area = 0.0;
        for e in &self.edges {
            match e.segment {
                Segment::Linear([a, b]) => area += f64::midpoint(a.x, b.x) * (b.y - a.y),
                _ => {
                    for (t, w) in GL {
                        let p = e.segment.point(t);
                        let d = derivative(&e.segment, t);
                        area += w * p.x * d.y;
                    }
                }
            }
        }
        -area
    }

    /// +1 for clockwise (filled), -1 for counter-clockwise (hole), 0 for degenerate.
    pub fn winding(&self) -> i32 {
        let a = self.signed_area();
        if a > 0.0 {
            1
        } else if a < 0.0 {
            -1
        } else {
            0
        }
    }

    /// Reverses the direction of the contour.
    pub fn reverse(&mut self) {
        self.edges.reverse();
        for e in &mut self.edges {
            e.segment.reverse();
        }
    }

    pub fn bounds(&self) -> Bounds {
        let mut b = Bounds::EMPTY;
        for e in &self.edges {
            let (lo, hi) = e.segment.bounds();
            b.include(lo, hi);
        }
        b
    }
}

/// True derivative of a segment (no degenerate fallback).
fn derivative(seg: &Segment, t: f64) -> Vec2 {
    match *seg {
        Segment::Linear([a, b]) => b - a,
        Segment::Quadratic([a, b, c]) => 2.0 * crate::math::mix(b - a, c - b, t),
        Segment::Cubic([a, b, c, d]) => {
            3.0 * crate::math::mix(crate::math::mix(b - a, c - b, t), crate::math::mix(c - b, d - c, t), t)
        }
    }
}

/// A vector shape made of closed contours.
///
/// Coordinates are y-up by default; set `inverse_y_axis` for y-down input (e.g. SVG) so the
/// generated bitmap is still upright.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shape {
    pub contours: Vec<Contour>,
    pub inverse_y_axis: bool,
}

/// Error returned by [`Shape::validate`].
#[derive(Clone, Debug, PartialEq)]
pub enum ShapeError {
    /// A contour is not closed or consecutive edges don't connect.
    Disconnected { contour: usize, edge: usize },
    /// A coordinate is NaN or infinite.
    NonFinite { contour: usize, edge: usize },
}

impl core::fmt::Display for ShapeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ShapeError::Disconnected { contour, edge } => {
                write!(
                    f,
                    "contour {contour}: edge {edge} does not connect to the previous edge"
                )
            }
            ShapeError::NonFinite { contour, edge } => {
                write!(f, "contour {contour}: edge {edge} has a non-finite coordinate")
            }
        }
    }
}

impl std::error::Error for ShapeError {}

/// Dot product threshold below which two tangents are considered exactly opposite (a cusp).
const CUSP_DOT_EPSILON: f64 = 1e-6;

impl Shape {
    pub fn new() -> Self {
        Shape::default()
    }

    pub fn add_contour(&mut self, c: Contour) {
        self.contours.push(c);
    }

    pub fn edge_count(&self) -> usize {
        self.contours.iter().map(|c| c.edges.len()).sum()
    }

    /// Checks that all contours are closed and all coordinates are finite.
    pub fn validate(&self) -> Result<(), ShapeError> {
        for (ci, c) in self.contours.iter().enumerate() {
            let Some(last) = c.edges.last() else { continue };
            let mut corner = last.end();
            for (ei, e) in c.edges.iter().enumerate() {
                if e.segment.control_points().iter().any(|p| !p.is_finite()) {
                    return Err(ShapeError::NonFinite { contour: ci, edge: ei });
                }
                if e.start() != corner {
                    return Err(ShapeError::Disconnected { contour: ci, edge: ei });
                }
                corner = e.end();
            }
        }
        Ok(())
    }

    /// Bounding box of the shape.
    pub fn bounds(&self) -> Bounds {
        let mut b = Bounds::EMPTY;
        for c in &self.contours {
            for e in &c.edges {
                let (lo, hi) = e.segment.bounds();
                b.include(lo, hi);
            }
        }
        b
    }

    /// Expands `bounds` to include the miters of convex corners when the outline is offset
    /// outward by `border` (shape units), with miters limited to `miter_limit * border`.
    /// Sharp corners of an MSDF extend along their miter, so atlas boxes need this room.
    /// `polarity` > 0 considers corners convex on the filled side's outside (right-turning
    /// for this crate's orientation), < 0 the opposite.
    pub fn bound_miters(&self, bounds: &mut Bounds, border: f64, miter_limit: f64, polarity: i32) {
        if border <= 0.0 || miter_limit <= 0.0 {
            return;
        }
        for c in &self.contours {
            let Some(last) = c.edges.last() else { continue };
            let mut prev_dir = last.direction(1.0).normalize_or_zero();
            for e in &c.edges {
                // Incoming direction and the reversed outgoing direction.
                let dir = -e.direction(0.0).normalize_or_zero();
                if (polarity as f64) * prev_dir.cross(dir) > 0.0 {
                    let q = 0.5 * (1.0 - prev_dir.dot(dir));
                    let len = if q > 0.0 {
                        (1.0 / q.sqrt()).min(miter_limit)
                    } else {
                        miter_limit
                    };
                    let m = e.start() + (prev_dir + dir).normalize_or_zero() * (border * len);
                    bounds.include(m, m);
                }
                prev_dir = e.direction(1.0).normalize_or_zero();
            }
        }
    }

    /// Prepares the shape for distance field generation:
    /// removes empty contours and zero-length edges, splits single-edge contours into three
    /// parts (so they can be given three colors), and slightly separates the tangents of
    /// edges that meet at a perfect 180-degree cusp so nearest-edge selection is unambiguous.
    pub fn normalize(&mut self) {
        for c in &mut self.contours {
            if c.edges.len() > 1 {
                let before = c.edges.len();
                c.edges.retain(|e| !e.segment.is_degenerate());
                if c.edges.is_empty() {
                    c.edges.clear();
                } else if c.edges.len() != before {
                    // Re-close gaps left by removed (zero-length) edges: endpoints already coincide.
                }
            }
        }
        self.contours
            .retain(|c| !c.edges.is_empty() && (c.edges.len() != 1 || !c.edges[0].segment.is_degenerate()));
        for c in &mut self.contours {
            if c.edges.len() == 1 {
                let e = c.edges[0];
                let parts = e.segment.split_in_thirds();
                c.edges = parts
                    .iter()
                    .map(|s| Edge {
                        segment: *s,
                        color: e.color,
                    })
                    .collect();
            } else {
                deconverge_cusps(c);
            }
        }
    }

    /// Reorients contours so that the filled region (according to `fill_rule`, evaluated on
    /// the shape as given) lies on the right-hand side of every edge - the orientation this
    /// crate's distance sign convention expects. Contours whose orientation cannot be decided
    /// (e.g. fully covered by other contours) are left unchanged.
    ///
    /// This makes the output independent of the contour direction conventions of the source
    /// (TrueType vs. PostScript outlines, SVG paths).
    pub fn orient_contours(&mut self, fill_rule: FillRule) {
        let bounds = self.bounds();
        if bounds.is_empty() {
            return;
        }
        let scale = bounds.width().max(bounds.height()).max(1e-300);
        let mut flips = vec![false; self.contours.len()];
        let mut crossings = Vec::new();
        // Vertical extent (control point hull) of every edge, so that the many winding number
        // queries below only intersect the edges that can cross their scanline.
        let spans: Vec<(f64, f64, &Segment)> = self
            .contours
            .iter()
            .flat_map(|c| c.edges.iter())
            .map(|e| {
                let pts = e.segment.control_points();
                let (mut lo, mut hi) = (pts[0].y, pts[0].y);
                for q in &pts[1..] {
                    lo = lo.min(q.y);
                    hi = hi.max(q.y);
                }
                (lo, hi, &e.segment)
            })
            .collect();
        let winding = |p: Vec2, crossings: &mut Vec<(f64, i32)>| -> i32 {
            crossings.clear();
            for &(lo, hi, seg) in &spans {
                // Half-open rule as in `scanline_crossings`: nothing at or above the top.
                if p.y >= lo && p.y < hi {
                    seg.scanline_crossings(p.y, crossings);
                }
            }
            -crossings.iter().filter(|(x, _)| *x > p.x).map(|(_, d)| *d).sum::<i32>()
        };
        // Signed vote of one sample: + if the fill is on the right of the edge at t.
        let sample = |e: &Edge, t: f64, len: f64, crossings: &mut Vec<(f64, i32)>| -> f64 {
            let p = e.point(t);
            let d = e.direction(t).normalize_or_zero();
            if d.is_zero() {
                return 0.0;
            }
            let right = d.perpendicular(false);
            // Offset small relative to both the shape and the edge.
            let eps = (1e-7 * scale).min(0.01 * len).max(1e-12 * scale);
            let wr = fill_rule.is_filled(winding(p + right * eps, crossings));
            let wl = fill_rule.is_filled(winding(p - right * eps, crossings));
            if wr == wl {
                0.0
            } else if wr {
                len
            } else {
                -len
            }
        };
        let mut order: Vec<(f64, usize)> = Vec::new();
        for (ci, c) in self.contours.iter().enumerate() {
            // Fast path: the midpoints of the (up to) four longest edges. When they all
            // agree, the contour's orientation is settled; anything else falls back to the
            // full vote below.
            order.clear();
            order.extend(c.edges.iter().enumerate().map(|(i, e)| (e.segment.length(), i)));
            order.sort_by(|a, b| b.0.total_cmp(&a.0));
            let (mut pos, mut neg) = (0usize, 0usize);
            let mut vote = 0.0;
            for &(len, i) in order.iter().take(4) {
                if len <= 0.0 {
                    continue;
                }
                let v = sample(&c.edges[i], 0.5, len, &mut crossings);
                pos += (v > 0.0) as usize;
                neg += (v < 0.0) as usize;
                vote += v;
            }
            if pos + neg > 0 && (pos == 0 || neg == 0) {
                flips[ci] = vote < 0.0;
                continue;
            }
            // Vote with samples taken from every edge, weighted by their length.
            let mut vote = 0.0;
            for e in &c.edges {
                let len = e.segment.length();
                if len <= 0.0 {
                    continue;
                }
                for &t in &[0.5, 0.25, 0.75] {
                    vote += sample(e, t, len, &mut crossings);
                }
            }
            flips[ci] = vote < 0.0;
        }
        drop(spans);
        for (c, f) in self.contours.iter_mut().zip(flips) {
            if f {
                c.reverse();
            }
        }
    }

    /// Winding number of the shape around `p` (positive inside clockwise contours).
    pub fn winding_number(&self, p: Vec2) -> i32 {
        let mut v = Vec::new();
        self.winding_number_with(p, &mut v)
    }

    fn winding_number_with(&self, p: Vec2, crossings: &mut Vec<(f64, i32)>) -> i32 {
        crossings.clear();
        for c in &self.contours {
            for e in &c.edges {
                e.segment.scanline_crossings(p.y, crossings);
            }
        }
        // Clockwise contours (filled) cross downward on the right of an interior point.
        -crossings.iter().filter(|(x, _)| *x > p.x).map(|(_, d)| *d).sum::<i32>()
    }

    /// Whether the point is inside the shape according to the fill rule.
    pub fn contains(&self, p: Vec2, fill_rule: FillRule) -> bool {
        fill_rule.is_filled(self.winding_number(p))
    }

    /// Collects the scanline intersections at height `y`, sorted by x.
    /// Each entry is `(x, dir)`; the winding number at x is `-sum(dir for entries with x' > x)`.
    pub fn scanline(&self, y: f64, out: &mut Vec<(f64, i32)>) {
        out.clear();
        for c in &self.contours {
            for e in &c.edges {
                e.segment.scanline_crossings(y, out);
            }
        }
        out.sort_by(|a, b| a.0.total_cmp(&b.0));
    }
}

/// At a perfect cusp (consecutive tangents pointing in opposite directions), rotates the
/// tangents of the adjacent curved edges by a tiny angle towards the side each curve actually
/// bends to, so the corner has a well-defined bisector. Straight edges are left untouched.
fn deconverge_cusps(c: &mut Contour) {
    let n = c.edges.len();
    for i in 0..n {
        let prev_i = (i + n - 1) % n;
        let prev_dir = c.edges[prev_i].direction(1.0).normalize_or_zero();
        let cur_dir = c.edges[i].direction(0.0).normalize_or_zero();
        if prev_dir.is_zero() || cur_dir.is_zero() || prev_dir.dot(cur_dir) >= CUSP_DOT_EPSILON - 1.0 {
            continue;
        }
        let corner = c.edges[i].start();
        // Axis pointing away from the cusp along the shared tangent line.
        let axis = (cur_dir - prev_dir).normalize_or_zero();
        let normal = axis.perpendicular(true);
        // Which side of the tangent line each curve lies on.
        let side = |seg: &Segment, at_end: bool| -> f64 {
            let mut s = 0.0;
            for k in 1..=4 {
                let t = k as f64 / 8.0;
                let q = seg.point(if at_end { 1.0 - t } else { t });
                s = (q - corner).dot(normal);
                if s.abs() > 1e-12 {
                    break;
                }
            }
            s
        };
        let sp = side(&c.edges[prev_i].segment, true);
        let sc = side(&c.edges[i].segment, false);
        // Angle offset: well above the cusp threshold, still visually negligible.
        let angle = 2.0 * (2.0 * CUSP_DOT_EPSILON).sqrt();
        let (ap, ac) = if sp != 0.0 && sc != 0.0 && sp.signum() == sc.signum() {
            // Both curves leave on the same side: rotate the one closer to the axis less.
            if sp.abs() < sc.abs() {
                (angle, 2.0 * angle)
            } else {
                (2.0 * angle, angle)
            }
        } else {
            (angle, angle)
        };
        rotate_end_tangent(&mut c.edges[prev_i], true, ap, sp, corner, normal);
        rotate_end_tangent(&mut c.edges[i], false, ac, sc, corner, normal);
    }
}

/// Rotates the tangent of a curved edge at one endpoint by `angle` radians, moving the
/// adjacent control point towards the side of the tangent line (`side` relative to `normal`)
/// the curve lies on. Quadratics are elevated to cubics so the other end is unaffected.
fn rotate_end_tangent(edge: &mut Edge, at_end: bool, angle: f64, side: f64, corner: Vec2, normal: Vec2) {
    if side == 0.0 || angle == 0.0 || matches!(edge.segment, Segment::Linear(_)) {
        return;
    }
    if matches!(edge.segment, Segment::Quadratic(_)) {
        edge.segment = edge.segment.to_cubic();
    }
    if let Segment::Cubic(ref mut p) = edge.segment {
        let idx = if at_end { 2 } else { 1 };
        let v = p[idx] - corner;
        if v.is_zero() {
            return;
        }
        // A counter-clockwise rotation moves v along perpendicular(ccw); pick the sign that moves
        // it towards the curve's side.
        let along = v.perpendicular(true).dot(normal);
        if along == 0.0 {
            return;
        }
        let a = angle * side.signum() * along.signum();
        let (s, cs) = a.sin_cos();
        p[idx] = corner + Vec2::new(v.x * cs - v.y * s, v.x * s + v.y * cs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(cw: bool) -> Contour {
        let pts = [
            Vec2::new(0.0, 0.0),
            Vec2::new(0.0, 1.0),
            Vec2::new(1.0, 1.0),
            Vec2::new(1.0, 0.0),
        ];
        let mut c = Contour::new();
        for i in 0..4 {
            c.add_edge(Edge::line(pts[i], pts[(i + 1) % 4]));
        }
        if !cw {
            c.reverse();
        }
        c
    }

    #[test]
    fn winding_and_area() {
        let c = square(true);
        assert!((c.signed_area() - 1.0).abs() < 1e-12);
        assert_eq!(c.winding(), 1);
        let c = square(false);
        assert_eq!(c.winding(), -1);
        let mut s = Shape::new();
        s.add_contour(square(true));
        assert_eq!(s.winding_number(Vec2::new(0.5, 0.5)), 1);
        assert_eq!(s.winding_number(Vec2::new(1.5, 0.5)), 0);
        // On a vertex row
        assert_eq!(s.winding_number(Vec2::new(0.5, 0.0)), 1);
    }

    #[test]
    fn curved_area_exact() {
        // Circle-ish via quadratic: compare signed area with polygon approximation.
        let mut c = Contour::new();
        c.add_edge(Edge::quadratic(
            Vec2::new(0.0, 0.0),
            Vec2::new(0.5, 1.0),
            Vec2::new(1.0, 0.0),
        ));
        c.add_edge(Edge::line(Vec2::new(1.0, 0.0), Vec2::new(0.0, 0.0)));
        // Area under parabola y = 2t(1-t)... = 2/3 * base * height(0.5) = 1/3
        assert!((c.signed_area().abs() - 1.0 / 3.0).abs() < 1e-12);
    }

    #[test]
    fn miters_extend_sharp_corners() {
        // Thin clockwise spike pointing up: apex at (0, 10).
        let mut c = Contour::new();
        let (a, b, t) = (Vec2::new(-1.0, 0.0), Vec2::new(1.0, 0.0), Vec2::new(0.0, 10.0));
        c.add_edge(Edge::line(a, t));
        c.add_edge(Edge::line(t, b));
        c.add_edge(Edge::line(b, a));
        let mut s = Shape::new();
        s.add_contour(c);
        assert_eq!(s.contours[0].winding(), 1);
        let mut bounds = s.bounds();
        s.bound_miters(&mut bounds, 0.5, 4.0, 1);
        // The apex miter reaches 4 * 0.5 above the tip; the blunter base corners reach about
        // one border below.
        assert!((bounds.top - 12.0).abs() < 1e-9, "{bounds:?}");
        assert!(bounds.bottom > -0.6 && bounds.bottom < -0.4, "{bounds:?}");
    }

    #[test]
    fn orient_fixes_reversed_contours() {
        let mut s = Shape::new();
        s.add_contour(square(false)); // wrong orientation for an outer contour
        let mut hole = Contour::new();
        let pts = [
            Vec2::new(0.25, 0.25),
            Vec2::new(0.25, 0.75),
            Vec2::new(0.75, 0.75),
            Vec2::new(0.75, 0.25),
        ];
        for i in 0..4 {
            hole.add_edge(Edge::line(pts[i], pts[(i + 1) % 4]));
        }
        s.add_contour(hole); // clockwise too; should become a hole under even-odd
        s.orient_contours(FillRule::EvenOdd);
        assert_eq!(s.contours[0].winding(), 1);
        assert_eq!(s.contours[1].winding(), -1);
    }
}
