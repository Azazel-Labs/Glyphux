// Generated: do not edit. The Glyphux addon's rust/build.rs translates the functions of color_core.wgsl
// with naga every build, and writes this file again. See README.md.

#ifndef GLYPHUX_COLOR
#define GLYPHUX_COLOR

// Include glyphux_color_types.gdshaderinc first, then define how the shader reads
// the tables: color_tile, color_paint, color_stop, color_recipe
// (each taking a ColorTables and a uint, and returning the record).

vec4 color_paint_color(ColorTables t_2, uint id, vec2 q) {
    float t_3 = 0.0;
    uint lo = 0u;
    uint hi = 0u;
    ColorPaint _e3 = color_paint(t_2, id);
    if ((_e3.header.x == 0u)) {
        return _e3.color;
    }
    vec3 point = vec3(q, 1.0);
    vec2 x = vec2(dot(_e3.row0_.xyz, point), dot(_e3.row1_.xyz, point));
    if ((_e3.header.x == 1u)) {
        t_3 = dot((x - _e3.geometry.xy), _e3.geometry.zw);
    } else {
        t_3 = ((length((x - _e3.geometry.xy)) - _e3.geometry.z) / (_e3.geometry.w - _e3.geometry.z));
    }
    uint first = _e3.header.z;
    uint last = ((first + _e3.header.w) - 1u);
    ColorStop _e49 = color_stop(t_2, first);
    float a = _e49.offset.x;
    ColorStop _e52 = color_stop(t_2, last);
    float b = _e52.offset.x;
    float span = (b - a);
    if ((_e3.header.y == 0u)) {
        float _e60 = t_3;
        if ((_e60 < a)) {
            ColorStop _e62 = color_stop(t_2, first);
            return _e62.color;
        }
        float _e64 = t_3;
        t_3 = clamp(_e64, a, b);
    } else {
        if ((_e3.header.y == 1u)) {
            float _e70 = t_3;
            float _e73 = t_3;
            t_3 = ((a + (_e70 - a)) - (floor(((_e73 - a) / span)) * span));
        } else {
            float _e79 = t_3;
            float _e81 = t_3;
            float v_3 = ((_e79 - a) - ((floor(((_e81 - a) / (2.0 * span))) * 2.0) * span));
            t_3 = ((a + span) - abs((v_3 - span)));
        }
    }
    lo = first;
    hi = (last + 1u);
    while(true) {
        uint _e99 = lo;
        uint _e100 = hi;
        if ((_e99 >= _e100)) {
            break;
        }
        uint _e102 = lo;
        uint _e103 = hi;
        uint mid = ((_e102 + _e103) / 2u);
        ColorStop _e107 = color_stop(t_2, mid);
        float _e110 = t_3;
        if ((_e107.offset.x <= _e110)) {
            lo = (mid + 1u);
        } else {
            hi = mid;
        }
    }
    uint _e114 = lo;
    if ((_e114 == first)) {
        ColorStop _e116 = color_stop(t_2, first);
        return _e116.color;
    }
    uint _e118 = lo;
    if ((_e118 > last)) {
        ColorStop _e120 = color_stop(t_2, last);
        return _e120.color;
    }
    uint _e122 = lo;
    ColorStop _e125 = color_stop(t_2, (_e122 - 1u));
    uint _e126 = lo;
    ColorStop _e127 = color_stop(t_2, _e126);
    float _e130 = t_3;
    return mix(_e125.color, _e127.color, ((_e130 - _e125.offset.x) / (_e127.offset.x - _e125.offset.x)));
}

vec4 color_recipe_color(ColorTables t_5, uint id_1, vec2 q_1) {
    vec4 result = vec4(0.0);
    uint i_3 = 0u;
    uint _e5 = color_recipe(t_5, (id_1 * 2u));
    uint _e10 = color_recipe(t_5, ((id_1 * 2u) + 1u));
    bool loop_init = true;
    while(true) {
        if (!loop_init) {
            uint _e28 = i_3;
            i_3 = (_e28 + 1u);
        }
        loop_init = false;
        uint _e16 = i_3;
        if ((_e16 < _e10)) {
        } else {
            break;
        }
        {
            uint _e18 = i_3;
            uint _e20 = color_recipe(t_5, (_e5 + _e18));
            vec4 _e21 = color_paint_color(t_5, _e20, q_1);
            vec4 _e22 = result;
            result = (_e21 + (_e22 * (1.0 - _e21.w)));
        }
    }
    vec4 _e31 = result;
    return _e31;
}

ColorCell color_cell(ColorTables t_7, vec2 q_2, vec4 config) {
    ColorCell out_ = ColorCell(ColorTile(uvec2(0u), 0u, 0u, uvec4(0u)), vec2(0.0));
    uint grid = uint(config.y);
    uvec2 cell_1 = min(uvec2(floor((((q_2 + vec2(1.0)) * 0.5) * float(grid)))), uvec2((grid - 1u)));
    ColorTile _e24 = color_tile(t_7, ((cell_1.y * grid) + cell_1.x));
    out_.tile = _e24;
    vec2 low = (((vec2(cell_1) * 2.0) / vec2(float(grid))) - vec2(1.0));
    uvec2 _e37 = out_.tile.origin;
    out_.texel = ((vec2(_e37) + (((q_2 - low) * config.x) * 0.5)) + vec2(1.0));
    ColorCell _e48 = out_;
    return _e48;
}

vec4 color_shade(ColorTables t_8, vec2 q_3, float pixels_per_unit, ColorCell cell, vec4 sample_, vec4 config_1) {
    vec4 weights = vec4(0.0);
    uint i_5 = 0u;
    float total = 0.0;
    uint best = 0u;
    float distance_ = -1e30;
    uint i_6 = 0u;
    bool local = false;
    vec4 color = vec4(0.0);
    uint i_7 = 0u;
    ColorTile tile = cell.tile;
    if ((tile.mode == 0u)) {
        return vec4(0.0);
    }
    if ((tile.mode == 1u)) {
        vec4 _e17 = color_recipe_color(t_8, tile.ids.x, q_3);
        return _e17;
    }
    vec4 d = (((sample_ - vec4(0.5)) * 2.0) * config_1.z);
    weights = vec4(clamp((vec3(0.5) + (d.xyz * pixels_per_unit)), vec3(0.0), vec3(1.0)), 0.0);
    bool loop_init_1 = true;
    while(true) {
        if (!loop_init_1) {
            uint _e51 = i_5;
            i_5 = (_e51 + 1u);
        }
        loop_init_1 = false;
        uint _e40 = i_5;
        if ((_e40 < 3u)) {
        } else {
            break;
        }
        {
            uint _e44 = i_5;
            if ((tile.ids[_e44] == 4294967295u)) {
                uint _e48 = i_5;
                weights[_e48] = 0.0;
            }
        }
    }
    if ((tile.ids.w != 4294967295u)) {
        float _e60 = weights.x;
        float _e64 = weights.y;
        float _e67 = weights.z;
        weights.w = max(0.0, (((1.0 - _e60) - _e64) - _e67));
    }
    vec4 _e71 = weights;
    total = dot(_e71, vec4(1.0));
    float _e76 = total;
    if ((_e76 == 0.0)) {
        bool loop_init_2 = true;
        while(true) {
            if (!loop_init_2) {
                uint _e104 = i_6;
                i_6 = (_e104 + 1u);
            }
            loop_init_2 = false;
            uint _e85 = i_6;
            if ((_e85 < 3u)) {
            } else {
                break;
            }
            {
                uint _e89 = i_6;
                if ((tile.ids[_e89] != 4294967295u)) {
                    uint _e95 = i_6;
                    float _e97 = distance_;
                    local = (d[_e95] > _e97);
                } else {
                    local = false;
                }
                bool _e100 = local;
                if (_e100) {
                    uint _e101 = i_6;
                    best = _e101;
                    uint _e102 = i_6;
                    distance_ = d[_e102];
                }
            }
        }
        uint _e107 = best;
        weights[_e107] = 1.0;
        total = 1.0;
    }
    bool loop_init_3 = true;
    while(true) {
        if (!loop_init_3) {
            uint _e136 = i_7;
            i_7 = (_e136 + 1u);
        }
        loop_init_3 = false;
        uint _e116 = i_7;
        if ((_e116 < 4u)) {
        } else {
            break;
        }
        {
            uint _e119 = i_7;
            float _e121 = weights[_e119];
            if ((_e121 > 0.0)) {
                vec4 _e124 = color;
                uint _e126 = i_7;
                vec4 _e128 = color_recipe_color(t_8, tile.ids[_e126], q_3);
                uint _e129 = i_7;
                float _e131 = weights[_e129];
                float _e132 = total;
                color = (_e124 + (_e128 * (_e131 / _e132)));
            }
        }
    }
    vec4 _e139 = color;
    return (_e139 * clamp((0.5 + (d.w * pixels_per_unit)), 0.0, 1.0));
}

#endif // GLYPHUX_COLOR
