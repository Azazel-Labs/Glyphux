// Generated: do not edit. The Glyphux addon's rust/build.rs translates text_core.wgsl
// with naga every build, and writes this file again. See README.md.

#ifndef GLYPHUX_TEXT
#define GLYPHUX_TEXT

struct TextGlyph {
    vec4 color;
    vec4 outline;
    vec4 params;
    vec4 glow;
    vec4 shadow;
    vec4 shadow_params;
    vec4 uv_rect;
};

struct TextPixel {
    vec2 uv;
    vec2 duv_dx;
    vec2 duv_dy;
    vec2 atlas_size;
    float px_range;
    float has_true_distance;
};

struct TextTaps {
    vec2 glow_a;
    vec2 glow_b;
    vec2 glow_c;
    vec2 glow_d;
    vec2 shadow_uv;
    float glow;
    float shadow;
};

struct TextSamples {
    vec4 center;
    vec4 glow_a;
    vec4 glow_b;
    vec4 glow_c;
    vec4 glow_d;
    vec4 shadow;
};

const uint TEXT_PASS_EFFECTS = 0u;

const uint TEXT_PASS_OUTLINE = 1u;

const uint TEXT_PASS_FILL = 2u;

const uint TEXT_PASS_ALL = 3u;

TextGlyph text_pass(TextGlyph g, uint part) {
    TextGlyph out_ = TextGlyph(vec4(0.0), vec4(0.0), vec4(0.0), vec4(0.0), vec4(0.0), vec4(0.0), vec4(0.0));
    bool local = false;
    out_ = g;
    if ((part == TEXT_PASS_EFFECTS)) {
        out_.color.w = 0.0;
        out_.outline.w = 0.0;
    }
    if (!((part == TEXT_PASS_OUTLINE))) {
        local = (part == TEXT_PASS_FILL);
    } else {
        local = true;
    }
    bool _e19 = local;
    if (_e19) {
        out_.glow.w = 0.0;
        out_.shadow.w = 0.0;
    }
    if ((part == TEXT_PASS_OUTLINE)) {
        out_.color.w = 0.0;
    }
    if ((part == TEXT_PASS_FILL)) {
        out_.outline.w = 0.0;
    }
    TextGlyph _e36 = out_;
    return _e36;
}

bool text_pass_draws(TextGlyph g_1, uint part_1) {
    bool local_1 = false;
    bool local_2 = false;
    bool local_3 = false;
    bool local_4 = false;
    bool local_5 = false;
    bool raw = (g_1.shadow_params.w > 0.5);
    if ((part_1 == TEXT_PASS_EFFECTS)) {
        if (!(raw)) {
            if ((g_1.glow.w > 0.0)) {
                local_2 = (g_1.params.z > 0.0);
            } else {
                local_2 = false;
            }
            bool _e22 = local_2;
            if (!(_e22)) {
                local_3 = (g_1.shadow.w > 0.0);
            } else {
                local_3 = true;
            }
            bool _e31 = local_3;
            local_1 = _e31;
        } else {
            local_1 = false;
        }
        bool _e33 = local_1;
        return _e33;
    }
    if ((part_1 == TEXT_PASS_OUTLINE)) {
        if (!(raw)) {
            local_4 = (g_1.outline.w > 0.0);
        } else {
            local_4 = false;
        }
        bool _e44 = local_4;
        if (_e44) {
            local_5 = (g_1.params.y > 0.0);
        } else {
            local_5 = false;
        }
        bool _e52 = local_5;
        return _e52;
    }
    return true;
}

float median3_(float a, float b, float c) {
    return max(min(a, b), min(max(a, b), c));
}

float text_px_per_texel(TextPixel p) {
    vec2 texels_per_px = ((abs(p.duv_dx) + abs(p.duv_dy)) * p.atlas_size);
    return (2.0 / max((texels_per_px.x + texels_per_px.y), 1e-6));
}

float text_screen_range(TextGlyph g_2, TextPixel p_1) {
    float px_range = ((g_2.params.w > 0.0) ? g_2.params.w : p_1.px_range);
    float _e10 = text_px_per_texel(p_1);
    return max((px_range * _e10), 1.0);
}

float text_outside(TextGlyph g_3, TextPixel p_2, vec4 s) {
    float _e6 = median3_(s.x, s.y, s.z);
    float field = ((p_2.has_true_distance > 0.5) ? s.w : _e6);
    float _e14 = text_screen_range(g_3, p_2);
    return (((field - 0.5) * _e14) + g_3.params.x);
}

float text_glow_radius(TextGlyph g_4, TextPixel p_3) {
    float _e4 = text_screen_range(g_4, p_3);
    return min(g_4.params.z, ((0.5 * _e4) - 0.5));
}

float text_glow_spread(TextGlyph g_5, TextPixel p_4, vec4 s_1) {
    bool local_6 = false;
    float _e3 = text_glow_radius(g_5, p_4);
    if (!((_e3 <= 0.0))) {
        local_6 = (g_5.glow.w <= 0.0);
    } else {
        local_6 = true;
    }
    bool _e14 = local_6;
    if (_e14) {
        return 0.0;
    }
    float _e18 = text_outside(g_5, p_4, s_1);
    return min((_e3 * 0.2), (max(-(_e18), 0.0) * 0.35));
}

vec2 text_tap(TextGlyph g_6, TextPixel p_5, vec2 offset, float spread) {
    vec2 uv = (p_5.uv + (((p_5.duv_dx * offset.x) + (p_5.duv_dy * offset.y)) * spread));
    return clamp(uv, min(g_6.uv_rect.xy, g_6.uv_rect.zw), max(g_6.uv_rect.xy, g_6.uv_rect.zw));
}

TextTaps text_taps(TextGlyph g_7, TextPixel p_6, vec4 s_2) {
    TextTaps t = TextTaps(vec2(0.0), vec2(0.0), vec2(0.0), vec2(0.0), vec2(0.0), 0.0, 0.0);
    float _e3 = text_glow_spread(g_7, p_6, s_2);
    vec2 _e9 = text_tap(g_7, p_6, vec2(-1.0, -1.0), _e3);
    t.glow_a = _e9;
    vec2 _e14 = text_tap(g_7, p_6, vec2(1.0, -1.0), _e3);
    t.glow_b = _e14;
    vec2 _e19 = text_tap(g_7, p_6, vec2(-1.0, 1.0), _e3);
    t.glow_c = _e19;
    vec2 _e24 = text_tap(g_7, p_6, vec2(1.0, 1.0), _e3);
    t.glow_d = _e24;
    t.glow = ((_e3 > 0.0) ? 1.0 : 0.0);
    vec2 back = ((p_6.duv_dx * g_7.shadow_params.x) + (p_6.duv_dy * g_7.shadow_params.y));
    t.shadow_uv = clamp((p_6.uv - back), min(g_7.uv_rect.xy, g_7.uv_rect.zw), max(g_7.uv_rect.xy, g_7.uv_rect.zw));
    t.shadow = ((g_7.shadow.w > 0.0) ? 1.0 : 0.0);
    TextTaps _e62 = t;
    return _e62;
}

float text_glow_term(TextGlyph g_8, TextPixel p_7, vec4 v, float radius) {
    float _e4 = text_outside(g_8, p_7, v);
    float f = clamp((1.0 + (_e4 / radius)), 0.0, 1.0);
    return (f * f);
}

vec4 text_color(TextGlyph g_9, TextPixel p_8, TextSamples v_1) {
    float fill = 0.0;
    vec3 rgb = vec3(0.0);
    float alpha = 0.0;
    bool local_7 = false;
    bool local_8 = false;
    float glow_value = 0.0;
    vec4 s_3 = v_1.center;
    float _e4 = text_px_per_texel(p_8);
    float _e5 = text_screen_range(g_9, p_8);
    float weight = g_9.params.x;
    float _e11 = median3_(s_3.x, s_3.y, s_3.z);
    float sd = (((_e11 - 0.5) * _e5) + weight);
    fill = clamp((sd + 0.5), 0.0, 1.0);
    float _e22 = text_outside(g_9, p_8, s_3);
    rgb = g_9.color.xyz;
    float _e28 = fill;
    alpha = (g_9.color.w * _e28);
    if ((p_8.has_true_distance > 0.5)) {
        float k = clamp(((_e4 * 2.0) - 1.0), 0.0, 1.0);
        float _e46 = fill;
        fill = mix(clamp((_e22 + 0.5), 0.0, 1.0), _e46, k);
        float _e50 = fill;
        alpha = (g_9.color.w * _e50);
        float ow = g_9.outline.w;
        if ((g_9.params.y > 0.0)) {
            local_7 = (ow > 0.0);
        } else {
            local_7 = false;
        }
        bool _e63 = local_7;
        if (_e63) {
            float width = min(g_9.params.y, ((0.5 * _e5) - 1.0));
            float band = clamp(((_e22 + width) + 0.5), 0.0, 1.0);
            float _e78 = fill;
            float oa = ((ow * band) * (1.0 - _e78));
            float _e84 = alpha;
            float _e90 = alpha;
            rgb = (((g_9.color.xyz * _e84) + (g_9.outline.xyz * oa)) / vec3(max((_e90 + oa), 1e-6)));
            float _e96 = alpha;
            alpha = (_e96 + oa);
        }
    }
    float _e98 = text_glow_radius(g_9, p_8);
    if ((_e98 > 0.0)) {
        local_8 = (g_9.glow.w > 0.0);
    } else {
        local_8 = false;
    }
    bool _e108 = local_8;
    if (_e108) {
        float falloff = clamp((1.0 + (_e22 / _e98)), 0.0, 1.0);
        glow_value = (falloff * falloff);
        float _e117 = text_glow_spread(g_9, p_8, s_3);
        if ((_e117 > 0.0)) {
            float _e121 = text_glow_term(g_9, p_8, v_1.glow_a, _e98);
            float _e123 = text_glow_term(g_9, p_8, v_1.glow_b, _e98);
            float _e126 = text_glow_term(g_9, p_8, v_1.glow_c, _e98);
            float _e129 = text_glow_term(g_9, p_8, v_1.glow_d, _e98);
            float total = (((_e121 + _e123) + _e126) + _e129);
            float _e131 = glow_value;
            glow_value = (mix(_e131, (total * 0.25), 0.8) * smoothstep(0.0, 0.15, falloff));
        }
        float _e142 = glow_value;
        float _e144 = alpha;
        float ga = ((g_9.glow.w * _e142) * (1.0 - _e144));
        vec3 _e148 = rgb;
        float _e149 = alpha;
        float _e155 = alpha;
        rgb = (((_e148 * _e149) + (g_9.glow.xyz * ga)) / vec3(max((_e155 + ga), 1e-6)));
        float _e161 = alpha;
        alpha = (_e161 + ga);
    }
    if ((g_9.shadow.w > 0.0)) {
        float _e168 = text_outside(g_9, p_8, v_1.shadow);
        float _e182 = alpha;
        float sa = ((g_9.shadow.w * clamp((0.5 + (_e168 / max(g_9.shadow_params.z, 1.0))), 0.0, 1.0)) * (1.0 - _e182));
        vec3 _e186 = rgb;
        float _e187 = alpha;
        float _e193 = alpha;
        rgb = (((_e186 * _e187) + (g_9.shadow.xyz * sa)) / vec3(max((_e193 + sa), 1e-6)));
        float _e199 = alpha;
        alpha = (_e199 + sa);
    }
    vec3 _e201 = rgb;
    float _e202 = alpha;
    return ((g_9.shadow_params.w > 0.5) ? vec4(s_3.xyz, 1.0) : vec4(_e201, _e202));
}

#endif // GLYPHUX_TEXT
