//! Decoding the stored images of slide formats that keep one JPEG, PNG or BMP file per tile:
//! always to 8-bit interleaved R, G, B.

use openreadout_core::bytes::{be_u16, be_u32, le_i32, le_u16, le_u32};
use openreadout_core::{Error, Result};

/// The container of one stored tile image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageCodec {
    /// JPEG (JFIF) stream.
    Jpeg,
    /// PNG file.
    Png,
    /// Windows bitmap (`BM`), uncompressed.
    Bmp,
}

impl ImageCodec {
    /// Lower-case name for `info` and assurance (`jpeg`, `png`, `bmp`).
    pub fn name(self) -> &'static str {
        match self {
            ImageCodec::Jpeg => "jpeg",
            ImageCodec::Png => "png",
            ImageCodec::Bmp => "bmp",
        }
    }

    /// Recognise a stored image by its first bytes.
    pub fn sniff(data: &[u8]) -> Option<ImageCodec> {
        if data.starts_with(&[0xFF, 0xD8]) {
            Some(ImageCodec::Jpeg)
        } else if data.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some(ImageCodec::Png)
        } else if data.starts_with(b"BM") {
            Some(ImageCodec::Bmp)
        } else {
            None
        }
    }
}

/// A decoded tile: `width × height` pixels of interleaved 8-bit R, G, B.
#[derive(Debug, Clone, PartialEq)]
pub struct RgbImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width · height · 3` samples.
    pub data: Vec<u8>,
}

/// Decode a stored tile of at most `max_pixels` pixels to 8-bit RGB; grey images are
/// replicated to three samples, an alpha channel is dropped.
pub fn decode_rgb(format: &'static str, data: &[u8], max_pixels: u64) -> Result<RgbImage> {
    let max_bytes = usize::try_from(max_pixels.saturating_mul(4)).unwrap_or(usize::MAX);
    match ImageCodec::sniff(data) {
        Some(ImageCodec::Jpeg) => {
            let r = openreadout_codecs::jpeg_decode_limited(data, max_bytes)
                .map_err(|e| Error::corrupt(format, format!("JPEG tile: {e}")))?;
            if r.bits_per_sample != 8 || r.float {
                return Err(Error::unsupported(
                    format,
                    format!("{}-bit JPEG tile", r.bits_per_sample),
                    "Only 8-bit JPEG tiles are known in slide files.",
                ));
            }
            to_rgb(format, r.width, r.height, r.channels as usize, r.data)
        }
        Some(ImageCodec::Png) => decode_png(format, data, max_bytes),
        Some(ImageCodec::Bmp) => decode_bmp(format, data, max_pixels),
        None => Err(Error::corrupt(
            format,
            "a tile is neither JPEG, PNG nor BMP (unknown leading bytes)",
        )),
    }
}

fn to_rgb(
    format: &'static str,
    width: u32,
    height: u32,
    channels: usize,
    data: Vec<u8>,
) -> Result<RgbImage> {
    let n = width as usize * height as usize;
    if data.len() < n.saturating_mul(channels) {
        return Err(Error::corrupt(
            format,
            "decoded tile is shorter than its size",
        ));
    }
    let data = match channels {
        3 => {
            let mut d = data;
            d.truncate(n * 3);
            d
        }
        1 => data[..n].iter().flat_map(|&g| [g, g, g]).collect(),
        2 => data[..n * 2]
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[0], p[0]])
            .collect(),
        4 => data[..n * 4]
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2]])
            .collect(),
        c => {
            return Err(Error::unsupported(
                format,
                format!("tile with {c} colour components"),
                "Only grey, grey + alpha, RGB and RGBA tiles are known.",
            ));
        }
    };
    Ok(RgbImage {
        width,
        height,
        data,
    })
}

fn decode_png(format: &'static str, data: &[u8], max_bytes: usize) -> Result<RgbImage> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(data));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec
        .read_info()
        .map_err(|e| Error::corrupt(format, format!("PNG tile: {e}")))?;
    let (w, h) = {
        let i = reader.info();
        (i.width, i.height)
    };
    if (w as usize).saturating_mul(h as usize).saturating_mul(4) > max_bytes {
        return Err(Error::corrupt(
            format,
            format!("PNG tile of {w} x {h} pixels is larger than any tile of the slide"),
        ));
    }
    let Some(size) = reader.output_buffer_size() else {
        return Err(Error::corrupt(format, "PNG tile size overflows"));
    };
    let mut buf = vec![0u8; size];
    let frame = reader
        .next_frame(&mut buf)
        .map_err(|e| Error::corrupt(format, format!("PNG tile: {e}")))?;
    buf.truncate(frame.buffer_size());
    let channels = frame.color_type.samples();
    to_rgb(format, frame.width, frame.height, channels, buf)
}

/// Uncompressed Windows bitmaps (`BI_RGB`): 24-bit B, G, R and 32-bit B, G, R, X rows, bottom-up
/// (positive height) or top-down (negative height), rows padded to 4 bytes; 8-bit with a
/// palette.
pub fn decode_bmp(format: &'static str, data: &[u8], max_pixels: u64) -> Result<RgbImage> {
    let bad = |w: &str| Error::corrupt(format, format!("BMP tile: {w}"));
    let pixel_offset = le_u32(data, 10).ok_or_else(|| bad("header truncated"))? as usize;
    let header_size = le_u32(data, 14).ok_or_else(|| bad("header truncated"))? as usize;
    if header_size < 40 {
        return Err(Error::unsupported(
            format,
            format!("BMP header of {header_size} bytes"),
            "Only BITMAPINFOHEADER (40 bytes) or later bitmap headers are known.",
        ));
    }
    let width = le_u32(data, 18).ok_or_else(|| bad("header truncated"))? as i32;
    let height = le_u32(data, 22).ok_or_else(|| bad("header truncated"))? as i32;
    let bpp = le_u16(data, 28).ok_or_else(|| bad("header truncated"))?;
    let compression = le_u32(data, 30).ok_or_else(|| bad("header truncated"))?;
    if compression != 0 {
        return Err(Error::unsupported(
            format,
            format!("BMP compression {compression}"),
            "Only uncompressed (BI_RGB) bitmaps are known.",
        ));
    }
    if width <= 0 || height == 0 {
        return Err(bad("empty or negative width"));
    }
    let w = width.unsigned_abs();
    let h = height.unsigned_abs();
    if u64::from(w) * u64::from(h) > max_pixels {
        return Err(bad("larger than any tile of the slide"));
    }
    let bottom_up = height > 0;
    let (row_bytes, palette): (usize, Option<Vec<[u8; 3]>>) = match bpp {
        24 | 32 => (((w as usize * usize::from(bpp) / 8) + 3) & !3, None),
        8 => {
            let colours = match le_u32(data, 46).ok_or_else(|| bad("header truncated"))? {
                0 => 256,
                n => n.min(256) as usize,
            };
            let pal_at = 14 + header_size;
            let pal: Vec<[u8; 3]> = (0..colours)
                .map(|i| {
                    data.get(pal_at + 4 * i..pal_at + 4 * i + 3)
                        .map(|p| [p[2], p[1], p[0]])
                        .ok_or_else(|| bad("palette truncated"))
                })
                .collect::<Result<_>>()?;
            ((w as usize + 3) & !3, Some(pal))
        }
        b => {
            return Err(Error::unsupported(
                format,
                format!("{b}-bit BMP tile"),
                "Only 8-, 24- and 32-bit bitmaps are known.",
            ));
        }
    };
    let need = row_bytes
        .checked_mul(h as usize)
        .and_then(|n| n.checked_add(pixel_offset))
        .ok_or_else(|| bad("size overflows"))?;
    if data.len() < need {
        return Err(bad("pixel data truncated"));
    }
    let mut out = Vec::with_capacity(w as usize * h as usize * 3);
    for y in 0..h as usize {
        let src_row = if bottom_up { h as usize - 1 - y } else { y };
        let row = &data[pixel_offset + src_row * row_bytes..][..row_bytes];
        match (&palette, bpp) {
            (Some(p), _) => {
                for &i in &row[..w as usize] {
                    out.extend_from_slice(p.get(usize::from(i)).unwrap_or(&[0, 0, 0]));
                }
            }
            (None, 24) => {
                for px in row[..w as usize * 3].as_chunks::<3>().0 {
                    out.extend_from_slice(&[px[2], px[1], px[0]]);
                }
            }
            _ => {
                for px in row[..w as usize * 4].as_chunks::<4>().0 {
                    out.extend_from_slice(&[px[2], px[1], px[0]]);
                }
            }
        }
    }
    Ok(RgbImage {
        width: w,
        height: h,
        data: out,
    })
}

/// Width and height of a stored image from its header only (JPEG frame header, PNG `IHDR`,
/// BMP info header), without decoding.
pub fn header_size(data: &[u8]) -> Option<(u32, u32)> {
    match ImageCodec::sniff(data)? {
        ImageCodec::Jpeg => {
            let mut i = 2usize;
            while i + 4 <= data.len() {
                if data[i] != 0xFF {
                    return None;
                }
                let m = data[i + 1];
                if m == 0xFF {
                    i += 1;
                    continue;
                }
                let len = usize::from(be_u16(data, i + 2)?);
                if matches!(m, 0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF) {
                    let h = be_u16(data, i + 5)?;
                    let w = be_u16(data, i + 7)?;
                    return Some((u32::from(w), u32::from(h)));
                }
                if m == 0xDA {
                    return None;
                }
                i = i.checked_add(2 + len)?;
            }
            None
        }
        ImageCodec::Png => Some((be_u32(data, 16)?, be_u32(data, 20)?)),
        ImageCodec::Bmp => {
            let w = le_i32(data, 18)?;
            let h = le_i32(data, 22)?;
            Some((w.unsigned_abs(), h.unsigned_abs()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::many_single_char_names)]
    fn bmp24(w: u32, h: i32, px: &[[u8; 3]]) -> Vec<u8> {
        let row = ((w as usize * 3) + 3) & !3;
        let mut d = b"BM".to_vec();
        let size = 54 + row * h.unsigned_abs() as usize;
        d.extend_from_slice(&(size as u32).to_le_bytes());
        d.extend_from_slice(&[0; 4]);
        d.extend_from_slice(&54u32.to_le_bytes());
        d.extend_from_slice(&40u32.to_le_bytes());
        d.extend_from_slice(&(w as i32).to_le_bytes());
        d.extend_from_slice(&h.to_le_bytes());
        d.extend_from_slice(&1u16.to_le_bytes());
        d.extend_from_slice(&24u16.to_le_bytes());
        d.extend_from_slice(&[0; 24]);
        // rows as stored (bottom-up when h > 0): px are given top row first
        let hh = h.unsigned_abs() as usize;
        for y in 0..hh {
            let src = if h > 0 { hh - 1 - y } else { y };
            let mut r = Vec::new();
            for x in 0..w as usize {
                let p = px[src * w as usize + x];
                r.extend_from_slice(&[p[2], p[1], p[0]]);
            }
            r.resize(row, 0);
            d.extend_from_slice(&r);
        }
        d
    }

    #[test]
    fn bmp_orientation_and_padding() {
        let px = [[1, 2, 3], [4, 5, 6], [7, 8, 9], [10, 11, 12]];
        for h in [2, -2] {
            let img = decode_bmp("t", &bmp24(2, h, &px), 100).unwrap();
            assert_eq!((img.width, img.height), (2, 2));
            assert_eq!(img.data, vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
            assert_eq!(header_size(&bmp24(2, h, &px)), Some((2, 2)));
        }
        let mut short = bmp24(2, 2, &px);
        short.truncate(58);
        assert!(decode_bmp("t", &short, 100).is_err());
        assert!(decode_bmp("t", &bmp24(2, 2, &px), 3).is_err());
    }

    #[test]
    fn sniffing() {
        assert_eq!(
            ImageCodec::sniff(&[0xFF, 0xD8, 0xFF]),
            Some(ImageCodec::Jpeg)
        );
        assert_eq!(ImageCodec::sniff(b"BMxx"), Some(ImageCodec::Bmp));
        assert!(ImageCodec::sniff(b"GIF8").is_none());
        assert!(decode_rgb("t", b"junk", 10).is_err());
        assert_eq!(
            header_size(&[0xFF, 0xD8, 0xFF, 0xC0, 0, 17, 8, 0, 2, 0, 3]),
            Some((3, 2))
        );
    }
}
