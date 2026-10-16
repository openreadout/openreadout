//! VBZ, the compression of nanopore signal: zstd around variable-byte integers of the zig-zag
//! encoded differences between successive samples.
//!
//! - POD5 signal rows: the variable-byte layer has one control bit per value (least
//!   significant bit first), 0 for one byte and 1 for two little-endian bytes; all control
//!   bytes come first. The row's `samples` gives the count.
//! - FAST5 chunks (HDF5 filter 32020, version 1): a u32 with the decoded size in bytes, then
//!   zstd of StreamVByte (two control bits per value for one to four bytes, four values per
//!   control byte).
//!
//! Derivation: `docs/provenance/nanopore.md`.

use std::io::Read;

/// Largest decoded signal accepted from one compressed row or chunk (samples).
pub const MAX_SAMPLES: usize = 1 << 30;

/// zstd-decompress `z`, refusing output larger than `max` bytes.
pub fn unzstd(z: &[u8], max: usize) -> Result<Vec<u8>, String> {
    let mut d = ruzstd::decoding::StreamingDecoder::new(z).map_err(|e| format!("zstd: {e}"))?;
    let mut out = Vec::new();
    let cap = u64::try_from(max).unwrap_or(u64::MAX).saturating_add(1);
    (&mut d)
        .take(cap)
        .read_to_end(&mut out)
        .map_err(|e| format!("zstd: {e}"))?;
    if out.len() > max {
        return Err(format!("zstd stream decodes to more than {max} bytes"));
    }
    Ok(out)
}

fn unzigzag(v: u32) -> i32 {
    ((v >> 1) as i32) ^ -((v & 1) as i32)
}

/// Undo the difference coding: each value is the previous plus the decoded difference.
fn integrate(diffs: impl Iterator<Item = u32>, n: usize) -> Vec<i16> {
    let mut out = Vec::with_capacity(n);
    let mut prev: i32 = 0;
    for z in diffs.take(n) {
        prev = prev.wrapping_add(unzigzag(z));
        out.push(prev as i16);
    }
    out
}

/// Decode the 16-bit variable-byte layer of a POD5 signal row (`n` samples).
pub fn svb16_decode(b: &[u8], n: usize) -> Result<Vec<i16>, String> {
    let ctrl_len = n.div_ceil(8);
    if b.len() < ctrl_len {
        return Err("signal row shorter than its control bytes".into());
    }
    let (ctrl, data) = b.split_at(ctrl_len);
    let mut at = 0usize;
    let mut diffs = Vec::with_capacity(n);
    for i in 0..n {
        let two = ctrl[i / 8] >> (i % 8) & 1 == 1;
        let v = if two {
            let s = data
                .get(at..at + 2)
                .ok_or("signal row ends inside its data")?;
            at += 2;
            u32::from(u16::from_le_bytes([s[0], s[1]]))
        } else {
            let s = *data.get(at).ok_or("signal row ends inside its data")?;
            at += 1;
            u32::from(s)
        };
        diffs.push(v);
    }
    Ok(integrate(diffs.into_iter(), n))
}

/// Decode a POD5 VBZ signal row of `n` samples.
pub fn pod5_row(z: &[u8], n: usize) -> Result<Vec<i16>, String> {
    if n > MAX_SAMPLES {
        return Err(format!("signal row of {n} samples"));
    }
    let raw = unzstd(z, n.div_ceil(8) + 2 * n)?;
    svb16_decode(&raw, n)
}

/// Decode standard StreamVByte (`n` values).
pub fn streamvbyte_decode(b: &[u8], n: usize) -> Result<Vec<u32>, String> {
    let ctrl_len = n.div_ceil(4);
    if b.len() < ctrl_len {
        return Err("StreamVByte block shorter than its control bytes".into());
    }
    let (ctrl, data) = b.split_at(ctrl_len);
    let mut at = 0usize;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let len = ((ctrl[i / 4] >> ((i % 4) * 2)) & 3) as usize + 1;
        let s = data
            .get(at..at + len)
            .ok_or("StreamVByte block ends inside its data")?;
        at += len;
        let mut v = [0u8; 4];
        v[..len].copy_from_slice(s);
        out.push(u32::from_le_bytes(v));
    }
    Ok(out)
}

/// Decode a FAST5 VBZ chunk (filter 32020) of 16-bit integers, with the filter's parameters
/// (version, integer size, zig-zag flag, zstd level).
pub fn fast5_chunk(chunk: &[u8], params: &[u32]) -> Result<Vec<i16>, String> {
    let version = params.first().copied().unwrap_or(0);
    let size = params.get(1).copied().unwrap_or(2);
    let zigzag = params.get(2).copied().unwrap_or(1) != 0;
    if size != 2 {
        return Err(format!("VBZ chunk of {size}-byte integers (only 2-byte signal is read)"));
    }
    let (n_bytes, body) = if version >= 1 {
        let h = chunk.get(..4).ok_or("VBZ chunk shorter than its header")?;
        (
            u32::from_le_bytes([h[0], h[1], h[2], h[3]]) as usize,
            &chunk[4..],
        )
    } else {
        return Err("VBZ filter version 0 chunks are not read".into());
    };
    let n = n_bytes / 2;
    if n > MAX_SAMPLES {
        return Err(format!("VBZ chunk of {n} samples"));
    }
    let raw = unzstd(body, n.div_ceil(4) + 4 * n)?;
    let v = streamvbyte_decode(&raw, n)?;
    if zigzag {
        Ok(integrate(v.into_iter(), n))
    } else {
        Ok(v.into_iter().map(|x| x as i16).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn svb16_matches_the_worked_example() {
        // [0,1,2,3,-1,100,1000,-300,5,5,…] (19 values), as pod5 encodes it before zstd
        let b = hex("c001000002020207ca0807270a620200000000000000000000");
        let v = svb16_decode(&b, 19).unwrap();
        assert_eq!(&v[..9], &[0, 1, 2, 3, -1, 100, 1000, -300, 5]);
        assert!(v[9..].iter().all(|x| *x == 5));
        assert!(svb16_decode(&b[..10], 19).is_err());
    }

    #[test]
    fn streamvbyte_first_values() {
        // control 0x01: value 0 has two bytes; 0x036a = 874 → +437, 0x15 = 21 → -11
        let b = hex("01000000006a0315");
        let v = streamvbyte_decode(&b, 2).unwrap();
        assert_eq!(v, vec![874, 21]);
        assert_eq!(integrate(v.into_iter(), 2), vec![437, 426]);
        assert!(fast5_chunk(&[1, 2], &[1, 2, 1, 1]).is_err());
        assert!(fast5_chunk(&[0; 8], &[1, 4, 1, 1]).is_err());
    }

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }
}
