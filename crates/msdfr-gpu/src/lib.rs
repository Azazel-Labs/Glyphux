//! GPU (wgpu / WGSL compute) distance field generation for [`msdfr`].
//!
//! The CPU prepares shapes and bins edges into tiles (exact culling bounds); a compute pass
//! evaluates every texel of a whole batch of glyphs in one dispatch, with the same edge
//! selection rules as the CPU generator.

pub mod atlas;
#[cfg(feature = "demo")]
pub mod demo;
pub mod field;
pub mod profile;
#[cfg(feature = "shaping")]
pub mod shaping;
pub mod text;
pub mod units;
mod workers;

pub use atlas::{AtlasConfig, AtlasConfigError, AtlasGlyph, AtlasStats, GlyphAtlas, GlyphKey};
pub use field::{FieldBatch, FieldGenerator, FieldJob, FieldLayout, PreparedJob, TILE, prepare_job, shader_source};
#[cfg(not(target_arch = "wasm32"))]
pub use field::{read_field, read_stencil, read_texture_layer};
pub use msdfr;
pub use msdfr::gpu::{FieldKind, GpuShape};
pub use profile::GpuProfiler;
#[cfg(feature = "shaping")]
pub use shaping::{ShapedGlyph, ShapedLine, Shaper, ShapingFont};
pub use text::{GlyphInstance, ResolvedStyle, Shadow, TextRenderer, TextStyle};
pub use units::{Length, Units};
pub use wgpu;

/// Limits the generator needs (fits WebGPU's and GLES 3.1's defaults).
pub fn required_limits() -> wgpu::Limits {
    wgpu::Limits {
        max_storage_buffers_per_shader_stage: 8,
        ..wgpu::Limits::downlevel_defaults()
    }
}

/// Opens the default adapter and a device suitable for the generator (blocking helper for
/// native tools and tests; apps usually pass their own device).
#[cfg(not(target_arch = "wasm32"))]
pub fn default_device() -> Result<(wgpu::Adapter, wgpu::Device, wgpu::Queue), String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster_block(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
        .map_err(|e| format!("no adapter: {e}"))?;
    let (device, queue) = pollster_block(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("msdfr"),
        // Timestamps for GpuProfiler where the adapter has them.
        required_features: adapter.features() & wgpu::Features::TIMESTAMP_QUERY,
        required_limits: required_limits().using_resolution(adapter.limits()),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        memory_hints: wgpu::MemoryHints::MemoryUsage,
        trace: wgpu::Trace::Off,
    }))
    .map_err(|e| format!("no device: {e}"))?;
    Ok((adapter, device, queue))
}

/// Minimal blocking executor (wgpu's native futures resolve after a poll or immediately).
#[cfg(not(target_arch = "wasm32"))]
fn pollster_block<F: Future>(f: F) -> F::Output {
    use std::sync::Arc;
    use std::task::{Context, Poll, Wake, Waker};
    struct Thread(std::thread::Thread);
    impl Wake for Thread {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(Thread(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut f = std::pin::pin!(f);
    loop {
        match f.as_mut().poll(&mut cx) {
            Poll::Ready(v) => return v,
            Poll::Pending => std::thread::park(),
        }
    }
}

/// Every WGSL module the crate builds, exactly as handed to wgpu, must pass naga's parser and
/// validator (WebGPU capabilities), so shader errors show up in `cargo test` without a GPU.
#[cfg(test)]
mod shader_validation {
    fn validate(name: &str, source: &str) {
        let module = naga::front::wgsl::parse_str(source)
            .unwrap_or_else(|e| panic!("{name}: WGSL parse error\n{}", e.emit_to_string(source)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("{name}: WGSL validation error\n{}", e.emit_to_string(source)));
    }

    #[test]
    fn field_generator() {
        validate("field generator", &crate::shader_source());
    }

    #[test]
    fn atlas_store() {
        for format in [
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureFormat::Rgba16Float,
            wgpu::TextureFormat::Rgba32Float,
        ] {
            validate(
                &format!("atlas store ({format:?})"),
                &crate::atlas::store_shader_source(format).expect("supported format"),
            );
        }
    }

    #[test]
    fn text() {
        validate("text", crate::text::TEXT_SHADER);
    }
}
