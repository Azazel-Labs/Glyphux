# glyphux-gpu

GPU generation of SDF / PSDF / MSDF / MTSDF glyph fields for [`glyphux`](../glyphux),
independent of the graphics API. This crate makes no graphics API calls. It contains
everything else needed to generate fields at run time: the glyph atlas' cache and page
allocation, the CPU preparation of each glyph (on worker threads), the packed batches the
compute kernels read, the WGSL source of every shader, text shaping and layout.

A backend owns the device objects and runs what this crate prepares:

| Crate | Graphics API |
|---|---|
| [`glyphux-wgpu`](../glyphux-wgpu) | wgpu: native (Vulkan, Metal, DX12, GL) and WebGPU. Adds the atlas texture, text and color glyph renderers, and a GPU profiler. Start here for a Rust application. |
| [`glyphux-godot`](../../examples/godot/addons/glyphux) | A Godot 4.6 addon: `GlyphuxLabel`, `GlyphuxRichLabel` and `GlyphuxLabel3D` nodes. |
| [`glyphux-unreal`](../../examples/unreal/Plugins/Glyphux) | An Unreal Engine 5.8 plugin, generating in the render graph (RDG): a UMG widget, a scene component, a surface material and Niagara particles. |

The [workspace README](../../README.md#how-it-works) explains the method; glyphux-wgpu's
README covers the [atlas, layout and rendering](../glyphux-wgpu/README.md#usage) and the
[validation against the CPU generator](../glyphux-wgpu/README.md#validation).

## Running a batch

Each frame, the application requests glyphs from a [`GlyphAtlas`](src/atlas.rs) at the size
they are drawn (directly, or through `text::layout_text`). A request returns the glyph's
final placement at once and queues its preparation on the workers. Then the backend:

1. Calls `atlas.take_batch()`. This waits for the frame's preparations and packs them into
   one `Batch`, or returns `None` when nothing is new.
2. Grows its texture array to `atlas.page_count()` pages of `config.format`, keeping the
   existing pages.
3. Creates a storage buffer for each `Binding` the batch uses. Inputs hold
   `batch.bytes(binding)`. Outputs (`Field`, `Stencil`) need `batch.output_size(binding)`
   bytes, and `Stencil` must be zeroed.
4. Dispatches `batch.kernels()` in order, with the listed workgroup counts, then
   `Kernel::Store` with `batch.workgroups` and the atlas pages bound as a write-only storage
   texture array.

Bind the inputs at exactly their size: the kernels take the lengths of the work list and
of the corners from their bindings. They also count on the dispatch shape that
`split_workgroups` gives (rows of 65535 workgroups) instead of reading the dispatch size,
which Godot's Direct3D 12 driver can't provide.

`Kernel::entry_point()` names each kernel's entry point. `Kernel::bindings()` lists what it
binds in group 0, and `Binding::read_only()` gives each binding's access. That is enough
to build pipeline and bind group layouts. The shaders are
[`shader_source()`](src/field.rs) (all kernels but the store),
[`atlas::store_shader_source(format)`](src/atlas.rs), [`text::TEXT_SHADER`](src/text.rs)
(drawing glyph quads) and `glyphux::color::COLOR_SHADER` (color glyphs). Unit tests check
every module with naga, and check that the kernels use only the bindings and access the
contract declares.

The WGSL is the only shader source. A backend for an API that doesn't take WGSL can
translate it with [naga](https://github.com/gfx-rs/wgpu/tree/trunk/naga) (to SPIR-V, GLSL,
HLSL or MSL), so every backend generates the same fields. The `spirv` feature does that for
SPIR-V ([`spirv::kernel`](src/spirv.rs)): one module per kernel, its entry point named
`main`, with only the bindings it uses (listed with it). [`spirv::bindings`](src/spirv.rs)
reads that list back from a module, without naga, so a backend can translate the kernels
when it is built and load them at run time. Godot's addon does: its build script translates
them, its library embeds them and creates its compute pipelines from them. On Vulkan and
Direct3D 12 its fields are identical to wgpu's.

## Drawing

The draw shaders are split the same way: `text::TEXT_CORE` and `glyphux::color::COLOR_CORE`
hold the shading math as plain functions, and the complete wgpu shaders (`TEXT_SHADER`,
`COLOR_SHADER`) add their bindings and entry points. A backend that draws in its own shaders
keeps only its bindings and texture reads, and calls the same functions:

* Text: sample the atlas at the pixel, ask `text_taps` where the glow and the shadow need
  more samples, take them, and get the color from `text_color`.
* Color glyphs: the backend defines the four table reads (`color_tile`, `color_paint`,
  `color_stop`, `color_recipe`; tables of several glyphs can share buffers, with each
  glyph's offsets in `ColorTables`). `color_cell` finds the pixel's tile, the backend samples
  the page there, and `color_shade` gives the color.

The `glsl` feature translates both cores to GLSL with naga ([`glsl`](src/glsl.rs)), which
Godot's shading language takes as it is (Godot's addon writes them into shader include files
when it is built). The `hlsl` feature translates the kernels and both cores to HLSL
([`hlsl`](src/hlsl.rs)): `hlsl::kernel` gives a kernel's source (entry point `main`, no
register assignments, for an engine that binds by name) with its bindings, and
`hlsl::color_library_with` threads a texture parameter through the color core's table reads,
for material systems whose functions cannot reach globals. Unreal's plugin writes them into
its shader directory when it is built. [`color_atlas`](src/color_atlas.rs) allocates
rectangles on color glyph pages, and `split_runs` picks a font per character for font
fallback.

For mirroring the atlas into textures that belong to someone else (an engine's own),
`take_written_rows()` lists the texel rows that batches have written to each page since
the last call.

Text that requests its glyphs only when it changes (retained mode, as the Godot addon's
labels) keeps them with `retain_layer` / `release_layer`: retained pages are never
recycled. `take_upgrades()` lists the glyphs a detail probe raised, to request again.

## Paragraphs

[`paragraph::layout`](src/paragraph.rs) lays out text in styled spans, each with its font
list (font fallback) and size: hard breaks, the bidi algorithm per paragraph, lines broken
at a width at UAX #14 opportunities (clusters for words wider than a line), each line
shaped on its own and put in visual order, then aligned. It returns positioned glyphs, each
with its span's style tag, and the lines' metrics, for any renderer to place.

## Features

* `shaping` (default): OpenType shaping with [HarfRust](https://github.com/harfbuzz/harfrust)
  and the Unicode bidi algorithm (`Shaper`, `ShapingFont`, `text::layout_text`), and
  paragraph layout with [unicode-linebreak](https://crates.io/crates/unicode-linebreak).
* `glsl`: the draw shaders' functions in GLSL (`glsl::text_library`, `glsl::color_library`).
* `spirv`: the compute kernels in SPIR-V (`spirv::kernel`).
