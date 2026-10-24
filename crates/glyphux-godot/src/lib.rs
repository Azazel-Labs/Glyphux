//! A Godot 4 GDExtension drawing text with glyphux-gpu.
//!
//! * `GlyphuxLabel` (`Node2D`): shaped text (HarfRust, bidi) in any `FontFile` or
//!   `FontVariation`, with weight, outline, glow and drop shadow, sharp at any zoom.
//! * `Glyphux` (engine singleton): the atlas mirror and generation counters, for debugging
//!   views.
//!
//! See `server` for how the atlas reaches Godot.

mod bbcode;
mod compose;
mod label;
mod label3d;
mod rd;
mod rich;
mod server;
mod text_draw;

use godot::classes::{Engine, RenderingServer, SceneTree, Texture2DArray, TextureLayered};

use crate::label::GlyphuxLabel;
use crate::label3d::GlyphuxLabel3D;
use crate::rich::GlyphuxRichLabel;
use godot::prelude::*;

struct GlyphuxExtension;

#[gdextension]
unsafe impl ExtensionLibrary for GlyphuxExtension {
    fn on_stage_init(stage: InitStage) {
        if stage == InitStage::Scene {
            Engine::singleton().register_singleton("Glyphux", &Glyphux::new_alloc());
        }
    }

    fn on_stage_deinit(stage: InitStage) {
        if stage == InitStage::Scene {
            let mut engine = Engine::singleton();
            if let Some(singleton) = engine.get_singleton("Glyphux") {
                let end_frame = singleton.callable("_end_frame");
                let mut rs = RenderingServer::singleton();
                if rs.is_connected("frame_pre_draw", &end_frame) {
                    rs.disconnect("frame_pre_draw", &end_frame);
                }
                let watch = singleton.callable("_watch");
                if let Some(mut tree) = engine.get_main_loop().and_then(|l| l.try_cast::<SceneTree>().ok())
                    && tree.is_connected("process_frame", &watch)
                {
                    tree.disconnect("process_frame", &watch);
                }
                engine.unregister_singleton("Glyphux");
                singleton.free();
            }
            server::shutdown();
        }
    }
}

/// The `Glyphux` singleton.
#[derive(GodotClass)]
#[class(init, tool, base = Object)]
pub struct Glyphux {}

// Methods of the singleton object, which holds no state itself.
#[allow(clippy::unused_self)]
#[godot_api]
impl Glyphux {
    /// The atlas pages as Godot sees them (`sampler2DArray`, RGBA: MTSDF), e.g. for a view
    /// of the atlas filling up. `null` without a GPU.
    #[func]
    fn get_atlas_texture(&self) -> Option<Gd<TextureLayered>> {
        server::with(|s| s.texture())
    }

    /// The color glyph pages (`sampler2DArray`, RGBA: region distances), `null` without a GPU.
    #[func]
    fn get_color_texture(&self) -> Option<Gd<Texture2DArray>> {
        server::with(|s| s.color_texture())
    }

    /// Connected to `RenderingServer.frame_pre_draw` once a label needs glyphs: generates the glyphs labels requested
    /// this frame and copies them into the atlas texture.
    #[func]
    fn _end_frame(&self) {
        server::end_frame();
    }

    /// Deferred once per frame when labels are dirty: updates them.
    #[func]
    fn _update_dirty(&self) {
        // Updates can dirty others (rarely): a few rounds at most.
        for _ in 0..4 {
            let dirty = server::take_dirty();
            if dirty.is_empty() {
                break;
            }
            for id in dirty {
                server::count_update();
                if let Ok(mut label) = Gd::<GlyphuxLabel>::try_from_instance_id(id) {
                    label.bind_mut().refresh();
                } else if let Ok(mut label) = Gd::<GlyphuxRichLabel>::try_from_instance_id(id) {
                    label.bind_mut().refresh();
                } else if let Ok(mut label) = Gd::<GlyphuxLabel3D>::try_from_instance_id(id) {
                    label.bind_mut().refresh();
                }
            }
        }
    }

    /// Connected to `SceneTree.process_frame`: dirties labels whose canvas scale changed or
    /// whose glyphs a detail probe raised.
    #[func]
    fn _watch(&self) {
        server::watch();
    }

    /// Generation counters and the atlas' size: `adapter`, `pages`, `max_pages`,
    /// `page_size`, `generated`, `generated_texels`, `probes`, `probe_upgrades`,
    /// `evicted_pages`, `failed`, `color_pages`, and `updates`: how many times labels have
    /// updated (they update only when something they show changed).
    #[func]
    fn get_stats(&self) -> VarDictionary {
        let mut d = VarDictionary::new();
        server::with(|s| {
            let stats = s.atlas.stats();
            let config = s.atlas.config();
            d.set("adapter", s.adapter());
            d.set("pages", s.page_count());
            d.set("max_pages", config.layers);
            d.set("page_size", config.size);
            d.set("generated", stats.generated as i64);
            d.set("generated_texels", stats.generated_texels as i64);
            d.set("probes", stats.probes as i64);
            d.set("probe_upgrades", stats.probe_upgrades as i64);
            d.set("evicted_pages", stats.evicted_layers as i64);
            d.set("failed", stats.failed as i64);
            d.set("color_pages", s.color_page_count() as i64);
            d.set("updates", server::updates() as i64);
        });
        d
    }
}
