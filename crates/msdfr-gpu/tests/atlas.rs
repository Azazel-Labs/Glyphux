//! Glyph atlas behavior that needs a device (skipped when no adapter is available).
use msdfr::Shape;
use msdfr_gpu::{AtlasConfig, GlyphAtlas, GlyphKey, default_device};

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
    let mut atlas = GlyphAtlas::new(&device, AtlasConfig::default());
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
