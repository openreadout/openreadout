//! 8-bit RGB rasters and their PNG and JPEG encodings.

use openreadout_core::{Error, Result};

/// An 8-bit RGB image, row-major, no padding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Raster {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width * height * 3` bytes.
    pub rgb: Vec<u8>,
}

impl Raster {
    /// A raster filled with one colour.
    pub fn new(width: u32, height: u32, fill: [u8; 3]) -> Self {
        let n = width as usize * height as usize;
        let mut rgb = Vec::with_capacity(n * 3);
        for _ in 0..n {
            rgb.extend_from_slice(&fill);
        }
        Raster { width, height, rgb }
    }

    /// The pixel at `(x, y)`, if inside.
    pub fn get(&self, x: u32, y: u32) -> Option<[u8; 3]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = (y as usize * self.width as usize + x as usize) * 3;
        Some([self.rgb[i], self.rgb[i + 1], self.rgb[i + 2]])
    }

    /// Set one pixel; out-of-range coordinates are ignored.
    pub fn set(&mut self, x: i64, y: i64, c: [u8; 3]) {
        if x < 0 || y < 0 || x >= i64::from(self.width) || y >= i64::from(self.height) {
            return;
        }
        let i = (y as usize * self.width as usize + x as usize) * 3;
        self.rgb[i..i + 3].copy_from_slice(&c);
    }

    /// True when every pixel is grey (R = G = B), so the image can be stored as greyscale.
    pub fn is_gray(&self) -> bool {
        self.rgb
            .as_chunks::<3>()
            .0
            .iter()
            .all(|p| p[0] == p[1] && p[1] == p[2])
    }

    /// Halve both dimensions (2×2 box average, rounding half up).
    pub fn half(&self) -> Raster {
        let (w, h) = (
            self.width.div_ceil(2).max(1),
            self.height.div_ceil(2).max(1),
        );
        let mut out = Raster::new(w, h, [0, 0, 0]);
        for y in 0..h {
            for x in 0..w {
                let mut sum = [0u32; 3];
                let mut n = 0u32;
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let (sx, sy) = (x * 2 + dx, y * 2 + dy);
                    if sx < self.width && sy < self.height {
                        let i = (sy as usize * self.width as usize + sx as usize) * 3;
                        for (s, v) in sum.iter_mut().zip(&self.rgb[i..i + 3]) {
                            *s += u32::from(*v);
                        }
                        n += 1;
                    }
                }
                let o = (y as usize * w as usize + x as usize) * 3;
                for (d, s) in out.rgb[o..o + 3].iter_mut().zip(sum) {
                    *d = ((s + n / 2) / n.max(1)) as u8;
                }
            }
        }
        out
    }

    /// PNG bytes: 8-bit greyscale when every pixel is grey, else RGB.
    pub fn to_png(&self) -> Result<Vec<u8>> {
        let mut buf = Vec::new();
        let gray = self.is_gray();
        {
            let mut e = png::Encoder::new(&mut buf, self.width, self.height);
            e.set_color(if gray {
                png::ColorType::Grayscale
            } else {
                png::ColorType::Rgb
            });
            e.set_depth(png::BitDepth::Eight);
            e.set_compression(png::Compression::Balanced);
            let bad = |e: png::EncodingError| Error::Other(format!("PNG encoding failed: {e}"));
            let mut w = e.write_header().map_err(bad)?;
            let data: Vec<u8> = if gray {
                self.rgb.as_chunks::<3>().0.iter().map(|p| p[0]).collect()
            } else {
                self.rgb.clone()
            };
            w.write_image_data(&data).map_err(bad)?;
            w.finish().map_err(bad)?;
        }
        Ok(buf)
    }

    /// JPEG bytes at `quality` (1–100); at most 65535 px on a side.
    pub fn to_jpeg(&self, quality: u8) -> Result<Vec<u8>> {
        if self.width > 65_535 || self.height > 65_535 {
            return Err(Error::Usage("JPEG images are limited to 65535 px".into()));
        }
        Ok(crate::jpeg::encode(self, quality))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_and_halves() {
        let mut r = Raster::new(5, 3, [10, 10, 10]);
        assert!(r.is_gray());
        let png = r.to_png().unwrap();
        assert_eq!(&png[1..4], b"PNG");
        r.set(0, 0, [255, 0, 0]);
        assert!(!r.is_gray());
        assert_eq!(r.get(0, 0), Some([255, 0, 0]));
        assert_eq!(r.get(5, 0), None);
        let h = r.half();
        assert_eq!((h.width, h.height), (3, 2));
        let j = r.to_jpeg(90).unwrap();
        assert_eq!(&j[..2], &[0xFF, 0xD8]);
    }
}
