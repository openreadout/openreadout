//! `Dataset` for Agilent ChemStation `.D` directories and single `.ch`/`.uv`/`.ms` files:
//! one trace per `.ch` signal, one multi-channel trace (one channel per wavelength) per `.uv`
//! file, one spectra run per `.ms` file.

use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use openreadout_core::bytes::read_range;
use openreadout_core::model::{
    CheckReport, ColumnInfo, FileInfo, Finding, InstrumentInfo, LsEntry, SignalChannelInfo,
    SpectraInfo, Spectrum, Table, TableInfo, Trace, TraceInfo,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::source::{Fs, Input};
use openreadout_core::{Error, Plane, Result};
use serde_json::{Value, json};

use crate::binary::tidy;
use crate::chemstation_decode::{
    BodyEnd, DecodedSignal, MS_RECORD_HEADER, MS_RECORD_TRAILER, ScanRecord, UV_RECORD_HEADER,
    decode_delta_records, decode_float64, decode_second_difference, ms_pairs, ms_record, uv_record,
    uv_values,
};
use crate::chemstation_header::{
    BodyEncoding, SignalHeader, SignalKind, looks_like_chemstation, text_preview,
};
use crate::chemstation_report::{
    MAX_REPORT_BYTES, ReportPeak, ReportTable, ResultModule, decode_text, parse_report_txt,
    parse_result_modules, parse_result_xml, parse_results_csv,
};
use crate::{CHEMSTATION_ID, ChemStationReader};

/// Largest `.ch` file decoded whole (they are kilobytes to a few megabytes).
const MAX_CH_BYTES: u64 = 512 << 20;
/// Bytes read from each file to parse its header (the largest header is 0x1800).
const HEADER_READ: u64 = 0x1800;
/// Largest text file previewed in the vendor tree.
const MAX_TEXT_PREVIEW: usize = 16 * 1024;

/// One file inside the `.D` directory.
#[derive(Debug, Clone)]
pub struct DirFile {
    /// Path relative to the `.D` directory, `/`-separated.
    pub rel: String,
    pub size: u64,
    /// ChemStation version string at byte 0, if the file has one.
    pub version: Option<String>,
}

/// Decoded content of one signal file.
#[derive(Debug, Clone)]
enum Body {
    /// `.ch`: raw values.
    Channel(DecodedSignal),
    /// `.uv`/`.ms`: located scan records, and how the walk ended.
    Scans(Vec<ScanRecord>, BodyEnd),
    /// Not decoded (unknown version).
    None,
}

/// One decodable ChemStation file.
#[derive(Debug, Clone)]
pub struct SignalFile {
    pub rel: String,
    pub path: PathBuf,
    pub size: u64,
    pub header: SignalHeader,
    body: Body,
}

impl SignalFile {
    /// Samples (`.ch`) or scans (`.uv`, `.ms`) found in the body.
    pub fn count(&self) -> u64 {
        match &self.body {
            Body::Channel(d) => d.values.len() as u64,
            Body::Scans(r, _) => r.len() as u64,
            Body::None => 0,
        }
    }
    fn stem(&self) -> String {
        Path::new(&self.rel)
            .file_stem()
            .map_or_else(|| self.rel.clone(), |s| s.to_string_lossy().into_owned())
    }
}

/// An opened `.D` directory or single ChemStation file.
#[derive(Debug)]
pub struct ChemStationDataset {
    path: PathBuf,
    is_dir: bool,
    /// Where the directory or file is read from.
    fs: Fs,
    files: Vec<DirFile>,
    signals: Vec<SignalFile>,
    total_size: u64,
    /// `acqmeth.txt` parsed (`chemstation_method`), when the directory has one.
    method_file: Option<Value>,
    /// The vendor's peak reports (`Report.TXT`, `RESULTS.CSV`), one table per signal.
    reports: Vec<ReportTable>,
    /// The instrument modules `Result.xml` lists (`ModuleInformation`).
    modules: Vec<ResultModule>,
}

fn walk(
    fs: &Fs,
    dir: &Path,
    base: &Path,
    depth: u32,
    out: &mut Vec<(String, PathBuf, u64)>,
) -> Result<()> {
    let mut entries: Vec<_> = fs
        .read_dir(dir)
        .map_err(|e| Error::io(dir, e))?
        .filter_map(std::result::Result::ok)
        .collect();
    entries.sort_by_key(openreadout_core::source::DirEntry::file_name);
    for e in entries {
        let p = e.path();
        let meta = fs.metadata(&p).map_err(|er| Error::io(&p, er))?;
        if meta.is_dir() {
            if depth < 4 {
                walk(fs, &p, base, depth + 1, out)?;
            }
            continue;
        }
        let rel = p
            .strip_prefix(base)
            .unwrap_or(&p)
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        out.push((rel, p, meta.len()));
    }
    Ok(())
}

fn read_head(fs: &Fs, path: &Path, n: u64) -> Result<Vec<u8>> {
    let mut f = fs.open(path).map_err(|e| Error::io(path, e))?;
    let mut buf = Vec::new();
    f.by_ref()
        .take(n)
        .read_to_end(&mut buf)
        .map_err(|e| Error::io(path, e))?;
    Ok(buf)
}

/// Is `path` a ChemStation `.D` directory (a `.d` directory holding at least one file with a
/// ChemStation version string at its top level)?
pub fn is_chemstation_dir(path: &Path) -> bool {
    is_chemstation_dir_in(&Fs::local(), path)
}

/// [`is_chemstation_dir`] in the namespace `fs`.
pub(crate) fn is_chemstation_dir_in(fs: &Fs, path: &Path) -> bool {
    if !fs.is_dir(path) || !crate::has_ext(path, &["d"]) {
        return false;
    }
    let Ok(rd) = fs.read_dir(path) else {
        return false;
    };
    rd.filter_map(std::result::Result::ok).any(|e| {
        let p = e.path();
        crate::has_ext(&p, &["ch", "uv", "ms"])
            && fs.is_file(&p)
            && read_head(fs, &p, 0x200).is_ok_and(|h| looks_like_chemstation(&h).is_some())
    })
}

/// Walk the scan records of a `.uv` or `.ms` file without decoding values.
fn index_scans(
    fs: &Fs,
    path: &Path,
    h: &SignalHeader,
    size: u64,
) -> Result<(Vec<ScanRecord>, BodyEnd)> {
    let mut f = fs.open(path).map_err(|e| Error::io(path, e))?;
    let uv = h.encoding == BodyEncoding::SpectrumRecords;
    let head_len = if uv {
        UV_RECORD_HEADER
    } else {
        MS_RECORD_HEADER
    } as u64;
    let mut pos = h.header_len;
    let mut out: Vec<ScanRecord> = Vec::new();
    let limit = h.declared_records.unwrap_or(u64::MAX);
    let mut buf = [0u8; 22];
    let end = loop {
        if out.len() as u64 >= limit {
            break BodyEnd::Terminator;
        }
        if pos >= size {
            break BodyEnd::EndOfFile;
        }
        if pos + head_len > size {
            break BodyEnd::Truncated {
                at: pos - h.header_len,
            };
        }
        f.seek(SeekFrom::Start(pos))
            .map_err(|e| Error::io(path, e))?;
        f.read_exact(&mut buf[..head_len as usize])
            .map_err(|e| Error::io(path, e))?;
        let rec = if uv {
            uv_record(&buf, pos)
        } else {
            ms_record(&buf, pos)
        };
        let Some(rec) = rec else {
            // zero padding at the end of the body ends the walk cleanly
            if buf[..4].iter().all(|&b| b == 0) {
                break BodyEnd::Terminator;
            }
            break BodyEnd::BadRecord {
                at: pos - h.header_len,
                detail: "record tag is not 67 or 70".into(),
            };
        };
        if rec.len < head_len {
            if rec.len == 0 && rec.time_ms == 0 {
                break BodyEnd::Terminator;
            }
            break BodyEnd::BadRecord {
                at: pos - h.header_len,
                detail: format!("record length {} is shorter than its header", rec.len),
            };
        }
        if pos + rec.len > size {
            break BodyEnd::Truncated {
                at: pos - h.header_len,
            };
        }
        pos += rec.len;
        out.push(rec);
    };
    Ok((out, end))
}

fn open_signal(fs: &Fs, rel: &str, path: &Path, size: u64) -> Result<Option<SignalFile>> {
    let head = read_head(fs, path, HEADER_READ)?;
    let Some(_) = looks_like_chemstation(&head) else {
        return Ok(None);
    };
    let Some(header) = SignalHeader::parse(&head) else {
        return Ok(None);
    };
    let body = match header.encoding {
        BodyEncoding::DeltaRecords | BodyEncoding::SecondDifference | BodyEncoding::Float64 => {
            if header.header_len == 0 || header.header_len > size {
                Body::Channel(DecodedSignal {
                    values: Vec::new(),
                    escapes: 0,
                    records: 0,
                    end: BodyEnd::Truncated { at: 0 },
                })
            } else {
                let bytes = crate::binary::read_file_in(fs, path, MAX_CH_BYTES, CHEMSTATION_ID)?;
                let b = &bytes[header.header_len as usize..];
                Body::Channel(match header.encoding {
                    BodyEncoding::DeltaRecords => decode_delta_records(b),
                    BodyEncoding::SecondDifference => {
                        decode_second_difference(b, header.version == "181")
                    }
                    _ => decode_float64(b),
                })
            }
        }
        BodyEncoding::SpectrumRecords | BodyEncoding::MassSpectrumRecords => {
            if header.header_len == 0 || header.header_len > size {
                Body::Scans(Vec::new(), BodyEnd::Truncated { at: 0 })
            } else {
                let (r, e) = index_scans(fs, path, &header, size)?;
                Body::Scans(r, e)
            }
        }
        BodyEncoding::NotDecoded => Body::None,
    };
    Ok(Some(SignalFile {
        rel: rel.to_string(),
        path: path.to_path_buf(),
        size,
        header,
        body,
    }))
}

/// Intervals between the header's first and last time for `n` values: version 81 headers span
/// `n` intervals (the last time is one interval past the last value), the others `n − 1`.
fn intervals(h: &SignalHeader, n: u64) -> u64 {
    if h.version == "81" {
        n
    } else {
        n.saturating_sub(1)
    }
}

/// A `.ms` record's TIC: the u32 six bytes into its trailer (after the pairs); four bytes read,
/// no pair decoded.
fn record_tic<R: Read + Seek + ?Sized>(
    f: &mut R,
    path: &Path,
    r: &ScanRecord,
) -> Result<Option<f64>> {
    let at = u64::from(r.count)
        .checked_mul(4)
        .and_then(|n| n.checked_add(MS_RECORD_HEADER as u64 + 6));
    Ok(match at {
        Some(at) if at + 4 <= r.len => {
            let b = read_range(f, path, r.offset + at, 4)?;
            b.get(..4)
                .map(|b| f64::from(u32::from_be_bytes([b[0], b[1], b[2], b[3]])))
        }
        _ => None,
    })
}

/// A trace's name: the header's signal text, else the file stem; `<stem> spectra` for `.uv`.
fn trace_name(s: &SignalFile) -> String {
    match s.header.kind {
        SignalKind::Spectra => format!("{} spectra", s.stem()),
        _ => s.header.signal.clone().unwrap_or_else(|| s.stem()),
    }
}

fn wavelength_label(raw: u32) -> String {
    let nm = f64::from(raw) / 20.0;
    if nm.fract() == 0.0 {
        format!("{nm:.0} nm")
    } else {
        format!("{nm} nm")
    }
}

fn put(m: &mut BTreeMap<String, Value>, k: &str, v: Option<impl Into<Value>>) {
    if let Some(v) = v {
        m.insert(k.to_string(), v.into());
    }
}

/// Header fields shared by traces and spectra, in our vocabulary.
fn header_extra(s: &SignalFile) -> BTreeMap<String, Value> {
    let h = &s.header;
    let mut m = BTreeMap::new();
    m.insert("file".into(), json!(s.rel));
    m.insert("format_version".into(), json!(h.version));
    m.insert("encoding".into(), json!(h.encoding.name()));
    put(&mut m, "file_type", h.file_type.clone());
    put(&mut m, "sample_name", h.sample_name.clone());
    put(&mut m, "description", h.description.clone());
    put(&mut m, "method", h.method.clone());
    put(&mut m, "operator", h.operator.clone());
    put(&mut m, "acquired_at", h.acquired_at.clone());
    put(&mut m, "acquired_text", h.acquired_text.clone());
    put(&mut m, "instrument", h.instrument_model.clone());
    put(&mut m, "instrument_name", h.instrument_name.clone());
    put(&mut m, "separation", h.separation.clone());
    put(&mut m, "software", h.software.clone());
    put(&mut m, "software_revision", h.software_revision.clone());
    put(&mut m, "signal", h.signal.clone());
    put(&mut m, "vial", h.vial.filter(|v| *v != 0));
    let (sig, reference) = h.wavelengths();
    if let Some([w, bw]) = sig {
        m.insert("wavelength_nm".into(), json!(w));
        if bw.is_finite() {
            m.insert("bandwidth_nm".into(), json!(bw));
        }
    }
    if let Some([w, bw]) = reference {
        m.insert("reference_wavelength_nm".into(), json!(w));
        if bw.is_finite() {
            m.insert("reference_bandwidth_nm".into(), json!(bw));
        }
    }
    // ChemStation names signal files `<detector><n><letter>` (`FID1A.ch`, `mwd1A.ch`); a file
    // renamed with a prefix (`entab-test_fid.ch`) keeps that name as its last part
    let stem = s.stem();
    let last = stem
        .rsplit(['-', '_', '.', ' '])
        .find(|p| !p.is_empty())
        .unwrap_or(&stem);
    let detector: String = last.chars().take_while(char::is_ascii_alphabetic).collect();
    if !detector.is_empty() {
        m.insert("detector".into(), json!(detector.to_ascii_uppercase()));
    }
    m
}

fn instrument(h: &SignalHeader) -> Option<InstrumentInfo> {
    if h.instrument_model.is_none() && h.software.is_none() && h.instrument_name.is_none() {
        return None;
    }
    Some(InstrumentInfo {
        manufacturer: Some("Agilent".into()),
        model: h.instrument_model.clone(),
        software: Some("ChemStation".into()),
        software_version: h.software.clone(),
        detector: None,
    })
}

impl ChemStationDataset {
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    /// Open an [`Input`] (a local path, a buffer, a host source, a folder held in memory).
    pub(crate) fn open_input(input: &Input) -> Result<Self> {
        let (path, fs) = (input.path(), input.fs());
        let meta = fs.metadata(path).map_err(|e| Error::io(path, e))?;
        let mut listing = Vec::new();
        if meta.is_dir() {
            walk(fs, path, path, 0, &mut listing)?;
        } else {
            let name = path
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            listing.push((name, path.to_path_buf(), meta.len()));
        }
        let mut files = Vec::new();
        let mut signals = Vec::new();
        for (rel, p, size) in &listing {
            let head = read_head(fs, p, 8)?;
            let version = crate::chemstation_header::version_of(&head);
            files.push(DirFile {
                rel: rel.clone(),
                size: *size,
                version: version.clone(),
            });
            let top_level = !rel.contains('/');
            let candidate = version.is_some()
                && top_level
                && (crate::has_ext(p, &["ch", "uv", "ms"]) || !meta.is_dir());
            if candidate && let Some(s) = open_signal(fs, rel, p, *size)? {
                if s.header.kind == SignalKind::Other && meta.is_dir() {
                    continue;
                }
                signals.push(s);
            }
        }
        if signals.is_empty() {
            return Err(if meta.is_dir() {
                Error::corrupt(
                    CHEMSTATION_ID,
                    "no ChemStation signal file (.ch, .uv, .ms) with a readable header in this directory",
                )
            } else {
                Error::corrupt(CHEMSTATION_ID, "no ChemStation version string at byte 0")
            });
        }
        let total_size = listing.iter().map(|(_, _, s)| s).sum();
        let method_file = listing
            .iter()
            .find(|(rel, _, size)| {
                rel.eq_ignore_ascii_case("acqmeth.txt")
                    && *size <= crate::chemstation_method::MAX_METHOD_TEXT
            })
            .and_then(|(_, p, _)| fs.read(p).ok())
            .and_then(|b| text_preview(&b, usize::MAX))
            .and_then(|t| crate::chemstation_method::parse_acqmeth(&t));
        let mut reports = Vec::new();
        // Result.xml (the XML export: limits, baselines, compounds) is preferred over the
        // printed Report.TXT of the same integration
        let result_xml = listing
            .iter()
            .find(|(rel, _, size)| {
                meta.is_dir() && rel.eq_ignore_ascii_case("result.xml") && *size <= MAX_REPORT_BYTES
            })
            .and_then(|(_, p, _)| fs.read(p).ok())
            .map(|b| decode_text(&b));
        let modules = result_xml
            .as_deref()
            .map(parse_result_modules)
            .unwrap_or_default();
        let result_xml = result_xml.as_deref().and_then(parse_result_xml);
        let have_xml = result_xml.is_some();
        reports.extend(result_xml.unwrap_or_default());
        for (rel, p, size) in &listing {
            if !meta.is_dir() || rel.contains('/') || *size > MAX_REPORT_BYTES {
                continue;
            }
            let parse: fn(&str) -> Vec<ReportTable> = if rel.eq_ignore_ascii_case("report.txt") {
                if have_xml {
                    continue;
                }
                parse_report_txt
            } else if rel.eq_ignore_ascii_case("results.csv") {
                parse_results_csv
            } else {
                continue;
            };
            let b = fs.read(p).map_err(|e| Error::io(p, e))?;
            reports.extend(parse(&decode_text(&b)));
        }
        Ok(ChemStationDataset {
            path: path.to_path_buf(),
            fs: fs.clone(),
            is_dir: meta.is_dir(),
            files,
            signals,
            total_size,
            method_file,
            reports,
            modules,
        })
    }

    /// The vendor's peak reports, one table per signal (for library users).
    pub fn reports(&self) -> &[ReportTable] {
        &self.reports
    }

    /// Our name for a report's signal: the trace of the `.ch` file it names (`FID1 A` →
    /// `FID1A`), `TIC <file>` for an MSD TIC integration, else the report's own text.
    fn report_signal(&self, t: &ReportTable) -> String {
        if let Some(file) = t.signal.strip_prefix("TIC:") {
            let file = file.trim().rsplit(['\\', '/']).next().unwrap_or("").trim();
            if let Some(s) = self.signals.iter().find(|s| {
                s.header.kind == SignalKind::MassSpectra && s.rel.eq_ignore_ascii_case(file)
            }) {
                return format!("TIC {}", s.rel);
            }
            return t.signal.clone();
        }
        let key: String = t.signal.chars().filter(|c| !c.is_whitespace()).collect();
        self.trace_signals()
            .iter()
            .map(|&i| &self.signals[i])
            .find(|s| s.stem().eq_ignore_ascii_case(&key))
            .map_or_else(|| t.signal.clone(), trace_name)
    }

    /// Rows of the `vendor_peaks` table.
    fn report_rows(&self) -> Vec<(String, &ReportPeak)> {
        self.reports
            .iter()
            .flat_map(|t| {
                let name = self.report_signal(t);
                t.peaks.iter().map(move |p| (name.clone(), p))
            })
            .collect()
    }

    /// The `vendor_peaks` table (tables[0]) when the directory holds a vendor report.
    fn report_table_info(&self) -> Option<TableInfo> {
        let rows = self.report_rows();
        if rows.is_empty() {
            return None;
        }
        let mut signals: Vec<String> = Vec::new();
        let mut types: Vec<String> = Vec::new();
        let mut names: Vec<String> = Vec::new();
        for (s, p) in &rows {
            if !signals.contains(s) {
                signals.push(s.clone());
            }
            if !types.contains(&p.peak_type) {
                types.push(p.peak_type.clone());
            }
            if let Some(n) = &p.name
                && !names.contains(n)
            {
                names.push(n.clone());
            }
        }
        let col =
            |index: u32, name: &str, unit: Option<String>, label: &str, dtype: &str| ColumnInfo {
                index,
                name: name.into(),
                label: Some(label.into()),
                dtype: dtype.into(),
                unit,
                range: None,
                extra: BTreeMap::new(),
            };
        let units = |f: fn(&ReportTable) -> Option<String>| {
            let u: Vec<Option<String>> = self.reports.iter().map(f).collect();
            u.first()
                .cloned()
                .flatten()
                .filter(|a| u.iter().all(|x| x.as_deref() == Some(a.as_str())))
        };
        let min = || Some("min".to_string());
        let mut columns = vec![
            col(
                0,
                "signal",
                None,
                "trace or TIC the vendor integrated (code into extra.categories)",
                "uint32",
            ),
            col(
                1,
                "peak",
                None,
                "peak number within its signal's table",
                "float64",
            ),
            col(
                2,
                "rt_min",
                min(),
                "retention time as the report prints it",
                "float64",
            ),
            col(
                3,
                "peak_type",
                None,
                "the report's peak/baseline type code (code into extra.categories)",
                "uint32",
            ),
            col(
                4,
                "width_min",
                min(),
                "peak width as the report prints it (Report.TXT)",
                "float64",
            ),
            col(
                5,
                "area",
                units(|t| t.area_unit.clone()),
                "peak area as the report prints it",
                "float64",
            ),
            col(
                6,
                "height",
                units(|t| t.height_unit.clone()),
                "peak height as the report prints it",
                "float64",
            ),
            col(
                7,
                "area_pct",
                Some("%".into()),
                "area percent of the signal's total",
                "float64",
            ),
            col(
                8,
                "amount",
                None,
                "calculated amount (standard-based reports)",
                "float64",
            ),
            col(
                9,
                "compound",
                None,
                "compound name (code into extra.categories)",
                "uint32",
            ),
            col(
                10,
                "first_scan",
                None,
                "MSD: 1-based scan number of the peak start",
                "float64",
            ),
            col(
                11,
                "max_scan",
                None,
                "MSD: 1-based scan number of the apex",
                "float64",
            ),
            col(
                12,
                "last_scan",
                None,
                "MSD: 1-based scan number of the peak end",
                "float64",
            ),
            col(
                13,
                "start_min",
                min(),
                "start of the integration (Result.xml)",
                "float64",
            ),
            col(
                14,
                "end_min",
                min(),
                "end of the integration (Result.xml)",
                "float64",
            ),
            col(
                15,
                "baseline_start",
                units(|t| t.height_unit.clone()),
                "the vendor's baseline at the start (Result.xml)",
                "float64",
            ),
            col(
                16,
                "baseline_end",
                units(|t| t.height_unit.clone()),
                "the vendor's baseline at the end (Result.xml)",
                "float64",
            ),
            col(
                17,
                "symmetry",
                None,
                "peak symmetry (Result.xml)",
                "float64",
            ),
        ];
        let amount_units: Vec<String> = rows
            .iter()
            .filter_map(|(_, p)| p.amount_unit.clone())
            .fold(Vec::new(), |mut v, u| {
                if !v.contains(&u) {
                    v.push(u);
                }
                v
            });
        if amount_units.len() == 1 {
            columns[8].unit = amount_units.first().cloned();
        }
        columns[0].extra.insert("categories".into(), json!(signals));
        columns[3].extra.insert("categories".into(), json!(types));
        columns[9].extra.insert("categories".into(), json!(names));
        let mut extra = BTreeMap::new();
        extra.insert("source".into(), json!("vendor"));
        extra.insert(
            "reports".into(),
            json!(
                self.reports
                    .iter()
                    .map(|t| json!({
                        "file": t.source,
                        "signal": self.report_signal(t),
                        "signal_text": t.signal,
                        "area_unit": t.area_unit,
                        "height_unit": t.height_unit,
                        "columns": t.columns,
                        "peaks": t.peaks.len(),
                        "rows_skipped": t.skipped,
                    }))
                    .collect::<Vec<_>>()
            ),
        );
        Some(TableInfo {
            index: 0,
            name: Some("vendor_peaks".into()),
            row_count: rows.len() as u64,
            columns,
            extra,
        })
    }

    /// Compare every report peak with our decoded signal: the retention time must fall on our
    /// trace's local maximum (within half a width or one sample) and the height agree within
    /// 10 % (Report.TXT: above a line through the trace 3 widths each side; MSD: the TIC at the
    /// apex scan), for peaks above 1 % of their table's largest.
    fn check_reports(&mut self, report: &mut CheckReport) {
        if self.reports.is_empty() {
            return;
        }
        report.performed(
            "vendor peak reports: retention time on our signal's local maximum, height within 10 %",
        );
        let mut off = Vec::new();
        let mut checked = 0usize;
        for table in self.reports.clone() {
            let largest = table
                .peaks
                .iter()
                .map(|peak| peak.height.abs())
                .fold(0.0, f64::max);
            let big = table
                .peaks
                .iter()
                .filter(|peak| peak.height.abs() >= 0.01 * largest);
            if table.signal.starts_with("TIC:") {
                let name = self.report_signal(&table);
                let Some(sig) = self
                    .signals
                    .iter()
                    .find(|sig| format!("TIC {}", sig.rel) == name)
                else {
                    continue;
                };
                let Body::Scans(recs, _) = &sig.body else {
                    continue;
                };
                let Ok(mut file) = self.fs.open(&sig.path) else {
                    continue;
                };
                for peak in big {
                    let Some(k) = peak.max_scan.and_then(|m| (m as usize).checked_sub(1)) else {
                        continue;
                    };
                    let Some(rec) = recs.get(k) else {
                        off.push(format!(
                            "{name} peak {}: apex scan {} past the {} scans",
                            peak.number,
                            k + 1,
                            recs.len()
                        ));
                        continue;
                    };
                    checked += 1;
                    let tic = record_tic(&mut file, &sig.path, rec)
                        .ok()
                        .flatten()
                        .unwrap_or(f64::NAN);
                    let dt = recs
                        .get(k + 1)
                        .or_else(|| k.checked_sub(1).and_then(|j| recs.get(j)))
                        .map_or(0.0, |o| {
                            (f64::from(o.time_ms) - f64::from(rec.time_ms)).abs() / 60_000.0
                        });
                    let rt = f64::from(rec.time_ms) / 60_000.0;
                    // (a NaN TIC is a mismatch)
                    let height_ok = (tic - peak.height).abs() <= 0.10 * peak.height.abs();
                    if (rt - peak.rt_min).abs() > dt.max(1e-3) || !height_ok {
                        off.push(format!(
                            "{name} peak {} at {:.3} min: apex scan {} at {rt:.3} min, TIC {tic} (report height {})",
                            peak.number, peak.rt_min, k + 1, peak.height
                        ));
                    }
                }
                continue;
            }
            let name = self.report_signal(&table);
            let Some(ti) = self
                .trace_signals()
                .iter()
                .position(|&i| trace_name(&self.signals[i]) == name)
            else {
                continue;
            };
            let sig = self.signals[self.trace_signals()[ti]].clone();
            let (Some(first_ms), Some(last_ms)) =
                (sig.header.first_time_ms, sig.header.last_time_ms)
            else {
                continue;
            };
            let Ok(tr) = self.read_trace(ti as u32, 0, 0, u64::MAX) else {
                continue;
            };
            let values = &tr.channels[0];
            let n = values.len();
            if n < 3 {
                continue;
            }
            let step = (last_ms - first_ms) / intervals(&sig.header, n as u64) as f64 / 60_000.0;
            let x0 = first_ms / 60_000.0;
            let idx = |m: f64| ((m - x0) / step).round();
            // Result.xml: the vendor's limits and baseline, so its area can be recomputed
            let mut area_checked = 0usize;
            let mut area_off = Vec::new();
            let largest_area = table
                .peaks
                .iter()
                .map(|peak| peak.area.abs())
                .fold(0.0, f64::max);
            for peak in table.peaks.iter().filter(|peak| {
                peak.start_min.is_finite()
                    && peak.end_min > peak.start_min
                    && peak.baseline_start.is_finite()
                    && peak.baseline_end.is_finite()
                    && !peak.flagged()
                    && peak.area.abs() >= 0.01 * largest_area
            }) {
                let (i0, i1) = (idx(peak.start_min), idx(peak.end_min));
                if i0 < 0.0 || i1 >= n as f64 || i1 <= i0 {
                    continue;
                }
                let (i0, i1) = (i0 as usize, i1 as usize);
                let line = |x: f64| {
                    peak.baseline_start
                        + (peak.baseline_end - peak.baseline_start) * (x - peak.start_min)
                            / (peak.end_min - peak.start_min)
                };
                let ours: f64 = (i0..i1)
                    .map(|i| {
                        let (xa, xb) = (x0 + i as f64 * step, x0 + (i + 1) as f64 * step);
                        f64::midpoint(values[i] - line(xa), values[i + 1] - line(xb)) * step * 60.0
                    })
                    .sum();
                area_checked += 1;
                // (a NaN area is a mismatch)
                let area_ok = (ours - peak.area).abs() <= 0.10 * peak.area.abs();
                if !area_ok {
                    area_off.push(format!(
                        "{name} peak at {:.3} min: area {} (ours between its limits above its baseline {ours:.3})",
                        peak.rt_min, peak.area
                    ));
                }
            }
            if area_checked > 0 {
                checked += area_checked;
                // a few overlapping or unusual peaks may be integrated differently; a scale or
                // unit error moves them all
                if area_off.len() * 4 > area_checked {
                    off.extend(area_off);
                } else if !area_off.is_empty() {
                    report.push(Finding::info(
                        "vendor_area_outliers",
                        format!(
                            "{} of {area_checked} vendor peak areas on {name} differ from our integration between the same limits by more than 10 % (overlapping or skimmed peaks): {}",
                            area_off.len(),
                            area_off.iter().take(3).cloned().collect::<Vec<_>>().join("; ")
                        ),
                    ));
                }
            }
            for peak in big {
                let width = if peak.width_min.is_finite() && peak.width_min > 0.0 {
                    peak.width_min
                } else {
                    2.0 * step
                };
                let centre = idx(peak.rt_min);
                let half = (width / 2.0 / step).ceil().max(1.0);
                let (lo, hi) = (
                    (centre - half).max(0.0) as usize,
                    (centre + half).min(n as f64 - 1.0) as usize,
                );
                if lo >= hi {
                    continue;
                }
                if peak.flagged() {
                    // skimmed or special peaks: their baseline is not a straight line
                    continue;
                }
                let apex = (lo..=hi).fold(lo, |m, j| if values[j] > values[m] { j } else { m });
                let base = if peak.start_min.is_finite()
                    && peak.end_min > peak.start_min
                    && peak.baseline_start.is_finite()
                    && peak.baseline_end.is_finite()
                {
                    // Result.xml: the vendor's own baseline under the apex
                    let x = x0 + apex as f64 * step;
                    peak.baseline_start
                        + (peak.baseline_end - peak.baseline_start) * (x - peak.start_min)
                            / (peak.end_min - peak.start_min)
                } else {
                    let (b0, b1) = (
                        idx(peak.rt_min - 3.0 * width).max(0.0) as usize,
                        (idx(peak.rt_min + 3.0 * width).min(n as f64 - 1.0)) as usize,
                    );
                    if b1 > b0 {
                        values[b0]
                            + (values[b1] - values[b0]) * (apex.saturating_sub(b0)) as f64
                                / (b1 - b0) as f64
                    } else {
                        0.0
                    }
                };
                let ours = values[apex] - base;
                checked += 1;
                let at_edge = apex == lo || apex == hi;
                if at_edge || (ours - peak.height).abs() > 0.10 * peak.height.abs() {
                    off.push(format!(
                        "{name} peak {} at {:.3} min: our maximum at {:.3} min, height {ours:.4} (report {})",
                        peak.number,
                        peak.rt_min,
                        x0 + apex as f64 * step,
                        peak.height
                    ));
                }
            }
        }
        let skipped: usize = self.reports.iter().map(|table| table.skipped).sum();
        if skipped > 0 {
            report.push(Finding::info(
                "vendor_report_rows_skipped",
                format!("{skipped} rows of the vendor peak reports did not parse as peaks"),
            ));
        }
        // A wrong scale, unit or signal moves most peaks; one or two peaks in a crowded region
        // (a neighbour inside the window, an unusual baseline) are reported, not warned about.
        if off.len() * 4 > checked || (checked < 4 && !off.is_empty()) {
            report.push(Finding::warning(
                "vendor_peak_mismatch",
                format!(
                    "{} of {checked} vendor report peaks do not match our decoded signal (retention time, height or area): {}",
                    off.len(),
                    off.iter().take(5).cloned().collect::<Vec<_>>().join("; ")
                ),
            ));
        } else if !off.is_empty() {
            report.push(Finding::info(
                "vendor_peak_outliers",
                format!(
                    "{} of {checked} vendor report peaks differ from our signal in a crowded region: {}",
                    off.len(),
                    off.iter().take(3).cloned().collect::<Vec<_>>().join("; ")
                ),
            ));
        }
    }

    /// Every decodable signal file (for library users).
    pub fn signals(&self) -> &[SignalFile] {
        &self.signals
    }

    /// Every file in the directory.
    pub fn files(&self) -> &[DirFile] {
        &self.files
    }

    /// Trace index → signal index.
    fn trace_signals(&self) -> Vec<usize> {
        self.signals
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                matches!(
                    s.header.kind,
                    SignalKind::Chromatogram | SignalKind::Spectra
                ) && !matches!(s.body, Body::None)
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// Spectra-run index → signal index.
    fn spectra_signals(&self) -> Vec<usize> {
        self.signals
            .iter()
            .enumerate()
            .filter(|(_, s)| s.header.kind == SignalKind::MassSpectra)
            .map(|(i, _)| i)
            .collect()
    }

    /// Header fields plus the directory's parsed `acqmeth.txt`.
    fn signal_extra(&self, s: &SignalFile) -> BTreeMap<String, Value> {
        let mut m = header_extra(s);
        if let Some(mf) = &self.method_file {
            m.insert("method_file".into(), mf.clone());
        }
        if !self.modules.is_empty() {
            let mods: Vec<Value> = self
                .modules
                .iter()
                .map(|x| {
                    let mut o = serde_json::Map::new();
                    for (k, v) in [
                        ("name", &x.name),
                        ("serial", &x.serial),
                        ("firmware", &x.firmware),
                        ("part_number", &x.part_number),
                    ] {
                        if let Some(v) = v {
                            o.insert(k.into(), json!(v));
                        }
                    }
                    Value::Object(o)
                })
                .collect();
            m.insert("result_modules".into(), Value::Array(mods));
        }
        m
    }

    fn trace_info(&self, index: u32, sig: &SignalFile) -> TraceInfo {
        let header = &sig.header;
        let mut extra = self.signal_extra(sig);
        match &sig.body {
            Body::Channel(d) => {
                let n = d.values.len() as u64;
                let (rate, step_ms) = match (header.first_time_ms, header.last_time_ms) {
                    (Some(a), Some(b)) if n > 1 && b > a => {
                        let step = (b - a) / intervals(header, n) as f64;
                        (1000.0 / step, Some(step))
                    }
                    _ => (0.0, None),
                };
                if let Some(a) = header.first_time_ms {
                    extra.insert("x_start_min".into(), json!(tidy(a / 60_000.0)));
                }
                if let Some(b) = header.last_time_ms {
                    extra.insert("x_end_min".into(), json!(tidy(b / 60_000.0)));
                }
                if let (Some(a), Some(step)) = (header.first_time_ms, step_ms) {
                    extra.insert(
                        "axis".into(),
                        json!({"quantity": "retention_time", "unit": "min",
                               "first": tidy(a / 60_000.0), "step": tidy(step / 60_000.0)}),
                    );
                }
                let dtype = match header.encoding {
                    BodyEncoding::Float64 => "float64",
                    BodyEncoding::SecondDifference => "int64",
                    _ => "int32",
                };
                let mut ch_extra = BTreeMap::new();
                if let Some(t) = &header.signal {
                    ch_extra.insert("signal".into(), json!(t));
                }
                TraceInfo {
                    index,
                    name: Some(header.signal.clone().unwrap_or_else(|| sig.stem())),
                    sample_rate_hz: tidy(rate),
                    sample_count: n,
                    sweep_count: 1,
                    channels: vec![SignalChannelInfo {
                        index: 0,
                        name: sig.stem(),
                        unit: header.units.clone(),
                        dtype: dtype.into(),
                        scale: header.scale,
                        offset: header.offset,
                        extra: ch_extra,
                    }],
                    start_s: header.first_time_ms.map(|a| tidy(a / 1000.0)),
                    extra,
                }
            }
            Body::Scans(recs, _) => {
                let n = recs.len() as u64;
                let first = recs.first().map(|r| f64::from(r.time_ms));
                let last = recs.last().map(|r| f64::from(r.time_ms));
                let (rate, step) = match (first, last) {
                    (Some(a), Some(b)) if n > 1 && b > a => {
                        let st = (b - a) / (n - 1) as f64;
                        (1000.0 / st, Some(st))
                    }
                    _ => (0.0, None),
                };
                let grid = recs.first().map_or([0; 3], |r| r.wavelength_raw);
                let count = recs.first().map_or(0, |r| r.count);
                // f64 records (tag 70) are one more header scale factor from the unit
                let float = recs.first().is_some_and(|r| r.float);
                let scale = if float {
                    header.scale * header.scale
                } else {
                    header.scale
                };
                if float {
                    extra.insert("record_encoding".into(), json!("float64"));
                }
                let channels = (0..count)
                    .map(|i| {
                        let raw = u32::from(grid[0]) + i * u32::from(grid[2]);
                        let mut e = BTreeMap::new();
                        e.insert("wavelength_nm".into(), json!(f64::from(raw) / 20.0));
                        SignalChannelInfo {
                            index: i,
                            name: wavelength_label(raw),
                            unit: header.units.clone(),
                            dtype: if float { "float64" } else { "int32" }.into(),
                            scale,
                            offset: 0.0,
                            extra: e,
                        }
                    })
                    .collect();
                if let Some(a) = first {
                    extra.insert("x_start_min".into(), json!(tidy(a / 60_000.0)));
                }
                if let Some(b) = last {
                    extra.insert("x_end_min".into(), json!(tidy(b / 60_000.0)));
                }
                if let (Some(a), Some(st)) = (first, step) {
                    extra.insert(
                        "axis".into(),
                        json!({"quantity": "retention_time", "unit": "min",
                               "first": tidy(a / 60_000.0), "step": tidy(st / 60_000.0)}),
                    );
                }
                extra.insert(
                    "wavelength_range_nm".into(),
                    json!([
                        f64::from(grid[0]) / 20.0,
                        f64::from(grid[1]) / 20.0,
                        f64::from(grid[2]) / 20.0
                    ]),
                );
                if recs.iter().any(|r| r.wavelength_raw != grid) {
                    extra.insert("wavelength_grid_varies".into(), json!(true));
                }
                TraceInfo {
                    index,
                    name: Some(format!("{} spectra", sig.stem())),
                    sample_rate_hz: tidy(rate),
                    sample_count: n,
                    sweep_count: 1,
                    channels,
                    start_s: first.map(|a| tidy(a / 1000.0)),
                    extra,
                }
            }
            Body::None => TraceInfo::default(),
        }
    }

    fn spectra_info(&self, index: u32, s: &SignalFile) -> SpectraInfo {
        let mut extra = self.signal_extra(s);
        extra.remove("detector");
        let (rt0, rt1, maxpairs) = match &s.body {
            Body::Scans(r, _) => (
                r.first().map(|x| f64::from(x.time_ms) / 1000.0),
                r.last().map(|x| f64::from(x.time_ms) / 1000.0),
                r.iter().map(|x| x.count).max().unwrap_or(0),
            ),
            _ => (None, None, 0),
        };
        extra.insert("max_pairs_per_scan".into(), json!(maxpairs));
        SpectraInfo {
            index,
            name: Some(s.stem()),
            scan_count: s.count(),
            ms_levels: vec![1],
            rt_range_s: rt0.zip(rt1).map(|(a, b)| [a, b]),
            instrument: instrument(&s.header),
            extra,
        }
    }

    fn signal_for_trace(&self, index: u32) -> Result<&SignalFile> {
        let map = self.trace_signals();
        map.get(index as usize)
            .map(|&i| &self.signals[i])
            .ok_or_else(|| {
                Error::Usage(format!(
                    "trace {index} out of range (this data set has {} traces)",
                    map.len()
                ))
            })
    }
}

impl Dataset for ChemStationDataset {
    fn info(&self) -> Result<FileInfo> {
        let traces: Vec<TraceInfo> = self
            .trace_signals()
            .iter()
            .enumerate()
            .map(|(t, &i)| self.trace_info(t as u32, &self.signals[i]))
            .collect();
        let spectra: Vec<SpectraInfo> = self
            .spectra_signals()
            .iter()
            .enumerate()
            .map(|(t, &i)| self.spectra_info(t as u32, &self.signals[i]))
            .collect();
        let mut notes = vec![
            "trace values are scaled: value = raw × scale + offset, with the scale factor from each file's header".to_string(),
        ];
        let undecoded: Vec<String> = self
            .signals
            .iter()
            .filter(|s| matches!(s.body, Body::None))
            .map(|s| format!("{} (version {})", s.rel, s.header.version))
            .collect();
        if !undecoded.is_empty() {
            notes.push(format!(
                "listed but not decoded (unknown version): {}",
                undecoded.join(", ")
            ));
        }
        if self.signals.iter().any(|s| match &s.body {
            Body::Channel(d) => !matches!(d.end, BodyEnd::EndOfFile | BodyEnd::Terminator),
            Body::Scans(_, e) => !matches!(e, BodyEnd::EndOfFile | BodyEnd::Terminator),
            Body::None => false,
        }) {
            notes.push("a signal body ends early (truncated or malformed); run `check`".into());
        }
        let version = self.signals.first().map(|s| s.header.version.clone());
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.total_size,
            format: ChemStationReader.descriptor(),
            format_version: version,
            images: Vec::new(),
            tables: self.report_table_info().into_iter().collect(),
            spectra,
            traces,
            plane_count: 0,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let mut files = serde_json::Map::new();
        for s in &self.signals {
            let mut j = s.header.to_json();
            if let Value::Object(m) = &mut j {
                m.insert("size".into(), json!(s.size));
            }
            files.insert(s.rel.clone(), j);
        }
        let mut other = serde_json::Map::new();
        let mut texts = serde_json::Map::new();
        for f in &self.files {
            if self.signals.iter().any(|s| s.rel == f.rel) {
                continue;
            }
            other.insert(f.rel.clone(), json!({"size": f.size, "version": f.version}));
            let textual = crate::has_ext(
                Path::new(&f.rel),
                &["txt", "xml", "ini", "log", "mac", "bak", "mth", "csv"],
            );
            if textual && f.size <= 4 * MAX_TEXT_PREVIEW as u64 && self.is_dir {
                let p = self.path.join(&f.rel);
                if let Ok(b) = self.fs.read(&p)
                    && let Some(t) = text_preview(&b, MAX_TEXT_PREVIEW)
                {
                    texts.insert(f.rel.clone(), Value::String(t));
                }
            }
        }
        Ok(json!({
            "signal_files": files,
            "other_files": other,
            "text_files": texts,
        }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for (k, s) in [
            ("format_version", Source::Inferred),
            ("traces[].sample_count", Source::Inferred),
            ("traces[].sample_rate_hz", Source::Inferred),
            ("traces[].start_s", Source::Inferred),
            ("traces[].channels[].unit", Source::Inferred),
            ("traces[].channels[].scale", Source::Inferred),
            ("traces[].channels[].offset", Source::PriorArt),
            ("traces[].extra.sample_name", Source::Inferred),
            ("traces[].extra.method", Source::Inferred),
            ("traces[].extra.operator", Source::Inferred),
            ("traces[].extra.acquired_at", Source::Inferred),
            ("traces[].extra.instrument", Source::Inferred),
            ("traces[].extra.signal", Source::Inferred),
            ("traces[].extra.vial", Source::Inferred),
            ("traces[].extra.detector", Source::Inferred),
            ("traces[].extra.wavelength_nm", Source::Inferred),
            ("traces[].extra.x_start_min", Source::Inferred),
            ("traces[].extra.x_end_min", Source::Inferred),
            ("traces[].extra.method_file", Source::Inferred),
            ("traces[].extra.result_modules", Source::Inferred),
            ("spectra[].extra.method_file", Source::Inferred),
            ("spectra[].scan_count", Source::Inferred),
            ("spectra[].rt_range_s", Source::Inferred),
        ] {
            p.insert(k.into(), s);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let mut out = Vec::new();
        for f in &self.files {
            let sig = self.signals.iter().find(|s| s.rel == f.rel);
            let kind = match sig.map(|s| s.header.kind) {
                Some(SignalKind::Chromatogram) => "signal",
                Some(SignalKind::Spectra) => "spectra",
                Some(SignalKind::MassSpectra) => "mass-spectra",
                _ if f.rel.to_ascii_lowercase().ends_with(".reg") => "register",
                _ if f.rel.to_ascii_lowercase().contains(".m/") => "method",
                _ => "file",
            };
            let details = match sig {
                Some(s) => {
                    let (what, n) = match s.header.kind {
                        SignalKind::Chromatogram => ("samples", s.count()),
                        _ => ("scans", s.count()),
                    };
                    let mut d = json!({"version": s.header.version, "encoding": s.header.encoding.name(),
                                       "header_len": s.header.header_len, what: n});
                    if let Some(t) = &s.header.signal {
                        d["signal"] = json!(t);
                    }
                    if let Some(u) = &s.header.units {
                        d["unit"] = json!(u);
                    }
                    d
                }
                None => json!({"version": f.version}),
            };
            out.push(LsEntry {
                kind: kind.into(),
                name: f.rel.clone(),
                offset: None,
                size: Some(f.size),
                image: None,
                details,
            });
            if let Some(s) = sig
                && !matches!(s.body, Body::None)
            {
                out.push(LsEntry {
                    kind: "segment".into(),
                    name: format!("{}:header", f.rel),
                    offset: Some(0),
                    size: Some(s.header.header_len.min(s.size)),
                    image: None,
                    details: json!({}),
                });
                out.push(LsEntry {
                    kind: "segment".into(),
                    name: format!("{}:body", f.rel),
                    offset: Some(s.header.header_len),
                    size: Some(s.size.saturating_sub(s.header.header_len)),
                    image: None,
                    details: json!({}),
                });
            }
        }
        Ok(out)
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::unsupported(
            CHEMSTATION_ID,
            "image planes",
            "ChemStation data holds chromatograms and spectra: use `openreadout trace`, `export --to csv`, or read_spectrum.",
        ))
    }

    fn read_trace(
        &mut self,
        index: u32,
        sweep: u32,
        first_sample: u64,
        max_samples: u64,
    ) -> Result<Trace> {
        if sweep != 0 {
            return Err(Error::Usage(format!(
                "sweep {sweep} out of range (ChemStation traces have one sweep)"
            )));
        }
        let sig = self.signal_for_trace(index)?.clone();
        let total = sig.count();
        if first_sample > total {
            return Err(Error::Usage(format!(
                "first sample {first_sample} is past the end ({total} samples)"
            )));
        }
        let n = max_samples.min(total - first_sample);
        let start = first_sample as usize;
        let end = start + n as usize;
        let header = &sig.header;
        match &sig.body {
            Body::Channel(d) => Ok(Trace {
                trace: index,
                sweep: 0,
                first_sample,
                channels: vec![
                    d.values[start..end]
                        .iter()
                        .map(|v| v * header.scale + header.offset)
                        .collect(),
                ],
            }),
            Body::Scans(recs, _) => {
                let grid = recs.first().map_or([0; 3], |r| r.wavelength_raw);
                let width = recs.first().map_or(0, |r| r.count) as usize;
                let mut cols = vec![Vec::with_capacity(n as usize); width];
                let mut file = self
                    .fs
                    .open(&sig.path)
                    .map_err(|e| Error::io(&sig.path, e))?;
                for r in &recs[start..end] {
                    let rec = read_range(&mut file, &sig.path, r.offset, r.len)?;
                    let mut vals = uv_values(&rec, r.count)
                        .map_err(|e| Error::corrupt_at(CHEMSTATION_ID, r.offset, e))?;
                    if r.float {
                        // f64 records: raw × scale is still one scale factor from the unit
                        for v in &mut vals {
                            *v *= header.scale;
                        }
                    }
                    if r.wavelength_raw == grid {
                        for (c, v) in vals.iter().enumerate().take(width) {
                            cols[c].push(v * header.scale);
                        }
                    } else {
                        // a record on another grid: place values by wavelength, NaN elsewhere
                        let [lo, _, step] = r.wavelength_raw;
                        for (c, col) in cols.iter_mut().enumerate() {
                            let wl = u32::from(grid[0]) + c as u32 * u32::from(grid[2]);
                            let k = wl
                                .checked_sub(u32::from(lo))
                                .filter(|d| step != 0 && d % u32::from(step) == 0)
                                .map(|d| (d / u32::from(step)) as usize);
                            col.push(
                                k.and_then(|k| vals.get(k))
                                    .map_or(f64::NAN, |v| v * header.scale),
                            );
                        }
                    }
                }
                Ok(Trace {
                    trace: index,
                    sweep: 0,
                    first_sample,
                    channels: cols,
                })
            }
            Body::None => Err(Error::unsupported(
                CHEMSTATION_ID,
                format!("version {} signal body", header.version),
                "This ChemStation file version is listed but its body encoding is not decoded yet.",
            )),
        }
    }

    fn visit_scan_headers(
        &mut self,
        run: u32,
        first: u64,
        visit: &mut dyn FnMut(openreadout_core::ScanHeader) -> bool,
    ) -> Result<bool> {
        let map = self.spectra_signals();
        let s = map
            .get(run as usize)
            .map(|&i| self.signals[i].clone())
            .ok_or_else(|| {
                Error::Usage(format!(
                    "spectra run {run} out of range (this data set has {} runs)",
                    map.len()
                ))
            })?;
        let Body::Scans(recs, _) = &s.body else {
            return Err(Error::Other("not a spectra file".into()));
        };
        let mut f = self.fs.open(&s.path).map_err(|e| Error::io(&s.path, e))?;
        for (i, r) in recs
            .iter()
            .enumerate()
            .skip(usize::try_from(first).unwrap_or(usize::MAX))
        {
            let tic = record_tic(&mut f, &s.path, r)?;
            let h = openreadout_core::ScanHeader {
                index: i as u64,
                scan_number: i as u64 + 1,
                ms_level: 1,
                rt_s: Some(f64::from(r.time_ms) / 1000.0),
                polarity: "unknown".into(),
                total_ion_current: tic,
                point_count: Some(u64::from(r.count)),
                ..openreadout_core::ScanHeader::default()
            };
            if !visit(h) {
                break;
            }
        }
        Ok(true)
    }

    fn read_spectrum(&mut self, index: u32, spectrum: u64) -> Result<Spectrum> {
        let map = self.spectra_signals();
        let s = map
            .get(index as usize)
            .map(|&i| self.signals[i].clone())
            .ok_or_else(|| {
                Error::Usage(format!(
                    "spectra run {index} out of range (this data set has {} runs)",
                    map.len()
                ))
            })?;
        let Body::Scans(recs, _) = &s.body else {
            return Err(Error::Other("not a spectra file".into()));
        };
        let r = recs.get(spectrum as usize).ok_or_else(|| {
            Error::Usage(format!(
                "spectrum {spectrum} out of range (run {index} has {} scans)",
                recs.len()
            ))
        })?;
        let mut f = self.fs.open(&s.path).map_err(|e| Error::io(&s.path, e))?;
        let rec = read_range(&mut f, &s.path, r.offset, r.len)?;
        let expected = MS_RECORD_HEADER as u64 + 4 * u64::from(r.count) + MS_RECORD_TRAILER as u64;
        if r.len != expected {
            return Err(Error::corrupt_at(
                CHEMSTATION_ID,
                r.offset,
                format!(
                    "scan record of {} bytes holds {} pairs (expected {expected} bytes)",
                    r.len, r.count
                ),
            ));
        }
        let (mut mz, mut intensity, tic) =
            ms_pairs(&rec, r.count).map_err(|e| Error::corrupt_at(CHEMSTATION_ID, r.offset, e))?;
        if mz.windows(2).all(|w| w[0] >= w[1]) {
            mz.reverse();
            intensity.reverse();
        }
        Ok(Spectrum {
            index: spectrum,
            scan_number: spectrum + 1,
            ms_level: 1,
            rt_s: Some(f64::from(r.time_ms) / 1000.0),
            polarity: "unknown".into(),
            centroided: false,
            precursor_mz: None,
            precursor_charge: None,
            scan_filter: None,
            total_ion_current: Some(f64::from(tic)),
            mz,
            intensity,
            ..Spectrum::default()
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), CHEMSTATION_ID);
        r.performed("every file: ChemStation version string at byte 0; known versions decoded");
        r.performed("header length inside the file");
        r.performed(".ch bodies decoded to the end: record tags, escape values, truncation");
        r.performed("point count against the header's first/last retention time");
        r.performed(".uv/.ms: scan records walked (tag, length) and counted against the header; every record decoded");
        for s in self.signals.clone() {
            let h = &s.header;
            let name = &s.rel;
            if matches!(s.body, Body::None) {
                r.push(Finding::warning(
                    "unknown_version",
                    format!("{name}: version {} is not decoded", h.version),
                ));
                continue;
            }
            if h.header_len == 0 || h.header_len > s.size {
                r.push(Finding::error(
                    "bad_header_length",
                    format!(
                        "{name}: header length {} does not fit the file ({} bytes)",
                        h.header_len, s.size
                    ),
                ));
                continue;
            }
            match &s.body {
                Body::Channel(d) => {
                    match &d.end {
                        BodyEnd::Truncated { at } => r.push(
                            Finding::error(
                                "truncated",
                                format!(
                                    "{name}: body ends inside a value ({} values decoded)",
                                    d.values.len()
                                ),
                            )
                            .at(h.header_len + at),
                        ),
                        BodyEnd::BadRecord { at, detail } => r.push(
                            Finding::error("bad_record", format!("{name}: {detail}"))
                                .at(h.header_len + at),
                        ),
                        BodyEnd::EndOfFile if h.encoding == BodyEncoding::DeltaRecords => {
                            r.push(Finding::info(
                                "no_terminator",
                                format!("{name}: body ends without the two zero bytes after the last record"),
                            ));
                        }
                        _ => {}
                    }
                    let n = d.values.len();
                    if n == 0 {
                        r.push(Finding::warning(
                            "no_samples",
                            format!("{name}: no samples"),
                        ));
                    }
                    if let (Some(a), Some(b)) = (h.first_time_ms, h.last_time_ms) {
                        if b <= a && n > 1 {
                            r.push(Finding::warning(
                                "bad_time_range",
                                format!("{name}: last retention time {b} ms is not after the first {a} ms"),
                            ));
                        } else if n > 2 {
                            // is the range a whole number of steps of a round interval?
                            let step = (b - a) / intervals(h, n as u64) as f64;
                            if (step - step.round()).abs() >= 1e-3 {
                                r.push(Finding::info(
                                    "time_range_mismatch",
                                    format!(
                                        "{name}: {n} values over {a}–{b} ms give a {step:.4} ms interval (not a whole number of milliseconds)"
                                    ),
                                ));
                            }
                        }
                    }
                }
                Body::Scans(recs, end) => {
                    match end {
                        BodyEnd::Truncated { at } => r.push(
                            Finding::error(
                                "truncated",
                                format!("{name}: body ends inside scan record {}", recs.len()),
                            )
                            .at(h.header_len + at),
                        ),
                        BodyEnd::BadRecord { at, detail } => r.push(
                            Finding::error(
                                "bad_record",
                                format!("{name}: scan {}: {detail}", recs.len()),
                            )
                            .at(h.header_len + at),
                        ),
                        _ => {}
                    }
                    if let Some(d) = h.declared_records
                        && d != recs.len() as u64
                    {
                        let sev = if (recs.len() as u64) < d && !matches!(end, BodyEnd::Terminator)
                        {
                            Finding::error(
                                "truncated",
                                format!("{name}: header declares {d} scans, {} found", recs.len()),
                            )
                        } else {
                            Finding::warning(
                                "scan_count_mismatch",
                                format!("{name}: header declares {d} scans, {} found", recs.len()),
                            )
                        };
                        r.push(sev);
                    }
                    let uv = h.encoding == BodyEncoding::SpectrumRecords;
                    let mut f = self.fs.open(&s.path).map_err(|e| Error::io(&s.path, e))?;
                    let mut bad = 0u64;
                    let grid = recs.first().map(|x| x.wavelength_raw);
                    let mut grid_changes = 0u64;
                    for (i, rec) in recs.iter().enumerate() {
                        let bytes = read_range(&mut f, &s.path, rec.offset, rec.len)?;
                        let res = if uv {
                            if Some(rec.wavelength_raw) != grid {
                                grid_changes += 1;
                            }
                            uv_values(&bytes, rec.count).map(|_| ())
                        } else {
                            let expected = MS_RECORD_HEADER as u64
                                + 4 * u64::from(rec.count)
                                + MS_RECORD_TRAILER as u64;
                            if rec.len == expected {
                                ms_pairs(&bytes, rec.count).map(|_| ())
                            } else {
                                Err(format!("{} bytes for {} pairs", rec.len, rec.count))
                            }
                        };
                        if let Err(e) = res {
                            bad += 1;
                            if bad <= 3 {
                                r.push(
                                    Finding::error("bad_record", format!("{name}: scan {i}: {e}"))
                                        .at(rec.offset),
                                );
                            }
                        }
                    }
                    if grid_changes > 0 {
                        r.push(Finding::info(
                            "wavelength_grid_changes",
                            format!("{name}: {grid_changes} scans use a different wavelength range than the first"),
                        ));
                    }
                    if recs.windows(2).any(|w| w[1].time_ms < w[0].time_ms) {
                        r.push(Finding::warning(
                            "time_not_monotonic",
                            format!("{name}: scan times decrease somewhere"),
                        ));
                    }
                }
                Body::None => {}
            }
        }
        self.check_reports(&mut r);
        Ok(r)
    }

    fn read_table(&mut self, index: u32, first_row: u64, max_rows: u64) -> Result<Table> {
        let Some(info) = self.report_table_info().filter(|_| index == 0) else {
            return Err(Error::Usage(format!(
                "table {index} not found (this data set has {} tables)",
                usize::from(!self.reports.is_empty())
            )));
        };
        let cats = |c: usize| -> Vec<String> {
            info.columns[c]
                .extra
                .get("categories")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .map(|x| x.as_str().unwrap_or_default().to_string())
                        .collect()
                })
                .unwrap_or_default()
        };
        let (signals, types, names) = (cats(0), cats(3), cats(9));
        let rows = self.report_rows();
        let start = usize::try_from(first_row.min(rows.len() as u64)).unwrap_or(0);
        let end = start
            .saturating_add(usize::try_from(max_rows).unwrap_or(usize::MAX))
            .min(rows.len());
        let code =
            |v: &[String], s: &str| v.iter().position(|c| c == s).map_or(f64::NAN, |i| i as f64);
        let scan = |s: Option<u32>| s.map_or(f64::NAN, f64::from);
        let mut columns: Vec<Vec<f64>> = vec![Vec::with_capacity(end - start); info.columns.len()];
        for (sig, p) in &rows[start..end] {
            let vals = [
                code(&signals, sig),
                f64::from(p.number),
                p.rt_min,
                code(&types, &p.peak_type),
                p.width_min,
                p.area,
                p.height,
                p.area_pct,
                p.amount,
                p.name.as_deref().map_or(f64::NAN, |n| code(&names, n)),
                scan(p.first_scan),
                scan(p.max_scan),
                scan(p.last_scan),
                p.start_min,
                p.end_min,
                p.baseline_start,
                p.baseline_end,
                p.symmetry,
            ];
            for (c, v) in columns.iter_mut().zip(vals) {
                c.push(v);
            }
        }
        Ok(Table {
            table: index,
            first_row: start as u64,
            columns,
        })
    }
}
