//! `GlyphuxRichLabel`: a `Control` that draws BBCode (see `bbcode`) with glyphux, wrapped
//! to its width, with links.
//!
//! The same machinery as `GlyphuxLabel` (`compose`, `text_draw`): the text becomes spans
//! with their own fonts, sizes and styles, laid out by glyphux-gpu's paragraph layout, and
//! nothing runs until a property, a font, the control's size or scale changes. Animated
//! tags (`wave`, `shake`, `rainbow`) move in the shader.

use std::collections::HashMap;

use glyphux_wgpu::glyphux_gpu::paragraph::ParagraphStyle;
use godot::classes::notify::ControlNotification;
use godot::classes::{CanvasItem, Control, Font, IControl, InputEvent, InputEventMouseButton, InputEventMouseMotion};
use godot::global::MouseButton;
use godot::prelude::*;

use crate::bbcode;
use crate::compose::{LabelCore, RichFonts, resolve_fonts, rich_content};
use crate::label::Align;
use crate::server;
use crate::text_draw::GlyphStyle;

#[derive(GodotClass)]
#[class(tool, base = Control)]
pub struct GlyphuxRichLabel {
    /// BBCode: Godot `RichTextLabel`'s tags (`b`, `i`, `u`, `s`, `code`, `color`,
    /// `font_size`, `font`, `outline_size`, `outline_color`, `url`, `wave`, `shake`,
    /// `rainbow`, `left`, `center`, `right`, `lb`, `rb`) and `glow`, `shadow`.
    #[export(multiline)]
    #[var(set = set_text)]
    text: GString,
    /// The normal font; its fallbacks draw what it lacks (e.g. color emoji).
    #[export]
    #[var(set = set_font)]
    font: Option<Gd<Font>>,
    /// For `[b]`; without one, the normal font at weight 700.
    #[export]
    #[var(set = set_bold_font)]
    bold_font: Option<Gd<Font>>,
    /// For `[i]`; without one, the normal font slanted.
    #[export]
    #[var(set = set_italics_font)]
    italics_font: Option<Gd<Font>>,
    #[export]
    #[var(set = set_bold_italics_font)]
    bold_italics_font: Option<Gd<Font>>,
    /// For `[code]`; without one, the normal font.
    #[export]
    #[var(set = set_mono_font)]
    mono_font: Option<Gd<Font>>,
    #[export(range = (1.0, 512.0, or_greater))]
    #[var(set = set_font_size)]
    font_size: f32,
    #[export]
    #[var(set = set_color)]
    color: Color,
    /// Alignment of paragraphs outside alignment tags.
    #[export]
    #[var(set = set_align)]
    align: Align,
    /// Wraps lines at the control's width.
    #[export]
    #[var(set = set_autowrap)]
    autowrap: bool,
    /// Makes the control's minimum height that of its text.
    #[export]
    #[var(set = set_fit_content)]
    fit_content: bool,
    #[export(range = (0.0, 4.0, 0.05, or_greater))]
    #[var(set = set_line_spacing)]
    line_spacing: f32,
    /// Underlines links.
    #[export]
    #[var(set = set_meta_underlined)]
    meta_underlined: bool,

    core: LabelCore,
    /// Each style's link.
    metas: Vec<Option<String>>,
    /// The link under the mouse.
    hovered: Option<String>,
    /// Fonts loaded by `[font=...]`.
    loaded: HashMap<String, Option<Gd<Font>>>,
    base: Base<Control>,
}

/// The class' `#[godot_api]` block: setters that compare, store and mark the label dirty,
/// font setters that also watch the font's `changed` signal, then `$items`.
macro_rules! rich_api {
    ([$($name:ident: $ty:ty => $setter:ident;)*] [$($font:ident => $font_setter:ident;)*] $($items:tt)*) => {
        #[godot_api]
        impl GlyphuxRichLabel {
            $(
                #[func]
                fn $setter(&mut self, value: $ty) {
                    if self.$name != value {
                        self.$name = value;
                        self.dirty();
                    }
                }
            )*
            $(
                #[func]
                fn $font_setter(&mut self, value: Option<Gd<Font>>) {
                    if self.$font != value {
                        let old = std::mem::replace(&mut self.$font, value.clone());
                        self.watch(old, value);
                        self.dirty();
                    }
                }
            )*
            $($items)*
        }
    };
}

#[godot_api]
impl IControl for GlyphuxRichLabel {
    fn init(base: Base<Control>) -> Self {
        GlyphuxRichLabel {
            text: GString::from("[b]Glyphux[/b] rich text"),
            font: None,
            bold_font: None,
            italics_font: None,
            bold_italics_font: None,
            mono_font: None,
            font_size: 24.0,
            color: Color::WHITE,
            align: Align::Start,
            autowrap: true,
            fit_content: false,
            line_spacing: 1.0,
            meta_underlined: true,
            core: LabelCore::default(),
            metas: Vec::new(),
            hovered: None,
            loaded: HashMap::new(),
            base,
        }
    }

    fn on_notification(&mut self, what: ControlNotification) {
        match what {
            ControlNotification::ENTER_TREE => {
                let parent = self.base().get_canvas_item();
                self.core.draw.enter(parent);
                self.base_mut().set_notify_transform(true);
                self.dirty();
            }
            ControlNotification::EXIT_TREE => {
                self.core.draw.exit();
                let id = self.base().instance_id();
                server::with(|s| s.unregister(id));
            }
            ControlNotification::RESIZED => {
                if self.autowrap {
                    self.dirty();
                }
            }
            ControlNotification::TRANSFORM_CHANGED => {
                let t = self.base().get_global_transform();
                let own = t.a.length().max(t.b.length());
                if (own - self.core.node_scale).abs() > 1e-5 * own {
                    self.dirty();
                }
            }
            ControlNotification::VISIBILITY_CHANGED => {
                if self.base().is_visible_in_tree() {
                    self.dirty();
                } else {
                    self.core.draw.clear();
                    let id = self.base().instance_id();
                    server::with(|s| s.unregister(id));
                }
            }
            _ => {}
        }
    }

    fn get_minimum_size(&self) -> Vector2 {
        if self.fit_content {
            Vector2::new(0.0, self.core.paragraphs.height)
        } else {
            Vector2::ZERO
        }
    }

    fn gui_input(&mut self, event: Gd<InputEvent>) {
        if let Ok(click) = event.clone().try_cast::<InputEventMouseButton>() {
            if click.is_pressed()
                && click.get_button_index() == MouseButton::LEFT
                && let Some(meta) = self.meta_at(click.get_position())
            {
                self.base_mut().emit_signal("meta_clicked", &[meta.to_variant()]);
                self.base_mut().accept_event();
            }
        } else if let Ok(motion) = event.try_cast::<InputEventMouseMotion>() {
            let meta = self.meta_at(motion.get_position());
            if meta != self.hovered {
                if let Some(old) = self.hovered.take() {
                    self.base_mut().emit_signal("meta_hover_ended", &[old.to_variant()]);
                }
                if let Some(new) = &meta {
                    self.base_mut().emit_signal("meta_hover_started", &[new.to_variant()]);
                }
                let shape = if meta.is_some() {
                    godot::classes::control::CursorShape::POINTING_HAND
                } else {
                    godot::classes::control::CursorShape::ARROW
                };
                self.base_mut().set_default_cursor_shape(shape);
                self.hovered = meta;
            }
        }
    }
}

rich_api! {
    [
        text: GString => set_text;
        font_size: f32 => set_font_size;
        color: Color => set_color;
        align: Align => set_align;
        autowrap: bool => set_autowrap;
        fit_content: bool => set_fit_content;
        line_spacing: f32 => set_line_spacing;
        meta_underlined: bool => set_meta_underlined;
    ]
    [
        font => set_font;
        bold_font => set_bold_font;
        italics_font => set_italics_font;
        bold_italics_font => set_bold_italics_font;
        mono_font => set_mono_font;
    ]
    /// A link was clicked: its `[url=meta]` (or the link's text).
    #[signal]
    fn meta_clicked(meta: Variant);
    #[signal]
    fn meta_hover_started(meta: Variant);
    #[signal]
    fn meta_hover_ended(meta: Variant);

    #[func]
    fn _resource_changed(&mut self) {
        self.dirty();
    }

    /// Height of the laid out text.
    #[func]
    fn get_content_height(&mut self) -> f32 {
        self.refresh();
        self.core.paragraphs.height
    }

    /// Lays out and draws now instead of at the end of the frame.
    #[func]
    fn update_now(&mut self) {
        self.refresh();
    }
}

impl GlyphuxRichLabel {
    fn dirty(&mut self) {
        if self.base().is_inside_tree() {
            server::mark_dirty(self.base().instance_id());
        }
    }

    fn watch(&mut self, old: Option<Gd<Font>>, new: Option<Gd<Font>>) {
        let callable = self.base().callable("_resource_changed");
        if let Some(mut old) = old
            && old.is_connected("changed", &callable)
        {
            old.disconnect("changed", &callable);
        }
        if let Some(mut new) = new
            && !new.is_connected("changed", &callable)
        {
            new.connect("changed", &callable);
        }
    }

    /// The link under `position` (control coordinates).
    fn meta_at(&self, position: Vector2) -> Option<String> {
        let p = &self.core.paragraphs;
        let line = p
            .lines
            .iter()
            .find(|l| position.y >= l.top && position.y < l.top + l.height)?;
        let glyph = p.glyphs[line.glyphs.clone()]
            .iter()
            .find(|g| position.x >= g.x && position.x < g.x + g.advance)?;
        self.metas.get(glyph.style as usize).cloned().flatten()
    }

    /// The update the server calls when dirty.
    pub fn refresh(&mut self) {
        let node: Gd<CanvasItem> = self.base().clone().upcast();
        if !node.is_inside_tree() || !node.is_visible_in_tree() {
            return;
        }
        let parsed = bbcode::parse(&self.text.to_string());
        let fonts = RichFonts {
            normal: self.font.clone(),
            bold: self.bold_font.clone(),
            italics: self.italics_font.clone(),
            bold_italics: self.bold_italics_font.clone(),
            mono: self.mono_font.clone(),
        };
        let base = GlyphStyle {
            color: self.color,
            ..GlyphStyle::default()
        };
        let content = rich_content(
            parsed,
            &fonts,
            &base,
            self.font_size,
            self.meta_underlined,
            &mut self.loaded,
        );
        let width = self.base().get_size().x;
        let paragraph = ParagraphStyle {
            width: Some(width.max(1.0)),
            wrap: self.autowrap,
            align: self.align.into(),
            line_spacing: self.line_spacing,
            base_rtl: None,
        };
        let core = &mut self.core;
        let laid = server::with(|s| {
            let Some(fonts) = resolve_fonts(s, &content.fonts).filter(|_| !content.spans.is_empty()) else {
                core.draw.clear_in(s);
                return false;
            };
            core.layout(s, &content.text, &content.spans, fonts, content.styles, &paragraph);
            core.apply_aligns(&content.aligns);
            core.render(s, &node, Vector2::ZERO, None);
            true
        });
        self.metas = content.metas;
        if laid == Some(true) && self.fit_content {
            self.base_mut().update_minimum_size();
        }
    }
}
