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

/// Width, height, precision and component count from the SOF55 marker, after checking the
/// header fields T.87 bounds that are cheap to check: the SOF55 and SOS segment lengths, the
/// scan's NEAR, interleave mode and point transform, and the RESET value of a preset-parameters
/// (LSE) segment. pure_jpegls trusts some of these, and its sample arithmetic overflows on
/// streams that break them (fuzz/known-upstream/pure_jpegls-2.0.0-*.jls).
fn frame_header(data: &[u8]) -> Result<(u32, u32, u8, u8)> {
    if !data.starts_with(&[0xFF, 0xD8]) {
        return Err(err("stream does not start with SOI"));
    }
    let mut frame: Option<(u32, u32, u8, u8)> = None;
    let mut pos = 2usize;
    while pos + 4 <= data.len() {
        if data[pos] != 0xFF {
            return Err(err(format!("expected a marker at byte {pos}")));
        }
        let marker = data[pos + 1];
        let len = usize::from(u16::from_be_bytes([data[pos + 2], data[pos + 3]]));
        if len < 2 {
            return Err(err("marker segment length below 2"));
        }
        let seg = data
            .get(pos + 4..pos + 2 + len)
            .ok_or_else(|| err(format!("marker 0xFF{marker:02X} is truncated")))?;
        match marker {
            // SOF55: P, Y, X, Nf, then 3 bytes per component
            0xF7 => {
                if seg.len() < 6 || seg.len() != 6 + 3 * usize::from(seg[5]) {
                    return Err(err(format!("SOF55 segment of {len} bytes")));
                }
                let height = u32::from(u16::from_be_bytes([seg[1], seg[2]]));
                let width = u32::from(u16::from_be_bytes([seg[3], seg[4]]));
                frame = Some((width, height, seg[0], seg[5]));
            }
            // LSE preset coding parameters (id 1): MAXVAL, T1, T2, T3, RESET
            0xF8 if seg.first() == Some(&1) => {
                if seg.len() != 11 {
                    return Err(err(format!("LSE preset segment of {len} bytes")));
                }
                let reset = u16::from_be_bytes([seg[9], seg[10]]);
                if reset != 0 && reset < 3 {
                    return Err(err(format!("LSE RESET {reset} below 3")));
                }
            }
            // SOS: Ns, 2 bytes per component, NEAR, ILV, point transform
            0xDA => {
                let Some((w, h, precision, components)) = frame else {
                    return Err(err("SOS before the SOF55 frame header"));
                };
                let ns = usize::from(*seg.first().unwrap_or(&0));
                if ns == 0 || seg.len() != 4 + 2 * ns {
                    return Err(err(format!("SOS segment of {len} bytes")));
                }
                let (near, ilv, al) = (seg[1 + 2 * ns], seg[2 + 2 * ns], seg[3 + 2 * ns] & 0x0F);
                let max_near = if (2..=16).contains(&precision) {
                    ((1u32 << precision) - 1) / 2
                } else {
                    0
                };
                if u32::from(near) > max_near.min(255) {
                    return Err(err(format!(
                        "NEAR {near} above the limit for {precision}-bit samples"
                    )));
                }
                if ns == 1 && ilv != 0 {
                    return Err(err(format!("interleave mode {ilv} with one component")));
                }
                if al >= precision {
                    return Err(err(format!(
                        "point transform {al} for {precision}-bit samples"
                    )));
                }
                return Ok((w, h, precision, components));
            }
            _ => {}
        }
        pos += 2 + len;
    }
    Err(err(match frame {
        None => "no SOF55 frame header",
        Some(_) => "no SOS scan header",
    }))
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
        return Err(err(format!(
            "frame header declares an implausible {w}x{h} image"
        )));
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

    /// The two fuzzer inputs that overflowed pure_jpegls' arithmetic break the header rules and
    /// are refused before decoding.
    #[test]
    fn refuses_the_fuzzer_headers() {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/known-upstream");
        for name in [
            "pure_jpegls-2.0.0-run-mode-overflow.jls",
            "pure_jpegls-2.0.0-context-overflow.jls",
        ] {
            let Ok(d) = std::fs::read(dir.join(name)) else {
                continue;
            };
            assert!(decode(&d, 1 << 20).is_err(), "{name}");
        }
    }
}
