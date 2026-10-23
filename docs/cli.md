# Offline command line tools

Published GitHub Releases build ZIP archives with `glyphux` and `glyphux-atlas`
for Windows, Linux and macOS. Archive names include the OS and CPU architecture.
Download your archive from [GitHub Releases](https://github.com/Azazel-Labs/Glyphux/releases),
extract it, and run the tools in a terminal (`.\glyphux.exe` in PowerShell).
On macOS/Linux, use `chmod +x glyphux glyphux-atlas` if your ZIP extractor did not
preserve executable permissions. Fonts are supplied by you as local files.
The tools run offline without a browser, server or GPU.

Alternatively install with `cargo install glyphux-cli`, or build from a checkout:

```sh
cargo build --release -p glyphux-cli
```

Executables are in `target/release`. Building may need network access for
dependencies; rendering only reads local files.

## Color emoji atlas baking

```sh
glyphux-atlas color --font Noto-COLRv1.ttf --text "🔥" --text "😀" -o emoji-atlas
glyphux-atlas color --font emoji.ttf --allglyphs -o emoji-font
glyphux color --font emoji.ttf --glyph-index 42 --palette 0 -o glyph-atlas
glyphux color --font emoji.ttf --text "🔥" -o fire-atlas --preview fire.png --size 256
glyphux-atlas color --help
```

Both commands use the same CPU-only baker. Output is a directory containing linear
RGBA8 distance pages (raw and PNG), tile/recipe/paint/gradient-stop/config buffers,
JSON metadata and the matching WGSL decoder. The default output is `color-atlas`.
It must be new or empty. See the [format and loader guide](color-atlas-format.md)
for the complete binding contract. No fonts or field compilation are needed to render
these baked buffers later.

| Purpose | Options | Default |
| :--- | :--- | :--- |
| Font | `--font FILE`, `--font-index N` | Face 0 |
| Selection | Repeat `--text EMOJI` or `--glyph-index ID`, or use `--allglyphs` | Required |
| Colors | `--palette N`, `--foreground RRGGBBAA` | Palette 0, opaque black foreground |
| Variations | Repeat `--fontvar AXIS=VALUE` | Font defaults |
| Destination | `-o DIR` | `color-atlas` |
| Pages | `--page-size N`, `--max-pages N` | 1024 square, at most 6 pages |
| Preparation | `--resolution N` | 1024 (16 to 2048) |
| Field resolution | `--base-resolution N` | 128 (power of two, 8 to 512) |
| Distance half-band | `--range R` | 0.125 |
| Minimum footprint | `--min-pixels N` | 16 |
| Tile grid limit | `--max-grid N` | 16 (power of two, <= base resolution) |
| Optional preview | `--preview FILE`, `--size N`, `--background RRGGBBAA` | Off; preview defaults to 256 pixels, transparent |
| Diagnostics | `--stats` | Off |

Each `--text` value must shape to one supported glyph; sequences are allowed.
Duplicate glyph IDs are baked once. `--allglyphs` selects COLR artwork only.
Unsupported paints and recipe overflow produce errors instead of missing artwork.
Bitmap-only and SVG color fonts are unsupported. If a glyph exceeds a page or
page count, increase the corresponding limit. The baker preserves tile gutters.

Foreground colors resolve COLR foreground references. Six hex digits mean opaque
RGB; eight include alpha. The range must be at least `1 / min-pixels`. Preview
size must be at least min-pixels. `--size` and `--background` require `--preview`
and affect only the display preview, never the baked distance data.

## Monochrome shapes and atlases

```sh
glyphux msdf --font MyFont.ttf A -o a.png --size 32 32 --pxrange 4 --autoframe
glyphux-atlas --font MyFont.ttf --type msdf --size 32 --imageout atlas.png --json atlas.json
glyphux --help
glyphux-atlas --help
```

Existing `sdf`, `psdf`, `msdf`, `mtsdf` and `metrics` modes and msdfgen-style
single-dash options remain available. Color mode has its own options, shown by
`glyphux color --help`.
