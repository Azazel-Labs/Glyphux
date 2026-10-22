//! End-to-end tests: generated fields must reproduce the exact shape when rendered.

use msdfr::*;

/// Renders the field at `up`x and counts pixels whose coverage differs from an exact
/// rasterization by more than 0.5 (i.e. visible artifacts).
fn artifact_pixels<const N: usize>(field: &Bitmap<N>, shape: &Shape, t: &Transform, up: u32) -> usize {
    let (w, h) = (field.width(), field.height());
    let px_range = t.range.width() * t.projection.scale.x;
    let img = render_sdf(field, w * up, h * up, px_range, t.zero_value());
    let mut proj = t.projection;
    proj.scale *= up as f64;
    let truth = rasterize(shape, w * up, h * up, &proj, FillRule::NonZero, 4);
    img.data()
        .iter()
        .zip(truth.data())
        .filter(|(a, b)| (*a - *b).abs() > 0.5)
        .count()
}

fn prepared(desc: &str) -> Shape {
    let mut s = Shape::from_description(desc).unwrap();
    s.normalize();
    s.orient_contours(FillRule::NonZero);
    edge_coloring_simple(&mut s, DEFAULT_ANGLE_THRESHOLD, 0);
    s
}

const SHAPES: &[&str] = &[
    // square
    "{ 0,0; 0,1; 1,1; 1,0; # }",
    // square with a hole (inner contour given in the same direction - orientation fixes it)
    "{ 0,0; 0,1; 1,1; 1,0; # } { 0.3,0.3; 0.3,0.7; 0.7,0.7; 0.7,0.3; # }",
    // teardrop (single cubic)
    "{ 0, 1; (+1.6, -0.8; -1.6, -0.8); # }",
    // star-ish polygon with sharp corners
    "{ 0,1; 0.2,0.3; 0.9,0.3; 0.35,-0.1; 0.6,-0.8; 0,-0.35; -0.6,-0.8; -0.35,-0.1; -0.9,0.3; -0.2,0.3; # }",
    // overlapping contours
    "{ 0,0; 0,1; 1,1; 1,0; # } { 0.5,0.5; 0.5,1.5; 1.5,1.5; 1.5,0.5; # }",
    // curvy blob with quadratic and cubic segments
    "{ 0,0; (0.5,-0.5); 1,0; (1.5,0.2; 1.3,0.9); 1,1; (0.5,1.4); 0,1; (-0.4,0.6; -0.3,0.2); # }",
];

#[test]
fn msdf_renders_shapes_without_artifacts() {
    for (i, desc) in SHAPES.iter().enumerate() {
        let shape = prepared(desc);
        for &size in &[24u32, 48] {
            let t = Transform::autoframe(shape.bounds(), size, size, 4.0).unwrap();
            let mut msdf = Bitmap::<3>::new(size, size);
            generate_msdf(&mut msdf, &shape, &t, &GeneratorConfig::default());
            let bad = artifact_pixels(&msdf, &shape, &t, 8);
            assert!(bad <= 2, "shape {i} at {size}px: {bad} artifact pixels");
            let mut sdf = Bitmap::<1>::new(size, size);
            generate_sdf(&mut sdf, &shape, &t, &GeneratorConfig::default());
            // A true SDF rounds corners, so allow a few pixels there.
            let bad_sdf = artifact_pixels(&sdf, &shape, &t, 8);
            assert!(bad_sdf < 1000, "sdf shape {i}: {bad_sdf}");
        }
    }
}

#[test]
fn mtsdf_alpha_is_true_sdf() {
    let shape = prepared(SHAPES[5]);
    let t = Transform::autoframe(shape.bounds(), 32, 32, 4.0).unwrap();
    let mut mt = Bitmap::<4>::new(32, 32);
    generate_mtsdf(&mut mt, &shape, &t, &GeneratorConfig::default());
    let mut sdf = Bitmap::<1>::new(32, 32);
    generate_sdf(&mut sdf, &shape, &t, &GeneratorConfig::default());
    for y in 0..32 {
        for x in 0..32 {
            assert_eq!(mt.pixel(x, y)[3], sdf.pixel(x, y)[0]);
        }
    }
}

#[test]
fn threading_is_deterministic() {
    let shape = prepared(SHAPES[3]);
    let t = Transform::autoframe(shape.bounds(), 200, 150, 6.0).unwrap();
    let mut a = Bitmap::<4>::new(200, 150);
    let mut b = Bitmap::<4>::new(200, 150);
    generate_mtsdf(&mut a, &shape, &t, &GeneratorConfig::default().with_threads(1));
    generate_mtsdf(&mut b, &shape, &t, &GeneratorConfig::default().with_threads(4));
    assert_eq!(a, b);
}

#[test]
fn culling_matches_brute_force() {
    // The accelerated evaluator must agree exactly with a plain scan over all edges.
    let shape = prepared(SHAPES[5]);
    let ps = PreparedShape::new(&shape);
    let t = Transform::autoframe(shape.bounds(), 40, 40, 4.0).unwrap();
    let mut sdf = Bitmap::<1>::new(40, 40);
    generate_sdf(&mut sdf, &shape, &t, &GeneratorConfig::default());
    let m = t.mapping();
    for y in 0..40u32 {
        for x in 0..40u32 {
            let p = t.projection.unproject(Vec2::new(x as f64 + 0.5, (39 - y) as f64 + 0.5));
            // brute force true distance
            let mut best = SignedDistance::INFINITE;
            for c in &shape.contours {
                for e in &c.edges {
                    let (sd, _) = e.segment.signed_distance(p);
                    if sd.closer_than(&best) {
                        best = sd;
                    }
                }
            }
            let v = m.map(best.distance) as f32;
            assert!(
                (sdf.pixel(x, y)[0] - v).abs() < 1e-6,
                "{x},{y}: {} vs {v}",
                sdf.pixel(x, y)[0]
            );
            let _ = &ps;
        }
    }
}

#[test]
fn y_down_shapes_are_upright() {
    let mut up = prepared(SHAPES[2]);
    let mut down = up.clone();
    for c in &mut down.contours {
        for e in &mut c.edges {
            for p in e.segment.control_points_mut() {
                p.y = -p.y;
            }
        }
        c.reverse();
    }
    down.inverse_y_axis = true;
    up.inverse_y_axis = false;
    let tu = Transform::autoframe(up.bounds(), 32, 32, 4.0).unwrap();
    let td = Transform::autoframe(down.bounds(), 32, 32, 4.0).unwrap();
    let mut a = Bitmap::<1>::new(32, 32);
    let mut b = Bitmap::<1>::new(32, 32);
    generate_sdf(&mut a, &up, &tu, &GeneratorConfig::default());
    generate_sdf(&mut b, &down, &td, &GeneratorConfig::default());
    for (x, y) in a.data().iter().zip(b.data()) {
        assert!((x - y).abs() < 1e-5);
    }
}

#[test]
fn svg_path_end_to_end() {
    let doc =
        r#"<svg viewBox="0 0 100 100"><path d="M10 10 H90 V90 H10 Z M30 30 V70 H70 V30 Z" fill-rule="evenodd"/></svg>"#;
    let svg = io::svg::load_svg(doc, io::svg::SvgPathSelection::All).unwrap();
    let mut shape = svg.shape;
    shape.normalize();
    edge_coloring_simple(&mut shape, DEFAULT_ANGLE_THRESHOLD, 0);
    let t = Transform::autoframe(shape.bounds(), 32, 32, 4.0).unwrap();
    let mut msdf = Bitmap::<3>::new(32, 32);
    generate_msdf(&mut msdf, &shape, &t, &GeneratorConfig::default());
    // center (in the hole) is outside, the ring is inside
    let c = msdf.pixel(16, 16);
    assert!(median(c[0], c[1], c[2]) < 0.5);
    let r = msdf.pixel(16, 5);
    assert!(median(r[0], r[1], r[2]) > 0.5);
    assert!(artifact_pixels(&msdf, &shape, &t, 8) <= 2);
}

#[test]
fn png_output_is_valid_zlib() {
    let mut b = Bitmap::<3>::new(17, 9);
    for (i, v) in b.data_mut().iter_mut().enumerate() {
        *v = (i % 13) as f32 / 12.0;
    }
    let png = io::image::encode(&b, io::ImageFormat::Png);
    assert_eq!(&png[1..4], b"PNG");
}
