//! Drift-resolved data of ion-mobility and SONAR functions: the per-scan index
//! (`_funcNNN.ind`), the compressed sections of `_funcNNN.cdt` (LZRW3, a public-domain
//! compression algorithm) and their decoding into time-of-flight indices and intensities per
//! drift bin. Every accessor is bounds-checked; malformed input yields `None` or an error
//! message.
//! Vocabulary and derivation: `docs/formats/waters-raw.md`, `docs/provenance/waters-raw.md`.

use openreadout_core::bytes::{le_f32, le_u32};

/// Bytes of the `_funcNNN.ind` header before the per-scan blocks.
pub const DRIFT_INDEX_HEADER: usize = 36;
/// Most drift bins per scan accepted (the corpus files have 200).
pub const MAX_DRIFT_BINS: usize = 4096;

/// One scan's entry in `_funcNNN.ind`.
#[derive(Debug, Clone, PartialEq)]
pub struct DriftScan {
    /// Byte offset of the scan's data in `_funcNNN.cdt` (the entries tile the file).
    pub offset: u64,
    /// Byte lengths of the scan's three compressed sections: positions, intensities, point
    /// flags.
    pub sections: [u32; 3],
    /// Per bin, the bytes of 8-byte long-step records stored between the first and second
    /// sections.
    pub long_steps: Vec<u32>,
    /// Per bin, the number of points.
    pub points: Vec<u32>,
    /// Retention time in seconds, as the block stores it in every bin.
    pub rt_s: Option<f32>,
    /// The second or fifth per-bin array holds a non-zero value (zero in every corpus file;
    /// their meaning is unknown).
    pub unknown_nonzero: bool,
}

impl DriftScan {
    /// Bytes the scan occupies in `_funcNNN.cdt`.
    pub fn byte_len(&self) -> u64 {
        self.sections.iter().map(|&s| u64::from(s)).sum::<u64>()
            + self.long_steps.iter().map(|&s| u64::from(s)).sum::<u64>()
    }
    /// Points over all bins.
    pub fn point_count(&self) -> u64 {
        self.points.iter().map(|&p| u64::from(p)).sum()
    }
}

/// A parsed `_funcNNN.ind`.
#[derive(Debug, Clone, PartialEq)]
pub struct DriftIndex {
    /// Drift bins per scan (u32 at byte 4).
    pub bins: usize,
    /// One entry per scan.
    pub scans: Vec<DriftScan>,
    /// Bytes after the last whole block.
    pub trailing: usize,
}

/// Parse `_funcNNN.ind`: the 36-byte header (bin count at 4), then per scan three u32
/// section lengths and five arrays of one u32 per bin (long-step bytes, unknown, retention
/// time as f32, points, unknown).
pub fn parse_drift_index(b: &[u8]) -> Option<DriftIndex> {
    let bins = le_u32(b, 4)? as usize;
    if bins == 0 || bins > MAX_DRIFT_BINS || b.len() < DRIFT_INDEX_HEADER {
        return None;
    }
    let block = 12 + 20 * bins;
    let body = b.get(DRIFT_INDEX_HEADER..)?;
    let mut scans = Vec::with_capacity(body.len() / block);
    let mut offset = 0u64;
    for blk in body.chunks_exact(block) {
        let sections = [le_u32(blk, 0)?, le_u32(blk, 4)?, le_u32(blk, 8)?];
        let arr = |k: usize| -> Vec<u32> {
            (0..bins)
                .map(|i| le_u32(blk, 12 + 4 * (k * bins + i)).unwrap_or(0))
                .collect()
        };
        let long_steps = arr(0);
        let points = arr(3);
        let unknown_nonzero = arr(1).iter().chain(&arr(4)).any(|&v| v != 0);
        let rt_s = le_f32(blk, 12 + 4 * (2 * bins)).filter(|v| v.is_finite());
        let s = DriftScan {
            offset,
            sections,
            long_steps,
            points,
            rt_s,
            unknown_nonzero,
        };
        offset = offset.checked_add(s.byte_len())?;
        scans.push(s);
    }
    Some(DriftIndex {
        bins,
        scans,
        trailing: body.len() % block,
    })
}

/// The string LZRW3's hash table points at before anything is decoded.
pub const LZRW3_START: &[u8; 18] = b"123456789012345678";

/// LZRW3's hash of the three bytes at an item's start: a 12-bit table index.
pub fn lzrw3_hash(b0: u8, b1: u8, b2: u8) -> usize {
    let v = (u32::from(b0) << 8) ^ (u32::from(b1) << 4) ^ u32::from(b2);
    ((v.wrapping_mul(40543) >> 4) & 0xFFF) as usize
}

/// Decompress one LZRW3 block: a u32 flag (0 compressed, 1 stored as is), then 16-bit control
/// words (lowest bit first) each governing 16 items: a literal byte, or a copy item whose first
/// byte's low nibble is the length − 3 and whose other 12 bits index a table of earlier item
/// positions hashed from their first three bytes. `None` when the block is malformed or would
/// exceed `max_out` bytes.
pub fn lzrw3_decompress(src: &[u8], max_out: usize) -> Option<Vec<u8>> {
    match le_u32(src, 0)? {
        1 => {
            let rest = src.get(4..)?;
            return (rest.len() <= max_out).then(|| rest.to_vec());
        }
        0 => {}
        _ => return None,
    }
    let base = LZRW3_START.len();
    let mut buf: Vec<u8> = Vec::with_capacity(base + src.len().saturating_mul(2).min(max_out));
    buf.extend_from_slice(LZRW3_START);
    let mut table = vec![0usize; 4096];
    // Item starts not yet hashed (their next two bytes were unknown when they were written).
    let mut pending: Vec<usize> = Vec::new();
    let mut p = 4usize;
    let mut control = 0u32;
    let mut bits = 0u32;
    while p < src.len() {
        if bits == 0 {
            let lo = *src.get(p)?;
            let hi = *src.get(p + 1)?;
            control = u32::from(lo) | (u32::from(hi) << 8);
            bits = 16;
            p += 2;
            if p >= src.len() {
                break;
            }
        }
        let copy = control & 1 == 1;
        control >>= 1;
        bits -= 1;
        let start = buf.len();
        if copy {
            let b1 = *src.get(p)?;
            let b2 = *src.get(p + 1)?;
            p += 2;
            let index = (usize::from(b1 & 0xF0) << 4) | usize::from(b2);
            let len = usize::from(b1 & 0x0F) + 3;
            let from = table[index];
            if buf.len() - base + len > max_out {
                return None;
            }
            for k in 0..len {
                let v = *buf.get(from + k)?;
                buf.push(v);
            }
            hash_pending(&buf, &mut table, &mut pending);
            table[index] = start;
        } else {
            if buf.len() - base >= max_out {
                return None;
            }
            buf.push(src[p]);
            p += 1;
            pending.push(start);
            hash_pending(&buf, &mut table, &mut pending);
        }
    }
    buf.drain(..base);
    Some(buf)
}

/// Enter every pending item start whose three bytes are now known into the table.
fn hash_pending(buf: &[u8], table: &mut [usize], pending: &mut Vec<usize>) {
    pending.retain(|&at| match buf.get(at..at + 3) {
        Some(&[a, b, c]) => {
            table[lzrw3_hash(a, b, c)] = at;
            false
        }
        _ => true,
    });
}

/// One drift bin of a scan: time-of-flight indices (ascending) and their intensities.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DriftBin {
    /// Time-of-flight index of each point.
    pub tof: Vec<u64>,
    /// Intensity of each point.
    pub intensity: Vec<i64>,
}

/// Decode one scan's bins from its bytes in `_funcNNN.cdt` (`data` starts at
/// [`DriftScan::offset`]): positions are u16 steps from the previous point of the bin (the
/// first from 0), with 8-byte (u32 point, u32 step) records for steps that do not fit;
/// intensities are i16 differences from the previous point of the bin.
pub fn decode_drift_scan(data: &[u8], scan: &DriftScan) -> Result<Vec<DriftBin>, String> {
    if scan.unknown_nonzero {
        return Err("a drift-index array whose meaning is unknown holds non-zero values".into());
    }
    let total = usize::try_from(scan.point_count()).map_err(|_| "too many points")?;
    let [l0, l1, l2] = scan.sections.map(|v| v as usize);
    let long: usize = scan.long_steps.iter().map(|&v| v as usize).sum();
    let section = |from: usize, len: usize, what: &str| -> Result<&[u8], String> {
        from.checked_add(len)
            .and_then(|end| data.get(from..end))
            .ok_or_else(|| format!("the {what} section runs past the end of the .cdt data"))
    };
    let pos_packed = section(0, l0, "position")?;
    let long_records = section(l0, long, "long-step")?;
    let val_packed = section(l0 + long, l1, "intensity")?;
    let flag_packed = section(l0 + long + l1, l2, "flag")?;
    let unpack = |s: &[u8], width: usize, what: &str| -> Result<Vec<u8>, String> {
        let out = lzrw3_decompress(s, total.saturating_mul(width))
            .ok_or_else(|| format!("the {what} section does not decompress"))?;
        if out.len() == total * width {
            Ok(out)
        } else {
            Err(format!(
                "the {what} section holds {} bytes for {total} points",
                out.len()
            ))
        }
    };
    let pos = unpack(pos_packed, 2, "position")?;
    let val = unpack(val_packed, 2, "intensity")?;
    unpack(flag_packed, 1, "flag")?;
    let mut bins = Vec::with_capacity(scan.points.len());
    let mut i = 0usize;
    let mut xi = 0usize;
    for (bin, (&count, &long_bytes)) in scan.points.iter().zip(&scan.long_steps).enumerate() {
        let count = count as usize;
        let mut steps: Vec<u64> = (i..i + count)
            .map(|k| u64::from(u16::from_le_bytes([pos[2 * k], pos[2 * k + 1]])))
            .collect();
        if long_bytes % 8 != 0 {
            return Err(format!(
                "bin {bin}: long-step bytes {long_bytes} are not whole records"
            ));
        }
        for _ in 0..long_bytes / 8 {
            let at = le_u32(long_records, xi).ok_or("long-step records run out")? as usize;
            let step = le_u32(long_records, xi + 4).ok_or("long-step records run out")?;
            xi += 8;
            *steps
                .get_mut(at)
                .ok_or_else(|| format!("bin {bin}: long step for point {at} of {count}"))? =
                u64::from(step);
        }
        let mut tof = Vec::with_capacity(count);
        let mut acc = 0u64;
        for s in steps {
            acc = acc.checked_add(s).ok_or("time-of-flight index overflows")?;
            tof.push(acc);
        }
        let mut intensity = Vec::with_capacity(count);
        let mut v = 0i64;
        for k in i..i + count {
            v += i64::from(i16::from_le_bytes([val[2 * k], val[2 * k + 1]]));
            intensity.push(v);
        }
        if intensity.iter().any(|&v| v < 0) {
            return Err(format!("bin {bin}: an intensity decodes negative"));
        }
        if tof.windows(2).any(|w| w[0] >= w[1]) {
            return Err(format!("bin {bin}: time-of-flight indices do not increase"));
        }
        i += count;
        bins.push(DriftBin { tof, intensity });
    }
    if xi != long_records.len() {
        return Err("long-step records left over".into());
    }
    Ok(bins)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compress with LZRW3's rules (literal-only control words): enough to test the decoder's
    /// literal path and its block header.
    fn literals(data: &[u8]) -> Vec<u8> {
        let mut out = vec![0, 0, 0, 0];
        for chunk in data.chunks(16) {
            out.extend_from_slice(&[0, 0]);
            out.extend_from_slice(chunk);
        }
        out
    }

    #[test]
    fn hash_matches_the_copy_codes_seen_in_the_corpus() {
        // `11 01 00` was copied with code 0x30 0x4F (index 0x34F) in HDMRM_Short_noLM.
        assert_eq!(lzrw3_hash(0x11, 0x01, 0x00), 0x34F);
        // `33 01 00` with 0xD0 0x2F (index 0xD2F), `5b 01 00` with 0xA0 0xAF (0xAAF).
        assert_eq!(lzrw3_hash(0x33, 0x01, 0x00), 0xD2F);
        assert_eq!(lzrw3_hash(0x5B, 0x01, 0x00), 0xAAF);
    }

    #[test]
    fn decompresses_literals_and_copies() {
        let data = b"abcdefabcdefabcdef";
        assert_eq!(lzrw3_decompress(&literals(data), 100).unwrap(), data);
        // "abcabcabc": three literals, then a copy of 6 from the table entry of "abc".
        let i = lzrw3_hash(b'a', b'b', b'c');
        let src = [
            0,
            0,
            0,
            0,
            0b1000,
            0,
            b'a',
            b'b',
            b'c',
            (((i >> 8) as u8) << 4) | 3,
            (i & 0xFF) as u8,
        ];
        assert_eq!(lzrw3_decompress(&src, 100).unwrap(), b"abcabcabc");
        // stored block
        assert_eq!(lzrw3_decompress(&[1, 0, 0, 0, 7, 8], 10).unwrap(), [7, 8]);
        // limits and malformed blocks
        assert!(lzrw3_decompress(&literals(data), 4).is_none());
        assert!(lzrw3_decompress(&[2, 0, 0, 0], 10).is_none());
        assert!(lzrw3_decompress(&[0, 0], 10).is_none());
    }

    #[test]
    fn decodes_a_scan() {
        // Two bins: [10, 11] with intensities [15, 6]; [70000, 70001, 70003] with [2, 10, 4].
        let pos: Vec<u8> = [10u16, 1, 0, 1, 2]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let val: Vec<u8> = [15i16, -9, 2, 8, -6]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let flags = [0u8; 5];
        let a = literals(&pos);
        let x: Vec<u8> = [0u32, 70000].iter().flat_map(|v| v.to_le_bytes()).collect();
        let b = literals(&val);
        let c = literals(&flags);
        let mut data = a.clone();
        data.extend(&x);
        data.extend(&b);
        data.extend(&c);
        let scan = DriftScan {
            offset: 0,
            sections: [a.len() as u32, b.len() as u32, c.len() as u32],
            long_steps: vec![0, 8],
            points: vec![2, 3],
            rt_s: Some(3.21),
            unknown_nonzero: false,
        };
        let bins = decode_drift_scan(&data, &scan).unwrap();
        assert_eq!(bins[0].tof, [10, 11]);
        assert_eq!(bins[0].intensity, [15, 6]);
        assert_eq!(bins[1].tof, [70000, 70001, 70003]);
        assert_eq!(bins[1].intensity, [2, 10, 4]);
        // truncated data is an error, not a panic
        assert!(decode_drift_scan(&data[..data.len() - 3], &scan).is_err());
    }

    #[test]
    fn parses_the_index() {
        let bins = 2usize;
        let mut b = vec![0u8; DRIFT_INDEX_HEADER];
        b[4..8].copy_from_slice(&(bins as u32).to_le_bytes());
        for (secs, rt) in [([10u32, 20, 5], 3.21f32), ([7, 8, 9], 4.0)] {
            for s in secs {
                b.extend(s.to_le_bytes());
            }
            for k in 0..5 {
                for _ in 0..bins {
                    let v = match k {
                        0 => 8u32,
                        2 => rt.to_bits(),
                        3 => 4,
                        _ => 0,
                    };
                    b.extend(v.to_le_bytes());
                }
            }
        }
        let ix = parse_drift_index(&b).unwrap();
        assert_eq!(ix.bins, 2);
        assert_eq!(ix.scans.len(), 2);
        assert_eq!(ix.scans[0].byte_len(), 35 + 16);
        assert_eq!(ix.scans[1].offset, 51);
        assert_eq!(ix.scans[1].rt_s, Some(4.0));
        assert_eq!(ix.scans[0].point_count(), 8);
        assert!(parse_drift_index(&b[..20]).is_none());
    }
}
