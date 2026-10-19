//! A BAF spectrum's records: the spectrum header (`0xBFA02002`), its centroid block
//! (`0xBFA01002`) and its profile block (`0xBFA01001`), plain or packed. `docs/formats/bruker-baf.md`
//! § Spectra.

use std::collections::BTreeMap;

use openreadout_core::bytes::{array, be_u32, le_f32, le_f64, le_u32};
use openreadout_core::{Error, Result};

use super::FORMAT_ID;
use super::records::{
    TAG_CALIBRATION, TAG_PRECURSORS, TAG_VARIABLES, bad, parse_variable_list, walk,
};

/// Most samples a profile may declare.
pub const MAX_SAMPLES: usize = 64 << 20;
/// Third coding byte of a profile whose payload is zlib-compressed float32 samples (the packed
/// delta coding has 6 there).
pub const ZLIB_CODING: u8 = 9;
/// First word of a packed profile in every development file.
pub const PACKED_MARKER: u32 = 0xEE77;

/// A spectrum record.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SpectrumRecord {
    /// Segment number.
    pub segment: u32,
    /// Spectrum id (1-based).
    pub id: u32,
    /// Block flags: 3 profile and lines, 2 lines only.
    pub flags: u32,
    /// Retention time in milliseconds.
    pub rt_ms: u32,
    /// Scan-type bytes: polarity (0 positive, 1 negative), ?, scan mode, ?.
    pub scan_type: [u8; 4],
    /// Variable values by id.
    pub variables: BTreeMap<u16, [u8; 8]>,
    /// Isolation m/z of each precursor stage.
    pub precursors: Vec<f32>,
    /// The calibration payload.
    pub calibration: Option<Vec<u8>>,
}

impl SpectrumRecord {
    /// MS level: one more than the number of precursors.
    pub fn ms_level(&self) -> u32 {
        1 + self.precursors.len() as u32
    }
}

/// Parse a spectrum record (its whole bytes).
pub fn parse_spectrum(rec: &[u8]) -> Result<SpectrumRecord> {
    let u = |k: usize| le_u32(rec, k).ok_or_else(|| bad("spectrum record cut short"));
    let hdr = u(8)? as usize;
    let mut s = SpectrumRecord {
        segment: u(12)?,
        id: u(16)?,
        flags: u(20)?,
        rt_ms: u(24)?,
        scan_type: array::<4>(rec, 32).ok_or_else(|| bad("spectrum record cut short"))?,
        ..SpectrumRecord::default()
    };
    if hdr < 40 || hdr > rec.len() {
        return Err(bad(format!("spectrum {} header of {hdr} bytes", s.id)));
    }
    for r in walk(rec, hdr, rec.len())? {
        let sub = &rec[r.offset..r.offset + r.len];
        match r.tag {
            TAG_VARIABLES => {
                let (Some(h), Some(n)) = (le_u32(sub, 8), le_u32(sub, 12)) else {
                    return Err(bad("short variable list"));
                };
                s.variables = parse_variable_list(sub, h as usize, n as usize)?;
            }
            TAG_PRECURSORS => {
                let n = le_u32(sub, 12).unwrap_or(0) as usize;
                for k in 0..n {
                    s.precursors.push(
                        le_f32(sub, 16 + 4 * k)
                            .ok_or_else(|| bad("precursor list runs past its record"))?,
                    );
                }
            }
            TAG_CALIBRATION if sub.len() > 12 => s.calibration = Some(sub[12..].to_vec()),
            _ => {}
        }
    }
    Ok(s)
}

/// A centroid block: positions (fractional sample indices) and intensities.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LineBlock {
    /// Spectrum id.
    pub id: u32,
    /// Element code (3: f64 positions, f32 intensities and widths; 1: all f32; 2: f64 positions
    /// and intensities).
    pub element_code: u32,
    /// Statistics: first and last sample of the range, minimum, maximum and sum of intensities.
    pub stats: [f32; 5],
    /// Positions.
    pub position: Vec<f64>,
    /// Intensities.
    pub intensity: Vec<f64>,
    /// A third array when stored (values of a few samples; read as peak widths).
    pub width: Option<Vec<f64>>,
}

fn array_values(data: &[u8], at: usize, n: usize, size: usize) -> Result<Vec<f64>> {
    let mut out = Vec::with_capacity(n);
    for k in 0..n {
        let v = if size == 8 {
            le_f64(data, at + 8 * k)
        } else {
            le_f32(data, at + 4 * k).map(f64::from)
        };
        out.push(v.ok_or_else(|| bad("a centroid array runs past its block"))?);
    }
    Ok(out)
}

/// Parse a centroid block (its whole bytes).
pub fn parse_lines(rec: &[u8]) -> Result<LineBlock> {
    let u = |k: usize| le_u32(rec, k).ok_or_else(|| bad("centroid block cut short"));
    let hdr = u(8)? as usize;
    let mut b = LineBlock {
        id: u(12)?,
        element_code: u(20)?,
        ..LineBlock::default()
    };
    let n = u(24)? as usize;
    if hdr < 52 || hdr > rec.len() || n > MAX_SAMPLES {
        return Err(bad(format!(
            "centroid block {} header of {hdr} bytes",
            b.id
        )));
    }
    for (k, v) in b.stats.iter_mut().enumerate() {
        *v = le_f32(rec, 32 + 4 * k).unwrap_or(0.0);
    }
    let data = &rec[hdr..];
    if le_u32(data, 0) != Some(n as u32) {
        return Err(bad(format!(
            "centroid block {}: the array header counts {:?} lines, the block {n}",
            b.id,
            le_u32(data, 0)
        )));
    }
    let offs: Vec<usize> = (0..8)
        .filter_map(|k| le_u32(data, 4 + 4 * k))
        .filter(|&v| v != 0)
        .map(|v| v as usize)
        .collect();
    let mut bounds = offs.clone();
    bounds.push(data.len());
    let mut arrays = Vec::new();
    for k in 0..offs.len() {
        if bounds[k + 1] < bounds[k] {
            return Err(bad(format!(
                "centroid block {}: array offsets out of order",
                b.id
            )));
        }
        let size = bounds[k + 1] - bounds[k];
        if n == 0 {
            arrays.push(Vec::new());
            continue;
        }
        let es = size / n;
        if !matches!(es, 4 | 8) || es * n != size {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!("a centroid array of {size} bytes for {n} lines"),
                "Only arrays of 4- or 8-byte values are known in BAF centroid blocks.",
            ));
        }
        arrays.push(array_values(data, bounds[k], n, es)?);
    }
    let mut it = arrays.into_iter();
    b.position = it.next().unwrap_or_default();
    b.intensity = it.next().unwrap_or_default();
    b.width = it.next();
    if b.intensity.len() != b.position.len() {
        return Err(bad(format!("centroid block {}: no intensity array", b.id)));
    }
    Ok(b)
}

/// Decode a profile block (its whole bytes) to its samples.
pub fn parse_profile(rec: &[u8]) -> Result<(u32, Vec<f64>)> {
    let hdr = le_u32(rec, 8).ok_or_else(|| bad("profile block cut short"))? as usize;
    let id = le_u32(rec, 12).unwrap_or(0);
    let coding = array::<4>(rec, 20).ok_or_else(|| bad("profile block cut short"))?;
    if hdr < 44 || hdr > rec.len() {
        return Err(bad(format!("profile block {id} header of {hdr} bytes")));
    }
    match coding[1] {
        4 => {
            let n = le_u32(rec, 24).unwrap_or(0) as usize;
            if n > MAX_SAMPLES || hdr + 4 * n != rec.len() {
                return Err(bad(format!(
                    "profile block {id}: {n} samples do not fill its {} bytes",
                    rec.len() - hdr
                )));
            }
            Ok((id, array_values(rec, hdr, n, 4)?))
        }
        1 if coding[2] == ZLIB_CODING => {
            let clen = le_u32(rec, 24).unwrap_or(0) as usize;
            if hdr + clen != rec.len() {
                return Err(bad(format!(
                    "profile block {id}: {clen} compressed bytes do not fill its {} bytes",
                    rec.len() - hdr
                )));
            }
            let raw = openreadout_codecs::zlib_decode(&rec[hdr..], 4 * MAX_SAMPLES + 4)
                .map_err(|e| bad(format!("profile block {id}: {e}")))?;
            if raw.len() % 4 != 0 || raw.len() > 4 * MAX_SAMPLES {
                return Err(bad(format!(
                    "profile block {id}: {} inflated bytes are not float32 samples",
                    raw.len()
                )));
            }
            Ok((id, array_values(&raw, 0, raw.len() / 4, 4)?))
        }
        1 => {
            let v = decode_packed(&rec[hdr..])?;
            Ok((id, v.into_iter().map(|x| x as f64).collect()))
        }
        _ => Err(Error::unsupported(
            FORMAT_ID,
            format!(
                "profile coding {:02x} {:02x} {:02x} {:02x}",
                coding[0], coding[1], coding[2], coding[3]
            ),
            "Only float32 profiles (plain or zlib-compressed) and the packed delta coding are known; the centroids are still readable (`spectrum --view centroid`).",
        )),
    }
}

/// Reads bits most significant first.
struct Bits<'a> {
    d: &'a [u8],
    pos: u64,
}

impl Bits<'_> {
    fn bit(&mut self) -> Result<u64> {
        let byte = *self
            .d
            .get((self.pos >> 3) as usize)
            .ok_or_else(|| bad("packed profile ended early"))?;
        let v = (byte >> (7 - (self.pos & 7))) & 1;
        self.pos += 1;
        Ok(u64::from(v))
    }

    fn bits(&mut self, w: u32) -> Result<u64> {
        let mut v = 0u64;
        for _ in 0..w {
            v = (v << 1) | self.bit()?;
        }
        Ok(v)
    }
}

/// Decode the packed profile coding: a header (format marker, sample count, big-endian code
/// table length, the code table) and a bit stream of class codes (c zero bits, then a one)
/// followed by delta bits; class 0 repeats the last sample, the last class is a 32-bit run
/// length, the one before it a 32-bit two's-complement delta, the others a magnitude with the
/// sign in the lowest bit.
#[allow(clippy::many_single_char_names)]
pub fn decode_packed(data: &[u8]) -> Result<Vec<i64>> {
    let marker = le_u32(data, 0).ok_or_else(|| bad("packed profile too short"))?;
    if marker != PACKED_MARKER {
        return Err(Error::unsupported(
            FORMAT_ID,
            format!("a packed profile with format marker {marker:#x}"),
            "Only marker 0xEE77 is known; read the centroids instead (`spectrum --view centroid`).",
        ));
    }
    let n = le_u32(data, 4).ok_or_else(|| bad("packed profile too short"))? as usize;
    let tlen = be_u32(data, 8).ok_or_else(|| bad("packed profile too short"))? as usize;
    if n > MAX_SAMPLES || tlen < 10 || tlen > data.len().saturating_sub(12) {
        return Err(bad(format!(
            "packed profile header out of range ({n} samples, table of {tlen} bytes)"
        )));
    }
    let table = &data[12..12 + tlen];
    let k = (tlen - 8) / 2;
    let widths: Vec<u32> = table[..k].iter().map(|&w| u32::from(w)).collect();
    let lengths: Vec<usize> = table[k..2 * k].iter().map(|&w| usize::from(w)).collect();
    let extra = (be_u32(table, 2 * k), be_u32(table, 2 * k + 4));
    let expected: Vec<usize> = (1..=k).collect();
    if k < 3
        || lengths != expected
        || extra != (Some(1), Some(k as u32 + 1))
        || widths.first() != Some(&0)
        || widths.last() != Some(&32)
        || widths[1..k - 1].iter().any(|&w| w == 0 || w > 30)
    {
        return Err(Error::unsupported(
            FORMAT_ID,
            "a packed profile with a code table not seen in development files",
            "Read the centroids instead (`spectrum --view centroid`), or send the file so the table can be checked.",
        ));
    }
    let mut base = vec![0i64, 1];
    for &w in &widths[1..k - 1] {
        let last = *base.last().unwrap_or(&1);
        base.push(last + (1i64 << w));
    }
    let mut out = Vec::with_capacity(n);
    let mut r = Bits {
        d: &data[12 + tlen..],
        pos: 0,
    };
    let mut cur = 0i64;
    while out.len() < n {
        let mut c = 0usize;
        while r.bit()? == 0 {
            c += 1;
            if c > k {
                return Err(bad(format!("bad class code at bit {}", r.pos)));
            }
        }
        if c == k {
            let run = r.bits(32)? as usize;
            if run > n - out.len() {
                return Err(bad("a run in a packed profile passes its sample count"));
            }
            out.resize(out.len() + run, cur);
            continue;
        }
        if c == k - 1 {
            let w = widths[c];
            let u = r.bits(w)?;
            let delta = if w == 32 {
                i64::from(u as u32 as i32)
            } else if u >> (w - 1) == 1 {
                u as i64 - (1i64 << w)
            } else {
                u as i64
            };
            cur = cur.wrapping_add(delta);
        } else if c > 0 {
            let u = r.bits(widths[c] + 1)?;
            let mag = (u >> 1) as i64 + base[c];
            cur = if u & 1 == 1 {
                cur.wrapping_sub(mag)
            } else {
                cur.wrapping_add(mag)
            };
        }
        out.push(cur);
    }
    Ok(out)
}

/// The value of variable `id` as a number, through its definition's type.
pub fn variable(
    vars: &BTreeMap<u16, [u8; 8]>,
    defs: &BTreeMap<u16, super::records::VariableDef>,
    id: Option<u16>,
) -> Option<f64> {
    let id = id?;
    defs.get(&id)?.decode(*vars.get(&id)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes bits most significant first.
    struct W(Vec<u8>, u32);
    impl W {
        fn put(&mut self, v: u64, n: u32) {
            for i in (0..n).rev() {
                if self.1.is_multiple_of(8) {
                    self.0.push(0);
                }
                let last = self.0.len() - 1;
                self.0[last] |= (((v >> i) & 1) as u8) << (7 - self.1 % 8);
                self.1 += 1;
            }
        }
        fn class(&mut self, c: u32) {
            self.put(0, c);
            self.put(1, 1);
        }
    }

    /// A profile record with coding `02 01 09 00`: a zlib stream (one stored deflate block)
    /// of float32 samples.
    #[test]
    fn zlib_float32_profiles() {
        let samples = [0.0f32, 21618.143, 0.0, 62095.562];
        let raw: Vec<u8> = samples.iter().flat_map(|v| v.to_le_bytes()).collect();
        let (mut a, mut b) = (1u32, 0u32);
        for &x in &raw {
            a = (a + u32::from(x)) % 65521;
            b = (b + a) % 65521;
        }
        let mut z = vec![0x78, 0x01, 0x01];
        let n = raw.len() as u16;
        z.extend_from_slice(&n.to_le_bytes());
        z.extend_from_slice(&(!n).to_le_bytes());
        z.extend_from_slice(&raw);
        z.extend_from_slice(&((b << 16) | a).to_be_bytes());
        let mut rec = Vec::new();
        rec.extend_from_slice(&((44 + z.len()) as u32).to_le_bytes());
        rec.extend_from_slice(&super::super::records::TAG_PROFILE.to_le_bytes());
        rec.extend_from_slice(&44u32.to_le_bytes());
        rec.extend_from_slice(&7u32.to_le_bytes());
        rec.extend_from_slice(&7u32.to_le_bytes());
        rec.extend_from_slice(&[2, 1, ZLIB_CODING, 0]);
        rec.extend_from_slice(&(z.len() as u32).to_le_bytes());
        rec.resize(44, 0);
        rec.extend_from_slice(&z);
        let (id, v) = parse_profile(&rec).unwrap();
        assert_eq!(id, 7);
        let want: Vec<f64> = samples.iter().map(|&x| f64::from(x)).collect();
        assert_eq!(v, want);
        // a compressed length that does not fill the record is corrupt
        let mut short = rec.clone();
        short[24] = short[24].wrapping_sub(1);
        assert_eq!(parse_profile(&short).unwrap_err().exit_code(), 4);
    }

    #[test]
    fn packed_profiles() {
        let widths = [0u8, 4, 5, 6, 7, 9, 10, 11, 32];
        let mut head = Vec::new();
        head.extend_from_slice(&0xEE77u32.to_le_bytes());
        head.extend_from_slice(&8u32.to_le_bytes());
        head.extend_from_slice(&26u32.to_be_bytes());
        head.extend_from_slice(&widths);
        head.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8, 9]);
        head.extend_from_slice(&1u32.to_be_bytes());
        head.extend_from_slice(&10u32.to_be_bytes());
        let mut w = W(Vec::new(), 0);
        // +5 (class 1: 4 + 1 bits, u = (5 - 1) << 1), -20 (class 2: base 17, u = (3 << 1) | 1),
        // repeat (class 0), a 32-bit delta of -1000 (class 8), a run of 3 (class 9), then +1.
        w.class(1);
        w.put(4 << 1, 5);
        w.class(2);
        w.put((3 << 1) | 1, 6);
        w.class(0);
        w.class(8);
        w.put(u64::from((-1000i32) as u32), 32);
        w.class(9);
        w.put(3, 32);
        w.class(1);
        w.put(0, 5);
        let mut data = head.clone();
        data.extend_from_slice(&w.0);
        assert_eq!(
            decode_packed(&data).unwrap(),
            [5, -15, -15, -1015, -1015, -1015, -1015, -1014]
        );
        // A different code table is refused (exit 6); a cut stream is corrupt (exit 4).
        let mut other = data.clone();
        other[12] = 1;
        assert_eq!(decode_packed(&other).unwrap_err().exit_code(), 6);
        assert_eq!(
            decode_packed(&data[..head.len() + 3])
                .unwrap_err()
                .exit_code(),
            4
        );
    }
}
