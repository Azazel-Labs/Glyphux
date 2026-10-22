//! `msdfr-atlas` - font atlas generator. Options mirror msdf-atlas-gen; its single-dash style
//! (`-font`, `-imageout`) is accepted as well.

use std::path::PathBuf;
use std::process::exit;
use std::time::Instant;

use clap::{Args, CommandFactory, FromArgMatches, Parser, ValueEnum};
use msdfr::io::image::ImageFormat;
use msdfr::{GeneratorConfig, Range};
use msdfr_atlas::{
    AtlasConfig, AtlasType, Charset, DimensionsConstraint, FontGeometry, GlyphOptions, GlyphSelection, GridConfig,
    RangeSpec, YOrigin, generate, layout,
};
use msdfr_cli::common::{Coloring, ErrorCorrection, Fill, parse_angle, parse_font_var};
use msdfr_cli::compat::normalize_args;

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum Type {
    Hardmask,
    Softmask,
    Sdf,
    Psdf,
    Msdf,
    Mtsdf,
}

impl From<Type> for AtlasType {
    fn from(t: Type) -> Self {
        match t {
            Type::Hardmask => AtlasType::HardMask,
            Type::Softmask => AtlasType::SoftMask,
            Type::Sdf => AtlasType::Sdf,
            Type::Psdf => AtlasType::Psdf,
            Type::Msdf => AtlasType::Msdf,
            Type::Mtsdf => AtlasType::Mtsdf,
        }
    }
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
enum OnOff {
    On,
    Off,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum Origin {
    Bottom,
    Top,
}

#[derive(Args, Debug)]
#[group(required = false, multiple = false)]
struct Glyphs {
    /// Charset file (msdf-atlas-gen syntax: 'A', "text", 0x41, [0x20, 0x7e]) [default: ASCII]
    #[arg(long, value_name = "FILE")]
    charset: Option<PathBuf>,
    /// Charset given inline, e.g. "[0x20, 0x7e], 0x2026"
    #[arg(long, value_name = "SPEC", allow_hyphen_values = true)]
    chars: Option<String>,
    /// File with glyph indices (charset syntax with numbers)
    #[arg(long, value_name = "FILE")]
    glyphset: Option<PathBuf>,
    /// All glyphs of the font
    #[arg(long)]
    allglyphs: bool,
}

#[derive(Args, Debug)]
#[group(required = false, multiple = false)]
struct Constraint {
    /// Power-of-two square dimensions
    #[arg(long)]
    pots: bool,
    /// Power-of-two rectangle dimensions
    #[arg(long)]
    potr: bool,
    /// Square dimensions
    #[arg(long)]
    square: bool,
    /// Square dimensions divisible by 2
    #[arg(long)]
    square2: bool,
    /// Square dimensions divisible by 4 (default)
    #[arg(long)]
    square4: bool,
    /// Any (near-square) rectangle
    #[arg(long)]
    anydims: bool,
}

/// Font atlas generator built on msdfr (msdf-atlas-gen compatible).
#[derive(Parser, Debug)]
#[command(name = "msdfr-atlas", version, about = "Font atlas generator: packs SDF, PSDF, MSDF, MTSDF or mask glyphs into one image with a JSON/CSV layout.", after_help = AFTER_HELP)]
struct Cli {
    // ---- Input
    /// Font file (TrueType / OpenType)
    #[arg(long, value_name = "FILE", help_heading = "Input")]
    font: PathBuf,
    /// Face index in a font collection
    #[arg(long, default_value_t = 0, help_heading = "Input")]
    faceindex: u32,
    #[command(flatten, next_help_heading = "Glyph selection")]
    glyphs: Glyphs,
    /// Variable font axis, e.g. wght=700 (repeatable)
    #[arg(long, value_name = "AXIS=VALUE", value_parser = parse_font_var, help_heading = "Input")]
    fontvar: Vec<(String, f32)>,
    /// Scale applied to the glyph geometry (1 = ems)
    #[arg(long, default_value_t = 1.0, help_heading = "Input")]
    fontscale: f64,

    // ---- Output
    /// Atlas type
    #[arg(long = "type", value_enum, default_value_t = Type::Msdf, help_heading = "Output")]
    atlas_type: Type,
    /// Image format (default: from the file extension, else png)
    #[arg(long, value_enum, help_heading = "Output")]
    format: Option<Format>,
    /// Atlas image file
    #[arg(long, value_name = "FILE", help_heading = "Output")]
    imageout: Option<PathBuf>,
    /// JSON layout file
    #[arg(long, value_name = "FILE", help_heading = "Output")]
    json: Option<PathBuf>,
    /// CSV layout file
    #[arg(long, value_name = "FILE", help_heading = "Output")]
    csv: Option<PathBuf>,
    /// Y direction of the layout's coordinates
    #[arg(long, value_enum, default_value_t = Origin::Bottom, help_heading = "Output")]
    yorigin: Origin,
    /// Print statistics
    #[arg(long, help_heading = "Output")]
    stats: bool,

    // ---- Atlas size
    /// Fixed atlas dimensions (the glyph size is maximized unless --size is given)
    #[arg(long, num_args = 2, value_names = ["W", "H"], help_heading = "Atlas size")]
    dimensions: Option<Vec<u32>>,
    #[command(flatten, next_help_heading = "Atlas size")]
    constraint: Constraint,
    /// Glyph size in pixels per em [default: 32]
    #[arg(long, value_name = "PX", help_heading = "Atlas size")]
    size: Option<f64>,
    /// Minimum glyph size when maximizing it
    #[arg(long, value_name = "PX", help_heading = "Atlas size")]
    minsize: Option<f64>,
    /// Distance range in pixels [default: 2]
    #[arg(long, value_name = "R", conflicts_with_all = ["emrange", "apxrange", "aemrange"], help_heading = "Atlas size")]
    pxrange: Option<f64>,
    /// Distance range in ems
    #[arg(long, value_name = "R", conflicts_with_all = ["apxrange", "aemrange"], help_heading = "Atlas size")]
    emrange: Option<f64>,
    /// Asymmetric distance range in pixels
    #[arg(long, num_args = 2, value_names = ["LO", "HI"], allow_negative_numbers = true, conflicts_with = "aemrange", help_heading = "Atlas size")]
    apxrange: Option<Vec<f64>>,
    /// Asymmetric distance range in ems
    #[arg(long, num_args = 2, value_names = ["LO", "HI"], allow_negative_numbers = true, help_heading = "Atlas size")]
    aemrange: Option<Vec<f64>>,
    /// Align glyph origins to the pixel grid
    #[arg(long, value_enum, default_value_t = OnOff::On, help_heading = "Atlas size")]
    pxalign: OnOff,
    /// Miter limit for the room reserved around sharp corners (0 disables)
    #[arg(long, default_value_t = 1.0, help_heading = "Atlas size")]
    miterlimit: f64,
    /// Padding around each glyph in pixels
    #[arg(long, default_value_t = 0.0, help_heading = "Atlas size")]
    pxpadding: f64,
    /// Padding around each glyph in ems
    #[arg(long, default_value_t = 0.0, help_heading = "Atlas size")]
    empadding: f64,
    /// Empty pixels between glyphs
    #[arg(long, default_value_t = 0, help_heading = "Atlas size")]
    spacing: u32,
    /// Lay glyphs out in a uniform grid sharing one origin
    #[arg(long, help_heading = "Atlas size")]
    uniformgrid: bool,
    /// Grid columns (implies --uniformgrid)
    #[arg(long, value_name = "N", help_heading = "Atlas size")]
    uniformcols: Option<u32>,
    /// Grid cell size in pixels (implies --uniformgrid)
    #[arg(long, num_args = 2, value_names = ["W", "H"], help_heading = "Atlas size")]
    uniformcell: Option<Vec<u32>>,

    // ---- Generation
    /// Corner angle threshold: radians, or degrees with a D suffix
    #[arg(long, value_parser = parse_angle, default_value = "3", help_heading = "Generation")]
    angle: f64,
    /// Edge coloring strategy
    #[arg(long, alias = "edgecoloring", value_enum, default_value_t = Coloring::Inktrap, help_heading = "Generation")]
    coloringstrategy: Coloring,
    /// Seed for edge coloring
    #[arg(long, default_value_t = 0, help_heading = "Generation")]
    seed: u64,
    /// MSDF error correction mode
    #[arg(long, value_enum, default_value_t = ErrorCorrection::Auto, help_heading = "Generation")]
    errorcorrection: ErrorCorrection,
    /// Disable overlapping contour support
    #[arg(long, help_heading = "Generation")]
    nooverlap: bool,
    /// Enable the scanline sign correction pass
    #[arg(long, help_heading = "Generation")]
    scanline: bool,
    /// Fill rule for the scanline pass
    #[arg(long, value_enum, default_value_t = Fill::Nonzero, help_heading = "Generation")]
    fillrule: Fill,
    /// Keep contour directions as given
    #[arg(long, help_heading = "Generation")]
    noorient: bool,
    /// Don't export kerning
    #[arg(long, help_heading = "Generation")]
    nokerning: bool,
    /// Worker threads (0 = all cores)
    #[arg(long, default_value_t = 0, help_heading = "Generation")]
    threads: usize,
}

const AFTER_HELP: &str = "msdf-atlas-gen-style single-dash options are accepted too, e.g.\n  msdfr-atlas -font font.ttf -type msdf -size 32 -pxrange 4 -imageout atlas.png -json atlas.json";

fn die(msg: impl std::fmt::Display) -> ! {
    eprintln!("error: {msg}");
    exit(1)
}

fn read(path: &PathBuf) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| die(format!("cannot read {}: {e}", path.display())))
}

fn main() {
    let cmd = Cli::command();
    let matches = cmd.clone().get_matches_from(normalize_args(&cmd, std::env::args_os()));
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());
    if cli.imageout.is_none() && cli.json.is_none() && cli.csv.is_none() {
        die("no output specified (use --imageout, --json and/or --csv)");
    }
    let t0 = Instant::now();

    let selection = if let Some(p) = &cli.glyphs.charset {
        GlyphSelection::Charset(Charset::parse(&read(p)).unwrap_or_else(|e| die(e)))
    } else if let Some(spec) = &cli.glyphs.chars {
        GlyphSelection::Charset(Charset::parse(spec).unwrap_or_else(|e| die(e)))
    } else if let Some(p) = &cli.glyphs.glyphset {
        let set = Charset::parse(&read(p)).unwrap_or_else(|e| die(e));
        GlyphSelection::Indices(
            set.iter()
                .map(|v| u16::try_from(v).unwrap_or_else(|_| die("glyph index out of range")))
                .collect(),
        )
    } else if cli.glyphs.allglyphs {
        GlyphSelection::All
    } else {
        GlyphSelection::Charset(Charset::ascii())
    };
    let opts = GlyphOptions {
        font_scale: cli.fontscale,
        coloring: cli.coloringstrategy.into(),
        angle_threshold: cli.angle,
        seed: cli.seed,
        orient: !cli.noorient,
        kerning: !cli.nokerning,
    };
    let data = std::fs::read(&cli.font).unwrap_or_else(|e| die(format!("cannot read {}: {e}", cli.font.display())));
    let font = FontGeometry::load(&data, cli.faceindex, &cli.fontvar, &selection, &opts).unwrap_or_else(|e| die(e));
    if !font.missing.is_empty() {
        eprintln!(
            "warning: {} requested character(s) are missing from the font",
            font.missing.len()
        );
    }
    if font.kerning_incomplete {
        eprintln!(
            "warning: the font's class-based kerning was skipped (more than {} glyphs)",
            msdfr_atlas::CLASS_KERNING_LIMIT
        );
    }
    let t_load = Instant::now();

    let c = &cli.constraint;
    let constraint = if c.pots {
        DimensionsConstraint::PowerOfTwoSquare
    } else if c.potr {
        DimensionsConstraint::PowerOfTwoRectangle
    } else if c.square {
        DimensionsConstraint::Square
    } else if c.square2 {
        DimensionsConstraint::EvenSquare
    } else if c.anydims {
        DimensionsConstraint::None
    } else {
        DimensionsConstraint::MultipleOfFourSquare
    };
    let range = if let Some(r) = cli.emrange {
        RangeSpec::Em(Range::symmetric(r))
    } else if let Some(a) = &cli.aemrange {
        RangeSpec::Em(Range::new(a[0], a[1]))
    } else if let Some(a) = &cli.apxrange {
        RangeSpec::Pixels(Range::new(a[0], a[1]))
    } else {
        RangeSpec::Pixels(Range::symmetric(cli.pxrange.unwrap_or(2.0)))
    };
    let grid = (cli.uniformgrid || cli.uniformcols.is_some() || cli.uniformcell.is_some()).then(|| GridConfig {
        columns: cli.uniformcols,
        cell: cli.uniformcell.as_ref().map(|c| (c[0], c[1])),
    });
    let mut generator = GeneratorConfig::default().with_overlap_support(!cli.nooverlap);
    cli.errorcorrection.apply(&mut generator.error_correction);
    if cli.scanline {
        generator.sign_correction = Some(cli.fillrule.into());
    }
    let config = AtlasConfig {
        atlas_type: cli.atlas_type.into(),
        size: cli.size,
        min_size: cli.minsize,
        dimensions: cli.dimensions.as_ref().map(|d| (d[0], d[1])),
        constraint,
        range,
        px_padding: cli.pxpadding,
        em_padding: cli.empadding,
        spacing: cli.spacing,
        px_align: cli.pxalign == OnOff::On,
        miter_limit: cli.miterlimit,
        y_origin: if cli.yorigin == Origin::Bottom {
            YOrigin::Bottom
        } else {
            YOrigin::Top
        },
        grid,
        generator,
        threads: cli.threads,
    };

    let t_layout;
    let atlas_layout;
    if let Some(out) = &cli.imageout {
        let atlas = generate(&font, &config).unwrap_or_else(|e| die(e));
        t_layout = Instant::now();
        let format: ImageFormat = cli
            .format
            .map(Into::into)
            .or_else(|| ImageFormat::from_extension(out))
            .unwrap_or(ImageFormat::Png);
        atlas
            .image
            .save(out, format)
            .unwrap_or_else(|e| die(format!("cannot write {}: {e}", out.display())));
        atlas_layout = atlas.layout;
    } else {
        atlas_layout = layout(&font, &config).unwrap_or_else(|e| die(e));
        t_layout = Instant::now();
    }
    if let Some(p) = &cli.json {
        std::fs::write(p, layout::to_json(&font, &atlas_layout, &config))
            .unwrap_or_else(|e| die(format!("cannot write {}: {e}", p.display())));
    }
    if let Some(p) = &cli.csv {
        std::fs::write(p, layout::to_csv(&font, &atlas_layout, config.y_origin))
            .unwrap_or_else(|e| die(format!("cannot write {}: {e}", p.display())));
    }
    if cli.stats {
        let ms = |a: Instant, b: Instant| (b - a).as_secs_f64() * 1000.0;
        eprintln!(
            "{} glyphs ({} missing), atlas {}x{}, {:.2} px/em, range {:.3} px; load {:.1} ms, generate {:.1} ms, total {:.1} ms",
            font.glyphs.len(),
            font.missing.len(),
            atlas_layout.width,
            atlas_layout.height,
            atlas_layout.scale,
            atlas_layout.range.width() * atlas_layout.scale,
            ms(t0, t_load),
            ms(t_load, t_layout),
            ms(t0, Instant::now())
        );
    }
}
