//! What both labels draw with: glyphs placed in the node's units, each with its own style,
//! turned into two meshes on canvas items of their own: monochrome glyphs with underlines
//! and strikethroughs, then color glyphs.
//!
//! Every glyph's record goes into a small float texture that the material reads (see
//! `shaders/`): its `GlyphInstance` in the node's units, or where its color tables are, and
//! its animation. Lengths stay in node units (the shader turns them into pixels), so a zoom
//! only changes which atlas fields are used: [`TextDraw::draw`] rebuilds nothing when no
//! record changed. The atlas pages of the glyphs shown are retained, so they are never
//! recycled while shown.

use std::ops::Range;
use std::rc::Rc;

use glyphux_wgpu::units::px;
use glyphux_wgpu::{AtlasGlyph, GlyphInstance, GlyphKey, TextStyle, Units};
use godot::classes::image::Format;
use godot::classes::mesh::{ArrayCustomFormat, ArrayFormat, ArrayType, PrimitiveType};
use godot::classes::{ArrayMesh, Image, ImageTexture, RenderingServer, Shader, ShaderMaterial, TextureLayered};
use godot::prelude::*;

use crate::server::{self, ColorGlyph, FontChoice, Server};

/// Glyph records per row of the monochrome data texture, and texels per record.
const GLYPHS_PER_ROW: usize = 64;
const GLYPH_TEXELS: usize = 12;
/// Texels per row of the color data texture, and per color glyph record.
const TABLE_WIDTH: usize = 256;
const COLOR_TEXELS: usize = 4;

/// How glyphs look, in the node's units.
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphStyle {
    pub color: Color,
    /// Units added to the outline (> 0 bolder).
    pub weight: f32,
    /// Color and width.
    pub outline: Option<(Color, f32)>,
    /// Color and radius.
    pub glow: Option<(Color, f32)>,
    /// Color, offset and blur.
    pub shadow: Option<(Color, Vector2, f32)>,
    /// Slant of synthetic italics (x offset per unit above the baseline).
    pub slant: f32,
    pub underline: bool,
    pub strike: bool,
    /// Amplitude (units) and frequency (Hz) of a vertical wave.
    pub wave: Option<(f32, f32)>,
    /// Amplitude (units) and rate (jumps per second).
    pub shake: Option<(f32, f32)>,
    /// Speed (cycles per second), saturation, value and spatial frequency of a rainbow fill.
    pub rainbow: Option<(f32, f32, f32, f32)>,
}

impl Default for GlyphStyle {
    fn default() -> Self {
        GlyphStyle {
            color: Color::WHITE,
            weight: 0.0,
            outline: None,
            glow: None,
            shadow: None,
            slant: 0.0,
            underline: false,
            strike: false,
            wave: None,
            shake: None,
            rainbow: None,
        }
    }
}

impl GlyphStyle {
    /// glyphux's style at `size` units per em, `extra_weight` em bolder (weight the font
    /// instance lacks).
    fn text_style(&self, size: f32, extra_weight: f32) -> TextStyle {
        let rgba = |c: Color| [c.r, c.g, c.b, c.a];
        let mut style =
            TextStyle::new(px(size.max(0.01)), rgba(self.color)).weight(px(self.weight + extra_weight * size));
        if let Some((c, w)) = self.outline.filter(|(c, w)| *w > 0.0 && c.a > 0.0) {
            style = style.outline(rgba(c), px(w));
        }
        if let Some((c, r)) = self.glow.filter(|(c, r)| *r > 0.0 && c.a > 0.0) {
            style = style.glow(rgba(c), px(r));
        }
        if let Some((c, o, b)) = self.shadow.filter(|(c, ..)| c.a > 0.0) {
            style = style.shadow(rgba(c), [px(o.x), px(o.y)], px(b));
        }
        style
    }

    fn animation(&self) -> [[f32; 4]; 2] {
        let (wa, wf) = self.wave.unwrap_or((0.0, 0.0));
        let (sa, sr) = self.shake.unwrap_or((0.0, 0.0));
        let (rs, rsat, rv, rf) = self.rainbow.unwrap_or((0.0, 0.0, 0.0, 0.0));
        [[wa, wf, sa, sr], [rs, rsat, rv, rf]]
    }
}

/// One glyph to draw.
pub struct Glyph<'a> {
    pub font: &'a FontChoice,
    /// Its id in its font.
    pub id: u32,
    /// Its origin (baseline, pen position) in node units.
    pub origin: [f32; 2],
    /// Its pen position and advance along its line (layout units: before any curve).
    pub pen: f32,
    pub advance: f32,
    /// Units per em.
    pub size: f32,
    /// Turned by an angle about a pivot (node units), e.g. along a curve.
    pub rotation: Option<(f32, [f32; 2])>,
    pub style: &'a GlyphStyle,
}

/// An underline or a strikethrough along a run of glyphs. It is drawn in their style, as a
/// piece under each glyph that moves with it (along a curve, with wave and shake).
pub struct Decoration<'a> {
    /// The glyphs (indices in [`TextDraw::draw`]'s).
    pub glyphs: Range<usize>,
    /// Where it starts and ends along the line (layout units, as [`Glyph::pen`]).
    pub x: [f32; 2],
    /// Its center below the baseline (layout units, y down) and its thickness.
    pub y: f32,
    pub thickness: f32,
    /// Drawn over the glyphs (strikethrough) instead of under them (underline).
    pub over: bool,
    pub style: &'a GlyphStyle,
}

/// One of the meshes: a canvas item of its own under the node's, its mesh and material, and
/// the data texture of its glyph records.
#[derive(Default)]
struct Layer {
    item: Option<Rid>,
    mesh: Option<Gd<ArrayMesh>>,
    material: Option<Gd<ShaderMaterial>>,
    data: Option<Gd<ImageTexture>>,
}

impl Layer {
    fn enter(&mut self, parent: Rid) {
        let mut rs = RenderingServer::singleton();
        let item = rs.canvas_item_create();
        rs.canvas_item_set_parent(item, parent);
        self.item = Some(item);
    }

    /// Releases everything: Godot keeps a default instance of node classes until after the
    /// rendering server is gone, so nothing may outlive the tree.
    fn exit(&mut self) {
        if let Some(item) = self.item.take() {
            RenderingServer::singleton().free_rid(item);
        }
        *self = Layer::default();
    }

    fn clear(&self) {
        if let Some(item) = self.item {
            RenderingServer::singleton().canvas_item_clear(item);
        }
    }

    fn material(&mut self, shader: &Gd<Shader>) -> Gd<ShaderMaterial> {
        self.material
            .get_or_insert_with(|| {
                let mut m = ShaderMaterial::new_gd();
                m.set_shader(shader);
                m
            })
            .clone()
    }

    /// Sets the data texture to `texels` RGBA32F texels, `width` per row.
    fn set_data(&mut self, texels: &[[f32; 4]], width: usize) -> Gd<ImageTexture> {
        set_texels(&mut self.data, texels, width)
    }

    /// Replaces the mesh with `quads`.
    fn set_mesh(&mut self, quads: &Quads) {
        let Some(item) = self.item else {
            return;
        };
        let mut rs = RenderingServer::singleton();
        rs.canvas_item_clear(item);
        let mesh = self.mesh.get_or_insert_with(ArrayMesh::new_gd);
        mesh.clear_surfaces();
        if quads.corners.is_empty() {
            return;
        }
        let arrays = quad_arrays(quads, None);
        mesh.add_surface_from_arrays_ex(PrimitiveType::TRIANGLES, &arrays)
            .flags(custom_flags())
            .done();
        if let Some(material) = &self.material {
            rs.canvas_item_set_material(item, material.get_rid());
        }
        rs.canvas_item_add_mesh(item, mesh.get_rid());
    }
}

/// A monochrome glyph: its record and its quad's corners.
#[derive(Clone, PartialEq)]
struct Mono {
    record: [[f32; 4]; GLYPH_TEXELS],
    corners: [Vector2; 4],
}

/// A color glyph: its compiled glyph, record and corners.
#[derive(Clone)]
struct ColorQuad {
    glyph: Rc<ColorGlyph>,
    record: [[f32; 4]; 4],
    corners: [Vector2; 4],
}

impl PartialEq for ColorQuad {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.glyph, &other.glyph) && self.record == other.record && self.corners == other.corners
    }
}

/// The corners of `rect` (node units: min x, min y, max x, max y) slanted about the baseline
/// at `baseline_y` and turned by `rotation`.
fn corners(rect: [f32; 4], baseline_y: f32, slant: f32, rotation: Option<(f32, [f32; 2])>) -> [Vector2; 4] {
    let [x0, y0, x1, y1] = rect;
    [[x0, y0], [x1, y0], [x0, y1], [x1, y1]].map(|[x, y]| {
        let x = x + slant * (baseline_y - y);
        let (x, y) = match rotation {
            None => (x, y),
            Some((a, [px, py])) => {
                let (s, c) = a.sin_cos();
                let (dx, dy) = (x - px, y - py);
                (px + c * dx - s * dy, py + s * dx + c * dy)
            }
        };
        Vector2::new(x, y)
    })
}

/// How a 3D label's mesh is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeshSettings {
    /// World units per layout pixel.
    pub pixel_size: f32,
    /// Faces the camera.
    pub billboard: bool,
    /// Hidden behind closer geometry (off: shows through).
    pub depth_test: bool,
}

/// Where a label draws. (One per label, never moved: the size difference is fine.)
#[allow(clippy::large_enum_variant)]
enum Target {
    /// Canvas items under the node's: monochrome glyphs (and decorations), color glyphs.
    Canvas { mono: Layer, colors: Layer },
    /// One mesh in the node's space (3D), a surface per kind.
    Mesh(Box<MeshTarget>),
}

struct MeshTarget {
    mesh: Gd<ArrayMesh>,
    settings: MeshSettings,
    mono_material: Option<Gd<ShaderMaterial>>,
    color_material: Option<Gd<ShaderMaterial>>,
    mono_data: Option<Gd<ImageTexture>>,
    color_data: Option<Gd<ImageTexture>>,
}

/// The meshes of one label.
pub struct TextDraw {
    target: Target,
    /// What is drawn now.
    drawn: Option<(Vec<Mono>, Vec<ColorQuad>)>,
    /// Atlas pages retained for the glyphs shown.
    retained: Vec<u32>,
}

impl Default for TextDraw {
    fn default() -> Self {
        TextDraw {
            target: Target::Canvas {
                mono: Layer::default(),
                colors: Layer::default(),
            },
            drawn: None,
            retained: Vec::new(),
        }
    }
}

/// The texels of monochrome glyph records.
fn mono_texels(mono: &[Mono]) -> Vec<[f32; 4]> {
    mono.iter().flat_map(|m| m.record).collect()
}

/// The texels of color glyph records, then of each distinct glyph's tables.
fn color_texels(colors: &[ColorQuad]) -> Vec<[f32; 4]> {
    let n = colors.len();
    let mut records = Vec::with_capacity(COLOR_TEXELS * n);
    let mut tables: Vec<[f32; 4]> = Vec::new();
    let mut placed: Vec<(*const ColorGlyph, [f32; 4])> = Vec::new();
    for q in colors {
        let ptr = Rc::as_ptr(&q.glyph);
        let bases = if let Some((_, b)) = placed.iter().find(|(p, _)| *p == ptr) {
            *b
        } else {
            let at = |tables: &Vec<[f32; 4]>| (COLOR_TEXELS * n + tables.len()) as f32;
            let g = &q.glyph;
            let tiles = at(&tables);
            tables.extend_from_slice(&g.tiles);
            let paints = at(&tables);
            tables.extend_from_slice(&g.paints);
            let stops = at(&tables);
            tables.extend_from_slice(&g.stops);
            // Recipe words: four per texel.
            let recipes = at(&tables) * 4.0;
            tables.extend_from_slice(&g.recipes);
            let b = [tiles, paints, stops, recipes];
            placed.push((ptr, b));
            b
        };
        records.push(bases);
        records.extend_from_slice(&q.record[1..]);
    }
    records.extend(tables);
    records
}

/// Quads of four vertices: corners (node units), uvs, and their record's index and drawing
/// pass (CUSTOM0.x and y).
#[derive(Default)]
struct Quads {
    corners: Vec<[Vector2; 4]>,
    uvs: Vec<[Vector2; 4]>,
    custom: Vec<[f32; 2]>,
}

/// Passes of monochrome glyphs (glyphux's `text_pass`): every glyph's shadow and glow, then
/// every outline, then every fill, so that no glyph's effects cover a neighbor.
const PASS_EFFECTS: f32 = 0.0;
const PASS_OUTLINE: f32 = 1.0;
const PASS_FILL: f32 = 2.0;

/// The quads of monochrome records, in passes: those with a shadow or glow, those with an
/// outline, then all.
fn mono_quads(mono: &[Mono]) -> Quads {
    let mut q = Quads::default();
    let effects = |m: &Mono| (m.record[5][3] > 0.0 && m.record[4][2] > 0.0) || m.record[7][3] > 0.0;
    let outline = |m: &Mono| m.record[3][3] > 0.0 && m.record[4][1] > 0.0;
    for (pass, wanted) in [
        (PASS_EFFECTS, &effects as &dyn Fn(&Mono) -> bool),
        (PASS_OUTLINE, &outline),
        (PASS_FILL, &|_: &Mono| true),
    ] {
        for (i, m) in mono.iter().enumerate().filter(|(_, m)| wanted(m)) {
            let [u0, v0, u1, v1] = m.record[1];
            q.corners.push(m.corners);
            q.uvs.push([
                Vector2::new(u0, v0),
                Vector2::new(u1, v0),
                Vector2::new(u0, v1),
                Vector2::new(u1, v1),
            ]);
            q.custom.push([i as f32, pass]);
        }
    }
    q
}

fn color_quads(colors: &[ColorQuad]) -> Quads {
    Quads {
        corners: colors.iter().map(|c| c.corners).collect(),
        uvs: vec![COLOR_Q; colors.len()],
        custom: (0..colors.len()).map(|i| [i as f32, 0.0]).collect(),
    }
}

/// The quad position (-1 to 1) of each corner of a color glyph.
const COLOR_Q: [Vector2; 4] = [
    Vector2::new(-1.0, -1.0),
    Vector2::new(1.0, -1.0),
    Vector2::new(-1.0, 1.0),
    Vector2::new(1.0, 1.0),
];

/// Mesh arrays of `quads`: corners in layout units, y down (in 3D, `pixel_size` world units
/// per unit with y up).
fn quad_arrays(quads: &Quads, pixel_size: Option<f32>) -> VarArray {
    let mut arrays = VarArray::new();
    arrays.resize(ArrayType::MAX.ord() as usize, &Variant::nil());
    let points = quads.corners.iter().flatten();
    let vertices = match pixel_size {
        None => PackedVector2Array::from_iter(points.copied()).to_variant(),
        Some(ps) => PackedVector3Array::from_iter(points.map(|p| Vector3::new(p.x * ps, -p.y * ps, 0.0))).to_variant(),
    };
    arrays.set(ArrayType::VERTEX.ord() as usize, &vertices);
    let uv: Vec<Vector2> = quads.uvs.iter().flatten().copied().collect();
    arrays.set(
        ArrayType::TEX_UV.ord() as usize,
        &PackedVector2Array::from(uv.as_slice()).to_variant(),
    );
    let colors = vec![Color::WHITE; 4 * quads.corners.len()];
    arrays.set(
        ArrayType::COLOR.ord() as usize,
        &PackedColorArray::from(colors.as_slice()).to_variant(),
    );
    let custom: Vec<f32> = quads
        .custom
        .iter()
        .flat_map(|&[i, pass]| std::iter::repeat_n([i, pass, 0.0, 0.0], 4).flatten())
        .collect();
    arrays.set(
        ArrayType::CUSTOM0.ord() as usize,
        &PackedFloat32Array::from(custom.as_slice()).to_variant(),
    );
    let indices: Vec<i32> = (0..quads.corners.len() as i32)
        .flat_map(|q| {
            let b = 4 * q;
            [b, b + 1, b + 2, b + 2, b + 1, b + 3]
        })
        .collect();
    arrays.set(
        ArrayType::INDEX.ord() as usize,
        &PackedInt32Array::from(indices.as_slice()).to_variant(),
    );
    arrays
}

/// Four floats per vertex in CUSTOM0.
fn custom_flags() -> ArrayFormat {
    let rgba_float = u64::from(ArrayCustomFormat::RGBA_FLOAT.ord() as u32);
    ArrayFormat::from_ord(rgba_float << ArrayFormat::CUSTOM0_SHIFT.ord())
}

fn set_texels(texture: &mut Option<Gd<ImageTexture>>, texels: &[[f32; 4]], width: usize) -> Gd<ImageTexture> {
    let rows = texels.len().div_ceil(width).max(1);
    let mut bytes = Vec::with_capacity(rows * width * 16);
    for t in texels {
        for v in t {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
    }
    bytes.resize(rows * width * 16, 0);
    let image = Image::create_from_data(
        width as i32,
        rows as i32,
        false,
        Format::RGBAF,
        &PackedByteArray::from(bytes.as_slice()),
    )
    .expect("an RGBAF image");
    let texture = texture.get_or_insert_with(ImageTexture::new_gd);
    texture.set_image(&image);
    texture.clone()
}

/// The records of decoration `d`'s pieces, one per glyph, appended to `mono`.
///
/// A decoration has no field in the atlas: the shader computes the exact distance to its
/// box (record 6: the box's half size; record 9: layer -1). Its texture coordinates are
/// layout units from the box's center, and its distance range is wide enough for its
/// effects, so it takes the glyphs' outline, glow, shadow and animation as they are.
fn decoration_pieces(d: &Decoration<'_>, glyphs: &[Glyph<'_>], scale: f32, mono: &mut Vec<Mono>) {
    let size = glyphs[d.glyphs.clone()].iter().map(|g| g.size).fold(0.0, f32::max);
    let rs = d.style.text_style(size, 0.0).resolve(&Units::new(1.0));
    // How far the effects reach, and a pixel to antialias the edge.
    let pad = rs.reach() + 1.0 / scale.max(1e-6);
    let field = AtlasGlyph {
        layer: 0,
        uv_min: [0.0; 2],
        uv_max: [0.0; 2],
        plane_min: [0.0; 2],
        plane_max: [0.0; 2],
        level: 0,
        px_range: 2.0 * (rs.reach() + 2.0),
    };
    let q = rs.instance(&field, [0.0, 0.0]);
    let half = [0.5 * (d.x[1] - d.x[0]), 0.5 * d.thickness];
    let center = f32::midpoint(d.x[0], d.x[1]);
    let animation = d.style.animation();
    for index in d.glyphs.clone() {
        let g = &glyphs[index];
        // Its stretch of the line (the line's ends padded for the effects).
        let from = if g.pen <= d.x[0] + 1e-4 { d.x[0] - pad } else { g.pen };
        let to = if g.pen + g.advance >= d.x[1] - 1e-4 {
            d.x[1] + pad
        } else {
            g.pen + g.advance
        };
        if to <= from {
            continue;
        }
        let (top, bottom) = (-half[1] - pad, half[1] + pad);
        let [ox, oy] = g.origin;
        let rect = [ox + from - g.pen, oy + d.y + top, ox + to - g.pen, oy + d.y + bottom];
        mono.push(Mono {
            record: [
                rect,
                [from - center, top, to - center, bottom],
                q.color,
                q.outline,
                q.params,
                q.glow,
                [half[0], half[1], 0.0, 0.0],
                q.shadow,
                q.shadow_params,
                [-1.0, ox, index as f32, 0.0],
                animation[0],
                animation[1],
            ],
            corners: corners(rect, oy, 0.0, g.rotation),
        });
    }
}

impl TextDraw {
    /// Draws into a mesh (3D) instead of canvas items: see [`Self::mesh_rid`].
    pub fn mesh(settings: MeshSettings) -> Self {
        TextDraw {
            target: Target::Mesh(Box::new(MeshTarget {
                mesh: ArrayMesh::new_gd(),
                settings,
                mono_material: None,
                color_material: None,
                mono_data: None,
                color_data: None,
            })),
            drawn: None,
            retained: Vec::new(),
        }
    }

    /// The mesh of a 3D label.
    pub fn mesh_rid(&self) -> Option<Rid> {
        match &self.target {
            Target::Mesh(m) => Some(m.mesh.get_rid()),
            Target::Canvas { .. } => None,
        }
    }

    /// Changes a 3D label's settings (redrawn on the next draw).
    pub fn set_mesh_settings(&mut self, settings: MeshSettings) {
        if let Target::Mesh(m) = &mut self.target
            && m.settings != settings
        {
            if m.settings.depth_test != settings.depth_test {
                m.mono_material = None;
                m.color_material = None;
            }
            m.settings = settings;
            self.drawn = None;
        }
    }

    /// Creates the canvas items under `parent` (the node's), in drawing order.
    pub fn enter(&mut self, parent: Rid) {
        if let Target::Canvas { mono, colors } = &mut self.target {
            mono.enter(parent);
            colors.enter(parent);
        }
        self.drawn = None;
    }

    /// Frees the canvas items (or empties the mesh) and releases the retained pages.
    pub fn exit(&mut self) {
        self.release();
        match &mut self.target {
            Target::Canvas { mono, colors } => {
                mono.exit();
                colors.exit();
            }
            Target::Mesh(m) => m.mesh.clear_surfaces(),
        }
        self.drawn = None;
    }

    /// Draws nothing (and lets the atlas recycle the pages it showed).
    pub fn clear(&mut self) {
        self.release();
        self.clear_target();
    }

    /// [`Self::clear`] from inside `server::with`.
    pub fn clear_in(&mut self, s: &mut Server) {
        for l in std::mem::take(&mut self.retained) {
            s.atlas.release_layer(l);
        }
        self.clear_target();
    }

    fn clear_target(&mut self) {
        match &mut self.target {
            Target::Canvas { mono, colors } => {
                mono.clear();
                colors.clear();
            }
            Target::Mesh(m) => m.mesh.clear_surfaces(),
        }
        self.drawn = None;
    }

    fn release(&mut self) {
        let layers = std::mem::take(&mut self.retained);
        if !layers.is_empty() {
            server::with(|s| {
                for l in layers {
                    s.atlas.release_layer(l);
                }
            });
        }
    }

    /// Requests the glyphs at `scale` screen pixels per unit and draws them (rebuilding only
    /// what changed). Returns the monochrome glyphs shown, for upgrade notices.
    pub fn draw(
        &mut self,
        s: &mut Server,
        glyphs: &[Glyph<'_>],
        decorations: &[Decoration<'_>],
        scale: f32,
    ) -> Vec<GlyphKey> {
        let local = Units::new(1.0);
        let mut mono: Vec<Mono> = Vec::new();
        let mut colors: Vec<ColorQuad> = Vec::new();
        let mut keys = Vec::new();
        let mut layers: Vec<u32> = Vec::new();
        for d in decorations.iter().filter(|d| !d.over) {
            decoration_pieces(d, glyphs, scale, &mut mono);
        }
        for (index, g) in glyphs.iter().enumerate() {
            let c = g.style.color;
            let foreground = [c.r, c.g, c.b, c.a].map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
            let [ox, oy] = g.origin;
            let animation = g.style.animation();
            if let Some(color) = s.color_glyph(&g.font.font, g.id, foreground) {
                let [min, max] = color.plane;
                let rect = [
                    ox + min[0] * g.size,
                    oy - max[1] * g.size,
                    ox + max[0] * g.size,
                    oy - min[1] * g.size,
                ];
                // Too small on screen for its tiles.
                if (rect[2] - rect[0]) * scale < color.config[3] {
                    continue;
                }
                colors.push(ColorQuad {
                    record: [
                        [0.0; 4],
                        color.config,
                        [color.page as f32, c.a, ox, index as f32],
                        animation[0],
                    ],
                    corners: corners(rect, oy, g.style.slant, g.rotation),
                    glyph: color,
                });
                continue;
            }
            let key = GlyphKey {
                font: g.font.font.id,
                glyph: g.id,
            };
            let rs = g
                .style
                .text_style(g.size, 0.0001 * g.font.missing_weight)
                .resolve(&local);
            let (size, reach) = (f64::from(g.size * scale), f64::from(rs.reach() * scale));
            let Some(placed) = s.atlas.get_with_reach(key, size, reach, || g.font.font.outline(g.id)) else {
                continue;
            };
            if !placed.is_visible() {
                continue;
            }
            keys.push(key);
            if !layers.contains(&placed.layer) {
                layers.push(placed.layer);
            }
            let q: GlyphInstance = rs.instance(&placed, g.origin);
            mono.push(Mono {
                record: [
                    q.rect,
                    q.uv,
                    q.color,
                    q.outline,
                    q.params,
                    q.glow,
                    [0.0; 4],
                    q.shadow,
                    q.shadow_params,
                    [q.layer as f32, ox, index as f32, 0.0],
                    animation[0],
                    animation[1],
                ],
                corners: corners(q.rect, oy, g.style.slant, g.rotation),
            });
        }
        for d in decorations.iter().filter(|d| d.over) {
            decoration_pieces(d, glyphs, scale, &mut mono);
        }
        // Retain before releasing: shared pages stay retained throughout.
        for &l in &layers {
            s.atlas.retain_layer(l);
        }
        for l in std::mem::replace(&mut self.retained, layers) {
            s.atlas.release_layer(l);
        }
        let unchanged = self.drawn.as_ref().is_some_and(|(m, c)| *m == mono && *c == colors);
        if !unchanged {
            match &mut self.target {
                Target::Canvas {
                    mono: mono_layer,
                    colors: color_layer,
                } => {
                    draw_canvas_mono(s, mono_layer, &mono);
                    draw_canvas_colors(s, color_layer, &colors);
                }
                Target::Mesh(m) => m.draw(s, &mono, &colors),
            }
            self.drawn = Some((mono, colors));
        }
        keys
    }
}

fn draw_canvas_mono(s: &mut Server, layer: &mut Layer, mono: &[Mono]) {
    if mono.is_empty() {
        layer.set_mesh(&Quads::default());
        return;
    }
    let mut material = layer.material(&s.text_shader());
    let data = layer.set_data(&mono_texels(mono), GLYPHS_PER_ROW * GLYPH_TEXELS);
    let atlas: Gd<TextureLayered> = s.texture();
    material.set_shader_parameter("gx_atlas", &atlas.to_variant());
    material.set_shader_parameter("gx_glyphs", &data.to_variant());
    material.set_shader_parameter("gx_px_range", &(s.atlas.config().px_range as f32).to_variant());
    material.set_shader_parameter("gx_true_distance", &1.0f32.to_variant());
    layer.set_mesh(&mono_quads(mono));
}

fn draw_canvas_colors(s: &mut Server, layer: &mut Layer, colors: &[ColorQuad]) {
    if colors.is_empty() {
        layer.set_mesh(&Quads::default());
        return;
    }
    let mut material = layer.material(&s.color_shader());
    let data = layer.set_data(&color_texels(colors), TABLE_WIDTH);
    material.set_shader_parameter("gx_pages", &s.color_texture().to_variant());
    material.set_shader_parameter("gx_tables", &data.to_variant());
    layer.set_mesh(&color_quads(colors));
}

impl MeshTarget {
    /// Rebuilds the mesh: a surface of monochrome glyphs (and decorations), one of color
    /// glyphs, whichever there are.
    fn draw(&mut self, s: &mut Server, mono: &[Mono], colors: &[ColorQuad]) {
        self.mesh.clear_surfaces();
        let ps = Some(self.settings.pixel_size);
        let (billboard, depth_test) = (self.settings.billboard, self.settings.depth_test);
        let pixel_size = self.settings.pixel_size;
        let common = move |m: &mut Gd<ShaderMaterial>| {
            m.set_shader_parameter("gx_pixel_size", &pixel_size.to_variant());
            m.set_shader_parameter("gx_billboard", &billboard.to_variant());
        };
        if !mono.is_empty() {
            let shader = s.text_3d_shader(depth_test);
            let mut material = self
                .mono_material
                .get_or_insert_with(|| {
                    let mut m = ShaderMaterial::new_gd();
                    m.set_shader(&shader);
                    m
                })
                .clone();
            let data = set_texels(&mut self.mono_data, &mono_texels(mono), GLYPHS_PER_ROW * GLYPH_TEXELS);
            material.set_shader_parameter("gx_atlas", &s.texture().to_variant());
            material.set_shader_parameter("gx_glyphs", &data.to_variant());
            material.set_shader_parameter("gx_px_range", &(s.atlas.config().px_range as f32).to_variant());
            material.set_shader_parameter("gx_true_distance", &1.0f32.to_variant());
            common(&mut material);
            let arrays = quad_arrays(&mono_quads(mono), ps);
            self.add_surface(&arrays, &material.upcast());
        }
        if !colors.is_empty() {
            let shader = s.color_3d_shader(depth_test);
            let mut material = self
                .color_material
                .get_or_insert_with(|| {
                    let mut m = ShaderMaterial::new_gd();
                    m.set_shader(&shader);
                    m
                })
                .clone();
            let data = set_texels(&mut self.color_data, &color_texels(colors), TABLE_WIDTH);
            material.set_shader_parameter("gx_pages", &s.color_texture().to_variant());
            material.set_shader_parameter("gx_tables", &data.to_variant());
            common(&mut material);
            let arrays = quad_arrays(&color_quads(colors), ps);
            self.add_surface(&arrays, &material.upcast());
        }
    }

    fn add_surface(&mut self, arrays: &VarArray, material: &Gd<godot::classes::Material>) {
        self.mesh
            .add_surface_from_arrays_ex(PrimitiveType::TRIANGLES, arrays)
            .flags(custom_flags())
            .done();
        let surface = self.mesh.get_surface_count() - 1;
        self.mesh.surface_set_material(surface, material);
    }
}
