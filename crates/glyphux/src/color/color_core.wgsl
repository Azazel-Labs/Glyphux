// How one pixel of a color glyph looks, shared by every backend.
//
// Plain functions, no bindings. A backend's shader defines the four table reads, which
// return record i of one glyph's tables (ColorField::encode), whose records start at the
// offsets in t (zeros for tables that hold one glyph):
//     fn color_tile(t: ColorTables, i: u32) -> ColorTile
//     fn color_paint(t: ColorTables, i: u32) -> ColorPaint
//     fn color_stop(t: ColorTables, i: u32) -> ColorStop
//     fn color_recipe(t: ColorTables, i: u32) -> u32
// then finds the pixel's tile with color_cell, samples the atlas page at the returned texel
// position (bilinear, no derivatives) when the tile needs it, and gets the premultiplied
// color from color_shade. Backends that don't take WGSL get these functions translated by
// naga.

struct ColorTile { origin: vec2<u32>, mode: u32, padding: u32, ids: vec4<u32> }
struct ColorPaint {
    header: vec4<u32>, // kind, extend, first stop, stop count
    color: vec4<f32>,
    row0: vec4<f32>,
    row1: vec4<f32>,
    geometry: vec4<f32>,
}
struct ColorStop { color: vec4<f32>, offset: vec4<f32> }
// Where a glyph's records start in each table (for tables shared by several glyphs).
struct ColorTables { tiles: u32, paints: u32, stops: u32, recipes: u32 }

// The tile under a pixel and where to sample the atlas page for it.
struct ColorCell {
    tile: ColorTile,
    // Texel position in the page (divide by the page size for texture coordinates).
    texel: vec2<f32>,
}

fn color_paint_color(t_: ColorTables, id: u32, q: vec2<f32>) -> vec4<f32> {
    let p = color_paint(t_, id);
    if p.header.x == 0u { return p.color; }
    let point = vec3<f32>(q, 1.0);
    let x = vec2<f32>(dot(p.row0.xyz, point), dot(p.row1.xyz, point));
    var t: f32;
    if p.header.x == 1u { t = dot(x-p.geometry.xy, p.geometry.zw); }
    else { t = (length(x-p.geometry.xy)-p.geometry.z)/(p.geometry.w-p.geometry.z); }
    let first = p.header.z;
    let last = first+p.header.w-1u;
    let a = color_stop(t_, first).offset.x;
    let b = color_stop(t_, last).offset.x;
    let span = b-a;
    if p.header.y == 0u {
        if t < a { return color_stop(t_, first).color; }
        t = clamp(t, a, b);
    }
    else if p.header.y == 1u { t = a+(t-a)-floor((t-a)/span)*span; }
    else { let v = (t-a)-floor((t-a)/(2.0*span))*2.0*span; t = a+span-abs(v-span); }
    // Upper bound selects the right-hand stop at duplicate offsets.
    var lo = first;
    var hi = last+1u;
    loop {
        if lo >= hi { break; }
        let mid = (lo+hi)/2u;
        if color_stop(t_, mid).offset.x <= t { lo = mid+1u; } else { hi = mid; }
    }
    if lo == first { return color_stop(t_, first).color; }
    if lo > last { return color_stop(t_, last).color; }
    let s0 = color_stop(t_, lo-1u);
    let s1 = color_stop(t_, lo);
    return mix(s0.color, s1.color, (t-s0.offset.x)/(s1.offset.x-s0.offset.x));
}

fn color_recipe_color(t: ColorTables, id: u32, q: vec2<f32>) -> vec4<f32> {
    let first = color_recipe(t, id*2u);
    let count = color_recipe(t, id*2u+1u);
    var result = vec4<f32>(0.0);
    for (var i = 0u; i < count; i += 1u) {
        let foreground = color_paint_color(t, color_recipe(t, first+i), q);
        result = foreground+result*(1.0-foreground.a);
    }
    return result;
}

// The tile under q (the quad's position, -1 to 1 on both axes) of a glyph with config
// (base resolution, grid, range, minimum pixels) and tables t.
fn color_cell(t: ColorTables, q: vec2<f32>, config: vec4<f32>) -> ColorCell {
    let grid = u32(config.y);
    let cell = min(vec2<u32>(floor((q+1.0)*0.5*f32(grid))), vec2<u32>(grid-1u));
    var out: ColorCell;
    out.tile = color_tile(t, cell.y*grid+cell.x);
    let low = vec2<f32>(cell)*2.0/f32(grid)-1.0;
    out.texel = vec2<f32>(out.tile.origin)+(q-low)*config.x*0.5+1.0;
    return out;
}

// The premultiplied color at q, `pixels_per_unit` screen pixels per unit of q (from
// derivatives taken before any tile-dependent control flow), with `sample` the atlas page
// at cell.texel (unused for tiles of mode 0 and 1).
fn color_shade(t: ColorTables, q: vec2<f32>, pixels_per_unit: f32, cell: ColorCell, sample: vec4<f32>, config: vec4<f32>) -> vec4<f32> {
    let tile = cell.tile;
    if tile.mode == 0u { return vec4<f32>(0.0); }
    if tile.mode == 1u { return color_recipe_color(t, tile.ids.x, q); }
    let d = (sample-0.5)*2.0*config.z;
    var weights = vec4<f32>(clamp(vec3<f32>(0.5)+d.xyz*pixels_per_unit, vec3<f32>(0.0), vec3<f32>(1.0)), 0.0);
    for (var i = 0u; i < 3u; i += 1u) { if tile.ids[i] == 0xffffffffu { weights[i] = 0.0; } }
    if tile.ids.w != 0xffffffffu { weights.w = max(0.0, 1.0-weights.x-weights.y-weights.z); }
    var total = dot(weights, vec4<f32>(1.0));
    if total == 0.0 {
        var best = 0u;
        var distance = -1e30;
        for (var i = 0u; i < 3u; i += 1u) {
            if tile.ids[i] != 0xffffffffu && d[i] > distance { best = i; distance = d[i]; }
        }
        weights[best] = 1.0;
        total = 1.0;
    }
    var color = vec4<f32>(0.0);
    for (var i = 0u; i < 4u; i += 1u) {
        if weights[i] > 0.0 { color += color_recipe_color(t, tile.ids[i], q)*(weights[i]/total); }
    }
    return color*clamp(0.5+d.a*pixels_per_unit, 0.0, 1.0);
}
