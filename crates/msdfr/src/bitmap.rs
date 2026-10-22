//! Simple owned float bitmaps.

/// A `width` x `height` bitmap with `N` `f32` channels per pixel.
///
/// Rows are stored top-to-bottom (image order): `pixel(x, 0)` is the top-left pixel.
#[derive(Clone, Debug, PartialEq)]
pub struct Bitmap<const N: usize> {
    width: u32,
    height: u32,
    data: Vec<f32>,
}

impl<const N: usize> Bitmap<N> {
    pub fn new(width: u32, height: u32) -> Self {
        Bitmap {
            width,
            height,
            data: vec![0.0; width as usize * height as usize * N],
        }
    }

    /// Wraps existing data (`width * height * N` floats, rows top-to-bottom).
    pub fn from_data(width: u32, height: u32, data: Vec<f32>) -> Self {
        assert_eq!(
            data.len(),
            width as usize * height as usize * N,
            "bitmap data has the wrong size"
        );
        Bitmap { width, height, data }
    }

    #[inline]
    pub fn width(&self) -> u32 {
        self.width
    }
    #[inline]
    pub fn height(&self) -> u32 {
        self.height
    }
    #[inline]
    pub fn channels(&self) -> usize {
        N
    }
    #[inline]
    pub fn data(&self) -> &[f32] {
        &self.data
    }
    #[inline]
    pub fn data_mut(&mut self) -> &mut [f32] {
        &mut self.data
    }
    pub fn into_data(self) -> Vec<f32> {
        self.data
    }

    #[inline]
    fn idx(&self, x: u32, y: u32) -> usize {
        (y as usize * self.width as usize + x as usize) * N
    }

    /// Pixel at column `x`, row `y` (row 0 = top).
    #[inline]
    pub fn pixel(&self, x: u32, y: u32) -> &[f32; N] {
        let i = self.idx(x, y);
        (&self.data[i..i + N]).try_into().unwrap()
    }

    #[inline]
    pub fn pixel_mut(&mut self, x: u32, y: u32) -> &mut [f32; N] {
        let i = self.idx(x, y);
        (&mut self.data[i..i + N]).try_into().unwrap()
    }

    /// Row `y` (top = 0) as a flat slice.
    #[inline]
    pub fn row(&self, y: u32) -> &[f32] {
        let s = y as usize * self.width as usize * N;
        &self.data[s..s + self.width as usize * N]
    }

    /// Converts to 8-bit per channel (clamped, rounded), rows top-to-bottom.
    pub fn to_u8(&self) -> Vec<u8> {
        self.data.iter().map(|&v| float_to_byte(v)).collect()
    }

    /// Quantizes the values to 8 bits and back, simulating storage in an 8-bit image.
    pub fn simulate_8bit(&mut self) {
        for v in &mut self.data {
            *v = float_to_byte(*v) as f32 / 255.0;
        }
    }

    /// Returns a copy flipped vertically.
    pub fn flipped_vertically(&self) -> Self {
        let mut out = Bitmap::new(self.width, self.height);
        let rl = self.width as usize * N;
        for y in 0..self.height as usize {
            let src = &self.data[y * rl..(y + 1) * rl];
            let dy = self.height as usize - 1 - y;
            out.data[dy * rl..(dy + 1) * rl].copy_from_slice(src);
        }
        out
    }

    /// Bilinearly interpolates the bitmap at texel-space position `(x, y)` where `(0, 0)` is the
    /// top-left corner of the top-left texel (texel centers at half-integers). Clamps to edges.
    pub fn sample(&self, x: f64, y: f64) -> [f32; N] {
        let w = self.width as i64;
        let h = self.height as i64;
        let fx = x - 0.5;
        let fy = y - 0.5;
        let x0 = fx.floor();
        let y0 = fy.floor();
        let tx = (fx - x0) as f32;
        let ty = (fy - y0) as f32;
        let x0 = x0 as i64;
        let y0 = y0 as i64;
        let cx = |v: i64| v.clamp(0, w - 1) as u32;
        let cy = |v: i64| v.clamp(0, h - 1) as u32;
        let a = self.pixel(cx(x0), cy(y0));
        let b = self.pixel(cx(x0 + 1), cy(y0));
        let c = self.pixel(cx(x0), cy(y0 + 1));
        let d = self.pixel(cx(x0 + 1), cy(y0 + 1));
        let mut out = [0.0; N];
        for i in 0..N {
            let top = a[i] + (b[i] - a[i]) * tx;
            let bot = c[i] + (d[i] - c[i]) * tx;
            out[i] = top + (bot - top) * ty;
        }
        out
    }
}

/// Converts a `[0, 1]` float to a byte (clamped and rounded; exact halves round down,
/// matching msdfgen so 8-bit outputs are comparable).
#[inline]
pub fn float_to_byte(v: f32) -> u8 {
    let v = (v * 255.0 - 0.5).ceil();
    if v >= 255.0 {
        255
    } else if v >= 0.0 {
        v as u8
    } else {
        0
    }
}
