# glyphux-wgpu

GPU generation of SDF / PSDF / MSDF / MTSDF glyph fields with [wgpu](https://wgpu.rs) 30
(WGSL compute), for apps that want distance fields generated at run time instead of baking
atlases: a glyph cache that picks the field resolution from the size text is drawn at, and an
MSDF text renderer on top of it. Native (Vulkan, Metal, DX12, GL) and the web ([WebGPU](https://www.w3.org/TR/webgpu/)).

This is the wgpu backend of [`glyphux-gpu`](../glyphux-gpu), which holds everything
independent of the graphics API (the atlas' cache and allocation, the CPU preparation, the
kernels' batches and WGSL, shaping and layout) and is re-exported here. This crate adds the
atlas texture, the compute passes, the renderers and the profiler. The
[Godot addon](../glyphux-godot) is another frontend on the same core.

The fields are the ones [`glyphux`](../glyphux) makes on the CPU, not an approximation of them:
the same edge selection, the same overlapping contour combiner and the same error
correction. Across four fonts, two sizes and all four field types, the GPU output matches the
CPU's to about 3·10⁻⁶ (f32 vs f64). The exception is where two edges are *exactly* as close
(symmetric geometry) and f32 breaks the tie the other way: at most a few dozen of ~600 000
values, see [Validation](#validation).

## How it works

The workspace [README](../../README.md#how-it-works) walks through the pipeline with figures
drawn from real output, including [the buffers of a batch](../../README.md#inside-a-gpu-batch)
and the steps that read and write them:

![The pipeline](../../docs/figures/pipeline.svg)

| Where | Step |
|---|---|
| CPU, once per glyph | Read the outline, assign its edges to color channels, and split the output into 8×8 tiles. For each tile, list the edges that could be closest to any of its pixels. Upload those lists and the outline data. |
| GPU distance calculation | For each texture pixel, find the nearest edge for each channel. Account for corners and overlapping outlines when calculating the stored distances. |
| GPU shortcut for distant tiles | Tiles far from the outline only need an inside/outside check. Fill them without calculating every pixel's distance. |
| GPU error correction | Protect pixels needed to preserve corners and edges, then find and fix combinations of channel values that would draw incorrectly. |
| GPU atlas store | Copy the distances into the glyph's rectangle on a texture page, limiting values to the range the texture can store. |
| Render | Draw a rectangle for each glyph. Read its three distances, take the middle value, and use it to smooth the edge at the displayed size. |

* **Exact tile culling.** For each tile the CPU computes, per channel, an upper bound of the
  nearest distance over the tile (from edge end and mid points) and keeps only edges whose
  lower bound (bounding box, corner perpendicular cones) is below it, sorted by that lower
  bound. The shader starts from those bounds as thresholds and stops scanning as soon as the
  next candidate's bound exceeds the current worst channel. This is the CPU evaluator's own
  culling, so nothing that could be nearest is ever skipped; bounds are widened by 10⁻⁴ for f32.
* **Real kernels.** Lines, quadratics (safeguarded Newton on monotone pieces of the cubic
  derivative) and cubics (glyphux's interval subdivision of the quintic with precomputed
  Bernstein data per interval). Nothing is flattened, so corners and curvature are exact at
  any magnification.
* **Batched.** Every glyph of a frame goes into one set of buffers and one dispatch per pass
  (work list of `(glyph, tile)` pairs, 2D dispatch beyond 65535 workgroups).
* **WebGPU limits.** At most 8 storage buffers per pass, no `f64`, no `inf` literals,
  workgroup size 64; the field passes and the error correction passes bind different subsets
  of one shader module.

Compared to the projects reviewed for the design:

* Like [msdfgl](https://github.com/nyyManni/msdfgl), the work is per texel on the GPU with
  the shape in buffers, but with real nearest-edge selection. msdfgl scans every segment, has
  no error correction or overlap handling, and works at one fixed size.
* Like the [quadratic stroke SDF](https://github.com/tuzhong007/quadratic-stroke-sdf) paper
  (and Vello), work is binned into tiles, so each tile sees a short candidate list and far
  tiles only get a sign.
* Unlike both, the result matches the CPU generator, error correction included.

## Color fonts and shared atlas pages

Color fonts describe an emoji as shapes filled with solid colors or gradients. We read
the supported [COLR](https://learn.microsoft.com/en-us/typography/opentype/spec/colr) shapes on the CPU, work out which paints remain visible, and build
distance fields for their boundaries. `ColorField` holds that result;
`ColorRenderer` uploads it and draws it. The monochrome pipeline above generates
fields on the GPU; this color pipeline currently builds them on the CPU.

![Compile a color glyph into local distance patches and paint recipes](../../docs/figures/color-prepare.svg)

The small packed image in the figure contains one glyph's distance data before upload.
We reserve a rectangle for it on a shared GPU page, copy it there, and add the rectangle's
position to every tile's stored texture position. Each patch includes a one-pixel border
(a gutter), so filtering its edge still reads values with the same channel meanings.

Here is what we put in the GPU's texture and tables:

| Storage | What it contains | How we use it |
|---|---|---|
| Atlas texture | Four 8-bit distance values per texel, read as numbers from 0 to 1 | R, G and B describe three local regions. Alpha describes the outer silhouette. |
| `tiles` buffer | Each tile's texture position, tile type, and four 32-bit recipe indices in `ids.xyzw` | Find the distance patch and the recipes assigned to its channels. |
| `recipes` buffer | A starting position and count for each recipe, followed by lists of 32-bit paint indices | Read the paints needed for one region, in back-to-front order. |
| `paints` buffer | One record per paint: its type, color or gradient geometry, coordinate transform, and stop-list location | Calculate that paint's color at the pixel being drawn. |
| `stops` buffer | Gradient positions and their RGBA colors | Find the two stops around the pixel's gradient position and blend between them. |

A **recipe** is just an ordered list of paints. An opaque paint hides earlier paints;
a translucent paint keeps some of the color underneath. One tile can refer to four
recipes even though RGB holds only three distances: the fourth recipe gets any share
left after the first three. We then scale those shares so their sum is one.

For a radial gradient, we put its center and inner and outer radii in a paint record's
`geometry` values. `header.x = 2` marks it as radial; `header.z` and `header.w` give
the first stop and the number of stops in the `stops` table. The paint also stores the
transform that converts the pixel position into the gradient's coordinate system.

For example, suppose a tile's `ids.x` is 7. That means its R channel belongs to recipe
7. We read that recipe's list of paint indices. If one is 12, we read `paints[12]`.
If it is radial, we measure the pixel's distance from its center, map that distance
between the stored radii, and blend the corresponding gradient stops. These table
references are **32-bit indices**. The texture's R channel is an **8-bit distance**;
it does not hold the gradient number.

The exact layouts and reads are in the shared [`color_core.wgsl`](../glyphux/src/color/color_core.wgsl)
(the shading, for every backend) and [`color.wgsl`](../glyphux/src/color/color.wgsl) (wgpu's
bindings).
For offline baking and direct buffer bindings, see the
[color atlas format](../../docs/color-atlas-format.md).

![GPU readback of fire, grinning face and robot sharing one color atlas page](../../docs/figures/color-atlas.svg)

We keep separate pages for monochrome and color glyphs. Both grow on demand, with separate limits:

| | Monochrome `GlyphAtlas` | Color `ColorRenderer` |
|---|---|---|
| Default limit | Six 1024 × 1024 pages | Six 1024 × 1024 pages |
| Configuration | `AtlasConfig::size`, `layers` | `ColorAtlasConfig::page_size`, `max_pages` |
| Allocation | `flush` grows a texture array and copies existing pages | `upload` creates another fixed-size page only when existing pages cannot fit |
| At the cap | Recycle the least recently used page unused this frame; otherwise placement may fail | Return an error; keep existing glyphs, including the old version if replacing one |
| Freeing space | Reuse a whole page | `remove` frees a rectangle and merges adjacent free space; `clear` releases all pages |

The color atlas does not move existing rectangles around or automatically discard
glyphs to make room. A glyph's distance image must fit in one free rectangle on one
page; several small gaps cannot hold a larger image. Empty color pages stay allocated
until `clear`. Use `page_count()`, `page_texture()` and `glyph_page(key)` to inspect
how many pages exist and where a glyph was placed. The demo's atlas toggle displays occupied
color pages once each, alongside the live monochrome pages.

```rust
use glyphux_wgpu::color::{ColorAtlasConfig, ColorRenderer};

let mut colors = ColorRenderer::with_config(&device, surface_format, ColorAtlasConfig {
    page_size: 1024,
    max_pages: 6, // default upper bound; pages are allocated only as needed
})?;
colors.upload(&device, &queue, key, &compiled_field)?;
// Each frame: prepare placements, then render ordered runs in your render pass.
```

![Decode local distances, evaluate paints and apply coverage](../../docs/figures/color-render.svg)

For each screen pixel, the shader uses its position within the glyph to find the tile.
An empty tile draws nothing. A tile entirely inside one region reads that region's
paint list directly. At a boundary, the shader samples RGBA from the page once, with
bilinear filtering: the GPU blends the four neighboring texels. The three region
distances tell us how much of each recipe to use at this pixel.

We calculate the paint colors and combine each recipe's layers from back to front.
Then we mix the recipes using their shares and multiply the result by outer-edge
coverage: how much of the pixel lies inside the emoji. Color values are kept multiplied
by opacity throughout this calculation (premultiplied alpha). We draw that result over
the background, letting the background show through the transparent portion.
The one filtered texture lookup is only part of the work; the shader also reads the
tile, recipe, paint and stop tables.

Color and monochrome glyphs use the same position, scale and rotation from the scene.
When they overlap, draw them in the intended order using `TextRenderer::render_range`
and `ColorRenderer::render_range`. Each color glyph still has its own paint buffers
and bind group, the set of resources passed to the shader. It takes one draw call per
color instance even when several share a texture page. The monochrome glow, weight
and outline effects do not apply to this color renderer. See
[color font support and limits](../../docs/color-glyphs.md).

The color figures are generated from real fields and GPU readback by
[`examples/figures_color.rs`](../../examples/figures_color.rs). Regenerate them with
`cargo run --release --bin figures -- docs/figures --color` (GPU required).

## Usage

### Glyph atlas + text renderer

```rust
use std::sync::Arc;
use glyphux_wgpu::{AtlasConfig, GlyphAtlas, Shaper, ShapingFont, TextRenderer, TextStyle, Units};
use glyphux_wgpu::text::{layout_text, Align};
use glyphux_wgpu::units::{em, px};

let font = ShapingFont::new(0, Arc::new(std::fs::read("Font.otf")?), 0).unwrap();
let mut shaper = Shaper::new();
let mut atlas = GlyphAtlas::new(&device, AtlasConfig::default())?; // on-demand 1024² MTSDF pages, max 6
let mut text = TextRenderer::new(&device, surface_format);
let units = Units::new(device_pixel_ratio);   // layout in logical (CSS) pixels
let style = TextStyle::new(px(48.0), [1.0; 4]).glow([1.0, 0.5, 0.2, 0.6], em(0.15));

// every frame:
atlas.begin_frame();
let mut quads = Vec::new();
layout_text(&mut atlas, &mut shaper, &font, "Hello, عالم", &style, &units, [20.0, 80.0], 600.0,
            Align::Start, None, &mut quads);
let mut encoder = device.create_command_encoder(&Default::default());
atlas.flush(&device, &mut encoder);            // generates the glyphs missed this frame
text.prepare(&device, &queue, &atlas, [w, h], &quads);
{
    let mut pass = encoder.begin_render_pass(/* your target */);
    text.render(&mut pass);
}
queue.submit([encoder.finish()]);
```

* **Units.** Sizes and effects are typed `Length`s: `px` (logical pixels), `pt`, `em` (of
  the text they belong to, so a glow in em scales with zooming text), `rem` and `%`. `Units`
  carries the device scale (and the root size for `rem`), so layout happens in logical pixels
  and looks the same on any display; `TextStyle` (size, color, weight, outline, glow, shadow)
  resolves to physical pixels once per target, including the reach to request glyphs with.
* **Levels.** A glyph shown at *s* px/em uses the field level `next_pow2(s / magnification)`,
  clamped to `[min_level, max_level]` texels per em (defaults: 2×, 32 to 128). Small text gets
  small fields, large text gets resolution, and the same glyph can be cached at several
  levels. The defaults are no lower because at 16 texels per em, or 4× magnification,
  notches thinner than a texel (where the arch of r, n and m meets the stem) smear into
  visible blobs.
* **Detail probe.** Some glyphs need more than their display size suggests: Devanagari
  hairlines break up at low levels. A CPU test renders a small MSDF of each new glyph at 2×
  and compares it with the exact coverage; the lowest level with at most 5 % of the ink wrong
  becomes that glyph's floor (never above what its display size needs). The test costs about
  0.5 ms per glyph, so it runs off the frame: on a worker thread natively, or
  `probes_per_flush` at a time on `wasm32`. A glyph is generated at its size level right
  away and regenerated once if its probe asks for more (6 of 154 glyphs in the shaping demo,
  none in Inter's ASCII). Off with `detail_probe: false`.
* **Cache.** We pack glyph rectangles in rows on each texture page. `AtlasConfig::layers` is the page
  cap (six by default), not an upfront allocation. `flush` grows the array as needed and
  copies existing pages before writing new glyphs. Acquire custom views and bind groups
  after flushing; `TextRenderer::prepare` automatically rebinds a grown atlas. Before the
  first generated glyph there is only a 1 by 1 placeholder. `page_count()` includes queued
  placements; `allocated_page_count()` reports full GPU pages. At the cap, the least recently used
  layer not used in the current frame is recycled. `texel_budget` caps the generation per
  flush (requests over it reuse another cached level of the glyph and are generated later).
* **Your own cache.** `AtlasTexture` is the texture and its passes alone: `GlyphAtlas` is
  glyphux-gpu's cache plus one. Keep the cache yourself and flush it with
  `texture.flush(&mut cache, &device, &mut encoder)` to choose between backends at run time,
  as the [Godot addon](../glyphux-godot) does.
* **Mirroring.** To draw from a copy of the atlas in another API (an engine's own textures,
  as the [Godot addon](../glyphux-godot) does), `take_written_rows()` lists the texel
  rows that flushes have written to each page since the last call. Read them back with
  `read_texture_rows` once the flush is submitted. `default_device_on(backends)` opens a
  device on the chosen backends; inside a host that draws with OpenGL on the same thread,
  leave out `Backends::GL`.
* **Placement.** `AtlasGlyph` gives the rectangle to draw relative to the glyph's baseline
  (`plane_min/max`, in em units with y pointing up). It also gives the matching rectangle
  in the atlas. Those texture coordinates stop half a texel inside the stored image to
  keep filtering from reading a neighboring glyph.
* **Rendering.** `text.wgsl`: median of RGB, screen-space range from `fwidth`, blending
  towards the true distance (alpha of MTSDF) when minified below ~1 texel per pixel. Per quad
  (`GlyphInstance`): weight, outline, glow and soft drop shadow (from the true distance; the
  shadow's lookups stay inside the glyph's atlas rectangle), and a rotation about a pivot for
  text on paths (`with_weight`, `with_outline`, `with_glow`, `with_shadow`, `with_rotation`).
  `GlyphInstance::atlas_page` draws a whole atlas layer's raw field, for debugging views.
* **Sign correction.** A positive distance means inside the shape; a negative one means
  outside. Combining overlapping or self-crossing outlines can sometimes give a pixel
  the wrong sign, particularly in variable fonts.
  As msdfgen's scanline pass (`GeneratorConfig::sign_correction` on the CPU), the CPU lists
  each texel row's crossings with the outline and the fine pass flips distances whose sign
  disagrees with the exact winding number (`AtlasConfig::sign_correction`, non-zero by
  default; `FieldJob::sign_correction` for raw generation). Narrow-band tiles take their sign
  from the exact winding number too, so one wrong texel can never fill a whole tile.
* **Range tiers.** Outlines, glows and shadows can only reach half the field's distance
  range outside the glyph. `GlyphAtlas::get_with_reach(key, px_per_em, reach_px, || shape)` generates the
  glyph with the range doubled as often as needed (up to `px_range << max_range_tier`, default
  8×) for effects reaching `reach_px` screen pixels at that size, and reuses a glyph already
  cached with a wider range. Each `AtlasGlyph` records its `px_range`, and the shader reads it
  per quad.
* `GlyphAtlas::get(key, px_per_em, || shape)` works with any `glyphux::Shape` source (SVG
  icons, custom outlines), not only fonts.

### Shaping (`shaping` feature, on by default)

[HarfRust](https://github.com/harfbuzz/harfrust), the HarfBuzz project's own Rust port,
does the OpenType shaping: ligatures, [GPOS](https://learn.microsoft.com/en-us/typography/opentype/spec/gpos) kerning and mark placement, contextual forms,
Indic reordering and conjuncts, Arabic joining, Thai marks, variable font instances and any
feature you list (`shaper.features = vec!["smcp".parse()?, "-liga".parse()?]`). Around it,
`Shaper::shape_line` runs the [Unicode bidi algorithm](https://www.unicode.org/reports/tr9/) ([unicode-bidi](https://github.com/servo/unicode-bidi))
to get visual runs, splits them by script, and shapes each piece with its direction and
script and the rest of the line as context. Glyph outlines come from the same file through
glyphux's loader, so the shaper's glyph ids index the atlas directly.

* `ShapingFont::new(id, data, index)`, `.with_variations(id, &[("wght", 700.0)])`.
* `Shaper::shape_line(&font, text) -> ShapedLine` (glyph ids, clusters, positions in em, in
  visual order); `text::layout_shaped` places one; `text::layout_text` does several `\n`
  separated lines with start/end/center alignment (start = right for RTL paragraphs).
* Paragraphs: `glyphux_gpu::paragraph::layout` wraps styled spans (fonts with fallbacks,
  sizes) to a width, in bidi order; place its glyphs with `GlyphAtlas::get_with_reach` and
  `ResolvedStyle::instance`. For more (justification, inline objects), use
  [parley](https://github.com/linebender/parley) or cosmic-text and feed their glyph runs to
  `GlyphAtlas::get`.

Checked against HarfBuzz 8.3 (glyph ids, clusters, advances and offsets), all 11 test lines
matched exactly: Latin ligatures with and without `liga`, kerning,
`tnum,frac,calt`, Arabic, pointed Hebrew, stacked Vietnamese marks, Devanagari conjuncts in
two fonts, Thai. The `shaping_demo` example (top-level `examples/`) renders lines like these
through the atlas.

### What it costs

The calling thread does only what layout needs. `GlyphAtlas::get` for a new glyph loads its
outline, measures its bounds and reserves its atlas rectangle. That takes about 5 µs, and the
returned `AtlasGlyph` (quad, texture coordinates) is final. Everything else (normalize, orient,
edge coloring, curve preparation, tile binning, corner protection and the detail probe) is
queued to worker threads (`worker_threads`, default: cores − 1, at most 4) and runs while the app
goes on with its frame. `flush` joins them: it runs any job no worker has started yet itself,
waits for the rest, records the upload into persistent buffers (grown to the next power of
two, never recreated in steady state) and records the passes. Uploads go through the command
encoder, so any number of flushes may be recorded before a submit. Every glyph
requested in a frame is generated in that frame; nothing arrives a frame late except the
detail probe's occasional upgrade. On `wasm32` (no threads) the same work runs inside `flush`.

Measured on a 2-core sandbox (one worker, software GL), 92 new ASCII glyphs at 120 px: `get`
4 to 5 µs per glyph; `flush` 0.7 to 1.4 ms in steady state when the app does ~8 ms of other work
between layout and flush (the wait for workers is ~0.02 ms; the rest is buffer writes), or
3 to 7 ms when it flushes right after layout and the preparation has had no time to overlap.
Shaping is about 3 µs per character. Glyphs already in the cache cost nothing but their quad.
The GPU side has to be measured on real hardware; it is only exercised on a software
rasterizer here.

`GpuProfiler` times the passes with timestamp queries (when the device has
`Features::TIMESTAMP_QUERY`; `default_device` requests it where available):

```rust
let mut prof = glyphux_wgpu::GpuProfiler::new(&device, &queue, 8);   // None without timestamps
// every frame:
if let Some(p) = prof.as_mut() { p.begin_frame(); }
atlas.flush_profiled(&device, &mut encoder, prof.as_mut());         // distance, correction, store
let scope = prof.as_mut().and_then(|p| p.scope("text"));           // your own passes too
// ... begin_render_pass with timestamp_writes: scope.map(|s| prof.as_ref().unwrap().render_writes(s))
let readback = prof.as_mut().and_then(|p| p.resolve(&device, &mut encoder));
queue.submit([encoder.finish()]);
if let (Some(p), Some(r)) = (prof.as_mut(), readback) { p.submitted(r); }
// a frame or two later (native apps: keep polling the device):
for (pass, ms) in prof.as_mut().unwrap().latest() { println!("{pass}: {ms:.3} ms"); }
```

The native demo can show these GPU timings. The web demo instead uses
`performance.now()` to measure CPU frame preparation and submission; that number
does not measure GPU execution time.

### Raw field generation

```rust
use glyphux_wgpu::{FieldGenerator, FieldJob, FieldKind, GpuShape};

let mut generator = FieldGenerator::new(&device);
let prepared = glyphux::PreparedShape::new(&shape);   // colored shape
let gpu = GpuShape::new(&prepared);
let jobs = [FieldJob::new(&prepared, &gpu, transform, 32, 32, FieldKind::MSDF)];
let mut enc = device.create_command_encoder(&Default::default());
let batch = generator.prepare(&device, &mut enc, &jobs); // bins on the CPU, records the upload
generator.encode(&mut enc, &batch);                     // fine pass + error correction
queue.submit([enc.finish()]);
// batch.field: storage buffer of vec4<f32> per texel; batch.layouts[i] says where job i is.
// read_field(&device, &queue, &batch) copies it back (native, blocking).
```

`FieldJob` also has `overlap`, `error_correction` (`glyphux::ErrorCorrectionConfig`, all of
msdfgen's modes), `band` (narrow band in texels) and `atlas_origin`. To do the CPU part on
your own threads, call `prepare_job(&job)` there (returns an owned `PreparedJob`) and pass the
results to `generator.upload(&device, &mut encoder, &prepared)`. Batches share the
generator's buffers: record a batch's passes before preparing the next one, and copy
`batch.field` out before a later batch's commands overwrite it.

## Validation

`tests/parity.rs` generates a few shapes on both sides on every test run, and with `--ignored`
every printable ASCII glyph (+ a few Latin-1 ones) of a font, reporting per-value differences
and differences of the median (the rendered value). Both skip when there is no adapter.

Every WGSL module (all template variants, exactly as passed to wgpu) is parsed and
validated by naga in glyphux-gpu's unit tests, so shader errors fail `cargo test` with
naga's diagnostics, no GPU needed.

Results on Mesa llvmpipe (GL backend), about 100 glyphs per font; "values" counts channel
values differing by more than 10⁻³:

| Font | Size / range | SDF, PSDF max diff | MSDF EC off: values / medians | MSDF EC auto: values / medians / EC decisions |
|---|---|---:|---:|---:|
| DejaVu Sans | 32 / 4 | 2.1·10⁻⁶ | 0 / 0 | 0 / 0 / 0 |
| | 45 / 6 | 1.7·10⁻⁶ | 7 / 2 | 11 / 2 / 4 |
| Inter | 32 / 4 | 2.1·10⁻⁶ | 0 / 0 | 0 / 0 / 0 |
| | 45 / 6 | 2.0·10⁻⁶ | 9 / 0 | 3 / 0 / 0 |
| Latin Modern Roman Bold | 32 / 4 | 2.9·10⁻⁶ | 4 / 0 | 4 / 0 / 0 |
| | 45 / 6 | 2.6·10⁻⁶ | 32 / 1 | 27 / 1 / 11 |
| Lora Italic (variable) | 32 / 4 | 2.9·10⁻⁶ | 6 / 0 | 6 / 0 / 0 |
| | 45 / 6 | 3.1·10⁻⁶ | 1 / 0 | 1 / 0 / 0 |

MTSDF behaves like MSDF. Every difference inspected is an exact tie: two edges at the same
distance (the dots of ":", or a texel on the symmetry axis of "v", "x", "W": at 45 px the
middle column is exactly on it), where f64 and f32 pick different, equally valid edges. Usually
that changes one channel and not the median; on a symmetry axis it can change two. Error
correction uses an explicit tolerance for the analogous tie (which texel of an equidistant
pair is blamed), without which symmetric glyphs differed on dozens of texels. The "always
check" distance mode differs on 6 to 12 more texels per font (exact-distance comparisons of
borderline candidates).

The narrow band (`band = range/2 + 2` texels, on by default in the atlas) changes only
channels of texels that are beyond the range anyway; renders with and without it are
pixel-identical in the demo.

Timings here are software rasterizer numbers (llvmpipe), not meaningful for a real GPU; on
llvmpipe a batch of ~100 glyphs at 32 px takes about as long as one CPU core.

## Demos

* `cargo run --release --bin text_demo -- Font.otf 1280 720 out 3` (from the repository root)
  renders the demo scene headless to PNGs and dumps the first atlas layer.
* [`examples/web-demo`](../../examples/web-demo) runs the same scene in the browser on
  WebGPU: animated glowing titles, a size ladder in many scripts, text on a spline, drop
  shadows, variable-font weights and an optional live view of the atlas. Build steps are in
  its README.
* [`examples/godot`](../../examples/godot) is a Godot 4.6 project using the
  [Godot addon](../glyphux-godot): `GlyphuxLabel` nodes drawn with Godot's renderers from a
  mirror of the atlas.

## Limitations

* The in-shader overlap combiner handles up to 64 contours per glyph. Glyphs with more are
  generated without overlap support, which is fine unless their contours overlap.
* WebGL 2 is not supported (no compute shaders); WebGPU only on the web.
