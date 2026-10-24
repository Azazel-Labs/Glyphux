//! Glyph atlas behavior that needs a device (skipped when no adapter is available).
use glyphux::Shape;
use glyphux_wgpu::{AtlasConfig, GlyphAtlas, GlyphKey, default_device, wgpu};

#[test]
fn pages_grow_preserve_pixels_and_respect_cap() {
    let (_, device, queue) = match default_device() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    let square = || Shape::from_description("{ 0,0; 0,1; 1,1; 1,0; # }").ok();
    for cap in [1, 2, 6] {
        let config = AtlasConfig {
            size: 64,
            layers: cap,
            min_level: 32,
            max_level: 32,
            detail_probe: false,
            ..AtlasConfig::default()
        };
        let mut atlas = GlyphAtlas::new(&device, config.clone()).unwrap();
        assert_eq!(atlas.page_count(), 0);
        assert_eq!(atlas.allocated_page_count(), 0);
        assert_eq!(atlas.texture().width(), 1);
        let mut reference = GlyphAtlas::new(&device, config).unwrap();
        let key = |glyph| GlyphKey { font: 0, glyph };
        reference.get(key(0), 64.0, square).unwrap();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        reference.flush(&device, &mut encoder);
        queue.submit([encoder.finish()]);
        let expected = glyphux_wgpu::read_texture_layer(&device, &queue, reference.texture(), 0);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        for i in 0..cap {
            let placed = atlas.get(key(i), 64.0, square).unwrap();
            assert_eq!(placed.layer, i);
            assert_eq!(atlas.page_count(), i + 1);
            // Growing after an earlier flush in this encoder must preserve that flush.
            atlas.flush(&device, &mut encoder);
            assert_eq!(atlas.allocated_page_count(), i + 1);
        }
        assert!(
            atlas.get(key(cap), 64.0, square).is_none(),
            "current-frame pages must not be evicted"
        );
        assert_eq!(atlas.page_count(), cap);
        queue.submit([encoder.finish()]);
        for page in 0..cap {
            assert_eq!(
                glyphux_wgpu::read_texture_layer(&device, &queue, atlas.texture(), page),
                expected
            );
        }
        atlas.begin_frame();
        if cap > 1 {
            atlas.get(key(0), 64.0, square).unwrap();
        }
        let next = atlas.get(key(cap), 64.0, square).unwrap();
        assert_eq!(next.layer, u32::from(cap > 1));
        assert_eq!(atlas.stats().evicted_layers, 1);
        assert_eq!(atlas.page_count(), cap);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        atlas.flush(&device, &mut encoder);
        queue.submit([encoder.finish()]);
        assert_eq!(atlas.texture().depth_or_array_layers(), cap);
    }
}

/// Effects must never be clipped by the field's range: at every size, the glyph returned for a
/// reach covers it on screen (half its range, in screen pixels), so a glow keeps its size as
/// text zooms across levels.
#[test]
fn reach_is_always_covered() {
    let device = match default_device() {
        Ok((_, d, _)) => d,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    let mut atlas = GlyphAtlas::new(&device, AtlasConfig::default()).expect("atlas configuration");
    let square = || Shape::from_description("{ 0,0; 0,0.7; 0.6,0.7; 0.6,0; # }").ok();
    let key = GlyphKey { font: 0, glyph: 1 };
    for px in (20..=300).step_by(7) {
        let px = f64::from(px);
        for reach in [0.2 * px, 0.05 * px, 6.0] {
            atlas.begin_frame();
            let g = atlas.get_with_reach(key, px, reach, square).expect("placed");
            let covered = 0.5 * f64::from(g.px_range) * px / f64::from(g.level);
            assert!(
                covered >= reach,
                "{px} px, reach {reach:.1}: level {} range {} covers only {covered:.1} px",
                g.level,
                g.px_range
            );
        }
    }
}

/// Glyphs flushed several times into one encoder before a submit come out exactly as glyphs
/// flushed and submitted one at a time (batches share the generator's buffers).
#[test]
fn several_flushes_per_submit() {
    let (device, queue) = match default_device() {
        Ok((_, d, q)) => (d, q),
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    let shapes = [
        "{ 0,0; 0,0.7; 0.6,0.7; 0.6,0; # }",
        "{ -0.4,0; (-0.4,0.4; 0.4,0.4); 0.4,0; (0.4,-0.4; -0.4,-0.4); # }",
        "{ 0,0; (0.3,0.9); 0.6,0; # }",
    ];
    let config = AtlasConfig {
        detail_probe: false,
        ..AtlasConfig::default()
    };
    let render = |one_submit: bool| {
        let mut atlas = GlyphAtlas::new(&device, config.clone()).expect("atlas configuration");
        atlas.begin_frame();
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        for (i, s) in shapes.iter().enumerate() {
            let key = GlyphKey {
                font: 0,
                glyph: i as u32,
            };
            atlas
                .get(key, 80.0, || Shape::from_description(s).ok())
                .expect("placed");
            atlas.flush(&device, &mut enc);
            if !one_submit {
                queue.submit([enc.finish()]);
                enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            }
        }
        queue.submit([enc.finish()]);
        glyphux_wgpu::read_texture_layer(&device, &queue, atlas.texture(), 0)
    };
    let separate = render(false);
    assert!(separate.iter().any(|&b| b != 0), "nothing was generated");
    assert!(
        render(true) == separate,
        "batches recorded into one submit overwrote each other"
    );
}

/// Absurd display sizes pick the top or bottom level instead of overflowing.
#[test]
fn levels_of_extreme_sizes() {
    let device = match default_device() {
        Ok((_, d, _)) => d,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    let mut atlas = GlyphAtlas::new(&device, AtlasConfig::default()).expect("atlas configuration");
    let (lo, hi) = (atlas.config().min_level, atlas.config().max_level);
    assert_eq!(atlas.level_for(f64::INFINITY), hi);
    assert_eq!(atlas.level_for(1e30), hi);
    assert_eq!(atlas.level_for(f64::NAN), lo);
    assert_eq!(atlas.level_for(-5.0), lo);
    assert_eq!(atlas.level_for(100.0), 64);
    let square = || Shape::from_description("{ 0,0; 0,1; 1,1; 1,0; # }").ok();
    atlas.begin_frame();
    for px in [f64::INFINITY, 1e30, f64::NAN] {
        atlas.get(GlyphKey { font: 0, glyph: 9 }, px, square);
    }
}

/// Unusable configurations are errors, not panics or validation failures later.
#[test]
fn invalid_configs() {
    let device = match default_device() {
        Ok((_, d, _)) => d,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    // Formats come from an enum of the supported ones; only filtering 32-bit floats depends
    // on the device.
    let f32_config = AtlasConfig {
        format: glyphux_wgpu::AtlasFormat::Rgba32Float,
        ..AtlasConfig::default()
    };
    let filterable = device.features().contains(wgpu::Features::FLOAT32_FILTERABLE);
    assert_eq!(GlyphAtlas::new(&device, f32_config).is_ok(), filterable);
    let bad = [
        AtlasConfig {
            size: 1 << 20,
            ..AtlasConfig::default()
        },
        AtlasConfig {
            min_level: 256,
            ..AtlasConfig::default()
        },
        AtlasConfig {
            layers: 0,
            ..AtlasConfig::default()
        },
    ];
    for config in bad {
        assert!(GlyphAtlas::new(&device, config).is_err());
    }
}

/// `take_written_rows` lists exactly the rows flushes generated into, and the rows read back
/// from there match the whole layer's.
#[test]
fn written_rows_cover_generated_glyphs() {
    let (_, device, queue) = match default_device() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    let square = || Shape::from_description("{ 0,0; 0,1; 1,1; 1,0; # }").ok();
    let mut atlas = GlyphAtlas::new(
        &device,
        AtlasConfig {
            size: 128,
            layers: 2,
            min_level: 32,
            max_level: 32,
            detail_probe: false,
            ..AtlasConfig::default()
        },
    )
    .unwrap();
    let key = |glyph| GlyphKey { font: 0, glyph };
    let a = atlas.get(key(0), 64.0, square).unwrap();
    let b = atlas.get(key(1), 64.0, square).unwrap();
    // Nothing is written before the flush.
    assert_eq!(atlas.take_written_rows(), vec![]);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    atlas.flush(&device, &mut encoder);
    queue.submit([encoder.finish()]);
    let written = atlas.take_written_rows();
    assert_eq!(written.len(), 1);
    let (layer, rows) = written[0].clone();
    assert_eq!(layer, a.layer);
    let size = 128.0;
    for g in [a, b] {
        assert!(rows.start as f32 <= g.uv_min[1] * size && g.uv_max[1] * size <= rows.end as f32);
    }
    let whole = glyphux_wgpu::read_texture_layer(&device, &queue, atlas.texture(), layer);
    let part = glyphux_wgpu::read_texture_rows(&device, &queue, atlas.texture(), layer, rows.clone());
    let row_bytes = 128 * 4;
    assert_eq!(
        part,
        whole[rows.start as usize * row_bytes..rows.end as usize * row_bytes]
    );
    // Cached glyphs write nothing more.
    atlas.begin_frame();
    atlas.get(key(0), 64.0, square).unwrap();
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    atlas.flush(&device, &mut encoder);
    queue.submit([encoder.finish()]);
    assert_eq!(atlas.take_written_rows(), vec![]);
}
