//! Distance field evaluation on the GPU: batching, upload and the fine pass.

use std::num::NonZeroU64;

use bytemuck::{Pod, Zeroable, cast_slice};
use textra::gpu::{FieldKind, GpuCorrection, GpuShape, TileBins, bin_tiles, corner_texels};
use textra::{ErrorCorrectionConfig, PreparedShape, Transform};

/// Side of the square texel tiles (one workgroup each).
pub const TILE: u32 = 8;

/// Most contours the shader's overlapping contour combiner handles; shapes with more are
/// evaluated without overlap support.
pub const MAX_OVERLAP_CONTOURS: usize = 64;

/// Per-glyph record of a batch (matches `Glyph` in `common.wgsl`, 144 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub(crate) struct GlyphRecord {
    pub edge_offset: u32,
    pub edge_count: u32,
    pub contour_offset: u32,
    pub contour_count: u32,
    pub curve_offset: u32,
    pub cand_offset: u32,
    pub tile_offset: u32,
    pub groups: u32,
    pub tiles_x: u32,
    pub tiles_y: u32,
    pub width: u32,
    pub height: u32,
    pub field_offset: u32,
    pub flags: u32,
    pub tile_size: u32,
    /// Index into the batch's `curves` of the glyph's scanline crossings (sign correction):
    /// `height + 1` row starts, then `(x, dir)` pairs, all as `f32`.
    pub scan_offset: u32,
    pub scale: [f32; 2],
    pub translate: [f32; 2],
    pub map_scale: f32,
    pub map_translate: f32,
    pub band2: f32,
    pub _pad1: f32,
    pub span: [f32; 3],
    pub radius: [f32; 3],
    pub zero: f32,
    pub improve: f32,
    pub ec_flags: u32,
    pub atlas_x: u32,
    pub atlas_y: u32,
    pub atlas_layer: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub(crate) struct WorkItem {
    pub glyph: u32,
    pub tile: u32,
}

/// One distance field to generate.
#[derive(Clone, Copy)]
pub struct FieldJob<'a> {
    /// The shape, prepared for distance queries (used for tile binning on the CPU).
    pub shape: &'a PreparedShape,
    /// Its GPU form (cache it alongside the prepared shape; see [`GpuShape::new`]).
    pub gpu: &'a GpuShape,
    pub transform: Transform,
    pub width: u32,
    pub height: u32,
    pub kind: FieldKind,
    /// msdfgen's overlapping contour combiner (only matters with several contours).
    pub overlap: bool,
    /// MSDF artifact correction (MSDF/MTSDF only; [`ErrorCorrectionConfig::DISABLED`] to skip).
    pub error_correction: ErrorCorrectionConfig,
    /// Destination `(x, y, layer)` for the atlas store pass (unused otherwise).
    pub atlas_origin: [u32; 3],
    /// Narrow band in texels (0 = off): tiles farther than this from the shape are only
    /// signed, and filled with a lower bound of their distance instead of exact values. With
    /// a band of at least half the range plus 2 texels, this only changes texels whose values
    /// lie beyond the range (clamped in fixed point textures).
    pub band: f64,
    /// msdfgen's scanline sign correction with this fill rule (as
    /// `GeneratorConfig::sign_correction` on the CPU): texels whose distance has the wrong sign
    /// (overlapping or self-intersecting contours) are flipped. `None`: off.
    pub sign_correction: Option<textra::FillRule>,
}

impl<'a> FieldJob<'a> {
    /// A job with overlap support, default error correction and no atlas placement.
    pub fn new(
        shape: &'a PreparedShape,
        gpu: &'a GpuShape,
        transform: Transform,
        width: u32,
        height: u32,
        kind: FieldKind,
    ) -> Self {
        FieldJob {
            shape,
            gpu,
            transform,
            width,
            height,
            kind,
            overlap: true,
            error_correction: ErrorCorrectionConfig::default(),
            atlas_origin: [0; 3],
            band: 0.0,
            sign_correction: None,
        }
    }
}

/// Where a job's texels are in [`FieldBatch::field`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FieldLayout {
    /// Index of the first texel (`vec4<f32>` each), rows top to bottom.
    pub offset: u32,
    pub width: u32,
    pub height: u32,
}

/// Everything the CPU computes for one job: its GPU records, tile bins and error correction
/// data. Owns its data and needs no device, so it can be built on any thread
/// ([`prepare_job`]) and uploaded later ([`FieldGenerator::upload`]).
pub struct PreparedJob {
    gpu: GpuShape,
    bins: TileBins,
    /// Row-space texel indices of protected corners (glyph-local).
    corners: Vec<u32>,
    /// The glyph record without its offsets into the batch arrays.
    record: GlyphRecord,
    /// Scanline crossings for sign correction (layout: [`GlyphRecord::scan_offset`]); empty
    /// without.
    scanlines: Vec<f32>,
}

/// CPU part of a job: tile binning, corner texels, error correction parameters.
pub fn prepare_job(job: &FieldJob<'_>) -> PreparedJob {
    prepare_job_owned(job, job.gpu.clone())
}

pub(crate) fn prepare_job_owned(job: &FieldJob<'_>, gpu: GpuShape) -> PreparedJob {
    let overlap = job.overlap && gpu.contours.len() <= MAX_OVERLAP_CONTOURS;
    let bins = bin_tiles(
        job.shape,
        &job.transform,
        job.width,
        job.height,
        TILE,
        job.kind,
        overlap,
    );
    let ec = if job.kind.channels == 3 && !gpu.edges.is_empty() {
        GpuCorrection::new(&job.transform, &job.error_correction)
    } else {
        GpuCorrection::DISABLED
    };
    let corners = if ec.flags & 2 != 0 {
        corner_texels(job.shape, &job.transform, job.width, job.height)
    } else {
        Vec::new()
    };
    let mapping = job.transform.mapping();
    let p = job.transform.projection;
    let band = job.band > 0.0;
    let rule = |r: textra::FillRule| match r {
        textra::FillRule::NonZero => 0,
        textra::FillRule::EvenOdd => 1,
        textra::FillRule::Positive => 2,
        textra::FillRule::Negative => 3,
    };
    let flags = job.kind.channels
        | (job.kind.true_distance as u32) << 2
        | (job.shape.inverse_y_axis() as u32) << 3
        | (band as u32) << 4
        | (job.sign_correction.is_some() as u32) << 5
        | job.sign_correction.map_or(0, rule) << 6;
    let scanlines = match job.sign_correction {
        Some(_) if !gpu.edges.is_empty() => scanlines(job),
        _ => Vec::new(),
    };
    let band_units = job.band / p.scale.x.min(p.scale.y).abs();
    let band2 = if band { (band_units * band_units) as f32 } else { 0.0 };
    let mut bins = bins;
    if band {
        far_tile_signs(&mut bins, job, band2);
    }
    let record = GlyphRecord {
        edge_count: gpu.edges.len() as u32,
        contour_count: gpu.contours.len() as u32,
        groups: bins.groups,
        tiles_x: bins.tiles_x,
        tiles_y: bins.tiles_y,
        width: job.width,
        height: job.height,
        flags,
        tile_size: TILE,
        scale: [p.scale.x as f32, p.scale.y as f32],
        translate: [p.translate.x as f32, p.translate.y as f32],
        map_scale: mapping.scale as f32,
        map_translate: mapping.translate as f32,
        span: ec.spans,
        radius: ec.radii,
        zero: ec.zero,
        improve: ec.min_improve_ratio,
        ec_flags: ec.flags,
        atlas_x: job.atlas_origin[0],
        atlas_y: job.atlas_origin[1],
        atlas_layer: job.atlas_origin[2],
        band2,
        ..Default::default()
    };
    PreparedJob {
        gpu,
        bins,
        corners,
        record,
        scanlines,
    }
}

/// Each texel row's crossings with the outline (rows top to bottom, as the field): `height + 1`
/// row starts, then `(x, dir)` pairs in shape units, all as `f32` (counts are exact).
fn scanlines(job: &FieldJob<'_>) -> Vec<f32> {
    let p = job.transform.projection;
    let h = job.height;
    let mut starts = Vec::with_capacity(h as usize + 1);
    let mut pairs = Vec::new();
    let mut crossings = Vec::new();
    for r in 0..h {
        let ty = if job.shape.inverse_y_axis() { r } else { h - 1 - r };
        let y = p.unproject(textra::Vec2::new(0.0, f64::from(ty) + 0.5)).y;
        job.shape.scanline(y, &mut crossings);
        starts.push((pairs.len() / 2) as f32);
        for &(x, dir) in &crossings {
            pairs.push(x as f32);
            pairs.push(dir as f32);
        }
    }
    starts.push((pairs.len() / 2) as f32);
    starts.extend(pairs);
    starts
}

/// Marks each tile the narrow band skips (`min_key > band2`, as `fine.wgsl` tests it) as inside
/// or outside, in its first group record's `_pad[1]` (1 inside, 0 outside). The tile holds no
/// edge, so the winding number at one texel holds for all of them; the sign of an evaluated
/// distance would not do (the overlapping contour combiner can get it wrong far inside partly
/// overlapping contours, and one wrong texel would fill the whole tile).
fn far_tile_signs(bins: &mut TileBins, job: &FieldJob<'_>, band2: f32) {
    let p = job.transform.projection;
    let flip = !job.shape.inverse_y_axis();
    for (t, &key) in bins.min_key.iter().enumerate() {
        if key <= band2 {
            continue;
        }
        let t = t as u32;
        let (x, r) = ((t % bins.tiles_x) * bins.tile, (t / bins.tiles_x) * bins.tile);
        let ty = if flip { job.height - 1 - r } else { r };
        let point = p.unproject(textra::Vec2::new(f64::from(x) + 0.5, f64::from(ty) + 0.5));
        let rule = job.sign_correction.unwrap_or_default();
        let inside = rule.is_filled(job.shape.winding_number(point));
        bins.ranges[(t * bins.groups) as usize]._pad[1] = u32::from(inside);
    }
}

/// A buffer reused from batch to batch, grown (by powers of two) when a batch needs more.
struct Slot {
    label: &'static str,
    usage: wgpu::BufferUsages,
    buffer: Option<wgpu::Buffer>,
}

impl Slot {
    fn new(label: &'static str, usage: wgpu::BufferUsages) -> Self {
        Slot {
            label,
            usage,
            buffer: None,
        }
    }

    /// A buffer of at least `size` bytes.
    fn ensure(&mut self, device: &wgpu::Device, size: u64) -> &wgpu::Buffer {
        let size = size.max(16);
        if self.buffer.as_ref().is_none_or(|b| b.size() < size) {
            self.buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(self.label),
                size: size.next_power_of_two().max(256),
                usage: self.usage,
                mapped_at_creation: false,
            }));
        }
        self.buffer.as_ref().unwrap()
    }

    /// Queues an upload of `bytes` (at least 16, zero padded) and returns the buffer and the
    /// bound size.
    fn write<'a>(&mut self, device: &wgpu::Device, uploads: &mut Uploads<'a>, bytes: &'a [u8]) -> (wgpu::Buffer, u64) {
        const ZERO: [u8; 16] = [0; 16];
        let bytes = if bytes.is_empty() { &ZERO[..] } else { bytes };
        let b = self.ensure(device, bytes.len() as u64).clone();
        uploads.parts.push((b.clone(), bytes));
        (b, bytes.len() as u64)
    }
}

/// Buffer uploads recorded into the command encoder, so they happen in order with the passes
/// that use them. (`Queue::write_buffer` would apply every upload before the whole
/// submission, so a later batch recorded into the same submission would overwrite the
/// buffers of an earlier one before its passes ran.)
#[derive(Default)]
struct Uploads<'a> {
    parts: Vec<(wgpu::Buffer, &'a [u8])>,
}

impl Uploads<'_> {
    /// Copies everything through one staging buffer.
    fn record(self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder) {
        // Every part is a whole number of 4-byte records, as copies require.
        let size: u64 = self.parts.iter().map(|(_, b)| b.len() as u64).sum();
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("textra upload"),
            size: size.max(wgpu::COPY_BUFFER_ALIGNMENT),
            usage: wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: true,
        });
        {
            let mut view = staging.get_mapped_range_mut(..).expect("mapped at creation");
            let mut offset = 0;
            for (_, bytes) in &self.parts {
                view.slice(offset..offset + bytes.len()).copy_from_slice(bytes);
                offset += bytes.len();
            }
        }
        staging.unmap();
        let mut offset = 0;
        for (buffer, bytes) in &self.parts {
            encoder.copy_buffer_to_buffer(&staging, offset, buffer, 0, Some(bytes.len() as u64));
            offset += bytes.len() as u64;
        }
    }
}

/// The generator's buffers, reused from batch to batch.
struct BufferSet {
    glyphs: Slot,
    work: Slot,
    edges: Slot,
    contours: Slot,
    curves: Slot,
    tiles: Slot,
    cands: Slot,
    corners: Slot,
    field: Slot,
    stencil: Slot,
}

impl BufferSet {
    fn new() -> Self {
        let up = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST;
        let rw = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC;
        BufferSet {
            glyphs: Slot::new("textra glyphs", up),
            work: Slot::new("textra work", up),
            edges: Slot::new("textra edges", up),
            contours: Slot::new("textra contours", up),
            curves: Slot::new("textra curves", up),
            tiles: Slot::new("textra tiles", up),
            cands: Slot::new("textra candidates", up),
            corners: Slot::new("textra corners", up),
            field: Slot::new("textra field", rw),
            stencil: Slot::new("textra stencil", rw),
        }
    }
}

pub(crate) fn sized(buffer: &wgpu::Buffer, size: u64) -> wgpu::BindingResource<'_> {
    wgpu::BindingResource::Buffer(wgpu::BufferBinding {
        buffer,
        offset: 0,
        size: NonZeroU64::new(size),
    })
}

/// The uploaded data of a batch of jobs and its output buffer.
pub struct FieldBatch {
    /// `vec4<f32>` per texel: channels as configured (MSDF in xyz, true distance in w). The
    /// buffer is reused by later batches (whose commands overwrite it) and may be larger than
    /// this batch.
    pub field: wgpu::Buffer,
    pub layouts: Vec<FieldLayout>,
    pub texels: u32,
    /// Per-glyph records and the work list, with their bound sizes (also bound by later
    /// passes such as the atlas store).
    pub(crate) glyphs: (wgpu::Buffer, u64),
    pub(crate) work: (wgpu::Buffer, u64),
    bind_group: wgpu::BindGroup,
    ec_bind_group: Option<wgpu::BindGroup>,
    pub(crate) stencil: Option<(wgpu::Buffer, u64)>,
    pub(crate) workgroups: (u32, u32),
    corner_workgroups: (u32, u32),
}

/// Compiled pipelines for the distance and error correction passes.
pub struct FieldGenerator {
    buffers: BufferSet,
    fine: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    ec_layout: wgpu::BindGroupLayout,
    ec_corners: wgpu::ComputePipeline,
    ec_protect: wgpu::ComputePipeline,
    ec_find: wgpu::ComputePipeline,
    ec_apply: wgpu::ComputePipeline,
}

/// WGSL source of all passes (shared declarations + entry points).
pub fn shader_source() -> String {
    let evaluate = include_str!("shaders/evaluate.wgsl");
    let variant = |name: &str, scan: &str| evaluate.replace("EVALUATE", name).replace("SCAN", scan);
    [
        include_str!("shaders/common.wgsl").to_string(),
        variant("evaluate_tile", "scan_tile"),
        variant("evaluate_all", "scan_all"),
        include_str!("shaders/fine.wgsl").to_string(),
        include_str!("shaders/ec.wgsl").to_string(),
    ]
    .join("\n")
}

pub(crate) fn storage_entry(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

/// Splits `n` workgroups over two dimensions (each at most 65535).
pub(crate) fn split_workgroups(n: u32) -> (u32, u32) {
    let x = n.clamp(1, 65535);
    (x, n.div_ceil(x).max(1))
}

impl FieldGenerator {
    pub fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("textra"),
            source: wgpu::ShaderSource::Wgsl(shader_source().into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("textra distance"),
            entries: &[
                storage_entry(0, true),
                storage_entry(1, true),
                storage_entry(2, true),
                storage_entry(3, true),
                storage_entry(4, true),
                storage_entry(5, true),
                storage_entry(6, true),
                storage_entry(7, false),
            ],
        });
        let ec_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("textra error correction"),
            entries: &[
                storage_entry(0, true),
                storage_entry(1, true),
                storage_entry(2, true),
                storage_entry(3, true),
                storage_entry(4, true),
                storage_entry(7, false),
                storage_entry(8, false),
                storage_entry(9, true),
            ],
        });
        let pipeline = |layout: &wgpu::BindGroupLayout, entry: &str| {
            let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(entry),
                bind_group_layouts: &[Some(layout)],
                immediate_size: 0,
            });
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pl),
                module: &module,
                entry_point: Some(entry),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            })
        };
        FieldGenerator {
            buffers: BufferSet::new(),
            fine: pipeline(&layout, "fine"),
            ec_corners: pipeline(&ec_layout, "ec_corners"),
            ec_protect: pipeline(&ec_layout, "ec_protect"),
            ec_find: pipeline(&ec_layout, "ec_find"),
            ec_apply: pipeline(&ec_layout, "ec_apply"),
            layout,
            ec_layout,
        }
    }

    /// Bins a batch of jobs and records their upload into `encoder` (one dispatch evaluates
    /// all of them). Record the batch's passes ([`Self::encode`]) into the same encoder, or a
    /// later one, before preparing the next batch: batches share the generator's buffers.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        jobs: &[FieldJob<'_>],
    ) -> FieldBatch {
        let prepared: Vec<PreparedJob> = jobs.iter().map(prepare_job).collect();
        self.upload(device, encoder, &prepared)
    }

    /// [`Self::prepare`] for jobs whose CPU part is done (see [`prepare_job`]).
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        jobs: &[PreparedJob],
    ) -> FieldBatch {
        let mut glyphs = Vec::with_capacity(jobs.len());
        let mut work = Vec::new();
        let mut edges = Vec::new();
        let mut contours = Vec::new();
        let mut curves: Vec<f32> = Vec::new();
        let mut tile_groups = Vec::new();
        let mut cands: Vec<u32> = Vec::new();
        let mut layouts = Vec::with_capacity(jobs.len());
        let mut texels = 0u32;
        let mut corners: Vec<u32> = Vec::new();
        let mut any_ec = false;
        for (gi, job) in jobs.iter().enumerate() {
            let (gpu, bins) = (&job.gpu, &job.bins);
            let mut record = job.record;
            record.edge_offset = edges.len() as u32;
            record.contour_offset = contours.len() as u32;
            record.curve_offset = curves.len() as u32;
            record.scan_offset = (curves.len() + gpu.curves.len()) as u32;
            record.cand_offset = (cands.len() / 2) as u32;
            record.tile_offset = tile_groups.len() as u32;
            record.field_offset = texels;
            if record.ec_flags != 0 {
                any_ec = true;
                corners.extend(job.corners.iter().map(|t| t + texels));
            }
            glyphs.push(record);
            layouts.push(FieldLayout {
                offset: texels,
                width: record.width,
                height: record.height,
            });
            texels += record.width * record.height;
            edges.extend_from_slice(&gpu.edges);
            contours.extend_from_slice(&gpu.contours);
            curves.extend_from_slice(&gpu.curves);
            curves.extend_from_slice(&job.scanlines);
            // The tile's smallest edge key rides along in its first group record (narrow band).
            let first = tile_groups.len();
            tile_groups.extend_from_slice(&bins.ranges);
            for (t, &k) in bins.min_key.iter().enumerate() {
                tile_groups[first + t * bins.groups as usize]._pad[0] = k.to_bits();
            }
            for (&i, &k) in bins.indices.iter().zip(&bins.keys) {
                cands.push(i);
                cands.push(k.to_bits());
            }
            if gpu.edges.is_empty() {
                continue;
            }
            for t in 0..bins.tiles_x * bins.tiles_y {
                work.push(WorkItem {
                    glyph: gi as u32,
                    tile: t,
                });
            }
        }
        let set = &mut self.buffers;
        let mut uploads = Uploads::default();
        let up = &mut uploads;
        let glyph_buf = set.glyphs.write(device, up, cast_slice(&glyphs));
        let work_buf = set.work.write(device, up, cast_slice(&work));
        let edge_buf = set.edges.write(device, up, cast_slice(&edges));
        let contour_buf = set.contours.write(device, up, cast_slice(&contours));
        let curve_buf = set.curves.write(device, up, cast_slice(&curves));
        let tile_buf = set.tiles.write(device, up, cast_slice(&tile_groups));
        let cand_buf = set.cands.write(device, up, cast_slice(&cands));
        let field_size = (texels.max(1) as u64) * 16;
        let field = set.field.ensure(device, field_size).clone();
        let (ec_bind_group, stencil) = if any_ec {
            let stencil_size = (texels.max(1) as u64) * 4;
            let stencil = set.stencil.ensure(device, stencil_size).clone();
            if corners.is_empty() {
                corners.push(u32::MAX);
            }
            let corner_buf = set.corners.write(device, up, cast_slice(&corners));
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("textra error correction"),
                layout: &self.ec_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: sized(&glyph_buf.0, glyph_buf.1),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: sized(&work_buf.0, work_buf.1),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: sized(&edge_buf.0, edge_buf.1),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: sized(&contour_buf.0, contour_buf.1),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: sized(&curve_buf.0, curve_buf.1),
                    },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: sized(&field, field_size),
                    },
                    wgpu::BindGroupEntry {
                        binding: 8,
                        resource: sized(&stencil, stencil_size),
                    },
                    wgpu::BindGroupEntry {
                        binding: 9,
                        resource: sized(&corner_buf.0, corner_buf.1),
                    },
                ],
            });
            (Some(bg), Some((stencil, stencil_size)))
        } else {
            (None, None)
        };
        uploads.record(device, encoder);
        let corner_workgroups = split_workgroups((corners.len() as u32).div_ceil(64));
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("textra distance"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: sized(&glyph_buf.0, glyph_buf.1),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: sized(&work_buf.0, work_buf.1),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: sized(&edge_buf.0, edge_buf.1),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: sized(&contour_buf.0, contour_buf.1),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: sized(&curve_buf.0, curve_buf.1),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: sized(&tile_buf.0, tile_buf.1),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: sized(&cand_buf.0, cand_buf.1),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: sized(&field, field_size),
                },
            ],
        });
        FieldBatch {
            field,
            layouts,
            texels,
            glyphs: glyph_buf,
            work: work_buf,
            bind_group,
            ec_bind_group,
            stencil,
            workgroups: split_workgroups(work.len() as u32),
            corner_workgroups,
        }
    }

    /// Records the distance pass of `batch`, followed by error correction where configured.
    pub fn encode(&self, encoder: &mut wgpu::CommandEncoder, batch: &FieldBatch) {
        self.encode_profiled(encoder, batch, None);
    }

    /// [`Self::encode`], timing the distance pass ("textra distance") and error correction
    /// ("textra correction") as separate passes when a profiler is given.
    pub fn encode_profiled(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        batch: &FieldBatch,
        mut profiler: Option<&mut crate::profile::GpuProfiler>,
    ) {
        if let Some((stencil, size)) = &batch.stencil {
            encoder.clear_buffer(stencil, 0, Some(*size));
        }
        let (wx, wy) = batch.workgroups;
        {
            let scope = profiler.as_deref_mut().and_then(|p| p.scope("textra distance"));
            let tw = scope.and_then(|s| profiler.as_deref().map(|p| p.compute_writes(s)));
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("textra distance"),
                timestamp_writes: tw,
            });
            pass.set_pipeline(&self.fine);
            pass.set_bind_group(0, &batch.bind_group, &[]);
            pass.dispatch_workgroups(wx, wy, 1);
        }
        if let Some(bg) = &batch.ec_bind_group {
            let scope = profiler.as_deref_mut().and_then(|p| p.scope("textra correction"));
            let tw = scope.and_then(|s| profiler.as_deref().map(|p| p.compute_writes(s)));
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("textra correction"),
                timestamp_writes: tw,
            });
            pass.set_bind_group(0, bg, &[]);
            pass.set_pipeline(&self.ec_corners);
            pass.dispatch_workgroups(batch.corner_workgroups.0, batch.corner_workgroups.1, 1);
            for p in [&self.ec_protect, &self.ec_find, &self.ec_apply] {
                pass.set_pipeline(p);
                pass.dispatch_workgroups(wx, wy, 1);
            }
        }
    }
}

/// Copies the whole field of a batch back to the CPU (blocking; for tests and tools).
#[cfg(not(target_arch = "wasm32"))]
pub fn read_field(device: &wgpu::Device, queue: &wgpu::Queue, batch: &FieldBatch) -> Vec<[f32; 4]> {
    let words = read_buffer(device, queue, &batch.field);
    let mut out: Vec<[f32; 4]> = words
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| {
            [
                f32::from_bits(c[0]),
                f32::from_bits(c[1]),
                f32::from_bits(c[2]),
                f32::from_bits(c[3]),
            ]
        })
        .collect();
    out.truncate(batch.texels as usize);
    out
}

/// Error correction flags per texel (1 error, 2 protected, 4 needs check); for debugging.
#[cfg(not(target_arch = "wasm32"))]
pub fn read_stencil(device: &wgpu::Device, queue: &wgpu::Queue, batch: &FieldBatch) -> Vec<u32> {
    match &batch.stencil {
        Some((s, size)) => {
            let mut v = read_buffer(device, queue, s);
            v.truncate((*size / 4) as usize);
            v
        }
        None => vec![0; batch.texels as usize],
    }
}

/// Copies layer `layer` of `texture` back (native, blocking): rows top to bottom, tightly
/// packed, in the texture's own format (RGBA8: 4 bytes per texel). For tests, screenshots and
/// debugging views of the atlas or of a render target.
#[cfg(not(target_arch = "wasm32"))]
pub fn read_texture_layer(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture, layer: u32) -> Vec<u8> {
    let (w, h) = (texture.width(), texture.height());
    let texel = texture.format().block_copy_size(None).expect("a color format");
    let row = (w * texel).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("textra readback"),
        size: u64::from(row) * u64::from(h),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d { x: 0, y: 0, z: layer },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &staging,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([enc.finish()]);
    let slice = staging.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let data = slice.get_mapped_range().unwrap();
    let tight = (w * texel) as usize;
    let out = data.chunks(row as usize).flat_map(|r| &r[..tight]).copied().collect();
    drop(data);
    staging.unmap();
    out
}

#[cfg(not(target_arch = "wasm32"))]
fn read_buffer(device: &wgpu::Device, queue: &wgpu::Queue, buffer: &wgpu::Buffer) -> Vec<u32> {
    let size = buffer.size();
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("textra readback"),
        size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    enc.copy_buffer_to_buffer(buffer, 0, &staging, 0, size);
    queue.submit([enc.finish()]);
    let slice = staging.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let data = slice.get_mapped_range().unwrap();
    let out = data
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    drop(data);
    staging.unmap();
    out
}
