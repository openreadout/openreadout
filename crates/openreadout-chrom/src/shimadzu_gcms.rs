//! GC-MS data of Shimadzu GCMSsolution `.qgd` files (`GCMS Raw Data`): the record index and
//! the records of `MS Raw Data`. See `docs/formats/shimadzu.md` § "GC-MS data".

use openreadout_core::bytes::{le_u16, le_u32};

/// Bytes of a record header.
pub const GCMS_HEADER_LEN: usize = 32;
/// Added to the width code of a record's selected-ion layout when its entries are MRM
/// transitions (Q1 and Q3) rather than selected ions (one m/z).
pub const GCMS_MRM_CODE: u16 = 100;
/// m/z values are stored as u16 m/z × this.
pub const GCMS_MZ_SCALE: f64 = 20.0;
/// Top bit of a 4-byte intensity: a flag, not part of the value.
pub const GCMS_FLAG_BIT: u32 = 0x8000_0000;

/// The record index: retention times, record offsets in `MS Raw Data` and stored TICs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GcmsIndex {
    pub rt_ms: Vec<u32>,
    pub offsets: Vec<u64>,
    pub tic: Vec<u64>,
    /// `true` when `Spectrum Index` is a u16 1 and u64 offsets (else u32 offsets).
    pub wide_offsets: bool,
}

/// Parse `Retention Time`, `Spectrum Index` and `TIC Data`; their counts must agree.
pub fn parse_index(rt: &[u8], index: &[u8], tic: &[u8]) -> Result<GcmsIndex, String> {
    if !rt.len().is_multiple_of(4) {
        return Err(format!("Retention Time holds {} bytes, not u32s", rt.len()));
    }
    let n = rt.len() / 4;
    let rt_ms: Vec<u32> = rt
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_le_bytes(*c))
        .collect();
    let (offsets, wide_offsets): (Vec<u64>, bool) = if index.len() == 4 * n {
        (
            index
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| u64::from(u32::from_le_bytes(*c)))
                .collect(),
            false,
        )
    } else if index.len() == 2 + 8 * n && le_u16(index, 0) == Some(1) {
        (
            index[2..]
                .as_chunks::<8>()
                .0
                .iter()
                .map(|c| u64::from_le_bytes(*c))
                .collect(),
            true,
        )
    } else {
        return Err(format!(
            "Spectrum Index holds {} bytes for {n} records (neither {} nor {})",
            index.len(),
            4 * n,
            2 + 8 * n
        ));
    };
    if tic.len() < 8 * n {
        return Err(format!(
            "TIC Data holds {} bytes for {n} records",
            tic.len()
        ));
    }
    let tic = tic[..8 * n]
        .as_chunks::<8>()
        .0
        .iter()
        .map(|c| u64::from_le_bytes(*c))
        .collect();
    if offsets.windows(2).any(|w| w[1] < w[0]) {
        return Err("Spectrum Index offsets are not in order".into());
    }
    Ok(GcmsIndex {
        rt_ms,
        offsets,
        tic,
        wide_offsets,
    })
}

/// What a record holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GcmsRecordKind {
    /// A scan: `count` points of u16 m/z × 20 and an intensity of `intensity_bytes` bytes.
    Scan { intensity_bytes: usize },
    /// Selected ions: `count` entries of u16 m/z × 20 and an intensity.
    Sim { intensity_bytes: usize },
    /// MRM transitions: `count` entries of u16 Q1 × 20, u16 Q3 × 20 and an intensity.
    Mrm { intensity_bytes: usize },
}

/// Intensity bytes of a width code: 2 and 3 as they are, 1 for four bytes.
fn intensity_bytes(code: u16) -> Option<usize> {
    match code {
        1 => Some(4),
        2 => Some(2),
        3 => Some(3),
        _ => None,
    }
}

/// A record header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GcmsHeader {
    pub record: u32,
    pub rt_ms: u32,
    pub event: u16,
    /// Subtracted from every stored intensity (the exports' values).
    pub intensity_offset: u16,
    pub kind: GcmsRecordKind,
    /// Points (scans) or transitions (MRM).
    pub count: u32,
}

/// Parse the header of the record in `rec` (the bytes from the record's offset to the next
/// record's), checking that its body fills the record exactly.
pub fn header(rec: &[u8]) -> Result<GcmsHeader, String> {
    let short = || format!("record of {} bytes is shorter than its header", rec.len());
    let record = le_u32(rec, 0).ok_or_else(short)?;
    let rt_ms = le_u32(rec, 4).ok_or_else(short)?;
    let event = le_u16(rec, 8).ok_or_else(short)?;
    let intensity_offset = le_u16(rec, 10).ok_or_else(short)?;
    let width_code = le_u16(rec, 20).ok_or_else(short)?;
    let points = le_u16(rec, 22).ok_or_else(short)?;
    let params = rec.get(24..GCMS_HEADER_LEN).ok_or_else(short)?;
    let body = rec.len() - GCMS_HEADER_LEN;
    let (kind, count, entry) = if params.iter().all(|&b| b == 0) && points > 0 {
        let bytes = intensity_bytes(width_code)
            .ok_or_else(|| format!("scan record with intensity width code {width_code}"))?;
        (
            GcmsRecordKind::Scan {
                intensity_bytes: bytes,
            },
            u32::from(points),
            2 + bytes,
        )
    } else if width_code == 0 && points == 0 {
        // selected ions or MRM: the layout code and the entry count follow the header
        let code = le_u16(params, 0).ok_or_else(short)?;
        let n = le_u16(params, 2).ok_or_else(short)?;
        let mrm = code > GCMS_MRM_CODE;
        let bytes = intensity_bytes(code % GCMS_MRM_CODE)
            .ok_or_else(|| format!("selected-ion record with layout code {code}"))?;
        let kind = if mrm {
            GcmsRecordKind::Mrm {
                intensity_bytes: bytes,
            }
        } else {
            GcmsRecordKind::Sim {
                intensity_bytes: bytes,
            }
        };
        (kind, u32::from(n), if mrm { 4 } else { 2 } + bytes)
    } else {
        return Err(format!(
            "record of unknown layout (width code {width_code}, {points} points, parameters {params:02x?})"
        ));
    };
    if body != count as usize * entry {
        return Err(format!(
            "record body of {body} bytes holds {count} entries of {entry} bytes"
        ));
    }
    Ok(GcmsHeader {
        record,
        rt_ms,
        event,
        intensity_offset,
        kind,
        count,
    })
}

/// The points of a scan record: (m/z, intensity minus the record's offset), and how many
/// points carried the 4-byte form's flag bit.
pub fn scan_points(rec: &[u8]) -> Result<(Vec<(f64, f64)>, usize), String> {
    let h = header(rec)?;
    let GcmsRecordKind::Scan { intensity_bytes } = h.kind else {
        return Err("not a scan record".into());
    };
    let w = 2 + intensity_bytes;
    let body = &rec[GCMS_HEADER_LEN..];
    let mut out = Vec::with_capacity(h.count as usize);
    let mut flagged = 0;
    for p in body.chunks_exact(w) {
        let mz = f64::from(u16::from_le_bytes([p[0], p[1]])) / GCMS_MZ_SCALE;
        let mut raw = p[2..]
            .iter()
            .rev()
            .fold(0u32, |acc, &b| (acc << 8) | u32::from(b));
        if intensity_bytes == 4 && raw & GCMS_FLAG_BIT != 0 {
            raw &= !GCMS_FLAG_BIT;
            flagged += 1;
        }
        out.push((mz, f64::from(raw) - f64::from(h.intensity_offset)));
    }
    Ok((out, flagged))
}

/// The first Q1 of an MRM record, for listing (its values are not read).
pub fn mrm_first_precursor(rec: &[u8]) -> Option<f64> {
    let h = header(rec).ok()?;
    if !matches!(h.kind, GcmsRecordKind::Mrm { .. }) || h.count == 0 {
        return None;
    }
    le_u16(rec, GCMS_HEADER_LEN).map(|q| f64::from(q) / GCMS_MZ_SCALE)
}

/// The stored TIC minus the offset of every point: the exports' total intensity.
pub fn reported_tic(stored: u64, h: &GcmsHeader) -> f64 {
    stored as f64 - f64::from(h.intensity_offset) * f64::from(h.count)
}

/// The record at `k`: the bytes from its offset to the next record's (the stream's end for the
/// last).
pub fn record<'a>(raw: &'a [u8], index: &GcmsIndex, k: usize) -> Result<&'a [u8], String> {
    let start = *index
        .offsets
        .get(k)
        .ok_or_else(|| format!("record {k} not in the index"))?;
    let end = index
        .offsets
        .get(k + 1)
        .copied()
        .unwrap_or(raw.len() as u64);
    let (s, e) = (
        usize::try_from(start).map_err(|_| "offset overflows".to_string())?,
        usize::try_from(end).map_err(|_| "offset overflows".to_string())?,
    );
    raw.get(s..e).ok_or_else(|| {
        format!(
            "record {k} at {start}..{end} lies outside MS Raw Data ({} bytes)",
            raw.len()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(width_code: u16, points: &[(u16, u32)], bytes: usize) -> Vec<u8> {
        let mut d = Vec::new();
        for v in [0u32, 240_000] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        d.extend_from_slice(&0u16.to_le_bytes());
        d.extend_from_slice(&470u16.to_le_bytes());
        d.extend_from_slice(&[0; 8]);
        d.extend_from_slice(&width_code.to_le_bytes());
        d.extend_from_slice(&(points.len() as u16).to_le_bytes());
        d.extend_from_slice(&[0; 8]);
        for (m, v) in points {
            d.extend_from_slice(&m.to_le_bytes());
            d.extend_from_slice(&v.to_le_bytes()[..bytes]);
        }
        d
    }

    /// The first points of the first scan of MTBLS2984 (2-byte intensities) and of a 4-byte
    /// scan of MTBLS2235 with its flag bit (export: m/z 73.05, 6,924,148).
    #[test]
    fn scans_subtract_the_offset() {
        let r = rec(2, &[(1700, 556), (1720, 535)], 2);
        let h = header(&r).unwrap();
        assert_eq!(h.rt_ms, 240_000);
        assert_eq!(h.kind, GcmsRecordKind::Scan { intensity_bytes: 2 });
        let (p, flagged) = scan_points(&r).unwrap();
        assert_eq!(p, vec![(85.0, 86.0), (86.0, 65.0)]);
        assert_eq!(flagged, 0);
        assert_eq!(reported_tic(556 + 535, &h), 151.0);
        let r = rec(1, &[(900, 1_782_498), (1461, 0x8069_a94a)], 4);
        let (p, flagged) = scan_points(&r).unwrap();
        assert_eq!(p, vec![(45.0, 1_782_028.0), (73.05, 6_924_148.0)]);
        assert_eq!(flagged, 1);
        // a body that does not fill the record is an error
        assert!(header(&r[..r.len() - 1]).is_err());
    }

    #[test]
    fn mrm_records_and_index() {
        let mut d = rec(0, &[], 0);
        d[24..28].copy_from_slice(&[102, 0, 1, 0]);
        d.extend_from_slice(&[0x1c, 0x11, 0x7e, 0x0b, 0x05, 0x02]);
        let h = header(&d).unwrap();
        assert_eq!(
            (h.kind, h.count),
            (GcmsRecordKind::Mrm { intensity_bytes: 2 }, 1)
        );
        // selected ions with 3-byte intensities (layout code 3)
        let mut d3 = rec(0, &[], 0);
        d3[24..28].copy_from_slice(&[3, 0, 2, 0]);
        d3.extend_from_slice(&[0; 10]);
        assert_eq!(
            header(&d3).unwrap().kind,
            GcmsRecordKind::Sim { intensity_bytes: 3 }
        );
        assert_eq!(mrm_first_precursor(&d), Some(219.0));
        assert!(scan_points(&d).is_err());
        let rt = [1u32, 2]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect::<Vec<_>>();
        let tic = [5u64, 6]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect::<Vec<_>>();
        let narrow = [0u32, 40]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect::<Vec<_>>();
        let mut wide = 1u16.to_le_bytes().to_vec();
        wide.extend([0u64, 40].iter().flat_map(|v| v.to_le_bytes()));
        for ix in [narrow, wide] {
            let i = parse_index(&rt, &ix, &tic).unwrap();
            assert_eq!(
                (i.offsets, i.tic, i.rt_ms),
                (vec![0, 40], vec![5, 6], vec![1, 2])
            );
        }
        assert!(parse_index(&rt, &[0; 6], &tic).is_err());
    }
}
