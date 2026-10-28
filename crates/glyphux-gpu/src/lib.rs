//! GPU distance field generation for [`glyphux`], independent of any graphics API.
//!
//! The CPU prepares shapes and bins edges into tiles (exact culling bounds); compute kernels
//! evaluate every texel of a whole batch of glyphs in one dispatch, with the same edge
//! selection rules as the CPU generator. This crate holds everything but the API calls: the
//! glyph atlas' cache and page allocation, the CPU preparation (on worker threads), the
//! batches the kernels read ([`field`]), text shaping and layout, and the WGSL of every
//! shader. A backend creates the buffers and textures, compiles the shaders for its API and
//! runs the kernels a batch lists:
//!
//! * `glyphux-wgpu`: wgpu (native and WebGPU), with text and color renderers.
//! * `glyphux-godot`: Godot 4's own `RenderingDevice`, as a GDExtension.
//! * `glyphux-unreal`: Unreal Engine 5's render graph, as a plugin.

pub mod atlas;
pub mod color_atlas;
pub mod field;
#[cfg(feature = "glsl")]
pub mod glsl;
#[cfg(feature = "hlsl")]
pub mod hlsl;
#[cfg(feature = "shaping")]
pub mod paragraph;
#[cfg(feature = "shaping")]
pub mod shaping;
pub mod spirv;
pub mod text;
pub mod units;
mod workers;

pub use atlas::{AtlasConfig, AtlasConfigError, AtlasFormat, AtlasGlyph, AtlasStats, GlyphAtlas, GlyphKey};
pub use field::{Batch, Binding, FieldJob, FieldLayout, Kernel, PreparedJob, TILE, pack, prepare_job, shader_source};
pub use glyphux;
pub use glyphux::gpu::{FieldKind, GpuShape};
#[cfg(feature = "shaping")]
pub use shaping::{ShapedGlyph, ShapedLine, Shaper, ShapingFont, split_runs, split_runs_with};
pub use text::{GlyphInstance, ResolvedStyle, Shadow, TextStyle};
pub use units::{Length, Units};

/// Every WGSL module the crate provides, exactly as backends get it, must pass naga's parser
/// and validator (WebGPU capabilities), so shader errors show up in `cargo test` without a GPU.
#[cfg(test)]
mod shader_validation {
    use crate::{AtlasFormat, Binding, Kernel};

    fn parse(name: &str, source: &str) -> (naga::Module, naga::valid::ModuleInfo) {
        let module = naga::front::wgsl::parse_str(source)
            .unwrap_or_else(|e| panic!("{name}: WGSL parse error\n{}", e.emit_to_string(source)));
        let info = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("{name}: WGSL validation error\n{}", e.emit_to_string(source)));
        (module, info)
    }

    #[test]
    fn field_generator() {
        parse("field generator", &crate::shader_source());
    }

    #[test]
    fn atlas_store() {
        for format in [
            AtlasFormat::Rgba8Unorm,
            AtlasFormat::Rgba16Float,
            AtlasFormat::Rgba32Float,
        ] {
            parse(
                &format!("atlas store ({format:?})"),
                &crate::atlas::store_shader_source(format),
            );
        }
    }

    #[test]
    fn text() {
        parse("text", crate::text::TEXT_SHADER);
    }

    #[test]
    fn color() {
        parse("color", glyphux::color::COLOR_SHADER);
    }

    /// Every kernel's entry point exists and uses only the bindings [`Kernel::bindings`]
    /// lists, with the access [`Binding::read_only`] gives: what backends build their bind
    /// group layouts from.
    #[test]
    fn kernel_contract() {
        use std::collections::BTreeSet;
        let field = parse("field generator", &crate::shader_source());
        let store = parse(
            "atlas store",
            &crate::atlas::store_shader_source(AtlasFormat::Rgba8Unorm),
        );
        for kernel in [
            Kernel::Distance,
            Kernel::CorrectCorners,
            Kernel::CorrectProtect,
            Kernel::CorrectFind,
            Kernel::CorrectApply,
            Kernel::Store,
        ] {
            let (module, info) = if kernel == Kernel::Store { &store } else { &field };
            let index = module
                .entry_points
                .iter()
                .position(|e| e.name == kernel.entry_point())
                .unwrap_or_else(|| panic!("{kernel:?}: no entry point {}", kernel.entry_point()));
            let declared: BTreeSet<u32> = kernel.bindings().iter().map(|b| b.index()).collect();
            let entry = info.get_entry_point(index);
            for (handle, var) in module.global_variables.iter() {
                let Some(rb) = &var.binding else {
                    continue;
                };
                assert_eq!(rb.group, 0, "{kernel:?}");
                if entry[handle].is_empty() {
                    continue;
                }
                assert!(
                    declared.contains(&rb.binding),
                    "{kernel:?} uses binding {} without declaring it",
                    rb.binding
                );
                let binding = Binding::ALL[rb.binding as usize];
                let writable = match var.space {
                    naga::AddressSpace::Storage { access } => access.contains(naga::StorageAccess::STORE),
                    _ => true,
                };
                assert_eq!(binding.read_only(), !writable, "{kernel:?}: {binding:?}");
            }
        }
    }
}
