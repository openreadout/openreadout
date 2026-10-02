//! Byte layouts of the streams of a legacy Sciex `.wiff` compound file and of the scan data in
//! its `.wiff.scan` companion. Every accessor is bounds-checked; malformed input yields `None`,
//! an empty list or an error string, never a panic.
//! Vocabulary and derivation: `docs/formats/sciex-wiff.md`, `docs/provenance/sciex-wiff.md`.

use openreadout_core::bytes::{Endian, le_f32, le_f64, le_u16, le_u32, utf16_units, utf16le};

/// Bytes of the preamble every decoded stream starts with (u32 0, u32 version, 24 zero bytes).
pub const STREAM_PREAMBLE: usize = 32;
/// Bytes per scan-index record in `SampleSubtree/SampleN/Idx`.
pub const INDEX_RECORD: usize = 54;
/// Bytes of the `.wiff.scan` header; index offsets count from here.
pub const SCAN_FILE_HEADER: u64 = 0x2C;
/// Bytes per record of `TOFCalibrationData` after its 0x38-byte head.
pub const TOF_CAL_RECORD: usize = 20;
/// Offset of the first per-scan record of `TOFCalibrationData`.
pub const TOF_CAL_FIRST: usize = 0x38;
/// Bytes per precursor slot of `DDERealTimeData`.
pub const PRECURSOR_SLOT: usize = 32;

/// Scan type code of an MRM experiment (`ExperimentHeader` u16 at 0x7A).
pub const SCAN_TYPE_MRM: u16 = 4;
/// Scan type code of a TOF MS experiment.
pub const SCAN_TYPE_TOF_MS: u16 = 8;
/// Scan type code of a TOF product-ion (MS/MS) experiment.
pub const SCAN_TYPE_TOF_PRODUCT: u16 = 9;

/// One record of the scan index.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IndexRecord {
    /// Offset of the scan's data in `.wiff.scan`, counted from [`SCAN_FILE_HEADER`].
    pub offset: u32,
    /// Bytes of scan data (0 for an experiment that recorded nothing in this cycle).
    pub byte_len: u32,
    /// Scan time, milliseconds.
    pub time_ms: f64,
    /// Total ion current.
    pub tic: f64,
    /// Base-peak intensity.
    pub base_peak_intensity: f64,
    /// Base-peak position (a TDC bin in TOF data, 0 in MRM data).
    pub base_peak_x: f64,
}

/// Parse the index records after the preamble; a trailing partial record is ignored.
pub fn parse_index(b: &[u8]) -> Vec<IndexRecord> {
    b.get(STREAM_PREAMBLE..)
        .unwrap_or(&[])
        .as_chunks::<INDEX_RECORD>()
        .0
        .iter()
        .filter_map(|r| {
            Some(IndexRecord {
                offset: le_u32(r, 0)?,
                byte_len: le_u32(r, 4)?,
                time_ms: le_f64(r, 8)?,
                tic: le_f64(r, 18)?,
                base_peak_intensity: le_f64(r, 26)?,
                base_peak_x: le_f64(r, 42)?,
            })
        })
        .collect()
}

/// Bytes after the last whole index record (non-zero when the index is cut off).
pub fn index_trailing(len: usize) -> usize {
    len.saturating_sub(STREAM_PREAMBLE) % INDEX_RECORD
}

/// Expand MRM scan data: little-endian f32 values in which a negative value −(n + 0.01)
/// stands for `n` zeros. At most `limit` values are produced.
pub fn expand_zero_runs(b: &[u8], limit: usize) -> Result<Vec<f32>, String> {
    let mut out = Vec::new();
    for c in b.as_chunks::<4>().0 {
        let v = f32::from_le_bytes(*c);
        if !v.is_finite() {
            return Err(format!("non-finite value {v} in MRM data"));
        }
        if v < 0.0 {
            let n = (-v).floor();
            if n > limit as f32 {
                return Err(format!("zero run of {n} values exceeds the scan"));
            }
            let n = n as usize;
            if out.len() + n > limit {
                return Err("zero runs exceed the scan's length".into());
            }
            out.resize(out.len() + n, 0.0);
        } else {
            if out.len() >= limit {
                return Err("more values than the scan holds".into());
            }
            out.push(v);
        }
    }
    Ok(out)
}

/// Decode a TOF scan: `FF FF FF FF` + u32 sets the TDC bin; bytes 0x00–0x7B are counts;
/// 0x7C/0x7D/0x7E are followed by a 1/2/4-byte count; 0x80–0xFB skip (byte − 0x80) steps;
/// 0xFC/0xFD/0xFE are followed by a 1/2/4-byte number of steps to skip; 0xFF bytes running to
/// the end of the scan end it. A count advances one step; a step is `step` TDC bins (the
/// experiment's bin grouping). Returns (bin, count) pairs for the non-empty steps.
pub fn decode_tdc(b: &[u8], step: u64) -> Result<Vec<(u64, u32)>, String> {
    let width = |code: u8| match code & 0x03 {
        0 => 1,
        1 => 2,
        _ => 4,
    };
    let mut out = Vec::new();
    let mut bin: u64 = 0;
    let mut have_start = false;
    let mut pos = 0usize;
    let int = |at: usize, n: usize| -> Option<u64> {
        let bytes = b.get(at..at.checked_add(n)?)?;
        Some(
            bytes
                .iter()
                .rev()
                .fold(0u64, |acc, &x| (acc << 8) | u64::from(x)),
        )
    };
    while pos < b.len() {
        let code = b[pos];
        if code == 0xFF {
            // 0xFF bytes that run to the end of the scan terminate it (one to four seen)
            if b[pos..].iter().all(|&x| x == 0xFF) {
                break;
            }
            if b.get(pos..pos + 4) == Some(&[0xFF; 4][..]) {
                match int(pos + 4, 4) {
                    Some(v) => {
                        bin = v;
                        have_start = true;
                        pos += 8;
                        continue;
                    }
                    None => return Err(format!("bin marker cut off at byte {pos}")),
                }
            }
            return Err(format!("unknown code 0xFF at byte {pos}"));
        }
        if !have_start {
            return Err("scan data does not start with a bin marker".into());
        }
        let (count, skip, used) = match code {
            0x00..=0x7B => (Some(u64::from(code)), 0, 1),
            0x7C..=0x7E => {
                let n = width(code);
                let v = int(pos + 1, n).ok_or_else(|| format!("count cut off at byte {pos}"))?;
                (Some(v), 0, 1 + n)
            }
            0x80..=0xFB => (None, u64::from(code - 0x80), 1),
            0xFC..=0xFE => {
                let n = width(code);
                let v = int(pos + 1, n).ok_or_else(|| format!("skip cut off at byte {pos}"))?;
                (None, v, 1 + n)
            }
            _ => return Err(format!("unknown code 0x{code:02X} at byte {pos}")),
        };
        if let Some(v) = count {
            let v = u32::try_from(v).map_err(|_| format!("count {v} too large"))?;
            out.push((bin, v));
            bin = bin.checked_add(step).ok_or("bin overflow")?;
        } else {
            let steps = skip.checked_mul(step).ok_or("bin overflow")?;
            bin = bin.checked_add(steps).ok_or("bin overflow")?;
        }
        pos += used;
    }
    Ok(out)
}

/// The TDC bins per stored step of an experiment: u32 at 0x38 of `ExperimentHeaderEx`
/// (4 in the TOF experiments of the corpus, 1 in the MRM experiment); 1 when absent or
/// implausible.
pub fn tdc_step(experiment_header_ex: &[u8]) -> u64 {
    le_u32(experiment_header_ex, 0x38)
        .filter(|v| (1..=256).contains(v))
        .map_or(1, u64::from)
}

/// One transition or mass range of an experiment (`MassRangeEx`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MassRange {
    /// First mass (MRM Q1, or the start of a scanned range).
    pub first_mz: f32,
    /// Second mass (MRM Q3).
    pub second_mz: f32,
    /// Fourth value: the expected retention time in minutes (scheduled MRM).
    pub expected_rt_min: f32,
    /// Compound name.
    pub name: String,
    /// Named parameters (`DP`, `CE`, `CXP`, …) with their first value.
    pub parameters: Vec<(String, f32)>,
}

impl MassRange {
    /// A parameter by name (`CE`, `DP`, …).
    pub fn parameter(&self, name: &str) -> Option<f32> {
        self.parameters
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| *v)
    }
}

fn utf16_at(b: &[u8], p: usize, byte_len: usize) -> Option<String> {
    b.get(p..p.checked_add(byte_len)?).map(utf16le)
}

/// Parse `MassRangeEx`: preamble, u32 (2), u32 parameters per range, then per range f32 first
/// mass, u32, f32 second mass, f32 expected retention time, u32, a u16-length UTF-16 name and
/// the parameters (u16-length UTF-16 key, f32, f32, u32). Stops at the first malformed range.
pub fn parse_mass_ranges(b: &[u8], expected: usize) -> Vec<MassRange> {
    let mut out = Vec::new();
    let Some(n_params) = le_u32(b, STREAM_PREAMBLE + 4) else {
        return out;
    };
    let n_params = n_params as usize;
    if n_params > 64 {
        return out;
    }
    let mut p = STREAM_PREAMBLE + 8;
    while out.len() < expected.min(1 << 16) {
        let (Some(first), Some(second), Some(rt), Some(name_len)) = (
            le_f32(b, p),
            le_f32(b, p + 8),
            le_f32(b, p + 12),
            le_u16(b, p + 20),
        ) else {
            break;
        };
        let name_len = name_len as usize;
        let Some(name) = utf16_at(b, p + 22, name_len) else {
            break;
        };
        p += 22 + name_len;
        let mut parameters = Vec::with_capacity(n_params);
        for _ in 0..n_params {
            let Some(kl) = le_u16(b, p) else { break };
            let kl = kl as usize;
            let (Some(key), Some(v)) = (utf16_at(b, p + 2, kl), le_f32(b, p + 2 + kl)) else {
                break;
            };
            parameters.push((key, v));
            p += 2 + kl + 12;
        }
        if parameters.len() != n_params {
            break;
        }
        out.push(MassRange {
            first_mz: first,
            second_mz: second,
            expected_rt_min: rt,
            name,
            parameters,
        });
    }
    out
}

/// One channel of an LC device recorded with the sample (`Devices/Device_K/Channel`).
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceChannel {
    /// Channel name (`Column Pressure`, `Pump A Flowrate`), trimmed.
    pub name: String,
    /// Unit, from the text after the name (`psi`, `nL/min`).
    pub unit: Option<String>,
    /// Samples per second (f64 after the unit).
    pub rate_hz: f64,
}

/// Parse a device's `Channel` stream: after the preamble, u32 (1), u16 channel count, then per
/// channel u32 (1), u16 byte length + UTF-16 name, u16 byte length + UTF-16 unit (`(psi)`),
/// f64 samples per second, u16 channel index, f32, f32, i32.
pub fn parse_device_channels(b: &[u8]) -> Vec<DeviceChannel> {
    let mut out = Vec::new();
    let Some(n) = le_u16(b, STREAM_PREAMBLE + 4) else {
        return out;
    };
    let mut p = STREAM_PREAMBLE + 6;
    for _ in 0..n.min(64) {
        let Some(nl) = le_u16(b, p + 4) else { break };
        let nl = nl as usize;
        let Some(name) = utf16_at(b, p + 6, nl) else {
            break;
        };
        let q = p + 6 + nl;
        let Some(ul) = le_u16(b, q) else { break };
        let ul = ul as usize;
        let Some(unit) = utf16_at(b, q + 2, ul) else {
            break;
        };
        let Some(rate_hz) = le_f64(b, q + 2 + ul) else {
            break;
        };
        let unit = unit
            .trim()
            .trim_start_matches('(')
            .trim_end_matches(')')
            .trim()
            .to_string();
        out.push(DeviceChannel {
            name: name.trim().to_string(),
            unit: (!unit.is_empty()).then_some(unit),
            rate_hz,
        });
        p = q + 2 + ul + 8 + 14;
    }
    out
}

/// A device's `DevData`: after the preamble, one f64 per channel per sample, sample-major.
pub fn parse_device_data(b: &[u8], channels: usize) -> Vec<Vec<f64>> {
    let mut out = vec![Vec::new(); channels];
    if channels == 0 {
        return out;
    }
    let body = b.get(STREAM_PREAMBLE..).unwrap_or_default();
    for row in body.chunks_exact(8 * channels) {
        for (k, c) in out.iter_mut().enumerate() {
            c.push(le_f64(row, 8 * k).unwrap_or(f64::NAN));
        }
    }
    out
}

/// What `ExperimentHeader` says about an experiment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ExperimentHeader {
    /// Scan type code (u16 at 0x7A): [`SCAN_TYPE_MRM`], [`SCAN_TYPE_TOF_MS`], …
    pub scan_type: u16,
    /// Polarity code (u16 at 0x56): 1 negative, 0 positive.
    pub polarity: u16,
    /// Number of mass ranges (u32 at 0xB0).
    pub range_count: u32,
}

/// Parse `ExperimentHeader`.
pub fn parse_experiment_header(b: &[u8]) -> Option<ExperimentHeader> {
    Some(ExperimentHeader {
        scan_type: le_u16(b, 0x7A)?,
        polarity: le_u16(b, 0x56)?,
        range_count: le_u32(b, 0xB0)?,
    })
}

/// Scheduled-MRM windows (`sMRMPro_adw_Times`): (start ms, end ms) per transition.
pub fn parse_windows(b: &[u8]) -> Vec<(u32, u32)> {
    b.get(STREAM_PREAMBLE..)
        .unwrap_or(&[])
        .as_chunks::<8>()
        .0
        .iter()
        .filter_map(|c| Some((le_u32(c, 0)?, le_u32(c, 4)?)))
        .collect()
}

/// The (a, t₀) TOF calibration of index record `i` (`TOFCalibrationData`).
pub fn tof_calibration(b: &[u8], i: usize) -> Option<(f64, f64)> {
    let at = TOF_CAL_FIRST.checked_add(i.checked_mul(TOF_CAL_RECORD)?)?;
    let a = le_f64(b, at)?;
    let t0 = le_f64(b, at + 8)?;
    (a.is_finite() && t0.is_finite() && a > 0.0).then_some((a, t0))
}

/// The default (a, t₀) pair at the head of `TOFCalibrationData`.
pub fn tof_default_calibration(b: &[u8]) -> Option<(f64, f64)> {
    let a = le_f64(b, STREAM_PREAMBLE)?;
    let t0 = le_f64(b, STREAM_PREAMBLE + 8)?;
    (a.is_finite() && t0.is_finite() && a > 0.0).then_some((a, t0))
}

/// m/z of TDC `bin` with bin width `width_ns` and calibration (a, t₀): (a·(width·bin − t₀))².
pub fn tof_mz(bin: f64, width_ns: f64, a: f64, t0: f64) -> f64 {
    let x = a * (bin * width_ns - t0);
    x * x
}

/// The precursor m/z and the third value of slot `slot` of `DDERealTimeData`.
pub fn precursor_slot(b: &[u8], slot: usize) -> Option<(f64, f64)> {
    let at = STREAM_PREAMBLE.checked_add(slot.checked_mul(PRECURSOR_SLOT)?)?;
    let mz = le_f64(b, at)?;
    let v = le_f64(b, at + 16)?;
    (mz.is_finite() && mz > 0.0).then_some((mz, v))
}

/// UTF-16LE text runs of at least `min` characters (printable ASCII range), in order.
pub fn utf16_runs(b: &[u8], min: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for u in utf16_units(b, Endian::Little) {
        match char::from_u32(u32::from(u)) {
            Some(ch) if (0x20..0x7F).contains(&u) || ch == '\u{b5}' || ch == '\u{b0}' => {
                cur.push(ch);
            }
            _ => {
                if cur.chars().count() >= min {
                    out.push(std::mem::take(&mut cur));
                }
                cur.clear();
            }
        }
    }
    if cur.chars().count() >= min {
        out.push(cur);
    }
    out
}

/// u16-length-prefixed UTF-16 strings of `SampleDABE/DATA`, after the preamble (whose last u32
/// is not zero in this stream: 4 or 6) and one u32 (100).
pub fn sample_strings(b: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut p = STREAM_PREAMBLE + 4;
    while let Some(n) = le_u16(b, p) {
        let n = n as usize;
        if !n.is_multiple_of(2) || n > 4096 {
            break;
        }
        let Some(s) = utf16_at(b, p + 2, n) else {
            break;
        };
        if s.chars().any(char::is_control) {
            break;
        }
        out.push(s);
        p += 2 + n;
        if out.len() >= 32 {
            break;
        }
    }
    out
}

/// `key: value` fields of the mass-spectrometer lines of the `Log` stream.
pub fn log_fields(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for part in text.split([',', '\n', '\r']) {
        if let Some((k, v)) = part.split_once(':') {
            let k = k.trim();
            let v = v.trim();
            if !k.is_empty() && k.len() < 48 && !v.is_empty() {
                out.push((k.to_string(), v.to_string()));
            }
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn device_channels_and_data() {
        // `Devices/Device_0/Channel` of the QTRAP 6500 PressureTrace1.wiff (ProteoWizard test
        // data), first channel only (count patched to 1)
        let b = hex(concat!(
            "0000000004000000000000000000000000000000000000000000000000000000",
            "010000000100",
            "010000002000200043006f006c0075006d006e0020005000720065007300730075007200650",
            "00a00280070007300690029000000000000000040000000000000c8420000c842ffffffff"
        ));
        let c = parse_device_channels(&b);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].name, "Column Pressure");
        assert_eq!(c[0].unit.as_deref(), Some("psi"));
        assert_eq!(c[0].rate_hz, 2.0);
        let mut d = vec![0u8; STREAM_PREAMBLE];
        for v in [1470.0f64, 1513.0] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        assert_eq!(parse_device_data(&d, 1), vec![vec![1470.0, 1513.0]]);
    }

    #[test]
    fn zero_runs() {
        let mut b = Vec::new();
        for v in [-3.01f32, 5.0, -1.01, 7.0] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        assert_eq!(
            expand_zero_runs(&b, 6).unwrap(),
            vec![0.0, 0.0, 0.0, 5.0, 0.0, 7.0]
        );
        assert!(expand_zero_runs(&b, 5).is_err());
        let nan = f32::NAN.to_le_bytes();
        assert!(expand_zero_runs(&nan, 10).is_err());
    }

    #[test]
    fn tdc() {
        let mut b = vec![0xFF, 0xFF, 0xFF, 0xFF];
        b.extend_from_slice(&100u32.to_le_bytes());
        b.extend_from_slice(&[
            0x05, 0x7D, 0xB6, 0x04, 0x83, 0x7C, 0xEC, 0xFD, 0x10, 0x00, 0x01,
        ]);
        b.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]);
        let d = decode_tdc(&b, 1).unwrap();
        assert_eq!(d, vec![(100, 5), (101, 1206), (105, 236), (122, 1)]);
        let d = decode_tdc(&b, 4).unwrap();
        assert_eq!(d, vec![(100, 5), (104, 1206), (120, 236), (188, 1)]);
        // four-byte count and skip
        let mut w = vec![0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0];
        w.extend_from_slice(&[0x7E, 0x01, 0x00, 0x01, 0x00, 0xFE, 0x02, 0, 0, 0, 0x03]);
        assert_eq!(decode_tdc(&w, 1).unwrap(), vec![(0, 65537), (3, 3)]);
        assert!(decode_tdc(&[0x05], 1).is_err());
        assert!(decode_tdc(&[0xFF, 0xFF, 0xFF, 0xFF, 1, 0, 0, 0, 0x7D, 1], 1).is_err());
        assert!(decode_tdc(&[0xFF, 0x00], 1).is_err());
        assert!(decode_tdc(&[0xFF, 0xFF, 0xFF, 0xFF, 1, 0, 0, 0, 0x7F], 1).is_err());
        let mut ex = vec![0u8; 0x40];
        ex[0x38] = 4;
        assert_eq!(tdc_step(&ex), 4);
        assert_eq!(tdc_step(&[0u8; 8]), 1);
    }

    #[test]
    fn index_and_misc() {
        let mut b = vec![0u8; STREAM_PREAMBLE + INDEX_RECORD + 3];
        let r = STREAM_PREAMBLE;
        b[r + 4..r + 8].copy_from_slice(&24u32.to_le_bytes());
        b[r + 8..r + 16].copy_from_slice(&580.0f64.to_le_bytes());
        b[r + 18..r + 26].copy_from_slice(&453.0f64.to_le_bytes());
        let ix = parse_index(&b);
        assert_eq!(ix.len(), 1);
        assert_eq!(ix[0].byte_len, 24);
        assert_eq!(ix[0].time_ms, 580.0);
        assert_eq!(ix[0].tic, 453.0);
        assert_eq!(index_trailing(b.len()), 3);
        assert!(
            (tof_mz(
                750_932.0,
                0.025,
                0.000_702_036_572_228_506,
                -14.112_704_257_181_66
            ) - 173.961_612_826)
                .abs()
                < 1e-6
        );
        let text = "Mass Spectrometer:QTRAP 6500+ Low Mass:0:,Component ID: QTRAP 6500+,Serial Number: DY1";
        let f = log_fields(text);
        assert!(f.contains(&("Component ID".into(), "QTRAP 6500+".into())));
        let mut w = vec![0u8; STREAM_PREAMBLE];
        w.extend_from_slice(&1u32.to_le_bytes());
        w.extend_from_slice(&9u32.to_le_bytes());
        assert_eq!(parse_windows(&w), vec![(1, 9)]);
    }

    #[test]
    fn mass_ranges() {
        let mut b = vec![0u8; STREAM_PREAMBLE];
        b.extend_from_slice(&2u32.to_le_bytes());
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&538.5f32.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&236.25f32.to_le_bytes());
        b.extend_from_slice(&7.5f32.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        let name: Vec<u8> = "Cer".encode_utf16().flat_map(u16::to_le_bytes).collect();
        b.extend_from_slice(&(name.len() as u16).to_le_bytes());
        b.extend_from_slice(&name);
        let key: Vec<u8> = "CE".encode_utf16().flat_map(u16::to_le_bytes).collect();
        b.extend_from_slice(&(key.len() as u16).to_le_bytes());
        b.extend_from_slice(&key);
        b.extend_from_slice(&37.5f32.to_le_bytes());
        b.extend_from_slice(&37.5f32.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        let r = parse_mass_ranges(&b, 5);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].first_mz, 538.5);
        assert_eq!(r[0].second_mz, 236.25);
        assert_eq!(r[0].name, "Cer");
        assert_eq!(r[0].parameter("ce"), Some(37.5));
        assert!(parse_mass_ranges(&b[..40], 5).is_empty());
    }
}
