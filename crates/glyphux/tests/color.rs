use glyphux::color::{
    Affine, ColorError, ColorField, ColorFieldConfig, ColorGlyph, ColorLayer, ColorLine, ColorStop, Extend, Paint,
    source_over,
};
use glyphux::{Shape, Vec2};

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Shape {
    Shape::from_description(&format!("{{ {x0},{y0}; {x1},{y0}; {x1},{y1}; {x0},{y1}; # }}")).unwrap()
}
fn glyph(paints: Vec<Paint>, layers: Vec<ColorLayer>) -> ColorGlyph {
    ColorGlyph {
        paints,
        layers,
        plane_min: Vec2::new(-1.0, -1.0),
        plane_max: Vec2::new(1.0, 1.0),
        advance: 2.0,
    }
}
fn config() -> ColorFieldConfig {
    ColorFieldConfig {
        resolution: 128,
        base_resolution: 64,
        ..ColorFieldConfig::default()
    }
}
fn line(extend: Extend) -> ColorLine {
    ColorLine {
        stops: vec![
            ColorStop {
                offset: 0.0,
                color: [1.0, 0.0, 0.0, 1.0],
            },
            ColorStop {
                offset: 1.0,
                color: [0.0, 0.0, 1.0, 0.0],
            },
        ],
        extend,
    }
}
fn close(a: [f32; 4], b: [f32; 4], tolerance: f32) {
    for i in 0..4 {
        assert!((a[i] - b[i]).abs() <= tolerance, "{a:?} != {b:?}");
    }
}

#[test]
fn analytic_paints_and_compositing() {
    let p = Paint::linear(
        Affine::IDENTITY,
        Vec2::ZERO,
        Vec2::new(1.0, 0.0),
        Vec2::new(1.0, 1.0),
        line(Extend::Pad),
    )
    .unwrap();
    close(p.evaluate(Vec2::new(0.75, 0.25)), [0.5, 0.0, 0.0, 0.5], 1e-6);
    close(line(Extend::Repeat).evaluate(-0.25), [0.25, 0.0, 0.0, 0.25], 1e-6);
    close(line(Extend::Reflect).evaluate(1.25), [0.25, 0.0, 0.0, 0.25], 1e-6);
    close(
        source_over([0.0, 0.0, 0.5, 0.5], [1.0, 0.0, 0.0, 1.0]),
        [0.5, 0.0, 0.5, 1.0],
        1e-6,
    );
    let radial = Paint::Radial {
        inverse: Affine([2.0, 0.0, 0.0, 0.0, 1.0, 0.0]),
        center: Vec2::ZERO,
        r0: 0.0,
        r1: 1.0,
        line: line(Extend::Pad),
    };
    close(radial.evaluate(Vec2::new(0.25, 0.0)), [0.5, 0.0, 0.0, 0.5], 1e-6);
    let t = Affine([2.0, 0.3, 4.0, 0.2, -3.0, 1.0]);
    let q = Vec2::new(0.4, -0.7);
    assert!((t.inverse().unwrap().apply(t.apply(q)) - q).length() < 1e-12);
}

#[test]
fn duplicate_stops_use_right_hand_color() {
    let mut l = line(Extend::Pad);
    l.stops.insert(
        1,
        ColorStop {
            offset: 0.5,
            color: [1.0, 0.0, 0.0, 1.0],
        },
    );
    l.stops.insert(
        2,
        ColorStop {
            offset: 0.5,
            color: [0.0, 1.0, 0.0, 1.0],
        },
    );
    l.validate().unwrap();
    close(l.evaluate(0.5), [0.0, 1.0, 0.0, 1.0], 0.0);
    let mut endpoint = line(Extend::Pad);
    endpoint.stops.insert(
        1,
        ColorStop {
            offset: 0.0,
            color: [0.0, 1.0, 0.0, 1.0],
        },
    );
    close(endpoint.evaluate(-0.1), [1.0, 0.0, 0.0, 1.0], 0.0);
    close(endpoint.evaluate(0.0), [0.0, 1.0, 0.0, 1.0], 0.0);
    assert!(
        Paint::linear(
            Affine::IDENTITY,
            Vec2::ZERO,
            Vec2::new(1.0, 0.0),
            Vec2::new(2.0, 0.0),
            l
        )
        .is_err()
    );
}

#[test]
fn internal_boundaries_preserve_opacity_and_outer_coverage() {
    let g = glyph(
        vec![Paint::Solid([1.0, 0.0, 0.0, 1.0]), Paint::Solid([0.0, 0.0, 1.0, 1.0])],
        vec![
            ColorLayer {
                shape: rect(-0.75, -0.75, 0.75, 0.75),
                paint: 0,
            },
            ColorLayer {
                shape: rect(0.0, -0.75, 0.75, 0.75),
                paint: 1,
            },
        ],
    );
    let f = ColorField::build(&g, config()).unwrap();
    let (c, reads) = f.sample(Vec2::ZERO, 32.0).unwrap();
    close(c, [0.5, 0.0, 0.5, 1.0], 0.02);
    assert_eq!(reads, 1);
    close(
        f.sample(Vec2::new(-0.75, 0.0), 32.0).unwrap().0,
        [0.5, 0.0, 0.0, 0.5],
        0.03,
    );
    assert!(f.sample(Vec2::ZERO, 4.0).is_err());
    assert!(f.render(64).unwrap().data().iter().all(|v| v.is_finite()));
}

#[test]
fn transparency_is_not_silhouette_coverage() {
    let g = glyph(
        vec![Paint::Solid([1.0, 0.0, 0.0, 1.0]), Paint::Solid([0.0, 0.0, 1.0, 0.5])],
        vec![
            ColorLayer {
                shape: rect(-0.8, -0.8, 0.8, 0.8),
                paint: 0,
            },
            ColorLayer {
                shape: rect(-0.5, -0.5, 0.5, 0.5),
                paint: 1,
            },
        ],
    );
    let f = ColorField::build(&g, config()).unwrap();
    assert!(f.recipes().contains(&vec![0, 1]));
    close(f.sample(Vec2::ZERO, 32.0).unwrap().0, [0.5, 0.0, 0.5, 1.0], 0.01);
    let g = glyph(
        vec![
            Paint::linear(
                Affine::IDENTITY,
                Vec2::new(-1.0, 0.0),
                Vec2::new(1.0, 0.0),
                Vec2::new(-1.0, 1.0),
                line(Extend::Pad),
            )
            .unwrap(),
        ],
        vec![ColorLayer {
            shape: rect(-0.9, -0.9, 0.9, 0.9),
            paint: 0,
        }],
    );
    let f = ColorField::build(&g, config()).unwrap();
    close(f.sample(Vec2::ZERO, 32.0).unwrap().0, [0.5, 0.0, 0.0, 0.5], 0.01);
}

#[test]
fn four_regions_use_residual_and_five_fail_explicitly() {
    let colors = [
        [1.0, 0.0, 0.0, 1.0],
        [0.0, 1.0, 0.0, 1.0],
        [0.0, 0.0, 1.0, 1.0],
        [1.0, 1.0, 1.0, 1.0],
    ];
    let g = glyph(
        colors.map(Paint::Solid).to_vec(),
        vec![
            ColorLayer {
                shape: rect(-0.8, -0.8, 0.0, 0.0),
                paint: 0,
            },
            ColorLayer {
                shape: rect(0.0, -0.8, 0.8, 0.0),
                paint: 1,
            },
            ColorLayer {
                shape: rect(-0.8, 0.0, 0.0, 0.8),
                paint: 2,
            },
            ColorLayer {
                shape: rect(0.0, 0.0, 0.8, 0.8),
                paint: 3,
            },
        ],
    );
    let f = ColorField::build(&g, config()).unwrap();
    assert!(f.tiles().iter().any(|t| t.recipes[3] != u32::MAX));
    for (p, c) in [(Vec2::new(-0.3, -0.3), colors[0]), (Vec2::new(0.3, 0.3), colors[3])] {
        close(f.sample(p, 32.0).unwrap().0, c, 0.01);
    }
    let mut many = g;
    many.paints.push(Paint::Solid([0.4, 0.4, 0.4, 1.0]));
    many.layers.push(ColorLayer {
        shape: rect(-0.01, -0.01, 0.01, 0.01),
        paint: 4,
    });
    assert!(matches!(
        ColorField::build(&many, config()),
        Err(ColorError::Overflow { .. })
    ));
}

#[test]
fn empty_glyph_is_transparent() {
    let f = ColorField::build(&glyph(Vec::new(), Vec::new()), config()).unwrap();
    assert_eq!(f.sample(Vec2::ZERO, 32.0).unwrap(), ([0.0; 4], 0));
}

#[cfg(feature = "font")]
#[test]
fn real_colr_fonts() {
    use glyphux::io::font::{Font, FontScaling};
    let font = Font::from_bytes(
        include_bytes!("assets/Noto-COLRv1-Subset.ttf"),
        0,
        FontScaling::EmNormalized,
    )
    .unwrap();
    for (c, reference) in [
        (
            '\u{1f525}',
            include_bytes!("assets/fire-prototype-32.rgba32f").as_slice(),
        ),
        (
            '\u{1f600}',
            include_bytes!("assets/grinning-prototype-32.rgba32f").as_slice(),
        ),
    ] {
        let g = font
            .color_glyph_by_index(font.glyph_index(c).unwrap(), 0, [0, 0, 0, 255])
            .unwrap();
        let f = ColorField::build(&g, ColorFieldConfig::default()).unwrap();
        let image = f.render(32).unwrap();
        assert!(image.data().chunks_exact(4).map(|p| p[3]).sum::<f32>() > 100.0);
        let expected: Vec<_> = reference
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
            .collect();
        let error: f32 = image
            .data()
            .iter()
            .zip(&expected)
            .map(|(a, b)| (a - b).abs())
            .sum::<f32>()
            / expected.len() as f32;
        assert!(error < 0.006, "prototype decoder disagreement for {c}: {error}");
    }
    let robot = font
        .color_glyph_by_index(font.glyph_index('\u{1f916}').unwrap(), 0, [0, 0, 0, 255])
        .unwrap();
    assert!(matches!(
        ColorField::build(&robot, ColorFieldConfig::default()),
        Err(ColorError::Overflow { .. })
    ));
    let font = Font::from_bytes(
        include_bytes!("assets/Twemoji-Subset.ttf"),
        0,
        FontScaling::EmNormalized,
    )
    .unwrap();
    let mut g = font
        .color_glyph_by_index(font.glyph_index('\u{1f916}').unwrap(), 0, [0, 0, 0, 255])
        .unwrap();
    assert_eq!(g.paints.len(), 10);
    // The solid-color prototype uses a fixed Twemoji frame, unlike the COLRv1
    // metrics-derived frame. Reframe this fixture before checking its documented pixel.
    let units = f64::from(font.face().units_per_em());
    let midpoint = (g.plane_min + g.plane_max) * 0.5;
    let half = (g.plane_max.x - g.plane_min.x) * 0.5;
    for edge in g
        .layers
        .iter_mut()
        .flat_map(|l| &mut l.shape.contours)
        .flat_map(|c| &mut c.edges)
    {
        let points: &mut [Vec2] = match &mut edge.segment {
            glyphux::Segment::Linear(p) => p,
            glyphux::Segment::Quadratic(p) => p,
            glyphux::Segment::Cubic(p) => p,
        };
        for p in points {
            *p = Vec2::new(
                ((midpoint.x + p.x * half) * units - 256.0) / 290.0,
                -((midpoint.y - p.y * half) * units - 192.0) / 290.0,
            );
        }
    }
    let f = ColorField::build(&g, ColorFieldConfig::default()).unwrap();
    assert_eq!(f.recipes().len(), 10);
    let (c, _) = f
        .sample(Vec2::new(2.0 * 39.5 / 128.0 - 1.0, 2.0 * 39.5 / 128.0 - 1.0), 64.0)
        .unwrap();
    close(c, [0.630989, 0.761561, 0.941144, 1.0], 0.015);
}
