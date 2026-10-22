//! A glyph cache in a texture array, filled on demand by the GPU generator.
//!
//! Each glyph is generated at a *level* (field texels per em, a power of two) chosen from
//! the size it is displayed at, so small text gets small fields and large text gets enough
//! resolution, and the same glyph can live at several levels. Requests during a frame are
//! collected and generated together in one batch by [`GlyphAtlas::flush`]; the returned
//! placements are valid for drawing right after that (same queue). When the atlas is full,
//! the least recently used layer that was not used in the current frame is recycled.

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hasher};

use msdfr::gpu::FieldKind;
use msdfr::{DEFAULT_ANGLE_THRESHOLD, ErrorCorrectionConfig, Projection, Range, Shape, Transform, Vec2};

use crate::field::{FieldBatch, FieldGenerator, storage_entry};

/// Atlas configuration.
#[derive(Clone, Debug)]
pub struct AtlasConfig {
    /// Side of each texture layer in texels.
    pub size: u32,
    /// Number of layers of the texture array.
    pub layers: u32,
    /// `Rgba8Unorm` (compact) or `Rgba16Float` (more precision for wide ranges).
    pub format: wgpu::TextureFormat,
    /// Field type; MTSDF keeps the true distance in alpha (for outlines, glows, shadows).
    pub kind: FieldKind,
    /// Distance range in field texels (total width), the same at all levels.
    pub px_range: f64,
    /// Smallest and largest level (field texels per em).
    pub min_level: u32,
    pub max_level: u32,
    /// How much a field may be magnified on screen before the next level is used. MSDF keeps
    /// straight edges and corners sharp at any magnification, but features thinner than a
    /// texel (the notch where an arch meets a stem in r, n, m, h) smear into gray blobs once
    /// magnified; 2x with a 32 texels/em floor keeps them below a pixel.
    pub magnification: f64,
    pub error_correction: ErrorCorrectionConfig,
    pub overlap_support: bool,
    /// Edge coloring strategy (msdfr's ink-trap aware coloring by default).
    pub coloring: msdfr::ColoringStrategy,
    /// Edge coloring corner angle threshold (radians).
    pub angle_threshold: f64,
    /// Skip exact evaluation of tiles farther than half the range plus 2 texels from the
    /// glyph (only their sign is computed): their values are clamped in the texture anyway
    /// (exactly so for `Rgba8Unorm`).
    pub narrow_band: bool,
    /// Per-glyph detail probe: a small CPU MSDF test finds the lowest level that reproduces
    /// a glyph (rendered at 2x, at most `detail_tolerance` of its ink area wrong); the glyph
    /// then never uses fewer texels per em than that (nor more than its display size needs).
    /// Hairline scripts get resolution, plain glyphs stay small. The probe (about 0.5 ms per
    /// glyph) runs off the frame: new glyphs are generated at their size level at once, and
    /// the few that need more are regenerated once their probe reports.
    pub detail_probe: bool,
    pub detail_tolerance: f64,
    /// Without worker threads (`wasm32`): probes run on the calling thread, this many per
    /// [`GlyphAtlas::flush`].
    pub probes_per_flush: usize,
    /// Threads preparing glyphs (outline processing, tile binning) and running detail probes
    /// while the application builds its frame; 0 = one less than the CPU count, at most 4.
    /// Ignored on `wasm32`, where [`GlyphAtlas::flush`] does that work.
    pub worker_threads: usize,
    /// Generation budget per flush in texels (0 = unlimited); requests beyond it fall back to
    /// another cached level of the same glyph when there is one, and are generated later.
    pub texel_budget: u32,
    /// Largest range tier for [`GlyphAtlas::get_with_reach`]: glyphs drawn with wide glows or
    /// outlines are generated with up to `px_range << max_range_tier` texels of range (and
    /// padding), so their distance field reaches far enough outside the glyph.
    pub max_range_tier: u32,
    /// msdfgen's scanline sign correction with this fill rule (non-zero, as fonts are filled,
    /// by default): fixes the sign of texels inside overlapping or self-intersecting contours
    /// (variable fonts), which the overlapping contour combiner can get wrong. `None`: off.
    pub sign_correction: Option<msdfr::FillRule>,
}

impl Default for AtlasConfig {
    fn default() -> Self {
        AtlasConfig {
            size: 1024,
            layers: 4,
            format: wgpu::TextureFormat::Rgba8Unorm,
            kind: FieldKind::MTSDF,
            px_range: 4.0,
            min_level: 32,
            max_level: 128,
            magnification: 2.0,
            error_correction: ErrorCorrectionConfig::default(),
            overlap_support: true,
            coloring: msdfr::ColoringStrategy::default(),
            angle_threshold: DEFAULT_ANGLE_THRESHOLD,
            narrow_band: true,
            detail_probe: true,
            detail_tolerance: 0.05,
            probes_per_flush: 2,
            worker_threads: 0,
            texel_budget: 0,
            max_range_tier: 3,
            sign_correction: Some(msdfr::FillRule::NonZero),
        }
    }
}

/// Identifies a glyph: the user's font id and the glyph id within it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GlyphKey {
    pub font: u32,
    pub glyph: u32,
}

/// Where a cached glyph is and how to draw it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AtlasGlyph {
    pub layer: u32,
    /// Texture coordinates (normalized) of the quad, inset by half a texel so bilinear
    /// filtering never reads a neighbor. Texture rows run top to bottom: `uv_min` is the
    /// top-left corner, i.e. `(plane_min.x, plane_max.y)`.
    pub uv_min: [f32; 2],
    pub uv_max: [f32; 2],
    /// The quad in shape units (em for fonts), y up, relative to the glyph origin.
    pub plane_min: [f32; 2],
    pub plane_max: [f32; 2],
    /// Field texels per em.
    pub level: u32,
    /// Distance range of the field in texels (the atlas' range, wider for glyphs requested
    /// with a reach, see [`GlyphAtlas::get_with_reach`]).
    pub px_range: f32,
}

impl AtlasGlyph {
    /// Whether there is anything to draw (false for blank glyphs such as spaces).
    pub fn is_visible(&self) -> bool {
        self.plane_max[0] > self.plane_min[0]
    }

    const BLANK: AtlasGlyph = AtlasGlyph {
        layer: 0,
        uv_min: [0.0; 2],
        uv_max: [0.0; 2],
        plane_min: [0.0; 2],
        plane_max: [0.0; 2],
        level: 0,
        px_range: 0.0,
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct CacheKey {
    key: GlyphKey,
    level: u32,
    /// Range tier: the field's range is the atlas' `px_range << tier`.
    tier: u32,
}

struct Entry {
    glyph: AtlasGlyph,
    last_used: u64,
}

struct Shelf {
    y: u32,
    h: u32,
    x: u32,
}

struct Layer {
    shelves: Vec<Shelf>,
    next_y: u32,
    last_used: u64,
    glyphs: Vec<CacheKey>,
}

impl Layer {
    fn new() -> Self {
        Layer {
            shelves: Vec::new(),
            next_y: 0,
            last_used: 0,
            glyphs: Vec::new(),
        }
    }

    /// Shelf allocation: the tightest shelf that fits, else a new shelf.
    fn alloc(&mut self, w: u32, h: u32, size: u32) -> Option<(u32, u32)> {
        let mut best: Option<usize> = None;
        for (i, s) in self.shelves.iter().enumerate() {
            if s.h >= h && s.h <= h + h / 4 + 4 && size - s.x >= w && best.is_none_or(|b| s.h < self.shelves[b].h) {
                best = Some(i);
            }
        }
        if let Some(i) = best {
            let s = &mut self.shelves[i];
            let x = s.x;
            s.x += w;
            return Some((x, s.y));
        }
        let sh = h.next_multiple_of(4).min(size);
        if w <= size && self.next_y + sh <= size {
            let y = self.next_y;
            self.next_y += sh;
            self.shelves.push(Shelf { y, h: sh, x: w });
            return Some((0, y));
        }
        None
    }
}

/// A glyph waiting for generation (its CPU preparation runs on the workers).
struct Pending {
    id: u64,
    ck: CacheKey,
}

/// Cached distance fields of glyphs in a texture array.
pub struct GlyphAtlas {
    config: AtlasConfig,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    generator: FieldGenerator,
    store: wgpu::ComputePipeline,
    store_layout: wgpu::BindGroupLayout,
    entries: FxHashMap<CacheKey, Entry>,
    /// Cached (level, tier) variants per glyph.
    levels: FxHashMap<GlyphKey, Vec<(u32, u32)>>,
    /// Probed minimum level per glyph (see [`AtlasConfig::detail_probe`]).
    detail: FxHashMap<GlyphKey, u32>,
    workers: crate::workers::Workers,
    /// Preparations finished before `flush` asked for them, by job id.
    ready: FxHashMap<u64, Option<Box<crate::field::PreparedJob>>>,
    next_job: u64,
    probes_in_flight: HashSet<GlyphKey, FxBuild>,
    layers: Vec<Layer>,
    pending: Vec<Pending>,
    pending_texels: u32,
    frame: u64,
    stats: AtlasStats,
}

/// Counters since the atlas was created.
#[derive(Clone, Copy, Debug, Default)]
pub struct AtlasStats {
    pub generated: u64,
    /// Detail probes finished, and how many of them raised a glyph's level.
    pub probes: u64,
    pub probe_upgrades: u64,
    pub generated_texels: u64,
    pub evicted_layers: u64,
    pub fallbacks: u64,
    pub failed: u64,
}

impl GlyphAtlas {
    pub fn new(device: &wgpu::Device, config: AtlasConfig) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("msdfr atlas"),
            size: wgpu::Extent3d {
                width: config.size,
                height: config.size,
                depth_or_array_layers: config.layers,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: config.format,
            usage: wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("msdfr atlas store"),
            source: wgpu::ShaderSource::Wgsl(store_shader_source(config.format).into()),
        });
        let store_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("msdfr atlas store"),
            entries: &[
                storage_entry(0, true),
                storage_entry(1, true),
                storage_entry(7, false),
                wgpu::BindGroupLayoutEntry {
                    binding: 10,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: config.format,
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                    },
                    count: None,
                },
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("msdfr atlas store"),
            bind_group_layouts: &[Some(&store_layout)],
            immediate_size: 0,
        });
        let store = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("msdfr atlas store"),
            layout: Some(&pl),
            module: &module,
            entry_point: Some("store"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });
        let layers = (0..config.layers).map(|_| Layer::new()).collect();
        let workers = crate::workers::Workers::new(&config);
        GlyphAtlas {
            generator: FieldGenerator::new(device),
            config,
            texture,
            view,
            store,
            store_layout,
            entries: FxHashMap::default(),
            levels: FxHashMap::default(),
            detail: FxHashMap::default(),
            workers,
            ready: FxHashMap::default(),
            next_job: 0,
            probes_in_flight: HashSet::default(),
            layers,
            pending: Vec::new(),
            pending_texels: 0,
            frame: 1,
            stats: AtlasStats::default(),
        }
    }

    pub fn config(&self) -> &AtlasConfig {
        &self.config
    }
    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }
    /// `D2Array` view for sampling.
    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }
    pub fn stats(&self) -> AtlasStats {
        self.stats
    }

    /// Starts a new frame: glyphs used from now on are protected from eviction until the
    /// next call.
    pub fn begin_frame(&mut self) {
        self.frame += 1;
        self.collect_probes();
    }

    /// Takes finished detail probes: glyphs whose probe asks for more resolution than they
    /// were generated at get regenerated on their next request.
    fn collect_probes(&mut self) {
        let done = self.workers.take_done();
        self.route(done);
    }

    fn route(&mut self, done: Vec<crate::workers::Done>) {
        use crate::workers::Done;
        for d in done {
            let (key, level) = match d {
                Done::Prep(id, job) => {
                    self.ready.insert(id, job);
                    continue;
                }
                Done::Probe(key, level) => (key, level),
            };
            self.probes_in_flight.remove(&key);
            self.stats.probes += 1;
            let generated = self
                .levels
                .get(&key)
                .and_then(|v| v.iter().map(|&(l, _)| l).max())
                .unwrap_or(0);
            if level > generated {
                self.stats.probe_upgrades += 1;
            }
            self.detail.insert(key, level);
        }
    }

    /// Level (field texels per em) for a glyph displayed at `px_per_em` screen pixels per em.
    pub fn level_for(&self, px_per_em: f64) -> u32 {
        self.pow2_level(px_per_em / self.config.magnification)
    }

    /// The power of two at or above `texels` (per em) within the configured levels. Huge,
    /// infinite and NaN sizes give the top or bottom level (no overflow).
    fn pow2_level(&self, texels: f64) -> u32 {
        let (lo, hi) = (self.config.min_level, self.config.max_level);
        if texels.is_nan() {
            return lo;
        }
        // Clamped in floating point first: `hi` is a level, so its power of two is in range.
        let t = texels.ceil().clamp(1.0, f64::from(hi)) as u32;
        t.checked_next_power_of_two().unwrap_or(hi).clamp(lo, hi)
    }

    /// Returns the placement of a glyph displayed at `px_per_em`, queueing its generation on
    /// a miss (`shape` is called then; `None` or an empty shape gives a blank glyph). Returns
    /// `None` if it cannot be placed (atlas full with glyphs of the current frame).
    pub fn get(&mut self, key: GlyphKey, px_per_em: f64, shape: impl FnOnce() -> Option<Shape>) -> Option<AtlasGlyph> {
        self.get_with_reach(key, px_per_em, 0.0, shape)
    }

    /// [`Self::get`] for a glyph drawn with effects reaching `reach_px` screen pixels outside
    /// its outline (glow radius, outline width, shadow offset + blur): the field's range is
    /// widened in power-of-two tiers (up to [`AtlasConfig::max_range_tier`]) until half of it
    /// covers the reach at this size, and if even the widest tier falls short the glyph uses
    /// a lower level (down to `min_level`) instead of clipping the effect. A glyph already
    /// cached with a wider range is reused.
    pub fn get_with_reach(
        &mut self,
        key: GlyphKey,
        px_per_em: f64,
        reach_px: f64,
        shape: impl FnOnce() -> Option<Shape>,
    ) -> Option<AtlasGlyph> {
        let size_level = self.level_for(px_per_em);
        // Never more texels per em than the glyph is displayed at.
        let cap = self.pow2_level(px_per_em);
        // Until its probe reports, a glyph uses its size level.
        let detail = self.detail.get(&key).copied();
        let probe_wanted = self.config.detail_probe && cap > size_level && detail.is_none();
        let mut level = match detail {
            Some(d) => size_level.max(d.min(cap)),
            None => size_level,
        };
        // Range (texels) for effects reaching `reach_px`: half the range lies outside the
        // outline, and at `level` a texel is `px_per_em / level` screen pixels. One extra pixel
        // keeps the effect's last pixel inside the field.
        let need = |level: u32| 2.0 * (reach_px + 1.0) * f64::from(level) / px_per_em.max(1e-6);
        // Beyond the widest tier, fewer texels per em (each covering more pixels) let the same
        // range reach farther: effects keep their size instead of being clipped, which would
        // jump with the level as the size changes.
        let widest = self.config.px_range * f64::from(1u32 << self.config.max_range_tier);
        while reach_px > 0.0 && level > self.config.min_level && need(level) > widest {
            level /= 2;
        }
        let need = need(level);
        let tier = (0..self.config.max_range_tier)
            .find(|&t| self.config.px_range * f64::from(1u32 << t) >= need)
            .unwrap_or(self.config.max_range_tier);
        let tier = if reach_px > 0.0 { tier } else { 0 };
        for t in tier..=self.config.max_range_tier.max(tier) {
            if let Some(e) = self.entries.get_mut(&CacheKey { key, level, tier: t }) {
                e.last_used = self.frame;
                let g = e.glyph;
                if g.is_visible() {
                    self.layers[g.layer as usize].last_used = self.frame;
                }
                return Some(g);
            }
        }
        let ck = CacheKey { key, level, tier };
        let px_range = self.config.px_range * f64::from(1u32 << tier);
        // Over budget: use another level of the same glyph if there is one.
        if self.config.texel_budget > 0
            && self.pending_texels >= self.config.texel_budget
            && let Some(g) = self.fallback(key, level, tier)
        {
            self.stats.fallbacks += 1;
            return Some(g);
        }
        // Only what layout needs happens here: the outline's bounds give the quad and the
        // atlas rectangle. Everything else is prepared on the workers.
        let Some(shape) = shape().filter(has_ink) else {
            return Some(self.insert_blank(ck));
        };
        let b = shape.bounds();
        if !(b.left < b.right && b.bottom < b.top) {
            return Some(self.insert_blank(ck));
        }
        let scale = level as f64;
        let pad = 0.5 * px_range / scale;
        let width = (((b.right - b.left) + 2.0 * pad) * scale).ceil() as u32 + 1;
        let height = (((b.top - b.bottom) + 2.0 * pad) * scale).ceil() as u32 + 1;
        let max = self.config.size;
        if width > max || height > max {
            self.stats.failed += 1;
            return None;
        }
        let Some((layer, x, y)) = self.allocate(width, height) else {
            self.stats.failed += 1;
            return self.fallback(key, level, tier);
        };
        let translate = Vec2::new(-b.left + pad, -b.bottom + pad);
        let transform = Transform::new(
            Projection::uniform(scale, translate),
            Range::symmetric(px_range / scale),
        );
        let s = max as f32;
        let glyph = AtlasGlyph {
            layer,
            uv_min: [(x as f32 + 0.5) / s, (y as f32 + 0.5) / s],
            uv_max: [(x + width) as f32 - 0.5, (y + height) as f32 - 0.5].map(|v| v / s),
            plane_min: [(0.5 / scale - translate.x) as f32, (0.5 / scale - translate.y) as f32],
            plane_max: [
                ((width as f64 - 0.5) / scale - translate.x) as f32,
                ((height as f64 - 0.5) / scale - translate.y) as f32,
            ],
            level,
            px_range: px_range as f32,
        };
        let id = self.next_job;
        self.next_job += 1;
        let probe = (probe_wanted && self.probes_in_flight.insert(key)).then_some(key);
        self.workers.submit(crate::workers::PrepJob {
            id,
            shape,
            transform,
            width,
            height,
            origin: [x, y, layer],
            px_range,
            probe,
        });
        self.pending.push(Pending { id, ck });
        self.pending_texels += width * height;
        self.entries.insert(
            ck,
            Entry {
                glyph,
                last_used: self.frame,
            },
        );
        self.levels.entry(key).or_default().push((level, tier));
        let l = &mut self.layers[layer as usize];
        l.glyphs.push(ck);
        l.last_used = self.frame;
        Some(glyph)
    }

    /// Turns a queued glyph into a blank one (its rectangle stays allocated until its layer
    /// is recycled).
    fn make_blank(&mut self, ck: CacheKey) {
        if let Some(v) = self.levels.get_mut(&ck.key) {
            v.retain(|&v| v != (ck.level, ck.tier));
            if v.is_empty() {
                self.levels.remove(&ck.key);
            }
        }
        if self.entries.contains_key(&ck) {
            self.insert_blank(ck);
        }
    }

    fn insert_blank(&mut self, ck: CacheKey) -> AtlasGlyph {
        let g = AtlasGlyph {
            level: ck.level,
            ..AtlasGlyph::BLANK
        };
        self.entries.insert(
            ck,
            Entry {
                glyph: g,
                last_used: self.frame,
            },
        );
        g
    }

    /// The cached variant of `key` closest to `level` (preferring larger, then a range tier
    /// at least `tier`), if any.
    fn fallback(&mut self, key: GlyphKey, level: u32, tier: u32) -> Option<AtlasGlyph> {
        let levels = self.levels.get(&key)?;
        let (best, best_tier) = levels.iter().copied().min_by_key(|&(l, t)| {
            let d = (l as i64 - level as i64).abs();
            (d, l < level, t < tier, t.abs_diff(tier))
        })?;
        let e = self.entries.get_mut(&CacheKey {
            key,
            level: best,
            tier: best_tier,
        })?;
        e.last_used = self.frame;
        self.layers[e.glyph.layer as usize].last_used = self.frame;
        Some(e.glyph)
    }

    fn allocate(&mut self, w: u32, h: u32) -> Option<(u32, u32, u32)> {
        let size = self.config.size;
        for (i, l) in self.layers.iter_mut().enumerate() {
            if let Some((x, y)) = l.alloc(w, h, size) {
                return Some((i as u32, x, y));
            }
        }
        // Recycle the least recently used layer not used in this frame.
        let frame = self.frame;
        let (i, _) = self
            .layers
            .iter()
            .enumerate()
            .filter(|(_, l)| l.last_used < frame)
            .min_by_key(|(_, l)| l.last_used)?;
        let old = std::mem::replace(&mut self.layers[i], Layer::new());
        for ck in old.glyphs {
            self.entries.remove(&ck);
            if let Some(v) = self.levels.get_mut(&ck.key) {
                v.retain(|&v| v != (ck.level, ck.tier));
                if v.is_empty() {
                    self.levels.remove(&ck.key);
                }
            }
        }
        self.stats.evicted_layers += 1;
        let (x, y) = self.layers[i].alloc(w, h, size)?;
        Some((i as u32, x, y))
    }

    /// Detail probes submitted and not yet reported.
    pub fn probes_pending(&self) -> usize {
        self.probes_in_flight.len()
    }

    /// Number of glyphs waiting for [`Self::flush`].
    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    /// Generates all queued glyphs into the atlas (one batch). Returns how many.
    ///
    /// The upload is recorded into `encoder`, in order with the passes, so a frame may flush
    /// any number of times before submitting.
    pub fn flush(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder) -> usize {
        self.flush_profiled(device, encoder, None)
    }

    /// [`Self::flush`] with GPU timestamps around its passes (distance, correction, store).
    pub fn flush_profiled(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        mut profiler: Option<&mut crate::profile::GpuProfiler>,
    ) -> usize {
        self.workers.run_probes_here(self.config.probes_per_flush);
        if self.pending.is_empty() {
            let done = self.workers.take_done();
            self.route(done);
            return 0;
        }
        let pending = std::mem::take(&mut self.pending);
        self.pending_texels = 0;
        // Wait for (or finish) this frame's preparations.
        let outstanding = pending.iter().filter(|p| !self.ready.contains_key(&p.id)).count();
        let done = self.workers.finish_preps(outstanding);
        self.route(done);
        let mut jobs: Vec<crate::field::PreparedJob> = Vec::with_capacity(pending.len());
        for p in &pending {
            match self.ready.remove(&p.id).flatten() {
                Some(job) => jobs.push(*job),
                // Nothing to draw after all (e.g. only degenerate edges), or the preparation
                // failed: the glyph's rectangle would show whatever the atlas held there.
                None => self.make_blank(p.ck),
            }
        }
        if jobs.is_empty() {
            return 0;
        }
        let batch = self.generator.upload(device, encoder, &jobs);
        self.generator.encode_profiled(encoder, &batch, profiler.as_deref_mut());
        self.encode_store(device, encoder, &batch, profiler);
        self.stats.generated += jobs.len() as u64;
        self.stats.generated_texels += batch.texels as u64;
        jobs.len()
    }

    fn encode_store(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        batch: &FieldBatch,
        mut profiler: Option<&mut crate::profile::GpuProfiler>,
    ) {
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("msdfr atlas store"),
            layout: &self.store_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: crate::field::sized(&batch.glyphs.0, batch.glyphs.1),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: crate::field::sized(&batch.work.0, batch.work.1),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: crate::field::sized(&batch.field, batch.texels.max(1) as u64 * 16),
                },
                wgpu::BindGroupEntry {
                    binding: 10,
                    resource: wgpu::BindingResource::TextureView(&self.view),
                },
            ],
        });
        let scope = profiler.as_deref_mut().and_then(|p| p.scope("msdfr store"));
        let tw = scope.and_then(|s| profiler.as_deref().map(|p| p.compute_writes(s)));
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("msdfr atlas store"),
            timestamp_writes: tw,
        });
        pass.set_pipeline(&self.store);
        pass.set_bind_group(0, &bg, &[]);
        let (x, y) = batch.workgroups;
        pass.dispatch_workgroups(x, y, 1);
    }
}

/// WGSL of the pass that copies generated fields into an atlas texture of `format`.
pub(crate) fn store_shader_source(format: wgpu::TextureFormat) -> String {
    let (format_name, lo, hi) = match format {
        wgpu::TextureFormat::Rgba8Unorm => ("rgba8unorm", "0.0", "1.0"),
        wgpu::TextureFormat::Rgba16Float => ("rgba16float", "-4.0", "5.0"),
        wgpu::TextureFormat::Rgba32Float => ("rgba32float", "-1e6", "1e6"),
        f => panic!("unsupported atlas format {f:?}"),
    };
    [
        include_str!("shaders/common.wgsl").to_string(),
        include_str!("shaders/store.wgsl")
            .replace("ATLAS_FORMAT", format_name)
            .replace("STORE_LO", lo)
            .replace("STORE_HI", hi),
    ]
    .join("\n")
}

/// Lowest level (power of two in `[min_level, max_level]`) at which an MSDF of `shape`,
/// rendered at 2x, gets at most `detail_tolerance` of the ink area wrong.
pub(crate) fn probe_detail(shape: &Shape, cfg: &AtlasConfig) -> u32 {
    let b = shape.bounds();
    let config = msdfr::GeneratorConfig::default().with_threads(1);
    let mut level = cfg.min_level.max(1).next_power_of_two();
    while level < cfg.max_level {
        let l = level as f64;
        let pad = 0.5 * cfg.px_range / l;
        let w = (((b.right - b.left) + 2.0 * pad) * l).ceil() as u32 + 1;
        let h = (((b.top - b.bottom) + 2.0 * pad) * l).ceil() as u32 + 1;
        let tr = Vec2::new(-b.left + pad, -b.bottom + pad);
        let t = Transform::new(Projection::uniform(l, tr), Range::symmetric(cfg.px_range / l));
        let mut field = msdfr::Bitmap::<3>::new(w, h);
        msdfr::generate_msdf(&mut field, shape, &t, &config);
        let k = 2;
        let shown = msdfr::render_sdf(&field, w * k, h * k, cfg.px_range, t.zero_value());
        let exact = msdfr::rasterize(
            shape,
            w * k,
            h * k,
            &Projection::uniform(l * k as f64, tr),
            msdfr::FillRule::NonZero,
            1,
        );
        let (mut ink, mut wrong) = (0usize, 0usize);
        for (a, e) in shown.data().iter().zip(exact.data()) {
            ink += (*e > 0.5) as usize;
            wrong += ((a - e).abs() > 0.5) as usize;
        }
        if wrong as f64 <= cfg.detail_tolerance * ink.max(1) as f64 {
            break;
        }
        level *= 2;
    }
    level.min(cfg.max_level)
}

/// Whether a raw outline has any edge of nonzero length (otherwise it draws nothing).
fn has_ink(shape: &Shape) -> bool {
    shape.contours.iter().any(|c| {
        c.edges.iter().any(|e| {
            let p = e.segment.control_points();
            p.iter().any(|q| *q != p[0])
        })
    })
}

/// FxHash, rustc's hasher. The atlas hashes a few small integers on every glyph lookup, every
/// frame; SipHash's resistance to crafted keys buys nothing there and costs most of the lookup.
#[derive(Clone, Copy, Default)]
struct FxHasher(u64);

impl FxHasher {
    fn add(&mut self, word: u64) {
        self.0 = (self.0.rotate_left(5) ^ word).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
}

impl Hasher for FxHasher {
    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut word = [0; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            self.add(u64::from_le_bytes(word));
        }
    }

    fn write_u32(&mut self, n: u32) {
        self.add(u64::from(n));
    }

    fn write_u64(&mut self, n: u64) {
        self.add(n);
    }

    fn write_usize(&mut self, n: usize) {
        self.add(n as u64);
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

type FxBuild = BuildHasherDefault<FxHasher>;
type FxHashMap<K, V> = HashMap<K, V, FxBuild>;
