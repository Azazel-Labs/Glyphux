//! Translates glyphux's WGSL with naga, every build, so the shaders always match the glyphux
//! crates the library links and naga only runs here:
//!
//! * the shading library the addon's Godot shaders include, into `../shaders/generated/`
//!   (shipped with the addon, and committed);
//! * the compute kernels in SPIR-V, into `OUT_DIR/kernels/`, which the library embeds.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use glyphux_gpu::glsl::{self, GlslLibrary};
use glyphux_gpu::{AtlasFormat, Kernel, spirv};

/// Formats the store kernel is generated for.
const FORMATS: [AtlasFormat; 3] = [
    AtlasFormat::Rgba8Unorm,
    AtlasFormat::Rgba16Float,
    AtlasFormat::Rgba32Float,
];

const README: &str = "# Generated: do not edit

The addon's build (`rust/build.rs`) writes these files every time it builds the library,
from the WGSL of the glyphux crates it links (translated with naga). Edit the WGSL in
Glyphux instead, and build again.

| File | |
|---|---|
| `glyphux_text.gdshaderinc` | glyphux-gpu's text shading (`text_core.wgsl`) in Godot's shading language. |
| `glyphux_color_types.gdshaderinc`, `glyphux_color.gdshaderinc` | glyphux's color glyph shading (`color_core.wgsl`): its types, then its functions. Between the two, a shader defines how the color tables are read. |

`../text_common.gdshaderinc` and `../color_common.gdshaderinc` show how to include them.
";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // Again if a generated file is changed or deleted (changes of the WGSL rebuild this script).
    println!("cargo:rerun-if-changed=../shaders/generated");
    pdb_path();

    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let shaders = manifest.join("../shaders/generated");
    let mut files: Vec<(PathBuf, Vec<u8>)> = vec![(shaders.join("README.md"), README.into())];
    let text = glsl::text_library();
    files.push((
        shaders.join("glyphux_text.gdshaderinc"),
        library("GLYPHUX_TEXT", "text_core.wgsl", &[&text.types, &text.functions]).into(),
    ));
    let color = glsl::color_library();
    files.push((
        shaders.join("glyphux_color_types.gdshaderinc"),
        library("GLYPHUX_COLOR_TYPES", "the types of color_core.wgsl", &[&color.types]).into(),
    ));
    files.push((
        shaders.join("glyphux_color.gdshaderinc"),
        color_functions(&color).into(),
    ));
    for (path, contents) in &files {
        write_if_changed(path, contents);
    }
    remove_others(&shaders, &files);

    let kernels = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR")).join("kernels");
    for kernel in Kernel::ALL {
        let formats: &[AtlasFormat] = if kernel == Kernel::Store {
            &FORMATS
        } else {
            &FORMATS[..1]
        };
        for &format in formats {
            let k = spirv::kernel(kernel, format).unwrap_or_else(|e| panic!("{kernel:?}: {e}"));
            write_if_changed(&kernels.join(kernel_file(kernel, format)), &k.bytes());
        }
    }
}

/// The file of `kernel`'s SPIR-V (the store kernel's per atlas format) in `OUT_DIR/kernels/`;
/// `rd.rs` embeds them by the same names.
fn kernel_file(kernel: Kernel, format: AtlasFormat) -> String {
    if kernel == Kernel::Store {
        format!("store_{}.spv", format.wgsl_name())
    } else {
        format!("{}.spv", kernel.entry_point())
    }
}

/// A generated include: the header, a guard and `parts`.
fn library(guard: &str, source: &str, parts: &[&str]) -> String {
    let mut s = format!(
        "// Generated: do not edit. The Glyphux addon's rust/build.rs translates {source}\n\
         // with naga every build, and writes this file again. See README.md.\n\n\
         #ifndef {guard}\n#define {guard}\n\n"
    );
    for part in parts {
        s.push_str(part.trim_end());
        s.push_str("\n\n");
    }
    writeln!(s, "#endif // {guard}").unwrap();
    s
}

fn color_functions(color: &GlslLibrary) -> String {
    let reads = glsl::COLOR_EXTERNS
        .iter()
        .filter_map(|e| e.trim_start().strip_prefix("fn ")?.split('(').next())
        .collect::<Vec<_>>()
        .join(", ");
    let body = format!(
        "// Include glyphux_color_types.gdshaderinc first, then define how the shader reads\n\
         // the tables: {reads}\n\
         // (each taking a ColorTables and a uint, and returning the record).\n\n{}",
        color.functions
    );
    library("GLYPHUX_COLOR", "the functions of color_core.wgsl", &[&body])
}

/// Writes only changed files, so Godot's editor and cargo don't see a change every build.
fn write_if_changed(path: &Path, contents: &[u8]) {
    if std::fs::read(path).is_ok_and(|old| old == contents) {
        return;
    }
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("create the generated folder");
    std::fs::write(path, contents).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

/// Removes files of earlier builds that this one no longer generates (and their `.uid` and
/// `.import` files, which Godot's editor adds next to them).
fn remove_others(dir: &Path, files: &[(PathBuf, Vec<u8>)]) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            remove_others(&path, files);
            continue;
        }
        let owner = path.with_extension("");
        let kept = files.iter().any(|(f, _)| {
            *f == path || (matches!(path.extension(), Some(e) if e == "uid" || e == "import") && *f == owner)
        });
        if !kept {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Godot's Windows editor copies the DLL and its PDB to a temporary name so the build can
/// replace them while it runs, and patches that name into the DLL in place of the PDB path.
/// Rust embeds only the PDB's file name, too short for the temporary one: embed the full path
/// (OUT_DIR is <profile>/build/<package>/out).
fn pdb_path() {
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
        let profile = out.ancestors().nth(3).expect("the profile directory");
        let pdb = profile.join("glyphux_godot.pdb");
        println!("cargo:rustc-link-arg-cdylib=/PDBALTPATH:{}", pdb.display());
    }
}
