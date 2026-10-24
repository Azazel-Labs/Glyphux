//! Render a shaped COLR glyph using the CPU decoder or the GPU color pipeline.
//! usage: color_glyph <font.ttf> <text> [output.png] [size] [cpu|gpu]
use glyphux::color::{ColorField, ColorFieldConfig};
use glyphux_wgpu::color::{ColorGlyphInstance, ColorRenderer};
use glyphux_wgpu::{GlyphKey, Shaper, ShapingFont, default_device, read_texture_layer, wgpu};
use std::sync::Arc;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() < 3 {
        return Err("usage: color_glyph <font.ttf> <text> [output.png] [size] [cpu|gpu]".into());
    }
    let size: u32 = args.get(4).map_or(Ok(256), |s| s.parse())?;
    if !(16..=4096).contains(&size) {
        return Err("size must be 16 to 4096".into());
    }
    let font = ShapingFont::new(0, Arc::new(std::fs::read(&args[1])?), 0).ok_or("invalid font")?;
    let mut shaper = Shaper::new();
    let shaped = shaper.shape_line(&font, &args[2]);
    if shaped.glyphs.len() != 1 {
        return Err("text must shape to one glyph (a sequence or ligature is allowed)".into());
    }
    let glyph = shaped.glyphs[0].glyph;
    let source = font.color_glyph(glyph, 0, [0, 0, 0, 255])?;
    let field = ColorField::build(&source, ColorFieldConfig::default())?;
    let mode = args.get(5).map_or("gpu", String::as_str);
    let pixels = if mode == "cpu" {
        field
            .render(size)?
            .data()
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|c| {
                // Encode a conventional straight-alpha PNG from premultiplied linear output.
                let srgb = |v: f32| {
                    let v = if c[3] > 0.0 { v / c[3] } else { 0.0 };
                    glyphux::float_to_byte(if v <= 0.0031308 {
                        12.92 * v
                    } else {
                        1.055 * v.powf(1.0 / 2.4) - 0.055
                    })
                };
                [srgb(c[0]), srgb(c[1]), srgb(c[2]), glyphux::float_to_byte(c[3])]
            })
            .collect::<Vec<_>>()
    } else if mode == "gpu" {
        let (_, device, queue) = default_device()?;
        // A solid background makes the exported GPU pixels conventional sRGB PNG data.
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("color demo"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let mut renderer = ColorRenderer::new(&device, target.format());
        let key = GlyphKey { font: 0, glyph };
        renderer.upload(&device, &queue, key, &field)?;
        renderer.prepare(
            &device,
            &queue,
            [size as f32; 2],
            &[ColorGlyphInstance {
                key,
                rect: [0.0, 0.0, size as f32, size as f32],
                transform: [1.0, 0.0, 0.0, 0.0],
                opacity: 1.0,
            }],
        )?;
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("color demo"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.025,
                            g: 0.03,
                            b: 0.04,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..wgpu::RenderPassDescriptor::default()
            });
            renderer.render(&mut pass);
        }
        queue.submit([encoder.finish()]);
        read_texture_layer(&device, &queue, &target, 0)
    } else {
        return Err("mode must be cpu or gpu".into());
    };
    let path = args.get(3).map_or("color-glyph.png", String::as_str);
    std::fs::write(path, glyphux::io::png::encode_png(size, size, 4, &pixels))?;
    println!(
        "{path}: {} paints, {} recipes, {}x{} tiles, {} atlas bytes",
        field.paints().len(),
        field.recipes().len(),
        field.grid(),
        field.grid(),
        field.atlas_bytes().len()
    );
    Ok(())
}
