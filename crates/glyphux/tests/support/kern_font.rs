// Synthetic SFNT fixture: three glyphs and caller-provided kern data, no outlines.
pub fn font(kern: Vec<u8>) -> Vec<u8> {
    let mut head = vec![0; 54];
    head[18..20].copy_from_slice(&1000u16.to_be_bytes());
    let tables = [
        (*b"head", head),
        (*b"hhea", vec![0; 36]),
        (*b"kern", kern),
        (*b"maxp", vec![0, 0, 0x50, 0, 0, 3]),
    ];
    let mut out = vec![0; 12 + 16 * tables.len()];
    out[..4].copy_from_slice(&0x00010000u32.to_be_bytes());
    out[4..6].copy_from_slice(&(tables.len() as u16).to_be_bytes());
    for (i, (tag, bytes)) in tables.into_iter().enumerate() {
        let offset = out.len() as u32;
        let entry = 12 + 16 * i;
        out[entry..entry + 4].copy_from_slice(&tag);
        out[entry + 8..entry + 12].copy_from_slice(&offset.to_be_bytes());
        out[entry + 12..entry + 16].copy_from_slice(&(bytes.len() as u32).to_be_bytes());
        out.extend(bytes);
        while out.len() % 4 != 0 {
            out.push(0);
        }
    }
    out
}

// Each subtable contains one (glyph 1, glyph 2) pair.
pub fn kern(subtables: &[(u16, i16)]) -> Vec<u8> {
    let mut out = vec![0, 0];
    out.extend((subtables.len() as u16).to_be_bytes());
    for &(coverage, value) in subtables {
        for word in [0, 20, coverage, 1, 6, 0, 0, 1, 2, value as u16] {
            out.extend(word.to_be_bytes());
        }
    }
    out
}
