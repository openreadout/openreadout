//! The resolution protocol: bring a decoded bitmap to the geometry and pixel type the
//! directory entry declares. The directory is authoritative (libCZI documentation page
//! `resolution_protocol`); sample conversions follow czifile's pixel-type conversion table
//! (BSD-3 prior art) and were checked black-box against pylibCZIrw on synthetic fixtures.
//! See `docs/formats/czi.md` and `docs/provenance/czi.md`.

use openreadout_codecs::Raster;
use openreadout_core::{Error, Result};

use crate::FORMAT_ID;
use crate::container::PixelTypeId;

/// Sample encodings we convert between.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    U8,
    U16,
    I32,
    F32,
    F64,
}

impl Kind {
    fn bytes(self) -> usize {
        match self {
            Kind::U8 => 1,
            Kind::U16 => 2,
            Kind::I32 | Kind::F32 => 4,
            Kind::F64 => 8,
        }
    }
    fn is_float(self) -> bool {
        matches!(self, Kind::F32 | Kind::F64)
    }
}

/// What the directory's pixel type means for the samples we return: kind and channel count
/// (RGB types return 3 samples, alpha dropped).
fn declared(pt: PixelTypeId) -> Result<(Kind, usize)> {
    Ok(match pt {
        PixelTypeId::Gray8 => (Kind::U8, 1),
        PixelTypeId::Gray16 => (Kind::U16, 1),
        PixelTypeId::Gray32Float => (Kind::F32, 1),
        PixelTypeId::Bgr24 | PixelTypeId::Bgra32 => (Kind::U8, 3),
        PixelTypeId::Bgr48 => (Kind::U16, 3),
        PixelTypeId::Bgr96Float => (Kind::F32, 3),
        PixelTypeId::Gray32 => (Kind::I32, 1),
        PixelTypeId::Gray64 => (Kind::F64, 1),
        other => {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!("pixel type {}", other.name()),
                "Complex-valued and unknown pixel types are not decoded.",
            ));
        }
    })
}

fn raster_kind(r: &Raster) -> Result<Kind> {
    Ok(match (r.bits_per_sample, r.float) {
        (8, false) => Kind::U8,
        (16, false) => Kind::U16,
        (32, true) => Kind::F32,
        (32, false) => Kind::I32,
        (64, true) => Kind::F64,
        (b, f) => {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!(
                    "decoded {b}-bit {} samples",
                    if f { "floating-point" } else { "integer" }
                ),
                "This compressed subblock decodes to a sample format the reader cannot convert.",
            ));
        }
    })
}

fn get(b: &[u8], k: Kind, i: usize) -> f64 {
    let o = i * k.bytes();
    match k {
        Kind::U8 => f64::from(b[o]),
        Kind::U16 => f64::from(u16::from_le_bytes([b[o], b[o + 1]])),
        Kind::I32 => f64::from(i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])),
        Kind::F32 => f64::from(f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])),
        Kind::F64 => f64::from_le_bytes([
            b[o],
            b[o + 1],
            b[o + 2],
            b[o + 3],
            b[o + 4],
            b[o + 5],
            b[o + 6],
            b[o + 7],
        ]),
    }
}

fn put(out: &mut Vec<u8>, k: Kind, v: f64) {
    let v = if v.is_nan() { 0.0 } else { v };
    match k {
        Kind::U8 => out.push(v.clamp(0.0, 255.0) as u8),
        Kind::U16 => out.extend_from_slice(&(v.clamp(0.0, 65535.0) as u16).to_le_bytes()),
        Kind::I32 => out
            .extend_from_slice(&(v.clamp(-2_147_483_648.0, 2_147_483_647.0) as i32).to_le_bytes()),
        Kind::F32 => out.extend_from_slice(&(v as f32).to_le_bytes()),
        Kind::F64 => out.extend_from_slice(&v.to_le_bytes()),
    }
}

/// Convert one sample between integer depths the way both oracles agree: 16 → 8 bit keeps
/// the high byte, 8 → 16 bit zero-extends. Floats clip to the integer range (truncating).
fn convert_sample(v: f64, from: Kind, to: Kind) -> f64 {
    match (from, to) {
        (Kind::U16, Kind::U8) => (v as u32 >> 8) as f64,
        _ if from.is_float() && !to.is_float() => v.trunc(),
        _ => v,
    }
}

/// Result of [`conform`]: the samples, and whether anything had to be changed.
#[derive(Debug)]
pub struct Conformed {
    pub data: Vec<u8>,
    /// Human-readable list of what was adjusted (empty when the bitmap already matched).
    pub adjustments: Vec<String>,
}

/// Bring a decoded raster (R,G,B or gray sample order) to `want`'s sample kind and channel
/// count and to `width` × `height`, cropping or zero-padding at the top-left origin.
pub fn conform(raster: Raster, want: PixelTypeId, width: u32, height: u32) -> Result<Conformed> {
    let (to, to_ch) = declared(want)?;
    let from = raster_kind(&raster)?;
    let from_ch = raster.channels as usize;
    if !matches!(from_ch, 1 | 3 | 4) {
        return Err(Error::unsupported(
            FORMAT_ID,
            format!("decoded bitmap with {from_ch} samples per pixel"),
            "Only gray, RGB and RGBA bitmaps can be converted to the declared pixel type.",
        ));
    }
    let (rw, rh) = (raster.width as usize, raster.height as usize);
    let need = rw
        .checked_mul(rh)
        .and_then(|n| n.checked_mul(from_ch * from.bytes()))
        .ok_or_else(|| Error::corrupt(FORMAT_ID, "decoded bitmap size overflows"))?;
    if raster.data.len() < need {
        return Err(Error::corrupt(
            FORMAT_ID,
            format!(
                "decoded bitmap holds {} bytes, its header says {need}",
                raster.data.len()
            ),
        ));
    }
    let mut adjustments = Vec::new();
    let (out_w, out_h) = (width as usize, height as usize);
    let same_layout = from == to && from_ch == to_ch;
    if !same_layout {
        adjustments.push(format!(
            "pixel type: stream has {from_ch} x {}-bit{} samples, directory declares {}",
            from.bytes() * 8,
            if from.is_float() { " float" } else { "" },
            want.name()
        ));
    }
    if rw != out_w || rh != out_h {
        adjustments.push(format!(
            "size: stream is {rw}x{rh}, directory declares {out_w}x{out_h} (cropped/zero-padded)"
        ));
    }
    let out_px = to_ch * to.bytes();
    if same_layout && rw == out_w && rh == out_h {
        // The usual case: hand the decoder's buffer on instead of copying it.
        let mut data = raster.data;
        data.truncate(need);
        return Ok(Conformed { data, adjustments });
    }
    let mut out = vec![0u8; out_w * out_h * out_px];
    let (cw, ch) = (rw.min(out_w), rh.min(out_h));
    let mut px = Vec::with_capacity(out_px);
    for y in 0..ch {
        for x in 0..cw {
            let base = (y * rw + x) * from_ch;
            px.clear();
            if same_layout {
                let start = base * from.bytes();
                px.extend_from_slice(&raster.data[start..start + out_px]);
            } else {
                let sample = |c: usize| convert_sample(get(&raster.data, from, base + c), from, to);
                match (from_ch, to_ch) {
                    (1, 1) => put(&mut px, to, sample(0)),
                    (1, 3) => {
                        let gray = sample(0);
                        for _ in 0..3 {
                            put(&mut px, to, gray);
                        }
                    }
                    (_, 1) => {
                        // gray = (R + G + B + 1) / 3 for integers, the plain mean for floats
                        let sum = sample(0) + sample(1) + sample(2);
                        put(
                            &mut px,
                            to,
                            if to.is_float() {
                                sum / 3.0
                            } else {
                                ((sum + 1.0) / 3.0).floor()
                            },
                        );
                    }
                    _ => {
                        for c in 0..3 {
                            put(&mut px, to, sample(c));
                        }
                    }
                }
            }
            let o = (y * out_w + x) * out_px;
            out[o..o + out_px].copy_from_slice(&px);
        }
    }
    Ok(Conformed {
        data: out,
        adjustments,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raster(w: u32, h: u32, channels: u32, bits: u32, data: Vec<u8>) -> Raster {
        Raster {
            width: w,
            height: h,
            channels,
            bits_per_sample: bits,
            float: false,
            bgr: false,
            data,
        }
    }

    #[test]
    fn matching_raster_passes_through() {
        let r = raster(2, 1, 1, 8, vec![7, 9]);
        let c = conform(r, PixelTypeId::Gray8, 2, 1).unwrap();
        assert_eq!(c.data, [7, 9]);
        assert!(c.adjustments.is_empty());
    }

    #[test]
    fn rgb48_to_bgr24_keeps_high_byte() {
        let s: Vec<u8> = [0x09DC_u16, 0x08B5, 0x08C5]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let c = conform(raster(1, 1, 3, 16, s), PixelTypeId::Bgr24, 1, 1).unwrap();
        assert_eq!(c.data, [0x09, 0x08, 0x08]);
        assert_eq!(c.adjustments.len(), 1);
    }

    #[test]
    fn gray8_to_gray16_zero_extends_and_rgb_to_gray_averages() {
        let c = conform(raster(1, 1, 1, 8, vec![200]), PixelTypeId::Gray16, 1, 1).unwrap();
        assert_eq!(c.data, 200u16.to_le_bytes());
        let c = conform(
            raster(1, 1, 3, 8, vec![10, 20, 31]),
            PixelTypeId::Gray8,
            1,
            1,
        )
        .unwrap();
        assert_eq!(c.data, [20]); // (10 + 20 + 31 + 1) / 3
        let c = conform(raster(1, 1, 1, 8, vec![5]), PixelTypeId::Bgr24, 1, 1).unwrap();
        assert_eq!(c.data, [5, 5, 5]);
        let c = conform(
            raster(1, 1, 4, 8, vec![1, 2, 3, 255]),
            PixelTypeId::Bgra32,
            1,
            1,
        )
        .unwrap();
        assert_eq!(c.data, [1, 2, 3]);
    }

    #[test]
    fn size_mismatch_crops_and_pads_at_origin() {
        let r = raster(3, 2, 1, 8, vec![1, 2, 3, 4, 5, 6]);
        let c = conform(r, PixelTypeId::Gray8, 2, 3).unwrap();
        assert_eq!(c.data, [1, 2, 4, 5, 0, 0]);
        assert_eq!(c.adjustments.len(), 1);
    }

    #[test]
    fn short_or_odd_rasters_are_errors() {
        assert!(conform(raster(4, 4, 1, 8, vec![0; 3]), PixelTypeId::Gray8, 4, 4).is_err());
        assert!(conform(raster(1, 1, 2, 8, vec![0; 2]), PixelTypeId::Gray8, 1, 1).is_err());
        assert!(conform(raster(1, 1, 1, 12, vec![0; 2]), PixelTypeId::Gray16, 1, 1).is_err());
        assert!(
            conform(
                raster(1, 1, 1, 8, vec![0]),
                PixelTypeId::Gray64ComplexFloat,
                1,
                1
            )
            .is_err()
        );
    }
}
