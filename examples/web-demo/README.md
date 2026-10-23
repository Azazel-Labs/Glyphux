# textra-gpu web demo

Live: https://azazel-labs.github.io/Textra/ (deployed by CI from main).

The `textra_gpu::demo::Scene` in a browser canvas on WebGPU. Monochrome glyph fields are
generated on the GPU the first time they are needed at a size level. Color emoji rendering
compiles its bundled COLR outlines once at startup, then evaluates cached region-distance
fields and analytic paints on the GPU.

The timing readout uses the browser's CPU clock (`performance.now()`) around frame
preparation and submission for the scene. It does not measure GPU execution time
or the interval between animation frames. The web demo does not request GPU timestamps.

* A title whose letters bounce and wobble one by one under a pulsing glow, its "-gpu"
  breathing from Thin to Black through Inter's variable font (instances at nine weights, the
  shader's weight offset blending between them).
* A subtitle that decodes from scrambled glyphs into the next tagline.
* A size ladder (7 to 36 px) cycling through Latin, Japanese, Chinese, Korean, Cyrillic and Greek.
* A big glyph (&, 永, 鬱, 龍, 한, ß, ॐ, ع) casting a soft drop shadow from a circling light,
  inside a spinning ring of text.
* Your text flowing along an animated cubic spline, each glyph glowing in its own hue.
* A line breathing through the atlas levels in a different script each cycle, outlined and
  glowing.
* The atlas pages themselves, live, as raw MSDF fields: watch them fill up as new text and
  sizes appear (toggle in the panel).
* Noto's gradient fire and grinning face, plus Twemoji's ten-color robot, bounce in
  the title and flow along the editable text spline. They use the same layout and
  animation transforms as text, with their own color rendering pipeline.
* The atlas toggle shows the monochrome pages and shared color atlas pages. Both pools
  allocate pages as needed, each with a default cap of six 1024 by 1024 pages.
  Color thumbnails show packed RGB region distances, not finished emoji colors.
* Glows use four extra samples with a filter that widens outside the glyph. The letter
  fill remains sharp; this is a visual experiment, not a measured speed improvement.

Wide glows and long shadows get their glyphs generated with a wider distance range
(`GlyphAtlas::get_with_reach`), so they never clip. Each line is split into runs by font:
the chosen font where it has the characters, and the bundled fallbacks elsewhere, so mixed
Latin, CJK and Arabic lines work.

The panel sets the text and the font: the bundled ones, any installed font (Chrome / Edge,
through the Local Font Access API, after you allow it) or a font file (any browser).

## Build

In VS Code, run the `web demo` task: it installs the matching `wasm-bindgen-cli` if needed,
builds, serves this folder on http://127.0.0.1:8000 and opens it. By hand:

```sh
rustup target add wasm32-unknown-unknown   # automatic with rust-toolchain.toml
cargo install wasm-bindgen-cli --version <wasm-bindgen version in Cargo.lock> --locked

cargo build --release                   # .cargo/config.toml makes wasm32 the default target
wasm-bindgen --target web --no-typescript --out-dir pkg \
  target/wasm32-unknown-unknown/release/textra_gpu_web_demo.wasm
python3 -m http.server 8000 --bind 127.0.0.1   # then open http://127.0.0.1:8000
```

The release build is tuned for speed (`opt-level = 3`, fat LTO, one codegen unit, wasm
`simd128` from `.cargo/config.toml`): glyph preparation and detail probes run on the page's
only thread. If [binaryen](https://github.com/WebAssembly/binaryen)'s `wasm-opt` is installed,
the `web demo` task also runs `wasm-opt -O3` on the result (about 15 % smaller).

Needs a browser with WebGPU (Chrome / Edge 113+, Safari 26+, Firefox 141+ on Windows). WebGL 2
cannot run the generator (no compute shaders).

## Files

* `src/lib.rs`: canvas, adapter/device, surface (sRGB view), `requestAnimationFrame` loop.
* `index.html`: keeps the canvas at CSS size × `devicePixelRatio`, loads `pkg/`.
* `assets/`: the fonts, Inter (embedded in the wasm), its variable version and the Noto Sans
  fallbacks (loaded by `index.html`), plus embedded color-emoji subsets; see
  `assets/README.md` for their licenses.
