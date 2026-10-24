//! Distance field jobs and batches: everything the GPU kernels need, prepared on the CPU and
//! independent of any graphics API.
//!
//! A batch runs the kernels of [`shader_source`] (and, for an atlas, [`Kernel::Store`]) in
//! the order [`Batch::kernels`] lists them. Each kernel binds the buffers of [`Binding`] in
//! group 0: a backend creates them, fills the inputs with [`Batch::bytes`], sizes the outputs
//! with [`Batch::output_size`] and dispatches the listed workgroup counts.

use bytemuck::{Pod, Zeroable, cast_slice};
use glyphux::gpu::{
    FieldKind, GpuContour, GpuCorrection, GpuEdge, GpuShape, GpuTileGroup, TileBins, bin_tiles, corner_texels,
};
use glyphux::{ErrorCorrectionConfig, PreparedShape, Transform};

/// Side of the square texel tiles (one workgroup each).
pub const TILE: u32 = 8;

/// Most contours the shader's overlapping contour combiner handles; shapes with more are
/// evaluated without overlap support.
pub const MAX_OVERLAP_CONTOURS: usize = 64;

/// Per-glyph record of a batch (matches `Glyph` in `common.wgsl`, 144 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct GlyphRecord {
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

/// One tile of one glyph (the per-tile kernels' work list).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct WorkItem {
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
    pub sign_correction: Option<glyphux::FillRule>,
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

/// Where a job's texels are in a batch's field output ([`Binding::Field`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FieldLayout {
    /// Index of the first texel (`vec4<f32>` each), rows top to bottom.
    pub offset: u32,
    pub width: u32,
    pub height: u32,
}

/// Everything the CPU computes for one job: its GPU records, tile bins and error correction
/// data. Owns its data and needs no device, so it can be built on any thread
/// ([`prepare_job`]) and packed into a [`Batch`] later ([`pack`]).
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
    let rule = |r: glyphux::FillRule| match r {
        glyphux::FillRule::NonZero => 0,
        glyphux::FillRule::EvenOdd => 1,
        glyphux::FillRule::Positive => 2,
        glyphux::FillRule::Negative => 3,
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
        let y = p.unproject(glyphux::Vec2::new(0.0, f64::from(ty) + 0.5)).y;
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
        let point = p.unproject(glyphux::Vec2::new(f64::from(x) + 0.5, f64::from(ty) + 0.5));
        let rule = job.sign_correction.unwrap_or_default();
        let inside = rule.is_filled(job.shape.winding_number(point));
        bins.ranges[(t * bins.groups) as usize]._pad[1] = u32::from(inside);
    }
}

/// WGSL source of the field kernels (shared declarations + entry points; see [`Kernel`]).
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

/// Splits `n` workgroups over two dimensions (each at most 65535): all of them along x up to
/// 65535, else rows of exactly 65535. The kernels count on it (`DISPATCH_X` in
/// `common.wgsl`) instead of reading the dispatch size.
pub fn split_workgroups(n: u32) -> (u32, u32) {
    let x = n.clamp(1, 65535);
    (x, n.div_ceil(x).max(1))
}

/// A buffer or texture the kernels bind, at binding [`Self::index`] of group 0.
///
/// Bind inputs at exactly their size ([`Batch::bytes`]): the kernels take the lengths of the
/// work list and of the corners from their bindings (`arrayLength`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Binding {
    /// [`GlyphRecord`] per job.
    Glyphs,
    /// [`WorkItem`] per tile.
    Work,
    Edges,
    Contours,
    /// Curve data, then each glyph's scanline crossings.
    Curves,
    /// Tile edge ranges.
    Tiles,
    /// Candidate edges per tile with their culling keys.
    Candidates,
    /// Output: `vec4<f32>` per texel ([`FieldLayout`] says where each job's are).
    Field,
    /// Error correction flags, one atomic `u32` per texel; zeroed before the corner kernel.
    Stencil,
    /// Protected corner texels (error correction).
    Corners,
    /// The atlas pages: a write-only storage texture array ([`Kernel::Store`] only).
    Atlas,
}

impl Binding {
    /// Every binding, in binding order.
    pub const ALL: [Binding; 11] = [
        Binding::Glyphs,
        Binding::Work,
        Binding::Edges,
        Binding::Contours,
        Binding::Curves,
        Binding::Tiles,
        Binding::Candidates,
        Binding::Field,
        Binding::Stencil,
        Binding::Corners,
        Binding::Atlas,
    ];

    /// Binding number in group 0.
    pub const fn index(self) -> u32 {
        self as u32
    }

    /// Whether the kernels only read it (storage buffers declared `read`).
    pub const fn read_only(self) -> bool {
        !matches!(self, Binding::Field | Binding::Stencil | Binding::Atlas)
    }
}

/// A compute entry point of the field shader ([`shader_source`]) or of the atlas store shader.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kernel {
    /// Distances of every texel of every tile (`fine`).
    Distance,
    /// Error correction: protect corners, protect edges, find artifacts, apply.
    CorrectCorners,
    CorrectProtect,
    CorrectFind,
    CorrectApply,
    /// Copies the field into the atlas pages (`store`, of the atlas store shader).
    Store,
}

impl Kernel {
    /// Every kernel, in the order a batch runs them ([`Batch::kernels`], then the store).
    pub const ALL: [Kernel; 6] = [
        Kernel::Distance,
        Kernel::CorrectCorners,
        Kernel::CorrectProtect,
        Kernel::CorrectFind,
        Kernel::CorrectApply,
        Kernel::Store,
    ];

    /// The WGSL entry point.
    pub const fn entry_point(self) -> &'static str {
        match self {
            Kernel::Distance => "fine",
            Kernel::CorrectCorners => "ec_corners",
            Kernel::CorrectProtect => "ec_protect",
            Kernel::CorrectFind => "ec_find",
            Kernel::CorrectApply => "ec_apply",
            Kernel::Store => "store",
        }
    }

    /// What it binds. (WebGPU allows 8 storage buffers per stage: each kernel binds at most
    /// that many.)
    pub const fn bindings(self) -> &'static [Binding] {
        use Binding as B;
        match self {
            Kernel::Distance => &[
                B::Glyphs,
                B::Work,
                B::Edges,
                B::Contours,
                B::Curves,
                B::Tiles,
                B::Candidates,
                B::Field,
            ],
            Kernel::CorrectCorners | Kernel::CorrectProtect | Kernel::CorrectFind | Kernel::CorrectApply => &[
                B::Glyphs,
                B::Work,
                B::Edges,
                B::Contours,
                B::Curves,
                B::Field,
                B::Stencil,
                B::Corners,
            ],
            Kernel::Store => &[B::Glyphs, B::Work, B::Field, B::Atlas],
        }
    }
}

/// Prepared jobs packed into the arrays the kernels bind ([`pack`]).
pub struct Batch {
    /// Where each job's texels are in the field output, in job order.
    pub layouts: Vec<FieldLayout>,
    /// Texels of all jobs.
    pub texels: u32,
    /// Workgroups of the per-tile kernels (all but [`Kernel::CorrectCorners`]), split over x
    /// and y.
    pub workgroups: (u32, u32),
    /// Workgroups of [`Kernel::CorrectCorners`].
    pub corner_workgroups: (u32, u32),
    /// Whether any job uses error correction (else [`Binding::Stencil`] and
    /// [`Binding::Corners`] are unused).
    pub error_correction: bool,
    glyphs: Vec<GlyphRecord>,
    work: Vec<WorkItem>,
    edges: Vec<GpuEdge>,
    contours: Vec<GpuContour>,
    curves: Vec<f32>,
    tiles: Vec<GpuTileGroup>,
    cands: Vec<u32>,
    corners: Vec<u32>,
}

impl Batch {
    /// The contents of an input binding (`None` for [`Binding::Field`], [`Binding::Stencil`]
    /// and [`Binding::Atlas`]). Empty arrays are given as 16 zero bytes, as zero-sized
    /// bindings are not allowed.
    pub fn bytes(&self, binding: Binding) -> Option<&[u8]> {
        const ZERO: [u8; 16] = [0; 16];
        let bytes: &[u8] = match binding {
            Binding::Glyphs => cast_slice(&self.glyphs),
            Binding::Work => cast_slice(&self.work),
            Binding::Edges => cast_slice(&self.edges),
            Binding::Contours => cast_slice(&self.contours),
            Binding::Curves => cast_slice(&self.curves),
            Binding::Tiles => cast_slice(&self.tiles),
            Binding::Candidates => cast_slice(&self.cands),
            Binding::Corners => cast_slice(&self.corners),
            Binding::Field | Binding::Stencil | Binding::Atlas => return None,
        };
        Some(if bytes.is_empty() { &ZERO } else { bytes })
    }

    /// Bytes to bind for an output buffer: [`Binding::Field`] (16 per texel) and
    /// [`Binding::Stencil`] (4 per texel; zero it before the kernels run). `None` for the rest.
    pub fn output_size(&self, binding: Binding) -> Option<u64> {
        let texels = u64::from(self.texels.max(1));
        match binding {
            Binding::Field => Some(texels * 16),
            Binding::Stencil => Some(texels * 4),
            _ => None,
        }
    }

    /// The kernels to run, in order, with their workgroup counts (x, y). [`Kernel::Store`]
    /// is not included: an atlas runs it last, with [`Self::workgroups`].
    pub fn kernels(&self) -> impl Iterator<Item = (Kernel, (u32, u32))> + '_ {
        let correct = [
            (Kernel::CorrectCorners, self.corner_workgroups),
            (Kernel::CorrectProtect, self.workgroups),
            (Kernel::CorrectFind, self.workgroups),
            (Kernel::CorrectApply, self.workgroups),
        ];
        std::iter::once((Kernel::Distance, self.workgroups))
            .chain(correct.into_iter().filter(|_| self.error_correction))
    }
}

/// Packs prepared jobs into one batch.
pub fn pack(jobs: &[PreparedJob]) -> Batch {
    let mut glyphs = Vec::with_capacity(jobs.len());
    let mut work = Vec::new();
    let mut edges = Vec::new();
    let mut contours = Vec::new();
    let mut curves: Vec<f32> = Vec::new();
    let mut tiles = Vec::new();
    let mut cands: Vec<u32> = Vec::new();
    let mut layouts = Vec::with_capacity(jobs.len());
    let mut texels = 0u32;
    let mut corners: Vec<u32> = Vec::new();
    let mut error_correction = false;
    for (gi, job) in jobs.iter().enumerate() {
        let (gpu, bins) = (&job.gpu, &job.bins);
        let mut record = job.record;
        record.edge_offset = edges.len() as u32;
        record.contour_offset = contours.len() as u32;
        record.curve_offset = curves.len() as u32;
        record.scan_offset = (curves.len() + gpu.curves.len()) as u32;
        record.cand_offset = (cands.len() / 2) as u32;
        record.tile_offset = tiles.len() as u32;
        record.field_offset = texels;
        if record.ec_flags != 0 {
            error_correction = true;
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
        let first = tiles.len();
        tiles.extend_from_slice(&bins.ranges);
        for (t, &k) in bins.min_key.iter().enumerate() {
            tiles[first + t * bins.groups as usize]._pad[0] = k.to_bits();
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
    if error_correction && corners.is_empty() {
        corners.push(u32::MAX);
    }
    Batch {
        layouts,
        texels,
        workgroups: split_workgroups(work.len() as u32),
        corner_workgroups: split_workgroups((corners.len() as u32).div_ceil(64)),
        error_correction,
        glyphs,
        work,
        edges,
        contours,
        curves,
        tiles,
        cands,
        corners,
    }
}

#[cfg(test)]
mod tests {
    /// The kernels number work items as `wid.x + wid.y * 65535` (`DISPATCH_X`).
    #[test]
    fn dispatch_rows_are_full() {
        for n in [0, 1, 100, 65_535, 65_536, 200_000] {
            let (x, y) = super::split_workgroups(n);
            assert!(x == n.max(1) || x == 65_535, "{n}: {x}");
            assert!(y == 1 || x == 65_535, "{n}: {x} x {y}");
            assert!(u64::from(x) * u64::from(y) >= u64::from(n));
        }
    }
}
