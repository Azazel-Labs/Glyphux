//! Edge coloring heuristics.
//!
//! Every corner of the shape must be formed by two edges that share at most one color
//! channel - this is what lets the median of the three channels reproduce sharp corners.

use crate::edge::{Edge, EdgeColor};
use crate::math::Vec2;
use crate::shape::{Contour, Shape};

/// Edge coloring strategy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ColoringStrategy {
    /// Switches color at every corner (msdfgen's default).
    Simple,
    /// Like `Simple`, but short splines between corners (typical of ink traps) don't consume
    /// a color switch, which keeps neighboring long splines distinct. Fewer artifacts on
    /// real fonts; the default here.
    #[default]
    InkTrap,
    /// Assigns colors so that edges of the same color are as far apart as possible,
    /// minimizing interference between nearby features (slower, best quality for complex glyphs).
    Distance,
}

/// Default corner angle threshold (radians); turns sharper than this are corners.
pub const DEFAULT_ANGLE_THRESHOLD: f64 = 3.0;

const PALETTE: [EdgeColor; 3] = [EdgeColor::CYAN, EdgeColor::MAGENTA, EdgeColor::YELLOW];

/// Color choice source. Colors cycle cyan -> magenta -> yellow, which keeps edges of the same
/// color as far apart as possible along a contour; the bits of `seed` (consumed one per
/// switch) occasionally reverse the direction to give different but equally valid colorings.
/// Seed 0 always cycles in the same direction.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let v = self.0 & 1;
        self.0 >>= 1;
        v
    }
    fn first(&mut self) -> EdgeColor {
        let i = (self.0 % 3) as usize;
        self.0 /= 3;
        PALETTE[i]
    }
    /// A different palette color than `cur`. If `banned` is a palette color distinct from
    /// `cur`, the result also differs from `banned` (i.e. it is the remaining third color).
    fn switch(&mut self, cur: EdgeColor, banned: Option<EdgeColor>) -> EdgeColor {
        if let Some(b) = banned {
            if b != cur && PALETTE.contains(&b) && PALETTE.contains(&cur) {
                return PALETTE.iter().copied().find(|c| *c != cur && *c != b).unwrap();
            }
        }
        let others: [EdgeColor; 2] = match cur {
            EdgeColor::CYAN => [EdgeColor::MAGENTA, EdgeColor::YELLOW],
            EdgeColor::MAGENTA => [EdgeColor::YELLOW, EdgeColor::CYAN],
            EdgeColor::YELLOW => [EdgeColor::CYAN, EdgeColor::MAGENTA],
            _ => return self.first(),
        };
        others[self.next() as usize]
    }
}

#[inline]
fn is_corner(a: Vec2, b: Vec2, cross_threshold: f64) -> bool {
    a.dot(b) <= 0.0 || a.cross(b).abs() > cross_threshold
}

/// Indices of edges that start at a corner.
fn find_corners(c: &Contour, cross_threshold: f64) -> Vec<usize> {
    let mut corners = Vec::new();
    let Some(last) = c.edges.last() else {
        return corners;
    };
    let mut prev = last.direction(1.0);
    for (i, e) in c.edges.iter().enumerate() {
        if is_corner(
            prev.normalize_or_zero(),
            e.direction(0.0).normalize_or_zero(),
            cross_threshold,
        ) {
            corners.push(i);
        }
        prev = e.direction(1.0);
    }
    corners
}

/// Splits position `i` of `n` into thirds: -1 (start), 0 (middle), +1 (end); balanced.
#[inline]
fn trichotomy(i: usize, n: usize) -> i32 {
    ((3 * i + 1) / n) as i32 - 1
}

/// Colors a contour with exactly one corner ("teardrop"): the edges after the corner, in the
/// middle, and before it get three different colors, splitting edges when there are too few.
fn color_teardrop(c: &mut Contour, corner: usize, rng: &mut Rng, color: &mut EdgeColor) {
    *color = rng.switch(*color, None);
    let c0 = *color;
    *color = rng.switch(*color, None);
    let c2 = *color;
    let colors = [c0, EdgeColor::WHITE, c2];
    let m = c.edges.len();
    if m < 3 {
        // Rotate so the corner edge comes first, then split everything in thirds.
        let mut parts = Vec::with_capacity(3 * m);
        for k in 0..m {
            let e = c.edges[(corner + k) % m];
            for s in e.segment.split_in_thirds() {
                parts.push(Edge {
                    segment: s,
                    color: e.color,
                });
            }
        }
        let n = parts.len();
        for (i, p) in parts.iter_mut().enumerate() {
            p.color = colors[(1 + trichotomy(i, n)) as usize];
        }
        c.edges = parts;
    } else {
        for i in 0..m {
            c.edges[(corner + i) % m].color = colors[(1 + trichotomy(i, m)) as usize];
        }
    }
}

/// Assigns a color to every edge so that each corner separates edges of different colors.
/// `angle_threshold` is the maximum turning angle (radians) still considered smooth
/// (msdfgen's default is 3.0). `seed` varies the color choices.
pub fn edge_coloring_simple(shape: &mut Shape, angle_threshold: f64, seed: u64) {
    let cross_threshold = angle_threshold.sin();
    let mut rng = Rng(seed);
    let mut color = rng.first();
    for c in &mut shape.contours {
        if c.edges.is_empty() {
            continue;
        }
        let corners = find_corners(c, cross_threshold);
        match corners.len() {
            0 => {
                color = rng.switch(color, None);
                for e in &mut c.edges {
                    e.color = color;
                }
            }
            1 => color_teardrop(c, corners[0], &mut rng, &mut color),
            n => {
                let m = c.edges.len();
                let start = corners[0];
                color = rng.switch(color, None);
                let initial = color;
                let mut spline = 0;
                for i in 0..m {
                    let idx = (start + i) % m;
                    if spline + 1 < n && corners[spline + 1] == idx {
                        spline += 1;
                        let banned = if spline == n - 1 { Some(initial) } else { None };
                        color = rng.switch(color, banned);
                    }
                    c.edges[idx].color = color;
                }
            }
        }
    }
}

/// Ink-trap aware coloring: corners that terminate a spline shorter than both neighboring
/// splines are "minor"; colors only switch at major corners and minor splines receive the
/// color complementary to their neighbors.
pub fn edge_coloring_ink_trap(shape: &mut Shape, angle_threshold: f64, seed: u64) {
    let cross_threshold = angle_threshold.sin();
    let mut rng = Rng(seed);
    let mut color = rng.first();
    for c in &mut shape.contours {
        if c.edges.is_empty() {
            continue;
        }
        let corners = find_corners(c, cross_threshold);
        match corners.len() {
            0 => {
                color = rng.switch(color, None);
                for e in &mut c.edges {
                    e.color = color;
                }
            }
            1 => color_teardrop(c, corners[0], &mut rng, &mut color),
            n => {
                let m = c.edges.len();
                // Length of the spline that starts at each corner.
                let mut spline_len = vec![0.0; n];
                for k in 0..n {
                    let (a, b) = (corners[k], corners[(k + 1) % n]);
                    let mut i = a;
                    loop {
                        spline_len[k] += c.edges[i].segment.length();
                        i = (i + 1) % m;
                        if i == b {
                            break;
                        }
                    }
                }
                // Spline k is minor if it is shorter than both neighbors.
                let mut minor = vec![false; n];
                if n > 3 {
                    for k in 0..n {
                        let prev = spline_len[(k + n - 1) % n];
                        let next = spline_len[(k + 1) % n];
                        if spline_len[k] < prev && spline_len[k] < next {
                            minor[k] = true;
                        }
                    }
                    // Two adjacent minors would leave a corner between equal colors; keep the
                    // shorter one minor.
                    for k in 0..n {
                        let j = (k + 1) % n;
                        if minor[k] && minor[j] {
                            if spline_len[k] <= spline_len[j] {
                                minor[j] = false;
                            } else {
                                minor[k] = false;
                            }
                        }
                    }
                }
                let majors: Vec<usize> = (0..n).filter(|&k| !minor[k]).collect();
                let mut spline_color = vec![EdgeColor::BLACK; n];
                let mut initial = None;
                for (idx, &k) in majors.iter().enumerate() {
                    let banned = if idx + 1 == majors.len() && majors.len() > 2 {
                        initial
                    } else {
                        None
                    };
                    color = rng.switch(color, banned);
                    if idx + 1 == majors.len() && majors.len() == 2 {
                        // Must differ from the first major spline (the only neighbor around).
                        if Some(color) == initial {
                            color = rng.switch(color, None);
                        }
                    }
                    spline_color[k] = color;
                    if initial.is_none() {
                        initial = Some(color);
                    }
                }
                for k in 0..n {
                    if minor[k] {
                        let prev = spline_color[(k + n - 1) % n];
                        let next = spline_color[(k + 1) % n];
                        // A color sharing at most one channel with each neighbor.
                        spline_color[k] = if prev == next {
                            rng.switch(prev, None)
                        } else {
                            EdgeColor((prev.0 & next.0) ^ 7)
                        };
                    }
                }
                for k in 0..n {
                    let (a, b) = (corners[k], corners[(k + 1) % n]);
                    let mut i = a;
                    loop {
                        c.edges[i].color = spline_color[k];
                        i = (i + 1) % m;
                        if i == b {
                            break;
                        }
                    }
                }
            }
        }
    }
}

/// Distance-aware coloring: splines are colored by a greedy graph coloring that maximizes
/// the distance between splines of equal color, respecting the corner constraints.
pub fn edge_coloring_by_distance(shape: &mut Shape, angle_threshold: f64, seed: u64) {
    let cross_threshold = angle_threshold.sin();
    let mut rng = Rng(seed);
    // Collect splines (runs of edges between corners) across all contours.
    struct Spline {
        contour: usize,
        edges: Vec<usize>,
        pts: Vec<Vec2>,
        prev: usize,
        next: usize,
    }
    let mut splines: Vec<Spline> = Vec::new();
    let mut fallback_contours = Vec::new();
    for (ci, c) in shape.contours.iter().enumerate() {
        if c.edges.is_empty() {
            continue;
        }
        let corners = find_corners(c, cross_threshold);
        if corners.len() < 2 {
            fallback_contours.push(ci);
            continue;
        }
        let m = c.edges.len();
        let first = splines.len();
        let n = corners.len();
        for k in 0..n {
            let (a, b) = (corners[k], corners[(k + 1) % n]);
            let mut edges = Vec::new();
            let mut pts = Vec::new();
            let mut i = a;
            loop {
                edges.push(i);
                for s in 0..=8 {
                    pts.push(c.edges[i].point(s as f64 / 8.0));
                }
                i = (i + 1) % m;
                if i == b {
                    break;
                }
            }
            let idx = first + k;
            splines.push(Spline {
                contour: ci,
                edges,
                pts,
                prev: first + (k + n - 1) % n,
                next: first + (k + 1) % n,
            });
            let _ = idx;
        }
    }
    // Pairwise spline distances (minimum over samples).
    let ns = splines.len();
    let mut dist = vec![f64::MAX; ns * ns];
    for i in 0..ns {
        for j in i + 1..ns {
            let mut d = f64::MAX;
            for a in &splines[i].pts {
                for b in &splines[j].pts {
                    d = d.min((*a - *b).length_squared());
                }
            }
            dist[i * ns + j] = d;
            dist[j * ns + i] = d;
        }
    }
    // Greedy: color splines in order of how constrained they are; choose the palette color
    // that is allowed (differs from already-colored corner neighbors) and maximizes the
    // minimum distance to splines of the same color.
    let mut assigned: Vec<Option<usize>> = vec![None; ns];
    let mut order: Vec<usize> = (0..ns).collect();
    // Start with splines that have many nearby splines.
    let crowd = |i: usize| -> f64 {
        let mut v: Vec<f64> = (0..ns).filter(|&j| j != i).map(|j| dist[i * ns + j]).collect();
        v.sort_by(f64::total_cmp);
        v.iter().take(4).sum::<f64>()
    };
    let crowd_vals: Vec<f64> = (0..ns).map(crowd).collect();
    order.sort_by(|&a, &b| crowd_vals[a].total_cmp(&crowd_vals[b]));
    for &i in &order {
        let mut best = None;
        let mut best_score = -1.0;
        let offset = (rng.next() % 3) as usize;
        for k in 0..3 {
            let col = (k + offset) % 3;
            let p = splines[i].prev;
            let n = splines[i].next;
            if assigned[p] == Some(col) && p != i || assigned[n] == Some(col) && n != i {
                continue;
            }
            // For a two-spline contour prev == next; also forbid equal colors in that case.
            let mut score = f64::MAX;
            for j in 0..ns {
                if j != i && assigned[j] == Some(col) {
                    score = score.min(dist[i * ns + j]);
                }
            }
            if score > best_score {
                best_score = score;
                best = Some(col);
            }
        }
        assigned[i] = Some(best.unwrap_or(0));
    }
    // Repair any remaining conflicts (possible for odd cycles when greedy order was unlucky).
    for _ in 0..4 {
        let mut changed = false;
        for i in 0..ns {
            let p = splines[i].prev;
            let n = splines[i].next;
            let ci = assigned[i].unwrap();
            if (p != i && assigned[p] == Some(ci)) || (n != i && assigned[n] == Some(ci)) {
                for col in 0..3 {
                    if Some(col) != assigned[p] && Some(col) != assigned[n] {
                        assigned[i] = Some(col);
                        changed = true;
                        break;
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
    for (i, s) in splines.iter().enumerate() {
        let col = PALETTE[assigned[i].unwrap()];
        for &e in &s.edges {
            shape.contours[s.contour].edges[e].color = col;
        }
    }
    // Contours with fewer than two corners use the simple rules.
    let mut color = rng.first();
    for ci in fallback_contours {
        let c = &mut shape.contours[ci];
        let corners = find_corners(c, cross_threshold);
        if corners.is_empty() {
            color = rng.switch(color, None);
            for e in &mut c.edges {
                e.color = color;
            }
        } else {
            color_teardrop(c, corners[0], &mut rng, &mut color);
        }
    }
}

/// Colors the shape with the given strategy.
pub fn edge_coloring(shape: &mut Shape, strategy: ColoringStrategy, angle_threshold: f64, seed: u64) {
    match strategy {
        ColoringStrategy::Simple => edge_coloring_simple(shape, angle_threshold, seed),
        ColoringStrategy::InkTrap => edge_coloring_ink_trap(shape, angle_threshold, seed),
        ColoringStrategy::Distance => edge_coloring_by_distance(shape, angle_threshold, seed),
    }
}

/// Checks that every corner of the shape (by the given threshold) joins edges sharing at
/// most one channel and that every edge has at least two channels.
pub fn coloring_is_valid(shape: &Shape, angle_threshold: f64) -> bool {
    let cross_threshold = angle_threshold.sin();
    for c in &shape.contours {
        if c.edges.is_empty() {
            continue;
        }
        let corners = find_corners(c, cross_threshold);
        let m = c.edges.len();
        for e in &c.edges {
            if e.color.channel_count() < 2 {
                return false;
            }
        }
        for &i in &corners {
            let prev = c.edges[(i + m - 1) % m].color;
            let cur = c.edges[i].color;
            if (prev & cur).channel_count() > 1 {
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge::Edge;

    fn polygon(pts: &[(f64, f64)]) -> Contour {
        let mut c = Contour::new();
        for i in 0..pts.len() {
            let a = Vec2::new(pts[i].0, pts[i].1);
            let b = Vec2::new(pts[(i + 1) % pts.len()].0, pts[(i + 1) % pts.len()].1);
            c.add_edge(Edge::line(a, b));
        }
        c
    }

    #[test]
    fn simple_coloring_valid() {
        for n in 2..12 {
            for seed in 0..20 {
                let pts: Vec<(f64, f64)> = (0..n.max(3))
                    .map(|i| {
                        let a = i as f64 / n.max(3) as f64 * std::f64::consts::TAU;
                        (a.cos(), -a.sin())
                    })
                    .collect();
                let mut s = Shape::new();
                s.add_contour(polygon(&pts));
                s.normalize();
                for strat in [
                    ColoringStrategy::Simple,
                    ColoringStrategy::InkTrap,
                    ColoringStrategy::Distance,
                ] {
                    let mut s2 = s.clone();
                    edge_coloring(&mut s2, strat, DEFAULT_ANGLE_THRESHOLD, seed);
                    assert!(
                        coloring_is_valid(&s2, DEFAULT_ANGLE_THRESHOLD),
                        "{strat:?} n={n} seed={seed}"
                    );
                }
            }
        }
    }

    #[test]
    fn switch_with_ban_gives_third() {
        let mut r = Rng(1);
        assert_eq!(r.switch(EdgeColor::CYAN, Some(EdgeColor::MAGENTA)), EdgeColor::YELLOW);
        assert_eq!(r.switch(EdgeColor::YELLOW, Some(EdgeColor::CYAN)), EdgeColor::MAGENTA);
    }
}
