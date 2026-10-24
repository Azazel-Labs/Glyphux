//! The shading functions translated from WGSL to GLSL with naga (feature `glsl`), for backends
//! whose shaders are written in a GLSL-like language: Godot's shading language takes them as
//! they are.
//!
//! The WGSL stays the only source: [`text_library`] is [`crate::text::TEXT_CORE`] and
//! [`color_library`] is `glyphux::color::COLOR_CORE`, translated. A host shader declares
//! [`GlslLibrary::types`], then defines the functions the library calls but leaves to the
//! host (the color tables' reads, see `COLOR_CORE`), then adds [`GlslLibrary::functions`].

use naga::back::glsl;

/// A library of GLSL functions, split so that host definitions can go in between.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlslLibrary {
    /// The struct declarations.
    pub types: String,
    /// The functions (in an order where each comes after the ones it calls).
    pub functions: String,
}

/// Translates a WGSL library of functions (no bindings, no entry points). `externs` are WGSL
/// stub definitions of the functions it calls that the host defines; they are translated
/// with the rest and left out of the result.
pub fn translate(wgsl: &str, externs: &[&str]) -> Result<GlslLibrary, String> {
    const ENTRY: &str = "glyphux_library_entry";
    let source = format!(
        "{wgsl}\n{}\n@compute @workgroup_size(1)\nfn {ENTRY}() {{}}\n",
        externs.join("\n")
    );
    let module = naga::front::wgsl::parse_str(&source).map_err(|e| e.emit_to_string(&source))?;
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::default(),
    )
    .validate(&module)
    .map_err(|e| e.emit_to_string(&source))?;
    let options = glsl::Options {
        version: glsl::Version::Embedded {
            version: 310,
            is_webgl: false,
        },
        ..glsl::Options::default()
    };
    let pipeline = glsl::PipelineOptions {
        shader_stage: naga::ShaderStage::Compute,
        entry_point: ENTRY.into(),
        multiview: None,
    };
    let mut out = String::new();
    glsl::Writer::new(
        &mut out,
        &module,
        &info,
        &options,
        &pipeline,
        naga::proc::BoundsCheckPolicies::default(),
    )
    .and_then(|mut w| w.write().map(|_| ()))
    .map_err(|e| e.to_string())?;
    // The host's functions, and the entry point (`main` in GLSL), are not part of the library.
    let mut skip: Vec<&str> = externs
        .iter()
        .filter_map(|e| e.trim_start().strip_prefix("fn ")?.split('(').next())
        .map(str::trim)
        .collect();
    skip.push("main");
    split(&out, &skip)
}

/// Splits naga's GLSL into struct declarations and functions (top level blocks: naga starts
/// them in the first column and ends them with `}` or `};` there), dropping the directives
/// and the functions named in `skip`.
fn split(glsl: &str, skip: &[&str]) -> Result<GlslLibrary, String> {
    let mut types = String::new();
    let mut functions = String::new();
    let mut lines = glsl.lines();
    while let Some(line) = lines.next() {
        if line.trim().is_empty()
            || line.starts_with('#')
            || line.starts_with("precision ")
            || line.starts_with("layout(")
        {
            continue;
        }
        let mut block = String::from(line);
        block.push('\n');
        if !line.ends_with(';') {
            for next in lines.by_ref() {
                block.push_str(next);
                block.push('\n');
                if next == "}" || next == "};" {
                    break;
                }
            }
        }
        if line.starts_with("struct ") {
            types.push_str(&block);
            types.push('\n');
        } else if let Some(name) = line.split('(').next().and_then(|s| s.split_whitespace().last()) {
            if !skip.contains(&name) {
                functions.push_str(&block);
                functions.push('\n');
            }
        } else {
            return Err(format!("unexpected line in naga's GLSL: {line}"));
        }
    }
    Ok(GlslLibrary { types, functions })
}

/// [`crate::text::TEXT_CORE`] in GLSL.
pub fn text_library() -> GlslLibrary {
    translate(crate::text::TEXT_CORE, &[]).expect("TEXT_CORE translates")
}

/// The color tables' reads a host defines for [`color_library`], as stubs.
pub const COLOR_EXTERNS: &[&str] = &[
    "fn color_tile(t: ColorTables, i: u32) -> ColorTile { var v: ColorTile; return v; }",
    "fn color_paint(t: ColorTables, i: u32) -> ColorPaint { var v: ColorPaint; return v; }",
    "fn color_stop(t: ColorTables, i: u32) -> ColorStop { var v: ColorStop; return v; }",
    "fn color_recipe(t: ColorTables, i: u32) -> u32 { return 0u; }",
];

/// `glyphux::color::COLOR_CORE` in GLSL. The host defines `color_tile`, `color_paint`,
/// `color_stop` and `color_recipe` (taking a `ColorTables` and a `uint`) between the types
/// and the functions.
pub fn color_library() -> GlslLibrary {
    translate(glyphux::color::COLOR_CORE, COLOR_EXTERNS).expect("COLOR_CORE translates")
}

#[cfg(test)]
mod tests {
    #[test]
    fn libraries_translate() {
        let text = super::text_library();
        assert!(text.types.contains("struct TextGlyph"));
        assert!(text.functions.contains("vec4 text_color("));
        assert!(!text.functions.contains("void main"));
        let color = super::color_library();
        assert!(color.types.contains("struct ColorTile"));
        assert!(color.functions.contains("vec4 color_shade("));
        assert!(
            !color.functions.contains("ColorTile color_tile("),
            "the host defines the reads"
        );
    }
}
