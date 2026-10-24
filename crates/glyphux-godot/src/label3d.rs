//! `GlyphuxLabel3D`: text in a 3D scene, a `GeometryInstance3D` like Godot's `Label3D`, with
//! plain text or BBCode.
//!
//! The same machinery as the 2D labels (`compose`, `text_draw`, the shaders' common parts):
//! the text is laid out in pixels (`font_size` per em) and drawn as a mesh in the node's
//! space, `pixel_size` world units per pixel, which is the node's own base. Like the 2D
//! labels it does nothing until something changes; its distance to the camera is watched
//! by the server, which dirties it only when its text would get another atlas field level.

use std::collections::HashMap;

use glyphux_wgpu::glyphux_gpu::paragraph::{ParagraphStyle, Span};
use godot::classes::notify::Node3DNotification;
use godot::classes::{Font, GeometryInstance3D, IGeometryInstance3D, Node3D};
use godot::prelude::*;

use crate::bbcode;
use crate::compose::{LabelCore, RichFonts, resolve_fonts, rich_content};
use crate::label::Align;
use crate::server;
use crate::text_draw::{GlyphStyle, MeshSettings, TextDraw};

/// Where the text box sits vertically against the node's origin.
#[derive(GodotConvert, Var, Export, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[godot(via = i64)]
pub enum VerticalAlign {
    Top,
    #[default]
    Center,
    Bottom,
    /// The first line's baseline.
    Baseline,
}

#[derive(GodotClass)]
#[class(tool, base = GeometryInstance3D)]
pub struct GlyphuxLabel3D {
    #[export(multiline)]
    #[var(set = set_text)]
    text: GString,
    /// Reads `text` as BBCode (as `GlyphuxRichLabel`; links are not clickable in 3D).
    #[export]
    #[var(set = set_bbcode_enabled)]
    bbcode_enabled: bool,
    /// A `FontFile` or a `FontVariation`; its fallbacks draw what it lacks.
    #[export]
    #[var(set = set_font)]
    font: Option<Gd<Font>>,
    /// Pixels per em; `pixel_size` world units per pixel.
    #[export(range = (1.0, 512.0, or_greater))]
    #[var(set = set_font_size)]
    font_size: f32,
    /// World units per pixel.
    #[export(range = (0.0001, 1.0, 0.0001, or_greater))]
    #[var(set = set_pixel_size)]
    pixel_size: f32,
    #[export]
    #[var(set = set_color)]
    color: Color,
    /// The font's weight (`wght` units), 0 for the font's own.
    #[export(range = (0.0, 1000.0, 1.0))]
    #[var(set = set_font_weight)]
    font_weight: f32,
    /// Pixels added to the outline (> 0 bolder, < 0 thinner).
    #[export(range = (-8.0, 8.0, 0.1, or_greater, or_less))]
    #[var(set = set_weight)]
    weight: f32,
    /// Faces the camera.
    #[export]
    #[var(set = set_billboard)]
    billboard: bool,
    /// Shows through closer geometry.
    #[export]
    #[var(set = set_no_depth_test)]
    no_depth_test: bool,

    #[export_group(name = "Layout")]
    /// Alignment of the lines, and of the box against the origin.
    #[export]
    #[var(set = set_align)]
    align: Align,
    #[export]
    #[var(set = set_vertical_align)]
    vertical_align: VerticalAlign,
    /// Width of the box in pixels (0: the widest line's).
    #[export(range = (0.0, 4096.0, or_greater))]
    #[var(set = set_width)]
    width: f32,
    /// Wraps lines at `width`.
    #[export]
    #[var(set = set_autowrap)]
    autowrap: bool,
    #[export(range = (0.0, 4.0, 0.05, or_greater))]
    #[var(set = set_line_spacing)]
    line_spacing: f32,

    #[export_group(name = "Outline")]
    #[export]
    #[var(set = set_outline_color)]
    outline_color: Color,
    #[export(range = (0.0, 32.0, 0.1, or_greater))]
    #[var(set = set_outline_width)]
    outline_width: f32,

    #[export_group(name = "Glow")]
    #[export]
    #[var(set = set_glow_color)]
    glow_color: Color,
    #[export(range = (0.0, 64.0, 0.1, or_greater))]
    #[var(set = set_glow_radius)]
    glow_radius: f32,

    #[export_group(name = "Shadow")]
    /// Transparent: no shadow.
    #[export]
    #[var(set = set_shadow_color)]
    shadow_color: Color,
    /// In pixels.
    #[export]
    #[var(set = set_shadow_offset)]
    shadow_offset: Vector2,
    #[export(range = (0.0, 32.0, 0.1, or_greater))]
    #[var(set = set_shadow_blur)]
    shadow_blur: f32,

    core: LabelCore,
    needs_layout: bool,
    loaded: HashMap<String, Option<Gd<Font>>>,
    base: Base<GeometryInstance3D>,
}

/// The class' `#[godot_api]` block: setters that compare, store and mark the label dirty
/// (`layout` for what changes the laid out text, `redraw` for the rest), then `$items`.
macro_rules! label3d_api {
    ([$($name:ident: $ty:ty => $setter:ident, $what:ident;)*] $($items:tt)*) => {
        #[godot_api]
        impl GlyphuxLabel3D {
            $(
                #[func]
                fn $setter(&mut self, value: $ty) {
                    if self.$name != value {
                        self.$name = value;
                        self.$what();
                    }
                }
            )*
            $($items)*
        }
    };
}

label3d_api! {
    [
        text: GString => set_text, layout;
        bbcode_enabled: bool => set_bbcode_enabled, layout;
        font_size: f32 => set_font_size, layout;
        pixel_size: f32 => set_pixel_size, redraw;
        color: Color => set_color, layout;
        font_weight: f32 => set_font_weight, layout;
        weight: f32 => set_weight, redraw;
        billboard: bool => set_billboard, redraw;
        no_depth_test: bool => set_no_depth_test, redraw;
        align: Align => set_align, layout;
        vertical_align: VerticalAlign => set_vertical_align, redraw;
        width: f32 => set_width, layout;
        autowrap: bool => set_autowrap, layout;
        line_spacing: f32 => set_line_spacing, layout;
        outline_color: Color => set_outline_color, layout;
        outline_width: f32 => set_outline_width, layout;
        glow_color: Color => set_glow_color, layout;
        glow_radius: f32 => set_glow_radius, layout;
        shadow_color: Color => set_shadow_color, layout;
        shadow_offset: Vector2 => set_shadow_offset, layout;
        shadow_blur: f32 => set_shadow_blur, layout;
    ]

    #[func]
    fn set_font(&mut self, font: Option<Gd<Font>>) {
        if self.font == font {
            return;
        }
        let watch = self.base().callable("_resource_changed");
        if let Some(mut old) = self.font.take()
            && old.is_connected("changed", &watch)
        {
            old.disconnect("changed", &watch);
        }
        if let Some(new) = &font {
            new.clone().connect("changed", &watch);
        }
        self.font = font;
        self.layout();
    }

    #[func]
    fn _resource_changed(&mut self) {
        self.layout();
    }

    /// Lays out and draws now instead of at the end of the frame.
    #[func]
    fn update_now(&mut self) {
        self.refresh();
    }
}

#[godot_api]
impl IGeometryInstance3D for GlyphuxLabel3D {
    fn init(base: Base<GeometryInstance3D>) -> Self {
        GlyphuxLabel3D {
            text: GString::from("Glyphux"),
            bbcode_enabled: false,
            font: None,
            font_size: 32.0,
            pixel_size: 0.005,
            color: Color::WHITE,
            font_weight: 0.0,
            weight: 0.0,
            billboard: false,
            no_depth_test: false,
            align: Align::Center,
            vertical_align: VerticalAlign::Center,
            width: 0.0,
            autowrap: false,
            line_spacing: 1.0,
            outline_color: Color::BLACK,
            outline_width: 0.0,
            glow_color: Color::from_rgba(1.0, 0.6, 0.2, 0.8),
            glow_radius: 0.0,
            shadow_color: Color::from_rgba(0.0, 0.0, 0.0, 0.0),
            shadow_offset: Vector2::new(3.0, 3.0),
            shadow_blur: 2.0,
            core: LabelCore::with_draw(TextDraw::mesh(MeshSettings {
                pixel_size: 0.005,
                billboard: false,
                depth_test: true,
            })),
            needs_layout: true,
            loaded: HashMap::new(),
            base,
        }
    }

    fn on_notification(&mut self, what: Node3DNotification) {
        match what {
            Node3DNotification::ENTER_TREE => {
                if let Some(mesh) = self.core.draw.mesh_rid() {
                    self.base_mut().set_base(mesh);
                }
                self.layout();
            }
            Node3DNotification::EXIT_TREE => {
                self.core.draw.exit();
                let id = self.base().instance_id();
                server::with(|s| s.unregister(id));
            }
            Node3DNotification::VISIBILITY_CHANGED => {
                if self.base().is_visible_in_tree() {
                    self.redraw();
                } else {
                    self.core.draw.clear();
                    let id = self.base().instance_id();
                    server::with(|s| s.unregister(id));
                }
            }
            _ => {}
        }
    }
}

impl GlyphuxLabel3D {
    fn layout(&mut self) {
        self.needs_layout = true;
        self.redraw();
    }

    fn redraw(&mut self) {
        if self.base().is_inside_tree() {
            server::mark_dirty(self.base().instance_id());
        }
    }

    fn style(&self) -> GlyphStyle {
        GlyphStyle {
            color: self.color,
            weight: self.weight,
            outline: Some((self.outline_color, self.outline_width)),
            glow: Some((self.glow_color, self.glow_radius)),
            shadow: Some((self.shadow_color, self.shadow_offset, self.shadow_blur)),
            ..GlyphStyle::default()
        }
    }

    /// The update the server calls when dirty.
    pub fn refresh(&mut self) {
        let node: Gd<Node3D> = self.base().clone().upcast();
        if !node.is_inside_tree() || !node.is_visible_in_tree() {
            return;
        }
        let Some(font) = self.font.clone() else {
            self.core.draw.clear();
            return;
        };
        self.core.draw.set_mesh_settings(MeshSettings {
            pixel_size: self.pixel_size,
            billboard: self.billboard,
            depth_test: !self.no_depth_test,
        });
        let needs_layout = std::mem::replace(&mut self.needs_layout, false);
        let wght = (self.font_weight > 0.0).then_some(self.font_weight);
        let style = self.style();
        let size = self.font_size.max(0.01);
        let content = if self.bbcode_enabled {
            let fonts = RichFonts {
                normal: Some(font.clone()),
                ..RichFonts::default()
            };
            Some(rich_content(
                bbcode::parse(&self.text.to_string()),
                &fonts,
                &style,
                size,
                true,
                &mut self.loaded,
            ))
        } else {
            None
        };
        let paragraph = ParagraphStyle {
            width: (self.width > 0.0).then_some(self.width),
            wrap: self.autowrap,
            align: self.align.into(),
            line_spacing: self.line_spacing,
            base_rtl: None,
        };
        let text = self.text.to_string();
        let (align, vertical, pixel_size) = (self.align, self.vertical_align, self.pixel_size);
        let scale = server::world_scale(&node, pixel_size);
        let id = node.instance_id();
        let core = &mut self.core;
        server::with(|s| {
            if needs_layout {
                if let Some(content) = content {
                    let Some(fonts) = resolve_fonts(s, &content.fonts).filter(|_| !content.spans.is_empty()) else {
                        core.draw.clear_in(s);
                        return;
                    };
                    core.layout(s, &content.text, &content.spans, fonts, content.styles, &paragraph);
                    core.apply_aligns(&content.aligns);
                } else {
                    let fonts = s.fonts(&font, wght);
                    if fonts.is_empty() {
                        core.draw.clear_in(s);
                        return;
                    }
                    let spans = [Span {
                        range: 0..text.len(),
                        fonts: 0,
                        size,
                        style: 0,
                    }];
                    core.layout(s, &text, &spans, vec![fonts], vec![style.clone()], &paragraph);
                }
            }
            // The box against the origin.
            let p = &core.paragraphs;
            let x = match align {
                Align::Start => 0.0,
                Align::Center => -0.5 * p.width,
                Align::End => -p.width,
            };
            let y = match vertical {
                VerticalAlign::Top => 0.0,
                VerticalAlign::Center => -0.5 * p.height,
                VerticalAlign::Bottom => -p.height,
                VerticalAlign::Baseline => -p.lines.first().map_or(0.0, |l| l.baseline),
            };
            let keys = core.draw_at(s, scale, Vector2::new(x, y), None);
            s.register_world(id, pixel_size, size, scale, keys);
        });
    }
}
