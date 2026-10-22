//! Mapping between shape coordinates, distance field texels and output values.

use crate::math::Vec2;
use crate::shape::Bounds;

/// Maps shape coordinates to texel coordinates: `texel = (shape + translate) * scale`.
/// Texel coordinates are y-up with the origin at the bottom-left corner of the bitmap;
/// texel `(x, y)` has its center at `(x + 0.5, y + 0.5)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Projection {
    pub scale: Vec2,
    pub translate: Vec2,
}

impl Default for Projection {
    fn default() -> Self {
        Projection {
            scale: Vec2::splat(1.0),
            translate: Vec2::ZERO,
        }
    }
}

impl Projection {
    pub fn new(scale: Vec2, translate: Vec2) -> Self {
        Projection { scale, translate }
    }
    pub fn uniform(scale: f64, translate: Vec2) -> Self {
        Projection {
            scale: Vec2::splat(scale),
            translate,
        }
    }
    #[inline]
    pub fn project(&self, p: Vec2) -> Vec2 {
        (p + self.translate).mul_elem(self.scale)
    }
    #[inline]
    pub fn unproject(&self, p: Vec2) -> Vec2 {
        p.div_elem(self.scale) - self.translate
    }
    #[inline]
    pub fn project_vector(&self, v: Vec2) -> Vec2 {
        v.mul_elem(self.scale)
    }
    #[inline]
    pub fn unproject_vector(&self, v: Vec2) -> Vec2 {
        v.div_elem(self.scale)
    }
}

/// A range of signed distances `[lower, upper]` (in shape units) that maps to output `[0, 1]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Range {
    pub lower: f64,
    pub upper: f64,
}

impl Range {
    /// Symmetric range of total width `width`: `[-width/2, +width/2]`.
    pub fn symmetric(width: f64) -> Self {
        Range {
            lower: -0.5 * width,
            upper: 0.5 * width,
        }
    }
    pub fn new(lower: f64, upper: f64) -> Self {
        Range { lower, upper }
    }
    pub fn width(&self) -> f64 {
        self.upper - self.lower
    }
    /// Converts a range given in texels to shape units for the given projection scale.
    pub fn px_to_units(&self, scale: f64) -> Range {
        Range {
            lower: self.lower / scale,
            upper: self.upper / scale,
        }
    }
}

/// Linear map from signed distance (shape units) to output value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DistanceMapping {
    pub scale: f64,
    pub translate: f64,
}

impl DistanceMapping {
    pub fn new(range: Range) -> Self {
        DistanceMapping {
            scale: 1.0 / (range.upper - range.lower),
            translate: -range.lower,
        }
    }
    #[inline(always)]
    pub fn map(&self, d: f64) -> f64 {
        self.scale * (d + self.translate)
    }
    /// Maps a distance difference (no offset).
    #[inline(always)]
    pub fn map_delta(&self, d: f64) -> f64 {
        self.scale * d
    }
    #[inline(always)]
    pub fn unmap(&self, v: f64) -> f64 {
        v / self.scale - self.translate
    }
}

/// Complete placement of a shape in a distance field.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub projection: Projection,
    pub range: Range,
}

impl Transform {
    pub fn new(projection: Projection, range: Range) -> Self {
        Transform { projection, range }
    }
    pub fn mapping(&self) -> DistanceMapping {
        DistanceMapping::new(self.range)
    }
    /// Output value that corresponds to the shape boundary (0.5 for symmetric ranges).
    pub fn zero_value(&self) -> f32 {
        self.mapping().map(0.0) as f32
    }

    /// Frames `bounds` into a `width` x `height` bitmap, leaving room for the distance range
    /// (`px_range` texels total width), preserving aspect ratio and centering the shape.
    /// Mirrors the behavior of msdfgen's `-autoframe` with `-pxrange`.
    pub fn autoframe(bounds: Bounds, width: u32, height: u32, px_range: f64) -> Option<Transform> {
        let (mut l, mut b, mut r, mut t) = (bounds.left, bounds.bottom, bounds.right, bounds.top);
        if !(l < r && b < t) {
            l = 0.0;
            b = 0.0;
            r = 1.0;
            t = 1.0;
        }
        let px_lower = -0.5 * px_range;
        let frame = Vec2::new(width as f64 + 2.0 * px_lower, height as f64 + 2.0 * px_lower);
        if frame.x <= 0.0 || frame.y <= 0.0 {
            return None;
        }
        let dims = Vec2::new(r - l, t - b);
        let (mut translate, scale) = if dims.x * frame.y < dims.y * frame.x {
            (
                Vec2::new(0.5 * (frame.x / frame.y * dims.y - dims.x) - l, -b),
                frame.y / dims.y,
            )
        } else {
            (
                Vec2::new(-l, 0.5 * (frame.y / frame.x * dims.x - dims.y) - b),
                frame.x / dims.x,
            )
        };
        translate -= Vec2::splat(px_lower / scale);
        Some(Transform {
            projection: Projection::uniform(scale, translate),
            range: Range::symmetric(px_range / scale),
        })
    }

    /// Frames `bounds` with a fixed `scale`, centering it in the bitmap.
    pub fn autoframe_with_scale(bounds: Bounds, width: u32, height: u32, scale: Vec2, range: Range) -> Transform {
        let (mut l, mut b, mut r, mut t) = (bounds.left, bounds.bottom, bounds.right, bounds.top);
        if !(l < r && b < t) {
            l = 0.0;
            b = 0.0;
            r = 1.0;
            t = 1.0;
        }
        let frame = Vec2::new(width as f64, height as f64);
        let dims = Vec2::new(r - l, t - b);
        let translate = (frame.div_elem(scale) - dims) * 0.5 - Vec2::new(l, b);
        Transform {
            projection: Projection::new(scale, translate),
            range,
        }
    }
}
