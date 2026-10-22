//! Minimal dependency-free PNG encoder (8-bit gray / gray+alpha / RGB / RGBA) with its own
//! deflate compressor (hash-chain LZ77 with lazy matching, dynamic Huffman blocks).

/// Encodes 8-bit pixel data (rows top-to-bottom, `channels` bytes per pixel) as PNG.
pub fn encode_png(width: u32, height: u32, channels: usize, pixels: &[u8]) -> Vec<u8> {
    assert!(matches!(channels, 1 | 2 | 3 | 4));
    assert_eq!(pixels.len(), width as usize * height as usize * channels);
    let color_type = match channels {
        1 => 0u8,
        2 => 4,
        3 => 2,
        _ => 6,
    };
    let raw = filter_rows(width as usize * channels, height as usize, channels, pixels);
    let mut out = Vec::with_capacity(raw.len() / 4 + 64);
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, color_type, 0, 0, 0]);
    chunk(&mut out, *b"IHDR", &ihdr);
    chunk(&mut out, *b"IDAT", &zlib_compress(&raw));
    chunk(&mut out, *b"IEND", &[]);
    out
}

/// Applies the PNG filter that minimizes the sum of absolute (signed) residuals to each row
/// and returns the filtered stream (filter byte + residuals per row).
fn filter_rows(stride: usize, height: usize, bpp: usize, pixels: &[u8]) -> Vec<u8> {
    let mut raw = Vec::with_capacity((stride + 1) * height);
    let zero_row = vec![0u8; stride];
    let mut bufs = vec![vec![0u8; stride]; 5];
    for y in 0..height {
        let row = &pixels[y * stride..(y + 1) * stride];
        let prev = if y > 0 {
            &pixels[(y - 1) * stride..y * stride]
        } else {
            &zero_row[..]
        };
        let n = bpp.min(stride);
        // Each filter in its own simple loop (the first `bpp` bytes have no left neighbor).
        let [b0, b1, b2, b3, b4] = &mut bufs[..] else {
            unreachable!()
        };
        b0.copy_from_slice(row);
        for i in 0..n {
            b1[i] = row[i];
            b2[i] = row[i].wrapping_sub(prev[i]);
            b3[i] = row[i].wrapping_sub(prev[i] >> 1);
            b4[i] = row[i].wrapping_sub(prev[i]);
        }
        for i in n..stride {
            let (a, b, c) = (row[i - bpp], prev[i], prev[i - bpp]);
            b1[i] = row[i].wrapping_sub(a);
            b2[i] = row[i].wrapping_sub(b);
            b3[i] = row[i].wrapping_sub(u16::midpoint(a as u16, b as u16) as u8);
            b4[i] = row[i].wrapping_sub(paeth(a, b, c));
        }
        let score = |v: &[u8]| -> u64 { v.iter().map(|&x| (x as i8).unsigned_abs() as u64).sum() };
        let mut best = 0;
        let mut best_score = u64::MAX;
        for (ft, b) in bufs.iter().enumerate() {
            let s = score(b);
            if s < best_score {
                best_score = s;
                best = ft;
            }
        }
        raw.push(best as u8);
        raw.extend_from_slice(&bufs[best]);
    }
    raw
}

#[inline(always)]
fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = a as i16 + b as i16 - c as i16;
    let pa = (p - a as i16).abs();
    let pb = (p - b as i16).abs();
    let pc = (p - c as i16).abs();
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

fn chunk(out: &mut Vec<u8>, kind: [u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(&kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// CRC-32 (slicing by 8).
fn crc32(data: &[u8]) -> u32 {
    static TABLES: std::sync::OnceLock<[[u32; 256]; 8]> = std::sync::OnceLock::new();
    let t = TABLES.get_or_init(|| {
        let mut t = [[0u32; 256]; 8];
        for n in 0..256 {
            let mut c = n as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
            t[0][n] = c;
        }
        for n in 0..256 {
            for k in 1..8 {
                let c = t[k - 1][n];
                t[k][n] = t[0][(c & 0xFF) as usize] ^ (c >> 8);
            }
        }
        t
    });
    let mut c = 0xFFFF_FFFFu32;
    let mut chunks = data.chunks_exact(8);
    for ch in &mut chunks {
        let lo = u32::from_le_bytes([ch[0], ch[1], ch[2], ch[3]]) ^ c;
        let hi = u32::from_le_bytes([ch[4], ch[5], ch[6], ch[7]]);
        c = t[7][(lo & 0xFF) as usize]
            ^ t[6][((lo >> 8) & 0xFF) as usize]
            ^ t[5][((lo >> 16) & 0xFF) as usize]
            ^ t[4][(lo >> 24) as usize]
            ^ t[3][(hi & 0xFF) as usize]
            ^ t[2][((hi >> 8) & 0xFF) as usize]
            ^ t[1][((hi >> 16) & 0xFF) as usize]
            ^ t[0][(hi >> 24) as usize];
    }
    for &b in chunks.remainder() {
        c = t[0][((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    c ^ 0xFFFF_FFFF
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in data.chunks(5552) {
        for &x in chunk {
            a += x as u32;
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    (b << 16) | a
}

struct BitWriter {
    out: Vec<u8>,
    acc: u64,
    n: u32,
}

impl BitWriter {
    #[inline(always)]
    fn put(&mut self, bits: u32, count: u32) {
        self.acc |= (bits as u64) << self.n;
        self.n += count;
        if self.n >= 32 {
            self.out.extend_from_slice(&(self.acc as u32).to_le_bytes());
            self.acc >>= 32;
            self.n -= 32;
        }
    }
    fn finish(mut self) -> Vec<u8> {
        while self.n > 0 {
            self.out.push(self.acc as u8);
            self.acc >>= 8;
            self.n = self.n.saturating_sub(8);
        }
        self.out
    }
}

const LEN_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258,
];
const LEN_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145,
    8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13,
];

/// Length code index for match lengths 0..=258 (entries below 3 unused).
static LEN_CODE: [u8; 259] = {
    let mut t = [0u8; 259];
    let mut code = 0;
    let mut len = 3;
    while len <= 258 {
        while code + 1 < 29 && LEN_BASE[code + 1] as usize <= len {
            code += 1;
        }
        t[len] = code as u8;
        len += 1;
    }
    t
};

#[inline(always)]
fn len_code(len: usize) -> usize {
    LEN_CODE[len] as usize
}

#[inline(always)]
fn dist_code(dist: usize) -> usize {
    if dist <= 4 {
        dist - 1
    } else {
        // Codes come in pairs per power of two.
        let l = usize::BITS - 1 - (dist - 1).leading_zeros(); // floor(log2(dist - 1))
        let code = 2 * l as usize + (((dist - 1) >> (l - 1)) & 1);
        debug_assert!(DIST_BASE[code] as usize <= dist);
        code
    }
}

/// An LZ77 token: a literal byte (`dist == 0`) or a match.
#[derive(Clone, Copy)]
struct Token {
    len_or_lit: u16,
    dist: u16,
}

/// Length-limited Huffman code lengths for `freq` (0 for unused symbols).
fn code_lengths(freq: &[u32], max_len: u32) -> Vec<u8> {
    let mut lens = vec![0u8; freq.len()];
    let mut syms: Vec<(u32, usize)> = freq
        .iter()
        .enumerate()
        .filter(|&(_, &f)| f > 0)
        .map(|(i, &f)| (f, i))
        .collect();
    match syms.len() {
        0 => return lens,
        1 => {
            lens[syms[0].1] = 1;
            return lens;
        }
        _ => {}
    }
    syms.sort_unstable();
    // Huffman tree with two queues (leaves sorted by weight, internal nodes created in
    // non-decreasing weight order).
    let n = syms.len();
    let mut weight: Vec<u64> = syms.iter().map(|s| s.0 as u64).collect();
    let mut parent = vec![usize::MAX; 2 * n - 1];
    let (mut li, mut ni) = (0usize, n);
    weight.reserve(n - 1);
    for next in n..2 * n - 1 {
        let mut pick = |weight: &Vec<u64>| -> usize {
            if li < n && (ni >= next || weight[li] <= weight[ni]) {
                li += 1;
                li - 1
            } else {
                ni += 1;
                ni - 1
            }
        };
        let a = pick(&weight);
        let b = pick(&weight);
        weight.push(weight[a] + weight[b]);
        parent[a] = next;
        parent[b] = next;
    }
    // Depths (root = 2n-2); parents always have larger indices.
    let mut depth = vec![0u32; 2 * n - 1];
    for i in (0..2 * n - 2).rev() {
        depth[i] = depth[parent[i]] + 1;
    }
    // Histogram of leaf depths, then enforce the limit (keeping the Kraft sum at 1).
    let mut count = vec![0u32; 64];
    for &d in &depth[..n] {
        count[d.min(63) as usize] += 1;
    }
    let ml = max_len as usize;
    for d in ml + 1..64 {
        count[ml] += count[d];
        count[d] = 0;
    }
    let mut total: u64 = (1..=ml).map(|d| (count[d] as u64) << (ml - d)).sum();
    while total > 1u64 << ml {
        count[ml] -= 1;
        for d in (1..ml).rev() {
            if count[d] > 0 {
                count[d] -= 1;
                count[d + 1] += 2;
                break;
            }
        }
        total -= 1;
    }
    // Most frequent symbols get the shortest codes.
    let mut d = 1;
    for &(_, sym) in syms.iter().rev() {
        while count[d] == 0 {
            d += 1;
        }
        lens[sym] = d as u8;
        count[d] -= 1;
    }
    lens
}

/// Canonical codes (bit-reversed, ready for LSB-first output) for the given lengths.
fn canonical_codes(lens: &[u8]) -> Vec<u16> {
    let mut bl_count = [0u16; 16];
    for &l in lens {
        bl_count[l as usize] += 1;
    }
    bl_count[0] = 0;
    let mut next = [0u16; 16];
    let mut code = 0u16;
    for bits in 1..16 {
        code = (code + bl_count[bits - 1]) << 1;
        next[bits] = code;
    }
    lens.iter()
        .map(|&l| {
            if l == 0 {
                0
            } else {
                let c = next[l as usize];
                next[l as usize] += 1;
                c.reverse_bits() >> (16 - l)
            }
        })
        .collect()
}

/// Fixed Huffman code lengths (RFC 1951 3.2.6).
fn fixed_lengths() -> (Vec<u8>, Vec<u8>) {
    let mut ll = vec![0u8; 288];
    for (i, l) in ll.iter_mut().enumerate() {
        *l = match i {
            144..=255 => 9,
            256..=279 => 7,
            _ => 8, // 0..=143, 280..=287
        };
    }
    (ll, vec![5u8; 30])
}

/// Run-length encodes code lengths with symbols 16/17/18: (symbol, extra bits value).
fn rle_lengths(lens: &[u8]) -> Vec<(u8, u8)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lens.len() {
        let l = lens[i];
        let mut run = 1;
        while i + run < lens.len() && lens[i + run] == l {
            run += 1;
        }
        let mut r = run;
        if l == 0 {
            while r >= 11 {
                let k = r.min(138);
                out.push((18, (k - 11) as u8));
                r -= k;
            }
            if r >= 3 {
                out.push((17, (r - 3) as u8));
                r = 0;
            }
        } else {
            out.push((l, 0));
            r -= 1;
            while r >= 3 {
                let k = r.min(6);
                out.push((16, (k - 3) as u8));
                r -= k;
            }
        }
        for _ in 0..r {
            out.push((l, 0));
        }
        i += run;
    }
    out
}

/// Writes one deflate block of `tokens` with dynamic or fixed codes, whichever is smaller.
fn write_block(w: &mut BitWriter, tokens: &[Token], last: bool) {
    let mut lf = vec![0u32; 286];
    let mut df = vec![0u32; 30];
    for t in tokens {
        if t.dist == 0 {
            lf[t.len_or_lit as usize] += 1;
        } else {
            lf[257 + len_code(t.len_or_lit as usize)] += 1;
            df[dist_code(t.dist as usize)] += 1;
        }
    }
    lf[256] = 1;
    let mut ll = code_lengths(&lf, 15);
    let mut dl = code_lengths(&df, 15);
    // At least one distance code must be described.
    if dl.iter().all(|&l| l == 0) {
        dl[0] = 1;
    }
    let hlit = 257.max(ll.iter().rposition(|&l| l > 0).unwrap() + 1);
    let hdist = 1.max(dl.iter().rposition(|&l| l > 0).unwrap() + 1);
    let mut all = ll[..hlit].to_vec();
    all.extend_from_slice(&dl[..hdist]);
    let rle = rle_lengths(&all);
    let mut cf = vec![0u32; 19];
    for &(s, _) in &rle {
        cf[s as usize] += 1;
    }
    let cl = code_lengths(&cf, 7);
    const ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];
    let hclen = 4.max(ORDER.iter().rposition(|&s| cl[s] > 0).unwrap() + 1);
    // Compare sizes (in bits) of both encodings, from the symbol histograms.
    let mut extra = 0u64;
    for k in 0..29 {
        extra += lf[257 + k] as u64 * LEN_EXTRA[k] as u64;
    }
    for k in 0..30 {
        extra += df[k] as u64 * DIST_EXTRA[k] as u64;
    }
    let body = |ll: &[u8], dl: &[u8]| -> u64 {
        let mut bits = extra;
        for (f, &l) in lf.iter().zip(ll) {
            bits += *f as u64 * l as u64;
        }
        for (f, &l) in df.iter().zip(dl) {
            bits += *f as u64 * l as u64;
        }
        bits
    };
    let mut header = 14 + 3 * hclen as u64;
    for &(s, _) in &rle {
        header += cl[s as usize] as u64
            + match s {
                16 => 2,
                17 => 3,
                18 => 7,
                _ => 0,
            };
    }
    let (fll, fdl) = fixed_lengths();
    let dynamic = header + body(&ll, &dl) < body(&fll, &fdl);
    w.put(last as u32, 1);
    if dynamic {
        w.put(2, 2);
        w.put((hlit - 257) as u32, 5);
        w.put((hdist - 1) as u32, 5);
        w.put((hclen - 4) as u32, 4);
        for &s in &ORDER[..hclen] {
            w.put(cl[s] as u32, 3);
        }
        let cc = canonical_codes(&cl);
        for &(s, x) in &rle {
            w.put(cc[s as usize] as u32, cl[s as usize] as u32);
            match s {
                16 => w.put(x as u32, 2),
                17 => w.put(x as u32, 3),
                18 => w.put(x as u32, 7),
                _ => {}
            }
        }
    } else {
        w.put(1, 2);
        ll = fll;
        dl = fdl;
    }
    let lc = canonical_codes(&ll);
    let dc = canonical_codes(&dl);
    for t in tokens {
        if t.dist == 0 {
            let s = t.len_or_lit as usize;
            w.put(lc[s] as u32, ll[s] as u32);
        } else {
            let len = t.len_or_lit as usize;
            let li = len_code(len);
            w.put(lc[257 + li] as u32, ll[257 + li] as u32);
            w.put((len - LEN_BASE[li] as usize) as u32, LEN_EXTRA[li] as u32);
            let dist = t.dist as usize;
            let di = dist_code(dist);
            w.put(dc[di] as u32, dl[di] as u32);
            w.put((dist - DIST_BASE[di] as usize) as u32, DIST_EXTRA[di] as u32);
        }
    }
    w.put(lc[256] as u32, ll[256] as u32);
}

/// Length of the common prefix of `data[a..]` and `data[b..]`, up to `max`.
#[inline(always)]
fn match_len(data: &[u8], a: usize, b: usize, max: usize) -> usize {
    let mut l = 0;
    while l + 8 <= max {
        let x = u64::from_le_bytes(data[a + l..a + l + 8].try_into().unwrap());
        let y = u64::from_le_bytes(data[b + l..b + l + 8].try_into().unwrap());
        let d = x ^ y;
        if d != 0 {
            return l + (d.trailing_zeros() / 8) as usize;
        }
        l += 8;
    }
    while l < max && data[a + l] == data[b + l] {
        l += 1;
    }
    l
}

/// Deflate blocks for `data`. Unless `last`, the stream is ended with an empty stored block
/// (a sync flush), so it is byte aligned and further blocks can be appended.
fn deflate_chunk(data: &[u8], last: bool) -> Vec<u8> {
    const WINDOW: usize = 32768;
    const HASH_BITS: u32 = 15;
    const MAX_CHAIN: usize = 16;
    /// Matches at least this long are taken without looking further.
    const NICE: usize = 128;
    const BLOCK_TOKENS: usize = 1 << 16;
    let n = data.len();
    let mut w = BitWriter {
        out: Vec::with_capacity(n / 4 + 64),
        acc: 0,
        n: 0,
    };
    let mut head = vec![u32::MAX; 1 << HASH_BITS];
    let mut prev = vec![u32::MAX; WINDOW];
    let hash = |i: usize| -> usize {
        let v = (data[i] as u32) | (data[i + 1] as u32) << 8 | (data[i + 2] as u32) << 16;
        (v.wrapping_mul(0x9E37_79B1) >> (32 - HASH_BITS)) as usize
    };
    let insert = |i: usize, head: &mut [u32], prev: &mut [u32]| {
        if i + 2 < n {
            let h = hash(i);
            prev[i % WINDOW] = head[h];
            head[h] = i as u32;
        }
    };
    // Longest match for position i (inserted already or not - only earlier positions count).
    let find = |i: usize, head: &[u32], prev: &[u32], min_better: usize| -> (usize, usize) {
        if i + 2 >= n {
            return (0, 0);
        }
        let max_len = (n - i).min(258);
        let mut best_len = min_better;
        let mut best_dist = 0;
        let mut cand = head[hash(i)];
        let mut chain = 0;
        while cand != u32::MAX && chain < MAX_CHAIN {
            let c = cand as usize;
            if c >= i || i - c > WINDOW {
                break;
            }
            // Cheap rejection: the byte that would extend the best match must match.
            if data[c + best_len.min(max_len - 1)] == data[i + best_len.min(max_len - 1)] {
                let l = match_len(data, c, i, max_len);
                if l > best_len {
                    best_len = l;
                    best_dist = i - c;
                    if l >= NICE.min(max_len) {
                        break;
                    }
                }
            }
            let nc = prev[c % WINDOW];
            if nc == u32::MAX || nc as usize >= c {
                break;
            }
            cand = nc;
            chain += 1;
        }
        if best_dist == 0 { (0, 0) } else { (best_len, best_dist) }
    };
    let mut tokens: Vec<Token> = Vec::with_capacity(BLOCK_TOKENS.min(n + 1));
    let mut i = 0;
    let mut pending: Option<(usize, usize)> = None; // match found at i - 1 (lazy evaluation)
    while i < n {
        let (len, dist) = find(i, &head, &prev, 2);
        if let Some((plen, pdist)) = pending.take() {
            if len > plen {
                // The match starting here is better: emit the previous byte as a literal.
                tokens.push(Token {
                    len_or_lit: data[i - 1] as u16,
                    dist: 0,
                });
                pending = Some((len, dist));
                insert(i, &mut head, &mut prev);
                i += 1;
            } else {
                // Take the previous match (it started at i - 1, which is already inserted).
                tokens.push(Token {
                    len_or_lit: plen as u16,
                    dist: pdist as u16,
                });
                for k in i..i - 1 + plen {
                    insert(k, &mut head, &mut prev);
                }
                i += plen - 1;
            }
        } else if len >= 3 {
            if len >= NICE {
                tokens.push(Token {
                    len_or_lit: len as u16,
                    dist: dist as u16,
                });
                for k in i..i + len {
                    insert(k, &mut head, &mut prev);
                }
                i += len;
            } else {
                pending = Some((len, dist));
                insert(i, &mut head, &mut prev);
                i += 1;
            }
        } else {
            tokens.push(Token {
                len_or_lit: data[i] as u16,
                dist: 0,
            });
            insert(i, &mut head, &mut prev);
            i += 1;
        }
        if tokens.len() >= BLOCK_TOKENS {
            write_block(&mut w, &tokens, false);
            tokens.clear();
        }
    }
    if let Some((plen, pdist)) = pending {
        tokens.push(Token {
            len_or_lit: plen as u16,
            dist: pdist as u16,
        });
    }
    write_block(&mut w, &tokens, last);
    if !last {
        w.put(0, 3);
    }
    let mut out = w.finish();
    if !last {
        out.extend_from_slice(&[0, 0, 0xFF, 0xFF]);
    }
    out
}

/// zlib stream of `data`: LZ77 with hash chains and lazy matching, dynamic Huffman blocks.
/// Large inputs are split into chunks compressed in parallel (each starting with an empty
/// window, at a negligible cost in size), as pigz does.
pub fn zlib_compress(data: &[u8]) -> Vec<u8> {
    const MIN_CHUNK: usize = 1 << 20;
    let threads = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    let k = (data.len() / MIN_CHUNK).clamp(1, threads.max(1));
    let mut out = vec![0x78, 0x9C];
    if k == 1 {
        out.extend_from_slice(&deflate_chunk(data, true));
    } else {
        let size = data.len().div_ceil(k);
        let parts: Vec<Vec<u8>> = std::thread::scope(|sc| {
            let hs: Vec<_> = data
                .chunks(size)
                .enumerate()
                .map(|(i, c)| sc.spawn(move || deflate_chunk(c, (i + 1) * size >= data.len())))
                .collect();
            hs.into_iter().map(|h| h.join().unwrap()).collect()
        });
        for p in &parts {
            out.extend_from_slice(p);
        }
    }
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_known() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b"The quick brown fox jumps over the lazy dog"), 0x414F_A339);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }

    #[test]
    fn dist_codes() {
        for d in 1..=32768usize {
            let c = dist_code(d);
            assert!(DIST_BASE[c] as usize <= d);
            assert!(c == 29 || (DIST_BASE[c + 1] as usize) > d, "{d}");
        }
    }

    #[test]
    fn code_lengths_valid() {
        // Skewed frequencies that would exceed the length limit without adjustment.
        let freq: Vec<u32> = (0..40).map(|i| 1u32 << (i % 31)).collect();
        let l = code_lengths(&freq, 15);
        assert!(l.iter().all(|&x| x <= 15 && x > 0));
        let kraft: f64 = l.iter().map(|&x| 0.5f64.powi(x as i32)).sum();
        assert!((kraft - 1.0).abs() < 1e-12, "{kraft}");
    }
}
