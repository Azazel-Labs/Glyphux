// Fine pass: one workgroup per 8x8 tile of one glyph, one invocation per texel.

// Narrow band (Glyph.flags bit 4): a tile farther than `band2` (squared, shape units) from
// every edge (TileGroup._pad0 of its first group: the tile's smallest edge key) lies entirely
// inside or outside (_pad1: 1 inside, from the exact winding number on the CPU) and is filled
// with the mapped distance lower bound (beyond the band, so clamped like the exact values).
var<workgroup> wg_far: u32;

// Whether the texel center at x `px` (shape units) of row `r` is inside the outline: the
// exact winding number from the row's scanline crossings, under the glyph's fill rule.
fn row_filled(g: Glyph, r: u32, px: f32) -> bool {
    let start = u32(curves[g.scan_offset + r]);
    let end = u32(curves[g.scan_offset + r + 1u]);
    let pairs = g.scan_offset + g.height + 1u;
    var winding = 0;
    for (var k = start; k < end; k++) {
        if (curves[pairs + 2u * k] > px) {
            winding -= i32(curves[pairs + 2u * k + 1u]);
        }
    }
    switch ((g.flags >> 6u) & 3u) {
        case 0u: { return winding != 0; }
        case 1u: { return (winding & 1) != 0; }
        case 2u: { return winding > 0; }
        default: { return winding < 0; }
    }
}

// msdfgen's scanline sign correction: distances whose sign disagrees with the exact inside
// test are flipped (the channels by their median, the true distance on its own).
fn correct_sign(g: Glyph, d: vec4<f32>, r: u32, px: f32) -> vec4<f32> {
    let inside = row_filled(g, r, px);
    var out = d;
    let s = scalar_of(d);
    if (s != 0.0 && (s > 0.0) != inside) {
        for (var c = 0u; c < nc; c++) {
            out[c] = -out[c];
        }
        if (nc == 0u) {
            out.w = -out.w;
        }
    }
    if (nc != 0u && td && d.w != 0.0 && (d.w > 0.0) != inside) {
        out.w = -out.w;
    }
    return out;
}

@compute @workgroup_size(8, 8)
fn fine(@builtin(workgroup_id) wid: vec3<u32>, @builtin(local_invocation_id) lid: vec3<u32>) {
    let wi = wid.x + wid.y * DISPATCH_X;
    if (wi >= arrayLength(&work)) {
        return;
    }
    let item = work[wi];
    let g = glyphs[item.x];
    let tile = item.y;
    let tx = tile % g.tiles_x;
    let tyi = tile / g.tiles_x;
    let x = tx * g.tile_size + lid.x;
    let r = tyi * g.tile_size + lid.y;
    let in_glyph = x < g.width && r < g.height && lid.x < g.tile_size && lid.y < g.tile_size;
    nc = g.flags & 3u;
    td = (g.flags & 4u) != 0u;
    let first = lid.x == 0u && lid.y == 0u;
    let tile_base = g.tile_offset + tile * g.groups;
    var min_key = 0.0;
    if (first) {
        min_key = bitcast<f32>(tile_groups[tile_base]._pad0);
        wg_far = select(0u, 1u, (g.flags & 16u) != 0u && min_key > g.band2);
    }
    if (workgroupUniformLoad(&wg_far) != 0u) {
        let inside = tile_groups[tile_base]._pad1 != 0u;
        if (in_glyph) {
            let lb = sqrt(bitcast<f32>(tile_groups[tile_base]._pad0));
            let d = select(-lb, lb, inside);
            field[g.field_offset + r * g.width + x] = vec4<f32>(g.map_scale * (d + g.map_translate));
        }
        return;
    }
    if (!in_glyph) {
        return;
    }
    let p = texel_point(g, x, r);
    var d = evaluate_tile(g, tile_base, p);
    if ((g.flags & 32u) != 0u) {
        d = correct_sign(g, d, r, p.x);
    }
    let v = g.map_scale * (d + vec4<f32>(g.map_translate));
    field[g.field_offset + r * g.width + x] = v;
}
