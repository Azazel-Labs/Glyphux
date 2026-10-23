#[path = "../../glyphux/tests/support/kern_font.rs"]
mod fixture;

use glyphux_atlas::{FontGeometry, GlyphOptions, GlyphSelection};

#[test]
fn exported_pairs_include_all_aliases_and_combined_values() {
    let data = fixture::font(fixture::kern(&[(1, -40), (1, -20)]));
    let opts = GlyphOptions {
        font_scale: 2.0,
        ..GlyphOptions::default()
    };
    let font = FontGeometry::load(&data, 0, &[], &GlyphSelection::Indices(vec![1, 2, 1, 2]), &opts).unwrap();
    assert_eq!(
        font.kerning,
        vec![(0, 1, -0.12), (0, 3, -0.12), (2, 1, -0.12), (2, 3, -0.12)]
    );
    assert!(!font.kerning_incomplete);
    let disabled = GlyphOptions { kerning: false, ..opts };
    let font = FontGeometry::load(&data, 0, &[], &GlyphSelection::All, &disabled).unwrap();
    assert_eq!(font.kerning, []);
}
