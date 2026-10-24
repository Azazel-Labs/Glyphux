// Monochrome glyphs, shared by the canvas and the spatial shaders: glyphux-gpu's text shading
// (text_core.wgsl, which the build translates into generated/) with Godot's bindings.
//
// A glyph's record is a row of 12 texels in `gx_glyphs` (64 glyphs per texture row): its
// GlyphInstance (rect, uv, color, outline, params, glow, then 0, shadow, shadow_params),
// its layer, animation phase (its pen position) and shake seed, then its animation: wave
// amplitude and frequency, shake amplitude and rate; rainbow speed, saturation, value and
// spatial frequency.
//
// Underlines and strikethroughs are records too, of layer -1: they have no field in the
// atlas, but the exact distance to a box (texel 6: its half size), with uvs in layout units
// from its center. So outlines, glows, shadows and animation apply to them as to glyphs. Lengths are in layout units
// (the label's units in 2D, its pixels in 3D), turned into screen pixels here, so zooming
// changes nothing but the glyphs' fields. Globals start with gx_, clear of the translated
// functions' names.

uniform sampler2DArray gx_atlas : filter_linear, repeat_disable;
uniform sampler2D gx_glyphs : filter_nearest, repeat_disable;
uniform float gx_px_range = 4.0;
uniform float gx_true_distance = 1.0;

vec4 gx_record(int i, int k) {
	return texelFetch(gx_glyphs, ivec2((i % 64) * 12 + k, i / 64), 0);
}

float gx_hash(float n) {
	return fract(sin(n) * 43758.5453);
}

vec3 gx_hsv(float h, float s, float v) {
	vec3 k = clamp(abs(mod(h * 6.0 + vec3(0.0, 4.0, 2.0), 6.0) - 3.0) - 1.0, 0.0, 1.0);
	return v * mix(vec3(1.0), k, s);
}

// A decoration's field texel at `q` (layout units from its box's center), like the atlas'
// (0.5 at the edge, `range` units across), every channel the true distance.
vec4 gx_box(vec2 q, vec2 half_size, float range) {
	vec2 d = abs(q) - half_size;
	float outside = length(max(d, 0.0)) + min(max(d.x, d.y), 0.0);
	return vec4(clamp(0.5 - outside / range, 0.0, 1.0));
}

vec3 gx_linear(vec3 c) {
	return mix(pow((c + 0.055) / 1.055, vec3(2.4)), c / 12.92, step(c, vec3(0.04045)));
}

#include "generated/glyphux_text.gdshaderinc"

// How far a glyph's vertices move at `time` (wave, shake), in layout units, y down.
vec2 gx_animate(int glyph, float time) {
	vec4 fx = gx_record(glyph, 10);
	vec4 at = gx_record(glyph, 9);
	vec2 d = vec2(0.0);
	if (fx.x > 0.0) {
		d.y += fx.x * sin(time * fx.y * 6.2831853 + at.y * 0.05);
	}
	if (fx.z > 0.0) {
		float step = floor(time * fx.w) + at.z * 17.0;
		d += fx.z * (vec2(gx_hash(step), gx_hash(step + 7.31)) * 2.0 - 1.0);
	}
	return d;
}

// A glyph's color (straight alpha) at texture coordinates `uv`, with their screen
// derivatives and `px` screen pixels per layout unit. A label draws its glyphs in three
// passes, so that no glyph's effects cover its neighbors: `pass` is text_pass's (0 the
// shadow and glow, 1 the outline, 2 the fill).
vec4 gx_shade(int glyph, int pass, vec2 uv, vec2 duv_dx, vec2 duv_dy, float px, float time) {
	TextGlyph g = text_pass(TextGlyph(gx_record(glyph, 2), gx_record(glyph, 3), gx_record(glyph, 4),
		gx_record(glyph, 5), gx_record(glyph, 7), gx_record(glyph, 8), gx_record(glyph, 1)), uint(pass));
	// Lengths (weight, outline, glow, shadow) to screen pixels.
	g.params.xyz *= px;
	g.shadow_params.xyz *= px;
	vec4 rainbow = gx_record(glyph, 11);
	if (rainbow.x > 0.0) {
		float hue = time * rainbow.x + gx_record(glyph, 9).y * rainbow.w * 0.01;
		g.color.rgb = gx_hsv(fract(hue), rainbow.y, rainbow.z);
	}
	float layer = gx_record(glyph, 9).x;
	// A decoration: one layout unit per "texel".
	bool box = layer < 0.0;
	vec2 half_size = gx_record(glyph, 6).xy;
	vec2 atlas_size = box ? vec2(1.0) : vec2(textureSize(gx_atlas, 0).xy);
	TextPixel p = TextPixel(uv, duv_dx, duv_dy, atlas_size, gx_px_range, gx_true_distance);
	vec4 center = box ? gx_box(uv, half_size, g.params.w) : textureGrad(gx_atlas, vec3(uv, layer), duv_dx, duv_dy);
	TextSamples v = TextSamples(center, vec4(0.0), vec4(0.0), vec4(0.0), vec4(0.0), vec4(0.0));
	TextTaps t = text_taps(g, p, v.center);
	if (t.glow > 0.5) {
		v.glow_a = box ? gx_box(t.glow_a, half_size, g.params.w) : textureLod(gx_atlas, vec3(t.glow_a, layer), 0.0);
		v.glow_b = box ? gx_box(t.glow_b, half_size, g.params.w) : textureLod(gx_atlas, vec3(t.glow_b, layer), 0.0);
		v.glow_c = box ? gx_box(t.glow_c, half_size, g.params.w) : textureLod(gx_atlas, vec3(t.glow_c, layer), 0.0);
		v.glow_d = box ? gx_box(t.glow_d, half_size, g.params.w) : textureLod(gx_atlas, vec3(t.glow_d, layer), 0.0);
	}
	if (t.shadow > 0.5) {
		v.shadow = box ? gx_box(t.shadow_uv, half_size, g.params.w) : textureLod(gx_atlas, vec3(t.shadow_uv, layer), 0.0);
	}
	return text_color(g, p, v);
}
