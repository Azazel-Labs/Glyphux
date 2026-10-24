// Color glyphs with wgpu: one instanced quad per glyph, the tables in storage buffers.
// Pixels are shaded by color_core.wgsl, which comes first in the module.
struct Globals { viewport: vec4<f32> }
struct Config { values: vec4<f32> } // base resolution, grid, range, minimum pixels
@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var<uniform> config: Config;
@group(1) @binding(1) var atlas: texture_2d<f32>;
@group(1) @binding(2) var linear_sampler: sampler;
@group(1) @binding(3) var<storage, read> tiles: array<ColorTile>;
@group(1) @binding(4) var<storage, read> paints: array<ColorPaint>;
@group(1) @binding(5) var<storage, read> stops: array<ColorStop>;
@group(1) @binding(6) var<storage, read> recipes: array<u32>;

fn color_tile(t: ColorTables, i: u32) -> ColorTile { return tiles[t.tiles+i]; }
fn color_paint(t: ColorTables, i: u32) -> ColorPaint { return paints[t.paints+i]; }
fn color_stop(t: ColorTables, i: u32) -> ColorStop { return stops[t.stops+i]; }
fn color_recipe(t: ColorTables, i: u32) -> u32 { return recipes[t.recipes+i]; }

struct Vertex {
    @builtin(position) position: vec4<f32>,
    @location(0) q: vec2<f32>,
    @location(1) @interpolate(flat) opacity: f32,
}
@vertex fn vs(@builtin(vertex_index) vertex: u32,
    @location(0) rect: vec4<f32>, @location(1) transform: vec4<f32>, @location(2) opacity: f32) -> Vertex {
    let corner = vec2<f32>(f32(vertex & 1u), f32((vertex >> 1u) & 1u));
    var p = mix(rect.xy, rect.zw, corner);
    let local = p-transform.zw;
    p = transform.zw+vec2<f32>(transform.x*local.x-transform.y*local.y, transform.y*local.x+transform.x*local.y);
    var out: Vertex;
    out.position = vec4<f32>(p/globals.viewport.xy*vec2<f32>(2.0, -2.0)+vec2<f32>(-1.0, 1.0), 0.0, 1.0);
    out.q = 2.0*corner-1.0;
    out.opacity = opacity;
    return out;
}

@fragment fn fs(in: Vertex) -> @location(0) vec4<f32> {
    // Derivatives are evaluated before any tile-dependent control flow.
    let footprint = max(length(dpdx(in.q)), length(dpdy(in.q)));
    let pixels_per_unit = 1.0/max(footprint, 1e-12);
    // One glyph's tables per bind group.
    let tables = ColorTables(0u, 0u, 0u, 0u);
    let cell = color_cell(tables, in.q, config.values);
    var sample = vec4<f32>(0.0);
    if cell.tile.mode >= 2u {
        sample = textureSampleLevel(atlas, linear_sampler, cell.texel/vec2<f32>(textureDimensions(atlas)), 0.0);
    }
    return color_shade(tables, in.q, pixels_per_unit, cell, sample, config.values)*in.opacity;
}

// Debug view of the actual packed texture. Alpha is distance data, not opacity.
@fragment fn fs_atlas(in: Vertex) -> @location(0) vec4<f32> {
    let size = textureDimensions(atlas);
    let p = min(vec2<u32>(clamp(in.q * 0.5 + 0.5, vec2<f32>(0.0), vec2<f32>(1.0)) * vec2<f32>(size)), size - vec2<u32>(1u));
    return vec4<f32>(textureLoad(atlas, p, 0).rgb, 1.0);
}
