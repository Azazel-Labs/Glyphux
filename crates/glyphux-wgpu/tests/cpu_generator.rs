//! Atlas pages filled by the CPU generator (`Generator::Cpu`, uploaded) against pages filled by
//! the GPU generator, for a font's glyphs (skipped when no adapter is available).
use glyphux::io::font::{Font, FontScaling};
use glyphux_wgpu::{AtlasConfig, AtlasFormat, Generator, GlyphAtlas, GlyphKey, default_device, wgpu};

const FONT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../examples/web-demo/assets/Inter-Regular.otf"
);

/// The font's printable ASCII glyphs at a few sizes into an atlas of `generator`; its pages and
/// the glyphs' placements.
fn render(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    format: AtlasFormat,
    generator: Generator,
) -> (Vec<Vec<u8>>, Vec<glyphux_wgpu::AtlasGlyph>) {
    let data = std::fs::read(FONT).unwrap();
    let font = Font::from_bytes(&data, 0, FontScaling::EmNormalized).unwrap();
    let config = AtlasConfig {
        size: 512,
        format,
        detail_probe: false,
        generator,
        ..AtlasConfig::default()
    };
    let mut atlas = GlyphAtlas::new(device, config).unwrap();
    // Uploads need no storage texture (WebGL 2 has none).
    let storage = atlas.texture().usage().contains(wgpu::TextureUsages::STORAGE_BINDING);
    assert_eq!(storage, generator == Generator::Gpu);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    let mut placed = Vec::new();
    for px in [16.0, 48.0, 200.0] {
        for c in (33u8..127).map(char::from) {
            let key = GlyphKey {
                font: 0,
                glyph: u32::from(c),
            };
            if let Some(g) = atlas.get(key, px, || font.glyph(c).ok().map(|g| g.shape)) {
                placed.push(g);
            }
        }
        // Several flushes into one encoder, the last ones growing the texture.
        atlas.flush(device, &mut encoder);
    }
    queue.submit([encoder.finish()]);
    let pages = (0..atlas.page_count())
        .map(|layer| glyphux_wgpu::read_texture_layer(device, queue, atlas.texture(), layer))
        .collect();
    (pages, placed)
}

#[test]
fn cpu_pages_match_gpu_pages() {
    let (device, queue) = match default_device() {
        Ok((_, d, q)) => (d, q),
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    for format in [AtlasFormat::Rgba8Unorm, AtlasFormat::Rgba16Float] {
        let (gpu, gpu_placed) = render(&device, &queue, format, Generator::Gpu);
        let (cpu, cpu_placed) = render(&device, &queue, format, Generator::Cpu);
        assert_eq!(cpu_placed, gpu_placed, "the same placements");
        assert_eq!(cpu.len(), gpu.len());
        assert!(cpu.len() > 1, "the texture grew");
        let decode = |bytes: &[u8]| -> Vec<f32> {
            match format {
                AtlasFormat::Rgba8Unorm => bytes.iter().map(|&b| f32::from(b) / 255.0).collect(),
                _ => bytes
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|&b| half(u16::from_le_bytes(b)).clamp(0.0, 1.0))
                    .collect(),
            }
        };
        let (mut texels, mut inked, mut differ, mut far) = (0usize, 0usize, 0usize, 0usize);
        for (c, g) in cpu.iter().zip(&gpu) {
            let (c, g) = (decode(c), decode(g));
            for (c, g) in c.as_chunks::<4>().0.iter().zip(g.as_chunks::<4>().0) {
                texels += 1;
                if c.iter().chain(g).any(|&v| v > 0.0) {
                    inked += 1;
                }
                // What text shows: the median of the color channels, and the true distance.
                let d = (median(c) - median(g)).abs().max((c[3] - g[3]).abs());
                differ += usize::from(d > 1.5 / 255.0);
                far += usize::from(d > 0.1);
            }
        }
        eprintln!("{format:?}: {texels} texels, {inked} inked, {differ} differ, {far} differ by more than 0.1");
        assert!(inked > texels / 20, "glyphs were drawn");
        // CPU and GPU agree up to rounding, apart from ties the GPU's f32 breaks differently.
        assert!(differ * 1000 <= inked, "{differ} of {inked} texels differ");
        assert!(far * 10_000 <= inked, "{far} of {inked} texels differ by more than 0.1");
    }
}

fn median(v: &[f32; 4]) -> f32 {
    v[0].min(v[1]).max(v[0].max(v[1]).min(v[2]))
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
