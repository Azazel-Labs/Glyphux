//! Offline color atlas pages and the storage buffers consumed by the color shader.

use glyphux::color::{ColorError, ColorField};
use std::{fmt::Write as _, path::Path};

/// A complete offline color atlas. Buffer slices use 256-byte aligned offsets.
pub struct BakedColorAtlas {
    pub page_size: u32,
    pub pages: Vec<Vec<u8>>,
    /// Tile, paint, gradient-stop, recipe, and config buffers.
    pub buffers: [Vec<u8>; 5],
    pub manifest: String,
}

fn append(buffer: &mut Vec<u8>, bytes: &[u8]) -> [usize; 2] {
    let offset = (buffer.len() + 255) & !255;
    buffer.resize(offset, 0);
    buffer.extend_from_slice(bytes);
    [offset, bytes.len()]
}

impl BakedColorAtlas {
    /// Pack already compiled glyphs into fixed-size pages. Glyph IDs must be unique.
    pub fn build(fields: &[(u16, ColorField)], page_size: u32, max_pages: usize) -> Result<Self, ColorError> {
        if fields.is_empty() || !(16..=8192).contains(&page_size) || max_pages == 0 {
            return Err(ColorError::Invalid("empty glyph set or atlas page limits"));
        }
        let mut ids = std::collections::HashSet::new();
        if fields.iter().any(|(id, _)| !ids.insert(*id)) {
            return Err(ColorError::Invalid("duplicate glyph IDs"));
        }
        let mut order: Vec<_> = fields.iter().collect();
        order.sort_by_key(|(id, field)| (std::cmp::Reverse(field.atlas_size()[1]), *id));
        let mut pages = Vec::<Vec<u8>>::new();
        // Shelf cursor: x, y, row height. Each glyph retains its own filtering gutters.
        let mut cursors = Vec::<[u32; 3]>::new();
        let mut buffers: [Vec<u8>; 5] = Default::default();
        let mut glyphs = Vec::new();
        for (id, field) in order {
            let [width, height] = field.atlas_size();
            if width > page_size || height > page_size {
                return Err(ColorError::Invalid("glyph patch exceeds --page-size"));
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
                    return Err(ColorError::Invalid("color atlas exceeds --max-pages"));
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
            let mut data = field.gpu_data();
            for tile in &mut data.tiles {
                tile.origin[0] += x;
                tile.origin[1] += y;
            }
            let tables = data.to_le_bytes();
            let mut slices = [[0usize; 2]; 5];
            for i in 0..4 {
                slices[i] = append(&mut buffers[i], &tables[i]);
            }
            let cfg = field.config();
            let config: Vec<_> = [
                cfg.base_resolution as f32,
                field.grid() as f32,
                cfg.range as f32,
                cfg.min_pixels as f32,
            ]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect();
            slices[4] = append(&mut buffers[4], &config);
            let (min, max) = field.plane_bounds();
            let mut record = format!(
                "{{\"glyphId\":{id},\"page\":{page},\"rect\":[{x},{y},{width},{height}],\"planeBounds\":[{},{},{},{}],\"advance\":{},\"grid\":{},\"recipeCount\":{},\"paintCount\":{},\"buffers\":{{",
                min.x,
                min.y,
                max.x,
                max.y,
                field.advance(),
                field.grid(),
                field.recipes().len(),
                field.paints().len()
            );
            for (i, name) in ["tiles", "paints", "stops", "recipes", "config"].iter().enumerate() {
                if i != 0 {
                    record.push(',');
                }
                write!(
                    record,
                    "\"{name}\":{{\"offset\":{},\"length\":{}}}",
                    slices[i][0], slices[i][1]
                )
                .unwrap();
            }
            record.push_str("}}");
            glyphs.push(record);
        }
        let page_records: Vec<_> = (0..pages.len()).map(|i| format!("{{\"rgba\":\"atlas-{i}.rgba\",\"png\":\"atlas-{i}.png\",\"width\":{page_size},\"height\":{page_size}}}")).collect();
        let manifest = format!(
            "{{\n  \"format\":\"glyphux-color-atlas\",\n  \"version\":1,\n  \"endianness\":\"little\",\n  \"textureFormat\":\"rgba8unorm\",\n  \"bufferAlignment\":256,\n  \"buffers\":{{\"tiles\":\"tiles.bin\",\"paints\":\"paints.bin\",\"stops\":\"stops.bin\",\"recipes\":\"recipes.bin\",\"config\":\"config.bin\"}},\n  \"pages\":[{}],\n  \"glyphs\":[\n    {}\n  ]\n}}\n",
            page_records.join(","),
            glyphs.join(",\n    ")
        );
        Ok(Self {
            page_size,
            pages,
            buffers,
            manifest,
        })
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
        for (name, bytes) in ["tiles", "paints", "stops", "recipes", "config"]
            .iter()
            .zip(&self.buffers)
        {
            std::fs::write(directory.join(format!("{name}.bin")), bytes)?;
        }
        std::fs::write(directory.join("color.wgsl"), glyphux::color::COLOR_SHADER)?;
        std::fs::write(directory.join("atlas.json"), &self.manifest)
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
        let atlas = BakedColorAtlas::build(&fields, 128, 1).unwrap();
        assert_eq!(atlas.pages.len(), 1);
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
        let original = field.gpu_data().to_le_bytes();
        let second = (original[0].len() + 255) & !255;
        for (i, tile) in field.tiles().iter().enumerate() {
            let offset = second + i * 32;
            assert_eq!(
                u32::from_le_bytes(atlas.buffers[0][offset..offset + 4].try_into().unwrap()),
                tile.origin[0] + width
            );
            assert_eq!(
                &atlas.buffers[0][offset + 16..offset + 32],
                &original[0][i * 32 + 16..i * 32 + 32]
            );
        }
        for (i, bytes) in original.iter().enumerate().skip(1) {
            let second = (bytes.len() + 255) & !255;
            assert_eq!(&atlas.buffers[i][second..second + bytes.len()], bytes);
        }
        assert!(BakedColorAtlas::build(&fields, 16, 1).is_err());
        assert!(BakedColorAtlas::build(&fields, width.max(height), 1).is_err());
        assert!(BakedColorAtlas::build(&[], 128, 1).is_err());
        assert!(BakedColorAtlas::build(&[(1, field.clone()), (1, field)], 128, 1).is_err());
    }
}
