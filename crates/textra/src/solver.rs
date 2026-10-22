//! Polynomial equation solvers (real roots only).

/// Up to `N` real roots.
#[derive(Clone, Copy, Debug)]
pub struct Roots<const N: usize> {
    pub roots: [f64; N],
    pub count: usize,
}

impl<const N: usize> Roots<N> {
    #[inline(always)]
    fn none() -> Self {
        Roots {
            roots: [0.0; N],
            count: 0,
        }
    }
    #[inline(always)]
    fn push(&mut self, r: f64) {
        if self.count < N {
            self.roots[self.count] = r;
            self.count += 1;
        }
    }
    #[inline(always)]
    pub fn as_slice(&self) -> &[f64] {
        &self.roots[..self.count]
    }

    #[inline(always)]
    fn as_mut_slice(&mut self) -> &mut [f64] {
        &mut self.roots[..self.count]
    }
}

impl From<Roots<2>> for Roots<3> {
    #[inline(always)]
    fn from(r: Roots<2>) -> Self {
        Roots {
            roots: [r.roots[0], r.roots[1], 0.0],
            count: r.count,
        }
    }
}

/// Solves `a x^2 + b x + c = 0`. Degenerate cases (all-zero coefficients) yield no roots.
#[inline]
pub fn solve_quadratic(a: f64, b: f64, c: f64) -> Roots<2> {
    let mut out = Roots::none();
    // Treat as linear if the quadratic term is negligible compared to the linear term.
    if a == 0.0 || b.abs() > 1e12 * a.abs() {
        if b == 0.0 {
            return out;
        }
        out.push(-c / b);
        return out;
    }
    let disc = b * b - 4.0 * a * c;
    if disc > 0.0 {
        let s = disc.sqrt();
        // Numerically stable form.
        let q = -0.5 * (b + if b >= 0.0 { s } else { -s });
        let r1 = q / a;
        let r2 = if q == 0.0 { -r1 } else { c / q };
        out.push(r1);
        out.push(r2);
    } else if disc == 0.0 {
        out.push(-0.5 * b / a);
    }
    out
}

/// Cube root of a non-negative number: bit-level initial estimate refined by two Halley
/// steps (accurate to a few ulps; several times faster than the soft-float `cbrt`).
#[inline(always)]
pub fn fast_cbrt(x: f64) -> f64 {
    if !(x > 0.0) || !x.is_finite() || x < 1e-290 {
        return x.cbrt();
    }
    // Divide the exponent by three via the integer representation.
    let bits = x.to_bits();
    let mut y = f64::from_bits(bits / 3 + 0x2A9F_7893_782D_A1CE);
    for _ in 0..2 {
        let y3 = y * y * y;
        y *= (y3 + 2.0 * x) / (2.0 * y3 + x);
    }
    // Final Newton step to polish the last bits.
    y - (y * y * y - x) / (3.0 * y * y)
}

/// Roots of a cubic `a x^3 + b x^2 + c x + d`, given normalized as `x^3 + a2 x^2 + a1 x + a0`
/// (`orig` holds the original coefficients `[a, b, c, d]`). Only roots near `[0, 1]` are
/// polished with a Newton step, and the three-root case uses a single `sin_cos` (angle
/// addition) instead of three cosines: this serves nearest-point queries on quadratic
/// curves, where only roots in `(0, 1)` matter.
#[inline]
pub fn solve_cubic_unit_normed(a2c: f64, a1: f64, a0: f64, orig: [f64; 4]) -> Roots<3> {
    let [a, b, c, d] = orig;
    let mut out = Roots::none();
    let sq = a2c * a2c;
    let q = (sq - 3.0 * a1) * (1.0 / 9.0);
    let r = (a2c * (2.0 * sq - 9.0 * a1) + 27.0 * a0) * (1.0 / 54.0);
    let r2 = r * r;
    let q3 = q * q * q;
    let a3 = a2c * (1.0 / 3.0);
    if r2 < q3 {
        let sqrt_q = q.sqrt();
        let th = (r / (q * sqrt_q)).clamp(-1.0, 1.0).acos() * (1.0 / 3.0);
        let (s, cs) = th.sin_cos();
        let m = -2.0 * sqrt_q;
        const H: f64 = 0.866_025_403_784_438_6; // sqrt(3)/2
        out.push(m * cs - a3);
        out.push(m * (-0.5 * cs - H * s) - a3);
        out.push(m * (-0.5 * cs + H * s) - a3);
    } else {
        let mut u = -fast_cbrt(r.abs() + (r2 - q3).sqrt());
        if r < 0.0 {
            u = -u;
        }
        let v = if u == 0.0 { 0.0 } else { q / u };
        out.push((u + v) - a3);
        if u == v || (u - v).abs() <= 1e-9 * (u.abs() + v.abs()) {
            out.push(-0.5 * (u + v) - a3);
        }
    }
    for x in out.as_mut_slice() {
        if *x > -0.25 && *x < 1.25 {
            let t = *x;
            let f = ((a * t + b) * t + c) * t + d;
            let df = (3.0 * a * t + 2.0 * b) * t + c;
            if df != 0.0 {
                let nt = t - f / df;
                let nf = ((a * nt + b) * nt + c) * nt + d;
                if nf.abs() <= f.abs() {
                    *x = nt;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check_cubic(a: f64, b: f64, c: f64, d: f64, expected: &[f64]) {
        let r = solve_cubic_unit_normed(b / a, c / a, d / a, [a, b, c, d]);
        for &e in expected {
            assert!(
                r.as_slice().iter().any(|&x| (x - e).abs() < 1e-9),
                "missing root {e} in {:?} for {a} {b} {c} {d}",
                r.as_slice()
            );
        }
    }

    #[test]
    fn cubic_roots() {
        // (x-1)(x-2)(x-3) = x^3 - 6x^2 + 11x - 6
        check_cubic(1.0, -6.0, 11.0, -6.0, &[1.0, 2.0, 3.0]);
        // 2(x-0.5)^2 (x+1) = 2x^3 - x^2 ... expand: (x^2 - x + .25)(2x+2) = 2x^3 + 2x^2 - 2x^2 - 2x + .5x + .5
        check_cubic(2.0, 0.0, -1.5, 0.5, &[0.5, -1.0]);
        // one real root: x^3 + x + 1
        check_cubic(1.0, 0.0, 1.0, 1.0, &[-0.6823278038280193]);
    }

    #[test]
    fn cbrt_accuracy() {
        for &x in &[1e-200, 1e-12, 0.001, 0.5, 1.0, 2.0, 27.0, 12345.678, 1e100, 1e300] {
            let r = fast_cbrt(x);
            assert!(((r - x.cbrt()) / x.cbrt()).abs() < 4e-16, "{x}: {r} vs {}", x.cbrt());
        }
    }

    #[test]
    fn quadratic_roots() {
        let r = solve_quadratic(1.0, -3.0, 2.0);
        assert_eq!(r.as_slice().len(), 2);
        let r = solve_quadratic(0.0, 2.0, -1.0);
        assert_eq!(r.as_slice(), &[0.5]);
        let r = solve_quadratic(1.0, 0.0, 1.0);
        assert_eq!(r.as_slice(), &[] as &[f64]);
    }
}
