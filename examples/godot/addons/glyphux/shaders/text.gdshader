// Monochrome glyphs on a canvas (GlyphuxLabel, GlyphuxRichLabel): text_common.gdshaderinc
// with the vertex data of a 2D mesh. Each vertex carries its glyph's index in CUSTOM0.x and
// its drawing pass in CUSTOM0.y.
shader_type canvas_item;
render_mode blend_mix;

#include "text_common.gdshaderinc"

varying flat int gx_glyph;
varying flat int gx_pass;
varying vec4 gx_modulate;
varying vec2 gx_local;

void vertex() {
	gx_glyph = int(CUSTOM0.x + 0.5);
	gx_pass = int(CUSTOM0.y + 0.5);
	// White vertices: the node's modulate.
	gx_modulate = COLOR;
	VERTEX += gx_animate(gx_glyph, TIME);
	gx_local = VERTEX;
}

void fragment() {
	float px = 2.0 / max(length(dFdx(gx_local)) + length(dFdy(gx_local)), 1e-6);
	COLOR = gx_shade(gx_glyph, gx_pass, UV, dFdx(UV), dFdy(UV), px, TIME) * gx_modulate;
}
