//! GPU vs CPU parity (skipped when no adapter is available): a few shapes on every run, and a
//! whole font on demand (`cargo test --release -p msdfr-gpu --test parity -- --ignored`;
//! `MSDFR_TEST_FONT` picks the font, default: the Inter font of the web demo).
use msdfr::io::font::{Font, FontScaling};
use msdfr::*;
use msdfr_gpu::*;

const SHAPES: &[&str] = &[
    // Square, overlapping squares, a curved blob (quadratic + cubic), a ring.
    "{ 0,0; 0,1; 1,1; 1,0; # }",
    "{ 0,0; 0,2; 2,2; 2,0; # } { 1,1; 1,3; 3,3; 3,1; # }",
    "{ 0,0; (0.5,1.5); 1,1; (1.5,1; 1.5,-0.5); 1,-0.5; # }",
    "{ -1,0; (-1,1; 1,1); 1,0; (1,-1; -1,-1); # } { -0.5,0; (-0.5,-0.5; 0.5,-0.5); 0.5,0; (0.5,0.5; -0.5,0.5); # }",
];

const DEFAULT_FONT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../examples/web-demo/assets/Inter-Regular.otf"
);

fn setup() -> Option<(wgpu::Device, wgpu::Queue)> {
    match default_device() {
        Ok((_, d, q)) => Some((d, q)),
        Err(e) => {
            eprintln!("skipping: {e}");
            None
        }
    }
}

/// Differences of one shape's GPU field from the CPU's.
#[derive(Default)]
struct Diff {
    /// Channel values differing by more than 1e-3.
    values: usize,
    /// Texels whose rendered value (median, or the single channel) differs by more than 1e-3.
    medians: usize,
    /// Non-finite GPU values.
    nan: usize,
    /// Texels (with a narrow band) whose rendered value is saturated on the wrong side of the
    /// outline according to exact coverage, on the GPU and on the CPU. The CPU's come from the
    /// overlapping contour combiner (msdfgen's semantics) inside partly overlapping contours;
    /// they don't count as GPU differences.
    gpu_wrong_side: usize,
    cpu_wrong_side: usize,
}

/// Generates `shapes` on the GPU and the CPU and compares them texel by texel.
fn compare(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    generator: &mut FieldGenerator,
    shapes: &[Shape],
    xfs: &[Transform],
    size: u32,
    kind: FieldKind,
    ec: ErrorCorrectionConfig,
    band: f64,
    sign_correction: Option<FillRule>,
) -> Vec<Diff> {
    let prepared: Vec<PreparedShape> = shapes.iter().map(PreparedShape::new).collect();
    let gpu: Vec<GpuShape> = prepared.iter().map(GpuShape::new).collect();
    let jobs: Vec<FieldJob<'_>> = (0..shapes.len())
        .map(|i| FieldJob {
            error_correction: ec,
            band,
            sign_correction,
            ..FieldJob::new(&prepared[i], &gpu[i], xfs[i], size, size, kind)
        })
        .collect();
    let batch = generator.prepare(device, queue, &jobs);
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    generator.encode(&mut enc, &batch);
    queue.submit([enc.finish()]);
    let field = read_field(device, queue, &batch);
    let cfg = GeneratorConfig::default()
        .with_threads(1)
        .with_error_correction(ec)
        .with_sign_correction(sign_correction);
    let mut diffs = Vec::new();
    for (i, layout) in batch.layouts.iter().enumerate() {
        let (s, ps, x) = (&shapes[i], &prepared[i], &xfs[i]);
        let (cpu, n) = match (kind.channels, kind.true_distance) {
            (0 | 1, _) => {
                let mut b = Bitmap::<1>::new(size, size);
                if kind.channels == 0 {
                    generate_sdf_prepared(&mut b, ps, s, x, &cfg);
                } else {
                    generate_psdf_prepared(&mut b, ps, s, x, &cfg);
                }
                (b.data().to_vec(), 1)
            }
            (_, false) => {
                let mut b = Bitmap::<3>::new(size, size);
                generate_msdf_prepared(&mut b, ps, s, x, &cfg);
                (b.data().to_vec(), 3)
            }
            _ => {
                let mut b = Bitmap::<4>::new(size, size);
                generate_mtsdf_prepared(&mut b, ps, s, x, &cfg);
                (b.data().to_vec(), 4)
            }
        };
        // Channel values may differ where two edges are exactly as close (symmetric
        // geometry): f32 can break such ties either way. That rarely changes the median
        // (only on symmetry axes, where two channels can flip together).
        let med = |v: &[f32]| v[0].min(v[1]).max(v[0].max(v[1]).min(v[2]));
        // With a narrow band, far tiles only get a sign: their values are exact once clamped
        // to [0, 1], as the atlas stores them.
        let cl = |v: f32| if band > 0.0 { v.clamp(0.0, 1.0) } else { v };
        let coverage = (band > 0.0).then(|| rasterize(s, size, size, &x.projection, FillRule::NonZero, 4));
        // Saturated on the wrong side of the outline (coverage 0 or 1 only, so exact).
        let wrong = |t: usize, m: f32| {
            coverage.as_ref().is_some_and(|c| {
                let c = c.data()[t];
                (c == 0.0 && m >= 1.0) || (c == 1.0 && m <= 0.0)
            })
        };
        let mut d = Diff::default();
        for t in 0..(size * size) as usize {
            let g = field[layout.offset as usize + t];
            let mut gv = [0f32; 4];
            for c in 0..n {
                // GPU layout: xyz = channels (x for single-channel), w = true distance.
                gv[c] = cl(if kind.channels == 0 || c == 3 { g[3] } else { g[c] });
                let cv = cl(cpu[t * n + c]);
                if !gv[c].is_finite() {
                    d.nan += 1;
                } else if (gv[c] - cv).abs() > 1e-3 && !(gv[c].abs() > 1e15 && cv.abs() > 1e15) {
                    d.values += 1;
                }
            }
            let (gm, cm) = if n >= 3 {
                let cm = med(&cpu[t * n..t * n + 3]);
                (med(&gv[..3]), cl(cm))
            } else {
                (gv[0], cl(cpu[t]))
            };
            let (gpu_wrong, cpu_wrong) = (wrong(t, gm), wrong(t, cm));
            d.gpu_wrong_side += usize::from(gpu_wrong);
            if gpu_wrong && std::env::var("DEBUG_TEXELS").is_ok() && d.gpu_wrong_side <= 6 {
                let (tx, tr) = (t as u32 % size, t as u32 / size);
                eprintln!(
                    "  shape {i} ({tx},{tr}) gpu {:?} cpu {:?}",
                    &gv[..n],
                    &cpu[t * n..t * n + n]
                );
            }
            d.cpu_wrong_side += usize::from(cpu_wrong);
            if (gm - cm).abs() > 1e-3 && !(gm.abs() > 1e15 && cm.abs() > 1e15) && !cpu_wrong {
                d.medians += 1;
            }
        }
        diffs.push(d);
    }
    diffs
}

#[test]
fn matches_cpu() {
    let Some((device, queue)) = setup() else {
        return;
    };
    let mut generator = FieldGenerator::new(&device);
    let size = 29;
    let mut shapes = Vec::new();
    for d in SHAPES {
        let mut s = Shape::from_description(d).unwrap();
        s.normalize();
        s.orient_contours(FillRule::NonZero);
        edge_coloring_simple(&mut s, DEFAULT_ANGLE_THRESHOLD, 0);
        shapes.push(s);
    }
    let xfs: Vec<Transform> = shapes
        .iter()
        .map(|s| Transform::autoframe(s.bounds(), size, size, 4.0).unwrap())
        .collect();
    for (kind, ec) in [
        (FieldKind::SDF, ErrorCorrectionConfig::DISABLED),
        (FieldKind::PSDF, ErrorCorrectionConfig::DISABLED),
        (FieldKind::MSDF, ErrorCorrectionConfig::DISABLED),
        (FieldKind::MSDF, ErrorCorrectionConfig::default()),
        (FieldKind::MTSDF, ErrorCorrectionConfig::default()),
    ] {
        let n = if kind.channels == 0 { 1 } else { kind.channels as usize };
        for (i, d) in compare(
            &device,
            &queue,
            &mut generator,
            &shapes,
            &xfs,
            size,
            kind,
            ec,
            0.0,
            None,
        )
        .iter()
        .enumerate()
        {
            let texels = (size * size) as usize;
            assert!(d.nan == 0, "{kind:?} shape {i}: {} non-finite values", d.nan);
            assert!(
                d.medians * 100 <= texels,
                "{kind:?} shape {i}: {} medians differ",
                d.medians
            );
            assert!(
                d.values * 50 <= texels * n,
                "{kind:?} shape {i}: {} values differ",
                d.values
            );
        }
    }
}

/// Every printable ASCII glyph (+ a few Latin-1 ones) of a font, all field types, error
/// correction off and on. Rendered values may differ only at exact ties between edges, which
/// f32 (and the GPU's fused multiply-adds) can break the other way: at most 1 texel in 10 000.
#[test]
#[ignore = "slow; run with --release -- --ignored"]
fn whole_font_matches_cpu() {
    let Some((device, queue)) = setup() else {
        return;
    };
    let path = std::env::var("MSDFR_TEST_FONT").unwrap_or(DEFAULT_FONT.into());
    let data = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let font = Font::from_bytes(&data, 0, FontScaling::EmNormalized).unwrap();
    let mut shapes = Vec::new();
    for c in (33u8..127).map(char::from).chain("éßøÆœ€™ﬁ".chars()) {
        if let Ok(g) = font.glyph(c) {
            let mut s = g.shape;
            if s.contours.is_empty() {
                continue;
            }
            s.normalize();
            edge_coloring_simple(&mut s, DEFAULT_ANGLE_THRESHOLD, 0);
            shapes.push(s);
        }
    }
    let mut generator = FieldGenerator::new(&device);
    let mut failures = Vec::new();
    for (size, px_range) in [(32u32, 4.0), (45, 6.0)] {
        let xfs: Vec<Transform> = shapes
            .iter()
            .map(|s| Transform::autoframe(s.bounds(), size, size, px_range).unwrap())
            .collect();
        for kind in [FieldKind::SDF, FieldKind::PSDF, FieldKind::MSDF, FieldKind::MTSDF] {
            for (ec_name, ec) in [
                ("off", ErrorCorrectionConfig::DISABLED),
                ("auto", ErrorCorrectionConfig::default()),
            ] {
                let diffs = compare(
                    &device,
                    &queue,
                    &mut generator,
                    &shapes,
                    &xfs,
                    size,
                    kind,
                    ec,
                    0.0,
                    None,
                );
                let sum = |f: fn(&Diff) -> usize| diffs.iter().map(f).sum::<usize>();
                let (values, medians, nan) = (sum(|d| d.values), sum(|d| d.medians), sum(|d| d.nan));
                println!("{size}/{px_range} {kind:?} ec {ec_name}: values {values} medians {medians} nan {nan}");
                let texels = shapes.len() * (size * size) as usize;
                if medians * 10_000 > texels || nan > 0 {
                    failures.push(format!(
                        "{size}/{px_range} {kind:?} ec {ec_name}: {medians} medians, {nan} nan"
                    ));
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} glyphs of {path}:\n{}",
        shapes.len(),
        failures.join("\n")
    );
}

/// A variable font (overlapping and self-intersecting contours) at weights 100 to 900, as the
/// atlas generates it: narrow band and scanline sign correction. Far tiles only get a sign,
/// which must be the exact one, and no saturated texel may be on the wrong side of the outline.
#[test]
#[ignore = "slow; run with --release -- --ignored"]
fn variable_font_weights_match_cpu() {
    let Some((device, queue)) = setup() else {
        return;
    };
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/web-demo/assets/InterVariable-Latin.ttf"
    );
    let data = std::fs::read(path).unwrap();
    let mut generator = FieldGenerator::new(&device);
    let (size, px_range) = (64u32, 4.0);
    let mut failures = Vec::new();
    for wght in [100.0, 400.0, 700.0, 800.0, 900.0] {
        let mut font = Font::from_bytes(&data, 0, FontScaling::EmNormalized).unwrap();
        font.set_variation("wght", wght);
        let mut shapes = Vec::new();
        let mut chars = Vec::new();
        for c in (33u8..127).map(char::from) {
            if let Ok(g) = font.glyph(c) {
                let mut s = g.shape;
                if s.contours.is_empty() {
                    continue;
                }
                s.normalize();
                s.orient_contours(FillRule::NonZero);
                edge_coloring_simple(&mut s, DEFAULT_ANGLE_THRESHOLD, 0);
                shapes.push(s);
                chars.push(c);
            }
        }
        let xfs: Vec<Transform> = shapes
            .iter()
            .map(|s| Transform::autoframe(s.bounds(), size, size, px_range).unwrap())
            .collect();
        let band = 0.5 * px_range + 2.0;
        let diffs = compare(
            &device,
            &queue,
            &mut generator,
            &shapes,
            &xfs,
            size,
            FieldKind::MTSDF,
            ErrorCorrectionConfig::default(),
            band,
            Some(FillRule::NonZero),
        );
        for (c, d) in chars.iter().zip(&diffs) {
            if d.cpu_wrong_side > 0 {
                println!(
                    "wght {wght} {c:?}: CPU on the wrong side at {} texels",
                    d.cpu_wrong_side
                );
            }
            if d.medians > 2 || d.nan > 0 || d.gpu_wrong_side > 0 {
                failures.push(format!(
                    "wght {wght} {c:?}: {} medians, {} values differ, GPU on the wrong side at {} texels",
                    d.medians, d.values, d.gpu_wrong_side
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{}",
        failures.join(
            "
"
        )
    );
}
