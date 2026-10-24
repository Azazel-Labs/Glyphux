// MSDF / MTSDF text rendering with wgpu: one instanced quad per glyph. Each pixel samples
// the atlas and leaves the rest to text_core.wgsl, which comes first in the module.

// The part of the glyphs a pipeline draws (text_pass): TextRenderer draws the instances
// three times, once per pass.
override text_pass_index: u32 = TEXT_PASS_ALL;

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

fn instance_glyph(inst: Instance) -> TextGlyph {
    var g: TextGlyph;
    g.color = inst.color;
    g.outline = inst.outline;
    g.params = inst.params;
    g.glow = inst.glow;
    g.shadow = inst.shadow;
    g.shadow_params = inst.shadow_params;
    g.uv_rect = inst.uv;
    return g;
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
    // Nothing to draw in this pass: a quad of no area.
    if (!text_pass_draws(instance_glyph(inst), text_pass_index)) {
        out.pos = vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    return out;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    var g: TextGlyph;
    g.color = in.color;
    g.outline = in.outline;
    g.params = in.params;
    g.glow = in.glow;
    g.shadow = in.shadow;
    g.shadow_params = in.shadow_params;
    g.uv_rect = in.uv_rect;
    g = text_pass(g, text_pass_index);
    var p: TextPixel;
    p.uv = in.uv;
    p.duv_dx = dpdx(in.uv);
    p.duv_dy = dpdy(in.uv);
    p.atlas_size = vec2<f32>(textureDimensions(atlas).xy);
    p.px_range = globals.px_range;
    p.has_true_distance = globals.has_true_distance;
    var v: TextSamples;
    v.center = textureSample(atlas, atlas_sampler, in.uv, in.layer);
    let t = text_taps(g, p, v.center);
    if (t.glow > 0.5) {
        v.glow_a = textureSampleLevel(atlas, atlas_sampler, t.glow_a, in.layer, 0.0);
        v.glow_b = textureSampleLevel(atlas, atlas_sampler, t.glow_b, in.layer, 0.0);
        v.glow_c = textureSampleLevel(atlas, atlas_sampler, t.glow_c, in.layer, 0.0);
        v.glow_d = textureSampleLevel(atlas, atlas_sampler, t.glow_d, in.layer, 0.0);
    }
    if (t.shadow > 0.5) {
        v.shadow = textureSampleLevel(atlas, atlas_sampler, t.shadow_uv, in.layer, 0.0);
    }
    return text_color(g, p, v);
}
