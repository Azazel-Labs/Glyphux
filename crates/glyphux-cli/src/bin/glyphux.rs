//! `glyphux` - distance fields for a single shape. Options mirror msdfgen's command line;
//! msdfgen-style single-dash options (`-font`, `-pxrange`) are accepted as well.

use std::path::PathBuf;
use std::process::exit;
use std::time::Instant;

use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand, ValueEnum};
use glyphux::io::font::{Font, FontScaling};
use glyphux::io::image::{ImageFormat, save};
use glyphux::io::svg::SvgPathSelection;
use glyphux::{
    Bitmap, EdgeColor, FillRule, GeneratorConfig, PreparedShape, Projection, Range, Shape, Transform, Vec2,
    edge_coloring, generate_msdf_prepared, generate_mtsdf_prepared, generate_psdf_prepared, generate_sdf_prepared, io,
    render_sdf, render_sdf_multi,
};
use glyphux_cli::common::{Coloring, ErrorCorrection, Fill, parse_angle, parse_font_var, parse_int};
use glyphux_cli::compat::normalize_args;

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum Mode {
    /// True signed distance field
    Sdf,
    /// Signed pseudo-distance field (single channel, sharp corners)
    Psdf,
    /// Multi-channel signed distance field
    Msdf,
    /// MSDF with the true SDF in the alpha channel
    Mtsdf,
    /// Only print the shape's metrics
    Metrics,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum Format {
    Png,
    Bmp,
    Fl32,
    Rgba,
    #[value(alias = "binary")]
    Bin,
    #[value(alias = "binaryfloat")]
    Binfloat,
    #[value(alias = "txt")]
    Text,
    #[value(alias = "txtfloat")]
    Textfloat,
}

impl From<Format> for ImageFormat {
    fn from(f: Format) -> Self {
        match f {
            Format::Png => ImageFormat::Png,
            Format::Bmp => ImageFormat::Bmp,
            Format::Fl32 => ImageFormat::Fl32,
            Format::Rgba => ImageFormat::Rgba,
            Format::Bin => ImageFormat::Binary,
            Format::Binfloat => ImageFormat::BinaryFloat,
            Format::Text => ImageFormat::Text,
            Format::Textfloat => ImageFormat::TextFloat,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum SvgPaths {
    /// Union of all filled elements
    All,
    /// Only the last path (msdfgen's behavior)
    Last,
}

#[derive(Args, Debug)]
#[group(required = true, multiple = false)]
struct Input {
    /// Glyph from a font file. CHAR is a character ('A' or A), a decimal or 0x-hex Unicode
    /// value, or a glyph index prefixed by g (g36, g0x24)
    #[arg(long, num_args = 2, value_names = ["FILE", "CHAR"], allow_hyphen_values = true)]
    font: Option<Vec<String>>,
    /// SVG file
    #[arg(long, value_name = "FILE")]
    svg: Option<PathBuf>,
    /// Text shape description file
    #[arg(long, value_name = "FILE")]
    shapedesc: Option<PathBuf>,
    /// Text shape description given on the command line
    #[arg(long, value_name = "DESC", allow_hyphen_values = true)]
    defineshape: Option<String>,
    /// Read a text shape description from standard input
    #[arg(long)]
    stdin: bool,
}

/// Pure Rust signed distance field generator (msdfgen compatible).
#[derive(Parser, Debug)]
#[command(name = "glyphux", version, subcommand_negates_reqs = true, args_conflicts_with_subcommands = true, about = "Offline distance fields for shapes and color emoji atlas baking. Use 'glyphux color --help' for COLR fonts.", after_help = AFTER_HELP)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    /// Type of distance field
    #[arg(value_enum, default_value_t = Mode::Msdf)]
    mode: Mode,

    #[command(flatten)]
    input: Input,

    // ---- Output
    /// Output file
    #[arg(
        short = 'o',
        long,
        value_name = "FILE",
        default_value = "output.png",
        help_heading = "Output"
    )]
    output: PathBuf,
    /// Output format (default: from the file extension, else png)
    #[arg(long, value_enum, help_heading = "Output")]
    format: Option<Format>,
    /// Output size in pixels
    #[arg(long, alias = "size", num_args = 2, value_names = ["W", "H"], default_values_t = [64u32, 64], help_heading = "Output")]
    dimensions: Vec<u32>,
    /// Render the result to a PNG to check it
    #[arg(long, num_args = 3, value_names = ["FILE", "W", "H"], help_heading = "Output")]
    testrender: Option<Vec<String>>,
    /// Render each channel separately to a PNG
    #[arg(long, num_args = 3, value_names = ["FILE", "W", "H"], help_heading = "Output")]
    testrendermulti: Option<Vec<String>>,
    /// Save the (colored) shape description
    #[arg(long, value_name = "FILE", help_heading = "Output")]
    exportshape: Option<PathBuf>,
    /// Print bounds and framing
    #[arg(long, help_heading = "Output")]
    printmetrics: bool,
    /// Print timings
    #[arg(long, alias = "time", help_heading = "Output")]
    stats: bool,

    // ---- Placement
    /// Fit the shape into the output (for previews; not for atlases)
    #[arg(long, help_heading = "Placement")]
    autoframe: bool,
    /// Shape units to pixels scale
    #[arg(long, value_name = "S", help_heading = "Placement")]
    scale: Option<f64>,
    /// Non-uniform scale
    #[arg(long, num_args = 2, value_names = ["SX", "SY"], conflicts_with = "scale", help_heading = "Placement")]
    ascale: Option<Vec<f64>>,
    /// Translation in shape units
    #[arg(long, num_args = 2, value_names = ["X", "Y"], allow_negative_numbers = true, help_heading = "Placement")]
    translate: Option<Vec<f64>>,
    /// Translation in pixels
    #[arg(long, num_args = 2, value_names = ["X", "Y"], allow_negative_numbers = true, help_heading = "Placement")]
    pxtranslate: Option<Vec<f64>>,
    /// Distance range in shape units
    #[arg(long, alias = "unitrange", value_name = "R", conflicts_with_all = ["arange", "pxrange", "apxrange"], help_heading = "Placement")]
    range: Option<f64>,
    /// Asymmetric distance range in shape units
    #[arg(long, num_args = 2, value_names = ["LO", "HI"], allow_negative_numbers = true, conflicts_with_all = ["pxrange", "apxrange"], help_heading = "Placement")]
    arange: Option<Vec<f64>>,
    /// Distance range in pixels [default: 2]
    #[arg(long, value_name = "R", conflicts_with = "apxrange", help_heading = "Placement")]
    pxrange: Option<f64>,
    /// Asymmetric distance range in pixels
    #[arg(long, num_args = 2, value_names = ["LO", "HI"], allow_negative_numbers = true, help_heading = "Placement")]
    apxrange: Option<Vec<f64>>,
    /// Invert the output's Y axis
    #[arg(long, help_heading = "Placement")]
    yflip: bool,

    // ---- Shape processing
    /// Corner angle threshold: radians, or degrees with a D suffix
    #[arg(long, value_name = "ANGLE", value_parser = parse_angle, default_value = "3", help_heading = "Shape")]
    angle: f64,
    /// Edge coloring strategy
    #[arg(long, alias = "edgecoloring", value_enum, default_value_t = Coloring::Inktrap, help_heading = "Shape")]
    coloringstrategy: Coloring,
    /// Seed for edge coloring
    #[arg(long, default_value_t = 0, help_heading = "Shape")]
    seed: u64,
    /// Keep contour directions as given (default: reorient by fill rule)
    #[arg(long, alias = "keeporder", help_heading = "Shape")]
    noorient: bool,
    /// Reverse all contours (implies --noorient)
    #[arg(long, help_heading = "Shape")]
    reverseorder: bool,
    /// Fill rule for orientation and the scanline pass
    #[arg(long, value_enum, default_value_t = Fill::Nonzero, help_heading = "Shape")]
    fillrule: Fill,
    /// Disable overlapping contour support
    #[arg(long, help_heading = "Shape")]
    nooverlap: bool,
    /// Keep overlapping contours as they are (default: replace them with the outline of
    /// their union, as variable fonts need; not with --noorient or --reverseorder)
    #[arg(long, help_heading = "Shape")]
    nopreprocess: bool,
    /// Disable the scanline sign correction pass
    #[arg(long, help_heading = "Shape")]
    noscanline: bool,
    /// Which SVG elements to use
    #[arg(long, value_enum, default_value_t = SvgPaths::All, help_heading = "Shape")]
    svgpath: SvgPaths,

    // ---- Fonts
    /// Glyph coordinates in ems
    #[arg(long, conflicts_with_all = ["noemnormalize", "legacyfontscaling"], help_heading = "Fonts")]
    emnormalize: bool,
    /// Glyph coordinates in font units
    #[arg(long, conflicts_with = "legacyfontscaling", help_heading = "Fonts")]
    noemnormalize: bool,
    /// Font units / 64 (default, msdfgen compatible)
    #[arg(long, help_heading = "Fonts")]
    legacyfontscaling: bool,
    /// Variable font axis, e.g. wght=700 (repeatable)
    #[arg(long, value_name = "AXIS=VALUE", value_parser = parse_font_var, help_heading = "Fonts")]
    fontvar: Vec<(String, f32)>,

    // ---- Error correction
    /// MSDF error correction mode
    #[arg(long, value_enum, default_value_t = ErrorCorrection::Auto, help_heading = "Error correction")]
    errorcorrection: ErrorCorrection,
    /// Minimum deviation ratio for an artifact
    #[arg(long, value_name = "R", help_heading = "Error correction")]
    errordeviationratio: Option<f64>,
    /// Minimum improvement ratio for a distance-checked correction
    #[arg(long, value_name = "R", help_heading = "Error correction")]
    errorimproveratio: Option<f64>,

    /// Worker threads (0 = automatic)
    #[arg(long, default_value_t = 0, help_heading = "Performance")]
    threads: usize,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Bake COLR emoji into distance atlas pages and offline shader buffers
    Color(glyphux_cli::color::ColorArgs),
}

const AFTER_HELP: &str = "msdfgen-style single-dash options are accepted too, e.g.\n  glyphux msdf -font font.ttf 'A' -o a.png -size 32 32 -pxrange 4 -autoframe";

fn die(msg: impl std::fmt::Display) -> ! {
    eprintln!("error: {msg}");
    exit(1)
}

fn parse_dim(v: &str, what: &str) -> u32 {
    v.parse().unwrap_or_else(|_| die(format!("invalid {what} '{v}'")))
}

fn main() {
    let cmd = Cli::command();
    let matches = cmd.clone().get_matches_from(normalize_args(&cmd, std::env::args_os()));
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());
    if let Some(Command::Color(options)) = &cli.command {
        options.run().unwrap_or_else(|error| die(error));
    } else {
        run(&cli);
    }
}

fn load_glyph(path: &PathBuf, code: &str, vars: &[(String, f32)], scaling: FontScaling) -> (Shape, f64) {
    let data = std::fs::read(path).unwrap_or_else(|e| die(format!("cannot read {}: {e}", path.display())));
    let mut font = Font::from_bytes(&data, 0, scaling).unwrap_or_else(|e| die(e));
    for (k, v) in vars {
        if !font.set_variation(k, *v) {
            die(format!("font has no variation axis '{k}'"));
        }
    }
    let glyph = if let Some(g) = code
        .strip_prefix('g')
        .filter(|s| s.starts_with(|c: char| c.is_ascii_digit()))
    {
        let id = parse_int(g).unwrap_or_else(|| die("invalid glyph index"));
        font.glyph_by_index(u16::try_from(id).unwrap_or_else(|_| die(format!("glyph index {id} out of range"))))
    } else {
        let c = if code.len() >= 3 && code.starts_with('\'') && code.ends_with('\'') {
            code[1..code.len() - 1].chars().next().unwrap()
        } else if code.chars().count() == 1 && !code.starts_with(|c: char| c.is_ascii_digit()) {
            code.chars().next().unwrap()
        } else {
            let v = parse_int(code).unwrap_or_else(|| die(format!("invalid character code '{code}'")));
            char::from_u32(v).unwrap_or_else(|| die("invalid Unicode value"))
        };
        font.glyph(c)
    }
    .unwrap_or_else(|e| die(e));
    (glyph.shape, glyph.advance)
}

fn read_text(path: &PathBuf) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| die(format!("cannot read {}: {e}", path.display())))
}

fn run(cli: &Cli) {
    let t_start = Instant::now();
    let (width, height) = (cli.dimensions[0], cli.dimensions[1]);
    let fill_rule: FillRule = cli.fillrule.into();
    let scaling = if cli.emnormalize {
        FontScaling::EmNormalized
    } else if cli.noemnormalize {
        FontScaling::None
    } else {
        FontScaling::Legacy
    };

    // Load the shape.
    let mut advance = None;
    let is_svg = cli.input.svg.is_some();
    let is_desc = cli.input.shapedesc.is_some() || cli.input.defineshape.is_some() || cli.input.stdin;
    let mut shape = if let Some(f) = &cli.input.font {
        let (s, adv) = load_glyph(&PathBuf::from(&f[0]), &f[1], &cli.fontvar, scaling);
        advance = Some(adv);
        s
    } else if let Some(p) = &cli.input.svg {
        let sel = match cli.svgpath {
            SvgPaths::All => SvgPathSelection::All,
            SvgPaths::Last => SvgPathSelection::Last,
        };
        // Contours are oriented per element; the union is non-zero.
        io::svg::load_svg(&read_text(p), sel).unwrap_or_else(|e| die(e)).shape
    } else {
        let text = if let Some(p) = &cli.input.shapedesc {
            read_text(p)
        } else if let Some(d) = &cli.input.defineshape {
            d.clone()
        } else {
            let mut t = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut t).unwrap_or_else(|e| die(e));
            t
        };
        Shape::from_description(&text).unwrap_or_else(|e| die(e))
    };
    if let Err(e) = shape.validate() {
        die(format!("invalid shape geometry: {e}"));
    }
    let skip_coloring = is_desc
        && shape
            .contours
            .iter()
            .any(|c| c.edges.iter().any(|e| e.color != EdgeColor::WHITE));
    shape.normalize();
    // Overlapping contours (variable fonts) leave specks inside the fill next to the points
    // where they cross: replace them with the outline of their union. That orients the
    // contours and resets the edge colors, so not when either is to be kept.
    let rule = if is_svg { FillRule::NonZero } else { fill_rule };
    if !cli.nopreprocess && !cli.noorient && !cli.reverseorder && !skip_coloring && shape.resolve_overlaps(rule) {
        shape.normalize();
    }
    if cli.reverseorder {
        for c in &mut shape.contours {
            c.reverse();
        }
    } else if !cli.noorient {
        shape.orient_contours(rule);
    }
    if cli.yflip {
        shape.inverse_y_axis = !shape.inverse_y_axis;
    }
    let t_loaded = Instant::now();

    // Framing.
    let range_units = match (&cli.range, &cli.arange) {
        (Some(r), _) => Some(Range::symmetric(*r)),
        (_, Some(a)) => Some(Range::new(a[0], a[1])),
        _ => None,
    };
    let range_px = match (&cli.pxrange, &cli.apxrange) {
        (Some(r), _) => Range::symmetric(*r),
        (_, Some(a)) => Range::new(a[0], a[1]),
        _ => Range::symmetric(2.0),
    };
    let scale = cli
        .scale
        .map(Vec2::splat)
        .or(cli.ascale.as_ref().map(|s| Vec2::new(s[0], s[1])));
    let bounds = shape.bounds();
    let mut transform = if cli.autoframe {
        match (scale, range_units) {
            (None, None) => {
                let mut t = Transform::autoframe(bounds, width, height, range_px.width())
                    .unwrap_or_else(|| die("cannot fit the specified pixel range"));
                t.range = range_px.px_to_units(t.projection.scale.x);
                t
            }
            (None, Some(r)) => {
                let mut b = bounds;
                b.left += r.lower;
                b.bottom += r.lower;
                b.right -= r.lower;
                b.top -= r.lower;
                let mut t = Transform::autoframe(b, width, height, 0.0).unwrap_or_else(|| die("cannot frame shape"));
                t.range = r;
                t
            }
            (Some(s), r) => {
                let r = r.unwrap_or_else(|| range_px.px_to_units(s.x.min(s.y)));
                Transform::autoframe_with_scale(bounds, width, height, s, r)
            }
        }
    } else {
        let s = scale.unwrap_or(Vec2::splat(1.0));
        let r = range_units.unwrap_or_else(|| range_px.px_to_units(s.x.min(s.y)));
        let t = cli.translate.as_ref().map_or(Vec2::ZERO, |t| Vec2::new(t[0], t[1]));
        Transform::new(Projection::new(s, t), r)
    };
    if let Some(p) = &cli.pxtranslate {
        transform.projection.translate += Vec2::new(p[0], p[1]).div_elem(transform.projection.scale);
    }

    if cli.printmetrics || cli.mode == Mode::Metrics {
        println!(
            "{}",
            if shape.inverse_y_axis {
                "Y-axis downward"
            } else {
                "Y-axis upward"
            }
        );
        if !bounds.is_empty() {
            println!(
                "bounds = {}, {}, {}, {}",
                bounds.left, bounds.bottom, bounds.right, bounds.top
            );
        }
        if let Some(adv) = advance {
            println!("advance = {adv}");
        }
        let p = transform.projection;
        println!("scale = {}, {}", p.scale.x, p.scale.y);
        println!("translate = {}, {}", p.translate.x, p.translate.y);
        println!("range {} to {}", transform.range.lower, transform.range.upper);
        println!("contours = {}, edges = {}", shape.contours.len(), shape.edge_count());
        if cli.mode == Mode::Metrics {
            return;
        }
    }

    if matches!(cli.mode, Mode::Msdf | Mode::Mtsdf) && !skip_coloring {
        edge_coloring(&mut shape, cli.coloringstrategy.into(), cli.angle, cli.seed);
    }
    if let Some(path) = &cli.exportshape {
        std::fs::write(path, shape.to_description()).unwrap_or_else(|e| die(format!("cannot write shape: {e}")));
    }
    let mut config = GeneratorConfig::default()
        .with_threads(cli.threads)
        .with_overlap_support(!cli.nooverlap);
    cli.errorcorrection.apply(&mut config.error_correction);
    if let Some(r) = cli.errordeviationratio {
        config.error_correction.min_deviation_ratio = r;
    }
    if let Some(r) = cli.errorimproveratio {
        config.error_correction.min_improve_ratio = r;
    }
    if !cli.noscanline {
        config.sign_correction = Some(fill_rule);
    }
    let t_colored = Instant::now();

    let format: ImageFormat = cli
        .format
        .map(Into::into)
        .or_else(|| ImageFormat::from_extension(&cli.output))
        .unwrap_or(ImageFormat::Png);
    let px_range = transform.range.width() * transform.projection.scale.x.min(transform.projection.scale.y);
    let zero = transform.zero_value();
    let ps = PreparedShape::new(&shape);
    let write = |r: std::io::Result<()>| r.unwrap_or_else(|e| die(format!("cannot write output: {e}")));
    let renders = |multi: bool| -> Option<(PathBuf, u32, u32)> {
        let v = if multi {
            cli.testrendermulti.as_ref()
        } else {
            cli.testrender.as_ref()
        }?;
        Some((
            PathBuf::from(&v[0]),
            parse_dim(&v[1], "render width"),
            parse_dim(&v[2], "render height"),
        ))
    };
    let t_gen;
    match cli.mode {
        Mode::Sdf | Mode::Psdf => {
            let mut bmp = Bitmap::<1>::new(width, height);
            if cli.mode == Mode::Sdf {
                generate_sdf_prepared(&mut bmp, &ps, &transform, &config);
            } else {
                generate_psdf_prepared(&mut bmp, &ps, &transform, &config);
            }
            t_gen = Instant::now();
            write(save(&bmp, &cli.output, format));
            for multi in [false, true] {
                if let Some((f, w, h)) = renders(multi) {
                    write(save(&render_sdf(&bmp, w, h, px_range, zero), f, ImageFormat::Png));
                }
            }
        }
        Mode::Msdf => {
            let mut bmp = Bitmap::<3>::new(width, height);
            generate_msdf_prepared(&mut bmp, &ps, &transform, &config);
            t_gen = Instant::now();
            write(save(&bmp, &cli.output, format));
            if let Some((f, w, h)) = renders(false) {
                write(save(&render_sdf(&bmp, w, h, px_range, zero), f, ImageFormat::Png));
            }
            if let Some((f, w, h)) = renders(true) {
                write(save(&render_sdf_multi(&bmp, w, h, px_range, zero), f, ImageFormat::Png));
            }
        }
        Mode::Mtsdf => {
            let mut bmp = Bitmap::<4>::new(width, height);
            generate_mtsdf_prepared(&mut bmp, &ps, &transform, &config);
            t_gen = Instant::now();
            write(save(&bmp, &cli.output, format));
            if let Some((f, w, h)) = renders(false) {
                write(save(&render_sdf(&bmp, w, h, px_range, zero), f, ImageFormat::Png));
            }
            if let Some((f, w, h)) = renders(true) {
                write(save(&render_sdf_multi(&bmp, w, h, px_range, zero), f, ImageFormat::Png));
            }
        }
        Mode::Metrics => unreachable!(),
    }
    if cli.stats {
        let ms = |a: Instant, b: Instant| (b - a).as_secs_f64() * 1000.0;
        eprintln!(
            "load {:.3} ms, prepare+color {:.3} ms, generate {:.3} ms, total {:.3} ms ({} edges, {}x{})",
            ms(t_start, t_loaded),
            ms(t_loaded, t_colored),
            ms(t_colored, t_gen),
            ms(t_start, Instant::now()),
            shape.edge_count(),
            width,
            height
        );
    }
}
