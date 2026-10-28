//! The Rust half of the Glyphux Unreal Engine plugin.
//!
//! The text classes are written here, with Rivet: `UGlyphuxTextBlock` (a UMG widget) and
//! `UGlyphuxTextComponent` (a scene component drawing text in the world). Their properties
//! are `UPROPERTY`s on the Unreal objects; laying out the text, the glyph atlas, color glyphs
//! and the glyph records the materials read are Rust. The C++ half (`../Source/Glyphux`) does
//! what only the engine can: it runs glyphux-gpu's compute kernels in Unreal's render graph
//! (`UGlyphuxSubsystem`), and draws the quads Rust describes (Slate custom vertices for the
//! widget, a procedural mesh for the component), with the plugin's materials.
//!
//! * `server`: the state every label shares (atlas, fonts, color glyph pages).
//! * `bbcode`: the rich text markup.
//! * `layout`: text, fonts and styles laid out into positioned glyphs.
//! * `draw`: positioned glyphs into quads and glyph records.
//! * `label`: what both classes share; `block` and `component` are the classes.

mod bbcode;
mod block;
mod component;
mod draw;
mod label;
mod layout;
mod records;
mod server;

use rivet::prelude::*;

rivet::module!();

engine_class!(pub FontFace = "UFontFace" in "Engine/FontFace.h");
engine_class!(pub Subsystem = "UGlyphuxSubsystem" in "GlyphuxSubsystem.h", bases(Gpu));
engine_class!(pub BlockBase = "UGlyphuxTextBlockBase" in "GlyphuxTextBlockBase.h", bases(BlockSink));
engine_class!(pub ComponentBase = "UGlyphuxTextComponentBase" in "GlyphuxTextComponentBase.h", bases(ComponentSink));

/// What the engine subsystem does for Rust: runs the atlas batches on the GPU, keeps the
/// color glyph pages, and reads fonts.
#[cpp_base(class = "UGlyphuxSubsystem", include = "GlyphuxSubsystem.h")]
pub trait Gpu {
    /// Starts a batch for an atlas of `pages` pages.
    fn begin_batch(&self, pages: u32);
    /// The contents of input buffer `binding` (`Binding::index`).
    fn batch_input(&self, binding: u32, bytes: &[u8]);
    /// The size of output buffer `binding`, zeroed.
    fn batch_output(&self, binding: u32, size: u32);
    /// A dispatch of kernel `kernel` (its index in `Kernel::ALL`).
    fn batch_dispatch(&self, kernel: u32, x: u32, y: u32);
    /// Texel rows `start` to `end` of atlas page `layer`, which the batch writes.
    fn batch_rows(&self, layer: u32, start: u32, end: u32);
    /// Runs the batch (in the render graph, before the frame draws).
    fn submit_batch(&self);
    /// Copies RGBA8 texels into a color glyph page.
    fn write_color_rows(&self, page: u32, x: u32, y: u32, width: u32, height: u32, bytes: &[u8]);
    /// A name of the font that stays the same while it exists (its path).
    fn font_path(&self, font: Obj<FontFace>) -> String;
    /// The font file's contents (empty if they cannot be read).
    fn font_bytes(&self, font: Obj<FontFace>) -> Vec<u8>;
    /// The font face asset at `path` (`[font=/Game/...]`), loaded.
    fn load_font(&self, path: &str) -> Option<Obj<FontFace>>;
}

/// What the widget's C++ base does with what Rust lays out.
#[cpp_base(class = "UGlyphuxTextBlockBase", include = "GlyphuxTextBlockBase.h")]
pub trait BlockSink {
    /// Replaces what the widget draws: quads (`records::QUAD_FLOATS` floats each), monochrome
    /// glyph records (texels of four floats, `RECORD_TEXELS` per glyph and `RECORDS_PER_ROW`
    /// glyphs per row of the records texture), color glyph tables (`TABLES_WIDTH` texels per
    /// row).
    fn set_draw_data(&self, quads: &[f32], records: &[f32], tables: &[f32], px_range: f32, animated: bool);
    /// The laid out text's size, in layout units (the widget's desired size).
    fn set_content_size(&self, width: f32, height: f32);
    /// Asks for `RefreshText` before the widget is next drawn.
    fn invalidate_text(&self);
}

/// What the component's C++ base does with what Rust lays out (as `BlockSink`).
#[cpp_base(class = "UGlyphuxTextComponentBase", include = "GlyphuxTextComponentBase.h")]
pub trait ComponentSink {
    fn set_draw_data(&self, quads: &[f32], records: &[f32], tables: &[f32], px_range: f32, animated: bool);
    /// The text box against the component's origin: its top-left corner and size, in layout
    /// units.
    fn set_content_box(&self, x: f32, y: f32, width: f32, height: f32);
    fn invalidate_text(&self);
}

/// `Rivet::Glyphux::Init`: the subsystem started.
#[export]
pub fn init(subsystem: Obj<Subsystem>) {
    server::init(subsystem);
}

/// `Rivet::Glyphux::Tick`: once per frame, before the labels refresh: starts the atlas'
/// frame and asks labels showing glyphs whose fields were refined to refresh.
#[export]
pub fn tick() {
    server::tick();
}

/// `Rivet::Glyphux::Flush`: generates the glyphs requested since the last flush.
#[export]
pub fn flush() {
    server::flush();
}

/// `Rivet::Glyphux::Shutdown`: the subsystem stops; the atlas is dropped.
#[export]
pub fn shutdown() {
    server::shutdown();
}

/// `Rivet::Glyphux::Stats`: the atlas' counters, for debugging.
#[export]
pub fn stats() -> String {
    server::stats()
}
