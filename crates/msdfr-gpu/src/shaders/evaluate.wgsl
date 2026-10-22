// Template, instantiated twice by the host (WGSL has no generics, and an entry point must not
// even reference bindings its pipeline layout lacks): EVALUATE(g, base, p) with SCAN(g, base,
// i, p) scanning group i.
//   evaluate_tile: from the tile's candidate lists (base: the tile's first group record)
//   evaluate_all:  from every edge, for points anywhere (base unused)
// Returns the distances at `p` (channels in xyz, true distance in w; unmapped) for the
// configured `nc`/`td`.
fn EVALUATE(g: Glyph, tile_base: u32, p: vec2<f32>) -> vec4<f32> {
    var d = vec4<f32>(0.0);
    if (g.groups == 1u) {
        SCAN(g, tile_base, 0u, p);
        for (var c = 0u; c < nc; c++) {
            d[c] = pick(bucket(c), near_pseudo(c, p));
        }
        d.w = tsd.d;
    } else {
        // Per-contour results, then msdfgen's overlapping contour combiner.
        var dists: array<vec4<f32>, MAX_CONTOURS>;
        var shape_acc = acc_empty();
        var inner_acc = acc_empty();
        var outer_acc = acc_empty();
        let n = min(g.groups, MAX_CONTOURS);
        for (var i = 0u; i < n; i++) {
            SCAN(g, tile_base, i, p);
            var pseudo = vec3<f32>(BIG);
            var di = vec4<f32>(0.0);
            for (var c = 0u; c < nc; c++) {
                pseudo[c] = near_pseudo(c, p);
                di[c] = pick(bucket(c), pseudo[c]);
            }
            di.w = tsd.d;
            dists[i] = di;
            acc_merge(&shape_acc, pseudo);
            let s = scalar_of(di);
            let w = contours[g.contour_offset + i].winding;
            if (w > 0 && s >= 0.0) {
                acc_merge(&inner_acc, pseudo);
            }
            if (w < 0 && s <= 0.0) {
                acc_merge(&outer_acc, pseudo);
            }
        }
        let shape_d = acc_resolve(shape_acc);
        let inner_d = acc_resolve(inner_acc);
        let outer_d = acc_resolve(outer_acc);
        let inner_s = scalar_of(inner_d);
        let outer_s = scalar_of(outer_d);
        var winding = 0;
        if (inner_s >= 0.0 && abs(inner_s) <= abs(outer_s)) {
            d = inner_d;
            winding = 1;
            for (var i = 0u; i < n; i++) {
                if (contours[g.contour_offset + i].winding > 0) {
                    let s = scalar_of(dists[i]);
                    if (abs(s) < abs(outer_s) && s > scalar_of(d)) {
                        d = dists[i];
                    }
                }
            }
        } else if (outer_s <= 0.0 && abs(outer_s) < abs(inner_s)) {
            d = outer_d;
            winding = -1;
            for (var i = 0u; i < n; i++) {
                if (contours[g.contour_offset + i].winding < 0) {
                    let s = scalar_of(dists[i]);
                    if (abs(s) < abs(inner_s) && s < scalar_of(d)) {
                        d = dists[i];
                    }
                }
            }
        } else {
            d = shape_d;
        }
        if (winding != 0) {
            for (var i = 0u; i < n; i++) {
                if (contours[g.contour_offset + i].winding != winding) {
                    let s = scalar_of(dists[i]);
                    let ds = scalar_of(d);
                    if (s * ds >= 0.0 && abs(s) < abs(ds)) {
                        d = dists[i];
                    }
                }
            }
            if (scalar_of(d) == scalar_of(shape_d)) {
                d = shape_d;
            }
        }
    }
    return d;
}
