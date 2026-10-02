//! `Dataset` for Agilent OpenLab CDS injections: a `.dx` container (zip) holding the injection
//! manifest (`injection.acmd`), one ChemStation-style version-179 signal part per detector
//! channel (`<guid>.CH`) and per instrument curve (`<guid>.IT`); the vendor's integration
//! results from the `.rx` package with the same stem; the sequence file (`.acaml`) of the
//! result-set folder when there is one. Layout: `docs/formats/openlab-cds.md`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_core::model::{
    CheckReport, ColumnInfo, FileInfo, Finding, LsEntry, SignalChannelInfo, Table, TableInfo,
    Trace, TraceInfo,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::source::{Fs, Input};
use openreadout_core::xmljson::xml_to_json;
use openreadout_core::{Error, Plane, Result};
use serde_json::{Value, json};

use crate::binary::tidy;
use crate::chemstation_decode::{
    BodyEnd, DecodedSignal, decode_delta_records, decode_float64, decode_second_difference,
};
use crate::chemstation_header::{BodyEncoding, SignalHeader, looks_like_chemstation};
use crate::openlab_xml::{
    InjectionManifest, InjectionResults, ManifestSignal, SequenceFile, SequenceInjection,
    parse_manifest, parse_results, parse_sequence,
};
use crate::{OPENLAB_ID, OpenLabReader};
use openreadout_core::zip::{ZipIndex, text as zip_text};

/// The manifest member of a `.dx`.
pub const MANIFEST_MEMBER: &str = "injection.acmd";
/// The results member of a `.rx`.
pub const RESULTS_MEMBER: &str = "Base/InjectionACAML";
/// Largest sequence file (`.acaml`) read.
const MAX_SEQUENCE_BYTES: u64 = 64 << 20;
/// Bytes per instrument-curve sample: f64 time (ms) and f64 raw value.
pub const PAIR_BYTES: usize = 16;

/// How a decoded part's body is stored.
#[derive(Debug, Clone)]
pub enum PartBody {
    /// A detector signal (`Signal179`): values on a regular time grid.
    Channel(DecodedSignal),
    /// An instrument curve (`InstrumentTrace179`): (time ms, raw value) pairs.
    Pairs {
        times_ms: Vec<f64>,
        raw: Vec<f64>,
        end: BodyEnd,
    },
    /// A DAD spectra part (`Spectra131`): a ChemStation-style `.uv` file, read with the
    /// ChemStation reader (index into the dataset's `spectra`).
    Spectra(usize),
    /// The manifest lists the signal but the container has no part for it.
    Missing,
    /// A part we list but do not decode (other content types, e.g. spectra).
    NotDecoded,
}

/// One signal of the manifest with its part.
#[derive(Debug, Clone)]
pub struct DxSignal {
    pub manifest: ManifestSignal,
    /// Member name of the part, when present.
    pub part: Option<String>,
    pub part_size: u64,
    /// The part's ChemStation-style header.
    pub header: Option<SignalHeader>,
    pub body: PartBody,
}

impl DxSignal {
    /// Values (channel) or samples (curve) decoded.
    pub fn count(&self) -> u64 {
        match &self.body {
            PartBody::Channel(d) => d.values.len() as u64,
            PartBody::Pairs { raw, .. } => raw.len() as u64,
            _ => 0,
        }
    }
    fn scale(&self) -> f64 {
        self.header.as_ref().map_or(1.0, |h| h.scale)
    }
    fn offset(&self) -> f64 {
        self.header.as_ref().map_or(0.0, |h| h.offset)
    }
    fn unit(&self) -> Option<String> {
        self.manifest
            .units
            .clone()
            .or_else(|| self.header.as_ref().and_then(|h| h.units.clone()))
    }
    /// Regular sample interval of a channel, ms: the header's first/last time over n − 1.
    fn channel_step_ms(&self) -> Option<(f64, f64)> {
        let h = self.header.as_ref()?;
        let n = self.count();
        let (a, b) = (h.first_time_ms?, h.last_time_ms?);
        let intervals = if h.version == "81" {
            n
        } else {
            n.saturating_sub(1)
        };
        (intervals > 0 && b > a).then(|| (a, (b - a) / intervals as f64))
    }
    /// Regular interval of a curve, ms (`None` when its times are not evenly spaced).
    fn curve_step_ms(&self) -> Option<(f64, f64)> {
        let PartBody::Pairs { times_ms, .. } = &self.body else {
            return None;
        };
        let first = *times_ms.first()?;
        if times_ms.len() < 2 {
            return None;
        }
        let step = (times_ms[times_ms.len() - 1] - first) / (times_ms.len() - 1) as f64;
        let regular = step > 0.0
            && times_ms
                .windows(2)
                .all(|w| ((w[1] - w[0]) - step).abs() <= 1e-6 * step);
        regular.then_some((first, step))
    }
    /// The sample nearest `t_min` (minutes) of a channel, scaled.
    fn value_near(&self, t_min: f64) -> Option<f64> {
        let PartBody::Channel(d) = &self.body else {
            return None;
        };
        let (a, step) = self.channel_step_ms()?;
        let k = ((t_min * 60_000.0 - a) / step).round();
        if !(0.0..d.values.len() as f64).contains(&k) {
            return None;
        }
        Some(d.values[k as usize] * self.scale() + self.offset())
    }
}

/// One vendor peak with the signal it belongs to.
#[derive(Debug, Clone)]
pub struct PeakRow {
    /// Index into the results' `signals`.
    pub result_signal: usize,
    /// Signal name (manifest channel), or the results' signal id when unmapped.
    pub signal: String,
    /// Our trace index, when mapped.
    pub trace: Option<u32>,
    pub peak: crate::openlab_xml::VendorPeak,
}

/// How result signals were matched to the container's signals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalMapping {
    /// Through the sequence file's signal ids and trace ids.
    SequenceFile,
    /// Each result signal's peak-limit baseline values equal one signal's samples.
    BaselineMatch,
    /// One integrable signal, one result signal with peaks.
    SingleSignal,
    /// Not matched (no container, or no evidence).
    Unmapped,
}

impl SignalMapping {
    /// Our name (JSON).
    pub fn name(self) -> &'static str {
        match self {
            SignalMapping::SequenceFile => "sequence_file",
            SignalMapping::BaselineMatch => "baseline_match",
            SignalMapping::SingleSignal => "single_signal",
            SignalMapping::Unmapped => "unmapped",
        }
    }
}

/// A member of the `.dx` container.
#[derive(Debug, Clone)]
pub struct DxPart {
    pub name: String,
    pub size: u64,
    pub compressed_size: u64,
    pub method: u16,
}

/// An opened OpenLab CDS injection (`.dx`, optionally with its `.rx`), or a `.rx` alone.
#[derive(Debug)]
pub struct OpenLabDataset {
    path: PathBuf,
    fs: Fs,
    /// The `.dx` file, when there is one.
    dx_path: Option<PathBuf>,
    dx_size: u64,
    parts: Vec<DxPart>,
    content_types: Option<String>,
    manifest: Option<InjectionManifest>,
    manifest_xml: Option<String>,
    signals: Vec<DxSignal>,
    results_path: Option<PathBuf>,
    results_size: u64,
    results: Option<InjectionResults>,
    results_xml: Option<String>,
    results_error: Option<String>,
    sequence_path: Option<PathBuf>,
    sequence: Option<SequenceFile>,
    mapping: SignalMapping,
    rows: Vec<PeakRow>,
    notes: Vec<String>,
    /// Spectra parts opened with the ChemStation `.uv` reader (in memory).
    spectra: Vec<crate::chemstation_dataset::ChemStationDataset>,
}

fn ext_is(p: &Path, e: &str) -> bool {
    p.extension()
        .is_some_and(|x| x.to_string_lossy().eq_ignore_ascii_case(e))
}

/// A sibling of `path` with extension `ext` and the same stem (case-insensitive).
fn sibling(fs: &Fs, path: &Path, ext: &str) -> Option<PathBuf> {
    let stem = path.file_stem()?.to_string_lossy().to_ascii_lowercase();
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty());
    let candidate = path.with_extension(ext);
    if fs.is_file(&candidate) {
        return Some(candidate);
    }
    let entries = fs.read_dir(dir.unwrap_or(Path::new("."))).ok()?;
    entries.flatten().map(|e| e.path()).find(|p| {
        ext_is(p, ext)
            && p.file_stem()
                .is_some_and(|s| s.to_string_lossy().to_ascii_lowercase() == stem)
            && fs.is_file(p)
    })
}

/// Every `.acaml` file next to `path`.
fn sequence_files(fs: &Fs, path: &Path) -> Vec<PathBuf> {
    let dir = path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let Ok(entries) = fs.read_dir(dir) else {
        return Vec::new();
    };
    let mut v: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| ext_is(p, "acaml") && fs.is_file(p))
        .collect();
    v.sort();
    v
}

/// The `.dx` files of a result-set folder (for the hint when a folder is opened).
pub fn injections_in(fs: &Fs, dir: &Path) -> Vec<String> {
    let Ok(entries) = fs.read_dir(dir) else {
        return Vec::new();
    };
    let mut v: Vec<String> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| ext_is(p, "dx"))
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();
    v.sort();
    v
}

/// Decode (time ms, raw value) pairs.
pub fn decode_pairs(body: &[u8]) -> (Vec<f64>, Vec<f64>, BodyEnd) {
    let n = body.len() / PAIR_BYTES;
    let mut times = Vec::with_capacity(n);
    let mut values = Vec::with_capacity(n);
    let (pairs, rest) = body.as_chunks::<PAIR_BYTES>();
    for pair in pairs {
        let mut a = [0u8; 8];
        let mut b = [0u8; 8];
        a.copy_from_slice(&pair[..8]);
        b.copy_from_slice(&pair[8..]);
        times.push(f64::from_le_bytes(a));
        values.push(f64::from_le_bytes(b));
    }
    let end = if rest.is_empty() {
        BodyEnd::EndOfFile
    } else {
        BodyEnd::Truncated {
            at: (n * PAIR_BYTES) as u64,
        }
    };
    (times, values, end)
}

/// Decode one part (a ChemStation-style header and body) as the manifest's content type says.
fn decode_part(kind: &str, bytes: &[u8]) -> (Option<SignalHeader>, PartBody) {
    if looks_like_chemstation(bytes).is_none() {
        return (None, PartBody::NotDecoded);
    }
    let Some(h) = SignalHeader::parse(bytes) else {
        return (None, PartBody::NotDecoded);
    };
    let hl = h.header_len as usize;
    let body = if h.header_len == 0 || hl > bytes.len() {
        None
    } else {
        Some(&bytes[hl..])
    };
    let empty = || DecodedSignal {
        values: Vec::new(),
        escapes: 0,
        records: 0,
        end: BodyEnd::Truncated { at: 0 },
    };
    let decoded = if kind.starts_with("InstrumentTrace") {
        match body {
            Some(b) => {
                let (times_ms, raw, end) = decode_pairs(b);
                PartBody::Pairs { times_ms, raw, end }
            }
            None => PartBody::Pairs {
                times_ms: Vec::new(),
                raw: Vec::new(),
                end: BodyEnd::Truncated { at: 0 },
            },
        }
    } else if kind.starts_with("Signal") {
        PartBody::Channel(match (h.encoding, body) {
            (_, None) => empty(),
            (BodyEncoding::Float64, Some(b)) => decode_float64(b),
            (BodyEncoding::SecondDifference, Some(b)) => {
                decode_second_difference(b, h.version == "181")
            }
            (BodyEncoding::DeltaRecords, Some(b)) => decode_delta_records(b),
            _ => return (Some(h), PartBody::NotDecoded),
        })
    } else {
        PartBody::NotDecoded
    };
    (Some(h), decoded)
}

fn part_ext(kind: &str) -> Option<&'static str> {
    if kind.starts_with("InstrumentTrace") {
        Some("it")
    } else if kind.starts_with("Spectra") {
        Some("uv")
    } else if kind.starts_with("Signal") {
        Some("ch")
    } else {
        None
    }
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
}

/// The last component of a Windows or POSIX path.
fn basename(s: &str) -> &str {
    s.rsplit(['\\', '/']).next().unwrap_or(s)
}

/// Injection volume in µL from the manifest's value and unit.
fn volume_ul(v: f64, unit: Option<&str>) -> Option<f64> {
    let u = unit.unwrap_or("µL").trim();
    let k = match u {
        "µL" | "μL" | "uL" | "ul" | "µl" | "μl" => 1.0,
        "nL" | "nl" => 1e-3,
        "mL" | "ml" => 1e3,
        _ => return None,
    };
    Some(v * k)
}

impl OpenLabDataset {
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    /// Open an [`Input`]: a `.dx` (with its `.rx` and the folder's `.acaml` when present) or a
    /// `.rx` (with its `.dx` when present).
    pub(crate) fn open_input(input: &Input) -> Result<Self> {
        let (path, fs) = (input.path(), input.fs());
        let meta = fs.metadata(path).map_err(|e| Error::io(path, e))?;
        if meta.is_dir() {
            let dx = injections_in(fs, path);
            let shown: Vec<&str> = dx.iter().take(5).map(String::as_str).collect();
            return Err(Error::unsupported(
                OPENLAB_ID,
                "an OpenLab CDS result-set folder as one data set",
                if dx.is_empty() {
                    "Open the injection files (.dx) of the result set one at a time.".to_string()
                } else {
                    format!(
                        "A result set holds one .dx per injection ({} here, e.g. {}); open one, or run `openreadout batch '{}/*.dx'`.",
                        dx.len(),
                        shown.join(", "),
                        path.display()
                    )
                },
            ));
        }
        let (dx_path, rx_path) = if ext_is(path, "rx") {
            (sibling(fs, path, "dx"), Some(path.to_path_buf()))
        } else {
            (Some(path.to_path_buf()), sibling(fs, path, "rx"))
        };
        let mut ds = OpenLabDataset {
            path: path.to_path_buf(),
            fs: fs.clone(),
            dx_path: dx_path.clone(),
            dx_size: 0,
            parts: Vec::new(),
            content_types: None,
            manifest: None,
            manifest_xml: None,
            signals: Vec::new(),
            results_path: None,
            results_size: 0,
            results: None,
            results_xml: None,
            results_error: None,
            sequence_path: None,
            sequence: None,
            mapping: SignalMapping::Unmapped,
            rows: Vec::new(),
            notes: Vec::new(),
            spectra: Vec::new(),
        };
        if let Some(dx) = &dx_path {
            ds.read_dx(dx)?;
            if ext_is(path, "rx") {
                ds.notes.push(format!(
                    "opened together with its injection container {}",
                    file_name(dx)
                ));
            }
        }
        if let Some(rx) = &rx_path {
            match ds.read_rx(rx) {
                Ok(()) => {}
                // the .rx is the file asked for: its errors are the caller's errors
                Err(e) if dx_path.is_none() || ext_is(path, "rx") => return Err(e),
                Err(e) => ds.results_error = Some(e.to_string()),
            }
        }
        ds.read_sequence();
        ds.map_results();
        Ok(ds)
    }

    fn read_dx(&mut self, dx: &Path) -> Result<()> {
        let z = ZipIndex::open(&self.fs, dx, OPENLAB_ID)?;
        self.dx_size = z.file_len;
        self.parts = z
            .members
            .iter()
            .map(|m| DxPart {
                name: m.name.clone(),
                size: m.size,
                compressed_size: m.compressed_size,
                method: m.method,
            })
            .collect();
        self.content_types = z.read_text("[Content_Types].xml").ok().flatten();
        let xml = z.read_text(MANIFEST_MEMBER)?.ok_or_else(|| {
            Error::corrupt(
                OPENLAB_ID,
                format!(
                    "{}: no {MANIFEST_MEMBER} member (not an OpenLab CDS injection container)",
                    file_name(dx)
                ),
            )
        })?;
        let manifest = parse_manifest(&xml)
            .map_err(|e| Error::corrupt(OPENLAB_ID, format!("{MANIFEST_MEMBER}: {e}")))?;
        for ms in &manifest.signals {
            let kind = ms.kind().to_string();
            let wanted = part_ext(&kind);
            let member = z
                .members
                .iter()
                .filter(|m| {
                    Path::new(&m.name)
                        .file_stem()
                        .is_some_and(|s| s.to_string_lossy().eq_ignore_ascii_case(&ms.trace_id))
                })
                .find(|m| wanted.is_none_or(|e| ext_is(Path::new(&m.name), e)))
                .cloned();
            let (part, part_size, header, body) = match member {
                None => (None, 0, None, PartBody::Missing),
                Some(m) if wanted.is_none() => (Some(m.name), m.size, None, PartBody::NotDecoded),
                Some(m) if kind.starts_with("Spectra") => {
                    // a ChemStation-style `.uv` file: read it with the ChemStation reader
                    let bytes = z.read(&m)?;
                    let header = SignalHeader::parse(&bytes);
                    let input = Input::from_bytes(format!("{}.uv", ms.trace_id), bytes);
                    match crate::chemstation_dataset::ChemStationDataset::open_input(&input) {
                        Ok(ds) => {
                            self.spectra.push(ds);
                            (
                                Some(m.name),
                                m.size,
                                header,
                                PartBody::Spectra(self.spectra.len() - 1),
                            )
                        }
                        Err(e) => {
                            self.notes
                                .push(format!("{}: spectra part not read: {e}", m.name));
                            (Some(m.name), m.size, header, PartBody::NotDecoded)
                        }
                    }
                }
                Some(m) => {
                    let bytes = z.read(&m)?;
                    let (h, b) = decode_part(&kind, &bytes);
                    (Some(m.name), m.size, h, b)
                }
            };
            self.signals.push(DxSignal {
                manifest: ms.clone(),
                part,
                part_size,
                header,
                body,
            });
        }
        let missing: Vec<&str> = self
            .signals
            .iter()
            .filter(|s| matches!(s.body, PartBody::Missing))
            .map(|s| s.manifest.channel.as_str())
            .collect();
        if !missing.is_empty() {
            self.notes.push(format!(
                "the manifest lists {} signal(s) the container has no part for: {}",
                missing.len(),
                missing.join(", ")
            ));
        }
        let undecoded: Vec<String> = self
            .signals
            .iter()
            .filter(|s| matches!(s.body, PartBody::NotDecoded))
            .map(|s| format!("{} ({})", s.manifest.channel, s.manifest.kind()))
            .collect();
        if !undecoded.is_empty() {
            self.notes
                .push(format!("listed but not decoded: {}", undecoded.join(", ")));
        }
        let short: Vec<String> = self
            .signals
            .iter()
            .filter(|s| {
                s.manifest.declared_values.is_some_and(|d| {
                    s.count() < d && !matches!(s.body, PartBody::Missing | PartBody::NotDecoded)
                })
            })
            .map(|s| {
                format!(
                    "{} ({} of {})",
                    s.manifest.channel,
                    s.count(),
                    s.manifest.declared_values.unwrap_or(0)
                )
            })
            .collect();
        if !short.is_empty() {
            self.notes.push(format!(
                "signal parts hold fewer values than the manifest declares: {}; run `check`",
                short.join(", ")
            ));
        }
        self.manifest = Some(manifest);
        self.manifest_xml = Some(xml);
        Ok(())
    }

    fn read_rx(&mut self, rx: &Path) -> Result<()> {
        let z = ZipIndex::open(&self.fs, rx, OPENLAB_ID)?;
        self.results_size = z.file_len;
        let xml = z.read_text(RESULTS_MEMBER)?.ok_or_else(|| {
            Error::corrupt(
                OPENLAB_ID,
                format!(
                    "{}: no {RESULTS_MEMBER} member (not an OpenLab CDS result package)",
                    file_name(rx)
                ),
            )
        })?;
        let r = parse_results(&xml)
            .map_err(|e| Error::corrupt(OPENLAB_ID, format!("{RESULTS_MEMBER}: {e}")))?;
        self.results = Some(r);
        self.results_xml = Some(xml);
        self.results_path = Some(rx.to_path_buf());
        Ok(())
    }

    /// The folder's sequence file, when it lists this injection.
    fn read_sequence(&mut self) {
        let dx_name = self.dx_path.as_deref().map(file_name).unwrap_or_default();
        let meas = self.results.as_ref().and_then(|r| r.measurement_id.clone());
        for p in sequence_files(&self.fs, &self.path) {
            let Ok(meta) = self.fs.metadata(&p) else {
                continue;
            };
            if meta.len() > MAX_SEQUENCE_BYTES {
                continue;
            }
            let Ok(bytes) = self.fs.read(&p) else {
                continue;
            };
            match parse_sequence(&zip_text(&bytes)) {
                Ok(s) if s.injection(&dx_name, meas.as_deref()).is_some() => {
                    self.sequence = Some(s);
                    self.sequence_path = Some(p);
                    return;
                }
                Ok(_) => {}
                Err(e) => self
                    .notes
                    .push(format!("sequence file {} not read: {e}", file_name(&p))),
            }
        }
    }

    fn sequence_injection(&self) -> Option<&SequenceInjection> {
        let dx_name = self.dx_path.as_deref().map(file_name).unwrap_or_default();
        let meas = self
            .results
            .as_ref()
            .and_then(|r| r.measurement_id.as_deref());
        self.sequence.as_ref()?.injection(&dx_name, meas)
    }

    /// Trace index → signal index: detector channels first (manifest order), then curves.
    fn trace_signals(&self) -> Vec<usize> {
        let ch = self
            .signals
            .iter()
            .enumerate()
            .filter(|(_, s)| matches!(s.body, PartBody::Channel(_)))
            .map(|(i, _)| i);
        let curves = self
            .signals
            .iter()
            .enumerate()
            .filter(|(_, s)| matches!(s.body, PartBody::Pairs { .. }))
            .map(|(i, _)| i);
        let spectra = self
            .signals
            .iter()
            .enumerate()
            .filter(|(_, s)| matches!(s.body, PartBody::Spectra(_)))
            .map(|(i, _)| i);
        ch.chain(curves).chain(spectra).collect()
    }

    fn trace_of_signal(&self, signal: usize) -> Option<u32> {
        self.trace_signals()
            .iter()
            .position(|&i| i == signal)
            .map(|t| t as u32)
    }

    /// Match every result signal to a container signal and flatten the peaks.
    fn map_results(&mut self) {
        let Some(r) = self.results.clone() else {
            return;
        };
        let mut target: Vec<Option<usize>> = vec![None; r.signals.len()];
        let mut names: Vec<Option<String>> = vec![None; r.signals.len()];
        let with_peaks: Vec<usize> = (0..r.signals.len())
            .filter(|&i| !r.signals[i].peaks.is_empty())
            .collect();
        let mut mapping = SignalMapping::Unmapped;
        if let Some(inj) = self.sequence_injection() {
            for (i, sr) in r.signals.iter().enumerate() {
                if let Some(ss) = inj.signals.iter().find(|s| s.id == sr.signal_id) {
                    names[i] = ss.name.clone();
                    target[i] = ss.trace_id.as_deref().and_then(|t| {
                        self.signals
                            .iter()
                            .position(|s| s.manifest.trace_id.eq_ignore_ascii_case(t))
                    });
                }
            }
            if with_peaks.iter().all(|&i| names[i].is_some()) {
                mapping = SignalMapping::SequenceFile;
            }
        }
        if mapping == SignalMapping::Unmapped && !self.signals.is_empty() {
            // the vendor's baseline at a peak limit is the signal sample there (BB ends)
            let mut all = true;
            for &i in &with_peaks {
                let mut scores: Vec<(usize, usize)> = Vec::new();
                for (k, s) in self.signals.iter().enumerate() {
                    if !matches!(s.body, PartBody::Channel(_)) {
                        continue;
                    }
                    let mut hits = 0usize;
                    for p in &r.signals[i].peaks {
                        for (t, b) in [(p.start_min, p.baseline_start), (p.end_min, p.baseline_end)]
                        {
                            if let (Some(t), Some(b)) = (t, b)
                                && let Some(y) = s.value_near(t)
                                && (y - b).abs() <= 1e-6 * b.abs().max(1.0)
                            {
                                hits += 1;
                            }
                        }
                    }
                    scores.push((hits, k));
                }
                scores.sort_unstable_by(|a, b| b.cmp(a));
                match scores.as_slice() {
                    [(h, k), rest @ ..] if *h > 0 && rest.first().is_none_or(|(h2, _)| h2 < h) => {
                        target[i] = Some(*k);
                    }
                    _ => all = false,
                }
            }
            let distinct = {
                let mut t: Vec<usize> = with_peaks.iter().filter_map(|&i| target[i]).collect();
                t.sort_unstable();
                t.dedup();
                t.len() == with_peaks.len()
            };
            if all && distinct && !with_peaks.is_empty() {
                mapping = SignalMapping::BaselineMatch;
            } else {
                target.fill(None);
                let integrable: Vec<usize> = self
                    .signals
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| matches!(s.body, PartBody::Channel(_)))
                    .map(|(k, _)| k)
                    .collect();
                if with_peaks.len() == 1 && integrable.len() == 1 {
                    target[with_peaks[0]] = Some(integrable[0]);
                    mapping = SignalMapping::SingleSignal;
                }
            }
        }
        let mut rows = Vec::new();
        for (i, sr) in r.signals.iter().enumerate() {
            let name = target[i]
                .map(|k| self.signals[k].manifest.channel.clone())
                .or_else(|| names[i].clone())
                .unwrap_or_else(|| sr.signal_id.clone());
            let trace = target[i].and_then(|k| self.trace_of_signal(k));
            let mut peaks = sr.peaks.clone();
            peaks.sort_by(|a, b| a.rt_min.total_cmp(&b.rt_min));
            for p in peaks {
                rows.push(PeakRow {
                    result_signal: i,
                    signal: name.clone(),
                    trace,
                    peak: p,
                });
            }
        }
        rows.sort_by_key(|r| r.trace.unwrap_or(u32::MAX));
        self.mapping = mapping;
        self.rows = rows;
    }

    /// Every decoded signal (for library users and tests).
    pub fn signals(&self) -> &[DxSignal] {
        &self.signals
    }

    /// The manifest, when a `.dx` was read.
    pub fn manifest(&self) -> Option<&InjectionManifest> {
        self.manifest.as_ref()
    }

    /// The vendor results, when a `.rx` was read.
    pub fn results(&self) -> Option<&InjectionResults> {
        self.results.as_ref()
    }

    /// The vendor peaks with their signals.
    pub fn peak_rows(&self) -> &[PeakRow] {
        &self.rows
    }

    /// How result signals were matched.
    pub fn mapping(&self) -> SignalMapping {
        self.mapping
    }

    /// The instrument module that recorded a signal (sequence file).
    fn module_of(&self, s: &DxSignal) -> Option<&crate::openlab_xml::InstrumentModule> {
        let seq = self.sequence.as_ref()?;
        let inj = self.sequence_injection()?;
        let ss = inj.signals.iter().find(|g| {
            g.trace_id
                .as_deref()
                .is_some_and(|t| t.eq_ignore_ascii_case(&s.manifest.trace_id))
        })?;
        let id = ss.module_id.as_deref()?;
        seq.modules.iter().find(|m| m.id == id)
    }

    fn separation(&self) -> Option<&'static str> {
        if let Some(t) = self.sequence.as_ref().and_then(|s| s.technique.as_deref()) {
            let t = t.to_ascii_lowercase();
            if t.starts_with("gas") {
                return Some("GC");
            }
            if t.starts_with("liquid") {
                return Some("LC");
            }
        }
        let src = self
            .manifest
            .as_ref()
            .and_then(|m| m.injection_source.as_deref())?;
        src.to_ascii_uppercase().starts_with("GC ").then_some("GC")
    }

    fn signal_extra(&self, s: &DxSignal) -> BTreeMap<String, Value> {
        let mut m = BTreeMap::new();
        let mut put = |k: &str, v: Option<Value>| {
            if let Some(v) = v {
                m.insert(k.to_string(), v);
            }
        };
        let ms = &s.manifest;
        put("file", self.dx_path.as_deref().map(|p| json!(file_name(p))));
        put("part", s.part.clone().map(Value::from));
        put("trace_id", Some(json!(ms.trace_id)));
        put("encoding", Some(json!(ms.kind())));
        put(
            "format_version",
            s.header.as_ref().map(|h| json!(h.version)),
        );
        put("channel", Some(json!(ms.channel)));
        put("signal", ms.description.clone().map(Value::from));
        put("detector", ms.device.clone().map(Value::from));
        put("device_number", ms.device_number.map(Value::from));
        put("integrable", ms.integrable.map(Value::from));
        put("declared_values", ms.declared_values.map(Value::from));
        if let Some(man) = &self.manifest {
            put("sample_name", man.sample_name.clone().map(Value::from));
            put("operator", man.operator.clone().map(Value::from));
            put("acquired_at", man.run_started.clone().map(Value::from));
            put(
                "method",
                man.acquisition_method
                    .as_deref()
                    .map(|p| json!(basename(p))),
            );
            put(
                "method_path",
                man.acquisition_method.clone().map(Value::from),
            );
            put("vial", man.location.clone().map(Value::from));
            put(
                "injection_volume",
                man.injection_volume
                    .and_then(|v| volume_ul(v, man.injection_volume_unit.as_deref()))
                    .map(|v| json!(tidy(v))),
            );
            put(
                "injection_source",
                man.injection_source.clone().map(Value::from),
            );
            put("sequence_line", man.sequence_line.map(Value::from));
            put("replicate", man.replicate.map(Value::from));
            put("barcode", man.barcode.clone().map(Value::from));
        }
        put("separation", self.separation().map(Value::from));
        put("software", Some(json!("OpenLab CDS")));
        put(
            "software_version",
            self.sequence
                .as_ref()
                .and_then(|q| q.acquisition_software.clone())
                .map(Value::from),
        );
        if let Some(seq) = &self.sequence {
            put(
                "instrument_name",
                seq.instrument_name.clone().map(Value::from),
            );
        }
        if let Some(md) = self.module_of(s) {
            put("instrument", md.part_number.clone().map(Value::from));
            put(
                "instrument_serial",
                md.serial_number.clone().map(Value::from),
            );
            put("module", md.name.clone().map(Value::from));
            put("module_firmware", md.firmware.clone().map(Value::from));
        }
        if let Some(h) = &s.header {
            let (sig, reference) = h.wavelengths();
            if let Some([w, bw]) = sig {
                put("wavelength_nm", Some(json!(w)));
                put("bandwidth_nm", bw.is_finite().then(|| json!(bw)));
            }
            if let Some([w, bw]) = reference {
                put("reference_wavelength_nm", Some(json!(w)));
                put("reference_bandwidth_nm", bw.is_finite().then(|| json!(bw)));
            }
        }
        m
    }

    fn trace_info(&self, index: u32, si: usize) -> TraceInfo {
        let s = &self.signals[si];
        let mut extra = self.signal_extra(s);
        if let PartBody::Spectra(k) = s.body {
            // the ChemStation reader's trace of the `.uv` part, under our name and index
            let inner = self.spectra[k]
                .info()
                .ok()
                .and_then(|i| i.traces.into_iter().next())
                .unwrap_or_default();
            for (k2, v) in inner.extra {
                extra.entry(k2).or_insert(v);
            }
            extra.insert("spectra".into(), json!(true));
            return TraceInfo {
                index,
                name: Some(
                    s.manifest
                        .description
                        .clone()
                        .unwrap_or_else(|| s.manifest.channel.clone()),
                ),
                extra,
                ..inner
            };
        }
        let n = s.count();
        let name = s
            .manifest
            .description
            .clone()
            .unwrap_or_else(|| s.manifest.channel.clone());
        let vendor = self.rows.iter().filter(|r| r.trace == Some(index)).count();
        if vendor > 0 {
            extra.insert("vendor_peak_count".into(), json!(vendor));
        }
        let channel = SignalChannelInfo {
            index: 0,
            name: s.manifest.channel.clone(),
            unit: s.unit(),
            dtype: "float64".into(),
            scale: s.scale(),
            offset: s.offset(),
            extra: BTreeMap::new(),
        };
        let (grid, quantity) = match &s.body {
            PartBody::Channel(_) => (s.channel_step_ms(), "retention_time"),
            _ => (s.curve_step_ms(), "time"),
        };
        let mut channels = vec![channel];
        let (rate, start_s) = match grid {
            Some((a, step)) => {
                extra.insert("x_start_min".into(), json!(tidy(a / 60_000.0)));
                extra.insert(
                    "x_end_min".into(),
                    json!(tidy((a + step * n.saturating_sub(1) as f64) / 60_000.0)),
                );
                extra.insert(
                    "axis".into(),
                    json!({"quantity": quantity, "unit": "min",
                           "first": tidy(a / 60_000.0), "step": tidy(step / 60_000.0)}),
                );
                (tidy(1000.0 / step), Some(tidy(a / 1000.0)))
            }
            None => {
                if let PartBody::Pairs { times_ms, .. } = &s.body
                    && !times_ms.is_empty()
                {
                    // uneven times: a time channel in minutes
                    extra.insert("irregular_times".into(), json!(true));
                    extra.insert("x_start_min".into(), json!(tidy(times_ms[0] / 60_000.0)));
                    extra.insert(
                        "x_end_min".into(),
                        json!(tidy(times_ms[times_ms.len() - 1] / 60_000.0)),
                    );
                    channels.push(SignalChannelInfo {
                        index: 1,
                        name: "time".into(),
                        unit: Some("min".into()),
                        dtype: "float64".into(),
                        scale: 1.0,
                        offset: 0.0,
                        extra: BTreeMap::new(),
                    });
                    (0.0, Some(tidy(times_ms[0] / 1000.0)))
                } else {
                    (0.0, None)
                }
            }
        };
        TraceInfo {
            index,
            name: Some(name),
            sample_rate_hz: rate,
            sample_count: n,
            sweep_count: 1,
            channels,
            start_s,
            extra,
        }
    }

    fn table_columns(&self) -> Vec<ColumnInfo> {
        let cats = |f: &dyn Fn(&PeakRow) -> String| -> Vec<String> {
            let mut v: Vec<String> = Vec::new();
            for r in &self.rows {
                let s = f(r);
                if !v.contains(&s) {
                    v.push(s);
                }
            }
            v
        };
        let signals = cats(&|r| r.signal.clone());
        let codes = cats(&|r| r.peak.baseline_code.clone().unwrap_or_default());
        let types = cats(&|r| r.peak.peak_type.clone().unwrap_or_default());
        let compounds = cats(&|r| r.peak.compound.clone().unwrap_or_default());
        let area_unit = self.rows.iter().find_map(|r| r.peak.area_unit.clone());
        let height_unit = self.rows.iter().find_map(|r| r.peak.height_unit.clone());
        let col = |index: u32,
                   name: &str,
                   dtype: &str,
                   unit: Option<String>,
                   label: &str,
                   categories: Option<&Vec<String>>| {
            let mut extra = BTreeMap::new();
            if let Some(c) = categories {
                extra.insert("categories".to_string(), json!(c));
            }
            ColumnInfo {
                index,
                name: name.into(),
                label: Some(label.into()),
                dtype: dtype.into(),
                unit,
                range: None,
                extra,
            }
        };
        let min = || Some("min".to_string());
        vec![
            col(
                0,
                "signal",
                "uint32",
                None,
                "signal the peak was integrated on (code into extra.categories)",
                Some(&signals),
            ),
            col(1, "rt_min", "float64", min(), "retention time", None),
            col(
                2,
                "start_min",
                "float64",
                min(),
                "start of the integration",
                None,
            ),
            col(
                3,
                "end_min",
                "float64",
                min(),
                "end of the integration",
                None,
            ),
            col(
                4,
                "area",
                "float64",
                area_unit,
                "peak area as the vendor software reports it",
                None,
            ),
            col(
                5,
                "height",
                "float64",
                height_unit.clone(),
                "peak height above the baseline",
                None,
            ),
            col(
                6,
                "area_percent",
                "float64",
                Some("%".into()),
                "area percent (the vendor's normalization)",
                None,
            ),
            col(
                7,
                "height_percent",
                "float64",
                Some("%".into()),
                "height percent",
                None,
            ),
            col(
                8,
                "width_base_min",
                "float64",
                min(),
                "peak width at the base (end − start)",
                None,
            ),
            col(
                9,
                "symmetry",
                "float64",
                None,
                "peak symmetry as the vendor defines it",
                None,
            ),
            col(
                10,
                "baseline_start",
                "float64",
                height_unit.clone(),
                "baseline value at the start",
                None,
            ),
            col(
                11,
                "baseline_end",
                "float64",
                height_unit.clone(),
                "baseline value at the end",
                None,
            ),
            col(
                12,
                "baseline_at_rt",
                "float64",
                height_unit,
                "baseline value at the retention time",
                None,
            ),
            col(
                13,
                "baseline_code",
                "uint32",
                None,
                "baseline code (BB, BV, VB, ...; code into extra.categories)",
                Some(&codes),
            ),
            col(
                14,
                "peak_type",
                "uint32",
                None,
                "peak type (code into extra.categories)",
                Some(&types),
            ),
            col(
                15,
                "compound",
                "uint32",
                None,
                "compound name, empty when unidentified (code into extra.categories)",
                Some(&compounds),
            ),
            col(
                16,
                "trace",
                "float64",
                None,
                "index of the trace (info → traces[]) the peak belongs to; NaN when not matched",
                None,
            ),
        ]
    }

    fn table_info(&self) -> Option<TableInfo> {
        let r = self.results.as_ref()?;
        let mut extra = BTreeMap::new();
        extra.insert(
            "source".into(),
            json!("calculated by the vendor software (OpenLab CDS data analysis) and stored in the .rx result package; not computed by OpenReadout"),
        );
        if let Some(p) = &self.results_path {
            extra.insert("results_file".into(), json!(file_name(p)));
        }
        let mut put = |k: &str, v: &Option<String>| {
            if let Some(v) = v {
                extra.insert(k.into(), json!(v));
            }
        };
        put("software", &r.software);
        put("processing_method", &r.processing_method);
        put("processing_method_version", &r.processing_method_version);
        put("quantitation", &r.quantitation);
        put("integrator", &r.integrator);
        put("processing_state", &r.processing_state);
        put("processed_at", &r.processed_at);
        extra.insert("signal_mapping".into(), json!(self.mapping.name()));
        let mut by_signal: BTreeMap<String, usize> = BTreeMap::new();
        for row in &self.rows {
            *by_signal.entry(row.signal.clone()).or_default() += 1;
        }
        extra.insert("peaks_by_signal".into(), json!(by_signal));
        Some(TableInfo {
            index: 0,
            name: Some("vendor_peaks".into()),
            row_count: self.rows.len() as u64,
            columns: self.table_columns(),
            extra,
        })
    }
}

fn code(categories: &[String], v: &str) -> f64 {
    categories
        .iter()
        .position(|c| c == v)
        .map_or(f64::NAN, |i| i as f64)
}

impl Dataset for OpenLabDataset {
    fn info(&self) -> Result<FileInfo> {
        let traces: Vec<TraceInfo> = self
            .trace_signals()
            .iter()
            .enumerate()
            .map(|(t, &i)| self.trace_info(t as u32, i))
            .collect();
        let mut notes = self.notes.clone();
        if !traces.is_empty() {
            notes.push("trace values are scaled: value = raw × scale, with the scale factor from each part's header".into());
        }
        if self.results.is_some() {
            notes.push(format!(
                "tables[0] (vendor_peaks) holds the vendor's own integration results ({} peaks, signals matched by {})",
                self.rows.len(),
                self.mapping.name()
            ));
        }
        if let Some(e) = &self.results_error {
            notes.push(format!("the result package was not read: {e}"));
        }
        if self.dx_path.is_none() {
            notes.push("no injection container (.dx) with the same name: results only".into());
        }
        let version = self
            .signals
            .iter()
            .find_map(|s| s.header.as_ref().map(|h| h.version.clone()));
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.dx_size + self.results_size,
            format: OpenLabReader.descriptor(),
            format_version: version,
            images: Vec::new(),
            tables: self.table_info().into_iter().collect(),
            spectra: Vec::new(),
            traces,
            plane_count: 0,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let mut headers = serde_json::Map::new();
        for s in &self.signals {
            if let (Some(p), Some(h)) = (&s.part, &s.header) {
                headers.insert(p.clone(), h.to_json());
            }
        }
        let parts: Vec<Value> = self
            .parts
            .iter()
            .map(|p| json!({"name": p.name, "size": p.size, "compressed_size": p.compressed_size, "method": p.method}))
            .collect();
        let seq = self.sequence.as_ref().map(|s| {
            json!({
                "file": self.sequence_path.as_deref().map(file_name),
                "instrument_name": s.instrument_name,
                "technique": s.technique,
                "acquisition_software": s.acquisition_software,
                "modules": s.modules.iter().map(|m| json!({
                    "name": m.name, "manufacturer": m.manufacturer, "type": m.kind,
                    "part_number": m.part_number, "serial_number": m.serial_number, "firmware": m.firmware,
                })).collect::<Vec<_>>(),
                "injections": s.injections.len(),
            })
        });
        Ok(json!({
            "content_types": self.content_types.as_deref().and_then(xml_to_json),
            "parts": parts,
            "manifest": self.manifest_xml.as_deref().and_then(xml_to_json),
            "part_headers": headers,
            "results_file": self.results_path.as_deref().map(file_name),
            "results": self.results_xml.as_deref().and_then(xml_to_json),
            "sequence_file": seq,
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
            ("traces[].extra.sample_name", Source::Inferred),
            ("traces[].extra.operator", Source::Inferred),
            ("traces[].extra.acquired_at", Source::Inferred),
            ("traces[].extra.method", Source::Inferred),
            ("traces[].extra.vial", Source::Inferred),
            ("traces[].extra.injection_volume", Source::Inferred),
            ("traces[].extra.signal", Source::Inferred),
            ("traces[].extra.instrument", Source::PriorArt),
            ("traces[].extra.instrument_serial", Source::PriorArt),
            ("traces[].extra.separation", Source::Inferred),
            ("traces[].extra.wavelength_nm", Source::Inferred),
            ("tables[].columns", Source::VendorImpl),
            ("tables[].extra.signal_mapping", Source::Inferred),
        ] {
            p.insert(k.into(), s);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let mut out = Vec::new();
        for part in &self.parts {
            let sig = self
                .signals
                .iter()
                .find(|s| s.part.as_deref() == Some(part.name.as_str()));
            let (kind, details) = match sig {
                Some(s) => (
                    match s.body {
                        PartBody::Channel(_) => "signal",
                        PartBody::Pairs { .. } => "instrument-curve",
                        _ => "part",
                    },
                    json!({"channel": s.manifest.channel, "content_type": s.manifest.encoding,
                           "values": s.count(), "declared_values": s.manifest.declared_values,
                           "unit": s.unit(), "compression": part.method}),
                ),
                None if part.name.eq_ignore_ascii_case(MANIFEST_MEMBER) => {
                    ("manifest", json!({"compression": part.method}))
                }
                None => ("part", json!({"compression": part.method})),
            };
            out.push(LsEntry {
                kind: kind.into(),
                name: part.name.clone(),
                offset: None,
                size: Some(part.size),
                image: None,
                details,
            });
        }
        for s in &self.signals {
            if matches!(s.body, PartBody::Missing) {
                out.push(LsEntry {
                    kind: "missing-part".into(),
                    name: format!("{} ({})", s.manifest.trace_id, s.manifest.channel),
                    offset: None,
                    size: None,
                    image: None,
                    details: json!({"content_type": s.manifest.encoding}),
                });
            }
        }
        if let Some(p) = &self.results_path {
            out.push(LsEntry {
                kind: "results".into(),
                name: file_name(p),
                offset: None,
                size: Some(self.results_size),
                image: None,
                details: json!({"peaks": self.rows.len(), "signal_mapping": self.mapping.name()}),
            });
        }
        if let Some(p) = &self.sequence_path {
            out.push(LsEntry {
                kind: "sequence".into(),
                name: file_name(p),
                offset: None,
                size: self.fs.metadata(p).ok().map(|m| m.len()),
                image: None,
                details: json!({"injections": self.sequence.as_ref().map_or(0, |s| s.injections.len())}),
            });
        }
        Ok(out)
    }

    fn member_files(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = Vec::new();
        for p in [&self.dx_path, &self.results_path, &self.sequence_path]
            .into_iter()
            .flatten()
        {
            if *p != self.path {
                v.push(p.clone());
            }
        }
        v
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::unsupported(
            OPENLAB_ID,
            "image planes",
            "OpenLab CDS injections hold chromatograms and instrument curves: use `openreadout analyze chromatogram --trace N`, `analyze peaks`, or `export --to csv`.",
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
                "sweep {sweep} out of range (OpenLab CDS traces have one sweep)"
            )));
        }
        let map = self.trace_signals();
        let &si = map.get(index as usize).ok_or_else(|| {
            Error::Usage(format!(
                "trace {index} out of range (this injection has {} traces)",
                map.len()
            ))
        })?;
        let sig = &self.signals[si];
        let total = sig.count();
        if first_sample > total {
            return Err(Error::Usage(format!(
                "first sample {first_sample} is past the end ({total} samples)"
            )));
        }
        let n = max_samples.min(total - first_sample);
        let (start, end) = (first_sample as usize, (first_sample + n) as usize);
        if let PartBody::Spectra(k) = sig.body {
            let mut trace = self.spectra[k].read_trace(0, 0, first_sample, max_samples)?;
            trace.trace = index;
            return Ok(trace);
        }
        let (scale, offset) = (sig.scale(), sig.offset());
        let channels = match &sig.body {
            PartBody::Channel(d) => {
                vec![
                    d.values[start..end]
                        .iter()
                        .map(|v| v * scale + offset)
                        .collect(),
                ]
            }
            PartBody::Pairs { times_ms, raw, .. } => {
                let values: Vec<f64> = raw[start..end].iter().map(|v| v * scale + offset).collect();
                if sig.curve_step_ms().is_some() {
                    vec![values]
                } else {
                    vec![
                        values,
                        times_ms[start..end].iter().map(|t| t / 60_000.0).collect(),
                    ]
                }
            }
            _ => Vec::new(),
        };
        Ok(Trace {
            trace: index,
            sweep: 0,
            first_sample,
            channels,
        })
    }

    fn read_table(&mut self, index: u32, first_row: u64, max_rows: u64) -> Result<Table> {
        let Some(info) = self.table_info().filter(|_| index == 0) else {
            return Err(Error::Usage(format!(
                "table {index} not found (this injection has {} tables)",
                usize::from(self.results.is_some())
            )));
        };
        let cats: Vec<Vec<String>> = info
            .columns
            .iter()
            .map(|c| {
                c.extra
                    .get("categories")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .map(|x| x.as_str().unwrap_or_default().to_string())
                            .collect()
                    })
                    .unwrap_or_default()
            })
            .collect();
        let start = (first_row as usize).min(self.rows.len());
        let end = start
            .saturating_add(max_rows.min(usize::MAX as u64) as usize)
            .min(self.rows.len());
        let o = |v: Option<f64>| v.unwrap_or(f64::NAN);
        let mut columns: Vec<Vec<f64>> = vec![Vec::with_capacity(end - start); info.columns.len()];
        for r in &self.rows[start..end] {
            let p = &r.peak;
            let vals = [
                code(&cats[0], &r.signal),
                p.rt_min,
                o(p.start_min),
                o(p.end_min),
                o(p.area),
                o(p.height),
                o(p.area_percent),
                o(p.height_percent),
                o(p.width_base_min),
                o(p.symmetry),
                o(p.baseline_start),
                o(p.baseline_end),
                o(p.baseline_at_apex),
                code(&cats[13], p.baseline_code.as_deref().unwrap_or_default()),
                code(&cats[14], p.peak_type.as_deref().unwrap_or_default()),
                code(&cats[15], p.compound.as_deref().unwrap_or_default()),
                r.trace.map_or(f64::NAN, f64::from),
            ];
            for (c, v) in columns.iter_mut().zip(vals) {
                c.push(v);
            }
        }
        Ok(Table {
            table: 0,
            first_row: start as u64,
            columns,
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), OPENLAB_ID);
        r.performed("container: zip central directory; every member inflated and checked against its CRC-32");
        r.performed("manifest (injection.acmd) parsed; every listed signal matched to its part");
        r.performed("signal parts: header, body decoded to the end, value count against the manifest, scale against the manifest's slope");
        r.performed("instrument curves: whole 16-byte samples, times increasing");
        // spectra parts: the ChemStation reader's check of each `.uv` part
        for k in 0..self.spectra.len() {
            let name = self
                .signals
                .iter()
                .find(|s| matches!(s.body, PartBody::Spectra(j) if j == k))
                .map_or_else(String::new, |s| s.manifest.channel.clone());
            let rep = self.spectra[k].check()?;
            for f in rep.findings {
                r.push(Finding {
                    message: format!("spectra {name}: {}", f.message),
                    ..f
                });
            }
            let records = self.spectra[k]
                .info()
                .ok()
                .and_then(|i| i.traces.first().map(|t| t.sample_count));
            let declared = self
                .signals
                .iter()
                .find(|s| matches!(s.body, PartBody::Spectra(j) if j == k))
                .and_then(|s| s.manifest.declared_records);
            if let (Some(n), Some(d)) = (records, declared)
                && n != d
            {
                r.push(Finding::warning(
                    "record_count_mismatch",
                    format!(
                        "spectra {name}: the part holds {n} spectra, the manifest declares {d}"
                    ),
                ));
            }
        }
        if !self.spectra.is_empty() {
            r.performed(format!(
                "{} spectra part(s): every record decoded (ChemStation .uv layout, tag 67 or 70) and counted against the manifest's NumberOfRecords",
                self.spectra.len()
            ));
        }
        r.performed("result package (.rx): results document parsed, peaks matched to signals, peak limits inside the signal's time range");
        if let Some(dx) = self.dx_path.clone() {
            let z = ZipIndex::open(&self.fs, &dx, OPENLAB_ID)?;
            for m in z.members.clone() {
                if let Err(e) = z.read(&m) {
                    r.push(Finding::error("bad_member", format!("{}: {e}", m.name)));
                }
            }
        }
        for s in &self.signals {
            let name = &s.manifest.channel;
            match &s.body {
                PartBody::Missing => r.push(Finding::warning(
                    "missing_part",
                    format!("{name}: the manifest lists trace {} but the container has no part for it", s.manifest.trace_id),
                )),
                PartBody::NotDecoded => r.push(Finding::info(
                    "not_decoded",
                    format!("{name}: {} parts are listed, not decoded", s.manifest.kind()),
                )),
                PartBody::Spectra(_) => {}
                PartBody::Channel(d) => match &d.end {
                    BodyEnd::Truncated { at } => r.push(Finding::error(
                        "truncated",
                        format!("{name}: body ends inside a value ({} values decoded) at body byte {at}", d.values.len()),
                    )),
                    BodyEnd::BadRecord { at, detail } => r.push(Finding::error(
                        "bad_record",
                        format!("{name}: {detail} at body byte {at}"),
                    )),
                    _ => {}
                },
                PartBody::Pairs { times_ms, end, .. } => {
                    if let BodyEnd::Truncated { at } = end {
                        r.push(Finding::error(
                            "truncated",
                            format!("{name}: body ends inside a sample at body byte {at}"),
                        ));
                    }
                    if times_ms.windows(2).any(|w| w[1] <= w[0]) {
                        r.push(Finding::warning(
                            "time_not_increasing",
                            format!("{name}: sample times do not increase"),
                        ));
                    }
                }
            }
            if matches!(s.body, PartBody::Channel(_) | PartBody::Pairs { .. }) {
                if let Some(d) = s.manifest.declared_values
                    && d != s.count()
                {
                    r.push(Finding::warning(
                        "value_count_mismatch",
                        format!(
                            "{name}: the part holds {} values, the manifest declares {d}",
                            s.count()
                        ),
                    ));
                }
                if s.count() == 0 {
                    r.push(Finding::warning(
                        "no_samples",
                        format!("{name}: no samples"),
                    ));
                }
                if let (Some(slope), Some(h)) = (s.manifest.slope, &s.header)
                    && (slope - h.scale).abs() > 1e-9 * slope.abs().max(1e-300)
                {
                    r.push(Finding::warning(
                        "scale_mismatch",
                        format!(
                            "{name}: header scale {} differs from the manifest slope {slope}",
                            h.scale
                        ),
                    ));
                }
            }
        }
        if let Some(e) = &self.results_error {
            r.push(Finding::error("bad_results", e.clone()));
        }
        if self.results.is_some() {
            if self.mapping == SignalMapping::Unmapped && !self.rows.is_empty() {
                r.push(Finding::info(
                    "results_unmapped",
                    "the vendor peaks could not be matched to a signal of the container"
                        .to_string(),
                ));
            }
            for row in &self.rows {
                let Some(t) = row.trace else { continue };
                let Some(&si) = self.trace_signals().get(t as usize) else {
                    continue;
                };
                let s = &self.signals[si];
                let range = s
                    .channel_step_ms()
                    .map(|(a, step)| (a, a + step * s.count().saturating_sub(1) as f64));
                if let Some((a, b)) = range {
                    let inside = |m: Option<f64>| {
                        m.is_none_or(|m| {
                            let ms = m * 60_000.0;
                            ms >= a - 1.0 && ms <= b + 1.0
                        })
                    };
                    if !inside(row.peak.start_min) || !inside(row.peak.end_min) {
                        r.push(Finding::warning(
                            "peak_outside_signal",
                            format!(
                                "{}: vendor peak at {:.3} min has limits outside the signal's {:.3}–{:.3} min",
                                row.signal,
                                row.peak.rt_min,
                                a / 60_000.0,
                                b / 60_000.0
                            ),
                        ));
                    }
                }
            }
        }
        Ok(r)
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;
    use openreadout_core::zip::zip_bytes;

    /// A version-179 part: 0x1800 header with the ChemStation fields the reader uses, then `body`.
    pub(crate) fn part179(
        signal: &str,
        units: &str,
        scale: f64,
        t0: f32,
        t1: f32,
        body: &[u8],
    ) -> Vec<u8> {
        let mut b = vec![0u8; 0x1800];
        b[..4].copy_from_slice(b"\x03179");
        b[0x108..0x10C].copy_from_slice(&13u32.to_be_bytes());
        b[0x11A..0x11E].copy_from_slice(&t0.to_be_bytes());
        b[0x11E..0x122].copy_from_slice(&t1.to_be_bytes());
        // UTF-16 version at 0x146 (detection of versions >= 100)
        b[0x146] = 3;
        for (i, c) in "179".encode_utf16().enumerate() {
            b[0x147 + 2 * i..0x149 + 2 * i].copy_from_slice(&c.to_le_bytes());
        }
        let mut put16 = |off: usize, s: &str| {
            let u: Vec<u16> = s.encode_utf16().collect();
            b[off] = u.len() as u8;
            for (i, c) in u.iter().enumerate() {
                b[off + 1 + 2 * i..off + 3 + 2 * i].copy_from_slice(&c.to_le_bytes());
            }
        };
        put16(0x15B, "OL DATA FILE");
        put16(0x104C, units);
        put16(0x1075, signal);
        b[0x127C..0x1284].copy_from_slice(&scale.to_be_bytes());
        b.extend_from_slice(body);
        b
    }

    pub(crate) fn acmd(signals: &[(&str, &str, &str, &str, u64)]) -> String {
        let mut s = String::from(
            "\u{feff}<?xml version=\"1.0\" encoding=\"utf-8\"?><ACMD xmlns=\"urn:schemas-agilent-com:acmd20\"><InjectionInfo><Location>7</Location><InjectionVolume>2</InjectionVolume><InjectionVolumeUnits>µL</InjectionVolumeUnits><SampleName>S</SampleName><RunOperator>op</RunOperator><RunDateTime>2023-01-02T03:04:05+01:00</RunDateTime><AcquisitionMethod>D:\\m\\gc.amx</AcquisitionMethod><InjectionSource>GC Injector</InjectionSource><Signals>",
        );
        for (enc, id, ch, unit, n) in signals {
            s.push_str(&format!("<Signal><Encoding>Agilent.OpenLab.Rawdata/{enc}</Encoding><TraceId>{id}</TraceId><DeviceName>FID</DeviceName><ChannelName>{ch}</ChannelName><Description>{ch}</Description><Units>{unit}</Units><NumberOfValues>{n}</NumberOfValues><Slope>0.5</Slope></Signal>"));
        }
        s.push_str("</Signals></InjectionInfo></ACMD>");
        s
    }

    fn f64s(v: &[f64]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    fn rx(peaks: &str) -> Vec<u8> {
        let xml = format!(
            "<ACAML xmlns=\"urn:schemas-agilent-com:acaml21\"><Doc><Content><Injections><Result><SignalResult><Signal_ID id=\"x\"/></SignalResult><SignalResult><Signal_ID id=\"y\"/>{peaks}</SignalResult><Integrator>T</Integrator></Result></Injections></Content></Doc></ACAML>"
        );
        zip_bytes(&[("Base/InjectionACAML", xml.as_bytes())]).unwrap()
    }

    fn fixture(dir: &Path) -> PathBuf {
        // FID2B: 10 values 1..10 (raw × 0.5), 1000 ms apart from 0; FID1A: flat 3; a pressure curve
        let a: Vec<f64> = (1..=10).map(f64::from).collect();
        let ch_a = part179("FID2B", "pA", 0.5, 0.0, 9000.0, &f64s(&a));
        let ch_b = part179("FID1A", "pA", 0.5, 0.0, 9000.0, &f64s(&[6.0; 10]));
        let pairs: Vec<f64> = (0..5)
            .flat_map(|i| [60.0 * f64::from(i) + 32.0, 1000.0])
            .collect();
        let it = part179("PMP1A,Pressure", "bar", 0.5, 0.0, 5000.0, &f64s(&pairs));
        let man = acmd(&[
            ("Signal179", "aaaa", "FID2B", "pA", 10),
            ("Signal179", "bbbb", "FID1A", "pA", 10),
            ("InstrumentTrace179", "cccc", "PMP1A", "bar", 5),
            ("InstrumentTrace179", "dddd", "PMP1B", "mL/min", 5),
        ]);
        let dx = dir.join("inj.dx");
        std::fs::write(
            &dx,
            zip_bytes(&[
                ("aaaa.CH", &ch_a),
                ("bbbb.CH", &ch_b),
                ("cccc.IT", &it),
                ("injection.acmd", man.as_bytes()),
            ])
            .unwrap(),
        )
        .unwrap();
        // one peak on FID2B from 2 s to 5 s: baseline = the samples there (1.5, 3.0)
        std::fs::write(
            dir.join("inj.rx"),
            rx("<Peak id=\"p\"><RetentionTime val=\"0.06\" unit=\"min\"/><Area val=\"3\" unit=\"pA·s\"/><BeginTime val=\"0.0333333333\" unit=\"min\"/><EndTime val=\"0.0833333333\" unit=\"min\"/><BaselineStart val=\"1.5\"/><BaselineEnd val=\"3\"/><BaselineCode>BB</BaselineCode></Peak>"),
        )
        .unwrap();
        dx
    }

    #[test]
    fn open_decode_map() {
        let dir = tempfile::tempdir().unwrap();
        let dx = fixture(dir.path());
        let mut ds = OpenLabDataset::open(&dx).unwrap();
        let info = ds.info().unwrap();
        assert_eq!(info.traces.len(), 3);
        assert_eq!(info.traces[0].channels[0].name, "FID2B");
        assert_eq!(info.traces[0].sample_rate_hz, 1.0);
        assert_eq!(info.traces[2].channels[0].name, "PMP1A");
        assert_eq!(info.traces[2].sample_rate_hz, tidy(1000.0 / 60.0));
        assert_eq!(info.traces[2].start_s, Some(0.032));
        assert_eq!(info.traces[0].extra["vial"], json!("7"));
        assert_eq!(info.traces[0].extra["injection_volume"], json!(2.0));
        assert_eq!(info.traces[0].extra["method"], json!("gc.amx"));
        assert_eq!(info.traces[0].extra["separation"], json!("GC"));
        assert_eq!(info.traces[0].extra["vendor_peak_count"], json!(1));
        assert!(info.notes.iter().any(|n| n.contains("PMP1B")));
        let t = ds.read_trace(0, 0, 2, 3).unwrap();
        assert_eq!(t.channels[0], vec![1.5, 2.0, 2.5]);
        let p = ds.read_trace(2, 0, 0, 10).unwrap();
        assert_eq!(p.channels, vec![vec![500.0; 5]]);
        assert_eq!(ds.mapping(), SignalMapping::BaselineMatch);
        assert_eq!(info.tables[0].row_count, 1);
        let tab = ds.read_table(0, 0, 10).unwrap();
        assert_eq!(tab.columns[0], vec![0.0]);
        assert_eq!(tab.columns[1], vec![0.06]);
        assert_eq!(tab.columns[16], vec![0.0]);
        assert!(ds.read_table(1, 0, 1).is_err());
        assert!(ds.read_trace(9, 0, 0, 1).is_err());
        let c = ds.check().unwrap();
        let codes: Vec<&str> = c.findings.iter().map(|f| f.code.as_str()).collect();
        assert!(codes.contains(&"missing_part"), "{codes:?}");
        assert!(!codes.contains(&"value_count_mismatch"), "{codes:?}");
        // the .rx alone, next to its .dx, opens the same injection
        let ds2 = OpenLabDataset::open(&dir.path().join("inj.rx")).unwrap();
        assert_eq!(ds2.info().unwrap().traces.len(), 3);
        // without its .dx: results only, unmapped
        std::fs::remove_file(&dx).unwrap();
        let mut ds3 = OpenLabDataset::open(&dir.path().join("inj.rx")).unwrap();
        let i3 = ds3.info().unwrap();
        assert!(i3.traces.is_empty());
        assert_eq!(ds3.mapping(), SignalMapping::Unmapped);
        assert!(ds3.read_table(0, 0, 5).unwrap().columns[16][0].is_nan());
    }

    #[test]
    fn malformed_containers_are_clean_errors() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.dx");
        std::fs::write(&p, zip_bytes(&[("other.xml", b"<a/>")]).unwrap()).unwrap();
        assert_eq!(OpenLabDataset::open(&p).unwrap_err().exit_code(), 4);
        std::fs::write(
            &p,
            zip_bytes(&[("injection.acmd", b"<ACMD><Injection")]).unwrap(),
        )
        .unwrap();
        assert_eq!(OpenLabDataset::open(&p).unwrap_err().exit_code(), 4);
        std::fs::write(&p, b"PK\x03\x04 truncated").unwrap();
        assert_eq!(OpenLabDataset::open(&p).unwrap_err().exit_code(), 4);
        // a part cut inside a value: decoded to the last whole value, reported by check
        let man = acmd(&[
            ("Signal179", "aaaa", "FID1A", "pA", 3),
            ("InstrumentTrace179", "cccc", "P", "bar", 2),
        ]);
        let ch = part179("FID1A", "pA", 1.0, 0.0, 2.0, &f64s(&[1.0, 2.0, 3.0])[..20]);
        let it = part179("P", "bar", 1.0, 0.0, 2.0, &f64s(&[1.0, 2.0, 3.0]));
        std::fs::write(
            &p,
            zip_bytes(&[
                ("aaaa.CH", &ch),
                ("cccc.IT", &it),
                ("injection.acmd", man.as_bytes()),
            ])
            .unwrap(),
        )
        .unwrap();
        let mut ds = OpenLabDataset::open(&p).unwrap();
        assert_eq!(ds.signals()[0].count(), 2);
        let c = ds.check().unwrap();
        assert_eq!(
            c.findings.iter().filter(|f| f.code == "truncated").count(),
            2
        );
        // a folder: unsupported with a hint naming its injections
        std::fs::write(dir.path().join("a.dx"), b"").unwrap();
        let e = OpenLabDataset::open(dir.path()).unwrap_err();
        assert_eq!(e.exit_code(), 6);
        assert!(e.hint().unwrap().contains("a.dx"));
    }

    #[test]
    fn detection() {
        use openreadout_core::FormatReader;
        use openreadout_core::model::DetectConfidence;
        let opc = zip_bytes(&[
            ("[Content_Types].xml", b"<Types/>"),
            ("xl/workbook.xml", b"<w/>"),
        ])
        .unwrap();
        let dx = zip_bytes(&[("injection.acmd", b"<ACMD/>")]).unwrap();
        let part = zip_bytes(&[("b11988f5-7a62-4da7-903a-17df3a95267d.CH", b"x")]).unwrap();
        let sniff = |head: &[u8], name: &str| {
            crate::OpenLabReader
                .sniff(head, Path::new(name))
                .map(|d| d.confidence)
        };
        // any Open Packaging Conventions file (an .xlsx) is not claimed without our extension
        assert_eq!(sniff(&opc, "plate.xlsx"), None);
        assert_eq!(sniff(&opc, "a.dx"), Some(DetectConfidence::Definite));
        assert_eq!(sniff(&dx, "renamed.zip"), Some(DetectConfidence::Likely));
        assert_eq!(sniff(&part, "a.dx"), Some(DetectConfidence::Definite));
        assert_eq!(sniff(b"##TITLE= a JCAMP-DX file", "a.dx"), None);
    }

    #[test]
    fn pairs_and_volumes() {
        let (t, v, e) = decode_pairs(&f64s(&[1.0, 2.0, 3.0]));
        assert_eq!((t, v), (vec![1.0], vec![2.0]));
        assert!(matches!(e, BodyEnd::Truncated { at: 16 }));
        assert_eq!(volume_ul(2.0, Some("nL")), Some(0.002));
        assert_eq!(volume_ul(1.0, Some("μL")), Some(1.0));
        assert_eq!(volume_ul(1.0, Some("drops")), None);
        assert_eq!(basename("D:\\a\\b.amx"), "b.amx");
    }
}
