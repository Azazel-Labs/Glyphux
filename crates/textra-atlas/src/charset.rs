//! Character sets, in msdf-atlas-gen's charset file syntax.
//!
//! Entries are separated by commas and/or whitespace:
//! * `65`, `0x41` - a Unicode code point,
//! * `'A'` - a character literal,
//! * `"Hello"` - every character of a string literal,
//! * `[0x20, 0x7e]` / `['a', 'z']` - an inclusive range.
//!
//! Literals support the escapes `\\`, `\'`, `\"`, `\n`, `\t`, `\r`, `\0` and `\u{...}`.

use std::collections::BTreeSet;

/// An ordered set of Unicode code points.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Charset(pub BTreeSet<u32>);

/// Charset parse error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CharsetError {
    pub message: String,
    pub position: usize,
}

impl std::fmt::Display for CharsetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "charset: {} (at byte {})", self.message, self.position)
    }
}

impl std::error::Error for CharsetError {}

impl Charset {
    pub fn new() -> Self {
        Charset::default()
    }

    /// Printable ASCII (0x20 - 0x7e), msdf-atlas-gen's default.
    pub fn ascii() -> Self {
        Charset((0x20..=0x7e).collect())
    }

    pub fn add(&mut self, cp: u32) {
        self.0.insert(cp);
    }

    pub fn add_str(&mut self, s: &str) {
        self.0.extend(s.chars().map(|c| c as u32));
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = u32> + '_ {
        self.0.iter().copied()
    }

    /// Parses the charset syntax described in the module documentation.
    pub fn parse(text: &str) -> Result<Charset, CharsetError> {
        let mut p = Parser {
            s: text.as_bytes(),
            text,
            i: 0,
        };
        let mut set = Charset::new();
        loop {
            p.skip_separators();
            if p.i >= p.s.len() {
                break;
            }
            match p.s[p.i] {
                b'[' => {
                    p.i += 1;
                    p.skip_separators();
                    let a = p.single()?;
                    p.skip_separators();
                    let b = p.single()?;
                    p.skip_separators();
                    if p.s.get(p.i) != Some(&b']') {
                        return Err(p.err("expected ']'"));
                    }
                    p.i += 1;
                    if a > b {
                        return Err(p.err("range start is greater than its end"));
                    }
                    // Both ends are valid code points, so the range holds at most 0x110000;
                    // surrogates are not characters.
                    set.0.extend((a..=b).filter(|&c| char::from_u32(c).is_some()));
                }
                b'"' => {
                    for c in p.string(b'"')? {
                        set.add(c);
                    }
                }
                _ => {
                    let c = p.single()?;
                    set.add(c);
                }
            }
        }
        Ok(set)
    }
}

impl FromIterator<char> for Charset {
    fn from_iter<T: IntoIterator<Item = char>>(iter: T) -> Self {
        Charset(iter.into_iter().map(|c| c as u32).collect())
    }
}

struct Parser<'a> {
    s: &'a [u8],
    text: &'a str,
    i: usize,
}

impl Parser<'_> {
    fn err(&self, m: &str) -> CharsetError {
        CharsetError {
            message: m.to_string(),
            position: self.i,
        }
    }

    fn skip_separators(&mut self) {
        while self.i < self.s.len() && (self.s[self.i].is_ascii_whitespace() || self.s[self.i] == b',') {
            self.i += 1;
        }
    }

    /// A number or a character literal.
    fn single(&mut self) -> Result<u32, CharsetError> {
        match self.s.get(self.i) {
            Some(b'\'') => {
                let v = self.string(b'\'')?;
                if v.len() != 1 {
                    return Err(self.err("a character literal must contain exactly one character"));
                }
                Ok(v[0])
            }
            Some(c) if c.is_ascii_digit() => {
                let start = self.i;
                while self.i < self.s.len() && self.s[self.i].is_ascii_alphanumeric() {
                    self.i += 1;
                }
                let tok = &self.text[start..self.i];
                let v = if let Some(h) = tok.strip_prefix("0x").or_else(|| tok.strip_prefix("0X")) {
                    u32::from_str_radix(h, 16)
                } else {
                    tok.parse::<u32>()
                };
                let invalid = |what: &str| CharsetError {
                    message: format!("{what} '{tok}'"),
                    position: start,
                };
                let v = v.map_err(|_| invalid("invalid number"))?;
                if char::from_u32(v).is_none() {
                    return Err(invalid("not a Unicode scalar value:"));
                }
                Ok(v)
            }
            _ => Err(self.err("expected a number, a character literal, a string or a range")),
        }
    }

    fn string(&mut self, quote: u8) -> Result<Vec<u32>, CharsetError> {
        self.i += 1;
        let mut out = Vec::new();
        loop {
            let rest = &self.text[self.i..];
            let Some(c) = rest.chars().next() else {
                return Err(self.err("unterminated literal"));
            };
            self.i += c.len_utf8();
            if c as u32 == quote as u32 {
                return Ok(out);
            }
            if c == '\\' {
                let Some(e) = self.text[self.i..].chars().next() else {
                    return Err(self.err("unterminated escape"));
                };
                self.i += e.len_utf8();
                let v = match e {
                    'n' => '\n' as u32,
                    't' => '\t' as u32,
                    'r' => '\r' as u32,
                    '0' => 0,
                    'u' => {
                        let r = &self.text[self.i..];
                        if !r.starts_with('{') {
                            return Err(self.err("expected \\u{...}"));
                        }
                        let end = r.find('}').ok_or_else(|| self.err("unterminated \\u{...}"))?;
                        let v = u32::from_str_radix(&r[1..end], 16)
                            .ok()
                            .and_then(char::from_u32)
                            .ok_or_else(|| self.err("invalid \\u escape"))?;
                        self.i += end + 1;
                        v as u32
                    }
                    other => other as u32,
                };
                out.push(v);
            } else {
                out.push(c as u32);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_syntax() {
        let c = Charset::parse(r#"[0x20, 0x22], 'A' 66, "xyz\"", ['a','c'] 0x263A '\u{1F600}'"#).unwrap();
        let v: Vec<u32> = c.iter().collect();
        assert_eq!(
            v,
            vec![
                0x20, 0x21, 0x22, 'A' as u32, 66, 'a' as u32, 'b' as u32, 'c' as u32, 'x' as u32, 'y' as u32,
                'z' as u32, 0x263A, 0x1F600
            ]
        );
        assert!(Charset::parse("[5, 1]").is_err());
        assert!(Charset::parse("'ab'").is_err());
        assert!(Charset::parse("\"open").is_err());
        assert_eq!(Charset::ascii().len(), 95);
    }

    #[test]
    fn only_unicode_scalar_values() {
        // Out of range: an error, not billions of entries.
        assert!(Charset::parse("[0, 0xFFFFFFFF]").is_err());
        assert!(Charset::parse("0x110000").is_err());
        assert!(Charset::parse("0xD800").is_err());
        assert!(Charset::parse(r"'\u{D800}'").is_err());
        assert!(Charset::parse(r"'\u{110000}'").is_err());
        // Ranges skip the surrogates.
        let c = Charset::parse("[0xD7FF, 0xE000]").unwrap();
        assert_eq!(c.iter().collect::<Vec<_>>(), vec![0xD7FF, 0xE000]);
    }
}
