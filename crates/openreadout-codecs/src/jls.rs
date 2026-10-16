//! JPEG-LS (ITU-T T.87, ISO/IEC 14495-1), lossless and near-lossless, as DICOM stores it
//! (transfer syntaxes 1.2.840.10008.1.2.4.80 and .81). Backed by `pure_jpegls` (library name `jpegls`) (MIT OR
//! Apache-2.0, pure Rust, no `unsafe`), which decodes single-component streams.

use crate::{CodecError, MAX_UNSIZED_OUTPUT, Raster, Result};

fn err(detail: impl Into<String>) -> CodecError {
    CodecError::Decode {
        codec: "jpeg-ls",
        detail: detail.into(),
    }
}

/// Width, height, precision and component count from the SOF55 marker.
fn frame_header(data: &[u8]) -> Result<(u32, u32, u8, u8)> {
    if !data.starts_with(&[0xFF, 0xD8]) {
        return Err(err("stream does not start with SOI"));
    }
    let mut pos = 2usize;
    while pos + 4 <= data.len() {
        if data[pos] != 0xFF {
            return Err(err(format!("expected a marker at byte {pos}")));
        }
        let marker = data[pos + 1];
        let len = usize::from(u16::from_be_bytes([data[pos + 2], data[pos + 3]]));
        if marker == 0xF7 {
            let seg = data
                .get(pos + 4..pos + 2 + len)
                .ok_or_else(|| err("SOF55 marker is truncated"))?;
            if seg.len() < 6 {
                return Err(err("SOF55 marker is truncated"));
            }
            let precision = seg[0];
            let height = u32::from(u16::from_be_bytes([seg[1], seg[2]]));
            let width = u32::from(u16::from_be_bytes([seg[3], seg[4]]));
            return Ok((width, height, precision, seg[5]));
        }
        if len < 2 {
            return Err(err("marker segment length below 2"));
        }
        pos += 2 + len;
    }
    Err(err("no SOF55 frame header"))
}

/// Decode a JPEG-LS stream, refusing (before decoding) an image of more than `max_bytes`.
/// Precisions up to 8 bits come back as u8, 9 to 16 bits as little-endian u16.
pub(crate) fn decode(data: &[u8], max_bytes: usize) -> Result<Raster> {
    let (w, h, precision, components) = frame_header(data)?;
    if w == 0 || h == 0 {
        return Err(err("frame header declares an empty image"));
    }
    if components != 1 {
        return Err(CodecError::Unsupported {
            codec: "jpeg-ls",
            detail: format!("{components}-component (colour) streams"),
        });
    }
    if !(2..=16).contains(&precision) {
        return Err(err(format!("sample precision {precision}")));
    }
    let bytes = u64::from(w) * u64::from(h) * 2;
    if bytes > max_bytes.min(MAX_UNSIZED_OUTPUT) as u64 {
        return Err(err(format!("frame header declares an implausible {w}x{h} image")));
    }
    let (px, dw, dh) = ::jpegls::decode(data, w, h).map_err(|e| err(e.to_string()))?;
    if (dw, dh) != (w, h) || px.len() != w as usize * h as usize {
        return Err(CodecError::SizeMismatch {
            codec: "jpeg-ls",
            got: px.len(),
            expected: w as usize * h as usize,
        });
    }
    let (bits, data) = if precision <= 8 {
        (8, px.iter().map(|&v| v.min(255) as u8).collect())
    } else {
        (16, px.iter().flat_map(|v| v.to_le_bytes()).collect())
    };
    Ok(Raster {
        width: w,
        height: h,
        channels: 1,
        bits_per_sample: bits,
        float: false,
        bgr: false,
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_ramp() {
        let px: Vec<u16> = (0..64u16 * 32).map(|i| i % 4096).collect();
        let mut opts = ::jpegls::EncodeOptions::default();
        opts.precision = Some(12);
        let mut enc = Vec::new();
        ::jpegls::encode_with_options(&px, 64, 32, &opts, &mut enc).expect("encodes");
        let r = decode(&enc, 1 << 20).expect("decodes");
        assert_eq!((r.width, r.height, r.bits_per_sample), (64, 32, 16));
        let back: Vec<u16> = r
            .data
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        assert_eq!(back, px);
    }

    #[test]
    fn refuses_garbage_and_huge_frames() {
        assert!(decode(b"not jpeg-ls", 1 << 20).is_err());
        // SOI, SOF55 declaring 65535 x 65535, then nothing
        let s = [
            0xFF, 0xD8, 0xFF, 0xF7, 0x00, 0x0B, 0x08, 0xFF, 0xFF, 0xFF, 0xFF, 0x01, 0x01, 0x11,
            0x00,
        ];
        assert!(decode(&s, 1 << 20).is_err());
    }
}
