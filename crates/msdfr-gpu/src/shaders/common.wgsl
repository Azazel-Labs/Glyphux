// msdfr-gpu: shared declarations and distance evaluation.
//
// The fine pass (fine.wgsl) evaluates each texel with the same selection semantics as
// msdfr's CPU evaluator (msdfgen's): per channel the nearest edge by true distance (ties
// broken by orthogonality), converted to a pseudo-distance, plus the perpendicular
// distances from corner extensions, and the overlapping contour combiner. Edges come from
// the tile's candidate lists (built with exact bounds on the CPU), sorted by a lower bound so
// the scan can stop early. Error correction (ec.wgsl) evaluates arbitrary points over all
// edges. This file is concatenated in front of the entry point files.

const BIG: f32 = 1e18;
const BIG2: f32 = 1e36;
const NONE: u32 = 0xffffffffu;
// Relative widening of culling thresholds (f32 evaluation error).
const MARGIN: f32 = 1.00002;
const MAX_CONTOURS: u32 = 64u;

struct Glyph {
    edge_offset: u32,
    edge_count: u32,
    contour_offset: u32,
    contour_count: u32,
    curve_offset: u32,
    cand_offset: u32,
    tile_offset: u32,
    groups: u32,
    tiles_x: u32,
    tiles_y: u32,
    width: u32,
    height: u32,
    field_offset: u32,
    // bits 0-1: channels (0, 1, 3); bit 2: true distance; bit 3: inverse y; bit 4: band skip;
    // bit 5: sign correction; bits 6-7: its fill rule (non-zero, even-odd, positive, negative)
    flags: u32,
    tile_size: u32,
    // sign correction: index into `curves` of height + 1 row starts, then (x, dir) pairs
    scan_offset: u32,
    scale: vec2<f32>,
    translate: vec2<f32>,
    map_scale: f32,
    map_translate: f32,
    // squared distance beyond which tiles are only signed (band skip)
    band2: f32,
    _pad1: f32,
    // Error correction parameters (see ec.wgsl).
    span_h: f32,
    span_v: f32,
    span_d: f32,
    rad_h: f32,
    rad_v: f32,
    rad_d: f32,
    zero: f32,
    improve: f32,
    ec_flags: u32,
    // Destination of the atlas store pass (store.wgsl).
    atlas_x: u32,
    atlas_y: u32,
    atlas_layer: u32,
}

struct Edge {
    kind: u32,
    curve: u32,
    cos_a2: f32,
    cos_b2: f32,
    p0: vec2<f32>,
    p1: vec2<f32>,
    p2: vec2<f32>,
    p3: vec2<f32>,
    bmin: vec2<f32>,
    bmax: vec2<f32>,
    dir0: vec2<f32>,
    dir1: vec2<f32>,
    bis_a: vec2<f32>,
    bis_b: vec2<f32>,
    k: vec4<f32>,
    end: vec2<f32>,
    contour: u32,
    _pad: u32,
}

struct Contour {
    start: u32,
    end: u32,
    winding: i32,
    _pad: u32,
}

struct TileGroup {
    offset: u32,
    count: u32,
    _pad0: u32,
    _pad1: u32,
    ub: vec4<f32>,
}

@group(0) @binding(0) var<storage, read> glyphs: array<Glyph>;
// (glyph index, tile index) per workgroup
@group(0) @binding(1) var<storage, read> work: array<vec2<u32>>;
@group(0) @binding(2) var<storage, read> edges: array<Edge>;
@group(0) @binding(3) var<storage, read> contours: array<Contour>;
@group(0) @binding(4) var<storage, read> curves: array<f32>;
@group(0) @binding(5) var<storage, read> tile_groups: array<TileGroup>;
// (edge index, bitcast key) per candidate
@group(0) @binding(6) var<storage, read> cands: array<vec2<u32>>;
@group(0) @binding(7) var<storage, read_write> field: array<vec4<f32>>;
// Error correction: per texel flags (ec.wgsl) and corner texels (global field indices).
@group(0) @binding(8) var<storage, read_write> stencil: array<atomic<u32>>;
@group(0) @binding(9) var<storage, read> corners: array<u32>;

// ---------------------------------------------------------------------------------------
// Signed distances

struct SD {
    d: f32,
    dot: f32,
}

struct KR {
    d: f32,
    dot: f32,
    param: f32,
}

fn cross2(a: vec2<f32>, b: vec2<f32>) -> f32 {
    return a.x * b.y - a.y * b.x;
}

fn nzs(x: f32) -> f32 {
    return select(-1.0, 1.0, x > 0.0);
}

// Relative tolerance under which two distances count as a tie (decided by `dot`, like the
// CPU's exact comparison): exact ties are common in font geometry, and f32 rounding would
// otherwise break them arbitrarily. Must stay well below MARGIN - 1.
const TIE: f32 = 2e-6;

fn closer(a: SD, b: SD) -> bool {
    let x = abs(a.d);
    let y = abs(b.d);
    return x < y * (1.0 - TIE) || (x <= y * (1.0 + TIE) && a.dot < b.dot);
}

fn median3(a: f32, b: f32, c: f32) -> f32 {
    return max(min(a, b), min(max(a, b), c));
}

// Nearest point at an end point (`unit`: unit tangent there).
fn endpoint_kr(p: vec2<f32>, ep: vec2<f32>, unit: vec2<f32>, at_end: bool, dist2: f32) -> KR {
    let pe = p - ep;
    let dist = sqrt(dist2);
    var dt = 0.0;
    if (dist > 0.0) {
        dt = abs(dot(unit, pe) / dist);
    }
    let proj = dot(pe, unit);
    var param = min(proj, 0.0);
    if (at_end) {
        param = 1.0 + max(proj, 0.0);
    }
    return KR(nzs(cross2(pe, unit)) * dist, dt, param);
}

fn line_kr(e: Edge, p: vec2<f32>) -> KR {
    let a = e.p0;
    let b = e.p1;
    let ab = b - a;
    let aq = p - a;
    let param = dot(aq, ab) * e.k.x;
    let eq = select(a - p, b - p, param > 0.5);
    let c = cross2(aq, ab);
    let ep2 = dot(eq, eq);
    if (param > 0.0 && param < 1.0) {
        let ortho = c * e.k.y;
        if (ortho * ortho < ep2) {
            return KR(ortho, 0.0, param);
        }
    }
    let epd = sqrt(ep2);
    var dt = 0.0;
    if (epd > 0.0 && e.k.y > 0.0) {
        dt = abs(dot(ab, eq) * e.k.y / epd);
    }
    return KR(nzs(c) * epd, dt, param);
}

// Root of the increasing cubic f(t) = ((ca t + cb) t + cc) t + cd in (lo, hi), f(lo) < 0 < f(hi):
// Newton steps inside the shrinking bracket, bisection otherwise.
fn cubic_root(ca: f32, cb: f32, cc: f32, cd: f32, lo0: f32, hi0: f32, flo: f32, fhi: f32) -> f32 {
    var lo = lo0;
    var hi = hi0;
    var t = lo - flo * (hi - lo) / (fhi - flo);
    if (!(t > lo && t < hi)) {
        t = 0.5 * (lo + hi);
    }
    var dxold = hi - lo;
    var dx = dxold;
    for (var i = 0; i < 48; i++) {
        let f = ((ca * t + cb) * t + cc) * t + cd;
        let df = (3.0 * ca * t + 2.0 * cb) * t + cc;
        if (f == 0.0) {
            return t;
        }
        if (f < 0.0) {
            lo = t;
        } else {
            hi = t;
        }
        let nt = t - f / df;
        let ok = df > 0.0 && nt > lo && nt < hi && abs(2.0 * f) <= abs(dxold * df);
        dxold = dx;
        if (ok) {
            dx = t - nt;
            t = nt;
            if (abs(dx) <= 1e-7) {
                return t;
            }
        } else {
            dx = 0.5 * (hi - lo);
            t = lo + dx;
            if (t == lo || t == hi) {
                return t;
            }
        }
    }
    return t;
}

fn quad_kr(e: Edge, p: vec2<f32>) -> KR {
    let p0 = e.p0;
    let p2 = e.p2;
    let ab = e.p1 - p0;
    let br = p2 - e.p1 - ab;
    let qa = p0 - p;
    let ca = e.k.x;
    let cb = e.k.y;
    let cc = e.k.z + dot(qa, br);
    let cd = dot(qa, ab);
    let d0 = dot(qa, qa);
    let e1 = p2 - p;
    let d1 = dot(e1, e1);
    var best = d0;
    var best_t = 0.0;
    var kind = 0u;
    if (d1 < d0) {
        best = d1;
        best_t = 1.0;
        kind = 2u;
    }
    // Critical points of f split [0, 1] into monotone pieces.
    var bp = array<f32, 4>(0.0, 1.0, 1.0, 1.0);
    var n = 1u;
    let qa3 = 3.0 * ca;
    let qb2 = 2.0 * cb;
    var c0 = 2.0;
    var c1 = 2.0;
    if (qa3 != 0.0) {
        let disc = qb2 * qb2 - 4.0 * qa3 * cc;
        if (disc >= 0.0) {
            let s = sqrt(disc);
            let q = -0.5 * (qb2 + select(-s, s, qb2 >= 0.0));
            if (q != 0.0) {
                c0 = q / qa3;
                c1 = cc / q;
            } else {
                c0 = 0.0;
                c1 = 0.0;
            }
            if (c1 < c0) {
                let tmp = c0;
                c0 = c1;
                c1 = tmp;
            }
        }
    } else if (qb2 != 0.0) {
        c0 = -cc / qb2;
    }
    if (c0 > 0.0 && c0 < 1.0) {
        bp[n] = c0;
        n++;
    }
    if (c1 > 0.0 && c1 < 1.0 && c1 != c0) {
        bp[n] = c1;
        n++;
    }
    bp[n] = 1.0;
    n++;
    var flo = cd;
    for (var k = 0u; k + 1u < n; k++) {
        let lo = bp[k];
        let hi = bp[k + 1u];
        var fhi: f32;
        if (k + 2u == n) {
            fhi = ((ca + cb) + cc) + cd;
        } else {
            fhi = ((ca * hi + cb) * hi + cc) * hi + cd;
        }
        if (flo < 0.0 && fhi > 0.0) {
            let t = cubic_root(ca, cb, cc, cd, lo, hi, flo, fhi);
            if (t > 0.0 && t < 1.0) {
                let q = qa + (2.0 * t) * ab + (t * t) * br;
                let d = dot(q, q);
                if (d <= best) {
                    best = d;
                    best_t = t;
                    kind = 1u;
                }
            }
        }
        flo = fhi;
    }
    if (kind == 0u) {
        return endpoint_kr(p, p0, e.dir0, false, best);
    }
    if (kind == 2u) {
        return endpoint_kr(p, p2, e.dir1, true, best);
    }
    let t = best_t;
    let q = qa + (2.0 * t) * ab + (t * t) * br;
    var dir = ab + t * br;
    if (dir.x == 0.0 && dir.y == 0.0) {
        dir = p2 - p0;
    }
    return KR(nzs(cross2(-q, dir)) * sqrt(best), 0.0, t);
}

// --- cubics -----------------------------------------------------------------------------

fn cv(i: u32) -> f32 {
    return curves[i];
}

fn cv2(i: u32) -> vec2<f32> {
    return vec2<f32>(curves[i], curves[i + 1u]);
}

fn poly5(c: array<f32, 6>, t: f32) -> f32 {
    return ((((c[5] * t + c[4]) * t + c[3]) * t + c[2]) * t + c[1]) * t + c[0];
}

fn dpoly5(c: array<f32, 6>, t: f32) -> f32 {
    return (((5.0 * c[5] * t + 4.0 * c[4]) * t + 3.0 * c[3]) * t + 2.0 * c[2]) * t + c[1];
}

fn quintic_root(c: array<f32, 6>, lo0: f32, hi0: f32, flo: f32, fhi: f32, start: f32) -> f32 {
    var lo = lo0;
    var hi = hi0;
    var t = start;
    if (!(t > lo && t < hi)) {
        t = lo - flo * (hi - lo) / (fhi - flo);
    }
    if (!(t > lo && t < hi)) {
        t = 0.5 * (lo + hi);
    }
    var dxold = hi - lo;
    var dx = dxold;
    for (var i = 0; i < 64; i++) {
        let f = poly5(c, t);
        let df = dpoly5(c, t);
        if (f == 0.0) {
            return t;
        }
        if (f < 0.0) {
            lo = t;
        } else {
            hi = t;
        }
        let nt = t - f / df;
        let ok = df > 0.0 && nt > lo && nt < hi && abs(2.0 * f) <= abs(dxold * df);
        dxold = dx;
        if (ok) {
            dx = t - nt;
            t = nt;
            if (abs(dx) <= 1e-7) {
                return t;
            }
        } else {
            dx = 0.5 * (hi - lo);
            t = lo + dx;
            if (t == lo || t == hi) {
                return t;
            }
        }
    }
    return t;
}

// Bernstein coefficients on [a, a + h] of the polynomial `coef` (power basis).
fn bernstein5(coef: array<f32, 6>, a: f32, h: f32) -> array<f32, 6> {
    var t5 = coef;
    for (var k = 0; k < 5; k++) {
        for (var j = 4; j >= k; j--) {
            t5[j] += a * t5[j + 1];
        }
    }
    var dk: array<f32, 6>;
    var hp = 1.0;
    for (var k = 0; k < 6; k++) {
        dk[k] = t5[k] * hp;
        hp *= h;
    }
    var b: array<f32, 6>;
    b[0] = dk[0];
    b[1] = dk[0] + 0.2 * dk[1];
    b[2] = dk[0] + 0.4 * dk[1] + 0.1 * dk[2];
    b[3] = dk[0] + 0.6 * dk[1] + 0.3 * dk[2] + 0.1 * dk[3];
    b[4] = dk[0] + 0.8 * dk[1] + 0.6 * dk[2] + 0.4 * dk[3] + 0.2 * dk[4];
    b[5] = dk[0] + dk[1] + dk[2] + dk[3] + dk[4] + dk[5];
    return b;
}

// Sign changes of the coefficients (zeros skipped); `last` receives the last non-zero one.
struct Changes {
    n: u32,
    last: f32,
}

fn sign_changes(b: array<f32, 6>) -> Changes {
    var n = 0u;
    var prev = 0.0;
    for (var j = 0; j < 6; j++) {
        let v = b[j];
        if (v != 0.0) {
            if (prev != 0.0 && (v > 0.0) != (prev > 0.0)) {
                n++;
            }
            prev = v;
        }
    }
    return Changes(n, prev);
}

struct CubicCtx {
    p0: vec2<f32>,
    a: vec2<f32>,
    b: vec2<f32>,
    c: vec2<f32>,
}

fn ceval(cx: CubicCtx, t: f32) -> vec2<f32> {
    return cx.p0 + t * (cx.a + t * (cx.b + t * cx.c));
}

fn cd1(cx: CubicCtx, t: f32) -> vec2<f32> {
    return cx.a + t * (2.0 * cx.b + (3.0 * t) * cx.c);
}

fn cd2(cx: CubicCtx, t: f32) -> vec2<f32> {
    return 2.0 * cx.b + (6.0 * t) * cx.c;
}

fn piece_far(p: vec2<f32>, pa: vec2<f32>, pb: vec2<f32>, sag: f32, lim: f32) -> bool {
    let ch = pb - pa;
    let ap = p - pa;
    let l2 = dot(ch, ch);
    var u = 0.0;
    if (l2 > 0.0) {
        u = clamp(dot(ap, ch) / l2, 0.0, 1.0);
    }
    let v = ap - ch * u;
    let r = (lim + sag) * 1.0001 + 1e-12;
    return dot(v, v) > r * r;
}

fn cubic_kr(e: Edge, p: vec2<f32>, curve_base: u32) -> KR {
    let base = curve_base + e.curve;
    let n = u32(cv(base));
    let cx = CubicCtx(cv2(base + 1u), cv2(base + 3u), cv2(base + 5u), cv2(base + 7u));
    let k1 = cv(base + 9u);
    let k2 = cv(base + 10u);
    let samples = base + 14u;
    let ivs = samples + 3u * n;
    var best = BIG2;
    var best_t = 0.0;
    for (var i = 0u; i < n; i++) {
        let sp = cv2(samples + 3u * i + 1u);
        let g = dot(sp - p, sp - p);
        if (g < best) {
            best = g;
            best_t = cv(samples + 3u * i);
        }
    }
    let q = cx.p0 - p;
    let coef = array<f32, 6>(dot(q, cx.a), 2.0 * dot(q, cx.b) + k1, 3.0 * dot(q, cx.c) + k2,
        cv(base + 11u), cv(base + 12u), cv(base + 13u));
    var sb = sqrt(best);
    // Subdivision stack (rare: pieces with several sign changes).
    var stack: array<vec2<f32>, 24>;
    for (var i = 0u; i + 1u < n; i++) {
        let iv = ivs + 22u * i;
        let pa = cv2(samples + 3u * i + 1u);
        let ch = cv2(iv);
        let ap = p - pa;
        let u = clamp(dot(ap, ch) * cv(iv + 2u), 0.0, 1.0);
        let v = ap - ch * u;
        let r = (sb + cv(iv + 3u)) * 1.0001 + 1e-12;
        if (dot(v, v) > r * r) {
            continue;
        }
        let a = cv(samples + 3u * i);
        let b = cv(samples + 3u * i + 3u);
        var bern: array<f32, 6>;
        for (var j = 1u; j < 5u; j++) {
            bern[j] = cv(iv + 4u + j) + q.x * cv(iv + 10u + j) + q.y * cv(iv + 16u + j);
        }
        bern[0] = poly5(coef, a);
        bern[5] = poly5(coef, b);
        var sp = 0u;
        var pa_ = a;
        var pb_ = b;
        var first = true;
        loop {
            var lo: f32;
            var hi: f32;
            var bb: array<f32, 6>;
            if (first) {
                lo = pa_;
                hi = pb_;
                bb = bern;
                first = false;
            } else {
                if (sp == 0u) {
                    break;
                }
                sp--;
                lo = stack[sp].x;
                hi = stack[sp].y;
                let qa_ = ceval(cx, lo);
                let qb_ = ceval(cx, hi);
                let dd = max(length(cd2(cx, lo)), length(cd2(cx, hi)));
                if (piece_far(p, qa_, qb_, 0.125 * (hi - lo) * (hi - lo) * dd, sb)) {
                    continue;
                }
                bb = bernstein5(coef, lo, hi - lo);
                bb[0] = poly5(coef, lo);
                bb[5] = poly5(coef, hi);
            }
            let sc = sign_changes(bb);
            if (sc.n == 0u) {
                continue;
            }
            if (sc.n == 1u) {
                // One root inside: a minimum if f goes from negative to positive.
                if (sc.last > 0.0) {
                    let ql = ceval(cx, lo);
                    let qh = ceval(cx, hi);
                    let chd = qh - ql;
                    let l2 = dot(chd, chd);
                    var u2 = 0.5;
                    if (l2 > 0.0) {
                        u2 = clamp(dot(p - ql, chd) / l2, 0.0, 1.0);
                    }
                    let t = quintic_root(coef, lo, hi, bb[0], bb[5], lo + (hi - lo) * u2);
                    let w = ceval(cx, t) - p;
                    let g = dot(w, w);
                    if (g < best) {
                        best = g;
                        best_t = t;
                        sb = sqrt(g);
                    }
                }
                continue;
            }
            // Several sign changes: subdivide.
            let m = 0.5 * (lo + hi);
            if (hi - lo < 1e-6 || sp + 2u > 24u) {
                let w = ceval(cx, m) - p;
                let g = dot(w, w);
                if (g < best) {
                    best = g;
                    best_t = m;
                    sb = sqrt(g);
                }
                continue;
            }
            stack[sp] = vec2<f32>(m, hi);
            stack[sp + 1u] = vec2<f32>(lo, m);
            sp += 2u;
        }
    }
    if (best_t <= 0.0) {
        return endpoint_kr(p, e.p0, e.dir0, false, best);
    }
    if (best_t >= 1.0) {
        return endpoint_kr(p, e.p3, e.dir1, true, best);
    }
    let w = ceval(cx, best_t) - p;
    var dir = cd1(cx, best_t);
    if (dir.x == 0.0 && dir.y == 0.0) {
        dir = cd2(cx, best_t);
        if (dir.x == 0.0 && dir.y == 0.0) {
            dir = e.end - e.p0;
        }
    }
    return KR(nzs(cross2(-w, dir)) * sqrt(best), 0.0, best_t);
}

fn edge_kr(e: Edge, p: vec2<f32>, curve_base: u32) -> KR {
    let kind = e.kind & 0xffu;
    if (kind == 0u) {
        return line_kr(e, p);
    }
    if (kind == 1u) {
        return quad_kr(e, p);
    }
    return cubic_kr(e, p, curve_base);
}

// ---------------------------------------------------------------------------------------
// Selection state of one group (a contour, or the whole shape)

struct Chan {
    sd: SD,
    near: u32,
    param: f32,
    neg: f32,
    pos: f32,
}

var<private> ch: array<Chan, 3>;
var<private> tsd: SD;
var<private> t2: array<f32, 8>;
var<private> ubv: vec4<f32>;
var<private> nc: u32;
var<private> td: bool;

fn chan_empty() -> Chan {
    return Chan(SD(-BIG, 0.0), NONE, 0.0, -BIG, BIG);
}

fn add_perp(c: u32, d: f32) {
    if (d <= 0.0 && d > ch[c].neg) {
        ch[c].neg = d;
    }
    if (d >= 0.0 && d < ch[c].pos) {
        ch[c].pos = d;
    }
}

// Upper bound of channel c's nearest distance over the tile (PSDF's single channel takes
// every edge, so it uses the any-edge bound).
fn chan_ub(c: u32) -> f32 {
    return select(ubv.w, ubv[c], nc == 3u);
}

fn update_thresholds() {
    var b = array<f32, 3>(0.0, 0.0, 0.0);
    for (var c = 0u; c < nc; c++) {
        let d = ch[c].sd.d;
        b[c] = min(d * d, chan_ub(c));
    }
    var base = 0.0;
    if (td) {
        base = min(tsd.d * tsd.d, ubv.w);
    }
    if (nc == 0u) {
        t2[0] = base * MARGIN;
    } else if (nc == 1u) {
        t2[1] = max(base, b[0]) * MARGIN;
    } else {
        let m1 = max(base, b[0]);
        let m2 = max(base, b[1]);
        let m3 = max(m1, b[1]);
        t2[0] = base * MARGIN;
        t2[1] = m1 * MARGIN;
        t2[2] = m2 * MARGIN;
        t2[3] = m3 * MARGIN;
        t2[4] = max(base, b[2]) * MARGIN;
        t2[5] = max(m1, b[2]) * MARGIN;
        t2[6] = max(m2, b[2]) * MARGIN;
        t2[7] = max(m3, b[2]) * MARGIN;
    }
}

fn init_state(ub: vec4<f32>) {
    for (var c = 0u; c < 3u; c++) {
        ch[c] = chan_empty();
    }
    tsd = SD(-BIG, 0.0);
    ubv = ub;
    // Initial thresholds from the tile's upper bounds (as in the CPU binning).
    var base = 0.0;
    if (td || nc == 1u) {
        base = ub.w;
    }
    if (nc == 0u) {
        t2[0] = base * MARGIN;
    } else if (nc == 1u) {
        t2[1] = base * MARGIN;
    } else {
        for (var m = 0u; m < 8u; m++) {
            var t = base;
            for (var c = 0u; c < 3u; c++) {
                if ((m & (1u << c)) != 0u) {
                    t = max(t, ub[c]);
                }
            }
            t2[m] = t * MARGIN;
        }
    }
}

fn perpendiculars(e: Edge, p: vec2<f32>, limit2: f32, mask: u32) {
    let ap = p - e.p0;
    let pa = cross2(ap, e.dir0);
    let va = dot(ap, e.bis_a) > 0.0 && dot(ap, e.dir0) < 0.0 && pa * pa < limit2;
    let bp = p - e.end;
    let pb = cross2(bp, e.dir1);
    let vb = dot(bp, e.bis_b) < 0.0 && dot(bp, e.dir1) > 0.0 && pb * pb < limit2;
    if (!(va || vb)) {
        return;
    }
    for (var c = 0u; c < nc; c++) {
        if ((mask & (1u << c)) != 0u) {
            if (va) {
                add_perp(c, pa);
            }
            if (vb) {
                add_perp(c, pb);
            }
        }
    }
}

fn edge_mask(e: Edge) -> u32 {
    if (nc == 3u) {
        return (e.kind >> 8u) & 7u;
    }
    if (nc == 1u) {
        return 1u;
    }
    return 0u;
}

fn visit(i: u32, p: vec2<f32>, curve_base: u32) {
    let e = edges[i];
    let mask = edge_mask(e);
    let lim = t2[mask];
    let dx = max(max(e.bmin.x - p.x, p.x - e.bmax.x), 0.0);
    let dy = max(max(e.bmin.y - p.y, p.y - e.bmax.y), 0.0);
    let lb2 = dx * dx + dy * dy;
    if (lb2 > lim) {
        if (nc > 0u) {
            let ap = p - e.p0;
            let bp = p - e.end;
            if (dot(ap, ap) * e.cos_a2 < lim || dot(bp, bp) * e.cos_b2 < lim) {
                perpendiculars(e, p, lim, mask);
            }
        }
        return;
    }
    let kr = edge_kr(e, p, curve_base);
    let sd = SD(kr.d, kr.dot);
    var improved = false;
    if (td && closer(sd, tsd)) {
        tsd = sd;
        improved = true;
    }
    for (var c = 0u; c < nc; c++) {
        if ((mask & (1u << c)) != 0u && closer(sd, ch[c].sd)) {
            ch[c].sd = sd;
            ch[c].near = i;
            ch[c].param = kr.param;
            improved = true;
        }
    }
    if (nc > 0u) {
        perpendiculars(e, p, kr.d * kr.d, mask);
    }
    if (improved) {
        update_thresholds();
    }
}

// Pseudo-distance of channel c's nearest edge (BIG if none).
fn near_pseudo(c: u32, p: vec2<f32>) -> f32 {
    if (ch[c].near == NONE) {
        return BIG;
    }
    let e = edges[ch[c].near];
    var d = ch[c].sd.d;
    let param = ch[c].param;
    if (param < 0.0) {
        let ep = p - e.p0;
        if (dot(ep, e.dir0) < 0.0) {
            let pd = cross2(ep, e.dir0);
            if (abs(pd) <= abs(d)) {
                d = pd;
            }
        }
    } else if (param > 1.0) {
        let ep = p - e.end;
        if (dot(ep, e.dir1) > 0.0) {
            let pd = cross2(ep, e.dir1);
            if (abs(pd) <= abs(d)) {
                d = pd;
            }
        }
    }
    return d;
}

fn bucket(c: u32) -> f32 {
    return select(ch[c].pos, ch[c].neg, ch[c].sd.d < 0.0);
}

fn pick(b: f32, np: f32) -> f32 {
    return select(b, np, abs(np) < abs(b));
}

fn scalar_of(d: vec4<f32>) -> f32 {
    if (nc == 3u) {
        return median3(d.x, d.y, d.z);
    }
    if (nc == 1u) {
        return d.x;
    }
    return d.w;
}

// ---------------------------------------------------------------------------------------
// Overlapping contour combiner (msdfgen's semantics, as in msdfr's combine_overlapping)

struct Acc {
    sd: array<SD, 3>,
    ps: vec3<f32>,
    neg: vec3<f32>,
    pos: vec3<f32>,
    tsd: SD,
}

fn acc_empty() -> Acc {
    var a: Acc;
    for (var c = 0; c < 3; c++) {
        a.sd[c] = SD(-BIG, 0.0);
    }
    a.ps = vec3<f32>(BIG);
    a.neg = vec3<f32>(-BIG);
    a.pos = vec3<f32>(BIG);
    a.tsd = SD(-BIG, 0.0);
    return a;
}

fn acc_merge(a: ptr<function, Acc>, pseudo: vec3<f32>) {
    for (var c = 0u; c < nc; c++) {
        if (closer(ch[c].sd, (*a).sd[c])) {
            (*a).sd[c] = ch[c].sd;
            (*a).ps[c] = select(pseudo[c], BIG, ch[c].near == NONE);
        }
        (*a).neg[c] = max((*a).neg[c], ch[c].neg);
        (*a).pos[c] = min((*a).pos[c], ch[c].pos);
    }
    if (closer(tsd, (*a).tsd)) {
        (*a).tsd = tsd;
    }
}

fn acc_resolve(a: Acc) -> vec4<f32> {
    var d = vec4<f32>(0.0);
    for (var c = 0u; c < nc; c++) {
        let b = select(a.pos[c], a.neg[c], a.sd[c].d < 0.0);
        d[c] = pick(b, a.ps[c]);
    }
    d.w = a.tsd.d;
    return d;
}

// ---------------------------------------------------------------------------------------

fn texel_point(g: Glyph, x: u32, r: u32) -> vec2<f32> {
    var ty = f32(g.height - 1u - r) + 0.5;
    if ((g.flags & 8u) != 0u) {
        ty = f32(r) + 0.5;
    }
    return vec2<f32>(f32(x) + 0.5, ty) / g.scale - g.translate;
}

// Scans every edge of group `i` (a contour, or the whole shape with a single group) without
// tile bounds: for points anywhere (error correction's distance checks).
fn scan_all(g: Glyph, tile_base: u32, i: u32, p: vec2<f32>) {
    init_state(vec4<f32>(BIG2));
    var start = 0u;
    var end = g.edge_count;
    if (g.groups > 1u) {
        let c = contours[g.contour_offset + i];
        start = c.start;
        end = c.end;
    }
    for (var k = start; k < end; k++) {
        visit(g.edge_offset + k, p, g.curve_offset);
    }
}

fn scan_tile(g: Glyph, tile_base: u32, i: u32, p: vec2<f32>) {
    let tg = tile_groups[tile_base + i];
    init_state(tg.ub);
    var top = 0u;
    if (nc == 3u) {
        top = 7u;
    } else if (nc == 1u) {
        top = 1u;
    }
    for (var k = 0u; k < tg.count; k++) {
        let cd = cands[g.cand_offset + tg.offset + k];
        if (bitcast<f32>(cd.y) > t2[top]) {
            break;
        }
        visit(g.edge_offset + cd.x, p, g.curve_offset);
    }
}

// ---------------------------------------------------------------------------------------
