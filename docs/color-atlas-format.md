# Offline color atlas format, version 1

```sh
glyphux-atlas color --font Noto-COLRv1.ttf --text "🔥" --text "😀" -o emoji-atlas
glyphux-atlas color --font emoji.ttf --allglyphs --page-size 1024 --max-pages 6 -o emoji-font
```

`glyphux color` accepts the same options. Each repeated `--text` is shaped into
one glyph, including supported sequences. Alternatively repeat `--glyph-index`,
or use `--allglyphs` to select all COLR glyphs. IDs are deduplicated. Unsupported
artwork fails explicitly rather than silently dropping glyphs. Output must be a
new or empty directory. Fonts, shaping and field compilation are needed only when
baking; the files below are sufficient to render the baked glyph IDs later.

## Rust API

Compile artwork with `ColorField::build`, then pack `(glyph_id, field)` pairs with
`BakedColorAtlas::build(&fields, ColorAtlasConfig::default())`. Packing returns a
`ColorAtlasError` for duplicate IDs, oversized patches or exhausted pages.
`atlas.write(path)` writes the bundle.

For direct integration, `atlas.pages()` exposes the distance pixels,
`atlas.glyphs()` exposes typed metrics and `BufferSlice` values sorted by glyph ID,
and `atlas.buffers()` exposes named `tiles`, `paints`, `stops`, `recipes` and
`config` byte vectors. Each slice's `offset` and `length` selects one glyph's
binding. `atlas.manifest()` serializes that same metadata for non-Rust consumers.

For a single field, `field.encode([x, y])` produces the same five named bindings
for a patch at that page origin. It handles tile relocation and little-endian
encoding, including the config uniform. The runtime renderer uses this encoder
too. Shader record structs and padding are implementation details; callers do
not need to construct them. The binary contract below remains version 1.

## Files

| File | Contents |
| :--- | :--- |
| `atlas.json` | Format/version, pages, glyph IDs, placement metrics and buffer slices |
| `atlas-N.rgba` | Raw row-major RGBA8 distance texels, top row first |
| `atlas-N.png` | Lossless copy of those same distance bytes for tools using PNG |
| `tiles.bin` | Tile origins, modes and recipe IDs |
| `paints.bin` | Solid colors and gradient geometry |
| `stops.bin` | Gradient colors and stop positions |
| `recipes.bin` | Recipe headers and ordered paint IDs |
| `config.bin` | Per-glyph decoder parameters |
| `color.wgsl` | The same decoder used by the runtime renderer |

These PNGs contain distance data, not finished emoji artwork. Upload either the
raw bytes or decoded PNG bytes as **RGBA8Unorm**, never sRGB. Do not premultiply
alpha, convert colors, or generate mipmaps. Alpha stores silhouette distance.
Use a clamp-to-edge linear sampler. Existing tile gutters are preserved.

## Load and bind

1. Require `format = "glyphux-color-atlas"` and `version = 1` in `atlas.json`.
2. Load each page into a 2D RGBA8Unorm texture at its recorded width/height.
   Raw rows are tightly packed at `width * 4` bytes; pad staging rows if your
   graphics API requires a larger copy alignment.
3. Load the `.bin` files into storage buffers (and `config.bin` into a uniform
   buffer). Integers and IEEE float32 values are little-endian. Each glyph's
   `buffers` entry gives a byte `offset` and `length` for each binding.
4. Bind **that glyph's slices**, not the whole files, to group 1 below. Offsets
   are aligned to 256 bytes; if a device requires greater alignment, copy each
   slice to its own buffer or repack with the required alignment. IDs and offsets
   inside each slice are local to that glyph. Select its `page` texture.
5. Draw a four-vertex triangle strip with the exported shader, following the
   vertex layout and blend state below. No font or recipe reconstruction is needed.

| Group 1 binding | Resource | Record size |
| :--- | :--- | ---: |
| 0 | Glyph slice of `config.bin` | 16 bytes |
| 1 | Glyph's atlas page texture | RGBA8Unorm |
| 2 | Linear filtering sampler | |
| 3 | Glyph slice of `tiles.bin` | 32 bytes |
| 4 | Glyph slice of `paints.bin` | 80 bytes |
| 5 | Glyph slice of `stops.bin` | 32 bytes |
| 6 | Glyph slice of `recipes.bin` | 4-byte words |

Group 0 binding 0 is a 16-byte uniform `[viewport_width, viewport_height, 0, 0]`.
Per-instance vertex attributes are `Float32x4` rect at location 0,
`Float32x4` transform at location 1, and `Float32` opacity at location 2.
The runtime's padded instance stride is 48 bytes: offsets 0, 16, 32, then 12 bytes
of padding. Rect is `[left, top, right, bottom]` in screen pixels. Transform is
`[cos_angle, sin_angle, pivot_x, pivot_y]`; `[1, 0, 0, 0]` is unrotated.
Use source factor `One`, destination factor `OneMinusSrcAlpha`, and operation
`Add` for both color and alpha. The fragment output is premultiplied linear RGBA;
an sRGB render target performs display encoding.

## Buffer layouts

- **Config:** four float32 values: base resolution, tile grid dimension, distance
  half-band, minimum valid footprint in pixels.
- **Tile:** eight u32 values: atlas origin x/y, mode, zero padding, four recipe
  IDs. Modes are 0 (empty), 1 (interior), 2 (boundary). `0xffffffff` means no
  recipe. Origins already include placement within the shared page.
- **Paint:** five 16-byte rows. Header is u32 `[kind, extend, first_stop, stop_count]`;
  the remaining rows are float32 color, inverse-transform row 0, inverse-transform
  row 1, and geometry. Kind 0 is solid, 1 linear, 2 radial. Extend 0 is pad, 1 repeat,
  2 reflect. Inverse rows contain `[xx, xy, tx, 0]` and `[yx, yy, ty, 0]`.
  Linear geometry is `[origin_x, origin_y, direction_x, direction_y]`; radial is
  `[center_x, center_y, radius_0, radius_1]`. Solid colors are premultiplied linear RGBA.
- **Stop:** float32 premultiplied linear RGBA, then `[position, 0, 0, 0]`.
- **Recipe:** the first `recipeCount * 2` u32 words are `[first_word, paint_count]`
  headers. `first_word` indexes the same glyph's recipe slice in u32 words, not
  bytes. The remaining words are local paint IDs, in back-to-front composite order.
  Unused paint/stop/recipe tables have a dummy record so GPU bindings are nonempty.

`planeBounds` is `[left, bottom, right, top]` in ems, with y up. For baseline
`(x, y)` in screen coordinates and pixels-per-em `s`, form the rectangle
`[x + left*s, y - top*s, x + right*s, y - bottom*s]`. `advance` is also in ems.
Shaping offsets, if any, should be applied to the baseline before placement.
Preserve the square domain and minimum footprint recorded by the field.

The manifest maps glyph IDs, not Unicode text. An application can shape text
separately or store its chosen glyph IDs. One bundle represents one font face,
variation instance, palette and foreground selection; keep different bundles
under different font/cache keys.

## Optional preview

```sh
glyphux color --font emoji.ttf --text "🔥" -o fire-atlas --preview fire.png --size 256
```

`--preview` evaluates one selected glyph to a display PNG. `--size` and
`--background` apply only to that preview and require `--preview`. They never
modify the baked distance atlas or paint buffers.
