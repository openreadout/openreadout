//! HORIBA LabSpec 6 files (`.l6s` spectra, `.l6m` maps; `docs/formats/horiba-labspec.md`): the
//! `LabSpec6` signature, then a tree of 24-byte records (type, id, key, 4 more bytes, an 8-byte
//! value). A float32 array record (type 0x1006, key `db d4 6c 7d`) holds the number of values;
//! the values follow the run of records written after it. Its `typm` and `uni|` text records
//! (in the run after it, or else the run before it) name the axis (`Intens`, `Spectr`, `X`,
//! `Y`) and its unit (`Cnt`, `1/cm`, `nm`, `µm`).
//!
//! The first `Intens` array and the axes that follow it are the data set: one spectrum, or a
//! map of `X` × `Y` spectra. Further data objects in the file are listed, not read.

use std::collections::BTreeMap;
use std::path::Path;

use openreadout_core::bytes::{le_f32, le_u32, latin1, until_nul};
use openreadout_core::model::{Finding, LsEntry};
use openreadout_core::provenance::Source;
use openreadout_core::source::SourceFile;
use openreadout_core::{Error, Result};
use serde_json::{Value, json};

use crate::common::{
    Facts, MapSpec, Parsed, Rows, SpectrumSet, SpectrumTable, Stored, XValues, read_at,
};

pub(crate) const MAGIC: &[u8; 8] = b"LabSpec6";
const FORMAT: &str = crate::LABSPEC_FORMAT_ID;
/// Largest file read (maps of tens of MB are common).
const MAX_BYTES: u64 = 1 << 30;
/// Bytes of one record.
const RECORD: usize = 24;
/// Key of a float32 array record.
const ARRAY_KEY: [u8; 4] = [0xdb, 0xd4, 0x6c, 0x7d];
/// Record type of a float32 array.
const ARRAY: u32 = 0x1006;
/// Record type of an inline text value (up to 8 bytes).
const TEXT8: u32 = 0x0007;
/// Record types seen in the corpus files; a run of records stops at anything else.
const KNOWN: [u32; 12] = [
    0x0002, 0x0003, 0x0004, 0x0005, 0x0006, 0x0007, 0x0008, 0x0009, 0x1006, 0x1007, 0x1008, 0x1009,
];
/// Values of the header word at byte 12 whose files are read; the corpus files with 1 have an
/// encoded body.
const PLAIN_FLAGS: [u32; 2] = [0, 8];

/// A float32 array and what names it.
#[derive(Debug, Clone)]
struct Array {
    header: usize,
    count: usize,
    values: usize,
    /// The `typm` text (`Intens`, `Spectr`, `X`, `Y`).
    axis: String,
    /// The `uni|` text (`Cnt`, `1/cm`, `µm`).
    unit: String,
}

fn record_type(b: &[u8], at: usize) -> Option<u32> {
    le_u32(b, at).filter(|t| KNOWN.contains(t) && at + RECORD <= b.len())
}

/// An inline text record's key and text.
fn text_record(b: &[u8], at: usize) -> Option<(&[u8], String)> {
    if le_u32(b, at)? != TEXT8 {
        return None;
    }
    let key = b.get(at + 8..at + 12)?;
    let v = until_nul(b.get(at + 16..at + 24)?);
    let text = match std::str::from_utf8(v) {
        Ok(s) => s.to_string(),
        Err(_) => latin1(v),
    };
    Some((key, text.trim().to_string()))
}

/// Labels found in the records from `start` stepping by `step` while the records are known.
fn labels(b: &[u8], start: usize, forward: bool) -> (String, String, usize) {
    let (mut axis, mut unit) = (String::new(), String::new());
    let mut at = start;
    loop {
        if record_type(b, at).is_none() {
            break;
        }
        if let Some((k, t)) = text_record(b, at) {
            if k == b"typm" && axis.is_empty() {
                axis = t;
            } else if k == b"uni|" && unit.is_empty() {
                unit = t;
            }
        }
        if forward {
            at += RECORD;
        } else {
            match at.checked_sub(RECORD) {
                Some(a) if a >= 16 => at = a,
                _ => break,
            }
        }
    }
    (axis, unit, at)
}

/// Every float32 array record whose values lie inside the file and are followed by a record or
/// the end of the file.
fn arrays(b: &[u8]) -> Vec<Array> {
    let mut out = Vec::new();
    let mut from = 16;
    while let Some(p) = b
        .get(from..)
        .and_then(|s| s.windows(4).position(|w| w == ARRAY_KEY))
    {
        let key_at = from + p;
        from = key_at + 1;
        let Some(h) = key_at.checked_sub(8) else {
            continue;
        };
        if le_u32(b, h) != Some(ARRAY) || h + RECORD > b.len() {
            continue;
        }
        let Some(count) = le_u32(b, h + 20).and_then(|c| usize::try_from(c).ok()) else {
            continue;
        };
        let (mut axis, mut unit, values) = labels(b, h + RECORD, true);
        if axis.is_empty() && let Some(before) = h.checked_sub(RECORD) {
            let (a, u, _) = labels(b, before, false);
            axis = a;
            if unit.is_empty() {
                unit = u;
            }
        }
        let Some(end) = count.checked_mul(4).and_then(|n| n.checked_add(values)) else {
            continue;
        };
        if end > b.len() || (end < b.len() && record_type(b, end).is_none()) {
            continue;
        }
        out.push(Array {
            header: h,
            count,
            values,
            axis,
            unit,
        });
    }
    out
}

fn floats(b: &[u8], a: &Array) -> Vec<f64> {
    (0..a.count)
        .map(|i| f64::from(le_f32(b, a.values + 4 * i).unwrap_or(f32::NAN)))
        .collect()
}

pub(crate) fn parse(f: &SourceFile, path: &Path, file_len: u64) -> Result<Parsed> {
    if file_len > MAX_BYTES {
        return Err(Error::unsupported(
            FORMAT,
            format!("a {} MiB LabSpec file", file_len >> 20),
            "LabSpec files above 1 GiB are not read; export the spectra as text from LabSpec.",
        ));
    }
    let b = read_at(f, path, 0, file_len, file_len)?;
    if !b.starts_with(MAGIC) {
        return Err(Error::corrupt(FORMAT, "no `LabSpec6` signature at byte 0"));
    }
    let flags = le_u32(&b, 12)
        .ok_or_else(|| Error::corrupt(FORMAT, "the file ends inside its header"))?;
    if !PLAIN_FLAGS.contains(&flags) {
        return Err(Error::unsupported(
            FORMAT,
            format!("a LabSpec 6 file whose body is encoded (header word {flags})"),
            "The records of this file are not stored plainly; open it in LabSpec and export the spectrum or map as text (File > Save As > .txt), which OpenReadout reads.",
        ));
    }
    let all = arrays(&b);
    let first = all.iter().position(|a| a.axis == "Intens").ok_or_else(|| {
        Error::unsupported(
            FORMAT,
            "a LabSpec file without an intensity array the reader can place",
            "Export the spectrum as text from LabSpec; the reader knows the record layout of single spectra and maps only.",
        )
    })?;
    let y = all[first].clone();
    let rest = &all[first + 1..];
    let group_end = rest
        .iter()
        .position(|a| a.axis == "Intens")
        .unwrap_or(rest.len());
    let axes = &rest[..group_end];
    let more_objects = rest[group_end..].iter().filter(|a| a.axis == "Intens").count();
    let spectral = axes.iter().find(|a| a.axis == "Spectr").ok_or_else(|| {
        Error::unsupported(
            FORMAT,
            "an intensity array without a spectral axis",
            "Export the data as text from LabSpec.",
        )
    })?;
    let ax = axes.iter().find(|a| a.axis == "X");
    let ay = axes.iter().find(|a| a.axis == "Y");
    let others: Vec<&str> = axes
        .iter()
        .filter(|a| !matches!(a.axis.as_str(), "Spectr" | "X" | "Y"))
        .map(|a| a.axis.as_str())
        .collect();
    if !others.is_empty() {
        return Err(Error::unsupported(
            FORMAT,
            format!("a LabSpec data object with the axes {others:?}"),
            "Only spectra and X × Y maps are read; export the data as text from LabSpec.",
        ));
    }
    let points = spectral.count;
    let nx = ax.map_or(1, |a| a.count);
    let ny = ay.map_or(1, |a| a.count);
    let spectra = nx.checked_mul(ny).unwrap_or(0);
    if points == 0 || points.checked_mul(spectra) != Some(y.count) {
        return Err(Error::corrupt(
            FORMAT,
            format!(
                "{} intensity values do not fill {points} points × {nx} × {ny} positions",
                y.count
            ),
        ));
    }
    let count = u32::try_from(spectra).map_err(|_| Error::corrupt(FORMAT, "too many spectra"))?;
    let xs = floats(&b, spectral);
    let (x_quantity, x_unit) = match spectral.unit.as_str() {
        "1/cm" => ("raman_shift", Some("1/cm".to_string())),
        "nm" => ("wavelength", Some("nm".to_string())),
        "" => ("x", None),
        u => ("x", Some(u.to_string())),
    };
    let y_unit = match y.unit.as_str() {
        "Cnt" => Some("counts".to_string()),
        "" => None,
        u => Some(u.to_string()),
    };
    let mut p = Parsed {
        format_version: Some(format!("LabSpec 6 (header word {flags})")),
        ..Parsed::default()
    };
    let mut extra = BTreeMap::new();
    extra.insert("y_unit_stored".to_string(), json!(y.unit));
    extra.insert("x_unit_stored".to_string(), json!(spectral.unit));
    let raman = x_quantity == "raman_shift";
    p.sets.push(SpectrumSet {
        name: match (raman, count) {
            (true, 1) => "Raman spectrum".to_string(),
            (true, n) => format!("Raman map ({n} spectra)"),
            (false, 1) => "Spectrum".to_string(),
            (false, n) => format!("Spectral map ({n} spectra)"),
        },
        x_quantity,
        x_unit,
        x: XValues::Listed(xs),
        y_name: "intensity".into(),
        y_unit,
        points: points as u64,
        count,
        rows: Rows::Strided {
            first: y.values as u64,
            stride: points as u64 * 4,
        },
        stored: Stored::F32,
        scale: 1.0,
        data_type: if raman { "RAMAN SPECTRUM" } else { "UNKNOWN" },
        extra,
    });
    if count > 1 {
        let xv = ax.map(|a| floats(&b, a)).unwrap_or_else(|| vec![0.0]);
        let yv = ay.map(|a| floats(&b, a)).unwrap_or_else(|| vec![0.0]);
        let (mut cx, mut cy) = (Vec::new(), Vec::new());
        for k in 0..spectra {
            cx.push(xv.get(k % nx).copied().unwrap_or(f64::NAN));
            cy.push(yv.get(k / nx).copied().unwrap_or(f64::NAN));
        }
        let xu = ax.map(|a| a.unit.clone()).filter(|u| !u.is_empty());
        let yu = ay.map(|a| a.unit.clone()).filter(|u| !u.is_empty());
        p.tables.push(SpectrumTable {
            name: "positions".into(),
            trace: 0,
            columns: vec![
                ("spectrum".into(), None, (0..spectra).map(|k| k as f64).collect()),
                ("x".into(), xu.clone(), cx),
                ("y".into(), yu.clone(), cy),
            ],
        });
        if nx > 1 && ny > 1 {
            let step = |v: &[f64]| (v.len() > 1).then(|| (v[1] - v[0]).abs());
            let um = |u: &Option<String>| u.as_deref() == Some("µm");
            p.maps.push(MapSpec {
                name: format!(
                    "{} map {nx} × {ny}",
                    if raman { "Raman" } else { "Spectral" }
                ),
                set: 0,
                width: nx as u32,
                height: ny as u32,
                pixels: (0..count).map(Some).collect(),
                pixel_um: (
                    step(&xv).filter(|_| um(&xu)),
                    step(&yv).filter(|_| um(&yu)),
                ),
                extra: BTreeMap::new(),
            });
        }
    }
    for a in &all {
        p.entries.push(LsEntry {
            kind: "block".into(),
            name: format!("float32 array ({})", if a.axis.is_empty() { "unnamed" } else { &a.axis }),
            offset: Some(a.header as u64),
            size: Some(RECORD as u64 + 4 * a.count as u64),
            image: None,
            details: json!({"values": a.count, "unit": a.unit, "values_at": a.values}),
        });
    }
    if more_objects > 0 {
        p.notes.push(format!(
            "the file holds {more_objects} more data object(s) after the first; only the first is read"
        ));
        p.findings.push(Finding::info(
            "data_objects_not_read",
            format!("{more_objects} further data object(s) are not read"),
        ));
    }
    let facts = &mut p.facts;
    Facts::text(&mut facts.vendor, "HORIBA", "format");
    Facts::text(&mut facts.software, "LabSpec 6", "the `LabSpec6` signature");
    p.provenance.insert("traces".into(), Source::Inferred);
    p.vendor = json!({
        "header_word": flags,
        "arrays": all.iter().map(|a| json!({"axis": a.axis, "unit": a.unit, "values": a.count, "offset": a.header})).collect::<Vec<Value>>(),
    });
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(t: u32, key: &[u8; 4], value: [u8; 8]) -> Vec<u8> {
        let mut v = t.to_le_bytes().to_vec();
        v.extend(7u32.to_le_bytes());
        v.extend(key);
        v.extend([0; 4]);
        v.extend(value);
        v
    }

    fn text(key: &[u8; 4], s: &str) -> Vec<u8> {
        let mut v = [0u8; 8];
        v[..s.len()].copy_from_slice(s.as_bytes());
        rec(TEXT8, key, v)
    }

    fn array(n: u32) -> Vec<u8> {
        let mut v = [0u8; 8];
        v[4..].copy_from_slice(&n.to_le_bytes());
        rec(ARRAY, &ARRAY_KEY, v)
    }

    #[test]
    fn finds_both_label_orders() {
        let mut b = b"LabSpec6".to_vec();
        b.extend([0u8; 56]);
        b.extend(text(b"typm", "Intens"));
        b.extend(text(b"uni|", "Cnt"));
        b.extend(array(3));
        for v in [10f32, 20., 30.] {
            b.extend(v.to_le_bytes());
        }
        b.extend(array(3));
        b.extend(text(b"typm", "Spectr"));
        b.extend(text(b"uni|", "1/cm"));
        for v in [100f32, 101., 102.] {
            b.extend(v.to_le_bytes());
        }
        let a = arrays(&b);
        assert_eq!(a.len(), 2);
        assert_eq!((a[0].axis.as_str(), a[0].unit.as_str()), ("Intens", "Cnt"));
        assert_eq!((a[1].axis.as_str(), a[1].unit.as_str()), ("Spectr", "1/cm"));
        assert_eq!(floats(&b, &a[1]), vec![100.0, 101.0, 102.0]);
        // cut short: no array is placed past the end
        assert!(arrays(&b[..b.len() - 2]).len() < 2);
    }
}
