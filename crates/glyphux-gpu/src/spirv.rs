//! The compute kernels in SPIR-V, for backends that create pipelines from SPIR-V (Godot's
//! `RenderingDevice`, Vulkan). `kernel` translates them from their WGSL with naga (feature
//! `spirv`); [`bindings`] reads what a module binds, so a backend can load modules translated
//! ahead of time without naga.
//!
//! Each kernel is a module of its own: only its entry point, named `main`, and only the
//! bindings it uses, which [`SpirvKernel::bindings`] lists (descriptor set 0, at
//! [`Binding::index`]). Pipelines whose bindings must match the module exactly can build
//! their layouts from that list.

#[cfg(feature = "spirv")]
use naga::back::spv;

use crate::Binding;
#[cfg(feature = "spirv")]
use crate::{AtlasFormat, Kernel};

/// One kernel's SPIR-V.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpirvKernel {
    pub words: Vec<u32>,
    /// What it binds, in binding order (a subset of [`Kernel::bindings`](crate::Kernel::bindings)).
    pub bindings: Vec<Binding>,
}

impl SpirvKernel {
    /// The module as bytes (little-endian words).
    pub fn bytes(&self) -> Vec<u8> {
        to_bytes(&self.words)
    }
}

/// The bindings `words` (a kernel's SPIR-V module) declares, in binding order: its
/// `OpDecorate ... Binding` instructions. `None` if it is not SPIR-V or binds an index that is
/// not a [`Binding`].
pub fn bindings(words: &[u32]) -> Option<Vec<Binding>> {
    const MAGIC: u32 = 0x0723_0203;
    const OP_DECORATE: u32 = 71;
    const DECORATION_BINDING: u32 = 33;
    if words.first() != Some(&MAGIC) || words.len() < 5 {
        return None;
    }
    let mut out = Vec::new();
    let mut at = 5;
    while at < words.len() {
        let count = (words[at] >> 16) as usize;
        if count == 0 || at + count > words.len() {
            return None;
        }
        let op = &words[at..at + count];
        if op[0] & 0xffff == OP_DECORATE && count >= 4 && op[2] == DECORATION_BINDING {
            out.push(*Binding::ALL.get(op[3] as usize)?);
        }
        at += count;
    }
    out.sort_by_key(|b| b.index());
    out.dedup();
    Some(out)
}

/// `words` as bytes (little-endian), as SPIR-V files hold them.
pub fn to_bytes(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}

/// SPIR-V bytes (little-endian words) as words; `None` if the length is not a whole number
/// of words.
pub fn from_bytes(bytes: &[u8]) -> Option<Vec<u32>> {
    let (words, rest) = bytes.as_chunks::<4>();
    rest.is_empty()
        .then(|| words.iter().map(|w| u32::from_le_bytes(*w)).collect())
}

/// `kernel` in SPIR-V; `format` is the atlas pages' format (for [`Kernel::Store`]).
#[cfg(feature = "spirv")]
pub fn kernel(kernel: Kernel, format: AtlasFormat) -> Result<SpirvKernel, String> {
    let source = match kernel {
        Kernel::Store => crate::atlas::store_shader_source(format),
        _ => crate::shader_source(),
    };
    let mut module = naga::front::wgsl::parse_str(&source).map_err(|e| e.emit_to_string(&source))?;
    // Only this kernel, as `main`, and what it uses.
    module.entry_points.retain(|e| e.name == kernel.entry_point());
    if module.entry_points.is_empty() {
        return Err(format!("no entry point {}", kernel.entry_point()));
    }
    module.entry_points[0].name = "main".into();
    naga::compact::compact(&mut module, naga::compact::KeepUnused::No);
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::default(),
    )
    .validate(&module)
    .map_err(|e| e.emit_to_string(&source))?;
    let mut bindings: Vec<Binding> = module
        .global_variables
        .iter()
        .filter_map(|(_, var)| var.binding.as_ref())
        .map(|b| Binding::ALL[b.binding as usize])
        .collect();
    bindings.sort_by_key(|b| b.index());
    let options = spv::Options {
        lang_version: (1, 3),
        ..spv::Options::default()
    };
    let pipeline = spv::PipelineOptions {
        shader_stage: naga::ShaderStage::Compute,
        entry_point: "main".into(),
    };
    let words = spv::write_vec(&module, &info, &options, Some(&pipeline)).map_err(|e| e.to_string())?;
    Ok(SpirvKernel { words, bindings })
}

#[cfg(all(test, feature = "spirv"))]
mod tests {
    use crate::{AtlasFormat, Kernel};

    #[test]
    fn kernels_translate() {
        for kernel in Kernel::ALL {
            let k = super::kernel(kernel, AtlasFormat::Rgba8Unorm).unwrap_or_else(|e| panic!("{kernel:?}: {e}"));
            assert_eq!(k.words[0], 0x0723_0203, "{kernel:?}: SPIR-V magic number");
            assert_ne!(k.bindings.len(), 0, "{kernel:?} binds nothing");
            assert_eq!(
                super::bindings(&k.words).as_ref(),
                Some(&k.bindings),
                "{kernel:?}: read back"
            );
            assert_eq!(super::from_bytes(&k.bytes()).as_ref(), Some(&k.words));
            assert!(
                k.bindings.iter().all(|b| kernel.bindings().contains(b)),
                "{kernel:?} binds {:?}",
                k.bindings
            );
        }
    }
}
