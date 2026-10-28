//! `UGlyphuxTextBlock`: a UMG widget drawing text with Glyphux, plain or BBCode, wrapped to
//! its width, with links.
//!
//! Its C++ parent, `UGlyphuxTextBlockBase`, owns the Slate widget: when the widget is painted
//! at a new size or scale, or after `invalidate_text`, it calls `refresh_text` here, and
//! draws the quads Rust gives it with the plugin's UI material.

use glyphux_gpu::paragraph::ParagraphStyle;
use rivet::prelude::*;

use crate::label::{self, Align, Label, Settings};
use crate::layout::Fonts;
use crate::server::LabelRef;
use crate::{BlockBase, BlockSink, FontFace};

/// Text drawn with Glyphux: sharp at any size, with outline, glow and drop shadow, color
/// emoji, fallback fonts, and BBCode.
#[uclass(
    parent = "UGlyphuxTextBlockBase",
    include = "GlyphuxTextBlockBase.h",
    meta(DisplayName = "Glyphux Text"),
    bases(BlockSink)
)]
pub struct GlyphuxTextBlock {
    /// The text. With Rich on, BBCode: [b] [i] [u] [s] [code] [color=c] [font_size=n]
    /// [font=/Game/...] [outline_size=n] [outline_color=c] [glow] [shadow] [url] [wave] [shake]
    /// [rainbow] [left] [center] [right] [lb] [rb].
    #[uproperty(
        EditAnywhere,
        BlueprintReadWrite,
        category = "Glyphux",
        meta(MultiLine = true),
        default = "[b]Glyphux[/b] text",
        setter
    )]
    text: String,
    /// Reads Text as BBCode.
    #[uproperty(EditAnywhere, BlueprintReadWrite, category = "Glyphux", default = true, setter)]
    rich: bool,
    /// The font (none: the engine's Roboto). Characters it lacks come from the fallback
    /// fonts, in order (a color emoji font, say).
    #[uproperty(EditAnywhere, BlueprintReadOnly, category = "Glyphux|Fonts")]
    font: Option<Obj<FontFace>>,
    #[uproperty(EditAnywhere, BlueprintReadOnly, category = "Glyphux|Fonts")]
    fallback_fonts: Vec<Obj<FontFace>>,
    /// For [b]; without one, Font at weight 700.
    #[uproperty(EditAnywhere, BlueprintReadOnly, category = "Glyphux|Fonts")]
    bold_font: Option<Obj<FontFace>>,
    /// For [i]; without one, Font slanted.
    #[uproperty(EditAnywhere, BlueprintReadOnly, category = "Glyphux|Fonts")]
    italic_font: Option<Obj<FontFace>>,
    #[uproperty(EditAnywhere, BlueprintReadOnly, category = "Glyphux|Fonts")]
    bold_italic_font: Option<Obj<FontFace>>,
    /// For [code]; without one, Font.
    #[uproperty(EditAnywhere, BlueprintReadOnly, category = "Glyphux|Fonts")]
    mono_font: Option<Obj<FontFace>>,
    /// Slate units per em.
    #[uproperty(
        EditAnywhere,
        BlueprintReadWrite,
        category = "Glyphux",
        default = 24.0,
        meta(ClampMin = 1),
        setter
    )]
    font_size: f32,
    #[uproperty(EditAnywhere, BlueprintReadWrite, category = "Glyphux", default = [1.0, 1.0, 1.0, 1.0], setter)]
    color: LinearColor,
    /// The weight in wght units (400 regular, 700 bold), 0 for the font's own. A variable font
    /// is drawn from instances 100 units apart with the rest added as extra weight, so the
    /// weight can animate smoothly.
    #[uproperty(
        EditAnywhere,
        BlueprintReadWrite,
        category = "Glyphux",
        default = 0.0,
        meta(ClampMin = 0, ClampMax = 1000),
        setter
    )]
    font_weight: f32,
    /// Units added to the outline of the glyphs (negative: thinner).
    #[uproperty(EditAnywhere, BlueprintReadWrite, category = "Glyphux", default = 0.0, setter)]
    weight: f32,
    #[uproperty(EditAnywhere, BlueprintReadWrite, category = "Glyphux|Layout", default = Start, setter)]
    align: Align,
    /// Wraps lines at the widget's width.
    #[uproperty(
        EditAnywhere,
        BlueprintReadWrite,
        category = "Glyphux|Layout",
        default = true,
        setter
    )]
    autowrap: bool,
    /// Multiplies the fonts' line height.
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
    /// How far the glow fades out; 0 for none.
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
    /// Underlines [url] links.
    #[uproperty(EditAnywhere, BlueprintReadWrite, category = "Glyphux", default = true, setter)]
    underline_links: bool,

    label: Label,
    /// The width the text was wrapped at.
    width: f32,
}

#[uclass_impl]
impl GlyphuxTextBlock {
    #[init]
    fn start(&mut self) {
        self.label.invalidate_layout();
    }

    fn me(&self) -> LabelRef {
        LabelRef::Block(Obj::<BlockBase>::from_raw(self.raw_object()).expect("an object"))
    }

    fn settings(&self) -> Settings {
        let font_weight = self.font_weight();
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
                width: (self.autowrap() && self.width > 0.0).then_some(self.width),
                wrap: self.autowrap(),
                align: self.align().into(),
                line_spacing: self.line_spacing(),
                base_rtl: None,
            },
            underline_links: self.underline_links(),
        }
    }

    /// Called by the C++ parent before painting, when the text is dirty or the widget was
    /// resized or rescaled: `scale` screen pixels per Slate unit, `width` the widget's width.
    #[uoverride]
    fn refresh_text(&mut self, scale: f32, width: f32) {
        if self.autowrap() && (width - self.width).abs() > 0.01 {
            self.width = width;
            self.label.invalidate_layout();
        }
        if !self.label.needs_update(scale, self.font_size()) {
            return;
        }
        let settings = self.settings();
        let me = self.me();
        let Some(update) = self.label.update(me, &settings, scale, |_| [0.0, 0.0]) else {
            return;
        };
        self.set_content_size(update.size[0], update.size[1]);
        if let Some(d) = update.draw {
            self.set_draw_data(&d.quads, &d.records, &d.tables, crate::server::px_range(), d.animated);
        }
    }

    /// The link (`[url=...]`, or the link's text) at `position`, in Slate units from the
    /// widget's top-left; empty where there is none.
    #[uoverride(const)]
    fn link_at(&self, position: Vector2) -> String {
        self.label
            .link_at([position.x as f32, position.y as f32])
            .unwrap_or_default()
    }

    /// The Slate widget is released: the text stops retaining atlas pages.
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

    /// The designer changed properties.
    #[uoverride]
    fn synchronize_properties(&mut self) {
        self.label.invalidate_layout();
        self.invalidate_text();
    }

    #[on_changed]
    fn changed(&mut self, _property: GlyphuxTextBlockProperty) {
        self.label.invalidate_layout();
        self.invalidate_text();
    }
}
