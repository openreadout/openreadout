//! `Dataset` for Shimadzu LabSolutions `.lcd`/`.gcd` files: the compound-file structure (every
//! storage and stream with its size), the text found in the `File Property` stream, and the
//! chromatogram streams located by name. Chromatogram samples are not decoded (see
//! `docs/formats/shimadzu.md`, known gaps).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_core::experiment::{Acquisition, Experiment, Method, Origin, Sample};
use openreadout_core::model::{
    CheckReport, ColumnInfo, FileInfo, Finding, LsEntry, SignalChannelInfo, SpectraInfo, Spectrum,
    Table, TableInfo, Trace, TraceInfo,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::source::{Fs, Input};
use openreadout_core::time::filetime_to_iso8601;
use openreadout_core::{Error, Plane, Result};
use serde_json::{Value, json};

use crate::{SHIMADZU_ID, ShimadzuReader};
use openreadout_core::bytes::{latin1_field, le_u32};
use openreadout_core::cfb::{Cfb, CfbEntry};

/// Largest stream read whole for `vendor`/`check`.
const MAX_STREAM: u64 = 64 << 20;

/// Printable single-byte text runs of at least `min` characters, with their offsets.
pub fn text_runs(b: &[u8], min: usize) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, &c) in b.iter().chain(std::iter::once(&0)).enumerate() {
        let printable = (0x20..0x7F).contains(&c);
        match (printable, start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                if i - s >= min {
                    out.push((s, latin1_field(&b[s..i])));
                }
                start = None;
            }
            _ => {}
        }
    }
    out
}

/// `<name>value</name>` pairs of the XML property records (newer files); values written as
/// `@StoX@` followed by hex digits are decoded to text (`@StoX@352E3031` → `5.01`).
pub fn property_fields(runs: &[(usize, String)]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (_, t) in runs {
        let mut rest = t.as_str();
        while let Some(open) = rest.find('<') {
            rest = &rest[open + 1..];
            let Some(close) = rest.find('>') else { break };
            let name = &rest[..close];
            if name.starts_with(['/', '?']) || name.contains(' ') {
                continue;
            }
            let body = &rest[close + 1..];
            let end_tag = format!("</{name}>");
            let Some(end) = body.find(&end_tag) else {
                continue;
            };
            let value = &body[..end];
            if value.contains('<') {
                continue; // a container element
            }
            let value = match value.strip_prefix("@StoX@") {
                Some(hex) => (0..hex.len() / 2)
                    .filter_map(|i| u8::from_str_radix(hex.get(2 * i..2 * i + 2)?, 16).ok())
                    .map(char::from)
                    .collect(),
                None => value.to_string(),
            };
            out.push((name.to_string(), value));
            rest = &body[end + end_tag.len()..];
        }
    }
    out
}

/// A chromatogram stream (`<Raw Data storage>/Chromatogram ChN`).
#[derive(Debug, Clone)]
pub struct ShimadzuChannel {
    /// Stream path inside the compound file.
    pub stream: String,
    pub size: u64,
    /// The four-byte tag at offset 0 (`RC\0\0` in the corpus).
    pub tag: String,
    /// The little-endian u32 fields at offsets 4 and 8 (unnamed; see the format notes).
    pub field_04: Option<u32>,
    pub field_08: Option<u32>,
}

/// What a decoded signal holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShimadzuSignalKind {
    /// A `Chromatogram ChN` stream: one detector channel, raw integer output.
    Chromatogram,
    /// `PDA 3D Raw Data/Max Plot`: the largest absorbance over the wavelengths, per time point.
    MaxPlot,
    /// `PDA 3D Raw Data/3D Raw Data`: a spectrum per time point.
    Pda,
    /// A `StatusLog ChN` stream: an instrument status (pressure, temperature) over the run.
    Status,
}

/// A signal exposed as a trace.
#[derive(Debug, Clone)]
pub struct ShimadzuSignal {
    pub kind: ShimadzuSignalKind,
    /// Stream holding the samples.
    pub stream: String,
    pub count: u32,
    pub interval_ms: u32,
    /// PDA wavelengths (nm).
    pub wavelengths: Vec<f64>,
    /// Detector identity, export name and scale of a chromatogram channel, when the file
    /// records them (`shimadzu_channels`).
    pub label: Option<crate::shimadzu_channels::ChannelLabel>,
    /// The stream holds f64 values after its header (GC `.gcd`), not integer blocks.
    pub f64_values: bool,
}

impl ShimadzuSignal {
    /// Points of the trace: a chromatogram gets the vendor's leading t = 0 point (its first
    /// reading repeated), as LabSolutions' own chromatograms and ASCII exports have it.
    pub fn points(&self) -> u32 {
        match self.kind {
            ShimadzuSignalKind::Chromatogram | ShimadzuSignalKind::Status if self.count > 0 => {
                self.count.saturating_add(1)
            }
            _ => self.count,
        }
    }
}

/// Channel number of a `… ChN` stream name.
fn channel_number(stream: &str) -> Option<u32> {
    stream
        .rsplit('/')
        .next()?
        .rsplit(" Ch")
        .next()?
        .trim()
        .parse()
        .ok()
}

/// Values per absorbance unit of PDA streams: stored integers are µAU (inferred; see the notes).
const PDA_PER_MAU: f64 = 1000.0;
/// Largest signal stream read into memory.
const MAX_SIGNAL_STREAM: u64 = 2 << 30;

/// An opened `.lcd`/`.gcd` file.
#[derive(Debug)]
pub struct ShimadzuDataset {
    path: PathBuf,
    /// Where the file is read from.
    fs: Fs,
    size: u64,
    cfb: Cfb,
    channels: Vec<ShimadzuChannel>,
    property_text: Vec<(usize, String)>,
    property_times: Vec<(usize, String)>,
    property_fields: Vec<(String, String)>,
    /// Decodable signals, in trace order.
    signals: Vec<ShimadzuSignal>,
    /// `<UPD ID>` values of the PDA method, when present.
    pda_method: Vec<(String, String)>,
    /// LabSolutions' own peak tables, every channel's peaks in trace order.
    peaks: Vec<crate::shimadzu_peaks::VendorPeak>,
    /// The stream each channel's peak table came from, and tables that could not be read.
    peak_sources: Vec<(String, String)>,
    peak_errors: Vec<String>,
    /// LC-MS records (`TLM Raw Data`), when the file has them.
    tlm: Option<Tlm>,
}

/// The LC-MS part of a file: the index, every record's header, and `MS Raw Data`.
#[derive(Debug)]
struct Tlm {
    index: crate::shimadzu_tlm::TlmIndex,
    /// Per record: its header, or why it could not be read.
    headers: Vec<std::result::Result<crate::shimadzu_tlm::TlmHeader, String>>,
    raw: Vec<u8>,
}

/// Largest `MS Raw Data` stream read (the corpus's are up to 13 MB).
const MAX_TLM_STREAM: u64 = 1 << 30;

impl Tlm {
    fn read<R: std::io::Read + std::io::Seek + ?Sized>(
        cfb: &Cfb,
        f: &mut R,
        path: &Path,
    ) -> Result<Option<Tlm>> {
        let (Some(rt), Some(si), Some(tic), Some(raw)) = (
            cfb.stream("TLM Raw Data/Retention Time").cloned(),
            cfb.stream("TLM Raw Data/Spectrum Index").cloned(),
            cfb.stream("TLM Raw Data/TIC Data").cloned(),
            cfb.stream("TLM Raw Data/MS Raw Data").cloned(),
        ) else {
            return Ok(None);
        };
        if raw.size > MAX_TLM_STREAM {
            return Ok(None);
        }
        let index = crate::shimadzu_tlm::parse_index(
            &cfb.read(f, path, &rt, MAX_STREAM)?,
            &cfb.read(f, path, &si, MAX_STREAM)?,
            &cfb.read(f, path, &tic, MAX_STREAM)?,
        )
        .map_err(|e| Error::corrupt(SHIMADZU_ID, e))?;
        let raw = cfb.read(f, path, &raw, MAX_TLM_STREAM)?;
        let headers = index
            .entries
            .iter()
            .map(|e| crate::shimadzu_tlm::record_header(&raw, *e))
            .collect();
        Ok(Some(Tlm {
            index,
            headers,
            raw,
        }))
    }

    fn info(&self) -> SpectraInfo {
        use crate::shimadzu_tlm as t;
        let mut levels: Vec<u32> = Vec::new();
        let mut kinds: BTreeMap<String, u64> = BTreeMap::new();
        let mut events: Vec<u32> = Vec::new();
        for h in self.headers.iter().flatten() {
            if let Some(l) = t::ms_level(h.code)
                && !levels.contains(&l)
            {
                levels.push(l);
            }
            *kinds.entry(t::acquisition_name(h.code)).or_default() += 1;
            if !events.contains(&h.event) {
                events.push(h.event);
            }
        }
        levels.sort_unstable();
        let rt = |v: Option<&u32>| v.map(|x| f64::from(*x) / 1000.0);
        let mut extra = BTreeMap::new();
        extra.insert("acquisitions".into(), json!(kinds));
        extra.insert("events".into(), json!(events.len()));
        extra.insert(
            "unreadable_records".into(),
            json!(self.headers.iter().filter(|h| h.is_err()).count()),
        );
        SpectraInfo {
            index: 0,
            name: Some("LC-MS".into()),
            scan_count: self.index.entries.len() as u64,
            ms_levels: levels,
            rt_range_s: rt(self.index.rt_ms.iter().min())
                .zip(rt(self.index.rt_ms.iter().max()))
                .map(|(a, b)| [a, b]),
            instrument: None,
            extra,
        }
    }
}

/// A stream of XML text as single-byte characters; NUL bytes (the `2D Data Item` streams start
/// with two) become spaces instead of ending the text.
fn xml_text(b: &[u8]) -> String {
    b.iter()
        .map(|&c| if c == 0 { ' ' } else { char::from(c) })
        .collect()
}

/// Name a trace takes in `info` (the label's name, else the stream-based label).
fn signal_name(sig: &ShimadzuSignal) -> String {
    match sig.kind {
        ShimadzuSignalKind::MaxPlot => "PDA max plot".into(),
        ShimadzuSignalKind::Pda => "PDA spectra".into(),
        _ => sig
            .label
            .as_ref()
            .map(|l| l.name.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| {
                format!(
                    "{} {}",
                    sig.stream
                        .split('/')
                        .next()
                        .unwrap_or("")
                        .trim_end_matches(" Raw Data"),
                    sig.stream
                        .rsplit('/')
                        .next()
                        .unwrap_or("")
                        .trim_start_matches("Chromatogram ")
                )
            }),
    }
}

/// LabSolutions' peak tables for the file's chromatograms (and PDA channels): the peaks, the
/// stream each channel's table came from, and tables that could not be read.
#[allow(clippy::type_complexity)]
fn vendor_peaks<R: std::io::Read + std::io::Seek>(
    cfb: &Cfb,
    f: &mut R,
    path: &Path,
    signals: &[ShimadzuSignal],
) -> (
    Vec<crate::shimadzu_peaks::VendorPeak>,
    Vec<(String, String)>,
    Vec<String>,
) {
    use crate::shimadzu_peaks as sp;
    let mut read = |name: &str| -> Option<Vec<u8>> {
        let e = cfb.stream(name).cloned()?;
        cfb.read(f, path, &e, MAX_STREAM).ok()
    };
    // (signal name, candidate streams, newer layout)
    let mut tables: Vec<(String, Vec<String>, bool)> = Vec::new();
    let items = read("LSS Raw Data/2D Data Item")
        .map(|b| crate::shimadzu_channels::data_items(&xml_text(&b)))
        .unwrap_or_default();
    for sig in signals
        .iter()
        .filter(|s| s.kind == ShimadzuSignalKind::Chromatogram)
    {
        let Some(n) = channel_number(&sig.stream) else {
            continue;
        };
        if sig.stream.starts_with("LC Raw Data/") {
            tables.push((
                signal_name(sig),
                vec![format!("LC Data Processing/Peak Table-{n}")],
                false,
            ));
        } else if let Some(it) = items.iter().find(|i| {
            crate::shimadzu_channels::is_chromatogram_item(i)
                && i.channel == n
                && !i.data_set.is_empty()
        }) {
            tables.push((
                signal_name(sig),
                vec![
                    format!("LSS Data Processing/PT-{}", it.data_set),
                    format!("LSS Data Processing Original/PT-{}", it.data_set),
                ],
                true,
            ));
        }
    }
    // PDA channels: `PT-<PDA data set id>.<k>` (the 3-D item's id without `.3D`); k = 0 is the
    // max plot
    let pda_base = cfb.entries.iter().find_map(|e| {
        let rest = e.path.strip_prefix("LSS Data Processing/PT-")?;
        let (base, k) = rest.rsplit_once('.')?;
        (e.is_stream && base.starts_with("PDA") && k.parse::<u32>().is_ok())
            .then(|| base.to_string())
    });
    if let Some(base) = pda_base {
        let wls = read("LSS Data Processing/Multi Chromato Table")
            .map(|b| sp::multi_chromato_wavelengths(&b))
            .unwrap_or_default();
        let prefix = format!("LSS Data Processing/PT-{base}.");
        let mut ks: Vec<u32> = cfb
            .entries
            .iter()
            .filter(|e| e.is_stream && e.size > 20 && e.path.starts_with(&prefix))
            .filter_map(|e| e.path[prefix.len()..].parse().ok())
            .collect();
        ks.sort_unstable();
        for k in ks {
            // channel k ≥ 1 is record k − 1 of `Multi Chromato Table`; channel 0 is not in it
            // (in the corpus file its areas fit ~192 nm, not the max plot): no wavelength given
            let name = match k
                .checked_sub(1)
                .and_then(|i| wls.get(i as usize).copied().flatten())
            {
                Some(w) => format!("PDA Ch{k} {w} nm"),
                None => format!("PDA Ch{k}"),
            };
            tables.push((name, vec![format!("{prefix}{k}")], true));
        }
    }
    let mut peaks = Vec::new();
    let mut sources = Vec::new();
    let mut errors = Vec::new();
    for (signal, candidates, newer) in tables {
        let Some((stream, b)) = candidates
            .iter()
            .find_map(|c| read(c).map(|b| (c.clone(), b)))
        else {
            continue;
        };
        let parsed = if newer {
            sp::peak_table_new(&b)
        } else {
            sp::peak_table_old(&b)
        };
        match parsed {
            Ok(ps) => {
                if !ps.is_empty() {
                    sources.push((signal.clone(), stream));
                }
                peaks.extend(ps.into_iter().map(|mut p| {
                    p.signal.clone_from(&signal);
                    p
                }));
            }
            Err(e) => errors.push(format!("{stream}: {e}")),
        }
    }
    (peaks, sources, errors)
}

fn is_chromatogram(e: &CfbEntry) -> bool {
    let name = e.path.rsplit('/').next().unwrap_or("");
    e.is_stream && e.size > 0 && name.starts_with("Chromatogram Ch")
}

impl ShimadzuDataset {
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    /// Open an [`Input`] (a local path, a buffer, a host source).
    pub(crate) fn open_input(input: &Input) -> Result<Self> {
        let (path, fs) = (input.path(), input.fs());
        let mut f = fs.open(path).map_err(|e| Error::io(path, e))?;
        let size = f.metadata().map_err(|e| Error::io(path, e))?.len();
        let cfb = Cfb::open(&mut f, path, SHIMADZU_ID)?;
        let mut channels = Vec::new();
        for e in cfb.entries.iter().filter(|e| is_chromatogram(e)) {
            let head = cfb.read(&mut f, path, e, 16).unwrap_or_default();
            channels.push(ShimadzuChannel {
                stream: e.path.clone(),
                size: e.size,
                tag: head
                    .get(..4)
                    .map(|t| {
                        t.iter()
                            .map(|&c| if c == 0 { '0' } else { c as char })
                            .collect()
                    })
                    .unwrap_or_default(),
                field_04: le_u32(&head, 4),
                field_08: le_u32(&head, 8),
            });
        }
        let mut property_text = Vec::new();
        let mut property_times = Vec::new();
        if let Some(e) = cfb.stream("File Property").cloned() {
            let b = cfb.read(&mut f, path, &e, MAX_STREAM).unwrap_or_default();
            property_text = text_runs(&b, 3);
            // Windows FILETIMEs (100 ns since 1601) between 1990 and 2100, 4-byte aligned
            for off in (0..b.len().saturating_sub(8)).step_by(4) {
                let lo = u64::from(le_u32(&b, off).unwrap_or(0));
                let hi = u64::from(le_u32(&b, off + 4).unwrap_or(0));
                let ft = hi << 32 | lo;
                if (0x01B2_1DD2_1300_0000..0x0208_7E4F_7E00_0000).contains(&ft) {
                    property_times.push((off, filetime_to_iso8601(ft)));
                }
            }
        }
        let property_fields = property_fields(&property_text);
        // Signals: chromatogram streams (header gives count and interval), then the PDA field.
        let mut signals = Vec::new();
        for c in &channels {
            if c.tag == "RC00"
                && let (Some(iv), Some(n)) = (c.field_04, c.field_08)
            {
                signals.push(ShimadzuSignal {
                    kind: ShimadzuSignalKind::Chromatogram,
                    stream: c.stream.clone(),
                    count: n,
                    interval_ms: iv,
                    wavelengths: Vec::new(),
                    label: None,
                    f64_values: crate::shimadzu_signal::is_f64_record(c.size, n),
                });
            }
        }
        // Who each chromatogram channel is: older layout from `Chromatogram Status` and the LC
        // configuration's modules, newer layout from `2D Data Item`.
        {
            use crate::shimadzu_channels as sc;
            let mut read = |name: &str| -> Option<Vec<u8>> {
                let e = cfb.stream(name).cloned()?;
                cfb.read(&mut f, path, &e, MAX_STREAM).ok()
            };
            let statuses = read("LC Raw Data/Chromatogram Status")
                .map(|b| sc::channel_statuses(&b))
                .unwrap_or_default();
            let lss_statuses = read("LSS Raw Data/Chromatogram Status")
                .map(|b| sc::channel_statuses(&b))
                .unwrap_or_default();
            let modules = read("LSS Configuration/LC Configuration")
                .map(|b| sc::lc_modules(&b))
                .unwrap_or_default();
            let items = read("LSS Raw Data/2D Data Item")
                .map(|b| sc::data_items(&xml_text(&b)))
                .unwrap_or_default();
            for storage in ["LC Raw Data/", "LSS Raw Data/"] {
                let idx: Vec<usize> = (0..signals.len())
                    .filter(|&i| signals[i].stream.starts_with(storage))
                    .collect();
                let chans: Vec<u32> = idx
                    .iter()
                    .map(|&i| channel_number(&signals[i].stream).unwrap_or(0))
                    .collect();
                let labels = if storage == "LC Raw Data/" {
                    sc::label_lc_channels(&chans, &statuses, &modules)
                } else {
                    sc::label_lss_channels(&chans, &items, &lss_statuses)
                };
                for (i, l) in idx.into_iter().zip(labels) {
                    signals[i].label = l.filter(|l| !l.name.is_empty());
                }
            }
        }
        let mut pda_method = Vec::new();
        if let Some(e) = cfb
            .entries
            .iter()
            .find(|e| e.is_stream && e.path.ends_with("/PDA.1.METHOD"))
            .cloned()
        {
            let b = cfb.read(&mut f, path, &e, MAX_STREAM).unwrap_or_default();
            pda_method = crate::shimadzu_signal::method_values(&b);
        }
        if let (Some(mp), Some(wt), Some(raw)) = (
            cfb.stream("PDA 3D Raw Data/Max Plot").cloned(),
            cfb.stream("PDA 3D Raw Data/Wavelength Table").cloned(),
            cfb.stream("PDA 3D Raw Data/3D Raw Data").cloned(),
        ) {
            let head = cfb.read(&mut f, path, &mp, 24).unwrap_or_default();
            let wl = cfb
                .read(&mut f, path, &wt, MAX_STREAM)
                .ok()
                .and_then(|b| crate::shimadzu_signal::wavelength_table(&b).ok());
            if let (Ok(h), Some(wl)) = (crate::shimadzu_signal::parse_rc_header(&head), wl) {
                signals.push(ShimadzuSignal {
                    kind: ShimadzuSignalKind::MaxPlot,
                    stream: mp.path.clone(),
                    count: h.count,
                    interval_ms: h.interval_ms,
                    wavelengths: Vec::new(),
                    label: None,
                    f64_values: false,
                });
                if !wl.is_empty() && raw.size > 0 {
                    signals.push(ShimadzuSignal {
                        kind: ShimadzuSignalKind::Pda,
                        stream: raw.path.clone(),
                        count: h.count,
                        interval_ms: h.interval_ms,
                        wavelengths: wl,
                        label: None,
                        f64_values: false,
                    });
                }
            }
        }
        // Status logs (pressures, temperatures), after the other traces so their indices stay.
        {
            use crate::shimadzu_channels as sc;
            let mut read = |name: &str| -> Option<Vec<u8>> {
                let e = cfb.stream(name).cloned()?;
                cfb.read(&mut f, path, &e, MAX_STREAM).ok()
            };
            let modules = read("LSS Configuration/LC Configuration")
                .map(|b| sc::lc_modules(&b))
                .unwrap_or_default();
            let items = read("LSS Raw Data/2D Data Item")
                .map(|b| sc::data_items(&xml_text(&b)))
                .unwrap_or_default();
            for storage in ["LC Raw Data/", "LSS Raw Data/"] {
                let mut logs: Vec<(u32, CfbEntry)> = cfb
                    .entries
                    .iter()
                    .filter(|e| {
                        e.is_stream
                            && e.size > 24
                            && e.path.starts_with(storage)
                            && e.path[storage.len()..].starts_with("StatusLog Ch")
                    })
                    .filter_map(|e| Some((channel_number(&e.path)?, e.clone())))
                    .collect();
                if logs.is_empty() {
                    continue;
                }
                logs.sort_by_key(|(n, _)| *n);
                let statuses = read(&format!("{storage}StatusLog Status"))
                    .map(|b| sc::channel_statuses(&b))
                    .unwrap_or_default();
                let chans: Vec<u32> = logs.iter().map(|(n, _)| *n).collect();
                let labels = if storage == "LC Raw Data/" {
                    sc::label_lc_status(&chans, &statuses, &modules)
                } else {
                    sc::label_lss_status(&chans, &items, &statuses)
                };
                for ((_, e), label) in logs.into_iter().zip(labels) {
                    // unscaled logs (no status record) are not exposed: their unit is unknown
                    let Some(label) = label else { continue };
                    let head = read(&e.path).unwrap_or_default();
                    let Ok(h) = crate::shimadzu_signal::parse_rc_header(&head) else {
                        continue;
                    };
                    signals.push(ShimadzuSignal {
                        kind: ShimadzuSignalKind::Status,
                        stream: e.path.clone(),
                        count: h.count,
                        interval_ms: h.interval_ms,
                        wavelengths: Vec::new(),
                        label: Some(label),
                        f64_values: false,
                    });
                }
            }
        }
        let (peaks, peak_sources, peak_errors) = vendor_peaks(&cfb, &mut f, path, &signals);
        let tlm = Tlm::read(&cfb, &mut f, path)?;
        Ok(ShimadzuDataset {
            tlm,
            peaks,
            peak_sources,
            peak_errors,
            signals,
            pda_method,
            property_fields,
            path: path.to_path_buf(),
            fs: fs.clone(),
            size,
            cfb,
            channels,
            property_text,
            property_times,
        })
    }

    /// Chromatogram streams found (for library users).
    pub fn channels(&self) -> &[ShimadzuChannel] {
        &self.channels
    }

    /// Decodable signals, in trace order.
    pub fn signals(&self) -> &[ShimadzuSignal] {
        &self.signals
    }

    /// LabSolutions' own peak tables, every channel's peaks in trace order.
    pub fn vendor_peaks(&self) -> &[crate::shimadzu_peaks::VendorPeak] {
        &self.peaks
    }

    /// Values of signal `i` as stored (integers, or the f64 values of a GC chromatogram),
    /// one row per time point, unscaled.
    fn rows_f64(&self, i: usize) -> Result<Vec<Vec<f64>>> {
        let sig = self
            .signals
            .get(i)
            .ok_or_else(|| Error::Usage(format!("trace {i} does not exist")))?;
        if !sig.f64_values {
            return Ok(self
                .signal_values(i)?
                .into_iter()
                .map(|r| r.into_iter().map(|v| v as f64).collect())
                .collect());
        }
        let entry = self
            .cfb
            .stream(&sig.stream)
            .cloned()
            .ok_or_else(|| Error::corrupt(SHIMADZU_ID, format!("{} is missing", sig.stream)))?;
        let mut file = self
            .fs
            .open(&self.path)
            .map_err(|er| Error::io(&self.path, er))?;
        let data = self
            .cfb
            .read(&mut file, &self.path, &entry, MAX_SIGNAL_STREAM)?;
        let (_, v) = crate::shimadzu_signal::decode_f64_record(&data, sig.count as usize)
            .map_err(|m| Error::corrupt(SHIMADZU_ID, format!("{}: {m}", sig.stream)))?;
        Ok(v.into_iter().map(|x| vec![x]).collect())
    }

    /// All values of signal `i`: one row per time point (one value for 2-D signals, one per
    /// wavelength for the PDA), unscaled. GC chromatograms stored as f64 are refused here (use
    /// `Dataset::read_trace`).
    pub fn signal_values(&self, i: usize) -> Result<Vec<Vec<i64>>> {
        use crate::shimadzu_signal::{decode_record, record_offsets};
        let sig = self
            .signals
            .get(i)
            .ok_or_else(|| Error::Usage(format!("trace {i} does not exist")))?;
        let entry = self
            .cfb
            .stream(&sig.stream)
            .cloned()
            .ok_or_else(|| Error::corrupt(SHIMADZU_ID, format!("{} is missing", sig.stream)))?;
        let mut file = self
            .fs
            .open(&self.path)
            .map_err(|er| Error::io(&self.path, er))?;
        let data = self
            .cfb
            .read(&mut file, &self.path, &entry, MAX_SIGNAL_STREAM)?;
        let bad = |m: crate::shimadzu_signal::SignalError| {
            Error::corrupt(SHIMADZU_ID, format!("{}: {m}", sig.stream))
        };
        if sig.f64_values {
            return Err(Error::unsupported(
                SHIMADZU_ID,
                "integer values of an f64 signal stream",
                "Read the trace with `trace` / Dataset::read_trace: this chromatogram is stored as f64 values.",
            ));
        }
        match sig.kind {
            ShimadzuSignalKind::Chromatogram
            | ShimadzuSignalKind::MaxPlot
            | ShimadzuSignalKind::Status => {
                let (_, v, _) = decode_record(&data, sig.count as usize).map_err(bad)?;
                Ok(v.into_iter().map(|x| vec![x]).collect())
            }
            ShimadzuSignalKind::Pda => {
                let offs = record_offsets(&data, sig.count as usize).map_err(bad)?;
                if offs.len() != sig.count as usize {
                    return Err(Error::corrupt(
                        SHIMADZU_ID,
                        format!(
                            "{}: {} spectra, the max plot has {} time points",
                            sig.stream,
                            offs.len(),
                            sig.count
                        ),
                    ));
                }
                offs.iter()
                    .map(|&o| {
                        let (_, v, _) =
                            decode_record(&data[o..], sig.wavelengths.len()).map_err(bad)?;
                        if v.len() != sig.wavelengths.len() {
                            return Err(bad(crate::shimadzu_signal::SignalError(format!(
                                "a spectrum of {} values, the wavelength table has {}",
                                v.len(),
                                sig.wavelengths.len()
                            ))));
                        }
                        Ok(v)
                    })
                    .collect()
            }
        }
    }

    fn signal_trace_info(&self, index: u32, sig: &ShimadzuSignal) -> TraceInfo {
        let step_min = f64::from(sig.interval_ms) / 60_000.0;
        let mut extra = BTreeMap::new();
        extra.insert("stream".into(), json!(sig.stream));
        extra.insert(
            "axis".into(),
            json!({"quantity": "retention_time", "unit": "min", "first": 0.0, "step": step_min}),
        );
        extra.insert("interval_ms".into(), json!(sig.interval_ms));
        let (name, unit, scale, detector) = match sig.kind {
            ShimadzuSignalKind::Chromatogram => {
                let storage = sig.stream.split('/').next().unwrap_or("");
                let ch = sig.stream.rsplit('/').next().unwrap_or("");
                let stream_label = format!(
                    "{} {}",
                    storage.trim_end_matches(" Raw Data"),
                    ch.trim_start_matches("Chromatogram ")
                );
                extra.insert("stream_label".into(), json!(stream_label));
                let l = sig.label.clone().unwrap_or_default();
                if !l.export_section.is_empty() {
                    extra.insert("export_section".into(), json!(l.export_section));
                }
                if let Some(v) = &l.detector_name {
                    extra.insert("detector_name".into(), json!(v));
                }
                if let Some(v) = &l.detector_model {
                    extra.insert("detector_model".into(), json!(v));
                }
                if let Some(v) = l.wavelength_nm {
                    extra.insert("wavelength_nm".into(), json!(v));
                }
                if let Some(v) = l.scale {
                    // values are the stored integers times this factor
                    extra.insert("raw_scale".into(), json!(v));
                }
                extra.insert("stored_points".into(), json!(sig.count));
                (
                    if l.name.is_empty() {
                        stream_label
                    } else {
                        l.name.clone()
                    },
                    l.unit.clone(),
                    1.0,
                    "chromatogram",
                )
            }
            ShimadzuSignalKind::MaxPlot => (
                "PDA max plot".to_string(),
                Some("mAU".to_string()),
                1.0 / PDA_PER_MAU,
                "pda_max_plot",
            ),
            ShimadzuSignalKind::Pda => (
                "PDA spectra".to_string(),
                Some("mAU".to_string()),
                1.0 / PDA_PER_MAU,
                "pda",
            ),
            ShimadzuSignalKind::Status => {
                let l = sig.label.clone().unwrap_or_default();
                extra.insert(
                    "stream_label".into(),
                    json!(format!(
                        "{} {}",
                        sig.stream
                            .split('/')
                            .next()
                            .unwrap_or("")
                            .trim_end_matches(" Raw Data"),
                        sig.stream.rsplit('/').next().unwrap_or("")
                    )),
                );
                if !l.export_section.is_empty() {
                    extra.insert("export_section".into(), json!(l.export_section));
                }
                if let Some(v) = &l.detector_model {
                    extra.insert("module_model".into(), json!(v));
                }
                if let Some(v) = l.scale {
                    extra.insert("raw_scale".into(), json!(v));
                }
                extra.insert(
                    "quantity".into(),
                    json!(crate::shimadzu_channels::quantity(l.unit.as_deref())),
                );
                extra.insert("stored_points".into(), json!(sig.count));
                (l.name.clone(), l.unit.clone(), 1.0, "status")
            }
        };
        if sig.f64_values {
            extra.insert("stored_as".into(), json!("f64"));
        }
        extra.insert("detector".into(), json!(detector));
        let channels: Vec<SignalChannelInfo> = if sig.kind == ShimadzuSignalKind::Pda {
            if let (Some(a), Some(b)) = (sig.wavelengths.first(), sig.wavelengths.last()) {
                extra.insert("wavelength_range_nm".into(), json!([a, b]));
            }
            sig.wavelengths
                .iter()
                .enumerate()
                .map(|(i, w)| {
                    let mut e = BTreeMap::new();
                    e.insert("wavelength_nm".into(), json!(w));
                    SignalChannelInfo {
                        index: i as u32,
                        name: format!("{w} nm"),
                        unit: unit.clone(),
                        dtype: "int64".into(),
                        scale,
                        offset: 0.0,
                        extra: e,
                    }
                })
                .collect()
        } else {
            vec![SignalChannelInfo {
                index: 0,
                name: name.clone(),
                unit: unit.clone(),
                dtype: if sig.f64_values { "float64" } else { "int64" }.into(),
                scale,
                offset: 0.0,
                extra: BTreeMap::new(),
            }]
        };
        let rate = if sig.interval_ms > 0 {
            1000.0 / f64::from(sig.interval_ms)
        } else {
            0.0
        };
        TraceInfo {
            index,
            name: Some(name),
            sample_rate_hz: rate,
            sample_count: u64::from(sig.points()),
            sweep_count: 1,
            channels,
            start_s: Some(0.0),
            extra,
        }
    }

    fn field(&self, name: &str) -> Option<&str> {
        self.property_fields
            .iter()
            .find(|(k, v)| k == name && !v.trim().is_empty())
            .map(|(_, v)| v.trim())
    }

    /// Sample, vial, operator, method and start time from `File Property`
    /// (`docs/formats/shimadzu.md` § Experiment facts).
    fn experiment_facts(&self) -> Option<Experiment> {
        let mut e = Experiment::default();
        let origin = |e: &mut Experiment, key: &str, from: String| {
            e.provenance.insert(
                key.into(),
                Origin {
                    source: Source::Inferred,
                    from,
                },
            );
        };
        let mut sample = Sample::default();
        let id = self
            .field("smpl_id")
            .map(|v| (v, "smpl_id"))
            .or_else(|| self.field("smpl_name").map(|v| (v, "smpl_name")));
        if let Some((v, k)) = id {
            let from = format!("File Property SampleInfo/{k}");
            sample.id = Some(v.to_string());
            sample.source_field = Some(from.clone());
            origin(&mut e, "sample.id", from);
        }
        if let Some(n) = self
            .field("smpl_name")
            .filter(|n| sample.id.as_deref() != Some(*n))
        {
            sample.name = Some(n.to_string());
            origin(
                &mut e,
                "sample.name",
                "File Property SampleInfo/smpl_name".into(),
            );
        }
        if let Some(v) = self.field("szVialNum").or_else(|| self.field("vial_num")) {
            sample.sequence_position = Some(v.to_string());
            origin(
                &mut e,
                "sample.sequence_position",
                "File Property SampleInfo/szVialNum".into(),
            );
        }
        if sample != Sample::default() {
            e.sample = Some(sample);
        }
        let v3 = self
            .property_text
            .iter()
            .any(|(o, t)| *o == 4 && t == "3.00");
        let mut acq = Acquisition::default();
        let operator = self
            .field("operator_name")
            .map(|v| {
                (
                    v.to_string(),
                    "File Property SampleInfo/operator_name".to_string(),
                )
            })
            .or_else(|| {
                self.field("szGeneratedBy").map(|v| {
                    (
                        v.to_string(),
                        "File Property FileProperty/szGeneratedBy".to_string(),
                    )
                })
            })
            .or_else(|| {
                self.property_text
                    .iter()
                    .find(|(o, _)| v3 && *o == 0x14)
                    .map(|(_, t)| (t.clone(), "File Property 0x0014".to_string()))
            });
        if let Some((v, from)) = operator {
            acq.operator = Some(v);
            origin(&mut e, "acquisition.operator", from);
        }
        let generated = match (
            self.field("dwHighGeneratedDateTime")
                .and_then(|v| v.parse::<i64>().ok()),
            self.field("dwLowGeneratedDateTime")
                .and_then(|v| v.parse::<i64>().ok()),
        ) {
            (Some(hi), Some(lo)) => u32::try_from(hi).ok().map(|hi| {
                (
                    (u64::from(hi) << 32) | u64::from(lo as u32),
                    "File Property FileProperty/dwHighGeneratedDateTime, dwLowGeneratedDateTime",
                )
            }),
            _ => None,
        }
        .or_else(|| {
            v3.then(|| {
                self.property_times
                    .iter()
                    .find(|(o, _)| *o == 0x34)
                    .map(|_| (0u64, "File Property 0x0034"))
            })
            .flatten()
        });
        if let Some((ft, from)) = generated {
            let iso = if ft == 0 {
                self.property_times
                    .iter()
                    .find(|(o, _)| *o == 0x34)
                    .map(|(_, t)| t.clone())
            } else {
                (0x01B2_1DD2_1300_0000..0x0208_7E4F_7E00_0000)
                    .contains(&ft)
                    .then(|| filetime_to_iso8601(ft))
            };
            if let Some(iso) = iso {
                acq.started_at = Some(iso);
                origin(&mut e, "acquisition.started_at", from.into());
            }
        }
        if acq != Acquisition::default() {
            e.acquisition = Some(acq);
        }
        let method = self
            .field("methodfile")
            .map(|v| {
                (
                    v.to_string(),
                    "File Property SampleInfoFile/methodfile".to_string(),
                )
            })
            .or_else(|| {
                self.property_text
                    .iter()
                    .find(|(_, t)| t.to_ascii_lowercase().ends_with(".lcm"))
                    .map(|(o, t)| (t.clone(), format!("File Property 0x{o:04X}")))
            });
        if let Some((path, from)) = method {
            let last = path.rsplit(['\\', '/']).next().unwrap_or(&path);
            let stem = last
                .get(..last.len().saturating_sub(4))
                .filter(|s| !s.trim().is_empty());
            if let Some(stem) = stem {
                e.method = Some(Method {
                    name: Some(stem.trim().to_string()),
                    ..Method::default()
                });
                origin(&mut e, "method.name", from);
            }
        }
        (!e.is_empty()).then_some(e)
    }

    /// Vendor areas against our traces (chromatograms and the PDA max plot): LabSolutions reports
    /// µ-units × s, i.e. 1000 × mV·s or mAU·s, and µV·s as stored for µV traces.
    fn check_vendor_areas(&mut self, r: &mut CheckReport) {
        let largest = self.peaks.iter().map(|p| p.area.abs()).fold(0.0, f64::max);
        let mut checked = 0usize;
        let mut off = Vec::new();
        let names: Vec<String> = self.signals.iter().map(signal_name).collect();
        for (i, name) in names.iter().enumerate() {
            let sig = self.signals[i].clone();
            if !matches!(
                sig.kind,
                ShimadzuSignalKind::Chromatogram | ShimadzuSignalKind::MaxPlot
            ) {
                continue;
            }
            let peaks: Vec<_> = self
                .peaks
                .iter()
                .filter(|p| &p.signal == name && p.area.abs() >= 0.01 * largest)
                .cloned()
                .collect();
            if peaks.is_empty() || sig.interval_ms == 0 {
                continue;
            }
            let Ok(t) = self.read_trace(i as u32, 0, 0, u64::MAX) else {
                continue;
            };
            let v = &t.channels[0];
            let per_unit = match sig.label.as_ref().and_then(|l| l.unit.as_deref()) {
                Some("µV") => 1.0,
                _ => 1000.0,
            };
            let dt = f64::from(sig.interval_ms) / 1000.0;
            for p in peaks {
                let idx = |m: f64| (m * 60.0 / dt).round() as usize;
                let (i0, i1) = (idx(p.start_min), idx(p.end_min));
                if i1 <= i0 || i1 >= v.len() {
                    continue;
                }
                let integ: f64 = v[i0..=i1]
                    .windows(2)
                    .map(|w| f64::midpoint(w[0], w[1]) * dt)
                    .sum();
                let base = f64::midpoint(p.baseline_start, p.baseline_end) / per_unit
                    * (i1 - i0) as f64
                    * dt;
                let ours = (integ - base) * per_unit;
                checked += 1;
                if ours == 0.0 || ((p.area - ours) / ours).abs() > 0.10 {
                    off.push(format!(
                        "{name} peak {} at {:.3} min: vendor {:.1}, ours {:.1}",
                        p.number, p.rt_min, p.area, ours
                    ));
                }
            }
        }
        if !off.is_empty() {
            r.push(Finding::warning(
                "vendor_area_mismatch",
                format!(
                    "{} of {checked} vendor peak areas differ from our trace by more than 10 % (units or scaling may be wrong): {}",
                    off.len(),
                    off.iter().take(5).cloned().collect::<Vec<_>>().join("; ")
                ),
            ));
        }
    }

    /// The `vendor_peaks` table (tables[0]) when the file holds LabSolutions peak tables.
    fn peak_table_info(&self) -> Option<TableInfo> {
        if self.peaks.is_empty() {
            return None;
        }
        let mut signals: Vec<String> = Vec::new();
        for p in &self.peaks {
            if !signals.contains(&p.signal) {
                signals.push(p.signal.clone());
            }
        }
        let unit_of = |s: &str| {
            self.signals
                .iter()
                .find(|g| signal_name(g) == s)
                .and_then(|g| match g.kind {
                    ShimadzuSignalKind::MaxPlot | ShimadzuSignalKind::Pda => Some("mAU".into()),
                    _ => g.label.as_ref().and_then(|l| l.unit.clone()),
                })
                .or_else(|| s.starts_with("PDA").then(|| "mAU".to_string()))
        };
        // LabSolutions reports areas and heights in µ-units (µV·s, µAU·s): 1000 × a trace in
        // mV or mAU; a trace already in µV is reported as stored
        let units: Vec<Option<String>> = signals.iter().map(|s| unit_of(s)).collect();
        let base = |u: &Option<String>| match u.as_deref() {
            Some("mV" | "µV") => Some("µV"),
            Some("mAU" | "µAU") => Some("µAU"),
            _ => None,
        };
        let common = units
            .first()
            .and_then(base)
            .filter(|b| units.iter().all(|u| base(u) == Some(*b)));
        let col = |index: u32, name: &str, unit: Option<String>, label: &str| ColumnInfo {
            index,
            name: name.into(),
            label: Some(label.into()),
            dtype: "float64".into(),
            unit,
            range: None,
            extra: BTreeMap::new(),
        };
        let min = || Some("min".to_string());
        let mut signal = col(
            0,
            "signal",
            None,
            "trace the peak was integrated on (code into extra.categories)",
        );
        signal.dtype = "uint32".into();
        signal
            .extra
            .insert("categories".into(), json!(signals.clone()));
        let columns = vec![
            signal,
            col(1, "peak", None, "peak number within its signal's table"),
            col(2, "rt_min", min(), "retention time"),
            col(3, "start_min", min(), "start of the integration"),
            col(4, "end_min", min(), "end of the integration"),
            col(
                5,
                "area",
                common.map(|b| format!("{b}*s")),
                "peak area as LabSolutions reports it (µ-units × s: 1000 × the trace's mV·s or mAU·s)",
            ),
            col(
                6,
                "height",
                common.map(str::to_string),
                "peak height above the baseline, as LabSolutions reports it (µ-units)",
            ),
            col(
                7,
                "baseline_start",
                common.map(str::to_string),
                "baseline value at the start",
            ),
            col(
                8,
                "baseline_end",
                common.map(str::to_string),
                "baseline value at the end",
            ),
            col(
                9,
                "capacity_factor",
                None,
                "k′ (older layout only; NaN otherwise)",
            ),
            col(
                10,
                "plates",
                None,
                "theoretical plate number (older layout only)",
            ),
            col(11, "plate_height", None, "plate height (older layout only)"),
            col(12, "tailing", None, "tailing factor (older layout only)"),
            col(
                13,
                "resolution",
                None,
                "resolution to the previous peak (older layout only)",
            ),
            col(
                14,
                "flags",
                None,
                "record flags (0x10 on peaks not marked V, valley, in the vendor's export)",
            ),
        ];
        let mut extra = BTreeMap::new();
        extra.insert("source".into(), json!("vendor"));
        extra.insert(
            "streams".into(),
            json!(
                self.peak_sources
                    .iter()
                    .map(|(s, st)| json!({"signal": s, "stream": st}))
                    .collect::<Vec<_>>()
            ),
        );
        Some(TableInfo {
            index: 0,
            name: Some("vendor_peaks".into()),
            row_count: self.peaks.len() as u64,
            columns,
            extra,
        })
    }

    fn raw_storages(&self) -> Vec<String> {
        self.cfb
            .entries
            .iter()
            .filter(|e| !e.is_stream && e.path.ends_with("Raw Data"))
            .map(|e| e.path.clone())
            .collect()
    }
}

impl Dataset for ShimadzuDataset {
    fn info(&self) -> Result<FileInfo> {
        let mut notes = vec![format!(
            "compound file with {} streams; raw-data storages: {}",
            self.cfb.entries.iter().filter(|e| e.is_stream).count(),
            self.raw_storages().join(", ")
        )];
        if self.signals.is_empty() && self.tlm.is_none() {
            notes.push("no decodable signal stream (no `Chromatogram ChN`, PDA 3-D or LC-MS data); see `info --view structure`".into());
        }
        if let Some(t) = &self.tlm {
            let profile = t
                .headers
                .iter()
                .flatten()
                .filter(|h| {
                    matches!(
                        h.code,
                        crate::shimadzu_tlm::TLM_FULL_SCAN
                            | crate::shimadzu_tlm::TLM_PRODUCT_ION_SCAN
                    )
                })
                .count();
            notes.push(format!(
                "LC-MS data (TLM Raw Data): {} records as spectra run 0 (MRM and SIM spectra; polarity not stated){}",
                t.index.entries.len(),
                if profile > 0 {
                    format!("; {profile} full-scan or product-ion-scan records are listed (TIC, retention time, precursor) but their spectra are not read (exit 6): the vendor software reports other values than the file stores")
                } else {
                    String::new()
                }
            ));
        }
        if self.signals.iter().any(|s| {
            s.kind == ShimadzuSignalKind::Chromatogram
                && s.label.as_ref().and_then(|l| l.scale).is_none()
        }) {
            notes.push("some `Chromatogram ChN` values are the detector's stored integers: the file records no unit or scale for them".into());
        }
        let version = self
            .property_fields
            .iter()
            .find(|(k, _)| k == "szVersion")
            .map(|(_, v)| v.clone())
            .or_else(|| {
                self.property_text
                    .iter()
                    .find(|(o, s)| *o == 4 && s.chars().all(|c| c.is_ascii_digit() || c == '.'))
                    .map(|(_, s)| s.clone())
            });
        if let Some((_, by)) = self
            .property_fields
            .iter()
            .find(|(k, _)| k == "szGeneratedBy")
        {
            notes.push(format!("generated by: {by}"));
        }
        if !self.peaks.is_empty() {
            notes.push(format!(
                "tables[0] (vendor_peaks) holds LabSolutions' own peak tables: {} peaks on {} signal(s)",
                self.peaks.len(),
                self.peak_sources.len()
            ));
        }
        for e in &self.peak_errors {
            notes.push(format!("a vendor peak table was not read: {e}"));
        }
        if self
            .signals
            .iter()
            .any(|s| s.kind == ShimadzuSignalKind::Status)
        {
            notes.push("status traces (pressures, temperatures) are scaled to the units their status record states".into());
        }
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.size,
            format: ShimadzuReader.descriptor(),
            format_version: version,
            images: Vec::new(),
            tables: self.peak_table_info().into_iter().collect(),
            spectra: self.tlm.as_ref().map(Tlm::info).into_iter().collect(),
            traces: self
                .signals
                .iter()
                .enumerate()
                .map(|(i, s)| self.signal_trace_info(i as u32, s))
                .collect(),
            plane_count: 0,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let streams: Vec<Value> = self
            .cfb
            .entries
            .iter()
            .filter(|e| e.is_stream)
            .map(|e| json!({"path": e.path, "size": e.size}))
            .collect();
        let text: BTreeMap<String, Value> = self
            .property_text
            .iter()
            .map(|(o, s)| (format!("0x{o:04X}"), json!(s)))
            .collect();
        let times: BTreeMap<String, Value> = self
            .property_times
            .iter()
            .map(|(o, s)| (format!("0x{o:04X}"), json!(s)))
            .collect();
        let channels: Vec<Value> = self
            .channels
            .iter()
            .map(|c| json!({"stream": c.stream, "size": c.size, "tag": c.tag, "field_04": c.field_04, "field_08": c.field_08}))
            .collect();
        Ok(json!({
            "compound_file_version": self.cfb.version,
            "sector_size": self.cfb.sector_size,
            "file_property_text": text,
            "file_property_times": times,
            "file_property_fields": self.property_fields.iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>(),
            "chromatogram_streams": channels,
            "pda_method": self.pda_method.iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>(),
            "streams": streams,
        }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        p.insert("format_version".into(), Source::Inferred);
        p
    }

    fn experiment(&self) -> Option<Experiment> {
        self.experiment_facts()
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        Ok(self
            .cfb
            .entries
            .iter()
            .filter(|e| !e.path.is_empty())
            .map(|e| {
                let ch = self.channels.iter().find(|c| c.stream == e.path);
                LsEntry {
                    kind: if ch.is_some() {
                        "chromatogram"
                    } else if e.is_stream {
                        "stream"
                    } else {
                        "storage"
                    }
                    .into(),
                    name: e.path.clone(),
                    offset: None,
                    size: e.is_stream.then_some(e.size),
                    image: None,
                    details: ch.map_or(
                        Value::Null,
                        |c| json!({"tag": c.tag, "field_04": c.field_04, "field_08": c.field_08}),
                    ),
                }
            })
            .collect())
    }

    fn read_table(&mut self, index: u32, first_row: u64, max_rows: u64) -> Result<Table> {
        let Some(info) = self.peak_table_info().filter(|_| index == 0) else {
            return Err(Error::Usage(format!(
                "table {index} not found (this file has {} tables)",
                usize::from(!self.peaks.is_empty())
            )));
        };
        let cats: Vec<String> = info.columns[0]
            .extra
            .get("categories")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .map(|x| x.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default();
        let start = usize::try_from(first_row.min(self.peaks.len() as u64)).unwrap_or(0);
        let end = start
            .saturating_add(usize::try_from(max_rows).unwrap_or(usize::MAX))
            .min(self.peaks.len());
        let o = |v: Option<f64>| v.unwrap_or(f64::NAN);
        let mut columns: Vec<Vec<f64>> = vec![Vec::with_capacity(end - start); info.columns.len()];
        for p in &self.peaks[start..end] {
            let vals = [
                cats.iter()
                    .position(|c| *c == p.signal)
                    .map_or(f64::NAN, |i| i as f64),
                f64::from(p.number),
                p.rt_min,
                p.start_min,
                p.end_min,
                p.area,
                p.height,
                p.baseline_start,
                p.baseline_end,
                o(p.capacity_factor),
                o(p.plates),
                o(p.plate_height),
                o(p.tailing),
                o(p.resolution),
                f64::from(p.flags),
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

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::unsupported(
            SHIMADZU_ID,
            "image planes",
            "Shimadzu LabSolutions files hold chromatograms: read them with `trace` (see `info` → traces[]).",
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
            return Err(Error::Usage(format!("trace {index} has one sweep (0)")));
        }
        let sig = self.signals.get(index as usize).cloned().ok_or_else(|| {
            Error::Usage(format!(
                "trace {index} does not exist; `info` → traces[] lists {}",
                self.signals.len()
            ))
        })?;
        let mut rows = self.rows_f64(index as usize)?;
        let divisor = if matches!(
            sig.kind,
            ShimadzuSignalKind::Chromatogram | ShimadzuSignalKind::Status
        ) {
            // stored values → the export's units (older layout: mV; status: bar, °C, …)
            sig.label
                .as_ref()
                .and_then(|l| l.scale)
                .map_or(1.0, |s| 1.0 / s)
        } else {
            PDA_PER_MAU
        };
        if matches!(
            sig.kind,
            ShimadzuSignalKind::Chromatogram | ShimadzuSignalKind::Status
        ) && let Some(first) = rows.first().cloned()
        {
            // the vendor's leading t = 0 point: the first reading repeated
            rows.insert(0, first);
        }

        let n = rows.first().map_or(1, Vec::len);
        let first = usize::try_from(first_sample.min(rows.len() as u64)).unwrap_or(0);
        let take = usize::try_from(max_samples)
            .unwrap_or(usize::MAX)
            .min(rows.len() - first);
        let mut channels = vec![Vec::with_capacity(take); n];
        for row in &rows[first..first + take] {
            for (c, v) in channels.iter_mut().zip(row) {
                c.push(*v / divisor);
            }
        }
        Ok(Trace {
            trace: index,
            sweep,
            first_sample: first as u64,
            channels,
        })
    }

    fn read_spectrum(&mut self, index: u32, spectrum: u64) -> Result<Spectrum> {
        use crate::shimadzu_tlm as t;
        let tl = self.tlm.as_ref().filter(|_| index == 0).ok_or_else(|| {
            Error::Usage(format!(
                "spectra run {index} not found (LC-MS data are run 0)"
            ))
        })?;
        let k = usize::try_from(spectrum).unwrap_or(usize::MAX);
        let entry = *tl.index.entries.get(k).ok_or_else(|| {
            Error::Usage(format!(
                "spectrum {spectrum} out of range ({} spectra)",
                tl.index.entries.len()
            ))
        })?;
        let record =
            t::inflate_record(&tl.raw, entry).map_err(|m| Error::corrupt(SHIMADZU_ID, m))?;
        let head =
            t::header(&record).ok_or_else(|| Error::corrupt(SHIMADZU_ID, "short LC-MS record"))?;
        if matches!(head.code, t::TLM_FULL_SCAN | t::TLM_PRODUCT_ION_SCAN) {
            return Err(Error::unsupported(
                SHIMADZU_ID,
                format!(
                    "{} spectra of LabSolutions LC-MS data",
                    t::acquisition_name(head.code)
                ),
                "The file stores these as profiles that the vendor software reports differently (full scans: a calibration the file does not state; product-ion scans: not checked). Convert the .lcd with ProteoWizard msconvert (vendor reader) and read the mzML; the TIC, retention time and precursor of these scans are available (`spectra`, `analyze chromatogram --tic`).",
            ));
        }
        let decoded = t::decode(&record).map_err(|m| Error::corrupt(SHIMADZU_ID, m))?;
        let mut extra = BTreeMap::new();
        extra.insert("acquisition".into(), json!(t::acquisition_name(head.code)));
        extra.insert("event".into(), json!(head.event));
        extra.insert("cycle".into(), json!(head.cycle));
        extra.insert("polarity_code".into(), json!(head.polarity_code));
        let (mz, intensity): (Vec<f64>, Vec<f32>) =
            decoded.points.iter().map(|(m, v)| (*m, *v as f32)).unzip();
        let base = decoded
            .points
            .iter()
            .copied()
            .fold(None, |b: Option<(f64, f64)>, p| match b {
                Some(q) if q.1 >= p.1 => Some(q),
                _ => Some(p),
            });
        Ok(Spectrum {
            index: spectrum,
            scan_number: spectrum + 1,
            ms_level: decoded.ms_level,
            rt_s: Some(f64::from(tl.index.rt_ms.get(k).copied().unwrap_or(head.rt_ms)) / 1000.0),
            polarity: "unknown".into(),
            centroided: decoded.centroided,
            precursor_mz: decoded.precursor_mz,
            precursor_charge: None,
            scan_filter: None,
            total_ion_current: tl.index.tic.get(k).map(|v| *v as f64),
            native_id: None,
            base_peak_mz: base.map(|b| b.0),
            base_peak_intensity: base.map(|b| b.1),
            precursor_intensity: None,
            isolation_window_mz: None,
            activation: None,
            collision_energy: None,
            inverse_reduced_mobility: None,
            scan_window_mz: decoded.scan_window_mz,
            extra,
            mz,
            intensity,
        })
    }

    fn visit_scan_headers(
        &mut self,
        run: u32,
        first: u64,
        visit: &mut dyn FnMut(openreadout_core::ScanHeader) -> bool,
    ) -> Result<bool> {
        use crate::shimadzu_tlm as t;
        let Some(tl) = self.tlm.as_ref().filter(|_| run == 0) else {
            return Ok(false);
        };
        for (k, h) in tl
            .headers
            .iter()
            .enumerate()
            .skip(usize::try_from(first).unwrap_or(usize::MAX))
        {
            let h = h
                .as_ref()
                .map_err(|m| Error::corrupt(SHIMADZU_ID, m.clone()))?;
            let mut extra = BTreeMap::new();
            extra.insert("acquisition".into(), json!(t::acquisition_name(h.code)));
            extra.insert("event".into(), json!(h.event));
            let sh = openreadout_core::ScanHeader {
                index: k as u64,
                scan_number: k as u64 + 1,
                ms_level: t::ms_level(h.code).unwrap_or(0),
                rt_s: Some(f64::from(tl.index.rt_ms.get(k).copied().unwrap_or(h.rt_ms)) / 1000.0),
                polarity: "unknown".into(),
                centroided: matches!(h.code, t::TLM_MRM | t::TLM_SIM),
                precursor_mz: h.precursor_mz(),
                total_ion_current: tl.index.tic.get(k).map(|v| *v as f64),
                extra,
                ..openreadout_core::ScanHeader::default()
            };
            if !visit(sh) {
                break;
            }
        }
        Ok(true)
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), SHIMADZU_ID);
        if let Some(tl) = &self.tlm {
            use crate::shimadzu_tlm as t;
            r.performed("LC-MS: every record inflated; its retention time equals the Retention Time stream; MRM/SIM/product-ion records decoded and their intensities summed to the TIC Data value");
            let (mut bad, mut tic_off, mut rt_off) = (Vec::new(), 0usize, 0usize);
            for (k, e) in tl.index.entries.iter().enumerate() {
                match t::inflate_record(&tl.raw, *e) {
                    Err(m) => bad.push(format!("record {k}: {m}")),
                    Ok(d) => {
                        let Some(h) = t::header(&d) else {
                            bad.push(format!("record {k}: short header"));
                            continue;
                        };
                        if tl.index.rt_ms.get(k) != Some(&h.rt_ms) {
                            rt_off += 1;
                        }
                        if matches!(h.code, t::TLM_FULL_SCAN | t::TLM_PRODUCT_ION_SCAN) {
                            continue;
                        }
                        match t::decode(&d) {
                            Err(m) => bad.push(format!("record {k}: {m}")),
                            Ok(s) => {
                                let sum: f64 = s.points.iter().map(|p| p.1).sum();
                                if tl.index.tic.get(k).map(|v| *v as f64) != Some(sum) {
                                    tic_off += 1;
                                }
                            }
                        }
                    }
                }
            }
            if !bad.is_empty() {
                r.push(Finding::error(
                    "bad_ms_record",
                    format!(
                        "{} LC-MS records do not decode: {}",
                        bad.len(),
                        bad.iter().take(3).cloned().collect::<Vec<_>>().join("; ")
                    ),
                ));
            }
            if tic_off > 0 {
                r.push(Finding::warning(
                    "ms_tic_mismatch",
                    format!("{tic_off} LC-MS records: the sum of the decoded intensities is not the stored TIC"),
                ));
            }
            if rt_off > 0 {
                r.push(Finding::warning(
                    "ms_rt_mismatch",
                    format!("{rt_off} LC-MS records: the record's retention time is not the Retention Time stream's"),
                ));
            }
        }
        r.performed("compound-file header, FAT/DIFAT, mini FAT and directory tree");
        r.performed("every stream readable through its sector chain to its declared size");
        r.performed("a `... Raw Data` storage is present");
        for p in &self.cfb.problems {
            r.push(Finding::error("bad_structure", p.clone()));
        }
        let mut f = self
            .fs
            .open(&self.path)
            .map_err(|e| Error::io(&self.path, e))?;
        let mut bad = 0;
        for e in self.cfb.entries.iter().filter(|e| e.is_stream) {
            if e.size > self.size {
                bad += 1;
                if bad <= 5 {
                    r.push(Finding::error(
                        "truncated",
                        format!(
                            "stream {} declares {} bytes, the file has {}",
                            e.path, e.size, self.size
                        ),
                    ));
                }
                continue;
            }
            if let Err(err) = self.cfb.read(&mut f, &self.path, e, MAX_STREAM) {
                bad += 1;
                if bad <= 5 {
                    r.push(Finding::error("truncated", err.to_string()));
                }
            }
        }
        if bad > 5 {
            r.push(Finding::error(
                "truncated",
                format!("{bad} streams unreadable in total"),
            ));
        }
        if !self.signals.is_empty() {
            r.performed("every signal stream decoded to its declared point count (block lengths and trailers agree); the PDA field's maximum over wavelengths, less the first spectrum, equals the stored max plot at every time point");
        }
        let mut max_plot: Option<Vec<i64>> = None;
        for i in 0..self.signals.len() {
            let sig = self.signals[i].clone();
            let rows = if sig.f64_values {
                self.rows_f64(i).map(|rows| {
                    rows.into_iter()
                        .map(|r| r.into_iter().map(|v| v as i64).collect::<Vec<i64>>())
                        .collect::<Vec<_>>()
                })
            } else {
                self.signal_values(i)
            };
            match rows {
                Err(e) => r.push(Finding::error("bad_signal", e.to_string())),
                Ok(rows) => {
                    if rows.len() as u64 != u64::from(sig.count) {
                        r.push(Finding::error(
                            "bad_signal",
                            format!(
                                "{}: {} points, the header says {}",
                                sig.stream,
                                rows.len(),
                                sig.count
                            ),
                        ));
                    }
                    match sig.kind {
                        ShimadzuSignalKind::MaxPlot => {
                            max_plot = Some(rows.iter().map(|v| v[0]).collect());
                        }
                        ShimadzuSignalKind::Pda => {
                            if let Some(mp) = &max_plot {
                                // LabSolutions takes the maximum after subtracting the
                                // first spectrum (docs/formats/shimadzu.md).
                                let first = rows.first().cloned().unwrap_or_default();
                                let bad = rows
                                    .iter()
                                    .zip(mp)
                                    .filter(|(row, m)| pda_max(row, &first) != Some(**m))
                                    .count();
                                if bad > 0 {
                                    r.push(Finding::error(
                                        "pda_max_plot_mismatch",
                                        format!("{bad} of {} spectra do not peak at the stored max-plot value", rows.len()),
                                    ));
                                }
                            }
                        }
                        ShimadzuSignalKind::Chromatogram | ShimadzuSignalKind::Status => {}
                    }
                }
            }
        }
        for e in &self.peak_errors {
            r.push(Finding::warning("vendor_peaks_unreadable", e.clone()));
        }
        if !self.peaks.is_empty() {
            r.performed("every vendor peak's area against our trace integrated between its start and end above its baseline (peaks above 1 % of the largest)");
            self.check_vendor_areas(&mut r);
        }
        if self.raw_storages().is_empty() {
            r.push(Finding::warning(
                "no_raw_data",
                "no storage named `... Raw Data`: this may not be a LabSolutions data file",
            ));
        }
        Ok(r)
    }
}

/// One PDA spectrum's maximum over wavelengths after subtracting the first spectrum: the value
/// LabSolutions stores in the max plot.
fn pda_max(row: &[i64], first: &[i64]) -> Option<i64> {
    row.iter()
        .zip(first)
        .map(|(v, f)| v.saturating_sub(*f))
        .max()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs() {
        let r = text_runs(b"\x1f\0\0\x003.00\0\0Admin\0ab\0", 3);
        assert_eq!(r, vec![(4, "3.00".to_string()), (10, "Admin".to_string())]);
        let f = property_fields(&[(
            0,
            "<a><szVersion>@StoX@352E3031</szVersion><n>1</n></a>".into(),
        )]);
        assert_eq!(
            f,
            vec![
                ("szVersion".into(), "5.01".into()),
                ("n".into(), "1".into())
            ]
        );
    }

    #[test]
    fn the_max_plot_is_taken_after_the_first_spectrum() {
        // lcd-tlm-sim: a first spectrum of 66 µAU at every wavelength and a max plot of 0
        let first = [66, 66, 66];
        assert_eq!(pda_max(&first, &first), Some(0));
        assert_eq!(pda_max(&[119, 80, 70], &first), Some(53));
        assert_eq!(pda_max(&[], &first), None);
    }
}
