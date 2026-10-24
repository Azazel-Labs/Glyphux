// Color glyphs, shared by the canvas and the spatial shaders: glyphux's color shading
// (color_core.wgsl, which the build translates into generated/) with Godot's bindings.
//
// Godot's shaders have no storage buffers here, so the tables are texels of `gx_tables`
// (256 per row, numbers as floats, -1 for the "none" index): first 4 texels per glyph (where
// its tile, paint, stop and recipe records start; its config; its color page, opacity, pen
// position and shake seed; its animation: wave amplitude and frequency, shake amplitude and
// rate), then the records: tiles of 2 texels, paints of 5, stops of 2, recipe words 4 per
// texel. Globals start with gx_, clear of the translated functions' names.

uniform sampler2DArray gx_pages : filter_linear, repeat_disable;
uniform sampler2D gx_tables : filter_nearest, repeat_disable;

vec4 gx_table(uint i) {
	return texelFetch(gx_tables, ivec2(int(i % 256u), int(i / 256u)), 0);
}

float gx_hash(float n) {
	return fract(sin(n) * 43758.5453);
}

vec3 gx_linear(vec3 c) {
	return mix(pow((c + 0.055) / 1.055, vec3(2.4)), c / 12.92, step(c, vec3(0.04045)));
}

uint gx_word(float v) {
	return v < 0.0 ? 4294967295u : uint(v + 0.5);
}

uvec4 gx_words(vec4 v) {
	return uvec4(gx_word(v.x), gx_word(v.y), gx_word(v.z), gx_word(v.w));
}

#include "generated/glyphux_color_types.gdshaderinc"

ColorTile color_tile(ColorTables t, uint i) {
	uvec4 a = gx_words(gx_table(t.tiles + 2u * i));
	return ColorTile(a.xy, a.z, a.w, gx_words(gx_table(t.tiles + 2u * i + 1u)));
}

ColorPaint color_paint(ColorTables t, uint i) {
	uint b = t.paints + 5u * i;
	return ColorPaint(gx_words(gx_table(b)), gx_table(b + 1u), gx_table(b + 2u), gx_table(b + 3u), gx_table(b + 4u));
}

ColorStop color_stop(ColorTables t, uint i) {
	return ColorStop(gx_table(t.stops + 2u * i), gx_table(t.stops + 2u * i + 1u));
}

uint color_recipe(ColorTables t, uint i) {
	uint w = t.recipes + i;
	vec4 v = gx_table(w / 4u);
	uint k = w % 4u;
	return gx_word(k == 0u ? v.x : (k == 1u ? v.y : (k == 2u ? v.z : v.w)));
}

#include "generated/glyphux_color.gdshaderinc"

// How far a glyph's vertices move at `time` (wave, shake), in layout units, y down.
vec2 gx_color_animate(int glyph, float time) {
	uint record = uint(glyph) * 4u;
	vec4 fx = gx_table(record + 3u);
	vec2 d = vec2(0.0);
	if (fx.x > 0.0) {
		d.y += fx.x * sin(time * fx.y * 6.2831853 + gx_table(record + 2u).z * 0.05);
	}
	if (fx.z > 0.0) {
		float step = floor(time * fx.w) + gx_table(record + 2u).w * 17.0;
		d += fx.z * (vec2(gx_hash(step), gx_hash(step + 7.31)) * 2.0 - 1.0);
	}
	return d;
}

// A glyph's premultiplied color at `q` (the quad, -1 to 1), `pixels_per_unit` screen pixels
// per unit of q (from derivatives taken before any tile-dependent control flow).
vec4 gx_color_shade(int glyph, vec2 q, float pixels_per_unit) {
	uint record = uint(glyph) * 4u;
	uvec4 bases = gx_words(gx_table(record));
	ColorTables t = ColorTables(bases.x, bases.y, bases.z, bases.w);
	vec4 config = gx_table(record + 1u);
	vec4 page = gx_table(record + 2u);
	ColorCell cell = color_cell(t, q, config);
	vec4 s = vec4(0.0);
	if (cell.tile.mode >= 2u) {
		s = textureLod(gx_pages, vec3(cell.texel / vec2(textureSize(gx_pages, 0).xy), page.x), 0.0);
	}
	// Premultiplied: the opacity scales the coverage of all four channels.
	return color_shade(t, q, pixels_per_unit, cell, s, config) * page.y;
}
