//! Offline COLR atlas baking, with CPU-only shaping and optional preview rendering.

use std::{collections::BTreeSet, path::PathBuf, sync::Arc, time::Instant};

use clap::{ArgGroup, Args};
use glyphux::color::{ColorField, ColorFieldConfig, source_over, srgb_to_linear};
use glyphux::io::font::{Font, FontScaling};

use crate::common::parse_font_var;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Debug, Args)]
#[command(group(ArgGroup::new("glyph-selection").required(true).args(["text", "glyph_index", "allglyphs"])))]
pub struct ColorArgs {
    /// Local font containing COLRv0/v1 artwork (bitmap-only and SVG fonts are unsupported)
    #[arg(long, value_name = "FILE", help_heading = "Font")]
    font: PathBuf,
    /// Face index in a font collection
    #[arg(long, default_value_t = 0, help_heading = "Font")]
    font_index: u32,
    /// Variable font axis, e.g. wght=700 (repeatable)
    #[arg(long, value_name = "AXIS=VALUE", value_parser = parse_font_var, help_heading = "Font")]
    fontvar: Vec<(String, f32)>,
    /// CPAL palette index
    #[arg(long, default_value_t = 0, help_heading = "Font")]
    palette: u16,
    /// Color for COLR foreground references, as RRGGBB or RRGGBBAA (sRGB)
    #[arg(long, default_value = "000000ff", value_parser = parse_rgba, help_heading = "Font")]
    foreground: [u8; 4],

    /// Emoji or sequence that shapes to one glyph (repeat for multiple emoji)
    #[arg(long, value_name = "EMOJI", help_heading = "Glyph selection")]
    text: Vec<String>,
    /// Glyph ID to bake instead of shaping text (repeatable)
    #[arg(long, value_name = "ID", help_heading = "Glyph selection")]
    glyph_index: Vec<u16>,
    /// Bake every glyph containing COLR artwork
    #[arg(long, help_heading = "Glyph selection")]
    allglyphs: bool,

    /// CPU preprocessing resolution
    #[arg(long, default_value_t = 1024, value_parser = clap::value_parser!(u32).range(16..=2048), help_heading = "Field quality")]
    resolution: u32,
    /// Field resolution (power of two, 8 to 512)
    #[arg(long, default_value_t = 128, value_parser = clap::value_parser!(u32).range(8..=512), help_heading = "Field quality")]
    base_resolution: u32,
    /// Distance band half-width in normalized glyph units, at least 1 / min-pixels
    #[arg(long, default_value_t = 0.125, help_heading = "Field quality")]
    range: f64,
    /// Smallest supported output size for this compiled field
    #[arg(long, default_value_t = 16, value_parser = clap::value_parser!(u32).range(1..=8192), help_heading = "Field quality")]
    min_pixels: u32,
    /// Maximum tile grid dimension (power of two, no larger than base-resolution)
    #[arg(long, default_value_t = 16, value_parser = clap::value_parser!(u32).range(1..=512), help_heading = "Field quality")]
    max_grid: u32,

    /// Width and height of each distance atlas page
    #[arg(long, default_value_t = 1024, value_parser = clap::value_parser!(u32).range(16..=8192), help_heading = "Atlas output")]
    page_size: u32,
    /// Maximum atlas page count
    #[arg(long, default_value_t = 6, value_parser = clap::value_parser!(u32).range(1..=256), help_heading = "Atlas output")]
    max_pages: u32,
    /// Output directory for atlas pages, binary buffers, metadata and WGSL decoder
    #[arg(
        short = 'o',
        long,
        default_value = "color-atlas",
        value_name = "DIR",
        help_heading = "Atlas output"
    )]
    output: PathBuf,

    /// Optional decoded PNG preview (requires exactly one selected glyph)
    #[arg(long, value_name = "FILE", help_heading = "Preview")]
    preview: Option<PathBuf>,
    /// Preview size in pixels (default 256), at least --min-pixels
    #[arg(long, requires = "preview", value_parser = clap::value_parser!(u32).range(1..=8192), help_heading = "Preview")]
    size: Option<u32>,
    /// Optional background, as RRGGBB or RRGGBBAA (default: transparent)
    #[arg(long, value_parser = parse_rgba, requires = "preview", help_heading = "Preview")]
    background: Option<[u8; 4]>,

    /// Print glyph ID, field sizes and elapsed time
    #[arg(long)]
    stats: bool,
}

fn parse_rgba(value: &str) -> std::result::Result<[u8; 4], String> {
    let hex = value.strip_prefix('#').unwrap_or(value);
    if !matches!(hex.len(), 6 | 8) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("expected RRGGBB or RRGGBBAA hexadecimal color".into());
    }
    let mut rgba = [255; 4];
    for (channel, pair) in rgba.iter_mut().zip(hex.as_bytes().chunks_exact(2)) {
        *channel = u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap();
    }
    Ok(rgba)
}

fn straight_srgb(color: [f32; 4]) -> [u8; 4] {
    let mut out = [0; 4];
    for channel in 0..3 {
        let linear = if color[3] > 0.0 { color[channel] / color[3] } else { 0.0 };
        out[channel] = glyphux::float_to_byte(if linear <= 0.0031308 {
            12.92 * linear
        } else {
            1.055 * linear.powf(1.0 / 2.4) - 0.055
        });
    }
    out[3] = glyphux::float_to_byte(color[3]);
    out
}

impl ColorArgs {
    pub fn run(&self) -> Result<()> {
        let start = Instant::now();
        if self.output.exists() && std::fs::read_dir(&self.output)?.next().is_some() {
            return Err("output directory must be empty".into());
        }
        if self.preview.as_ref().is_some_and(|path| path.exists()) {
            return Err("preview file already exists".into());
        }
        let size = self.size.unwrap_or(256);
        if self.preview.is_some() && size < self.min_pixels {
            return Err("--size must be at least --min-pixels".into());
        }
        let config = self.field_config();
        config.validate()?;
        let data = Arc::new(std::fs::read(&self.font).map_err(|error| {
            std::io::Error::new(error.kind(), format!("cannot read {}: {error}", self.font.display()))
        })?);
        let mut font = Font::from_bytes(&data, self.font_index, FontScaling::EmNormalized)?;
        for (axis, value) in &self.fontvar {
            if !value.is_finite() || !font.set_variation(axis, *value) {
                return Err(format!("invalid or unsupported font variation {axis}={value}").into());
            }
        }
        let ids = self.select_glyphs(&font, &data)?;
        let mut fields = Vec::new();
        let mut preview = None;
        for id in ids {
            let glyph_start = Instant::now();
            if id >= font.glyph_count() {
                return Err(format!("glyph {id} is outside the font ({} glyphs)", font.glyph_count()).into());
            }
            let artwork = font
                .color_glyph_by_index(id, self.palette, self.foreground)
                .map_err(|error| format!("glyph {id}: {error}"))?;
            let field = ColorField::build(&artwork, config).map_err(|error| format!("glyph {id}: {error}"))?;
            if self.preview.is_some() {
                preview = Some(self.render_preview(&field, size)?);
            }
            if self.stats {
                eprintln!(
                    "glyph {id}: {} paints, {} recipes, {}x{} tiles, {} atlas bytes; {:.3} s",
                    field.paints().len(),
                    field.recipes().len(),
                    field.grid(),
                    field.grid(),
                    field.atlas_bytes().len(),
                    glyph_start.elapsed().as_secs_f64()
                );
            }
            fields.push((id, field));
        }
        let atlas = glyphux_atlas::color::BakedColorAtlas::build(
            &fields,
            glyphux_atlas::color::ColorAtlasConfig {
                page_size: self.page_size,
                max_pages: self.max_pages as usize,
            },
        )?;
        atlas.write(&self.output)?;
        if let (Some(path), Some(bytes)) = (&self.preview, preview) {
            // Never let a preview overwrite one of the newly baked buffers or pages.
            let mut file = std::fs::File::create_new(path)?;
            std::io::Write::write_all(&mut file, &bytes)?;
        }
        if self.stats {
            eprintln!(
                "baked {} glyphs in {} pages to {} ({:.3} s total)",
                fields.len(),
                atlas.pages().len(),
                self.output.display(),
                start.elapsed().as_secs_f64()
            );
        }
        Ok(())
    }

    fn field_config(&self) -> ColorFieldConfig {
        ColorFieldConfig {
            resolution: self.resolution,
            base_resolution: self.base_resolution,
            range: self.range,
            min_pixels: self.min_pixels,
            max_grid: self.max_grid,
        }
    }

    fn select_glyphs(&self, font: &Font<'_>, data: &Arc<Vec<u8>>) -> Result<BTreeSet<u16>> {
        let mut ids = BTreeSet::new();
        ids.extend(self.glyph_index.iter().copied());
        if self.allglyphs {
            for id in 0..font.glyph_count() {
                if font.face().is_color_glyph(glyphux::io::font::ttf_parser::GlyphId(id)) {
                    ids.insert(id);
                }
            }
        }
        if !self.text.is_empty() {
            let blob: Arc<dyn AsRef<[u8]> + Send + Sync> = data.clone();
            let instance = harfrust::Font::new(harfrust::font::Blob::from(blob), self.font_index)
                .ok_or("cannot prepare font for shaping")?;
            let instance = instance
                .instance_builder()
                .variations(self.fontvar.iter().map(|(axis, value)| (axis.as_str(), *value)))
                .build();
            let shaper = harfrust::ShaperFont::new(&instance);
            for text in &self.text {
                let mut buffer = harfrust::Buffer::new();
                buffer.push_str(text);
                buffer.guess_segment_properties();
                harfrust::shape(&shaper, &mut buffer, harfrust::ShapeOptions::new())
                    .map_err(|error| format!("cannot shape emoji: {error:?}"))?;
                let infos = buffer.glyph_infos();
                if infos.len() != 1 || infos[0].glyph_id == 0 {
                    return Err(format!("--text {text:?} must shape to exactly one supported glyph; choose a font containing the emoji or sequence").into());
                }
                ids.insert(u16::try_from(infos[0].glyph_id)?);
            }
        }
        if ids.is_empty() {
            return Err("no COLR glyphs selected".into());
        }
        if self.preview.is_some() && ids.len() != 1 {
            return Err("--preview requires exactly one selected glyph".into());
        }
        Ok(ids)
    }

    fn render_preview(&self, field: &ColorField, size: u32) -> Result<Vec<u8>> {
        let image = field.render(size)?;
        let background = self.background.map_or([0.0; 4], |rgba| {
            let alpha = f32::from(rgba[3]) / 255.0;
            [
                srgb_to_linear(f32::from(rgba[0]) / 255.0) * alpha,
                srgb_to_linear(f32::from(rgba[1]) / 255.0) * alpha,
                srgb_to_linear(f32::from(rgba[2]) / 255.0) * alpha,
                alpha,
            ]
        });
        let pixels: Vec<u8> = image
            .data()
            .chunks_exact(4)
            .flat_map(|pixel| straight_srgb(source_over([pixel[0], pixel[1], pixel[2], pixel[3]], background)))
            .collect();
        Ok(glyphux::io::png::encode_png(size, size, 4, &pixels))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_color_encoding_unpremultiplies_in_linear_light() {
        assert_eq!(straight_srgb([0.5, 0.0, 0.0, 0.5]), [255, 0, 0, 127]);
        assert_eq!(straight_srgb([0.0; 4]), [0; 4]);
        assert_eq!(straight_srgb([srgb_to_linear(0.5), 0.0, 0.0, 1.0]), [127, 0, 0, 255]);
        assert_eq!(parse_rgba("#12345678").unwrap(), [0x12, 0x34, 0x56, 0x78]);
        assert!(parse_rgba("é12345").is_err());
    }
}
