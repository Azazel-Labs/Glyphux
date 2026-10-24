//! `Shape::resolve_overlaps`: overlapping contours become the outline of their union.

#[cfg(feature = "font")]
use glyphux::io::font::{Font, FontScaling};
use glyphux::*;

fn shape(desc: &str) -> Shape {
    let mut s = Shape::from_description(desc).unwrap();
    s.normalize();
    s.orient_contours(FillRule::NonZero);
    s
}

fn area(s: &Shape) -> f64 {
    s.contours.iter().map(Contour::signed_area).sum()
}

/// Every contour is closed: each edge starts where the previous one ends.
fn assert_closed(s: &Shape) {
    for c in &s.contours {
        for (i, e) in c.edges.iter().enumerate() {
            let prev = &c.edges[(i + c.edges.len() - 1) % c.edges.len()];
            assert!((prev.segment.end() - e.segment.start()).length() < 1e-9, "open contour");
        }
    }
}

#[test]
fn crossing_squares_become_their_union() {
    let mut s = shape("{ 0,0; 0,2; 2,2; 2,0; # } { 1,1; 1,3; 3,3; 3,1; # }");
    assert!(s.resolve_overlaps(FillRule::NonZero));
    assert_eq!(s.contours.len(), 1);
    assert_closed(&s);
    assert!((area(&s) - 7.0).abs() < 1e-9, "area {}", area(&s));
    for p in [Vec2::new(0.5, 0.5), Vec2::new(1.5, 1.5), Vec2::new(2.5, 2.5)] {
        assert!(s.contains(p, FillRule::NonZero), "{p:?}");
    }
    for p in [Vec2::new(2.5, 0.5), Vec2::new(0.5, 2.5)] {
        assert!(!s.contains(p, FillRule::NonZero), "{p:?}");
    }
}

#[test]
fn a_covered_contour_is_dropped() {
    // Both turn the same way: the small square is inside the fill (winding 2).
    let mut s = shape("{ 0,0; 0,4; 4,4; 4,0; # } { 1,1; 1,2; 2,2; 2,1; # }");
    assert!(s.resolve_overlaps(FillRule::NonZero));
    assert_eq!(s.contours.len(), 1);
    assert!((area(&s) - 16.0).abs() < 1e-9);
}

#[test]
fn holes_and_separate_contours_stay() {
    for desc in [
        // A square with a square hole.
        "{ 0,0; 0,4; 4,4; 4,0; # } { 1,1; 2,1; 2,2; 1,2; # }",
        // Two squares apart.
        "{ 0,0; 0,1; 1,1; 1,0; # } { 2,0; 2,1; 3,1; 3,0; # }",
    ] {
        let mut s = shape(desc);
        let before = s.clone();
        assert!(!s.resolve_overlaps(FillRule::NonZero), "{desc}");
        assert_eq!(s, before);
    }
}

#[test]
fn curves_crossing() {
    // A blob of quadratics crossing a square's right side twice.
    let mut s =
        shape("{ 0,0; 0,2; 2,2; 2,0; # } { 1.5,1; (1.5,2); 2.5,2; (3.5,2); 3.5,1; (3.5,0); 2.5,0; (1.5,0); # }");
    let before = area(&s);
    assert!(s.resolve_overlaps(FillRule::NonZero));
    assert_eq!(s.contours.len(), 1);
    assert_closed(&s);
    assert!(area(&s) < before, "the overlap is counted once");
    for p in [Vec2::new(1.0, 1.0), Vec2::new(1.8, 1.0), Vec2::new(3.0, 1.0)] {
        assert!(s.contains(p, FillRule::NonZero), "{p:?}");
    }
}

/// Renders the field at `up`x and counts pixels whose coverage differs from an exact
/// rasterization of `truth` by more than 0.5 (visible artifacts).
fn artifact_pixels(field: &Bitmap<4>, truth: &Shape, t: &Transform, up: u32) -> usize {
    let (w, h) = (field.width(), field.height());
    let px_range = t.range.width() * t.projection.scale.x;
    let img = render_sdf(field, w * up, h * up, px_range, t.zero_value());
    let mut proj = t.projection;
    proj.scale *= up as f64;
    let exact = rasterize(truth, w * up, h * up, &proj, FillRule::NonZero, 4);
    img.data()
        .iter()
        .zip(exact.data())
        .filter(|(a, b)| (*a - *b).abs() > 0.5)
        .count()
}

/// A variable font's glyphs, whose strokes overlap (`x`, `k`, `y`...), resolve into closed
/// contours filling the same area, and their fields render without the specks the
/// overlapping contour combiner leaves next to the crossings.
#[cfg(feature = "font")]
#[test]
fn variable_font_glyphs() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/web-demo/assets/InterVariable-Latin.ttf"
    );
    let data = std::fs::read(path).unwrap();
    let (mut resolved_total, mut overlapping_total, mut changed) = (0, 0, 0);
    for wght in [400.0, 800.0] {
        let mut font = Font::from_bytes(&data, 0, FontScaling::EmNormalized).unwrap();
        font.set_variation("wght", wght);
        for c in "xykAKMWXYZ&#$48".chars() {
            let original = shape_of(&font, c);
            let mut resolved = original.clone();
            if !resolved.resolve_overlaps(FillRule::NonZero) {
                continue;
            }
            changed += 1;
            assert_closed(&resolved);
            resolved.normalize();
            resolved.orient_contours(FillRule::NonZero);
            let t = Transform::autoframe(original.bounds(), 48, 48, 4.0).unwrap();
            for (x, y) in [(0.3, 0.4), (0.5, 0.5), (0.61, 0.27)] {
                let b = original.bounds();
                let p = Vec2::new(b.left + x * b.width(), b.bottom + y * b.height());
                assert_eq!(
                    resolved.contains(p, FillRule::NonZero),
                    original.contains(p, FillRule::NonZero),
                    "{c} at {wght}: {p:?}"
                );
            }
            let field = |s: &Shape| {
                let mut s = s.clone();
                edge_coloring_simple(&mut s, DEFAULT_ANGLE_THRESHOLD, 0);
                let mut out = Bitmap::<4>::new(48, 48);
                // As the atlas generates them: with the scanline sign correction.
                let config = GeneratorConfig::default().with_sign_correction(Some(FillRule::NonZero));
                generate_mtsdf(&mut out, &s, &t, &config);
                out
            };
            let resolved_artifacts = artifact_pixels(&field(&resolved), &original, &t, 8);
            let overlapping_artifacts = artifact_pixels(&field(&original), &original, &t, 8);
            assert!(
                resolved_artifacts <= overlapping_artifacts,
                "{c} at {wght}: {resolved_artifacts} artifacts resolved, {overlapping_artifacts} not"
            );
            resolved_total += resolved_artifacts;
            overlapping_total += overlapping_artifacts;
        }
    }
    assert!(changed > 10, "only {changed} glyphs had overlaps");
    assert!(
        resolved_total * 10 < overlapping_total,
        "{resolved_total} artifact pixels resolved, {overlapping_total} not"
    );
}

#[cfg(feature = "font")]
fn shape_of(font: &Font<'_>, c: char) -> Shape {
    let mut s = font.glyph(c).unwrap().shape;
    s.normalize();
    s.orient_contours(FillRule::NonZero);
    s
}
