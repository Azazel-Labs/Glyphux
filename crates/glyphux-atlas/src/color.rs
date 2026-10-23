//! Offline color atlas pages and the storage buffers consumed by the color shader.
//!
//! ```no_run
//! use glyphux::color::ColorField;
//! use glyphux_atlas::color::{BakedColorAtlas, ColorAtlasConfig};
//!
//! fn bake(fields: &[(u16, ColorField)]) -> Result<(), Box<dyn std::error::Error>> {
//!     let atlas = BakedColorAtlas::build(fields, ColorAtlasConfig::default())?;
//!     for glyph in atlas.glyphs() {
//!         let slice = glyph.buffers.recipes;
//!         let recipes = &atlas.buffers().recipes[slice.offset..slice.offset + slice.length];
//!         println!("glyph {}: {} recipe bytes", glyph.glyph_id, recipes.len());
//!     }
//!     atlas.write(std::path::Path::new("emoji-atlas"))?;
//!     Ok(())
//! }
//! ```

use glyphux::color::{ColorBuffers, ColorError, ColorField};
use std::{fmt, fmt::Write as _, path::Path};

/// Alignment used by every glyph's buffer slice in an offline bundle.
pub const COLOR_BUFFER_ALIGNMENT: usize = 256;

/// Limits for packing complete glyph patches into square pages.
#[derive(Clone, Copy, Debug)]
pub struct ColorAtlasConfig {
    pub page_size: u32,
    pub max_pages: usize,
}

impl Default for ColorAtlasConfig {
    fn default() -> Self {
        Self {
            page_size: 1024,
            max_pages: 6,
        }
    }
}

/// A byte range to bind for one glyph. Offsets are aligned to 256 bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufferSlice {
    pub offset: usize,
    pub length: usize,
}

/// Layout needed to draw a baked glyph without retaining its source font or field.
#[derive(Clone, Debug)]
pub struct BakedColorGlyph {
    pub glyph_id: u16,
    pub page: usize,
    /// Atlas patch in pixels: x, y, width, height.
    pub rect: [u32; 4],
    /// Left, bottom, right, top, in font units normalized to the em.
    pub plane_bounds: [f64; 4],
    pub advance: f64,
    pub grid: u32,
    pub recipe_count: usize,
    pub paint_count: usize,
    pub buffers: ColorBuffers<BufferSlice>,
}

/// Failures to pack a color atlas. No files have been written when building fails.
#[derive(Debug)]
pub enum ColorAtlasError {
    Empty,
    InvalidLimits,
    DuplicateGlyph(u16),
    GlyphTooLarge {
        glyph_id: u16,
        size: [u32; 2],
        page_size: u32,
    },
    PageLimit {
        max_pages: usize,
    },
    Encoding(ColorError),
}

impl fmt::Display for ColorAtlasError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "no color glyphs to pack"),
            Self::InvalidLimits => write!(f, "page size must be 16 to 8192 and page count must be positive"),
            Self::DuplicateGlyph(id) => write!(f, "duplicate glyph ID {id}"),
            Self::GlyphTooLarge {
                glyph_id,
                size,
                page_size,
            } => write!(
                f,
                "glyph {glyph_id} patch {}x{} exceeds page size {page_size}",
                size[0], size[1]
            ),
            Self::PageLimit { max_pages } => write!(f, "color atlas exceeds the limit of {max_pages} pages"),
            Self::Encoding(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for ColorAtlasError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Encoding(error) => Some(error),
            _ => None,
        }
    }
}

/// Complete offline atlas with immutable pages, named buffers, and typed glyph metadata.
/// Use [`Self::write`] for a portable bundle, or access the data directly for integration.
#[derive(Debug)]
pub struct BakedColorAtlas {
    page_size: u32,
    pages: Vec<Vec<u8>>,
    buffers: ColorBuffers<Vec<u8>>,
    glyphs: Vec<BakedColorGlyph>,
}

fn append(buffer: &mut Vec<u8>, bytes: &[u8]) -> BufferSlice {
    let offset = buffer.len().next_multiple_of(COLOR_BUFFER_ALIGNMENT);
    buffer.resize(offset, 0);
    buffer.extend_from_slice(bytes);
    BufferSlice {
        offset,
        length: bytes.len(),
    }
}

impl BakedColorAtlas {
    /// Pack already compiled glyphs into fixed-size pages. Glyph IDs must be unique.
    pub fn build(fields: &[(u16, ColorField)], config: ColorAtlasConfig) -> Result<Self, ColorAtlasError> {
        let ColorAtlasConfig { page_size, max_pages } = config;
        if fields.is_empty() {
            return Err(ColorAtlasError::Empty);
        }
        if !(16..=8192).contains(&page_size) || max_pages == 0 {
            return Err(ColorAtlasError::InvalidLimits);
        }
        let mut ids = std::collections::HashSet::new();
        for (id, _) in fields {
            if !ids.insert(*id) {
                return Err(ColorAtlasError::DuplicateGlyph(*id));
            }
        }
        let mut order: Vec<_> = fields.iter().collect();
        order.sort_by_key(|(id, field)| (std::cmp::Reverse(field.atlas_size()[1]), *id));
        let mut pages = Vec::<Vec<u8>>::new();
        // Shelf cursor: x, y, row height. Each glyph retains its own filtering gutters.
        let mut cursors = Vec::<[u32; 3]>::new();
        let mut buffers = ColorBuffers::<Vec<u8>>::default();
        let mut glyphs = Vec::new();
        for (id, field) in order {
            let [width, height] = field.atlas_size();
            if width > page_size || height > page_size {
                return Err(ColorAtlasError::GlyphTooLarge {
                    glyph_id: *id,
                    size: [width, height],
                    page_size,
                });
            }
            let mut placement = None;
            for (page, cursor) in cursors.iter_mut().enumerate() {
                let [mut x, mut y, mut row] = *cursor;
                if x + width > page_size {
                    x = 0;
                    y += row;
                    row = 0;
                }
                if y + height <= page_size {
                    *cursor = [x + width, y, row.max(height)];
                    placement = Some((page, x, y));
                    break;
                }
            }
            let (page, x, y) = if let Some(p) = placement {
                p
            } else {
                if pages.len() >= max_pages {
                    return Err(ColorAtlasError::PageLimit { max_pages });
                }
                let page = pages.len();
                pages.push(vec![0; page_size as usize * page_size as usize * 4]);
                cursors.push([width, 0, height]);
                (page, 0, 0)
            };
            for row in 0..height {
                let destination = (((y + row) * page_size + x) * 4) as usize;
                let source = (row * width * 4) as usize;
                pages[page][destination..destination + width as usize * 4]
                    .copy_from_slice(&field.atlas_bytes()[source..source + width as usize * 4]);
            }
            let encoded = field.encode([x, y]).map_err(ColorAtlasError::Encoding)?;
            let slices = ColorBuffers {
                tiles: append(&mut buffers.tiles, &encoded.tiles),
                paints: append(&mut buffers.paints, &encoded.paints),
                stops: append(&mut buffers.stops, &encoded.stops),
                recipes: append(&mut buffers.recipes, &encoded.recipes),
                config: append(&mut buffers.config, &encoded.config),
            };
            let (min, max) = field.plane_bounds();
            glyphs.push(BakedColorGlyph {
                glyph_id: *id,
                page,
                rect: [x, y, width, height],
                plane_bounds: [min.x, min.y, max.x, max.y],
                advance: field.advance(),
                grid: field.grid(),
                recipe_count: field.recipes().len(),
                paint_count: field.paints().len(),
                buffers: slices,
            });
        }
        glyphs.sort_by_key(|glyph| glyph.glyph_id);
        Ok(Self {
            page_size,
            pages,
            buffers,
            glyphs,
        })
    }

    /// Width and height of every square RGBA8 page.
    pub fn page_size(&self) -> u32 {
        self.page_size
    }
    /// Linear RGBA8 distance pixels, with no mipmaps or color conversion.
    pub fn pages(&self) -> &[Vec<u8>] {
        &self.pages
    }
    /// Concatenated shader bindings. Use each glyph's slices to address them.
    pub fn buffers(&self) -> &ColorBuffers<Vec<u8>> {
        &self.buffers
    }
    /// Glyphs sorted by glyph ID, independent of packing order.
    pub fn glyphs(&self) -> &[BakedColorGlyph] {
        &self.glyphs
    }

    /// Serialize the portable version 1 manifest. All paths are relative to the bundle.
    pub fn manifest(&self) -> String {
        let page_size = self.page_size;
        let mut glyphs = Vec::with_capacity(self.glyphs.len());
        for glyph in &self.glyphs {
            let BakedColorGlyph {
                glyph_id: id,
                page,
                rect: [x, y, width, height],
                plane_bounds: [left, bottom, right, top],
                advance,
                grid,
                recipe_count,
                paint_count,
                buffers,
            } = glyph;
            let mut record = format!(
                "{{\"glyphId\":{id},\"page\":{page},\"rect\":[{x},{y},{width},{height}],\"planeBounds\":[{left},{bottom},{right},{top}],\"advance\":{advance},\"grid\":{grid},\"recipeCount\":{recipe_count},\"paintCount\":{paint_count},\"buffers\":{{"
            );
            for (i, (name, slice)) in buffers.iter().enumerate() {
                if i != 0 {
                    record.push(',');
                }
                write!(
                    record,
                    "\"{name}\":{{\"offset\":{},\"length\":{}}}",
                    slice.offset, slice.length
                )
                .unwrap();
            }
            record.push_str("}}");
            glyphs.push(record);
        }
        let page_records: Vec<_> = (0..self.pages.len()).map(|i| format!("{{\"rgba\":\"atlas-{i}.rgba\",\"png\":\"atlas-{i}.png\",\"width\":{page_size},\"height\":{page_size}}}")).collect();
        format!(
            "{{\n  \"format\":\"glyphux-color-atlas\",\n  \"version\":1,\n  \"endianness\":\"little\",\n  \"textureFormat\":\"rgba8unorm\",\n  \"bufferAlignment\":256,\n  \"buffers\":{{\"tiles\":\"tiles.bin\",\"paints\":\"paints.bin\",\"stops\":\"stops.bin\",\"recipes\":\"recipes.bin\",\"config\":\"config.bin\"}},\n  \"pages\":[{}],\n  \"glyphs\":[\n    {}\n  ]\n}}\n",
            page_records.join(","),
            glyphs.join(",\n    ")
        )
    }

    /// Write a new bundle directory. Refuse nonempty destinations to avoid stale pages.
    /// The manifest is written last; it is the completion marker for the bundle.
    pub fn write(&self, directory: &Path) -> std::io::Result<()> {
        if directory.exists() && std::fs::read_dir(directory)?.next().is_some() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "output directory must be empty",
            ));
        }
        std::fs::create_dir_all(directory)?;
        for (index, pixels) in self.pages.iter().enumerate() {
            std::fs::write(directory.join(format!("atlas-{index}.rgba")), pixels)?;
            std::fs::write(
                directory.join(format!("atlas-{index}.png")),
                glyphux::io::png::encode_png(self.page_size, self.page_size, 4, pixels),
            )?;
        }
        for (name, bytes) in self.buffers.iter() {
            std::fs::write(directory.join(format!("{name}.bin")), bytes)?;
        }
        std::fs::write(directory.join("color.wgsl"), glyphux::color::COLOR_SHADER)?;
        std::fs::write(directory.join("atlas.json"), self.manifest())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glyphux::color::{ColorFieldConfig, ColorGlyph, ColorLayer, Paint};
    use glyphux::{Shape, Vec2};

    #[test]
    fn relocates_tiles_preserves_pixels_and_keeps_recipe_ids_local() {
        let glyph = ColorGlyph {
            paints: vec![Paint::Solid([1.0, 0.0, 0.0, 1.0])],
            layers: vec![ColorLayer {
                shape: Shape::from_description("{ -0.7,-0.7; 0.7,-0.7; 0.7,0.7; -0.7,0.7; # }").unwrap(),
                paint: 0,
            }],
            plane_min: Vec2::new(-1.0, -1.0),
            plane_max: Vec2::new(1.0, 1.0),
            advance: 2.0,
        };
        let field = ColorField::build(
            &glyph,
            ColorFieldConfig {
                resolution: 32,
                base_resolution: 16,
                max_grid: 4,
                ..ColorFieldConfig::default()
            },
        )
        .unwrap();
        let fields = [(1, field.clone()), (2, field.clone())];
        let atlas = BakedColorAtlas::build(
            &fields,
            ColorAtlasConfig {
                page_size: 128,
                max_pages: 1,
            },
        )
        .unwrap();
        assert_eq!(atlas.pages().len(), 1);
        assert_eq!(atlas.page_size(), 128);
        assert_eq!(
            atlas.glyphs().iter().map(|glyph| glyph.glyph_id).collect::<Vec<_>>(),
            [1, 2]
        );
        for glyph in atlas.glyphs() {
            let encoded = field.encode([glyph.rect[0], glyph.rect[1]]).unwrap();
            for (((name, slice), (buffer_name, packed)), (_, expected)) in
                glyph.buffers.iter().zip(atlas.buffers().iter()).zip(encoded.iter())
            {
                assert_eq!(name, buffer_name);
                assert_eq!(slice.offset % COLOR_BUFFER_ALIGNMENT, 0);
                assert_eq!(&packed[slice.offset..slice.offset + slice.length], expected);
            }
        }
        let [width, height] = field.atlas_size();
        for x in [0, width] {
            for y in 0..height {
                let dst = ((y * 128 + x) * 4) as usize;
                let src = (y * width * 4) as usize;
                assert_eq!(
                    &atlas.pages[0][dst..dst + width as usize * 4],
                    &field.atlas_bytes()[src..src + width as usize * 4]
                );
            }
        }
        let original = field.encode([0, 0]).unwrap();
        let second = (original.tiles.len() + 255) & !255;
        for (i, tile) in field.tiles().iter().enumerate() {
            let offset = second + i * 32;
            assert_eq!(
                u32::from_le_bytes(atlas.buffers.tiles[offset..offset + 4].try_into().unwrap()),
                tile.origin[0] + width
            );
            assert_eq!(
                &atlas.buffers.tiles[offset + 16..offset + 32],
                &original.tiles[i * 32 + 16..i * 32 + 32]
            );
        }
        for ((_, bytes), (_, packed)) in original.iter().zip(atlas.buffers.iter()).skip(1) {
            let second = (bytes.len() + 255) & !255;
            assert_eq!(&packed[second..second + bytes.len()], bytes);
        }
        assert!(
            BakedColorAtlas::build(
                &fields,
                ColorAtlasConfig {
                    page_size: 16,
                    max_pages: 1
                }
            )
            .is_err()
        );
        assert!(
            BakedColorAtlas::build(
                &fields,
                ColorAtlasConfig {
                    page_size: width.max(height),
                    max_pages: 1
                }
            )
            .is_err()
        );
        assert!(matches!(
            BakedColorAtlas::build(&[], ColorAtlasConfig::default()),
            Err(ColorAtlasError::Empty)
        ));
        assert!(matches!(
            BakedColorAtlas::build(&[(1, field.clone()), (1, field)], ColorAtlasConfig::default()),
            Err(ColorAtlasError::DuplicateGlyph(1))
        ));
    }
}
