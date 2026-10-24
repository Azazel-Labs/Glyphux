//! Running field batches with wgpu: the kernels' buffers, pipelines and passes.

use std::collections::HashMap;
use std::num::NonZeroU64;

pub use glyphux_gpu::field::*;

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
            label: Some("glyphux upload"),
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

fn label(binding: Binding) -> &'static str {
    match binding {
        Binding::Glyphs => "glyphux glyphs",
        Binding::Work => "glyphux work",
        Binding::Edges => "glyphux edges",
        Binding::Contours => "glyphux contours",
        Binding::Curves => "glyphux curves",
        Binding::Tiles => "glyphux tiles",
        Binding::Candidates => "glyphux candidates",
        Binding::Field => "glyphux field",
        Binding::Stencil => "glyphux stencil",
        Binding::Corners => "glyphux corners",
        Binding::Atlas => "glyphux atlas",
    }
}

pub(crate) fn sized(buffer: &wgpu::Buffer, size: u64) -> wgpu::BindingResource<'_> {
    wgpu::BindingResource::Buffer(wgpu::BufferBinding {
        buffer,
        offset: 0,
        size: NonZeroU64::new(size),
    })
}

/// The bind group layout of `kernel`; `atlas` is the storage texture format for
/// [`Binding::Atlas`].
pub(crate) fn bind_group_layout(
    device: &wgpu::Device,
    kernel: Kernel,
    atlas: Option<wgpu::TextureFormat>,
) -> wgpu::BindGroupLayout {
    let entries: Vec<wgpu::BindGroupLayoutEntry> = kernel
        .bindings()
        .iter()
        .map(|&b| wgpu::BindGroupLayoutEntry {
            binding: b.index(),
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: match (b, atlas) {
                (Binding::Atlas, Some(format)) => wgpu::BindingType::StorageTexture {
                    access: wgpu::StorageTextureAccess::WriteOnly,
                    format,
                    view_dimension: wgpu::TextureViewDimension::D2Array,
                },
                (Binding::Atlas, None) => panic!("the atlas binding needs a texture format"),
                _ => wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage {
                        read_only: b.read_only(),
                    },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
            },
            count: None,
        })
        .collect();
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(kernel.entry_point()),
        entries: &entries,
    })
}

/// A compute pipeline for `kernel` of `module` with `layout`.
pub(crate) fn pipeline(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    kernel: Kernel,
    layout: &wgpu::BindGroupLayout,
) -> wgpu::ComputePipeline {
    let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(kernel.entry_point()),
        bind_group_layouts: &[Some(layout)],
        immediate_size: 0,
    });
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(kernel.entry_point()),
        layout: Some(&pl),
        module,
        entry_point: Some(kernel.entry_point()),
        compilation_options: wgpu::PipelineCompilationOptions::default(),
        cache: None,
    })
}

/// A batch uploaded into the generator's buffers, ready for [`FieldGenerator::encode`].
pub struct FieldBatch {
    /// `vec4<f32>` per texel: channels as configured (MSDF in xyz, true distance in w). The
    /// buffer is reused by later batches (whose commands overwrite it) and may be larger than
    /// this batch.
    pub field: wgpu::Buffer,
    pub layouts: Vec<FieldLayout>,
    pub texels: u32,
    /// The buffer and bound size of each binding (also bound by later passes such as the
    /// atlas store).
    pub(crate) buffers: HashMap<Binding, (wgpu::Buffer, u64)>,
    bind_group: wgpu::BindGroup,
    ec_bind_group: Option<wgpu::BindGroup>,
    kernels: Vec<(Kernel, (u32, u32))>,
    pub(crate) workgroups: (u32, u32),
}

impl FieldBatch {
    /// The entry for `binding` of a bind group.
    pub(crate) fn entry(&self, binding: Binding) -> wgpu::BindGroupEntry<'_> {
        let (buffer, size) = &self.buffers[&binding];
        wgpu::BindGroupEntry {
            binding: binding.index(),
            resource: sized(buffer, *size),
        }
    }
}

/// Compiled pipelines for the distance and error correction passes.
pub struct FieldGenerator {
    buffers: HashMap<Binding, Slot>,
    layout: wgpu::BindGroupLayout,
    ec_layout: wgpu::BindGroupLayout,
    pipelines: HashMap<Kernel, wgpu::ComputePipeline>,
}

impl FieldGenerator {
    pub fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("glyphux"),
            source: wgpu::ShaderSource::Wgsl(shader_source().into()),
        });
        let layout = bind_group_layout(device, Kernel::Distance, None);
        // The error correction kernels share one layout.
        let ec_layout = bind_group_layout(device, Kernel::CorrectCorners, None);
        let mut pipelines = HashMap::new();
        pipelines.insert(Kernel::Distance, pipeline(device, &module, Kernel::Distance, &layout));
        for k in [
            Kernel::CorrectCorners,
            Kernel::CorrectProtect,
            Kernel::CorrectFind,
            Kernel::CorrectApply,
        ] {
            pipelines.insert(k, pipeline(device, &module, k, &ec_layout));
        }
        let up = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST;
        let rw = up | wgpu::BufferUsages::COPY_SRC;
        let buffers = Binding::ALL
            .into_iter()
            .filter(|&b| b != Binding::Atlas)
            .map(|b| (b, Slot::new(label(b), if b.read_only() { up } else { rw })))
            .collect();
        FieldGenerator {
            buffers,
            layout,
            ec_layout,
            pipelines,
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
        self.upload_batch(device, encoder, &pack(jobs))
    }

    /// Records the upload of a packed batch.
    pub fn upload_batch(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        batch: &Batch,
    ) -> FieldBatch {
        let mut uploads = Uploads::default();
        let mut buffers = HashMap::new();
        for b in Binding::ALL {
            if matches!(b, Binding::Stencil | Binding::Corners) && !batch.error_correction {
                continue;
            }
            let Some(slot) = self.buffers.get_mut(&b) else {
                continue;
            };
            if let Some(bytes) = batch.bytes(b) {
                let buffer = slot.ensure(device, bytes.len() as u64).clone();
                uploads.parts.push((buffer.clone(), bytes));
                buffers.insert(b, (buffer, bytes.len() as u64));
            } else if let Some(size) = batch.output_size(b) {
                buffers.insert(b, (slot.ensure(device, size).clone(), size));
            }
        }
        uploads.record(device, encoder);
        let group = |label: &str, layout: &wgpu::BindGroupLayout, kernel: Kernel| {
            let entries: Vec<wgpu::BindGroupEntry<'_>> = kernel
                .bindings()
                .iter()
                .map(|b| {
                    let (buffer, size) = &buffers[b];
                    wgpu::BindGroupEntry {
                        binding: b.index(),
                        resource: sized(buffer, *size),
                    }
                })
                .collect();
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout,
                entries: &entries,
            })
        };
        let bind_group = group("glyphux distance", &self.layout, Kernel::Distance);
        let ec_bind_group = batch
            .error_correction
            .then(|| group("glyphux error correction", &self.ec_layout, Kernel::CorrectCorners));
        FieldBatch {
            field: buffers[&Binding::Field].0.clone(),
            layouts: batch.layouts.clone(),
            texels: batch.texels,
            buffers,
            bind_group,
            ec_bind_group,
            kernels: batch.kernels().collect(),
            workgroups: batch.workgroups,
        }
    }

    /// Records the distance pass of `batch`, followed by error correction where configured.
    pub fn encode(&self, encoder: &mut wgpu::CommandEncoder, batch: &FieldBatch) {
        self.encode_profiled(encoder, batch, None);
    }

    /// [`Self::encode`], timing the distance pass ("glyphux distance") and error correction
    /// ("glyphux correction") as separate passes when a profiler is given.
    pub fn encode_profiled(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        batch: &FieldBatch,
        mut profiler: Option<&mut crate::profile::GpuProfiler>,
    ) {
        if let Some((stencil, size)) = batch.buffers.get(&Binding::Stencil) {
            encoder.clear_buffer(stencil, 0, Some(*size));
        }
        let (distance, correction): (Vec<_>, Vec<_>) =
            batch.kernels.iter().copied().partition(|(k, _)| *k == Kernel::Distance);
        for (label, kernels, bind_group) in [
            ("glyphux distance", distance, Some(&batch.bind_group)),
            ("glyphux correction", correction, batch.ec_bind_group.as_ref()),
        ] {
            let Some(bind_group) = bind_group.filter(|_| !kernels.is_empty()) else {
                continue;
            };
            let scope = profiler.as_deref_mut().and_then(|p| p.scope(label));
            let tw = scope.and_then(|s| profiler.as_deref().map(|p| p.compute_writes(s)));
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some(label),
                timestamp_writes: tw,
            });
            pass.set_bind_group(0, bind_group, &[]);
            for (kernel, (x, y)) in kernels {
                pass.set_pipeline(&self.pipelines[&kernel]);
                pass.dispatch_workgroups(x, y, 1);
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
    match batch.buffers.get(&Binding::Stencil) {
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
    read_texture_rows(device, queue, texture, layer, 0..texture.height())
}

/// [`read_texture_layer`] for the texel rows `rows` of the layer only (e.g. those listed by
/// [`glyphux_gpu::GlyphAtlas::take_written_rows`]).
#[cfg(not(target_arch = "wasm32"))]
pub fn read_texture_rows(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    layer: u32,
    rows: std::ops::Range<u32>,
) -> Vec<u8> {
    let (w, h) = (texture.width(), rows.len() as u32);
    if h == 0 {
        return Vec::new();
    }
    let texel = texture.format().block_copy_size(None).expect("a color format");
    let row = (w * texel).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("glyphux readback"),
        size: u64::from(row) * u64::from(h),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: 0,
                y: rows.start,
                z: layer,
            },
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
        label: Some("glyphux readback"),
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
