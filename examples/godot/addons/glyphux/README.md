# Glyphux for Godot

A Godot 4.6 addon that draws text with [Glyphux](https://github.com/Azazel-Labs/Glyphux):
text shaped with HarfRust and drawn from multi-channel distance fields, which the GPU
generates the first time each glyph is drawn, at a resolution chosen for the size it
appears on screen. Text stays sharp when zoomed, follows curves, uses variable font
weights, falls back to other fonts, draws color emoji (COLR fonts), wraps, and can have
outline, glow and drop shadow effects. It works with all three renderers: Forward+, Mobile
and Compatibility.

| Node | |
|---|---|
| `GlyphuxLabel` (`Node2D`) | Plain text in 2D, optionally wrapped or along a `Curve2D`. |
| `GlyphuxRichLabel` (`Control`) | BBCode in user interfaces: styles, sizes, fonts, effects, animation, links. |
| `GlyphuxLabel3D` (`GeometryInstance3D`) | Plain text or BBCode in a 3D scene, like `Label3D`. |

## Install

1. Build the library into `bin/`. This needs a clone of the
   [Glyphux repository](https://github.com/Azazel-Labs/Glyphux) (the addon is its demo
   project's `addons/glyphux`), [Rust](https://rustup.rs) and Python 3:

   ```
   python examples/godot/addons/glyphux/build.py
   ```

   It builds the library for the platform it runs on. `--dev` compiles faster but makes an
   unoptimized library, which only the editor and debug exports load.
2. Copy this folder into your project as `addons/glyphux` (the `.gdextension` file expects
   that path). `rust/` and `build.py` only build it and can be left out; Godot ignores them
   either way.
3. Restart the editor if it was open. The nodes appear in the Create Node dialog.

Commit the copy with its `bin/` so that your team needs no Rust. After updating Glyphux,
build and copy again.

## GlyphuxLabel

```gdscript
var label := GlyphuxLabel.new()
label.font = preload("res://fonts/MyFont.ttf")   # a FontFile, or a FontVariation of one
label.font_size = 32
label.text = "Hello, world"
label.glow_radius = 8
add_child(label)
```

| Property | |
|---|---|
| `text` | Lines separated by `\n`. The first line's baseline starts at the node's origin. |
| `font` | A `FontFile` (any imported .ttf or .otf) or a `FontVariation`. A variation uses its OpenType axes and face index. The font's `fallbacks` draw the characters it lacks, for example a color emoji font. Embolden and transform settings are ignored. |
| `font_size` | Pixels per em, in the node's units. |
| `color` | Fill color. Color glyphs keep their own colors and take its opacity. |
| `font_weight` | The weight in `wght` units (400 regular, 700 bold), 0 for the font's own. A variable font is drawn from instances 100 units apart, with the rest added as extra weight, so the weight can animate smoothly. Other fonts keep their weight and add the difference. |
| `weight` | Units added to the outline (negative values make it thinner). |
| `align`, `width`, `autowrap`, `line_spacing` | Start (right for right-to-left lines), End or Center, within `width` (without a width: from, around or up to the origin). `autowrap` wraps lines at `width`, between words (UAX #14). `line_spacing` multiplies the fonts' line height. |
| `curve`, `curve_offset`, `curve_loop` | A `Curve2D` in the node's units (a `Path2D`'s curve, for example) that the baseline follows, each glyph turned along it. The text starts `curve_offset` along the curve, aligned within its length. `curve_loop` wraps it around the end. |
| `outline_color`, `outline_width` | An outline outside the fill. |
| `glow_color`, `glow_radius` | A glow that fades out over the radius. |
| `shadow_color`, `shadow_offset`, `shadow_blur` | A drop shadow. A transparent color turns it off. |

`get_text_size()` returns the text's size. `update_now()` updates at once instead of at the
end of the frame.

## GlyphuxRichLabel

A `Control` that draws BBCode, wrapped to its width. It understands the tags of Godot's
`RichTextLabel` and two more:

| Tag | |
|---|---|
| `[b]` `[i]` `[u]` `[s]` `[code]` | Bold, italics, underline, strikethrough, the monospace font. |
| `[color=c]` `[font_size=n]` | Color (a name or `#hex`), size. |
| `[font=res://...]` `[font name=res://... size=n]` | A font resource: a `FontFile` (an imported .ttf or .otf) or a `FontVariation` (`.tres`), with its axes and fallbacks; optionally a size. A path that doesn't load warns and keeps the label's font. |
| `[outline_size=n]` `[outline_color=c]` | An outline. |
| `[glow color=c radius=n]` `[shadow color=c offset=x,y blur=n]` | A glow, a drop shadow. |
| `[url]` `[url=meta]` | A link: `meta_clicked(meta)` (the link's text without a meta), `meta_hover_started`, `meta_hover_ended`. |
| `[wave amp=50 freq=5]` `[shake rate=20 level=5]` `[rainbow freq=1 sat=0.8 val=0.8 speed=1]` | Animation, in the shader: it costs no CPU per frame. |
| `[left]` `[center]` `[right]` `[fill]` | The alignment of the paragraphs inside. |
| `[lb]` `[rb]` | `[` and `]`. |

Tags combine: nested tags add up, and the inner one wins where two set the same thing.
Underlines and strikethroughs take their text's color, outline, glow, shadow and animation
(strikethroughs are drawn over the text, underlines under it).

Properties: `text`, `font` (with its fallbacks), `bold_font`, `italics_font`,
`bold_italics_font` and `mono_font` (without them, bold uses weight 700 and italics a
slant), `font_size`, `color`, `align`, `autowrap`, `fit_content` (the control's minimum
height becomes the text's), `line_spacing` and `meta_underlined`.

## GlyphuxLabel3D

Text in a 3D scene, a `GeometryInstance3D` like `Label3D`: its `GeometryInstance3D`
settings (visibility ranges, layers, transparency...) apply. The text is laid out in pixels
(`font_size` per em) and drawn `pixel_size` world units per pixel, unshaded and double
sided. It has `GlyphuxLabel`'s properties except the curve, plus:

| Property | |
|---|---|
| `bbcode_enabled` | Reads `text` as BBCode (links are not clickable in 3D). |
| `pixel_size` | World units per pixel. |
| `billboard` | Faces the camera. |
| `no_depth_test` | Shows through closer geometry. |
| `align`, `vertical_align` | Where the text box sits against the node's origin (centered by default). |

The fields' resolution follows the camera: the label measures how many screen pixels a
pixel of its text covers (its `pixel_size` and scale, projected at its origin), and gets
higher resolution fields as it comes closer, smaller ones as it moves away. A label spanning
a large depth range uses the resolution its origin needs.

## Updates

Labels do nothing until something they show changes. Then they are updated once, at the
end of the frame. What changes them:

* their properties, and the `changed` signals of their fonts and curve;
* their scale (2D: the node's, its canvas' camera or `CanvasLayer`, the window's stretch;
  3D: the distance to the camera), but only when that changes the resolution of their
  fields. The server checks each canvas, and each 3D label, once per frame;
* a glyph they show getting more resolution from its detail probe;
* becoming visible.

Moving or turning a label costs nothing; animated BBCode runs in the shader.
`Glyphux.get_stats().updates` counts the updates.

## Shaders

The labels' shaders are in `shaders/`, and load from `res://addons/glyphux/shaders/`: when
exporting only selected resources, add `addons/glyphux/shaders/*` to the export's resource
filter.

| Path | |
|---|---|
| `shaders/generated/` | Glyphux's shading functions, translated from its WGSL when the library is built (see the README there). Do not edit them. Your own shaders can include them. |
| `shaders/text_common.gdshaderinc`, `color_common.gdshaderinc` | What is particular to Godot (uniforms, how glyph records and textures are read, animation), around the generated functions. |
| `shaders/text.gdshader`, `color.gdshader` | The canvas shaders (`GlyphuxLabel`, `GlyphuxRichLabel`). |
| `shaders/*_3d.gdshader`, `*_3d_no_depth_test.gdshader` | The spatial shaders (`GlyphuxLabel3D`), which share `*_3d.gdshaderinc`. |

## Limitations

* With the Compatibility renderer, Glyphux runs on a GPU device of its own and copies the
  rows it generates into Godot's texture, which pauses the main thread on frames that
  generate new glyphs. Forward+ and Mobile have neither.
* A color glyph takes some milliseconds to compile, on the main thread, the first time it
  is drawn. Color glyphs whose shapes the format can't hold (more than four paint regions
  meeting) come from the next fallback font.
* BBCode has no images, tables or lists, and `[fill]` aligns to the start (no
  justification).
* Only Godot's default single-threaded rendering mode has been tested.

## More

The library's sources, how it works and how to build it: [`rust/`](rust/README.md). The
demo project around this folder is
[`examples/godot`](https://github.com/Azazel-Labs/Glyphux/tree/main/examples/godot).
