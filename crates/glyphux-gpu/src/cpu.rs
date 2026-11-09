//! The CPU generator's side of an atlas ([`crate::atlas::Generator::Cpu`]): glyphux's own
//! generator fills each glyph's field, and its texels are converted exactly as the store pass
//! converts the GPU's, so a backend without compute shaders uploads the same atlas pages.

use glyphux::gpu::FieldKind;
use glyphux::{Bitmap, GeneratorConfig, PreparedShape, Transform};

use crate::atlas::{AtlasConfig, AtlasFormat};

/// The field of one glyph in the atlas' texel format, rows top to bottom (as the store pass
/// writes them), `width * format.texel_size()` bytes per row.
pub(crate) fn generate_texels(
    shape: &PreparedShape,
    transform: &Transform,
    width: u32,
    height: u32,
    cfg: &AtlasConfig,
) -> Vec<u8> {
    // One thread per glyph: the workers already run glyphs side by side, and wasm32 has no
    // threads to spawn.
    let config = GeneratorConfig::default()
        .with_threads(1)
        .with_overlap_support(cfg.overlap_support)
        .with_error_correction(cfg.error_correction)
        .with_sign_correction(cfg.sign_correction);
    let values = field_values(shape, transform, width, height, cfg.kind, &config);
    let mut out = Vec::with_capacity(values.len() * cfg.format.texel_size() as usize);
    for texel in values {
        store_texel(texel, cfg.format, &mut out);
    }
    out
}

/// Each texel's four values as the store pass computes them from the field: a single channel
/// fills all four, MSDF without a true distance carries the median in alpha.
fn field_values(
    shape: &PreparedShape,
    transform: &Transform,
    width: u32,
    height: u32,
    kind: FieldKind,
    config: &GeneratorConfig,
) -> Vec<[f32; 4]> {
    match (kind.channels, kind.true_distance) {
        (0 | 1, _) => {
            let mut b = Bitmap::<1>::new(width, height);
            if kind.channels == 0 {
                glyphux::generate_sdf_prepared(&mut b, shape, transform, config);
            } else {
                glyphux::generate_psdf_prepared(&mut b, shape, transform, config);
            }
            b.data().iter().map(|&v| [v; 4]).collect()
        }
        (_, false) => {
            let mut b = Bitmap::<3>::new(width, height);
            glyphux::generate_msdf_prepared(&mut b, shape, transform, config);
            b.data()
                .as_chunks::<3>()
                .0
                .iter()
                .map(|&[x, y, z]| [x, y, z, glyphux::median(x, y, z)])
                .collect()
        }
        _ => {
            let mut b = Bitmap::<4>::new(width, height);
            glyphux::generate_mtsdf_prepared(&mut b, shape, transform, config);
            b.data().as_chunks::<4>().0.to_vec()
        }
    }
}

/// Appends one texel in `format`, clamped as `store.wgsl` clamps it (`store_shader_source`).
fn store_texel(texel: [f32; 4], format: AtlasFormat, out: &mut Vec<u8>) {
    for v in texel {
        match format {
            // As a unorm storage texture stores it: round to the nearest of 256 steps.
            AtlasFormat::Rgba8Unorm => out.push((v.clamp(0.0, 1.0) * 255.0).round() as u8),
            AtlasFormat::Rgba16Float => out.extend_from_slice(&f16_bits(v.clamp(-4.0, 5.0)).to_le_bytes()),
            AtlasFormat::Rgba32Float => out.extend_from_slice(&v.clamp(-1e6, 1e6).to_le_bytes()),
        }
    }
}

/// IEEE 754 half precision bits of `v`, rounded to nearest even (finite inputs; values beyond
/// the half range become infinity, NaN stays NaN).
pub(crate) fn f16_bits(v: f32) -> u16 {
    let bits = v.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xff) as i32;
    let mant = bits & 0x7f_ffff;
    if exp == 0xff {
        return sign | 0x7c00 | if mant != 0 { 0x200 } else { 0 };
    }
    let e = exp - 127 + 15;
    if e >= 0x1f {
        return sign | 0x7c00;
    }
    if e <= 0 {
        // Subnormal half (or zero): shift the full significand into place, then round.
        if e < -10 {
            return sign;
        }
        let m = mant | 0x80_0000;
        let shift = (14 - e) as u32;
        let half = m >> shift;
        let rest = m & ((1 << shift) - 1);
        let midpoint = 1 << (shift - 1);
        let round = u32::from(rest > midpoint || (rest == midpoint && half & 1 == 1));
        return sign | (half + round) as u16;
    }
    let half = ((e as u32) << 10) | (mant >> 13);
    let rest = mant & 0x1fff;
    // A carry out of the significand bumps the exponent, up to infinity: still correct.
    let round = u32::from(rest > 0x1000 || (rest == 0x1000 && half & 1 == 1));
    sign | (half + round) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_floats_round_to_nearest_even() {
        assert_eq!(f16_bits(0.0), 0x0000);
        assert_eq!(f16_bits(-0.0), 0x8000);
        assert_eq!(f16_bits(1.0), 0x3c00);
        assert_eq!(f16_bits(0.5), 0x3800);
        assert_eq!(f16_bits(-4.0), 0xc400);
        assert_eq!(f16_bits(5.0), 0x4500);
        assert_eq!(f16_bits(65504.0), 0x7bff);
        assert_eq!(f16_bits(1e6), 0x7c00);
        assert_eq!(f16_bits(f32::NAN) & 0x7e00, 0x7e00);
        // 1 + 2^-11 lies halfway between 1 and the next half: ties go to the even one.
        assert_eq!(f16_bits(1.0 + 2f32.powi(-11)), 0x3c00);
        assert_eq!(f16_bits(1.0 + 3.0 * 2f32.powi(-11)), 0x3c02);
        // Smallest subnormal half, and half of it (a tie, to even: zero).
        assert_eq!(f16_bits(2f32.powi(-24)), 0x0001);
        assert_eq!(f16_bits(2f32.powi(-25)), 0x0000);
        assert_eq!(f16_bits(1.5 * 2f32.powi(-24)), 0x0002);
        // Largest subnormal rounds up into the smallest normal.
        assert_eq!(f16_bits(2f32.powi(-14) - 2f32.powi(-26)), 0x0400);
    }

    #[test]
    fn texels_clamp_like_the_store_pass() {
        let mut out = Vec::new();
        store_texel([-0.5, 0.25, 0.5, 2.0], AtlasFormat::Rgba8Unorm, &mut out);
        assert_eq!(out, [0, 64, 128, 255]);
        out.clear();
        store_texel([-9.0, 0.5, 1.0, 9.0], AtlasFormat::Rgba16Float, &mut out);
        let halves: Vec<u16> = out.as_chunks::<2>().0.iter().map(|&b| u16::from_le_bytes(b)).collect();
        assert_eq!(halves, [0xc400, 0x3800, 0x3c00, 0x4500]);
        out.clear();
        store_texel([-2e6, 0.5, 1.0, 2e6], AtlasFormat::Rgba32Float, &mut out);
        let floats: Vec<f32> = out.as_chunks::<4>().0.iter().map(|&b| f32::from_le_bytes(b)).collect();
        assert_eq!(floats, [-1e6, 0.5, 1.0, 1e6]);
    }
}
