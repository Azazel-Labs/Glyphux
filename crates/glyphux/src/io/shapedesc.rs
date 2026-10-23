//! msdfgen-compatible text shape description.
//!
//! ```text
//! @y-up
//! { -1, -1; m; -1, +1; y; +1, +1; m; +1, -1; y; # }
//! { 0, 1; (+1.6, -0.8; -1.6, -0.8); # }
//! ```
//! Each contour is enclosed in braces; points are `x, y` separated by `;`. Between two points
//! there may be an edge specification: a color (`c`, `m`, `y`, `w`, `r`, `g`, `b`) and/or one
//! or two control points in parentheses. `#` closes the contour back to its first point.

use std::fmt::Write;

use crate::edge::{Edge, EdgeColor, Segment};
use crate::math::Vec2;
use crate::shape::{Contour, Shape};

/// Shape description parse error.
#[derive(Clone, Debug, PartialEq)]
pub struct ParseError {
    pub message: String,
    pub position: usize,
}

impl core::fmt::Display for ParseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{} (at byte {})", self.message, self.position)
    }
}

impl std::error::Error for ParseError {}

fn err<T>(message: impl Into<String>, position: usize) -> Result<T, ParseError> {
    Err(ParseError {
        message: message.into(),
        position,
    })
}

enum Item {
    Point(Vec2),
    Close,
    Spec {
        color: Option<EdgeColor>,
        controls: Vec<Vec2>,
    },
}

fn parse_point(s: &str, pos: usize) -> Result<Vec2, ParseError> {
    let mut parts = s.split(',');
    let (Some(x), Some(y), None) = (parts.next(), parts.next(), parts.next()) else {
        return err(format!("expected a point 'x, y', found '{}'", s.trim()), pos);
    };
    // Only finite numbers: Rust also parses "inf", "NaN" and overflowing exponents.
    let number = |s: &str| match s.trim().parse::<f64>() {
        Ok(v) if v.is_finite() => Ok(v),
        _ => err(format!("invalid number '{}'", s.trim()), pos),
    };
    Ok(Vec2::new(number(x)?, number(y)?))
}

fn color_of(c: char) -> Option<EdgeColor> {
    Some(match c.to_ascii_lowercase() {
        'c' => EdgeColor::CYAN,
        'm' => EdgeColor::MAGENTA,
        'y' => EdgeColor::YELLOW,
        'w' => EdgeColor::WHITE,
        'r' => EdgeColor::RED,
        'g' => EdgeColor::GREEN,
        'b' => EdgeColor::BLUE,
        'k' => EdgeColor::BLACK,
        _ => return None,
    })
}

fn parse_item(raw: &str, pos: usize) -> Result<Item, ParseError> {
    let s = raw.trim();
    if s == "#" {
        return Ok(Item::Close);
    }
    let first = s.chars().next().unwrap_or(' ');
    if first == '(' || first.is_ascii_alphabetic() {
        let mut rest = s;
        let mut color = None;
        if let Some(c) = rest.chars().next().filter(char::is_ascii_alphabetic) {
            color = Some(color_of(c).ok_or_else(|| ParseError {
                message: format!("unknown edge color '{c}'"),
                position: pos,
            })?);
            rest = rest[c.len_utf8()..].trim_start();
        }
        let mut controls = Vec::new();
        if !rest.is_empty() {
            if !(rest.starts_with('(') && rest.ends_with(')')) {
                return err(format!("invalid edge specification '{s}'"), pos);
            }
            let inner = &rest[1..rest.len() - 1];
            for p in inner.split(';') {
                if !p.trim().is_empty() {
                    controls.push(parse_point(p, pos)?);
                }
            }
            if controls.is_empty() || controls.len() > 2 {
                return err("an edge must have one or two control points", pos);
            }
        }
        return Ok(Item::Spec { color, controls });
    }
    Ok(Item::Point(parse_point(s, pos)?))
}

impl Shape {
    /// Parses msdfgen's text shape description format.
    pub fn from_description(text: &str) -> Result<Shape, ParseError> {
        let mut shape = Shape::new();
        let bytes = text.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            // Bytes, not chars: every token starts with an ASCII byte, and the directive scan
            // below stops only at ASCII bytes, so slices always fall on char boundaries.
            let c = bytes[i];
            if c.is_ascii_whitespace() {
                i += 1;
            } else if c == b'@' {
                let start = i;
                while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'{' {
                    i += 1;
                }
                match &text[start..i] {
                    "@y-up" => shape.inverse_y_axis = false,
                    "@y-down" | "@invert-y" => shape.inverse_y_axis = true,
                    other => return err(format!("unknown directive '{other}'"), start),
                }
            } else if c == b'{' {
                let start = i + 1;
                let mut depth = 0;
                let mut j = start;
                while j < bytes.len() && !(bytes[j] == b'}' && depth == 0) {
                    match bytes[j] {
                        b'(' => depth += 1,
                        b')' => depth -= 1,
                        _ => {}
                    }
                    j += 1;
                }
                if j >= bytes.len() {
                    return err("unterminated contour", i);
                }
                let contour = parse_contour(&text[start..j], start)?;
                shape.contours.push(contour);
                i = j + 1;
            } else {
                let c = text[i..].chars().next().unwrap_or_default();
                return err(format!("unexpected character '{c}'"), i);
            }
        }
        Ok(shape)
    }

    /// Writes the shape (including edge colors if any edge is not white) in msdfgen's text
    /// shape description format.
    pub fn to_description(&self) -> String {
        let colored = self
            .contours
            .iter()
            .any(|c| c.edges.iter().any(|e| e.color != EdgeColor::WHITE));
        let mut out = String::new();
        out.push_str(if self.inverse_y_axis { "@y-down\n" } else { "@y-up\n" });
        for c in &self.contours {
            out.push_str("{\n");
            if !c.edges.is_empty() {
                for e in &c.edges {
                    let code = if colored {
                        match e.color {
                            EdgeColor::YELLOW => "y",
                            EdgeColor::MAGENTA => "m",
                            EdgeColor::CYAN => "c",
                            EdgeColor::WHITE => "w",
                            EdgeColor::RED => "r",
                            EdgeColor::GREEN => "g",
                            EdgeColor::BLUE => "b",
                            _ => "k",
                        }
                    } else {
                        ""
                    };
                    let p = e.segment.control_points();
                    let _ = writeln!(out, "\t{}, {};", fmt_num(p[0].x), fmt_num(p[0].y));
                    match e.segment {
                        Segment::Linear(_) => {
                            if !code.is_empty() {
                                let _ = writeln!(out, "\t\t{code};");
                            }
                        }
                        Segment::Quadratic(_) => {
                            let _ = writeln!(out, "\t\t{code}({}, {});", fmt_num(p[1].x), fmt_num(p[1].y));
                        }
                        Segment::Cubic(_) => {
                            let _ = writeln!(
                                out,
                                "\t\t{code}({}, {}; {}, {});",
                                fmt_num(p[1].x),
                                fmt_num(p[1].y),
                                fmt_num(p[2].x),
                                fmt_num(p[2].y)
                            );
                        }
                    }
                }
                out.push_str("\t#\n");
            }
            out.push_str("}\n");
        }
        out
    }
}

fn fmt_num(v: f64) -> String {
    if v == 0.0 { "0".into() } else { format!("{v}") }
}

/// Splits on `;` outside parentheses, returning (item, byte offset).
fn split_items(s: &str, base: usize) -> Vec<(&str, usize)> {
    let mut out = Vec::new();
    let mut depth = 0;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ';' if depth == 0 => {
                out.push((&s[start..i], base + start));
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push((&s[start..], base + start));
    out.into_iter().filter(|(t, _)| !t.trim().is_empty()).collect()
}

fn parse_contour(s: &str, base: usize) -> Result<Contour, ParseError> {
    let mut contour = Contour::new();
    let items = split_items(s, base);
    let mut parsed = Vec::with_capacity(items.len());
    for (t, pos) in &items {
        parsed.push((parse_item(t, *pos)?, *pos));
    }
    let mut first: Option<Vec2> = None;
    let mut last: Option<Vec2> = None;
    let mut pending: Option<(Option<EdgeColor>, Vec<Vec2>)> = None;
    for (item, pos) in parsed {
        match item {
            Item::Spec { color, controls } => {
                if last.is_none() {
                    return err("edge specification before the first point", pos);
                }
                if pending.is_some() {
                    return err("two consecutive edge specifications", pos);
                }
                pending = Some((color, controls));
            }
            Item::Point(_) | Item::Close => {
                let p = match item {
                    Item::Point(p) => p,
                    _ => match first {
                        Some(f) => f,
                        None => return err("'#' before any point", pos),
                    },
                };
                if let Some(a) = last {
                    let (color, controls) = pending.take().unwrap_or((None, Vec::new()));
                    let mut e = match controls.len() {
                        0 => Edge::line(a, p),
                        1 => Edge::quadratic(a, controls[0], p),
                        _ => Edge::cubic(a, controls[0], controls[1], p),
                    };
                    if let Some(c) = color {
                        e.color = c;
                    }
                    contour.add_edge(e);
                } else {
                    first = Some(p);
                }
                last = Some(p);
            }
        }
    }
    if pending.is_some() {
        return err("contour ends with an edge specification", base + s.len());
    }
    // Implicitly close the contour if the last point is not the first.
    if let (Some(f), Some(l)) = (first, last) {
        if f != l {
            contour.add_edge(Edge::line(l, f));
        }
    }
    Ok(contour)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_finite_numbers_are_errors() {
        for t in [
            "{ 0,0; inf,0; 0,1; # }",
            "{ 0,0; 1e999,0; 0,1; # }",
            "{ NaN,0; 1,0; 0,1; # }",
        ] {
            assert!(Shape::from_description(t).is_err(), "{t}");
        }
    }

    #[test]
    fn non_ascii_is_an_error() {
        // U+00E0 is encoded C3 A0, and 0xA0 read as a char is a no-break space.
        for t in ["@\u{e0}{ 0,0; 1,0; 0,1; # }", "\u{85}{ 0,0; 1,0; 0,1; # }", "\u{e0}"] {
            assert!(Shape::from_description(t).is_err(), "{t:?}");
        }
    }

    #[test]
    fn parse_examples() {
        let s = Shape::from_description("{ -1, -1; m; -1, +1; y; +1, +1; m; +1, -1; y; # }").unwrap();
        assert_eq!(s.contours.len(), 1);
        assert_eq!(s.contours[0].edges.len(), 4);
        assert_eq!(s.contours[0].edges[0].color, EdgeColor::MAGENTA);
        assert_eq!(s.contours[0].edges[3].color, EdgeColor::YELLOW);
        let s = Shape::from_description("{ 0, 1; (+1.6, -0.8; -1.6, -0.8); # }").unwrap();
        assert!(matches!(s.contours[0].edges[0].segment, Segment::Cubic(_)));
        let s = Shape::from_description("@y-down\n{\n\t0, 1;\n\t\tm(0.5, 0.4; 0.5, 0);\n\t1, -0.2;\n\t#\n}\n").unwrap();
        assert!(s.inverse_y_axis);
        assert_eq!(s.contours[0].edges[0].color, EdgeColor::MAGENTA);
        assert_eq!(s.contours[0].edges.len(), 2);
    }

    #[test]
    fn roundtrip() {
        let s = Shape::from_description("{ -1, -1; m; -1, 1; c(0, 2); 1, 1; y(2, 1; 2, -1); 1, -1; m; # }").unwrap();
        let t = s.to_description();
        let s2 = Shape::from_description(&t).unwrap();
        assert_eq!(s, s2);
    }
}
