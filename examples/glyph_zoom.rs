//! Renders text from fields of one fixed atlas level, magnified (artifact hunting).
//!
//! usage: glyph_zoom <font> <text> <level> <px> <out.png> [ec: auto|off|fast|full]
use std::sync::Arc;

use glyphux_wgpu::text::{Align, TextStyle, layout_text};
use glyphux_wgpu::units::{Units, px};
use glyphux_wgpu::{AtlasConfig, GlyphAtlas, Shaper, ShapingFont, TextRenderer, default_device, glyphux, wgpu};

const USAGE: &str = "usage: glyph_zoom <font> <text> <level> <px> <out.png> [ec: auto|off|fast|full]";

/// Prints `msg` and the usage line, and exits.
fn fail(msg: &str) -> ! {
    eprintln!("{msg}\n{USAGE}");
    std::process::exit(2)
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 6 {
        fail("missing arguments");
    }
    let level: u32 = a[3].parse().unwrap_or_else(|_| fail("the level is not a number"));
    let size: f32 = a[4].parse().unwrap_or_else(|_| fail("the size is not a number"));
    let data = std::fs::read(&a[1]).unwrap_or_else(|e| fail(&format!("{}: {e}", a[1])));
    let font = ShapingFont::new(0, Arc::new(data), 0).unwrap_or_else(|| fail(&format!("{}: not a font", a[1])));
    // level 0: the atlas' own level choice (MAG / MINL override magnification / min_level).
    let mut config = if level == 0 {
        let mut c = AtlasConfig::default();
        if let Ok(m) = std::env::var("MAG") {
            c.magnification = m.parse().unwrap();
        }
        if let Ok(m) = std::env::var("MINL") {
            c.min_level = m.parse().unwrap();
        }
        c
    } else {
        AtlasConfig {
            min_level: level,
            max_level: level,
            detail_probe: false,
            ..Default::default()
        }
    };
    match a.get(6).map(String::as_str) {
        Some("off") => config.error_correction = glyphux::ErrorCorrectionConfig::DISABLED,
        Some("fast") => {
            config.error_correction.distance_check = glyphux::DistanceCheckMode::DoNotCheck;
        }
        Some("full") => {
            config.error_correction.distance_check = glyphux::DistanceCheckMode::AlwaysCheck;
        }
        _ => {}
    }
    match std::env::var("COLORING").as_deref() {
        Ok("simple") => config.coloring = glyphux::ColoringStrategy::Simple,
        Ok("distance") => config.coloring = glyphux::ColoringStrategy::Distance,
        Ok("inktrap") => config.coloring = glyphux::ColoringStrategy::InkTrap,
        _ => {}
    }
    let mut shaper = Shaper::new();
    let line = shaper.shape_line(&font, &a[2]);
    let (w, h) = ((line.width * size + size * 0.4) as u32, (size * 1.35) as u32);
    let (_, device, queue) = default_device().expect("device");
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let mut atlas = GlyphAtlas::new(&device, config).expect("atlas configuration");
    let mut renderer = TextRenderer::new(&device, format);
    let mut quads = Vec::new();
    // Frames until the detail probes (level 0 only) have reported and every glyph is at its
    // final level; a few seconds at most.
    for frame in 0..1000 {
        atlas.begin_frame();
        quads.clear();
        layout_text(
            &mut atlas,
            &mut shaper,
            &font,
            &a[2],
            &TextStyle::new(px(size), [1.0; 4]),
            &Units::default(),
            [size * 0.2, size * 1.0],
            w as f32,
            Align::Start,
            None,
            &mut quads,
        );
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        let generated = atlas.flush(&device, &mut enc);
        queue.submit([enc.finish()]);
        if generated == 0 && atlas.probes_pending() == 0 {
            break;
        }
        if frame == 999 {
            eprintln!("warning: detail probes still pending; rendering anyway");
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    renderer.prepare(&device, &queue, &atlas, [w as f32, h as f32], &quads);
    {
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        renderer.render(&mut pass);
    }
    queue.submit([enc.finish()]);
    let pixels = glyphux_wgpu::read_texture_layer(&device, &queue, &target, 0);
    std::fs::write(&a[5], glyphux::io::png::encode_png(w, h, 4, &pixels)).unwrap();
}
