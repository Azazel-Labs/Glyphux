//! The glyph atlas on a wgpu device: [`glyphux_gpu::GlyphAtlas`]'s cache with its texture
//! array, which the field kernels and the store pass fill (or, with [`Generator::Cpu`], uploads
//! of the fields generated on the CPU, on devices without compute shaders such as WebGL 2).
//!
//! [`GlyphAtlas`] dereferences to the cache, so requests (`get`, `get_with_reach`,
//! `begin_frame`, the layout functions of [`crate::text`]) work on it directly; [`GlyphAtlas::flush`]
//! generates what was requested into the texture. [`AtlasTexture`] is the texture and its
//! passes alone, for applications that keep the cache themselves (e.g. to choose a backend
//! at run time).

use std::ops::{Deref, DerefMut};

pub use glyphux_gpu::atlas::{
    AtlasConfig, AtlasConfigError, AtlasField, AtlasFormat, AtlasGlyph, AtlasStats, Generator, GlyphKey,
    store_shader_source,
};
use wgpu::util::DeviceExt;

use crate::field::{Binding, FieldBatch, FieldGenerator, Kernel, bind_group_layout, pipeline};

/// The wgpu format of atlas pages of `format`.
pub fn texture_format(format: AtlasFormat) -> wgpu::TextureFormat {
    match format {
        AtlasFormat::Rgba8Unorm => wgpu::TextureFormat::Rgba8Unorm,
        AtlasFormat::Rgba16Float => wgpu::TextureFormat::Rgba16Float,
        AtlasFormat::Rgba32Float => wgpu::TextureFormat::Rgba32Float,
    }
}

/// Checks that `config` works on `device`: its own values, the format (32-bit floats must be
/// filterable), the texture size, and compute shaders for [`Generator::Gpu`].
pub fn validate(config: &AtlasConfig, device: &wgpu::Device) -> Result<(), AtlasConfigError> {
    config.validate()?;
    if config.format == AtlasFormat::Rgba32Float && !device.features().contains(wgpu::Features::FLOAT32_FILTERABLE) {
        return Err(AtlasConfigError::Format(config.format));
    }
    let limits = device.limits();
    // WebGL 2 devices (wgpu's downlevel WebGL 2 limits) have no compute workgroups at all.
    if config.generator == Generator::Gpu && limits.max_compute_invocations_per_workgroup == 0 {
        return Err(AtlasConfigError::Invalid(
            "the device has no compute shaders: use Generator::Cpu",
        ));
    }
    if config.size > limits.max_texture_dimension_2d || config.layers > limits.max_texture_array_layers {
        return Err(AtlasConfigError::TooLarge {
            size: config.size,
            layers: config.layers,
        });
    }
    Ok(())
}

/// Cached distance fields of glyphs in a texture array, generated on demand: glyphux-gpu's
/// cache and an [`AtlasTexture`].
pub struct GlyphAtlas {
    atlas: glyphux_gpu::GlyphAtlas,
    gpu: AtlasTexture,
}

/// The texture array of a glyph atlas and the passes that fill it.
pub struct AtlasTexture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    format: AtlasFormat,
    size: u32,
    texture_pages: u32,
    /// The generator and store passes; `None` with [`Generator::Cpu`].
    passes: Option<Passes>,
}

struct Passes {
    generator: FieldGenerator,
    store: wgpu::ComputePipeline,
    store_layout: wgpu::BindGroupLayout,
}

impl Deref for GlyphAtlas {
    type Target = glyphux_gpu::GlyphAtlas;

    fn deref(&self) -> &Self::Target {
        &self.atlas
    }
}

impl DerefMut for GlyphAtlas {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.atlas
    }
}

/// A texture array for the atlas; the store pass writes it as a storage texture, uploads of
/// CPU fields copy into it.
fn atlas_texture(device: &wgpu::Device, format: AtlasFormat, size: u32, pages: u32, storage: bool) -> wgpu::Texture {
    let storage = if storage {
        wgpu::TextureUsages::STORAGE_BINDING
    } else {
        wgpu::TextureUsages::empty()
    };
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("glyphux atlas"),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: pages,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: texture_format(format),
        usage: storage
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

fn array_view(texture: &wgpu::Texture) -> wgpu::TextureView {
    texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    })
}

impl GlyphAtlas {
    /// Creates pipelines and a tiny placeholder texture. Full pages are allocated by flush.
    pub fn new(device: &wgpu::Device, config: AtlasConfig) -> Result<Self, AtlasConfigError> {
        let gpu = AtlasTexture::new(device, &config)?;
        Ok(GlyphAtlas {
            atlas: glyphux_gpu::GlyphAtlas::new(config)?,
            gpu,
        })
    }

    /// The cache (also reachable through `Deref`).
    pub fn cache(&self) -> &glyphux_gpu::GlyphAtlas {
        &self.atlas
    }

    pub fn cache_mut(&mut self) -> &mut glyphux_gpu::GlyphAtlas {
        &mut self.atlas
    }

    /// Full GPU pages allocated by the last flush; zero before the first generation.
    pub fn allocated_page_count(&self) -> u32 {
        self.gpu.allocated_page_count()
    }

    /// Current texture array. Before first generation this is a 1 by 1 placeholder.
    /// Growth replaces the texture during flush; acquire views and bind groups afterwards.
    pub fn texture(&self) -> &wgpu::Texture {
        self.gpu.texture()
    }

    /// `D2Array` view for sampling.
    pub fn view(&self) -> &wgpu::TextureView {
        self.gpu.view()
    }

    /// Generates all queued glyphs into the atlas (one batch, or with [`Generator::Cpu`] one
    /// upload). Returns how many.
    ///
    /// The upload is recorded into `encoder`, in order with the passes, so a frame may flush
    /// any number of times before submitting.
    pub fn flush(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder) -> usize {
        self.gpu.flush(&mut self.atlas, device, encoder)
    }

    /// [`Self::flush`] with GPU timestamps around its passes (distance, correction, store).
    pub fn flush_profiled(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        profiler: Option<&mut crate::profile::GpuProfiler>,
    ) -> usize {
        self.gpu.flush_profiled(&mut self.atlas, device, encoder, profiler)
    }
}

impl AtlasTexture {
    /// Creates pipelines and a tiny placeholder texture for an atlas of `config` (checked
    /// against the device). Full pages are allocated by flush. With [`Generator::Cpu`] there
    /// are no pipelines, and the texture is not a storage texture.
    pub fn new(device: &wgpu::Device, config: &AtlasConfig) -> Result<Self, AtlasConfigError> {
        validate(config, device)?;
        let passes = (config.generator == Generator::Gpu).then(|| {
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("glyphux atlas store"),
                source: wgpu::ShaderSource::Wgsl(store_shader_source(config.format).into()),
            });
            let store_layout = bind_group_layout(device, Kernel::Store, Some(texture_format(config.format)));
            Passes {
                generator: FieldGenerator::new(device),
                store: pipeline(device, &module, Kernel::Store, &store_layout),
                store_layout,
            }
        });
        let texture = atlas_texture(device, config.format, 1, 1, passes.is_some());
        let view = array_view(&texture);
        Ok(AtlasTexture {
            texture,
            view,
            format: config.format,
            size: config.size,
            texture_pages: 0,
            passes,
        })
    }

    /// Full GPU pages allocated by the last flush; zero before the first generation.
    pub fn allocated_page_count(&self) -> u32 {
        self.texture_pages
    }

    /// Current texture array. Before first generation this is a 1 by 1 placeholder.
    /// Growth replaces the texture during flush; acquire views and bind groups afterwards.
    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }

    /// `D2Array` view for sampling.
    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    /// Generates the glyphs queued in `atlas` (which must have this texture's configuration)
    /// into the texture, in one batch (or, with [`Generator::Cpu`], uploads the fields the CPU
    /// generated). Returns how many.
    pub fn flush(
        &mut self,
        atlas: &mut glyphux_gpu::GlyphAtlas,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
    ) -> usize {
        self.flush_profiled(atlas, device, encoder, None)
    }

    /// [`Self::flush`] with GPU timestamps around its passes (distance, correction, store).
    pub fn flush_profiled(
        &mut self,
        atlas: &mut glyphux_gpu::GlyphAtlas,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        mut profiler: Option<&mut crate::profile::GpuProfiler>,
    ) -> usize {
        if self.passes.is_none() {
            let fields = atlas.take_fields();
            if fields.is_empty() {
                return 0;
            }
            self.grow_texture(device, encoder, atlas.page_count());
            self.upload_fields(device, encoder, &fields);
            return fields.len();
        }
        let Some(batch) = atlas.take_batch() else {
            return 0;
        };
        self.grow_texture(device, encoder, atlas.page_count());
        let Some(passes) = &mut self.passes else {
            return 0;
        };
        let uploaded = passes.generator.upload_batch(device, encoder, &batch);
        passes
            .generator
            .encode_profiled(encoder, &uploaded, profiler.as_deref_mut());
        self.encode_store(device, encoder, &uploaded, profiler);
        batch.layouts.len()
    }

    /// Copies CPU-generated fields into their rectangles, through one staging buffer.
    fn upload_fields(&self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, fields: &[AtlasField]) {
        let texel = self.format.texel_size();
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        // Rows padded to the copy alignment; every field starts on a padded row.
        let row_bytes = |f: &AtlasField| (f.width * texel).next_multiple_of(align);
        let total: u32 = fields.iter().map(|f| row_bytes(f) * f.height).sum();
        let mut data = vec![0u8; total as usize];
        let mut offsets = Vec::with_capacity(fields.len());
        let mut offset = 0usize;
        for f in fields {
            offsets.push(offset as u64);
            let (src_row, dst_row) = ((f.width * texel) as usize, row_bytes(f) as usize);
            for (r, src) in f.texels.chunks_exact(src_row).take(f.height as usize).enumerate() {
                let at = offset + r * dst_row;
                data[at..at + src_row].copy_from_slice(src);
            }
            offset += dst_row * f.height as usize;
        }
        let staging = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("glyphux atlas upload"),
            contents: &data,
            usage: wgpu::BufferUsages::COPY_SRC,
        });
        for (f, offset) in fields.iter().zip(offsets) {
            encoder.copy_buffer_to_texture(
                wgpu::TexelCopyBufferInfo {
                    buffer: &staging,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset,
                        bytes_per_row: Some(row_bytes(f)),
                        rows_per_image: Some(f.height),
                    },
                },
                wgpu::TexelCopyTextureInfo {
                    texture: &self.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: f.x,
                        y: f.y,
                        z: f.layer,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width: f.width,
                    height: f.height,
                    depth_or_array_layers: 1,
                },
            );
        }
    }

    fn grow_texture(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, pages: u32) {
        if pages <= self.texture_pages {
            return;
        }
        let texture = atlas_texture(device, self.format, self.size, pages, self.passes.is_some());
        if self.texture_pages > 0 {
            encoder.copy_texture_to_texture(
                self.texture.as_image_copy(),
                texture.as_image_copy(),
                wgpu::Extent3d {
                    width: self.size,
                    height: self.size,
                    depth_or_array_layers: self.texture_pages,
                },
            );
        }
        self.view = array_view(&texture);
        self.texture = texture;
        self.texture_pages = pages;
    }

    fn encode_store(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        batch: &FieldBatch,
        mut profiler: Option<&mut crate::profile::GpuProfiler>,
    ) {
        let Some(passes) = &self.passes else {
            return;
        };
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("glyphux atlas store"),
            layout: &passes.store_layout,
            entries: &[
                batch.entry(Binding::Glyphs),
                batch.entry(Binding::Work),
                batch.entry(Binding::Field),
                wgpu::BindGroupEntry {
                    binding: Binding::Atlas.index(),
                    resource: wgpu::BindingResource::TextureView(&self.view),
                },
            ],
        });
        let scope = profiler.as_deref_mut().and_then(|p| p.scope("glyphux store"));
        let tw = scope.and_then(|s| profiler.as_deref().map(|p| p.compute_writes(s)));
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("glyphux atlas store"),
            timestamp_writes: tw,
        });
        pass.set_pipeline(&passes.store);
        pass.set_bind_group(0, &bg, &[]);
        let (x, y) = batch.workgroups;
        pass.dispatch_workgroups(x, y, 1);
    }
}
