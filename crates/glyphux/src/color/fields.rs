use std::collections::HashMap;

use super::{ColorError, ColorGlyph, Paint, source_over};
use crate::{Bitmap, FillRule, Projection, Vec2, rasterize};

pub const NO_RECIPE: u32 = u32::MAX;

/// CPU preprocessing quality and the smallest supported screen footprint.
#[derive(Clone, Copy, Debug)]
pub struct ColorFieldConfig {
    pub resolution: u32,
    pub base_resolution: u32,
    /// Half-width of the encoded distance band in normalized glyph units.
    pub range: f64,
    /// Shortcuts and tile neighborhoods remain valid at this square output size or larger.
    pub min_pixels: u32,
    /// Maximum power-of-two tile grid dimension, at most the base resolution.
    pub max_grid: u32,
}

impl Default for ColorFieldConfig {
    fn default() -> Self {
        Self {
            resolution: 1024,
            base_resolution: 128,
            range: 0.125,
            min_pixels: 16,
            max_grid: 16,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum TileMode {
    Empty,
    Interior,
    Boundary,
}

#[derive(Clone, Copy, Debug)]
pub struct ColorTile {
    pub origin: [u32; 2],
    /// First three are explicit distances; the fourth is the residual recipe.
    pub recipes: [u32; 4],
    pub mode: TileMode,
}

/// Compiled color glyph. Upload the atlas as linear RGBA8, without mipmaps.
#[derive(Clone, Debug)]
pub struct ColorField {
    pub(crate) config: ColorFieldConfig,
    pub(crate) grid: u32,
    pub(crate) tiles: Vec<ColorTile>,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) atlas: Vec<u8>,
    pub(crate) paints: Vec<Paint>,
    pub(crate) recipes: Vec<Vec<usize>>,
    pub(crate) plane_min: Vec2,
    pub(crate) plane_max: Vec2,
    pub(crate) advance: f64,
}

impl ColorField {
    /// Compile visible recipes, extend labels, build scalar EDTs, and pack local tiles.
    /// Exceeding four local recipes fails instead of losing artwork.
    pub fn build(glyph: &ColorGlyph, config: ColorFieldConfig) -> Result<Self, ColorError> {
        let h = config.resolution as usize;
        let b = config.base_resolution;
        if !(16..=2048).contains(&h)
            || !(8..=512).contains(&b)
            || !b.is_power_of_two()
            || config.max_grid == 0
            || !config.max_grid.is_power_of_two()
            || config.max_grid > b
            || config.min_pixels < 1
            || !config.range.is_finite()
            || config.range < 1.0 / f64::from(config.min_pixels)
            || !super::finite_point(glyph.plane_min)
            || !super::finite_point(glyph.plane_max)
            || glyph.plane_max.x <= glyph.plane_min.x
            || glyph.plane_max.y <= glyph.plane_min.y
            || !glyph.advance.is_finite()
        {
            return Err(ColorError::Invalid("field configuration or glyph bounds"));
        }
        for paint in &glyph.paints {
            paint.validate()?;
        }
        let mut labels = vec![NO_RECIPE; h * h];
        let mut recipes: Vec<Vec<usize>> = Vec::new();
        let mut recipe_ids = HashMap::new();
        for layer in &glyph.layers {
            let paint = glyph
                .paints
                .get(layer.paint)
                .ok_or(ColorError::Invalid("layer paint index"))?;
            for edge in layer.shape.contours.iter().flat_map(|c| &c.edges) {
                for p in edge_points(edge) {
                    if !super::finite_point(*p) || p.x.abs() > 1.0 || p.y.abs() > 1.0 {
                        return Err(ColorError::Invalid(
                            "outline control points outside normalized glyph domain",
                        ));
                    }
                }
            }
            let mut shape = layer.shape.clone();
            shape.inverse_y_axis = true;
            let coverage = rasterize(
                &shape,
                h as u32,
                h as u32,
                &Projection::uniform(h as f64 / 2.0, Vec2::new(1.0, 1.0)),
                FillRule::NonZero,
                1,
            );
            let mut transitions = HashMap::new();
            for (i, label) in labels.iter_mut().enumerate() {
                if coverage.pixel((i % h) as u32, (i / h) as u32)[0] < 0.5 {
                    continue;
                }
                let prior = if paint.is_opaque() { NO_RECIPE } else { *label };
                if let Some(&id) = transitions.get(&prior) {
                    *label = id;
                    continue;
                }
                let mut recipe = if prior == NO_RECIPE {
                    Vec::new()
                } else {
                    recipes[prior as usize].clone()
                };
                recipe.push(layer.paint);
                if recipe.len() > 256 || recipes.len() >= 4096 {
                    return Err(ColorError::Invalid("recipe complexity limit"));
                }
                let id = *recipe_ids.entry(recipe.clone()).or_insert_with(|| {
                    let id = recipes.len() as u32;
                    recipes.push(recipe);
                    id
                });
                transitions.insert(prior, id);
                *label = id;
            }
        }
        // Compact surviving recipes before extending labels into the exterior.
        let mut remap = vec![NO_RECIPE; recipes.len()];
        let mut surviving = Vec::new();
        for label in &mut labels {
            if *label == NO_RECIPE {
                continue;
            }
            let entry = &mut remap[*label as usize];
            if *entry == NO_RECIPE {
                *entry = surviving.len() as u32;
                surviving.push(recipes[*label as usize].clone());
            }
            *label = *entry;
        }
        let inside: Vec<bool> = labels.iter().map(|&v| v != NO_RECIPE).collect();
        let outer = signed_distance(&inside, h);
        let (_, nearest) = edt(&inside, h);
        for i in 0..labels.len() {
            if !inside[i] && nearest[i] != usize::MAX {
                labels[i] = labels[nearest[i]];
            }
        }
        let margin = (1.0 / f64::from(config.min_pixels)).max(2.0 / f64::from(b));
        let mut best: Option<(usize, u32, Vec<ColorTile>)> = None;
        let mut overflow = ColorError::Overflow {
            tile: [0, 0],
            regions: 5,
        };
        let mut grid = 1;
        while grid <= config.max_grid {
            match make_tiles(&labels, &outer, h, grid, margin) {
                Ok(tiles) => {
                    let count = tiles.iter().filter(|t| t.mode == TileMode::Boundary).count();
                    let side = (b / grid + 2) as usize;
                    let columns = (count as f64).sqrt().ceil().max(1.0) as usize;
                    let rows = count.div_ceil(columns).max(1);
                    let size = columns * rows * side * side * 4 + tiles.len() * 32;
                    if best.as_ref().is_none_or(|(old, _, _)| size < *old) {
                        best = Some((size, grid, tiles));
                    }
                }
                Err(e) => overflow = e,
            }
            grid *= 2;
        }
        let (_, grid, mut tiles) = best.ok_or(overflow)?;
        let count = tiles.iter().filter(|t| t.mode == TileMode::Boundary).count() as u32;
        let side = b / grid + 2;
        let columns = f64::from(count).sqrt().ceil().max(1.0) as u32;
        let width = columns * side;
        let height = count.div_ceil(columns).max(1) * side;
        let mut atlas = vec![0; (width * height * 4) as usize];
        let mut patch = 0;
        for (i, tile) in tiles.iter_mut().enumerate() {
            if tile.mode != TileMode::Boundary {
                continue;
            }
            tile.origin = [(patch % columns) * side, (patch / columns) * side];
            patch += 1;
            write_patch(
                &mut atlas,
                width,
                tile.origin,
                side,
                i as u32,
                grid,
                b,
                h,
                config.range,
                &outer,
                3,
            );
        }
        // One region field at a time bounds working memory independently of recipe count.
        for id in 0..surviving.len() {
            if !tiles
                .iter()
                .any(|t| t.mode == TileMode::Boundary && t.recipes[..3].contains(&(id as u32)))
            {
                continue;
            }
            let mask: Vec<bool> = labels.iter().map(|&v| v == id as u32).collect();
            let distances = signed_distance(&mask, h);
            for (i, tile) in tiles.iter().enumerate() {
                if tile.mode != TileMode::Boundary {
                    continue;
                }
                for channel in 0..3 {
                    if tile.recipes[channel] == id as u32 {
                        write_patch(
                            &mut atlas,
                            width,
                            tile.origin,
                            side,
                            i as u32,
                            grid,
                            b,
                            h,
                            config.range,
                            &distances,
                            channel,
                        );
                    }
                }
            }
        }
        Ok(Self {
            config,
            grid,
            tiles,
            width,
            height,
            atlas,
            paints: glyph.paints.clone(),
            recipes: surviving,
            plane_min: glyph.plane_min,
            plane_max: glyph.plane_max,
            advance: glyph.advance,
        })
    }

    pub fn config(&self) -> ColorFieldConfig {
        self.config
    }
    pub fn grid(&self) -> u32 {
        self.grid
    }
    pub fn tiles(&self) -> &[ColorTile] {
        &self.tiles
    }
    pub fn atlas_size(&self) -> [u32; 2] {
        [self.width, self.height]
    }
    pub fn atlas_bytes(&self) -> &[u8] {
        &self.atlas
    }
    pub fn paints(&self) -> &[Paint] {
        &self.paints
    }
    pub fn recipes(&self) -> &[Vec<usize>] {
        &self.recipes
    }
    pub fn plane_bounds(&self) -> (Vec2, Vec2) {
        (self.plane_min, self.plane_max)
    }
    pub fn advance(&self) -> f64 {
        self.advance
    }

    /// Decode one normalized glyph position. `pixels_per_unit` is half the square output
    /// size. Returns premultiplied linear RGBA and the filtered texture lookup count.
    pub fn sample(&self, q: Vec2, pixels_per_unit: f64) -> Result<([f32; 4], u32), ColorError> {
        if !super::finite_point(q)
            || !pixels_per_unit.is_finite()
            || pixels_per_unit * 2.0 < f64::from(self.config.min_pixels)
        {
            return Err(ColorError::Invalid("pixel footprint below compiled minimum"));
        }
        if q.x.abs() > 1.0 || q.y.abs() > 1.0 {
            return Ok(([0.0; 4], 0));
        }
        let tx = (((q.x + 1.0) * f64::from(self.grid) / 2.0).floor() as u32).min(self.grid - 1);
        let ty = (((q.y + 1.0) * f64::from(self.grid) / 2.0).floor() as u32).min(self.grid - 1);
        let tile = self.tiles[(ty * self.grid + tx) as usize];
        if tile.mode == TileMode::Empty {
            return Ok(([0.0; 4], 0));
        }
        if tile.mode == TileMode::Interior {
            return Ok((self.recipe(tile.recipes[0], q), 0));
        }
        let low = Vec2::new(
            f64::from(tx) * 2.0 / f64::from(self.grid) - 1.0,
            f64::from(ty) * 2.0 / f64::from(self.grid) - 1.0,
        );
        let z = (q - low) * (f64::from(self.config.base_resolution) / 2.0)
            + Vec2::new(f64::from(tile.origin[0]) + 0.5, f64::from(tile.origin[1]) + 0.5);
        let v = self.bilinear(z);
        let d: [f64; 4] = std::array::from_fn(|i| (v[i] - 0.5) * 2.0 * self.config.range);
        let mut weights = [0.0; 4];
        for i in 0..3 {
            if tile.recipes[i] != NO_RECIPE {
                weights[i] = (0.5 + d[i] * pixels_per_unit).clamp(0.0, 1.0);
            }
        }
        if tile.recipes[3] != NO_RECIPE {
            weights[3] = (1.0 - weights[..3].iter().sum::<f64>()).max(0.0);
        }
        let mut sum: f64 = weights.iter().sum();
        if sum == 0.0 {
            let i = (0..3)
                .filter(|&i| tile.recipes[i] != NO_RECIPE)
                .max_by(|&a, &b| d[a].total_cmp(&d[b]));
            if let Some(i) = i {
                weights[i] = 1.0;
                sum = 1.0;
            }
        }
        let mut color = [0.0; 4];
        let coverage = (0.5 + d[3] * pixels_per_unit).clamp(0.0, 1.0);
        for i in 0..4 {
            if weights[i] == 0.0 {
                continue;
            }
            let paint = self.recipe(tile.recipes[i], q);
            for c in 0..4 {
                color[c] += paint[c] * (weights[i] / sum * coverage) as f32;
            }
        }
        Ok((color, 1))
    }

    pub fn render(&self, size: u32) -> Result<Bitmap<4>, ColorError> {
        if size < self.config.min_pixels || size > 8192 {
            return Err(ColorError::Invalid("output size"));
        }
        let mut out = Bitmap::new(size, size);
        for y in 0..size {
            for x in 0..size {
                let q = Vec2::new(
                    2.0 * (f64::from(x) + 0.5) / f64::from(size) - 1.0,
                    2.0 * (f64::from(y) + 0.5) / f64::from(size) - 1.0,
                );
                *out.pixel_mut(x, y) = self.sample(q, f64::from(size) / 2.0)?.0;
            }
        }
        Ok(out)
    }

    fn recipe(&self, id: u32, q: Vec2) -> [f32; 4] {
        self.recipes[id as usize].iter().fold([0.0; 4], |back, &paint| {
            source_over(self.paints[paint].evaluate(q), back)
        })
    }

    fn bilinear(&self, p: Vec2) -> [f64; 4] {
        let x = p.x.clamp(0.0, f64::from(self.width - 1));
        let y = p.y.clamp(0.0, f64::from(self.height - 1));
        let ix = x.floor() as u32;
        let iy = y.floor() as u32;
        let fx = x - f64::from(ix);
        let fy = y - f64::from(iy);
        std::array::from_fn(|c| {
            let at = |xx: u32, yy: u32| f64::from(self.atlas[((yy * self.width + xx) * 4) as usize + c]) / 255.0;
            let right = (ix + 1).min(self.width - 1);
            let bottom = (iy + 1).min(self.height - 1);
            (at(ix, iy) * (1.0 - fx) + at(right, iy) * fx) * (1.0 - fy)
                + (at(ix, bottom) * (1.0 - fx) + at(right, bottom) * fx) * fy
        })
    }
}

pub(super) fn edge_points(edge: &crate::Edge) -> &[Vec2] {
    match &edge.segment {
        crate::Segment::Linear(p) => p,
        crate::Segment::Quadratic(p) => p,
        crate::Segment::Cubic(p) => p,
    }
}

fn make_tiles(labels: &[u32], outer: &[f64], h: usize, grid: u32, margin: f64) -> Result<Vec<ColorTile>, ColorError> {
    let mut tiles = Vec::new();
    for y in 0..grid {
        for x in 0..grid {
            let lo = |v: u32| {
                ((f64::from(v) / f64::from(grid) - margin / 2.0) * h as f64)
                    .floor()
                    .max(0.0) as usize
            };
            let hi = |v: u32| {
                ((f64::from(v + 1) / f64::from(grid) + margin / 2.0) * h as f64)
                    .ceil()
                    .min(h as f64) as usize
            };
            let mut counts: HashMap<u32, usize> = HashMap::new();
            let mut min = f64::INFINITY;
            let mut max = f64::NEG_INFINITY;
            for yy in lo(y)..hi(y) {
                for xx in lo(x)..hi(x) {
                    let i = yy * h + xx;
                    min = min.min(outer[i]);
                    max = max.max(outer[i]);
                    if labels[i] != NO_RECIPE {
                        *counts.entry(labels[i]).or_default() += 1;
                    }
                }
            }
            let mut tile = ColorTile {
                origin: [0; 2],
                recipes: [NO_RECIPE; 4],
                mode: TileMode::Empty,
            };
            if max >= -margin && !counts.is_empty() {
                if counts.len() > 4 {
                    return Err(ColorError::Overflow {
                        tile: [x, y],
                        regions: counts.len(),
                    });
                }
                let mut ids: Vec<_> = counts.into_iter().collect();
                ids.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
                if ids.len() == 4 {
                    ids.rotate_left(1);
                }
                for (i, (id, _)) in ids.iter().enumerate() {
                    tile.recipes[i] = *id;
                }
                tile.mode = if ids.len() == 1 && min > margin {
                    TileMode::Interior
                } else {
                    TileMode::Boundary
                };
            }
            tiles.push(tile);
        }
    }
    Ok(tiles)
}

fn write_patch(
    atlas: &mut [u8],
    width: u32,
    origin: [u32; 2],
    side: u32,
    tile: u32,
    grid: u32,
    b: u32,
    h: usize,
    range: f64,
    distances: &[f64],
    channel: usize,
) {
    for y in 0..side {
        for x in 0..side {
            let px = (f64::from(tile % grid) / f64::from(grid) + (f64::from(x) - 0.5) / f64::from(b)) * h as f64 - 0.5;
            let py = (f64::from(tile / grid) / f64::from(grid) + (f64::from(y) - 0.5) / f64::from(b)) * h as f64 - 0.5;
            let px = px.clamp(0.0, (h - 1) as f64);
            let py = py.clamp(0.0, (h - 1) as f64);
            let ix = px.floor() as usize;
            let iy = py.floor() as usize;
            let fx = px - ix as f64;
            let fy = py - iy as f64;
            let rx = (ix + 1).min(h - 1);
            let by = (iy + 1).min(h - 1);
            let d = (distances[iy * h + ix] * (1.0 - fx) + distances[iy * h + rx] * fx) * (1.0 - fy)
                + (distances[by * h + ix] * (1.0 - fx) + distances[by * h + rx] * fx) * fy;
            atlas[(((origin[1] + y) * width + origin[0] + x) * 4) as usize + channel] =
                (255.0 * (0.5 + d / (2.0 * range)).clamp(0.0, 1.0)).round() as u8;
        }
    }
}

fn signed_distance(mask: &[bool], h: usize) -> Vec<f64> {
    if mask.iter().all(|&b| b) {
        return vec![2.0; mask.len()];
    }
    if mask.iter().all(|&b| !b) {
        return vec![-2.0; mask.len()];
    }
    let (to_inside, _) = edt(mask, h);
    let outside: Vec<_> = mask.iter().map(|b| !b).collect();
    let (to_outside, _) = edt(&outside, h);
    (0..mask.len())
        .map(|i| (to_outside[i].sqrt() - to_inside[i].sqrt() - if mask[i] { 0.5 } else { -0.5 }) * 2.0 / h as f64)
        .collect()
}

// Separable squared Euclidean distance transform with exact nearest-site indices.
fn edt(mask: &[bool], h: usize) -> (Vec<f64>, Vec<usize>) {
    let mut d = vec![f64::INFINITY; mask.len()];
    let mut ids = vec![usize::MAX; mask.len()];
    let mut f = vec![0.0; h];
    for y in 0..h {
        for x in 0..h {
            f[x] = if mask[y * h + x] { 0.0 } else { f64::INFINITY };
        }
        let (row, sites) = edt_line(&f);
        for x in 0..h {
            d[y * h + x] = row[x];
            if sites[x] != usize::MAX {
                ids[y * h + x] = y * h + sites[x];
            }
        }
    }
    for x in 0..h {
        for y in 0..h {
            f[y] = d[y * h + x];
        }
        let (column, sites) = edt_line(&f);
        let old: Vec<_> = (0..h).map(|y| ids[y * h + x]).collect();
        for y in 0..h {
            d[y * h + x] = column[y];
            ids[y * h + x] = if sites[y] == usize::MAX {
                usize::MAX
            } else {
                old[sites[y]]
            };
        }
    }
    (d, ids)
}

fn edt_line(f: &[f64]) -> (Vec<f64>, Vec<usize>) {
    let mut v: Vec<usize> = Vec::new();
    let mut starts: Vec<f64> = Vec::new();
    for q in 0..f.len() {
        if !f[q].is_finite() {
            continue;
        }
        let mut start = f64::NEG_INFINITY;
        while let Some(&p) = v.last() {
            start = (f[q] + (q * q) as f64 - f[p] - (p * p) as f64) / (2.0 * (q - p) as f64);
            if start > *starts.last().unwrap() {
                break;
            }
            v.pop();
            starts.pop();
        }
        if v.is_empty() {
            start = f64::NEG_INFINITY;
        }
        v.push(q);
        starts.push(start);
    }
    let mut d = vec![f64::INFINITY; f.len()];
    let mut ids = vec![usize::MAX; f.len()];
    if !v.is_empty() {
        let mut k = 0;
        for q in 0..f.len() {
            while k + 1 < v.len() && starts[k + 1] < q as f64 {
                k += 1;
            }
            d[q] = (q as f64 - v[k] as f64).powi(2) + f[v[k]];
            ids[q] = v[k];
        }
    }
    (d, ids)
}

#[cfg(test)]
mod tests {
    #[test]
    fn euclidean_transform_matches_brute_force() {
        let h = 9;
        let mask: Vec<_> = (0..h * h).map(|i| i == 2 || i == 43 || i == 72).collect();
        let (distance, sites) = super::edt(&mask, h);
        for i in 0..h * h {
            let squared = |j: usize| (i % h).abs_diff(j % h).pow(2) + (i / h).abs_diff(j / h).pow(2);
            let expected = (0..h * h).filter(|&j| mask[j]).map(squared).min().unwrap();
            assert_eq!(distance[i], expected as f64);
            assert!(mask[sites[i]]);
            assert_eq!(squared(sites[i]), expected);
        }
    }
}
