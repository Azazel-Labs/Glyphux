//! Legacy horizontal kerning. Read coverage ourselves because ttf-parser does not
//! expose the OpenType minimum-distance and override bits. Delegate payload parsing
//! (including class tables) to ttf-parser.

use ttf_parser::kern::{Format, Subtable0, Subtable2, Subtable3};

pub(super) fn subtables(mut data: &[u8]) -> impl Iterator<Item = (Format<'_>, bool)> {
    let apple = data.get(..4) == Some(&[0, 1, 0, 0]);
    let mut count = if apple {
        data.get(4..8).map_or(0, |b| u32::from_be_bytes(b.try_into().unwrap()))
    } else if data.get(..2) == Some(&[0, 0]) {
        data.get(2..4)
            .map_or(0, |b| u32::from(u16::from_be_bytes(b.try_into().unwrap())))
    } else {
        0
    };
    let single = count == 1;
    data = data.get(if apple { 8 } else { 4 }..).unwrap_or(&[]);
    std::iter::from_fn(move || {
        while count > 0 {
            count -= 1;
            let header = if apple { 8 } else { 6 };
            if data.len() < header {
                count = 0;
                return None;
            }
            let (length, format, eligible, replace) = if apple {
                let coverage = data[4];
                (
                    u32::from_be_bytes(data[..4].try_into().unwrap()) as usize,
                    data[5],
                    coverage & 0xe0 == 0,
                    false,
                )
            } else {
                let coverage = data[5];
                (
                    if single {
                        data.len()
                    } else {
                        usize::from(u16::from_be_bytes([data[2], data[3]]))
                    },
                    data[4],
                    coverage & 7 == 1,
                    coverage & 8 != 0,
                )
            };
            if length < header || length > data.len() {
                count = 0;
                return None;
            }
            let payload = &data[header..length];
            data = &data[length..];
            if !eligible {
                continue;
            }
            let parsed = match format {
                0 => Subtable0::parse(payload).map(Format::Format0),
                2 => Subtable2::parse(header as u8, payload).map(Format::Format2),
                3 if apple => Subtable3::parse(payload).map(Format::Format3),
                _ => None,
            };
            if let Some(parsed) = parsed {
                return Some((parsed, replace));
            }
        }
        None
    })
}
