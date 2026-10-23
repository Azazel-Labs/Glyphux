//! Shader buffer layout shared by runtime uploads and offline color atlas baking.
use super::{ColorField, Extend, Paint};

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
#[cfg_attr(feature = "gpu", derive(bytemuck::Pod, bytemuck::Zeroable))]
pub struct ColorTileData {
    pub origin: [u32; 2],
    pub mode: u32,
    pub padding: u32,
    pub ids: [u32; 4],
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
#[cfg_attr(feature = "gpu", derive(bytemuck::Pod, bytemuck::Zeroable))]
pub struct ColorPaintData {
    pub header: [u32; 4],
    pub color: [f32; 4],
    pub row0: [f32; 4],
    pub row1: [f32; 4],
    pub geometry: [f32; 4],
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
#[cfg_attr(feature = "gpu", derive(bytemuck::Pod, bytemuck::Zeroable))]
pub struct ColorStopData {
    pub color: [f32; 4],
    pub offset: [f32; 4],
}

/// Per-glyph storage tables. IDs are local to these tables.
#[derive(Clone, Debug)]
pub struct ColorGpuData {
    pub tiles: Vec<ColorTileData>,
    pub paints: Vec<ColorPaintData>,
    pub stops: Vec<ColorStopData>,
    pub recipes: Vec<u32>,
}

impl ColorField {
    /// Encode the tables used by the color shader, before atlas relocation.
    pub fn gpu_data(&self) -> ColorGpuData {
        let mut paints = Vec::new();
        let mut stops = Vec::new();
        let premultiply = |c: [f32; 4]| [c[0] * c[3], c[1] * c[3], c[2] * c[3], c[3]];
        for paint in self.paints() {
            let mut p = ColorPaintData {
                header: [0; 4],
                color: [0.0; 4],
                row0: [0.0; 4],
                row1: [0.0; 4],
                geometry: [0.0; 4],
            };
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
        let tiles: Vec<_> = self
            .tiles()
            .iter()
            .map(|t| ColorTileData {
                origin: t.origin,
                mode: t.mode as u32,
                padding: 0,
                ids: t.recipes,
            })
            .collect();
        let mut recipes = vec![0u32; (self.recipes().len() * 2).max(1)];
        for (id, recipe) in self.recipes().iter().enumerate() {
            recipes[id * 2] = recipes.len() as u32;
            recipes[id * 2 + 1] = recipe.len() as u32;
            recipes.extend(recipe.iter().map(|&i| i as u32));
        }
        ColorGpuData {
            tiles,
            paints,
            stops,
            recipes,
        }
    }
}

impl ColorGpuData {
    /// Little-endian bytes for tile, paint, stop and recipe storage bindings, in that order.
    pub fn to_le_bytes(&self) -> [Vec<u8>; 4] {
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
        let mut tables: [Vec<u8>; 4] = Default::default();
        for tile in &self.tiles {
            uints(&mut tables[0], &tile.origin);
            uints(&mut tables[0], &[tile.mode, tile.padding]);
            uints(&mut tables[0], &tile.ids);
        }
        for paint in &self.paints {
            uints(&mut tables[1], &paint.header);
            for row in [paint.color, paint.row0, paint.row1, paint.geometry] {
                floats(&mut tables[1], &row);
            }
        }
        for stop in &self.stops {
            floats(&mut tables[2], &stop.color);
            floats(&mut tables[2], &stop.offset);
        }
        uints(&mut tables[3], &self.recipes);
        tables
    }
}
