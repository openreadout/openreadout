//! A tiny RGB raster: rectangles, lines and a 3×5 bitmap font for labels. Integer arithmetic
//! only, so every drawing is bit-for-bit reproducible.

use crate::color::Rgb;

/// An 8-bit RGB image, row-major, no padding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Canvas {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width * height * 3` bytes.
    pub rgb: Vec<u8>,
}

impl Canvas {
    /// A canvas filled with one colour.
    pub fn new(width: u32, height: u32, fill: Rgb) -> Self {
        let n = width as usize * height as usize;
        let mut rgb = Vec::with_capacity(n * 3);
        for _ in 0..n {
            rgb.extend_from_slice(&fill);
        }
        Canvas { width, height, rgb }
    }

    /// Set one pixel; out-of-range coordinates are ignored.
    pub fn set(&mut self, x: i64, y: i64, c: Rgb) {
        if x < 0 || y < 0 || x >= i64::from(self.width) || y >= i64::from(self.height) {
            return;
        }
        let i = (y as usize * self.width as usize + x as usize) * 3;
        self.rgb[i..i + 3].copy_from_slice(&c);
    }

    /// Fill a rectangle; the parts outside the canvas are ignored.
    pub fn fill_rect(&mut self, x: i64, y: i64, w: i64, h: i64, c: Rgb) {
        for yy in y.max(0)..(y + h).min(i64::from(self.height)) {
            for xx in x.max(0)..(x + w).min(i64::from(self.width)) {
                self.set(xx, yy, c);
            }
        }
    }

    /// Bresenham line, both end points included.
    pub fn line(&mut self, x0: i64, y0: i64, x1: i64, y1: i64, c: Rgb) {
        let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
        let (sx, sy) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
        let (mut x, mut y, mut err) = (x0, y0, dx + dy);
        loop {
            self.set(x, y, c);
            if x == x1 && y == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                err += dx;
                y += sy;
            }
        }
    }

    /// Draw `text` (ASCII; lowercase is shown as uppercase, unknown characters as blanks) with
    /// its top-left corner at `(x, y)`, each font pixel `scale`×`scale`. Returns the width drawn.
    pub fn text(&mut self, x: i64, y: i64, scale: i64, c: Rgb, text: &str) -> i64 {
        self.draw_glyphs(x, y, scale, c, text, |ch| (glyph(ch), 3))
    }

    /// Like [`Canvas::text`], but the lowercase letters that have a glyph of their own (`c`, `m`,
    /// `n`: enough for `µm`, `nm`, `mm`, `cm`) keep their case, so units read as units. The
    /// width drawn is [`text_cased_width`].
    pub fn text_cased(&mut self, x: i64, y: i64, scale: i64, c: Rgb, text: &str) -> i64 {
        self.draw_glyphs(x, y, scale, c, text, cased_glyph)
    }

    fn draw_glyphs(
        &mut self,
        x: i64,
        y: i64,
        scale: i64,
        c: Rgb,
        text: &str,
        glyph_of: impl Fn(char) -> ([u8; 5], i64),
    ) -> i64 {
        let mut cx = x;
        for ch in text.chars() {
            let (g, w) = glyph_of(ch);
            for (row, bits) in g.iter().enumerate() {
                for col in 0..w {
                    if bits & (1 << (w - 1 - col)) != 0 {
                        self.fill_rect(cx + col * scale, y + row as i64 * scale, scale, scale, c);
                    }
                }
            }
            cx += (w + 1) * scale;
        }
        cx - x
    }

    /// True when every pixel is grey (R = G = B), so the image can be stored as greyscale.
    pub fn is_gray(&self) -> bool {
        self.rgb
            .as_chunks::<3>()
            .0
            .iter()
            .all(|p| p[0] == p[1] && p[1] == p[2])
    }

    /// Halve both dimensions (2×2 box average, rounding half up), for fitting a byte budget.
    pub fn half(&self) -> Canvas {
        let (w, h) = (
            self.width.div_ceil(2).max(1),
            self.height.div_ceil(2).max(1),
        );
        let mut out = Canvas::new(w, h, [0, 0, 0]);
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
                    *d = ((s + n / 2) / n) as u8;
                }
            }
        }
        out
    }
}

/// Width in pixels of `text` drawn at `scale`.
pub fn text_width(text: &str, scale: i64) -> i64 {
    text.chars().count() as i64 * 4 * scale
}

/// Width in pixels of `text` drawn by [`Canvas::text_cased`] at `scale`.
pub fn text_cased_width(text: &str, scale: i64) -> i64 {
    text.chars().map(|ch| (cased_glyph(ch).1 + 1) * scale).sum()
}

/// The glyph and its width for [`Canvas::text_cased`]: lowercase `c`, `m` (5 wide) and `n`,
/// else the 3-wide uppercase glyph.
fn cased_glyph(c: char) -> ([u8; 5], i64) {
    match c {
        'c' => ([0, 3, 4, 4, 3], 3),
        'm' => ([0, 0b11110, 0b10101, 0b10101, 0b10101], 5),
        'n' => ([0, 6, 5, 5, 5], 3),
        _ => (glyph(c), 3),
    }
}

/// 3×5 glyphs: five rows, three bits each (bit 2 = left column).
fn glyph(c: char) -> [u8; 5] {
    match c.to_ascii_uppercase() {
        '0' => [7, 5, 5, 5, 7],
        '1' => [2, 6, 2, 2, 7],
        '2' => [7, 1, 7, 4, 7],
        '3' => [7, 1, 7, 1, 7],
        '4' => [5, 5, 7, 1, 1],
        '5' => [7, 4, 7, 1, 7],
        '6' => [7, 4, 7, 5, 7],
        '7' => [7, 1, 1, 2, 2],
        '8' => [7, 5, 7, 5, 7],
        '9' => [7, 5, 7, 1, 7],
        'A' => [2, 5, 7, 5, 5],
        'B' => [6, 5, 6, 5, 6],
        'C' => [3, 4, 4, 4, 3],
        'D' => [6, 5, 5, 5, 6],
        'E' => [7, 4, 6, 4, 7],
        'F' => [7, 4, 6, 4, 4],
        'G' => [3, 4, 5, 5, 3],
        'H' => [5, 5, 7, 5, 5],
        'I' => [7, 2, 2, 2, 7],
        'J' => [1, 1, 1, 5, 2],
        'K' => [5, 5, 6, 5, 5],
        'L' => [4, 4, 4, 4, 7],
        'M' => [5, 7, 7, 5, 5],
        'N' => [6, 5, 5, 5, 5],
        'O' => [2, 5, 5, 5, 2],
        'P' => [6, 5, 6, 4, 4],
        'Q' => [2, 5, 5, 6, 3],
        'R' => [6, 5, 6, 5, 5],
        'S' => [3, 4, 2, 1, 6],
        'T' => [7, 2, 2, 2, 2],
        'U' => [5, 5, 5, 5, 7],
        'V' => [5, 5, 5, 5, 2],
        'W' => [5, 5, 7, 7, 5],
        'X' => [5, 5, 2, 5, 5],
        'Y' => [5, 5, 2, 2, 2],
        'Z' => [7, 1, 2, 4, 7],
        '-' => [0, 0, 7, 0, 0],
        '.' => [0, 0, 0, 0, 2],
        ':' => [0, 2, 0, 2, 0],
        '/' => [1, 1, 2, 4, 4],
        '_' => [0, 0, 0, 0, 7],
        '(' => [1, 2, 2, 2, 1],
        ')' => [4, 2, 2, 2, 4],
        '+' => [0, 2, 7, 2, 0],
        ',' => [0, 0, 0, 2, 4],
        // micro sign and Greek mu
        '\u{b5}' | '\u{3bc}' => [0, 5, 5, 7, 4],
        _ => [0; 5],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_endpoints_and_clipping() {
        let mut c = Canvas::new(4, 4, [0, 0, 0]);
        c.line(-2, 0, 3, 3, [255, 255, 255]);
        assert_eq!(
            &c.rgb[(3 * 4 + 3) * 3..(3 * 4 + 3) * 3 + 3],
            &[255, 255, 255]
        );
        assert!(c.is_gray());
        c.set(0, 0, [1, 2, 3]);
        assert!(!c.is_gray());
    }

    #[test]
    fn text_and_half() {
        let mut c = Canvas::new(20, 7, [0, 0, 0]);
        assert_eq!(c.text(0, 0, 1, [9, 9, 9], "A1"), 8);
        assert_eq!(text_width("A1", 2), 16);
        assert_eq!(
            c.text_cased(0, 0, 1, [9, 9, 9], "5 µm"),
            text_cased_width("5 µm", 1)
        );
        assert_eq!(text_cased_width("5 µm", 1), 4 + 4 + 4 + 6);
        let h = c.half();
        assert_eq!((h.width, h.height), (10, 4));
    }
}
