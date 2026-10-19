//! European Data Format files: EDF, EDF+ (`.edf`), and BioSemi's 24-bit variants BDF and BDF+
//! (`.bdf`). A 256-byte header, 256 bytes per signal, then data records holding each signal's
//! samples in turn. Signals that share a sample rate form one trace; the TALs of the
//! annotation signals form the `annotations` table. Notes and vocabulary: `docs/formats/edf.md`;
//! provenance: `docs/provenance/edf.md`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_core::bytes::{latin1, read_block};
use openreadout_core::model::{
    CheckReport, ColumnInfo, DetectConfidence, FileInfo, Finding, FormatDescriptor, LsEntry,
    Severity, SignalChannelInfo, Table, TableInfo, Trace, TraceInfo,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, Detection, FormatReader, PlaneIndex, has_extension};
use openreadout_core::source::{Fs, Input, SourceFile};
use openreadout_core::{Error, Plane, Result};
use serde_json::{Value, json};

/// Format id of EDF, EDF+, BDF and BDF+ files.
pub const EDF_FORMAT_ID: &str = "edf";
/// Bytes of the fixed header, and of each signal's header.
pub const EDF_HEADER_BLOCK: u64 = 256;
/// Signals accepted at most (the field has four digits).
pub const MAX_EDF_SIGNALS: usize = 9999;
/// Annotation bytes read at most when the file is opened.
pub const MAX_EDF_ANNOTATION_BYTES: u64 = 256 << 20;
/// Annotations kept at most.
pub const MAX_EDF_ANNOTATIONS: usize = 4_000_000;
/// Samples per channel decoded per `read_trace` call at most.
pub const MAX_EDF_READ: u64 = 1 << 24;
/// Bytes of data records read per `read_trace` call at most (one record is always read).
pub const MAX_EDF_READ_BYTES: u64 = 64 << 20;
/// Gap between the end of one data record and the start of the next that still counts as
/// contiguous, seconds.
pub const EDF_CONTIGUOUS_TOLERANCE_S: f64 = 1e-6;

/// One signal of the header.
#[derive(Debug, Clone, PartialEq)]
pub struct EdfSignal {
    /// Label (16 characters), e.g. `EEG Fpz-Cz`.
    pub label: String,
    /// Transducer type (80 characters).
    pub transducer: String,
    /// Physical dimension (8 characters), the unit of the physical values.
    pub physical_dimension: String,
    /// Physical minimum.
    pub physical_min: f64,
    /// Physical maximum.
    pub physical_max: f64,
    /// Digital minimum.
    pub digital_min: f64,
    /// Digital maximum.
    pub digital_max: f64,
    /// Prefiltering (80 characters), e.g. `HP:0.1Hz LP:75Hz`.
    pub prefiltering: String,
    /// Samples in each data record.
    pub samples_per_record: u64,
    /// Reserved (32 characters).
    pub reserved: String,
    /// An `EDF Annotations` / `BDF Annotations` signal, which holds TALs, not samples.
    pub annotations: bool,
    /// Byte offset of this signal's samples within a data record.
    pub offset_in_record: u64,
}

impl EdfSignal {
    /// Physical units per digital step: (physical max − physical min) / (digital max − digital
    /// min); 1 when the digital range is empty.
    pub fn scale(&self) -> f64 {
        let d = self.digital_max - self.digital_min;
        let s = (self.physical_max - self.physical_min) / d;
        if d == 0.0 || !s.is_finite() { 1.0 } else { s }
    }
    /// Physical value of digital 0: physical min − digital min × scale; 0 when the digital
    /// range is empty.
    pub fn offset(&self) -> f64 {
        if self.calibrated() {
            self.physical_min - self.digital_min * self.scale()
        } else {
            0.0
        }
    }
    /// The four extremes give a usable linear map.
    pub fn calibrated(&self) -> bool {
        let d = self.digital_max - self.digital_min;
        d != 0.0
            && ((self.physical_max - self.physical_min) / d).is_finite()
            && self.physical_min.is_finite()
    }
}

/// One annotation of a TAL.
#[derive(Debug, Clone, PartialEq)]
pub struct EdfAnnotation {
    /// Onset in seconds after the first data record's start (the header's start time plus the
    /// subsecond start).
    pub onset_s: f64,
    /// Duration in seconds (0 when the TAL gives none).
    pub duration_s: f64,
    /// The text.
    pub text: String,
}

/// The parsed file.
#[derive(Debug, Clone)]
pub struct EdfFile {
    /// 24-bit samples (BDF, BDF+) rather than 16-bit (EDF, EDF+).
    pub bdf: bool,
    /// The version field as written (`0` for EDF, byte 255 + `BIOSEMI` for BDF).
    pub version_field: String,
    /// Local patient identification (80 characters).
    pub patient: String,
    /// Local recording identification (80 characters).
    pub recording: String,
    /// Start date as written (`dd.mm.yy`).
    pub start_date: String,
    /// Start time as written (`hh.mm.ss`).
    pub start_time: String,
    /// Header bytes as the header states them.
    pub header_bytes: u64,
    /// The reserved field (`EDF+C`, `EDF+D`, `BDF+C`, `24BIT`, …).
    pub reserved: String,
    /// Data records as the header states them (−1 while recording).
    pub declared_records: i64,
    /// Data records read: whole records in the file, at most the declared count.
    pub records: u64,
    /// Duration of a data record, seconds.
    pub record_duration_s: f64,
    /// Signals in header order.
    pub signals: Vec<EdfSignal>,
    /// Bytes of one data record.
    pub record_bytes: u64,
    /// File length.
    pub file_len: u64,
    /// Start of the first data record in seconds after the header's start time: the subsecond
    /// start of EDF+ and BDF+ files, from the first time-keeping TAL (0 otherwise).
    pub start_offset_s: f64,
    /// Start of each data record in seconds after the first one's, from the time-keeping TALs
    /// (EDF+ and BDF+ files; empty otherwise).
    pub record_starts: Vec<f64>,
    /// Every annotation, in file order (time-keeping TALs left out); onsets count from the first
    /// data record's start.
    pub annotations: Vec<EdfAnnotation>,
    /// Problems found while parsing.
    pub findings: Vec<Finding>,
}

fn field(b: &[u8], at: usize, n: usize) -> String {
    latin1(b.get(at..at + n).unwrap_or_default())
        .trim_end_matches(['\0', ' '])
        .trim_start()
        .to_string()
}

fn number(v: &str) -> Option<f64> {
    v.trim()
        .replace(',', ".")
        .parse::<f64>()
        .ok()
        .filter(|x| x.is_finite())
}

/// True when `head` starts with an EDF or BDF header: the version field, then numeric header
/// length and signal count that agree.
pub fn looks_like_edf(head: &[u8]) -> bool {
    if head.len() < 256 {
        return false;
    }
    let bdf = head[0] == 0xFF && &head[1..8] == b"BIOSEMI";
    let edf = head[0] == b'0' && head[1..8].iter().all(|&c| c == b' ');
    if !(bdf || edf) {
        return false;
    }
    let bytes = field(head, 184, 8).parse::<u64>().ok();
    let ns = field(head, 252, 4).parse::<u64>().ok();
    match (bytes, ns) {
        (Some(b), Some(n)) => b == 256 * (n + 1),
        _ => false,
    }
}

impl EdfFile {
    /// `EDF`, `EDF+C`, `EDF+D`, `BDF`, `BDF+C` or `BDF+D`.
    pub fn variant(&self) -> String {
        let base = if self.bdf { "BDF" } else { "EDF" };
        for kind in ["C", "D"] {
            if self.reserved.starts_with(&format!("{base}+{kind}")) {
                return format!("{base}+{kind}");
            }
        }
        base.to_string()
    }
    /// An EDF+ or BDF+ file (the reserved field starts with `EDF+` / `BDF+`).
    pub fn plus(&self) -> bool {
        self.variant().contains('+')
    }
    /// Bytes per sample: 2 (EDF) or 3 (BDF).
    pub fn sample_bytes(&self) -> u64 {
        if self.bdf { 3 } else { 2 }
    }
    /// Offset of the first data record.
    pub fn data_offset(&self) -> u64 {
        EDF_HEADER_BLOCK * (self.signals.len() as u64 + 1)
    }
    /// Offset of data record `k`.
    pub fn record_offset(&self, k: u64) -> u64 {
        self.data_offset() + k * self.record_bytes
    }
    /// Sample rate of a signal: samples per record / record duration.
    pub fn rate_hz(&self, s: &EdfSignal) -> f64 {
        s.samples_per_record as f64 / self.record_duration_s
    }
    /// Groups of ordinary (non-annotation) signals that share a sample count per record, in the
    /// order their first signal appears: each group is a trace.
    pub fn groups(&self) -> Vec<Vec<usize>> {
        let mut out: Vec<(u64, Vec<usize>)> = Vec::new();
        for (i, s) in self.signals.iter().enumerate() {
            if s.annotations || s.samples_per_record == 0 {
                continue;
            }
            match out.iter_mut().find(|(n, _)| *n == s.samples_per_record) {
                Some((_, v)) => v.push(i),
                None => out.push((s.samples_per_record, vec![i])),
            }
        }
        out.into_iter().map(|(_, v)| v).collect()
    }
    /// Data records grouped into runs of contiguous records (`[first, end)`): one run unless an
    /// `EDF+D` / `BDF+D` file's time-keeping TALs show gaps.
    pub fn sweeps(&self) -> Vec<(u64, u64)> {
        if self.records == 0 {
            return Vec::new();
        }
        if !self.variant().ends_with("+D") || self.record_starts.len() as u64 != self.records {
            return vec![(0, self.records)];
        }
        let mut out = Vec::new();
        let mut first = 0u64;
        for k in 1..self.records as usize {
            let expected = self.record_starts[k - 1] + self.record_duration_s;
            if (self.record_starts[k] - expected).abs() > EDF_CONTIGUOUS_TOLERANCE_S {
                out.push((first, k as u64));
                first = k as u64;
            }
        }
        out.push((first, self.records));
        out
    }
    /// Start of data record `k` in seconds after the first one's.
    pub fn record_start(&self, k: u64) -> f64 {
        self.record_starts
            .get(k as usize)
            .copied()
            .unwrap_or(k as f64 * self.record_duration_s)
    }
    /// Recording start, `yyyy-mm-ddThh:mm:ss[.fraction]` local time: the header's date and time
    /// plus `start_offset_s`, the year from the EDF+ `Startdate dd-MMM-yyyy` subfield when it
    /// has one, else from `yy` with 1985 as the clipping year.
    pub fn recorded_at(&self) -> Option<String> {
        let d: Vec<u32> = self
            .start_date
            .split(['.', ':', '-', '/'])
            .map(|x| x.trim().parse().ok())
            .collect::<Option<Vec<u32>>>()?;
        let t: Vec<u32> = self
            .start_time
            .split(['.', ':'])
            .map(|x| x.trim().parse().ok())
            .collect::<Option<Vec<u32>>>()?;
        if d.len() != 3 || t.len() != 3 {
            return None;
        }
        let (mut day, mut month, yy) = (d[0], d[1], d[2]);
        let mut year = if yy >= 85 { 1900 + yy } else { 2000 + yy };
        if let Some((pd, pm, py)) = self.recording_start_date() {
            (day, month, year) = (pd, pm, py);
        }
        if !(1..=31).contains(&day)
            || !(1..=12).contains(&month)
            || t[0] > 23
            || t[1] > 59
            || t[2] > 60
        {
            return None;
        }
        // the first data record starts `start_offset_s` after the header's time
        let whole = self.start_offset_s.floor();
        let frac = self.start_offset_s - whole;
        let day0 = openreadout_core::time::days_from_civil(i64::from(year), month, day);
        let secs = day0 * 86_400
            + i64::from(t[0]) * 3600
            + i64::from(t[1]) * 60
            + i64::from(t[2])
            + whole as i64;
        let (y, m, d) = openreadout_core::time::civil_from_days(secs.div_euclid(86_400));
        let sod = secs.rem_euclid(86_400);
        let fraction = if frac > 0.0 {
            format!("{frac}").trim_start_matches('0').to_string()
        } else {
            String::new()
        };
        Some(format!(
            "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}{fraction}",
            sod / 3600,
            sod / 60 % 60,
            sod % 60
        ))
    }
    /// The EDF+ recording field's subfields after `Startdate`: start date, admin code,
    /// technician, equipment; `None` unless the field starts with `Startdate`.
    pub fn recording_subfields(&self) -> Option<Vec<String>> {
        let mut it = self.recording.split_whitespace();
        (it.next()? == "Startdate").then(|| it.map(str::to_string).collect())
    }
    fn recording_start_date(&self) -> Option<(u32, u32, u32)> {
        let sub = self.recording_subfields()?;
        let mut p = sub.first()?.split('-');
        let day: u32 = p.next()?.parse().ok()?;
        let month = openreadout_core::time::month_from_abbrev(p.next()?)?;
        let year: u32 = p.next()?.parse().ok()?;
        (year >= 1985 && p.next().is_none()).then_some((day, month, year))
    }
}

/// Parse the header and the annotation signals of every data record.
pub fn parse_edf(f: &mut SourceFile, path: &Path, file_len: u64) -> Result<EdfFile> {
    let head = read_block(f, path, 0, EDF_HEADER_BLOCK, file_len)?;
    let b = &head.bytes;
    if b.len() < 256 {
        return Err(Error::corrupt(
            EDF_FORMAT_ID,
            format!("the file has {} bytes; an EDF header has 256", b.len()),
        ));
    }
    let bdf = b[0] == 0xFF;
    if !(bdf && &b[1..8] == b"BIOSEMI" || b[0] == b'0') {
        return Err(Error::corrupt(
            EDF_FORMAT_ID,
            "the version field is neither `0` (EDF) nor byte 255 + `BIOSEMI` (BDF)",
        ));
    }
    let version_field = if bdf {
        "\u{FF}BIOSEMI".to_string()
    } else {
        field(b, 0, 8)
    };
    let mut findings = Vec::new();
    let header_bytes = field(b, 184, 8).parse::<u64>().unwrap_or(0);
    let ns = field(b, 252, 4).parse::<usize>().map_err(|_| {
        Error::corrupt(
            EDF_FORMAT_ID,
            format!(
                "the number of signals {:?} is not a number",
                field(b, 252, 4)
            ),
        )
    })?;
    if ns > MAX_EDF_SIGNALS {
        return Err(Error::corrupt(
            EDF_FORMAT_ID,
            format!("{ns} signals is more than {MAX_EDF_SIGNALS}"),
        ));
    }
    let computed = EDF_HEADER_BLOCK * (ns as u64 + 1);
    if header_bytes != computed {
        findings.push(Finding::warning(
            "header_bytes",
            format!(
                "the header states {header_bytes} header bytes; {ns} signals take {computed}, which is used"
            ),
        ));
    }
    let declared_records = field(b, 236, 8).parse::<i64>().unwrap_or(-1);
    let duration_text = field(b, 244, 8);
    let record_duration_s = number(&duration_text)
        .filter(|d| *d >= 0.0)
        .ok_or_else(|| {
            Error::corrupt(
                EDF_FORMAT_ID,
                format!("the data record duration {duration_text:?} is not a number"),
            )
        })?;
    let sig = read_block(f, path, EDF_HEADER_BLOCK, 256 * ns as u64, file_len)?;
    if (sig.len() as u64) < 256 * ns as u64 {
        return Err(Error::corrupt_at(
            EDF_FORMAT_ID,
            EDF_HEADER_BLOCK,
            "the signal headers are cut off by the end of the file",
        ));
    }
    let s = &sig.bytes;
    // field-major: every signal's label, then every signal's transducer, …
    let column = |start: usize, width: usize, i: usize| field(s, start * ns + i * width, width);
    let widths = [16usize, 80, 8, 8, 8, 8, 8, 80, 8, 32];
    let mut starts = [0usize; 10];
    for k in 1..10 {
        starts[k] = starts[k - 1] + widths[k - 1];
    }
    let bdf_ann = "BDF Annotations";
    let mut signals = Vec::with_capacity(ns);
    let mut offset = 0u64;
    let sample_bytes = if bdf { 3 } else { 2 };
    for i in 0..ns {
        let get = |k: usize| column(starts[k], widths[k], i);
        let label = get(0);
        let num = |k: usize| number(&get(k));
        let samples_per_record = get(8).parse::<u64>().map_err(|_| {
            Error::corrupt(
                EDF_FORMAT_ID,
                format!(
                    "signal {i}: samples per record {:?} is not a number",
                    get(8)
                ),
            )
        })?;
        let (pmin, pmax, dmin, dmax) = (num(3), num(4), num(5), num(6));
        if pmin.is_none() || pmax.is_none() || dmin.is_none() || dmax.is_none() {
            findings.push(Finding::error(
                "bad_range",
                format!("signal {i} ({label}): a physical or digital extreme is not a number"),
            ));
        }
        let annotations = label == "EDF Annotations" || label == bdf_ann;
        signals.push(EdfSignal {
            transducer: get(1),
            physical_dimension: get(2),
            physical_min: pmin.unwrap_or(f64::NAN),
            physical_max: pmax.unwrap_or(f64::NAN),
            digital_min: dmin.unwrap_or(f64::NAN),
            digital_max: dmax.unwrap_or(f64::NAN),
            prefiltering: get(7),
            samples_per_record,
            reserved: get(9),
            annotations,
            offset_in_record: offset,
            label,
        });
        offset = samples_per_record
            .checked_mul(sample_bytes)
            .and_then(|n| offset.checked_add(n))
            .ok_or_else(|| Error::corrupt(EDF_FORMAT_ID, "the data record size overflows"))?;
    }
    let mut out = EdfFile {
        bdf,
        version_field,
        patient: field(b, 8, 80),
        recording: field(b, 88, 80),
        start_date: field(b, 168, 8),
        start_time: field(b, 176, 8),
        header_bytes,
        reserved: field(b, 192, 44),
        declared_records,
        records: 0,
        record_duration_s,
        signals,
        record_bytes: offset,
        file_len,
        start_offset_s: 0.0,
        record_starts: Vec::new(),
        annotations: Vec::new(),
        findings,
    };
    let data = file_len.saturating_sub(out.data_offset());
    let whole = data.checked_div(out.record_bytes).unwrap_or(0);
    out.records = if declared_records < 0 {
        out.findings.push(Finding::warning(
            "records_unknown",
            format!("the header gives −1 data records (still recording); the file holds {whole} whole records"),
        ));
        whole
    } else {
        let d = declared_records as u64;
        if whole < d {
            out.findings.push(
                Finding::error(
                    "truncated",
                    format!(
                        "the header declares {d} data records; the file holds {whole} whole ones"
                    ),
                )
                .at(out.record_offset(whole)),
            );
        } else if out.record_bytes > 0 && data > d * out.record_bytes {
            out.findings.push(Finding::warning(
                "extra_bytes",
                format!(
                    "{} bytes after the {d} declared data records",
                    data - d * out.record_bytes
                ),
            ));
        }
        whole.min(d)
    };
    if !out.groups().is_empty() && record_duration_s == 0.0 {
        return Err(Error::unsupported(
            EDF_FORMAT_ID,
            "a zero data record duration with ordinary signals",
            "The header gives a data record duration of 0, so the sample rates cannot be derived; the file may be damaged.",
        ));
    }
    read_annotations(f, path, &mut out)?;
    Ok(out)
}

/// Read every annotation signal of every record: time-keeping TALs give the record starts, the
/// others the annotations.
fn read_annotations(f: &mut SourceFile, path: &Path, out: &mut EdfFile) -> Result<()> {
    let ann: Vec<(u64, u64)> = out
        .signals
        .iter()
        .filter(|s| s.annotations)
        .map(|s| {
            (
                s.offset_in_record,
                s.samples_per_record * out.sample_bytes(),
            )
        })
        .collect();
    if ann.is_empty() || out.records == 0 {
        return Ok(());
    }
    let per_record: u64 = ann.iter().map(|a| a.1).sum();
    let budget = (MAX_EDF_ANNOTATION_BYTES / per_record.max(1)).max(1);
    let records = out.records.min(budget);
    if records < out.records {
        out.findings.push(Finding::warning(
            "annotations_capped",
            format!(
                "annotations were read from the first {records} of {} data records",
                out.records
            ),
        ));
    }
    // whole runs of records when the annotations are a large part of each record, else one
    // small read per record and annotation signal
    let chunked = per_record * 8 >= out.record_bytes;
    let chunk_records = if chunked {
        ((8u64 << 20) / out.record_bytes.max(1)).max(1)
    } else {
        1
    };
    let plus = out.plus();
    let mut missing_time = 0u64;
    let mut bad_tals = 0u64;
    // onsets and record starts in nanoseconds after the header's start time, exact
    let mut starts: Vec<i128> = Vec::new();
    let mut raw: Vec<(i128, Option<i128>, String)> = Vec::new();
    let step = seconds_ns(&format!("{}", out.record_duration_s)).unwrap_or(0);
    let mut k = 0u64;
    while k < records {
        let n = chunk_records.min(records - k);
        let blocks: Vec<Vec<Vec<u8>>> = if chunked {
            let at = out.record_offset(k);
            let blk = read_block(f, path, at, n * out.record_bytes, out.file_len)?;
            (0..n)
                .map(|j| {
                    ann.iter()
                        .map(|(o, len)| {
                            blk.slice(at + j * out.record_bytes + o, *len as usize)
                                .unwrap_or_default()
                                .to_vec()
                        })
                        .collect()
                })
                .collect()
        } else {
            let mut parts = Vec::with_capacity(ann.len());
            for (o, len) in &ann {
                let at = out.record_offset(k) + o;
                parts.push(read_block(f, path, at, *len, out.file_len)?.bytes);
            }
            vec![parts]
        };
        for parts in blocks {
            let mut start = None;
            for (si, bytes) in parts.iter().enumerate() {
                for (ti, tal) in bytes
                    .split(|&c| c == 0)
                    .filter(|t| !t.is_empty())
                    .enumerate()
                {
                    let Some((onset, duration, texts)) = parse_tal(tal) else {
                        bad_tals += 1;
                        continue;
                    };
                    let mut texts = texts.into_iter();
                    if si == 0 && ti == 0 && plus {
                        // the time-keeping TAL: its first annotation is empty
                        let mut t = texts.clone();
                        if t.next().is_some_and(|x| x.is_empty()) {
                            start = Some(onset);
                            texts = t;
                        }
                    }
                    for text in texts.filter(|x| !x.is_empty()) {
                        if raw.len() < MAX_EDF_ANNOTATIONS {
                            raw.push((onset, duration, text));
                        }
                    }
                }
            }
            if plus {
                let t = start.unwrap_or_else(|| {
                    missing_time += 1;
                    starts.last().map_or(0, |l| l + step)
                });
                starts.push(t);
            }
        }
        k += n;
    }
    // times count from the first data record's start (the subsecond start of EDF+)
    let origin = starts.first().copied().unwrap_or(0);
    out.start_offset_s = origin as f64 / 1e9;
    out.record_starts = starts.iter().map(|t| (t - origin) as f64 / 1e9).collect();
    out.annotations = raw
        .into_iter()
        .map(|(o, d, text)| EdfAnnotation {
            onset_s: (o - origin) as f64 / 1e9,
            duration_s: d.map_or(0.0, |d| d as f64 / 1e9),
            text,
        })
        .collect();
    if missing_time > 0 {
        out.findings.push(Finding::warning(
            "missing_time_keeping",
            format!("{missing_time} data record(s) do not start with a time-keeping TAL; they are taken to follow the previous record"),
        ));
    }
    if bad_tals > 0 {
        out.findings.push(Finding::warning(
            "bad_tal",
            format!("{bad_tals} TAL(s) could not be parsed and were skipped"),
        ));
    }
    if out.annotations.len() >= MAX_EDF_ANNOTATIONS {
        out.findings.push(Finding::warning(
            "annotations_capped",
            format!("only the first {MAX_EDF_ANNOTATIONS} annotations are kept"),
        ));
    }
    if out.variant().ends_with("+C") && out.record_starts.len() as u64 == out.records {
        let gaps = (1..out.record_starts.len())
            .filter(|&k| {
                (out.record_starts[k] - out.record_starts[k - 1] - out.record_duration_s).abs()
                    > EDF_CONTIGUOUS_TOLERANCE_S
            })
            .count();
        if gaps > 0 {
            out.findings.push(Finding::warning(
                "record_time_gap",
                format!("the file is marked continuous, but {gaps} data record(s) do not start where the previous one ends; samples are read as continuous"),
            ));
        }
    }
    Ok(())
}

/// A TAL time (`+12.5`, `-0.065`, `30`) in nanoseconds, exact: a sign, digits, and up to nine
/// decimals (further decimals are dropped).
pub fn seconds_ns(text: &str) -> Option<i128> {
    let t = text.trim();
    let (neg, t) = match t.as_bytes().first() {
        Some(b'+') => (false, &t[1..]),
        Some(b'-') => (true, &t[1..]),
        _ => (false, t),
    };
    let (int, frac) = t.split_once('.').unwrap_or((t, ""));
    if int.is_empty() && frac.is_empty()
        || !int.bytes().all(|c| c.is_ascii_digit())
        || !frac.bytes().all(|c| c.is_ascii_digit())
        || int.len() > 20
    {
        return None;
    }
    let whole: i128 = if int.is_empty() { 0 } else { int.parse().ok()? };
    let mut ns: i128 = 0;
    for (i, c) in frac.bytes().take(9).enumerate() {
        ns += i128::from(c - b'0') * 10i128.pow(8 - i as u32);
    }
    let v = whole.checked_mul(1_000_000_000)?.checked_add(ns)?;
    Some(if neg { -v } else { v })
}

/// One TAL without its closing 0 byte: `±onset[\x15duration]\x14text\x14text\x14…`. Returns
/// the onset and the duration in nanoseconds ([`seconds_ns`]) and the texts (the first may be
/// empty: a time-keeping TAL).
pub fn parse_tal(tal: &[u8]) -> Option<(i128, Option<i128>, Vec<String>)> {
    let split = tal.iter().position(|&c| c == 20)?;
    let stamp = latin1(&tal[..split]);
    let (onset, duration) = match stamp.split_once('\u{15}') {
        Some((o, d)) => (o, Some(d)),
        None => (stamp.as_str(), None),
    };
    if !onset.starts_with(['+', '-']) {
        return None;
    }
    let onset = seconds_ns(onset)?;
    let duration = match duration {
        Some(d) if !d.trim().is_empty() => Some(seconds_ns(d)?),
        _ => None,
    };
    let rest = &tal[split + 1..];
    let mut texts: Vec<String> = rest
        .split(|&c| c == 20)
        .map(|t| String::from_utf8_lossy(t).into_owned())
        .collect();
    // the text list ends with byte 20, which leaves one empty piece
    if texts.last().is_some_and(String::is_empty) {
        texts.pop();
    }
    Some((onset, duration, texts))
}

/// The EDF / BDF reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct EdfReader;

impl FormatReader for EdfReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&crate::assurance::EDF)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: EDF_FORMAT_ID.into(),
            name: "European Data Format (EDF, EDF+, BDF, BDF+)".into(),
            vendor: "open specification (edfplus.info); BDF by BioSemi".into(),
            extensions: vec!["edf".into(), "bdf".into()],
            family: "electrophysiology".into(),
            can_read: true,
            can_write: false,
            confidence: crate::assurance::EDF.confidence,
            known_gaps: vec![
                "Signals with different sample rates are separate traces; nothing is resampled".into(),
                "The BioSemi Status channel is scaled like any signal; its trigger bits are not split out".into(),
                "Local patient identification is kept in the vendor metadata only".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if looks_like_edf(head) {
            return Some(Detection {
                format_id: EDF_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        has_extension(path, &["edf", "bdf"]).then(|| Detection {
            format_id: EDF_FORMAT_ID,
            confidence: DetectConfidence::ExtensionOnly,
            note: Some("EDF/BDF extension but no EDF header".into()),
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(EdfDataset::open(path)?))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(EdfDataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// An opened EDF or BDF file.
#[derive(Debug)]
pub struct EdfDataset {
    path: PathBuf,
    fs: Fs,
    handle: Option<SourceFile>,
    file: EdfFile,
    groups: Vec<Vec<usize>>,
    sweeps: Vec<(u64, u64)>,
    texts: Vec<String>,
}

impl EdfDataset {
    /// Open a local file.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    /// Open an [`Input`].
    pub fn open_input(input: &Input) -> Result<Self> {
        let path = input.path().to_path_buf();
        let fs = input.fs().clone();
        let mut f = fs.open(&path).map_err(|e| Error::io(&path, e))?;
        let file_len = f.size().map_err(|e| Error::io(&path, e))?;
        let file = parse_edf(&mut f, &path, file_len)?;
        let groups = file.groups();
        let sweeps = file.sweeps();
        let mut texts: Vec<String> = Vec::new();
        let mut seen = BTreeMap::new();
        for a in &file.annotations {
            if !seen.contains_key(&a.text) {
                seen.insert(a.text.clone(), texts.len());
                texts.push(a.text.clone());
            }
        }
        Ok(EdfDataset {
            path,
            fs,
            handle: Some(f),
            file,
            groups,
            sweeps,
            texts,
        })
    }

    /// The parsed file.
    pub fn edf(&self) -> &EdfFile {
        &self.file
    }

    fn handle(&mut self) -> Result<&mut SourceFile> {
        if self.handle.is_none() {
            self.handle = Some(
                self.fs
                    .open(&self.path)
                    .map_err(|e| Error::io(&self.path, e))?,
            );
        }
        Ok(self.handle.as_mut().expect("just opened"))
    }

    fn trace_info(&self, g: usize) -> TraceInfo {
        let f = &self.file;
        let sigs = &self.groups[g];
        let spr = f.signals[sigs[0]].samples_per_record;
        let rate = f.rate_hz(&f.signals[sigs[0]]);
        let counts: Vec<u64> = self.sweeps.iter().map(|(a, b)| (b - a) * spr).collect();
        let mut extra = BTreeMap::new();
        extra.insert("samples_per_record".into(), json!(spr));
        extra.insert("record_duration_s".into(), json!(f.record_duration_s));
        extra.insert("data_records".into(), json!(f.records));
        if g == 0 {
            if let Some(t) = f.recorded_at() {
                extra.insert("recorded_at".into(), json!(t));
            }
            if let Some(sub) = f.recording_subfields() {
                for (k, name) in [(1, "admin_code"), (2, "technician"), (3, "equipment")] {
                    if let Some(v) = sub.get(k).filter(|v| v.as_str() != "X") {
                        extra.insert(name.into(), json!(v));
                    }
                }
            }
        }
        if self.sweeps.len() > 1 {
            extra.insert(
                "sweep_starts_s".into(),
                json!(
                    self.sweeps
                        .iter()
                        .map(|(a, _)| f.record_start(*a))
                        .collect::<Vec<_>>()
                ),
            );
            extra.insert("sweep_sample_counts".into(), json!(counts));
        }
        let dtype = if f.bdf { "int24" } else { "int16" };
        let channels = sigs
            .iter()
            .enumerate()
            .map(|(c, &i)| {
                let s = &f.signals[i];
                let mut e = BTreeMap::new();
                e.insert("signal".into(), json!(i));
                e.insert("physical_min".into(), json!(s.physical_min));
                e.insert("physical_max".into(), json!(s.physical_max));
                e.insert("digital_min".into(), json!(s.digital_min));
                e.insert("digital_max".into(), json!(s.digital_max));
                if !s.transducer.is_empty() {
                    e.insert("transducer".into(), json!(s.transducer));
                }
                if !s.prefiltering.is_empty() {
                    e.insert("prefiltering".into(), json!(s.prefiltering));
                }
                SignalChannelInfo {
                    index: c as u32,
                    name: if s.label.is_empty() {
                        format!("ch{c}")
                    } else {
                        s.label.clone()
                    },
                    unit: (!s.physical_dimension.is_empty()).then(|| s.physical_dimension.clone()),
                    dtype: dtype.into(),
                    scale: s.scale(),
                    offset: s.offset(),
                    extra: e,
                }
            })
            .collect();
        TraceInfo {
            index: g as u32,
            name: Some(if self.groups.len() == 1 {
                "signals".into()
            } else {
                format!("signals at {rate} Hz")
            }),
            sample_rate_hz: rate,
            sample_count: counts.iter().copied().max().unwrap_or(0),
            sweep_count: u32::try_from(self.sweeps.len()).unwrap_or(u32::MAX),
            channels,
            start_s: Some(self.sweeps.first().map_or(0.0, |(a, _)| f.record_start(*a))),
            extra,
        }
    }

    fn annotation_table(&self) -> Option<TableInfo> {
        let f = &self.file;
        if f.annotations.is_empty() {
            return None;
        }
        let col = |i: u32, name: &str, dtype: &str, unit: Option<&str>| ColumnInfo {
            index: i,
            name: name.into(),
            label: None,
            dtype: dtype.into(),
            unit: unit.map(str::to_string),
            range: None,
            extra: BTreeMap::new(),
        };
        let mut text = col(2, "text", "int32", None);
        text.extra.insert("categories".into(), json!(self.texts));
        let mut extra = BTreeMap::new();
        if self.groups.is_empty()
            && let Some(t) = f.recorded_at()
        {
            extra.insert("recorded_at".into(), json!(t));
        }
        Some(TableInfo {
            index: 0,
            name: Some("annotations".into()),
            row_count: f.annotations.len() as u64,
            columns: vec![
                col(0, "onset_s", "float64", Some("s")),
                col(1, "duration_s", "float64", Some("s")),
                text,
            ],
            extra,
        })
    }
}

/// Decode `n` samples of `bytes` (2- or 3-byte little-endian two's complement).
fn decode(bytes: &[u8], bdf: bool, scale: f64, offset: f64, out: &mut Vec<f64>) {
    if bdf {
        for x in bytes.chunks_exact(3) {
            let v = i32::from_le_bytes([x[0], x[1], x[2], 0]);
            let v = (v << 8) >> 8;
            out.push(f64::from(v) * scale + offset);
        }
    } else {
        for x in bytes.chunks_exact(2) {
            out.push(f64::from(i16::from_le_bytes([x[0], x[1]])) * scale + offset);
        }
    }
}

impl Dataset for EdfDataset {
    fn info(&self) -> Result<FileInfo> {
        let f = &self.file;
        let ordinary = f.signals.iter().filter(|s| !s.annotations).count();
        let mut notes = vec![format!(
            "{} file: {ordinary} signal(s) in {} trace(s) (one per sample rate), {} data record(s) of {} s",
            f.variant(),
            self.groups.len(),
            f.records,
            f.record_duration_s
        )];
        if self.sweeps.len() > 1 {
            notes.push(format!(
                "discontinuous recording: {} runs of contiguous data records are sweeps (`extra.sweep_starts_s`)",
                self.sweeps.len()
            ));
        }
        if !f.annotations.is_empty() {
            notes.push(format!(
                "{} annotation(s) in the `annotations` table (onset and duration in seconds after the first sample)",
                f.annotations.len()
            ));
        }
        if f.findings.iter().any(|x| x.severity == Severity::Error) {
            notes.push("structural problems found (e.g. truncation); run `check`".into());
        }
        let traces = (0..self.groups.len()).map(|g| self.trace_info(g)).collect();
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: f.file_len,
            format: EdfReader.descriptor(),
            format_version: Some(f.variant()),
            images: Vec::new(),
            tables: self.annotation_table().into_iter().collect(),
            spectra: Vec::new(),
            traces,
            plane_count: 0,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let f = &self.file;
        Ok(json!({
            "version": f.version_field,
            "patient": f.patient,
            "recording": f.recording,
            "start_date": f.start_date,
            "start_time": f.start_time,
            "header_bytes": f.header_bytes,
            "reserved": f.reserved,
            "data_records": f.declared_records,
            "record_duration_s": f.record_duration_s,
            "signals": f.signals.iter().map(|s| json!({
                "label": s.label, "transducer": s.transducer,
                "physical_dimension": s.physical_dimension,
                "physical_min": s.physical_min, "physical_max": s.physical_max,
                "digital_min": s.digital_min, "digital_max": s.digital_max,
                "prefiltering": s.prefiltering, "samples_per_record": s.samples_per_record,
                "reserved": s.reserved,
            })).collect::<Vec<_>>(),
        }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for k in [
            "format_version",
            "traces[].sample_rate_hz",
            "traces[].sample_count",
            "traces[].sweep_count",
            "traces[].start_s",
            "traces[].channels[].name",
            "traces[].channels[].unit",
            "traces[].channels[].scale",
            "traces[].channels[].offset",
            "traces[].extra.recorded_at",
            "traces[].extra.sweep_starts_s",
            "traces[].extra.equipment",
            "tables[].row_count",
            "tables[].extra.recorded_at",
        ] {
            p.insert(k.into(), Source::Spec);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let f = &self.file;
        let mut out = vec![LsEntry {
            kind: "header".into(),
            name: format!("{} header", f.variant()),
            offset: Some(0),
            size: Some(EDF_HEADER_BLOCK),
            image: None,
            details: json!({"signals": f.signals.len(), "reserved": f.reserved}),
        }];
        for (i, s) in f.signals.iter().enumerate() {
            out.push(LsEntry {
                kind: if s.annotations {
                    "annotation-signal"
                } else {
                    "signal"
                }
                .into(),
                name: s.label.clone(),
                offset: None,
                size: None,
                image: None,
                details: json!({
                    "signal": i, "samples_per_record": s.samples_per_record,
                    "offset_in_record": s.offset_in_record, "unit": s.physical_dimension,
                }),
            });
        }
        out.push(LsEntry {
            kind: "data-records".into(),
            name: format!("{} data records", f.records),
            offset: Some(f.data_offset()),
            size: Some(f.records * f.record_bytes),
            image: None,
            details: json!({"record_bytes": f.record_bytes, "record_duration_s": f.record_duration_s}),
        });
        Ok(out)
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::unsupported(
            EDF_FORMAT_ID,
            "image planes",
            "EDF and BDF files hold sampled signals: use `openreadout trace`.",
        ))
    }

    fn read_trace(&mut self, index: u32, sweep: u32, first: u64, max: u64) -> Result<Trace> {
        let g = index as usize;
        let Some(sigs) = self.groups.get(g).cloned() else {
            return Err(Error::Usage(format!(
                "trace index {index} out of range ({} traces)",
                self.groups.len()
            )));
        };
        let Some(&(r0, r1)) = self.sweeps.get(sweep as usize) else {
            return Err(Error::Usage(format!(
                "sweep {sweep} out of range ({} sweeps)",
                self.sweeps.len()
            )));
        };
        let f = &self.file;
        let spr = f.signals[sigs[0]].samples_per_record;
        let n = (r1 - r0) * spr;
        if first > n {
            return Err(Error::Usage(format!(
                "first sample {first} is past the end ({n} samples)"
            )));
        }
        let rec_bytes = f.record_bytes.max(1);
        let max_records = (MAX_EDF_READ_BYTES / rec_bytes).max(1);
        let mut count = max.min(n - first).min(MAX_EDF_READ);
        let rec_a = first / spr;
        // keep the records touched within the read budget
        let rec_end = (first + count).div_ceil(spr).min(rec_a + max_records);
        count = count.min(rec_end * spr - first);
        let mut channels: Vec<Vec<f64>> = vec![Vec::with_capacity(count as usize); sigs.len()];
        if count == 0 {
            return Ok(Trace {
                trace: index,
                sweep,
                first_sample: first,
                channels,
            });
        }
        let at = f.record_offset(r0 + rec_a);
        let len = (rec_end - rec_a) * f.record_bytes;
        let (bdf, sb, file_len) = (f.bdf, f.sample_bytes(), f.file_len);
        let cal: Vec<(u64, f64, f64)> = sigs
            .iter()
            .map(|&i| {
                let s = &f.signals[i];
                (s.offset_in_record, s.scale(), s.offset())
            })
            .collect();
        let path = self.path.clone();
        let block = read_block(self.handle()?, &path, at, len, file_len)?;
        if (block.len() as u64) < len {
            return Err(Error::corrupt_at(
                EDF_FORMAT_ID,
                at,
                "data records cut off by the end of the file",
            ));
        }
        for r in rec_a..rec_end {
            let lo = if r == rec_a { first - r * spr } else { 0 };
            let hi = spr.min(first + count - r * spr);
            let base = (r - rec_a) * rec_bytes;
            for (c, (off, scale, offset)) in cal.iter().enumerate() {
                let s = (base + off + lo * sb) as usize;
                let e = (base + off + hi * sb) as usize;
                decode(&block.bytes[s..e], bdf, *scale, *offset, &mut channels[c]);
            }
        }
        Ok(Trace {
            trace: index,
            sweep,
            first_sample: first,
            channels,
        })
    }

    fn read_table(&mut self, index: u32, first_row: u64, max_rows: u64) -> Result<Table> {
        if index != 0 || self.file.annotations.is_empty() {
            return Err(Error::Usage(format!("table index {index} out of range")));
        }
        let rows = &self.file.annotations;
        let a = (first_row as usize).min(rows.len());
        let b = a
            .saturating_add(max_rows.min(usize::MAX as u64) as usize)
            .min(rows.len());
        let code: BTreeMap<&str, usize> = self
            .texts
            .iter()
            .enumerate()
            .map(|(i, t)| (t.as_str(), i))
            .collect();
        let slice = &rows[a..b];
        Ok(Table {
            table: 0,
            first_row: a as u64,
            columns: vec![
                slice.iter().map(|r| r.onset_s).collect(),
                slice.iter().map(|r| r.duration_s).collect(),
                slice
                    .iter()
                    .map(|r| code.get(r.text.as_str()).map_or(f64::NAN, |&i| i as f64))
                    .collect(),
            ],
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let f = &self.file;
        let mut r = CheckReport::new(self.path.display().to_string(), EDF_FORMAT_ID);
        r.performed("header: version, header length, signal count, record count and duration parse; signal headers read");
        r.performed("file length against the data records; every annotation signal's TALs parsed");
        for x in &f.findings {
            r.push(x.clone());
        }
        for (i, s) in f.signals.iter().enumerate() {
            if s.annotations {
                continue;
            }
            if !(s.digital_max > s.digital_min) {
                r.push(Finding::error(
                    "bad_digital_range",
                    format!(
                        "signal {i} ({}): digital maximum {} is not above the minimum {}",
                        s.label, s.digital_max, s.digital_min
                    ),
                ));
            }
            if s.physical_max == s.physical_min {
                r.push(Finding::error(
                    "bad_physical_range",
                    format!(
                        "signal {i} ({}): physical minimum and maximum are equal",
                        s.label
                    ),
                ));
            }
            let lim = if f.bdf { 8_388_608.0 } else { 32_768.0 };
            if s.digital_min < -lim || s.digital_max > lim - 1.0 {
                r.push(Finding::warning(
                    "digital_range_exceeds_sample_size",
                    format!(
                        "signal {i} ({}): digital range {}…{} exceeds the {}-bit sample size",
                        s.label,
                        s.digital_min,
                        s.digital_max,
                        if f.bdf { 24 } else { 16 }
                    ),
                ));
            }
        }
        if f.plus() && !f.signals.iter().any(|s| s.annotations) {
            r.push(Finding::warning(
                "no_annotation_signal",
                "an EDF+/BDF+ file must hold an annotation signal; record start times are taken as contiguous",
            ));
        }
        Ok(r)
    }
}

#[cfg(test)]
mod tests;
