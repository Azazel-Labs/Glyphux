//! Color glyph rendering with one filtered region-distance sample per boundary pixel.
//!
//! CPU compilation uses [`ColorField`]. Upload once per glyph/variation/palette key, then
//! draw alongside regular text in the same render pass. Color artwork uses its own paints;
//! monochrome text styles (outlines, weight and shadows) do not apply to this pipeline.

use crate::GlyphKey;
use glyphux::color::{ColorError, ColorField};
use std::collections::HashMap;
use wgpu::util::DeviceExt;

#[path = "color_atlas.rs"]
mod atlas;
use atlas::Rect;

/// Fixed-size color pages are allocated on demand, up to this bound.
#[derive(Clone, Copy, Debug)]
pub struct ColorAtlasConfig {
    pub page_size: u32,
    pub max_pages: u32,
}
impl Default for ColorAtlasConfig {
    fn default() -> Self {
        Self {
            page_size: 1024,
            max_pages: 6,
        }
    }
}

struct Page {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    free: Vec<Rect>,
}

pub(crate) const COLOR_SHADER: &str = glyphux::color::COLOR_SHADER;

/// One color glyph placed in screen pixels. The font key must distinguish palette,
/// foreground color and variation instance as well as font face.
#[derive(Clone, Copy, Debug)]
pub struct ColorGlyphInstance {
    pub key: GlyphKey,
    pub rect: [f32; 4],
    /// cos, sin, pivot x, pivot y. Only rotation and uniform scaling are supported.
    pub transform: [f32; 4],
    pub opacity: f32,
}

impl ColorGlyphInstance {
    pub fn new(key: GlyphKey, field: &ColorField, origin: [f32; 2], px_per_em: f32) -> Self {
        let (min, max) = field.plane_bounds();
        Self {
            key,
            rect: [
                origin[0] + min.x as f32 * px_per_em,
                origin[1] - max.y as f32 * px_per_em,
                origin[0] + max.x as f32 * px_per_em,
                origin[1] - min.y as f32 * px_per_em,
            ],
            transform: [1.0, 0.0, 0.0, 0.0],
            opacity: 1.0,
        }
    }
    pub fn with_rotation(mut self, radians: f32, pivot: [f32; 2]) -> Self {
        let (sin, cos) = radians.sin_cos();
        self.transform = [cos, sin, pivot[0], pivot[1]];
        self
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Instance {
    rect: [f32; 4],
    transform: [f32; 4],
    opacity: f32,
    padding: [f32; 3],
}

struct Uploaded {
    page: usize,
    rect: Rect,
    bind_group: wgpu::BindGroup,
    minimum: u32,
}

/// Resident color glyph resources. Call `remove`/`clear` to release cached glyphs.
/// Upload and CPU preprocessing are explicit so applications can schedule them off-frame.
pub struct ColorRenderer {
    pipeline: wgpu::RenderPipeline,
    atlas_pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    globals: wgpu::Buffer,
    globals_group: wgpu::BindGroup,
    sampler: wgpu::Sampler,
    glyphs: HashMap<GlyphKey, Uploaded>,
    config: ColorAtlasConfig,
    pages: Vec<Page>,
    instances: Option<wgpu::Buffer>,
    prepared: Vec<Instance>,
    order: Vec<GlyphKey>,
}

fn buffer(device: &wgpu::Device, label: &str, data: &[u8], usage: wgpu::BufferUsages) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: data,
        usage,
    })
}

fn buffer_entry(
    binding: u32,
    ty: wgpu::BufferBindingType,
    visibility: wgpu::ShaderStages,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

impl ColorRenderer {
    pub fn new(device: &wgpu::Device, target: wgpu::TextureFormat) -> Self {
        Self::with_config(device, target, ColorAtlasConfig::default()).expect("default color atlas fits device")
    }

    pub fn with_config(
        device: &wgpu::Device,
        target: wgpu::TextureFormat,
        config: ColorAtlasConfig,
    ) -> Result<Self, ColorError> {
        if config.page_size == 0 || config.page_size > device.limits().max_texture_dimension_2d || config.max_pages == 0
        {
            return Err(ColorError::Invalid("invalid color atlas configuration"));
        }
        let global_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("glyphux color globals"),
            entries: &[buffer_entry(
                0,
                wgpu::BufferBindingType::Uniform,
                wgpu::ShaderStages::VERTEX,
            )],
        });
        let fragment = wgpu::ShaderStages::FRAGMENT;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("glyphux color glyph"),
            entries: &[
                buffer_entry(0, wgpu::BufferBindingType::Uniform, fragment),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: fragment,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: fragment,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                buffer_entry(3, wgpu::BufferBindingType::Storage { read_only: true }, fragment),
                buffer_entry(4, wgpu::BufferBindingType::Storage { read_only: true }, fragment),
                buffer_entry(5, wgpu::BufferBindingType::Storage { read_only: true }, fragment),
                buffer_entry(6, wgpu::BufferBindingType::Storage { read_only: true }, fragment),
            ],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("glyphux color"),
            source: wgpu::ShaderSource::Wgsl(COLOR_SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("glyphux color"),
            bind_group_layouts: &[Some(&global_layout), Some(&layout)],
            immediate_size: 0,
        });
        let attributes = wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32];
        let make_pipeline = |entry| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("glyphux color"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: size_of::<Instance>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &attributes,
                    })],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleStrip,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(entry),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: target,
                        blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let pipeline = make_pipeline("fs");
        let atlas_pipeline = make_pipeline("fs_atlas");
        let globals = buffer(
            device,
            "glyphux color globals",
            &[0; 16],
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let globals_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("glyphux color globals"),
            layout: &global_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            }],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("glyphux color linear"),
            min_filter: wgpu::FilterMode::Linear,
            mag_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Ok(Self {
            pipeline,
            atlas_pipeline,
            layout,
            globals,
            globals_group,
            sampler,
            glyphs: HashMap::new(),
            config,
            pages: Vec::new(),
            instances: None,
            prepared: Vec::new(),
            order: Vec::new(),
        })
    }

    /// Uploads or replaces one compiled glyph. RGBA8 data is kept linear and unmodified.
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        key: GlyphKey,
        field: &ColorField,
    ) -> Result<(), ColorError> {
        let [width, height] = field.atlas_size();
        let limits = device.limits();
        if width.max(height) > self.config.page_size {
            return Err(ColorError::Invalid("color glyph exceeds configured page size"));
        }
        // Plan against copies so an out-of-space replacement leaves the old glyph intact.
        let mut free: Vec<_> = self.pages.iter().map(|p| p.free.clone()).collect();
        if let Some(old) = self.glyphs.get(&key) {
            atlas::release(&mut free[old.page], old.rect);
        }
        let mut placement = free
            .iter_mut()
            .enumerate()
            .find_map(|(i, f)| atlas::allocate(f, width, height).map(|r| (i, r)));
        if placement.is_none() && free.len() < self.config.max_pages as usize {
            let mut f = vec![Rect {
                x: 0,
                y: 0,
                w: self.config.page_size,
                h: self.config.page_size,
            }];
            let rect = atlas::allocate(&mut f, width, height).unwrap();
            placement = Some((free.len(), rect));
            free.push(f);
        }
        let (page, rect) = placement.ok_or(ColorError::Invalid(
            "color atlas full; remove cached glyphs or clear the atlas",
        ))?;
        let encoded = field.encode([rect.x, rect.y])?;
        let storage = [&encoded.tiles, &encoded.paints, &encoded.stops, &encoded.recipes];
        if storage
            .iter()
            .any(|d| d.len() as u64 > limits.max_storage_buffer_binding_size)
        {
            return Err(ColorError::Invalid("paint buffers exceed device limits"));
        }
        let buffers: Vec<_> = storage
            .iter()
            .map(|d| buffer(device, "glyphux color data", d, wgpu::BufferUsages::STORAGE))
            .collect();
        let uniform = buffer(
            device,
            "glyphux color config",
            &encoded.config,
            wgpu::BufferUsages::UNIFORM,
        );
        if page == self.pages.len() {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("glyphux shared color page"),
                size: wgpu::Extent3d {
                    width: self.config.page_size,
                    height: self.config.page_size,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_DST
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            self.pages.push(Page {
                texture,
                view,
                free: Vec::new(),
            });
        }
        for (p, f) in self.pages.iter_mut().zip(free) {
            p.free = f;
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.pages[page].texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: rect.x,
                    y: rect.y,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            field.atlas_bytes(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        let view = &self.pages[page].view;
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("glyphux color glyph"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: buffers[0].as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: buffers[1].as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: buffers[2].as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: buffers[3].as_entire_binding(),
                },
            ],
        });
        self.glyphs.insert(
            key,
            Uploaded {
                page,
                rect,
                bind_group,
                minimum: field.config().min_pixels,
            },
        );
        Ok(())
    }

    pub fn atlas_config(&self) -> ColorAtlasConfig {
        self.config
    }
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }
    /// The allocated texture page, including currently unused space.
    pub fn page_texture(&self, page: usize) -> Option<&wgpu::Texture> {
        self.pages.get(page).map(|p| &p.texture)
    }
    pub fn glyph_page(&self, key: GlyphKey) -> Option<usize> {
        self.glyphs.get(&key).map(|g| g.page)
    }
    /// One resident key per occupied page, in page order, for the atlas debug view.
    pub fn page_keys(&self) -> Vec<GlyphKey> {
        (0..self.pages.len())
            .filter_map(|page| {
                self.glyphs
                    .iter()
                    .filter(|(_, g)| g.page == page)
                    .map(|(k, _)| *k)
                    .min_by_key(|k| (k.font, k.glyph))
            })
            .collect()
    }

    pub fn contains(&self, key: GlyphKey) -> bool {
        self.glyphs.contains_key(&key)
    }
    pub fn remove(&mut self, key: GlyphKey) {
        if let Some(old) = self.glyphs.remove(&key) {
            atlas::release(&mut self.pages[old.page].free, old.rect);
        }
    }
    pub fn clear(&mut self) {
        self.glyphs.clear();
        self.pages.clear();
        self.order.clear();
    }

    /// Validates the compiled footprint bound and uploads screen placement. A failed
    /// prepare clears the pending draw, so stale instances cannot be drawn accidentally.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        viewport: [f32; 2],
        instances: &[ColorGlyphInstance],
    ) -> Result<(), ColorError> {
        self.order.clear();
        if viewport.iter().any(|v| !v.is_finite() || *v <= 0.0) {
            return Err(ColorError::Invalid("viewport"));
        }
        self.prepared.clear();
        for i in instances {
            let glyph = self
                .glyphs
                .get(&i.key)
                .ok_or(ColorError::Invalid("color glyph has not been uploaded"))?;
            let width = i.rect[2] - i.rect[0];
            let height = i.rect[3] - i.rect[1];
            if i.rect.iter().chain(&i.transform).any(|v| !v.is_finite())
                || !i.opacity.is_finite()
                || !(0.0..=1.0).contains(&i.opacity)
                || width.min(height) < glyph.minimum as f32
                || (width - height).abs() > 1e-4 * width.max(height)
                || (i.transform[0] * i.transform[0] + i.transform[1] * i.transform[1] - 1.0).abs() > 1e-4
            {
                return Err(ColorError::Invalid(
                    "color glyph requires a square uniform scale above its compiled minimum and a valid rotation",
                ));
            }
            self.prepared.push(Instance {
                rect: i.rect,
                transform: i.transform,
                opacity: i.opacity,
                padding: [0.0; 3],
            });
        }
        queue.write_buffer(
            &self.globals,
            0,
            bytemuck::cast_slice(&[viewport[0], viewport[1], 0.0, 0.0]),
        );
        if !self.prepared.is_empty() {
            let bytes = bytemuck::cast_slice(&self.prepared);
            let size = bytes.len() as u64;
            if self.instances.as_ref().is_none_or(|buffer| buffer.size() < size) {
                self.instances = Some(device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("glyphux color instances"),
                    size: size.next_power_of_two(),
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
            }
            queue.write_buffer(self.instances.as_ref().unwrap(), 0, bytes);
        }
        self.order.extend(instances.iter().map(|i| i.key));
        Ok(())
    }

    /// Draws in caller order with premultiplied source-over. To interleave monochrome
    /// and color glyphs, submit each ordered run to its corresponding renderer.
    pub fn render(&self, pass: &mut wgpu::RenderPass<'_>) {
        self.render_range(pass, 0..self.order.len() as u32, false);
    }

    /// Draw a prepared range, optionally displaying raw packed RGB atlas data.
    pub fn render_range(&self, pass: &mut wgpu::RenderPass<'_>, range: std::ops::Range<u32>, atlas: bool) {
        assert!(range.end <= self.order.len() as u32);
        let Some(instances) = &self.instances else {
            return;
        };
        pass.set_pipeline(if atlas { &self.atlas_pipeline } else { &self.pipeline });
        pass.set_bind_group(0, &self.globals_group, &[]);
        pass.set_vertex_buffer(0, instances.slice(..));
        for i in range {
            let key = &self.order[i as usize];
            if let Some(glyph) = self.glyphs.get(key) {
                pass.set_bind_group(1, &glyph.bind_group, &[]);
                pass.draw(0..4, i..i + 1);
            }
        }
    }
}
