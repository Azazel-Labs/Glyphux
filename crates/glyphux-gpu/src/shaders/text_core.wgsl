// How one pixel of a glyph quad looks (MSDF / MTSDF text), shared by every backend.
//
// Plain functions, no bindings: a backend's shader samples the atlas and passes the samples
// in. It samples once at the pixel, asks text_taps where the glow and the drop shadow need
// more samples, takes those (without derivatives, textureSampleLevel or textureLod) and
// gets the color from text_color. Backends that don't take WGSL get these functions
// translated by naga.

// One glyph's drawing parameters: a GlyphInstance without its quad.
struct TextGlyph {
    // Fill color (straight alpha).
    color: vec4<f32>,
    // Outline color (straight alpha; MTSDF atlases only).
    outline: vec4<f32>,
    // x: weight (pixels added to the distance: > 0 bolder), y: outline width in pixels,
    // z: glow radius in pixels, w: the glyph's distance range in texels (0: the atlas').
    params: vec4<f32>,
    // Glow color (straight alpha: the glow's opacity at the glyph's edge).
    glow: vec4<f32>,
    // Drop shadow color (straight alpha).
    shadow: vec4<f32>,
    // Drop shadow offset (screen pixels, y down) and blur (pixels); w: 1 = draw the raw field.
    shadow_params: vec4<f32>,
    // The glyph's texture rectangle (the glow's and shadow's samples stay inside it).
    uv_rect: vec4<f32>,
}

// The pixel's place in the atlas.
struct TextPixel {
    uv: vec2<f32>,
    // Texture coordinates per screen pixel along x and y.
    duv_dx: vec2<f32>,
    duv_dy: vec2<f32>,
    // Atlas page size in texels.
    atlas_size: vec2<f32>,
    // Distance range of the atlas fields in texels.
    px_range: f32,
    // 1 if the atlas holds the true distance in alpha (MTSDF).
    has_true_distance: f32,
}

// Where to sample for the glow's filter (four taps, if glow > 0.5) and the drop shadow (if
// shadow > 0.5).
struct TextTaps {
    glow_a: vec2<f32>,
    glow_b: vec2<f32>,
    glow_c: vec2<f32>,
    glow_d: vec2<f32>,
    shadow_uv: vec2<f32>,
    glow: f32,
    shadow: f32,
}

// The atlas samples: at the pixel, then at the taps.
struct TextSamples {
    center: vec4<f32>,
    glow_a: vec4<f32>,
    glow_b: vec4<f32>,
    glow_c: vec4<f32>,
    glow_d: vec4<f32>,
    shadow: vec4<f32>,
}

// Glyphs drawn in passes keep one glyph's effects off its neighbors: every glyph's drop
// shadow and glow first, then every outline, then every fill. text_pass keeps the part of a
// glyph that pass `part` draws (TEXT_PASS_ALL: all of it, in a single pass), and
// text_pass_draws says whether that part is empty.
const TEXT_PASS_EFFECTS: u32 = 0u;
const TEXT_PASS_OUTLINE: u32 = 1u;
const TEXT_PASS_FILL: u32 = 2u;
const TEXT_PASS_ALL: u32 = 3u;

fn text_pass(g: TextGlyph, part: u32) -> TextGlyph {
    var out = g;
    if (part == TEXT_PASS_EFFECTS) {
        out.color.a = 0.0;
        out.outline.a = 0.0;
    }
    if (part == TEXT_PASS_OUTLINE || part == TEXT_PASS_FILL) {
        out.glow.a = 0.0;
        out.shadow.a = 0.0;
    }
    if (part == TEXT_PASS_OUTLINE) {
        out.color.a = 0.0;
    }
    if (part == TEXT_PASS_FILL) {
        out.outline.a = 0.0;
    }
    return out;
}

fn text_pass_draws(g: TextGlyph, part: u32) -> bool {
    // The atlas page view (the raw field) is drawn with the fill.
    let raw = g.shadow_params.w > 0.5;
    if (part == TEXT_PASS_EFFECTS) {
        return !raw && ((g.glow.a > 0.0 && g.params.z > 0.0) || g.shadow.a > 0.0);
    }
    if (part == TEXT_PASS_OUTLINE) {
        return !raw && g.outline.a > 0.0 && g.params.y > 0.0;
    }
    return true;
}

fn median3(a: f32, b: f32, c: f32) -> f32 {
    return max(min(a, b), min(max(a, b), c));
}

// Screen pixels per field texel (> 1 when magnified).
fn text_px_per_texel(p: TextPixel) -> f32 {
    let texels_per_px = (abs(p.duv_dx) + abs(p.duv_dy)) * p.atlas_size;
    return 2.0 / max(texels_per_px.x + texels_per_px.y, 1e-6);
}

// The distance range in screen pixels.
fn text_screen_range(g: TextGlyph, p: TextPixel) -> f32 {
    let px_range = select(p.px_range, g.params.w, g.params.w > 0.0);
    return max(px_range * text_px_per_texel(p), 1.0);
}

// Distance for effects outside the glyph, in screen pixels: the true distance where
// available (rounded corners), else the median.
fn text_outside(g: TextGlyph, p: TextPixel, s: vec4<f32>) -> f32 {
    let field = select(median3(s.r, s.g, s.b), s.a, p.has_true_distance > 0.5);
    return (field - 0.5) * text_screen_range(g, p) + g.params.x;
}

// The glow's radius in screen pixels, within half the field's range (beyond it the distance
// is clamped and would show the quad).
fn text_glow_radius(g: TextGlyph, p: TextPixel) -> f32 {
    return min(g.params.z, 0.5 * text_screen_range(g, p) - 0.5);
}

// How far the glow's filter reaches outside the shape (0: no filter). It widens the glow
// without changing the sharp fill.
fn text_glow_spread(g: TextGlyph, p: TextPixel, s: vec4<f32>) -> f32 {
    let radius = text_glow_radius(g, p);
    if (radius <= 0.0 || g.glow.a <= 0.0) {
        return 0.0;
    }
    return min(radius * 0.2, max(-text_outside(g, p, s), 0.0) * 0.35);
}

fn text_tap(g: TextGlyph, p: TextPixel, offset: vec2<f32>, spread: f32) -> vec2<f32> {
    let uv = p.uv + (p.duv_dx * offset.x + p.duv_dy * offset.y) * spread;
    return clamp(uv, min(g.uv_rect.xy, g.uv_rect.zw), max(g.uv_rect.xy, g.uv_rect.zw));
}

// Where the pixel needs more samples, given the sample at the pixel.
fn text_taps(g: TextGlyph, p: TextPixel, s: vec4<f32>) -> TextTaps {
    let spread = text_glow_spread(g, p, s);
    var t: TextTaps;
    t.glow_a = text_tap(g, p, vec2<f32>(-1.0, -1.0), spread);
    t.glow_b = text_tap(g, p, vec2<f32>(1.0, -1.0), spread);
    t.glow_c = text_tap(g, p, vec2<f32>(-1.0, 1.0), spread);
    t.glow_d = text_tap(g, p, vec2<f32>(1.0, 1.0), spread);
    t.glow = select(0.0, 1.0, spread > 0.0);
    // The glyph's distance where this pixel would be without the shadow's offset (clamped to
    // the glyph's rectangle, whose padding reads as far outside).
    let back = p.duv_dx * g.shadow_params.x + p.duv_dy * g.shadow_params.y;
    t.shadow_uv = clamp(p.uv - back, min(g.uv_rect.xy, g.uv_rect.zw), max(g.uv_rect.xy, g.uv_rect.zw));
    t.shadow = select(0.0, 1.0, g.shadow.a > 0.0);
    return t;
}

fn text_glow_term(g: TextGlyph, p: TextPixel, v: vec4<f32>, radius: f32) -> f32 {
    let f = clamp(1.0 + text_outside(g, p, v) / radius, 0.0, 1.0);
    return f * f;
}

// The pixel's color (straight alpha).
fn text_color(g: TextGlyph, p: TextPixel, v: TextSamples) -> vec4<f32> {
    let s = v.center;
    let px_per_texel = text_px_per_texel(p);
    let screen_range = text_screen_range(g, p);
    // Signed distance in screen pixels (positive inside), with an optional weight offset.
    let weight = g.params.x;
    let sd = (median3(s.r, s.g, s.b) - 0.5) * screen_range + weight;
    var fill = clamp(sd + 0.5, 0.0, 1.0);
    let sd_outside = text_outside(g, p, s);
    var rgb = g.color.rgb;
    var alpha = g.color.a * fill;
    if (p.has_true_distance > 0.5) {
        let sdt = sd_outside;
        // Below about one texel per pixel the multi-channel corners alias; blend towards the
        // true distance.
        let k = clamp(px_per_texel * 2.0 - 1.0, 0.0, 1.0);
        fill = mix(clamp(sdt + 0.5, 0.0, 1.0), fill, k);
        alpha = g.color.a * fill;
        let ow = g.outline.w;
        if (g.params.y > 0.0 && ow > 0.0) {
            // Outline band outside the fill. (The field only covers half the range outside
            // the shape.)
            let width = min(g.params.y, 0.5 * screen_range - 1.0);
            let band = clamp(sdt + width + 0.5, 0.0, 1.0);
            let oa = ow * band * (1.0 - fill);
            rgb = (g.color.rgb * alpha + g.outline.rgb * oa) / max(alpha + oa, 1e-6);
            alpha = alpha + oa;
        }
    }
    // Glow under the glyph, fading to nothing before the edge of the field's range.
    let radius = text_glow_radius(g, p);
    if (radius > 0.0 && g.glow.a > 0.0) {
        let falloff = clamp(1.0 + sd_outside / radius, 0.0, 1.0);
        var glow_value = falloff * falloff;
        // Filter glow opacity, not signed distance. Four extra samples share the center
        // weight.
        if (text_glow_spread(g, p, s) > 0.0) {
            let total = text_glow_term(g, p, v.glow_a, radius) + text_glow_term(g, p, v.glow_b, radius)
                + text_glow_term(g, p, v.glow_c, radius) + text_glow_term(g, p, v.glow_d, radius);
            // Keep the original support so filtering cannot expose the glyph quad.
            glow_value = mix(glow_value, total * 0.25, 0.8) * smoothstep(0.0, 0.15, falloff);
        }
        let ga = g.glow.a * glow_value * (1.0 - alpha);
        rgb = (rgb * alpha + g.glow.rgb * ga) / max(alpha + ga, 1e-6);
        alpha = alpha + ga;
    }
    // Drop shadow under everything.
    if (g.shadow.a > 0.0) {
        let sds = text_outside(g, p, v.shadow);
        let sa = g.shadow.a * clamp(0.5 + sds / max(g.shadow_params.z, 1.0), 0.0, 1.0) * (1.0 - alpha);
        rgb = (rgb * alpha + g.shadow.rgb * sa) / max(alpha + sa, 1e-6);
        alpha = alpha + sa;
    }
    // Atlas page view: the raw field. Chosen without branching: an early return for it at the
    // top miscompiled the regular path in Chrome (D3D12), tinting magnified text with the
    // field's channels.
    return select(vec4<f32>(rgb, alpha), vec4<f32>(s.rgb, 1.0), g.shadow_params.w > 0.5);
}
