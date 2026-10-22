//! Saving bitmaps in various formats.

use std::io::Write;
use std::path::Path;

use crate::bitmap::{Bitmap, float_to_byte};

/// Output file formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageFormat {
    /// 8-bit PNG (gray, RGB or RGBA).
    Png,
    /// 8-bit BMP.
    Bmp,
    /// msdfgen's float format: "FL32", height, width, channels, then f32 rows bottom-to-top.
    Fl32,
    /// msdfgen-compatible "RGBA" format: magic, big-endian width/height, 8-bit RGBA rows.
    Rgba,
    /// Raw 8-bit values, rows top-to-bottom.
    Binary,
    /// Raw little-endian f32 values, rows top-to-bottom.
    BinaryFloat,
    /// 8-bit values as decimal text, one row per line.
    Text,
    /// Float values as text, one row per line.
    TextFloat,
}

impl ImageFormat {
    /// Guesses the format from a file extension.
    pub fn from_extension(path: &Path) -> Option<ImageFormat> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        Some(match ext.as_str() {
            "png" => ImageFormat::Png,
            "bmp" => ImageFormat::Bmp,
            "fl32" => ImageFormat::Fl32,
            "rgba" => ImageFormat::Rgba,
            "bin" => ImageFormat::Binary,
            "txt" => ImageFormat::Text,
            _ => return None,
        })
    }

    pub fn from_name(name: &str) -> Option<ImageFormat> {
        Some(match name {
            "png" => ImageFormat::Png,
            "bmp" => ImageFormat::Bmp,
            "fl32" => ImageFormat::Fl32,
            "rgba" => ImageFormat::Rgba,
            "bin" | "binary" => ImageFormat::Binary,
            "binfloat" | "binaryfloat" | "binfloatle" => ImageFormat::BinaryFloat,
            "text" | "txt" => ImageFormat::Text,
            "textfloat" | "txtfloat" => ImageFormat::TextFloat,
            _ => return None,
        })
    }
}

/// Saves a bitmap in the given format.
pub fn save<const N: usize>(bitmap: &Bitmap<N>, path: impl AsRef<Path>, format: ImageFormat) -> std::io::Result<()> {
    let bytes = encode(bitmap, format);
    std::fs::write(path, bytes)
}

/// Encodes a bitmap in the given format.
pub fn encode<const N: usize>(bitmap: &Bitmap<N>, format: ImageFormat) -> Vec<u8> {
    let (w, h) = (bitmap.width(), bitmap.height());
    match format {
        ImageFormat::Png => crate::io::png::encode_png(w, h, N, &bitmap.to_u8()),
        ImageFormat::Bmp => encode_bmp(bitmap),
        ImageFormat::Fl32 => {
            let mut out = Vec::with_capacity(16 + bitmap.data().len() * 4);
            out.extend_from_slice(b"FL32");
            out.extend_from_slice(&h.to_le_bytes());
            out.extend_from_slice(&w.to_le_bytes());
            out.extend_from_slice(&[N as u8, 0, 0, 0]);
            for y in (0..h).rev() {
                for v in bitmap.row(y) {
                    out.extend_from_slice(&v.to_le_bytes());
                }
            }
            out
        }
        ImageFormat::Rgba => {
            let mut out = Vec::with_capacity(12 + (w * h * 4) as usize);
            out.extend_from_slice(b"RGBA");
            out.extend_from_slice(&w.to_be_bytes());
            out.extend_from_slice(&h.to_be_bytes());
            for y in 0..h {
                for x in 0..w {
                    let p = bitmap.pixel(x, y);
                    let px: [u8; 4] = match N {
                        1 => {
                            let v = float_to_byte(p[0]);
                            [v, v, v, 255]
                        }
                        3 => [float_to_byte(p[0]), float_to_byte(p[1]), float_to_byte(p[2]), 255],
                        _ => [
                            float_to_byte(p[0]),
                            float_to_byte(p[1]),
                            float_to_byte(p[2]),
                            float_to_byte(p[3]),
                        ],
                    };
                    out.extend_from_slice(&px);
                }
            }
            out
        }
        ImageFormat::Binary => bitmap.to_u8(),
        ImageFormat::BinaryFloat => bitmap.data().iter().flat_map(|v| v.to_le_bytes()).collect(),
        ImageFormat::Text | ImageFormat::TextFloat => {
            let mut out = Vec::new();
            for y in 0..h {
                let row = bitmap.row(y);
                for (i, v) in row.iter().enumerate() {
                    if i > 0 {
                        out.push(b' ');
                    }
                    if format == ImageFormat::Text {
                        write!(out, "{}", float_to_byte(*v)).unwrap();
                    } else {
                        write!(out, "{v}").unwrap();
                    }
                }
                out.push(b'\n');
            }
            out
        }
    }
}

pub fn save_png<const N: usize>(bitmap: &Bitmap<N>, path: impl AsRef<Path>) -> std::io::Result<()> {
    save(bitmap, path, ImageFormat::Png)
}

pub fn save_bmp<const N: usize>(bitmap: &Bitmap<N>, path: impl AsRef<Path>) -> std::io::Result<()> {
    save(bitmap, path, ImageFormat::Bmp)
}

pub fn save_fl32<const N: usize>(bitmap: &Bitmap<N>, path: impl AsRef<Path>) -> std::io::Result<()> {
    save(bitmap, path, ImageFormat::Fl32)
}

fn encode_bmp<const N: usize>(bitmap: &Bitmap<N>) -> Vec<u8> {
    let (w, h) = (bitmap.width(), bitmap.height());
    let bpp: u32 = if N == 4 { 4 } else { 3 };
    let row_size = (w * bpp + 3) & !3;
    let data_size = row_size * h;
    let mut out = Vec::with_capacity(54 + data_size as usize);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(54 + data_size).to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(h as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&((bpp * 8) as u16).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&data_size.to_le_bytes());
    out.extend_from_slice(&2835u32.to_le_bytes());
    out.extend_from_slice(&2835u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    for y in (0..h).rev() {
        let start = out.len();
        for x in 0..w {
            let p = bitmap.pixel(x, y);
            match N {
                1 => {
                    let v = float_to_byte(p[0]);
                    out.extend_from_slice(&[v, v, v]);
                }
                3 => out.extend_from_slice(&[float_to_byte(p[2]), float_to_byte(p[1]), float_to_byte(p[0])]),
                _ => out.extend_from_slice(&[
                    float_to_byte(p[2]),
                    float_to_byte(p[1]),
                    float_to_byte(p[0]),
                    float_to_byte(p[3]),
                ]),
            }
        }
        while (out.len() - start) < row_size as usize {
            out.push(0);
        }
    }
    out
}

/// Reads an FL32 file (as written by msdfgen or [`save_fl32`]) into a top-to-bottom bitmap.
/// Returns `(width, height, channels, data)`.
pub fn read_fl32(bytes: &[u8]) -> Option<(u32, u32, usize, Vec<f32>)> {
    if bytes.len() < 16 || &bytes[0..4] != b"FL32" {
        return None;
    }
    let h = u32::from_le_bytes(bytes[4..8].try_into().ok()?);
    let w = u32::from_le_bytes(bytes[8..12].try_into().ok()?);
    let n = bytes[12] as usize;
    let count = w as usize * h as usize * n;
    if bytes.len() < 16 + count * 4 {
        return None;
    }
    let mut data = vec![0f32; count];
    let row = w as usize * n;
    for y in 0..h as usize {
        let src_row = h as usize - 1 - y;
        for i in 0..row {
            let o = 16 + (src_row * row + i) * 4;
            data[y * row + i] = f32::from_le_bytes(bytes[o..o + 4].try_into().ok()?);
        }
    }
    Some((w, h, n, data))
}
