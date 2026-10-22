# msdfr-gpu

GPU generation of SDF / PSDF / MSDF / MTSDF glyph fields with [wgpu](https://wgpu.rs) 30
(WGSL compute), for apps that want distance fields generated at run time instead of baking
atlases: a glyph cache that picks the field resolution from the size text is drawn at, and an
MSDF text renderer on top of it. Native (Vulkan, Metal, DX12, GL) and the web (WebGPU).

The fields are the ones [`msdfr`](../msdfr) makes on the CPU, not an approximation of them:
the same edge selection, the same overlapping contour combiner and the same error
correction. Across four fonts, two sizes and all four field types, the GPU output matches the
CPU's to about 3·10⁻⁶ (f32 vs f64). The exception is where two edges are *exactly* as close
(symmetric geometry) and f32 breaks the tie the other way: at most a few dozen of ~600 000
values, see [Validation](#validation).

## How it works

The workspace [README](../../README.md#how-it-works) walks through the pipeline with figures
drawn from real output, including [the buffers of a batch](../../README.md#inside-a-gpu-batch)
and the passes over them. In short:

![The pipeline](../../docs/figures/pipeline.svg)

| Where | Step |
|---|---|
| CPU, once per glyph | Shape → normalize → orient → color edges; `PreparedShape` → `GpuShape` (edges, contours, cubic interval data); `bin_tiles`: for each 8×8 tile and contour, exact upper bounds of the nearest distance per channel and every edge that can beat them, sorted by a lower bound; corner texels and error correction parameters. |
| GPU fine pass | One 8×8 workgroup per tile of each glyph. Per texel: the nearest edge per channel from the tile's sorted candidates (with early exit), corner perpendiculars, pseudo-distance, and the overlapping contour combiner (up to 64 contours). |
| GPU narrow band | Far tiles only get their sign (from the exact winding number); the rest of the tile is filled in. |
| GPU error correction | Protect corners → protect edges → find artifacts (with exact distance checks at edges) → apply: msdfgen's error correction. |
| GPU atlas store | Field → texture array layer, clamped. |
| Render | Instanced quads, median and screen-space anti-aliasing. |

* **Exact tile culling.** For each tile the CPU computes, per channel, an upper bound of the
  nearest distance over the tile (from edge end and mid points) and keeps only edges whose
  lower bound (bounding box, corner perpendicular cones) is below it, sorted by that lower
  bound. The shader starts from those bounds as thresholds and stops scanning as soon as the
  next candidate's bound exceeds the current worst channel. This is the CPU evaluator's own
  culling, so nothing that could be nearest is ever skipped; bounds are widened by 10⁻⁴ for f32.
* **Real kernels.** Lines, quadratics (safeguarded Newton on monotone pieces of the cubic
  derivative) and cubics (msdfr's interval subdivision of the quintic with precomputed
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

## Usage

### Glyph atlas + text renderer

```rust
use std::sync::Arc;
use msdfr_gpu::{AtlasConfig, GlyphAtlas, Shaper, ShapingFont, TextRenderer, TextStyle, Units};
use msdfr_gpu::text::{layout_text, Align};
use msdfr_gpu::units::{em, px};

let font = ShapingFont::new(0, Arc::new(std::fs::read("Font.otf")?), 0).unwrap();
let mut shaper = Shaper::new();
let mut atlas = GlyphAtlas::new(&device, AtlasConfig::default())?; // 4 × 1024² MTSDF, rgba8
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
* **Cache.** Shelf packing per layer of a texture array; when full, the least recently used
  layer not used in the current frame is recycled. `texel_budget` caps the generation per
  flush (requests over it reuse another cached level of the glyph and are generated later).
* **Placement.** `AtlasGlyph` has the quad in em units (`plane_min/max`, y up) and texture
  coordinates inset by half a texel, so bilinear filtering never touches a neighbor.
* **Rendering.** `text.wgsl`: median of RGB, screen-space range from `fwidth`, blending
  towards the true distance (alpha of MTSDF) when minified below ~1 texel per pixel. Per quad
  (`GlyphInstance`): weight, outline, glow and soft drop shadow (from the true distance; the
  shadow's lookups stay inside the glyph's atlas rectangle), and a rotation about a pivot for
  text on paths (`with_weight`, `with_outline`, `with_glow`, `with_shadow`, `with_rotation`).
  `GlyphInstance::atlas_page` draws a whole atlas layer's raw field, for debugging views.
* **Sign correction.** The overlapping contour combiner (msdfgen's) can give texels inside
  overlapping or self-intersecting contours the wrong sign; variable fonts are full of them.
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
* `GlyphAtlas::get(key, px_per_em, || shape)` works with any `msdfr::Shape` source (SVG
  icons, custom outlines), not only fonts.

### Shaping (`shaping` feature, on by default)

[HarfRust](https://github.com/harfbuzz/harfrust), the HarfBuzz project's own Rust port,
does the OpenType shaping: ligatures, GPOS kerning and mark placement, contextual forms,
Indic reordering and conjuncts, Arabic joining, Thai marks, variable font instances and any
feature you list (`shaper.features = vec!["smcp".parse()?, "-liga".parse()?]`). Around it,
`Shaper::shape_line` runs the Unicode bidi algorithm ([unicode-bidi](https://github.com/servo/unicode-bidi))
to get visual runs, splits them by script, and shapes each piece with its direction and
script and the rest of the line as context. Glyph outlines come from the same file through
msdfr's loader, so the shaper's glyph ids index the atlas directly.

* `ShapingFont::new(id, data, index)`, `.with_variations(id, &[("wght", 700.0)])`.
* `Shaper::shape_line(&font, text) -> ShapedLine` (glyph ids, clusters, positions in em, in
  visual order); `text::layout_shaped` places one; `text::layout_text` does several `\n`
  separated lines with start/end/center alignment (start = right for RTL paragraphs).
* Not covered: line wrapping and font fallback. For paragraphs, use
  [parley](https://github.com/linebender/parley) or cosmic-text and feed their glyph runs to
  `GlyphAtlas::get`. (The demo scene does simple per-character fallback with
  `ShapingFont::has_glyph`.)

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
let mut prof = msdfr_gpu::GpuProfiler::new(&device, &queue, 8);   // None without timestamps
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

The demo scene (native and web) shows these timings in its top-right corner.

### Raw field generation

```rust
use msdfr_gpu::{FieldGenerator, FieldJob, FieldKind, GpuShape};

let mut generator = FieldGenerator::new(&device);
let prepared = msdfr::PreparedShape::new(&shape);   // colored shape
let gpu = GpuShape::new(&prepared);
let jobs = [FieldJob::new(&prepared, &gpu, transform, 32, 32, FieldKind::MSDF)];
let mut enc = device.create_command_encoder(&Default::default());
let batch = generator.prepare(&device, &mut enc, &jobs); // bins on the CPU, records the upload
generator.encode(&mut enc, &batch);                     // fine pass + error correction
queue.submit([enc.finish()]);
// batch.field: storage buffer of vec4<f32> per texel; batch.layouts[i] says where job i is.
// read_field(&device, &queue, &batch) copies it back (native, blocking).
```

`FieldJob` also has `overlap`, `error_correction` (`msdfr::ErrorCorrectionConfig`, all of
msdfgen's modes), `band` (narrow band in texels) and `atlas_origin`. To do the CPU part on
your own threads, call `prepare_job(&job)` there (returns an owned `PreparedJob`) and pass the
results to `generator.upload(&device, &mut encoder, &prepared)`. Batches share the
generator's buffers: record a batch's passes before preparing the next one, and copy
`batch.field` out before a later batch's commands overwrite it.

## Validation

`tests/parity.rs` generates a few shapes on both sides on every test run, and with `--ignored`
every printable ASCII glyph (+ a few Latin-1 ones) of a font, reporting per-value differences
and differences of the median (the rendered value). Both skip when there is no adapter.

Every WGSL module the crate builds (all template variants, exactly as passed to wgpu) is
parsed and validated by naga in the unit tests, so shader errors fail `cargo test` with
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

## Limitations

* The in-shader overlap combiner handles up to 64 contours per glyph. Glyphs with more are
  generated without overlap support, which is fine unless their contours overlap.
* Shaping covers lines, not paragraphs: there is no line wrapping or font fallback (see
  above).
* WebGL 2 is not supported (no compute shaders); WebGPU only on the web.
