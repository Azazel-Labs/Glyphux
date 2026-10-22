//! Rendering distance fields (for previews and testing) and exact rasterization of shapes.

use crate::bitmap::Bitmap;
use crate::math::{Vec2, median};
use crate::shape::{FillRule, Shape};
use crate::transform::Projection;

/// Renders a distance field as an anti-aliased coverage image of size `width` x `height`.
/// `px_range` is the distance field's range in its own texels, `zero` the value at the edge
/// (0.5 for symmetric ranges). Multi-channel fields use the median of the first three channels.
pub fn render_sdf<const N: usize>(sdf: &Bitmap<N>, width: u32, height: u32, px_range: f64, zero: f32) -> Bitmap<1> {
    let mut out = Bitmap::<1>::new(width, height);
    let sx = sdf.width() as f64 / width as f64;
    let sy = sdf.height() as f64 / height as f64;
    // Distance range in output pixels.
    let screen_range = f64::midpoint(px_range / sx, px_range / sy);
    for y in 0..height {
        for x in 0..width {
            let s = sdf.sample((x as f64 + 0.5) * sx, (y as f64 + 0.5) * sy);
            let d = if N >= 3 { median(s[0], s[1], s[2]) } else { s[0] };
            let v = ((d - zero) as f64 * screen_range + 0.5).clamp(0.0, 1.0);
            out.pixel_mut(x, y)[0] = v as f32;
        }
    }
    out
}

/// Renders every channel separately (useful to inspect how a multi-channel field works).
pub fn render_sdf_multi<const N: usize>(
    sdf: &Bitmap<N>,
    width: u32,
    height: u32,
    px_range: f64,
    zero: f32,
) -> Bitmap<N> {
    let mut out = Bitmap::<N>::new(width, height);
    let sx = sdf.width() as f64 / width as f64;
    let sy = sdf.height() as f64 / height as f64;
    let screen_range = f64::midpoint(px_range / sx, px_range / sy);
    for y in 0..height {
        for x in 0..width {
            let s = sdf.sample((x as f64 + 0.5) * sx, (y as f64 + 0.5) * sy);
            let px = out.pixel_mut(x, y);
            for c in 0..N {
                px[c] = ((s[c] - zero) as f64 * screen_range + 0.5).clamp(0.0, 1.0) as f32;
            }
        }
    }
    out
}

/// Exactly rasterizes the shape's coverage into a `width` x `height` image using
/// `samples` x `samples` supersampling per pixel. `projection` maps shape coordinates to
/// pixel coordinates of a y-up bitmap (as for distance field generation).
pub fn rasterize(
    shape: &Shape,
    width: u32,
    height: u32,
    projection: &Projection,
    fill_rule: FillRule,
    samples: u32,
) -> Bitmap<1> {
    let mut out = Bitmap::<1>::new(width, height);
    let s = samples.max(1);
    let inv = 1.0 / s as f64;
    let mut crossings = Vec::new();
    let mut acc = vec![0u32; width as usize];
    for r in 0..height {
        acc.fill(0);
        let base = if shape.inverse_y_axis { r } else { height - 1 - r } as f64;
        for sy in 0..s {
            let ty = base + (sy as f64 + 0.5) * inv;
            let y = projection.unproject(Vec2::new(0.0, ty)).y;
            shape.scanline(y, &mut crossings);
            let total: i32 = crossings.iter().map(|c| c.1).sum();
            let mut left = 0;
            let mut k = 0;
            for x in 0..width {
                for sx in 0..s {
                    let tx = x as f64 + (sx as f64 + 0.5) * inv;
                    let px = projection.unproject(Vec2::new(tx, 0.0)).x;
                    while k < crossings.len() && crossings[k].0 <= px {
                        left += crossings[k].1;
                        k += 1;
                    }
                    if fill_rule.is_filled(-(total - left)) {
                        acc[x as usize] += 1;
                    }
                }
            }
        }
        let norm = 1.0 / (s * s) as f32;
        for x in 0..width {
            out.pixel_mut(x, r)[0] = acc[x as usize] as f32 * norm;
        }
    }
    out
}
