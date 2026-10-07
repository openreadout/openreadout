//! JPEG 2000 (ISO/IEC 15444-1) decoding.
//!
//! Primary backend: `rust-j2k` (MIT OR Apache-2.0, pure Rust, no dependencies, no `unsafe`),
//! which reconstructs quantized coefficients at the mid-point of their bins as OpenJPEG does:
//! on the Aperio JPEG 2000 tiles of the corpus it is within 1 grey level of OpenJPEG
//! (0.03 % of samples differ), where `hayro-jpeg2000`-derived decoders reconstruct at the bin
//! edge and differ by up to 43 levels on coarsely quantized tiles. JP2 files are unwrapped to
//! their `jp2c` codestream here (`rust-j2k` reads raw codestreams). Codestreams `rust-j2k`
//! declares unsupported (HTJ2K, Part 2 extensions) fall back to `dicom-toolkit-jpeg2000`.

use crate::{CodecError, MAX_UNSIZED_OUTPUT, Raster, Result};

const CODESTREAM_MAGIC: &[u8] = b"\xFF\x4F\xFF\x51";
const JP2_MAGIC: &[u8] = b"\x00\x00\x00\x0C\x6A\x50\x20\x20";

fn err(detail: impl Into<String>) -> CodecError {
    CodecError::Decode {
        codec: "jpeg2000",
        detail: detail.into(),
    }
}

/// The contiguous codestream of a JP2 file (its `jp2c` box), or `data` itself when it is a raw
/// codestream.
pub(crate) fn codestream(data: &[u8]) -> Result<&[u8]> {
    if data.starts_with(CODESTREAM_MAGIC) {
        return Ok(data);
    }
    if !data.starts_with(JP2_MAGIC) {
        return Err(err(
            "neither a JPEG 2000 codestream (FF4F FF51) nor a JP2 file",
        ));
    }
    let mut pos = 0usize;
    while pos + 8 <= data.len() {
        let len32 = u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
        let kind = &data[pos + 4..pos + 8];
        let (header, len) = match len32 {
            0 => (8, data.len() - pos),
            1 => {
                let b = data
                    .get(pos + 8..pos + 16)
                    .ok_or_else(|| err("JP2 box header is truncated"))?;
                let l = u64::from_be_bytes(b.try_into().unwrap_or([0; 8]));
                (16, usize::try_from(l).unwrap_or(usize::MAX))
            }
            l => (8, l as usize),
        };
        if len < header {
            return Err(err("invalid JP2 box length"));
        }
        let end = pos.saturating_add(len).min(data.len());
        if kind == b"jp2c" {
            return Ok(&data[pos + header..end]);
        }
        pos = end;
    }
    Err(err("JP2 file without a codestream box"))
}

/// Largest number of declared samples per byte of codestream (a compression ratio of 65536 to
/// one for 8-bit samples). Blank 512 x 512 RGB tiles need 12 bytes at this ratio; real tiles,
/// noise included, are far larger.
const MAX_SAMPLES_PER_CODED_BYTE: u64 = 1 << 16;

/// Width, height and component count from the SIZ marker (image area on the reference grid).
pub(crate) fn siz(cs: &[u8]) -> Result<(u32, u32, u16)> {
    let g = |o: usize| -> Result<u32> {
        cs.get(o..o + 4)
            .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
            .ok_or_else(|| err("SIZ marker is truncated"))
    };
    if !cs.starts_with(CODESTREAM_MAGIC) {
        return Err(err("codestream does not start with SOC SIZ"));
    }
    let (xsiz, ysiz, x0, y0) = (g(8)?, g(12)?, g(16)?, g(20)?);
    let csiz = cs
        .get(40..42)
        .map(|b| u16::from_be_bytes([b[0], b[1]]))
        .ok_or_else(|| err("SIZ marker is truncated"))?;
    if xsiz <= x0 || ysiz <= y0 || csiz == 0 {
        return Err(err("SIZ marker declares an empty image"));
    }
    Ok((xsiz - x0, ysiz - y0, csiz))
}

/// Decode a JPEG 2000 codestream or JP2 file. Refuses before decoding when the image declared
/// by the SIZ marker would decode to more than `max_bytes` (at 2 bytes per sample).
pub(crate) fn decode(data: &[u8], max_bytes: usize) -> Result<Raster> {
    let cs = codestream(data)?;
    let (w, h, c) = siz(cs)?;
    let bytes = u64::from(w)
        .saturating_mul(u64::from(h))
        .saturating_mul(u64::from(c))
        .saturating_mul(2);
    if bytes > max_bytes.min(MAX_UNSIZED_OUTPUT) as u64 {
        return Err(err(format!(
            "codestream declares an implausible {w}x{h}x{c} image"
        )));
    }
    // The decoders allocate several bytes per declared sample before reading any coded data,
    // so a few hundred bytes declaring a 300-megapixel image cost gigabytes. Coded data that
    // small cannot hold such an image in any instrument file: refuse more than
    // `MAX_SAMPLES_PER_CODED_BYTE` samples per byte of codestream.
    let samples = u64::from(w)
        .saturating_mul(u64::from(h))
        .saturating_mul(u64::from(c));
    if samples > (cs.len() as u64).saturating_mul(MAX_SAMPLES_PER_CODED_BYTE) {
        return Err(err(format!(
            "a {}-byte codestream declares a {w}x{h}x{c} image",
            cs.len()
        )));
    }
    match rust_j2k::decode(cs) {
        Ok(img) => from_rust_j2k(&img),
        Err(rust_j2k::Error::Unsupported(what)) => fallback(data).map_err(|e| match e {
            CodecError::Decode { detail, .. } => CodecError::Unsupported {
                codec: "jpeg2000",
                detail: format!("{what} ({detail})"),
            },
            other => other,
        }),
        // A codestream rust-j2k refuses as malformed may still be one the more lenient decoder
        // reads (dicom-toolkit-jpeg2000's own encoder writes a COD segment length rust-j2k and
        // OpenJPEG reject); rust-j2k's error is reported when both fail.
        Err(e) => fallback(data).map_err(|_| err(e.to_string())),
    }
}

/// Samples of every component on the full image grid: sub-sampled components (4:2:2 and 4:2:0
/// chroma, as in Aperio 33003 tiles) are replicated, as OpenJPEG's `sycc422/420_to_rgb` do.
fn upsampled(c: &rust_j2k::Component, w: u32, h: u32) -> Result<std::borrow::Cow<'_, [i32]>> {
    if (c.width, c.height) == (w, h) {
        return Ok(std::borrow::Cow::Borrowed(&c.samples));
    }
    let (xs, ys) = (
        u32::from(c.x_sampling.max(1)),
        u32::from(c.y_sampling.max(1)),
    );
    if c.width == 0
        || c.height == 0
        || c.width.saturating_mul(xs) < w
        || c.height.saturating_mul(ys) < h
    {
        return Err(err("component grid does not cover the image"));
    }
    let mut v = Vec::with_capacity(w as usize * h as usize);
    for y in 0..h {
        let row = ((y / ys).min(c.height - 1) * c.width) as usize;
        for x in 0..w {
            v.push(c.samples[row + (x / xs).min(c.width - 1) as usize]);
        }
    }
    Ok(std::borrow::Cow::Owned(v))
}

fn from_rust_j2k(img: &rust_j2k::Image) -> Result<Raster> {
    let comps = &img.components;
    let first = comps
        .first()
        .ok_or_else(|| err("codestream has no components"))?;
    let (w, h) = (img.width, img.height);
    let depth = first.bit_depth;
    if u64::from(w) * u64::from(h) > MAX_UNSIZED_OUTPUT as u64 {
        return Err(err("image area too large"));
    }
    let planes = comps
        .iter()
        .map(|c| upsampled(c, w, h))
        .collect::<Result<Vec<_>>>()?;
    for c in comps {
        if c.signed {
            return Err(CodecError::Unsupported {
                codec: "jpeg2000",
                detail: "signed samples".into(),
            });
        }
        if c.bit_depth != depth {
            return Err(CodecError::Unsupported {
                codec: "jpeg2000",
                detail: "components of different bit depths".into(),
            });
        }
    }
    let n = comps.len();
    let px = w as usize * h as usize;
    let data = if depth <= 8 {
        let mut v = vec![0u8; px * n];
        for (ci, c) in planes.iter().enumerate() {
            for (dst, &s) in v.iter_mut().skip(ci).step_by(n).zip(c.iter()) {
                *dst = s.clamp(0, 255) as u8;
            }
        }
        v
    } else if depth <= 16 {
        let mut v = vec![0u8; px * n * 2];
        for (ci, c) in planes.iter().enumerate() {
            let pairs = v.as_chunks_mut::<2>().0;
            for (dst, &s) in pairs.iter_mut().skip(ci).step_by(n).zip(c.iter()) {
                *dst = (s.clamp(0, 65535) as u16).to_le_bytes();
            }
        }
        v
    } else {
        return Err(CodecError::Unsupported {
            codec: "jpeg2000",
            detail: format!("{depth}-bit samples"),
        });
    };
    Ok(Raster {
        width: w,
        height: h,
        channels: n as u32,
        bits_per_sample: if depth <= 8 { 8 } else { 16 },
        float: false,
        bgr: false,
        data,
    })
}

/// `dicom-toolkit-jpeg2000` (a `hayro-jpeg2000` fork) for what `rust-j2k` does not decode.
fn fallback(data: &[u8]) -> Result<Raster> {
    use dicom_toolkit_jpeg2000::{DecodeSettings, Image};
    let img = Image::new(data, &DecodeSettings::default()).map_err(|e| err(format!("{e:?}")))?;
    let bm = img.decode_native().map_err(|e| err(format!("{e:?}")))?;
    let expected = bm.width as usize
        * bm.height as usize
        * usize::from(bm.num_components)
        * usize::from(bm.bytes_per_sample);
    if bm.data.len() != expected {
        return Err(CodecError::SizeMismatch {
            codec: "jpeg2000",
            got: bm.data.len(),
            expected,
        });
    }
    Ok(Raster {
        width: bm.width,
        height: bm.height,
        channels: u32::from(bm.num_components),
        bits_per_sample: 8 * u32::from(bm.bytes_per_sample),
        float: false,
        bgr: false,
        data: bm.data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jp2_boxes_are_unwrapped() {
        let cs = b"\xFF\x4F\xFF\x51rest";
        let mut jp2 = JP2_MAGIC.to_vec();
        jp2.extend_from_slice(&[0x0D, 0x0A, 0x87, 0x0A]);
        jp2.extend_from_slice(&[0, 0, 0, 12]);
        jp2.extend_from_slice(b"ftypjp2 ");
        jp2.extend_from_slice(&(8 + cs.len() as u32).to_be_bytes());
        jp2.extend_from_slice(b"jp2c");
        jp2.extend_from_slice(cs);
        assert_eq!(codestream(&jp2).unwrap(), cs);
        assert!(codestream(b"junk").is_err());
        assert!(siz(cs).is_err());
    }
}
