# Generated: do not edit

The addon's build (`rust/build.rs`) writes these files every time it builds the library,
from the WGSL of the glyphux crates it links (translated with naga). Edit the WGSL in
Glyphux instead, and build again.

| File | |
|---|---|
| `glyphux_text.gdshaderinc` | glyphux-gpu's text shading (`text_core.wgsl`) in Godot's shading language. |
| `glyphux_color_types.gdshaderinc`, `glyphux_color.gdshaderinc` | glyphux's color glyph shading (`color_core.wgsl`): its types, then its functions. Between the two, a shader defines how the color tables are read. |

`../text_common.gdshaderinc` and `../color_common.gdshaderinc` show how to include them.
