//! Axon Text File (ATF 1.0) exported by Clampfit/pCLAMP: a text header and a tab-separated table
//! whose first column is time and whose other columns are one channel of one sweep each. Layout
//! and names: `docs/formats/abf.md` (section ATF).

use openreadout_core::source::Input;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use openreadout_core::bytes::latin1;
use openreadout_core::model::{
    CheckReport, DetectConfidence, FileInfo, FormatDescriptor, LsEntry, SignalChannelInfo, Trace,
    TraceInfo,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, Detection, FormatReader, PlaneIndex, has_extension};
use openreadout_core::{Error, Finding, Plane, Result};
use serde_json::{Value, json};

/// Format id of ATF files.
pub const ATF_FORMAT_ID: &str = "atf";
/// Largest ATF file read (the table is indexed line by line).
pub const MAX_ATF_LEN: u64 = 2 << 30;
/// Most header records accepted.
pub const MAX_HEADER_RECORDS: usize = 10_000;
/// Most columns accepted.
pub const MAX_COLUMNS: usize = 100_000;

/// The ATF reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct AtfReader;

/// True when `head` starts with the `ATF` signature line.
pub fn looks_like_atf(head: &[u8]) -> bool {
    let line = head.split(|&b| b == b'\n').next().unwrap_or(&[]);
    let text = String::from_utf8_lossy(line);
    let mut words = text.split_whitespace();
    words.next() == Some("ATF") && words.next().is_some_and(|v| v.parse::<f64>().is_ok())
}

impl FormatReader for AtfReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&crate::assurance::ATF)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: ATF_FORMAT_ID.into(),
            name: "Axon Text File (ATF)".into(),
            vendor: "Molecular Devices (Axon Instruments) pCLAMP".into(),
            extensions: vec!["atf".into()],
            family: "electrophysiology".into(),
            can_read: true,
            can_write: false,
            confidence: crate::assurance::ATF.confidence,
            known_gaps: vec![
                "Only time-series ATF (first column time) is read; GenePix results files (also ATF) are read by `genepix-gpr`, GenePix array lists exit 6".into(),
                "Values are the decimal text as written by the exporting program (already rounded)".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        // GenePix results and array lists are ATF too; their reader is `genepix-gpr`
        if looks_like_atf(head)
            && String::from_utf8_lossy(&head[..head.len().min(8192)]).contains("\"Type=GenePix")
        {
            return None;
        }
        if looks_like_atf(head) {
            return Some(Detection {
                format_id: ATF_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        has_extension(path, &["atf"]).then(|| Detection {
            format_id: ATF_FORMAT_ID,
            confidence: DetectConfidence::ExtensionOnly,
            note: Some("ATF extension but no `ATF` signature line".into()),
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn reads_any_source(&self) -> bool {
        true
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(AtfDataset::open_input(input)?))
    }
}

/// One data column: its title, channel, sweep and unit.
#[derive(Debug, Clone, PartialEq)]
pub struct AtfColumn {
    /// Column title as written.
    pub title: String,
    /// Channel (signal) index.
    pub channel: usize,
    /// Sweep index.
    pub sweep: usize,
    /// Unit from the title's parentheses.
    pub unit: Option<String>,
}

/// An opened ATF file.
#[derive(Debug)]
pub struct AtfDataset {
    path: PathBuf,
    text: String,
    /// `ATF` version.
    pub version: String,
    /// Header records in order (key, values).
    pub header: Vec<(String, Vec<String>)>,
    /// Title of the first (time) column.
    pub time_title: String,
    /// Multiplier from the time column's unit to seconds.
    pub time_scale: f64,
    /// Data columns (all but time).
    pub columns: Vec<AtfColumn>,
    /// Channel names (signals) in order of first appearance.
    pub channels: Vec<String>,
    /// Sweeps.
    pub sweeps: usize,
    /// Byte offset of every data row.
    pub rows: Vec<usize>,
    /// First two times (s), for the rate.
    pub times: (f64, f64),
    findings: Vec<Finding>,
}

fn unquote(s: &str) -> String {
    s.trim().trim_matches('"').trim().to_string()
}

/// `Trace #1 (pA)` → `pA`.
fn unit_of(title: &str) -> Option<String> {
    let open = title.rfind('(')?;
    let close = title[open..].find(')')? + open;
    let u = title[open + 1..close].trim();
    (!u.is_empty()).then(|| u.to_string())
}

fn corrupt(d: impl Into<String>) -> Error {
    Error::corrupt(ATF_FORMAT_ID, d.into())
}

impl AtfDataset {
    /// Read and index an ATF file.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    #[allow(clippy::many_single_char_names)]
    pub(crate) fn open_input(input: &Input) -> Result<Self> {
        let path = input.path();
        let f = input.open()?;
        let len = f.metadata().map_err(|e| Error::io(path, e))?.len();
        if len > MAX_ATF_LEN {
            return Err(Error::unsupported(
                ATF_FORMAT_ID,
                format!("an ATF file of {len} bytes"),
                "ATF files up to 2 GiB are read; export the recording as ABF instead.",
            ));
        }
        let mut bytes = Vec::new();
        f.take(MAX_ATF_LEN)
            .read_to_end(&mut bytes)
            .map_err(|e| Error::io(path, e))?;
        let text = String::from_utf8(bytes).unwrap_or_else(|e| latin1(e.as_bytes()));
        let mut lines = text.split_inclusive('\n');
        let mut at = 0usize;
        let mut next = |what: &str| -> Result<(usize, String)> {
            let l = lines
                .next()
                .ok_or_else(|| corrupt(format!("the file ends before its {what}")))?;
            let start = at;
            at += l.len();
            Ok((start, l.trim_end_matches(['\r', '\n']).to_string()))
        };
        let (_, sig) = next("signature line")?;
        let mut w = sig.split_whitespace();
        if w.next() != Some("ATF") {
            return Err(corrupt("no ATF signature line"));
        }
        let version = w.next().unwrap_or("").to_string();
        let (_, counts) = next("header counts")?;
        let n: Vec<usize> = counts
            .split_whitespace()
            .filter_map(|x| x.parse().ok())
            .collect();
        let [n_header, n_columns] = n[..] else {
            return Err(corrupt(format!("header counts line {counts:?}")));
        };
        if n_header > MAX_HEADER_RECORDS || n_columns == 0 || n_columns > MAX_COLUMNS {
            return Err(corrupt(format!(
                "{n_header} header records and {n_columns} columns"
            )));
        }
        let mut header = Vec::new();
        for _ in 0..n_header {
            let (_, l) = next("header records")?;
            let fields: Vec<&str> = l.split('\t').collect();
            let first = unquote(fields[0]);
            let (key, mut values) = match first.split_once('=') {
                Some((k, v)) => (
                    k.trim().to_string(),
                    if v.is_empty() {
                        Vec::new()
                    } else {
                        vec![v.to_string()]
                    },
                ),
                None => (first.clone(), Vec::new()),
            };
            values.extend(
                fields[1..]
                    .iter()
                    .map(|v| unquote(v))
                    .filter(|v| !v.is_empty()),
            );
            header.push((key, values));
        }
        let (_, titles) = next("column titles")?;
        let titles: Vec<String> = titles.split('\t').map(unquote).collect();
        if titles.len() < n_columns {
            return Err(corrupt(format!(
                "{} column titles for {n_columns} columns",
                titles.len()
            )));
        }
        let time_title = titles[0].clone();
        let time_scale = match unit_of(&time_title).as_deref() {
            Some("s") | None => 1.0,
            Some("ms") => 1e-3,
            Some("us" | "µs") => 1e-6,
            Some(u) => {
                return Err(Error::unsupported(
                    ATF_FORMAT_ID,
                    format!("an ATF table whose first column is {time_title:?} ({u})"),
                    "Only time-series ATF files (first column time in s or ms, as Clampfit exports them) are read.",
                ));
            }
        };
        if !time_title.to_ascii_lowercase().starts_with("time") {
            return Err(Error::unsupported(
                ATF_FORMAT_ID,
                format!("an ATF table whose first column is {time_title:?}"),
                "Only time-series ATF files (first column time, as Clampfit exports them) are read; GenePix result files are not.",
            ));
        }
        let signals: Vec<String> = header
            .iter()
            .find(|(k, _)| k == "Signals")
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        let mut channels: Vec<String> = Vec::new();
        let mut seen: BTreeMap<usize, usize> = BTreeMap::new();
        let mut columns = Vec::new();
        for (k, title) in titles[1..n_columns].iter().enumerate() {
            let signal = signals.get(k).cloned().unwrap_or_else(|| title.clone());
            let ch = if let Some(i) = channels.iter().position(|c| *c == signal) {
                i
            } else {
                channels.push(signal);
                channels.len() - 1
            };
            let sweep = *seen.get(&ch).unwrap_or(&0);
            seen.insert(ch, sweep + 1);
            columns.push(AtfColumn {
                title: title.clone(),
                channel: ch,
                sweep,
                unit: unit_of(title),
            });
        }
        let sweeps = seen.values().copied().max().unwrap_or(0);
        let mut findings = Vec::new();
        if seen.values().any(|&n| n != sweeps) {
            findings.push(Finding::warning(
                "uneven_sweeps",
                "channels have different numbers of sweep columns",
            ));
        }
        let mut rows = Vec::new();
        let mut times = (f64::NAN, f64::NAN);
        for l in lines {
            let start = at;
            at += l.len();
            if l.trim().is_empty() {
                continue;
            }
            if rows.len() < 2 {
                let t = l
                    .split('\t')
                    .next()
                    .and_then(|v| v.trim().parse::<f64>().ok())
                    .unwrap_or(f64::NAN);
                if rows.is_empty() {
                    times.0 = t * time_scale;
                } else {
                    times.1 = t * time_scale;
                }
            }
            rows.push(start);
        }
        Ok(AtfDataset {
            path: path.to_path_buf(),
            text,
            version,
            header,
            time_title,
            time_scale,
            columns,
            channels,
            sweeps,
            rows,
            times,
            findings,
        })
    }

    fn rate(&self) -> f64 {
        let dt = self.times.1 - self.times.0;
        if dt.is_finite() && dt > 0.0 {
            1.0 / dt
        } else {
            0.0
        }
    }

    fn header_value(&self, key: &str) -> Option<&Vec<String>> {
        self.header.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// Parse row `r` (all columns, time first).
    fn row(&self, r: usize) -> Result<Vec<f64>> {
        let start = self.rows[r];
        let end = self.rows.get(r + 1).copied().unwrap_or(self.text.len());
        let line = self.text[start..end].trim_end_matches(['\r', '\n']);
        let v: Vec<f64> = line
            .split('\t')
            .map(|x| {
                let x = x.trim();
                if x.is_empty() {
                    Ok(f64::NAN)
                } else {
                    x.parse::<f64>()
                }
            })
            .collect::<std::result::Result<_, _>>()
            .map_err(|e| corrupt(format!("data row {r}: {e}")))?;
        if v.len() < self.columns.len() + 1 {
            return Err(corrupt(format!(
                "data row {r} has {} values for {} columns (truncated?)",
                v.len(),
                self.columns.len() + 1
            )));
        }
        Ok(v)
    }
}

impl Dataset for AtfDataset {
    fn info(&self) -> Result<FileInfo> {
        let channels = self
            .channels
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let unit = self
                    .columns
                    .iter()
                    .find(|c| c.channel == i)
                    .and_then(|c| c.unit.clone());
                SignalChannelInfo {
                    index: i as u32,
                    name: name.clone(),
                    unit,
                    dtype: "float64".into(),
                    scale: 1.0,
                    offset: 0.0,
                    extra: BTreeMap::new(),
                }
            })
            .collect();
        let mut extra = BTreeMap::new();
        for (k, v) in &self.header {
            let key = match k.as_str() {
                "AcquisitionMode" => "acquisition_mode",
                "Comment" => "comment",
                "SweepStartTimesMS" => "sweep_start_times_ms",
                "SignalsExported" => "signals_exported",
                "SyncTimeUnits" => "sync_time_units",
                _ => continue,
            };
            extra.insert(key.into(), json!(v.join(",")));
        }
        extra.insert("atf_version".into(), json!(self.version));
        extra.insert("time_column".into(), json!(self.time_title));
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.text.len() as u64,
            format: AtfReader.descriptor(),
            format_version: Some(format!("ATF {}", self.version)),
            images: Vec::new(),
            tables: Vec::new(),
            spectra: Vec::new(),
            traces: vec![TraceInfo {
                index: 0,
                name: None,
                sample_rate_hz: self.rate(),
                sample_count: self.rows.len() as u64,
                sweep_count: self.sweeps as u32,
                channels,
                start_s: self.times.0.is_finite().then_some(self.times.0),
                extra,
            }],
            plane_count: 0,
            notes: vec![format!(
                "Axon Text File: {} sweep(s) × {} channel(s), one column each, sharing the time column; values as written in the text",
                self.sweeps,
                self.channels.len()
            )],
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        Ok(json!({
            "version": self.version,
            "header": self.header.iter().map(|(k, v)| json!({"key": k, "values": v})).collect::<Vec<_>>(),
            "time_column": self.time_title,
            "columns": self.columns.iter().map(|c| json!({"title": c.title, "channel": c.channel, "sweep": c.sweep, "unit": c.unit})).collect::<Vec<_>>(),
        }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for k in [
            "traces[].channels[].name",
            "traces[].channels[].unit",
            "traces[].sweep_count",
        ] {
            p.insert(k.into(), Source::PriorArt);
        }
        p.insert("traces[].sample_rate_hz".into(), Source::Inferred);
        p.insert("traces[].start_s".into(), Source::Inferred);
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        Ok(vec![
            LsEntry {
                kind: "header".into(),
                name: "ATF header".into(),
                offset: Some(0),
                size: self.rows.first().map(|r| *r as u64),
                image: None,
                details: json!({"records": self.header.len(), "columns": self.columns.len() + 1}),
            },
            LsEntry {
                kind: "records".into(),
                name: "data rows".into(),
                offset: self.rows.first().map(|r| *r as u64),
                size: self.rows.first().map(|r| (self.text.len() - r) as u64),
                image: None,
                details: json!({"rows": self.rows.len()}),
            },
        ])
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::unsupported(
            ATF_FORMAT_ID,
            "image planes",
            "ATF files hold sampled signals: use `openreadout trace` or `openreadout export --format csv`.",
        ))
    }

    fn read_trace(
        &mut self,
        index: u32,
        sweep: u32,
        first_sample: u64,
        max_samples: u64,
    ) -> Result<Trace> {
        if index != 0 {
            return Err(Error::Usage(format!(
                "trace {index} out of range (an ATF file holds one trace, index 0)"
            )));
        }
        if sweep as usize >= self.sweeps {
            return Err(Error::Usage(format!(
                "sweep {sweep} out of range (file has {} sweeps)",
                self.sweeps
            )));
        }
        let total = self.rows.len() as u64;
        if first_sample > total {
            return Err(Error::Usage(format!(
                "first sample {first_sample} is past the end ({total} samples)"
            )));
        }
        let n = max_samples.min(total - first_sample).min(1 << 24);
        let cols: Vec<Option<usize>> = (0..self.channels.len())
            .map(|ch| {
                self.columns
                    .iter()
                    .position(|c| c.channel == ch && c.sweep == sweep as usize)
            })
            .collect();
        let mut out: Vec<Vec<f64>> = (0..cols.len())
            .map(|_| Vec::with_capacity(n as usize))
            .collect();
        for r in first_sample..first_sample + n {
            let v = self.row(r as usize)?;
            for (c, col) in cols.iter().enumerate() {
                out[c].push(col.map_or(f64::NAN, |k| v[k + 1]));
            }
        }
        Ok(Trace {
            trace: 0,
            sweep,
            first_sample,
            channels: out,
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), ATF_FORMAT_ID);
        r.performed("ATF signature, header counts, header records and column titles");
        r.performed("every data row holds a number for every column (truncation)");
        r.performed("the time column advances by a constant step");
        for f in &self.findings {
            r.push(f.clone());
        }
        let dt = self.times.1 - self.times.0;
        let mut irregular = 0u64;
        let mut prev: Option<f64> = None;
        for k in 0..self.rows.len() {
            match self.row(k) {
                Ok(v) => {
                    let t = v[0] * self.time_scale;
                    if let Some(p) = prev
                        && ((t - p) - dt).abs() > 1e-6 * dt.abs().max(1e-12)
                    {
                        irregular += 1;
                    }
                    prev = Some(t);
                }
                Err(e) => {
                    r.push(Finding::error("truncated", e.to_string()));
                    break;
                }
            }
        }
        if !self.rows.is_empty() && !self.text.ends_with('\n') {
            r.push(
                Finding::error(
                    "truncated",
                    "the last data row has no line end: the file was cut off (its last value may be incomplete)",
                )
                .at(self.text.len() as u64),
            );
        }
        if irregular > 0 {
            r.push(Finding::warning(
                "irregular_time",
                format!("{irregular} time steps differ from the first"),
            ));
        }
        if self.header_value("Signals").is_none() {
            r.push(Finding::info(
                "no_signals",
                "no `Signals=` header record: every column is its own channel",
            ));
        }
        if self.rows.is_empty() {
            r.push(Finding::warning(
                "no_samples",
                "the file holds no data rows",
            ));
        }
        Ok(r)
    }
}
