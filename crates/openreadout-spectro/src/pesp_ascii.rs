//! PerkinElmer `.sp` saved as text (`PE <technique> … ASCII PEDS <version>`): a header of one
//! value per line, then `#HDR`, `#GR` and `#DATA` blocks; `#DATA` holds x and y pairs.
//!
//! Layout and vocabulary: `docs/formats/perkinelmer-sp.md` (§ ASCII form); provenance:
//! `docs/provenance/perkinelmer-sp.md` (2026-10-06).

use std::collections::BTreeMap;
use std::path::Path;

use openreadout_core::model::LsEntry;
use openreadout_core::provenance::Source;
use openreadout_core::source::SourceFile;
use openreadout_core::{Error, Result};
use serde_json::{Value, json};

use crate::PESP_FORMAT_ID as FMT;
use crate::common::{Facts, Parsed, Rows, SpectrumSet, Stored, XValues, read_at};

/// Largest text file read (a spectrum of a few thousand points is tens of kilobytes).
const MAX_TEXT: u64 = 16 << 20;

/// Whether `head` starts like a text `.sp` file: `PE`, a technique code, then `ASCII` and
/// `PEDS` on the first line.
pub(crate) fn is_ascii_sp(head: &[u8]) -> bool {
    let line = head
        .split(|&b| b == b'\r' || b == b'\n')
        .next()
        .unwrap_or(&[]);
    let text = String::from_utf8_lossy(line);
    let mut words = text.split_whitespace();
    words.next() == Some("PE")
        && text.contains(" ASCII ")
        && text.split_whitespace().any(|w| w == "PEDS")
}

/// A parsed text `.sp` file and its y values (little-endian f64, read through a memory source).
pub(crate) struct Opened {
    pub(crate) parsed: Parsed,
    pub(crate) values: Vec<u8>,
}

fn corrupt(what: impl Into<String>) -> Error {
    Error::corrupt(FMT, what.into())
}

/// The lines of block `name` (`#GR`): from the line after the marker to the next `#` line.
fn block<'a>(lines: &[&'a str], name: &str) -> Option<Vec<&'a str>> {
    let at = lines.iter().position(|l| l.trim() == name)?;
    Some(
        lines[at + 1..]
            .iter()
            .take_while(|l| !l.trim_start().starts_with('#'))
            .copied()
            .collect(),
    )
}

fn number(s: &str, what: &str) -> Result<f64> {
    s.trim()
        .parse::<f64>()
        .map_err(|_| corrupt(format!("#GR {what} is not a number: {:?}", s.trim())))
}

/// Parse a text `.sp` file.
pub(crate) fn parse(f: &SourceFile, path: &Path, file_len: u64) -> Result<Opened> {
    if file_len > MAX_TEXT {
        return Err(Error::unsupported(
            FMT,
            format!("a {file_len}-byte text .sp file"),
            "Text (PEDS) .sp files over 16 MiB are not read; single spectra are a few kilobytes.",
        ));
    }
    let b = read_at(f, path, 0, file_len, file_len)?;
    let text: String = b.iter().map(|&c| char::from(c)).collect(); // Latin-1
    let lines: Vec<&str> = text.lines().map(|l| l.trim_end_matches('\r')).collect();
    let first = lines.first().copied().unwrap_or_default();
    if !is_ascii_sp(first.as_bytes()) {
        return Err(corrupt(
            "not a text .sp file: the first line is not `PE … ASCII PEDS …`",
        ));
    }
    let words: Vec<&str> = first.split_whitespace().collect();
    let technique = words.get(1).copied().unwrap_or_default().to_string();
    let version = words
        .iter()
        .position(|w| *w == "PEDS")
        .and_then(|i| words.get(i + 1))
        .map(|v| format!("PEDS {v}"));
    let gr = block(&lines, "#GR").ok_or_else(|| corrupt("no #GR block"))?;
    let data_at = lines
        .iter()
        .position(|l| l.trim() == "#DATA")
        .ok_or_else(|| corrupt("no #DATA block"))?;
    let field = |i: usize| gr.get(i).copied().unwrap_or_default();
    let x_unit_text = field(0).trim().to_string();
    let y_unit_text = field(1).trim().to_string();
    let first_x = number(field(4), "first x (line 5)")?;
    let step = number(field(5), "x interval (line 6)")?;
    let declared = number(field(6), "point count (line 7)")?;
    let y_max = number(field(8), "largest y (line 9)")?;
    let y_min = number(field(9), "smallest y (line 10)")?;

    let mut xs = Vec::new();
    let mut ys = Vec::new();
    for (k, l) in lines[data_at + 1..].iter().enumerate() {
        let l = l.trim();
        if l.is_empty() {
            continue;
        }
        if l.starts_with('#') {
            break;
        }
        let mut it = l.split_whitespace();
        let (Some(x), Some(y), None) = (it.next(), it.next(), it.next()) else {
            return Err(corrupt(format!(
                "#DATA line {} is not an x and y pair: {l:?}",
                k + 1
            )));
        };
        let (Ok(x), Ok(y)) = (x.parse::<f64>(), y.parse::<f64>()) else {
            return Err(corrupt(format!(
                "#DATA line {} is not numeric: {l:?}",
                k + 1
            )));
        };
        xs.push(x);
        ys.push(y);
    }
    // The #GR block describes the data: refuse a file where it does not.
    let n = xs.len();
    let tol = |a: f64, b: f64| (a - b).abs() <= 1e-6 * a.abs().max(b.abs()).max(1.0);
    if n == 0 || !tol(declared, n as f64) {
        return Err(corrupt(format!(
            "#GR declares {declared} points but #DATA holds {n}"
        )));
    }
    if !tol(xs[0], first_x) {
        return Err(corrupt(format!(
            "#GR first x {first_x} but #DATA starts at {}",
            xs[0]
        )));
    }
    if let Some(i) = (0..n).find(|&i| !tol(xs[i], first_x + step * i as f64)) {
        return Err(corrupt(format!(
            "#DATA x {} at point {i} is off the #GR grid (first {first_x}, interval {step})",
            xs[i]
        )));
    }
    let (lo, hi) = ys
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), &y| {
            (a.min(y), b.max(y))
        });
    if !tol(lo, y_min) || !tol(hi, y_max) {
        return Err(corrupt(format!(
            "#GR y range {y_min}..{y_max} but #DATA spans {lo}..{hi}"
        )));
    }

    let (xq, xu, dtype) = crate::pesp::x_axis(&x_unit_text);
    let (yq, yu) = crate::pesp::y_axis(&y_unit_text);
    let data_type = if technique == "FL" && xq == "wavelength" {
        "FLUORESCENCE SPECTRUM"
    } else {
        dtype
    };
    let mut values = Vec::with_capacity(n * 8);
    for y in &ys {
        values.extend_from_slice(&y.to_le_bytes());
    }
    let mut extra = BTreeMap::new();
    extra.insert("technique_code".into(), json!(technique));
    extra.insert("data_interval".into(), crate::common::num(step));
    extra.insert("x_units_text".into(), json!(x_unit_text));
    extra.insert("y_units_text".into(), json!(y_unit_text));
    let mut parsed = Parsed {
        format_version: version,
        ..Parsed::default()
    };
    let mut facts = Facts::default();
    Facts::text(&mut facts.vendor, "PerkinElmer", "format");
    parsed.facts = facts;
    parsed.sets.push(SpectrumSet {
        name: yq.to_string(),
        x_quantity: xq,
        x_unit: xu.map(str::to_string),
        x: XValues::Regular {
            first: xs[0],
            last: xs[n - 1],
        },
        y_name: yq.into(),
        y_unit: yu,
        points: n as u64,
        count: 1,
        rows: Rows::Listed(vec![0]),
        stored: Stored::F64,
        scale: 1.0,
        data_type,
        extra,
    });
    let header: Vec<Value> = lines[..data_at].iter().map(|l| json!(l.trim())).collect();
    parsed.vendor = json!({ "header_lines": header });
    parsed.entries.push(LsEntry {
        kind: "block".into(),
        name: "#DATA".into(),
        offset: None,
        size: Some(n as u64),
        image: None,
        details: json!({"points": n}),
    });
    parsed.notes.push(format!(
        "PerkinElmer .sp saved as text ({}): the x and y pairs of #DATA as written; header fields other than the technique code are kept in the vendor tree by line",
        parsed.format_version.as_deref().unwrap_or("PEDS")
    ));
    for (k, v) in [
        ("traces[].sample_count", Source::Inferred),
        ("traces[].extra.axis", Source::Inferred),
        ("traces[].extra.y_quantity", Source::Inferred),
    ] {
        parsed.provenance.insert(k.into(), v);
    }
    Ok(Opened { parsed, values })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "PE FL        SPECTRUM    ASCII       PEDS        1.60\r\n  -1\r\nNAME.SP\r\n#HDR\r\n-1\r\n#GR\r\nNM\r\n\r\n0.0000238\r\n0.0\r\n500.0\r\n0.5\r\n3\r\n8\r\n3.0\r\n1.0\r\n#DATA\r\n500.000000\t2.0\r\n500.500000\t3.0\r\n501.000000\t1.0\r\n";

    fn open(s: &str) -> Result<Opened> {
        let src = openreadout_core::source::MemSource::new("a.sp", s.as_bytes().to_vec());
        let f = SourceFile::new(std::sync::Arc::new(src));
        parse(&f, Path::new("a.sp"), s.len() as u64)
    }

    #[test]
    fn reads_the_data_pairs() {
        assert!(is_ascii_sp(FILE.as_bytes()));
        let o = open(FILE).unwrap();
        let s = &o.parsed.sets[0];
        assert_eq!((s.points, s.x_quantity), (3, "wavelength"));
        assert_eq!(s.data_type, "FLUORESCENCE SPECTRUM");
        assert_eq!(o.parsed.format_version.as_deref(), Some("PEDS 1.60"));
        let y: Vec<f64> = o
            .values
            .chunks(8)
            .map(|c| f64::from_le_bytes(c.try_into().unwrap()))
            .collect();
        assert_eq!(y, vec![2.0, 3.0, 1.0]);
    }

    #[test]
    fn refuses_a_header_that_disagrees_with_the_data() {
        assert!(open(&FILE.replace("\r\n3\r\n8", "\r\n4\r\n8")).is_err());
        assert!(open(&FILE.replace("501.000000\t1.0", "502.000000\t1.0")).is_err());
        assert!(open(&FILE.replace("\r\n3.0\r\n1.0\r\n#DATA", "\r\n9.0\r\n1.0\r\n#DATA")).is_err());
        assert!(open(&FILE.replace("#DATA", "#NODATA")).is_err());
        assert!(!is_ascii_sp(b"PEPE 2D constant interval"));
    }
}
