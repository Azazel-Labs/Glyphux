// MSDF / MTSDF text rendering: one instanced quad per glyph.

struct Globals {
    // Target size in pixels.
    viewport: vec2<f32>,
    // Distance range of the atlas fields in texels.
    px_range: f32,
    // 1 if the atlas holds the true distance in alpha (MTSDF).
    has_true_distance: f32,
}

@group(0) @binding(0) var<uniform> globals: Globals;
@group(0) @binding(1) var atlas: texture_2d_array<f32>;
@group(0) @binding(2) var atlas_sampler: sampler;

struct Instance {
    // Quad in pixels (min x, min y, max x, max y), y down.
    @location(0) rect: vec4<f32>,
    // Texture coordinates of the quad's top-left and bottom-right corners.
    @location(1) uv: vec4<f32>,
    // Fill color (straight alpha).
    @location(2) color: vec4<f32>,
    // Outline color (straight alpha; MTSDF atlases only).
    @location(3) outline: vec4<f32>,
    // x: weight (pixels added to the distance: > 0 bolder), y: outline width in pixels,
    // z: glow radius in pixels, w: the glyph's distance range in texels (0: the atlas').
    @location(4) params: vec4<f32>,
    // Glow color (straight alpha: the glow's opacity at the glyph's edge).
    @location(5) glow: vec4<f32>,
    // Rotation (cos, sin) about a pivot (pixels); (0, 0) = none.
    @location(6) transform: vec4<f32>,
    // Drop shadow color (straight alpha).
    @location(7) shadow: vec4<f32>,
    // Drop shadow offset (screen pixels, y down) and blur (pixels); w: 1 = draw the raw field.
    @location(8) shadow_params: vec4<f32>,
    @location(9) layer: u32,
}

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) outline: vec4<f32>,
    @location(3) params: vec4<f32>,
    @location(4) glow: vec4<f32>,
    @location(5) shadow: vec4<f32>,
    @location(6) shadow_params: vec4<f32>,
    // The glyph's texture rectangle (the shadow's lookups stay inside it).
    @location(7) @interpolate(flat) uv_rect: vec4<f32>,
    @location(8) @interpolate(flat) layer: u32,
}

@vertex
fn vs(@builtin(vertex_index) vi: u32, inst: Instance) -> VsOut {
    let corner = vec2<f32>(f32(vi & 1u), f32(vi >> 1u));
    var p = mix(inst.rect.xy, inst.rect.zw, corner);
    let cs = inst.transform.xy;
    if (any(cs != vec2<f32>(0.0))) {
        let d = p - inst.transform.zw;
        p = inst.transform.zw + vec2<f32>(cs.x * d.x - cs.y * d.y, cs.y * d.x + cs.x * d.y);
    }
    var out: VsOut;
    out.pos = vec4<f32>(p.x / globals.viewport.x * 2.0 - 1.0, 1.0 - p.y / globals.viewport.y * 2.0, 0.0, 1.0);
    out.uv = mix(inst.uv.xy, inst.uv.zw, corner);
    out.color = inst.color;
    out.outline = inst.outline;
    out.params = inst.params;
    out.glow = inst.glow;
    out.shadow = inst.shadow;
    out.shadow_params = inst.shadow_params;
    out.uv_rect = inst.uv;
    out.layer = inst.layer;
    return out;
}

fn median3(a: f32, b: f32, c: f32) -> f32 {
    return max(min(a, b), min(max(a, b), c));
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    let s = textureSample(atlas, atlas_sampler, in.uv, in.layer);
    // Texture coordinates per screen pixel along x and y.
    let duv_dx = dpdx(in.uv);
    let duv_dy = dpdy(in.uv);
    // Screen pixels per field texel (> 1 when magnified).
    let size = vec2<f32>(textureDimensions(atlas).xy);
    let texels_per_px = (abs(duv_dx) + abs(duv_dy)) * size;
    let px_per_texel = 2.0 / max(texels_per_px.x + texels_per_px.y, 1e-6);
    let px_range = select(globals.px_range, in.params.w, in.params.w > 0.0);
    let screen_range = max(px_range * px_per_texel, 1.0);
    // Signed distance in screen pixels (positive inside), with an optional weight offset.
    let weight = in.params.x;
    let sd = (median3(s.r, s.g, s.b) - 0.5) * screen_range + weight;
    var fill = clamp(sd + 0.5, 0.0, 1.0);
    // Distance for effects outside the glyph: the true distance where available (rounded
    // corners), else the median.
    var sd_outside = sd;
    var rgb = in.color.rgb;
    var alpha = in.color.a * fill;
    if (globals.has_true_distance > 0.5) {
        let sdt = (s.a - 0.5) * screen_range + weight;
        sd_outside = sdt;
        // Below about one texel per pixel the multi-channel corners alias; blend towards the
        // true distance.
        let k = clamp(px_per_texel * 2.0 - 1.0, 0.0, 1.0);
        fill = mix(clamp(sdt + 0.5, 0.0, 1.0), fill, k);
        alpha = in.color.a * fill;
        let ow = in.outline.w;
        if (in.params.y > 0.0 && ow > 0.0) {
            // Outline band outside the fill. (The field only covers half the range outside
            // the shape.)
            let width = min(in.params.y, 0.5 * screen_range - 1.0);
            let band = clamp(sdt + width + 0.5, 0.0, 1.0);
            let oa = ow * band * (1.0 - fill);
            rgb = (in.color.rgb * alpha + in.outline.rgb * oa) / max(alpha + oa, 1e-6);
            alpha = alpha + oa;
        }
    }
    // Glow under the glyph, fading to nothing before the edge of the field's range (beyond
    // it the distance is clamped and would show the quad).
    let radius = min(in.params.z, 0.5 * screen_range - 0.5);
    if (radius > 0.0 && in.glow.a > 0.0) {
        let falloff = clamp(1.0 + sd_outside / radius, 0.0, 1.0);
        let ga = in.glow.a * falloff * falloff * (1.0 - alpha);
        rgb = (rgb * alpha + in.glow.rgb * ga) / max(alpha + ga, 1e-6);
        alpha = alpha + ga;
    }
    // Drop shadow under everything: the glyph's distance where this pixel would be without
    // the offset (clamped to the glyph's rectangle, whose padding reads as far outside).
    if (in.shadow.a > 0.0) {
        let back = duv_dx * in.shadow_params.x + duv_dy * in.shadow_params.y;
        let uv = clamp(in.uv - back, min(in.uv_rect.xy, in.uv_rect.zw), max(in.uv_rect.xy, in.uv_rect.zw));
        let ss = textureSampleLevel(atlas, atlas_sampler, uv, in.layer, 0.0);
        var sds = (median3(ss.r, ss.g, ss.b) - 0.5) * screen_range + weight;
        if (globals.has_true_distance > 0.5) {
            sds = (ss.a - 0.5) * screen_range + weight;
        }
        let sa = in.shadow.a * clamp(0.5 + sds / max(in.shadow_params.z, 1.0), 0.0, 1.0) * (1.0 - alpha);
        rgb = (rgb * alpha + in.shadow.rgb * sa) / max(alpha + sa, 1e-6);
        alpha = alpha + sa;
    }
    // Atlas page view: the raw field. Chosen without branching: an early return for it at the
    // top miscompiled the regular path in Chrome (D3D12), tinting magnified text with the
    // field's channels.
    return select(vec4<f32>(rgb, alpha), vec4<f32>(s.rgb, 1.0), in.shadow_params.w > 0.5);
}
