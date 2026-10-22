//! Small 2D vector type and scalar helpers.

use core::ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Neg, Sub, SubAssign};

/// A 2D vector / point with `f64` components.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec2 {
    pub x: f64,
    pub y: f64,
}

/// Alias used where a value semantically represents a position.
pub type Point2 = Vec2;

impl Vec2 {
    pub const ZERO: Vec2 = Vec2 { x: 0.0, y: 0.0 };

    #[inline(always)]
    pub const fn new(x: f64, y: f64) -> Self {
        Vec2 { x, y }
    }

    #[inline(always)]
    pub const fn splat(v: f64) -> Self {
        Vec2 { x: v, y: v }
    }

    #[inline(always)]
    pub fn dot(self, o: Vec2) -> f64 {
        self.x * o.x + self.y * o.y
    }

    /// 2D cross product (z component of the 3D cross product).
    #[inline(always)]
    pub fn cross(self, o: Vec2) -> f64 {
        self.x * o.y - self.y * o.x
    }

    #[inline(always)]
    pub fn length_squared(self) -> f64 {
        self.dot(self)
    }

    #[inline(always)]
    pub fn length(self) -> f64 {
        self.length_squared().sqrt()
    }

    /// Unit vector in the same direction, or the zero vector if `self` is zero.
    #[inline(always)]
    pub fn normalize_or_zero(self) -> Vec2 {
        let len = self.length();
        if len > 0.0 {
            Vec2::new(self.x / len, self.y / len)
        } else {
            Vec2::ZERO
        }
    }

    /// Unit vector in the same direction, or `fallback` if `self` is zero.
    #[inline(always)]
    pub fn normalize_or(self, fallback: Vec2) -> Vec2 {
        let len = self.length();
        if len > 0.0 {
            Vec2::new(self.x / len, self.y / len)
        } else {
            fallback
        }
    }

    /// A vector perpendicular to this one. `ccw == true` rotates by +90 degrees.
    #[inline(always)]
    pub fn perpendicular(self, ccw: bool) -> Vec2 {
        if ccw {
            Vec2::new(-self.y, self.x)
        } else {
            Vec2::new(self.y, -self.x)
        }
    }

    #[inline(always)]
    pub fn min(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x.min(o.x), self.y.min(o.y))
    }

    #[inline(always)]
    pub fn max(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x.max(o.x), self.y.max(o.y))
    }

    #[inline(always)]
    pub fn is_zero(self) -> bool {
        self.x == 0.0 && self.y == 0.0
    }

    #[inline(always)]
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }

    /// Component-wise multiplication.
    #[inline(always)]
    pub fn mul_elem(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x * o.x, self.y * o.y)
    }

    /// Component-wise division.
    #[inline(always)]
    pub fn div_elem(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x / o.x, self.y / o.y)
    }
}

impl Add for Vec2 {
    type Output = Vec2;
    #[inline(always)]
    fn add(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x + o.x, self.y + o.y)
    }
}
impl Sub for Vec2 {
    type Output = Vec2;
    #[inline(always)]
    fn sub(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x - o.x, self.y - o.y)
    }
}
impl Neg for Vec2 {
    type Output = Vec2;
    #[inline(always)]
    fn neg(self) -> Vec2 {
        Vec2::new(-self.x, -self.y)
    }
}
impl Mul<f64> for Vec2 {
    type Output = Vec2;
    #[inline(always)]
    fn mul(self, s: f64) -> Vec2 {
        Vec2::new(self.x * s, self.y * s)
    }
}
impl Mul<Vec2> for f64 {
    type Output = Vec2;
    #[inline(always)]
    fn mul(self, v: Vec2) -> Vec2 {
        Vec2::new(self * v.x, self * v.y)
    }
}
impl Div<f64> for Vec2 {
    type Output = Vec2;
    #[inline(always)]
    fn div(self, s: f64) -> Vec2 {
        Vec2::new(self.x / s, self.y / s)
    }
}
impl AddAssign for Vec2 {
    #[inline(always)]
    fn add_assign(&mut self, o: Vec2) {
        self.x += o.x;
        self.y += o.y;
    }
}
impl SubAssign for Vec2 {
    #[inline(always)]
    fn sub_assign(&mut self, o: Vec2) {
        self.x -= o.x;
        self.y -= o.y;
    }
}
impl MulAssign<f64> for Vec2 {
    #[inline(always)]
    fn mul_assign(&mut self, s: f64) {
        self.x *= s;
        self.y *= s;
    }
}
impl DivAssign<f64> for Vec2 {
    #[inline(always)]
    fn div_assign(&mut self, s: f64) {
        self.x /= s;
        self.y /= s;
    }
}
impl From<(f64, f64)> for Vec2 {
    fn from(v: (f64, f64)) -> Self {
        Vec2::new(v.0, v.1)
    }
}
impl From<[f64; 2]> for Vec2 {
    fn from(v: [f64; 2]) -> Self {
        Vec2::new(v[0], v[1])
    }
}

/// Linear interpolation.
#[inline(always)]
pub fn mix(a: Vec2, b: Vec2, t: f64) -> Vec2 {
    a + (b - a) * t
}

/// Linear interpolation of scalars.
#[inline(always)]
pub fn mixf(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// Median of three values.
#[inline(always)]
pub fn median<T: PartialOrd + Copy>(a: T, b: T, c: T) -> T {
    max_t(min_t(a, b), min_t(max_t(a, b), c))
}

#[inline(always)]
fn min_t<T: PartialOrd + Copy>(a: T, b: T) -> T {
    if b < a { b } else { a }
}

#[inline(always)]
fn max_t<T: PartialOrd + Copy>(a: T, b: T) -> T {
    if a < b { b } else { a }
}

/// Returns 1 for positive values and -1 otherwise (never zero).
#[inline(always)]
pub fn non_zero_sign(x: f64) -> f64 {
    if x > 0.0 { 1.0 } else { -1.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_works() {
        assert_eq!(median(1, 2, 3), 2);
        assert_eq!(median(3, 1, 2), 2);
        assert_eq!(median(2, 3, 1), 2);
        assert_eq!(median(0.5f32, 0.5, 0.1), 0.5);
    }

    #[test]
    fn cross_sign() {
        assert!(Vec2::new(1.0, 0.0).cross(Vec2::new(0.0, 1.0)) > 0.0);
    }
}
