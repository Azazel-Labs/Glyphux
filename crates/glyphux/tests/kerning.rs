#![cfg(feature = "font")]

#[path = "support/kern_font.rs"]
mod fixture;

use glyphux::io::font::{Font, FontScaling};

fn check(data: &[u8], expected: f64) {
    for (scaling, scale) in [
        (FontScaling::None, 1.0),
        (FontScaling::EmNormalized, 0.001),
        (FontScaling::Legacy, 1.0 / 64.0),
    ] {
        let font = Font::from_bytes(data, 0, scaling).unwrap();
        assert_eq!(font.kerning(1, 2), expected * scale);
        assert_eq!(font.kerning(2, 1), 0.0);
        let pairs = if expected == 0.0 {
            vec![]
        } else {
            vec![(1, 2, expected * scale)]
        };
        assert_eq!(font.kerning_pairs(), (pairs, false));
    }
}

#[test]
fn additive_override_and_zero_pairs() {
    check(&fixture::font(fixture::kern(&[(1, -40), (1, -20)])), -60.0);
    check(&fixture::font(fixture::kern(&[(1, -40), (9, -20), (1, 5)])), -15.0);
    check(&fixture::font(fixture::kern(&[(1, -40), (9, 0)])), 0.0);
    check(&fixture::font(fixture::kern(&[(1, -40), (1, 40)])), 0.0);
}

#[test]
fn excludes_vertical_cross_stream_and_minimum_distance() {
    check(
        &fixture::font(fixture::kern(&[(0, -100), (5, -200), (3, 300), (1, -25)])),
        -25.0,
    );
}

#[test]
fn skips_unknown_formats_and_accepts_legacy_single_subtable_length() {
    check(&fixture::font(fixture::kern(&[(0x0401, -200), (1, -25)])), -25.0);
    let mut kern = fixture::kern(&[(1, -40)]);
    // Some fonts overflow this 16-bit field for a large, single format-0 table.
    kern[6..8].copy_from_slice(&0u16.to_be_bytes());
    check(&fixture::font(kern), -40.0);
}

#[test]
fn truncated_subtables_are_safe() {
    let kern = fixture::kern(&[(1, -40), (1, -20)]);
    for end in 0..kern.len() {
        let expected = if end >= 24 { -40.0 } else { 0.0 };
        check(&fixture::font(kern[..end].to_vec()), expected);
    }
}

#[test]
fn apple_subtables_accumulate_and_ignore_cross_stream_and_variations() {
    let mut kern = vec![0, 1, 0, 0, 0, 0, 0, 5];
    for (coverage, value) in [(0u8, -40i16), (0, -20), (0x80, -100), (0x40, -200), (0x20, -300)] {
        kern.extend(22u32.to_be_bytes());
        kern.extend([coverage, 0, 0, 0]);
        for word in [1, 6, 0, 0, 1, 2, value as u16] {
            kern.extend(word.to_be_bytes());
        }
    }
    check(&fixture::font(kern), -60.0);
}

#[test]
fn class_based_subtables_combine_with_explicit_pairs() {
    let mut kern = fixture::kern(&[(1, -40)]);
    kern[2..4].copy_from_slice(&2u16.to_be_bytes());
    // Format 2: one left class and one right class, with a -20 adjustment.
    for word in [0u16, 28, 0x0201, 2, 14, 20, 26, 1, 1, 26, 2, 1, 0, (-20i16) as u16] {
        kern.extend(word.to_be_bytes());
    }
    let bytes = fixture::font(kern);
    let font = Font::from_bytes(&bytes, 0, FontScaling::None).unwrap();
    assert_eq!(font.kerning(1, 2), -60.0);
    assert_eq!(font.kerning_pairs(), (vec![(1, 2, -40.0)], true));
}
