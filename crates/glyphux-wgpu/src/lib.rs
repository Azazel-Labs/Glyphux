//! The wgpu backend of [`glyphux_gpu`]: distance fields generated with wgpu compute passes
//! (native and WebGPU), the glyph atlas texture, and renderers for MSDF text and color
//! glyphs.
//!
//! glyphux-gpu's API is re-exported, so this crate is all an application needs; its
//! [`GlyphAtlas`] is glyphux-gpu's cache with a texture.

pub mod atlas;
pub mod color;
#[cfg(feature = "demo")]
pub mod demo;
pub mod field;
pub mod profile;
pub mod text;

pub use atlas::{AtlasTexture, GlyphAtlas};
pub use field::{FieldBatch, FieldGenerator};
#[cfg(not(target_arch = "wasm32"))]
pub use field::{read_field, read_stencil, read_texture_layer, read_texture_rows};
pub use glyphux_gpu::{
    AtlasConfig, AtlasConfigError, AtlasFormat, AtlasGlyph, AtlasStats, Batch, Binding, FieldJob, FieldKind,
    FieldLayout, GlyphInstance, GlyphKey, GpuShape, Kernel, Length, PreparedJob, ResolvedStyle, Shadow, TILE,
    TextStyle, Units, glyphux, pack, prepare_job, shader_source, units,
};
#[cfg(feature = "shaping")]
pub use glyphux_gpu::{ShapedGlyph, ShapedLine, Shaper, ShapingFont, shaping, split_runs, split_runs_with};
pub use profile::GpuProfiler;
pub use text::TextRenderer;
pub use {glyphux_gpu, wgpu};

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
    default_device_on(wgpu::Backends::default())
}

/// [`default_device`] on one of `backends`. A host application that draws with OpenGL on the
/// same thread should leave out [`wgpu::Backends::GL`], whose context would replace its own.
#[cfg(not(target_arch = "wasm32"))]
pub fn default_device_on(backends: wgpu::Backends) -> Result<(wgpu::Adapter, wgpu::Device, wgpu::Queue), String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster_block(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
        .map_err(|e| format!("no adapter: {e}"))?;
    let (device, queue) = pollster_block(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("glyphux"),
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
