# Glyphux for Unreal

An Unreal Engine 5.8 plugin that draws text with [Glyphux](https://github.com/Azazel-Labs/Glyphux):
text shaped with HarfRust and drawn from multi-channel distance fields, which the GPU
generates (in Unreal's render graph) the first time each glyph is drawn, at a resolution
chosen for the size it appears on screen. Text stays sharp up close, uses variable font
weights, falls back to other fonts, draws color emoji (COLR fonts), wraps, and can have
outline, glow and drop shadow effects, with BBCode for all of it.

| Class | |
|---|---|
| Glyphux Text Block (`UGlyphuxTextBlock`, UMG widget) | Text or BBCode in user interfaces, with clickable links. |
| Glyphux Text (`UGlyphuxTextComponent`, scene component) | Text or BBCode in the world, optionally a billboard or through walls. |
| Surface material | A text on any mesh, flat or curved, skinned or fractured, by its texture coordinates. |
| Glyphux Niagara Library | A text's glyphs as Niagara particles: sprites, or mesh fragments that shatter. |

The plugin is mostly Rust: the classes' properties, layout, BBCode and the GPU work are in
`rust/`, and the C++ that makes them Unreal classes is generated from it by
[Ferreal](https://github.com/Azazel-Labs/Ferreal) when the module builds. The Rust compiles to a
static library linked into the module, so it works in the editor, in packaged games and in
monolithic builds alike.

## Install

1. Install [rustup](https://rustup.rs), once. The first build installs the rest: the Rust
   toolchain the crate pins (`rust/rust-toolchain.toml`), the one target the build needs,
   and the crates, Ferreal among them, from
   [its repository](https://github.com/Azazel-Labs/Ferreal) (the version in
   `rust/Cargo.lock`; `cargo update -p ferreal` in `rust/` moves to the newest).

2. Copy `Plugins/Glyphux` into your project's `Plugins` folder, or open the demo project
   (`examples/unreal/GlyphuxDemo.uproject`), which has it. If you copy it elsewhere, fix the
   path of the glyphux-gpu dependency in `rust/Cargo.toml`.
3. Build the project (from your IDE, or by opening the .uproject and letting Unreal build
   it). `Source/Glyphux/Glyphux.Build.cs` runs `cargo build`, which also writes the
   generated C++ and shaders; the first build compiles the Rust dependencies and takes a
   few minutes.
4. In the editor, enable the **Python Editor Script Plugin** and **Editor Scripting
   Utilities**, then run
   `Content/Python/glyphux_materials.py` (Tools > Execute Python Script) to make the
   materials, and for shattering text, `Content/Python/glyphux_shards.py`. MATERIALS.md
   says what they make, to build the materials by hand instead.

To call engine classes from the plugin's Rust with `ferreal::engine!` (typed bindings
generated from Unreal's reflection data), also copy the `unreal/Ferreal` plugin of [Ferreal's repository](https://github.com/Azazel-Labs/Ferreal) into the
project's `Plugins`; the plugin does not need it as it is.

Set `FERREAL_SKIP_CARGO=1` in the environment to link the library already built (in
`rust/target/unreal`) without running cargo: for build machines without Rust, with the
library and the generated files committed.

The Rust targets set up are Windows (x86_64 MSVC), Linux (x86_64 and arm64) and macOS
(arm64); other platforms need their target added to `BuildRust` in `Glyphux.Build.cs` (a
console's Rust target comes from its platform holder's SDK).

## Glyphux Text Block

A UMG widget: add it to a widget blueprint like a Text Block.

| Property | |
|---|---|
| Text, Rich | The text; with Rich, BBCode (below). |
| Font, Fallback Fonts | A Font Face asset (import a .ttf or .otf as a Font Face); none: the engine's Roboto. The fallbacks draw the characters it lacks, a color emoji font for example. |
| Bold, Italic, Bold Italic, Mono Font | The faces of `[b]`, `[i]`, both and `[code]`; without one, the font at weight 700, slanted, or as it is. |
| Font Size, Color | Pixels per em; the fill color (color glyphs keep theirs and take its opacity). |
| Font Weight, Weight | The weight in `wght` units (0: the font's own; variable fonts animate smoothly); units added to the outline. |
| Align, Autowrap, Line Spacing | Alignment within the widget; wrapping at its width; a multiple of the line height. |
| Outline, Glow, Shadow | Colors and sizes of the effects (a transparent shadow color turns it off). |
| Underline Links | Underlines `[url]` links. |
| On Link Clicked | Called with a link's meta (or its text) when clicked. |

## Glyphux Text

A scene component (Add Component > Glyphux Text). The same properties, without links, plus:

| Property | |
|---|---|
| Pixel Size | World units per layout pixel. |
| Width, Align, Vertical Align | The box's width (0: the widest line's) and how it sits on the origin. |
| Billboard | Turns to face the camera while playing. |
| No Depth Test | Draws through closer geometry. |
| Draw Text | Off: lays out without drawing, as the source of a surface or particles. |
| Fixed Scale | Screen pixels per layout pixel the fields are made for; 0 measures it from the nearest view. |
| On Text Updated | Broadcast after each new layout. |

The text lies in the component's YZ plane, facing +X. Blueprint functions: **Get Glyphs**,
**Get Glyph Material**, **Get Surface Material**, **Bind To Material** (MATERIALS.md and
NIAGARA.md say what for), **Refresh** (lay out again after changing fonts from Blueprints).

## BBCode

| Tag | |
|---|---|
| `[b]` `[i]` `[u]` `[s]` `[code]` | Bold, italics, underline, strikethrough, the monospace font. |
| `[color=c]` `[font_size=n]` | Color (a name or `#hex`, sRGB), size. |
| `[font=/Game/Fonts/MyFace.MyFace]` | A Font Face asset by path. |
| `[outline_size=n]` `[outline_color=c]` | An outline. |
| `[glow color=c radius=n]` `[shadow color=c offset=x,y blur=n]` | A glow, a drop shadow. |
| `[url]` `[url=meta]` | A link (clickable in widgets). |
| `[wave amp=50 freq=5]` `[shake rate=20 level=5]` `[rainbow freq=1 sat=0.8 val=0.8 speed=1]` | Animation (wave and shake move the vertices, rainbow is in the shader). |
| `[left]` `[center]` `[right]` `[fill]` | The alignment of the paragraphs inside. |
| `[lb]` `[rb]` | `[` and `]`. |

## Where things are

| Path | |
|---|---|
| `rust/` | The crate: the classes (`block.rs`, `component.rs`, `label.rs`), layout, BBCode, the atlas and font server (`server.rs`), the quads (`draw.rs`). `build.rs` writes the generated files. |
| `Source/Glyphux/` | The C++ parents of the Rust classes (drawing with Slate and a procedural mesh), the subsystem that owns the atlas, the draw data textures. `Public/Ferreal`, `Private/Ferreal` and `Private/Generated` are generated. |
| `Source/GlyphuxShaders/` | The field generation kernels as global shaders in the render graph (loaded early, for the shader directory). `Generated/` is generated. |
| `Source/GlyphuxNiagara/` | The Niagara function library and shard sets. |
| `Shaders/Private/` | `GlyphuxMaterial.ush`, the materials' functions; `Generated/` holds the kernels and shading code, translated by naga from glyphux's WGSL. |
| `Content/Python/` | The scripts that make the materials and shard meshes. |

`Project Settings > Plugins > Glyphux` names the materials. `UGlyphuxSubsystem::GetGlyphuxStats()`
reports the atlas use.

## Status

This plugin is new and has not yet been built against a released Unreal Engine 5.8: the
Rust, the generated C++ (with mock Unreal headers), the translated shaders (with glslang and
spirv-val) and the shard math are checked, but expect small fixes to the C++ against the
engine headers on the first build. Please report them.
