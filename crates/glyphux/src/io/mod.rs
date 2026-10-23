//! Input (shape description, SVG, fonts) and output (PNG, BMP, FL32, raw, text) formats.

pub mod image;
pub mod png;
pub mod shapedesc;
pub mod svg;

#[cfg(feature = "font")]
pub mod font;

pub use image::{ImageFormat, save, save_bmp, save_fl32, save_png};
