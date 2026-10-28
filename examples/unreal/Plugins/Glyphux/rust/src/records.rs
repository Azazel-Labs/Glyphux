//! The sizes of what Rust gives C++ to draw (see `draw`). `build.rs` reads this file too and
//! writes these numbers into `GlyphuxRecords.h`, so both sides always agree.

// Some of these are only read by build.rs (and C++).
#![allow(dead_code)]

/// Texels per monochrome glyph record, and records per row of the records texture.
pub const RECORD_TEXELS: usize = 12;
pub const RECORDS_PER_ROW: usize = 64;
/// Texels per row of the color tables texture (and of a text surface's grid).
pub const TABLES_WIDTH: usize = 256;
/// Quads per row of a text surface's quads texture (three texels each).
pub const SURFACE_QUADS_PER_ROW: usize = 64;
/// Floats per quad: corners (top-left, top-right, bottom-left, bottom-right; x, y), the
/// texture coordinates of the top-left and bottom-right corners, the glyph's record index,
/// its pass, then its animation: wave amplitude and frequency (Hz), shake amplitude and rate
/// (per second), phase and seed.
pub const QUAD_FLOATS: usize = 20;
/// The passes of monochrome glyphs (glyphux's `text_pass`): 0 shadow and glow, 1 outline,
/// 2 fill, and 3 all at once (a glyph drawn alone: a particle, a surface). Color glyphs are
/// `PASS_COLOR`.
pub const PASS_ALL: usize = 3;
pub const PASS_COLOR: usize = 4;
