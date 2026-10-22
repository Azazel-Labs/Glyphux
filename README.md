# msdfr

[![CI](https://github.com/Azazel-Labs/Glyst/actions/workflows/ci.yml/badge.svg)](https://github.com/Azazel-Labs/Glyst/actions/workflows/ci.yml)
· [Live web demo](https://azazel-labs.github.io/Glyst/) (WebGPU)

A pure Rust generator of signed distance fields for vector shapes and font glyphs —
true SDF, pseudo-SDF, **multi-channel SDF (MSDF)** and **MSDF + true SDF in alpha (MTSDF)** —
based on Viktor Chlumský's multi-channel method ([msdfgen](https://github.com/Chlumsky/msdfgen)).

It is a from-scratch reimplementation, not a port: the same selection semantics (so outputs
match msdfgen bit-for-bit when given the same colored shape), with a faster search, an exact
cubic distance, more robust preprocessing and no C dependencies.

| Crate | What it is |
|---|---|
| [`msdfr`](crates/msdfr) | The library: shapes, edge coloring, SDF/PSDF/MSDF/MTSDF generation, error correction, font/SVG/shape-description input, PNG & co. output. No required dependencies (`ttf-parser` for fonts and `bytemuck` for the `gpu` feature, both optional). No `unsafe`. |
| [`msdfr-atlas`](crates/msdfr-atlas) | Font atlases on top of `msdfr`, in the role of [msdf-atlas-gen](https://github.com/Chlumsky/msdf-atlas-gen): charsets, glyph boxes, packing, parallel generation, JSON/CSV layouts in msdf-atlas-gen's format. Depends only on `msdfr`. |
| [`msdfr-cli`](crates/msdfr-cli) | The `msdfr` and `msdfr-atlas` command line tools ([clap](https://crates.io/crates/clap)); also accept msdfgen / msdf-atlas-gen's single-dash options. |
| [`msdfr-gpu`](crates/msdfr-gpu) | Optional: the same fields generated on the GPU with wgpu 30 compute shaders (native and WebGPU), a glyph atlas that picks the field resolution from the displayed size, and an MSDF text renderer. Separate workspace (the only crate depending on wgpu). |

```
cargo install --path crates/msdfr-cli
msdfr msdf --font MyFont.ttf A -o a.png --size 32 32 --pxrange 4 --autoframe --testrender preview.png 512 512
msdfr-atlas --font MyFont.ttf --type msdf --size 32 --pxrange 4 --imageout atlas.png --json atlas.json
# msdfgen / msdf-atlas-gen style works too:
msdfr msdf -font MyFont.ttf 'A' -o a.png -size 32 32 -pxrange 4 -autoframe
```

## Better / faster / stronger

**Faster** — 3.2–3.9× faster than msdfgen 1.13 single-threaded for MSDF/MTSDF, more with threads:

| Workload (94 printable ASCII glyphs, error correction on) | msdfgen | msdfr | speedup |
|---|---:|---:|---:|
| DejaVu Sans (TrueType) MSDF 32 px, 1 thread | 144.9 ms | 36.7 ms | **3.9×** |
| DejaVu Sans MSDF 64 px, 1 thread | 436.9 ms | 111.6 ms | **3.9×** |
| DejaVu Sans MTSDF 32 px, 1 thread | 146.0 ms | 37.0 ms | **3.9×** |
| DejaVu Sans SDF 32 px, 1 thread | 34.0 ms | 14.9 ms | 2.3× |
| Inter Regular (CFF / cubic) MSDF 32 px, 1 thread | 158.3 ms | 42.0 ms | **3.8×** |
| Inter Regular MSDF 64 px, 1 thread | 492.0 ms | 134.8 ms | **3.7×** |
| Inter Regular SDF 32 px, 1 thread | 50.3 ms | 20.6 ms | 2.4× |
| Atlas: DejaVu MSDF 32 px, 2 threads (msdfgen: OpenMP; msdfr: `par_map`) | 88.1 ms | 18.7 ms | **4.7×** |
| Large: DejaVu MSDF 256 px, 2 threads in each glyph | 3055 ms | 788 ms | **3.9×** |

<sub>2-core Intel Xeon @ 2.1 GHz VM, msdfgen built with `-O3` and OpenMP (`OMP_NUM_THREADS=1`
for the single-thread rows), same shapes, same "simple" edge coloring, best of several runs.
msdfr side: `cargo bench -p msdfr`.</sub>

Where the speed comes from (none of it changes results):
* **Exact culling.** Each 8×8 tile computes provable upper bounds of the nearest distance per
  channel and drops edges that can't beat them; per texel, edges are rejected by bounding-box
  and corner-extension lower bounds against the current best. Only ~1–3 edges per texel get a
  full distance evaluation (msdfgen evaluates every edge).
* **Spatial coherence.** The nearest edges of the previous texel are evaluated first so the
  bounds are tight immediately; texels are visited in serpentine order. Curve kernels start
  Newton at the parameter found for the previous texel.
* **Lean distance kernels.** Precomputed per-edge data (reciprocal lengths for lines, power-basis
  coefficients for quadratics; for cubics the Bernstein coefficients of the point-independent
  parts of the distance derivative on every sample interval, so a query needs 12 multiply-adds
  per interval instead of a basis conversion). No allocation or large stack initialization per
  call; Newton stops once its step is below 1e-9 (the squared distance is stationary there).
* **Vectorized error correction.** Exact, conservative prefilters run over the whole field as
  flat, branchless structure-of-arrays loops the compiler vectorizes (build with
  `-C target-cpu=native` to let it use AVX2 and wider): each texel pair / 2×2 cell is tested once for channel crossings, and the scalar
  artifact tests only visit the few neighbor directions and channel pairs that pass. Edge
  protection likewise only looks at pairs near the edge with a channel crossing zero. The
  exact-distance check only revisits texels the base scan flagged, so msdfgen's default "auto"
  mode costs almost nothing extra.
* **Built-in threading** across rows of large outputs and a `par_map` helper for atlases.

**Stronger** — exactness and robustness:
* *Bit-identical to msdfgen* for TrueType fonts in all four modes and all error-correction
  modes when given the same colored shape (3 fonts × 94 glyphs × 4 modes, ~6 M values,
  0 differences; CFF fonts within 2.4e-7).
* *Exact cubic distance.* msdfgen finds the nearest point on a cubic with a few Newton
  iterations from fixed starts, which can miss tight curves. msdfr isolates the roots of the
  distance derivative in Bernstein form (Descartes' rule of signs) with rigorous pruning
  bounds (checked against brute force by the `cubic_kernel_matches_brute_force` test: 0 misses in 8.6 M queries on
  the cubic edges of two CFF fonts). On random curvy cubic shapes: msdfgen had 36 / 57 600 texels off by more than
  0.01 px (worst 1.89 px); msdfr had none (worst 0.0004 px, the ground truth's sampling error).
* *Contour orientation fixing* by fill rule (non-zero / even-odd) before generation, so
  TrueType (clockwise) and PostScript/CFF (counter-clockwise) outlines, SVG paths and hand-made
  shapes all work without relying on per-texel sign repair.
* *Tie-safe culling*: bounds use a relative margin so exact ties at corners are still decided
  by the orthogonality rule rather than by rounding.
* Cusps are deconverged toward the side each curve actually bends to; zero-length edges are dropped.

**Better** — defaults and features:
* Ink-trap-aware edge coloring by default: fewer visible artifacts than msdfgen's CLI default
  (pixels off by > 0.5 when rendered at 8×, vs. exact rasterization, all printable ASCII):

  | | msdfgen default | msdfr default |
  |---|---:|---:|
  | DejaVu Sans 24 px, range 3 | 365 | **304** |
  | DejaVu Sans 32 px, range 4 | 75 | **67** |
  | Inter 24 px, range 3 | 598 | **411** |
  | Inter 32 px, range 4 | 319 | **187** |

  (msdfgen with `-coloringstrategy inktrap` gets 285 / 67 / 400 / 187 — the field
  generation itself is equivalent; the gain is the default.)
* Zero required dependencies; font loading uses the pure-Rust `ttf-parser` (optional
  `font` feature, on by default) including CFF2 and variable fonts (`-fontvar wght=700`).
* SVG input with all path commands (including arcs), `<rect>`, `<circle>`, `<ellipse>`,
  `<polygon>`, nested `<g>` transforms, `fill-rule`, `display:none` / `fill="none"`, and the
  union of all shapes (msdfgen without Skia uses only the last path).
* Built-in PNG output with its own deflate (lazy-matching LZ77, dynamic Huffman blocks,
  parallel chunks for large images) — atlases 25–30% smaller than msdf-atlas-gen's — plus
  BMP, FL32, RGBA, raw and text output.
* `msdfgen`-compatible text shape descriptions (read and write) and command line.

## Font atlases (`msdfr-atlas`)

Same job and output format as msdf-atlas-gen, so existing loaders and shaders work unchanged:
the JSON has the same `atlas` / `metrics` / `glyphs` (`planeBounds` in ems, `atlasBounds` in
pixels) / `kerning` structure and values, and text rendered from either atlas is visually
identical (mean coverage difference 0.0005).

| Atlas (MSDF, 32 px/em, range 4 px) | msdf-atlas-gen | msdfr-atlas | speedup |
|---|---:|---:|---:|
| DejaVu Sans, all 6 253 glyphs, 1 thread | 22.2 s | 4.4 s | **5.1×** |
| DejaVu Sans, all 6 253 glyphs, 2 threads | 16.8 s | 2.6 s | **6.6×** |
| Inter Regular (CFF), all glyphs, 1 thread | 7.9 s | 2.0 s | **3.9×** |
| Inter Regular, all glyphs, 2 threads | 5.5 s | 1.2 s | **4.6×** |
| DejaVu Sans ASCII MTSDF 48 px/em, 2 threads | 189 ms | 50 ms | **3.8×** |

<sub>Wall-clock time of the command line tools including font loading and writing PNG + JSON.</sub>

Features: charsets in msdf-atlas-gen syntax (`'A'`, `"text"`, `0x41`, `[0x20, 0x7e]`; files
or inline `--chars`), glyph-index sets and `--allglyphs`; atlas types hardmask, softmask, sdf,
psdf, msdf, mtsdf; fixed `--dimensions` (the glyph size is then maximized) or automatic
dimensions with `--pots`, `--potr`, `--square`, `--square2`, `--square4` (default), `--anydims`;
pixel or em ranges (also asymmetric), padding, spacing, miter room for sharp corners, pixel
alignment, uniform grids (`--uniformgrid`, `--uniformcols`, `--uniformcell`), `--yorigin`,
kerning, variable fonts and every msdfr generator option. Packing uses MaxRects for small sets
and a skyline packer for large ones, with a bisection search for the smallest dimensions.

```rust
use msdfr_atlas::*;

let data = std::fs::read("font.ttf")?;
let font = FontGeometry::load(&data, 0, &[], &GlyphSelection::Charset(Charset::ascii()), &GlyphOptions::default())?;
let config = AtlasConfig { size: Some(32.0), range: RangeSpec::Pixels(msdfr::Range::symmetric(4.0)), ..AtlasConfig::default() };
let atlas = generate(&font, &config)?;
atlas.image.save("atlas.png", msdfr::io::ImageFormat::Png)?;
std::fs::write("atlas.json", layout::to_json(&font, &atlas.layout, &config))?;
```

## GPU generation (`msdfr-gpu`)

For apps that generate glyph fields at run time: the CPU bins each glyph's edges into 8×8
tiles with exact culling bounds, and WGSL compute passes evaluate every texel of all glyphs
requested in a frame in one batch — msdfr's selection rules, overlapping contour combiner and
error correction included, matching the CPU output to ~3·10⁻⁶ apart from exact ties.
`GlyphAtlas` caches glyphs in a texture array at power-of-two levels chosen from the
displayed size; `TextRenderer` draws them. See [its README](crates/msdfr-gpu/README.md) and
the [web demo](examples/web-demo).

```
cargo run --release --bin text_demo -- examples/web-demo/assets/Inter-Regular.otf
```

## Library

```rust
use msdfr::*;
use msdfr::io::font::{Font, FontScaling};

let data = std::fs::read("MyFont.ttf")?;
let font = Font::from_bytes(&data, 0, FontScaling::EmNormalized)?;
let mut shape = font.glyph('A')?.shape;

shape.normalize();
shape.orient_contours(FillRule::NonZero);
edge_coloring(&mut shape, ColoringStrategy::InkTrap, DEFAULT_ANGLE_THRESHOLD, 0);

// 32x32 texels, shape units (ems) -> texels: (p + translate) * scale, 0.125 em range.
let transform = Transform::new(Projection::uniform(32.0, Vec2::new(0.125, 0.125)), Range::symmetric(0.125));
let mut msdf = Bitmap::<3>::new(32, 32);
generate_msdf(&mut msdf, &shape, &transform, &GeneratorConfig::default());
msdfr::io::save_png(&msdf, "a.png")?;
```

* Generators: `generate_sdf`, `generate_psdf`, `generate_msdf`, `generate_mtsdf` (and
  `*_prepared` variants that reuse a `PreparedShape`).
* `GeneratorConfig`: `overlap_support` (default on), `error_correction` (msdfgen's modes:
  `EdgePriority`/`Indiscriminate`/`EdgeOnly` × distance check never/at edges/always),
  `sign_correction: Option<FillRule>` (scanline pass), `threads` (0 = automatic).
* Shapes: build from `Edge::line` / `Edge::quadratic` / `Edge::cubic`, parse with
  `Shape::from_description`, load SVG with `io::svg::load_svg`, glyphs with `io::font::Font`.
* Coordinates are y-up; set `shape.inverse_y_axis` for y-down data (SVG loading does this).
  `Bitmap` rows are stored top-to-bottom (image order); `save_fl32` writes msdfgen's bottom-up layout.
* `Transform::autoframe(bounds, w, h, px_range)` mirrors msdfgen's `-autoframe`.
* `render_sdf`, `render_sdf_multi` and `rasterize` for previews and testing.
* `par_map(&items, threads, f)` for building atlases in parallel.

Rendering an MSDF works as usual: sample, take the median of the three channels, and use
`screenPxRange() * (median - 0.5)` as the signed distance in screen pixels.

## Command line

Both tools are built with clap (`--help` lists everything, grouped by topic). Option names
follow msdfgen / msdf-atlas-gen, and their single-dash spelling (`-pxrange 4`) is rewritten to
`--pxrange 4` before parsing, so existing scripts keep working.

`msdfr [sdf|psdf|msdf|mtsdf|metrics] <input> [options]`. Inputs:
`--font <file> <char>` (`'A'`, `65`, `0x41`, or glyph index `g36`), `--svg <file>`,
`--shapedesc <file>`, `--defineshape "<desc>"`, `--stdin`. Common options: `-o`, `--size`/`--dimensions`,
`--pxrange`, `--range`, `--scale`, `--translate`, `--autoframe`, `--angle`, `--coloringstrategy`,
`--errorcorrection`, `--fillrule`, `--testrender`, `--exportshape`, `--printmetrics`, `--threads`, `--stats`.

`msdfr-atlas --font <file> [--charset <file> | --chars <spec> | --glyphset <file> | --allglyphs] --imageout <file> [--json <file>] [--csv <file>] [options]`.

Differences from msdfgen's defaults: ink-trap coloring, contour reorientation (`-noorient`
to disable) and the "auto" error correction mode even with the scanline pass (msdfgen falls
back to "auto-fast"); the scanline pass downgrades to "auto-fast" only if it actually had to
flip signs.

## Tests

`cargo test --release --workspace` runs unit tests (distance functions against brute force, solvers,
coloring validity, parsers, PNG checksums) and integration tests that render generated fields
and compare them with an exact rasterization, check thread determinism and verify that the
culled evaluator matches a brute-force scan; the atlas crate tests the charset parser, packing
(no overlaps, efficiency), plane/atlas bound consistency, size maximization and rendered tiles.
`msdfr-gpu` adds GPU vs CPU parity tests and naga validation of every WGSL module it builds.

Slower checks are `#[ignore]`d and run with `cargo test --release --workspace -- --ignored`:
`fingerprint` hashes the exact output of 54 generator configurations and compares them with
`crates/msdfr/tests/fingerprint.txt`, so optimizations provably don't change results
(`MSDFR_BLESS=1` accepts an intended change); `cubic_kernel_matches_brute_force` checks the
cubic kernel; `whole_font_matches_cpu` compares GPU and CPU fields over a whole font.
`cargo bench -p msdfr` times every mode. All of them use the committed Inter font unless
`MSDFR_TEST_FONT` / `MSDFR_BENCH_FONT` names another.

Runnable demos live in [`examples/`](examples) (`cargo run --release --bin <name>`): `glyph`
(the snippet above), `text_demo`, `shaping_demo`, `glyph_zoom`, and the browser
[`web-demo`](examples/web-demo).

## Development

**CI** (`.github/workflows/ci.yml`, on pushes to main and pull requests): `cargo fmt --check`,
clippy with warnings as errors and the release test suite (GPU tests skip themselves without
an adapter), plus the web demo built for wasm32, bound with wasm-bindgen and optimized with
`wasm-opt -O3`. On main, once both pass, the demo is deployed to GitHub Pages (repository
Settings → Pages → Source: *GitHub Actions*).

Rust 2024 edition, latest stable toolchain (`rust-toolchain.toml` pins `stable` with rustfmt,
clippy and rust-src; the web demo's adds the `wasm32-unknown-unknown` target). Minimum
versions: 1.85 for the core crates, 1.93 for `msdfr-gpu` (wgpu 30).

* **Formatting**: `rustfmt.toml` (style edition 2024, 120 columns) covers every crate in the folder.
  `cargo fmt --all` here, plus `cargo fmt` in `examples/web-demo`.
* **Lints**: `[workspace.lints]` in `Cargo.toml` (the web demo, a separate wasm32-only
  workspace, carries the same table since it cannot inherit it): clippy `all` + `pedantic` as warnings,
  with the numeric cast and short-name lints off (geometry code), deliberate hot-path
  choices allowed (`#[inline(always)]`, branchless `&`/`|` on booleans). `unsafe_code` is
  forbidden in every crate: GPU records are turned into bytes with `bytemuck` (`derive(Pod)`
  checks at compile time that a struct has no padding), and SIMD comes from autovectorization
  instead of runtime-dispatched `target_feature` functions. Both workspaces are
  clippy-clean.
* **Line endings**: `.gitattributes` stores and checks out every text file with LF on every
  OS; `.editorconfig` tells editors the same.
* **VS Code**: open this folder. `.vscode/settings.json` links the two Cargo workspaces
  (root and `examples/web-demo`) for rust-analyzer, checks with
  clippy and formats on save. Tasks (`Terminal → Run Task`): `check: all (clippy)` (default
  build task), `test: all` (default test task), `test: slow`, `bench`, `fmt: all`,
  `run: text_demo` and `web demo` (builds, serves and opens the browser demo). `launch.json`
  has CodeLLDB debug configurations for the glyph example, the CLI, unit tests, `text_demo`
  and the parity tests; tasks and
  launches ask for a font path (default: `examples/web-demo/assets/Inter-Regular.otf`,
  committed under the SIL Open Font License). Recommended extensions are listed in
  `.vscode/extensions.json`.

## Credits and license

The multi-channel distance field method, the pseudo-distance/perpendicular-distance selection,
the overlapping contour combiner and the error correction heuristics are by Viktor Chlumský
(msdfgen, MIT license); the atlas layout format and command line follow his msdf-atlas-gen. This crate is an independent Rust implementation, MIT licensed.
