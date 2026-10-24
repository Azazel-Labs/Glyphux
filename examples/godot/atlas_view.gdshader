// One page of the glyph atlas, as stored: MTSDF, the multi-channel distances in RGB.
shader_type canvas_item;

uniform sampler2DArray atlas : filter_linear, repeat_disable;
uniform float page = 0.0;

void fragment() {
	COLOR = vec4(texture(atlas, vec3(UV, page)).rgb, 1.0);
}
