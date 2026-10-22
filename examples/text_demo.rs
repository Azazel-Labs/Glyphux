//! Headless render of the demo scene to PNG files (and the atlas' first layer).
//!
//! usage: text_demo <font> [width] [height] [out_prefix] [frames]
//! The other fonts in the font's folder are added as fallbacks (e.g. the web demo's assets,
//! for the CJK and other scripts in the scene); one with "Variable" in its name animates the
//! title's weight.
use std::path::Path;

use msdfr_gpu::demo::Scene;
use msdfr_gpu::{default_device, msdfr, wgpu};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let font_path = Path::new(&a[1]);
    let data = std::fs::read(font_path).expect("font");
    let w: u32 = a.get(2).map_or(1280, |s| s.parse().unwrap());
    let h: u32 = a.get(3).map_or(720, |s| s.parse().unwrap());
    let prefix = a.get(4).cloned().unwrap_or("demo".into());
    let frames: u32 = a.get(5).map_or(3, |s| s.parse().unwrap());
    let (adapter, device, queue) = default_device().expect("device");
    println!("adapter: {:?}", adapter.get_info().name);
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("target"),
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
    let mut config = Scene::default_config();
    config.narrow_band = std::env::var("NOBAND").is_err();
    let name = font_path.file_stem().unwrap().to_string_lossy();
    let mut scene = Scene::with_config(&device, format, &name, data, config).expect("scene");
    let dir = font_path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = entry.path();
        let is_font = p.extension().is_some_and(|e| e == "ttf" || e == "otf");
        if is_font && p != font_path {
            let fallback = p.file_stem().unwrap().to_string_lossy().into_owned();
            let data = std::fs::read(&p).unwrap();
            let added = if fallback.contains("Variable") {
                scene.set_weight_font(data)
            } else {
                scene.add_font(&fallback, data, 0)
            };
            if let Err(e) = added {
                eprintln!("{e}");
            }
        }
    }
    println!(
        "GPU timestamps: {}",
        if scene.enable_profiling(&device, &queue) {
            "on"
        } else {
            "unavailable"
        }
    );
    for f in 0..frames {
        let t = f as f32 * std::f32::consts::PI / (0.8 * (frames.max(2) - 1) as f32);
        let start = std::time::Instant::now();
        scene.render(&device, &queue, &view, [w, h], t);
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let ms = start.elapsed().as_secs_f64() * 1e3;
        let px = msdfr_gpu::read_texture_layer(&device, &queue, &target, 0);
        let path = format!("{prefix}_{f}.png");
        std::fs::write(&path, msdfr::io::png::encode_png(w, h, 4, &px)).unwrap();
        let st = scene.atlas.stats();
        println!(
            "{path}: {:.1} ms wall, CPU {:.2} ms, generated {} glyphs ({} total, {} texels, {} probes, {} upgrades) | GPU (previous frame) {}",
            ms,
            scene.cpu_ms,
            scene.last_generated,
            st.generated,
            st.generated_texels,
            st.probes,
            st.probe_upgrades,
            scene
                .gpu_times
                .iter()
                .map(|(l, t)| format!("{l} {t:.3} ms"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    let size = scene.atlas.config().size;
    let px = msdfr_gpu::read_texture_layer(&device, &queue, scene.atlas.texture(), 0);
    // Show the atlas' MSDF channels (alpha = true distance, made opaque).
    let rgb: Vec<u8> = px
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[0], p[1], p[2], 255])
        .collect();
    std::fs::write(
        format!("{prefix}_atlas.png"),
        msdfr::io::png::encode_png(size, size, 4, &rgb),
    )
    .unwrap();
}
