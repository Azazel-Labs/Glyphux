//! Benchmark: distance fields for all printable ASCII glyphs of a font, in every mode.
//!
//! usage: cargo bench -p textra
//! Environment: `TEXTRA_BENCH_FONT` (default: the Inter font of the web demo), `TEXTRA_BENCH_SIZE`
//! (default 32), `TEXTRA_BENCH_ITERS` (default 10). Each mode runs single-threaded, then MSDF
//! once more with the glyphs distributed over all cores (the usual way to build an atlas).
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use textra::io::font::{Font, FontScaling};
use textra::{
    Bitmap, DEFAULT_ANGLE_THRESHOLD, GeneratorConfig, PreparedShape, Projection, Range, Shape, Transform, Vec2,
    edge_coloring_simple, generate_msdf_prepared, generate_mtsdf_prepared, generate_psdf_prepared,
    generate_sdf_prepared,
};

const DEFAULT_FONT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../examples/web-demo/assets/Inter-Regular.otf"
);

fn env_or<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn main() {
    let path = std::env::var("TEXTRA_BENCH_FONT").unwrap_or(DEFAULT_FONT.into());
    let size: u32 = env_or("TEXTRA_BENCH_SIZE", 32);
    let iters: usize = env_or("TEXTRA_BENCH_ITERS", 10);
    let px_range = (f64::from(size) / 8.0).max(2.0);
    let data = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let font = Font::from_bytes(&data, 0, FontScaling::EmNormalized).unwrap();
    let mut shapes: Vec<Shape> = Vec::new();
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
    let prepared: Vec<PreparedShape> = shapes.iter().map(PreparedShape::new).collect();
    let cfg = GeneratorConfig::default().with_threads(1);
    let run_one = |mode: &str, i: usize| -> f32 {
        let (ps, x) = (&prepared[i], &xfs[i]);
        match mode {
            "msdf" => {
                let mut b = Bitmap::<3>::new(size, size);
                generate_msdf_prepared(&mut b, ps, x, &cfg);
                b.pixel(size / 2, size / 2)[0]
            }
            "mtsdf" => {
                let mut b = Bitmap::<4>::new(size, size);
                generate_mtsdf_prepared(&mut b, ps, x, &cfg);
                b.pixel(size / 2, size / 2)[0]
            }
            "sdf" => {
                let mut b = Bitmap::<1>::new(size, size);
                generate_sdf_prepared(&mut b, ps, x, &cfg);
                b.pixel(size / 2, size / 2)[0]
            }
            _ => {
                let mut b = Bitmap::<1>::new(size, size);
                generate_psdf_prepared(&mut b, ps, x, &cfg);
                b.pixel(size / 2, size / 2)[0]
            }
        }
    };
    let threads = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    let runs = [("sdf", 1), ("psdf", 1), ("msdf", 1), ("mtsdf", 1), ("msdf", threads)];
    let name = std::path::Path::new(&path).file_name().unwrap().to_string_lossy();
    println!(
        "{name}: {} glyphs, {size} px, range {px_range} px, best of {iters}",
        shapes.len()
    );
    for (mode, threads) in runs {
        let mut best = f64::MAX;
        // Keeps the work observable so the optimizer cannot drop it.
        let mut checksum = 0.0f64;
        for _ in 0..iters {
            let t0 = Instant::now();
            let next = AtomicUsize::new(0);
            checksum += std::thread::scope(|sc| {
                let workers: Vec<_> = (0..threads)
                    .map(|_| {
                        sc.spawn(|| {
                            let mut acc = 0.0f64;
                            loop {
                                let i = next.fetch_add(1, Ordering::Relaxed);
                                if i >= shapes.len() {
                                    break acc;
                                }
                                acc += f64::from(run_one(mode, i));
                            }
                        })
                    })
                    .collect();
                workers.into_iter().map(|h| h.join().unwrap()).sum::<f64>()
            });
            best = best.min(t0.elapsed().as_secs_f64() * 1000.0);
        }
        println!(
            "  {mode:5} {threads:2} thread(s): {best:8.2} ms ({:6.1} us/glyph) [chk {checksum:.3}]",
            best * 1000.0 / shapes.len() as f64
        );
    }
}
