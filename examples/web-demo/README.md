# msdfr-gpu web demo

Live: https://azazel-labs.github.io/Glyst/ (deployed by CI from main).

The `msdfr_gpu::demo::Scene` in a browser canvas on WebGPU. Every glyph's field is generated
on the GPU the first time it is needed at a size level; nothing is pre-baked.

* A title whose letters bounce and wobble one by one under a pulsing glow, its "-gpu"
  breathing from Thin to Black through Inter's variable font (instances at nine weights, the
  shader's weight offset blending between them).
* A subtitle that decodes from scrambled glyphs into the next tagline.
* A size ladder (7–36 px) cycling through Latin, Japanese, Chinese, Korean, Cyrillic and Greek.
* A big glyph (&, 永, 鬱, 龍, 한, ß, ॐ, ع) casting a soft drop shadow from a circling light,
  inside a spinning ring of text.
* Your text flowing along an animated cubic spline, each glyph glowing in its own hue.
* A line breathing through the atlas levels in a different script each cycle, outlined and
  glowing.
* The atlas pages themselves, live, as raw MSDF fields: watch them fill up as new text and
  sizes appear (toggle in the panel).

Wide glows and long shadows get their glyphs generated with a wider distance range (`GlyphAtlas::get_with_reach`),
so they never clip. Each line is split into runs by font: the chosen font where it has the
characters, the bundled fallbacks elsewhere (mixed Latin / CJK / Arabic lines work).

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
  target/wasm32-unknown-unknown/release/msdfr_gpu_web_demo.wasm
python3 -m http.server 8000 --bind 127.0.0.1   # then open http://127.0.0.1:8000
```

The release build is tuned for speed (`opt-level = 3`, fat LTO, one codegen unit, wasm
`simd128` from `.cargo/config.toml`): glyph preparation and detail probes run on the page's
only thread. If [binaryen](https://github.com/WebAssembly/binaryen)'s `wasm-opt` is installed,
the `web demo` task also runs `wasm-opt -O3` on the result (about 15 % smaller).

Needs a browser with WebGPU (Chrome / Edge 113+, Safari 26+, Firefox 141+ on Windows). WebGL 2
cannot run the generator (no compute shaders).

## Files

* `src/lib.rs` — canvas, adapter/device, surface (sRGB view), `requestAnimationFrame` loop.
* `index.html` — keeps the canvas at CSS size × `devicePixelRatio`, loads `pkg/`.
* `assets/` — the embedded font (Inter, SIL OFL; see `assets/README.md`).
