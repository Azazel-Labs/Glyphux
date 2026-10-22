//! An animated text scene shared by the native (headless) and web demos: a glowing title, a
//! decoding subtitle, a size ladder cycling through scripts, a big glyph inside a spinning
//! ring of text, the user's text flowing along an animated spline, and a line zooming
//! through the atlas levels. Text is split into runs by font: the primary font where it has
//! the characters, the first fallback that does elsewhere.

use std::collections::HashMap;
use std::sync::Arc;

use crate::atlas::{AtlasConfig, GlyphAtlas, GlyphKey};
use crate::shaping::{ShapedLine, Shaper, ShapingFont};
use crate::text::{GlyphInstance, TextRenderer, TextStyle};
use crate::units::{Units, em, px};

/// Taglines the subtitle decodes into, one after another.
const TAGLINES: &[&str] = &[
    "multi-channel signed distance fields, generated on the GPU in real time",
    "every glyph is generated the first time it is needed, at the size it is shown",
    "sharp corners at any size · outlines · glows · text on paths · any font",
];
/// Characters the subtitle scrambles through before it settles.
const SCRAMBLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789#%&*+=?@<>/";
/// Pangrams for the size ladder, a few seconds each.
const PANGRAMS: &[&str] = &[
    "Sphinx of black quartz, judge my vow.",
    "いろはにほへと ちりぬるを わかよたれそ",
    "我能吞下玻璃而不伤身体。",
    "다람쥐 헌 쳇바퀴에 타고파",
    "Съешь же ещё этих мягких французских булок.",
    "Ξεσκεπάζω την ψυχοφθόρα βδελυγμία.",
];
/// The zooming line, one per zoom cycle.
const ZOOM: &[&str] = &[
    "Zoom & glide",
    "拡大と滑走",
    "缩放与滑行",
    "확대와 활주",
    "تكبير وانزلاق",
    "ज़ूम और ग्लाइड",
    "ซูมและเลื่อน",
];
/// The big glyph, a few seconds each: corners, strokes and dense CJK detail.
const BIG: &[&str] = &["&", "永", "鬱", "龍", "한", "ß", "ॐ", "ع"];
const RING: &str = "signed distance fields · generated on the GPU · ";
const DEFAULT_TEXT: &str = "Hello, 世界! Type something · 日本語 · 한국어 · العربية";

pub const BACKGROUND: [u8; 3] = [0x14, 0x16, 0x1c];

/// Colors are linear (the target is expected to be sRGB).
fn srgb(r: u8, g: u8, b: u8, a: f32) -> [f32; 4] {
    let l = |c: u8| {
        let c = c as f32 / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    [l(r), l(g), l(b), a]
}

/// A saturated color of hue `h` (turns), linear.
fn hue(h: f32, a: f32) -> [f32; 4] {
    let h = h.rem_euclid(1.0) * 6.0;
    let c = |k: f32| {
        let x = (h + k).rem_euclid(6.0);
        (1.0 - (x.min(4.0 - x).clamp(0.0, 1.0))).powf(2.2)
    };
    [c(5.0), c(3.0), c(1.0), a]
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A polyline sampled from a curve, parametrized by arc length.
struct Path {
    points: Vec<[f32; 2]>,
    /// Arc length at each point.
    lengths: Vec<f32>,
}

impl Path {
    fn sample(n: usize, f: impl Fn(f32) -> [f32; 2]) -> Self {
        let points: Vec<[f32; 2]> = (0..=n).map(|i| f(i as f32 / n as f32)).collect();
        let mut lengths = vec![0.0];
        for w in points.windows(2) {
            let d = ((w[1][0] - w[0][0]).powi(2) + (w[1][1] - w[0][1]).powi(2)).sqrt();
            lengths.push(lengths.last().unwrap() + d);
        }
        Path { points, lengths }
    }

    fn length(&self) -> f32 {
        *self.lengths.last().unwrap()
    }

    /// Point and tangent angle at arc length `s` (clamped to the path).
    fn at(&self, s: f32) -> ([f32; 2], f32) {
        let s = s.clamp(0.0, self.length());
        let i = self.lengths.partition_point(|&l| l < s).clamp(1, self.points.len() - 1);
        let (a, b) = (self.points[i - 1], self.points[i]);
        let span = (self.lengths[i] - self.lengths[i - 1]).max(1e-6);
        let k = (s - self.lengths[i - 1]) / span;
        let p = [a[0] + (b[0] - a[0]) * k, a[1] + (b[1] - a[1]) * k];
        // Tangent over a few segments, for a smooth rotation from glyph to glyph.
        let (j0, j1) = (i.saturating_sub(3), (i + 2).min(self.points.len() - 1));
        let (c, d) = (self.points[j0], self.points[j1]);
        (p, (d[1] - c[1]).atan2(d[0] - c[0]))
    }
}

struct NamedFont {
    name: String,
    font: ShapingFont,
    /// Takes part in per-character fallback (not the variable-weight instances).
    fallback: bool,
}

/// A piece of a line set in one font: positions are relative to the line's origin (em).
struct Run {
    font: usize,
    line: ShapedLine,
    x: f32,
}

/// A line shaped run by run.
struct Shaped {
    runs: Vec<Run>,
    /// Advance width in em.
    width: f32,
}

/// What a layout step needs, borrowed from the scene.
struct Ctx<'a> {
    fonts: &'a [NamedFont],
    primary: usize,
    runs_for: &'a mut HashMap<String, Vec<(usize, std::ops::Range<usize>)>>,
    shaper: &'a mut Shaper,
    atlas: &'a mut GlyphAtlas,
    out: &'a mut Vec<GlyphInstance>,
    units: Units,
}

impl Ctx<'_> {
    /// Splits `text` into runs by font (byte ranges): letters and digits go to the primary
    /// font if it has them; anything else stays in the current run's font if that has it;
    /// otherwise the first font that has the character takes over.
    fn split(&mut self, text: &str) -> Vec<(usize, std::ops::Range<usize>)> {
        if let Some(r) = self.runs_for.get(text) {
            return r.clone();
        }
        let has = |f: usize, c: char| self.fonts[f].font.has_glyph(c);
        let mut runs: Vec<(usize, std::ops::Range<usize>)> = Vec::new();
        for (i, c) in text.char_indices() {
            let current = runs.last().map(|r| r.0);
            let f = if c.is_alphanumeric() && has(self.primary, c) {
                self.primary
            } else if let Some(f) = current.filter(|&f| c.is_whitespace() || has(f, c)) {
                f
            } else if has(self.primary, c) {
                self.primary
            } else {
                (0..self.fonts.len())
                    .find(|&f| self.fonts[f].fallback && has(f, c))
                    .unwrap_or(self.primary)
            };
            match runs.last_mut() {
                Some(r) if r.0 == f => r.1.end = i + c.len_utf8(),
                _ => runs.push((f, i..i + c.len_utf8())),
            }
        }
        // Changing text (the scrambling subtitle) would grow the cache without bound.
        if self.runs_for.len() > 512 {
            self.runs_for.clear();
        }
        self.runs_for.insert(text.to_owned(), runs.clone());
        runs
    }

    fn shape(&mut self, text: &str) -> Shaped {
        let mut x = 0.0;
        let mut runs = Vec::new();
        for (font, range) in self.split(text) {
            let line = self.shaper.shape_line(&self.fonts[font].font, &text[range]);
            let w = line.width;
            runs.push(Run { font, line, x });
            x += w;
        }
        Shaped { runs, width: x }
    }

    /// Logical pixels per em of text in `style`.
    fn em(&self, style: &TextStyle) -> f32 {
        self.units.text_px(style.size) / self.units.scale
    }

    /// Places one glyph with its origin at `origin`, optionally rotated about a pivot
    /// (logical pixels).
    fn glyph(&mut self, f: usize, glyph: u32, origin: [f32; 2], style: &TextStyle, rotation: Option<(f32, [f32; 2])>) {
        let rs = style.resolve(&self.units);
        let font = &self.fonts[f].font;
        let key = GlyphKey { font: font.id, glyph };
        let (size, reach) = (f64::from(rs.px_per_em), f64::from(rs.reach()));
        let Some(g) = self.atlas.get_with_reach(key, size, reach, || font.outline(glyph)) else {
            return;
        };
        if !g.is_visible() {
            return;
        }
        let mut inst = rs.instance(&g, self.units.to_physical(origin));
        if let Some((angle, pivot)) = rotation {
            inst = inst.with_rotation(angle, self.units.to_physical(pivot));
        }
        self.out.push(inst);
    }

    /// Shapes `text` in font `f` only (no fallback).
    fn shape_in(&mut self, f: usize, text: &str) -> Shaped {
        let line = self.shaper.shape_line(&self.fonts[f].font, text);
        let width = line.width;
        Shaped {
            runs: vec![Run { font: f, line, x: 0.0 }],
            width,
        }
    }

    /// Sets a shaped line in the size of `base` with a per-glyph style, offset (logical pixels)
    /// and rotation (radians, about the glyph's baseline midpoint); returns the pen after it.
    fn place_each(
        &mut self,
        shaped: &Shaped,
        origin: [f32; 2],
        base: &TextStyle,
        each: impl Fn(usize) -> (TextStyle, [f32; 2], f32),
    ) -> [f32; 2] {
        let em = self.em(base);
        let glyphs = shaped
            .runs
            .iter()
            .flat_map(|r| r.line.glyphs.iter().map(move |g| (r.font, r.x, g)));
        for (i, (font, x, sg)) in glyphs.enumerate() {
            let (style, d, angle) = each(i);
            let at = [origin[0] + (x + sg.x) * em + d[0], origin[1] - sg.y * em + d[1]];
            let pivot = [at[0] + 0.5 * sg.advance * em, at[1]];
            self.glyph(font, sg.glyph, at, &style, Some((angle, pivot)));
        }
        [origin[0] + shaped.width * em, origin[1]]
    }

    /// Sets a shaped line on a straight baseline starting at `origin`; returns the pen after it.
    fn place(&mut self, shaped: &Shaped, origin: [f32; 2], style: &TextStyle) -> [f32; 2] {
        let em = self.em(style);
        for run in &shaped.runs {
            for sg in &run.line.glyphs {
                let at = [origin[0] + (run.x + sg.x) * em, origin[1] - sg.y * em];
                self.glyph(run.font, sg.glyph, at, style, None);
            }
        }
        [origin[0] + shaped.width * em, origin[1]]
    }

    fn line(&mut self, text: &str, origin: [f32; 2], style: &TextStyle) -> [f32; 2] {
        let shaped = self.shape(text);
        self.place(&shaped, origin, style)
    }

    /// Sets `text` in the size of `base` along `path`, its start `start` logical pixels along
    /// it (wrapping around if `closed`, else glyphs off either end are dropped). `style` gets
    /// each glyph's index.
    fn on_path(
        &mut self,
        text: &str,
        base: &TextStyle,
        path: &Path,
        start: f32,
        closed: bool,
        style: impl Fn(usize) -> TextStyle,
    ) {
        let em = self.em(base);
        let shaped = self.shape(text);
        let total = path.length();
        let glyphs = shaped
            .runs
            .iter()
            .flat_map(|r| r.line.glyphs.iter().map(move |g| (r.font, r.x, g)));
        for (i, (font, x, sg)) in glyphs.enumerate() {
            let mid = start + (x + sg.x + 0.5 * sg.advance) * em;
            let s = if closed { mid.rem_euclid(total) } else { mid };
            if !(0.0..=total).contains(&s) {
                continue;
            }
            let (p, angle) = path.at(s);
            // The glyph's baseline midpoint sits on the path; the quad turns about it.
            let origin = [p[0] - 0.5 * sg.advance * em, p[1] - sg.y * em];
            self.glyph(font, sg.glyph, origin, &style(i), Some((angle, p)));
        }
    }
}

pub struct Scene {
    fonts: Vec<NamedFont>,
    primary: usize,
    runs_for: HashMap<String, Vec<(usize, std::ops::Range<usize>)>>,
    shaper: Shaper,
    pub atlas: GlyphAtlas,
    renderer: TextRenderer,
    instances: Vec<GlyphInstance>,
    text: String,
    /// Glyphs generated in the last frame.
    pub last_generated: usize,
    profiler: Option<crate::profile::GpuProfiler>,
    /// Latest GPU pass durations (ms) by label, and those of the latest frame that generated.
    pub gpu_times: Vec<(&'static str, f64)>,
    pub gpu_generation: Vec<(&'static str, f64)>,
    /// CPU time of the last frame's layout + atlas work (ms; native only).
    pub cpu_ms: f64,
    /// Instances of the variable font at [`WEIGHTS`] (indices into `fonts`).
    weights: Vec<usize>,
    /// Draw the atlas pages along the bottom.
    pub show_atlas: bool,
    /// The target's device scale ([`Self::set_scale`]); the scene is laid out in logical pixels.
    units: Units,
}

/// Weights the variable font is instanced at; the shader's weight offset blends between them.
const WEIGHTS: [f32; 9] = [100.0, 200.0, 300.0, 400.0, 500.0, 600.0, 700.0, 800.0, 900.0];

impl Scene {
    /// The scene's default atlas: MTSDF (glows and outlines use the true distance), more layers
    /// than the default for CJK text at many sizes.
    pub fn default_config() -> AtlasConfig {
        AtlasConfig {
            layers: 8,
            ..AtlasConfig::default()
        }
    }

    /// A scene whose text is set in `font_data` (named `name`), with fallbacks added by
    /// [`Self::add_font`].
    pub fn new(
        device: &wgpu::Device,
        target: wgpu::TextureFormat,
        name: &str,
        font_data: Vec<u8>,
    ) -> Result<Self, String> {
        Self::with_config(device, target, name, font_data, Self::default_config())
    }

    pub fn with_config(
        device: &wgpu::Device,
        target: wgpu::TextureFormat,
        name: &str,
        font_data: Vec<u8>,
        config: AtlasConfig,
    ) -> Result<Self, String> {
        let mut scene = Scene {
            fonts: Vec::new(),
            primary: 0,
            runs_for: HashMap::new(),
            shaper: Shaper::new(),
            atlas: GlyphAtlas::new(device, config),
            renderer: TextRenderer::new(device, target),
            instances: Vec::new(),
            text: DEFAULT_TEXT.to_owned(),
            last_generated: 0,
            profiler: None,
            gpu_times: Vec::new(),
            gpu_generation: Vec::new(),
            cpu_ms: 0.0,
            weights: Vec::new(),
            show_atlas: false,
            units: Units::default(),
        };
        scene.add_font(name, font_data, 0)?;
        Ok(scene)
    }

    /// Adds a font (face `index` of a collection) to the fallback list; a font already added
    /// under `name` is replaced.
    pub fn add_font(&mut self, name: &str, data: Vec<u8>, index: u32) -> Result<(), String> {
        let slot = self
            .fonts
            .iter()
            .position(|f| f.name == name)
            .unwrap_or(self.fonts.len());
        // Ids are never reused, so the atlas never mixes up two fonts' glyphs.
        let id = self.fonts.iter().map(|f| f.font.id + 1).max().unwrap_or(0);
        let font = ShapingFont::new(id, Arc::new(data), index).ok_or_else(|| format!("{name}: not a font"))?;
        let named = NamedFont {
            name: name.to_owned(),
            font,
            fallback: true,
        };
        if slot == self.fonts.len() {
            self.fonts.push(named);
        } else {
            self.fonts[slot] = named;
        }
        self.runs_for.clear();
        Ok(())
    }

    /// A variable font with a `wght` axis for the title's breathing weight (replaces the
    /// previous one).
    pub fn set_weight_font(&mut self, data: Vec<u8>) -> Result<(), String> {
        let data = Arc::new(data);
        for (k, &wght) in WEIGHTS.iter().enumerate() {
            let id = self.fonts.iter().map(|f| f.font.id + 1).max().unwrap_or(0);
            let font = ShapingFont::new(id, data.clone(), 0)
                .ok_or("variable font: not a font")?
                .with_variations(id, &[("wght", wght)]);
            let named = NamedFont {
                name: format!("weight {wght}"),
                font,
                fallback: false,
            };
            if let Some(&slot) = self.weights.get(k) {
                self.fonts[slot] = named;
            } else {
                self.weights.push(self.fonts.len());
                self.fonts.push(named);
            }
        }
        Ok(())
    }

    /// Sets all text in the font added as `name` where it has the characters (other fonts
    /// fill in per line). Returns whether there is such a font.
    pub fn set_primary_font(&mut self, name: &str) -> bool {
        let Some(i) = self.fonts.iter().position(|f| f.name == name) else {
            return false;
        };
        self.primary = i;
        self.runs_for.clear();
        true
    }

    /// Physical pixels per logical pixel of the target (the device pixel ratio): the scene is
    /// laid out in logical pixels, so it looks the same on any display.
    pub fn set_scale(&mut self, scale: f32) {
        self.units.scale = scale.max(0.1);
    }

    /// The text flowing along the spline.
    pub fn set_text(&mut self, text: &str) {
        text.clone_into(&mut self.text);
    }

    /// Times the GPU passes with timestamp queries (if the device has them) and shows the
    /// results in the scene. Returns whether timestamps are available.
    pub fn enable_profiling(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> bool {
        self.profiler = crate::profile::GpuProfiler::new(device, queue, 8);
        self.profiler.is_some()
    }

    /// The timing overlay (top right), one string per line.
    fn stats_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        let get = |v: &[(&str, f64)], k: &str| v.iter().find(|(l, _)| *l == k).map(|(_, t)| *t);
        if self.profiler.is_some() {
            if let Some(t) = get(&self.gpu_times, "msdfr text") {
                lines.push(format!("GPU text draw {t:.3} ms"));
            }
            let g = &self.gpu_generation;
            if !g.is_empty() {
                let total: f64 = g.iter().map(|(_, t)| t).sum();
                lines.push(format!(
                    "last generation {total:.3} ms (distance {:.3}, correction {:.3}, store {:.3})",
                    get(g, "msdfr distance").unwrap_or(0.0),
                    get(g, "msdfr correction").unwrap_or(0.0),
                    get(g, "msdfr store").unwrap_or(0.0)
                ));
            }
        } else {
            lines.push("no GPU timestamps on this device".to_owned());
        }
        let st = self.atlas.stats();
        let glyphs = format!(
            "{} glyphs generated, {} raised by the detail probe",
            st.generated, st.probe_upgrades
        );
        #[cfg(not(target_arch = "wasm32"))]
        let glyphs = format!("CPU {:.2} ms  ·  {glyphs}", self.cpu_ms);
        lines.push(glyphs);
        lines.push(format!("font: {}", self.fonts[self.primary].name));
        lines
    }

    /// Lays out the scene at time `t` (seconds) for a target of `size` physical pixels.
    #[allow(clippy::too_many_lines)]
    fn layout(&mut self, size: [f32; 2], t: f32) {
        self.instances.clear();
        let stats = self.stats_lines();
        let weights = self.weights.clone();
        let units = self.units;
        // Everything below is in logical pixels.
        let (w, full_h) = (size[0] / units.scale, size[1] / units.scale);
        let margin = (w * 0.04).max(12.0);
        // Atlas pages along the bottom (drawn first, under everything), live as they fill up.
        let layers = self.atlas.config().layers;
        let mut strip = 0.0;
        let mut strip_label = None;
        if self.show_atlas {
            let gap = (w * 0.006).max(4.0);
            let th = ((w * 0.55 - margin) / layers as f32 - gap).min(full_h * 0.11).max(16.0);
            let y = full_h - margin - th;
            for l in 0..layers {
                let x = margin + l as f32 * (th + gap);
                let rect = [x, y, x + th, y + th].map(|v| v * units.scale);
                self.instances.push(GlyphInstance::atlas_page(rect, l));
            }
            let label_px = (11.0 * (full_h / 720.0).clamp(0.6, 2.0)).max(10.0);
            strip_label = Some((label_px, [margin, y - label_px * 0.6]));
            strip = th + label_px * 2.2;
        }
        let mut cx = Ctx {
            fonts: &self.fonts,
            primary: self.primary,
            runs_for: &mut self.runs_for,
            shaper: &mut self.shaper,
            atlas: &mut self.atlas,
            out: &mut self.instances,
            units,
        };
        let fg = srgb(0xe8, 0xe6, 0xe3, 1.0);
        let dim = srgb(0x9a, 0x9c, 0xa3, 1.0);
        let accent = srgb(0xff, 0x8a, 0x3d, 1.0);
        let teal = srgb(0x4f, 0xd1, 0xc5, 1.0);
        let h = full_h - strip;
        let scale = (h / 720.0).clamp(0.6, 2.0);
        let text = |size: f32, color: [f32; 4]| TextStyle::new(px(size), color);
        if let Some((size, at)) = strip_label {
            let label = format!(
                "atlas: {layers} pages of {0}×{0} texels, generated as text needs them",
                cx.atlas.config().size
            );
            cx.line(&label, at, &text(size, dim));
        }

        // Title: "msdfr" bouncing letter by letter, wobbling, glowing with a slow pulse;
        // "-gpu" breathing between the variable font's thinnest and boldest weights.
        let title = (h * 0.11).min(w * 0.09);
        let pulse = 0.5 + 0.5 * (t * 2.2).sin();
        let glow = text(title, accent).glow(srgb(0xff, 0x6a, 0x1a, 0.35 + 0.35 * pulse), em(0.08 + 0.1 * pulse));
        let shaped = cx.shape("msdfr");
        let pen = cx.place_each(&shaped, [margin, margin + title * 0.8], &glow, |i| {
            let hop = (t * 3.2 - i as f32 * 0.55).sin().abs();
            let wobble = 0.09 * (t * 2.3 + i as f32 * 1.3).sin();
            (glow, [0.0, -hop * title * 0.1], wobble)
        });
        let wght = 500.0 + 400.0 * (t * 1.3).sin();
        let style = text(title, fg);
        if weights.is_empty() {
            cx.line("-gpu", pen, &style);
        } else {
            // The instance just below the weight, emboldened by the remainder (about 1% of
            // an em per 100 units of weight).
            let pos = ((wght - WEIGHTS[0]) / 100.0).clamp(0.0, (WEIGHTS.len() - 1) as f32);
            let k = (pos as usize).min(WEIGHTS.len() - 2);
            let shaped = cx.shape_in(weights[k], "-gpu");
            cx.place(&shaped, pen, &style.weight(em(0.01 * (pos - k as f32))));
        }

        // Subtitle: decodes into the next tagline every few seconds, scrambled glyphs first.
        let sub = (title * 0.22).max(11.0);
        let period = 7.0;
        let k = (t / period) as usize;
        let target = TAGLINES[k % TAGLINES.len()];
        let revealed = (smoothstep(0.0, 1.6, t - k as f32 * period) * target.chars().count() as f32) as usize;
        let tick = (t * 24.0) as usize;
        let shown: String = target
            .chars()
            .enumerate()
            .map(|(i, c)| {
                if i < revealed || c == ' ' {
                    c
                } else {
                    let r = (i * 7919 + tick * 104_729) % SCRAMBLE.len();
                    SCRAMBLE[r] as char
                }
            })
            .collect();
        cx.line(&shown, [margin, margin + title * 1.15 + sub], &text(sub, dim));

        // Size ladder (the label column is fixed so the samples line up), one script after
        // another.
        let pangram = PANGRAMS[(t / 5.0) as usize % PANGRAMS.len()];
        let mut y = margin + title * 1.3 + sub * 2.2;
        for s in [9.0, 12.0, 16.0, 24.0, 36.0] {
            let s = s * scale;
            y += s * 1.3;
            cx.line(&format!("{} px", s.round()), [margin, y], &text(11.0 * scale, dim));
            cx.line(pangram, [margin + 60.0 * scale, y], &text(s, fg));
        }

        // A big glyph at the right inside a spinning ring of text: sharp corners and dense
        // strokes at any magnification.
        let big = (h * 0.3).min(w * 0.2);
        let center = [w - margin - big * 0.95, h * 0.44];
        let beat = 3.5;
        let k = (t / beat) as usize;
        let fade =
            smoothstep(0.0, 0.35, t - k as f32 * beat) * (1.0 - smoothstep(beat - 0.35, beat, t - k as f32 * beat));
        let glyph = cx.shape(BIG[k % BIG.len()]);
        let gw = glyph.width * big;
        // A drop shadow cast by a light circling the glyph.
        let light = t * 0.9;
        let style = text(big, [fg[0], fg[1], fg[2], fade]).shadow(
            [0.0, 0.0, 0.0, 0.75 * fade],
            [em(0.05 * light.cos()), em(0.05 * light.sin())],
            em(0.04),
        );
        cx.place(&glyph, [center[0] - gw * 0.5, center[1] + big * 0.36], &style);
        let r = big * 0.8;
        let ring = Path::sample(256, |u| {
            let a = u * std::f32::consts::TAU;
            [center[0] + r * a.cos(), center[1] + r * a.sin()]
        });
        let ring_px = big * 0.085;
        let ring_style = text(ring_px, fg);
        // Enough copies of the ring text to go all the way round.
        let copies = (ring.length() / (cx.shape(RING).width * ring_px)).floor().max(1.0) as usize;
        cx.on_path(&RING.repeat(copies), &ring_style, &ring, -t * big * 0.25, true, |i| {
            let h = t * 0.05 + i as f32 / 48.0;
            ring_style.color(hue(h, 1.0)).glow(hue(h, 0.45), em(0.25))
        });

        // The user's text flowing along an animated cubic spline, each glyph glowing in its
        // own hue.
        let x0 = margin;
        // Between the ladder and the zooming line, left of the ring.
        let label_size = 12.0 * scale;
        let zoom_max = 18.0 + h * 0.12;
        let zoom_top = h - margin - label_size * 2.0 - zoom_max;
        let x1 = (center[0] - r - ring_px * 2.0).max(w * 0.4);
        let wave_y = f32::midpoint(y, zoom_top);
        let amp = (0.22 * (zoom_top - y)).max(0.0);
        let ctrl = [
            [x0, wave_y],
            [x0 + (x1 - x0) / 3.0, wave_y - amp * 2.0 * (t * 1.1).sin()],
            [x0 + (x1 - x0) * 2.0 / 3.0, wave_y + amp * 2.0 * (t * 1.4 + 1.0).sin()],
            [x1, wave_y + amp * 0.5 * (t * 0.7).cos()],
        ];
        let spline = Path::sample(128, |u| {
            let v = 1.0 - u;
            let b = [v * v * v, 3.0 * v * v * u, 3.0 * v * u * u, u * u * u];
            [0, 1].map(|c| (0..4).map(|i| b[i] * ctrl[i][c]).sum())
        });
        let text_px = (h * 0.05).max(16.0);
        let flowing = text(text_px, fg);
        let user = if self.text.trim().is_empty() {
            " "
        } else {
            self.text.as_str()
        };
        let measured = cx.shape(user);
        // Scrolls in from the right, across, and out at the left, then again.
        let travel = spline.length() + measured.width * text_px;
        let start = spline.length() - (t * 90.0 * scale).rem_euclid(travel);
        cx.on_path(user, &flowing, &spline, start, false, |i| {
            let twinkle = 0.6 + 0.4 * (t * 3.0 + i as f32 * 0.5).sin();
            flowing.glow(hue(t * 0.15 + i as f32 * 0.04, 0.85), em(0.45 * twinkle))
        });

        // A breathing line: its size sweeps over the atlas levels (outlined with the MTSDF's
        // true distance channel), in another script each cycle.
        let phase = t * 0.8;
        let z = 0.5 - 0.5 * phase.cos();
        let zoom_text = ZOOM[(phase / std::f32::consts::TAU) as usize % ZOOM.len()];
        let s = 18.0 + z * (zoom_max - 18.0);
        let base = h - margin - label_size * 2.0 - s * 0.24;
        let level = cx.atlas.level_for(f64::from(s * units.scale));
        let style = text(s, teal)
            .outline(srgb(0x0b, 0x4a, 0x44, 1.0), em(0.03))
            .glow(srgb(0x4f, 0xd1, 0xc5, 0.5), em(0.2));
        cx.line(zoom_text, [margin, base], &style);
        let label = format!("{s:.0} px  ·  size level {level} texels/em");
        cx.line(&label, [margin, h - margin], &text(label_size, dim));

        // Timings, top right.
        let size_px = (11.0 * scale).max(10.0);
        for (i, line) in stats.iter().enumerate() {
            let shaped = cx.shape(line);
            let x = w - margin - shaped.width * size_px;
            let y = margin + size_px * (1.0 + 1.5 * i as f32);
            cx.place(&shaped, [x.max(margin), y], &text(size_px, dim));
        }
    }

    /// Renders a frame into `target` (`size` in pixels). Submits its own command buffer.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target: &wgpu::TextureView,
        size: [u32; 2],
        t: f32,
    ) {
        #[cfg(not(target_arch = "wasm32"))]
        let start = std::time::Instant::now();
        if let Some(p) = &mut self.profiler {
            let latest = p.latest().to_vec();
            if !latest.is_empty() {
                if latest.iter().any(|(l, _)| l.starts_with("msdfr d")) {
                    self.gpu_generation = latest.iter().filter(|(l, _)| *l != "msdfr text").copied().collect();
                }
                self.gpu_times = latest;
            }
        }
        self.atlas.begin_frame();
        let sizef = [size[0] as f32, size[1] as f32];
        self.layout(sizef, t);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("msdfr demo"),
        });
        self.last_generated = self
            .atlas
            .flush_profiled(device, queue, &mut encoder, self.profiler.as_mut());
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.cpu_ms = start.elapsed().as_secs_f64() * 1e3;
        }
        let text_scope = self.profiler.as_mut().and_then(|p| p.scope("msdfr text"));
        self.renderer
            .prepare(device, queue, &self.atlas, sizef, &self.instances);
        {
            let bg = srgb(BACKGROUND[0], BACKGROUND[1], BACKGROUND[2], 1.0);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("msdfr demo"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: bg[0] as f64,
                            g: bg[1] as f64,
                            b: bg[2] as f64,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: text_scope.and_then(|s| self.profiler.as_ref().map(|p| p.render_writes(s))),
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.renderer.render(&mut pass);
        }
        let readback = self.profiler.as_mut().and_then(|p| p.resolve(device, &mut encoder));
        queue.submit([encoder.finish()]);
        if let (Some(p), Some(r)) = (&mut self.profiler, readback) {
            p.submitted(r);
        }
    }
}
