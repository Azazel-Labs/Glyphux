//! `UGlyphuxTextComponent`: text in the world, plain or BBCode, on the plane facing the
//! component's +X axis (or facing the camera).
//!
//! Its C++ parent, `UGlyphuxTextComponentBase` (a procedural mesh component), measures every
//! frame how many screen pixels a layout pixel covers at the component's origin (its
//! `PixelSize` world units, its scale, the camera's distance) and calls `refresh_text`, which
//! does nothing unless the text changed or that scale needs another atlas field level. It
//! builds the mesh from the quads Rust gives it, `PixelSize` world units per layout pixel.

use glyphux_gpu::paragraph::ParagraphStyle;
use rivet::prelude::*;

use crate::label::{self, Align, Label, Settings, VerticalAlign};
use crate::layout::Fonts;
use crate::server::LabelRef;
use crate::{ComponentBase, ComponentSink, FontFace};

/// Text in the world drawn with Glyphux: sharp at any distance, with outline, glow and drop
/// shadow, color emoji, fallback fonts, and BBCode.
#[uclass(
    parent = "UGlyphuxTextComponentBase",
    include = "GlyphuxTextComponentBase.h",
    class_group = "Rendering",
    meta(BlueprintSpawnableComponent, DisplayName = "Glyphux Text"),
    bases(ComponentSink)
)]
pub struct GlyphuxTextComponent {
    /// The text. With Rich on, BBCode (links are not clickable in the world).
    #[uproperty(
        EditAnywhere,
        BlueprintReadWrite,
        category = "Glyphux",
        meta(MultiLine = true),
        default = "Glyphux",
        setter
    )]
    text: String,
    /// Reads Text as BBCode.
    #[uproperty(EditAnywhere, BlueprintReadWrite, category = "Glyphux", default = false, setter)]
    rich: bool,
    /// The font (none: the engine's Roboto). Characters it lacks come from the fallback
    /// fonts, in order.
    #[uproperty(EditAnywhere, BlueprintReadOnly, category = "Glyphux|Fonts")]
    font: Option<Obj<FontFace>>,
    #[uproperty(EditAnywhere, BlueprintReadOnly, category = "Glyphux|Fonts")]
    fallback_fonts: Vec<Obj<FontFace>>,
    #[uproperty(EditAnywhere, BlueprintReadOnly, category = "Glyphux|Fonts")]
    bold_font: Option<Obj<FontFace>>,
    #[uproperty(EditAnywhere, BlueprintReadOnly, category = "Glyphux|Fonts")]
    italic_font: Option<Obj<FontFace>>,
    #[uproperty(EditAnywhere, BlueprintReadOnly, category = "Glyphux|Fonts")]
    bold_italic_font: Option<Obj<FontFace>>,
    #[uproperty(EditAnywhere, BlueprintReadOnly, category = "Glyphux|Fonts")]
    mono_font: Option<Obj<FontFace>>,
    /// Layout pixels per em (Pixel Size world units each).
    #[uproperty(
        EditAnywhere,
        BlueprintReadWrite,
        category = "Glyphux",
        default = 32.0,
        meta(ClampMin = 1),
        setter
    )]
    font_size: f32,
    #[uproperty(EditAnywhere, BlueprintReadWrite, category = "Glyphux", default = [1.0, 1.0, 1.0, 1.0], setter)]
    color: LinearColor,
    /// The weight in wght units (400 regular, 700 bold), 0 for the font's own.
    #[uproperty(
        EditAnywhere,
        BlueprintReadWrite,
        category = "Glyphux",
        default = 0.0,
        meta(ClampMin = 0, ClampMax = 1000),
        setter
    )]
    font_weight: f32,
    /// Layout pixels added to the outline of the glyphs (negative: thinner).
    #[uproperty(EditAnywhere, BlueprintReadWrite, category = "Glyphux", default = 0.0, setter)]
    weight: f32,
    /// How lines align in the box, and the box against the origin.
    #[uproperty(EditAnywhere, BlueprintReadWrite, category = "Glyphux|Layout", default = Center, setter)]
    align: Align,
    #[uproperty(EditAnywhere, BlueprintReadWrite, category = "Glyphux|Layout", default = Center, setter)]
    vertical_align: VerticalAlign,
    /// Width of the box in layout pixels (0: the widest line's).
    #[uproperty(
        EditAnywhere,
        BlueprintReadWrite,
        category = "Glyphux|Layout",
        default = 0.0,
        meta(ClampMin = 0),
        setter
    )]
    width: f32,
    /// Wraps lines at Width.
    #[uproperty(
        EditAnywhere,
        BlueprintReadWrite,
        category = "Glyphux|Layout",
        default = false,
        setter
    )]
    autowrap: bool,
    #[uproperty(
        EditAnywhere,
        BlueprintReadWrite,
        category = "Glyphux|Layout",
        default = 1.0,
        meta(ClampMin = 0),
        setter
    )]
    line_spacing: f32,
    #[uproperty(EditAnywhere, BlueprintReadWrite, category = "Glyphux|Effects", default = [0.0, 0.0, 0.0, 1.0], setter)]
    outline_color: LinearColor,
    #[uproperty(
        EditAnywhere,
        BlueprintReadWrite,
        category = "Glyphux|Effects",
        default = 0.0,
        meta(ClampMin = 0),
        setter
    )]
    outline_width: f32,
    #[uproperty(EditAnywhere, BlueprintReadWrite, category = "Glyphux|Effects", default = [1.0, 0.6, 0.2, 0.8], setter)]
    glow_color: LinearColor,
    #[uproperty(
        EditAnywhere,
        BlueprintReadWrite,
        category = "Glyphux|Effects",
        default = 0.0,
        meta(ClampMin = 0),
        setter
    )]
    glow_radius: f32,
    /// Transparent: no shadow.
    #[uproperty(EditAnywhere, BlueprintReadWrite, category = "Glyphux|Effects", default = [0.0, 0.0, 0.0, 0.0], setter)]
    shadow_color: LinearColor,
    #[uproperty(EditAnywhere, BlueprintReadWrite, category = "Glyphux|Effects", default = [3.0, 3.0], setter)]
    shadow_offset: Vector2,
    #[uproperty(
        EditAnywhere,
        BlueprintReadWrite,
        category = "Glyphux|Effects",
        default = 2.0,
        meta(ClampMin = 0),
        setter
    )]
    shadow_blur: f32,

    label: Label,
}

#[uclass_impl]
impl GlyphuxTextComponent {
    #[init]
    fn start(&mut self) {
        self.label.invalidate_layout();
    }

    fn me(&self) -> LabelRef {
        LabelRef::Component(Obj::<ComponentBase>::from_raw(self.raw_object()).expect("an object"))
    }

    fn settings(&self) -> Settings {
        let font_weight = self.font_weight();
        let width = self.width();
        Settings {
            text: self.text(),
            rich: self.rich(),
            fonts: Fonts {
                normal: self.font(),
                fallbacks: self.fallback_fonts(),
                bold: self.bold_font(),
                italics: self.italic_font(),
                bold_italics: self.bold_italic_font(),
                mono: self.mono_font(),
            },
            font_size: self.font_size(),
            style: label::style(
                self.color(),
                self.weight(),
                (self.outline_color(), self.outline_width()),
                (self.glow_color(), self.glow_radius()),
                (self.shadow_color(), self.shadow_offset(), self.shadow_blur()),
            ),
            font_weight: (font_weight > 0.0).then_some(font_weight),
            paragraph: ParagraphStyle {
                width: (width > 0.0).then_some(width),
                wrap: self.autowrap(),
                align: self.align().into(),
                line_spacing: self.line_spacing(),
                base_rtl: None,
            },
            underline_links: true,
        }
    }

    /// Called by the C++ parent every frame the component is visible: `scale` screen pixels
    /// per layout pixel. Draws only when something changed.
    #[uoverride]
    fn refresh_text(&mut self, scale: f32) {
        if !self.label.needs_update(scale, self.font_size()) {
            return;
        }
        let settings = self.settings();
        let (align, vertical) = (self.align(), self.vertical_align());
        let me = self.me();
        let Some(update) = self.label.update(me, &settings, scale, |p| {
            // The box against the origin.
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
            [x, y]
        }) else {
            return;
        };
        self.set_content_box(update.offset[0], update.offset[1], update.size[0], update.size[1]);
        if let Some(d) = update.draw {
            self.set_draw_data(&d.quads, &d.records, &d.tables, crate::server::px_range(), d.animated);
        }
    }

    /// The component is hidden or unregistered: the text stops retaining atlas pages.
    #[uoverride]
    fn release_text(&mut self) {
        self.label.release();
    }

    /// Lays out again (after changing fonts from Blueprints, which have no setters).
    #[ufunction(BlueprintCallable, category = "Glyphux")]
    fn refresh(&mut self) {
        self.label.invalidate_layout();
        self.invalidate_text();
    }

    #[on_changed]
    fn changed(&mut self, _property: GlyphuxTextComponentProperty) {
        self.label.invalidate_layout();
        self.invalidate_text();
    }
}
