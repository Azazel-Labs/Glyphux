// Monochrome glyphs in a 3D scene (GlyphuxLabel3D): text_common.gdshaderinc on a mesh in the
// node's space: x right, y up, `gx_pixel_size` world units per layout pixel. Each vertex
// carries its glyph's index in CUSTOM0.x and its drawing pass in CUSTOM0.y. The body of
// text_3d.gdshader and text_3d_no_depth_test.gdshader, which only differ in their render
// modes.

#include "text_common.gdshaderinc"

uniform float gx_pixel_size = 0.005;
uniform bool gx_billboard = false;

varying flat int gx_glyph;
varying flat int gx_pass;
varying vec2 gx_local;

void vertex() {
	gx_glyph = int(CUSTOM0.x + 0.5);
	gx_pass = int(CUSTOM0.y + 0.5);
	// Layout pixels (y down), animated.
	vec2 local = vec2(VERTEX.x, -VERTEX.y) / gx_pixel_size + gx_animate(gx_glyph, TIME);
	VERTEX.xy = vec2(local.x, -local.y) * gx_pixel_size;
	gx_local = local;
	if (gx_billboard) {
		// Facing the camera, keeping the node's position and scale.
		MODELVIEW_MATRIX = VIEW_MATRIX * mat4(INV_VIEW_MATRIX[0], INV_VIEW_MATRIX[1], INV_VIEW_MATRIX[2], MODEL_MATRIX[3]);
		MODELVIEW_MATRIX = MODELVIEW_MATRIX * mat4(vec4(length(MODEL_MATRIX[0].xyz), 0.0, 0.0, 0.0),
			vec4(0.0, length(MODEL_MATRIX[1].xyz), 0.0, 0.0), vec4(0.0, 0.0, length(MODEL_MATRIX[2].xyz), 0.0),
			vec4(0.0, 0.0, 0.0, 1.0));
	}
}

void fragment() {
	float px = 2.0 / max(length(dFdx(gx_local)) + length(dFdy(gx_local)), 1e-6);
	vec4 c = gx_shade(gx_glyph, gx_pass, UV, dFdx(UV), dFdy(UV), px, TIME);
	// Colors are sRGB; the 3D scene renders in linear color unless its output is sRGB.
	ALBEDO = OUTPUT_IS_SRGB ? c.rgb : gx_linear(c.rgb);
	ALPHA = c.a;
}
