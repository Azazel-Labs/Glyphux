# Color glyphs

For offline distance atlas and buffer baking, use
`glyphux-atlas color --font font.ttf --text "🔥" -o fire-atlas`.
See the [baked format and loader guide](color-atlas-format.md) for pages, binary tables
and shader bindings. Add `--preview fire.png` only when a decoded preview is wanted.

Glyphux supports the local region-distance representation from the v0.16 color-glyph
prototype. The CPU imports [COLR](https://learn.microsoft.com/en-us/typography/opentype/spec/colr) artwork and compiles a `ColorField`. The GPU evaluates
its paints and recipes while drawing a glyph quad. The existing monochrome MSDF path
continues to use `GlyphAtlas` and `TextRenderer`.

```rust
use glyphux::color::{ColorField, ColorFieldConfig};
use glyphux::io::font::{Font, FontScaling};

let bytes = std::fs::read("Noto-COLRv1.ttf")?;
let font = Font::from_bytes(&bytes, 0, FontScaling::EmNormalized)?;
let glyph = font.glyph_index('\u{1f525}').ok_or("missing fire glyph")?;
let artwork = font.color_glyph_by_index(glyph, 0, [0, 0, 0, 255])?;
let field = ColorField::build(&artwork, ColorFieldConfig::default())?;
let preview = field.render(128)?; // premultiplied linear RGBA
```

For text, use `Shaper::shape_line` and pass each resulting glyph ID to
`ShapingFont::color_glyph`. This preserves the shaper's choice of ligatures, variation
selectors and emoji sequences when the supplied font supports them. Font fallback and
mixed-font run selection remain the application's responsibility.

## GPU drawing

`glyphux_gpu::color::ColorRenderer` uploads compiled fields and draws
`ColorGlyphInstance`s. Call `upload` once, `prepare` with the viewport and instances,
then `render` inside the render pass. `ColorGlyphInstance::new` places the field using
its baseline-relative plane bounds and pixels per em. Add the shaped glyph's x/y
offsets to its origin. `with_rotation` rotates around a screen-space pivot.

Use an sRGB render target for display, or a linear target when another stage applies
the display transfer function. The pipeline outputs premultiplied linear RGBA and uses
premultiplied source-over blending. An instance's opacity multiplies all four components.

Keys are `GlyphKey { font, glyph }`. The font identifier must distinguish palette,
foreground color, face and variation instance. Uploading an existing key replaces it;
`remove` and `clear` release cached resources. CPU compilation and upload are explicit,
so the application can schedule expensive preprocessing off-frame. Color glyphs share
linear RGBA atlas pages, allocated on demand. `ColorAtlasConfig` defaults to 1024 by
1024 texels per page and a maximum of six pages. Use `ColorRenderer::with_config` to
choose a one-page cap or another bounded configuration. Tile origins are relocated
into the shared page; each field's filtering gutters are preserved.

`remove` frees a glyph's rectangle for reuse; `clear` releases all pages. A full atlas
returns an explicit error without evicting glyphs or damaging a previous upload under
the same key. Adjacent free rectangles are merged, but there is no automatic compaction
or eviction. Empty allocated pages are retained until `clear`. `page_count`,
`page_texture` and `glyph_page` expose residency for diagnostics. Paint buffers and bind
groups remain per glyph, with one draw per instance; shared texture pages alone do not
provide monochrome-style batching.

Color glyphs and monochrome glyphs can share a render pass. Submit ordered runs through
their corresponding renderers to preserve overlap order. Monochrome style effects such
as weight, outline, glow and shadow are not implemented for this color pipeline.

## Representation

1. Import solid colors, three-point linear gradients and concentric expanding radial
   gradients, preserving transforms and layer order. Colors become linear-light RGBA.
2. Rasterize layers into visible recipes. Opaque paints replace the underlying recipe;
   translucent paints append to it. Deduplicate recipes and remove hidden ones.
3. Extend labels into the exterior using the nearest interior sample. Keep geometric
   union coverage independent. Build sampled Euclidean distance fields.
4. Select a uniform tile grid that fits at most four local recipes. RGB stores three
   explicit distances. A stores the union distance. The fourth recipe uses residual
   weight. Each boundary patch has a one-texel gutter with its own channel mapping.
5. Empty and interior tiles require no filtered texture fetch. Boundary tiles require
   one bilinear RGBA fetch, followed by normalized region weights and analytic paint
   evaluation. Composite each recipe back-to-front, blend recipes, then apply outer
   coverage once.

The atlas is linear `Rgba8Unorm`, with no mipmaps. Its alpha channel is distance data.
RGB must not be premultiplied by it. These scalar region fields are not MSDFs; taking
their median would decode the wrong representation.

Gradient stops interpolate premultiplied linear color. PAD, REPEAT and REFLECT are
supported, with stable right-hand selection at duplicate stops. Paint opacity and
outer coverage remain separate. The interpolation convention follows the
[OpenType CPAL specification](https://learn.microsoft.com/en-us/typography/opentype/spec/cpal#interpolation-of-colors).

## Supported scope and limits

- [COLRv0](https://learn.microsoft.com/en-us/typography/opentype/spec/colr#overview) solid layers and supported [COLRv1](https://learn.microsoft.com/en-us/typography/opentype/spec/colr#graphic-compositions) paint graphs, through Glyphux's existing
  `ttf-parser` dependency. Outline and paint transforms are distinct. Explicit
  foreground RGBA resolves font foreground-color references.
- Affine transforms in imported artwork. Clip boxes are accepted only when they
  contain every transformed outline control point. Nested outline clips, isolated
  compositing groups, sweep gradients and non-concentric/non-expanding radial
  gradients produce explicit errors. Bitmap-only and SVG color fonts are not supported.
- Uniform screen scaling and rotation. The GPU derives coverage from screen-space
  derivatives. Non-square instance rectangles and sizes below the compiled minimum
  return errors. The default minimum is 16 pixels across the normalized glyph domain,
  not 16 pixels per em. Arbitrary anisotropic and perspective transforms are not supported.
- Four local recipes maximum. The compiler tries all power-of-two grids through
  `max_grid` and returns `ColorError::Overflow` if none fits. It never drops excess
  recipes. The Noto robot's documented overflow is therefore an error at the default
  settings, not a partially rendered success.
- Sampled geometry, scalar SDF corner behavior, tiny regions and center-sampled
  gradients retain the prototype's quality limits. There is no lossless overflow
  fallback, analytic region Boolean geometry, or gradient minification filter.
- The font parser exposes combined paint alpha as RGBA8, so imported stop alpha can
  be quantized compared with the prototype's floating-point COLR alpha. Manually
  constructed paints retain floating-point alpha.
- Paint and recipe buffer reads add work beyond the one filtered texture sample.
  CPU/GPU parity tests establish rendering agreement, not a GPU performance claim.
- GF16/IF15 research-container import/export is not implemented. Applications use the
  typed Rust artwork/field APIs; the prototype packets are used to generate test fixtures.

## Run the example

```text
cargo run --release --bin color_glyph -- crates/glyphux/tests/assets/Noto-COLRv1-Subset.ttf "🔥" target/fire.png 256 gpu
cargo run --release --bin color_glyph -- crates/glyphux/tests/assets/Twemoji-Subset.ttf "🤖" target/robot.png 128 cpu
```

The GPU example composites over a dark background. The CPU example writes a
straight-alpha sRGB PNG. Both shape the input before loading the artwork.

Tests cover analytic paint math, alpha compositing, four-region residual weights,
overflow, empty glyphs, independent EDT distances, real COLRv0/v1 fonts, saved prototype
decoder outputs, shader validation, and CPU/GPU agreement with rotation and opacity.
