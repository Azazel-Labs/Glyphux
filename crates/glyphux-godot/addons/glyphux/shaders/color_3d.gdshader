// Color glyphs in a 3D scene (GlyphuxLabel3D), unshaded, double sided and depth tested:
// color_3d.gdshaderinc.
shader_type spatial;
render_mode unshaded, cull_disabled, depth_draw_never, blend_premul_alpha;

#include "color_3d.gdshaderinc"
