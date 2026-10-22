// MSDF error correction (msdfr's correction.rs / msdfgen's MSDFErrorCorrection).
//
// Passes, each over the texels of a batch (same work list as the fine pass), except the
// corner pass which runs over the batch's corner list:
//   ec_corners  protect the 4 texels around each color-changing corner
//   ec_protect  protect texels whose extreme channels form an edge with a neighbor
//   ec_find     find artifacts (field only), then verify candidates against the exact
//               pseudo-distance of the shape where configured
//   ec_apply    flatten the texels marked as errors to their median
//
// Glyph.ec_flags: bit 0 enabled, bit 1 protect corners and edges, bit 2 protect everything,
// bit 3 field-only search, bit 4 distance check, bit 5 distance check only for texels
// flagged by the field-only search (two-phase "check at edge").

const EC_ON: u32 = 1u;
const EC_PROTECT: u32 = 2u;
const EC_PROTECT_ALL: u32 = 4u;
const EC_FIND: u32 = 8u;
const EC_CHECK: u32 = 16u;
const EC_ONLY_FLAGGED: u32 = 32u;

const ST_ERROR: u32 = 1u;
const ST_PROTECTED: u32 = 2u;
const ST_NEEDS_CHECK: u32 = 4u;

const CANDIDATE: u32 = 1u;
const ARTIFACT: u32 = 2u;
const INVERSION: u32 = 4u;

const ARTIFACT_T_EPSILON: f32 = 0.01;
// Texels closer to the edge than a neighbor are not blamed for an artifact between them. In
// symmetric geometry the two are often exactly as close (the CPU then blames both); the f32
// field is only accurate to a few 1e-6, so near-ties count as ties.
const BLAME_TIE: f32 = 4e-6;

struct Texel {
    g: Glyph,
    x: i32,
    r: i32,
    w: i32,
    h: i32,
    base: u32,
}

var<private> cur: Texel;
// Classifier state: distance checks enabled, and (field-only) inversion candidates seen.
var<private> use_distance: bool;
var<private> inversion_candidate: bool;

fn px(x: i32, r: i32) -> vec3<f32> {
    return field[cur.base + u32(r * cur.w + x)].xyz;
}

fn med(v: vec3<f32>) -> f32 {
    return median3(v.x, v.y, v.z);
}

// Bilinear sample in row space (texel centers at half-integers), clamped to the glyph.
fn sample_field(x: f32, y: f32) -> vec3<f32> {
    let fx = x - 0.5;
    let fy = y - 0.5;
    let x0 = floor(fx);
    let y0 = floor(fy);
    let tx = fx - x0;
    let ty = fy - y0;
    let xi = i32(x0);
    let yi = i32(y0);
    let xa = clamp(xi, 0, cur.w - 1);
    let xb = clamp(xi + 1, 0, cur.w - 1);
    let ya = clamp(yi, 0, cur.h - 1);
    let yb = clamp(yi + 1, 0, cur.h - 1);
    let a = px(xa, ya);
    let b = px(xb, ya);
    let c = px(xa, yb);
    let d = px(xb, yb);
    let top = a + (b - a) * tx;
    let bot = c + (d - c) * tx;
    return top + (bot - top) * ty;
}

fn range_test(at: f32, bt: f32, xt: f32, am: f32, bm: f32, xm: f32, span: f32, prot: bool) -> u32 {
    let z = cur.g.zero;
    let inversion = (am > z && bm > z && xm <= z) || (am < z && bm < z && xm >= z);
    if (inversion || (!prot && median3(am, bm, xm) != xm)) {
        let ax = (xt - at) * span;
        let bx = (bt - xt) * span;
        let inv = select(0u, INVERSION, inversion);
        // The true field can change by at most `span` per unit of t.
        if (!(xm >= am - ax && xm <= am + ax && xm >= bm - bx && xm <= bm + bx)) {
            return CANDIDATE | ARTIFACT | inv;
        }
        return CANDIDATE | inv;
    }
    return 0u;
}

// Confirms a candidate by comparing the interpolated value before and after the correction
// with the shape's exact pseudo-distance at that point.
fn classify(dir: vec2<f32>, t: f32, flags: u32) -> bool {
    if (!use_distance) {
        if ((flags & INVERSION) != 0u) {
            inversion_candidate = true;
        }
        return (flags & ARTIFACT) != 0u;
    }
    if ((flags & CANDIDATE) == 0u) {
        return false;
    }
    if ((flags & ARTIFACT) != 0u) {
        return true;
    }
    let g = cur.g;
    let tx = t * dir.x;
    let tr = t * dir.y;
    let bx = f32(cur.x) + 0.5;
    let br = f32(cur.r) + 0.5;
    let old = sample_field(bx + tx, br + tr);
    let wa = (1.0 - abs(tx)) * (1.0 - abs(tr));
    let c = px(cur.x, cur.r);
    let cm = med(c);
    let nw = old + wa * (vec3<f32>(cm) - c);
    let old_m = med(old);
    let new_m = med(nw);
    var ty = f32(cur.h - 1 - cur.r) + 0.5 - tr;
    if ((g.flags & 8u) != 0u) {
        ty = br + tr;
    }
    let sp = vec2<f32>(bx + tx, ty) / g.scale - g.translate;
    // Single-channel pseudo-distance over all edges.
    let saved_nc = nc;
    let saved_td = td;
    nc = 1u;
    td = false;
    let ref_d = evaluate_all(g, 0u, sp).x;
    nc = saved_nc;
    td = saved_td;
    let refv = g.map_scale * (ref_d + g.map_translate);
    return g.improve * abs(new_m - refv) < abs(old_m - refv);
}

fn lerp_median(a: vec3<f32>, b: vec3<f32>, t: f32) -> f32 {
    return med(a + (b - a) * t);
}

fn linear_artifact(a: vec3<f32>, am: f32, b: vec3<f32>, bm: f32, span: f32, prot: bool,
                   dir: vec2<f32>) -> bool {
    let da3 = vec3<f32>(a.y - a.x, a.z - a.y, a.x - a.z);
    let db3 = vec3<f32>(b.y - b.x, b.z - b.y, b.x - b.z);
    for (var k = 0u; k < 3u; k++) {
        let da = da3[k];
        let db = db3[k];
        if (da == db) {
            continue;
        }
        // Where the two channels cross, the median is extreme.
        let t = da / (da - db);
        if (t > ARTIFACT_T_EPSILON && t < 1.0 - ARTIFACT_T_EPSILON) {
            let xm = lerp_median(a, b, t);
            let f = range_test(0.0, 1.0, t, am, bm, xm, span, prot);
            if (f != 0u && classify(dir, t, f)) {
                return true;
            }
        }
    }
    return false;
}

fn quad_median(a: vec3<f32>, l: vec3<f32>, q: vec3<f32>, t: f32) -> f32 {
    return med(t * (t * q + l) + a);
}

struct Roots {
    n: u32,
    r: array<f32, 2>,
}

fn solve_quadratic(a: f32, b: f32, c: f32) -> Roots {
    var out = Roots(0u, array<f32, 2>(0.0, 0.0));
    if (a == 0.0 || abs(b) > 1e12 * abs(a)) {
        if (b == 0.0) {
            return out;
        }
        out.n = 1u;
        out.r[0] = -c / b;
        return out;
    }
    let disc = b * b - 4.0 * a * c;
    if (disc > 0.0) {
        let s = sqrt(disc);
        let q = -0.5 * (b + select(-s, s, b >= 0.0));
        let r1 = q / a;
        out.n = 2u;
        out.r[0] = r1;
        out.r[1] = select(-r1, c / q, q != 0.0);
    } else if (disc == 0.0) {
        out.n = 1u;
        out.r[0] = -0.5 * b / a;
    }
    return out;
}

fn diagonal_artifact(a: vec3<f32>, am: f32, b: vec3<f32>, c: vec3<f32>, d: vec3<f32>, dm: f32,
                     span: f32, prot: bool, dir: vec2<f32>) -> bool {
    // Along the diagonal: v(t) = a + l t + q t^2.
    let abc = a - b - c;
    let l = -a - abc;
    let q = d + abc;
    for (var k = 0u; k < 3u; k++) {
        let i = (k + 1u) % 3u;
        let j = k;
        let da = a[i] - a[j];
        let dbc = (b[i] - b[j]) + (c[i] - c[j]);
        let dd = d[i] - d[j];
        // Bernstein coefficients (da, dbc/2, dd) of the same sign: no crossing in [0, 1].
        if ((da > 0.0 && dbc > 0.0 && dd > 0.0) || (da < 0.0 && dbc < 0.0 && dd < 0.0)) {
            continue;
        }
        let roots = solve_quadratic(dd - dbc + da, dbc - da - da, da);
        for (var ri = 0u; ri < roots.n; ri++) {
            let t = roots.r[ri];
            if (!(t > ARTIFACT_T_EPSILON && t < 1.0 - ARTIFACT_T_EPSILON)) {
                continue;
            }
            let xm = quad_median(a, l, q, t);
            var f = range_test(0.0, 1.0, t, am, dm, xm, span, prot);
            // Also compare against the medians at the channels' local extremes.
            var tes = array<f32, 2>(-0.5 * l[j] / q[j], -0.5 * l[i] / q[i]);
            for (var e = 0u; e < 2u; e++) {
                let te = tes[e];
                if (te > 0.0 && te < 1.0) {
                    var t0 = 0.0;
                    var t1 = 1.0;
                    var m0 = am;
                    var m1 = dm;
                    if (te > t) {
                        t1 = te;
                        m1 = quad_median(a, l, q, te);
                    } else {
                        t0 = te;
                        m0 = quad_median(a, l, q, te);
                    }
                    f |= range_test(t0, t1, t, m0, m1, xm, span, prot);
                }
            }
            if (f != 0u && classify(dir, t, f)) {
                return true;
            }
        }
    }
    return false;
}

// Checks the texel against its 8 neighbors.
fn texel_has_artifact(prot: bool) -> bool {
    let x = cur.x;
    let r = cur.r;
    let z = cur.g.zero;
    let c = px(x, r);
    let cm = med(c);
    let cdist = abs(cm - z);
    var axis = array<vec2<i32>, 4>(vec2<i32>(-1, 0), vec2<i32>(0, 1), vec2<i32>(1, 0), vec2<i32>(0, -1));
    for (var k = 0u; k < 4u; k++) {
        let o = axis[k];
        let nx = x + o.x;
        let nr = r + o.y;
        if (nx < 0 || nr < 0 || nx >= cur.w || nr >= cur.h) {
            continue;
        }
        let n = px(nx, nr);
        let nm = med(n);
        // Only blame the texel that is farther from the edge.
        if (cdist < abs(nm - z) - BLAME_TIE) {
            continue;
        }
        let span = select(cur.g.span_v, cur.g.span_h, o.x != 0);
        if (linear_artifact(c, cm, n, nm, span, prot, vec2<f32>(o))) {
            return true;
        }
    }
    var diag = array<vec2<i32>, 4>(vec2<i32>(-1, -1), vec2<i32>(1, -1), vec2<i32>(-1, 1), vec2<i32>(1, 1));
    for (var k = 0u; k < 4u; k++) {
        let o = diag[k];
        let nx = x + o.x;
        let nr = r + o.y;
        if (nx < 0 || nr < 0 || nx >= cur.w || nr >= cur.h) {
            continue;
        }
        let d = px(nx, nr);
        let dm = med(d);
        if (cdist < abs(dm - z) - BLAME_TIE) {
            continue;
        }
        let b = px(nx, r);
        let cc = px(x, nr);
        if (diagonal_artifact(c, cm, b, cc, d, dm, cur.g.span_d, prot, vec2<f32>(o))) {
            return true;
        }
    }
    return false;
}

// Bit mask of channels that form an edge between texels `a` and `b`.
fn edge_between_texels(a: vec3<f32>, b: vec3<f32>) -> u32 {
    let z = cur.g.zero;
    var mask = 0u;
    for (var k = 0u; k < 3u; k++) {
        let den = a[k] - b[k];
        if (den == 0.0) {
            continue;
        }
        let t = (a[k] - z) / den;
        if (t > 0.0 && t < 1.0) {
            let v = a + (b - a) * t;
            if (med(v) == v[k]) {
                mask |= 1u << k;
            }
        }
    }
    return mask;
}

fn crosses(a: vec3<f32>, b: vec3<f32>, z: f32) -> bool {
    let zv = vec3<f32>(z);
    let above_a = a > zv;
    let below_a = a < zv;
    let above_b = b > zv;
    let below_b = b < zv;
    return any((above_a & below_b) | (below_a & above_b));
}

// Sets up `cur` for the invocation; false if it has no texel.
fn ec_texel(wid: vec3<u32>, nwg: vec3<u32>, lid: vec3<u32>) -> bool {
    let wi = wid.x + wid.y * nwg.x;
    if (wi >= arrayLength(&work)) {
        return false;
    }
    let item = work[wi];
    let g = glyphs[item.x];
    if ((g.ec_flags & EC_ON) == 0u || (g.flags & 3u) != 3u) {
        return false;
    }
    let tx = item.y % g.tiles_x;
    let tyi = item.y / g.tiles_x;
    let x = tx * g.tile_size + lid.x;
    let r = tyi * g.tile_size + lid.y;
    if (x >= g.width || r >= g.height || lid.x >= g.tile_size || lid.y >= g.tile_size) {
        return false;
    }
    cur = Texel(g, i32(x), i32(r), i32(g.width), i32(g.height), g.field_offset);
    return true;
}

@compute @workgroup_size(64)
fn ec_corners(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let i = gid.x + gid.y * nwg.x * 64u;
    if (i < arrayLength(&corners)) {
        let t = corners[i];
        if (t != NONE) {
            atomicOr(&stencil[t], ST_PROTECTED);
        }
    }
}

@compute @workgroup_size(8, 8)
fn ec_protect(@builtin(workgroup_id) wid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>,
              @builtin(local_invocation_id) lid: vec3<u32>) {
    if (!ec_texel(wid, nwg, lid)) {
        return;
    }
    let idx = cur.base + u32(cur.r * cur.w + cur.x);
    if ((cur.g.ec_flags & EC_PROTECT_ALL) != 0u) {
        atomicOr(&stencil[idx], ST_PROTECTED);
        return;
    }
    if ((cur.g.ec_flags & EC_PROTECT) == 0u) {
        return;
    }
    let z = cur.g.zero;
    let c = px(cur.x, cur.r);
    let cm = med(c);
    let cd = abs(cm - z);
    // Pairs are ordered by texel index (as on the CPU, where each pair is examined once).
    var offs = array<vec2<i32>, 8>(
        vec2<i32>(1, 0), vec2<i32>(-1, 0), vec2<i32>(0, 1), vec2<i32>(0, -1),
        vec2<i32>(1, 1), vec2<i32>(-1, -1), vec2<i32>(-1, 1), vec2<i32>(1, -1));
    for (var k = 0u; k < 8u; k++) {
        let o = offs[k];
        let nx = cur.x + o.x;
        let nr = cur.r + o.y;
        if (nx < 0 || nr < 0 || nx >= cur.w || nr >= cur.h) {
            continue;
        }
        let n = px(nx, nr);
        let nd = abs(med(n) - z);
        var radius = cur.g.rad_d;
        if (o.y == 0) {
            radius = cur.g.rad_h;
        } else if (o.x == 0) {
            radius = cur.g.rad_v;
        }
        if (!(cd + nd < radius) || !crosses(c, n, z)) {
            continue;
        }
        let first = o.y > 0 || (o.y == 0 && o.x > 0);
        var mask = 0u;
        if (first) {
            mask = edge_between_texels(c, n);
        } else {
            mask = edge_between_texels(n, c);
        }
        let ext = (c != vec3<f32>(cm)) & vec3<bool>((mask & 1u) != 0u, (mask & 2u) != 0u, (mask & 4u) != 0u);
        if (any(ext)) {
            atomicOr(&stencil[idx], ST_PROTECTED);
            return;
        }
    }
}

@compute @workgroup_size(8, 8)
fn ec_find(@builtin(workgroup_id) wid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>,
           @builtin(local_invocation_id) lid: vec3<u32>) {
    if (!ec_texel(wid, nwg, lid)) {
        return;
    }
    let idx = cur.base + u32(cur.r * cur.w + cur.x);
    var st = atomicLoad(&stencil[idx]);
    let f = cur.g.ec_flags;
    if ((f & EC_FIND) != 0u) {
        use_distance = false;
        inversion_candidate = false;
        if (texel_has_artifact((st & ST_PROTECTED) != 0u)) {
            st |= ST_ERROR;
        } else if (inversion_candidate) {
            st |= ST_NEEDS_CHECK;
        }
    }
    if ((f & EC_CHECK) != 0u && (st & ST_ERROR) == 0u) {
        let only = (f & EC_ONLY_FLAGGED) != 0u;
        if (!only || (st & ST_NEEDS_CHECK) != 0u) {
            use_distance = true;
            if (texel_has_artifact(only || (st & ST_PROTECTED) != 0u)) {
                st |= ST_ERROR;
            }
        }
    }
    atomicStore(&stencil[idx], st);
}

@compute @workgroup_size(8, 8)
fn ec_apply(@builtin(workgroup_id) wid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>,
            @builtin(local_invocation_id) lid: vec3<u32>) {
    if (!ec_texel(wid, nwg, lid)) {
        return;
    }
    let idx = cur.base + u32(cur.r * cur.w + cur.x);
    if ((atomicLoad(&stencil[idx]) & ST_ERROR) != 0u) {
        let v = field[idx];
        let m = median3(v.x, v.y, v.z);
        field[idx] = vec4<f32>(m, m, m, v.w);
    }
}
