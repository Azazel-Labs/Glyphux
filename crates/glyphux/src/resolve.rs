//! Removing overlaps: a shape's contours replaced by the outline of the region they fill.
//!
//! Variable fonts keep a glyph's strokes as separate contours that overlap (the two strokes
//! of an `x`), so parts of their edges lie inside the fill. The overlapping contour combiner
//! handles that well except next to the points where contours cross, where the distance it
//! picks comes from a buried edge: the field then has specks inside the shape. msdfgen
//! resolves the shape's geometry first for the same reason; [`Shape::resolve_overlaps`]
//! does that here.

use crate::edge::{Edge, EdgeColor, Segment};
use crate::math::Vec2;
use crate::shape::{Contour, FillRule, Shape};

/// Recursion limit of the intersection search (each level halves a curve).
const MAX_DEPTH: u32 = 48;

impl Shape {
    /// Replaces the contours with the outline of the region they fill under `fill_rule`:
    /// edges are split where contours cross, the pieces that don't separate filled from
    /// empty space are dropped, and the rest are linked into new contours, each with the
    /// fill on its right (this crate's orientation). Returns whether the shape changed.
    ///
    /// Shapes whose contours neither cross nor cover each other are left as they are, and so
    /// is any shape whose pieces don't link up into closed contours. Edge colors are reset:
    /// color the edges afterwards.
    pub fn resolve_overlaps(&mut self, fill_rule: FillRule) -> bool {
        let bounds = self.bounds();
        if bounds.is_empty() {
            return false;
        }
        let scale = bounds.width().max(bounds.height()).max(1e-300);
        // How flat a curve must be to stand for its chord, and how close points must be to
        // count as the same.
        let flat = 1e-7 * scale;
        let same = 1e-6 * scale;

        let segments: Vec<(usize, Segment)> = self
            .contours
            .iter()
            .enumerate()
            .flat_map(|(c, contour)| contour.edges.iter().map(move |e| (c, e.segment)))
            .collect();
        let boxes: Vec<(Vec2, Vec2)> = segments.iter().map(|(_, s)| s.bounds()).collect();
        let mut cuts: Vec<Vec<f64>> = vec![Vec::new(); segments.len()];
        let mut hits = Vec::new();
        for i in 0..segments.len() {
            for j in i + 1..segments.len() {
                if !overlaps(boxes[i], boxes[j], same) {
                    continue;
                }
                hits.clear();
                intersect(
                    &segments[i].1,
                    (0.0, 1.0),
                    &segments[j].1,
                    (0.0, 1.0),
                    flat,
                    0,
                    &mut hits,
                );
                for &(s, t) in &hits {
                    // A crossing at a segment's end needs no cut there (contours meet at
                    // their vertices, and consecutive edges at their shared one).
                    if !at_end(&segments[i].1, s, same) {
                        cuts[i].push(s);
                    }
                    if !at_end(&segments[j].1, t, same) {
                        cuts[j].push(t);
                    }
                }
            }
        }
        let crossed: Vec<bool> = {
            let mut crossed = vec![false; self.contours.len()];
            for (k, c) in cuts.iter().enumerate() {
                if !c.is_empty() {
                    crossed[segments[k].0] = true;
                }
            }
            crossed
        };

        let mut crossings = Vec::new();
        // Whether a piece separates filled from empty space; `Some(true)` if the fill is on
        // its left (it must be turned around), `None` if it doesn't separate them.
        let mut classify = |s: &Segment| -> Option<bool> {
            let len = s.length();
            let mut d = s.direction(0.5).normalize_or_zero();
            if d.is_zero() {
                d = (s.end() - s.start()).normalize_or_zero();
            }
            if d.is_zero() || len <= 0.0 {
                return None;
            }
            let p = s.point(0.5);
            let right = d.perpendicular(false);
            let eps = (1e-7 * scale).min(0.01 * len).max(1e-12 * scale);
            let filled_right = fill_rule.is_filled(self.winding_number_with(p + right * eps, &mut crossings));
            let filled_left = fill_rule.is_filled(self.winding_number_with(p - right * eps, &mut crossings));
            (filled_right != filled_left).then_some(filled_left)
        };

        let mut pieces: Vec<Segment> = Vec::new();
        let mut changed = false;
        let mut first = 0;
        for (c, contour) in self.contours.iter().enumerate() {
            let n = contour.edges.len();
            let range = first..first + n;
            first += n;
            if !crossed[c] {
                // Not crossing anything: the whole contour is on the outline or inside the
                // fill (covered by another contour). The midpoint of its longest edge says which.
                let longest = contour
                    .edges
                    .iter()
                    .max_by(|a, b| a.segment.length().total_cmp(&b.segment.length()));
                match longest.map(|e| classify(&e.segment)) {
                    Some(Some(flip)) => {
                        for e in &contour.edges {
                            let mut s = e.segment;
                            if flip {
                                s.reverse();
                            }
                            pieces.push(s);
                        }
                        changed |= flip;
                    }
                    _ => changed = true,
                }
                continue;
            }
            for k in range {
                let s = segments[k].1;
                let mut ts = std::mem::take(&mut cuts[k]);
                ts.sort_by(f64::total_cmp);
                ts.dedup_by(|a, b| (s.point(*a) - s.point(*b)).length() <= same);
                let mut from = 0.0;
                for &t in ts.iter().chain(std::iter::once(&1.0)) {
                    let piece = sub_segment(&s, from, t);
                    from = t;
                    match classify(&piece) {
                        Some(flip) => {
                            let mut piece = piece;
                            if flip {
                                piece.reverse();
                            }
                            pieces.push(piece);
                        }
                        None => changed = true,
                    }
                }
                changed |= !ts.is_empty();
            }
        }
        if !changed {
            return false;
        }
        dedup_pieces(&mut pieces, same);
        let Some(contours) = link(pieces, same) else {
            return false;
        };
        self.contours = contours;
        true
    }
}

fn overlaps(a: (Vec2, Vec2), b: (Vec2, Vec2), margin: f64) -> bool {
    a.0.x <= b.1.x + margin && b.0.x <= a.1.x + margin && a.0.y <= b.1.y + margin && b.0.y <= a.1.y + margin
}

/// Whether `s` at `t` is within `same` of one of its ends.
fn at_end(s: &Segment, t: f64, same: f64) -> bool {
    let p = s.point(t);
    (p - s.start()).length() <= same || (p - s.end()).length() <= same
}

/// Whether the control points of `s` lie within `flat` of its chord.
fn is_flat(s: &Segment, flat: f64) -> bool {
    let pts = s.control_points();
    let (a, b) = (pts[0], pts[pts.len() - 1]);
    let chord = b - a;
    let len = chord.length();
    pts[1..pts.len() - 1].iter().all(|&q| {
        let off = if len > 0.0 {
            chord.cross(q - a).abs() / len
        } else {
            (q - a).length()
        };
        off <= flat
    })
}

/// The crossings of `a` and `b` (whose parameters map to `ar` and `ba` of the whole
/// segments), as pairs of whole-segment parameters: both are halved until flat, then their
/// chords are intersected.
fn intersect(
    a: &Segment,
    ar: (f64, f64),
    b: &Segment,
    br: (f64, f64),
    flat: f64,
    depth: u32,
    out: &mut Vec<(f64, f64)>,
) {
    if !overlaps(a.bounds(), b.bounds(), flat) {
        return;
    }
    let (a_flat, b_flat) = (is_flat(a, flat), is_flat(b, flat));
    if (a_flat && b_flat) || depth >= MAX_DEPTH {
        if let Some((s, t)) = chords(a.start(), a.end(), b.start(), b.end()) {
            out.push((ar.0 + (ar.1 - ar.0) * s, br.0 + (br.1 - br.0) * t));
        }
        return;
    }
    let size = |s: &Segment| {
        let (lo, hi) = s.bounds();
        (hi - lo).length()
    };
    if !a_flat && (b_flat || size(a) >= size(b)) {
        let (a0, a1) = a.split(0.5);
        let mid = f64::midpoint(ar.0, ar.1);
        intersect(&a0, (ar.0, mid), b, br, flat, depth + 1, out);
        intersect(&a1, (mid, ar.1), b, br, flat, depth + 1, out);
    } else {
        let (b0, b1) = b.split(0.5);
        let mid = f64::midpoint(br.0, br.1);
        intersect(a, ar, &b0, (br.0, mid), flat, depth + 1, out);
        intersect(a, ar, &b1, (mid, br.1), flat, depth + 1, out);
    }
}

/// Where segments `a0 a1` and `b0 b1` cross: their parameters, both in 0 to 1 (ends
/// included). `None` if they don't, or are parallel.
fn chords(a0: Vec2, a1: Vec2, b0: Vec2, b1: Vec2) -> Option<(f64, f64)> {
    let (da, db) = (a1 - a0, b1 - b0);
    let den = da.cross(db);
    if den == 0.0 {
        return None;
    }
    let w = b0 - a0;
    let s = w.cross(db) / den;
    let t = w.cross(da) / den;
    const SLACK: f64 = 1e-9;
    ((-SLACK..=1.0 + SLACK).contains(&s) && (-SLACK..=1.0 + SLACK).contains(&t))
        .then(|| (s.clamp(0.0, 1.0), t.clamp(0.0, 1.0)))
}

/// The part of `s` from `t0` to `t1`.
fn sub_segment(s: &Segment, t0: f64, t1: f64) -> Segment {
    let right = if t0 > 0.0 { s.split(t0).1 } else { *s };
    if t1 < 1.0 {
        right.split((t1 - t0) / (1.0 - t0)).0
    } else {
        right
    }
}

/// Drops pieces that repeat an earlier one (contours drawn twice, or sharing an edge with
/// the same direction).
fn dedup_pieces(pieces: &mut Vec<Segment>, same: f64) {
    let close = |a: Vec2, b: Vec2| (a - b).length() <= same;
    let mut kept: Vec<Segment> = Vec::with_capacity(pieces.len());
    for p in pieces.drain(..) {
        let repeat = kept
            .iter()
            .any(|k| close(k.start(), p.start()) && close(k.end(), p.end()) && close(k.point(0.5), p.point(0.5)));
        if !repeat {
            kept.push(p);
        }
    }
    *pieces = kept;
}

/// Links pieces end to start into closed contours, snapping their ends together. `None` if
/// some piece can't be linked.
fn link(mut pieces: Vec<Segment>, same: f64) -> Option<Vec<Contour>> {
    let close = |a: Vec2, b: Vec2| (a - b).length() <= same;
    let mut used = vec![false; pieces.len()];
    let mut contours = Vec::new();
    while let Some(start) = used.iter().position(|u| !u) {
        used[start] = true;
        let begin = pieces[start].start();
        let mut chain = vec![pieces[start]];
        loop {
            let end = chain.last().expect("a piece").end();
            if close(end, begin) {
                break;
            }
            // Of the pieces starting here, the one that turns most to the right keeps a
            // contour that touches another one at a point from crossing into it.
            let incoming = chain.last().expect("a piece").direction(1.0).normalize_or_zero();
            let next = (0..pieces.len())
                .filter(|&k| !used[k] && close(pieces[k].start(), end))
                .max_by(|&x, &y| {
                    let turn = |k: usize| -> f64 {
                        let d = pieces[k].direction(0.0).normalize_or_zero();
                        let (cross, dot) = (incoming.cross(d), incoming.dot(d));
                        // Angle turned to the right: clockwise is a negative cross product.
                        (-cross).atan2(dot)
                    };
                    turn(x).total_cmp(&turn(y))
                })?;
            used[next] = true;
            let mut piece = pieces[next];
            piece.control_points_mut()[0] = end;
            pieces[next] = piece;
            chain.push(piece);
        }
        let last = chain.len() - 1;
        let n = chain[last].control_points().len();
        chain[last].control_points_mut()[n - 1] = begin;
        let mut contour = Contour::new();
        for segment in chain {
            contour.add_edge(Edge {
                segment,
                color: EdgeColor::WHITE,
            });
        }
        contours.push(contour);
    }
    Some(contours)
}
