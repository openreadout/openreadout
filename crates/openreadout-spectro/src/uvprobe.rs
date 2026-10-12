//! Shimadzu UVProbe `.spc` files (UV-Vis and UV-Vis-NIR spectrophotometers): a compound file
//! (OLE2) whose `DataStorage1/DataSetGroup/DataSet<n>` storages each hold one spectrum as two
//! float64 arrays (`X Data.1`, `Y Data.1`), a header naming the software, instrument and serial
//! number, and the method as text pages. Notes: `docs/formats/shimadzu-uvprobe.md`; provenance:
//! `docs/provenance/shimadzu-uvprobe.md`. Not Galactic SPC, which shares the extension.

use std::collections::BTreeMap;
use std::path::Path;

use openreadout_core::cfb::Cfb;
use openreadout_core::jet::ole_date_iso;
use openreadout_core::model::{Finding, LsEntry};
use openreadout_core::provenance::Source;
use openreadout_core::source::{Input, SourceFile};
use openreadout_core::{Error, Result};
use serde_json::{Map, Value, json};

use crate::common::{Facts, Parsed, Rows, SpectrumSet, Stored, XValues};

const FMT: &str = crate::UVPROBE_FORMAT_ID;
/// The storage that holds the data sets.
const GROUP: &str = "DataStorage1/DataSetGroup";
/// Streams larger than this are not read (a spectrum of 10 M points is far beyond any scan).
const MAX_STREAM: u64 = 80 << 20;
/// Data sets looked for (`DataSet1` …).
pub(crate) const MAX_DATA_SETS: u32 = 256;

/// A parsed file and the y values of its data sets, one after another.
pub(crate) struct Opened {
    pub(crate) parsed: Parsed,
    pub(crate) values: Vec<u8>,
}

/// Whether a compound file is a UVProbe file: it has the first data set's x array.
pub(crate) fn is_uvprobe(input: &Input) -> bool {
    let Ok(mut f) = input.open() else {
        return false;
    };
    Cfb::open(&mut f, input.path(), FMT).is_ok_and(|c| {
        c.stream(&format!(
            "{GROUP}/DataSet1/DataSpectrumStorage/Data/X Data.1"
        ))
        .is_some()
    })
}

/// A one-byte-length text at `at` and the offset after it: UTF-8, else Latin-1.
fn pstr(b: &[u8], at: usize) -> Option<(String, usize)> {
    let n = usize::from(*b.get(at)?);
    let s = b.get(at + 1..at + 1 + n)?;
    let text = match std::str::from_utf8(s) {
        Ok(t) => t.to_string(),
        Err(_) => openreadout_core::bytes::latin1(s),
    };
    Some((text.trim_end_matches('\0').to_string(), at + 1 + n))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn f64_at(b: &[u8], at: usize) -> Option<f64> {
    Some(f64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

/// `DataSetHeaderInfo`: data set name, creation date, organization, software and version,
/// instrument model and series, a second model text and the serial number.
#[derive(Debug, Default)]
struct HeaderInfo {
    name: String,
    created: Option<f64>,
    organization: String,
    software: String,
    version: String,
    model: String,
    series: String,
    instrument: String,
    serial: String,
}

fn header_info(h: &[u8]) -> Option<HeaderInfo> {
    let (name, at) = pstr(h, 0)?;
    let created = f64_at(h, at + 4)?;
    let (organization, at) = pstr(h, at + 4 + 8 + 6)?;
    let (software, at) = pstr(h, at + 8)?;
    let (version, at) = pstr(h, at)?;
    let (model, at) = pstr(h, at)?;
    let (series, at) = pstr(h, at)?;
    let (instrument, at) = pstr(h, at + 4)?;
    let (serial, _) = pstr(h, at)?;
    Some(HeaderInfo {
        name,
        created: Some(created),
        organization,
        software,
        version,
        model,
        series,
        instrument,
        serial,
    })
}

/// A method page (`PageTexts<n>`): two u32, a title, a u32 count, then label and value texts.
fn method_page(b: &[u8]) -> Option<(String, Vec<(String, String)>)> {
    let (title, at) = pstr(b, 8)?;
    let n = u32_at(b, at)?;
    let mut at = at + 4;
    let mut items = Vec::new();
    for _ in 0..n.min(256) {
        let (label, next) = pstr(b, at)?;
        let (value, next) = pstr(b, next)?;
        items.push((label, value));
        at = next;
    }
    Some((title, items))
}

/// What a value code means: (channel name, unit).
fn value_kind(code: u32) -> Option<(&'static str, Option<&'static str>)> {
    match code {
        2 => Some(("absorbance", None)),
        4 => Some(("reflectance", Some("%"))),
        _ => None,
    }
}

/// Parse a UVProbe file.
pub(crate) fn parse(f: &SourceFile, path: &Path) -> Result<Opened> {
    let mut file = f.clone();
    let cfb = Cfb::open(&mut file, path, FMT)?;
    let mut read = |name: &str| -> Option<Vec<u8>> {
        let e = cfb.stream(name)?.clone();
        cfb.read(&mut file, path, &e, MAX_STREAM).ok()
    };
    let mut parsed = Parsed::default();
    let mut facts = Facts::default();
    let mut vendor = Map::new();
    let mut values = Vec::new();
    let mut sets = Vec::new();
    let declared = read(&format!("{GROUP}/DataSetGroupHeaderInfo")).and_then(|b| u32_at(&b, 0));
    for n in 1..=MAX_DATA_SETS {
        let ds = format!("{GROUP}/DataSet{n}");
        let data = format!("{ds}/DataSpectrumStorage/Data");
        if cfb.stream(&format!("{data}/X Data.1")).is_none() {
            break;
        }
        if cfb.stream(&format!("{data}/Data Header.2")).is_some() {
            parsed.findings.push(Finding::warning(
                "unsupported_data_set",
                format!("data set {n} holds more than one spectrum (`.2` arrays): only the first is read"),
            ));
        }
        let dh = read(&format!("{data}/Data Header.1"))
            .ok_or_else(|| Error::corrupt(FMT, format!("data set {n}: no `Data Header.1`")))?;
        let (Some(points), Some(code)) = (u32_at(&dh, 0), u32_at(&dh, 4)) else {
            return Err(Error::corrupt(
                FMT,
                format!("data set {n}: `Data Header.1` is {} bytes", dh.len()),
            ));
        };
        let x = read(&format!("{data}/X Data.1")).unwrap_or_default();
        let y = read(&format!("{data}/Y Data.1"))
            .ok_or_else(|| Error::corrupt(FMT, format!("data set {n}: no `Y Data.1`")))?;
        let want = u64::from(points) * 8;
        if x.len() as u64 != want || y.len() as u64 != want {
            return Err(Error::corrupt(
                FMT,
                format!(
                    "data set {n}: {points} points need {want} bytes; `X Data.1` holds {}, `Y Data.1` {}",
                    x.len(),
                    y.len()
                ),
            ));
        }
        let xs: Vec<f64> = x
            .as_chunks::<8>()
            .0
            .iter()
            .map(|c| f64::from_le_bytes(*c))
            .collect();
        // the header's first and last x and step; a regular axis when the array follows them
        let hi =
            read(&format!("{ds}/DataSpectrumStorage/DataHeader/Header Info")).unwrap_or_default();
        let (first, last, step) = (f64_at(&hi, 0), f64_at(&hi, 8), f64_at(&hi, 16));
        let regular = match (xs.first(), xs.last(), first, last) {
            (Some(&a), Some(&b), Some(fa), Some(fb)) if xs.len() > 1 => {
                let d = (b - a) / (xs.len() - 1) as f64;
                let tol = 1e-9 * a.abs().max(1.0);
                (a - fa).abs() <= tol
                    && (b - fb).abs() <= tol
                    && xs
                        .iter()
                        .enumerate()
                        .all(|(i, v)| (v - (a + d * i as f64)).abs() <= tol)
            }
            _ => false,
        };
        let xv = if regular {
            XValues::Regular {
                first: xs[0],
                last: xs[xs.len() - 1],
            }
        } else {
            XValues::Listed(xs.clone())
        };
        let info = read(&format!("{ds}/DataSetHeaderInfo")).and_then(|h| header_info(&h));
        let mut extra = BTreeMap::new();
        extra.insert("value_code".into(), json!(code));
        extra.insert("data_set".into(), json!(n));
        let mut pages = Vec::new();
        for p in 0..16 {
            let Some(b) = read(&format!("{ds}/MethodStorage/PageTexts{p}")) else {
                break;
            };
            if let Some((title, items)) = method_page(&b) {
                pages.push(json!({
                    "title": title,
                    "items": items.iter().map(|(l, v)| json!([l, v])).collect::<Vec<_>>(),
                }));
            }
        }
        let (name, unit) = value_kind(code).unwrap_or_else(|| {
            parsed.findings.push(Finding::warning(
                "unknown_value_code",
                format!("data set {n}: value code {code} has not been seen with a known measuring mode; the values are returned unnamed"),
            ));
            ("y", None)
        });
        if let Some(h) = &info {
            if n == 1 {
                Facts::text(
                    &mut facts.software,
                    &h.software,
                    "DataSetHeaderInfo software",
                );
                Facts::text(
                    &mut facts.software_version,
                    &h.version,
                    "DataSetHeaderInfo version",
                );
                Facts::text(&mut facts.model, &h.model, "DataSetHeaderInfo model");
                Facts::text(
                    &mut facts.serial,
                    &h.serial,
                    "DataSetHeaderInfo serial number",
                );
                if let Some(t) = h.created.and_then(ole_date_iso) {
                    facts.started_at =
                        Some((t, "DataSetHeaderInfo creation date (local time)".into()));
                }
            }
            extra.insert("data_set_name".into(), json!(h.name));
            vendor.insert(
                format!("data_set_{n}"),
                json!({
                    "name": h.name, "created": h.created.and_then(ole_date_iso),
                    "organization": h.organization, "software": h.software, "version": h.version,
                    "model": h.model, "series": h.series, "instrument": h.instrument,
                    "serial": h.serial, "points": points, "value_code": code,
                    "header_first_x": first, "header_last_x": last, "header_step": step,
                    "method_pages": pages,
                }),
            );
        } else {
            parsed.findings.push(Finding::warning(
                "unreadable_header_info",
                format!("data set {n}: `DataSetHeaderInfo` does not parse: software, instrument and date are unknown"),
            ));
        }
        sets.push(SpectrumSet {
            name: info
                .as_ref()
                .map(|h| h.name.clone())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| format!("data set {n}")),
            x_quantity: "wavelength",
            x_unit: Some("nm".into()),
            x: xv,
            y_name: name.into(),
            y_unit: unit.map(str::to_string),
            points: u64::from(points),
            count: 1,
            rows: Rows::Listed(vec![values.len() as u64]),
            stored: Stored::F64,
            scale: 1.0,
            data_type: "UV/VIS SPECTRUM",
            extra,
        });
        values.extend_from_slice(&y);
    }
    if sets.is_empty() {
        return Err(Error::unsupported(
            FMT,
            "a compound file without UVProbe's `DataSet1` spectrum",
            "Only Shimadzu UVProbe spectrum files are read; this compound file is something else.",
        ));
    }
    if let Some(d) = declared
        && d as usize != sets.len()
    {
        parsed.findings.push(Finding::warning(
            "data_set_count",
            format!(
                "`DataSetGroupHeaderInfo` declares {d} data set(s); {} found",
                sets.len()
            ),
        ));
    }
    facts.vendor = Some(("Shimadzu".into(), "file format".into()));
    parsed.format_version = read("Version/File Format Version")
        .and_then(|b| u32_at(&b, 0))
        .map(|v| format!("file format {v:#x}"));
    vendor.insert(
        "streams".into(),
        json!(
            cfb.entries
                .iter()
                .filter(|e| e.is_stream)
                .map(|e| json!({"path": e.path, "size": e.size}))
                .collect::<Vec<_>>()
        ),
    );
    for e in cfb.entries.iter().filter(|e| e.is_stream) {
        parsed.entries.push(LsEntry {
            kind: "stream".into(),
            name: e.path.clone(),
            offset: None,
            size: Some(e.size),
            image: None,
            details: Value::Null,
        });
    }
    for p in &cfb.problems {
        parsed
            .findings
            .push(Finding::warning("compound_file", p.clone()));
    }
    parsed.sets = sets;
    parsed.facts = facts;
    parsed.vendor = json!({ "uvprobe": Value::Object(vendor) });
    parsed.notes.push(
        "Shimadzu UVProbe spectrum file: values as stored (float64); the date is when UVProbe created the data set, in the PC's local time".into(),
    );
    parsed.provenance.insert("traces".into(), Source::Inferred);
    Ok(Opened { parsed, values })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_info_layout() {
        let mut h = vec![7u8];
        h.extend_from_slice(b"RawData");
        h.extend_from_slice(&[0; 4]);
        h.extend_from_slice(&45853.5f64.to_le_bytes());
        h.extend_from_slice(&[0; 6]);
        h.push(3);
        h.extend_from_slice(b"Org");
        h.extend_from_slice(&[0; 8]);
        for s in ["UVProbe", "2.43", "UV-3600Plus", "UV-3600Plus Series"] {
            h.push(s.len() as u8);
            h.extend_from_slice(s.as_bytes());
        }
        h.extend_from_slice(&1u32.to_le_bytes());
        for s in ["UV-3600Plus", "A12015200066"] {
            h.push(s.len() as u8);
            h.extend_from_slice(s.as_bytes());
        }
        let i = header_info(&h).unwrap();
        assert_eq!(
            (i.name.as_str(), i.software.as_str(), i.version.as_str()),
            ("RawData", "UVProbe", "2.43")
        );
        assert_eq!(
            (i.model.as_str(), i.serial.as_str()),
            ("UV-3600Plus", "A12015200066")
        );
        assert_eq!(
            i.created.and_then(ole_date_iso).as_deref(),
            Some("2025-07-15T12:00:00")
        );
        assert!(header_info(&h[..20]).is_none());
    }

    #[test]
    fn method_pages_and_codes() {
        let mut b = vec![1, 0, 0, 0, 3, 0, 0, 0];
        b.push(5);
        b.extend_from_slice(b"[Ins]");
        b.extend_from_slice(&1u32.to_le_bytes());
        b.push(5);
        b.extend_from_slice(b"Mode:");
        b.push(3);
        b.extend_from_slice(b"Abs");
        let (t, items) = method_page(&b).unwrap();
        assert_eq!(t, "[Ins]");
        assert_eq!(items, vec![("Mode:".to_string(), "Abs".to_string())]);
        assert_eq!(value_kind(2), Some(("absorbance", None)));
        assert_eq!(value_kind(4), Some(("reflectance", Some("%"))));
        assert_eq!(value_kind(3), None);
    }
}
