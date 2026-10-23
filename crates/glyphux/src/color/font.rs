use super::{Affine, ColorError, ColorGlyph, ColorLayer, ColorLine, ColorStop, Extend, Paint, srgb_to_linear};
use crate::io::font::{Font, ttf_parser};
use crate::{Segment, Shape, Vec2};
use ttf_parser::colr::{self, Painter};

impl Font<'_> {
    /// Imports COLRv0/v1 layers by shaped glyph ID. `foreground` is sRGB RGBA8 and
    /// resolves COLR's foreground-color references. Bitmap and SVG emoji are not COLR.
    /// Unsupported clipping, compositing, and gradient geometry return explicit errors.
    pub fn color_glyph_by_index(&self, id: u16, palette: u16, foreground: [u8; 4]) -> Result<ColorGlyph, ColorError> {
        let face = self.face();
        if !face.is_color_glyph(ttf_parser::GlyphId(id)) {
            return Err(ColorError::Unsupported("glyph has no COLR artwork"));
        }
        if face.color_palettes().is_none_or(|n| palette >= n.get()) {
            return Err(ColorError::Invalid("palette index"));
        }
        let advance = f64::from(face.glyph_hor_advance(ttf_parser::GlyphId(id)).unwrap_or(0));
        let height = f64::from(face.tables().hhea.ascender) - f64::from(face.tables().hhea.descender);
        let s = 0.55 * advance.max(height);
        if s <= 0.0 {
            return Err(ColorError::Invalid("font metrics"));
        }
        let cx = advance / 2.0;
        let cy = f64::midpoint(
            f64::from(face.tables().hhea.ascender),
            f64::from(face.tables().hhea.descender),
        );
        let scale = self.metrics().em_size / f64::from(face.units_per_em());
        let frame = Affine([1.0 / s, 0.0, -cx / s, 0.0, -1.0 / s, cy / s]);
        let mut importer = Importer {
            font: self,
            palette,
            scale,
            transform: frame,
            transforms: Vec::new(),
            clips: Vec::new(),
            pending: None,
            paints: Vec::new(),
            layers: Vec::new(),
            error: None,
        };
        let result = face.paint_color_glyph(
            ttf_parser::GlyphId(id),
            palette,
            ttf_parser::RgbaColor::new(foreground[0], foreground[1], foreground[2], foreground[3]),
            &mut importer,
        );
        if let Some(error) = importer.error {
            return Err(error);
        }
        if result.is_none() {
            return Err(ColorError::Invalid("malformed COLR paint graph"));
        }
        Ok(ColorGlyph {
            paints: importer.paints,
            layers: importer.layers,
            plane_min: Vec2::new((cx - s) * scale, (cy - s) * scale),
            plane_max: Vec2::new((cx + s) * scale, (cy + s) * scale),
            advance: advance * scale,
        })
    }
}

enum Clip {
    Outline(Shape),
    Box(colr::ClipBox, Affine),
}

struct Importer<'f, 'a> {
    font: &'f Font<'a>,
    palette: u16,
    scale: f64,
    transform: Affine,
    transforms: Vec<Affine>,
    clips: Vec<Clip>,
    pending: Option<Shape>,
    paints: Vec<Paint>,
    layers: Vec<ColorLayer>,
    error: Option<ColorError>,
}

fn color(c: ttf_parser::RgbaColor) -> [f32; 4] {
    [
        srgb_to_linear(f32::from(c.red) / 255.0),
        srgb_to_linear(f32::from(c.green) / 255.0),
        srgb_to_linear(f32::from(c.blue) / 255.0),
        f32::from(c.alpha) / 255.0,
    ]
}

fn line(stops: impl Iterator<Item = colr::ColorStop>, extend: colr::GradientExtend) -> ColorLine {
    let mut stops: Vec<_> = stops
        .map(|s| ColorStop {
            offset: f64::from(s.stop_offset),
            color: color(s.color),
        })
        .collect();
    stops.sort_by(|a, b| a.offset.total_cmp(&b.offset));
    ColorLine {
        stops,
        extend: match extend {
            colr::GradientExtend::Pad => Extend::Pad,
            colr::GradientExtend::Repeat => Extend::Repeat,
            colr::GradientExtend::Reflect => Extend::Reflect,
        },
    }
}

impl Importer<'_, '_> {
    fn add_paint(&mut self, paint: colr::Paint<'_>) -> Result<(), ColorError> {
        let inverse = self.transform.inverse()?;
        let coords = self.font.face().variation_coordinates();
        let paint = match paint {
            colr::Paint::Solid(c) => Paint::Solid(color(c)),
            colr::Paint::LinearGradient(g) => Paint::linear(
                inverse,
                Vec2::new(f64::from(g.x0), f64::from(g.y0)),
                Vec2::new(f64::from(g.x1), f64::from(g.y1)),
                Vec2::new(f64::from(g.x2), f64::from(g.y2)),
                line(g.stops(self.palette, coords), g.extend),
            )?,
            colr::Paint::RadialGradient(g) => {
                if g.x0 != g.x1 || g.y0 != g.y1 {
                    return Err(ColorError::Unsupported("non-concentric radial gradient"));
                }
                Paint::Radial {
                    inverse,
                    center: Vec2::new(f64::from(g.x0), f64::from(g.y0)),
                    r0: f64::from(g.r0),
                    r1: f64::from(g.r1),
                    line: line(g.stops(self.palette, coords), g.extend),
                }
            }
            colr::Paint::SweepGradient(_) => return Err(ColorError::Unsupported("sweep gradient")),
        };
        paint.validate()?;
        let shape = self
            .clips
            .iter()
            .rev()
            .find_map(|c| match c {
                Clip::Outline(s) => Some(s),
                Clip::Box(..) => None,
            })
            .or(self.pending.as_ref())
            .ok_or(ColorError::Unsupported("paint without an outline"))?;
        for clip in &self.clips {
            if let Clip::Box(bounds, inverse) = clip {
                for p in shape
                    .contours
                    .iter()
                    .flat_map(|c| &c.edges)
                    .flat_map(super::fields::edge_points)
                {
                    let p = inverse.apply(*p);
                    if p.x < f64::from(bounds.x_min)
                        || p.x > f64::from(bounds.x_max)
                        || p.y < f64::from(bounds.y_min)
                        || p.y > f64::from(bounds.y_max)
                    {
                        return Err(ColorError::Unsupported("clip box cuts the outline control hull"));
                    }
                }
            }
        }
        let id = self.paints.iter().position(|p| *p == paint).unwrap_or_else(|| {
            let id = self.paints.len();
            self.paints.push(paint);
            id
        });
        self.layers.push(ColorLayer {
            shape: shape.clone(),
            paint: id,
        });
        Ok(())
    }
}

impl<'a> Painter<'a> for Importer<'_, 'a> {
    fn outline_glyph(&mut self, glyph: ttf_parser::GlyphId) {
        let Ok(mut glyph) = self.font.glyph_by_index(glyph.0) else {
            self.error = Some(ColorError::Invalid("outline glyph index"));
            return;
        };
        for edge in glyph.shape.contours.iter_mut().flat_map(|c| &mut c.edges) {
            let points: &mut [Vec2] = match &mut edge.segment {
                Segment::Linear(p) => p,
                Segment::Quadratic(p) => p,
                Segment::Cubic(p) => p,
            };
            for p in points {
                *p = self.transform.apply(*p / self.scale);
            }
        }
        glyph.shape.inverse_y_axis = true;
        self.pending = Some(glyph.shape);
    }
    fn paint(&mut self, paint: colr::Paint<'a>) {
        if self.error.is_none() {
            if let Err(e) = self.add_paint(paint) {
                self.error = Some(e);
            }
        }
    }
    fn push_clip(&mut self) {
        if self.clips.iter().any(|c| matches!(c, Clip::Outline(_))) {
            self.error = Some(ColorError::Unsupported("nested outline clips"));
        }
        if let Some(shape) = self.pending.take() {
            self.clips.push(Clip::Outline(shape));
        } else {
            self.error = Some(ColorError::Invalid("clip without outline"));
        }
    }
    fn push_clip_box(&mut self, clipbox: colr::ClipBox) {
        match self.transform.inverse() {
            Ok(inverse) => self.clips.push(Clip::Box(clipbox, inverse)),
            Err(e) => self.error = Some(e),
        }
    }
    fn pop_clip(&mut self) {
        self.clips.pop();
    }
    fn push_layer(&mut self, _: colr::CompositeMode) {
        self.error = Some(ColorError::Unsupported("isolated composite groups"));
    }
    fn pop_layer(&mut self) {}
    fn push_transform(&mut self, t: ttf_parser::Transform) {
        self.transforms.push(self.transform);
        self.transform = self.transform.compose(Affine([
            f64::from(t.a),
            f64::from(t.c),
            f64::from(t.e),
            f64::from(t.b),
            f64::from(t.d),
            f64::from(t.f),
        ]));
    }
    fn pop_transform(&mut self) {
        if let Some(t) = self.transforms.pop() {
            self.transform = t;
        }
    }
}
