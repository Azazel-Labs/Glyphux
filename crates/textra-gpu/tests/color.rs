//! CPU/GPU agreement, filtering across tile mappings, and premultiplied blending.
#![cfg(not(target_arch = "wasm32"))]
use textra::Vec2;
use textra::color::{
    Affine, ColorField, ColorFieldConfig, ColorGlyph, ColorLayer, ColorLine, ColorStop, Extend, Paint,
};
use textra::io::font::{Font, FontScaling};
use textra_gpu::color::{ColorGlyphInstance, ColorRenderer};
use textra_gpu::{GlyphKey, default_device, read_texture_layer, wgpu};

#[test]
fn color_cpu_gpu_parity() {
    let (_, device, queue) = match default_device() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    let mut renderer = ColorRenderer::new(&device, wgpu::TextureFormat::Rgba8Unorm);
    let mut cases = Vec::new();
    for (bytes, character) in [
        (
            include_bytes!("../../textra/tests/assets/Noto-COLRv1-Subset.ttf").as_slice(),
            '\u{1f525}',
        ),
        (
            include_bytes!("../../textra/tests/assets/Twemoji-Subset.ttf").as_slice(),
            '\u{1f916}',
        ),
    ] {
        let font = Font::from_bytes(bytes, 0, FontScaling::EmNormalized).unwrap();
        let id = font.glyph_index(character).unwrap();
        let glyph = font.color_glyph_by_index(id, 0, [0, 0, 0, 255]).unwrap();
        cases.push(glyph);
    }
    let gradient = |extend| {
        Paint::linear(
            Affine::IDENTITY,
            Vec2::new(-0.123, 0.07),
            Vec2::new(0.37, 0.12),
            Vec2::new(0.15, 0.7),
            ColorLine {
                extend,
                stops: vec![
                    ColorStop {
                        offset: 0.0,
                        color: [1.0, 0.0, 0.0, 1.0],
                    },
                    ColorStop {
                        offset: 0.371,
                        color: [0.0, 1.0, 0.0, 0.4],
                    },
                    ColorStop {
                        offset: 0.371,
                        color: [0.0, 0.0, 1.0, 0.7],
                    },
                    ColorStop {
                        offset: 1.0,
                        color: [1.0, 1.0, 0.0, 0.0],
                    },
                ],
            },
        )
        .unwrap()
    };
    cases.push(ColorGlyph {
        paints: vec![
            Paint::Solid([0.2, 0.3, 0.1, 1.0]),
            gradient(Extend::Repeat),
            gradient(Extend::Reflect),
        ],
        layers: vec![
            ColorLayer {
                shape: textra::Shape::from_description("{ -0.9,-0.9; 0.9,-0.9; 0.9,0.9; -0.9,0.9; # }").unwrap(),
                paint: 0,
            },
            ColorLayer {
                shape: textra::Shape::from_description("{ -0.8,-0.8; 0,-0.8; 0,0.8; -0.8,0.8; # }").unwrap(),
                paint: 1,
            },
            ColorLayer {
                shape: textra::Shape::from_description("{ 0,-0.8; 0.8,-0.8; 0.8,0.8; 0,0.8; # }").unwrap(),
                paint: 2,
            },
        ],
        plane_min: Vec2::new(-1.0, -1.0),
        plane_max: Vec2::new(1.0, 1.0),
        advance: 2.0,
    });
    for (font_id, glyph) in cases.into_iter().enumerate() {
        let field = ColorField::build(
            &glyph,
            ColorFieldConfig {
                resolution: 256,
                ..ColorFieldConfig::default()
            },
        )
        .unwrap();
        let key = GlyphKey {
            font: font_id as u32,
            glyph: 0,
        };
        renderer.upload(&device, &queue, key, &field).unwrap();
        for (size, angle, opacity) in [(32u32, 0.0f32, 1.0f32), (64, 0.35, 0.6)] {
            let side = size * 2;
            let margin = size as f32 / 2.0;
            let pivot = size as f32;
            let instance = ColorGlyphInstance {
                key,
                rect: [margin, margin, margin + size as f32, margin + size as f32],
                transform: [angle.cos(), angle.sin(), pivot, pivot],
                opacity,
            };
            renderer
                .prepare(&device, &queue, [side as f32; 2], &[instance])
                .unwrap();
            let target = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("color parity"),
                size: wgpu::Extent3d {
                    width: side,
                    height: side,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = target.create_view(&wgpu::TextureViewDescriptor::default());
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("color parity"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: 0.1,
                                g: 0.2,
                                b: 0.3,
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
            let pixels = read_texture_layer(&device, &queue, &target, 0);
            let mut max_error = 0.0f32;
            let mut mean = 0.0;
            for y in 0..side {
                for x in 0..side {
                    let dx = f64::from(x) + 0.5 - f64::from(pivot);
                    let dy = f64::from(y) + 0.5 - f64::from(pivot);
                    let q = Vec2::new(
                        (f64::from(angle.cos()) * dx + f64::from(angle.sin()) * dy) * 2.0 / f64::from(size),
                        (-f64::from(angle.sin()) * dx + f64::from(angle.cos()) * dy) * 2.0 / f64::from(size),
                    );
                    let color = field.sample(q, f64::from(size) / 2.0).unwrap().0.map(|v| v * opacity);
                    let background = [0.1, 0.2, 0.3, 1.0];
                    for c in 0..4 {
                        let expected = color[c] + background[c] * (1.0 - color[3]);
                        let actual = f32::from(pixels[((y * side + x) * 4) as usize + c]) / 255.0;
                        let error = (expected - actual).abs();
                        max_error = max_error.max(error);
                        mean += error;
                    }
                }
            }
            mean /= (side * side * 4) as f32;
            assert!(
                max_error < 0.025 && mean < 0.004,
                "font {font_id}, size {size}, max {max_error}, mean {mean}"
            );
        }
        let invalid = ColorGlyphInstance {
            key,
            rect: [0.0, 0.0, 8.0, 8.0],
            transform: [1.0, 0.0, 0.0, 0.0],
            opacity: 1.0,
        };
        assert!(renderer.prepare(&device, &queue, [64.0; 2], &[invalid]).is_err());
        // Retain earlier glyphs so subsequent cases exercise atlas offsets and sharing.
        assert!(renderer.contains(key));
    }
    assert_eq!(renderer.page_count(), 1);
}

#[test]
fn shared_pages_grow_to_cap_and_reuse_space() {
    use textra_gpu::color::ColorAtlasConfig;
    let (_, device, queue) = match default_device() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    let font = Font::from_bytes(
        include_bytes!("../../textra/tests/assets/Noto-COLRv1-Subset.ttf"),
        0,
        FontScaling::EmNormalized,
    )
    .unwrap();
    let glyph = font
        .color_glyph_by_index(font.glyph_index('\u{1f525}').unwrap(), 0, [0, 0, 0, 255])
        .unwrap();
    let field = ColorField::build(
        &glyph,
        ColorFieldConfig {
            resolution: 256,
            ..ColorFieldConfig::default()
        },
    )
    .unwrap();
    let [w, h] = field.atlas_size();
    for cap in [1, 2] {
        let mut renderer = ColorRenderer::with_config(
            &device,
            wgpu::TextureFormat::Rgba8Unorm,
            ColorAtlasConfig {
                page_size: w.max(h),
                max_pages: cap,
            },
        )
        .unwrap();
        assert_eq!(renderer.page_count(), 0);
        let mut keys = Vec::new();
        for glyph in 0..128 {
            let key = GlyphKey { font: 0, glyph };
            if renderer.upload(&device, &queue, key, &field).is_err() {
                break;
            }
            keys.push(key);
        }
        assert!(!keys.is_empty() && keys.len() < 128);
        assert_eq!(renderer.page_count(), cap as usize);
        assert_eq!(renderer.page_keys().len(), cap as usize);
        for page in 0..renderer.page_count() {
            let bytes = read_texture_layer(&device, &queue, renderer.page_texture(page).unwrap(), 0);
            let stride = w.max(h) as usize * 4;
            for y in 0..h as usize {
                assert_eq!(
                    &bytes[y * stride..y * stride + w as usize * 4],
                    &field.atlas_bytes()[y * w as usize * 4..(y + 1) * w as usize * 4]
                );
            }
        }
        let full = GlyphKey { font: 9, glyph: 999 };
        assert!(renderer.upload(&device, &queue, full, &field).is_err());
        for &key in &keys {
            assert!(renderer.contains(key));
        }
        let first = keys[0];
        let page = renderer.glyph_page(first).unwrap();
        // Replacement can reuse its own rectangle even when all pages are full.
        renderer.upload(&device, &queue, first, &field).unwrap();
        assert_eq!(renderer.glyph_page(first), Some(page));
        renderer.remove(first);
        assert!(!renderer.contains(first));
        renderer.upload(&device, &queue, full, &field).unwrap();
        assert_eq!(renderer.glyph_page(full), Some(page));
        assert_eq!(renderer.page_count(), cap as usize);
        renderer.clear();
        assert_eq!(renderer.page_count(), 0);
        assert!(renderer.page_texture(0).is_none());
    }
}
