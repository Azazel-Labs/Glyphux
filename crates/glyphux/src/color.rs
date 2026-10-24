//! Color glyphs encoded as local region distances and analytic paint recipes.
//!
//! RGB stores up to three region SDFs; alpha stores the independent outer union SDF.
//! These fields are linear data, not MSDFs and not premultiplied color images.

mod buffers;
mod fields;
pub use buffers::ColorBuffers;
#[cfg(feature = "font")]
mod font;

pub use fields::{ColorField, ColorFieldConfig, ColorTile, TileMode};

use crate::{Shape, Vec2};

/// Invalid input or a color-font operation this implementation cannot represent.
#[derive(Clone, Debug, PartialEq)]
pub enum ColorError {
    Invalid(&'static str),
    Unsupported(&'static str),
    /// No candidate tile grid could represent every local recipe.
    Overflow {
        tile: [u32; 2],
        regions: usize,
    },
}

impl core::fmt::Display for ColorError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Invalid(s) => write!(f, "invalid color glyph: {s}"),
            Self::Unsupported(s) => write!(f, "unsupported color glyph: {s}"),
            Self::Overflow { tile, regions } => write!(f, "color tile {tile:?} needs {regions} recipes (maximum 4)"),
        }
    }
}
impl std::error::Error for ColorError {}

/// Row-major affine map: `[xx, xy, tx, yx, yy, ty]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine(pub [f64; 6]);

impl Affine {
    pub const IDENTITY: Self = Self([1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);

    pub fn apply(self, p: Vec2) -> Vec2 {
        let [a, b, c, d, e, f] = self.0;
        Vec2::new(a * p.x + b * p.y + c, d * p.x + e * p.y + f)
    }

    /// Applies `rhs` first, then this map.
    pub fn compose(self, rhs: Self) -> Self {
        let [a, b, c, d, e, f] = self.0;
        let [g, h, i, j, k, l] = rhs.0;
        Self([
            a * g + b * j,
            a * h + b * k,
            a * i + b * l + c,
            d * g + e * j,
            d * h + e * k,
            d * i + e * l + f,
        ])
    }

    pub fn inverse(self) -> Result<Self, ColorError> {
        let [a, b, c, d, e, f] = self.0;
        let det = a * e - b * d;
        if !self.0.iter().all(|v| v.is_finite()) || !det.is_finite() || det == 0.0 {
            return Err(ColorError::Invalid("singular or non-finite paint transform"));
        }
        Ok(Self([
            e / det,
            -b / det,
            (b * f - e * c) / det,
            -d / det,
            a / det,
            (d * c - a * f) / det,
        ]))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Extend {
    Pad,
    Repeat,
    Reflect,
}

/// Linear-light, unpremultiplied RGBA at a gradient stop.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorStop {
    pub offset: f64,
    pub color: [f32; 4],
}

#[derive(Clone, Debug, PartialEq)]
pub struct ColorLine {
    pub stops: Vec<ColorStop>,
    pub extend: Extend,
}

impl ColorLine {
    pub fn validate(&self) -> Result<(), ColorError> {
        if self.stops.len() < 2
            || self
                .stops
                .iter()
                .any(|s| !s.offset.is_finite() || !valid_color(s.color))
            || self.stops.windows(2).any(|w| w[0].offset > w[1].offset)
            || self.stops[0].offset >= self.stops[self.stops.len() - 1].offset
        {
            return Err(ColorError::Invalid(
                "gradient needs ordered finite stops with a nonzero span",
            ));
        }
        Ok(())
    }

    /// Evaluates validated stops, returning premultiplied linear RGBA.
    pub fn evaluate(&self, mut t: f64) -> [f32; 4] {
        let first = self.stops[0];
        let last = self.stops[self.stops.len() - 1];
        let length = last.offset - first.offset;
        if self.extend == Extend::Pad && t < first.offset {
            return premultiply(first.color);
        }
        t = match self.extend {
            Extend::Pad => t.clamp(first.offset, last.offset),
            Extend::Repeat => first.offset + (t - first.offset).rem_euclid(length),
            Extend::Reflect => first.offset + length - ((t - first.offset).rem_euclid(2.0 * length) - length).abs(),
        };
        let i = self.stops.partition_point(|s| s.offset <= t);
        if i == 0 {
            return premultiply(first.color);
        }
        if i == self.stops.len() {
            return premultiply(last.color);
        }
        let lo = self.stops[i - 1];
        let hi = self.stops[i];
        let u = ((t - lo.offset) / (hi.offset - lo.offset)) as f32;
        let a = premultiply(lo.color);
        let b = premultiply(hi.color);
        std::array::from_fn(|c| a[c] * (1.0 - u) + b[c] * u)
    }
}

/// Paint coordinates are obtained by applying `inverse` to glyph coordinates.
#[derive(Clone, Debug, PartialEq)]
pub enum Paint {
    Solid([f32; 4]),
    Linear {
        inverse: Affine,
        origin: Vec2,
        direction: Vec2,
        line: ColorLine,
    },
    Radial {
        inverse: Affine,
        center: Vec2,
        r0: f64,
        r1: f64,
        line: ColorLine,
    },
}

impl Paint {
    /// COLR's three-point linear gradient, including sheared color lines.
    pub fn linear(inverse: Affine, p0: Vec2, p1: Vec2, p2: Vec2, line: ColorLine) -> Result<Self, ColorError> {
        let a = p1 - p0;
        let b = p2 - p0;
        let v = Vec2::new(b.y, -b.x);
        let denominator = v.dot(a);
        if denominator == 0.0 || !denominator.is_finite() {
            return Err(ColorError::Invalid("degenerate linear gradient"));
        }
        let paint = Self::Linear {
            inverse,
            origin: p0,
            direction: v / denominator,
            line,
        };
        paint.validate()?;
        Ok(paint)
    }

    pub fn validate(&self) -> Result<(), ColorError> {
        match self {
            Self::Solid(c) if valid_color(*c) => Ok(()),
            Self::Solid(_) => Err(ColorError::Invalid(
                "color components must be finite and between 0 and 1",
            )),
            Self::Linear {
                inverse,
                origin,
                direction,
                line,
            } => {
                inverse.inverse()?;
                if !finite_point(*origin) || !finite_point(*direction) || direction.dot(*direction) == 0.0 {
                    return Err(ColorError::Invalid("linear gradient coordinates"));
                }
                line.validate()
            }
            Self::Radial {
                inverse,
                center,
                r0,
                r1,
                line,
            } => {
                inverse.inverse()?;
                if !finite_point(*center) || !r0.is_finite() || !r1.is_finite() || *r0 < 0.0 || r1 <= r0 {
                    return Err(ColorError::Unsupported(
                        "radial gradient must be concentric and expanding",
                    ));
                }
                line.validate()
            }
        }
    }

    pub fn is_opaque(&self) -> bool {
        match self {
            Self::Solid(c) => c[3] == 1.0,
            Self::Linear { line, .. } | Self::Radial { line, .. } => line.stops.iter().all(|s| s.color[3] == 1.0),
        }
    }

    /// Premultiplied linear-light RGBA.
    pub fn evaluate(&self, q: Vec2) -> [f32; 4] {
        match self {
            Self::Solid(c) => premultiply(*c),
            Self::Linear {
                inverse,
                origin,
                direction,
                line,
            } => line.evaluate((inverse.apply(q) - *origin).dot(*direction)),
            Self::Radial {
                inverse,
                center,
                r0,
                r1,
                line,
            } => line.evaluate(((inverse.apply(q) - *center).length() - r0) / (r1 - r0)),
        }
    }
}

/// A geometric layer in back-to-front source-over order.
#[derive(Clone, Debug)]
pub struct ColorLayer {
    pub shape: Shape,
    pub paint: usize,
}

/// Input to the color-field compiler. Geometry lives in `[-1, 1]`, y downward.
#[derive(Clone, Debug)]
pub struct ColorGlyph {
    pub paints: Vec<Paint>,
    pub layers: Vec<ColorLayer>,
    /// Domain bounds in font units (or the font loader's requested scaling), y upward.
    pub plane_min: Vec2,
    pub plane_max: Vec2,
    pub advance: f64,
}

pub(crate) fn finite_point(p: Vec2) -> bool {
    p.x.is_finite() && p.y.is_finite()
}
fn valid_color(c: [f32; 4]) -> bool {
    c.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v))
}
pub(crate) fn premultiply(c: [f32; 4]) -> [f32; 4] {
    [c[0] * c[3], c[1] * c[3], c[2] * c[3], c[3]]
}

/// Source-over for premultiplied colors; alpha is applied exactly once.
pub fn source_over(foreground: [f32; 4], background: [f32; 4]) -> [f32; 4] {
    std::array::from_fn(|i| foreground[i] + background[i] * (1.0 - foreground[3]))
}

pub fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// WGSL decoder for the version 1 color atlas buffer layout: [`COLOR_CORE`] followed by
/// its bindings and entry points for wgpu.
pub const COLOR_SHADER: &str = concat!(
    include_str!("color/color_core.wgsl"),
    "
",
    include_str!("color/color.wgsl")
);

/// The color shading functions of [`COLOR_SHADER`] without bindings or entry points, for
/// backends that read the tables their own way (see the file's comments).
pub const COLOR_CORE: &str = include_str!("color/color_core.wgsl");
