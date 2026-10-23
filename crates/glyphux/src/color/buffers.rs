//! Shader buffer layout shared by runtime uploads and offline color atlas baking.
use super::{ColorError, ColorField, Extend, Paint, premultiply};

#[derive(Clone, Copy, Debug, Default)]
struct ColorPaintData {
    header: [u32; 4],
    color: [f32; 4],
    row0: [f32; 4],
    row1: [f32; 4],
    geometry: [f32; 4],
}
#[derive(Clone, Copy, Debug, Default)]
struct ColorStopData {
    color: [f32; 4],
    offset: [f32; 4],
}

/// Named shader bindings. Storage tables contain glyph-local IDs; config is a uniform.
/// Bytes produced by [`ColorField::encode`] are little-endian, ready for upload.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ColorBuffers<T> {
    pub tiles: T,
    pub paints: T,
    pub stops: T,
    pub recipes: T,
    pub config: T,
}

impl<T> ColorBuffers<T> {
    /// Visit storage bindings followed by the config uniform, with manifest names.
    pub fn iter(&self) -> impl Iterator<Item = (&'static str, &T)> {
        [
            ("tiles", &self.tiles),
            ("paints", &self.paints),
            ("stops", &self.stops),
            ("recipes", &self.recipes),
            ("config", &self.config),
        ]
        .into_iter()
    }
}

impl ColorField {
    /// Encode all shader bindings for a patch placed at `origin` in an atlas page.
    /// Tile coordinates become page-relative; recipe and paint IDs remain local.
    /// Returns an error if the relocated patch exceeds the coordinate range.
    pub fn encode(&self, origin: [u32; 2]) -> Result<ColorBuffers<Vec<u8>>, ColorError> {
        for (offset, size) in origin.into_iter().zip(self.atlas_size()) {
            offset
                .checked_add(size)
                .ok_or(ColorError::Invalid("atlas origin overflows coordinates"))?;
        }
        let mut paints = Vec::new();
        let mut stops = Vec::new();
        for paint in self.paints() {
            let mut p = ColorPaintData::default();
            match paint {
                Paint::Solid(c) => p.color = premultiply(*c),
                Paint::Linear {
                    inverse, origin, line, ..
                }
                | Paint::Radial {
                    inverse,
                    center: origin,
                    line,
                    ..
                } => {
                    p.header = [
                        if matches!(paint, Paint::Linear { .. }) { 1 } else { 2 },
                        match line.extend {
                            Extend::Pad => 0,
                            Extend::Repeat => 1,
                            Extend::Reflect => 2,
                        },
                        stops.len() as u32,
                        line.stops.len() as u32,
                    ];
                    p.row0 = [inverse.0[0] as f32, inverse.0[1] as f32, inverse.0[2] as f32, 0.0];
                    p.row1 = [inverse.0[3] as f32, inverse.0[4] as f32, inverse.0[5] as f32, 0.0];
                    p.geometry = match paint {
                        Paint::Linear { direction, .. } => {
                            [origin.x as f32, origin.y as f32, direction.x as f32, direction.y as f32]
                        }
                        Paint::Radial { r0, r1, .. } => [origin.x as f32, origin.y as f32, *r0 as f32, *r1 as f32],
                        Paint::Solid(_) => unreachable!(),
                    };
                    for stop in &line.stops {
                        stops.push(ColorStopData {
                            color: premultiply(stop.color),
                            offset: [stop.offset as f32, 0.0, 0.0, 0.0],
                        });
                    }
                }
            }
            paints.push(p);
        }
        if paints.is_empty() {
            paints.push(ColorPaintData::default());
        }
        if stops.is_empty() {
            stops.push(ColorStopData::default());
        }
        let mut recipes = vec![0u32; (self.recipes().len() * 2).max(1)];
        for (id, recipe) in self.recipes().iter().enumerate() {
            recipes[id * 2] = recipes.len() as u32;
            recipes[id * 2 + 1] = recipe.len() as u32;
            recipes.extend(recipe.iter().map(|&i| i as u32));
        }
        fn uints(out: &mut Vec<u8>, values: &[u32]) {
            for value in values {
                out.extend_from_slice(&value.to_le_bytes());
            }
        }
        fn floats(out: &mut Vec<u8>, values: &[f32]) {
            for value in values {
                out.extend_from_slice(&value.to_le_bytes());
            }
        }
        let mut tables = ColorBuffers::<Vec<u8>>::default();
        for tile in self.tiles() {
            uints(
                &mut tables.tiles,
                &[tile.origin[0] + origin[0], tile.origin[1] + origin[1]],
            );
            uints(&mut tables.tiles, &[tile.mode as u32, 0]);
            uints(&mut tables.tiles, &tile.recipes);
        }
        for paint in &paints {
            uints(&mut tables.paints, &paint.header);
            for row in [paint.color, paint.row0, paint.row1, paint.geometry] {
                floats(&mut tables.paints, &row);
            }
        }
        for stop in &stops {
            floats(&mut tables.stops, &stop.color);
            floats(&mut tables.stops, &stop.offset);
        }
        uints(&mut tables.recipes, &recipes);
        let cfg = self.config();
        floats(
            &mut tables.config,
            &[
                cfg.base_resolution as f32,
                self.grid() as f32,
                cfg.range as f32,
                cfg.min_pixels as f32,
            ],
        );
        Ok(tables)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Vec2,
        color::{ColorFieldConfig, ColorTile, TileMode},
    };

    #[test]
    fn encoding_matches_the_portable_shader_contract() {
        // Explicit ABI fixture: two recipes, one premultiplied solid, one relocated tile.
        let field = ColorField {
            config: ColorFieldConfig::default(),
            grid: 1,
            tiles: vec![ColorTile {
                origin: [2, 3],
                mode: TileMode::Boundary,
                recipes: [0, 1, u32::MAX, u32::MAX],
            }],
            width: 8,
            height: 8,
            atlas: vec![0; 8 * 8 * 4],
            paints: vec![Paint::Solid([1.0, 0.5, 0.0, 0.5])],
            recipes: vec![vec![], vec![0]],
            plane_min: Vec2::new(-1.0, -1.0),
            plane_max: Vec2::new(1.0, 1.0),
            advance: 2.0,
        };
        let data = field.encode([10, 20]).unwrap();
        let words = |bytes: &[u8]| {
            bytes
                .chunks_exact(4)
                .map(|word| u32::from_le_bytes(word.try_into().unwrap()))
                .collect::<Vec<_>>()
        };
        assert_eq!(words(&data.tiles), [12, 23, 2, 0, 0, 1, u32::MAX, u32::MAX]);
        assert_eq!(words(&data.recipes), [4, 0, 4, 1, 0]);
        assert_eq!(data.paints.len(), 80);
        assert_eq!(words(&data.paints[..16]), [0; 4]);
        assert_eq!(words(&data.paints[16..32]), [0.5f32, 0.25, 0.0, 0.5].map(f32::to_bits));
        assert!(data.paints[32..].iter().all(|&byte| byte == 0));
        assert_eq!(data.stops, [0; 32]); // Nonempty binding even without gradients.
        assert_eq!(words(&data.config), [128.0f32, 1.0, 0.125, 16.0].map(f32::to_bits));
        assert!(field.encode([u32::MAX, 0]).is_err());
        assert!(field.encode([0, u32::MAX]).is_err());
    }
}
