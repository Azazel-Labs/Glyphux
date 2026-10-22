//! # msdfr
//!
//! A pure Rust generator of signed distance fields for vector shapes and font glyphs:
//! true SDF, pseudo-SDF, multi-channel SDF (MSDF) and MSDF + true SDF in alpha (MTSDF),
//! based on the multi-channel method by Viktor Chlumský (msdfgen).
//!
//! ```
//! use msdfr::*;
//!
//! // A square, clockwise in y-up coordinates (filled side on the right of each edge).
//! let mut shape = Shape::from_description("{ 0,0; 0,1; 1,1; 1,0; # }").unwrap();
//! shape.normalize();
//! edge_coloring_simple(&mut shape, DEFAULT_ANGLE_THRESHOLD, 0);
//!
//! let transform = Transform::autoframe(shape.bounds(), 32, 32, 4.0).unwrap();
//! let mut msdf = Bitmap::<3>::new(32, 32);
//! generate_msdf(&mut msdf, &shape, &transform, &GeneratorConfig::default());
//! // Inside texels are > 0.5, outside < 0.5.
//! let center = msdf.pixel(16, 16);
//! assert!(median(center[0], center[1], center[2]) > 0.5);
//! ```

#![allow(
    clippy::needless_range_loop,
    clippy::neg_cmp_op_on_partial_ord,
    clippy::manual_range_patterns
)]

pub mod batch;
pub mod bitmap;
pub mod coloring;
pub mod correction;
pub mod edge;
mod eval;
pub mod generate;
#[cfg(feature = "gpu")]
pub mod gpu;
pub mod io;
pub mod math;
pub mod render;
pub mod shape;
pub(crate) mod solver;
pub mod transform;

pub use batch::par_map;
pub use bitmap::{Bitmap, float_to_byte};
pub use coloring::{
    ColoringStrategy, DEFAULT_ANGLE_THRESHOLD, coloring_is_valid, edge_coloring, edge_coloring_by_distance,
    edge_coloring_ink_trap, edge_coloring_simple,
};
pub use correction::{DistanceCheckMode, ErrorCorrectionConfig, ErrorCorrectionMode, correct_msdf_fast};
pub use edge::{Edge, EdgeColor, Segment, SignedDistance};
pub use eval::PreparedShape;
pub use generate::{
    GeneratorConfig, distance_sign_correction, generate_msdf, generate_msdf_prepared, generate_mtsdf,
    generate_mtsdf_prepared, generate_psdf, generate_psdf_prepared, generate_sdf, generate_sdf_prepared,
    shape_distance, shape_true_distance,
};
pub use math::{Point2, Vec2, median};
pub use render::{rasterize, render_sdf, render_sdf_multi};
pub use shape::{Bounds, Contour, FillRule, Shape, ShapeError};
pub use transform::{DistanceMapping, Projection, Range, Transform};

/// Applies error correction to an MSDF/MTSDF that was generated with correction disabled.
pub fn msdf_error_correction<const N: usize>(
    sdf: &mut Bitmap<N>,
    shape: &Shape,
    transform: &Transform,
    config: &GeneratorConfig,
) -> usize {
    let ps = PreparedShape::new(shape);
    correction::correct_msdf(
        sdf,
        &ps,
        transform,
        &config.error_correction,
        config.overlap_support,
        config.threads.max(1),
    )
}
