//! `GlyphuxLabel`: a `Node2D` that draws plain text with glyphux, its first line's baseline
//! at the node's origin, optionally wrapped or along a curve.
//!
//! It does nothing until something it shows changes: its setters, its font's and curve's
//! `changed` signals and its scale mark it dirty, and the server updates it once at the end
//! of the frame (see `server`). Changes of text, fonts or box lay the text out again; the
//! others only place its glyphs again.

use glyphux_wgpu::glyphux_gpu::paragraph::{ParagraphStyle, Span};
use glyphux_wgpu::text::Align as TextAlign;
use godot::classes::notify::CanvasItemNotification;
use godot::classes::{CanvasItem, Curve2D, Font, INode2D, Node2D};
use godot::prelude::*;

use crate::compose::{CurvePlacement, LabelCore};
use crate::server;
use crate::text_draw::GlyphStyle;

/// Horizontal alignment of the lines within `width` (or along the curve).
#[derive(GodotConvert, Var, Export, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[godot(via = i64)]
pub enum Align {
    /// Left for left-to-right lines, right for right-to-left ones.
    #[default]
    Start,
    End,
    Center,
}

impl From<Align> for TextAlign {
    fn from(a: Align) -> Self {
        match a {
            Align::Start => TextAlign::Start,
            Align::End => TextAlign::End,
            Align::Center => TextAlign::Center,
        }
    }
}

#[derive(GodotClass)]
#[class(tool, base = Node2D)]
pub struct GlyphuxLabel {
    #[export(multiline)]
    #[var(set = set_text)]
    text: GString,
    /// A `FontFile` (any imported .ttf or .otf) or a `FontVariation` of one. Its fallbacks
    /// draw what it lacks (e.g. a color emoji font).
    #[export]
    #[var(set = set_font)]
    font: Option<Gd<Font>>,
    /// Pixels per em, in the node's units.
    #[export(range = (1.0, 512.0, or_greater))]
    #[var(set = set_font_size)]
    font_size: f32,
    #[export]
    #[var(set = set_color)]
    color: Color,
    /// The font's weight (`wght` units, e.g. 400 regular, 700 bold) for variable fonts, which
    /// are instanced 100 units apart with the rest drawn as extra weight; fonts without the
    /// axis keep their own weight and draw the difference. 0: the font's own.
    #[export(range = (0.0, 1000.0, 1.0))]
    #[var(set = set_font_weight)]
    font_weight: f32,
    /// Units added to the outline (> 0 bolder, < 0 thinner).
    #[export(range = (-8.0, 8.0, 0.1, or_greater, or_less))]
    #[var(set = set_weight)]
    weight: f32,

    #[export_group(name = "Layout")]
    #[export]
    #[var(set = set_align)]
    align: Align,
    /// Width of the box the lines are aligned in (0: the widest line's).
    #[export(range = (0.0, 4096.0, or_greater))]
    #[var(set = set_width)]
    width: f32,
    /// Wraps lines at `width` (between words; within a word wider than the box).
    #[export]
    #[var(set = set_autowrap)]
    autowrap: bool,
    /// Distance between baselines, in multiples of the font's line height.
    #[export(range = (0.0, 4.0, 0.05, or_greater))]
    #[var(set = set_line_spacing)]
    line_spacing: f32,

    #[export_group(name = "Curve")]
    /// The baseline to follow, in the node's units (e.g. a `Path2D`'s curve). Lines after
    /// the first follow it a line height further out.
    #[export]
    #[var(set = set_curve)]
    curve: Option<Gd<Curve2D>>,
    /// Where the text starts along the curve, in the node's units (plus the alignment).
    #[export]
    #[var(set = set_curve_offset)]
    curve_offset: f32,
    /// Wraps the text around the curve's end (for closed curves) instead of cutting it off.
    #[export]
    #[var(set = set_curve_loop)]
    curve_loop: bool,

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
    /// In the node's units, the same for every glyph (also on curves).
    #[export]
    #[var(set = set_shadow_offset)]
    shadow_offset: Vector2,
    #[export(range = (0.0, 32.0, 0.1, or_greater))]
    #[var(set = set_shadow_blur)]
    shadow_blur: f32,

    core: LabelCore,
    /// Whether the next update lays the text out again.
    needs_layout: bool,
    base: Base<Node2D>,
}

/// The class' `#[godot_api]` block: setters that compare, store and mark the label dirty
/// (`layout` for what changes the laid out text, `redraw` for the rest), then `$items`.
macro_rules! label_api {
    ([$($name:ident: $ty:ty => $setter:ident, $what:ident;)*] $($items:tt)*) => {
        #[godot_api]
        impl GlyphuxLabel {
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

#[godot_api]
impl INode2D for GlyphuxLabel {
    fn init(base: Base<Node2D>) -> Self {
        GlyphuxLabel {
            text: GString::from("Glyphux"),
            font: None,
            font_size: 32.0,
            color: Color::WHITE,
            font_weight: 0.0,
            weight: 0.0,
            align: Align::Start,
            width: 0.0,
            autowrap: false,
            line_spacing: 1.0,
            curve: None,
            curve_offset: 0.0,
            curve_loop: false,
            outline_color: Color::BLACK,
            outline_width: 0.0,
            glow_color: Color::from_rgba(1.0, 0.6, 0.2, 0.8),
            glow_radius: 0.0,
            shadow_color: Color::from_rgba(0.0, 0.0, 0.0, 0.0),
            shadow_offset: Vector2::new(3.0, 3.0),
            shadow_blur: 2.0,
            core: LabelCore::default(),
            needs_layout: true,
            base,
        }
    }

    fn on_notification(&mut self, what: CanvasItemNotification) {
        match what {
            CanvasItemNotification::ENTER_TREE => {
                let parent = self.base().get_canvas_item();
                self.core.draw.enter(parent);
                self.base_mut().set_notify_transform(true);
                self.layout();
            }
            CanvasItemNotification::EXIT_TREE => {
                self.core.draw.exit();
                let id = self.base().instance_id();
                server::with(|s| s.unregister(id));
            }
            CanvasItemNotification::TRANSFORM_CHANGED => {
                // Moving and turning change nothing: only the scale picks fields.
                let t = self.base().get_global_transform();
                let own = t.a.length().max(t.b.length());
                if (own - self.core.node_scale).abs() > 1e-5 * own {
                    self.redraw();
                }
            }
            CanvasItemNotification::VISIBILITY_CHANGED => {
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

label_api! {
    [
    text: GString => set_text, layout;
    font_size: f32 => set_font_size, layout;
    color: Color => set_color, redraw;
    font_weight: f32 => set_font_weight, layout;
    weight: f32 => set_weight, redraw;
    align: Align => set_align, layout;
    width: f32 => set_width, layout;
    autowrap: bool => set_autowrap, layout;
    line_spacing: f32 => set_line_spacing, layout;
    curve_offset: f32 => set_curve_offset, redraw;
    curve_loop: bool => set_curve_loop, redraw;
    outline_color: Color => set_outline_color, redraw;
    outline_width: f32 => set_outline_width, redraw;
    glow_color: Color => set_glow_color, redraw;
    glow_radius: f32 => set_glow_radius, redraw;
    shadow_color: Color => set_shadow_color, redraw;
    shadow_offset: Vector2 => set_shadow_offset, redraw;
    shadow_blur: f32 => set_shadow_blur, redraw;
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
    fn set_curve(&mut self, curve: Option<Gd<Curve2D>>) {
        if self.curve == curve {
            return;
        }
        let watch = self.base().callable("_curve_changed");
        if let Some(mut old) = self.curve.take()
            && old.is_connected("changed", &watch)
        {
            old.disconnect("changed", &watch);
        }
        if let Some(new) = &curve {
            new.clone().connect("changed", &watch);
        }
        self.curve = curve;
        self.layout();
    }

    #[func]
    fn _resource_changed(&mut self) {
        self.layout();
    }

    #[func]
    fn _curve_changed(&mut self) {
        // Its length is the alignment box.
        self.layout();
    }

    /// Width and height of the laid out lines (from the first line's top to the last
    /// line's bottom), in the node's units.
    #[func]
    fn get_text_size(&mut self) -> Vector2 {
        if self.needs_layout {
            self.refresh();
        }
        Vector2::new(self.core.paragraphs.width, self.core.paragraphs.height)
    }

    /// Lays out and draws now instead of at the end of the frame (e.g. before reading
    /// `get_text_size`).
    #[func]
    fn update_now(&mut self) {
        self.refresh();
    }
}

impl GlyphuxLabel {
    /// The text, fonts or box changed.
    fn layout(&mut self) {
        self.needs_layout = true;
        self.redraw();
    }

    /// Something else changed: update at the end of the frame.
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

    /// The update the server calls for a dirty label.
    pub fn refresh(&mut self) {
        let node: Gd<CanvasItem> = self.base().clone().upcast();
        if !node.is_inside_tree() || !node.is_visible_in_tree() {
            return;
        }
        let Some(font) = self.font.clone() else {
            self.core.draw.clear();
            return;
        };
        let wght = (self.font_weight > 0.0).then_some(self.font_weight);
        let curve = self
            .curve
            .clone()
            .filter(|c| c.get_point_count() > 1)
            .map(|curve| CurvePlacement {
                offset: self.curve_offset,
                wrap: self.curve_loop,
                curve,
            });
        let needs_layout = std::mem::replace(&mut self.needs_layout, false);
        let text = self.text.to_string();
        let style = self.style();
        let paragraph = ParagraphStyle {
            width: match &curve {
                Some(c) => Some(c.curve.get_baked_length()),
                None => (self.width > 0.0).then_some(self.width),
            },
            wrap: self.autowrap && curve.is_none(),
            align: self.align.into(),
            line_spacing: self.line_spacing,
            base_rtl: None,
        };
        let size = self.font_size.max(0.01);
        let (width, align) = (self.width, self.align);
        let core = &mut self.core;
        server::with(|s| {
            if needs_layout {
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
            } else {
                core.styles = vec![style];
            }
            // The first line's baseline at the origin; without a width, the lines start at,
            // center on or end at it.
            let baseline = core.paragraphs.lines.first().map_or(0.0, |l| l.baseline);
            let x = match (curve.is_some() || width > 0.0, align) {
                (false, Align::Center) => -0.5 * core.paragraphs.width,
                (false, Align::End) => -core.paragraphs.width,
                _ => 0.0,
            };
            core.render(s, &node, Vector2::new(x, -baseline), curve.as_ref());
        });
    }
}
