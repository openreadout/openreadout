//! Decoders for ChemStation signal bodies. All return raw (unscaled) values plus a note of how
//! the body ended, so `check` can tell a clean end from a truncated one.

use openreadout_core::bytes::{
    be_i16, be_i32, be_u16, be_u32, le_f64, le_i16, le_i32, le_u16, le_u32,
};

/// How a body walk ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BodyEnd {
    /// An explicit terminator (two zero bytes after the last record).
    Terminator,
    /// The last value ended exactly at the end of the file.
    EndOfFile,
    /// The file ended inside a record or value (truncated).
    Truncated {
        /// Offset (relative to the body start) of the incomplete item.
        at: u64,
    },
    /// A record header did not have the expected form.
    BadRecord { at: u64, detail: String },
}

/// Result of decoding a one-channel body.
#[derive(Debug, Clone)]
pub struct DecodedSignal {
    /// Raw values in stored units (multiply by the header scale).
    pub values: Vec<f64>,
    /// Absolute values introduced by escape codes.
    pub escapes: u64,
    /// Records walked (delta records only).
    pub records: u64,
    pub end: BodyEnd,
}

/// Versions 30/130: `0x10, n` then `n` big-endian i16 first differences (`0x8000` → i32).
pub fn decode_delta_records(body: &[u8]) -> DecodedSignal {
    let mut values = Vec::new();
    let mut escapes = 0;
    let mut records = 0;
    let mut pos = 0usize;
    let mut cur: i64 = 0;
    let end = loop {
        if pos == body.len() {
            break BodyEnd::EndOfFile;
        }
        let (Some(&tag), Some(&n)) = (body.get(pos), body.get(pos + 1)) else {
            break BodyEnd::Truncated { at: pos as u64 };
        };
        if tag == 0 && n == 0 {
            break BodyEnd::Terminator;
        }
        if tag != 0x10 {
            break BodyEnd::BadRecord {
                at: pos as u64,
                detail: format!("record tag 0x{tag:02X}, expected 0x10"),
            };
        }
        pos += 2;
        records += 1;
        let mut truncated = None;
        for _ in 0..n {
            let Some(d) = be_i16(body, pos) else {
                truncated = Some(pos);
                break;
            };
            if d == i16::MIN {
                let Some(abs) = be_i32(body, pos + 2) else {
                    truncated = Some(pos);
                    break;
                };
                cur = i64::from(abs);
                escapes += 1;
                pos += 6;
            } else {
                cur += i64::from(d);
                pos += 2;
            }
            values.push(cur as f64);
        }
        if let Some(at) = truncated {
            break BodyEnd::Truncated { at: at as u64 };
        }
    };
    DecodedSignal {
        values,
        escapes,
        records,
        end,
    }
}

/// Versions 81/181: big-endian i16 second differences; `0x7FFF` → 48-bit absolute (i32 high
/// word, u16 low word) and a reset of the running first difference. With `zero_terminates`
/// (version 181) a final `0x0000` word ends the body instead of adding a sample.
pub fn decode_second_difference(body: &[u8], zero_terminates: bool) -> DecodedSignal {
    let mut values = Vec::with_capacity(body.len() / 2);
    let mut escapes = 0;
    let mut pos = 0usize;
    let mut cur: i64 = 0;
    let mut diff: i64 = 0;
    let end = loop {
        if pos == body.len() {
            break BodyEnd::EndOfFile;
        }
        let Some(w) = be_i16(body, pos) else {
            break BodyEnd::Truncated { at: pos as u64 };
        };
        if zero_terminates && w == 0 && pos + 2 == body.len() {
            break BodyEnd::Terminator;
        }
        if w == i16::MAX {
            let (Some(hi), Some(lo)) = (be_i32(body, pos + 2), be_u16(body, pos + 6)) else {
                break BodyEnd::Truncated { at: pos as u64 };
            };
            cur = i64::from(hi) * 65_536 + i64::from(lo);
            diff = 0;
            escapes += 1;
            pos += 8;
        } else {
            diff += i64::from(w);
            cur += diff;
            pos += 2;
        }
        values.push(cur as f64);
    };
    DecodedSignal {
        values,
        escapes,
        records: 0,
        end,
    }
}

/// Versions 179/181: little-endian f64.
pub fn decode_float64(body: &[u8]) -> DecodedSignal {
    let n = body.len() / 8;
    let values = (0..n).filter_map(|i| le_f64(body, i * 8)).collect();
    DecodedSignal {
        values,
        escapes: 0,
        records: 0,
        end: if body.len().is_multiple_of(8) {
            BodyEnd::EndOfFile
        } else {
            BodyEnd::Truncated { at: (n * 8) as u64 }
        },
    }
}

/// One scan record of a `.uv` or `.ms` file, located but not decoded.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScanRecord {
    /// Absolute file offset of the record.
    pub offset: u64,
    /// Record length in bytes.
    pub len: u64,
    /// Retention time, milliseconds.
    pub time_ms: u32,
    /// `.uv`: first/last/step wavelength × 20. `.ms`: unused (0).
    pub wavelength_raw: [u16; 3],
    /// `.uv`: values in the record. `.ms`: m/z–intensity pairs.
    pub count: u32,
    /// `.ms`: total ion current from the record trailer.
    pub tic: Option<u32>,
    /// `.uv`: the record stores little-endian f64 values (tag 70) instead of 16-bit
    /// differences (tag 67).
    pub float: bool,
}

/// `.uv` record tag of 16-bit differences.
pub const UV_TAG_DIFFS: u16 = 67;
/// `.uv` record tag of f64 values (OpenLab CDS spectra parts).
pub const UV_TAG_FLOAT: u16 = 70;

/// Header bytes of a `.uv` record before the values.
pub const UV_RECORD_HEADER: usize = 22;
/// Header bytes of a `.ms` record before the pairs, and trailer bytes after them.
pub const MS_RECORD_HEADER: usize = 18;
pub const MS_RECORD_TRAILER: usize = 10;

/// Parse the header of a `.uv` record from its first 22 bytes. `None` when the tag is neither
/// 67 nor 70.
pub fn uv_record(head: &[u8], offset: u64) -> Option<ScanRecord> {
    let tag = le_u16(head, 0)?;
    if tag != UV_TAG_DIFFS && tag != UV_TAG_FLOAT {
        return None;
    }
    let len = u64::from(le_u16(head, 2)?);
    let lo = le_u16(head, 8)?;
    let hi = le_u16(head, 10)?;
    let step = le_u16(head, 12)?;
    let count = if step == 0 || hi < lo {
        0
    } else {
        u32::from((hi - lo) / step) + 1
    };
    Some(ScanRecord {
        offset,
        len,
        time_ms: le_u32(head, 4)?,
        wavelength_raw: [lo, hi, step],
        count,
        tic: None,
        float: tag == UV_TAG_FLOAT,
    })
}

/// Decode the values of a `.uv` record (the whole record, header included). Raw units: tag 67
/// integers; tag 70 the stored f64 values (which are one header scale factor further from the
/// unit than tag-67 integers: see `docs/formats/openlab-cds.md`).
pub fn uv_values(rec: &[u8], count: u32) -> Result<Vec<f64>, String> {
    if le_u16(rec, 0) == Some(UV_TAG_FLOAT) {
        let need = UV_RECORD_HEADER + 8 * count as usize;
        let body = rec
            .get(UV_RECORD_HEADER..need)
            .ok_or("record ends before its last value")?;
        return body
            .as_chunks::<8>()
            .0
            .iter()
            .map(|c| {
                let v = f64::from_le_bytes(*c);
                if v.is_finite() {
                    Ok(v)
                } else {
                    Err("a non-finite value".to_string())
                }
            })
            .collect();
    }
    let mut pos = UV_RECORD_HEADER;
    let mut cur: i64 = 0;
    let mut out = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let d = le_i16(rec, pos).ok_or("record ends before its last value")?;
        if d == i16::MIN {
            cur = i64::from(le_i32(rec, pos + 2).ok_or("record ends inside an absolute value")?);
            pos += 6;
        } else {
            cur += i64::from(d);
            pos += 2;
        }
        out.push(cur as f64);
    }
    Ok(out)
}

/// Parse the header of a `.ms` record from its first 18 bytes.
pub fn ms_record(head: &[u8], offset: u64) -> Option<ScanRecord> {
    let words = be_u16(head, 0)?;
    let pairs = be_u16(head, 12)?;
    Some(ScanRecord {
        offset,
        len: 2 * u64::from(words),
        time_ms: be_u32(head, 2)?,
        wavelength_raw: [0; 3],
        count: u32::from(pairs),
        tic: None,
        float: false,
    })
}

/// Decode a `.ms` record's pairs: (m/z, intensity), in stored order, and the trailer TIC.
pub fn ms_pairs(rec: &[u8], count: u32) -> Result<(Vec<f64>, Vec<f32>, u32), String> {
    let mut mz = Vec::with_capacity(count as usize);
    let mut inten = Vec::with_capacity(count as usize);
    for i in 0..count as usize {
        let at = MS_RECORD_HEADER + 4 * i;
        let m = be_u16(rec, at).ok_or("record ends before its last pair")?;
        let v = be_u16(rec, at + 2).ok_or("record ends before its last pair")?;
        mz.push(f64::from(m) / 20.0);
        inten.push(ms_intensity(v));
    }
    let t = MS_RECORD_HEADER + 4 * count as usize + 6;
    let tic = be_u32(rec, t).ok_or("record ends inside its trailer")?;
    Ok((mz, inten, tic))
}

/// Packed intensity: 14-bit mantissa × 8^(top two bits).
pub fn ms_intensity(v: u16) -> f32 {
    let mantissa = f32::from(v & 0x3FFF);
    let exp = i32::from(v >> 14);
    mantissa * 8f32.powi(exp)
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    #[test]
    fn delta_records() {
        // two records: escape to 1000, +2, +3; then -1; terminator
        let body = [
            0x10, 3, 0x80, 0x00, 0, 0, 0x03, 0xE8, 0, 2, 0, 3, 0x10, 1, 0xFF, 0xFF, 0, 0,
        ];
        let d = decode_delta_records(&body);
        assert_eq!(d.values, vec![1000.0, 1002.0, 1005.0, 1004.0]);
        assert_eq!(d.end, BodyEnd::Terminator);
        assert_eq!(d.escapes, 1);
        let t = decode_delta_records(&body[..9]);
        assert!(matches!(t.end, BodyEnd::Truncated { .. }));
        let bad = decode_delta_records(&[0x11, 1, 0, 0]);
        assert!(matches!(bad.end, BodyEnd::BadRecord { .. }));
    }

    #[test]
    fn second_difference() {
        // escape to 2*65536+5, then second differences 1, 1, -2
        let body = [0x7F, 0xFF, 0, 0, 0, 2, 0, 5, 0, 1, 0, 1, 0xFF, 0xFE];
        let d = decode_second_difference(&body, false);
        assert_eq!(d.values, vec![131_077.0, 131_078.0, 131_080.0, 131_080.0]);
        assert_eq!(d.end, BodyEnd::EndOfFile);
        let mut z = body.to_vec();
        z.extend([0, 0]);
        assert_eq!(decode_second_difference(&z, true).values.len(), 4);
        assert_eq!(decode_second_difference(&z, true).end, BodyEnd::Terminator);
        assert_eq!(decode_second_difference(&z, false).values.len(), 5);
        assert!(matches!(
            decode_second_difference(&body[..5], false).end,
            BodyEnd::Truncated { at: 0 }
        ));
    }

    #[test]
    fn float64_and_intensity() {
        let mut b = 1.5f64.to_le_bytes().to_vec();
        b.extend_from_slice(&[1, 2, 3]);
        let d = decode_float64(&b);
        assert_eq!(d.values, vec![1.5]);
        assert_eq!(d.end, BodyEnd::Truncated { at: 8 });
        assert_eq!(ms_intensity(41737), 574_016.0);
        assert_eq!(ms_intensity(112), 112.0);
    }
}
