//! The state every label shares: the glyph atlas (the subsystem runs its batches on the
//! GPU), the shaper, the fonts, the color glyph pages, and the labels to tell when glyphs they
//! show get better fields.
//!
//! It lives on the game thread, created when the subsystem starts (`init`). Labels request
//! glyphs as they refresh; [`flush`] sends the glyphs requested so far to the GPU in one batch
//! (the labels flush after they refresh, so new text is drawn the frame it appears). Color
//! glyphs are compiled on the CPU the first time they are drawn, and their texels copied
//! into the color pages at once.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use glyphux_gpu::color_atlas::{self, Rect};
use glyphux_gpu::glyphux::color::{ColorField, ColorFieldConfig};
use glyphux_gpu::glyphux::io::font::ttf_parser;
use glyphux_gpu::{AtlasConfig, Binding, GlyphAtlas, GlyphKey, Kernel, Shaper, ShapingFont};
use rivet::Obj;

use crate::{BlockBase, BlockSink, ComponentBase, ComponentSink, FontFace, Gpu, Subsystem};

/// OpenType tag of the weight axis.
const WGHT: u32 = u32::from_be_bytes(*b"wght");
/// Side of the color glyph pages, and how many there are (the subsystem's texture holds
/// them stacked vertically).
pub const COLOR_PAGE: u32 = 1024;
pub const COLOR_PAGES: usize = 6;

/// Fonts are cached by their asset and the point of the design space they are drawn at.
#[derive(Clone, PartialEq, Eq, Hash)]
struct FontKey {
    path: String,
    /// OpenType tags and the bits of their values, sorted by tag.
    variations: Vec<(u32, u32)>,
}

/// A font to shape with.
#[derive(Clone)]
pub struct FontChoice {
    pub font: Rc<ShapingFont>,
    /// Weight (`wght` units) requested beyond the instance's, drawn as extra weight: weights
    /// are instanced 100 units apart, and fonts without the axis keep their own.
    pub missing_weight: f32,
}

/// A color glyph compiled into a color page, with its tables as texels for the materials
/// (numbers as floats, -1 for the "none" index; see `GlyphuxMaterial.ush`).
pub struct ColorGlyph {
    pub page: u32,
    /// Base resolution, grid, range, minimum pixels.
    pub config: [f32; 4],
    /// The quad in em units relative to the glyph origin (y up): min and max.
    pub plane: [[f32; 2]; 2],
    pub tiles: Vec<[f32; 4]>,
    pub paints: Vec<[f32; 4]>,
    pub stops: Vec<[f32; 4]>,
    /// Recipe words, four per texel.
    pub recipes: Vec<[f32; 4]>,
}

/// Color glyphs are cached per font, glyph and foreground color (COLR layers may use it).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct ColorKey {
    font: u32,
    glyph: u32,
    foreground: [u8; 4],
}

#[derive(Clone)]
enum ColorState {
    /// The glyph has no color artwork: drawn from the monochrome atlas.
    None,
    /// Color artwork the color pages can't hold (see `ColorField::build`'s limits).
    Failed,
    Ready(Rc<ColorGlyph>),
}

#[derive(Clone, Copy)]
enum Weight {
    Axis(f32, f32),
    Fixed(f32),
}

/// A label the server tells when it should refresh.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LabelRef {
    Block(Obj<BlockBase>),
    Component(Obj<ComponentBase>),
}

impl LabelRef {
    pub fn invalidate(self) {
        match self {
            LabelRef::Block(b) => b.invalidate_text(),
            LabelRef::Component(c) => c.invalidate_text(),
        }
    }
}

pub struct Server {
    gpu: Obj<Subsystem>,
    pub atlas: GlyphAtlas,
    pub shaper: Shaper,
    fonts: HashMap<FontKey, Rc<ShapingFont>>,
    /// Font file contents by asset path, shared by its weights.
    data: HashMap<String, Arc<Vec<u8>>>,
    /// The weight axis' range, or else the font's weight class, per font.
    weights: HashMap<String, Weight>,
    next_font: u32,
    color_free: Vec<Vec<Rect>>,
    color_glyphs: HashMap<ColorKey, ColorState>,
    /// Labels showing glyphs, and which.
    labels: HashMap<LabelRefKey, (LabelRef, Vec<GlyphKey>)>,
    /// Label updates so far.
    pub updates: u64,
    /// The font of labels without one, once looked up.
    default_font: Option<Option<Obj<FontFace>>>,
}

/// The font face labels without a font use.
const DEFAULT_FONT: &str = "/Engine/EngineFonts/Faces/RobotoRegular.RobotoRegular";

/// A label's identity (its object).
type LabelRefKey = usize;

fn key_of(label: LabelRef) -> LabelRefKey {
    match label {
        LabelRef::Block(b) => b.raw().0 as usize,
        LabelRef::Component(c) => c.raw().0 as usize,
    }
}

thread_local! {
    static STATE: RefCell<Option<Server>> = const { RefCell::new(None) };
}

/// Starts the server for the subsystem `gpu`.
pub fn init(gpu: Obj<Subsystem>) {
    let atlas = match GlyphAtlas::new(AtlasConfig::default()) {
        Ok(a) => a,
        Err(e) => {
            rivet::error!("glyphux: {e}");
            return;
        }
    };
    let server = Server {
        gpu,
        atlas,
        shaper: Shaper::new(),
        fonts: HashMap::new(),
        data: HashMap::new(),
        weights: HashMap::new(),
        next_font: 0,
        color_free: Vec::new(),
        color_glyphs: HashMap::new(),
        labels: HashMap::new(),
        updates: 0,
        default_font: None,
    };
    STATE.with_borrow_mut(|s| *s = Some(server));
}

/// Drops the server (the subsystem stops).
pub fn shutdown() {
    let old = STATE.with_borrow_mut(Option::take);
    drop(old);
}

/// Runs `f` with the server; `None` before `init`, after `shutdown`, while the thread exits,
/// and from inside another `with` (C++ calling back into a label while the server is busy).
pub fn with<R>(f: impl FnOnce(&mut Server) -> R) -> Option<R> {
    STATE
        .try_with(|cell| {
            let mut guard = cell.try_borrow_mut().ok()?;
            guard.as_mut().map(f)
        })
        .ok()
        .flatten()
}

/// Start of a frame: the atlas' frame (for recycling pages), and labels showing glyphs that
/// detail probes raised are asked to refresh.
pub fn tick() {
    let to_refresh = with(|s| {
        s.atlas.begin_frame();
        let raised = s.atlas.take_upgrades();
        if raised.is_empty() {
            return Vec::new();
        }
        s.labels
            .values()
            .filter(|(_, keys)| keys.iter().any(|k| raised.contains(k)))
            .map(|(l, _)| *l)
            .collect()
    })
    .unwrap_or_default();
    // Outside the borrow: the C++ may call back.
    for label in to_refresh {
        label.invalidate();
    }
}

/// Sends the glyphs requested since the last flush to the GPU.
pub fn flush() {
    with(Server::flush);
}

/// The atlas' counters.
pub fn stats() -> String {
    with(|s| {
        let st = s.atlas.stats();
        format!(
            "pages {}/{}, generated {} ({} texels), probes {} ({} upgrades), evicted pages {}, failed {}, color pages {}, label updates {}",
            s.atlas.page_count(),
            s.atlas.config().layers,
            st.generated,
            st.generated_texels,
            st.probes,
            st.probe_upgrades,
            st.evicted_layers,
            st.failed,
            s.color_free.len(),
            s.updates
        )
    })
    .unwrap_or_else(|| "glyphux: not started".into())
}

/// A table word as the materials read it.
fn word(w: u32) -> f32 {
    if w == u32::MAX { -1.0 } else { w as f32 }
}

fn texels(values: &[f32]) -> Vec<[f32; 4]> {
    values
        .chunks(4)
        .map(|c| {
            let mut t = [0.0; 4];
            t[..c.len()].copy_from_slice(c);
            t
        })
        .collect()
}

impl Server {
    /// Records which glyphs `label` shows, to tell it when they get better fields.
    pub fn register(&mut self, label: LabelRef, keys: Vec<GlyphKey>) {
        self.labels.insert(key_of(label), (label, keys));
    }

    pub fn unregister(&mut self, label: LabelRef) {
        self.labels.remove(&key_of(label));
    }

    /// The fonts to shape with: `font`, then `fallbacks`, at weight `wght` (`wght` units) when
    /// given. Fonts whose data can't be read are left out.
    pub fn fonts(&mut self, font: Obj<FontFace>, fallbacks: &[Obj<FontFace>], wght: Option<f32>) -> Vec<FontChoice> {
        std::iter::once(font)
            .chain(fallbacks.iter().copied())
            .filter_map(|f| self.font(f, wght))
            .collect()
    }

    /// The font face asset at `path`, loaded (`[font=...]`).
    pub fn load_font(&self, path: &str) -> Option<Obj<FontFace>> {
        self.gpu.load_font(path)
    }

    /// The font of labels without one: the engine's Roboto, loaded once.
    pub fn default_font(&mut self) -> Option<Obj<FontFace>> {
        if self.default_font.is_none() {
            self.default_font = Some(self.gpu.load_font(DEFAULT_FONT));
        }
        self.default_font.flatten()
    }

    /// The shaping font for a font face asset, at weight `wght`.
    pub fn font(&mut self, face: Obj<FontFace>, wght: Option<f32>) -> Option<FontChoice> {
        let path = self.gpu.font_path(face);
        let data = if let Some(d) = self.data.get(&path) {
            d.clone()
        } else {
            let bytes = self.gpu.font_bytes(face);
            if bytes.is_empty() {
                rivet::warn!("glyphux: cannot read the font {path} (its data is not loaded)");
                return None;
            }
            let d = Arc::new(bytes);
            self.data.insert(path.clone(), d.clone());
            d
        };
        // The weight: the nearest instance at or below it on a 100 unit step, the rest drawn
        // as extra weight (so the weight can animate smoothly).
        let mut variations = Vec::new();
        let mut missing_weight = 0.0;
        if let Some(wght) = wght {
            let weight = *self.weights.entry(path.clone()).or_insert_with(|| {
                let face = ttf_parser::Face::parse(&data, 0).ok();
                let axis = face
                    .as_ref()
                    .and_then(|f| f.variation_axes().into_iter().find(|a| a.tag.0 == WGHT));
                match (axis, &face) {
                    (Some(a), _) => Weight::Axis(a.min_value, a.max_value),
                    (None, Some(f)) => Weight::Fixed(f32::from(f.weight().to_number())),
                    (None, None) => Weight::Fixed(400.0),
                }
            });
            match weight {
                Weight::Axis(lo, hi) => {
                    let step = ((wght.clamp(lo, hi) / 100.0).floor() * 100.0).clamp(lo, hi);
                    missing_weight = wght - step;
                    variations.push((WGHT, step.to_bits()));
                }
                Weight::Fixed(own) => missing_weight = wght - own,
            }
        }
        let key = FontKey { path, variations };
        if let Some(f) = self.fonts.get(&key) {
            return Some(FontChoice {
                font: f.clone(),
                missing_weight,
            });
        }
        let id = self.next_font;
        let mut shaping = ShapingFont::new(id, data, 0)?;
        if !key.variations.is_empty() {
            let tags: Vec<(String, f32)> = key
                .variations
                .iter()
                .map(|&(t, v)| {
                    (
                        String::from_utf8_lossy(&t.to_be_bytes()).into_owned(),
                        f32::from_bits(v),
                    )
                })
                .collect();
            let axes: Vec<(&str, f32)> = tags.iter().map(|(t, v)| (t.as_str(), *v)).collect();
            shaping = shaping.with_variations(id, &axes);
        }
        self.next_font += 1;
        let shaping = Rc::new(shaping);
        self.fonts.insert(key, shaping.clone());
        Some(FontChoice {
            font: shaping,
            missing_weight,
        })
    }

    /// The color version of a glyph (COLR), compiled and placed on a color page the first
    /// time; `None` for glyphs without one (and color glyphs that don't compile or fit).
    pub fn color_glyph(&mut self, font: &ShapingFont, glyph: u32, foreground: [u8; 4]) -> Option<Rc<ColorGlyph>> {
        match self.color_state(font, glyph, foreground) {
            ColorState::Ready(g) => Some(g),
            _ => None,
        }
    }

    /// Whether `font` can draw `c`: it has a glyph for it, and if that glyph is in color, the
    /// color pages can hold it (else a fallback font should draw it).
    pub fn can_draw(&mut self, font: &ShapingFont, c: char, foreground: [u8; 4]) -> bool {
        font.glyph(c)
            .is_some_and(|g| !matches!(self.color_state(font, g, foreground), ColorState::Failed))
    }

    fn color_state(&mut self, font: &ShapingFont, glyph: u32, foreground: [u8; 4]) -> ColorState {
        let key = ColorKey {
            font: font.id,
            glyph,
            foreground,
        };
        if let Some(state) = self.color_glyphs.get(&key) {
            return state.clone();
        }
        let state = match font.color_glyph(glyph, 0, foreground) {
            Err(_) => ColorState::None,
            Ok(artwork) => match self.place_color(&artwork, glyph) {
                Some(g) => ColorState::Ready(Rc::new(g)),
                None => ColorState::Failed,
            },
        };
        self.color_glyphs.insert(key, state.clone());
        state
    }

    fn place_color(&mut self, artwork: &glyphux_gpu::glyphux::color::ColorGlyph, glyph: u32) -> Option<ColorGlyph> {
        let field = match ColorField::build(
            artwork,
            ColorFieldConfig {
                resolution: 512,
                ..ColorFieldConfig::default()
            },
        ) {
            Ok(f) => f,
            Err(e) => {
                rivet::info!("glyphux: color glyph {glyph} left to fallback fonts: {e}");
                return None;
            }
        };
        let [w, h] = field.atlas_size();
        let found = self
            .color_free
            .iter_mut()
            .enumerate()
            .find_map(|(i, free)| color_atlas::allocate(free, w, h).map(|r| (i, r)));
        let (page, rect) = match found {
            Some(found) => found,
            None if self.color_free.len() < COLOR_PAGES => {
                let mut free = vec![Rect {
                    x: 0,
                    y: 0,
                    w: COLOR_PAGE,
                    h: COLOR_PAGE,
                }];
                let rect = color_atlas::allocate(&mut free, w, h)?;
                self.color_free.push(free);
                (self.color_free.len() - 1, rect)
            }
            None => {
                rivet::warn!("glyphux: the color glyph pages are full");
                return None;
            }
        };
        self.gpu
            .write_color_rows(page as u32, rect.x, rect.y, w, h, field.atlas_bytes());
        let encoded = field.encode([rect.x, rect.y]).ok()?;
        let words = |bytes: &[u8]| -> Vec<u32> {
            bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| u32::from_le_bytes(*b))
                .collect()
        };
        let tiles: Vec<f32> = words(&encoded.tiles).into_iter().map(word).collect();
        // Paints: 4 words of header, then 16 floats.
        let paints: Vec<f32> = words(&encoded.paints)
            .chunks(20)
            .flat_map(|p| {
                p.iter()
                    .enumerate()
                    .map(|(i, &w)| if i < 4 { word(w) } else { f32::from_bits(w) })
                    .collect::<Vec<_>>()
            })
            .collect();
        let stops: Vec<f32> = words(&encoded.stops).into_iter().map(f32::from_bits).collect();
        let recipes: Vec<f32> = words(&encoded.recipes).into_iter().map(word).collect();
        let config: Vec<f32> = words(&encoded.config).into_iter().map(f32::from_bits).collect();
        let (min, max) = field.plane_bounds();
        Some(ColorGlyph {
            page: page as u32,
            config: [config[0], config[1], config[2], config[3]],
            plane: [[min.x as f32, min.y as f32], [max.x as f32, max.y as f32]],
            tiles: texels(&tiles),
            paints: texels(&paints),
            stops: texels(&stops),
            recipes: texels(&recipes),
        })
    }

    /// Sends the glyphs requested since the last flush to the GPU, in one batch.
    pub fn flush(&mut self) {
        let Some(batch) = self.atlas.take_batch() else {
            return;
        };
        let gpu = self.gpu;
        gpu.begin_batch(self.atlas.page_count());
        for b in Binding::ALL {
            if matches!(b, Binding::Stencil | Binding::Corners) && !batch.error_correction {
                continue;
            }
            if let Some(bytes) = batch.bytes(b) {
                gpu.batch_input(b.index(), bytes);
            } else if let Some(size) = batch.output_size(b) {
                gpu.batch_output(b.index(), u32::try_from(size).unwrap_or(u32::MAX));
            }
        }
        let index = |k: Kernel| Kernel::ALL.iter().position(|x| *x == k).expect("a kernel") as u32;
        for (kernel, (x, y)) in batch.kernels() {
            gpu.batch_dispatch(index(kernel), x, y);
        }
        let (x, y) = batch.workgroups;
        gpu.batch_dispatch(index(Kernel::Store), x, y);
        for (layer, rows) in self.atlas.take_written_rows() {
            gpu.batch_rows(layer, rows.start, rows.end);
        }
        gpu.submit_batch();
    }
}

/// The distance range of the atlas' fields, in texels (the materials need it).
pub fn px_range() -> f32 {
    with(|s| s.atlas.config().px_range as f32).unwrap_or(AtlasConfig::default().px_range as f32)
}
