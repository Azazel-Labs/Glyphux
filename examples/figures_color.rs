//! Color figures use the real compiler, atlas bytes, tile descriptors and CPU decoder.
use super::{INK, MUTED, OUTLINE, Svg, encode_png};
use textra::color::{ColorField, ColorFieldConfig, TileMode};
use textra::io::font::{Font, FontScaling};

fn field(bytes: &[u8], character: char) -> ColorField {
    let font = Font::from_bytes(bytes, 0, FontScaling::EmNormalized).unwrap();
    let glyph = font
        .color_glyph_by_index(font.glyph_index(character).unwrap(), 0, [0, 0, 0, 255])
        .unwrap();
    ColorField::build(&glyph, ColorFieldConfig::default()).unwrap()
}

fn rendered(field: &ColorField, size: u32) -> Vec<u8> {
    let bitmap = field.render(size).unwrap();
    let bytes: Vec<u8> = bitmap
        .data()
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| {
            let channel = |c: f32| {
                let linear = c + 1.0 - p[3]; // Composite premultiplied color over white.
                textra::float_to_byte(if linear <= 0.0031308 {
                    12.92 * linear
                } else {
                    1.055 * linear.powf(1.0 / 2.4) - 0.055
                })
            };
            [channel(p[0]), channel(p[1]), channel(p[2])]
        })
        .collect();
    encode_png(size, size, 3, &bytes)
}

fn label(svg: &mut Svg, x: f64, y: f64, text: &str) {
    svg.text(x, y, 14.0, MUTED, "start", 400, text);
}

pub(super) fn write(out: &str) {
    let noto = include_bytes!("../crates/textra/tests/assets/Noto-COLRv1-Subset.ttf");
    let robot = field(
        include_bytes!("../crates/textra/tests/assets/Twemoji-Subset.ttf"),
        '\u{1f916}',
    );
    let fire = field(noto, '\u{1f525}');
    let grin = field(noto, '\u{1f600}');
    let mut svg = Svg::new(1000.0, 700.0);
    svg.text(
        28.0,
        38.0,
        25.0,
        INK,
        "start",
        700,
        "Color emoji: compile geometry, preserve paint",
    );
    label(
        &mut svg,
        28.0,
        65.0,
        "Actual Twemoji robot: renderer output, selected tile grid and packed distance atlas.",
    );
    svg.heading(
        28.0,
        108.0,
        "1. Import COLR layers",
        "Solid colors and supported gradients",
    );
    svg.image(28.0, 142.0, 220.0, 220.0, &rendered(&robot, 220));
    svg.heading(
        340.0,
        108.0,
        "2. Compile visible recipes",
        "A recipe is a back-to-front paint stack",
    );
    svg.image(340.0, 142.0, 220.0, 220.0, &rendered(&robot, 220));
    let grid = robot.grid();
    for i in 0..=grid {
        let d = f64::from(i) * 220.0 / f64::from(grid);
        svg.add(format!(
            r##"<path d="M {} 142 v 220 M 340 {} h 220" stroke="#007c91" stroke-width="0.7" fill="none"/>"##,
            340.0 + d,
            142.0 + d
        ));
    }
    svg.heading(
        660.0,
        108.0,
        "3. Pack glyph-local distances",
        "Before upload into a shared GPU page",
    );
    let [w, h] = robot.atlas_size();
    let rgb: Vec<u8> = robot
        .atlas_bytes()
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect();
    svg.image(660.0, 142.0, 300.0, 220.0, &encode_png(w, h, 3, &rgb));
    label(
        &mut svg,
        28.0,
        392.0,
        &format!(
            "{} paints / {} visible recipes",
            robot.paints().len(),
            robot.recipes().len()
        ),
    );
    label(
        &mut svg,
        340.0,
        392.0,
        &format!("{grid} x {grid} tiles / up to 4 recipes per tile"),
    );
    label(&mut svg, 660.0, 392.0, &format!("{w} x {h} texels / raw RGB values"));
    for (i, name) in [
        "R: local distance",
        "G: local distance",
        "B: local distance",
        "A: outer union distance",
    ]
    .iter()
    .enumerate()
    {
        let x = 28.0 + i as f64 * 245.0;
        let bytes: Vec<u8> = robot.atlas_bytes().as_chunks::<4>().0.iter().map(|p| p[i]).collect();
        svg.image(x, 425.0, 220.0, 155.0, &encode_png(w, h, 1, &bytes));
        label(&mut svg, x, 607.0, name);
    }
    label(
        &mut svg,
        28.0,
        646.0,
        "RGB stores three explicit region distances; a fourth recipe uses residual weight. Alpha stores distance, not opacity.",
    );
    label(
        &mut svg,
        28.0,
        674.0,
        "The compiler reports overflow if no allowed grid fits. These scalar region fields are decoded without an MSDF median.",
    );
    std::fs::write(format!("{out}/color-prepare.svg"), svg.finish()).unwrap();

    let mut svg = Svg::new(1000.0, 710.0);
    svg.text(
        28.0,
        38.0,
        25.0,
        INK,
        "start",
        700,
        "Color emoji: from a tile to a finished pixel",
    );
    label(
        &mut svg,
        28.0,
        65.0,
        "Real ColorField decoder output. Gradients are evaluated from paint data at the pixel position.",
    );
    for (i, (name, f)) in [
        ("Noto fire", &fire),
        ("Noto grinning face", &grin),
        ("Twemoji robot", &robot),
    ]
    .iter()
    .enumerate()
    {
        let x = 28.0 + i as f64 * 325.0;
        svg.heading(x, 108.0, name, "Same compiled field at different sizes");
        svg.image(x, 140.0, 160.0, 160.0, &rendered(f, 160));
        svg.image(x + 175.0, 236.0, 64.0, 64.0, &rendered(f, 64));
        svg.image(x + 254.0, 268.0, 32.0, 32.0, &rendered(f, 32));
        label(&mut svg, x, 328.0, "160 px");
        label(&mut svg, x + 175.0, 328.0, "64 px");
        label(&mut svg, x + 254.0, 328.0, "32 px");
    }
    let modes = [TileMode::Empty, TileMode::Interior, TileMode::Boundary];
    for (i, (title, detail)) in [
        ("Empty tile", "Transparent / 0 filtered fetches"),
        ("Interior tile", "Evaluate recipe / 0 filtered fetches"),
        ("Boundary tile", "Decode distances / 1 bilinear RGBA fetch"),
    ]
    .iter()
    .enumerate()
    {
        let x = 28.0 + i as f64 * 325.0;
        svg.heading(x, 390.0, title, detail);
        let n = robot.tiles().iter().filter(|t| t.mode == modes[i]).count();
        label(
            &mut svg,
            x,
            439.0,
            &format!("Robot: {n} of {} tiles", robot.tiles().len()),
        );
    }
    svg.add(r##"<path d="M28 467 H972" stroke="#d0d7de"/>"##);
    for (i, (title, body)) in [
        ("1. Form local weights", "Three explicit weights + optional residual"),
        (
            "2. Evaluate each recipe",
            "Composite paints back-to-front in linear light",
        ),
        ("3. Blend and cover", "Normalize weights; apply outer coverage once"),
        (
            "4. Composite the result",
            "Premultiplied source-over onto the background",
        ),
    ]
    .iter()
    .enumerate()
    {
        let y = 501.0 + i as f64 * 42.0;
        svg.text(28.0, y, 16.0, OUTLINE, "start", 600, title);
        label(&mut svg, 350.0, y, body);
    }
    label(
        &mut svg,
        28.0,
        682.0,
        "One filtered fetch is not the total shader cost: tile, recipe and paint buffers are also read. No speed claim is implied.",
    );
    std::fs::write(format!("{out}/color-render.svg"), svg.finish()).unwrap();
    shared_atlas(out, &[&fire, &grin, &robot]);
    println!("Wrote {out}/color-prepare.svg, color-render.svg and color-atlas.svg");
}

/// Capture the actual shared GPU page, with vector annotations and a pixel-exact crop.
fn shared_atlas(out: &str, fields: &[&ColorField]) {
    use textra_gpu::color::ColorRenderer;
    use textra_gpu::{GlyphKey, default_device, read_texture_layer, wgpu};
    let (_, device, queue) = default_device().expect("GPU required for shared color atlas figure");
    let mut renderer = ColorRenderer::new(&device, wgpu::TextureFormat::Rgba8UnormSrgb);
    assert_eq!(renderer.page_count(), 0);
    for (i, field) in fields.iter().enumerate() {
        renderer
            .upload(
                &device,
                &queue,
                GlyphKey {
                    font: 0,
                    glyph: i as u32,
                },
                field,
            )
            .unwrap();
    }
    assert_eq!(renderer.page_count(), 1, "these three glyphs should share one page");
    let size = renderer.atlas_config().page_size;
    let rgba = read_texture_layer(&device, &queue, renderer.page_texture(0).unwrap(), 0);
    let rgb: Vec<u8> = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect();
    let mut cw = 1;
    let mut ch = 1;
    for (i, p) in rgba.as_chunks::<4>().0.iter().enumerate() {
        if *p != [0; 4] {
            cw = cw.max(i as u32 % size + 1);
            ch = ch.max(i as u32 / size + 1);
        }
    }
    let mut crop = Vec::new();
    for y in 0..ch {
        let a = (y * size * 3) as usize;
        crop.extend_from_slice(&rgb[a..a + (cw * 3) as usize]);
    }
    let mut svg = Svg::new(1000.0, 780.0);
    svg.text(
        28.0,
        38.0,
        25.0,
        INK,
        "start",
        700,
        "Color atlas: many glyphs, shared pages",
    );
    label(
        &mut svg,
        28.0,
        65.0,
        "Actual GPU readback: fire, grinning face and robot uploaded into the same 1024 x 1024 page.",
    );
    svg.heading(
        28.0,
        108.0,
        "Compile each glyph on the CPU",
        "Distance patches + tile descriptors + paint recipes",
    );
    for (i, f) in fields.iter().enumerate() {
        svg.image(28.0 + i as f64 * 100.0, 128.0, 82.0, 82.0, &rendered(f, 82));
    }
    svg.heading(
        370.0,
        108.0,
        "Upload into one shared page",
        "Relocate tile origins; preserve patch gutters",
    );
    label(&mut svg, 370.0, 150.0, "The page stores distances, not emoji colors.");
    label(&mut svg, 370.0, 177.0, "Paints and recipes stay in per-glyph buffers.");
    label(&mut svg, 370.0, 204.0, "Each draw selects a glyph's page and metadata.");
    svg.heading(
        28.0,
        251.0,
        "Page 0: allocated",
        "Whole page, raw RGB distance channels",
    );
    svg.image(28.0, 285.0, 320.0, 320.0, &encode_png(size, size, 3, &rgb));
    svg.add(format!(
        r##"<rect x="28" y="285" width="{}" height="{}" fill="none" stroke="#ff8a00" stroke-width="2"/>"##,
        f64::from(cw) * 320.0 / f64::from(size),
        f64::from(ch) * 320.0 / f64::from(size)
    ));
    svg.heading(
        385.0,
        251.0,
        "Occupied region: enlarged",
        "Same texels read back from page 0",
    );
    svg.image(385.0, 285.0, 260.0, 320.0, &encode_png(cw, ch, 3, &crop));
    svg.heading(
        695.0,
        251.0,
        "Page 1: not allocated",
        "Created only when existing pages cannot fit",
    );
    svg.add(r##"<rect x="695" y="285" width="270" height="270" rx="8" fill="#f6f8fa" stroke="#8c959f" stroke-dasharray="8 6"/>"##);
    svg.text(830.0, 412.0, 19.0, MUTED, "middle", 600, "No texture yet");
    svg.text(830.0, 443.0, 14.0, MUTED, "middle", 400, "Default cap: 2 pages");
    label(
        &mut svg,
        28.0,
        639.0,
        "On demand: 0 pages -> 1 page -> 2 pages. Page size and cap are configurable; pages stay fixed in size.",
    );
    label(
        &mut svg,
        28.0,
        675.0,
        "Remove a glyph: reuse its rectangle. Clear the cache: release all pages. Full at the cap: return an error.",
    );
    label(
        &mut svg,
        28.0,
        711.0,
        "Monochrome uses a separate growing texture array and can recycle a page unused in the current frame.",
    );
    label(
        &mut svg,
        28.0,
        747.0,
        "Shared color storage does not yet batch draws: paint buffers and bind groups remain per glyph.",
    );
    std::fs::write(format!("{out}/color-atlas.svg"), svg.finish()).unwrap();
}
