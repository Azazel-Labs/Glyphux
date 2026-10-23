//! Headless render of shaped text (ligatures, Arabic joining, bidi, Indic conjuncts, Thai
//! marks, variable font weights) through the glyph atlas, to a PNG.
//!
//! usage: shaping_demo [out.png]   (uses fonts commonly found on Linux; missing ones are skipped)
use std::sync::Arc;

use glyphux_gpu::text::{Align, TextStyle, layout_text};
use glyphux_gpu::units::{Units, px};
use glyphux_gpu::{AtlasConfig, GlyphAtlas, Shaper, ShapingFont, TextRenderer, default_device, glyphux, wgpu};

/// Font path, variation axes, text, size (px).
type Line = (&'static str, &'static [(&'static str, f32)], &'static str, f32);

const LINES: &[Line] = &[
    (
        "/usr/share/texmf/fonts/opentype/public/lm/lmroman10-bold.otf",
        &[],
        "Ligatures: office, affluent, fjord",
        34.0,
    ),
    (
        "/usr/share/fonts/opentype/inter/Inter-Regular.otf",
        &[],
        "Kerning: AVATAR, Type, WAVY · 1/2 → ½",
        34.0,
    ),
    (
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        &[],
        "Bidi: The word שָׁלוֹם means peace (123).",
        30.0,
    ),
    (
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        &[],
        "السلام عليكم ورحمة الله",
        40.0,
    ),
    (
        "/usr/share/fonts/truetype/freefont/FreeSerif.ttf",
        &[],
        "नमस्ते दुनिया · क्षत्रिय हिन्दी",
        40.0,
    ),
    (
        "/usr/share/fonts/opentype/tlwg/Loma-Bold.otf",
        &[],
        "สวัสดีครับ ภาษาไทย",
        36.0,
    ),
    (
        "/usr/share/fonts/truetype/google-fonts/Lora-Variable.ttf",
        &[("wght", 400.0)],
        "Lora variable, wght 400",
        34.0,
    ),
    (
        "/usr/share/fonts/truetype/google-fonts/Lora-Variable.ttf",
        &[("wght", 700.0)],
        "Lora variable, wght 700",
        34.0,
    ),
];

fn main() {
    let out_path = std::env::args().nth(1).unwrap_or("shaping.png".into());
    let (w, h) = (1100u32, 560u32);
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
    let mut config = AtlasConfig::default();
    if let Ok(l) = std::env::var("MIN_LEVEL") {
        config.min_level = l.parse().unwrap();
    }
    let mut atlas = GlyphAtlas::new(&device, config).expect("atlas configuration");
    let mut renderer = TextRenderer::new(&device, format);
    let mut shaper = Shaper::new();
    let mut data_cache: Vec<(String, Arc<Vec<u8>>)> = Vec::new();
    let mut fonts = Vec::new();
    for (i, (path, vars, text, size)) in LINES.iter().enumerate() {
        let data = match data_cache.iter().find(|(p, _)| p == path) {
            Some((_, d)) => d.clone(),
            None => {
                if let Ok(d) = std::fs::read(path) {
                    let d = Arc::new(d);
                    data_cache.push((path.to_string(), d.clone()));
                    d
                } else {
                    eprintln!("skipping {path}");
                    continue;
                }
            }
        };
        let mut font = ShapingFont::new(i as u32, data, 0).expect("font");
        if !vars.is_empty() {
            font = font.with_variations(i as u32, vars);
        }
        let shaped = shaper.shape_line(&font, text);
        println!(
            "{:2} glyphs  {}{}",
            shaped.glyphs.len(),
            if shaped.rtl { "(rtl) " } else { "" },
            text
        );
        fonts.push((font, *text, *size));
    }
    // Frames until the detail probes (on their worker thread) have reported and every glyph
    // is at its final level; a few seconds at most.
    let mut quads = Vec::new();
    for frame in 0..1000 {
        atlas.begin_frame();
        quads.clear();
        let mut y = 50.0;
        for (font, text, size) in &fonts {
            let color = [0.93, 0.92, 0.9, 1.0];
            // Right-to-left paragraphs start at the right edge.
            layout_text(
                &mut atlas,
                &mut shaper,
                font,
                text,
                &TextStyle::new(px(*size), color),
                &Units::default(),
                [40.0, y],
                (w - 80) as f32,
                Align::Start,
                None,
                &mut quads,
            );
            y += size * 1.65;
        }
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        let generated = atlas.flush(&device, &mut enc);
        queue.submit([enc.finish()]);
        if generated == 0 && atlas.probes_pending() == 0 {
            let st = atlas.stats();
            println!(
                "settled after {frame} frames: {} glyphs, {} probes, {} raised",
                st.generated, st.probes, st.probe_upgrades
            );
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
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.007,
                        g: 0.008,
                        b: 0.012,
                        a: 1.0,
                    }),
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
    let px = glyphux_gpu::read_texture_layer(&device, &queue, &target, 0);
    std::fs::write(&out_path, glyphux::io::png::encode_png(w, h, 4, &px)).unwrap();
    println!("wrote {out_path}");
}
