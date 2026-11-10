//! The compute kernels and the shading functions translated from WGSL to HLSL with naga
//! (feature `hlsl`), for backends whose shaders are HLSL: Unreal Engine's global shaders, for
//! example.
//!
//! The WGSL stays the only source, as for the [`spirv`](crate::spirv) and
//! [`glsl`](crate::glsl) translations:
//!
//! * [`kernel`] is one kernel as a compute shader of its own: only its entry point, named
//!   `main`, and only the resources it uses, which [`HlslKernel::bindings`] lists with their
//!   HLSL names and types. Storage buffers are (`RW`)`ByteAddressBuffer`s, the atlas a
//!   `RWTexture2DArray`.
//! * [`text_library`] and [`color_library`] are [`crate::text::TEXT_CORE`] and
//!   `glyphux::color::COLOR_CORE` as HLSL functions, for a host's draw shaders.
//!
//! The output has no `register(...)` assignments: hosts that bind resources by name (Unreal
//! does, through shader reflection) assign their own. A host that needs explicit registers
//! can number [`HlslKernel::bindings`] in order.

use std::fmt::Write as _;

use naga::back::hlsl;

use crate::{AtlasFormat, Binding, Kernel};

/// How a kernel resource is declared in HLSL.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HlslResource {
    /// `ByteAddressBuffer`: a read-only storage buffer.
    Buffer,
    /// `RWByteAddressBuffer`: a read-write storage buffer.
    RwBuffer,
    /// `RWTexture2DArray<...>`: the atlas pages, written by the store kernel.
    RwTextureArray,
}

impl HlslResource {
    /// The HLSL type, as the kernel declares it.
    pub fn hlsl_type(self, format: AtlasFormat) -> &'static str {
        match self {
            HlslResource::Buffer => "ByteAddressBuffer",
            HlslResource::RwBuffer => "RWByteAddressBuffer",
            HlslResource::RwTextureArray => match format {
                AtlasFormat::Rgba8Unorm => "RWTexture2DArray<unorm float4>",
                AtlasFormat::Rgba16Float | AtlasFormat::Rgba32Float => "RWTexture2DArray<float4>",
            },
        }
    }
}

/// A resource one kernel binds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HlslBinding {
    pub binding: Binding,
    /// Its name in the HLSL source (the WGSL variable's).
    pub name: String,
    pub resource: HlslResource,
}

/// One kernel's HLSL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HlslKernel {
    /// A complete compute shader: the kernel as `main`, with `[numthreads]`.
    pub source: String,
    /// What it binds, in binding order (a subset of [`Kernel::bindings`]).
    pub bindings: Vec<HlslBinding>,
}

/// The entry point every [`kernel`] is renamed to.
pub const ENTRY_POINT: &str = "main";

fn options() -> hlsl::Options {
    hlsl::Options {
        shader_model: hlsl::ShaderModel::V6_0,
        // Bindings come out with registers numbered from their group and binding, which
        // `strip_registers` removes.
        fake_missing_bindings: true,
        zero_initialize_workgroup_memory: true,
        ..hlsl::Options::default()
    }
}

fn validate(module: &naga::Module, source: &str) -> Result<naga::valid::ModuleInfo, String> {
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::default(),
    )
    .validate(module)
    .map_err(|e| e.emit_to_string(source))
}

fn write(
    module: &naga::Module,
    info: &naga::valid::ModuleInfo,
    entry: Option<(naga::ShaderStage, String)>,
) -> Result<String, String> {
    let options = options();
    let pipeline = hlsl::PipelineOptions { entry_point: entry };
    let mut out = String::new();
    hlsl::Writer::new(&mut out, &options, &pipeline)
        .write(module, info, None)
        .map_err(|e| e.to_string())?;
    Ok(wrap_array_returns(&strip_registers(&out)))
}

/// Makes naga's functions that return arrays return a struct holding the array instead:
/// `typedef T ret_f[N]; ret_f f(...)` becomes `struct ret_f { T v[N]; }; ret_f f(...)`, its
/// `return x;` statements fill the struct, and its calls read `.v`. HLSL compilers take the
/// typedef, but Unreal's shader minifier (dead code removal, on by default) names a typedef
/// after its last token, `[N]` here, so it never sees the type used and drops it.
fn wrap_array_returns(hlsl: &str) -> String {
    let mut text = hlsl.to_string();
    let typedefs: Vec<(String, String, String)> = hlsl
        .lines()
        .filter_map(|line| {
            let decl = line.strip_prefix("typedef ")?.strip_suffix(';')?;
            let (ty, rest) = decl.split_once(' ')?;
            let open = rest.find('[')?;
            Some((ty.to_string(), rest[..open].to_string(), rest[open..].to_string()))
        })
        .collect();
    for (ty, ret, dims) in typedefs {
        text = text.replacen(
            &format!("typedef {ty} {ret}{dims};"),
            &format!("struct {ret} {{ {ty} v{dims}; }};"),
            1,
        );
        // The function returning it: `ret_f f(` in the first column.
        let head = format!("\n{ret} ");
        let Some(at) = text.find(&head) else { continue };
        let start = at + head.len();
        let Some(function) = text[start..].split('(').next().map(str::trim).map(str::to_string) else {
            continue;
        };
        let Some(len) = text[start..].find("\n}") else { continue };
        let end = start + len;
        let body = wrap_returns(&text[start..end], &ret);
        text = format!("{}{body}{}", &text[..start], &text[end..]);
        text = read_member(&text, &function, &format!("{ret} "));
    }
    text
}

/// `return x;` in `body` as `{ ret glyphux_ret; glyphux_ret.v = x; return glyphux_ret; }`.
fn wrap_returns(body: &str, ret: &str) -> String {
    let mut out = String::with_capacity(body.len() + 64);
    let mut rest = body;
    while let Some(at) = rest.find("return ") {
        out.push_str(&rest[..at]);
        rest = &rest[at + "return ".len()..];
        let Some(end) = rest.find(';') else { break };
        let _ = write!(
            out,
            "{{ {ret} glyphux_ret; glyphux_ret.v = {}; return glyphux_ret; }}",
            &rest[..end]
        );
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

/// Appends `.v` to every call of `function` in `text`, except its definition (the call
/// preceded by `definition`, its return type).
fn read_member(text: &str, function: &str, definition: &str) -> String {
    let pattern = format!("{function}(");
    let mut out = String::with_capacity(text.len() + 64);
    let mut rest = text;
    // The closing parentheses still to mark, by their depth below the current position.
    let mut pending: Vec<usize> = Vec::new();
    let mut depth = 0usize;
    loop {
        let next_call = rest.find(&pattern).filter(|&at| {
            rest[..at].chars().next_back().is_none_or(|c| !is_ident(c))
                && !(at == 0 && out.chars().next_back().is_some_and(is_ident))
        });
        let next_paren = rest.find(['(', ')']);
        match (next_call, next_paren) {
            (Some(call), Some(paren)) if call <= paren => {
                let is_definition = rest[..call].ends_with(definition) || (call == 0 && out.ends_with(definition));
                out.push_str(&rest[..call + pattern.len()]);
                rest = &rest[call + pattern.len()..];
                depth += 1;
                if !is_definition {
                    pending.push(depth);
                }
            }
            (_, Some(paren)) => {
                let c = rest.as_bytes()[paren];
                out.push_str(&rest[..=paren]);
                rest = &rest[paren + 1..];
                if c == b'(' {
                    depth += 1;
                } else {
                    if pending.last() == Some(&depth) {
                        pending.pop();
                        out.push_str(".v");
                    }
                    depth = depth.saturating_sub(1);
                }
            }
            _ => break,
        }
    }
    out.push_str(rest);
    out
}

/// Removes every `: register(...)` assignment.
fn strip_registers(hlsl: &str) -> String {
    let mut out = String::with_capacity(hlsl.len());
    let mut rest = hlsl;
    while let Some(at) = rest.find(" : register(") {
        out.push_str(&rest[..at]);
        let after = &rest[at + " : register(".len()..];
        let Some(end) = after.find(')') else {
            rest = after;
            break;
        };
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// `kernel` in HLSL; `format` is the atlas pages' format (for [`Kernel::Store`]).
pub fn kernel(kernel: Kernel, format: AtlasFormat) -> Result<HlslKernel, String> {
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
    module.entry_points[0].name = ENTRY_POINT.into();
    naga::compact::compact(&mut module, naga::compact::KeepUnused::No);
    let info = validate(&module, &source)?;
    let mut bindings: Vec<HlslBinding> = Vec::new();
    for (_, var) in module.global_variables.iter() {
        let Some(b) = &var.binding else {
            continue;
        };
        let binding = *Binding::ALL
            .get(b.binding as usize)
            .ok_or_else(|| format!("{kernel:?} binds {}, which is not a Binding", b.binding))?;
        let name = var
            .name
            .clone()
            .ok_or_else(|| format!("{kernel:?}: binding {} has no name", b.binding))?;
        let resource = match binding {
            Binding::Atlas => HlslResource::RwTextureArray,
            b if b.read_only() => HlslResource::Buffer,
            _ => HlslResource::RwBuffer,
        };
        bindings.push(HlslBinding {
            binding,
            name,
            resource,
        });
    }
    bindings.sort_by_key(|b| b.binding.index());
    let hlsl = write(&module, &info, Some((naga::ShaderStage::Compute, ENTRY_POINT.into())))?;
    // naga keeps the WGSL names unless they collide with HLSL's: check, since hosts bind by name.
    for b in &bindings {
        let declared = format!("{} {}", b.resource.hlsl_type(format), b.name);
        let alt = format!("{} {};", b.resource.hlsl_type(format), b.name);
        if !hlsl.contains(&declared) && !hlsl.contains(&alt) {
            return Err(format!("{kernel:?}: naga did not declare `{declared}` (renamed?)"));
        }
    }
    Ok(HlslKernel { source: hlsl, bindings })
}

/// Translates a WGSL library of functions (no bindings, no entry points) into HLSL types and
/// functions. `externs` are WGSL stub definitions of functions the library calls but that the
/// host defines: in the result they are prototypes, which the host's definitions (after the
/// library) complete.
pub fn translate(wgsl: &str, externs: &[&str]) -> Result<String, String> {
    const ENTRY: &str = "glyphux_library_entry";
    let source = format!(
        "{wgsl}\n{}\n@compute @workgroup_size(1)\nfn {ENTRY}() {{}}\n",
        externs.join("\n")
    );
    let module = naga::front::wgsl::parse_str(&source).map_err(|e| e.emit_to_string(&source))?;
    let info = validate(&module, &source)?;
    let hlsl = write(&module, &info, None)?;
    let names: Vec<&str> = externs
        .iter()
        .filter_map(|e| e.trim_start().strip_prefix("fn ")?.split('(').next())
        .map(str::trim)
        .collect();
    library(&hlsl, ENTRY, &names)
}

/// naga's HLSL as a library: the entry point left out, the functions named in `externs` turned
/// into prototypes (naga starts top level blocks in the first column and ends them with `}`
/// or `};` there).
fn library(hlsl: &str, entry: &str, externs: &[&str]) -> Result<String, String> {
    let mut out = String::new();
    let mut lines = hlsl.lines().peekable();
    let mut found = vec![false; externs.len()];
    while let Some(line) = lines.next() {
        // An attribute line belongs to the block after it.
        let mut block = String::from(line);
        block.push('\n');
        let mut head = line;
        if line.starts_with('[') {
            let Some(next) = lines.next() else {
                break;
            };
            block.push_str(next);
            block.push('\n');
            head = next;
        }
        let opens_block = !head.trim_end().ends_with(';') && !head.trim().is_empty() && !head.starts_with('#');
        if opens_block && !head.starts_with("//") {
            for next in lines.by_ref() {
                block.push_str(next);
                block.push('\n');
                if next == "}" || next == "};" {
                    break;
                }
            }
        }
        let name = head
            .split('(')
            .next()
            .and_then(|s| s.split_whitespace().last())
            .unwrap_or("");
        if head.contains('(') && name == entry {
            continue;
        }
        if let Some(i) = externs.iter().position(|e| *e == name).filter(|_| head.contains('(')) {
            found[i] = true;
            out.push_str(head.trim_end().trim_end_matches('{').trim_end());
            out.push_str(";\n");
            continue;
        }
        out.push_str(&block);
    }
    if let Some(i) = found.iter().position(|f| !f) {
        return Err(format!("naga's HLSL has no function {}", externs[i]));
    }
    Ok(out)
}

/// [`crate::text::TEXT_CORE`] in HLSL.
pub fn text_library() -> String {
    translate(crate::text::TEXT_CORE, &[]).expect("TEXT_CORE translates")
}

/// The color tables' reads a host defines for [`color_library`], as stubs (the same as the
/// GLSL library's).
pub const COLOR_EXTERNS: &[&str] = &[
    "fn color_tile(t: ColorTables, i: u32) -> ColorTile { var v: ColorTile; return v; }",
    "fn color_paint(t: ColorTables, i: u32) -> ColorPaint { var v: ColorPaint; return v; }",
    "fn color_stop(t: ColorTables, i: u32) -> ColorStop { var v: ColorStop; return v; }",
    "fn color_recipe(t: ColorTables, i: u32) -> u32 { return 0u; }",
];

/// `glyphux::color::COLOR_CORE` in HLSL. It declares `color_tile`, `color_paint`,
/// `color_stop` and `color_recipe` (taking a `ColorTables` and a `uint`) without defining
/// them: the host defines them after it.
pub fn color_library() -> String {
    translate(glyphux::color::COLOR_CORE, COLOR_EXTERNS).expect("COLOR_CORE translates")
}

/// [`color_library`] for hosts that read the tables from a resource they can only pass as a
/// parameter (a texture input of an Unreal material's Custom node, say): the table reads, and
/// every function that calls them directly or not, take `param` (an HLSL parameter
/// declaration, such as `Texture2D tables`) first, and pass it on. naga's helper functions
/// (`naga_*`) are renamed `glyphux_color_naga_*`, so that the library can share a shader with
/// others.
pub fn color_library_with(param: &str) -> String {
    let name = param
        .split_whitespace()
        .last()
        .expect("a parameter declaration: type and name");
    let externs: Vec<&str> = COLOR_EXTERNS
        .iter()
        .filter_map(|e| e.trim_start().strip_prefix("fn ")?.split('(').next())
        .map(str::trim)
        .collect();
    thread_parameter(&color_library(), &externs, param, name).replace_word_prefix("naga_", "glyphux_color_naga_")
}

/// Identifier boundaries in HLSL source.
fn is_ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

trait ReplaceWordPrefix {
    /// Replaces `from` where it starts an identifier.
    fn replace_word_prefix(&self, from: &str, to: &str) -> String;
}

impl ReplaceWordPrefix for String {
    fn replace_word_prefix(&self, from: &str, to: &str) -> String {
        let mut out = String::with_capacity(self.len());
        let mut rest = self.as_str();
        while let Some(at) = rest.find(from) {
            let starts = rest[..at].chars().next_back().is_none_or(|c| !is_ident(c))
                && !(at == 0 && out.chars().next_back().is_some_and(is_ident));
            out.push_str(&rest[..at]);
            out.push_str(if starts { to } else { from });
            rest = &rest[at + from.len()..];
        }
        out.push_str(rest);
        out
    }
}

/// Inserts `arg` as the first argument of every call of `function` in `text`.
fn add_argument(text: &str, function: &str, arg: &str) -> String {
    let pattern = format!("{function}(");
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(&pattern) {
        let whole = rest[..at].chars().next_back().is_none_or(|c| !is_ident(c))
            && !(at == 0 && out.chars().next_back().is_some_and(is_ident));
        out.push_str(&rest[..at + pattern.len()]);
        rest = &rest[at + pattern.len()..];
        if whole {
            out.push_str(arg);
            if !rest.trim_start().starts_with(')') {
                out.push_str(", ");
            }
        }
    }
    out.push_str(rest);
    out
}

/// Makes the functions named in `roots`, and every function calling them directly or not,
/// take `param` first, and passes `name` to them in every call.
fn thread_parameter(hlsl: &str, roots: &[&str], param: &str, name: &str) -> String {
    // Top level blocks (naga starts them in the first column), with the name of the
    // function each declares.
    let mut blocks: Vec<(Option<String>, String)> = Vec::new();
    let mut lines = hlsl.lines().peekable();
    while let Some(line) = lines.next() {
        let mut block = format!("{line}\n");
        let is_function =
            line.contains('(') && !line.starts_with(' ') && !line.starts_with("//") && !line.starts_with('#');
        if is_function && !line.trim_end().ends_with(';') {
            for next in lines.by_ref() {
                block.push_str(next);
                block.push('\n');
                if next == "}" {
                    break;
                }
            }
        }
        let fname = is_function
            .then(|| {
                line.split('(')
                    .next()
                    .and_then(|h| h.split_whitespace().last())
                    .map(str::to_string)
            })
            .flatten();
        blocks.push((fname, block));
    }
    // The functions that need the parameter: the roots and their callers, to a fixed point.
    let mut threaded: Vec<String> = roots.iter().map(ToString::to_string).collect();
    loop {
        let before = threaded.len();
        for (fname, block) in &blocks {
            let Some(fname) = fname else { continue };
            if threaded.contains(fname) {
                continue;
            }
            let body = block.split_once('\n').map_or("", |(_, b)| b);
            if threaded.iter().any(|t| add_argument(body, t, "X") != body) {
                threaded.push(fname.clone());
            }
        }
        if threaded.len() == before {
            break;
        }
    }
    let mut out = String::with_capacity(hlsl.len() + 64 * threaded.len());
    for (fname, block) in &blocks {
        let (head, body) = block.split_once('\n').unwrap_or((block, ""));
        let head = match fname {
            Some(f) if threaded.contains(f) => add_argument(head, f, param),
            _ => head.to_string(),
        };
        let mut body = body.to_string();
        for t in &threaded {
            body = add_argument(&body, t, name);
        }
        out.push_str(&head);
        out.push('\n');
        out.push_str(&body);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernels_translate() {
        for format in [
            AtlasFormat::Rgba8Unorm,
            AtlasFormat::Rgba16Float,
            AtlasFormat::Rgba32Float,
        ] {
            for kernel in Kernel::ALL {
                let k = super::kernel(kernel, format).unwrap_or_else(|e| panic!("{kernel:?}: {e}"));
                assert!(k.source.contains("[numthreads("), "{kernel:?}: a compute shader");
                assert!(
                    k.source.contains(&format!(" {ENTRY_POINT}(")),
                    "{kernel:?}: entry point"
                );
                assert!(!k.source.contains("register("), "{kernel:?}: registers left in");
                assert_ne!(k.bindings.len(), 0, "{kernel:?} binds nothing");
                assert!(
                    k.bindings.iter().all(|b| kernel.bindings().contains(&b.binding)),
                    "{kernel:?} binds {:?}",
                    k.bindings
                );
                let store = k.bindings.iter().any(|b| b.binding == Binding::Atlas);
                assert_eq!(
                    store,
                    kernel == Kernel::Store,
                    "{kernel:?}: only the store kernel writes the atlas"
                );
            }
        }
    }

    #[test]
    fn libraries_translate() {
        let text = text_library();
        assert!(text.contains("struct TextGlyph"));
        assert!(text.contains("text_color("));
        assert!(!text.contains("glyphux_library_entry"));
        let color = color_library();
        assert!(color.contains("struct ColorTile"));
        assert!(color.contains("color_shade("));
        for name in ["color_tile", "color_paint", "color_stop", "color_recipe"] {
            let at = color
                .find(&format!(" {name}("))
                .unwrap_or_else(|| panic!("{name} declared"));
            let line_end = color[at..].find('\n').map_or(color.len(), |e| at + e);
            assert!(color[at..line_end].trim_end().ends_with(';'), "{name} is a prototype");
        }
    }

    #[test]
    fn color_library_threads_its_tables() {
        let color = color_library_with("Texture2D tables");
        for name in ["color_tile", "color_paint", "color_stop", "color_recipe"] {
            assert!(
                color.contains(&format!("{name}(Texture2D tables, ColorTables")),
                "{name} takes the tables"
            );
        }
        // Every caller passes them on, up to the entry points of the library.
        assert!(color.contains("color_cell(Texture2D tables, ColorTables"));
        assert!(color.contains("color_shade(Texture2D tables, ColorTables"));
        assert!(color.contains("color_tile(tables, "));
        assert!(!color.contains(" naga_"), "helpers renamed");
        assert!(color.contains("glyphux_color_naga_"));
    }

    #[test]
    fn arguments_and_prefixes() {
        assert_eq!(
            add_argument("a = f(x) + gf(y) + f();", "f", "t"),
            "a = f(t, x) + gf(y) + f(t);"
        );
        assert_eq!(
            "naga_div(x) + my_naga_div(y)"
                .to_string()
                .replace_word_prefix("naga_", "p_naga_"),
            "p_naga_div(x) + my_naga_div(y)"
        );
    }

    #[test]
    fn array_returns_are_wrapped() {
        let hlsl = "typedef float ret_f[2];\nret_f f(float a) {\n    float r[2] = { a, a };\n    return r;\n}\n\nvoid g() {\n    float x[2] = f(h(1.0));\n    float y = f(f(2.0)[0])[1];\n}\n";
        assert_eq!(
            wrap_array_returns(hlsl),
            "struct ret_f { float v[2]; };\nret_f f(float a) {\n    float r[2] = { a, a };\n    { ret_f glyphux_ret; glyphux_ret.v = r; return glyphux_ret; }\n}\n\nvoid g() {\n    float x[2] = f(h(1.0)).v;\n    float y = f(f(2.0).v[0]).v[1];\n}\n"
        );
        // Unreal's shader minifier drops array typedefs: none are left.
        for kernel in Kernel::ALL {
            let k = super::kernel(kernel, AtlasFormat::Rgba8Unorm).unwrap();
            assert!(
                !k.source.lines().any(|l| l.starts_with("typedef ") && l.contains('[')),
                "{kernel:?}"
            );
        }
        for lib in [text_library(), color_library_with("Texture2D tables")] {
            assert!(!lib.lines().any(|l| l.starts_with("typedef ") && l.contains('[')));
        }
    }

    #[test]
    fn registers_are_stripped() {
        assert_eq!(
            strip_registers("ByteAddressBuffer a : register(t0);\nRWByteAddressBuffer b : register(u1, space0);\n"),
            "ByteAddressBuffer a;\nRWByteAddressBuffer b;\n"
        );
    }
}
