// Generated: do not edit. The Glyphux addon's rust/build.rs translates the types of color_core.wgsl
// with naga every build, and writes this file again. See README.md.

#ifndef GLYPHUX_COLOR_TYPES
#define GLYPHUX_COLOR_TYPES

struct ColorTile {
    uvec2 origin;
    uint mode;
    uint padding;
    uvec4 ids;
};

struct ColorPaint {
    uvec4 header;
    vec4 color;
    vec4 row0_;
    vec4 row1_;
    vec4 geometry;
};

struct ColorStop {
    vec4 color;
    vec4 offset;
};

struct ColorTables {
    uint tiles;
    uint paints;
    uint stops;
    uint recipes;
};

struct ColorCell {
    ColorTile tile;
    vec2 texel;
};

#endif // GLYPHUX_COLOR_TYPES
