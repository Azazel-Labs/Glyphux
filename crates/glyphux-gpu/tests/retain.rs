//! Retained layers are never recycled; released ones are (no GPU needed: placement only).
use glyphux_gpu::glyphux::Shape;
use glyphux_gpu::{AtlasConfig, GlyphAtlas, GlyphKey};

#[test]
fn retained_layers_survive_eviction() {
    let square = || Shape::from_description("{ 0,0; 0,1; 1,1; 1,0; # }").ok();
    let mut atlas = GlyphAtlas::new(AtlasConfig {
        size: 64,
        layers: 1,
        min_level: 32,
        max_level: 32,
        detail_probe: false,
        ..AtlasConfig::default()
    })
    .unwrap();
    let key = |glyph| GlyphKey { font: 0, glyph };
    let first = atlas.get(key(0), 64.0, square).unwrap();
    atlas.retain_layer(first.layer);
    atlas.begin_frame();
    // The only layer is full and retained: the new glyph can't be placed.
    assert!(atlas.get(key(1), 64.0, square).is_none());
    assert_eq!(atlas.stats().evicted_layers, 0);
    atlas.release_layer(first.layer);
    assert!(atlas.get(key(1), 64.0, square).is_some());
    assert_eq!(atlas.stats().evicted_layers, 1);
}
