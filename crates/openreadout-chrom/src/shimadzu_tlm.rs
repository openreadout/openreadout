//! LC-MS data of LabSolutions `.lcd` files (`TLM Raw Data`; `docs/formats/shimadzu.md`
//! § LC-MS). `Retention Time` (u32 ms per record), `TIC Data` (u64 per record) and `Spectrum
//! Index` (24-byte entries: size, 1, offset, 0, record number, next number) locate the records
//! of `MS Raw Data`: `i32 −1`, u32 inflated length, u32 packed length, then a zlib stream. An
//! inflated record starts with a 20-byte header (retention time, cycle start, event, cycle,
//! record index), the acquisition code (u16) and, at +40, a count. MRM (15) and SIM (11) records
//! hold (Q1 × 100, Q3 × 100, intensity) triples. Full scans (10) and product-ion scans (14) hold
//! a profile of u32 intensities on a 0.1 m/z grid, but the vendor's software reports other values
//! at the same m/z for full scans (it applies something the file does not state) and no
//! product-ion scan could be checked, so their spectra are refused.

use miniz_oxide::inflate::TINFLStatus;
use miniz_oxide::inflate::core::{DecompressorOxide, decompress, inflate_flags};
use openreadout_core::bytes::{le_u16, le_u32};

/// Largest inflated record accepted (a full scan of 19,520 points is 78 kB).
pub const MAX_TLM_RECORD: usize = 16 << 20;

/// Acquisition code of a record.
pub const TLM_FULL_SCAN: u16 = 10;
/// SIM: selected ions, (m/z, m/z, intensity) triples.
pub const TLM_SIM: u16 = 11;
/// Product-ion scan: a profile after a precursor.
pub const TLM_PRODUCT_ION_SCAN: u16 = 14;
/// MRM: (Q1, Q3, intensity) triples.
pub const TLM_MRM: u16 = 15;

/// Where a record lies in `MS Raw Data`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TlmEntry {
    pub offset: u64,
    pub size: u32,
}

/// The three parallel index streams.
#[derive(Debug, Clone, Default)]
pub struct TlmIndex {
    pub entries: Vec<TlmEntry>,
    pub rt_ms: Vec<u32>,
    pub tic: Vec<u64>,
}

/// The fixed part of an inflated record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TlmHeader {
    pub rt_ms: u32,
    pub cycle_rt_ms: u32,
    pub event: u32,
    pub cycle: u32,
    pub record: u32,
    pub code: u16,
    /// The u32 at +36 (0 or 1; differs between the two polarities of a two-polarity method;
    /// which value is which is not known).
    pub polarity_code: u32,
    /// The count at +40 (triples, or profile points).
    pub count: u32,
    /// The u32 at +44: Q1 × 100 of an MRM record, the precursor × 100 of a product-ion scan.
    pub first_q1: u32,
}

/// One decoded record: MS level, precursor m/z (MRM, product-ion scan), and (m/z, intensity)
/// points in ascending m/z.
#[derive(Debug, Clone, PartialEq)]
pub struct TlmSpectrum {
    pub ms_level: u32,
    pub precursor_mz: Option<f64>,
    pub centroided: bool,
    pub scan_window_mz: Option<[f64; 2]>,
    pub points: Vec<(f64, f64)>,
}

/// Parse `Retention Time`, `Spectrum Index` and `TIC Data`; their counts must agree.
pub fn parse_index(rt: &[u8], index: &[u8], tic: &[u8]) -> Result<TlmIndex, String> {
    if !index.len().is_multiple_of(24)
        || !rt.len().is_multiple_of(4)
        || !tic.len().is_multiple_of(8)
    {
        return Err(format!(
            "TLM index streams of {} / {} / {} bytes are not whole records (24 / 4 / 8)",
            index.len(),
            rt.len(),
            tic.len()
        ));
    }
    let n = index.len() / 24;
    if rt.len() / 4 != n || tic.len() / 8 != n {
        return Err(format!(
            "TLM streams disagree: {n} index entries, {} retention times, {} TIC values",
            rt.len() / 4,
            tic.len() / 8
        ));
    }
    let entries = index
        .as_chunks::<24>()
        .0
        .iter()
        .map(|c| TlmEntry {
            size: u32::from_le_bytes([c[0], c[1], c[2], c[3]]),
            offset: u64::from(u32::from_le_bytes([c[8], c[9], c[10], c[11]])),
        })
        .collect();
    Ok(TlmIndex {
        entries,
        rt_ms: rt
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| u32::from_le_bytes(*c))
            .collect(),
        tic: tic
            .as_chunks::<8>()
            .0
            .iter()
            .map(|c| u64::from_le_bytes(*c))
            .collect(),
    })
}

/// The packed zlib bytes of a record and its declared inflated length.
fn packed(raw: &[u8], e: TlmEntry) -> Result<(&[u8], usize), String> {
    let start = usize::try_from(e.offset).map_err(|_| "record offset too large".to_string())?;
    let end = start
        .checked_add(e.size as usize)
        .filter(|&x| x <= raw.len())
        .ok_or_else(|| {
            format!(
                "record at {start} runs past MS Raw Data ({} bytes)",
                raw.len()
            )
        })?;
    let rec = &raw[start..end];
    if le_u32(rec, 0) != Some(u32::MAX) {
        return Err(format!("record at {start} does not start with -1"));
    }
    let ulen = le_u32(rec, 4).ok_or("record header cut")? as usize;
    let clen = le_u32(rec, 8).ok_or("record header cut")? as usize;
    let data = rec
        .get(12..12usize.checked_add(clen).ok_or("record length overflows")?)
        .ok_or_else(|| {
            format!(
                "record at {start}: {clen} packed bytes past its {} bytes",
                rec.len()
            )
        })?;
    if ulen > MAX_TLM_RECORD {
        return Err(format!("record at {start} inflates to {ulen} bytes"));
    }
    Ok((data, ulen))
}

/// Inflate a record whole (its declared length must match).
pub fn inflate_record(raw: &[u8], e: TlmEntry) -> Result<Vec<u8>, String> {
    let (data, ulen) = packed(raw, e)?;
    let out = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(data, ulen)
        .map_err(|err| format!("record at {}: zlib {:?}", e.offset, err.status))?;
    if out.len() != ulen {
        return Err(format!(
            "record at {} inflates to {} bytes, not the {ulen} it declares",
            e.offset,
            out.len()
        ));
    }
    Ok(out)
}

/// Inflate only the first 44 bytes of a record (its header), for listing scans quickly.
pub fn record_header(raw: &[u8], e: TlmEntry) -> Result<TlmHeader, String> {
    let (data, ulen) = packed(raw, e)?;
    let mut out = [0u8; 48];
    let mut state = Box::new(DecompressorOxide::new());
    let flags = inflate_flags::TINFL_FLAG_PARSE_ZLIB_HEADER
        | inflate_flags::TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF;
    let (status, _, n) = decompress(&mut state, data, &mut out, 0, flags);
    let enough = n == out.len() || (matches!(status, TINFLStatus::Done) && n == ulen);
    if !enough || n < 48 {
        return Err(format!(
            "record at {}: its header does not inflate ({status:?}, {n} bytes)",
            e.offset
        ));
    }
    header(&out).ok_or_else(|| format!("record at {}: short header", e.offset))
}

/// The header of an inflated record.
pub fn header(d: &[u8]) -> Option<TlmHeader> {
    Some(TlmHeader {
        rt_ms: le_u32(d, 0)?,
        cycle_rt_ms: le_u32(d, 4)?,
        event: le_u32(d, 8)?,
        cycle: le_u32(d, 12)?,
        record: le_u32(d, 16)?,
        code: le_u16(d, 20)?,
        polarity_code: le_u32(d, 36)?,
        count: le_u32(d, 40)?,
        first_q1: le_u32(d, 44).unwrap_or(0),
    })
}

impl TlmHeader {
    /// The precursor m/z the header states (MRM, product-ion scan).
    pub fn precursor_mz(&self) -> Option<f64> {
        matches!(self.code, TLM_MRM | TLM_PRODUCT_ION_SCAN)
            .then(|| f64::from(self.first_q1) / 100.0)
    }
}

/// MS level of an acquisition code (`None`: not read).
pub fn ms_level(code: u16) -> Option<u32> {
    match code {
        TLM_SIM | TLM_FULL_SCAN => Some(1),
        TLM_MRM | TLM_PRODUCT_ION_SCAN => Some(2),
        _ => None,
    }
}

/// Our name for an acquisition code.
pub fn acquisition_name(code: u16) -> String {
    match code {
        TLM_MRM => "mrm".into(),
        TLM_SIM => "sim".into(),
        TLM_PRODUCT_ION_SCAN => "product ion scan".into(),
        TLM_FULL_SCAN => "full scan".into(),
        c => format!("code {c}"),
    }
}

/// Decode an inflated record's points (full scans and unknown codes are errors).
pub fn decode(record: &[u8]) -> Result<TlmSpectrum, String> {
    let head = header(record).ok_or("record shorter than its header")?;
    let n = head.count as usize;
    match head.code {
        TLM_MRM | TLM_SIM => {
            let end = n.checked_mul(12).and_then(|x| x.checked_add(44)).ok_or("count overflows")?;
            let body = record.get(44..end).ok_or_else(|| format!("{n} ion entries past the {} record bytes", record.len()))?;
            let mut pts = Vec::with_capacity(n);
            let mut q1 = None;
            for entry in body.as_chunks::<12>().0 {
                let (precursor, product, intensity) = (
                    u32::from_le_bytes([entry[0], entry[1], entry[2], entry[3]]),
                    u32::from_le_bytes([entry[4], entry[5], entry[6], entry[7]]),
                    u32::from_le_bytes([entry[8], entry[9], entry[10], entry[11]]),
                );
                if head.code == TLM_SIM && precursor != product {
                    return Err(format!("SIM entry with two m/z values ({precursor}, {product})"));
                }
                if head.code == TLM_MRM {
                    match q1 {
                        None => q1 = Some(precursor),
                        Some(first) if first != precursor => {
                            return Err(format!("MRM record with two precursors ({first}, {precursor})"));
                        }
                        _ => {}
                    }
                }
                pts.push((f64::from(product) / 100.0, f64::from(intensity)));
            }
            pts.sort_by(|x, y| x.0.total_cmp(&y.0));
            Ok(TlmSpectrum {
                ms_level: if head.code == TLM_MRM { 2 } else { 1 },
                precursor_mz: q1.map(|raw| f64::from(raw) / 100.0),
                centroided: true,
                scan_window_mz: None,
                points: pts,
            })
        }
        TLM_PRODUCT_ION_SCAN => Err("product-ion scan spectra are not read: their profiles could not be checked against the vendor's (docs/formats/shimadzu.md)".into()),
        TLM_FULL_SCAN => Err("full-scan spectra are not read: the vendor software reports other values than the file stores at the same m/z (docs/formats/shimadzu.md)".into()),
        c => Err(format!("acquisition code {c} is not read")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(code: u16, body: &[u8], count: u32) -> Vec<u8> {
        let mut d = Vec::new();
        for v in [60_000u32, 60_000, 1, 1, 0] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        d.extend_from_slice(&code.to_le_bytes());
        d.extend_from_slice(&1u16.to_le_bytes());
        d.extend_from_slice(&[0; 12]);
        d.extend_from_slice(&0u32.to_le_bytes());
        d.extend_from_slice(&count.to_le_bytes());
        d.extend_from_slice(body);
        d
    }

    fn pack(d: &[u8]) -> Vec<u8> {
        let z = miniz_oxide::deflate::compress_to_vec_zlib(d, 6);
        let mut r = Vec::new();
        r.extend_from_slice(&u32::MAX.to_le_bytes());
        r.extend_from_slice(&(d.len() as u32).to_le_bytes());
        r.extend_from_slice(&(z.len() as u32).to_le_bytes());
        r.extend_from_slice(&z);
        r
    }

    #[test]
    fn mrm_sim_and_product_scans() {
        let mut body = Vec::new();
        for (a, b, v) in [(54420u32, 39730u32, 0u32), (54420, 32095, 10)] {
            for x in [a, b, v] {
                body.extend_from_slice(&x.to_le_bytes());
            }
        }
        let d = rec(TLM_MRM, &body, 2);
        let raw = pack(&d);
        let e = TlmEntry {
            offset: 0,
            size: raw.len() as u32,
        };
        assert_eq!(inflate_record(&raw, e).unwrap(), d);
        let h = record_header(&raw, e).unwrap();
        assert_eq!((h.code, h.count, h.event, h.rt_ms), (TLM_MRM, 2, 1, 60_000));
        let s = decode(&d).unwrap();
        assert_eq!(s.ms_level, 2);
        assert_eq!(s.precursor_mz, Some(544.2));
        assert_eq!(s.points, vec![(320.95, 10.0), (397.3, 0.0)]);
        // a product-ion scan header: precursor, then 3 points from m/z 39.0
        let mut body = Vec::new();
        for x in [104_420u32, 104_420, 3900, 3930, 5, 6, 7] {
            body.extend_from_slice(&x.to_le_bytes());
        }
        let raw = pack(&rec(TLM_PRODUCT_ION_SCAN, &body, 3));
        let h = record_header(
            &raw,
            TlmEntry {
                offset: 0,
                size: raw.len() as u32,
            },
        )
        .unwrap();
        assert_eq!(h.precursor_mz(), Some(1044.2));
        // profile spectra (product-ion and full scans) and unknown codes are refused
        assert!(decode(&rec(TLM_PRODUCT_ION_SCAN, &body, 3)).is_err());
        assert!(decode(&rec(TLM_FULL_SCAN, &body, 3)).is_err());
        assert!(decode(&rec(99, &[], 0)).is_err());
    }

    #[test]
    fn broken_records_are_errors() {
        assert!(parse_index(&[0; 4], &[0; 24], &[0; 16]).is_err());
        assert!(parse_index(&[0; 5], &[0; 24], &[0; 8]).is_err());
        let raw = pack(&rec(TLM_MRM, &[], 5));
        let e = TlmEntry {
            offset: 0,
            size: raw.len() as u32,
        };
        let d = inflate_record(&raw, e).unwrap();
        assert!(decode(&d).is_err()); // five triples announced, none stored
        assert!(
            inflate_record(
                &raw,
                TlmEntry {
                    offset: 4,
                    size: 10
                }
            )
            .is_err()
        );
        assert!(
            inflate_record(
                &raw,
                TlmEntry {
                    offset: 0,
                    size: raw.len() as u32 + 1
                }
            )
            .is_err()
        );
        let mut bad = raw.clone();
        bad[14] ^= 0xFF;
        assert!(
            inflate_record(&bad, e).is_err() || decode(&inflate_record(&bad, e).unwrap()).is_err()
        );
    }
}
