# glyphux-godot

The GDExtension library (godot-rust) of the Glyphux Godot addon: `GlyphuxLabel`,
`GlyphuxRichLabel` and `GlyphuxLabel3D`. What they do and how to use them is in the addon's
[README](../README.md).

| Path | |
|---|---|
| `..` (`addons/glyphux/`) | The addon, as Godot projects use it: `glyphux.gdextension`, the libraries (`bin/`, built), the shaders, icons and the user documentation. Projects copy it; `rust/` and `build.py` can be left out. |
| `src/` | The library. |
| `build.rs` | Translates Glyphux's WGSL with naga: the shading library into `../shaders/generated/`, the compute kernels (SPIR-V) into the build's output, which the library embeds. |
| `../build.py` | Builds the library into `../bin/`. |
| `.gdignore` | Keeps Godot from importing this folder. |

## Building

The crate is a member of the Glyphux workspace and builds against its crates. The demo
project uses the addon where it is, so building is all it needs. With
[Rust](https://rustup.rs) and Python 3, from the repository:

```
python examples/godot/addons/glyphux/build.py          # bin/ for this platform
python examples/godot/addons/glyphux/build.py --dev    # unoptimized, faster to compile
python examples/godot/addons/glyphux/build.py --check  # formatting and lints
```

It builds with the workspace's `godot` profile: release, except that panics unwind, so
godot-rust turns a panic in a callback into a Godot error instead of taking the editor down.
`--dev` builds unoptimized (the editor and debug exports only). godot-rust ships prebuilt
bindings for Godot 4.6's API, so building needs no Godot binary.

The generated shaders are committed (they ship with the addon); CI checks that they are
what the build writes.

## How it works

1. A label lays out its text with glyphux-gpu's paragraph layout: spans with their own
   fonts and sizes, font fallback per character, line breaking at a width, bidi order. It
   requests its glyphs from the atlas at the resolution its size on screen calls for, and
   retains the atlas pages they are on, so they are never recycled while shown.
2. Just before Godot draws the frame (`RenderingServer.frame_pre_draw`), the addon generates
   every glyph requested during the frame in one GPU batch. With Forward+ and Mobile, the
   batch runs on Godot's own `RenderingDevice`, in Godot's frame: Glyphux's compute kernels
   (embedded in the library, in SPIR-V) store the fields straight into an RD texture,
   which the labels sample through a `Texture2DArrayRD`.
3. The Compatibility renderer (OpenGL) has no `RenderingDevice`. There the addon runs the
   batch on a GPU device of its own (wgpu: Vulkan, Direct3D 12 or Metal) and copies the
   texel rows that the batch wrote into a `Texture2DArray`. Frames with no new glyphs copy
   nothing. `GLYPHUX_BACKEND=wgpu` in the environment picks this path with every renderer,
   for comparisons; `Glyphux.get_stats().adapter` says which one runs.
4. Color glyphs are compiled on the CPU the first time they are drawn: distance patches on
   shared color pages, and tables of the paints that fill them.
5. Each label draws monochrome glyphs and color glyphs: 2D labels on canvas items of their
   own, 3D labels as surfaces of one mesh. Each glyph's record (its quad's parameters, or
   where its color tables are, and its animation) is a row of a small float texture that
   the material reads. Underlines and strikethroughs are records of the monochrome mesh, a
   piece under each glyph: instead of an atlas field, the shader takes the exact distance
   to their box, so they get the same outline, glow, shadow and animation as their glyphs.

## Shaders

The shaders are written once, in Glyphux's WGSL, and translated with naga by `build.rs`,
so they always match the Glyphux crates the library links, and nothing is translated at run
time. The compute kernels are one SPIR-V module each (the store kernel one per atlas
format); the library reads their bindings back from the modules. The draw shading
(`text_core.wgsl`, `color_core.wgsl`) becomes GLSL that Godot's shading language takes as
it is, in include files that the hand-written shaders wrap with what is particular to
Godot. So the addon draws exactly what Glyphux's wgpu renderers draw.
