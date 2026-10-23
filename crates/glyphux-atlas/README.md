# glyphux-atlas

Font atlas generation on top of [glyphux](../glyphux): charsets, glyph boxes, MaxRects / skyline
packing, parallel SDF / PSDF / MSDF / MTSDF / mask generation and JSON / CSV layouts in
[msdf-atlas-gen](https://github.com/Chlumsky/msdf-atlas-gen)'s format. See the workspace
[README](../../README.md) for details and the `glyphux-atlas` command line tool in `glyphux-cli`.


Color emoji baking lives in `glyphux_atlas::color`. Compile `ColorField` values,
pack them with `BakedColorAtlas::build(&fields, ColorAtlasConfig::default())`, and
write a portable bundle with `atlas.write(path)`. The atlas also provides
read-only pages, named buffers and typed glyph metadata for direct integration.
See the [color atlas format](../../docs/color-atlas-format.md) for the Rust API,
offline CLI and shader bindings.
