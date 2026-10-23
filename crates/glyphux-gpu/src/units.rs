//! Typed lengths for text layout: logical pixels, points and font-relative units, resolved to
//! the render target's physical pixels through [`Units`].
//!
//! ```
//! use glyphux_gpu::units::{Units, em, pt, px};
//! let units = Units::new(1.5); // devicePixelRatio 1.5, root size 16 px
//! assert_eq!(px(20.0).resolve(&units, 0.0), 30.0);
//! assert_eq!(pt(12.0).resolve(&units, 0.0), 24.0);
//! assert_eq!(em(0.25).resolve(&units, 48.0), 12.0); // a quarter of 48 px text
//! assert_eq!(units.text_px(em(2.0)), 48.0); // text sizes in em are relative to the root
//! ```

use std::ops::{Mul, Neg};

/// A length in a text style or a layout.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Length {
    /// Logical (CSS) pixels: physical pixels divided by [`Units::scale`].
    Px(f32),
    /// Points, 1/72 inch: 4/3 logical pixels.
    Pt(f32),
    /// Times the size of the text the length belongs to (an effect's text); for a text size
    /// itself, times the root size.
    Em(f32),
    /// Times the root text size ([`Units::root`]).
    Rem(f32),
    /// Percent of the text's size: `pct(20.0)` is `em(0.2)`.
    Percent(f32),
}

pub const fn px(v: f32) -> Length {
    Length::Px(v)
}
pub const fn pt(v: f32) -> Length {
    Length::Pt(v)
}
pub const fn em(v: f32) -> Length {
    Length::Em(v)
}
pub const fn rem(v: f32) -> Length {
    Length::Rem(v)
}
pub const fn pct(v: f32) -> Length {
    Length::Percent(v)
}

impl Length {
    pub const ZERO: Length = Length::Px(0.0);

    /// In physical pixels, for text whose em is `em_px` physical pixels.
    pub fn resolve(self, units: &Units, em_px: f32) -> f32 {
        match self {
            Length::Px(v) => v * units.scale,
            Length::Pt(v) => v * (4.0 / 3.0) * units.scale,
            Length::Em(v) => v * em_px,
            Length::Rem(v) => v * units.root_px(),
            Length::Percent(v) => v * 0.01 * em_px,
        }
    }
}

impl Default for Length {
    fn default() -> Self {
        Length::ZERO
    }
}

/// Scales the length, keeping its unit (for animation: `em(0.2) * pulse`).
impl Mul<f32> for Length {
    type Output = Length;
    fn mul(self, k: f32) -> Length {
        match self {
            Length::Px(v) => Length::Px(v * k),
            Length::Pt(v) => Length::Pt(v * k),
            Length::Em(v) => Length::Em(v * k),
            Length::Rem(v) => Length::Rem(v * k),
            Length::Percent(v) => Length::Percent(v * k),
        }
    }
}

impl Neg for Length {
    type Output = Length;
    fn neg(self) -> Length {
        self * -1.0
    }
}

/// How [`Length`]s become physical pixels of a render target.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Units {
    /// Physical pixels per logical pixel (the device pixel ratio).
    pub scale: f32,
    /// Root text size in logical pixels: what `rem`, and `em` / `%` text sizes, refer to.
    pub root: f32,
}

impl Units {
    /// `scale` physical pixels per logical pixel, 16 px root size (as browsers).
    pub fn new(scale: f32) -> Self {
        Units { scale, root: 16.0 }
    }

    pub fn with_root(self, root_px: f32) -> Self {
        Units { root: root_px, ..self }
    }

    /// The root text size in physical pixels.
    pub fn root_px(&self) -> f32 {
        self.root * self.scale
    }

    /// Physical pixels per em of text of `size` (`em` and `%` relative to the root).
    pub fn text_px(&self, size: Length) -> f32 {
        size.resolve(self, self.root_px())
    }

    /// A point in logical pixels, in physical pixels.
    pub fn to_physical(&self, p: [f32; 2]) -> [f32; 2] {
        [p[0] * self.scale, p[1] * self.scale]
    }
}

impl Default for Units {
    fn default() -> Self {
        Units::new(1.0)
    }
}
