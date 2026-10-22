//! Renders text from fields of one fixed atlas level, magnified (artifact hunting).
//!
//! usage: glyph_zoom <font> <text> <level> <px> <out.png> [ec: auto|off|fast|full]
use std::sync::Arc;

use msdfr_gpu::text::{Align, TextStyle, layout_text};
use msdfr_gpu::units::{Units, px};
use msdfr_gpu::{AtlasConfig, GlyphAtlas, Shaper, ShapingFont, TextRenderer, default_device, msdfr, wgpu};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let level: u32 = a[3].parse().unwrap();
    let size: f32 = a[4].parse().unwrap();
    let font = ShapingFont::new(0, Arc::new(std::fs::read(&a[1]).unwrap()), 0).unwrap();
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
        Some("off") => config.error_correction = msdfr::ErrorCorrectionConfig::DISABLED,
        Some("fast") => {
            config.error_correction.distance_check = msdfr::DistanceCheckMode::DoNotCheck;
        }
        Some("full") => {
            config.error_correction.distance_check = msdfr::DistanceCheckMode::AlwaysCheck;
        }
        _ => {}
    }
    match std::env::var("COLORING").as_deref() {
        Ok("simple") => config.coloring = msdfr::ColoringStrategy::Simple,
        Ok("distance") => config.coloring = msdfr::ColoringStrategy::Distance,
        Ok("inktrap") => config.coloring = msdfr::ColoringStrategy::InkTrap,
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
    let mut atlas = GlyphAtlas::new(&device, config);
    let mut renderer = TextRenderer::new(&device, format);
    let mut quads = Vec::new();
    atlas.begin_frame();
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
    atlas.flush(&device, &mut enc);
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
    let pixels = read_rgba(&device, &queue, &target, w, h);
    std::fs::write(&a[5], msdfr::io::png::encode_png(w, h, 4, &pixels)).unwrap();
}

fn read_rgba(device: &wgpu::Device, queue: &wgpu::Queue, tex: &wgpu::Texture, w: u32, h: u32) -> Vec<u8> {
    let row = (w * 4).next_multiple_of(256);
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (row * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([enc.finish()]);
    buf.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let data = buf.slice(..).get_mapped_range().unwrap();
    (0..h)
        .flat_map(|y| data[(y * row) as usize..(y * row + w * 4) as usize].to_vec())
        .collect()
}
