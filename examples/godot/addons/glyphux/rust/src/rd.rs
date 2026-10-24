//! The atlas on Godot's own `RenderingDevice` (Forward+ and Mobile): glyphux's kernels, as
//! SPIR-V that build.rs translates from their WGSL (embedded here), run in Godot's frame and
//! store the fields into an RD texture that the labels' material samples through a
//! `Texture2DArrayRD`. No second device, no copies back to the CPU.
//!
//! Each batch gets buffers of exactly its size (the kernels take the work list's length from
//! its binding) and a uniform set per kernel with the bindings its SPIR-V declares, all freed
//! once recorded (Godot frees them when the GPU is done with them).

use std::collections::HashMap;

use glyphux_wgpu::glyphux_gpu::GlyphAtlas;
use glyphux_wgpu::glyphux_gpu::spirv;
use glyphux_wgpu::{AtlasFormat, Binding, Kernel};
use godot::classes::rendering_device::{DataFormat, ShaderStage, TextureType, TextureUsageBits, UniformType};
use godot::classes::{RdShaderSpirv, RdTextureFormat, RdTextureView, RdUniform, RenderingDevice, Texture2DArrayRd};
use godot::prelude::*;

/// The SPIR-V of a kernel that build.rs wrote into `OUT_DIR/kernels/`.
macro_rules! kernel_file {
    ($name:literal) => {
        include_bytes!(concat!(env!("OUT_DIR"), "/kernels/", $name, ".spv"))
    };
}

/// `kernel`'s SPIR-V, `format` the atlas pages' (for the store kernel).
fn kernel_spirv(kernel: Kernel, format: AtlasFormat) -> &'static [u8] {
    match kernel {
        Kernel::Distance => kernel_file!("fine"),
        Kernel::CorrectCorners => kernel_file!("ec_corners"),
        Kernel::CorrectProtect => kernel_file!("ec_protect"),
        Kernel::CorrectFind => kernel_file!("ec_find"),
        Kernel::CorrectApply => kernel_file!("ec_apply"),
        Kernel::Store => match format {
            AtlasFormat::Rgba8Unorm => kernel_file!("store_rgba8unorm"),
            AtlasFormat::Rgba16Float => kernel_file!("store_rgba16float"),
            AtlasFormat::Rgba32Float => kernel_file!("store_rgba32float"),
        },
    }
}

struct Compiled {
    shader: Rid,
    pipeline: Rid,
    bindings: Vec<Binding>,
}

pub struct RdAtlas {
    rd: Gd<RenderingDevice>,
    pipelines: HashMap<Kernel, Compiled>,
    format: DataFormat,
    size: u32,
    texture: Rid,
    /// Pages in `texture` (0: the 1×1 placeholder).
    pages: u32,
    resource: Gd<Texture2DArrayRd>,
}

fn data_format(format: AtlasFormat) -> DataFormat {
    match format {
        AtlasFormat::Rgba8Unorm => DataFormat::R8G8B8A8_UNORM,
        AtlasFormat::Rgba16Float => DataFormat::R16G16B16A16_SFLOAT,
        AtlasFormat::Rgba32Float => DataFormat::R32G32B32A32_SFLOAT,
    }
}

impl RdAtlas {
    /// Compiles the kernels for an atlas like `atlas` on `rd`, Godot's main rendering device.
    pub fn new(mut rd: Gd<RenderingDevice>, atlas: &GlyphAtlas) -> Result<Self, String> {
        let config = atlas.config();
        let mut pipelines = HashMap::new();
        let mut failed = Vec::new();
        for kernel in Kernel::ALL {
            let bytes = kernel_spirv(kernel, config.format);
            let bindings = spirv::from_bytes(bytes)
                .and_then(|words| spirv::bindings(&words))
                .ok_or_else(|| format!("{kernel:?}: the embedded SPIR-V is not valid"))?;
            let mut source = RdShaderSpirv::new_gd();
            source.set_stage_bytecode(ShaderStage::COMPUTE, &PackedByteArray::from(bytes));
            let error = source.get_stage_compile_error(ShaderStage::COMPUTE);
            if !error.is_empty() {
                return Err(format!("{kernel:?}: {error}"));
            }
            let shader = rd.shader_create_from_spirv(&source);
            if !shader.is_valid() {
                return Err(format!("{kernel:?}: Godot did not accept the SPIR-V"));
            }
            let pipeline = rd.compute_pipeline_create(shader);
            if !pipeline.is_valid() {
                rd.free_rid(shader);
                failed.push(format!("{kernel:?}"));
                continue;
            }
            pipelines.insert(
                kernel,
                Compiled {
                    shader,
                    pipeline,
                    bindings,
                },
            );
        }
        if !failed.is_empty() {
            for p in pipelines.values() {
                rd.free_rid(p.pipeline);
                rd.free_rid(p.shader);
            }
            return Err(format!(
                "Godot could not create the compute pipelines of {}",
                failed.join(", ")
            ));
        }
        let format = data_format(config.format);
        let mut this = RdAtlas {
            texture: Rid::Invalid,
            rd,
            pipelines,
            format,
            size: config.size,
            pages: 0,
            resource: Texture2DArrayRd::new_gd(),
        };
        let placeholder = this.create_texture(1, 1);
        this.texture = placeholder;
        this.resource.set_texture_rd_rid(placeholder);
        Ok(this)
    }

    /// The pages as Godot samples them.
    pub fn texture(&self) -> Gd<Texture2DArrayRd> {
        self.resource.clone()
    }

    pub fn page_count(&self) -> u32 {
        self.pages
    }

    pub fn device_name(&self) -> String {
        self.rd.get_device_name().to_string()
    }

    /// A texture array of `pages` pages; Godot's arrays need at least two layers.
    fn create_texture(&mut self, size: u32, pages: u32) -> Rid {
        let mut format = RdTextureFormat::new_gd();
        format.set_format(self.format);
        format.set_width(size);
        format.set_height(size);
        format.set_array_layers(pages.max(2));
        format.set_texture_type(TextureType::TYPE_2D_ARRAY);
        format.set_usage_bits(
            TextureUsageBits::SAMPLING_BIT
                | TextureUsageBits::STORAGE_BIT
                | TextureUsageBits::CAN_COPY_FROM_BIT
                | TextureUsageBits::CAN_COPY_TO_BIT
                | TextureUsageBits::CAN_UPDATE_BIT,
        );
        self.rd.texture_create(&format, &RdTextureView::new_gd())
    }

    /// Grows the texture to `pages` pages, keeping the existing ones.
    fn grow(&mut self, pages: u32) {
        if pages <= self.pages {
            return;
        }
        let texture = self.create_texture(self.size, pages);
        let side = self.size as f32;
        for layer in 0..self.pages {
            self.rd.texture_copy(
                self.texture,
                texture,
                Vector3::ZERO,
                Vector3::ZERO,
                Vector3::new(side, side, 1.0),
                0,
                0,
                layer,
                layer,
            );
        }
        // The resource keeps its RID for Godot: materials keep sampling it.
        self.resource.set_texture_rd_rid(texture);
        self.rd.free_rid(self.texture);
        self.texture = texture;
        self.pages = pages;
    }

    /// Generates the glyphs queued in `atlas` into the texture (recorded into Godot's frame).
    /// Returns how many.
    pub fn flush(&mut self, atlas: &mut GlyphAtlas) -> usize {
        let Some(batch) = atlas.take_batch() else {
            return 0;
        };
        self.grow(atlas.page_count());
        let rd = &mut self.rd;
        let mut buffers: HashMap<Binding, Rid> = HashMap::new();
        for b in Binding::ALL {
            if matches!(b, Binding::Stencil | Binding::Corners) && !batch.error_correction {
                continue;
            }
            let rid = if let Some(bytes) = batch.bytes(b) {
                rd.storage_buffer_create_ex(bytes.len() as u32)
                    .data(&PackedByteArray::from(bytes))
                    .done()
            } else if let Some(size) = batch.output_size(b) {
                // Zeros: the error correction flags must start cleared.
                let zeros = PackedByteArray::from(vec![0u8; size as usize].as_slice());
                rd.storage_buffer_create_ex(size as u32).data(&zeros).done()
            } else {
                continue;
            };
            buffers.insert(b, rid);
        }
        let mut sets = Vec::new();
        let list = rd.compute_list_begin();
        let store = std::iter::once((Kernel::Store, batch.workgroups));
        for (kernel, (x, y)) in batch.kernels().chain(store) {
            let p = &self.pipelines[&kernel];
            let mut uniforms: Array<Gd<RdUniform>> = Array::new();
            for &b in &p.bindings {
                let mut u = RdUniform::new_gd();
                u.set_binding(b.index() as i32);
                if b == Binding::Atlas {
                    u.set_uniform_type(UniformType::IMAGE);
                    u.add_id(self.texture);
                } else {
                    u.set_uniform_type(UniformType::STORAGE_BUFFER);
                    u.add_id(buffers[&b]);
                }
                uniforms.push(&u);
            }
            let set = rd.uniform_set_create(&uniforms, p.shader, 0);
            rd.compute_list_bind_compute_pipeline(list, p.pipeline);
            rd.compute_list_bind_uniform_set(list, set, 0);
            rd.compute_list_dispatch(list, x, y, 1);
            rd.compute_list_add_barrier(list);
            sets.push(set);
        }
        rd.compute_list_end();
        // Freed once the GPU is done with them.
        for rid in sets.into_iter().chain(buffers.into_values()) {
            rd.free_rid(rid);
        }
        batch.layouts.len()
    }

    /// Frees the device objects.
    pub fn free(&mut self) {
        for p in self.pipelines.values() {
            self.rd.free_rid(p.pipeline);
            self.rd.free_rid(p.shader);
        }
        self.pipelines.clear();
        if self.texture.is_valid() {
            self.rd.free_rid(self.texture);
            self.texture = Rid::Invalid;
        }
    }
}
