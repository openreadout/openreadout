//! Shimadzu LabSolutions signal streams: the 2-D `RC` layout (`Chromatogram ChN`, `Max Plot`)
//! and the 3-D PDA stream (one such record per time point). Layout and evidence:
//! `docs/formats/shimadzu.md` ("Signal streams") and `docs/provenance/shimadzu.md`
//! (2026-09-24). Every read is bounds-checked; malformed data is an error, never a panic.

use openreadout_core::bytes::{le_u16, le_u32, utf16le};

/// Bytes of an `RC` record header.
pub const RC_HEADER_LEN: usize = 24;
/// Most values one block holds.
pub const BLOCK_VALUES: usize = 256;

/// The header of an `RC` record.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RcHeader {
    /// Sampling interval in ms (2-D streams); 1 in 3-D records.
    pub interval_ms: u32,
    /// Number of values.
    pub count: u32,
    /// Bytes of the record (header included).
    pub length: u32,
}

/// Why a signal stream did not decode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalError(pub String);

impl std::fmt::Display for SignalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn err<T>(msg: impl Into<String>) -> Result<T, SignalError> {
    Err(SignalError(msg.into()))
}

/// Parse the header at the start of `b`.
pub fn parse_rc_header(b: &[u8]) -> Result<RcHeader, SignalError> {
    if b.len() < RC_HEADER_LEN || &b[..4] != b"RC\0\0" {
        return err("not an `RC` signal record (no `RC` tag or shorter than its header)");
    }
    Ok(RcHeader {
        interval_ms: le_u32(b, 4).unwrap_or(0),
        count: le_u32(b, 8).unwrap_or(0),
        length: le_u32(b, 12).unwrap_or(0),
    })
}

/// One variable-length value at `b[p..]`: `(value, bytes)`. A first byte `0x80`–`0x9F`
/// (top bits `100`) is a one-byte prefix before the value (seen on an analog-board channel,
/// `0x82` before every value; the values then equal the vendor export's), not a 5-byte value.
fn value(b: &[u8], p: usize) -> Result<(i64, usize), SignalError> {
    let Some(&first) = b.get(p) else {
        return err(format!("value at byte {p} lies past the end of the data"));
    };
    if first >> 5 == 4 {
        let (v, k) = plain_value(b, p + 1)?;
        return Ok((v, k + 1));
    }
    plain_value(b, p)
}

fn plain_value(b: &[u8], p: usize) -> Result<(i64, usize), SignalError> {
    let Some(&first) = b.get(p) else {
        return err(format!("value at byte {p} lies past the end of the data"));
    };
    if first >> 5 == 4 {
        return err(format!("two value prefixes in a row at byte {p}"));
    }
    let extra = usize::from(first >> 5);
    let Some(rest) = b.get(p + 1..p + 1 + extra) else {
        return err(format!("{}-byte value at byte {p} is cut off", extra + 1));
    };
    let mut v = i64::from(first & 0x1f);
    for &x in rest {
        v = (v << 8) | i64::from(x);
    }
    let bits = 5 + 8 * extra as u32;
    if v & (1 << (bits - 1)) != 0 {
        v -= 1 << bits;
    }
    Ok((v, extra + 1))
}

/// Decode the values of the record at the start of `b` (header included); returns them and the
/// bytes consumed. `max` bounds the count accepted from the header.
pub fn decode_record(b: &[u8], max: usize) -> Result<(RcHeader, Vec<i64>, usize), SignalError> {
    let header = parse_rc_header(b)?;
    let n = header.count as usize;
    if n > max {
        return err(format!(
            "record declares {n} values (more than {max} accepted)"
        ));
    }
    let mut out = Vec::with_capacity(n);
    let mut pos = RC_HEADER_LEN;
    while out.len() < n {
        let Some(len) = le_u16(b, pos).map(usize::from) else {
            return err(format!(
                "block header at byte {pos} is cut off after {} of {n} values",
                out.len()
            ));
        };
        let start = pos + 2;
        let end = start + len;
        if end + 2 > b.len() {
            return err(format!(
                "block at byte {pos} ({len} bytes) runs past the end of the data"
            ));
        }
        if le_u16(b, end).map(usize::from) != Some(len) {
            return err(format!(
                "block at byte {pos}: trailing length {:?} differs from the leading {len}",
                le_u16(b, end).map(usize::from)
            ));
        }
        let mut cursor = start;
        let mut acc = 0i64;
        let mut in_block = 0usize;
        while cursor < end {
            let (v, used) = value(b, cursor)?;
            cursor += used;
            acc = if in_block == 0 {
                v
            } else {
                acc.wrapping_add(v)
            };
            in_block += 1;
            if in_block > BLOCK_VALUES || out.len() >= n {
                return err(format!(
                    "block at byte {pos} holds more values than expected"
                ));
            }
            out.push(acc);
        }
        if cursor != end {
            return err(format!(
                "block at byte {pos}: the last value runs past the block"
            ));
        }
        pos = end + 2;
    }
    Ok((header, out, pos))
}

/// Whether a signal stream of `len` bytes holds `count` little-endian f64 values after its
/// header instead of integer blocks (a GC `.gcd` chromatogram: 24 + 8 × count bytes).
pub fn is_f64_record(len: u64, count: u32) -> bool {
    count > 0 && len == RC_HEADER_LEN as u64 + 8 * u64::from(count)
}

/// The f64 values of such a record (header included in `b`).
pub fn decode_f64_record(b: &[u8], max: usize) -> Result<(RcHeader, Vec<f64>), SignalError> {
    let h = parse_rc_header(b)?;
    let n = h.count as usize;
    if n > max {
        return err(format!(
            "record declares {n} values (more than {max} accepted)"
        ));
    }
    let body = b
        .get(RC_HEADER_LEN..RC_HEADER_LEN + 8 * n)
        .ok_or_else(|| SignalError(format!("{n} f64 values run past the end of the data")))?;
    Ok((
        h,
        body.as_chunks::<8>()
            .0
            .iter()
            .map(|c| f64::from_le_bytes(*c))
            .collect(),
    ))
}

/// Offsets of the records of a 3-D stream (each record says its own length).
pub fn record_offsets(b: &[u8], max_records: usize) -> Result<Vec<usize>, SignalError> {
    let mut out = Vec::new();
    let mut p = 0usize;
    while p < b.len() {
        let h = parse_rc_header(&b[p..])?;
        let len = h.length as usize;
        if len < RC_HEADER_LEN || p + len > b.len() {
            return err(format!(
                "3-D record {} at byte {p} declares {len} bytes; {} remain",
                out.len(),
                b.len() - p
            ));
        }
        out.push(p);
        if out.len() > max_records {
            return err(format!("more than {max_records} 3-D records"));
        }
        p += len;
    }
    Ok(out)
}

/// The wavelengths (nm) of a `Wavelength Table` stream: u32 count, then count u32 × 1/100 nm.
pub fn wavelength_table(b: &[u8]) -> Result<Vec<f64>, SignalError> {
    let Some(n) = le_u32(b, 0) else {
        return err("wavelength table is shorter than its count");
    };
    (0..n as usize)
        .map(|i| {
            le_u32(b, 4 + 4 * i)
                .map(|w| f64::from(w) / 100.0)
                .ok_or_else(|| SignalError(format!("wavelength table ends after {i} of {n}")))
        })
        .collect()
}

/// `<UPD ID="name"><Val>value</Val>` pairs of a UTF-16LE method document (PDA method).
pub fn method_values(b: &[u8]) -> Vec<(String, String)> {
    let text = utf16le(b);
    let mut out = Vec::new();
    let mut rest = text.as_str();
    while let Some(i) = rest.find("<UPD ID=\"") {
        rest = &rest[i + 9..];
        let Some(j) = rest.find('"') else { break };
        let id = rest[..j].to_string();
        rest = &rest[j..];
        if let (Some(a), Some(b)) = (rest.find("<Val>"), rest.find("</Val>"))
            && a < b
        {
            out.push((id, rest[a + 5..b].to_string()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(values: &[&[u8]], count: u32) -> Vec<u8> {
        let mut b = b"RC\0\0".to_vec();
        b.extend(500u32.to_le_bytes());
        b.extend(count.to_le_bytes());
        b.extend(0u32.to_le_bytes());
        b.extend([0u8; 8]);
        let data: Vec<u8> = values.concat();
        b.extend((data.len() as u16).to_le_bytes());
        b.extend(&data);
        b.extend((data.len() as u16).to_le_bytes());
        b
    }

    #[test]
    fn decodes_the_first_values_of_the_corpus_file() {
        // 22596, then differences −20826, −1553, +47 (the 3.00 corpus file's first bytes)
        let b = record(
            &[
                &[0x40, 0x58, 0x44],
                &[0x5f, 0xae, 0xa6],
                &[0x39, 0xef],
                &[0x20, 0x2f],
            ],
            4,
        );
        let (h, v, used) = decode_record(&b, 100).unwrap();
        assert_eq!(h.interval_ms, 500);
        assert_eq!(v, vec![22596, 1770, 217, 264]);
        assert_eq!(used, b.len());
        // one-byte values: 5-bit signed
        let b = record(&[&[0x03], &[0x1f]], 2);
        assert_eq!(decode_record(&b, 10).unwrap().1, vec![3, 2]);
        // a 0x80-0x9F prefix byte before each value (an analog-board channel: -25000, then 0s)
        let b = record(
            &[&[0x82, 0x5f, 0x9e, 0x58], &[0x82, 0x00], &[0x82, 0x00]],
            3,
        );
        assert_eq!(
            decode_record(&b, 10).unwrap().1,
            vec![-25000, -25000, -25000]
        );
        assert!(decode_record(&record(&[&[0x82, 0x82, 0x00]], 1), 10).is_err());
    }

    #[test]
    fn malformed_records_are_errors() {
        let b = record(&[&[0x40, 0x58, 0x44]], 2);
        assert!(decode_record(&b, 10).is_err()); // count says 2, data ends
        let mut b = record(&[&[0x40, 0x58, 0x44]], 1);
        let l = b.len();
        b[l - 1] = 9; // trailing length differs
        assert!(decode_record(&b, 10).is_err());
        assert!(decode_record(&b[..20], 10).is_err());
        assert!(decode_record(b"XX", 10).is_err());
        let b = record(&[&[0x60, 1]], 1); // a 4-byte value cut off
        assert!(decode_record(&b, 10).is_err());
        for cut in 0..b.len() {
            let _ = decode_record(&b[..cut], 10);
            let _ = record_offsets(&b[..cut], 10);
        }
    }

    #[test]
    fn tables_and_method_text() {
        let mut t = 2u32.to_le_bytes().to_vec();
        t.extend(18969u32.to_le_bytes());
        t.extend(19092u32.to_le_bytes());
        assert_eq!(wavelength_table(&t).unwrap(), vec![189.69, 190.92]);
        assert!(wavelength_table(&t[..8]).is_err());
        let m: Vec<u8> = "<UPD ID=\"SmplRt\"><Val>640</Val><Type>2</Type></UPD>"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        assert_eq!(method_values(&m), vec![("SmplRt".into(), "640".into())]);
    }
}
