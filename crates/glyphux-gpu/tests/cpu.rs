//! The CPU generator (`Generator::Cpu`): where its fields go and what they hold (no GPU
//! needed).
use glyphux_gpu::glyphux::Shape;
use glyphux_gpu::{AtlasConfig, AtlasField, AtlasFormat, AtlasGlyph, Generator, GlyphAtlas, GlyphKey};

const SIZE: u32 = 128;

fn config(format: AtlasFormat, generator: Generator) -> AtlasConfig {
    AtlasConfig {
        size: SIZE,
        layers: 2,
        min_level: 32,
        max_level: 32,
        detail_probe: false,
        format,
        generator,
        ..AtlasConfig::default()
    }
}

/// A right triangle: ink at the bottom left, none at the top right (so a field upside down
/// would show).
fn triangle() -> Option<Shape> {
    Shape::from_description("{ 0,0; 0,1; 1,0; # }").ok()
}

const KEY: GlyphKey = GlyphKey { font: 0, glyph: 1 };

/// The texel of `field` (`texel` bytes each) at shape coordinates `x`, `y`: texel centers start
/// at the quad's corner, rows run top to bottom.
fn texel_at<'a>(field: &'a AtlasField, g: &AtlasGlyph, texel: usize, x: f32, y: f32) -> &'a [u8] {
    let level = g.level as f32;
    let column = ((x - g.plane_min[0]) * level).round() as usize;
    let row = ((g.plane_max[1] - y) * level).round() as usize;
    let at = (row * field.width as usize + column) * texel;
    &field.texels[at..at + texel]
}

#[test]
fn fields_fill_the_placed_rectangles() {
    let mut atlas = GlyphAtlas::new(config(AtlasFormat::Rgba8Unorm, Generator::Cpu)).unwrap();
    let g = atlas.get(KEY, 64.0, triangle).unwrap();
    // A glyph with nothing to draw is blank and gets no field.
    let space = atlas.get(GlyphKey { font: 0, glyph: 2 }, 64.0, || Some(Shape::default()));
    assert!(!space.unwrap().is_visible());
    // Batches are for the GPU generator: the request stays queued for take_fields.
    assert!(atlas.take_batch().is_none());
    let fields = atlas.take_fields();
    assert_eq!(fields.len(), 1);
    let f = &fields[0];
    let corner = |uv: f32, half: f32| (uv * SIZE as f32 + half).round() as u32;
    assert_eq!(
        (f.layer, f.x, f.y),
        (g.layer, corner(g.uv_min[0], -0.5), corner(g.uv_min[1], -0.5))
    );
    assert_eq!(
        (f.x + f.width, f.y + f.height),
        (corner(g.uv_max[0], 0.5), corner(g.uv_max[1], 0.5))
    );
    assert_eq!(f.texels.len(), (f.width * f.height * 4) as usize);
    assert_eq!(atlas.take_written_rows(), vec![(f.layer, f.y..f.y + f.height)]);
    assert_eq!(atlas.stats().generated, 1);
    assert_eq!(atlas.stats().generated_texels, u64::from(f.width * f.height));
    // Far inside and far outside the outline (9 texels from it, the range is 4), the median of
    // the color channels and the true distance in alpha saturate.
    let shown = |t: &[u8]| [t[0].min(t[1]).max(t[0].max(t[1]).min(t[2])), t[3]];
    assert_eq!(shown(texel_at(f, &g, 4, 0.5, 0.1)), [255; 2]);
    assert_eq!(shown(texel_at(f, &g, 4, 0.5, 0.9)), [0; 2]);
    // Generated once: a second request is a cache hit.
    assert_eq!(atlas.get(KEY, 64.0, triangle), Some(g));
    assert_eq!(atlas.take_fields(), Vec::<AtlasField>::new());
}

#[test]
fn gpu_atlases_return_no_fields() {
    let mut atlas = GlyphAtlas::new(config(AtlasFormat::Rgba8Unorm, Generator::Gpu)).unwrap();
    atlas.get(KEY, 64.0, triangle).unwrap();
    assert_eq!(atlas.take_fields(), Vec::<AtlasField>::new());
    assert_eq!(atlas.take_batch().map(|b| b.layouts.len()), Some(1));
}

/// IEEE half precision bits to f32 (finite values).
fn half(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = i32::from((bits >> 10) & 0x1f);
    let mant = f32::from(bits & 0x3ff);
    sign * if exp == 0 {
        mant * 2f32.powi(-24)
    } else {
        (1.0 + mant / 1024.0) * 2f32.powi(exp - 15)
    }
}

#[test]
fn float_formats_hold_the_same_field() {
    let field = |format| {
        let mut atlas = GlyphAtlas::new(config(format, Generator::Cpu)).unwrap();
        let g = atlas.get(KEY, 64.0, triangle).unwrap();
        (g, atlas.take_fields().remove(0))
    };
    let (g, unorm) = field(AtlasFormat::Rgba8Unorm);
    let (_, f16) = field(AtlasFormat::Rgba16Float);
    let (_, f32) = field(AtlasFormat::Rgba32Float);
    let halves: Vec<f32> = f16
        .texels
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&b| half(u16::from_le_bytes(b)))
        .collect();
    let floats: Vec<f32> = f32
        .texels
        .as_chunks::<4>()
        .0
        .iter()
        .map(|&b| f32::from_le_bytes(b))
        .collect();
    assert_eq!(halves.len(), unorm.texels.len());
    assert_eq!(floats.len(), unorm.texels.len());
    for ((&byte, &h), &v) in unorm.texels.iter().zip(&halves).zip(&floats) {
        // The same values, at each format's precision and clamped to its range.
        assert!((h - v.clamp(-4.0, 5.0)).abs() <= 4e-3 * v.abs().max(1.0), "{h} vs {v}");
        let from_float = (v.clamp(0.0, 1.0) * 255.0).round();
        assert_eq!(f32::from(byte), from_float);
    }
    // Float formats keep distances beyond the range: far outside is well below zero.
    let level = g.level as f32;
    let column = ((0.5 - g.plane_min[0]) * level).round() as usize;
    let row = ((g.plane_max[1] - 0.9) * level).round() as usize;
    let alpha = floats[(row * unorm.width as usize + column) * 4 + 3];
    assert!(alpha < -1.0, "true distance {alpha}");
}
