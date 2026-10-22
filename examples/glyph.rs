//! Generates an MSDF for one glyph (the README example).
//! usage: glyph <font.ttf> <char> [out.png]
use msdfr::io::font::{Font, FontScaling};
use msdfr::{
    Bitmap, ColoringStrategy, DEFAULT_ANGLE_THRESHOLD, FillRule, GeneratorConfig, Projection, Range, Transform, Vec2,
    edge_coloring, generate_msdf, io,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let data = std::fs::read(&args[1])?;
    let c = args[2].chars().next().unwrap();
    let font = Font::from_bytes(&data, 0, FontScaling::EmNormalized)?;
    let mut shape = font.glyph(c)?.shape;

    shape.normalize();
    shape.orient_contours(FillRule::NonZero);
    edge_coloring(&mut shape, ColoringStrategy::InkTrap, DEFAULT_ANGLE_THRESHOLD, 0);

    // 32x32 texels, shape units (ems) -> texels: (p + translate) * scale, 0.125 em range.
    let transform = Transform::new(
        Projection::uniform(32.0, Vec2::new(0.125, 0.125)),
        Range::symmetric(0.125),
    );
    let mut msdf = Bitmap::<3>::new(32, 32);
    generate_msdf(&mut msdf, &shape, &transform, &GeneratorConfig::default());
    io::save_png(&msdf, args.get(3).map_or("glyph.png", String::as_str))?;
    Ok(())
}
