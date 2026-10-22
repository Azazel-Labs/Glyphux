//! SVG input: path data parser (all commands including arcs) and a minimal SVG document
//! reader supporting `<path>`, `<rect>`, `<circle>`, `<ellipse>`, `<polygon>`, nested `<g>`
//! transforms and `fill-rule`.

use crate::edge::Edge;
use crate::math::Vec2;
use crate::shape::{Contour, FillRule, Shape};

/// SVG parse error.
#[derive(Clone, Debug, PartialEq)]
pub struct SvgError(pub String);

impl core::fmt::Display for SvgError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "SVG error: {}", self.0)
    }
}

impl std::error::Error for SvgError {}

/// 2D affine transform `[a c e; b d f]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Affine {
    pub const IDENTITY: Affine = Affine {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    pub fn apply(&self, p: Vec2) -> Vec2 {
        Vec2::new(
            self.a * p.x + self.c * p.y + self.e,
            self.b * p.x + self.d * p.y + self.f,
        )
    }

    /// `self * o` (apply `o` first).
    pub fn mul(&self, o: &Affine) -> Affine {
        Affine {
            a: self.a * o.a + self.c * o.b,
            b: self.b * o.a + self.d * o.b,
            c: self.a * o.c + self.c * o.d,
            d: self.b * o.c + self.d * o.d,
            e: self.a * o.e + self.c * o.f + self.e,
            f: self.b * o.e + self.d * o.f + self.f,
        }
    }
}

/// Parses an SVG `transform` attribute.
pub fn parse_transform(s: &str) -> Result<Affine, SvgError> {
    let mut m = Affine::IDENTITY;
    let mut rest = s.trim();
    while !rest.is_empty() {
        rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == ',');
        if rest.is_empty() {
            break;
        }
        let bad = || SvgError(format!("bad transform '{s}'"));
        let open = rest.find('(').ok_or_else(bad)?;
        let close = open + rest[open..].find(')').ok_or_else(bad)?;
        let name = rest[..open].trim();
        let mut nums = Vec::new();
        let mut lx = Lexer::new(&rest[open + 1..close]);
        while let Some(v) = lx.number() {
            nums.push(v);
        }
        let t = match (name, nums.as_slice()) {
            ("matrix", [a, b, c, d, e, f]) => Affine {
                a: *a,
                b: *b,
                c: *c,
                d: *d,
                e: *e,
                f: *f,
            },
            ("translate", [x]) => Affine {
                e: *x,
                ..Affine::IDENTITY
            },
            ("translate", [x, y]) => Affine {
                e: *x,
                f: *y,
                ..Affine::IDENTITY
            },
            ("scale", [x]) => Affine {
                a: *x,
                d: *x,
                ..Affine::IDENTITY
            },
            ("scale", [x, y]) => Affine {
                a: *x,
                d: *y,
                ..Affine::IDENTITY
            },
            ("rotate", [deg, rest @ ..]) if rest.is_empty() || rest.len() == 2 => {
                let (s, c) = deg.to_radians().sin_cos();
                let r = Affine {
                    a: c,
                    b: s,
                    c: -s,
                    d: c,
                    e: 0.0,
                    f: 0.0,
                };
                if let [cx, cy] = rest {
                    Affine {
                        e: *cx,
                        f: *cy,
                        ..Affine::IDENTITY
                    }
                    .mul(&r)
                    .mul(&Affine {
                        e: -cx,
                        f: -cy,
                        ..Affine::IDENTITY
                    })
                } else {
                    r
                }
            }
            ("skewX", [deg]) => Affine {
                c: deg.to_radians().tan(),
                ..Affine::IDENTITY
            },
            ("skewY", [deg]) => Affine {
                b: deg.to_radians().tan(),
                ..Affine::IDENTITY
            },
            _ => {
                return Err(SvgError(format!("unsupported transform '{}'", &rest[..=close])));
            }
        };
        m = m.mul(&t);
        rest = &rest[close + 1..];
    }
    Ok(m)
}

struct Lexer<'a> {
    s: &'a [u8],
    i: usize,
}

impl<'a> Lexer<'a> {
    fn new(s: &'a str) -> Self {
        Lexer { s: s.as_bytes(), i: 0 }
    }
    fn skip_ws(&mut self) {
        while self.i < self.s.len() && (self.s[self.i].is_ascii_whitespace() || self.s[self.i] == b',') {
            self.i += 1;
        }
    }
    fn peek_command(&mut self) -> Option<u8> {
        self.skip_ws();
        self.s
            .get(self.i)
            .copied()
            .filter(|c| c.is_ascii_alphabetic() && *c != b'e' && *c != b'E')
    }
    fn at_number(&mut self) -> bool {
        self.skip_ws();
        matches!(self.s.get(self.i), Some(c) if c.is_ascii_digit() || *c == b'-' || *c == b'+' || *c == b'.')
    }
    fn number(&mut self) -> Option<f64> {
        self.skip_ws();
        let start = self.i;
        let s = self.s;
        let mut i = self.i;
        if i < s.len() && (s[i] == b'+' || s[i] == b'-') {
            i += 1;
        }
        let mut digits = false;
        while i < s.len() && s[i].is_ascii_digit() {
            i += 1;
            digits = true;
        }
        if i < s.len() && s[i] == b'.' {
            i += 1;
            while i < s.len() && s[i].is_ascii_digit() {
                i += 1;
                digits = true;
            }
        }
        if !digits {
            return None;
        }
        if i < s.len() && (s[i] == b'e' || s[i] == b'E') {
            let mut j = i + 1;
            if j < s.len() && (s[j] == b'+' || s[j] == b'-') {
                j += 1;
            }
            if j < s.len() && s[j].is_ascii_digit() {
                while j < s.len() && s[j].is_ascii_digit() {
                    j += 1;
                }
                i = j;
            }
        }
        self.i = i;
        std::str::from_utf8(&s[start..i]).ok()?.parse().ok()
    }
    /// Arc flags may be written without separators ("a1 1 0 011 1").
    fn flag(&mut self) -> Option<bool> {
        self.skip_ws();
        match self.s.get(self.i) {
            Some(b'0') => {
                self.i += 1;
                Some(false)
            }
            Some(b'1') => {
                self.i += 1;
                Some(true)
            }
            _ => None,
        }
    }
    fn done(&mut self) -> bool {
        self.skip_ws();
        self.i >= self.s.len()
    }
}

/// Builds contours from path commands, applying a transform.
struct PathBuilder {
    shape: Shape,
    cur: Contour,
    start: Vec2,
    pos: Vec2,
    xf: Affine,
}

impl PathBuilder {
    fn new(xf: Affine) -> Self {
        PathBuilder {
            shape: Shape::new(),
            cur: Contour::new(),
            start: Vec2::ZERO,
            pos: Vec2::ZERO,
            xf,
        }
    }
    fn t(&self, p: Vec2) -> Vec2 {
        self.xf.apply(p)
    }
    fn move_to(&mut self, p: Vec2) {
        self.close();
        self.start = p;
        self.pos = p;
    }
    fn line_to(&mut self, p: Vec2) {
        if p != self.pos {
            self.cur.add_edge(Edge::line(self.t(self.pos), self.t(p)));
        }
        self.pos = p;
    }
    fn quad_to(&mut self, c: Vec2, p: Vec2) {
        if p != self.pos || c != self.pos {
            self.cur
                .add_edge(Edge::quadratic(self.t(self.pos), self.t(c), self.t(p)));
        }
        self.pos = p;
    }
    fn cubic_to(&mut self, c1: Vec2, c2: Vec2, p: Vec2) {
        if p != self.pos || c1 != self.pos || c2 != self.pos {
            self.cur
                .add_edge(Edge::cubic(self.t(self.pos), self.t(c1), self.t(c2), self.t(p)));
        }
        self.pos = p;
    }
    fn close(&mut self) {
        if !self.cur.edges.is_empty() {
            if self.pos != self.start {
                self.cur.add_edge(Edge::line(self.t(self.pos), self.t(self.start)));
            }
            // Make the contour exactly closed after transformation rounding.
            let first = self.cur.edges[0].start();
            let last = self.cur.edges.len() - 1;
            let pts = self.cur.edges[last].segment.control_points_mut();
            let n = pts.len();
            pts[n - 1] = first;
            self.shape.add_contour(std::mem::take(&mut self.cur));
        }
        self.pos = self.start;
    }
    /// Elliptical arc from the current position (SVG endpoint parameterization).
    #[allow(clippy::too_many_arguments)]
    fn arc_to(&mut self, rx: f64, ry: f64, rot_deg: f64, large: bool, sweep: bool, p: Vec2) {
        let p0 = self.pos;
        if p == p0 {
            return;
        }
        let (mut rx, mut ry) = (rx.abs(), ry.abs());
        if rx == 0.0 || ry == 0.0 {
            self.line_to(p);
            return;
        }
        let (sphi, cphi) = rot_deg.to_radians().sin_cos();
        let dx = (p0.x - p.x) * 0.5;
        let dy = (p0.y - p.y) * 0.5;
        let x1 = cphi * dx + sphi * dy;
        let y1 = -sphi * dx + cphi * dy;
        let lambda = (x1 * x1) / (rx * rx) + (y1 * y1) / (ry * ry);
        if lambda > 1.0 {
            let s = lambda.sqrt();
            rx *= s;
            ry *= s;
        }
        let num = rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1;
        let den = rx * rx * y1 * y1 + ry * ry * x1 * x1;
        let mut k = if den > 0.0 { (num / den).max(0.0).sqrt() } else { 0.0 };
        if large == sweep {
            k = -k;
        }
        let cx1 = k * rx * y1 / ry;
        let cy1 = -k * ry * x1 / rx;
        let cx = cphi * cx1 - sphi * cy1 + f64::midpoint(p0.x, p.x);
        let cy = sphi * cx1 + cphi * cy1 + f64::midpoint(p0.y, p.y);
        let ang = |ux: f64, uy: f64, vx: f64, vy: f64| -> f64 { (ux * vy - uy * vx).atan2(ux * vx + uy * vy) };
        let ux = (x1 - cx1) / rx;
        let uy = (y1 - cy1) / ry;
        let vx = (-x1 - cx1) / rx;
        let vy = (-y1 - cy1) / ry;
        let theta1 = ang(1.0, 0.0, ux, uy);
        let mut dtheta = ang(ux, uy, vx, vy);
        if !sweep && dtheta > 0.0 {
            dtheta -= std::f64::consts::TAU;
        } else if sweep && dtheta < 0.0 {
            dtheta += std::f64::consts::TAU;
        }
        let segs = (dtheta.abs() / std::f64::consts::FRAC_PI_2).ceil().max(1.0) as usize;
        let delta = dtheta / segs as f64;
        let kappa = 4.0 / 3.0 * (delta / 4.0).tan();
        let point = |t: f64| -> (Vec2, Vec2) {
            let (st, ct) = t.sin_cos();
            let pos = Vec2::new(
                cx + rx * ct * cphi - ry * st * sphi,
                cy + rx * ct * sphi + ry * st * cphi,
            );
            let der = Vec2::new(-rx * st * cphi - ry * ct * sphi, -rx * st * sphi + ry * ct * cphi);
            (pos, der)
        };
        let mut t = theta1;
        for i in 0..segs {
            let (a, da) = point(t);
            let t2 = t + delta;
            let (mut b, db) = point(t2);
            if i + 1 == segs {
                b = p;
            }
            let _ = a;
            let c1 = self.pos + da * kappa;
            let c2 = b - db * kappa;
            self.cubic_to(c1, c2, b);
            t = t2;
        }
        self.pos = p;
    }
}

/// Parses SVG path data (`d` attribute) into a shape (coordinates as in the SVG, y-down).
pub fn parse_path_data(d: &str) -> Result<Shape, SvgError> {
    parse_path_data_with(d, Affine::IDENTITY)
}

fn parse_path_data_with(d: &str, xf: Affine) -> Result<Shape, SvgError> {
    let mut b = PathBuilder::new(xf);
    let mut lx = Lexer::new(d);
    let mut cmd = 0u8;
    let mut last_ctrl: Option<Vec2> = None; // for S/T reflection
    let mut last_kind = 0u8; // 'C' or 'Q' when the previous command was a cubic/quadratic
    let bad = |c: u8| SvgError(format!("malformed path data near command '{}'", c as char));
    loop {
        if let Some(c) = lx.peek_command() {
            cmd = c;
            lx.i += 1;
        } else if lx.done() {
            break;
        } else if cmd == 0 || !lx.at_number() {
            return Err(SvgError("path data must start with a command".into()));
        }
        // Implicit repetition: after M/m, further pairs are L/l.
        let rel = cmd.is_ascii_lowercase();
        let base = if rel { b.pos } else { Vec2::ZERO };
        let up = cmd.to_ascii_uppercase();
        let mut kind = 0u8;
        let mut ctrl = None;
        match up {
            b'M' => {
                let p = base
                    + Vec2::new(
                        lx.number().ok_or_else(|| bad(cmd))?,
                        lx.number().ok_or_else(|| bad(cmd))?,
                    );
                b.move_to(p);
                cmd = if rel { b'l' } else { b'L' };
            }
            b'L' => {
                let p = base
                    + Vec2::new(
                        lx.number().ok_or_else(|| bad(cmd))?,
                        lx.number().ok_or_else(|| bad(cmd))?,
                    );
                b.line_to(p);
            }
            b'H' => {
                let x = lx.number().ok_or_else(|| bad(cmd))?;
                let p = Vec2::new(if rel { b.pos.x + x } else { x }, b.pos.y);
                b.line_to(p);
            }
            b'V' => {
                let y = lx.number().ok_or_else(|| bad(cmd))?;
                let p = Vec2::new(b.pos.x, if rel { b.pos.y + y } else { y });
                b.line_to(p);
            }
            b'C' | b'S' => {
                let c1 = if up == b'C' {
                    base + Vec2::new(
                        lx.number().ok_or_else(|| bad(cmd))?,
                        lx.number().ok_or_else(|| bad(cmd))?,
                    )
                } else {
                    match (last_kind, last_ctrl) {
                        (b'C', Some(c)) => b.pos * 2.0 - c,
                        _ => b.pos,
                    }
                };
                let c2 = base
                    + Vec2::new(
                        lx.number().ok_or_else(|| bad(cmd))?,
                        lx.number().ok_or_else(|| bad(cmd))?,
                    );
                let p = base
                    + Vec2::new(
                        lx.number().ok_or_else(|| bad(cmd))?,
                        lx.number().ok_or_else(|| bad(cmd))?,
                    );
                b.cubic_to(c1, c2, p);
                kind = b'C';
                ctrl = Some(c2);
            }
            b'Q' | b'T' => {
                let c = if up == b'Q' {
                    base + Vec2::new(
                        lx.number().ok_or_else(|| bad(cmd))?,
                        lx.number().ok_or_else(|| bad(cmd))?,
                    )
                } else {
                    match (last_kind, last_ctrl) {
                        (b'Q', Some(c)) => b.pos * 2.0 - c,
                        _ => b.pos,
                    }
                };
                let p = base
                    + Vec2::new(
                        lx.number().ok_or_else(|| bad(cmd))?,
                        lx.number().ok_or_else(|| bad(cmd))?,
                    );
                if c == b.pos || c == p {
                    b.line_to(p);
                } else {
                    b.quad_to(c, p);
                }
                kind = b'Q';
                ctrl = Some(c);
            }
            b'A' => {
                let rx = lx.number().ok_or_else(|| bad(cmd))?;
                let ry = lx.number().ok_or_else(|| bad(cmd))?;
                let rot = lx.number().ok_or_else(|| bad(cmd))?;
                let large = lx.flag().ok_or_else(|| bad(cmd))?;
                let sweep = lx.flag().ok_or_else(|| bad(cmd))?;
                let p = base
                    + Vec2::new(
                        lx.number().ok_or_else(|| bad(cmd))?,
                        lx.number().ok_or_else(|| bad(cmd))?,
                    );
                b.arc_to(rx, ry, rot, large, sweep, p);
            }
            b'Z' => {
                b.close();
                // A command letter must follow.
                if lx.at_number() {
                    return Err(bad(cmd));
                }
            }
            _ => return Err(SvgError(format!("unknown path command '{}'", cmd as char))),
        }
        last_kind = kind;
        last_ctrl = ctrl;
    }
    b.close();
    Ok(b.shape)
}

/// Options for loading SVG documents.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SvgPathSelection {
    /// Union of all filled shapes in the document.
    #[default]
    All,
    /// Only the last path-like element (msdfgen's behavior).
    Last,
}

/// Result of loading an SVG document.
#[derive(Clone, Debug)]
pub struct SvgShape {
    /// The shape in SVG coordinates (y-down, `inverse_y_axis` set). Contours of each element
    /// have been oriented according to the element's fill rule, so the whole shape can be
    /// treated with the non-zero rule.
    pub shape: Shape,
    /// The `viewBox` (x, y, width, height) if present.
    pub view_box: Option<[f64; 4]>,
}

struct Element<'a> {
    name: &'a str,
    attrs: Vec<(&'a str, String)>,
    self_closing: bool,
    closing: bool,
}

fn decode_entities(s: &str) -> String {
    s.replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// Iterates over XML tags (ignores comments, processing instructions, CDATA, doctype).
fn tags(doc: &str) -> Vec<Element<'_>> {
    let mut out = Vec::new();
    let b = doc.as_bytes();
    let mut i = 0;
    while let Some(off) = doc[i..].find('<') {
        let start = i + off;
        if doc[start..].starts_with("<!--") {
            i = doc[start..].find("-->").map_or(doc.len(), |e| start + e + 3);
            continue;
        }
        if doc[start..].starts_with("<![CDATA[") {
            i = doc[start..].find("]]>").map_or(doc.len(), |e| start + e + 3);
            continue;
        }
        if doc[start..].starts_with("<?") || doc[start..].starts_with("<!") {
            i = doc[start..].find('>').map_or(doc.len(), |e| start + e + 1);
            continue;
        }
        // Find the end of the tag, respecting quotes.
        let mut j = start + 1;
        let mut quote = 0u8;
        while j < b.len() {
            let c = b[j];
            if quote != 0 {
                if c == quote {
                    quote = 0;
                }
            } else if c == b'"' || c == b'\'' {
                quote = c;
            } else if c == b'>' {
                break;
            }
            j += 1;
        }
        let inner = &doc[start + 1..j.min(doc.len())];
        i = (j + 1).min(doc.len());
        let closing = inner.starts_with('/');
        let inner = inner.trim_start_matches('/');
        let self_closing = inner.trim_end().ends_with('/');
        let inner = inner.trim_end().trim_end_matches('/');
        let name_end = inner.find(|c: char| c.is_whitespace()).unwrap_or(inner.len());
        let name = &inner[..name_end];
        let name = name.rsplit(':').next().unwrap_or(name);
        let mut attrs = Vec::new();
        let ab = inner.as_bytes();
        let mut k = name_end;
        loop {
            while k < ab.len() && ab[k].is_ascii_whitespace() {
                k += 1;
            }
            if k >= ab.len() {
                break;
            }
            let ks = k;
            while k < ab.len() && ab[k] != b'=' && !ab[k].is_ascii_whitespace() {
                k += 1;
            }
            let key = &inner[ks..k];
            while k < ab.len() && ab[k].is_ascii_whitespace() {
                k += 1;
            }
            if k >= ab.len() || ab[k] != b'=' {
                continue;
            }
            k += 1;
            while k < ab.len() && ab[k].is_ascii_whitespace() {
                k += 1;
            }
            if k >= ab.len() {
                break;
            }
            let q = ab[k];
            if q == b'"' || q == b'\'' {
                let vs = k + 1;
                let ve = inner[vs..].find(q as char).map_or(inner.len(), |e| vs + e);
                attrs.push((key, decode_entities(&inner[vs..ve])));
                k = ve + 1;
            } else {
                let vs = k;
                while k < ab.len() && !ab[k].is_ascii_whitespace() {
                    k += 1;
                }
                attrs.push((key, decode_entities(&inner[vs..k])));
            }
        }
        out.push(Element {
            name,
            attrs,
            self_closing,
            closing,
        });
    }
    out
}

fn attr<'a>(e: &'a Element<'_>, key: &str) -> Option<&'a str> {
    e.attrs.iter().find(|(k, _)| *k == key).map(|(_, v)| v.as_str())
}

/// Reads a property from the attribute or the `style` attribute.
fn prop(e: &Element<'_>, key: &str) -> Option<String> {
    if let Some(style) = attr(e, "style") {
        for decl in style.split(';') {
            if let Some((k, v)) = decl.split_once(':') {
                if k.trim() == key {
                    return Some(v.trim().to_string());
                }
            }
        }
    }
    attr(e, key).map(|s| s.trim().to_string())
}

fn num_attr(e: &Element<'_>, key: &str) -> f64 {
    attr(e, key)
        .map(|s| s.trim().trim_end_matches("px"))
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0)
}

fn ellipse_path(cx: f64, cy: f64, rx: f64, ry: f64) -> String {
    format!(
        "M{} {}A{rx} {ry} 0 1 1 {} {}A{rx} {ry} 0 1 1 {} {}Z",
        cx + rx,
        cy,
        cx - rx,
        cy,
        cx + rx,
        cy
    )
}

/// What an element inherits from its ancestors.
#[derive(Clone, Copy)]
struct Inherited {
    xf: Affine,
    rule: Option<FillRule>,
    /// Inside an element that is never rendered (`defs`, `clipPath`…, `display: none`).
    hidden: bool,
    /// `fill: none` in effect (a property that descendants can override).
    no_fill: bool,
}

/// Loads an SVG document.
pub fn load_svg(doc: &str, selection: SvgPathSelection) -> Result<SvgShape, SvgError> {
    let mut stack: Vec<(&str, Inherited)> = Vec::new();
    let mut cur = Inherited {
        xf: Affine::IDENTITY,
        rule: None,
        hidden: false,
        no_fill: false,
    };
    let mut view_box = None;
    let mut parts: Vec<Shape> = Vec::new();
    for e in tags(doc) {
        if e.closing {
            if let Some(pos) = stack.iter().rposition(|(n, _)| *n == e.name) {
                cur = stack[pos].1;
                stack.truncate(pos);
            }
            continue;
        }
        let local = match attr(&e, "transform") {
            Some(t) => cur.xf.mul(&parse_transform(t)?),
            None => cur.xf,
        };
        let rule = match prop(&e, "fill-rule").as_deref() {
            Some("evenodd") => Some(FillRule::EvenOdd),
            Some("nonzero") => Some(FillRule::NonZero),
            _ => cur.rule,
        };
        let is_hidden = cur.hidden
            || matches!(e.name, "defs" | "clipPath" | "mask" | "symbol" | "pattern" | "marker")
            || prop(&e, "display").as_deref() == Some("none");
        let no_fill = match prop(&e, "fill").as_deref() {
            Some("none") => true,
            Some(_) => false,
            None => cur.no_fill,
        };
        let path_data: Option<String> = match e.name {
            "svg" => {
                if view_box.is_none() {
                    if let Some(vb) = attr(&e, "viewBox") {
                        let mut lx = Lexer::new(vb);
                        let v: Vec<f64> = std::iter::from_fn(|| lx.number()).collect();
                        if v.len() == 4 {
                            view_box = Some([v[0], v[1], v[2], v[3]]);
                        }
                    }
                }
                None
            }
            "path" => attr(&e, "d").map(ToString::to_string),
            "rect" => {
                let (x, y, w, h) = (
                    num_attr(&e, "x"),
                    num_attr(&e, "y"),
                    num_attr(&e, "width"),
                    num_attr(&e, "height"),
                );
                if w > 0.0 && h > 0.0 {
                    let mut rx = num_attr(&e, "rx");
                    let mut ry = num_attr(&e, "ry");
                    if attr(&e, "rx").is_none() {
                        rx = ry;
                    }
                    if attr(&e, "ry").is_none() {
                        ry = rx;
                    }
                    let (rx, ry) = (rx.min(w / 2.0), ry.min(h / 2.0));
                    if rx > 0.0 && ry > 0.0 {
                        Some(format!(
                            "M{} {}H{}A{rx} {ry} 0 0 1 {} {}V{}A{rx} {ry} 0 0 1 {} {}H{}A{rx} {ry} 0 0 1 {} {}V{}A{rx} {ry} 0 0 1 {} {}Z",
                            x + rx,
                            y,
                            x + w - rx,
                            x + w,
                            y + ry,
                            y + h - ry,
                            x + w - rx,
                            y + h,
                            x + rx,
                            x,
                            y + h - ry,
                            y + ry,
                            x + rx,
                            y
                        ))
                    } else {
                        Some(format!("M{x} {y}H{}V{}H{x}Z", x + w, y + h))
                    }
                } else {
                    None
                }
            }
            "circle" => {
                let r = num_attr(&e, "r");
                (r > 0.0).then(|| ellipse_path(num_attr(&e, "cx"), num_attr(&e, "cy"), r, r))
            }
            "ellipse" => {
                let (rx, ry) = (num_attr(&e, "rx"), num_attr(&e, "ry"));
                (rx > 0.0 && ry > 0.0).then(|| ellipse_path(num_attr(&e, "cx"), num_attr(&e, "cy"), rx, ry))
            }
            "polygon" | "polyline" => attr(&e, "points").map(|p| format!("M{p}Z")),
            _ => None,
        };
        if let Some(d) = path_data {
            if !is_hidden && !no_fill {
                // (Mirroring transforms reverse contours; orienting them below fixes that.)
                let mut s = parse_path_data_with(&d, local)?;
                s.normalize();
                s.orient_contours(rule.unwrap_or(FillRule::NonZero));
                parts.push(s);
            }
        }
        if !e.self_closing
            && !matches!(
                e.name,
                "path" | "rect" | "circle" | "ellipse" | "polygon" | "polyline" | "line"
            )
        {
            stack.push((e.name, cur));
            cur = Inherited {
                xf: local,
                rule,
                hidden: is_hidden && e.name != "svg",
                // Icon sets often put fill="none" (with a stroke) on the root: the outlines are
                // still wanted.
                no_fill: no_fill && e.name != "svg",
            };
        } else if !e.self_closing {
            // Elements like <path ...></path>: push so the closing tag restores state correctly.
            stack.push((e.name, cur));
        }
    }
    let mut shape = Shape::new();
    match selection {
        SvgPathSelection::Last => {
            if let Some(last) = parts.pop() {
                shape = last;
            }
        }
        SvgPathSelection::All => {
            for p in parts {
                shape.contours.extend(p.contours);
            }
        }
    }
    shape.inverse_y_axis = true;
    if shape.contours.is_empty() {
        return Err(SvgError("no filled path found".into()));
    }
    Ok(SvgShape { shape, view_box })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_transforms_are_errors() {
        for t in ["rotate)(1)", "a)(", "scale(2", ")", "translate 1 2"] {
            assert!(parse_transform(t).is_err(), "{t}");
        }
        let doc = r#"<svg><path transform="a)(" d="M0 0h1v1z"/></svg>"#;
        assert!(load_svg(doc, SvgPathSelection::All).is_err());
    }

    #[test]
    fn path_commands() {
        let s = parse_path_data("M0 0 L10 0 l0 10 H0 z").unwrap();
        assert_eq!(s.contours.len(), 1);
        assert_eq!(s.contours[0].edges.len(), 4);
        let s = parse_path_data("M0,0 C 1,1 2,1 3,0 S 5,-1 6,0 Q 7 1 8 0 T 10 0 Z").unwrap();
        assert_eq!(s.contours[0].edges.len(), 5);
        let s = parse_path_data("m1.5.5-1-1zM0 0h1v1h-1z").unwrap();
        assert_eq!(s.contours.len(), 2);
        // Arc with compact flags
        let s = parse_path_data("M10 0a10 10 0 1110 0z").unwrap();
        s.validate().unwrap();
        assert!(s.contours[0].edges.len() >= 2);
    }

    #[test]
    fn arc_is_circle() {
        let s = parse_path_data("M 1 0 A 1 1 0 1 1 -1 0 A 1 1 0 1 1 1 0 Z").unwrap();
        for c in &s.contours {
            for e in &c.edges {
                for i in 0..=10 {
                    let p = e.point(i as f64 / 10.0);
                    assert!((p.length() - 1.0).abs() < 1e-3, "{p:?}");
                }
            }
        }
    }

    #[test]
    fn document() {
        let doc = r#"<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">
            <!-- comment <path d="M0 0"/> -->
            <g transform="translate(10,10)"><rect x="0" y="0" width="20" height="10"/></g>
            <circle cx="50" cy="50" r="10" fill="none"/>
            <path fill-rule="evenodd" d="M0 0H50V50H0Z M10 10H40V40H10Z"/>
        </svg>"#;
        let s = load_svg(doc, SvgPathSelection::All).unwrap();
        assert_eq!(s.view_box, Some([0.0, 0.0, 100.0, 100.0]));
        assert_eq!(s.shape.contours.len(), 3);
        let b = s.shape.contours[0].bounds();
        assert_eq!((b.left, b.bottom, b.right, b.top), (10.0, 10.0, 30.0, 20.0));
        // Evenodd hole must be oriented opposite to its outer contour.
        assert_eq!(s.shape.contours[1].winding(), -s.shape.contours[2].winding());
    }

    #[test]
    fn fill_is_inherited_and_overridable() {
        let doc = r##"<svg fill="none">
            <g fill="none"><path fill="#000" d="M0 0h1v1h-1z"/><path d="M2 0h1v1h-1z"/></g>
            <g style="fill:none"><g><rect width="1" height="1" style="fill: red"/></g></g>
            <path d="M5 0h1v1h-1z"/>
            <defs><path fill="#000" d="M7 0h1v1h-1z"/></defs>
        </svg>"##;
        let s = load_svg(doc, SvgPathSelection::All).unwrap();
        // The overriding path, the red rect and the path under the root's fill="none".
        let lefts: Vec<f64> = s.shape.contours.iter().map(|c| c.bounds().left).collect();
        assert_eq!(lefts, [0.0, 0.0, 5.0]);
    }
}
