// Color glyphs in a 3D scene (GlyphuxLabel3D with no_depth_test), unshaded, double sided
// and drawn over closer geometry: color_3d.gdshaderinc.
shader_type spatial;
render_mode unshaded, cull_disabled, depth_draw_never, depth_test_disabled, blend_premul_alpha;

#include "color_3d.gdshaderinc"
