//! The state every label shares: the glyph atlas and the backend that generates it, the color
//! glyph pages, the shaper, the fonts and the shaders.
//!
//! Labels request glyphs while they process. Just before Godot draws the frame
//! (`RenderingServer.frame_pre_draw`), [`end_frame`] generates every monochrome glyph missed
//! in the frame in one batch:
//!
//! * on Godot's own `RenderingDevice` (Forward+, Mobile; [`crate::rd`]), straight into the
//!   texture the labels sample;
//! * with the Compatibility renderer (OpenGL, which has no `RenderingDevice`), on a wgpu
//!   device of glyphux's own, reading back the texel rows the batch wrote into a mirror.
//!
//! `GLYPHUX_BACKEND=wgpu` in the environment picks the second everywhere (for comparisons).
//! Color glyphs are compiled on the CPU when first drawn and copied into their pages' mirror
//! directly.
//!
//! Labels do nothing until something they show changes: they [`mark_dirty`] themselves, and
//! one deferred call at the end of the frame updates the dirty ones. What dirties a label:
//! its own properties, its fonts' and curve's `changed` signals, its scale, and two things
//! the server watches for registered labels at the start of each frame ([`watch`]): the
//! scale of each canvas (camera, `CanvasLayer`, the window's stretch; checked once per
//! canvas, not per label) and detail probes raising the resolution of glyphs a label shows.
//! Labels retain the atlas pages their glyphs are on, so pages they show are never recycled.
//!
//! The shaders are the addon's `shaders/`: glyphux's own shading functions, which build.rs
//! translates from its WGSL into `shaders/generated/`, inside wrappers that read Godot's
//! textures.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use glyphux_wgpu::GlyphKey;
use glyphux_wgpu::glyphux::color::{ColorField, ColorFieldConfig};
use glyphux_wgpu::glyphux::io::font::ttf_parser;
use glyphux_wgpu::glyphux_gpu::GlyphAtlas;
use glyphux_wgpu::glyphux_gpu::color_atlas::{self, Rect};
use glyphux_wgpu::{AtlasConfig, AtlasTexture, Shaper, ShapingFont, wgpu};
use godot::classes::image::Format;
use godot::classes::{
    CanvasItem, Engine, Font, FontFile, FontVariation, Image, Node3D, RenderingServer, SceneTree, Shader,
    Texture2DArray, TextureLayered,
};

use crate::rd::RdAtlas;
use godot::prelude::*;

/// OpenType tag of the weight axis.
const WGHT: u32 = u32::from_be_bytes(*b"wght");
/// Side of the color glyph pages, and how many there may be.
const COLOR_PAGE: u32 = 1024;
const COLOR_PAGES: usize = 6;

/// Fonts are cached by their file and the point of the design space they are drawn at.
#[derive(Clone, PartialEq, Eq, Hash)]
struct FontKey {
    file: InstanceId,
    index: u32,
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

/// A color glyph compiled into a color page, with its tables as texels for the shader
/// (numbers as floats, -1 for the "none" index; see `shaders/color.gdshader`).
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

/// A Godot layered texture holding a copy of pages kept on the CPU (RGBA8, rows top to
/// bottom): Godot updates whole layers.
struct Mirror {
    texture: Gd<Texture2DArray>,
    size: u32,
    pages: Vec<Vec<u8>>,
    dirty: Vec<bool>,
    /// Pages in `texture` (0: the 1×1 placeholder).
    texture_pages: usize,
}

impl Mirror {
    fn new(size: u32) -> Self {
        let mut texture = Texture2DArray::new_gd();
        let placeholder = Image::create_empty(1, 1, false, Format::RGBA8).expect("a 1×1 image");
        texture.create_from_images(&array![&placeholder]);
        Mirror {
            texture,
            size,
            pages: Vec::new(),
            dirty: Vec::new(),
            texture_pages: 0,
        }
    }

    fn ensure_pages(&mut self, pages: usize) {
        let bytes = (self.size * self.size * 4) as usize;
        self.pages.resize_with(pages.max(self.pages.len()), || vec![0; bytes]);
        self.dirty.resize(self.pages.len(), true);
    }

    /// Copies `rows` rows of `width` texels to `(x, y)` of a page.
    fn write(&mut self, page: usize, [x, y]: [u32; 2], width: u32, data: &[u8]) {
        let stride = (self.size * 4) as usize;
        let row = (width * 4) as usize;
        for (r, src) in data.chunks(row).enumerate() {
            let at = (y as usize + r) * stride + x as usize * 4;
            self.pages[page][at..at + src.len()].copy_from_slice(src);
        }
        self.dirty[page] = true;
    }

    /// Updates the Godot texture with the changed pages.
    fn upload(&mut self) {
        let side = self.size as i32;
        let image = |page: &Vec<u8>| {
            Image::create_from_data(
                side,
                side,
                false,
                Format::RGBA8,
                &PackedByteArray::from(page.as_slice()),
            )
            .expect("an RGBA8 page")
        };
        if self.pages.len() == self.texture_pages {
            for (layer, page) in self.pages.iter().enumerate() {
                if self.dirty[layer] {
                    self.texture.update_layer(&image(page), layer as i32);
                }
            }
        } else {
            // Godot's layered textures don't grow: create it again, under the same RID, so
            // materials keep sampling it.
            let images: Array<Gd<Image>> = self.pages.iter().map(image).collect();
            self.texture.create_from_images(&images);
            self.texture_pages = self.pages.len();
        }
        self.dirty.fill(false);
    }
}

/// What generates the atlas.
enum Backend {
    Rd(RdAtlas),
    Wgpu(Box<WgpuAtlas>),
}

/// glyphux's own wgpu device, with a mirror of the pages.
struct WgpuAtlas {
    device: wgpu::Device,
    queue: wgpu::Queue,
    gpu: AtlasTexture,
    mirror: Mirror,
}

pub struct Server {
    backend: Backend,
    /// The atlas' cache (glyphux-gpu's; the backend holds the texture).
    pub atlas: GlyphAtlas,
    pub shaper: Shaper,
    fonts: HashMap<FontKey, Rc<ShapingFont>>,
    /// Font file contents by `FontFile`, shared by its variations.
    data: HashMap<InstanceId, Arc<Vec<u8>>>,
    /// The weight axis' range, or else the font's weight class, per font file and face.
    weights: HashMap<(InstanceId, u32), Weight>,
    next_font: u32,
    color: Mirror,
    color_free: Vec<Vec<Rect>>,
    color_glyphs: HashMap<ColorKey, ColorState>,
    text_shader: Gd<Shader>,
    color_shader: Gd<Shader>,
    /// 3D shaders without and with depth test.
    text_3d: [Gd<Shader>; 2],
    color_3d: [Gd<Shader>; 2],
    adapter: String,
    /// Labels in the tree, by node: their canvas and the glyphs they show.
    labels: HashMap<InstanceId, Registered>,
    /// The scale of each canvas with labels, as last laid out.
    canvases: HashMap<Rid, f32>,
    /// Glyphs raised by detail probes, for [`watch`].
    upgrades: Vec<GlyphKey>,
}

/// A label the server watches for it.
pub struct Registered {
    pub watch: Watch,
    pub keys: Vec<GlyphKey>,
}

/// What a label's scale depends on.
#[derive(Clone, Copy, PartialEq)]
pub enum Watch {
    /// A 2D label: its canvas (camera, `CanvasLayer`, stretch), checked once per canvas.
    Canvas(Rid),
    /// A 3D label: its distance to the camera. It is dirtied when the field level of its
    /// text (`font_size` layout pixels per em) changes, which [`world_scale`] measures.
    World {
        pixel_size: f32,
        font_size: f32,
        level: u32,
    },
}

/// Screen pixels per layout pixel of a 3D label: `pixel_size` world units, scaled by the
/// node and projected by the viewport's camera at the node's position.
pub fn world_scale(node: &Gd<Node3D>, pixel_size: f32) -> f32 {
    let t = node.get_global_transform();
    let own = t.basis.col_a().length().max(t.basis.col_b().length());
    let Some(viewport) = node.get_viewport() else {
        return pixel_size * own;
    };
    let stretch = {
        let f = viewport.get_final_transform();
        f.a.length().max(f.b.length())
    };
    let Some(camera) = viewport.get_camera_3d() else {
        return pixel_size * own * stretch;
    };
    let at = t.origin;
    if camera.is_position_behind(at) {
        return pixel_size * own * stretch;
    }
    let up = camera.get_global_transform().basis.col_b().normalized();
    let a = camera.unproject_position(at);
    let b = camera.unproject_position(at + up);
    (a - b).length() * pixel_size * own * stretch
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

enum State {
    Uninit,
    Ready(Box<Server>),
    /// No usable GPU: reported once, labels draw nothing.
    Failed,
    /// The extension is shutting down.
    Closed,
}

thread_local! {
    static STATE: RefCell<State> = const { RefCell::new(State::Uninit) };
    /// Labels to update at the end of the frame, and whether that update is scheduled.
    static DIRTY: RefCell<(Vec<InstanceId>, bool)> = const { RefCell::new((Vec::new(), false)) };
    /// Label updates so far (`Glyphux.get_stats().updates`).
    static UPDATES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Counts a label update.
pub fn count_update() {
    UPDATES.set(UPDATES.get() + 1);
}

/// Label updates so far.
pub fn updates() -> u64 {
    UPDATES.get()
}

/// Asks for `label` to be updated once at the end of this frame (before it is drawn).
pub fn mark_dirty(label: InstanceId) {
    let schedule = DIRTY.with_borrow_mut(|(list, scheduled)| {
        if !list.contains(&label) {
            list.push(label);
        }
        !std::mem::replace(scheduled, true)
    });
    if schedule && let Some(mut glyphux) = Engine::singleton().get_singleton("Glyphux") {
        glyphux.call_deferred("_update_dirty", &[]);
    }
}

/// The labels marked dirty since the last call.
pub fn take_dirty() -> Vec<InstanceId> {
    DIRTY.with_borrow_mut(|(list, scheduled)| {
        *scheduled = false;
        std::mem::take(list)
    })
}

/// The scale of the canvas `item` is drawn on (its `CanvasLayer` or camera, then the
/// window's stretch), without the item's own transform.
pub fn canvas_scale(item: &Gd<CanvasItem>) -> f32 {
    let canvas = item.get_canvas_transform();
    let screen = match item.get_viewport() {
        Some(v) => v.get_final_transform() * canvas,
        None => canvas,
    };
    screen.a.length().max(screen.b.length()).max(1e-6)
}

/// Start of a frame (`SceneTree.process_frame`): dirties the labels on canvases whose scale
/// changed and those showing glyphs a detail probe raised. One check per canvas.
pub fn watch() {
    let Some((groups, world, upgrades)) = with(|s| {
        let mut groups: HashMap<Rid, Vec<InstanceId>> = HashMap::new();
        let mut world = Vec::new();
        for (id, r) in &s.labels {
            match r.watch {
                Watch::Canvas(canvas) => groups.entry(canvas).or_default().push(*id),
                Watch::World { .. } => world.push(*id),
            }
        }
        let upgrades = std::mem::take(&mut s.upgrades);
        let dirty_upgrades: Vec<InstanceId> = if upgrades.is_empty() {
            Vec::new()
        } else {
            s.labels
                .iter()
                .filter(|(_, r)| r.keys.iter().any(|k| upgrades.contains(k)))
                .map(|(id, _)| *id)
                .collect()
        };
        (groups, world, dirty_upgrades)
    }) else {
        return;
    };
    for id in upgrades {
        mark_dirty(id);
    }
    // 3D labels: the level their text would get now.
    for id in world {
        let Ok(node) = Gd::<Node3D>::try_from_instance_id(id) else {
            continue;
        };
        let changed = with(|s| {
            let Some(Watch::World {
                pixel_size,
                font_size,
                level,
            }) = s.labels.get(&id).map(|r| r.watch)
            else {
                return false;
            };
            let now = s.atlas.level_for(f64::from(font_size * world_scale(&node, pixel_size)));
            now != level
        });
        if changed == Some(true) {
            mark_dirty(id);
        }
    }
    for (canvas, ids) in groups {
        let Some(item) = ids
            .iter()
            .find_map(|&id| Gd::<CanvasItem>::try_from_instance_id(id).ok())
        else {
            continue;
        };
        let scale = canvas_scale(&item);
        let changed = with(|s| {
            let old = s.canvases.insert(canvas, scale);
            old.is_some_and(|o| (o - scale).abs() > 1e-5 * scale)
        });
        if changed == Some(true) {
            for id in ids {
                mark_dirty(id);
            }
        }
    }
}

/// Runs `f` with the server, creating it on first use. `None` without a GPU.
pub fn with<R>(f: impl FnOnce(&mut Server) -> R) -> Option<R> {
    STATE.with_borrow_mut(|state| {
        if matches!(state, State::Uninit) {
            *state = match Server::new() {
                Ok(s) => State::Ready(Box::new(s)),
                Err(e) => {
                    godot_error!("glyphux: {e}");
                    State::Failed
                }
            };
        }
        match state {
            State::Ready(s) => Some(f(s)),
            _ => None,
        }
    })
}

/// Generates the glyphs requested this frame and mirrors them (before Godot draws it).
pub fn end_frame() {
    STATE.with_borrow_mut(|state| {
        if let State::Ready(s) = state {
            s.end_frame();
        }
    });
}

/// Releases the device and the Godot resources (extension shutdown).
pub fn shutdown() {
    let state = STATE.with_borrow_mut(|state| std::mem::replace(state, State::Closed));
    drop(state);
}

/// An OpenType axis tag as `FontVariation.variation_opentype` keys it: a number (Godot's
/// inspector, `TextServer.name_to_tag`) or the tag's four letters.
fn axis_tag(v: &Variant) -> Option<u32> {
    if let Ok(t) = v.try_to::<i64>() {
        return u32::try_from(t).ok();
    }
    let name = v.try_to::<GString>().ok()?.to_string();
    let bytes: [u8; 4] = name.as_bytes().try_into().ok()?;
    Some(u32::from_be_bytes(bytes))
}

/// A number, integer or float (a hand-written `.tres` stores `800`, the inspector `800.0`).
fn number(v: &Variant) -> Option<f64> {
    v.try_to::<f64>()
        .ok()
        .or_else(|| v.try_to::<i64>().ok().map(|i| i as f64))
}

/// Where the addon is (`glyphux.gdextension` names the same folder).
const ADDON: &str = "res://addons/glyphux";

/// The addon's shader `shaders/{name}.gdshader`.
fn load_shader(name: &str) -> Result<Gd<Shader>, String> {
    let path = format!("{ADDON}/shaders/{name}.gdshader");
    try_load::<Shader>(&path).map_err(|e| {
        format!(
            "cannot load {path} ({e}): the addon must be in {ADDON} with its shaders/ folder,              and exports must include addons/glyphux/shaders/*"
        )
    })
}

/// A table word as the shader reads it.
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
    fn new() -> Result<Self, String> {
        let config = AtlasConfig::default();
        let size = config.size;
        let atlas = GlyphAtlas::new(config.clone()).map_err(|e| e.to_string())?;
        let rd = RenderingServer::singleton()
            .get_rendering_device()
            .filter(|_| std::env::var("GLYPHUX_BACKEND").as_deref() != Ok("wgpu"));
        let rd = rd.and_then(|rd| {
            RdAtlas::new(rd, &atlas)
                .inspect_err(|e| {
                    godot_warn!("glyphux: falling back to a wgpu device of its own: {e}");
                })
                .ok()
        });
        let (backend, adapter) = if let Some(rd) = rd {
            let name = format!("{} (Godot RenderingDevice)", rd.device_name());
            (Backend::Rd(rd), name)
        } else {
            // Not OpenGL: on Godot's Compatibility renderer its context would replace Godot's.
            let (adapter, device, queue) = glyphux_wgpu::default_device_on(wgpu::Backends::PRIMARY)?;
            let gpu = AtlasTexture::new(&device, &config).map_err(|e| e.to_string())?;
            let info = adapter.get_info();
            let backend = Backend::Wgpu(Box::new(WgpuAtlas {
                device,
                queue,
                gpu,
                mirror: Mirror::new(size),
            }));
            (
                backend,
                format!("{} ({:?}, glyphux's wgpu device)", info.name, info.backend),
            )
        };
        // The `Glyphux` singleton's `_end_frame` (a method, so the connection outlives no
        // Rust closure; disconnected at shutdown).
        if let Some(glyphux) = Engine::singleton().get_singleton("Glyphux") {
            RenderingServer::singleton().connect("frame_pre_draw", &glyphux.callable("_end_frame"));
            if let Some(mut tree) = Engine::singleton()
                .get_main_loop()
                .and_then(|l| l.try_cast::<SceneTree>().ok())
            {
                tree.connect("process_frame", &glyphux.callable("_watch"));
            }
        }
        Ok(Server {
            backend,
            atlas,
            shaper: Shaper::new(),
            fonts: HashMap::new(),
            data: HashMap::new(),
            weights: HashMap::new(),
            next_font: 0,
            color: Mirror::new(COLOR_PAGE),
            color_free: Vec::new(),
            color_glyphs: HashMap::new(),
            text_shader: load_shader("text")?,
            color_shader: load_shader("color")?,
            text_3d: [load_shader("text_3d_no_depth_test")?, load_shader("text_3d")?],
            color_3d: [load_shader("color_3d_no_depth_test")?, load_shader("color_3d")?],
            adapter,
            labels: HashMap::new(),
            canvases: HashMap::new(),
            upgrades: Vec::new(),
        })
    }

    /// Records what a 2D label shows, for [`watch`]; `scale` is its canvas' scale.
    pub fn register(&mut self, label: InstanceId, canvas: Rid, scale: f32, keys: Vec<GlyphKey>) {
        self.canvases.entry(canvas).or_insert(scale);
        self.labels.insert(
            label,
            Registered {
                watch: Watch::Canvas(canvas),
                keys,
            },
        );
    }

    /// Records what a 3D label shows, for [`watch`]: drawn at `scale` screen pixels per
    /// layout pixel, `font_size` layout pixels per em.
    pub fn register_world(
        &mut self,
        label: InstanceId,
        pixel_size: f32,
        font_size: f32,
        scale: f32,
        keys: Vec<GlyphKey>,
    ) {
        let level = self.atlas.level_for(f64::from(font_size * scale));
        self.labels.insert(
            label,
            Registered {
                watch: Watch::World {
                    pixel_size,
                    font_size,
                    level,
                },
                keys,
            },
        );
    }

    pub fn unregister(&mut self, label: InstanceId) {
        if let Some(Registered {
            watch: Watch::Canvas(canvas),
            ..
        }) = self.labels.remove(&label)
            && !self.labels.values().any(|o| o.watch == Watch::Canvas(canvas))
        {
            self.canvases.remove(&canvas);
        }
    }

    /// The monochrome atlas as the labels sample it.
    pub fn texture(&self) -> Gd<TextureLayered> {
        match &self.backend {
            Backend::Rd(rd) => rd.texture().upcast(),
            Backend::Wgpu(w) => w.mirror.texture.clone().upcast(),
        }
    }

    /// Pages of the monochrome atlas on the GPU.
    pub fn page_count(&self) -> u32 {
        match &self.backend {
            Backend::Rd(rd) => rd.page_count(),
            Backend::Wgpu(w) => w.gpu.allocated_page_count(),
        }
    }

    /// The color glyph pages' mirror.
    pub fn color_texture(&self) -> Gd<Texture2DArray> {
        self.color.texture.clone()
    }

    pub fn color_page_count(&self) -> usize {
        self.color.pages.len()
    }

    pub fn text_shader(&self) -> Gd<Shader> {
        self.text_shader.clone()
    }

    pub fn color_shader(&self) -> Gd<Shader> {
        self.color_shader.clone()
    }

    /// The 3D text shader, with or without depth test.
    pub fn text_3d_shader(&self, depth_test: bool) -> Gd<Shader> {
        self.text_3d[usize::from(depth_test)].clone()
    }

    /// The 3D color glyph shader, with or without depth test.
    pub fn color_3d_shader(&self, depth_test: bool) -> Gd<Shader> {
        self.color_3d[usize::from(depth_test)].clone()
    }

    pub fn adapter(&self) -> &str {
        &self.adapter
    }

    /// The fonts to shape with: `font`, then its fallbacks, at weight `wght` (`wght` units)
    /// when given. Fonts without data (e.g. a `SystemFont`) are left out.
    pub fn fonts(&mut self, font: &Gd<Font>, wght: Option<f32>) -> Vec<FontChoice> {
        let mut out: Vec<FontChoice> = self.font(font, wght).into_iter().collect();
        for fallback in font.get_fallbacks().iter_shared() {
            out.extend(self.font(&fallback, wght));
        }
        out
    }

    /// The shaping font for a Godot `FontFile` or `FontVariation` (its variation axes and
    /// face index; Godot's embolden and transform are not applied).
    pub fn font(&mut self, font: &Gd<Font>, wght: Option<f32>) -> Option<FontChoice> {
        let mut variations = Vec::new();
        let mut index = 0;
        let mut font = font.clone();
        // A variation of a variation: the outermost setting of an axis wins.
        let file = loop {
            match font.try_cast::<FontVariation>() {
                Ok(v) => {
                    let axes = v.get_variation_opentype();
                    for (tag, value) in axes.iter_shared() {
                        let (Some(tag), Some(value)) = (axis_tag(&tag), number(&value)) else {
                            continue;
                        };
                        if !variations.iter().any(|&(t, _)| t == tag) {
                            variations.push((tag, (value as f32).to_bits()));
                        }
                    }
                    if index == 0 {
                        index = v.get_variation_face_index().max(0) as u32;
                    }
                    font = v.get_base_font()?;
                }
                Err(f) => break f.try_cast::<FontFile>().ok()?,
            }
        };
        let id = file.instance_id();
        let data = if let Some(d) = self.data.get(&id) {
            d.clone()
        } else {
            let bytes = file.get_data();
            if bytes.is_empty() {
                return None;
            }
            let d = Arc::new(bytes.to_vec());
            self.data.insert(id, d.clone());
            d
        };
        // The weight: the nearest instance at or below it on a 100 unit step, the rest drawn
        // as extra weight (as the web demo animates its title).
        let mut missing_weight = 0.0;
        if let Some(wght) = wght {
            let weight = *self.weights.entry((id, index)).or_insert_with(|| {
                let face = ttf_parser::Face::parse(&data, index).ok();
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
                    variations.retain(|&(t, _)| t != WGHT);
                    variations.push((WGHT, step.to_bits()));
                }
                Weight::Fixed(own) => missing_weight = wght - own,
            }
        }
        variations.sort_unstable();
        let key = FontKey {
            file: id,
            index,
            variations,
        };
        if let Some(f) = self.fonts.get(&key) {
            return Some(FontChoice {
                font: f.clone(),
                missing_weight,
            });
        }
        let id = self.next_font;
        let mut shaping = ShapingFont::new(id, data, key.index)?;
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
        let state = self.compile_color(font, glyph, foreground);
        self.color_glyphs.insert(key, state.clone());
        state
    }

    fn compile_color(&mut self, font: &ShapingFont, glyph: u32, foreground: [u8; 4]) -> ColorState {
        let Ok(artwork) = font.color_glyph(glyph, 0, foreground) else {
            return ColorState::None;
        };
        match self.place_color(&artwork, glyph) {
            Some(g) => ColorState::Ready(Rc::new(g)),
            None => ColorState::Failed,
        }
    }

    fn place_color(&mut self, artwork: &glyphux_wgpu::glyphux::color::ColorGlyph, glyph: u32) -> Option<ColorGlyph> {
        let field = match ColorField::build(
            artwork,
            ColorFieldConfig {
                resolution: 512,
                ..ColorFieldConfig::default()
            },
        ) {
            Ok(f) => f,
            Err(e) => {
                godot::global::print_verbose(&[
                    format!("glyphux: color glyph {glyph} left to fallback fonts: {e}").to_variant()
                ]);
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
                godot_warn!("glyphux: the color glyph pages are full");
                return None;
            }
        };
        self.color.ensure_pages(self.color_free.len());
        self.color.write(page, [rect.x, rect.y], w, field.atlas_bytes());
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

    /// Generates the glyphs requested this frame, mirrors what changed and starts the next
    /// frame.
    fn end_frame(&mut self) {
        match &mut self.backend {
            Backend::Rd(rd) => {
                rd.flush(&mut self.atlas);
            }
            Backend::Wgpu(w) => {
                let WgpuAtlas {
                    device,
                    queue,
                    gpu,
                    mirror,
                } = &mut **w;
                let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
                gpu.flush(&mut self.atlas, device, &mut encoder);
                queue.submit([encoder.finish()]);
                let written = self.atlas.take_written_rows();
                let pages = gpu.allocated_page_count() as usize;
                if !written.is_empty() || pages != mirror.texture_pages {
                    mirror.ensure_pages(pages);
                    let size = mirror.size;
                    for (layer, rows) in written {
                        let data = glyphux_wgpu::read_texture_rows(device, queue, gpu.texture(), layer, rows.clone());
                        mirror.write(layer as usize, [0, rows.start], size, &data);
                    }
                    mirror.upload();
                }
                // Frees finished staging buffers; nothing waits.
                let _ = device.poll(wgpu::PollType::Poll);
            }
        }
        if self.color.dirty.contains(&true) {
            self.color.upload();
        }
        self.atlas.begin_frame();
        let raised = self.atlas.take_upgrades();
        self.upgrades.extend(raised);
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Backend::Rd(rd) = &mut self.backend {
            rd.free();
        }
    }
}
