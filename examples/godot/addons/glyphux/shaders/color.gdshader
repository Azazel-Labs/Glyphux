// Color glyphs on a canvas (GlyphuxLabel, GlyphuxRichLabel): color_common.gdshaderinc with
// the vertex data of a 2D mesh. Each vertex carries its glyph's index in CUSTOM0.x and the
// quad position (-1 to 1) in UV.
shader_type canvas_item;
render_mode blend_premul_alpha;

#include "color_common.gdshaderinc"

varying flat int gx_glyph;
varying vec4 gx_modulate;

void vertex() {
	gx_glyph = int(CUSTOM0.x + 0.5);
	gx_modulate = COLOR;
	VERTEX += gx_color_animate(gx_glyph, TIME);
}

void fragment() {
	float footprint = max(length(dFdx(UV)), length(dFdy(UV)));
	vec4 c = gx_color_shade(gx_glyph, UV, 1.0 / max(footprint, 1e-12));
	// The node's modulate scales the coverage of all four channels.
	COLOR = vec4(c.rgb * gx_modulate.rgb, c.a) * gx_modulate.a;
}
