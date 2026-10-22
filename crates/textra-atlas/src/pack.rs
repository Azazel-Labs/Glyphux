//! Rectangle packing (MaxRects, best short side fit) and atlas dimension selection.

/// An axis-aligned integer rectangle; `y` grows downward (image rows).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    fn contains(&self, o: &Rect) -> bool {
        o.x >= self.x && o.y >= self.y && o.x + o.w <= self.x + self.w && o.y + o.h <= self.y + self.h
    }
    fn intersects(&self, o: &Rect) -> bool {
        o.x < self.x + self.w && o.x + o.w > self.x && o.y < self.y + self.h && o.y + o.h > self.y
    }
}

/// MaxRects bin packer.
pub struct MaxRects {
    free: Vec<Rect>,
}

impl MaxRects {
    pub fn new(width: u32, height: u32) -> Self {
        MaxRects {
            free: vec![Rect {
                x: 0,
                y: 0,
                w: width,
                h: height,
            }],
        }
    }

    /// Places a `w` x `h` rectangle, returning its position, or `None` if it doesn't fit.
    pub fn insert(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        // Best short side fit, ties broken by long side.
        let mut best: Option<(u32, u32, Rect)> = None;
        for f in &self.free {
            if f.w >= w && f.h >= h {
                let lw = f.w - w;
                let lh = f.h - h;
                let short = lw.min(lh);
                let long = lw.max(lh);
                if best.is_none_or(|(s, l, _)| (short, long) < (s, l)) {
                    best = Some((short, long, Rect { x: f.x, y: f.y, w, h }));
                }
            }
        }
        let (_, _, placed) = best?;
        self.split(&placed);
        Some((placed.x, placed.y))
    }

    fn split(&mut self, used: &Rect) {
        let mut new_free = Vec::with_capacity(self.free.len() + 4);
        for f in self.free.drain(..) {
            if !f.intersects(used) {
                new_free.push(f);
                continue;
            }
            if used.x > f.x {
                new_free.push(Rect {
                    x: f.x,
                    y: f.y,
                    w: used.x - f.x,
                    h: f.h,
                });
            }
            if used.x + used.w < f.x + f.w {
                new_free.push(Rect {
                    x: used.x + used.w,
                    y: f.y,
                    w: f.x + f.w - (used.x + used.w),
                    h: f.h,
                });
            }
            if used.y > f.y {
                new_free.push(Rect {
                    x: f.x,
                    y: f.y,
                    w: f.w,
                    h: used.y - f.y,
                });
            }
            if used.y + used.h < f.y + f.h {
                new_free.push(Rect {
                    x: f.x,
                    y: used.y + used.h,
                    w: f.w,
                    h: f.y + f.h - (used.y + used.h),
                });
            }
        }
        // Remove free rectangles contained in others.
        new_free.sort_by_key(|r| std::cmp::Reverse(r.w as u64 * r.h as u64));
        let mut kept: Vec<Rect> = Vec::with_capacity(new_free.len());
        for r in new_free {
            if r.w > 0 && r.h > 0 && !kept.iter().any(|k| k.contains(&r)) {
                kept.push(r);
            }
        }
        self.free = kept;
    }
}

/// Skyline bottom-left packer: fast (linear in the skyline length per insertion) and
/// efficient for many similarly sized rectangles such as glyphs.
pub struct Skyline {
    width: u32,
    height: u32,
    /// Segments `(x, y, width)` of the skyline, left to right; `y` grows downward.
    nodes: Vec<(u32, u32, u32)>,
}

impl Skyline {
    pub fn new(width: u32, height: u32) -> Self {
        Skyline {
            width,
            height,
            nodes: vec![(0, 0, width)],
        }
    }

    /// Lowest y at which a rectangle of width `w` starting at node `i` fits, with the wasted
    /// area below it.
    fn fit(&self, i: usize, w: u32, h: u32) -> Option<(u32, u64)> {
        let x = self.nodes[i].0;
        if x + w > self.width {
            return None;
        }
        let mut y = 0;
        let mut remaining = w;
        let mut j = i;
        while remaining > 0 {
            let (_, ny, nw) = self.nodes[j];
            y = y.max(ny);
            if y + h > self.height {
                return None;
            }
            remaining = remaining.saturating_sub(nw);
            j += 1;
        }
        // Waste: area between the skyline and the rectangle's bottom.
        let mut waste = 0u64;
        let mut remaining = w;
        let mut j = i;
        while remaining > 0 {
            let (_, ny, nw) = self.nodes[j];
            let span = nw.min(remaining);
            waste += (y - ny) as u64 * span as u64;
            remaining -= span;
            j += 1;
        }
        Some((y, waste))
    }

    /// Places a `w` x `h` rectangle as low as possible (ties: least waste, then leftmost).
    pub fn insert(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        let mut best: Option<(u32, u64, usize)> = None;
        for i in 0..self.nodes.len() {
            if let Some((y, waste)) = self.fit(i, w, h) {
                if best.is_none_or(|(by, bw, _)| (y + h, waste) < (by + h, bw)) {
                    best = Some((y, waste, i));
                }
            }
        }
        let (y, _, i) = best?;
        let x = self.nodes[i].0;
        // New segment, then trim the segments it covers.
        self.nodes.insert(i, (x, y + h, w));
        let end = x + w;
        let j = i + 1;
        while j < self.nodes.len() {
            let (nx, ny, nw) = self.nodes[j];
            if nx >= end {
                break;
            }
            if nx + nw <= end {
                self.nodes.remove(j);
            } else {
                self.nodes[j] = (end, ny, nx + nw - end);
                break;
            }
        }
        // Merge neighbors at the same height.
        let mut k = 0;
        while k + 1 < self.nodes.len() {
            if self.nodes[k].1 == self.nodes[k + 1].1 {
                self.nodes[k].2 += self.nodes[k + 1].2;
                self.nodes.remove(k + 1);
            } else {
                k += 1;
            }
        }
        Some((x, y))
    }
}

/// Rectangle counts up to which the slower but tighter MaxRects packer is used.
const MAXRECTS_LIMIT: usize = 400;

/// Packs all `sizes` into a `width` x `height` area (largest first for better packing).
/// Returns positions in input order, or `None` if they don't fit.
pub fn pack(sizes: &[(u32, u32)], width: u32, height: u32) -> Option<Vec<(u32, u32)>> {
    let mut order: Vec<usize> = (0..sizes.len()).filter(|&i| sizes[i].0 > 0 && sizes[i].1 > 0).collect();
    let mut out = vec![(0, 0); sizes.len()];
    if order.len() <= MAXRECTS_LIMIT {
        order.sort_by_key(|&i| {
            let (w, h) = sizes[i];
            (std::cmp::Reverse(w.max(h)), std::cmp::Reverse(w.min(h)))
        });
        let mut packer = MaxRects::new(width, height);
        for i in order {
            out[i] = packer.insert(sizes[i].0, sizes[i].1)?;
        }
    } else {
        order.sort_by_key(|&i| (std::cmp::Reverse(sizes[i].1), std::cmp::Reverse(sizes[i].0)));
        let mut packer = Skyline::new(width, height);
        for i in order {
            out[i] = packer.insert(sizes[i].0, sizes[i].1)?;
        }
    }
    Some(out)
}

/// Constraint on the atlas dimensions when they are chosen automatically.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DimensionsConstraint {
    /// Square with power-of-two side.
    PowerOfTwoSquare,
    /// Power-of-two width and height.
    PowerOfTwoRectangle,
    /// Square with a side divisible by 4 (msdf-atlas-gen's default).
    #[default]
    MultipleOfFourSquare,
    /// Square with an even side.
    EvenSquare,
    /// Any square.
    Square,
    /// Any rectangle (smallest height for a near-square width).
    None,
}

fn round_up(v: u32, c: DimensionsConstraint) -> u32 {
    let v = v.max(1);
    match c {
        DimensionsConstraint::PowerOfTwoSquare | DimensionsConstraint::PowerOfTwoRectangle => v.next_power_of_two(),
        DimensionsConstraint::MultipleOfFourSquare => v.div_ceil(4) * 4,
        DimensionsConstraint::EvenSquare => v.div_ceil(2) * 2,
        DimensionsConstraint::Square | DimensionsConstraint::None => v,
    }
}

fn next_size(v: u32, c: DimensionsConstraint) -> u32 {
    match c {
        DimensionsConstraint::PowerOfTwoSquare | DimensionsConstraint::PowerOfTwoRectangle => v * 2,
        _ => round_up(v + 1, c),
    }
}

/// Chooses the smallest dimensions satisfying `constraint` that fit all `sizes`, and packs
/// them. Returns `(width, height, positions)`.
pub fn pack_auto(sizes: &[(u32, u32)], constraint: DimensionsConstraint) -> (u32, u32, Vec<(u32, u32)>) {
    let area: u64 = sizes.iter().map(|&(w, h)| w as u64 * h as u64).sum();
    let max_w = sizes.iter().map(|s| s.0).max().unwrap_or(1);
    let max_h = sizes.iter().map(|s| s.1).max().unwrap_or(1);
    if sizes.iter().all(|&(w, h)| w == 0 || h == 0) {
        let s = round_up(1, constraint);
        return (s, s, vec![(0, 0); sizes.len()]);
    }
    let lower = ((area as f64).sqrt().ceil() as u32).max(max_w).max(max_h);
    match constraint {
        DimensionsConstraint::PowerOfTwoRectangle => {
            // Candidate power-of-two rectangles by increasing area.
            let mut s = lower.next_power_of_two();
            loop {
                let mut cands = vec![(s, s)];
                if s / 2 >= max_h.max(1) {
                    cands.insert(0, (s, s / 2));
                }
                if s / 2 >= max_w.max(1) {
                    cands.insert(1.min(cands.len()), (s / 2, s));
                }
                for (w, h) in cands {
                    if (w as u64 * h as u64) < area {
                        continue;
                    }
                    if let Some(p) = pack(sizes, w, h) {
                        return (w, h, p);
                    }
                }
                s *= 2;
            }
        }
        DimensionsConstraint::None => {
            // Near-square width, then the tightest height.
            let mut w = lower;
            loop {
                if let Some(p) = pack(sizes, w, u32::MAX / 2) {
                    let used = sizes
                        .iter()
                        .zip(&p)
                        .map(|(&(_, sh), &(_, y))| y + sh)
                        .max()
                        .unwrap_or(1);
                    return (w, used, p);
                }
                w += 1;
            }
        }
        DimensionsConstraint::PowerOfTwoSquare => {
            let mut s = round_up(lower, constraint);
            loop {
                if let Some(p) = pack(sizes, s, s) {
                    return (s, s, p);
                }
                s = next_size(s, constraint);
            }
        }
        _ => {
            // Grow geometrically to a size that fits, then bisect down to the smallest one.
            let step = match constraint {
                DimensionsConstraint::MultipleOfFourSquare => 4,
                DimensionsConstraint::EvenSquare => 2,
                _ => 1,
            };
            let mut lo = round_up(lower, constraint); // may or may not fit
            if let Some(p) = pack(sizes, lo, lo) {
                return (lo, lo, p);
            }
            let mut hi = lo;
            let mut fit = loop {
                hi = round_up(hi + (hi / 16).max(step), constraint);
                if let Some(p) = pack(sizes, hi, hi) {
                    break p;
                }
                lo = hi;
            };
            while hi - lo > step {
                let mid = round_up(lo + (hi - lo) / 2, constraint);
                if mid >= hi || mid <= lo {
                    break;
                }
                match pack(sizes, mid, mid) {
                    Some(p) => {
                        hi = mid;
                        fit = p;
                    }
                    None => lo = mid,
                }
            }
            (hi, hi, fit)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(sizes: &[(u32, u32)], w: u32, h: u32, pos: &[(u32, u32)]) {
        let rects: Vec<Rect> = sizes
            .iter()
            .zip(pos)
            .map(|(&(rw, rh), &(x, y))| Rect { x, y, w: rw, h: rh })
            .collect();
        for (i, a) in rects.iter().enumerate() {
            assert!(a.x + a.w <= w && a.y + a.h <= h, "rect {i} out of bounds");
            for b in &rects[i + 1..] {
                assert!(!a.intersects(b), "overlap {a:?} {b:?}");
            }
        }
    }

    #[test]
    fn packs_without_overlap() {
        let mut seed = 7u64;
        let mut sizes = Vec::new();
        for _ in 0..2000 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            sizes.push((((seed >> 33) % 40 + 4) as u32, ((seed >> 45) % 40 + 4) as u32));
        }
        for c in [
            DimensionsConstraint::PowerOfTwoSquare,
            DimensionsConstraint::PowerOfTwoRectangle,
            DimensionsConstraint::MultipleOfFourSquare,
            DimensionsConstraint::Square,
            DimensionsConstraint::None,
        ] {
            let (w, h, p) = pack_auto(&sizes, c);
            check(&sizes, w, h, &p);
            // The MaxRects path too.
            let (w2, h2, p2) = pack_auto(&sizes[..300], c);
            check(&sizes[..300], w2, h2, &p2);
            let area: u64 = sizes.iter().map(|&(a, b)| a as u64 * b as u64).sum();
            let eff = area as f64 / (w as f64 * h as f64);
            let pot = matches!(
                c,
                DimensionsConstraint::PowerOfTwoSquare | DimensionsConstraint::PowerOfTwoRectangle
            );
            assert!(eff > if pot { 0.25 } else { 0.75 }, "{c:?}: {w}x{h} efficiency {eff}");
        }
    }
}
