//! Drawing glyphs from a [`GlyphAtlas`]: an instanced quad pipeline. Layout and styles are
//! glyphux-gpu's ([`glyphux_gpu::text`], re-exported here).

pub use glyphux_gpu::text::*;
use wgpu::util::DeviceExt;

use crate::atlas::GlyphAtlas;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals {
    viewport: [f32; 2],
    px_range: f32,
    has_true_distance: f32,
}

/// Renders [`GlyphInstance`]s sampled from a glyph atlas.
///
/// It draws them in three passes (the shader's `text_pass`): every glyph's drop shadow and
/// glow, then every outline, then every fill, so that no glyph's effects cover its
/// neighbors. Quads with nothing to draw in a pass have no area there.
pub struct TextRenderer {
    /// One per pass: effects, outlines, fills.
    pipelines: [wgpu::RenderPipeline; 3],
    layout: wgpu::BindGroupLayout,
    globals: wgpu::Buffer,
    sampler: wgpu::Sampler,
    /// Bound to the texture of the atlas last passed to `prepare`.
    bind_group: Option<(wgpu::Texture, wgpu::BindGroup)>,
    instances: Option<wgpu::Buffer>,
    capacity: usize,
    count: u32,
}

impl TextRenderer {
    /// `target`: format of the color attachment the text is drawn into (alpha blended).
    pub fn new(device: &wgpu::Device, target: wgpu::TextureFormat) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("glyphux text"),
            source: wgpu::ShaderSource::Wgsl(TEXT_SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("glyphux text"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("glyphux text"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let attrs = wgpu::vertex_attr_array![
            0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4,
            5 => Float32x4, 6 => Float32x4, 7 => Float32x4, 8 => Float32x4, 9 => Uint32
        ];
        let pipelines = [0.0, 1.0, 2.0].map(|pass| {
            let constants = [("text_pass_index", pass)];
            let options = || wgpu::PipelineCompilationOptions {
                constants: &constants,
                ..Default::default()
            };
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("glyphux text"),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs"),
                    compilation_options: options(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: size_of::<GlyphInstance>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &attrs,
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
                    entry_point: Some("fs"),
                    compilation_options: options(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: target,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        });
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("glyphux text globals"),
            size: size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("glyphux text"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        TextRenderer {
            pipelines,
            layout,
            globals,
            sampler,
            bind_group: None,
            instances: None,
            capacity: 0,
            count: 0,
        }
    }

    /// Uploads this frame's glyphs (`viewport`: target size in pixels), drawn from `atlas`
    /// (any atlas: the renderer rebinds when it changes).
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        atlas: &GlyphAtlas,
        viewport: [f32; 2],
        instances: &[GlyphInstance],
    ) {
        let cfg = atlas.config();
        let g = Globals {
            viewport,
            px_range: cfg.px_range as f32,
            has_true_distance: if cfg.kind.true_distance && cfg.kind.channels == 3 {
                1.0
            } else {
                0.0
            },
        };
        queue.write_buffer(&self.globals, 0, bytemuck::bytes_of(&g));
        if self.bind_group.as_ref().is_none_or(|(t, _)| t != atlas.texture()) {
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("glyphux text"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.globals.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(atlas.view()),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            });
            self.bind_group = Some((atlas.texture().clone(), bg));
        }
        self.count = instances.len() as u32;
        if instances.is_empty() {
            return;
        }
        let bytes = bytemuck::cast_slice(instances);
        if instances.len() > self.capacity {
            self.capacity = instances.len().next_power_of_two();
            let mut init = bytes.to_vec();
            init.resize(self.capacity * size_of::<GlyphInstance>(), 0);
            self.instances = Some(device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("glyphux text instances"),
                contents: &init,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            }));
        } else if let Some(b) = &self.instances {
            queue.write_buffer(b, 0, bytes);
        }
    }

    /// Draws the prepared glyphs.
    pub fn render(&self, pass: &mut wgpu::RenderPass<'_>) {
        self.render_range(pass, 0..self.count);
    }

    /// Draws an ordered range of prepared instances (in three passes, see [`TextRenderer`]).
    pub fn render_range(&self, pass: &mut wgpu::RenderPass<'_>, range: std::ops::Range<u32>) {
        assert!(range.end <= self.count);
        let (Some((_, bg)), Some(inst)) = (&self.bind_group, &self.instances) else {
            return;
        };
        if self.count == 0 {
            return;
        }
        pass.set_bind_group(0, bg, &[]);
        pass.set_vertex_buffer(0, inst.slice(..));
        for pipeline in &self.pipelines {
            pass.set_pipeline(pipeline);
            pass.draw(0..4, range.clone());
        }
    }
}
