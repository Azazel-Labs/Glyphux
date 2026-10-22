//! Drawing glyphs from a [`GlyphAtlas`]: an instanced quad pipeline and simple line layout.

use textra::io::font::Font;
use wgpu::util::DeviceExt;

use crate::atlas::{AtlasGlyph, GlyphAtlas, GlyphKey};
use crate::units::{Length, Units};

/// One glyph quad (vertex buffer layout of `text.wgsl`, 160 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GlyphInstance {
    /// Quad in target pixels: min x, min y, max x, max y (y down).
    pub rect: [f32; 4],
    /// Texture coordinates of the top-left and bottom-right corners.
    pub uv: [f32; 4],
    /// Fill color, straight alpha (linear if the target is sRGB).
    pub color: [f32; 4],
    /// Outline color, straight alpha (MTSDF atlases only).
    pub outline: [f32; 4],
    /// x: weight in pixels (> 0 bolder, < 0 thinner); y: outline width in pixels; z: glow
    /// radius in pixels (outline and glow reach at most half the distance range on screen:
    /// request wide ones with [`GlyphAtlas::get_with_reach`]); w: the glyph's distance range
    /// in texels ([`AtlasGlyph::px_range`]; 0 = the atlas').
    pub params: [f32; 4],
    /// Glow color, straight alpha (its opacity at the glyph's edge, fading out over the radius).
    pub glow: [f32; 4],
    /// Rotation of the quad: cos, sin, pivot x, pivot y (pixels). Cos and sin 0: none.
    pub transform: [f32; 4],
    /// Drop shadow color, straight alpha.
    pub shadow: [f32; 4],
    /// Drop shadow offset x, y (screen pixels, y down) and blur (pixels); w: 1 draws the raw
    /// field instead of the glyph (see [`Self::atlas_page`]).
    pub shadow_params: [f32; 4],
    pub layer: u32,
    pub _pad: [u32; 3],
}

impl GlyphInstance {
    /// Places `glyph` with its origin (baseline, pen position) at `origin` (pixels, y down)
    /// at `px_per_em`.
    pub fn new(glyph: &AtlasGlyph, origin: [f32; 2], px_per_em: f32, color: [f32; 4]) -> Self {
        GlyphInstance {
            rect: [
                origin[0] + glyph.plane_min[0] * px_per_em,
                origin[1] - glyph.plane_max[1] * px_per_em,
                origin[0] + glyph.plane_max[0] * px_per_em,
                origin[1] - glyph.plane_min[1] * px_per_em,
            ],
            uv: [glyph.uv_min[0], glyph.uv_min[1], glyph.uv_max[0], glyph.uv_max[1]],
            color,
            params: [0.0, 0.0, 0.0, glyph.px_range],
            layer: glyph.layer,
            ..Default::default()
        }
    }

    pub fn with_outline(mut self, color: [f32; 4], width_px: f32) -> Self {
        self.outline = color;
        self.params[1] = width_px;
        self
    }

    pub fn with_weight(mut self, px: f32) -> Self {
        self.params[0] = px;
        self
    }

    pub fn with_glow(mut self, color: [f32; 4], radius_px: f32) -> Self {
        self.glow = color;
        self.params[2] = radius_px;
        self
    }

    /// A drop shadow `offset` screen pixels away (y down), softened over `blur` pixels. Like
    /// outlines and glows it must fit in the field's range: request the glyph with a reach of
    /// at least the offset's length plus the blur.
    pub fn with_shadow(mut self, color: [f32; 4], offset: [f32; 2], blur_px: f32) -> Self {
        self.shadow = color;
        self.shadow_params = [offset[0], offset[1], blur_px, self.shadow_params[3]];
        self
    }

    /// A quad showing the raw distance field of atlas layer `layer` (all of it) in `rect`
    /// (pixels), e.g. to watch the atlas fill up.
    pub fn atlas_page(rect: [f32; 4], layer: u32) -> Self {
        GlyphInstance {
            rect,
            uv: [0.0, 0.0, 1.0, 1.0],
            color: [1.0; 4],
            shadow_params: [0.0, 0.0, 0.0, 1.0],
            layer,
            ..Default::default()
        }
    }

    /// Rotates the quad by `angle` radians (clockwise on screen, y down) about `pivot` (pixels),
    /// e.g. the glyph's origin to set it along a path.
    pub fn with_rotation(mut self, angle: f32, pivot: [f32; 2]) -> Self {
        let (sin, cos) = angle.sin_cos();
        // An exact zero pair would read as "no rotation"; nudge it (only possible at 0 anyway).
        self.transform = [
            if cos == 0.0 && sin == 0.0 { 1.0 } else { cos },
            sin,
            pivot[0],
            pivot[1],
        ];
        self
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals {
    viewport: [f32; 2],
    px_range: f32,
    has_true_distance: f32,
}

/// Renders [`GlyphInstance`]s sampled from a glyph atlas.
pub struct TextRenderer {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    globals: wgpu::Buffer,
    sampler: wgpu::Sampler,
    /// Bound to the texture of the atlas last passed to `prepare`.
    bind_group: Option<(wgpu::Texture, wgpu::BindGroup)>,
    instances: Option<wgpu::Buffer>,
    capacity: usize,
    count: u32,
}

pub(crate) const TEXT_SHADER: &str = include_str!("shaders/text.wgsl");

impl TextRenderer {
    /// `target`: format of the color attachment the text is drawn into (alpha blended).
    pub fn new(device: &wgpu::Device, target: wgpu::TextureFormat) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("textra text"),
            source: wgpu::ShaderSource::Wgsl(TEXT_SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("textra text"),
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
            label: Some("textra text"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let attrs = wgpu::vertex_attr_array![
            0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4,
            5 => Float32x4, 6 => Float32x4, 7 => Float32x4, 8 => Float32x4, 9 => Uint32
        ];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("textra text"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
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
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("textra text globals"),
            size: size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("textra text"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        TextRenderer {
            pipeline,
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
                label: Some("textra text"),
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
                label: Some("textra text instances"),
                contents: &init,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            }));
        } else if let Some(b) = &self.instances {
            queue.write_buffer(b, 0, bytes);
        }
    }

    /// Draws the prepared glyphs.
    pub fn render(&self, pass: &mut wgpu::RenderPass<'_>) {
        let (Some((_, bg)), Some(inst)) = (&self.bind_group, &self.instances) else {
            return;
        };
        if self.count == 0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, bg, &[]);
        pass.set_vertex_buffer(0, inst.slice(..));
        pass.draw(0..4, 0..self.count);
    }
}

/// How text is drawn: its size, color and effects, with every length typed (see
/// [`crate::units`]). Effects in `em` scale with the text.
///
/// ```
/// use textra_gpu::text::TextStyle;
/// use textra_gpu::units::{em, px};
/// let title = TextStyle::new(px(48.0), [1.0, 0.5, 0.2, 1.0]).glow([1.0, 0.4, 0.1, 0.6], em(0.15));
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextStyle {
    /// Size of an em (`em` / `%` relative to the root size).
    pub size: Length,
    /// Fill color, straight alpha (linear if the target is sRGB).
    pub color: [f32; 4],
    /// Added to the distance: bolder if positive, thinner if negative.
    pub weight: Length,
    /// Color and width.
    pub outline: Option<([f32; 4], Length)>,
    /// Color (alpha: opacity at the outline) and radius.
    pub glow: Option<([f32; 4], Length)>,
    pub shadow: Option<Shadow>,
}

/// A soft drop shadow.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shadow {
    pub color: [f32; 4],
    /// Offset (x right, y down).
    pub offset: [Length; 2],
    pub blur: Length,
}

impl TextStyle {
    pub fn new(size: Length, color: [f32; 4]) -> Self {
        TextStyle {
            size,
            color,
            weight: Length::ZERO,
            outline: None,
            glow: None,
            shadow: None,
        }
    }

    pub fn size(self, size: Length) -> Self {
        TextStyle { size, ..self }
    }

    pub fn color(self, color: [f32; 4]) -> Self {
        TextStyle { color, ..self }
    }

    pub fn weight(self, weight: Length) -> Self {
        TextStyle { weight, ..self }
    }

    pub fn outline(self, color: [f32; 4], width: Length) -> Self {
        TextStyle {
            outline: Some((color, width)),
            ..self
        }
    }

    pub fn glow(self, color: [f32; 4], radius: Length) -> Self {
        TextStyle {
            glow: Some((color, radius)),
            ..self
        }
    }

    pub fn shadow(self, color: [f32; 4], offset: [Length; 2], blur: Length) -> Self {
        TextStyle {
            shadow: Some(Shadow { color, offset, blur }),
            ..self
        }
    }

    /// The style in physical pixels of a target with `units`.
    pub fn resolve(&self, units: &Units) -> ResolvedStyle {
        let em_px = units.text_px(self.size);
        let r = |l: Length| l.resolve(units, em_px);
        ResolvedStyle {
            px_per_em: em_px,
            color: self.color,
            weight: r(self.weight),
            outline: self.outline.map(|(c, w)| (c, r(w))),
            glow: self.glow.map(|(c, w)| (c, r(w))),
            shadow: self
                .shadow
                .map(|s| (s.color, [r(s.offset[0]), r(s.offset[1])], r(s.blur))),
        }
    }
}

/// A [`TextStyle`] in physical pixels (from [`TextStyle::resolve`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResolvedStyle {
    pub px_per_em: f32,
    pub color: [f32; 4],
    pub weight: f32,
    pub outline: Option<([f32; 4], f32)>,
    pub glow: Option<([f32; 4], f32)>,
    /// Color, offset, blur.
    pub shadow: Option<([f32; 4], [f32; 2], f32)>,
}

impl ResolvedStyle {
    /// How far the effects reach outside the outline: the reach to request glyphs with
    /// ([`GlyphAtlas::get_with_reach`]) so that nothing is clipped.
    pub fn reach(&self) -> f32 {
        let outline = self.outline.map_or(0.0, |(_, w)| w);
        let glow = self.glow.map_or(0.0, |(_, r)| r);
        let shadow = self.shadow.map_or(0.0, |(_, d, b)| d[0].hypot(d[1]) + b);
        outline.max(glow).max(shadow) + self.weight.max(0.0)
    }

    /// The quad of `glyph` with its origin at `origin` (physical pixels), styled.
    pub fn instance(&self, glyph: &AtlasGlyph, origin: [f32; 2]) -> GlyphInstance {
        let mut g = GlyphInstance::new(glyph, origin, self.px_per_em, self.color).with_weight(self.weight);
        if let Some((c, w)) = self.outline {
            g = g.with_outline(c, w);
        }
        if let Some((c, r)) = self.glow {
            g = g.with_glow(c, r);
        }
        if let Some((c, d, b)) = self.shadow {
            g = g.with_shadow(c, d, b);
        }
        g
    }

    /// Requests a glyph from the atlas with this style's reach and appends its quad, origin at
    /// `origin` (physical pixels), unless it is blank or cannot be placed.
    pub fn place(
        &self,
        atlas: &mut GlyphAtlas,
        key: GlyphKey,
        origin: [f32; 2],
        shape: impl FnOnce() -> Option<textra::Shape>,
        out: &mut Vec<GlyphInstance>,
    ) -> Option<GlyphInstance> {
        let g = atlas.get_with_reach(key, f64::from(self.px_per_em), f64::from(self.reach()), shape)?;
        if !g.is_visible() {
            return None;
        }
        let inst = self.instance(&g, origin);
        out.push(inst);
        Some(inst)
    }
}

/// A font registered for layout: its parsed data and the id used in [`GlyphKey`]s.
pub struct FontRef<'a> {
    pub id: u32,
    pub font: &'a Font<'a>,
}

/// Lays out one line of text (character map + `kern` table; no shaping) with its baseline
/// starting at `origin` (logical pixels, y down), requesting glyphs from the atlas. Appends
/// the quads to `out` and returns the pen position after the last glyph (logical pixels).
pub fn layout_line(
    atlas: &mut GlyphAtlas,
    font: &FontRef<'_>,
    text: &str,
    style: &TextStyle,
    units: &Units,
    origin: [f32; 2],
    out: &mut Vec<GlyphInstance>,
) -> [f32; 2] {
    let rs = style.resolve(units);
    let em = rs.px_per_em;
    let mut pen = units.to_physical(origin);
    let mut prev: Option<u16> = None;
    for ch in text.chars() {
        let Some(gid) = font.font.glyph_index(ch) else {
            prev = None;
            continue;
        };
        if let Some(p) = prev {
            pen[0] += (font.font.kerning(p, gid) as f32) * em;
        }
        let key = GlyphKey {
            font: font.id,
            glyph: gid as u32,
        };
        rs.place(
            atlas,
            key,
            pen,
            || font.font.glyph_by_index(gid).ok().map(|g| g.shape),
            out,
        );
        pen[0] += font.font.advance(gid) as f32 * em;
        prev = Some(gid);
    }
    [pen[0] / units.scale, origin[1]]
}

/// Places a shaped line with the start of its baseline at `origin` (logical pixels, y down),
/// requesting its glyphs from the atlas. Appends the quads to `out` and returns the pen
/// position after the line (logical pixels).
#[cfg(feature = "shaping")]
pub fn layout_shaped(
    atlas: &mut GlyphAtlas,
    font: &crate::shaping::ShapingFont,
    line: &crate::shaping::ShapedLine,
    style: &TextStyle,
    units: &Units,
    origin: [f32; 2],
    out: &mut Vec<GlyphInstance>,
) -> [f32; 2] {
    let rs = style.resolve(units);
    let em = rs.px_per_em;
    let o = units.to_physical(origin);
    for sg in &line.glyphs {
        let key = GlyphKey {
            font: font.id,
            glyph: sg.glyph,
        };
        let at = [o[0] + sg.x * em, o[1] - sg.y * em];
        rs.place(atlas, key, at, || font.outline(sg.glyph), out);
    }
    [origin[0] + line.width * em / units.scale, origin[1]]
}

/// Horizontal alignment of [`layout_text`] lines within a box.
#[cfg(feature = "shaping")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Align {
    /// Left for left-to-right paragraphs, right for right-to-left ones.
    #[default]
    Start,
    End,
    Center,
}

/// Shapes and places text of one or more lines (`\n` separated): the first baseline at
/// `origin[1]`, lines `line_height` em apart (`None`: the font's), aligned within
/// `[origin[0], origin[0] + width]` (logical pixels). Returns the bottom baseline's y.
#[cfg(feature = "shaping")]
#[allow(clippy::too_many_arguments)]
pub fn layout_text(
    atlas: &mut GlyphAtlas,
    shaper: &mut crate::shaping::Shaper,
    font: &crate::shaping::ShapingFont,
    text: &str,
    style: &TextStyle,
    units: &Units,
    origin: [f32; 2],
    width: f32,
    align: Align,
    line_height: Option<f32>,
    out: &mut Vec<GlyphInstance>,
) -> f32 {
    let lh = line_height.unwrap_or(font.metrics().line_height as f32);
    // Logical pixels per em.
    let em = units.text_px(style.size) / units.scale;
    let mut y = origin[1];
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            y += lh * em;
        }
        let shaped = shaper.shape_line(font, line);
        let w = shaped.width * em;
        let x = match (align, shaped.rtl) {
            (Align::Start, false) | (Align::End, true) => origin[0],
            (Align::Start, true) | (Align::End, false) => origin[0] + width - w,
            (Align::Center, _) => origin[0] + 0.5 * (width - w),
        };
        layout_shaped(atlas, font, &shaped, style, units, [x, y], out);
    }
    y
}
