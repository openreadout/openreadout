//! ChemStation signal-file headers: version, body encoding, strings and numeric fields at fixed
//! offsets. Layout and derivation: `docs/formats/chemstation.md`, `docs/provenance/chemstation.md`.

use openreadout_core::bytes::{be_f32, be_f64, be_i32, be_u16, be_u32, latin1, le_u16, utf16le};
use openreadout_core::time::{full_year, month_from_abbrev};
use serde_json::{Map, Value, json};

use crate::binary::{iso, pascal8, pascal16, tidy};

/// How the body after the header is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyEncoding {
    /// Versions 30 and 130: records `0x10, n` of `n` big-endian i16 first differences;
    /// `0x8000` introduces a big-endian i32 absolute value.
    DeltaRecords,
    /// Versions 81 and 181: big-endian i16 second differences; `0x7FFF` introduces a 48-bit
    /// big-endian absolute value and resets the running difference.
    SecondDifference,
    /// Version 179: little-endian f64 values.
    Float64,
    /// Version 131 (`.uv`): one record per scan of little-endian first differences over a
    /// wavelength range.
    SpectrumRecords,
    /// Version 2 (`.ms`): one record per scan of big-endian m/z–intensity pairs.
    MassSpectrumRecords,
    /// A version we list but do not decode.
    NotDecoded,
}

impl BodyEncoding {
    /// Our name for the encoding (JSON).
    pub fn name(self) -> &'static str {
        match self {
            BodyEncoding::DeltaRecords => "delta_records",
            BodyEncoding::SecondDifference => "second_difference",
            BodyEncoding::Float64 => "float64",
            BodyEncoding::SpectrumRecords => "spectrum_records",
            BodyEncoding::MassSpectrumRecords => "mass_spectrum_records",
            BodyEncoding::NotDecoded => "not_decoded",
        }
    }
}

/// What a signal file holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalKind {
    /// One detector channel over time (`.ch`).
    Chromatogram,
    /// Absorbance spectra over time (`.uv`).
    Spectra,
    /// Mass spectra over time (`.ms`).
    MassSpectra,
    /// Anything else with a ChemStation version string (`.reg` registers, ...).
    Other,
}

/// Versions whose body we decode.
pub const DECODED_VERSIONS: &[&str] = &["2", "30", "81", "130", "131", "179", "181"];

/// Parsed header of one ChemStation file.
#[derive(Debug, Clone)]
pub struct SignalHeader {
    /// Version string at byte 0 (`81`, `179`, ...).
    pub version: String,
    pub kind: SignalKind,
    pub encoding: BodyEncoding,
    /// Where the body starts.
    pub header_len: u64,
    /// `GC DATA FILE`, `LC DATA FILE`, `MSD Spectral File`, `GC / MS Data File`.
    pub file_type: Option<String>,
    pub sample_name: Option<String>,
    pub description: Option<String>,
    pub operator: Option<String>,
    /// Acquisition date/time as written.
    pub acquired_text: Option<String>,
    /// `acquired_text` as ISO-8601, when it parses.
    pub acquired_at: Option<String>,
    /// Instrument model or module string (`HP G1530A`, `G1365B`, `GCI`, `DAD1`).
    pub instrument_model: Option<String>,
    /// `GC` or `LC`.
    pub separation: Option<String>,
    pub method: Option<String>,
    /// Instrument name as configured in the software (`Asterix ChemStation`).
    pub instrument_name: Option<String>,
    pub software: Option<String>,
    pub software_revision: Option<String>,
    /// Y-axis unit (`pA`, `mAU`, ...).
    pub units: Option<String>,
    /// Signal description (`FID1A, Front Signal`, `MWD A, Sig=210,5 Ref=360,100`, MS scan range).
    pub signal: Option<String>,
    pub vial: Option<u16>,
    pub sequence_line: Option<u16>,
    pub replicate: Option<u16>,
    /// First and last retention time in the header, milliseconds.
    pub first_time_ms: Option<f64>,
    pub last_time_ms: Option<f64>,
    /// `value = raw * scale + offset`.
    pub scale: f64,
    pub offset: f64,
    /// Scan count written in the header (`.uv`, `.ms`).
    pub declared_records: Option<u64>,
    /// Every string found in the header: (offset, our name or `""`, value).
    pub strings: Vec<(usize, &'static str, String)>,
}

/// Parse the version string at byte 0: a length byte (1–3) followed by ASCII digits.
pub fn version_of(head: &[u8]) -> Option<String> {
    let n = *head.first()? as usize;
    if !(1..=3).contains(&n) {
        return None;
    }
    let digits = head.get(1..=n)?;
    if !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    Some(String::from_utf8_lossy(digits).into_owned())
}

/// Does `head` start like a ChemStation data file? Checks the version string and, for versions
/// below 100, the file-type string at 0x04 or, for 1xx, the version repeated at 0x146.
pub fn looks_like_chemstation(head: &[u8]) -> Option<String> {
    let v = version_of(head)?;
    let num: u32 = v.parse().ok()?;
    if num < 100 {
        // 1-byte padding after the version, then a length-prefixed type string at 0x04
        let t = pascal8(head, 4)?;
        let known = ["MSD Spectral File", "GC / MS Data File"];
        if known.iter().any(|k| t.eq_ignore_ascii_case(k)) || t.ends_with("DATA FILE") {
            return Some(v);
        }
        // `.reg` files carry the version and no type string: accept only a zero byte at 0x03
        if head.len() > 8 && head.get(v.len() + 1) == Some(&0) {
            return Some(v);
        }
        return None;
    }
    // new layout: the version repeated as UTF-16 at 0x146
    if pascal16(head, 0x146).is_some_and(|s| s == v) {
        return Some(v);
    }
    None
}

fn kind_and_encoding(version: &str) -> (SignalKind, BodyEncoding) {
    match version {
        "30" | "130" => (SignalKind::Chromatogram, BodyEncoding::DeltaRecords),
        "81" | "181" => (SignalKind::Chromatogram, BodyEncoding::SecondDifference),
        "179" => (SignalKind::Chromatogram, BodyEncoding::Float64),
        "131" => (SignalKind::Spectra, BodyEncoding::SpectrumRecords),
        "2" => (SignalKind::MassSpectra, BodyEncoding::MassSpectrumRecords),
        _ => (SignalKind::Other, BodyEncoding::NotDecoded),
    }
}

/// Parse a ChemStation date string to ISO-8601. Seen: `18-Nov-10, 15:48:06`,
/// `8/20/20 2:00:32 PM`, `28 Jun 13  10:59 am -0500`, `17 Dec 19  10:04 am`.
pub fn parse_date(text: &str) -> Option<String> {
    let mut tokens: Vec<&str> = text.split([' ', ',']).filter(|t| !t.is_empty()).collect();
    let mut zone = None;
    if let Some(last) = tokens.last()
        && last.len() == 5
        && (last.starts_with('+') || last.starts_with('-'))
        && last[1..].chars().all(|c| c.is_ascii_digit())
    {
        let hh: i32 = last[1..3].parse().ok()?;
        let mm: i32 = last[3..5].parse().ok()?;
        let m = hh * 60 + mm;
        zone = Some(if last.starts_with('-') { -m } else { m });
        tokens.pop();
    }
    let mut pm = None;
    if let Some(last) = tokens.last() {
        match last.to_ascii_lowercase().as_str() {
            "am" => pm = Some(false),
            "pm" => pm = Some(true),
            _ => {}
        }
        if pm.is_some() {
            tokens.pop();
        }
    }
    let time_tok = tokens.iter().position(|t| t.contains(':'))?;
    let time = tokens[time_tok];
    let date: Vec<&str> = tokens[..time_tok].to_vec();
    let (year, month, day) = if date.len() == 1 && date[0].contains('-') {
        let parts: Vec<&str> = date[0].split('-').collect();
        if parts.len() != 3 {
            return None;
        }
        (
            parts[2].parse().ok()?,
            month_from_abbrev(parts[1].get(..3)?)?,
            parts[0].parse().ok()?,
        )
    } else if date.len() == 1 && date[0].contains('/') {
        let parts: Vec<&str> = date[0].split('/').collect();
        if parts.len() != 3 {
            return None;
        }
        (
            parts[2].parse().ok()?,
            parts[0].parse().ok()?,
            parts[1].parse().ok()?,
        )
    } else if date.len() == 3 {
        (
            date[2].parse().ok()?,
            month_from_abbrev(date[1].get(..3)?)?,
            date[0].parse().ok()?,
        )
    } else {
        return None;
    };
    let clock: Vec<&str> = time.split(':').collect();
    let mut hour: u32 = clock.first()?.parse().ok()?;
    let minute: u32 = clock.get(1)?.parse().ok()?;
    let second: u32 = clock.get(2).map_or(Some(0), |x| x.parse().ok())?;
    match pm {
        Some(true) if hour < 12 => hour += 12,
        Some(false) if hour == 12 => hour = 0,
        _ => {}
    }
    iso(full_year(year), month, day, hour, minute, second, zone)
}

/// Offsets of single-byte strings in versions below 100 (our names).
const LEGACY_STRINGS: &[(usize, &str)] = &[
    (0x04, "file_type"),
    (0x18, "sample_name"),
    (0x56, "description"),
    (0x94, "operator"),
    (0xB2, "acquired_text"),
    (0xD0, "instrument_model"),
    (0xDA, "separation"),
    (0xE4, "method"),
    (0x140, "signal"),
    (0x244, "units"),
    (0x254, "signal"),
];

/// Offsets of UTF-16 strings in versions 130, 131, 179, 181 (our names).
const UNICODE_STRINGS: &[(usize, &str)] = &[
    (0x146, "version"),
    (0x15B, "file_type"),
    (0x35A, "sample_name"),
    (0x758, "operator"),
    (0x957, "acquired_text"),
    (0x9BC, "instrument_model"),
    (0x9E5, "separation"),
    (0xA0E, "method"),
    (0xC11, "instrument_name"),
    (0xC15, "units"),
    (0xE11, "software"),
    (0xEDA, "software_revision"),
    (0xFD7, "vial_text"),
    (0x104C, "units"),
    (0x1075, "signal"),
];

fn printable(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| !c.is_control())
}

/// Scan a header for further length-prefixed strings not at a known offset.
fn scan_strings(b: &[u8], wide: bool, known: &[(usize, &str)]) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut i = 1usize;
    while i < b.len() {
        let n = b[i] as usize;
        let prev_zero = b[i - 1] == 0;
        if !(2..=120).contains(&n) || !prev_zero || known.iter().any(|(o, _)| *o == i) {
            i += 1;
            continue;
        }
        let (bytes_len, ok) = if wide {
            let Some(s) = b.get(i + 1..i + 1 + 2 * n) else {
                break;
            };
            let ok = s
                .as_chunks::<2>()
                .0
                .iter()
                .all(|c| (0x20..0x7F).contains(&c[0]) && c[1] == 0);
            (2 * n, ok)
        } else {
            let Some(s) = b.get(i + 1..i + 1 + n) else {
                break;
            };
            (n, s.iter().all(|c| (0x20..0x7F).contains(c)))
        };
        let after_zero = b.get(i + 1 + bytes_len).is_none_or(|&c| c == 0);
        if ok && after_zero {
            let s = if wide {
                pascal16(b, i).unwrap_or_default()
            } else {
                pascal8(b, i).unwrap_or_default()
            };
            if s.len() >= 2 {
                out.push((i, s));
            }
            i += 1 + bytes_len;
        } else {
            i += 1;
        }
    }
    out
}

impl SignalHeader {
    /// Parse the header from the first bytes of a file (at least the header length, or the
    /// whole file when shorter). `None` when the version string is missing.
    pub fn parse(b: &[u8]) -> Option<SignalHeader> {
        let version = version_of(b)?;
        let num: u32 = version.parse().ok()?;
        let (kind, encoding) = kind_and_encoding(&version);
        let wide = num >= 100;
        let header_len = if encoding == BodyEncoding::MassSpectrumRecords {
            // length in 16-bit words at 0x10A (257 → 512 bytes)
            be_u16(b, 0x10A).map_or(0, |w| 2 * u64::from(w.saturating_sub(1)))
        } else {
            // 512-byte blocks at 0x108 (3 → 1024, 13 → 6144)
            be_u32(b, 0x108).map_or(0, |n| 512 * u64::from(n.saturating_sub(1)))
        };
        let mut h = SignalHeader {
            version: version.clone(),
            kind,
            encoding,
            header_len,
            file_type: None,
            sample_name: None,
            description: None,
            operator: None,
            acquired_text: None,
            acquired_at: None,
            instrument_model: None,
            separation: None,
            method: None,
            instrument_name: None,
            software: None,
            software_revision: None,
            units: None,
            signal: None,
            vial: None,
            sequence_line: None,
            replicate: None,
            first_time_ms: None,
            last_time_ms: None,
            scale: 1.0,
            offset: 0.0,
            declared_records: None,
            strings: Vec::new(),
        };
        let table = if wide {
            UNICODE_STRINGS
        } else {
            LEGACY_STRINGS
        };
        let limit = usize::try_from(header_len)
            .unwrap_or(usize::MAX)
            .min(b.len());
        let hb = &b[..limit.max(b.len().min(0x200))];
        for &(off, name) in table {
            // offsets that only apply to some versions
            let applies = match (name, off) {
                ("units", 0xC15) => version == "131",
                ("units", 0x104C) | ("signal", 0x1075) => version != "131",
                ("signal", 0x140) => kind == SignalKind::MassSpectra,
                ("signal", 0x254) | ("units", 0x244) => kind != SignalKind::MassSpectra,
                _ => true,
            };
            if !applies || off >= hb.len() {
                continue;
            }
            let s = if wide {
                pascal16(hb, off)
            } else {
                pascal8(hb, off)
            };
            let Some(s) = s.filter(|s| printable(s)) else {
                continue;
            };
            h.strings.push((off, name, s.clone()));
            let slot = match name {
                "file_type" => &mut h.file_type,
                "sample_name" => &mut h.sample_name,
                "description" => &mut h.description,
                "operator" => &mut h.operator,
                "acquired_text" => &mut h.acquired_text,
                "instrument_model" => &mut h.instrument_model,
                "separation" => &mut h.separation,
                "method" => &mut h.method,
                "instrument_name" => &mut h.instrument_name,
                "software" => &mut h.software,
                "software_revision" => &mut h.software_revision,
                "units" => &mut h.units,
                "signal" => &mut h.signal,
                _ => continue,
            };
            if slot.is_none() {
                *slot = Some(s);
            }
        }
        for (off, s) in scan_strings(hb, wide, table) {
            if !h.strings.iter().any(|(o, _, _)| *o == off) {
                h.strings.push((off, "", s));
            }
        }
        h.strings.sort_by_key(|(o, _, _)| *o);
        h.acquired_at = h.acquired_text.as_deref().and_then(parse_date);
        h.sequence_line = be_u16(b, 0xFC);
        h.vial = be_u16(b, 0xFE);
        h.replicate = be_u16(b, 0x100);
        let (t0, t1) = match version.as_str() {
            "81" | "179" | "181" => (
                be_f32(b, 0x11A).map(f64::from),
                be_f32(b, 0x11E).map(f64::from),
            ),
            "30" | "130" => (
                be_i32(b, 0x11A).map(f64::from),
                be_i32(b, 0x11E).map(f64::from),
            ),
            _ => (None, None),
        };
        h.first_time_ms = t0.filter(|v| v.is_finite()).map(tidy);
        h.last_time_ms = t1.filter(|v| v.is_finite()).map(tidy);
        match version.as_str() {
            "30" | "81" => {
                h.offset = be_f64(b, 0x27C).filter(|v| v.is_finite()).unwrap_or(0.0);
                h.scale = be_f64(b, 0x284)
                    .filter(|v| v.is_finite() && *v != 0.0)
                    .unwrap_or(1.0);
            }
            "130" | "179" | "181" => {
                h.scale = be_f64(b, 0x127C)
                    .filter(|v| v.is_finite() && *v != 0.0)
                    .unwrap_or(1.0);
            }
            "131" => {
                h.scale = be_f64(b, 0xC0D)
                    .filter(|v| v.is_finite() && *v != 0.0)
                    .unwrap_or(1.0);
                h.declared_records = be_u32(b, 0x116).map(u64::from);
            }
            "2" => {
                let gcms = h
                    .file_type
                    .as_deref()
                    .is_some_and(|t| t.to_ascii_uppercase().starts_with("GC"));
                h.declared_records = if gcms {
                    le_u16(b, 0x142).map(u64::from)
                } else {
                    be_u32(b, 0x116).map(u64::from)
                };
            }
            _ => {}
        }
        Some(h)
    }

    /// Detector wavelength and bandwidth from a `Sig=210,5` clause, and the reference from
    /// `Ref=360,100` (`Ref=off` gives none).
    pub fn wavelengths(&self) -> (Option<[f64; 2]>, Option<[f64; 2]>) {
        let Some(s) = self.signal.as_deref() else {
            return (None, None);
        };
        let grab = |key: &str| -> Option<[f64; 2]> {
            let i = s.find(key)? + key.len();
            let rest: String = s[i..]
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == ',')
                .collect();
            let mut p = rest.split(',').filter(|x| !x.is_empty());
            let w: f64 = p.next()?.parse().ok()?;
            let bw: f64 = p.next().and_then(|x| x.parse().ok()).unwrap_or(f64::NAN);
            Some([w, bw])
        };
        (grab("Sig="), grab("Ref="))
    }

    /// The header as JSON: every string with its offset, and the numeric fields.
    pub fn to_json(&self) -> Value {
        let mut strings = Map::new();
        for (off, name, s) in &self.strings {
            let key = if name.is_empty() {
                format!("0x{off:04X}")
            } else {
                format!("0x{off:04X} {name}")
            };
            strings.insert(key, Value::String(s.clone()));
        }
        json!({
            "version": self.version,
            "encoding": self.encoding.name(),
            "header_len": self.header_len,
            "strings": strings,
            "numbers": {
                "0x00FC sequence_line": self.sequence_line,
                "0x00FE vial": self.vial,
                "0x0100 replicate": self.replicate,
                "first_time_ms": self.first_time_ms,
                "last_time_ms": self.last_time_ms,
                "scale": self.scale,
                "offset": self.offset,
                "declared_records": self.declared_records,
            }
        })
    }
}

/// Human-readable Latin-1 text of a small text file (for `vendor`); `None` for binary content.
pub(crate) fn text_preview(b: &[u8], limit: usize) -> Option<String> {
    // UTF-16LE with BOM
    let t = if b.starts_with(&[0xFF, 0xFE]) {
        utf16le(&b[2..])
    } else if b.len() > 1 && b.iter().skip(1).step_by(2).take(64).all(|&c| c == 0) {
        utf16le(b)
    } else {
        let sample = &b[..b.len().min(4096)];
        let bad = sample
            .iter()
            .filter(|&&c| c < 0x09 || (c > 0x0D && c < 0x20))
            .count();
        if bad > sample.len() / 50 {
            return None;
        }
        latin1(b)
    };
    let t = t.replace('\r', "");
    Some(if t.len() > limit {
        let mut cut = limit;
        while !t.is_char_boundary(cut) {
            cut -= 1;
        }
        format!("{}…", &t[..cut])
    } else {
        t
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(
            parse_date("18-Nov-10, 15:48:06").as_deref(),
            Some("2010-11-18T15:48:06")
        );
        assert_eq!(
            parse_date("8/20/20 2:00:32 PM").as_deref(),
            Some("2020-08-20T14:00:32")
        );
        assert_eq!(
            parse_date("28 Jun 13  10:59 am -0500").as_deref(),
            Some("2013-06-28T10:59:00-05:00")
        );
        assert_eq!(
            parse_date("17 Dec 19  12:04 am").as_deref(),
            Some("2019-12-17T00:04:00")
        );
        assert_eq!(parse_date("garbage"), None);
    }

    #[test]
    fn versions() {
        assert_eq!(version_of(b"\x0281\x00").as_deref(), Some("81"));
        assert_eq!(version_of(b"\x03179").as_deref(), Some("179"));
        assert_eq!(version_of(b"\x04abcd"), None);
        let mut h = vec![0u8; 0x200];
        h[..3].copy_from_slice(b"\x0230");
        h[4] = 12;
        h[5..17].copy_from_slice(b"LC DATA FILE");
        assert_eq!(looks_like_chemstation(&h).as_deref(), Some("30"));
    }

    #[test]
    fn signal_wavelengths() {
        let mut h = SignalHeader::parse(b"\x0230\x00").unwrap();
        h.signal = Some("MWD A, Sig=210,5 Ref=360,100".into());
        assert_eq!(h.wavelengths(), (Some([210.0, 5.0]), Some([360.0, 100.0])));
        h.signal = Some("DAD1A, Sig=215.0,16.0  Ref=off".into());
        let (s, r) = h.wavelengths();
        assert_eq!(s, Some([215.0, 16.0]));
        assert!(r.is_none());
    }
}
