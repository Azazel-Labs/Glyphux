struct Globals { viewport: vec4<f32> }
struct Config { values: vec4<f32> } // base resolution, grid, range, minimum pixels
struct Tile { origin: vec2<u32>, mode: u32, padding: u32, ids: vec4<u32> }
struct Paint {
    header: vec4<u32>, // kind, extend, first stop, stop count
    color: vec4<f32>,
    row0: vec4<f32>,
    row1: vec4<f32>,
    geometry: vec4<f32>,
}
struct Stop { color: vec4<f32>, offset: vec4<f32> }
@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var<uniform> config: Config;
@group(1) @binding(1) var atlas: texture_2d<f32>;
@group(1) @binding(2) var linear_sampler: sampler;
@group(1) @binding(3) var<storage, read> tiles: array<Tile>;
@group(1) @binding(4) var<storage, read> paints: array<Paint>;
@group(1) @binding(5) var<storage, read> stops: array<Stop>;
@group(1) @binding(6) var<storage, read> recipes: array<u32>;

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

fn paint_color(id: u32, q: vec2<f32>) -> vec4<f32> {
    let p = paints[id];
    if p.header.x == 0u { return p.color; }
    let point = vec3<f32>(q, 1.0);
    let x = vec2<f32>(dot(p.row0.xyz, point), dot(p.row1.xyz, point));
    var t: f32;
    if p.header.x == 1u { t = dot(x-p.geometry.xy, p.geometry.zw); }
    else { t = (length(x-p.geometry.xy)-p.geometry.z)/(p.geometry.w-p.geometry.z); }
    let first = p.header.z;
    let last = first+p.header.w-1u;
    let a = stops[first].offset.x;
    let b = stops[last].offset.x;
    let span = b-a;
    if p.header.y == 0u {
        if t < a { return stops[first].color; }
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
        if stops[mid].offset.x <= t { lo = mid+1u; } else { hi = mid; }
    }
    if lo == first { return stops[first].color; }
    if lo > last { return stops[last].color; }
    let s0 = stops[lo-1u];
    let s1 = stops[lo];
    return mix(s0.color, s1.color, (t-s0.offset.x)/(s1.offset.x-s0.offset.x));
}

fn recipe_color(id: u32, q: vec2<f32>) -> vec4<f32> {
    let first = recipes[id*2u];
    let count = recipes[id*2u+1u];
    var result = vec4<f32>(0.0);
    for (var i = 0u; i < count; i += 1u) {
        let foreground = paint_color(recipes[first+i], q);
        result = foreground+result*(1.0-foreground.a);
    }
    return result;
}

@fragment fn fs(in: Vertex) -> @location(0) vec4<f32> {
    // Derivatives are evaluated before any tile-dependent control flow.
    let footprint = max(length(dpdx(in.q)), length(dpdy(in.q)));
    let pixels_per_unit = 1.0/max(footprint, 1e-12);
    let grid = u32(config.values.y);
    let cell = min(vec2<u32>(floor((in.q+1.0)*0.5*f32(grid))), vec2<u32>(grid-1u));
    let tile = tiles[cell.y*grid+cell.x];
    if tile.mode == 0u { return vec4<f32>(0.0); }
    if tile.mode == 1u { return recipe_color(tile.ids.x, in.q)*in.opacity; }
    let low = vec2<f32>(cell)*2.0/f32(grid)-1.0;
    let uv = (vec2<f32>(tile.origin)+(in.q-low)*config.values.x*0.5+1.0)/vec2<f32>(textureDimensions(atlas));
    let sample = textureSampleLevel(atlas, linear_sampler, uv, 0.0);
    let d = (sample-0.5)*2.0*config.values.z;
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
        if weights[i] > 0.0 { color += recipe_color(tile.ids[i], in.q)*(weights[i]/total); }
    }
    return color*clamp(0.5+d.a*pixels_per_unit, 0.0, 1.0)*in.opacity;
}

// Debug view of the actual packed texture. Alpha is distance data, not opacity.
@fragment fn fs_atlas(in: Vertex) -> @location(0) vec4<f32> {
    let size = textureDimensions(atlas);
    let p = min(vec2<u32>(clamp(in.q * 0.5 + 0.5, vec2<f32>(0.0), vec2<f32>(1.0)) * vec2<f32>(size)), size - vec2<u32>(1u));
    return vec4<f32>(textureLoad(atlas, p, 0).rgb, 1.0);
}
