//! Draws the figures of the README's "How it works" section from real generator output.
//!
//! usage: figures [out_dir] (default `docs/figures`), run from the repository root. The atlas
//! figure needs a GPU adapter and is skipped without one.
use std::fmt::Write as _;
use std::sync::Arc;

use textra::gpu::{FieldKind, bin_tiles};
use textra::io::font::{Font, FontScaling};
use textra::io::png::encode_png;
use textra::{
    Bitmap, ColoringStrategy, DEFAULT_ANGLE_THRESHOLD, Edge, ErrorCorrectionConfig, FillRule, GeneratorConfig,
    PreparedShape, Projection, Range, Segment, Shape, Transform, Vec2, edge_coloring, generate_msdf, generate_sdf,
    median,
};
use textra_gpu::{AtlasConfig, GlyphAtlas, GlyphKey, Shaper, ShapingFont, default_device, read_texture_layer, wgpu};

const ASSETS: &str = "examples/web-demo/assets";
const OUTLINE: &str = "#e8590c";
const INK: &str = "#24292f";
const MUTED: &str = "#6e7781";
const GRID: &str = "#e4e7eb";

#[path = "figures_color.rs"]
mod color;

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "docs/figures".into());
    std::fs::create_dir_all(&out).unwrap();
    if std::env::args().any(|a| a == "--color") {
        color::write(&out);
        return;
    }
    let inter = std::fs::read(format!("{ASSETS}/Inter-Regular.otf")).unwrap();
    let font = Font::from_bytes(&inter, 0, FontScaling::EmNormalized).unwrap();

    let r = GlyphField::new(&font, 'R', 32.0, 4.0);
    let batch: Vec<BatchGlyph> = ['G', 'p', 'u']
        .into_iter()
        .map(|c| BatchGlyph::new(&GlyphField::new(&font, c, 32.0, 4.0), c))
        .collect();
    let figures = [
        ("pipeline.svg", pipeline()),
        ("coloring.svg", coloring(&r)),
        ("tiles.svg", tiles(&GlyphField::new(&font, 'R', 128.0, 4.0))),
        ("field.svg", field(&r)),
        ("median.svg", median_figure(&r)),
        ("correction.svg", correction(&GlyphField::new(&font, 'X', 24.0, 4.0))),
        ("shaping.svg", shaping(inter.clone())),
        ("batch.svg", batch_buffers(&batch)),
        ("lookup.svg", batch_lookup(&batch)),
        ("passes.svg", batch_passes()),
    ];
    for (name, svg) in figures {
        std::fs::write(format!("{out}/{name}"), svg).unwrap();
        println!("{out}/{name}");
    }
    match atlas(&inter) {
        Ok(png) => {
            std::fs::write(format!("{out}/atlas.png"), png).unwrap();
            println!("{out}/atlas.png");
        }
        Err(e) => eprintln!("skipping atlas.png: {e}"),
    }
}

/// A colored glyph framed in a field of `texels_per_em` texels per em with a `px_range` texel
/// distance range, as the GPU atlas frames them.
struct GlyphField {
    shape: Shape,
    prepared: PreparedShape,
    transform: Transform,
    w: u32,
    h: u32,
    px_range: f64,
}

impl GlyphField {
    fn new(font: &Font<'_>, c: char, texels_per_em: f64, px_range: f64) -> Self {
        let mut shape = font.glyph(c).unwrap().shape;
        shape.normalize();
        shape.orient_contours(FillRule::NonZero);
        edge_coloring(&mut shape, ColoringStrategy::InkTrap, DEFAULT_ANGLE_THRESHOLD, 0);
        let b = shape.bounds();
        let pad = 0.5 * px_range + 1.0;
        let s = texels_per_em;
        let transform = Transform::new(
            Projection::uniform(s, Vec2::new(pad / s - b.left, pad / s - b.bottom)),
            Range::symmetric(px_range / s),
        );
        GlyphField {
            prepared: PreparedShape::new(&shape),
            w: (b.width() * s + 2.0 * pad).ceil() as u32,
            h: (b.height() * s + 2.0 * pad).ceil() as u32,
            shape,
            transform,
            px_range,
        }
    }

    fn msdf(&self, config: &GeneratorConfig) -> Bitmap<3> {
        let mut out = Bitmap::new(self.w, self.h);
        generate_msdf(&mut out, &self.shape, &self.transform, config);
        out
    }

    /// Maps the field to the page: texel (0, 0)'s top-left corner at `origin`, `cell` px per texel.
    fn view(&self, origin: [f64; 2], cell: f64) -> View {
        View {
            proj: self.transform.projection,
            origin,
            cell,
            h: f64::from(self.h),
        }
    }
}

/// Field texel coordinates (y up) to page coordinates (y down).
#[derive(Clone, Copy)]
struct View {
    proj: Projection,
    origin: [f64; 2],
    cell: f64,
    h: f64,
}

impl View {
    fn texel(&self, x: f64, y_up: f64) -> (f64, f64) {
        (
            self.origin[0] + x * self.cell,
            self.origin[1] + (self.h - y_up) * self.cell,
        )
    }

    fn pt(&self, p: Vec2) -> (f64, f64) {
        let t = self.proj.project(p);
        self.texel(t.x, t.y)
    }

    /// SVG path commands for one edge, without the initial move.
    fn edge_tail(&self, e: &Edge) -> String {
        let f = |p: Vec2| {
            let (x, y) = self.pt(p);
            format!("{x:.2},{y:.2}")
        };
        match e.segment {
            Segment::Linear([_, b]) => format!("L{}", f(b)),
            Segment::Quadratic([_, c, b]) => format!("Q{} {}", f(c), f(b)),
            Segment::Cubic([_, c1, c2, b]) => format!("C{} {} {}", f(c1), f(c2), f(b)),
        }
    }

    fn edge(&self, e: &Edge) -> String {
        let (x, y) = self.pt(e.start());
        format!("M{x:.2},{y:.2}{}", self.edge_tail(e))
    }

    fn shape(&self, shape: &Shape) -> String {
        let mut d = String::new();
        for c in shape.contours.iter().filter(|c| !c.edges.is_empty()) {
            let (x, y) = self.pt(c.edges[0].start());
            write!(d, "M{x:.2},{y:.2}").unwrap();
            for e in &c.edges {
                d += &self.edge_tail(e);
            }
            d += "Z";
        }
        d
    }

    /// Texel grid lines of a `w` x `h` field.
    fn grid(&self, w: u32, h: u32, color: &str) -> String {
        let mut d = String::new();
        let [x0, y0] = self.origin;
        let (x1, y1) = (x0 + f64::from(w) * self.cell, y0 + f64::from(h) * self.cell);
        for i in 0..=w {
            let x = x0 + f64::from(i) * self.cell;
            write!(d, "M{x:.2},{y0:.2}V{y1:.2}").unwrap();
        }
        for j in 0..=h {
            let y = y0 + f64::from(j) * self.cell;
            write!(d, "M{x0:.2},{y:.2}H{x1:.2}").unwrap();
        }
        format!(r#"<path d="{d}" stroke="{color}" stroke-width="1" fill="none"/>"#)
    }
}

/// Display color of an edge color (white: all three channels).
fn edge_color(bits: u8) -> &'static str {
    match bits & 7 {
        1 => "#e5383b",
        2 => "#2b9348",
        3 => "#e0a800",
        4 => "#2f6fdf",
        5 => "#c2389b",
        6 => "#1798a8",
        _ => "#8c959f",
    }
}

fn edge_color_name(bits: u8) -> &'static str {
    match bits & 7 {
        1 => "red",
        2 => "green",
        3 => "yellow (red + green)",
        4 => "blue",
        5 => "magenta (red + blue)",
        6 => "cyan (green + blue)",
        _ => "white (all three)",
    }
}

/// An SVG document on a white rounded card, so it reads in light and dark themes.
struct Svg {
    body: String,
    w: f64,
    h: f64,
}

impl Svg {
    fn new(w: f64, h: f64) -> Self {
        Svg {
            body: String::new(),
            w,
            h,
        }
    }

    fn add(&mut self, s: impl AsRef<str>) {
        self.body += s.as_ref();
        self.body.push('\n');
    }

    fn text(&mut self, x: f64, y: f64, size: f64, color: &str, anchor: &str, weight: u32, s: &str) {
        self.add(format!(
            r#"<text x="{x:.1}" y="{y:.1}" font-size="{size}" fill="{color}" text-anchor="{anchor}" font-weight="{weight}">{}</text>"#,
            escape(s)
        ));
    }

    /// A heading and a muted subheading above a panel at `x`.
    fn heading(&mut self, x: f64, y: f64, title: &str, sub: &str) {
        self.text(x, y, 15.0, INK, "start", 600, title);
        if !sub.is_empty() {
            self.text(x, y + 18.0, 12.0, MUTED, "start", 400, sub);
        }
    }

    fn image(&mut self, x: f64, y: f64, w: f64, h: f64, png: &[u8]) {
        self.add(format!(
            r#"<image x="{x:.1}" y="{y:.1}" width="{w:.1}" height="{h:.1}" href="data:image/png;base64,{}"/>"#,
            base64(png)
        ));
    }

    fn finish(self) -> String {
        format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" width="{w}" height="{h}" font-family="-apple-system, 'Segoe UI', Helvetica, Arial, sans-serif">
<rect width="{w}" height="{h}" rx="10" fill="#ffffff"/>
{body}</svg>
"##,
            w = self.w,
            h = self.h,
            body = self.body
        )
    }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n =
            (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= c.len() {
                s.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

/// An RGB image enlarged `k` times with nearest-neighbor sampling (crisp texels in any viewer).
fn enlarge(w: u32, h: u32, rgb: &[u8], k: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((w * h * k * k * 3) as usize);
    for y in 0..h * k {
        for x in 0..w * k {
            let i = (((y / k) * w + x / k) * 3) as usize;
            out.extend_from_slice(&rgb[i..i + 3]);
        }
    }
    encode_png(w * k, h * k, 3, &out)
}

fn byte(v: f32) -> u8 {
    textra::float_to_byte(v)
}

/// A legend row of colored line swatches.
fn legend(svg: &mut Svg, x: f64, y: f64, items: &[(&str, &str)]) {
    let mut x = x;
    for &(color, label) in items {
        svg.add(format!(
            r#"<path d="M{x:.1},{y:.1}h22" stroke="{color}" stroke-width="4" stroke-linecap="round"/>"#
        ));
        svg.text(x + 30.0, y + 4.0, 12.0, MUTED, "start", 400, label);
        x += 40.0 + 6.5 * label.len() as f64;
    }
}

// ---------------------------------------------------------------------------------------------
// Figures

fn box_text(svg: &mut Svg, x: f64, y: f64, w: f64, h: f64, fill: &str, stroke: &str, lines: &[&str]) {
    svg.add(format!(
        r#"<rect x="{x:.1}" y="{y:.1}" width="{w:.1}" height="{h:.1}" rx="7" fill="{fill}" stroke="{stroke}" stroke-width="1.2"/>"#
    ));
    let top = y + h / 2.0 - 8.0 * (lines.len() as f64 - 1.0) + 4.5;
    for (i, l) in lines.iter().enumerate() {
        let (size, color, weight) = if i == 0 { (13.0, INK, 600) } else { (11.5, MUTED, 400) };
        svg.text(x + w / 2.0, top + 16.0 * i as f64, size, color, "middle", weight, l);
    }
}

fn arrow(svg: &mut Svg, d: &str, color: &str, dashed: bool) {
    let dash = if dashed { r#" stroke-dasharray="5 4""# } else { "" };
    svg.add(format!(
        r#"<path d="{d}" stroke="{color}" stroke-width="1.6" fill="none" marker-end="url(#arrow)"{dash}/>"#
    ));
}

/// The whole pipeline: what runs where, and how often.
fn pipeline() -> String {
    let (bw, bh, gap, x0) = (178.0, 62.0, 34.0, 28.0);
    let mut svg = Svg::new(x0 * 2.0 + 4.0 * bw + 3.0 * gap, 462.0);
    svg.add(format!(
        r#"<defs><marker id="arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0,0L10,5L0,10z" fill="{MUTED}"/></marker></defs>"#
    ));
    let col = |i: f64| x0 + i * (bw + gap);
    let lanes = [
        (
            30.0,
            "Every frame, monochrome text",
            "CPU shaping and layout; instanced draws",
            "#eef6ff",
            "#9cc3f5",
        ),
        (
            180.0,
            "Once per new glyph (or new size level)",
            "CPU, on worker threads while the app builds its frame",
            "#f3f0ff",
            "#b7a8ec",
        ),
        (
            330.0,
            "Once per frame, every new glyph in one batch",
            "GPU compute passes",
            "#effaf1",
            "#98d4a6",
        ),
    ];
    for &(y, title, sub, fill, _) in &lanes {
        svg.add(format!(
            r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="112" rx="9" fill="{fill}"/>"#,
            x0 - 14.0,
            y - 4.0,
            svg.w - 2.0 * x0 + 28.0
        ));
        svg.add(format!(
            r#"<text x="{x0}" y="{:.1}" font-size="13" fill="{INK}" font-weight="600">{title}<tspan fill="{MUTED}" font-size="12" font-weight="400"> · {sub}</tspan></text>"#,
            y + 14.0
        ));
    }
    let row = |l: usize| lanes[l].0 + 32.0;
    let boxes: [(usize, f64, &[&str]); 11] = [
        (0, 0.0, &["1  Shape", "HarfRust + bidi → glyph ids"]),
        (0, 1.0, &["2  Look up glyphs", "atlas key: font, glyph, level"]),
        (0, 3.0, &["9  Draw quads", "median, AA, outline, glow"]),
        (1, 0.0, &["3  Load outline", "lines, quadratics, cubics"]),
        (1, 1.0, &["4  Color edges", "corners change channels"]),
        (1, 2.0, &["5  Bin into 8×8 tiles", "exact candidate lists"]),
        (1, 3.0, &["", ""]),
        (2, 0.0, &["6  Distance pass", "nearest edge per channel"]),
        (2, 1.0, &["7  Error correction", "4 passes, msdfgen's rules"]),
        (2, 2.0, &["8  Store in atlas", "on-demand pages, LRU at cap"]),
        (2, 3.0, &["", ""]),
    ];
    for &(lane, c, lines) in &boxes {
        if lines[0].is_empty() {
            continue;
        }
        let (_, _, _, _, stroke) = lanes[lane];
        box_text(&mut svg, col(c), row(lane), bw, bh, "#ffffff", stroke, lines);
    }
    // The cache between generation and drawing.
    let (ax, ay) = (col(3.0), row(2));
    box_text(
        &mut svg,
        ax,
        ay,
        bw,
        bh,
        "#fff7e6",
        "#f0b45a",
        &["Glyph atlas", "up to 2 × 1024² pages"],
    );
    let mid = |l: usize| row(l) + bh / 2.0;
    for (l, a, b) in [
        (0, 0.0, 1.0),
        (1, 0.0, 1.0),
        (1, 1.0, 2.0),
        (2, 0.0, 1.0),
        (2, 1.0, 2.0),
        (2, 2.0, 3.0),
    ] {
        arrow(
            &mut svg,
            &format!("M{:.1},{:.1}H{:.1}", col(a) + bw, mid(l), col(b) - 3.0),
            MUTED,
            false,
        );
    }
    // Hit: straight to drawing.
    arrow(
        &mut svg,
        &format!("M{:.1},{:.1}H{:.1}", col(2.0), mid(0), col(3.0) - 3.0),
        MUTED,
        false,
    );
    svg.text(
        col(2.0) + 6.0,
        mid(0) - 8.0,
        11.5,
        MUTED,
        "start",
        400,
        "cached: draw right away",
    );
    // Miss: prepare the glyph.
    arrow(
        &mut svg,
        &format!(
            "M{:.1},{:.1}V{:.1}H{:.1}V{:.1}",
            col(1.0) + bw / 2.0,
            row(0) + bh,
            row(0) + bh + 33.0,
            col(0.0) + bw / 2.0,
            row(1) - 3.0
        ),
        OUTLINE,
        true,
    );
    svg.text(
        col(1.0) + bw / 2.0 + 8.0,
        row(0) + bh + 29.0,
        11.5,
        OUTLINE,
        "start",
        400,
        "missing: placed now, generated this frame",
    );
    // Prepared glyphs go to the GPU batch.
    arrow(
        &mut svg,
        &format!(
            "M{:.1},{:.1}V{:.1}H{:.1}V{:.1}",
            col(2.0) + bw / 2.0,
            row(1) + bh,
            row(1) + bh + 33.0,
            col(0.0) + bw / 2.0,
            row(2) - 3.0
        ),
        MUTED,
        false,
    );
    // The atlas feeds the draw.
    arrow(
        &mut svg,
        &format!("M{:.1},{:.1}V{:.1}", ax + bw / 2.0, ay, row(0) + bh + 3.0),
        "#f0b45a",
        false,
    );
    svg.text(
        ax + bw / 2.0 + 8.0,
        row(1) + 4.0,
        11.5,
        "#c98a1c",
        "start",
        400,
        "sampled",
    );
    svg.finish()
}

/// Step 4: edge coloring of the outline.
fn coloring(g: &GlyphField) -> String {
    let cell = 13.0;
    let (pw, ph) = (f64::from(g.w) * cell, f64::from(g.h) * cell);
    let mut svg = Svg::new(pw + 250.0, ph + 84.0);
    svg.heading(
        20.0,
        30.0,
        "Edge coloring",
        "Inter 'R', 32 texels per em; each edge feeds two of the three channels",
    );
    let v = g.view([20.0, 62.0], cell);
    svg.add(v.grid(g.w, g.h, GRID));
    let mut used = Vec::new();
    for c in &g.shape.contours {
        for e in &c.edges {
            svg.add(format!(
                r#"<path d="{}" stroke="{}" stroke-width="3.2" fill="none" stroke-linecap="round"/>"#,
                v.edge(e),
                edge_color(e.color.bits())
            ));
            if !used.contains(&e.color.bits()) {
                used.push(e.color.bits());
            }
        }
    }
    // Corners where the color changes: the sharp corners the field will keep.
    for c in &g.shape.contours {
        let n = c.edges.len();
        for (i, e) in c.edges.iter().enumerate() {
            if c.edges[(i + n - 1) % n].color != e.color {
                let (x, y) = v.pt(e.start());
                svg.add(format!(r#"<circle cx="{x:.2}" cy="{y:.2}" r="3.6" fill="{INK}"/>"#));
            }
        }
    }
    used.sort_unstable();
    let lx = 20.0 + pw + 28.0;
    for (i, &b) in used.iter().enumerate() {
        legend(
            &mut svg,
            lx,
            80.0 + 24.0 * i as f64,
            &[(edge_color(b), edge_color_name(b))],
        );
    }
    let y = 80.0 + 24.0 * used.len() as f64;
    svg.add(format!(
        r#"<circle cx="{:.1}" cy="{y:.1}" r="3.6" fill="{INK}"/>"#,
        lx + 11.0
    ));
    svg.text(lx + 30.0, y + 4.0, 12.0, MUTED, "start", 400, "color changes: a corner");
    svg.text(lx + 30.0, y + 20.0, 12.0, MUTED, "start", 400, "that stays sharp");
    svg.finish()
}

/// Step 5: per-tile candidate lists, and the far tiles that only need a sign.
fn tiles(g: &GlyphField) -> String {
    let tile = 8;
    let bins = bin_tiles(&g.prepared, &g.transform, g.w, g.h, tile, FieldKind::MSDF, true);
    let cell = 5.0;
    let (pw, ph) = (f64::from(g.w) * cell, f64::from(g.h) * cell);
    let mut svg = Svg::new((pw + 40.0).max(560.0), ph + 116.0);
    let v = g.view([20.0, 62.0], cell);
    let edges: Vec<&Edge> = g.shape.contours.iter().flat_map(|c| &c.edges).collect();
    let s = g.transform.projection.scale.x;
    // The atlas evaluates distances only within this band of the outline (in texels).
    let band = 0.5 * g.px_range + 2.0;
    let groups = bins.groups as usize;
    let tile_count = (bins.tiles_x * bins.tiles_y) as usize;
    let candidates = |t: usize| -> Vec<u32> {
        bins.ranges[t * groups..(t + 1) * groups]
            .iter()
            .flat_map(|r| {
                bins.indices[r.offset as usize..(r.offset + r.count) as usize]
                    .iter()
                    .copied()
            })
            .collect()
    };
    let far = |t: usize| f64::from(bins.min_key[t]).sqrt() * s > band;
    // Highlight the interior near tile with the shortest list.
    let interior = |t: usize| {
        let (tx, ty) = (t as u32 % bins.tiles_x, t as u32 / bins.tiles_x);
        (1..bins.tiles_x - 1).contains(&tx) && (1..bins.tiles_y - 1).contains(&ty)
    };
    let pick = (0..tile_count)
        .filter(|&t| !far(t) && interior(t))
        .min_by_key(|&t| (candidates(t).len(), t))
        .unwrap();
    svg.heading(
        20.0,
        30.0,
        "Tile binning",
        &format!(
            "Inter 'R', 128 texels per em: the highlighted tile checks {} of the {} edges",
            candidates(pick).len(),
            edges.len()
        ),
    );
    let tsz = cell * f64::from(tile);
    for t in 0..tile_count {
        let (tx, ty) = (t as u32 % bins.tiles_x, t as u32 / bins.tiles_x);
        let x = 20.0 + f64::from(tx) * tsz;
        let y = 62.0 + f64::from(ty) * tsz;
        let w = tsz.min(pw - f64::from(tx) * tsz);
        let h = tsz.min(ph - f64::from(ty) * tsz);
        let fill = if t == pick {
            "#fde3d3"
        } else if far(t) {
            "#f2f3f5"
        } else {
            "#ffffff"
        };
        svg.add(format!(
            r#"<rect x="{x:.1}" y="{y:.1}" width="{w:.1}" height="{h:.1}" fill="{fill}"/>"#
        ));
    }
    svg.add(format!(
        r##"<path d="{}" fill="#d0d7de" fill-opacity="0.35" stroke="#afb8c1" stroke-width="1.2"/>"##,
        v.shape(&g.shape)
    ));
    for i in candidates(pick) {
        let e = edges[i as usize];
        svg.add(format!(
            r#"<path d="{}" stroke="{}" stroke-width="3.4" fill="none" stroke-linecap="round"/>"#,
            v.edge(e),
            edge_color(e.color.bits())
        ));
    }
    for t in 0..tile_count {
        let (tx, ty) = (t as u32 % bins.tiles_x, t as u32 / bins.tiles_x);
        let x = 20.0 + f64::from(tx) * tsz;
        let y = 62.0 + f64::from(ty) * tsz;
        let w = tsz.min(pw - f64::from(tx) * tsz);
        let h = tsz.min(ph - f64::from(ty) * tsz);
        svg.add(format!(
            r##"<rect x="{x:.1}" y="{y:.1}" width="{w:.1}" height="{h:.1}" fill="none" stroke="#57606a" stroke-width="1.4"/>"##
        ));
        let (label, color) = if far(t) {
            ("sign".to_string(), "#8c959f")
        } else {
            (candidates(t).len().to_string(), if t == pick { OUTLINE } else { INK })
        };
        let size = if far(t) { 9.0 } else { 12.0 };
        svg.add(format!(
            r##"<text x="{:.1}" y="{:.1}" font-size="{size}" fill="{color}" text-anchor="middle" font-weight="600" stroke="#ffffff" stroke-width="3" paint-order="stroke">{label}</text>"##,
            x + w / 2.0,
            y + h / 2.0 + 4.0
        ));
    }
    svg.text(
        22.0,
        62.0 + ph + 26.0,
        12.0,
        MUTED,
        "start",
        400,
        "number: edges that could be nearest somewhere in the tile (sorted; the scan stops early)",
    );
    svg.text(
        22.0,
        62.0 + ph + 44.0,
        12.0,
        MUTED,
        "start",
        400,
        &format!("sign: farther than {band} texels from the outline, so only inside / outside is computed"),
    );
    svg.finish()
}

/// Step 6: the field itself, and what each channel holds.
fn field(g: &GlyphField) -> String {
    let msdf = g.msdf(&GeneratorConfig::default());
    let cell = 13.0;
    let (pw, ph) = (f64::from(g.w) * cell, f64::from(g.h) * cell);
    // Three channel panels stacked beside the field.
    let slot = ph / 3.0;
    let ccell = (slot - 12.0) / f64::from(g.h);
    let (iw, ih) = (f64::from(g.w) * ccell, f64::from(g.h) * ccell);
    let mut svg = Svg::new(pw + iw + 120.0, ph + 92.0);
    svg.heading(
        20.0,
        30.0,
        "The multi-channel distance field",
        "One RGB texel per square; each channel alone is a distorted 'R'",
    );
    let rgb: Vec<u8> = msdf.data().iter().map(|&v| byte(v)).collect();
    svg.image(20.0, 62.0, pw, ph, &enlarge(g.w, g.h, &rgb, cell as u32));
    let v = g.view([20.0, 62.0], cell);
    svg.add(format!(
        r##"<path d="{}" fill="none" stroke="#ffffff" stroke-opacity="0.85" stroke-width="1.3"/>"##,
        v.shape(&g.shape)
    ));
    // Each channel thresholded at its own edge, against the true outline.
    let k = 6;
    let (rw, rh) = (g.w * k, g.h * k);
    let channels = textra::render_sdf_multi(&msdf, rw, rh, g.px_range, 0.5);
    let x = 20.0 + pw + 32.0;
    for (c, (name, color)) in [
        ("red", [229u8, 56, 59]),
        ("green", [43, 147, 72]),
        ("blue", [47, 111, 223]),
    ]
    .into_iter()
    .enumerate()
    {
        // Inside the channel's shape: its color; outside: white.
        let px: Vec<u8> = channels
            .data()
            .chunks(3)
            .flat_map(|p| color.map(|ch| byte(1.0 - p[c] * (1.0 - f32::from(ch) / 255.0))))
            .collect();
        let y = 62.0 + c as f64 * slot;
        svg.image(x, y, iw, ih, &encode_png(rw, rh, 3, &px));
        let cv = g.view([x, y], ccell);
        svg.add(format!(
            r#"<path d="{}" fill="none" stroke="{INK}" stroke-opacity="0.7" stroke-width="1"/>"#,
            cv.shape(&g.shape)
        ));
        svg.text(x + iw + 8.0, y + ih / 2.0 + 4.0, 12.0, MUTED, "start", 400, name);
    }
    svg.text(
        22.0,
        62.0 + ph + 22.0,
        12.0,
        MUTED,
        "start",
        400,
        "white line: the exact outline; values are 0.5 on it, brighter inside",
    );
    svg.finish()
}

/// Rendering, step 9: why the median of three channels keeps corners that one channel rounds.
fn median_figure(g: &GlyphField) -> String {
    let mut sdf = Bitmap::<1>::new(g.w, g.h);
    generate_sdf(&mut sdf, &g.shape, &g.transform, &GeneratorConfig::default());
    let msdf = g.msdf(&GeneratorConfig::default());
    // The stem's top left corner: the outline vertex farthest up and left.
    let corner = g
        .shape
        .contours
        .iter()
        .flat_map(|c| &c.edges)
        .map(Edge::start)
        .min_by(|a, b| (a.x - a.y).total_cmp(&(b.x - b.y)))
        .unwrap();
    let t = g.transform.projection.project(corner);
    let span = 6.0; // texels shown
    let (cx, cy) = (t.x.round() - 2.0, t.y.round() + 2.0); // the corner sits near the crop's top left
    let side = 300.0;
    let px = 300;
    let cell = side / span;
    let render = |f: &dyn Fn(f64, f64) -> f32| -> Vec<u8> {
        let mut out = Vec::with_capacity(px * px * 3);
        for j in 0..px {
            for i in 0..px {
                // Field coordinates (x right, rows down; texel centers at +0.5).
                let fx = cx + (i as f64 + 0.5) / px as f64 * span;
                let fy = f64::from(g.h) - (cy - (j as f64 + 0.5) / px as f64 * span);
                let d = f(fx, fy);
                let screen_range = g.px_range * f64::from(px as u32) / span;
                let a = ((f64::from(d) - 0.5) * screen_range + 0.5).clamp(0.0, 1.0);
                let c = byte((1.0 - a * 0.92) as f32);
                out.extend_from_slice(&[c, c, c]);
            }
        }
        encode_png(px as u32, px as u32, 3, &out)
    };
    let one = render(&|x, y| sdf.sample(x, y)[0]);
    let three = render(&|x, y| {
        let s = msdf.sample(x, y);
        median(s[0], s[1], s[2])
    });
    let mut svg = Svg::new(2.0 * side + 60.0, side + 120.0);
    svg.heading(
        20.0,
        30.0,
        "Sharp corners from three channels",
        "The stem's top corner of the 'R' above, magnified: 6 × 6 texels",
    );
    for (i, (png, label)) in [
        (one, "one channel (true distance): rounded"),
        (three, "median of three (MSDF): sharp"),
    ]
    .iter()
    .enumerate()
    {
        let x = 20.0 + i as f64 * (side + 20.0);
        let y = 62.0;
        svg.add(format!(
            r#"<clipPath id="c{i}"><rect x="{x}" y="{y}" width="{side}" height="{side}"/></clipPath>"#
        ));
        svg.image(x, y, side, side, png);
        // Texel grid and outline, in a view whose origin puts texel (cx, cy) at the panel corner.
        let v = View {
            proj: g.transform.projection,
            origin: [x - cx * cell, y + (cy - f64::from(g.h)) * cell],
            cell,
            h: f64::from(g.h),
        };
        let mut d = String::new();
        for k in 1..span as u32 {
            let o = f64::from(k) * cell;
            write!(d, "M{:.1},{y}V{:.1}M{x},{:.1}H{:.1}", x + o, y + side, y + o, x + side).unwrap();
        }
        svg.add(format!(
            r##"<g clip-path="url(#c{i})"><path d="{d}" stroke="#ffffff" stroke-opacity="0.18" stroke-width="1"/><path d="{}" fill="none" stroke="{OUTLINE}" stroke-width="2"/></g>"##,
            v.shape(&g.shape)
        ));
        svg.add(format!(
            r##"<rect x="{x}" y="{y}" width="{side}" height="{side}" fill="none" stroke="#d0d7de"/>"##
        ));
        svg.text(x + side / 2.0, y + side + 22.0, 12.5, INK, "middle", 500, label);
    }
    svg.text(
        22.0,
        62.0 + side + 46.0,
        12.0,
        MUTED,
        "start",
        400,
        "orange: the exact outline · faint lines: texels",
    );
    svg.finish()
}

/// Step 7: error correction removes artifacts where channels cross between texels.
fn correction(g: &GlyphField) -> String {
    let off = g.msdf(&GeneratorConfig::default().with_error_correction(ErrorCorrectionConfig::DISABLED));
    let on = g.msdf(&GeneratorConfig::default());
    let k = 12;
    let cell = f64::from(k);
    let (pw, ph) = (f64::from(g.w) * cell, f64::from(g.h) * cell);
    let mut svg = Svg::new(2.0 * pw + 60.0, ph + 118.0);
    let changed: Vec<(u32, u32)> = (0..g.h)
        .flat_map(|y| (0..g.w).map(move |x| (x, y)))
        .filter(|&(x, y)| off.pixel(x, y) != on.pixel(x, y))
        .collect();
    svg.heading(
        20.0,
        30.0,
        "Error correction",
        &format!(
            "Inter 'X', 24 texels per em, drawn {k}× magnified; {} texels corrected",
            changed.len()
        ),
    );
    for (i, (f, label)) in [(&off, "without correction"), (&on, "with correction")]
        .into_iter()
        .enumerate()
    {
        let x = 20.0 + i as f64 * (pw + 20.0);
        let y = 62.0;
        let cov = textra::render_sdf(f, g.w * k, g.h * k, g.px_range, 0.5);
        let px: Vec<u8> = cov
            .data()
            .iter()
            .flat_map(|&a| {
                let c = byte(1.0 - a * 0.92);
                [c, c, c]
            })
            .collect();
        svg.image(x, y, pw, ph, &encode_png(g.w * k, g.h * k, 3, &px));
        let v = g.view([x, y], cell);
        svg.add(format!(
            r#"<path d="{}" fill="none" stroke="{OUTLINE}" stroke-width="1.6"/>"#,
            v.shape(&g.shape)
        ));
        if i == 1 {
            let mut d = String::new();
            for &(tx, ty) in &changed {
                write!(
                    d,
                    "M{:.1},{:.1}h{cell}v{cell}h-{cell}z",
                    x + f64::from(tx) * cell,
                    y + f64::from(ty) * cell
                )
                .unwrap();
            }
            svg.add(format!(
                r##"<path d="{d}" fill="none" stroke="#2f6fdf" stroke-width="1.6"/>"##
            ));
        }
        svg.add(format!(
            r##"<rect x="{x}" y="{y}" width="{pw}" height="{ph}" fill="none" stroke="#d0d7de"/>"##
        ));
        svg.text(x + pw / 2.0, y + ph + 22.0, 12.5, INK, "middle", 500, label);
    }
    svg.text(
        22.0,
        62.0 + ph + 46.0,
        12.0,
        MUTED,
        "start",
        400,
        "orange: the exact outline · blue squares: texels flattened to their median",
    );
    svg.finish()
}

/// Steps 1 and 9: shaping maps characters to glyphs and orders them for display.
fn shaping(inter: Vec<u8>) -> String {
    let arabic = std::fs::read(format!("{ASSETS}/NotoSansArabic-Subset.ttf")).unwrap();
    let latin = ShapingFont::new(0, Arc::new(inter), 0).unwrap();
    let arabic = ShapingFont::new(1, Arc::new(arabic), 0).unwrap();
    let mut shaper = Shaper::new();
    let mut svg = Svg::new(940.0, 330.0);
    svg.add(format!(
        r#"<defs><marker id="arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse"><path d="M0,0L10,5L0,10z" fill="{MUTED}"/></marker></defs>"#
    ));
    svg.heading(
        20.0,
        30.0,
        "Shaping",
        "Characters in memory order (top) become positioned glyphs in screen order (bottom)",
    );
    let panels = [
        (
            &latin,
            "Contextual alternates (Inter): \"->\" becomes one arrow",
            "Ta->b",
            20.0,
        ),
        (
            &arabic,
            "Right to left, joining forms, dots as marks (Noto Sans Arabic)",
            "تكبير",
            470.0,
        ),
    ];
    for (font, title, text, x0) in panels {
        let line = shaper.shape_line(font, text);
        shaping_panel(&mut svg, font, &line, title, text, x0, 76.0);
    }
    svg.finish()
}

fn shaping_panel(
    svg: &mut Svg,
    font: &ShapingFont,
    line: &textra_gpu::ShapedLine,
    title: &str,
    text: &str,
    x0: f64,
    y0: f64,
) {
    svg.text(x0, y0, 13.0, INK, "start", 600, title);
    let chars: Vec<(usize, char)> = text.char_indices().filter(|(_, c)| *c != ' ').collect();
    let cw = 38.0;
    let gap = 8.0;
    let cy = y0 + 20.0;
    let mut char_pos = Vec::new();
    for (i, &(_, c)) in chars.iter().enumerate() {
        let x = x0 + i as f64 * (cw + gap);
        svg.add(format!(
            r##"<rect x="{x:.1}" y="{cy:.1}" width="{cw}" height="{cw}" rx="5" fill="#ffffff" stroke="#afb8c1"/>"##
        ));
        svg.text(
            x + cw / 2.0,
            cy + cw / 2.0 + 7.0,
            20.0,
            INK,
            "middle",
            400,
            &c.to_string(),
        );
        char_pos.push(x + cw / 2.0);
    }
    svg.text(
        x0,
        cy + cw + 16.0,
        11.0,
        MUTED,
        "start",
        400,
        &format!("{} characters", chars.len()),
    );
    // Glyph boxes sized by advance, in screen order.
    let em = 44.0;
    let gy = cy + 128.0;
    let gh = 60.0;
    let mut x = x0;
    let mut glyph_pos = Vec::new();
    for sg in &line.glyphs {
        let w = (f64::from(sg.advance) * em + 12.0).max(26.0);
        let ligature = chars
            .iter()
            .filter(|&&(o, _)| cluster_of(line, o) == Some(sg.cluster))
            .count()
            > 1;
        let (fill, stroke) = if ligature {
            ("#fde3d3", OUTLINE)
        } else {
            ("#ffffff", "#57606a")
        };
        svg.add(format!(
            r#"<rect x="{x:.1}" y="{gy:.1}" width="{w:.1}" height="{gh}" rx="5" fill="{fill}" stroke="{stroke}"/>"#
        ));
        if let Some(shape) = font.outline(sg.glyph) {
            let b = shape.bounds();
            let ox = x + w / 2.0 - f64::midpoint(b.left, b.right) * em;
            let base = gy + gh - 16.0;
            let v = View {
                proj: Projection::uniform(em, Vec2::new(ox / em, 0.0)),
                origin: [0.0, base],
                cell: 1.0,
                h: 0.0,
            };
            svg.add(format!(r#"<path d="{}" fill="{INK}"/>"#, v.shape(&shape)));
        }
        glyph_pos.push((x + w / 2.0, sg.cluster));
        x += w + gap;
    }
    svg.text(
        x0,
        gy + gh + 18.0,
        11.0,
        MUTED,
        "start",
        400,
        &format!("{} glyphs, one quad each", line.glyphs.len()),
    );
    for (i, &(offset, _)) in chars.iter().enumerate() {
        let cluster = cluster_of(line, offset);
        for &(gx, _) in glyph_pos.iter().filter(|&&(_, c)| Some(c) == cluster) {
            let crossing = (char_pos[i] - gx).abs() > cw;
            arrow(
                svg,
                &format!("M{:.1},{:.1}L{:.1},{:.1}", char_pos[i], cy + cw + 24.0, gx, gy - 3.0),
                if crossing { "#2f6fdf" } else { MUTED },
                false,
            );
        }
    }
}

/// The cluster (byte offset) the character at `offset` was shaped into.
fn cluster_of(line: &textra_gpu::ShapedLine, offset: usize) -> Option<u32> {
    line.glyphs
        .iter()
        .map(|g| g.cluster)
        .filter(|&c| c as usize <= offset)
        .max()
}

/// Step 8: a corner of the GPU atlas after requesting text at several sizes.
fn atlas(inter: &[u8]) -> Result<Vec<u8>, String> {
    let (_, device, queue) = default_device()?;
    let config = AtlasConfig {
        detail_probe: false,
        ..AtlasConfig::default()
    };
    let mut atlas = GlyphAtlas::new(&device, config).expect("atlas configuration");
    let jp = std::fs::read(format!("{ASSETS}/NotoSansJP-Subset.ttf")).unwrap();
    let fonts = [
        Font::from_bytes(inter, 0, FontScaling::EmNormalized).unwrap(),
        Font::from_bytes(&jp, 0, FontScaling::EmNormalized).unwrap(),
    ];
    let requests: [(u32, &str, f64); 6] = [
        (0, "textra-gpu", 220.0),
        (1, "拡大と滑走", 120.0),
        (0, "Glyphs at every size", 72.0),
        (0, "Sharp corners at any zoom · outlines · glows", 30.0),
        (1, "いろはにほへと ちりぬるを", 30.0),
        (0, "abcdefghijklmnopqrstuvwxyz 0123456789", 14.0),
    ];
    atlas.begin_frame();
    for &(f, text, px) in &requests {
        let font = &fonts[f as usize];
        for c in text.chars() {
            let Some(gid) = font.glyph_index(c) else { continue };
            let key = GlyphKey {
                font: f,
                glyph: u32::from(gid),
            };
            atlas.get(key, px, || font.glyph_by_index(gid).ok().map(|g| g.shape));
        }
    }
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    atlas.flush(&device, &mut enc);
    queue.submit([enc.finish()]);
    let size = atlas.config().size;
    let px = read_texture_layer(&device, &queue, atlas.texture(), 0);
    // Crop to the filled part.
    let (mut w, mut h) = (0, 0);
    for y in 0..size {
        for x in 0..size {
            if px[((y * size + x) * 4 + 3) as usize] != 0 {
                w = w.max(x + 1);
                h = h.max(y + 1);
            }
        }
    }
    let rgb: Vec<u8> = (0..h)
        .flat_map(|y| (0..w).map(move |x| (y * size + x) as usize * 4))
        .flat_map(|i| [px[i], px[i + 1], px[i + 2]])
        .collect();
    Ok(encode_png(w, h, 3, &rgb))
}

// ---------------------------------------------------------------------------------------------
// GPU batch figures: the buffers of one generation batch, built from real glyphs the way
// textra-gpu's prepare_job and FieldGenerator::upload lay them out.

/// Bytes of the WGSL `Glyph` record (common.wgsl: 36 four-byte fields).
const GLYPH_RECORD: usize = 144;

/// One glyph's share of each batch buffer, in elements.
struct BatchGlyph {
    name: char,
    w: u32,
    h: u32,
    tiles_x: u32,
    tiles_y: u32,
    groups: u32,
    edges: usize,
    contours: usize,
    curves: usize,
    tile_groups: usize,
    cands: usize,
    corners: usize,
    bins: textra::gpu::TileBins,
}

impl BatchGlyph {
    fn new(g: &GlyphField, name: char) -> Self {
        let gpu = textra::gpu::GpuShape::new(&g.prepared);
        let bins = bin_tiles(&g.prepared, &g.transform, g.w, g.h, 8, FieldKind::MTSDF, true);
        let corners = textra::gpu::corner_texels(&g.prepared, &g.transform, g.w, g.h);
        // Sign correction: a start index per row (and one past the last), then (x, dir) pairs.
        let mut crossings = 0;
        let mut row = Vec::new();
        for r in 0..g.h {
            let y = g
                .transform
                .projection
                .unproject(Vec2::new(0.0, f64::from(g.h - 1 - r) + 0.5))
                .y;
            g.prepared.scanline(y, &mut row);
            crossings += row.len();
        }
        BatchGlyph {
            name,
            w: g.w,
            h: g.h,
            tiles_x: bins.tiles_x,
            tiles_y: bins.tiles_y,
            groups: bins.groups,
            edges: gpu.edges.len(),
            contours: gpu.contours.len(),
            curves: gpu.curves.len() + g.h as usize + 1 + 2 * crossings,
            tile_groups: bins.ranges.len(),
            cands: bins.indices.len(),
            corners: corners.len(),
            bins,
        }
    }

    fn texels(&self) -> usize {
        (self.w * self.h) as usize
    }

    fn tiles(&self) -> usize {
        (self.tiles_x * self.tiles_y) as usize
    }
}

const GLYPH_HUES: [&str; 3] = ["#3b82c4", "#e07b1f", "#2a9d8f"];
const GLYPH_TINTS: [&str; 3] = ["#dbe8f6", "#fbe5cf", "#d6efec"];

fn kb(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    }
}

/// The ten storage buffers of one batch, each the concatenation of every glyph's part, with
/// one glyph's record offsets pointing into them.
fn batch_buffers(glyphs: &[BatchGlyph]) -> String {
    let edge = size_of::<textra::gpu::GpuEdge>();
    let contour = size_of::<textra::gpu::GpuContour>();
    let group = size_of::<textra::gpu::GpuTileGroup>();
    let sel = 1; // the glyph whose offsets are shown
    let prefix = |f: &dyn Fn(&BatchGlyph) -> usize| glyphs[..sel].iter().map(f).sum::<usize>();
    // (binding, name, element, bytes per element, count per glyph, offset label for `sel`,
    // written by the GPU)
    type Row<'a> = (u32, &'a str, &'a str, usize, Vec<usize>, String, bool);
    let count = |f: &dyn Fn(&BatchGlyph) -> usize| glyphs.iter().map(f).collect::<Vec<_>>();
    let rows: Vec<Row<'_>> = vec![
        (
            0,
            "glyphs",
            "Glyph record",
            GLYPH_RECORD,
            count(&|_| 1),
            format!("glyphs[{sel}]"),
            false,
        ),
        (
            1,
            "work",
            "(glyph, tile)",
            8,
            count(&|g| g.tiles()),
            format!("({sel}, 0 … {})", glyphs[sel].tiles() - 1),
            false,
        ),
        (
            2,
            "edges",
            "Edge",
            edge,
            count(&|g| g.edges),
            format!("edge_offset = {}", prefix(&|g| g.edges)),
            false,
        ),
        (
            3,
            "contours",
            "Contour",
            contour,
            count(&|g| g.contours),
            format!("contour_offset = {}", prefix(&|g| g.contours)),
            false,
        ),
        (
            4,
            "curves",
            "f32",
            4,
            count(&|g| g.curves),
            format!("curve_offset = {}", prefix(&|g| g.curves)),
            false,
        ),
        (
            5,
            "tile_groups",
            "TileGroup",
            group,
            count(&|g| g.tile_groups),
            format!("tile_offset = {}", prefix(&|g| g.tile_groups)),
            false,
        ),
        (
            6,
            "cands",
            "(edge, bound)",
            8,
            count(&|g| g.cands),
            format!("cand_offset = {}", prefix(&|g| g.cands)),
            false,
        ),
        (
            7,
            "field",
            "vec4<f32> texel",
            16,
            count(&|g| g.texels()),
            format!("field_offset = {}", prefix(&|g| g.texels())),
            true,
        ),
        (
            8,
            "stencil",
            "u32 flags",
            4,
            count(&|g| g.texels()),
            format!("field_offset = {}", prefix(&|g| g.texels())),
            true,
        ),
        (
            9,
            "corners",
            "u32 texel",
            4,
            count(&|g| g.corners),
            "batch texel indices".into(),
            false,
        ),
    ];
    let max_bytes = rows.iter().map(|r| r.3 * r.4.iter().sum::<usize>()).max().unwrap() as f64;
    let (x0, bar_x, bar_max, row_h) = (20.0, 250.0, 560.0, 44.0);
    let top = 96.0;
    let mut svg = Svg::new(940.0, top + rows.len() as f64 * row_h + 150.0);
    svg.heading(
        20.0,
        30.0,
        "One batch: every glyph generated in a frame, in ten storage buffers",
        &format!(
            "Real sizes for Inter 'G', 'p' and 'u' at 32 texels per em; bar length ~ sqrt(bytes), split per glyph; glyph {sel}'s record offsets shown"
        ),
    );
    svg.text(
        bar_x,
        top - 14.0,
        11.5,
        MUTED,
        "start",
        600,
        "BINDING · BUFFER · ELEMENT",
    );
    for (i, (binding, name, element, size, counts, offset, gpu_written)) in rows.iter().enumerate() {
        let y = top + i as f64 * row_h;
        let total: usize = counts.iter().sum();
        let bytes = total * size;
        svg.text(x0, y + 17.0, 13.0, INK, "start", 600, &format!("{binding}  {name}"));
        svg.text(
            x0 + 18.0,
            y + 33.0,
            11.5,
            MUTED,
            "start",
            400,
            &format!("{element} · {size} B"),
        );
        let len = (bar_max * (bytes as f64 / max_bytes).sqrt()).max(36.0);
        let mut x = bar_x;
        for (k, &c) in counts.iter().enumerate() {
            let w = len * c as f64 / total.max(1) as f64;
            let (fill, stroke) = if *gpu_written {
                (GLYPH_TINTS[k], GLYPH_HUES[k])
            } else {
                (GLYPH_HUES[k], "#ffffff")
            };
            svg.add(format!(
                r#"<rect x="{x:.1}" y="{:.1}" width="{w:.1}" height="20" fill="{fill}" stroke="{stroke}" stroke-width="1.2"/>"#,
                y + 4.0
            ));
            if w > 30.0 {
                let color = if *gpu_written { GLYPH_HUES[k] } else { "#ffffff" };
                svg.text(x + w / 2.0, y + 18.0, 11.0, color, "middle", 600, &c.to_string());
            }
            if k == sel {
                svg.add(format!(
                    r#"<path d="M{x:.1},{:.1}v-7" stroke="{}" stroke-width="1.5"/>"#,
                    y + 4.0,
                    GLYPH_HUES[k]
                ));
                svg.text(x + 3.0, y + 0.5, 10.5, GLYPH_HUES[k], "start", 600, offset);
            }
            x += w;
        }
        svg.text(bar_x + len + 10.0, y + 18.0, 12.0, INK, "start", 500, &kb(bytes));
    }
    let mut y = top + rows.len() as f64 * row_h + 14.0;
    for (k, g) in glyphs.iter().enumerate() {
        svg.add(format!(
            r#"<rect x="{x0:.1}" y="{:.1}" width="14" height="14" fill="{}"/>"#,
            y - 11.0,
            GLYPH_HUES[k]
        ));
        let contours = if g.contours == 1 { "contour" } else { "contours" };
        let label = format!(
            "glyph {k}: '{}', {}×{} texels, {}×{} tiles of 8×8, {} {contours}",
            g.name, g.w, g.h, g.tiles_x, g.tiles_y, g.contours
        );
        svg.text(x0 + 20.0, y, 12.0, INK, "start", 400, &label);
        y += 18.0;
    }
    y += 8.0;
    svg.text(
        x0,
        y + 26.0,
        12.0,
        MUTED,
        "start",
        400,
        "Solid: uploaded each flush (one staging buffer, copied in the command encoder, in order with the passes).",
    );
    svg.text(
        x0,
        y + 44.0,
        12.0,
        MUTED,
        "start",
        400,
        "Outlined: written by the GPU passes. Buffers are reused from batch to batch, grown to the next power of two.",
    );
    svg.text(
        x0,
        y + 62.0,
        12.0,
        MUTED,
        "start",
        400,
        "curves holds each cubic's interval data, then its sign-correction scanlines (scan_offset).",
    );
    svg.finish()
}

/// How one invocation finds its data: from its work item to its texel in the atlas.
fn batch_lookup(glyphs: &[BatchGlyph]) -> String {
    let sel = 1;
    let g = &glyphs[sel];
    let groups = g.groups as usize;
    // A tile with candidates (near the outline), the one with the fewest.
    let tile = (0..g.tiles())
        .filter(|&t| (0..groups).any(|c| g.bins.ranges[t * groups + c].count > 0))
        .min_by_key(|&t| (0..groups).map(|c| g.bins.ranges[t * groups + c].count).sum::<u32>())
        .unwrap();
    let k = glyphs[..sel].iter().map(BatchGlyph::tiles).sum::<usize>() + tile;
    let (tx, ty) = (tile as u32 % g.tiles_x, tile as u32 / g.tiles_x);
    let tile_offset: usize = glyphs[..sel].iter().map(|g| g.tile_groups).sum();
    let cand_offset: usize = glyphs[..sel].iter().map(|g| g.cands).sum();
    let edge_offset: usize = glyphs[..sel].iter().map(|g| g.edges).sum();
    let field_offset: usize = glyphs[..sel].iter().map(BatchGlyph::texels).sum();
    let ranges = &g.bins.ranges[tile * groups..(tile + 1) * groups];
    let (x, row) = (tx * 8 + 3, ty * 8 + 5); // one texel of the tile
    let mut svg = Svg::new(940.0, 440.0);
    svg.add(format!(
        r#"<defs><marker id="arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0,0L10,5L0,10z" fill="{MUTED}"/></marker></defs>"#
    ));
    svg.heading(
        20.0,
        30.0,
        "One texel's path through the buffers",
        &format!(
            "Glyph {sel} ('{}') of the batch above: work item {k} is its tile {tile}; the invocation for texel ({x}, {row}) of that tile",
            g.name
        ),
    );
    let (bw, bh) = (206.0, 118.0);
    let col = |i: f64| 20.0 + i * (bw + 22.0);
    let (r1, r2) = (70.0, 70.0 + bh + 60.0);
    let edges_listed: Vec<String> = ranges
        .iter()
        .flat_map(|r| {
            g.bins.indices[r.offset as usize..(r.offset + r.count) as usize]
                .iter()
                .map(ToString::to_string)
        })
        .collect();
    let group_lines: Vec<String> = ranges
        .iter()
        .enumerate()
        .map(|(c, r)| format!("contour {c}: {} candidates", r.count))
        .collect();
    let boxes: [(f64, f64, &str, Vec<String>, usize); 8] = [
        (
            col(0.0),
            r1,
            "work",
            vec![
                format!("work[{k}] = ({sel}, {tile})"),
                "one 8×8 workgroup per item,".into(),
                "one invocation per texel".into(),
            ],
            1,
        ),
        (
            col(1.0),
            r1,
            "glyphs",
            vec![
                format!("glyphs[{sel}]"),
                format!("{}×{} texels, {}×{} tiles", g.w, g.h, g.tiles_x, g.tiles_y),
                format!("tile_offset {tile_offset}, cand_offset {cand_offset}"),
                format!("edge_offset {edge_offset}, field_offset {field_offset}"),
            ],
            0,
        ),
        (
            col(2.0),
            r1,
            "tile_groups",
            {
                let mut v = vec![
                    format!("[tile_offset + {tile}·{groups} + c]"),
                    format!("tile ({tx}, {ty}): offset, count, and"),
                ];
                v.push("upper bounds per channel".into());
                v.extend(group_lines.iter().take(2).cloned());
                v
            },
            5,
        ),
        (
            col(3.0),
            r1,
            "cands",
            vec![
                "[cand_offset + offset …]".into(),
                format!(
                    "edges {}",
                    edges_listed.iter().take(6).cloned().collect::<Vec<_>>().join(", ")
                ),
                "sorted by lower bound: the scan".into(),
                "stops when none can be nearer".into(),
            ],
            6,
        ),
        (
            col(3.0),
            r2,
            "edges",
            vec![
                "edges[edge_offset + i]".to_string(),
                "lines, quadratics, cubics; cubics".into(),
                "read their interval data from".into(),
                "curves, the combiner contours".into(),
            ],
            2,
        ),
        (
            col(2.0),
            r2,
            "field",
            vec![
                format!("field[field_offset + {row}·{} + {x}]", g.w),
                "vec4: MSDF in rgb,".into(),
                "true distance in a".into(),
            ],
            7,
        ),
        (
            col(1.0),
            r2,
            "stencil",
            vec![
                format!("stencil[field_offset + {row}·{} + {x}]", g.w),
                "error correction flags: protected".into(),
                "(corners from binding 9, edges),".into(),
                "artifact; artifacts → median".into(),
            ],
            8,
        ),
        (
            col(0.0),
            r2,
            "atlas texture",
            vec![
                "layer atlas_layer, at".to_string(),
                format!("(atlas_x + {x}, atlas_y + {row})"),
                "the store pass clamps to the".into(),
                "format (rgba8 or rgba16f)".into(),
            ],
            10,
        ),
    ];
    for (bx, by, title, lines, binding) in &boxes {
        let hue = if *binding == 10 { "#c98a1c" } else { "#57606a" };
        svg.add(format!(
            r##"<rect x="{bx:.1}" y="{by:.1}" width="{bw}" height="{bh}" rx="8" fill="#ffffff" stroke="{hue}" stroke-width="1.2"/>"##
        ));
        let tag = match binding {
            10 => "texture".to_string(),
            2 => "bindings 2, 3, 4".to_string(),
            b => format!("binding {b}"),
        };
        svg.text(bx + 12.0, by + 20.0, 13.0, INK, "start", 600, title);
        svg.text(bx + bw - 10.0, by + 20.0, 10.5, MUTED, "end", 400, &tag);
        for (i, l) in lines.iter().enumerate() {
            let (size, color, weight) = if i == 0 {
                (11.5, GLYPH_HUES[sel], 600)
            } else {
                (11.5, MUTED, 400)
            };
            svg.text(bx + 12.0, by + 42.0 + 17.0 * i as f64, size, color, "start", weight, l);
        }
    }
    for i in 0..3 {
        let x = col(i as f64) + bw;
        arrow(
            &mut svg,
            &format!("M{x:.1},{:.1}H{:.1}", r1 + bh / 2.0, col(i as f64 + 1.0) - 3.0),
            MUTED,
            false,
        );
    }
    arrow(
        &mut svg,
        &format!("M{:.1},{:.1}V{:.1}", col(3.0) + bw / 2.0, r1 + bh, r2 - 3.0),
        MUTED,
        false,
    );
    for i in (1..=3).rev() {
        arrow(
            &mut svg,
            &format!(
                "M{:.1},{:.1}H{:.1}",
                col(i as f64),
                r2 + bh / 2.0,
                col(i as f64 - 1.0) + bw + 3.0
            ),
            MUTED,
            false,
        );
    }
    svg.text(
        20.0,
        r2 + bh + 34.0,
        12.0,
        MUTED,
        "start",
        400,
        "Every array is shared by the whole batch: a glyph's record holds its offsets into them, so one dispatch covers all glyphs.",
    );
    svg.finish()
}

/// Which pass reads and writes which buffer, in order.
fn batch_passes() -> String {
    let cols = [
        "glyphs",
        "work",
        "edges",
        "contours",
        "curves",
        "tile_groups",
        "cands",
        "field",
        "stencil",
        "corners",
        "atlas",
        "instances",
    ];
    // R read, W write, X read and write (atomics or in place), U upload.
    let rows: [(&str, &str, &str); 8] = [
        ("upload", "copies from one staging buffer", "UUUUUUU..U.."),
        ("fine", "one workgroup per (glyph, tile)", "RRRRRRRW...."),
        ("ec_corners", "one invocation per corner texel", "........XR.."),
        ("ec_protect", "per texel: protect edges", "RR.....RX..."),
        ("ec_find", "per texel: flag artifacts", "RRRRR..RX..."),
        ("ec_apply", "per texel: flagged → median", "RR.....XR..."),
        ("store", "per texel: into the atlas", "RR.....R..W."),
        ("text (render)", "instanced quads, every frame", "..........RR"),
    ];
    let (x0, label_w, cw, rh, top) = (20.0, 250.0, 54.0, 34.0, 120.0);
    let w = x0 + label_w + cw * cols.len() as f64 + 20.0;
    let mut svg = Svg::new(w, top + rh * rows.len() as f64 + 70.0);
    svg.heading(
        20.0,
        30.0,
        "The passes of a flush, and what each one touches",
        "Error correction (ec_*) runs when it is enabled; the text pass draws from the atlas every frame",
    );
    for (i, c) in cols.iter().enumerate() {
        let x = x0 + label_w + (i as f64 + 0.5) * cw;
        svg.add(format!(
            r#"<text x="{x:.1}" y="{:.1}" font-size="11.5" fill="{INK}" text-anchor="start" transform="rotate(-35 {x:.1} {:.1})">{c}</text>"#,
            top - 10.0,
            top - 10.0
        ));
    }
    for (r, (pass, what, cells)) in rows.iter().enumerate() {
        let y = top + r as f64 * rh;
        if r % 2 == 0 {
            svg.add(format!(
                r##"<rect x="{x0}" y="{y:.1}" width="{:.1}" height="{rh}" fill="#f6f8fa"/>"##,
                w - 2.0 * x0
            ));
        }
        svg.text(x0 + 8.0, y + 15.0, 13.0, INK, "start", 600, pass);
        svg.text(x0 + 8.0, y + 29.0, 11.0, MUTED, "start", 400, what);
        for (i, ch) in cells.chars().enumerate() {
            let (fill, text, color) = match ch {
                'R' => ("#dbe8f6", "read", "#2f5f99"),
                'W' => ("#e07b1f", "write", "#ffffff"),
                'X' => ("#f4b26b", "r/w", "#5a3200"),
                'U' => ("#8c959f", "copy", "#ffffff"),
                _ => continue,
            };
            let x = x0 + label_w + i as f64 * cw + 4.0;
            svg.add(format!(
                r#"<rect x="{x:.1}" y="{:.1}" width="{:.1}" height="{:.1}" rx="4" fill="{fill}"/>"#,
                y + 6.0,
                cw - 8.0,
                rh - 12.0
            ));
            svg.text(x + (cw - 8.0) / 2.0, y + 21.5, 10.5, color, "middle", 600, text);
        }
    }
    svg.text(
        x0,
        top + rh * rows.len() as f64 + 26.0,
        12.0,
        MUTED,
        "start",
        400,
        "Two bind groups per batch, distance (0 to 7) and error correction (0 to 4, 7 to 9): at most 8 storage buffers each, as WebGPU allows.",
    );
    svg.text(
        x0,
        top + rh * rows.len() as f64 + 44.0,
        12.0,
        MUTED,
        "start",
        400,
        "The store pass binds the atlas as a storage texture; the text pass samples it, with the glyph instances as a vertex buffer.",
    );
    svg.finish()
}
