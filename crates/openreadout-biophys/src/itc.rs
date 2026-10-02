//! MicroCal isothermal titration calorimetry raw files (`.itc`: VP-ITC, iTC200, MicroCal ITC
//! software): a text header (method, concentrations, cell volume, instrument) and the
//! thermogram, one block per injection. Layout: `docs/formats/microcal-itc.md`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use openreadout_core::experiment::{
    Acquisition, Experiment, ExperimentInstrument, Measurement, MeasurementKind, Method, Origin,
    Quantity,
};
use openreadout_core::model::{
    CheckReport, FileInfo, Finding, FormatDescriptor, LsEntry, Severity, SignalChannelInfo, Table,
    TableInfo, Trace, TraceInfo,
};
use openreadout_core::pixel::Plane;
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, PlaneIndex};
use openreadout_core::vocab;
use openreadout_core::{ColumnInfo, Error, Result};
use serde_json::{Map, Value, json};

pub(crate) const FORMAT_ID: &str = "microcal-itc";
/// Largest file read (the largest seen is 0.6 MB).
pub(crate) const MAX_BYTES: u64 = 256 << 20;

/// One injection as the method lists it, with where its block starts in the data.
#[derive(Debug, Clone, Default)]
struct Injection {
    volume_ul: f64,
    duration_s: f64,
    spacing_s: f64,
    filter_s: f64,
    /// From the block line (`@n,…, <start>`), when written.
    start_s: Option<f64>,
    /// First data row of the injection's block.
    first_row: Option<usize>,
    /// Volume written on the block line.
    block_volume_ul: Option<f64>,
}

/// A parsed `.itc` file.
#[derive(Debug, Default)]
pub(crate) struct Itc {
    pub(crate) declared_injections: Option<usize>,
    injections: Vec<Injection>,
    target_temperature_c: Option<f64>,
    initial_delay_s: Option<f64>,
    stirring_rpm: Option<f64>,
    reference_power: Option<f64>,
    syringe_mm: Option<f64>,
    cell_mm: Option<f64>,
    cell_volume_ml: Option<f64>,
    comment: String,
    instrument_id: Option<String>,
    software: Option<String>,
    header: Map<String, Value>,
    /// Data columns: `columns[c][row]`.
    columns: Vec<Vec<f64>>,
    /// Row where the pre-injection baseline (`@0`) starts.
    baseline_row: Option<usize>,
    findings: Vec<Finding>,
}

fn num(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

fn nums(s: &str) -> Vec<Option<f64>> {
    s.split(',').map(num).collect()
}

/// Does the text look like a MicroCal `.itc` file?
pub(crate) fn looks_like(head: &[u8]) -> bool {
    head.starts_with(b"$ITC") || head.starts_with(b"\xef\xbb\xbf$ITC")
}

pub(crate) fn parse(text: &str) -> Result<Itc> {
    let mut it = Itc::default();
    let mut dollar: Vec<String> = Vec::new();
    let mut hash: Vec<String> = Vec::new();
    let mut percent: Vec<String> = Vec::new();
    let mut in_comment = false;
    let mut comment: Vec<String> = Vec::new();
    let mut ncols = 0usize;
    let mut bad_rows = 0usize;
    let mut first_line = true;
    for raw in text.lines() {
        let line = raw.trim_end_matches('\r');
        let line = if first_line {
            first_line = false;
            line.trim_start_matches('\u{feff}')
        } else {
            line
        };
        let t = line.trim();
        if let Some(rest) = t.strip_prefix('@') {
            in_comment = false;
            let row = it.columns.first().map_or(0, Vec::len);
            let f = nums(rest);
            let n = rest
                .split(',')
                .next()
                .and_then(|s| s.trim().parse::<usize>().ok());
            match n {
                Some(0) => it.baseline_row = Some(row),
                Some(k) if k >= 1 => {
                    if it.injections.len() < k {
                        it.injections.resize(k, Injection::default());
                    }
                    let inj = &mut it.injections[k - 1];
                    inj.first_row = Some(row);
                    inj.block_volume_ul = f.get(1).copied().flatten();
                    inj.start_s = f.get(3).copied().flatten();
                }
                _ => bad_rows += 1,
            }
            continue;
        }
        if in_comment && !t.starts_with('%') {
            comment.push(line.to_string());
            continue;
        }
        if let Some(rest) = t.strip_prefix('$') {
            dollar.push(rest.trim().to_string());
        } else if let Some(rest) = t.strip_prefix('#') {
            hash.push(rest.trim().to_string());
        } else if let Some(rest) = t.strip_prefix('?') {
            in_comment = true;
            comment.push(rest.to_string());
        } else if let Some(rest) = t.strip_prefix('%') {
            in_comment = false;
            percent.push(rest.trim().to_string());
        } else if t.starts_with(|c: char| c.is_ascii_digit() || c == '-' || c == '.') {
            let vals: Vec<Option<f64>> = nums(t);
            if vals.iter().any(Option::is_none) {
                bad_rows += 1;
                continue;
            }
            if ncols == 0 {
                ncols = vals.len();
                it.columns = vec![Vec::new(); ncols];
            }
            if vals.len() != ncols {
                bad_rows += 1;
                continue;
            }
            for (c, v) in vals.into_iter().enumerate() {
                it.columns[c].push(v.unwrap_or(f64::NAN));
            }
        } else if !t.is_empty() {
            bad_rows += 1;
        }
    }
    if dollar.first().map(String::as_str) != Some("ITC") {
        return Err(Error::corrupt(
            FORMAT_ID,
            "the file does not start with `$ITC`",
        ));
    }
    // `$` lines: count, a flag, temperature, delay, stirring, reference power, a code, ADC gain,
    // three booleans, then one line per injection
    let d = |i: usize| dollar.get(i).and_then(|s| num(s));
    it.declared_injections = d(1)
        .filter(|v| *v >= 0.0 && v.fract() == 0.0)
        .map(|v| v as usize);
    it.target_temperature_c = d(3);
    it.initial_delay_s = d(4);
    it.stirring_rpm = d(5);
    it.reference_power = d(6);
    let mut method_lines = Vec::new();
    for s in dollar.iter().skip(1) {
        let v = nums(s);
        if v.len() == 4 && v.iter().all(Option::is_some) {
            method_lines.push(v.into_iter().flatten().collect::<Vec<f64>>());
        }
    }
    for (k, m) in method_lines.iter().enumerate() {
        if it.injections.len() <= k {
            it.injections.resize(k + 1, Injection::default());
        }
        let inj = &mut it.injections[k];
        inj.volume_ul = m[0];
        inj.duration_s = m[1];
        inj.spacing_s = m[2];
        inj.filter_s = m[3];
    }
    let h = |i: usize| hash.get(i).and_then(|s| num(s));
    it.syringe_mm = h(1);
    it.cell_mm = h(2);
    it.cell_volume_ml = h(3);
    it.comment = comment
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    it.instrument_id = percent.first().cloned().filter(|s| !s.is_empty());
    it.software = percent
        .last()
        .cloned()
        .filter(|s| s.chars().any(char::is_alphabetic) && Some(s) != it.instrument_id.as_ref());
    it.header = {
        let mut m = Map::new();
        m.insert("dollar_lines".into(), json!(dollar));
        m.insert("hash_lines".into(), json!(hash));
        m.insert("percent_lines".into(), json!(percent));
        m
    };
    if ncols < 2 || it.columns[0].is_empty() {
        return Err(Error::corrupt(
            FORMAT_ID,
            "no data rows (time, power, …) after the header",
        ));
    }
    if bad_rows > 0 {
        it.findings.push(Finding::warning(
            "unreadable_rows",
            format!("{bad_rows} lines are neither header, block markers nor data rows with the file's {ncols} numbers"),
        ));
    }
    if let Some(n) = it.declared_injections {
        let blocks = it
            .injections
            .iter()
            .filter(|i| i.first_row.is_some())
            .count();
        if n != method_lines.len() {
            it.findings.push(Finding::warning(
                "injection_count",
                format!(
                    "the header declares {n} injections and lists {}",
                    method_lines.len()
                ),
            ));
        }
        if blocks < n {
            it.findings.push(Finding::warning(
                "incomplete_run",
                format!("{blocks} of {n} injections have data (the run stopped early or the file is truncated)"),
            ));
        }
    }
    Ok(it)
}

impl Itc {
    fn rows(&self) -> usize {
        self.columns.first().map_or(0, Vec::len)
    }

    /// `(start, step)` in s when the time column is evenly spaced.
    fn regular_time(&self) -> Option<(f64, f64)> {
        let t = self.columns.first()?;
        if t.len() < 2 {
            return t.first().map(|s| (*s, 0.0));
        }
        let step = t[1] - t[0];
        if !(step.is_finite() && step > 0.0) {
            return None;
        }
        let ok = t
            .windows(2)
            .all(|w| ((w[1] - w[0]) - step).abs() <= 1e-6 * step.max(1.0));
        ok.then_some((t[0], step))
    }

    fn model(&self) -> Option<String> {
        let id = self.instrument_id.as_deref()?;
        let u = id.to_ascii_uppercase();
        Some(if u.starts_with("VPITC") {
            "VP-ITC".into()
        } else if u.starts_with("ITC200") {
            "iTC200".into()
        } else if u.starts_with("MICROCALITC") {
            "MicroCal ITC".into()
        } else {
            id.split(['_', ' ']).next().unwrap_or(id).to_string()
        })
    }
}

/// An opened `.itc` file.
#[derive(Debug)]
pub struct ItcDataset {
    descriptor: FormatDescriptor,
    path: PathBuf,
    size: u64,
    itc: Itc,
}

impl ItcDataset {
    pub(crate) fn new(descriptor: FormatDescriptor, path: PathBuf, size: u64, itc: Itc) -> Self {
        ItcDataset {
            descriptor,
            path,
            size,
            itc,
        }
    }

    /// Channels of the thermogram (the time column is a channel only when sampling is uneven).
    fn channel_columns(&self) -> Vec<(usize, &'static str, Option<&'static str>)> {
        let mut out = Vec::new();
        if self.itc.regular_time().is_none() {
            out.push((0, "time", Some("s")));
        }
        out.push((1, "differential_power", Some("µcal/s")));
        if self.itc.columns.len() > 2 {
            out.push((2, "cell_temperature", Some("°C")));
        }
        out
    }
}

impl Dataset for ItcDataset {
    fn info(&self) -> Result<FileInfo> {
        let it = &self.itc;
        let mut channels: Vec<SignalChannelInfo> = self
            .channel_columns()
            .into_iter()
            .enumerate()
            .map(|(i, (_, name, unit))| SignalChannelInfo {
                index: i as u32,
                name: name.into(),
                unit: unit.map(str::to_string),
                dtype: "float64".into(),
                scale: 1.0,
                offset: 0.0,
                extra: BTreeMap::new(),
            })
            .collect();
        for c in 3..it.columns.len() {
            channels.push(SignalChannelInfo {
                index: channels.len() as u32,
                name: format!("column_{}", c + 1),
                unit: None,
                dtype: "float64".into(),
                scale: 1.0,
                offset: 0.0,
                extra: BTreeMap::from([(
                    "note".to_string(),
                    json!("a further data column whose meaning has not been identified"),
                )]),
            });
        }
        let mut extra = BTreeMap::new();
        extra.insert("kind".into(), json!("thermogram"));
        extra.insert("plot".into(), json!("single"));
        let (rate, start) = match it.regular_time() {
            Some((t0, step)) if step > 0.0 => {
                extra.insert(
                    "axis".into(),
                    json!({"quantity": "time", "unit": "s", "first": t0, "step": step}),
                );
                (1.0 / step, Some(t0))
            }
            _ => (0.0, None),
        };
        extra.insert("injection_table".into(), json!(0));
        let trace = TraceInfo {
            index: 0,
            name: Some("thermogram".into()),
            sample_rate_hz: rate,
            sample_count: it.rows() as u64,
            sweep_count: 1,
            channels,
            start_s: start,
            extra,
        };
        let col = |i: u32, name: &str, unit: Option<&str>| ColumnInfo {
            index: i,
            name: name.into(),
            label: None,
            dtype: "float64".into(),
            unit: unit.map(str::to_string),
            range: None,
            extra: BTreeMap::new(),
        };
        let table = TableInfo {
            index: 0,
            name: Some("injections".into()),
            row_count: it.injections.len() as u64,
            columns: vec![
                col(0, "injection", None),
                col(1, "volume", Some("µL")),
                col(2, "duration", Some("s")),
                col(3, "spacing", Some("s")),
                col(4, "filter_period", Some("s")),
                col(5, "start", Some("s")),
                col(6, "first_sample", None),
            ],
            extra: BTreeMap::from([("trace".to_string(), json!(0))]),
        };
        let mut notes = Vec::new();
        if it.findings.iter().any(|f| f.severity == Severity::Warning) {
            notes.push("the header and the data disagree in places; run `check`".into());
        }
        notes.push("differential power as recorded; integrated heats (the analysis software's baseline and integration) are not stored in .itc files".into());
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.size,
            format: self.descriptor.clone(),
            format_version: it.software.clone(),
            plane_count: 0,
            images: Vec::new(),
            tables: vec![table],
            spectra: Vec::new(),
            traces: vec![trace],
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let mut m = self.itc.header.clone();
        m.insert("comment".into(), json!(self.itc.comment));
        Ok(json!({ "microcal_itc": Value::Object(m) }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        p.insert("traces".into(), Source::Inferred);
        p.insert("tables".into(), Source::Inferred);
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let mut e = Vec::new();
        if let Some(r) = self.itc.baseline_row {
            e.push(LsEntry {
                kind: "block".into(),
                name: "@0 baseline".into(),
                offset: None,
                size: None,
                image: None,
                details: json!({"first_sample": r}),
            });
        }
        for (k, i) in self.itc.injections.iter().enumerate() {
            e.push(LsEntry {
                kind: "block".into(),
                name: format!("@{} injection", k + 1),
                offset: None,
                size: None,
                image: None,
                details: json!({"first_sample": i.first_row, "volume_ul": i.volume_ul}),
            });
        }
        Ok(e)
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::Usage(
            "an ITC file holds a thermogram (trace 0) and its injections (table 0), not images"
                .into(),
        ))
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = self.check_headers()?;
        r.performed("every data row parsed; values checked for non-finite numbers");
        let bad: usize = self
            .itc
            .columns
            .iter()
            .map(|c| c.iter().filter(|v| !v.is_finite()).count())
            .sum();
        if bad > 0 {
            r.push(Finding::warning(
                "non_finite_values",
                format!("{bad} non-finite numbers in the data"),
            ));
        }
        if self.itc.regular_time().is_none() {
            r.push(Finding::info(
                "uneven_sampling",
                "the time column is not evenly spaced: the trace carries it as channel 0",
            ));
        }
        Ok(r)
    }

    fn check_headers(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), self.descriptor.id.clone());
        r.performed("header parsed; injection lines counted against the declared number and the data blocks");
        for f in &self.itc.findings {
            r.push(f.clone());
        }
        Ok(r)
    }

    fn read_trace(&mut self, index: u32, sweep: u32, first: u64, max: u64) -> Result<Trace> {
        if index != 0 || sweep != 0 {
            return Err(Error::Usage(format!(
                "trace {index} sweep {sweep} out of range (one trace, one sweep)"
            )));
        }
        let n = self.itc.rows() as u64;
        let a = first.min(n) as usize;
        let b = first.saturating_add(max).min(n) as usize;
        let mut channels: Vec<Vec<f64>> = self
            .channel_columns()
            .into_iter()
            .map(|(c, _, _)| self.itc.columns[c][a..b].to_vec())
            .collect();
        for c in 3..self.itc.columns.len() {
            channels.push(self.itc.columns[c][a..b].to_vec());
        }
        Ok(Trace {
            trace: 0,
            sweep: 0,
            first_sample: a as u64,
            channels,
        })
    }

    fn read_table(&mut self, index: u32, first_row: u64, max_rows: u64) -> Result<Table> {
        if index != 0 {
            return Err(Error::Usage(format!(
                "table {index} out of range (one table)"
            )));
        }
        let inj = &self.itc.injections;
        let n = inj.len() as u64;
        let a = first_row.min(n) as usize;
        let b = first_row.saturating_add(max_rows).min(n) as usize;
        let t = &self.itc.columns[0];
        let rows = &inj[a..b];
        let start = |i: &Injection| {
            i.start_s
                .or_else(|| i.first_row.and_then(|r| t.get(r).copied()))
                .unwrap_or(f64::NAN)
        };
        Ok(Table {
            table: 0,
            first_row: a as u64,
            columns: vec![
                (a..b).map(|k| (k + 1) as f64).collect(),
                rows.iter().map(|i| i.volume_ul).collect(),
                rows.iter().map(|i| i.duration_s).collect(),
                rows.iter().map(|i| i.spacing_s).collect(),
                rows.iter().map(|i| i.filter_s).collect(),
                rows.iter().map(start).collect(),
                rows.iter()
                    .map(|i| i.first_row.map_or(f64::NAN, |r| r as f64))
                    .collect(),
            ],
        })
    }

    fn experiment(&self) -> Option<Experiment> {
        let it = &self.itc;
        let mut e = Experiment::default();
        let o = |from: &str| Origin {
            source: Source::Inferred,
            from: from.into(),
        };
        let mut ins = ExperimentInstrument {
            vendor: Some("Malvern Panalytical (MicroCal)".into()),
            kind: vocab::term("OBI:0000930"),
            ..ExperimentInstrument::default()
        };
        if let Some(m) = it.model() {
            ins.model = Some(m);
            e.provenance.insert(
                "instrument.model".into(),
                o("first `%` line (instrument identifier)"),
            );
        }
        if let Some(s) = &it.software {
            let (name, ver) = s
                .split_once(" Ver:")
                .map_or((s.as_str(), None), |(n, v)| (n.trim(), Some(v)));
            ins.software = Some(name.to_string());
            if let Some(v) = ver {
                let v = v.split("Run time").next().unwrap_or(v).trim();
                if !v.is_empty() {
                    ins.software_version = Some(v.to_string());
                    e.provenance
                        .insert("instrument.software_version".into(), o("last `%` line"));
                }
            }
            e.provenance
                .insert("instrument.software".into(), o("last `%` line"));
        }
        e.instrument = Some(ins);
        let mut m = Method {
            technique: vocab::term("CHMO:0000683"),
            ..Method::default()
        };
        let mut param = |k: &str, v: Option<f64>, unit: &str, from: &str| {
            if let Some(v) = v {
                m.parameters.insert(k.into(), Quantity::number(v, unit));
                e.provenance
                    .insert(format!("method.parameters.{k}"), o(from));
            }
        };
        param("cell_concentration", it.cell_mm, "mM", "third `#` line");
        param(
            "syringe_concentration",
            it.syringe_mm,
            "mM",
            "second `#` line",
        );
        param("cell_volume", it.cell_volume_ml, "mL", "fourth `#` line");
        param(
            "temperature",
            it.target_temperature_c,
            "°C",
            "fourth `$` line",
        );
        param("initial_delay", it.initial_delay_s, "s", "fifth `$` line");
        param("stirring_speed", it.stirring_rpm, "rpm", "sixth `$` line");
        param(
            "reference_power",
            it.reference_power,
            "µcal/s",
            "seventh `$` line",
        );
        if let Some(n) = it.declared_injections {
            m.parameters.insert("injections".into(), Quantity::plain(n));
            e.provenance
                .insert("method.parameters.injections".into(), o("second `$` line"));
        }
        e.method = Some(m);
        if !it.comment.is_empty() {
            e.acquisition = Some(Acquisition {
                comment: Some(it.comment.clone()),
                duration_s: it.columns.first().and_then(|t| t.last().copied()),
                ..Acquisition::default()
            });
            e.provenance
                .insert("acquisition.comment".into(), o("`?` comment lines"));
        } else if let Some(t) = it.columns.first().and_then(|t| t.last().copied()) {
            e.acquisition = Some(Acquisition {
                duration_s: Some(t),
                ..Acquisition::default()
            });
        }
        e.provenance
            .insert("acquisition.duration_s".into(), o("last data row's time"));
        e.measurements.push(Measurement {
            kind: MeasurementKind::Trace,
            indices: vec![0],
            what: format!(
                "isothermal titration calorimetry: {} injections, differential power over {} samples",
                it.injections.len(),
                it.rows()
            ),
            technique: vocab::term("CHMO:0000683"),
            terms: Vec::new(),
            parameters: BTreeMap::new(),
        });
        crate::complete_provenance(&mut e);
        Some(e)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp)] // values parsed from short decimal text
    use super::*;

    const SAMPLE: &str = "$ITC\r\n$ 2 \r\n$NOT\r\n$ 25 \r\n$ 60 \r\n$ 750 \r\n$ 10 \r\n$ 2 \r\n$ADCGainCode:  0 \r\n$False,True,True\r\n$ 0.4 , 0.8 , 150 , 5 \r\n$ 2 , 4 , 150 , 5 \r\n# 0 \r\n# 0.5 \r\n# 0.02 \r\n# 0.2 \r\n# 25 \r\n# 4 \r\n# 1 \r\n?\r\nlysozyme into NAG3\r\n% ITC200_12.08.168\r\n% 0.2 \r\n% ITC200 Ver: 1.26.1\r\n@0\r\n1.00,5.0,25.0\r\n2.00,5.1,25.0\r\n@1,0.4000,0.8, 3.0 \r\n3.00,4.0,25.0\r\n4.00,4.9,25.0\r\n@2,2.0000,4.0, 5.0 \r\n5.00,3.0,25.0\r\n6.00,5.0,25.0\r\n";

    #[test]
    fn header_blocks_and_rows() {
        let it = parse(SAMPLE).unwrap();
        assert_eq!(it.declared_injections, Some(2));
        assert_eq!(it.injections.len(), 2);
        assert_eq!(it.injections[1].volume_ul, 2.0);
        assert_eq!(it.injections[1].first_row, Some(4));
        assert_eq!(it.injections[0].start_s, Some(3.0));
        assert_eq!(it.cell_mm, Some(0.02));
        assert_eq!(it.syringe_mm, Some(0.5));
        assert_eq!(it.cell_volume_ml, Some(0.2));
        assert_eq!(it.comment, "lysozyme into NAG3");
        assert_eq!(it.model().as_deref(), Some("iTC200"));
        assert_eq!(it.regular_time(), Some((1.0, 1.0)));
        assert!(it.findings.is_empty(), "{:?}", it.findings);
    }

    #[test]
    fn damage() {
        assert!(parse("hello").is_err());
        assert!(parse("$ITC\r\n$ 2\r\n").is_err());
        // a run that stopped after the first injection
        let short = &SAMPLE[..SAMPLE.find("@2").unwrap()];
        let it = parse(short).unwrap();
        assert!(it.findings.iter().any(|f| f.code == "incomplete_run"));
        for cut in (0..SAMPLE.len()).step_by(7) {
            let _ = parse(&SAMPLE[..cut]);
        }
    }
}
