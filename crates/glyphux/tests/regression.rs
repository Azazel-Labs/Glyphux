//! Slow regression checks over a whole font, run on demand:
//! `cargo test --release -p glyphux --test regression -- --ignored`
//!
//! `GLYPHUX_TEST_FONT` picks the font (default: the Inter font of the web demo).
use std::fmt::Write;

use glyphux::io::font::{Font, FontScaling};
use glyphux::{
    Bitmap, DEFAULT_ANGLE_THRESHOLD, DistanceCheckMode, ErrorCorrectionConfig, ErrorCorrectionMode, GeneratorConfig,
    Projection, Range, Shape, Transform, Vec2, edge_coloring_simple, generate_msdf, generate_mtsdf, generate_psdf,
    generate_sdf,
};

const DEFAULT_FONT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../examples/web-demo/assets/Inter-Regular.otf"
);
const GOLDEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fingerprint.txt");

fn font_data() -> Vec<u8> {
    let path = std::env::var("GLYPHUX_TEST_FONT").unwrap_or(DEFAULT_FONT.into());
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn ascii_shapes(font: &Font<'_>) -> Vec<Shape> {
    let mut shapes = Vec::new();
    for c in 33u8..127 {
        if let Ok(g) = font.glyph(c as char) {
            let mut s = g.shape;
            if s.contours.is_empty() {
                continue;
            }
            s.normalize();
            edge_coloring_simple(&mut s, DEFAULT_ANGLE_THRESHOLD, 0);
            shapes.push(s);
        }
    }
    shapes
}

fn fnv(h: &mut u64, data: &[f32]) {
    for v in data {
        for b in v.to_bits().to_le_bytes() {
            *h ^= b as u64;
            *h = h.wrapping_mul(0x100000001b3);
        }
    }
}

/// Hashes the exact output bits of 54 generator configurations over the font's printable ASCII
/// glyphs and compares them with `tests/fingerprint.txt`, so optimizations provably don't change
/// results. Set `GLYPHUX_BLESS=1` to rewrite the file after an intended change. Hashes can differ
/// between platforms (libm), so bless on the platform you compare on.
#[test]
#[ignore = "slow; run with --release -- --ignored"]
fn fingerprint() {
    let ecs: [(&str, ErrorCorrectionConfig); 6] = [
        ("off", ErrorCorrectionConfig::DISABLED),
        ("edge-auto", ErrorCorrectionConfig::default()),
        (
            "edge-fast",
            ErrorCorrectionConfig {
                distance_check: DistanceCheckMode::DoNotCheck,
                ..Default::default()
            },
        ),
        (
            "edge-full",
            ErrorCorrectionConfig {
                distance_check: DistanceCheckMode::AlwaysCheck,
                ..Default::default()
            },
        ),
        (
            "indisc-auto",
            ErrorCorrectionConfig {
                mode: ErrorCorrectionMode::Indiscriminate,
                ..Default::default()
            },
        ),
        (
            "edgeonly",
            ErrorCorrectionConfig {
                mode: ErrorCorrectionMode::EdgeOnly,
                ..Default::default()
            },
        ),
    ];
    let data = font_data();
    let font = Font::from_bytes(&data, 0, FontScaling::EmNormalized).unwrap();
    let shapes = ascii_shapes(&font);
    let mut out = String::new();
    for &(size, px_range) in &[(16u32, 2.0f64), (32, 4.0), (47, 8.0)] {
        let xfs: Vec<Transform> = shapes
            .iter()
            .map(|s| {
                let b = s.bounds();
                let (w, h) = (b.width(), b.height());
                let frame = size as f64 - px_range;
                let scale = frame / w.max(h);
                let tr = Vec2::new(
                    -b.left + (frame / scale - w) / 2.0 + 0.5 * px_range / scale,
                    -b.bottom + (frame / scale - h) / 2.0 + 0.5 * px_range / scale,
                );
                Transform::new(Projection::uniform(scale, tr), Range::symmetric(px_range / scale))
            })
            .collect();
        for overlap in [true, false] {
            let base = GeneratorConfig::default().with_threads(1).with_overlap_support(overlap);
            let mut hs = [0xcbf29ce484222325u64; 3];
            for (i, s) in shapes.iter().enumerate() {
                let cfg = base.with_error_correction(ErrorCorrectionConfig::DISABLED);
                let mut b = Bitmap::<1>::new(size, size);
                generate_sdf(&mut b, s, &xfs[i], &cfg);
                fnv(&mut hs[0], b.data());
                let mut b = Bitmap::<1>::new(size, size);
                generate_psdf(&mut b, s, &xfs[i], &cfg);
                fnv(&mut hs[1], b.data());
                let mut b = Bitmap::<4>::new(size, size);
                generate_mtsdf(&mut b, s, &xfs[i], &cfg);
                fnv(&mut hs[2], b.data());
            }
            for (name, h) in ["sdf", "psdf", "mtsdf-noec"].iter().zip(hs) {
                writeln!(out, "{size} ov={overlap} {name} {h:016x}").unwrap();
            }
            for (ecn, ec) in &ecs {
                let cfg = base.with_error_correction(*ec);
                let mut h = 0xcbf29ce484222325u64;
                for (i, s) in shapes.iter().enumerate() {
                    let mut b = Bitmap::<3>::new(size, size);
                    generate_msdf(&mut b, s, &xfs[i], &cfg);
                    fnv(&mut h, b.data());
                }
                writeln!(out, "{size} ov={overlap} msdf {ecn} {h:016x}").unwrap();
            }
        }
    }
    if std::env::var("GLYPHUX_BLESS").is_ok() {
        std::fs::write(GOLDEN, &out).unwrap();
        return;
    }
    let golden = std::fs::read_to_string(GOLDEN).expect("tests/fingerprint.txt missing; run once with GLYPHUX_BLESS=1");
    let changed: Vec<&str> = out.lines().filter(|l| !golden.lines().any(|g| g == *l)).collect();
    assert!(
        changed.is_empty(),
        "outputs changed (GLYPHUX_BLESS=1 to accept):\n{}",
        changed.join("\n")
    );
}
