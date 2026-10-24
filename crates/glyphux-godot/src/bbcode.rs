//! The BBCode of `GlyphuxRichLabel`: Godot `RichTextLabel`'s tags and parameters where they
//! exist, plus `glow` and `shadow`.
//!
//! | Tag | |
//! |---|---|
//! | `[b]` `[i]` `[u]` `[s]` `[code]` | bold, italics, underline, strikethrough, the monospace font |
//! | `[color=c]` `[font_size=n]` | color (name or `#hex`), size |
//! | `[font=res://...]` `[font name=res://... size=n]` | a font resource (`FontFile`, `FontVariation`), and a size |
//! | `[outline_size=n]` `[outline_color=c]` | outline |
//! | `[glow color=c radius=n]` `[shadow color=c offset=x,y blur=n]` | glow, drop shadow |
//! | `[url]` `[url=meta]` | a link: `meta_clicked` with the meta (or the text) |
//! | `[wave amp=50 freq=5]` `[shake rate=20 level=5]` `[rainbow freq=1 sat=0.8 val=0.8 speed=1]` | animation, in the shader |
//! | `[left]` `[center]` `[right]` `[fill]` | the alignment of the paragraphs inside |
//! | `[lb]` `[rb]` | `[` and `]` |
//!
//! Unknown or malformed tags stay in the text as written.

use std::ops::Range;

/// The attributes of a run of text (`None`: the label's).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Attrs {
    pub bold: bool,
    pub italic: bool,
    pub mono: bool,
    pub underline: bool,
    pub strike: bool,
    pub color: Option<String>,
    pub size: Option<f32>,
    pub font: Option<String>,
    pub outline_size: Option<f32>,
    pub outline_color: Option<String>,
    /// Color and radius.
    pub glow: Option<(Option<String>, f32)>,
    /// Color, offset and blur.
    pub shadow: Option<(Option<String>, [f32; 2], f32)>,
    /// Amplitude and frequency, as Godot's `[wave]`.
    pub wave: Option<(f32, f32)>,
    /// Rate and level, as Godot's `[shake]`.
    pub shake: Option<(f32, f32)>,
    /// Frequency, saturation, value and speed, as Godot's `[rainbow]`.
    pub rainbow: Option<(f32, f32, f32, f32)>,
    pub url: Option<String>,
}

/// Paragraph alignment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
    Fill,
}

/// Parsed BBCode: the plain text, its runs (covering it, in order) and alignments.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Parsed {
    pub text: String,
    pub runs: Vec<(Range<usize>, Attrs)>,
    /// Byte ranges of the text inside alignment tags.
    pub aligns: Vec<(Range<usize>, Align)>,
}

/// `key=value` parameters of a tag; a tag's own `=value` has the empty key.
fn params(body: &str) -> (String, Vec<(String, String)>) {
    let (name, rest) = match body.find([' ', '=']) {
        Some(i) => (&body[..i], &body[i..]),
        None => (body, ""),
    };
    let mut out = Vec::new();
    let mut rest = rest;
    if let Some(v) = rest.strip_prefix('=') {
        // `[tag=value ...]`: the value runs to the next space.
        let end = v.find(' ').unwrap_or(v.len());
        out.push((String::new(), unquote(&v[..end])));
        rest = &v[end..];
    }
    for word in rest.split_whitespace() {
        if let Some((k, v)) = word.split_once('=') {
            out.push((k.to_string(), unquote(v)));
        }
    }
    (name.to_ascii_lowercase(), out)
}

fn unquote(v: &str) -> String {
    v.trim_matches('"').to_string()
}

fn number(params: &[(String, String)], key: &str, default: f32) -> f32 {
    params
        .iter()
        .find(|(k, _)| k == key)
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(default)
}

fn text_param(params: &[(String, String)], key: &str) -> Option<String> {
    params.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
}

/// Applies an opening tag to `attrs`; `false` for tags it doesn't know.
fn open(name: &str, p: &[(String, String)], attrs: &mut Attrs) -> bool {
    let value = text_param(p, "");
    match name {
        "b" => attrs.bold = true,
        "i" => attrs.italic = true,
        "u" => attrs.underline = true,
        "s" => attrs.strike = true,
        "code" => attrs.mono = true,
        "color" => attrs.color = value,
        "font_size" => attrs.size = value.and_then(|v| v.parse().ok()),
        "font" => {
            attrs.font = value.or_else(|| text_param(p, "name"));
            if let Some(size) = text_param(p, "size").and_then(|v| v.parse().ok()) {
                attrs.size = Some(size);
            }
        }
        "outline_size" => attrs.outline_size = value.and_then(|v| v.parse().ok()),
        "outline_color" => attrs.outline_color = value,
        "glow" => attrs.glow = Some((text_param(p, "color"), number(p, "radius", 8.0))),
        "shadow" => {
            let offset = text_param(p, "offset")
                .and_then(|o| {
                    let (x, y) = o.split_once(',')?;
                    Some([x.trim().parse().ok()?, y.trim().parse().ok()?])
                })
                .unwrap_or([2.0, 2.0]);
            attrs.shadow = Some((text_param(p, "color"), offset, number(p, "blur", 2.0)));
        }
        "wave" => attrs.wave = Some((number(p, "amp", 20.0), number(p, "freq", 5.0))),
        "shake" => attrs.shake = Some((number(p, "rate", 20.0), number(p, "level", 5.0))),
        "rainbow" => {
            attrs.rainbow = Some((
                number(p, "freq", 1.0),
                number(p, "sat", 0.8),
                number(p, "val", 0.8),
                number(p, "speed", 1.0),
            ));
        }
        "url" => attrs.url = Some(value.unwrap_or_default()),
        _ => return false,
    }
    true
}

fn alignment(name: &str) -> Option<Align> {
    match name {
        "left" => Some(Align::Left),
        "center" => Some(Align::Center),
        "right" => Some(Align::Right),
        "fill" => Some(Align::Fill),
        _ => None,
    }
}

/// Parses `source`.
pub fn parse(source: &str) -> Parsed {
    let mut out = Parsed::default();
    // Open tags: name, the attributes before it, where its text starts.
    let mut stack: Vec<(String, Attrs, usize)> = Vec::new();
    let mut attrs = Attrs::default();
    let push_text = |out: &mut Parsed, attrs: &Attrs, s: &str| {
        if s.is_empty() {
            return;
        }
        let start = out.text.len();
        out.text.push_str(s);
        let end = out.text.len();
        match out.runs.last_mut() {
            Some((r, a)) if a == attrs && r.end == start => r.end = end,
            _ => out.runs.push((start..end, attrs.clone())),
        }
    };
    let mut rest = source;
    while let Some(open_at) = rest.find('[') {
        push_text(&mut out, &attrs, &rest[..open_at]);
        let after = &rest[open_at + 1..];
        let Some(close_at) = after.find(']') else {
            push_text(&mut out, &attrs, &rest[open_at..]);
            rest = "";
            break;
        };
        let body = &after[..close_at];
        let whole = &rest[open_at..=open_at + 1 + close_at];
        rest = &after[close_at + 1..];
        if body == "lb" {
            push_text(&mut out, &attrs, "[");
            continue;
        }
        if body == "rb" {
            push_text(&mut out, &attrs, "]");
            continue;
        }
        if let Some(name) = body.strip_prefix('/') {
            let name = name.trim().to_ascii_lowercase();
            if let Some(i) = stack.iter().rposition(|(n, ..)| *n == name) {
                // Closing an outer tag closes the ones inside it too.
                let (_, before, start) = stack[i].clone();
                stack.truncate(i);
                if let Some(a) = alignment(&name) {
                    out.aligns.push((start..out.text.len(), a));
                } else {
                    attrs = before;
                }
                continue;
            }
            push_text(&mut out, &attrs, whole);
            continue;
        }
        let (name, p) = params(body);
        if alignment(&name).is_some() {
            stack.push((name, attrs.clone(), out.text.len()));
            continue;
        }
        let before = attrs.clone();
        if open(&name, &p, &mut attrs) {
            stack.push((name, before, out.text.len()));
        } else {
            push_text(&mut out, &attrs, whole);
        }
    }
    push_text(&mut out, &attrs, rest);
    // Alignment tags still open run to the end.
    for (name, _, start) in stack {
        if let Some(a) = alignment(&name) {
            out.aligns.push((start..out.text.len(), a));
        }
    }
    // `[url]text[/url]` links to its text.
    for (range, a) in &mut out.runs {
        if a.url.as_deref() == Some("") {
            a.url = Some(out.text[range.clone()].to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runs(p: &Parsed) -> Vec<(&str, &Attrs)> {
        p.runs.iter().map(|(r, a)| (&p.text[r.clone()], a)).collect()
    }

    #[test]
    fn nests_and_merges() {
        let p = parse("a [b]bold [i]both[/i][/b] c");
        assert_eq!(p.text, "a bold both c");
        let r = runs(&p);
        assert_eq!(r[0].0, "a ");
        assert_eq!(r[1].0, "bold ");
        assert!(r[1].1.bold && !r[1].1.italic);
        assert_eq!(r[2].0, "both");
        assert!(r[2].1.bold && r[2].1.italic);
        assert_eq!(r[3].0, " c");
        assert_eq!(*r[3].1, Attrs::default());
    }

    #[test]
    fn values_and_parameters() {
        let p = parse("[color=#ff8800]x[/color][font_size=32]y[/font_size][wave amp=10 freq=2]z[/wave]");
        let r = runs(&p);
        assert_eq!(r[0].1.color.as_deref(), Some("#ff8800"));
        assert_eq!(r[1].1.size, Some(32.0));
        assert_eq!(r[2].1.wave, Some((10.0, 2.0)));
        let p = parse("[shadow color=black offset=3,4 blur=1]s[/shadow][glow radius=6]g[/glow]");
        let r = runs(&p);
        assert_eq!(r[0].1.shadow, Some((Some("black".into()), [3.0, 4.0], 1.0)));
        assert_eq!(r[1].1.glow, Some((None, 6.0)));
        let p = parse("[font=res://a.tres]a[/font][font name=res://b.ttf size=30]b[/font]");
        let r = runs(&p);
        assert_eq!(r[0].1.font.as_deref(), Some("res://a.tres"));
        assert_eq!((r[1].1.font.as_deref(), r[1].1.size), (Some("res://b.ttf"), Some(30.0)));
    }

    #[test]
    fn links_and_escapes() {
        let p = parse("[url]https://x[/url] [url=meta]text[/url] [lb]b[rb]");
        let r = runs(&p);
        assert_eq!(r[0].1.url.as_deref(), Some("https://x"));
        assert_eq!(r[2].1.url.as_deref(), Some("meta"));
        assert!(p.text.ends_with("[b]"));
    }

    #[test]
    fn unknown_tags_stay() {
        let p = parse("[nope]x[/nope] [b]y");
        assert_eq!(p.text, "[nope]x[/nope] y");
        assert!(runs(&p).last().unwrap().1.bold, "unclosed tags run to the end");
    }

    #[test]
    fn alignment_ranges() {
        let p = parse("left\n[center]mid\nmid2[/center]\n[right]end");
        assert_eq!(p.text, "left\nmid\nmid2\nend");
        assert_eq!(p.aligns[0], (5..13, Align::Center));
        assert_eq!(p.aligns[1], (14..17, Align::Right));
    }
}
