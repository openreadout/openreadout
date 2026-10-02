//! GenePix Results files (`.gpr`, GenePix Pro): an Axon Text File (ATF 1.0) whose header records
//! describe the scan (scanner, wavelengths, PMT gains, laser power, pixel size, GAL file) and whose
//! table has one row per microarray feature. Layout: `docs/formats/genepix-gpr.md`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use openreadout_core::experiment::{
    Acquisition, Experiment, ExperimentInstrument, Measurement, MeasurementKind, Method, Origin,
    Quantity,
};
use openreadout_core::model::{
    CheckReport, FileInfo, Finding, FormatDescriptor, LsEntry, Table, TableInfo, Trace,
};
use openreadout_core::pixel::Plane;
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, PlaneIndex};
use openreadout_core::{ColumnInfo, Error, Result};
use serde_json::{Map, Value, json};

pub(crate) const FORMAT_ID: &str = "genepix-gpr";
/// Largest file read (the largest seen is 12 MB).
pub(crate) const MAX_BYTES: u64 = 1 << 30;
/// Most header records and columns accepted.
const MAX_RECORDS: usize = 10_000;

fn unquote(s: &str) -> &str {
    s.trim().trim_matches('"').trim()
}

/// The `Type=` header record of an ATF head (`GenePix Results 3`), when there is one.
pub(crate) fn atf_type(head: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(head);
    let mut lines = text.lines();
    let sig = lines.next()?;
    if sig.split_whitespace().next() != Some("ATF") {
        return None;
    }
    lines
        .take(64)
        .map(unquote)
        .find_map(|l| l.strip_prefix("Type=").map(|t| t.trim().to_string()))
}

/// A column: numbers (non-numeric cells as NaN are not allowed: then it is text).
#[derive(Debug, Clone)]
enum Column {
    Numbers(Vec<f64>),
    Text(Vec<f64>, Vec<String>),
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Gpr {
    version: String,
    kind: String,
    header: Vec<(String, Vec<String>)>,
    titles: Vec<String>,
    columns: Vec<Column>,
    rows: usize,
    findings: Vec<Finding>,
}

impl Gpr {
    fn header(&self, key: &str) -> Option<&[String]> {
        self.header
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_slice())
    }
    fn header1(&self, key: &str) -> Option<&str> {
        self.header(key)
            .and_then(|v| v.first())
            .map(String::as_str)
            .filter(|s| !s.is_empty())
    }
}

/// Parse the whole file.
pub(crate) fn parse(text: &str) -> Result<Gpr> {
    let mut lines = text.lines();
    let sig = lines
        .next()
        .ok_or_else(|| Error::corrupt(FORMAT_ID, "empty file"))?;
    let mut w = sig.split_whitespace();
    if w.next() != Some("ATF") {
        return Err(Error::corrupt(FORMAT_ID, "no `ATF` signature line"));
    }
    let version = w.next().unwrap_or("").to_string();
    let counts: Vec<usize> = lines
        .next()
        .unwrap_or("")
        .split_whitespace()
        .filter_map(|x| x.parse().ok())
        .collect();
    let [n_header, n_columns] = counts[..] else {
        return Err(Error::corrupt(
            FORMAT_ID,
            "the header-count line is not two numbers",
        ));
    };
    if n_header > MAX_RECORDS || n_columns == 0 || n_columns > MAX_RECORDS {
        return Err(Error::corrupt(
            FORMAT_ID,
            format!("{n_header} header records and {n_columns} columns"),
        ));
    }
    let mut header = Vec::new();
    for _ in 0..n_header {
        let l = lines
            .next()
            .ok_or_else(|| Error::corrupt(FORMAT_ID, "the file ends inside its header"))?;
        let fields: Vec<&str> = l.split('\t').collect();
        let first = unquote(fields[0]);
        let (key, mut values) = match first.split_once('=') {
            Some((k, v)) => (k.trim().to_string(), vec![v.trim().to_string()]),
            None => (first.to_string(), Vec::new()),
        };
        values.extend(fields[1..].iter().map(|v| unquote(v).to_string()));
        header.push((key, values));
    }
    let kind = header
        .iter()
        .find(|(k, _)| k == "Type")
        .and_then(|(_, v)| v.first().cloned())
        .unwrap_or_default();
    if !kind.starts_with("GenePix Results") {
        return Err(Error::unsupported(
            FORMAT_ID,
            format!("a GenePix file of type `{kind}`"),
            "Only GenePix Results files (.gpr) are read; array lists (.gal) and settings files are not.",
        ));
    }
    let titles: Vec<String> = lines
        .next()
        .ok_or_else(|| Error::corrupt(FORMAT_ID, "no column titles"))?
        .split('\t')
        .map(|t| unquote(t).to_string())
        .collect();
    if titles.len() != n_columns {
        return Err(Error::corrupt(
            FORMAT_ID,
            format!(
                "{} column titles, the header says {n_columns}",
                titles.len()
            ),
        ));
    }
    let mut cells: Vec<Vec<&str>> = vec![Vec::new(); n_columns];
    let mut short = 0usize;
    for l in lines {
        if l.trim().is_empty() {
            continue;
        }
        let f: Vec<&str> = l.split('\t').collect();
        if f.len() != n_columns {
            short += 1;
            continue;
        }
        for (k, v) in f.into_iter().enumerate() {
            cells[k].push(v);
        }
    }
    let mut findings = Vec::new();
    if short > 0 {
        findings.push(Finding::error(
            "ragged_rows",
            format!("{short} rows do not have {n_columns} cells; they are left out"),
        ));
    }
    let rows = cells.first().map_or(0, Vec::len);
    let columns = cells
        .iter()
        .map(|c| {
            // GenePix writes `Error` where a ratio is undefined: a number column with gaps
            let parsed: Vec<Option<f64>> = c
                .iter()
                .map(|v| unquote(v).parse::<f64>().ok().filter(|x| x.is_finite()))
                .collect();
            let gap = |v: &&str| matches!(unquote(v), "Error" | "");
            let numeric = parsed.iter().any(Option::is_some)
                && c.iter().zip(&parsed).all(|(v, p)| p.is_some() || gap(v));
            if numeric {
                Column::Numbers(parsed.into_iter().map(|x| x.unwrap_or(f64::NAN)).collect())
            } else {
                let mut labels: Vec<String> = Vec::new();
                let mut index: BTreeMap<String, usize> = BTreeMap::new();
                let codes = c
                    .iter()
                    .map(|v| {
                        let v = unquote(v).to_string();
                        let n = labels.len();
                        let i = *index.entry(v.clone()).or_insert_with(|| {
                            labels.push(v);
                            n
                        });
                        i as f64
                    })
                    .collect();
                Column::Text(codes, labels)
            }
        })
        .collect();
    Ok(Gpr {
        version,
        kind,
        header,
        titles,
        columns,
        rows,
        findings,
    })
}

/// An opened `.gpr` file.
#[derive(Debug)]
pub struct GprDataset {
    descriptor: FormatDescriptor,
    path: PathBuf,
    size: u64,
    g: Gpr,
}

impl GprDataset {
    pub(crate) fn new(descriptor: FormatDescriptor, path: PathBuf, size: u64, g: Gpr) -> Self {
        GprDataset {
            descriptor,
            path,
            size,
            g,
        }
    }
}

/// Column units: none is stated in the file (positions and diameters are left unitless).
fn unit_of(_title: &str) -> Option<&'static str> {
    None
}

impl Dataset for GprDataset {
    fn info(&self) -> Result<FileInfo> {
        let g = &self.g;
        let columns = g
            .titles
            .iter()
            .zip(&g.columns)
            .enumerate()
            .map(|(k, (t, c))| ColumnInfo {
                index: k as u32,
                name: t.clone(),
                label: None,
                dtype: match c {
                    Column::Numbers(_) => "float64",
                    Column::Text(..) => "uint32",
                }
                .into(),
                unit: unit_of(t).map(str::to_string),
                range: None,
                extra: match c {
                    Column::Numbers(_) => BTreeMap::new(),
                    Column::Text(_, labels) => {
                        BTreeMap::from([("categories".to_string(), json!(labels))])
                    }
                },
            })
            .collect();
        let mut extra = BTreeMap::new();
        extra.insert("kind".to_string(), json!("microarray_features"));
        if let Some(w) = g.header("Wavelengths") {
            extra.insert("wavelengths_nm".to_string(), json!(w));
        }
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.size,
            format: self.descriptor.clone(),
            format_version: Some(format!("ATF {} ({})", g.version, g.kind)),
            plane_count: 0,
            images: Vec::new(),
            tables: vec![TableInfo {
                index: 0,
                name: Some("features".into()),
                row_count: g.rows as u64,
                columns,
                extra,
            }],
            spectra: Vec::new(),
            traces: Vec::new(),
            notes: vec![
                "one row per feature as GenePix wrote it (medians, means, backgrounds, ratios, flags); intensities are as stored, not re-normalized".into(),
            ],
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let m: Map<String, Value> = self
            .g
            .header
            .iter()
            .map(|(k, v)| (k.clone(), if v.len() == 1 { json!(v[0]) } else { json!(v) }))
            .collect();
        Ok(json!({ "genepix": Value::Object(m) }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        p.insert("tables".into(), Source::Spec);
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        Ok(Vec::new())
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::Usage(
            "a GenePix results file holds a feature table (table 0), not images; the scan TIFF is a separate file".into(),
        ))
    }

    fn check(&mut self) -> Result<CheckReport> {
        self.check_headers()
    }

    fn check_headers(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), self.descriptor.id.clone());
        r.performed("ATF header and every table row parsed; rows counted against the column count");
        for f in &self.g.findings {
            r.push(f.clone());
        }
        Ok(r)
    }

    fn read_trace(&mut self, index: u32, _sweep: u32, _first: u64, _max: u64) -> Result<Trace> {
        Err(Error::Usage(format!(
            "trace {index}: a GenePix results file holds a feature table (table 0), no traces"
        )))
    }

    fn read_table(&mut self, index: u32, first_row: u64, max_rows: u64) -> Result<Table> {
        if index != 0 {
            return Err(Error::Usage(format!(
                "table {index} out of range (one table)"
            )));
        }
        let n = self.g.rows as u64;
        let a = first_row.min(n) as usize;
        let b = first_row.saturating_add(max_rows).min(n) as usize;
        Ok(Table {
            table: 0,
            first_row: a as u64,
            columns: self
                .g
                .columns
                .iter()
                .map(|c| match c {
                    Column::Numbers(v) | Column::Text(v, _) => v[a..b].to_vec(),
                })
                .collect(),
        })
    }

    fn experiment(&self) -> Option<Experiment> {
        let g = &self.g;
        let mut exp = Experiment::default();
        let origin = |from: &str| Origin {
            source: Source::Spec,
            from: from.into(),
        };
        let mut ins = ExperimentInstrument {
            vendor: Some("Molecular Devices (Axon GenePix)".into()),
            ..ExperimentInstrument::default()
        };
        if let Some(s) = g.header1("Scanner") {
            // `GenePix 4000 [140014]`: model, serial in brackets
            let (model, serial) = match s.split_once('[') {
                Some((method, rest)) => (method.trim(), Some(rest.trim_end_matches(']').trim())),
                None => (s.trim(), None),
            };
            ins.model = Some(model.to_string());
            ins.serial = serial.filter(|x| !x.is_empty()).map(str::to_string);
            exp.provenance
                .insert("instrument.model".into(), origin("Scanner"));
        }
        if let Some(c) = g.header1("Creator") {
            // `GenePix Pro 7.3.0.0`
            let (name, ver) = c
                .rfind(' ')
                .map_or((c, None), |i| (&c[..i], Some(&c[i + 1..])));
            ins.software = Some(name.to_string());
            ins.software_version = ver.map(str::to_string);
            exp.provenance
                .insert("instrument.software".into(), origin("Creator"));
        }
        exp.instrument = Some(ins);
        let mut method = Method::default();
        // `Wavelengths=635\t532` (GenePix Pro 6-7), `ImageName=\t532` (GenePix Pro 3)
        // positions matter: the other per-channel records are in the same order
        let wl_pos: Vec<Option<f64>> = g
            .header("Wavelengths")
            .or_else(|| g.header("ImageName"))
            .unwrap_or(&[])
            .iter()
            .map(|w| w.trim().parse().ok())
            .collect();
        let wl: Vec<f64> = wl_pos.iter().flatten().copied().collect();
        let per = |key: &str| -> Vec<Option<f64>> {
            g.header(key)
                .unwrap_or(&[])
                .iter()
                .map(|v| v.trim().parse().ok())
                .collect()
        };
        let (pmt, laser, power) = (per("PMTGain"), per("LaserPower"), per("ScanPower"));
        let volts = per("PMTVolts");
        for (k, w) in wl_pos.iter().enumerate() {
            let Some(w) = w else { continue };
            let tag = format!("{w}");
            if let Some(Some(v)) = pmt.get(k) {
                method
                    .parameters
                    .insert(format!("pmt_gain_{tag}"), Quantity::plain(*v));
            }
            if let Some(Some(v)) = volts.get(k) {
                method
                    .parameters
                    .insert(format!("pmt_voltage_{tag}"), Quantity::number(*v, "V"));
            }
            if let Some(Some(v)) = laser.get(k) {
                method
                    .parameters
                    .insert(format!("laser_power_{tag}"), Quantity::plain(*v));
            }
            if let Some(Some(v)) = power.get(k) {
                method
                    .parameters
                    .insert(format!("scan_power_{tag}"), Quantity::plain(*v));
            }
        }
        if !wl.is_empty() {
            method.parameters.insert(
                "wavelengths".into(),
                Quantity::plain(
                    wl.iter()
                        .map(|w| format!("{w} nm"))
                        .collect::<Vec<_>>()
                        .join(", "),
                ),
            );
            exp.provenance.insert(
                "method.parameters".into(),
                origin("Wavelengths, PMTGain, LaserPower, ScanPower"),
            );
        }
        if let Some(p) = g.header1("PixelSize").and_then(|s| s.parse::<f64>().ok()) {
            method
                .parameters
                .insert("pixel_size".into(), Quantity::plain(p));
        }
        exp.method = Some(method);
        if let Some(d) = g.header1("DateTime") {
            // `2022/05/17 17:07:54`, local time
            let iso = d.replacen('/', "-", 2).replacen(' ', "T", 1);
            if iso.len() == 19 {
                exp.acquisition = Some(Acquisition {
                    started_at: Some(iso),
                    comment: g.header1("Comment").map(str::to_string),
                    ..Acquisition::default()
                });
                exp.provenance.insert(
                    "acquisition.started_at".into(),
                    origin("DateTime (local time)"),
                );
            }
        }
        exp.measurements.push(Measurement {
            kind: MeasurementKind::Table,
            indices: vec![0],
            what: format!(
                "microarray scan: {} features, {} wavelength(s)",
                g.rows,
                wl.len()
            ),
            technique: None,
            terms: Vec::new(),
            parameters: BTreeMap::new(),
        });
        let keys: Vec<String> = exp
            .method
            .iter()
            .flat_map(|method| method.parameters.keys().cloned())
            .collect();
        for k in keys {
            exp.provenance
                .entry(format!("method.parameters.{k}"))
                .or_insert_with(|| origin("header records Wavelengths/ImageName, PMTGain/PMTVolts, LaserPower, ScanPower, PixelSize"));
        }
        if exp.instrument.as_ref().is_some_and(|i| i.serial.is_some()) {
            exp.provenance.insert(
                "instrument.serial".into(),
                origin("Scanner (bracketed serial)"),
            );
        }
        if exp
            .instrument
            .as_ref()
            .is_some_and(|i| i.software_version.is_some())
        {
            exp.provenance
                .insert("instrument.software_version".into(), origin("Creator"));
        }
        if exp
            .acquisition
            .as_ref()
            .is_some_and(|a| a.comment.is_some())
        {
            exp.provenance
                .insert("acquisition.comment".into(), origin("Comment"));
        }
        crate::complete_provenance(&mut exp);
        Some(exp)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp)]
    use super::*;

    const SAMPLE: &str = "ATF\t1.0\n4\t5\n\"Type=GenePix Results 3\"\n\"Wavelengths=635\t532\"\n\"PMTGain=480\t100\"\n\"Scanner=GenePix 4000 [140014]\"\n\"Block\"\t\"Column\"\t\"Name\"\t\"F635 Median\"\t\"Flags\"\n1\t1\t\"GST\"\t812\t0\n1\t2\t\"empty\"\t30\t-50\n";

    #[test]
    fn parses_a_results_file() {
        let g = parse(SAMPLE).unwrap();
        assert_eq!(g.rows, 2);
        assert_eq!(g.titles[3], "F635 Median");
        let Column::Numbers(v) = &g.columns[3] else {
            panic!("numbers expected")
        };
        assert_eq!(v, &vec![812.0, 30.0]);
        let Column::Text(c, l) = &g.columns[2] else {
            panic!("text expected")
        };
        assert_eq!(
            (c.clone(), l.clone()),
            (vec![0.0, 1.0], vec!["GST".to_string(), "empty".to_string()])
        );
        assert_eq!(g.header("Wavelengths").unwrap(), &["635", "532"]);
        assert_eq!(
            atf_type(SAMPLE.as_bytes()).as_deref(),
            Some("GenePix Results 3")
        );
    }

    #[test]
    fn refusals() {
        assert!(parse("hello").is_err());
        let gal = SAMPLE.replace("GenePix Results 3", "GenePix ArrayList V1.0");
        assert!(matches!(parse(&gal), Err(Error::Unsupported { .. })));
        let short = SAMPLE.replace("1\t2\t\"empty\"\t30\t-50\n", "1\t2\n");
        let g = parse(&short).unwrap();
        assert_eq!(g.rows, 1);
        assert_eq!(g.findings.len(), 1);
        for cut in 0..SAMPLE.len() {
            if let Some(s) = SAMPLE.get(..cut) {
                let _ = parse(s);
            }
        }
    }
}
