//! What the widget and the component share: their settings, gathered from their properties,
//! and the update that lays out and draws them.
//!
//! Labels do nothing until something they show changes: a property (`#[on_changed]`), their
//! size on screen (enough to change the atlas field level of their text), or a glyph they
//! show getting a better field (the server tells them). Then they lay out again if the text
//! changed, request their glyphs, and give C++ what to draw if it changed.

use std::collections::HashMap;

use ferreal::prelude::*;
use glyphux_gpu::paragraph::{ParagraphStyle, Paragraphs};
use glyphux_gpu::text::Align as TextAlign;

use crate::FontFace;
use crate::bbcode;
use crate::draw::{DrawData, GlyphStyle, Rgba};
use crate::layout::{Fonts, LabelCore, plain_content, resolve_fonts, rich_content};
use crate::server::{self, LabelRef};

/// How lines are aligned in the text box (and the box against a component's origin).
#[uenum(BlueprintType, cpp_name = "EGlyphuxAlign")]
pub enum Align {
    /// Left for left-to-right paragraphs, right for right-to-left ones.
    Start,
    Center,
    End,
}

impl From<Align> for TextAlign {
    fn from(a: Align) -> Self {
        match a {
            Align::Start => TextAlign::Start,
            Align::Center => TextAlign::Center,
            Align::End => TextAlign::End,
        }
    }
}

/// Where a component's text box sits vertically against its origin.
#[uenum(BlueprintType, cpp_name = "EGlyphuxVerticalAlign")]
pub enum VerticalAlign {
    Top,
    Center,
    Bottom,
    /// The first line's baseline.
    Baseline,
}

/// A label's settings, from its properties.
pub struct Settings {
    pub text: String,
    pub rich: bool,
    pub fonts: Fonts,
    pub font_size: f32,
    pub style: GlyphStyle,
    /// The font's weight (`wght` units), `None` for the font's own.
    pub font_weight: Option<f32>,
    pub paragraph: ParagraphStyle,
    pub underline_links: bool,
}

/// A style from a label's color, weight and effect properties.
pub fn style(
    color: LinearColor,
    weight: f32,
    outline: (LinearColor, f32),
    glow: (LinearColor, f32),
    shadow: (LinearColor, Vector2, f32),
) -> GlyphStyle {
    let rgba = |c: LinearColor| -> Rgba { c.to_array() };
    GlyphStyle {
        color: rgba(color),
        weight,
        outline: Some((rgba(outline.0), outline.1)),
        glow: Some((rgba(glow.0), glow.1)),
        shadow: Some((rgba(shadow.0), [shadow.1.x as f32, shadow.1.y as f32], shadow.2)),
        ..GlyphStyle::default()
    }
}

/// What C++ is given after an update.
pub struct Update {
    /// What to draw, if it changed.
    pub draw: Option<DrawData>,
    /// The text box: its top-left corner (where the text was placed) and size.
    pub offset: [f32; 2],
    pub size: [f32; 2],
}

/// The state of a label.
#[derive(Default)]
pub struct Label {
    core: LabelCore,
    needs_layout: bool,
    /// The scale and field level of the text when last drawn.
    drawn_at: Option<(f32, u32)>,
    /// Fonts loaded by `[font=...]`.
    loaded: HashMap<String, Option<Obj<FontFace>>>,
    /// The object, once known (to unregister it).
    me: Option<LabelRef>,
}

impl Label {
    /// Something that changes the laid out text changed.
    pub fn invalidate_layout(&mut self) {
        self.needs_layout = true;
        self.drawn_at = None;
    }

    /// Whether an update at `scale` (screen pixels per unit) would draw anything new: the
    /// layout or drawing changed, or the scale changes the atlas field level of text of
    /// `font_size`.
    pub fn needs_update(&self, scale: f32, font_size: f32) -> bool {
        if self.needs_layout {
            return true;
        }
        let Some((old, level)) = self.drawn_at else {
            return true;
        };
        if (old - scale).abs() <= 1e-4 * scale {
            return false;
        }
        server::with(|s| s.atlas.level_for(f64::from(font_size * scale)) != level).unwrap_or(false)
    }

    /// Lays out (if needed) and draws `settings` at `scale`, placing the box with `place`
    /// (from the laid out paragraphs to the box's top-left). `None` before the plugin started.
    pub fn update(
        &mut self,
        me: LabelRef,
        settings: &Settings,
        scale: f32,
        place: impl FnOnce(&Paragraphs) -> [f32; 2],
    ) -> Option<Update> {
        self.me = Some(me);
        let needs_layout = std::mem::replace(&mut self.needs_layout, false);
        let core = &mut self.core;
        let loaded = &mut self.loaded;
        let result = server::with(|s| {
            s.updates += 1;
            if needs_layout {
                // Without a font, the engine's Roboto.
                let mut label_fonts = settings.fonts.clone();
                if label_fonts.normal.is_none() {
                    label_fonts.normal = s.default_font();
                }
                let content = match label_fonts.normal {
                    None => None,
                    Some(_) if settings.rich => Some(rich_content(
                        s,
                        bbcode::parse(&settings.text),
                        &label_fonts,
                        &settings.style,
                        settings.font_size,
                        settings.font_weight,
                        settings.underline_links,
                        loaded,
                    )),
                    Some(font) => Some(plain_content(
                        &settings.text,
                        font,
                        settings.font_weight,
                        &settings.style,
                        settings.font_size,
                    )),
                };
                let fonts = content
                    .as_ref()
                    .filter(|c| !c.spans.is_empty())
                    .and_then(|c| resolve_fonts(s, &c.fonts, &label_fonts.fallbacks));
                match (content, fonts) {
                    (Some(c), Some(fonts)) => {
                        core.layout(s, &c.text, &c.spans, fonts, c.styles, &settings.paragraph);
                        core.apply_aligns(&c.aligns);
                        core.metas = c.metas;
                    }
                    _ => {
                        // Nothing to draw (no font, or no text).
                        core.draw.release(s);
                        core.paragraphs = Paragraphs::default();
                        core.metas.clear();
                        s.unregister(me);
                        return Update {
                            draw: Some(DrawData::default()),
                            offset: [0.0; 2],
                            size: [0.0; 2],
                        };
                    }
                }
            }
            let offset = place(&core.paragraphs);
            let (draw, keys) = core.draw_at(s, scale, offset);
            s.register(me, keys);
            s.flush();
            Update {
                draw,
                offset,
                size: [core.paragraphs.width, core.paragraphs.height],
            }
        });
        if result.is_some() {
            let level = server::with(|s| s.atlas.level_for(f64::from(settings.font_size * scale))).unwrap_or(0);
            self.drawn_at = Some((scale, level));
        } else {
            self.needs_layout = needs_layout;
        }
        result
    }

    /// The link under `position` (layout units from the text box's top-left).
    pub fn link_at(&self, position: [f32; 2]) -> Option<String> {
        self.core.link_at(position)
    }

    /// Stops showing anything: the atlas may recycle the pages it showed.
    pub fn release(&mut self) {
        let core = &mut self.core;
        let me = self.me;
        server::with(|s| {
            core.draw.release(s);
            if let Some(me) = me {
                s.unregister(me);
            }
        });
        self.drawn_at = None;
    }
}

impl Drop for Label {
    fn drop(&mut self) {
        self.release();
    }
}
