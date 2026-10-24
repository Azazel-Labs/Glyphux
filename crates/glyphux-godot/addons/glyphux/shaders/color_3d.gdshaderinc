// Color glyphs in a 3D scene (GlyphuxLabel3D): color_common.gdshaderinc on a mesh in the
// node's space: x right, y up, `gx_pixel_size` world units per layout pixel. Each vertex
// carries its glyph's index in CUSTOM0.x and the quad position (-1 to 1) in UV. The body of
// color_3d.gdshader and color_3d_no_depth_test.gdshader, which only differ in their render
// modes.
#include "color_common.gdshaderinc"

uniform float gx_pixel_size = 0.005;
uniform bool gx_billboard = false;

varying flat int gx_glyph;

void vertex() {
	gx_glyph = int(CUSTOM0.x + 0.5);
	vec2 local = vec2(VERTEX.x, -VERTEX.y) / gx_pixel_size + gx_color_animate(gx_glyph, TIME);
	VERTEX.xy = vec2(local.x, -local.y) * gx_pixel_size;
	if (gx_billboard) {
		// Facing the camera, keeping the node's position and scale.
		MODELVIEW_MATRIX = VIEW_MATRIX * mat4(INV_VIEW_MATRIX[0], INV_VIEW_MATRIX[1], INV_VIEW_MATRIX[2], MODEL_MATRIX[3]);
		MODELVIEW_MATRIX = MODELVIEW_MATRIX * mat4(vec4(length(MODEL_MATRIX[0].xyz), 0.0, 0.0, 0.0),
			vec4(0.0, length(MODEL_MATRIX[1].xyz), 0.0, 0.0), vec4(0.0, 0.0, length(MODEL_MATRIX[2].xyz), 0.0),
			vec4(0.0, 0.0, 0.0, 1.0));
	}
}

void fragment() {
	float footprint = max(length(dFdx(UV)), length(dFdy(UV)));
	vec4 c = gx_color_shade(gx_glyph, UV, 1.0 / max(footprint, 1e-12));
	// Premultiplied sRGB; the 3D scene renders in linear color unless its output is sRGB.
	vec3 straight = c.rgb / max(c.a, 1e-6);
	ALBEDO = (OUTPUT_IS_SRGB ? straight : gx_linear(straight)) * c.a;
	ALPHA = c.a;
}
