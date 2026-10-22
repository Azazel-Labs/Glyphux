//! Layout export in msdf-atlas-gen's JSON and CSV formats.

use std::fmt::Write;

use crate::{AtlasConfig, AtlasLayout, BoundsRect, FontGeometry, RangeSpec, YOrigin};

/// Formats a number compactly (shortest round-trip representation, no trailing `.0`).
fn num(v: f64) -> String {
    // JSON has no infinity or NaN.
    if !v.is_finite() {
        return "null".into();
    }
    if v == 0.0 {
        return "0".into();
    }
    if v.fract() == 0.0 && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    format!("{v}")
}

fn bounds_json(name: &str, b: &BoundsRect) -> String {
    format!(
        "\"{name}\":{{\"left\":{},\"bottom\":{},\"right\":{},\"top\":{}}}",
        num(b.left),
        num(b.bottom),
        num(b.right),
        num(b.top)
    )
}

/// The layout as JSON:
/// `{"atlas":{...},"metrics":{...},"glyphs":[...],"kerning":[...]}` with plane bounds in ems
/// and atlas bounds in pixels. Glyphs loaded by index use `"index"` instead of `"unicode"`.
pub fn to_json(font: &FontGeometry, layout: &AtlasLayout, cfg: &AtlasConfig) -> String {
    let y_origin = cfg.y_origin;
    let mut s = String::new();
    // Distance range in pixels (as msdf-atlas-gen reports it).
    let px_range = layout.range.width() * layout.scale;
    let middle = match cfg.range {
        RangeSpec::Pixels(r) => f64::midpoint(r.lower, r.upper),
        RangeSpec::Em(r) => f64::midpoint(r.lower, r.upper) * layout.scale,
    };
    let _ = write!(
        s,
        "{{\"atlas\":{{\"type\":\"{}\",\"distanceRange\":{},\"distanceRangeMiddle\":{},\"size\":{},\"width\":{},\"height\":{},\"yOrigin\":\"{}\"",
        cfg.atlas_type.name(),
        num(px_range),
        num(middle),
        num(layout.scale),
        layout.width,
        layout.height,
        if y_origin == YOrigin::Bottom { "bottom" } else { "top" }
    );
    if cfg.grid.is_some() {
        if let Some(p) = layout.placements.iter().flatten().next() {
            let cols = layout.width.div_ceil(p.rect.w + cfg.spacing);
            let rows = layout.height.div_ceil(p.rect.h + cfg.spacing);
            let _ = write!(
                s,
                ",\"grid\":{{\"cellWidth\":{},\"cellHeight\":{},\"columns\":{},\"rows\":{}}}",
                p.rect.w, p.rect.h, cols, rows
            );
        }
    }
    s.push('}');
    let m = &font.metrics;
    let flip = if y_origin == YOrigin::Top { -1.0 } else { 1.0 };
    let _ = write!(
        s,
        ",\"metrics\":{{\"emSize\":{},\"lineHeight\":{},\"ascender\":{},\"descender\":{},\"underlineY\":{},\"underlineThickness\":{}}}",
        num(m.em_size),
        num(m.line_height),
        num(flip * m.ascender),
        num(flip * m.descender),
        num(flip * m.underline_y),
        num(m.underline_thickness)
    );
    s.push_str(",\"glyphs\":[");
    for (i, g) in font.glyphs.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        match g.codepoint {
            Some(cp) => {
                let _ = write!(s, "{{\"unicode\":{cp}");
            }
            None => {
                let _ = write!(s, "{{\"index\":{}", g.glyph_index);
            }
        }
        let _ = write!(s, ",\"advance\":{}", num(g.advance));
        if let (Some(pb), Some(ab)) = (layout.plane_bounds(i, y_origin), layout.atlas_bounds(i, y_origin)) {
            s.push(',');
            s.push_str(&bounds_json("planeBounds", &pb));
            s.push(',');
            s.push_str(&bounds_json("atlasBounds", &ab));
        }
        s.push('}');
    }
    s.push_str("],\"kerning\":[");
    for (k, &(a, b, adv)) in font.kerning.iter().enumerate() {
        if k > 0 {
            s.push(',');
        }
        let (ga, gb) = (&font.glyphs[a], &font.glyphs[b]);
        match (ga.codepoint, gb.codepoint) {
            (Some(ca), Some(cb)) => {
                let _ = write!(s, "{{\"unicode1\":{ca},\"unicode2\":{cb},\"advance\":{}}}", num(adv));
            }
            _ => {
                let _ = write!(
                    s,
                    "{{\"index1\":{},\"index2\":{},\"advance\":{}}}",
                    ga.glyph_index,
                    gb.glyph_index,
                    num(adv)
                );
            }
        }
    }
    s.push_str("]}\n");
    s
}

/// The layout as CSV, one glyph per line:
/// `unicode-or-index,advance,planeLeft,planeBottom,planeRight,planeTop,atlasLeft,atlasBottom,atlasRight,atlasTop`
/// (bounds are 0 for glyphs without an outline).
pub fn to_csv(font: &FontGeometry, layout: &AtlasLayout, y_origin: YOrigin) -> String {
    let mut s = String::new();
    for (i, g) in font.glyphs.iter().enumerate() {
        let id = g.codepoint.unwrap_or(g.glyph_index as u32);
        let z = BoundsRect {
            left: 0.0,
            bottom: 0.0,
            right: 0.0,
            top: 0.0,
        };
        let pb = layout.plane_bounds(i, y_origin).unwrap_or(z);
        let ab = layout.atlas_bounds(i, y_origin).unwrap_or(z);
        let _ = writeln!(
            s,
            "{id},{},{},{},{},{},{},{},{},{}",
            num(g.advance),
            num(pb.left),
            num(pb.bottom),
            num(pb.right),
            num(pb.top),
            num(ab.left),
            num(ab.bottom),
            num(ab.right),
            num(ab.top)
        );
    }
    s
}

#[cfg(test)]
mod tests {
    #[test]
    fn json_numbers() {
        assert_eq!(super::num(0.0), "0");
        assert_eq!(super::num(-3.0), "-3");
        assert_eq!(super::num(0.25), "0.25");
        assert_eq!(super::num(f64::INFINITY), "null");
        assert_eq!(super::num(f64::NAN), "null");
    }
}
