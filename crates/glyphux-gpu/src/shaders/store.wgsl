// Atlas store pass: copies each glyph's field into its rectangle of the atlas texture array
// (Glyph.atlas_*), clamped for the texture format. Rows are stored top to bottom, so texture
// y grows downwards like the field rows (for y-up shapes).
// ATLAS_FORMAT and the clamp range are substituted by the host.

@group(0) @binding(10) var atlas: texture_storage_2d_array<ATLAS_FORMAT, write>;

const STORE_MIN: f32 = STORE_LO;
const STORE_MAX: f32 = STORE_HI;

@compute @workgroup_size(8, 8)
fn store(@builtin(workgroup_id) wid: vec3<u32>, @builtin(local_invocation_id) lid: vec3<u32>) {
    let wi = wid.x + wid.y * DISPATCH_X;
    if (wi >= arrayLength(&work)) {
        return;
    }
    let item = work[wi];
    let g = glyphs[item.x];
    let x = (item.y % g.tiles_x) * g.tile_size + lid.x;
    let r = (item.y / g.tiles_x) * g.tile_size + lid.y;
    if (x >= g.width || r >= g.height || lid.x >= g.tile_size || lid.y >= g.tile_size) {
        return;
    }
    let v = field[g.field_offset + r * g.width + x];
    var o = v;
    let channels = g.flags & 3u;
    let has_td = (g.flags & 4u) != 0u;
    if (channels == 0u) {
        o = vec4<f32>(v.w);
    } else if (channels == 1u) {
        o = vec4<f32>(v.x);
    } else if (!has_td) {
        o = vec4<f32>(v.xyz, median3(v.x, v.y, v.z));
    }
    o = clamp(o, vec4<f32>(STORE_MIN), vec4<f32>(STORE_MAX));
    textureStore(atlas, vec2<u32>(g.atlas_x + x, g.atlas_y + r), g.atlas_layer, o);
}
