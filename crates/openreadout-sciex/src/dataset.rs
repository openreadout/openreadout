//! An open Sciex `.wiff` (+ `.wiff.scan`): method, sample and scan index read from the compound
//! file at open, scan data read lazily from `.wiff.scan`.
//! Vocabulary and derivation: `docs/formats/sciex-wiff.md`, `docs/provenance/sciex-wiff.md`.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use openreadout_core::bytes::{le_f64, le_u32};
use openreadout_core::cfb::Cfb;
use openreadout_core::model::{
    CheckReport, FileInfo, Finding, InstrumentInfo, LsEntry, SignalChannelInfo, SpectraInfo,
    Spectrum, Trace, TraceInfo,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::source::{Fs, Input};
use openreadout_core::{Error, Plane, Result};
use serde_json::{Value, json};

use crate::layout::{
    DeviceChannel, ExperimentHeader, IndexRecord, MassRange, SAMPLE_BLOCK_HEADER,
    SAMPLE_BLOCK_MAGIC, SCAN_FILE_HEADER, SCAN_TYPE_ENHANCED_MS, SCAN_TYPE_ENHANCED_PRODUCT_ION,
    SCAN_TYPE_MRM, SCAN_TYPE_NEUTRAL_LOSS, SCAN_TYPE_PRECURSOR_ION, SCAN_TYPE_Q1, SCAN_TYPE_TOF_MS,
    SCAN_TYPE_TOF_PRODUCT, STREAM_PREAMBLE, decode_grid_scan, decode_tdc, dependent_charges,
    expand_zero_runs, index_trailing, log_fields, parse_device_channels, parse_device_data,
    parse_experiment_header, parse_index, parse_mass_ranges, parse_smrm_window_s, parse_windows,
    precursor_slot,
    sample_strings, tdc_step, tof_calibration, tof_default_calibration, tof_mz, utf16_runs,
};
use crate::{FORMAT_ID, SciexWiffReader};

/// Largest stream read whole from the compound file (the scan index is the largest).
const MAX_STREAM: u64 = 1 << 30;
/// Largest scan read from `.wiff.scan`.
const MAX_SCAN: u64 = 1 << 28;
/// Largest number of values one MRM cycle may expand to.
const MAX_ROW: usize = 1 << 22;

/// One experiment of the acquisition method (period 0).
#[derive(Debug, Clone, Default)]
pub struct Experiment {
    /// Zero-based experiment number (`ExperimentN`).
    pub number: u32,
    /// `ExperimentHeader`, when present.
    pub header: Option<ExperimentHeader>,
    /// Transitions or mass ranges (`MassRangeEx`).
    pub ranges: Vec<MassRange>,
    /// TDC bins per stored step of the experiment's TOF data (`ExperimentHeaderEx`).
    pub tdc_step: u64,
    /// Detection window (s) of a scheduled MRM experiment (`sMRM`), when it is scheduled.
    pub scheduled_window_s: Option<u32>,
}

impl Experiment {
    fn scan_type(&self) -> Option<u16> {
        self.header.map(|h| h.scan_type)
    }
    fn polarity(&self) -> &'static str {
        match self.header.map(|h| h.polarity) {
            Some(1) => "negative",
            Some(0) => "positive",
            _ => "unknown",
        }
    }
    fn kind(&self) -> &'static str {
        match self.scan_type() {
            Some(SCAN_TYPE_MRM) => "MRM",
            Some(SCAN_TYPE_TOF_MS) => "TOF MS",
            Some(SCAN_TYPE_TOF_PRODUCT) => "TOF product ion",
            Some(SCAN_TYPE_Q1) => "Q1 scan",
            Some(SCAN_TYPE_PRECURSOR_ION) => "precursor ion",
            Some(SCAN_TYPE_NEUTRAL_LOSS) => "neutral loss",
            Some(SCAN_TYPE_ENHANCED_PRODUCT_ION) => "enhanced product ion",
            Some(SCAN_TYPE_ENHANCED_MS) => "enhanced MS",
            Some(_) => "other",
            None => "unknown",
        }
    }
    fn ms_level(&self) -> u32 {
        match self.scan_type() {
            Some(SCAN_TYPE_TOF_MS | SCAN_TYPE_ENHANCED_MS | SCAN_TYPE_Q1) => 1,
            _ => 2,
        }
    }
}

/// What one spectrum of a sample is made of.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SpectrumRef {
    /// Transitions (indices into the experiment's ranges) sharing one Q1 in one cycle.
    Srm { record: u32, transitions: Vec<u32> },
    /// A TOF scan (one index record).
    Tof { record: u32 },
    /// A quadrupole or ion-trap scan stored as counts on an m/z grid (one index record).
    Grid { record: u32 },
}

impl SpectrumRef {
    fn record(&self) -> u32 {
        match self {
            SpectrumRef::Srm { record, .. }
            | SpectrumRef::Tof { record }
            | SpectrumRef::Grid { record } => *record,
        }
    }
}

/// One sample (`SampleSubtree/SampleN`): a run.
#[derive(Debug, Clone, Default)]
pub struct Sample {
    /// Sample number (1-based, `SampleN`).
    pub number: u32,
    /// The scan index.
    pub index: Vec<IndexRecord>,
    /// Bytes after the last whole index record.
    pub index_trailing: usize,
    /// Scheduled-MRM windows (start, end) in ms per transition.
    pub windows: Vec<(u32, u32)>,
    /// The windows come from the method (expected times and detection window), not the sample.
    pub windows_from_method: bool,
    /// Text of the `Log` stream.
    pub log: Option<String>,
    /// Strings of `SampleDABE/DATA` (sample name, id, comment, data file, method, …).
    pub strings: Vec<String>,
    /// Acquisition start (ISO-8601 local clock, with its UTC offset when the compound file gives it) from `SampleTable`.
    pub started_at: Option<String>,
    tof_calibration: Vec<u8>,
    tdc_width_ns: Option<f64>,
    precursors: Vec<u8>,
    /// Precursor charges of data-dependent scans by (cycle, dependent position)
    /// (`DDERealTimeDataEx`).
    charges: BTreeMap<(u32, u32), u16>,
    /// Single-file layout (no `.wiff.scan`): the sample's `Scan` stream inside the `.wiff`,
    /// as its sectors and size; index offsets count from its 32-byte preamble.
    scan_stream: Option<(Vec<u32>, u64)>,
    /// Where the sample's scan data start in `.wiff.scan`: after its block header, the samples'
    /// blocks following each other; `None` when the block header is not where the earlier
    /// samples' data end.
    scan_base: Option<u64>,
    spectra: Vec<SpectrumRef>,
    /// LC devices recorded with the sample: their channels and values (one per sample time).
    pub devices: Vec<Device>,
}

/// An LC device's channels and, per channel, its values.
pub type Device = (Vec<DeviceChannel>, Vec<Vec<f64>>);

/// An open `.wiff`.
#[derive(Debug)]
pub struct SciexDataset {
    path: PathBuf,
    /// Where the `.wiff` and `.wiff.scan` are read from.
    fs: Fs,
    opened: PathBuf,
    scan_path: Option<PathBuf>,
    scan_len: Option<u64>,
    size_bytes: u64,
    cfb: Cfb,
    experiments: Vec<Experiment>,
    periods: usize,
    samples: Vec<Sample>,
    software: Option<String>,
    method_path: Option<String>,
    notes: Vec<String>,
}

fn read_stream<R: Read + Seek>(cfb: &Cfb, f: &mut R, path: &Path, name: &str) -> Option<Vec<u8>> {
    let e = cfb.stream(name)?;
    if e.size > MAX_STREAM {
        return None;
    }
    cfb.read(f, path, e, MAX_STREAM).ok()
}

/// The `.wiff` a path names: the file itself, or the `.wiff` beside a `.wiff.scan` (name matched
/// case-insensitively: published pairs differ in case, e.g. `X_AQ.wiff` + `X_aq.wiff.scan`).
pub fn wiff_path(fs: &Fs, path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if name.to_ascii_lowercase().ends_with(".wiff.scan") {
        let wiff = &name[..name.len() - 5];
        return path
            .parent()
            .and_then(|dir| fs.find_in_dir(dir, wiff))
            .unwrap_or_else(|| path.with_file_name(wiff));
    }
    path.to_path_buf()
}

/// The `.wiff.scan` beside a `.wiff` (name matched case-insensitively).
fn scan_file(fs: &Fs, wiff: &Path) -> Option<PathBuf> {
    let want = format!("{}.scan", wiff.file_name()?.to_string_lossy());
    fs.find_in_dir(wiff.parent()?, &want)
        .filter(|p| fs.is_file(p))
}

/// The acquisition start from `SampleTable`'s u32: the acquisition computer's local clock stored
/// as seconds since 1970 as if it were UTC (`docs/provenance/sciex-wiff.md`, 2026-09-26). The
/// compound file's storages carry creation times in UTC ([MS-CFB]); the earliest is within a
/// second of the start, so their difference, rounded to a quarter hour, is the computer's UTC
/// offset. With it the time is written with that offset (`…+08:00`), without it as a local clock
/// with no zone.
fn local_start_to_iso(secs: u32, cfb: &Cfb) -> Option<String> {
    /// Seconds from 1601-01-01 (FILETIME) to 1970-01-01.
    const EPOCH_DIFF_SECS: i64 = 11_644_473_600;
    // 1990-01-01 .. 2100-01-01: anything else is not a plausible acquisition time
    if !(631_152_000..4_102_444_800).contains(&u64::from(secs)) {
        return None;
    }
    let clock = openreadout_core::time::unix_to_iso8601(i64::from(secs), 0);
    let clock = clock.strip_suffix('Z').unwrap_or(&clock).to_string();
    let created = cfb
        .entries
        .iter()
        .filter(|e| !e.is_stream && e.created > 0)
        .map(|e| i64::try_from(e.created / 10_000_000).unwrap_or(0) - EPOCH_DIFF_SECS)
        .filter(|s| (631_152_000..4_102_444_800).contains(s))
        .min();
    let Some(utc) = created else {
        return Some(clock);
    };
    let offset = i64::from(secs) - utc;
    let quarter = (offset + 450).div_euclid(900) * 900;
    if (offset - quarter).abs() > 120 || quarter.abs() > 14 * 3600 {
        return Some(clock);
    }
    let sign = if quarter < 0 { '-' } else { '+' };
    let q = quarter.abs();
    Some(format!(
        "{clock}{sign}{:02}:{:02}",
        q / 3600,
        (q % 3600) / 60
    ))
}

impl SciexDataset {
    /// Open a `.wiff` (or the `.wiff` beside a `.wiff.scan`).
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    /// Open an [`Input`]: a local `.wiff` (or `.wiff.scan`), or one held in memory or by the
    /// host, with its companion in the same namespace.
    pub fn open_input(input: &Input) -> Result<Self> {
        let fs = input.fs().clone();
        let opened = input.path().to_path_buf();
        let path = wiff_path(&fs, &opened);
        let mut f = fs.open(&path).map_err(|e| Error::io(&path, e))?;
        let size = f.size().map_err(|e| Error::io(&path, e))?;
        let cfb = Cfb::open(&mut f, &path, FORMAT_ID)?;
        let mut notes: Vec<String> = cfb
            .problems
            .iter()
            .map(|p| format!("compound file: {p}"))
            .collect();
        // Method: experiments of period 0.
        let base = "MethodSubtree/Method1/DeviceMethod0";
        let periods = cfb
            .entries
            .iter()
            .filter(|e| !e.is_stream)
            .filter_map(|e| e.path.strip_prefix(&format!("{base}/Period")))
            .filter(|rest| !rest.contains('/'))
            .count();
        let mut experiments = Vec::new();
        for n in 0..4096u32 {
            let dir = format!("{base}/Period0/Experiment{n}");
            if !cfb
                .entries
                .iter()
                .any(|e| e.path.eq_ignore_ascii_case(&dir))
            {
                break;
            }
            let header = read_stream(&cfb, &mut f, &path, &format!("{dir}/ExperimentHeader"))
                .and_then(|b| parse_experiment_header(&b));
            let expected = header.map_or(1 << 16, |h| h.range_count as usize);
            // Newer files: `MassRangeEx` (4th value: expected retention time of a scheduled
            // transition). Older Analyst files: `MassRange`, the same records with the dwell
            // time (ms) in that place.
            let ranges = match read_stream(
                &cfb,
                &mut f,
                &path,
                &format!("{dir}/MassRangeEx/MassRangeEx"),
            ) {
                Some(b) => parse_mass_ranges(&b, expected),
                None => read_stream(&cfb, &mut f, &path, &format!("{dir}/MassRange/MassRange"))
                    .map(|b| {
                        parse_mass_ranges(&b, expected)
                            .into_iter()
                            .map(|mut r| {
                                r.parameters.push(("dwell_ms".into(), r.expected_rt_min));
                                r.expected_rt_min = f32::NAN;
                                r
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            };
            if let Some(h) = header
                && ranges.len() != h.range_count as usize
            {
                notes.push(format!(
                    "experiment {n}: {} of {} mass ranges parsed",
                    ranges.len(),
                    h.range_count
                ));
            }
            let tdc_step = read_stream(&cfb, &mut f, &path, &format!("{dir}/ExperimentHeaderEx"))
                .map_or(1, |b| tdc_step(&b));
            let scheduled_window_s = read_stream(&cfb, &mut f, &path, &format!("{dir}/sMRM"))
                .and_then(|b| parse_smrm_window_s(&b));
            experiments.push(Experiment {
                number: n,
                header,
                ranges,
                tdc_step,
                scheduled_window_s,
            });
        }
        if periods > 1 {
            notes.push(format!(
                "{periods} periods in the method: only period 0's experiments are described; scans are assigned to experiments by position"
            ));
        }
        let software = read_stream(&cfb, &mut f, &path, "FileRec_Str").and_then(|b| {
            utf16_runs(&b, 4).into_iter().find_map(|s| {
                let i = s.find("Analyst").or_else(|| s.find("SCIEX OS"))?;
                let t = &s[i..];
                Some(t.split('&').next().unwrap_or(t).trim().to_string())
            })
        });
        let method_path = read_stream(
            &cfb,
            &mut f,
            &path,
            "MethodSubtree/Method1/AcqMethodFileInfoStm",
        )
        .and_then(|b| {
            utf16_runs(&b, 4).into_iter().next().map(|s| {
                let lower = s.to_ascii_lowercase();
                match lower.find(".dam") {
                    Some(i) => s[..i + 4].to_string(),
                    None => s,
                }
            })
        });
        let scan_path = scan_file(&fs, &path);
        let scan_len = scan_path
            .as_ref()
            .and_then(|p| fs.metadata(p).ok())
            .map(|m| m.len());
        // Without a `.wiff.scan`, older Analyst (QS) files keep each sample's scans in a `Scan`
        // stream inside the `.wiff`.
        let single_file = scan_path.is_none()
            && cfb
                .stream("SampleSubtree/Sample1/Scan")
                .is_some_and(|e| e.size > STREAM_PREAMBLE as u64);
        if single_file {
            notes.push(
                "single-file layout: the scans are read from each sample's Scan stream inside the .wiff (no .wiff.scan)".into(),
            );
        } else if scan_path.is_none() {
            notes.push(format!(
                "no {}.scan beside the .wiff: metadata and the scan index only, no spectra",
                path.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            ));
        }
        let sample_table = read_stream(&cfb, &mut f, &path, "SampleSubtree/SampleTable");
        let mut samples = Vec::new();
        for n in 1..=4096u32 {
            let dir = format!("SampleSubtree/Sample{n}");
            if !cfb
                .entries
                .iter()
                .any(|e| e.path.eq_ignore_ascii_case(&dir))
            {
                break;
            }
            let idx = read_stream(&cfb, &mut f, &path, &format!("{dir}/Idx")).unwrap_or_default();
            let index = parse_index(&idx);
            let index_trailing = index_trailing(idx.len());
            if index_trailing != 0 {
                notes.push(format!(
                    "sample {n}: the scan index ends {index_trailing} bytes into a record (truncated); {} whole records are used",
                    index.len()
                ));
            }
            let windows = read_stream(
                &cfb,
                &mut f,
                &path,
                &format!("{dir}/SampleDAM/sMRMPro_adw1/sMRMPro_adw_Times"),
            )
            .map(|b| parse_windows(&b))
            .filter(|w| !w.is_empty());
            let windows_from_method = windows.is_none() && !derived_windows(&experiments).is_empty();
            let windows = windows.unwrap_or_else(|| derived_windows(&experiments));
            let log = read_stream(&cfb, &mut f, &path, &format!("{dir}/Log"))
                .map(|b| utf16_runs(&b, 2).join("\n"));
            let strings = read_stream(&cfb, &mut f, &path, &format!("{dir}/SampleDABE/DATA"))
                .map(|b| sample_strings(&b))
                .unwrap_or_default();
            let started_at = (n == 1)
                .then(|| sample_table.as_deref().and_then(|t| le_u32(t, 0x3E)))
                .flatten()
                .and_then(|s| local_start_to_iso(s, &cfb));
            let tof_calibration =
                read_stream(&cfb, &mut f, &path, &format!("{dir}/TOFCalibrationData"))
                    .unwrap_or_default();
            let tdc_width_ns = read_stream(&cfb, &mut f, &path, &format!("{dir}/TDCInfo"))
                .and_then(|b| le_f64(&b, 32))
                .filter(|w| w.is_finite() && *w > 0.0);
            let precursors = read_stream(&cfb, &mut f, &path, &format!("{dir}/DDERealTimeData"))
                .unwrap_or_default();
            let charges = read_stream(&cfb, &mut f, &path, &format!("{dir}/DDERealTimeDataEx"))
                .map(|b| dependent_charges(&b).into_iter().collect())
                .unwrap_or_default();
            let scan_stream = if single_file {
                match cfb.stream(&format!("{dir}/Scan")) {
                    Some(e) => cfb.sectors(e)?.map(|sectors| (sectors, e.size)),
                    None => None,
                }
            } else {
                None
            };
            let mut devices = Vec::new();
            for k in 0..64u32 {
                let ddir = format!("{dir}/Devices/Device_{k}");
                let Some(ch) = read_stream(&cfb, &mut f, &path, &format!("{ddir}/Channel")) else {
                    break;
                };
                let channels = parse_device_channels(&ch);
                let data = read_stream(&cfb, &mut f, &path, &format!("{ddir}/DevData"))
                    .map(|b| parse_device_data(&b, channels.len()))
                    .unwrap_or_default();
                if !channels.is_empty() && data.first().is_some_and(|c| !c.is_empty()) {
                    devices.push((channels, data));
                }
            }
            let mut s = Sample {
                number: n,
                index,
                index_trailing,
                windows,
                windows_from_method,
                log,
                strings,
                started_at,
                tof_calibration,
                tdc_width_ns,
                precursors,
                charges,
                scan_stream,
                scan_base: None,
                spectra: Vec::new(),
                devices,
            };
            let have_scan = scan_path.is_some() || s.scan_stream.is_some();
            s.spectra = spectrum_refs(&s, &experiments, have_scan);
            samples.push(s);
        }
        // `.wiff.scan` holds one block per sample: a 24-byte header, then the scans its index
        // points to (offsets count from the end of that header).
        if let Some(sp) = &scan_path
            && let Ok(mut h) = fs.open(sp)
        {
            let mut pos = SCAN_FILE_HEADER - SAMPLE_BLOCK_HEADER;
            let mut ok = true;
            for s in &mut samples {
                let mut head = [0u8; 12];
                ok = ok
                    && h.seek(SeekFrom::Start(pos)).is_ok()
                    && h.read_exact(&mut head).is_ok()
                    && le_u32(&head, 0) == Some(SAMPLE_BLOCK_MAGIC)
                    && le_u32(&head, 8) == Some(s.number);
                s.scan_base = ok.then_some(pos + SAMPLE_BLOCK_HEADER);
                let data = s
                    .index
                    .iter()
                    .map(|r| u64::from(r.offset) + u64::from(r.byte_len))
                    .max()
                    .unwrap_or(0);
                pos = pos + SAMPLE_BLOCK_HEADER + data;
            }
            // Files whose first block has no header keep the fixed start of the scan data.
            if let Some(first) = samples.first_mut()
                && first.scan_base.is_none()
            {
                first.scan_base = Some(SCAN_FILE_HEADER);
            }
        }
        if samples.is_empty() {
            return Err(Error::corrupt(
                FORMAT_ID,
                "no SampleSubtree/Sample1 storage: not a WIFF data file",
            ));
        }
        let unsupported: BTreeSet<&str> = experiments
            .iter()
            .filter(|e| {
                !matches!(
                    e.scan_type(),
                    Some(
                        SCAN_TYPE_MRM
                            | SCAN_TYPE_TOF_MS
                            | SCAN_TYPE_TOF_PRODUCT
                            | SCAN_TYPE_Q1
                            | SCAN_TYPE_PRECURSOR_ION
                            | SCAN_TYPE_NEUTRAL_LOSS
                            | SCAN_TYPE_ENHANCED_PRODUCT_ION
                            | SCAN_TYPE_ENHANCED_MS
                    )
                )
            })
            .map(Experiment::kind)
            .collect();
        if !unsupported.is_empty() {
            notes.push(format!(
                "experiments of scan type(s) {} are listed but their scans are not decoded",
                unsupported.into_iter().collect::<Vec<_>>().join(", ")
            ));
        }
        let size_bytes = size + scan_len.unwrap_or(0);
        Ok(SciexDataset {
            path,
            fs,
            opened,
            scan_path,
            scan_len,
            size_bytes,
            cfb,
            experiments,
            periods,
            samples,
            software,
            method_path,
            notes,
        })
    }

    fn experiment_of(&self, record: u32) -> Option<(u32, &Experiment)> {
        let n = self.experiments.len().max(1) as u32;
        let e = self.experiments.get((record % n) as usize)?;
        Some((record / n + 1, e))
    }

    fn sample(&self, run: u32) -> Result<&Sample> {
        self.samples.get(run as usize).ok_or_else(|| {
            Error::Usage(format!(
                "run {run} does not exist ({} samples)",
                self.samples.len()
            ))
        })
    }

    fn read_scan_bytes(&self, s: &Sample, rec: &IndexRecord) -> Result<Vec<u8>> {
        let len = u64::from(rec.byte_len);
        if let (None, Some((sectors, size))) = (&self.scan_path, &s.scan_stream) {
            if len > MAX_SCAN {
                return Err(Error::unsupported(
                    FORMAT_ID,
                    format!("a scan of {len} bytes"),
                    "Scans above 256 MiB are not read.",
                ));
            }
            let start = STREAM_PREAMBLE as u64 + u64::from(rec.offset);
            let mut f = self
                .fs
                .open(&self.path)
                .map_err(|e| Error::io(&self.path, e))?;
            return self
                .cfb
                .read_range(&mut f, &self.path, sectors, *size, start, len);
        }
        let path = self.scan_path.as_ref().ok_or_else(|| {
            Error::unsupported(
                FORMAT_ID,
                "spectra without the .wiff.scan file",
                "Put the .wiff.scan that belongs to this .wiff beside it (same name plus .scan).",
            )
        })?;
        let len = u64::from(rec.byte_len);
        if len > MAX_SCAN {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!("a scan of {len} bytes"),
                "Scans above 256 MiB are not read.",
            ));
        }
        let base = s.scan_base.ok_or_else(|| {
            Error::unsupported(
                FORMAT_ID,
                format!("the scans of sample {} in the .wiff.scan", s.number),
                "The sample's block in the .wiff.scan is not where the earlier samples' data end; read sample 1, or export mzML from the vendor software.",
            )
        })?;
        let start = base + u64::from(rec.offset);
        let file_len = self.scan_len.unwrap_or(0);
        if start + len > file_len {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                start,
                format!(
                    "scan data of {len} bytes at {start} runs past the end of {} ({file_len} bytes; truncated)",
                    path.display()
                ),
            ));
        }
        let mut f = self.fs.open(path).map_err(|e| Error::io(path, e))?;
        f.seek(SeekFrom::Start(start))
            .map_err(|e| Error::io(path, e))?;
        let mut buf = vec![0u8; len as usize];
        f.read_exact(&mut buf).map_err(|e| Error::io(path, e))?;
        Ok(buf)
    }

    fn spectrum_of(&self, run: u32, index: u64) -> Result<Spectrum> {
        self.spectrum_or_header(run, index, true)
    }

    /// Spectrum `index` of run `run`; with `decode` false only its header: nothing is read from
    /// the `.wiff.scan` (no peaks; an MRM group's TIC and base peak are left out, a TOF scan's
    /// come from the index record).
    fn spectrum_or_header(&self, run: u32, index: u64, decode: bool) -> Result<Spectrum> {
        let s = self.sample(run)?;
        let r = usize::try_from(index)
            .ok()
            .and_then(|i| s.spectra.get(i))
            .ok_or_else(|| {
                Error::Usage(format!(
                    "spectrum {index} does not exist ({} spectra in run {run})",
                    s.spectra.len()
                ))
            })?;
        let record = r.record();
        let rec = s
            .index
            .get(record as usize)
            .ok_or_else(|| Error::corrupt(FORMAT_ID, format!("index record {record} missing")))?;
        let (cycle, exp) = self
            .experiment_of(record)
            .ok_or_else(|| Error::corrupt(FORMAT_ID, "no experiment described in the method"))?;
        let bytes = if decode {
            self.read_scan_bytes(s, rec)?
        } else {
            Vec::new()
        };
        let mut extra = BTreeMap::new();
        extra.insert("cycle".into(), json!(cycle));
        extra.insert("experiment".into(), json!(exp.number + 1));
        extra.insert("scan_type".into(), json!(exp.kind()));
        extra.insert("index_record".into(), json!(record));
        let native = format!(
            "sample={} period=1 cycle={cycle} experiment={}",
            s.number,
            exp.number + 1
        );
        let mut sp = Spectrum {
            index,
            // MRM groups share their cycle's index record: number spectra by position instead.
            scan_number: index + 1,
            ms_level: exp.ms_level(),
            rt_s: Some(rec.time_ms / 1000.0),
            polarity: exp.polarity().into(),
            centroided: false,
            ..Spectrum::default()
        };
        match r {
            SpectrumRef::Srm { transitions, .. } => {
                let n = exp.ranges.len();
                let row = if decode {
                    expand_zero_runs(&bytes, (2 * n).min(MAX_ROW)).map_err(|e| {
                        Error::corrupt_at(FORMAT_ID, SCAN_FILE_HEADER + u64::from(rec.offset), e)
                    })?
                } else {
                    vec![0.0; 2 * n]
                };
                // A cycle holds the transitions' values after as many leading values (zero in
                // every corpus file: QTRAP 6500/6500+, Analyst 1.6-1.7), or (older Analyst
                // files) the transitions' values alone.
                let base = if row.len() == 2 * n {
                    n
                } else if row.len() == n {
                    0
                } else {
                    return Err(Error::corrupt_at(
                        FORMAT_ID,
                        SCAN_FILE_HEADER + u64::from(rec.offset),
                        format!(
                            "MRM cycle holds {} values, the method lists {n} transitions (expected {n} or {})",
                            row.len(),
                            2 * n
                        ),
                    ));
                };
                let mut pts: Vec<(f64, f32, u32)> = transitions
                    .iter()
                    .map(|&t| {
                        let m = &exp.ranges[t as usize];
                        (f64::from(m.second_mz), row[base + t as usize], t)
                    })
                    .collect();
                pts.sort_by(|a, b| a.0.total_cmp(&b.0));
                let first = &exp.ranges[transitions[0] as usize];
                let ces: BTreeSet<u32> = transitions
                    .iter()
                    .filter_map(|&t| exp.ranges[t as usize].parameter("CE"))
                    .map(f32::to_bits)
                    .collect();
                sp.precursor_mz = Some(f64::from(first.first_mz));
                sp.activation = Some("CID".into());
                sp.collision_energy = (ces.len() == 1)
                    .then(|| first.parameter("CE").map(f64::from))
                    .flatten();
                sp.centroided = true;
                sp.native_id = Some(format!("{native} transition={}", transitions[0]));
                extra.insert("transitions".into(), json!(transitions));
                extra.insert(
                    "compounds".into(),
                    json!(
                        pts.iter()
                            .map(|p| exp.ranges[p.2 as usize].name.clone())
                            .collect::<Vec<_>>()
                    ),
                );
                if ces.len() > 1 {
                    extra.insert(
                        "collision_energies".into(),
                        json!(
                            pts.iter()
                                .map(|p| exp.ranges[p.2 as usize].parameter("CE"))
                                .collect::<Vec<_>>()
                        ),
                    );
                }
                if decode {
                    let tic: f64 = pts.iter().map(|p| f64::from(p.1)).sum();
                    sp.total_ion_current = Some(tic);
                    if let Some(b) = pts.iter().max_by(|a, b| a.1.total_cmp(&b.1)) {
                        sp.base_peak_mz = Some(b.0);
                        sp.base_peak_intensity = Some(f64::from(b.1));
                    }
                    sp.mz = pts.iter().map(|p| p.0).collect();
                    sp.intensity = pts.iter().map(|p| p.1).collect();
                }
            }
            SpectrumRef::Tof { .. } => {
                let width = s.tdc_width_ns.ok_or_else(|| {
                    Error::unsupported(
                        FORMAT_ID,
                        "TOF data without TDCInfo",
                        "The bin width (TDCInfo stream) is missing, so m/z cannot be computed.",
                    )
                })?;
                let (a, t0) = tof_calibration(&s.tof_calibration, record as usize)
                    .or_else(|| tof_default_calibration(&s.tof_calibration))
                    .ok_or_else(|| {
                        Error::unsupported(
                            FORMAT_ID,
                            "TOF data without a calibration",
                            "The TOFCalibrationData stream holds no calibration for this scan.",
                        )
                    })?;
                let st = exp.tdc_step.max(1);
                let bins = if decode {
                    decode_tdc(&bytes, st).map_err(|e| {
                        Error::corrupt_at(FORMAT_ID, SCAN_FILE_HEADER + u64::from(rec.offset), e)
                    })?
                } else {
                    Vec::new()
                };
                // Non-empty steps with the empty step on each side of a run, so the profile
                // keeps its shape.
                let mut mz = Vec::with_capacity(bins.len() * 2);
                let mut it = Vec::with_capacity(bins.len() * 2);
                let mut last: Option<u64> = None;
                for (k, &(bin, count)) in bins.iter().enumerate() {
                    if bin >= st && last.is_none_or(|l| l < bin - st) {
                        mz.push(tof_mz((bin - st) as f64, width, a, t0));
                        it.push(0.0);
                    }
                    mz.push(tof_mz(bin as f64, width, a, t0));
                    it.push(count as f32);
                    last = Some(bin);
                    let next = bins.get(k + 1).map(|b| b.0);
                    if next.is_none_or(|n| n > bin + st) {
                        mz.push(tof_mz((bin + st) as f64, width, a, t0));
                        it.push(0.0);
                        last = Some(bin + st);
                    }
                }
                sp.total_ion_current = Some(rec.tic);
                if rec.base_peak_x > 0.0 {
                    sp.base_peak_mz = Some(tof_mz(rec.base_peak_x, width, a, t0));
                    sp.base_peak_intensity = Some(rec.base_peak_intensity);
                } else if let Some(&(bin, count)) = bins.iter().rev().max_by_key(|&&(_, c)| c) {
                    // no stored position (some product-ion scans): the first bin with the
                    // largest count
                    sp.base_peak_mz = Some(tof_mz(bin as f64, width, a, t0));
                    sp.base_peak_intensity = Some(f64::from(count));
                }
                if exp.scan_type() == Some(SCAN_TYPE_TOF_PRODUCT) {
                    let products: Vec<u32> = self
                        .experiments
                        .iter()
                        .filter(|e| e.scan_type() == Some(SCAN_TYPE_TOF_PRODUCT))
                        .map(|e| e.number)
                        .collect();
                    if let Some(pos) = products.iter().position(|&n| n == exp.number) {
                        let slot = (cycle as usize - 1) * products.len() + pos;
                        if let Some((p, v)) = precursor_slot(&s.precursors, slot) {
                            sp.precursor_mz = Some(p);
                            extra.insert("precursor_slot_value".into(), json!(v));
                        }
                    }
                    // Without data-dependent precursors (SWATH), the experiment's fixed m/z is
                    // its isolation window's centre.
                    if s.precursors.is_empty()
                        && let Some(m) = exp.header.and_then(|h| h.fixed_mz).filter(|&m| m > 0.0)
                    {
                        sp.precursor_mz = Some(m);
                        extra.insert("data_independent".into(), json!(true));
                    }
                    // No field states it: the exports label QSTAR (Analyst QS) product ions
                    // collision-induced dissociation and TripleTOF ones beam-type CID.
                    sp.activation = Some(
                        if self
                            .software
                            .as_deref()
                            .is_some_and(|sw| sw.starts_with("Analyst QS"))
                        {
                            "CID"
                        } else {
                            "HCD"
                        }
                        .into(),
                    );
                    // A product-ion experiment whose method CE is 0 uses a rolling (per
                    // precursor) collision energy that is not stored per scan: none is reported.
                    sp.collision_energy = exp
                        .ranges
                        .first()
                        .and_then(|m| m.parameter("CE"))
                        .map(f64::from)
                        .filter(|&ce| ce != 0.0);
                }
                extra.insert(
                    "calibration".into(),
                    json!({"a": a, "t0": t0, "bin_width_ns": width}),
                );
                sp.native_id = Some(native);
                sp.mz = mz;
                sp.intensity = it;
            }
            SpectrumRef::Grid { .. } => {
                self.fill_grid(s, rec, exp, cycle, &bytes, decode, &mut sp, &mut extra)?;
                sp.native_id = Some(native);
            }
        }
        sp.extra = extra;
        Ok(sp)
    }

    /// Fill a grid scan (precursor ion, neutral loss, enhanced MS, enhanced product ion) into
    /// `sp`: the counts on the scan's m/z grid times their segment's scale, with the empty step on
    /// either side of every run of points; precursor, fixed m/z and collision energy from the
    /// method and `DDERealTimeData`.
    #[allow(clippy::too_many_arguments)]
    fn fill_grid(
        &self,
        s: &Sample,
        rec: &IndexRecord,
        exp: &Experiment,
        cycle: u32,
        bytes: &[u8],
        decode: bool,
        sp: &mut Spectrum,
        extra: &mut BTreeMap<String, Value>,
    ) -> Result<()> {
        let st = exp.scan_type();
        let fixed = exp.header.and_then(|h| h.fixed_mz).filter(|&m| m > 0.0);
        match st {
            // The exports give a precursor-ion scan's fixed product as its selected ion.
            Some(SCAN_TYPE_PRECURSOR_ION) => {
                sp.precursor_mz = fixed;
                if let Some(m) = fixed {
                    extra.insert("product_mz".into(), json!(m));
                }
            }
            Some(SCAN_TYPE_NEUTRAL_LOSS) => {
                if let Some(m) = fixed {
                    extra.insert("neutral_loss_mz".into(), json!(m));
                }
            }
            Some(SCAN_TYPE_ENHANCED_PRODUCT_ION) => {
                let dependent: Vec<u32> = self
                    .experiments
                    .iter()
                    .filter(|e| e.scan_type() == Some(SCAN_TYPE_ENHANCED_PRODUCT_ION))
                    .map(|e| e.number)
                    .collect();
                if let Some(pos) = dependent.iter().position(|&n| n == exp.number) {
                    let slot = (cycle as usize - 1) * dependent.len() + pos;
                    if let Some((p, _)) = precursor_slot(&s.precursors, slot) {
                        sp.precursor_mz = Some(p);
                    }
                    sp.precursor_charge = s
                        .charges
                        .get(&(cycle, pos as u32 + 1))
                        .map(|&z| i32::from(z));
                }
            }
            _ => {}
        }
        if sp.ms_level >= 2 {
            // Beam-type CID in the collision cell, as the exports label these scans.
            sp.activation = Some("HCD".into());
            let first = exp.ranges.first();
            sp.collision_energy = first
                .and_then(|m| m.parameter("CE"))
                .map(f64::from)
                .filter(|&ce| ce != 0.0);
            if let Some(spread) = first.and_then(|m| m.parameter("CES")).filter(|&v| v != 0.0) {
                extra.insert("collision_energy_spread".into(), json!(spread));
            }
        }
        sp.total_ion_current = Some(rec.tic);
        if !decode {
            return Ok(());
        }
        let g = decode_grid_scan(bytes).map_err(|e| {
            Error::corrupt_at(FORMAT_ID, SCAN_FILE_HEADER + u64::from(rec.offset), e)
        })?;
        // The grid's range is in the scan itself, which a header read does not open: it goes
        // in `extra`, so headers and spectra agree on `scan_window_mz` (not set).
        if let (Some(a), Some(b)) = (g.segments.first(), g.segments.last()) {
            extra.insert("grid_mz".into(), json!([a.first, b.end]));
        }
        // Empty steps between runs of points are kept (one on each side of a run); the
        // vendor library leaves out the one before the first point and after the last.
        let mut mz = Vec::with_capacity(g.points.len() * 2);
        let mut it = Vec::with_capacity(g.points.len() * 2);
        let mut last: Option<u64> = None;
        let mut base: Option<(f64, f64)> = None;
        for (i, &(k, count)) in g.points.iter().enumerate() {
            if last.is_some_and(|l| l + 1 < k)
                && let Some((m, _)) = g.at(k - 1)
            {
                mz.push(m);
                it.push(0.0);
            }
            let (m, scale) = g.at(k).ok_or_else(|| {
                Error::corrupt_at(
                    FORMAT_ID,
                    SCAN_FILE_HEADER + u64::from(rec.offset),
                    format!("point at step {k} lies past the grid"),
                )
            })?;
            let v = f64::from(count) * scale;
            if base.is_none_or(|b| v > b.1) {
                base = Some((m, v));
            }
            mz.push(m);
            it.push(v as f32);
            last = Some(k);
            let next = g.points.get(i + 1).map(|p| p.0);
            if next.is_some_and(|n| n > k + 1)
                && let Some((m, _)) = g.at(k + 1)
            {
                mz.push(m);
                it.push(0.0);
                last = Some(k + 1);
            }
        }
        sp.base_peak_mz = base.map(|b| b.0);
        sp.base_peak_intensity = base.map(|b| b.1);
        sp.mz = mz;
        sp.intensity = it;
        Ok(())
    }

    fn log_value(&self, s: &Sample, key: &str) -> Option<String> {
        let log = s.log.as_deref()?;
        log_fields(log)
            .into_iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v)
    }

    fn sample_string(s: &Sample, i: usize) -> Option<String> {
        s.strings
            .get(i)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty() && v != "N/A")
    }

    fn spectra_info(&self, s: &Sample) -> SpectraInfo {
        let times: Vec<f64> = s
            .spectra
            .iter()
            .filter_map(|r| s.index.get(r.record() as usize))
            .map(|r| r.time_ms / 1000.0)
            .collect();
        let rt_range_s = if times.is_empty() {
            None
        } else {
            Some([
                times.iter().copied().fold(f64::INFINITY, f64::min),
                times.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            ])
        };
        let levels: BTreeSet<u32> = s
            .spectra
            .iter()
            .filter_map(|r| self.experiment_of(r.record()))
            .map(|(_, e)| e.ms_level())
            .collect();
        let mut extra = BTreeMap::new();
        let mut put = |k: &str, v: Option<String>| {
            if let Some(v) = v {
                extra.insert(k.to_string(), Value::from(v));
            }
        };
        put("sample_name", Self::sample_string(s, 0));
        put("sample_id", Self::sample_string(s, 1));
        put("acquired_at", s.started_at.clone());
        put("instrument_serial", self.log_value(s, "Serial Number"));
        put("method", self.method_path.clone());
        let pols: BTreeSet<&str> = self.experiments.iter().map(Experiment::polarity).collect();
        extra.insert("polarities".into(), json!(pols));
        extra.insert(
            "experiments".into(),
            json!(
                self.experiments
                    .iter()
                    .map(|e| json!({
                        "experiment": e.number + 1,
                        "scan_type": e.kind(),
                        "polarity": e.polarity(),
                        "mass_ranges": e.ranges.len(),
                    }))
                    .collect::<Vec<_>>()
            ),
        );
        extra.insert(
            "cycles".into(),
            json!(s.index.len() / self.experiments.len().max(1)),
        );
        if s.windows_from_method {
            extra.insert("scheduled_windows".into(), json!("from the method"));
        }
        extra.insert(
            "stored_spectra".into(),
            json!(if self
                .experiments
                .iter()
                .all(|e| e.scan_type() == Some(SCAN_TYPE_MRM))
            {
                "SRM"
            } else {
                "profile"
            }),
        );
        extra.insert(
            "method_summary".into(),
            json!({
                "instrument_method": self.method_path,
                "experiments": self.experiments.iter().map(Experiment::kind).collect::<Vec<_>>(),
            }),
        );
        let (software, software_version) = match self.software.as_deref() {
            Some(sw) => match sw.rsplit_once(' ') {
                Some((name, v)) if v.starts_with(|c: char| c.is_ascii_digit()) => {
                    (Some(name.to_string()), Some(v.to_string()))
                }
                _ => (Some(sw.to_string()), None),
            },
            None => (None, None),
        };
        SpectraInfo {
            index: s.number - 1,
            name: Self::sample_string(s, 0).or_else(|| {
                self.path
                    .file_stem()
                    .map(|n| n.to_string_lossy().into_owned())
            }),
            scan_count: s.spectra.len() as u64,
            ms_levels: levels.into_iter().collect(),
            rt_range_s,
            instrument: Some(InstrumentInfo {
                manufacturer: Some("SCIEX".into()),
                model: self.log_value(s, "Component ID"),
                software,
                software_version,
                detector: None,
            }),
            extra,
        }
    }

    fn trace_infos(&self) -> Vec<TraceInfo> {
        let chan = |i: u32, name: &str, unit: &str| SignalChannelInfo {
            index: i,
            name: name.into(),
            unit: Some(unit.into()),
            dtype: "float64".into(),
            scale: 1.0,
            offset: 0.0,
            extra: BTreeMap::new(),
        };
        let mut out = Vec::new();
        for s in &self.samples {
            for (k, name) in ["TIC", "BPC"].iter().enumerate() {
                let mut extra = BTreeMap::new();
                extra.insert("sample".into(), json!(s.number));
                extra.insert(
                    "kind".into(),
                    json!(if k == 0 {
                        "total ion current"
                    } else {
                        "base peak"
                    }),
                );
                extra.insert(
                    "source".into(),
                    json!("the scan index (Idx), one point per non-empty scan"),
                );
                extra.insert("irregular_sampling".into(), json!(true));
                out.push(TraceInfo {
                    index: out.len() as u32,
                    name: Some(if self.samples.len() > 1 {
                        format!("{name} (sample {})", s.number)
                    } else {
                        (*name).to_string()
                    }),
                    sample_rate_hz: 0.0,
                    sample_count: self.trace_rows(s).len() as u64,
                    sweep_count: 1,
                    channels: vec![chan(0, "time", "s"), chan(1, "intensity", "counts")],
                    start_s: self.trace_rows(s).first().map(|r| r.time_ms / 1000.0),
                    extra,
                });
            }
        }
        for (s, d, (channels, data)) in self.device_traces() {
            let mut extra = BTreeMap::new();
            extra.insert("sample".into(), json!(s));
            extra.insert("device".into(), json!(d));
            extra.insert(
                "source".into(),
                json!(format!(
                    "SampleSubtree/Sample{s}/Devices/Device_{d} (Channel, DevData)"
                )),
            );
            out.push(TraceInfo {
                index: out.len() as u32,
                name: Some(format!(
                    "LC device {}: {}{}",
                    d + 1,
                    channels
                        .iter()
                        .map(|c| c.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                    if self.samples.len() > 1 {
                        format!(" (sample {s})")
                    } else {
                        String::new()
                    }
                )),
                sample_rate_hz: channels[0].rate_hz,
                sample_count: data[0].len() as u64,
                sweep_count: 1,
                channels: channels
                    .iter()
                    .enumerate()
                    .map(|(i, c)| SignalChannelInfo {
                        index: i as u32,
                        name: c.name.clone(),
                        unit: c.unit.clone(),
                        dtype: "float64".into(),
                        scale: 1.0,
                        offset: 0.0,
                        extra: BTreeMap::new(),
                    })
                    .collect(),
                start_s: Some(0.0),
                extra,
            });
        }
        out
    }

    /// The device traces (sample number, device number, channels and values): devices whose
    /// channels share one positive sampling rate.
    fn device_traces(&self) -> Vec<(u32, usize, &Device)> {
        let mut out = Vec::new();
        for s in &self.samples {
            for (d, dev) in s.devices.iter().enumerate() {
                let r = dev.0[0].rate_hz;
                if r.is_finite()
                    && r > 0.0
                    && dev.0.iter().all(|c| c.rate_hz.to_bits() == r.to_bits())
                {
                    out.push((s.number, d, dev));
                }
            }
        }
        out
    }

    /// Index records a TIC/BPC trace holds: every record with data (MRM: every cycle).
    fn trace_rows<'a>(&self, s: &'a Sample) -> Vec<&'a IndexRecord> {
        s.index.iter().filter(|r| r.byte_len > 0).collect()
    }

    /// Base-peak intensity of an index record: the stored value, or for MRM cycles (which store
    /// none) the largest transition value of the cycle.
    fn base_peak_of(&self, s: &Sample, record: u32, r: &IndexRecord) -> Result<f64> {
        if r.base_peak_intensity != 0.0 || r.tic == 0.0 {
            return Ok(r.base_peak_intensity);
        }
        match self.experiment_of(record) {
            Some((_, e))
                if matches!(
                    e.scan_type(),
                    Some(
                        SCAN_TYPE_Q1
                            | SCAN_TYPE_PRECURSOR_ION
                            | SCAN_TYPE_NEUTRAL_LOSS
                            | SCAN_TYPE_ENHANCED_PRODUCT_ION
                            | SCAN_TYPE_ENHANCED_MS
                    )
                ) =>
            {
                let bytes = self.read_scan_bytes(s, r)?;
                let g = decode_grid_scan(&bytes).map_err(|err| {
                    Error::corrupt_at(FORMAT_ID, SCAN_FILE_HEADER + u64::from(r.offset), err)
                })?;
                Ok(g.points
                    .iter()
                    .filter_map(|&(k, c)| g.at(k).map(|(_, scale)| f64::from(c) * scale))
                    .fold(0.0, f64::max))
            }
            Some((_, e)) if e.scan_type() == Some(SCAN_TYPE_MRM) => {
                let bytes = self.read_scan_bytes(s, r)?;
                let row =
                    expand_zero_runs(&bytes, (2 * e.ranges.len()).min(MAX_ROW)).map_err(|err| {
                        Error::corrupt_at(FORMAT_ID, SCAN_FILE_HEADER + u64::from(r.offset), err)
                    })?;
                Ok(row.iter().copied().fold(0.0f32, f32::max).into())
            }
            _ => Ok(r.base_peak_intensity),
        }
    }
}

/// Windows of a scheduled MRM experiment whose sample stores none (`sMRMPro_adw_Times` absent):
/// each transition's expected retention time ± half the method's detection window, in ms
/// (docs/provenance/sciex-wiff.md, 2026-10-06); a transition whose expected time is 0 is not
/// scheduled and is active throughout. Empty when the first experiment is not a scheduled MRM
/// experiment. Analyst judges a window at each transition's own time within the cycle, which the
/// file does not give us, so a cycle at a window's edge may be kept or left out differently.
#[allow(clippy::float_cmp)] // 0.0 is the stored value of an unscheduled transition
fn derived_windows(experiments: &[Experiment]) -> Vec<(u32, u32)> {
    let Some(exp) = experiments.first() else {
        return Vec::new();
    };
    let (Some(SCAN_TYPE_MRM), Some(w)) = (exp.scan_type(), exp.scheduled_window_s) else {
        return Vec::new();
    };
    let half_ms = f64::from(w) * 500.0;
    let mut out = Vec::with_capacity(exp.ranges.len());
    for r in &exp.ranges {
        let rt = f64::from(r.expected_rt_min);
        if !rt.is_finite() || rt < 0.0 {
            return Vec::new();
        }
        // a transition without an expected time is acquired in every cycle
        if rt == 0.0 {
            out.push((0, u32::MAX));
            continue;
        }
        let centre = rt * 60_000.0;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let clamp = |x: f64| x.round().clamp(0.0, f64::from(u32::MAX)) as u32;
        out.push((clamp(centre - half_ms), clamp(centre + half_ms)));
    }
    out
}

/// The spectra of a sample, in index order: one per (cycle, Q1) for MRM experiments (the
/// scheduled transitions only), one per non-empty record for TOF experiments.
// Scan times and window bounds are whole milliseconds (u32 in the file), so exact float
// comparison is intended.
#[allow(clippy::float_cmp)]
fn spectrum_refs(sample: &Sample, experiments: &[Experiment], have_scan: bool) -> Vec<SpectrumRef> {
    let mut out = Vec::new();
    if experiments.is_empty() || !have_scan {
        return out;
    }
    let n_exp = experiments.len() as u32;
    let first_time = sample.index.first().map(|first| first.time_ms);
    for (i, rec) in sample.index.iter().enumerate() {
        if rec.byte_len == 0 {
            continue;
        }
        let i = i as u32;
        let exp = &experiments[(i % n_exp) as usize];
        match exp.scan_type() {
            Some(SCAN_TYPE_MRM) => {
                let time = rec.time_ms;
                let boundary_switch = rec.tic == 0.0 && Some(time) != first_time;
                let active: Vec<u32> = (0..exp.ranges.len() as u32)
                    .filter(|&k| match sample.windows.get(k as usize) {
                        Some(&(start, end)) if !sample.windows.is_empty() => {
                            let (start, end) = (f64::from(start), f64::from(end));
                            start <= time
                                && time <= end
                                && !(boundary_switch && (time == start || time == end))
                        }
                        _ => sample.windows.is_empty(),
                    })
                    .collect();
                // group by Q1, in transition order of each group's first member
                let mut groups: Vec<(u32, Vec<u32>)> = Vec::new();
                for k in active {
                    let q1 = exp.ranges[k as usize].first_mz.to_bits();
                    match groups.iter_mut().find(|group| group.0 == q1) {
                        Some(group) => group.1.push(k),
                        None => groups.push((q1, vec![k])),
                    }
                }
                for (_, transitions) in groups {
                    out.push(SpectrumRef::Srm {
                        record: i,
                        transitions,
                    });
                }
            }
            Some(SCAN_TYPE_TOF_MS | SCAN_TYPE_TOF_PRODUCT) => {
                out.push(SpectrumRef::Tof { record: i });
            }
            Some(
                SCAN_TYPE_Q1
                | SCAN_TYPE_PRECURSOR_ION
                | SCAN_TYPE_NEUTRAL_LOSS
                | SCAN_TYPE_ENHANCED_PRODUCT_ION
                | SCAN_TYPE_ENHANCED_MS,
            ) => {
                out.push(SpectrumRef::Grid { record: i });
            }
            _ => {}
        }
    }
    out
}

impl Dataset for SciexDataset {
    fn info(&self) -> Result<FileInfo> {
        let mut notes = self.notes.clone();
        if self
            .experiments
            .iter()
            .any(|e| e.scan_type() == Some(SCAN_TYPE_MRM))
        {
            notes.push("MRM: one spectrum per cycle and precursor (Q1) holding its scheduled transitions (m/z = Q3); native ids add transition=<first transition>".into());
        }
        if self.experiments.iter().any(|e| {
            matches!(
                e.scan_type(),
                Some(SCAN_TYPE_TOF_MS | SCAN_TYPE_TOF_PRODUCT)
            )
        }) {
            notes.push("TOF: the stored time-to-digital histogram of each scan as a profile (non-empty bins and their empty neighbours), calibrated per scan; vendor peak picking is not reproduced".into());
        }
        if self.experiments.iter().any(|e| {
            matches!(
                e.scan_type(),
                Some(
                    SCAN_TYPE_Q1
                        | SCAN_TYPE_PRECURSOR_ION
                        | SCAN_TYPE_NEUTRAL_LOSS
                        | SCAN_TYPE_ENHANCED_PRODUCT_ION
                        | SCAN_TYPE_ENHANCED_MS
                )
            )
        }) {
            notes.push("Q1, precursor ion, neutral loss, enhanced MS and enhanced product ion scans: the stored counts on the scan's m/z grid as a profile (non-empty steps and their empty neighbours), each count times its segment's intensity scale".into());
        }
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.size_bytes,
            format: SciexWiffReader.descriptor(),
            format_version: None,
            images: Vec::new(),
            tables: Vec::new(),
            spectra: self
                .samples
                .iter()
                .filter(|s| !s.spectra.is_empty())
                .map(|s| self.spectra_info(s))
                .collect(),
            traces: self.trace_infos(),
            plane_count: 0,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let samples: Vec<Value> = self
            .samples
            .iter()
            .map(|s| {
                json!({
                    "sample": s.number,
                    "strings": s.strings,
                    "started_at": s.started_at,
                    "index_records": s.index.len(),
                    "scheduled_windows": s.windows.len(),
                    "log": s.log,
                    "tdc_bin_width_ns": s.tdc_width_ns,
                })
            })
            .collect();
        Ok(json!({
            "software": self.software,
            "acquisition_method": self.method_path,
            "periods": self.periods,
            "experiments": self.experiments.iter().map(|e| json!({
                "experiment": e.number + 1,
                "scan_type_code": e.header.map(|h| h.scan_type),
                "scan_type": e.kind(),
                "polarity": e.polarity(),
                "mass_ranges": e.ranges.iter().map(|m| json!({
                    "first_mz": m.first_mz,
                    "second_mz": m.second_mz,
                    "expected_rt_min": m.expected_rt_min,
                    "name": m.name,
                    "parameters": m.parameters.iter().map(|(k, v)| (k.clone(), json!(v))).collect::<serde_json::Map<_, _>>(),
                })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "samples": samples,
            "streams": self.cfb.entries.iter().filter(|e| e.is_stream).map(|e| json!({"path": e.path, "size": e.size})).collect::<Vec<_>>(),
        }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut m = ProvenanceMap::new();
        for k in [
            "spectra[].scan_count",
            "spectra[].ms_levels",
            "spectra[].rt_range_s",
            "spectra[].instrument.model",
            "spectra[].instrument.software_version",
            "spectra[].extra.acquired_at",
            "spectra[].extra.sample_name",
            "spectra[].extra.polarities",
            "traces[]",
        ] {
            m.insert(k.into(), Source::Inferred);
        }
        m
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let mut out: Vec<LsEntry> = self
            .cfb
            .entries
            .iter()
            .filter(|e| e.is_stream)
            .map(|e| {
                let role = match e.path.rsplit('/').next().unwrap_or("") {
                    "Idx" => "scan index",
                    "Log" => "instrument log",
                    "DATA" => "sample information",
                    "ExperimentHeader" => "experiment",
                    "MassRangeEx" => "transitions / mass ranges",
                    "sMRMPro_adw_Times" => "scheduled MRM windows",
                    "TOFCalibrationData" => "TOF calibration per scan",
                    "TDCInfo" => "TDC bin width",
                    "DDERealTimeData" => "data-dependent precursors",
                    "DDERealTimeDataEx" => "data-dependent precursor charges",
                    "SampleTable" => "acquisition start",
                    _ => "stream",
                };
                LsEntry {
                    kind: "stream".into(),
                    name: e.path.clone(),
                    offset: None,
                    size: Some(e.size),
                    image: None,
                    details: json!({ "role": role }),
                }
            })
            .collect();
        if let (Some(p), Some(n)) = (&self.scan_path, self.scan_len) {
            out.push(LsEntry {
                kind: "file".into(),
                name: p
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                offset: None,
                size: Some(n),
                image: None,
                details: json!({"role": "scan data"}),
            });
        }
        for s in &self.samples {
            out.push(LsEntry {
                kind: "run".into(),
                name: format!("sample {}", s.number),
                offset: None,
                size: None,
                image: None,
                details: json!({
                    "spectra": s.spectra.len(),
                    "index_records": s.index.len(),
                    "experiments": self.experiments.len(),
                }),
            });
        }
        Ok(out)
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::unsupported(
            FORMAT_ID,
            "images",
            "WIFF files hold spectra and chromatograms; use `spectra` or `trace`.",
        ))
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut rep = CheckReport::new(self.path.display().to_string(), FORMAT_ID);
        rep.performed("compound file: header, FAT, directory");
        for p in &self.cfb.problems {
            rep.push(Finding::error("container", p.clone()));
        }
        rep.performed(format!(
            "method: {} experiments, their headers and mass ranges",
            self.experiments.len()
        ));
        if self.experiments.is_empty() {
            rep.push(Finding::warning(
                "missing_method",
                "no MethodSubtree/Method1/DeviceMethod0/Period0/Experiment0 storage",
            ));
        }
        for s in &self.samples {
            if s.index_trailing != 0 {
                rep.push(Finding::error(
                    "truncated",
                    format!(
                        "sample {}: the scan index ends {} bytes into a record",
                        s.number, s.index_trailing
                    ),
                ));
            }
            if s.index.windows(2).any(|w| w[1].time_ms < w[0].time_ms) {
                rep.push(Finding::warning(
                    "time_not_monotonic",
                    format!("sample {}: scan times decrease somewhere", s.number),
                ));
            }
            if self.scan_len.is_some() && s.scan_base.is_none() && !s.index.is_empty() {
                rep.push(Finding::error(
                    "container",
                    format!(
                        "sample {}: its block in the .wiff.scan is not where the earlier samples' data end",
                        s.number
                    ),
                ));
            }
            let bound = match (self.scan_len, &s.scan_stream) {
                (Some(len), _) => Some((
                    len,
                    s.scan_base.unwrap_or(SCAN_FILE_HEADER),
                    "the .wiff.scan",
                )),
                (None, Some((_, size))) => Some((*size, STREAM_PREAMBLE as u64, "its Scan stream")),
                (None, None) => None,
            };
            match bound {
                None => rep.push(Finding::warning(
                    "missing_file",
                    "the .wiff.scan companion is missing: no spectra can be read",
                )),
                Some((len, base, what)) => {
                    let outside = s
                        .index
                        .iter()
                        .filter(|r| base + u64::from(r.offset) + u64::from(r.byte_len) > len)
                        .count();
                    if outside > 0 {
                        rep.push(Finding::error(
                            "truncated",
                            format!(
                                "sample {}: {outside} scans point past the end of {what} ({len} bytes)",
                                s.number
                            ),
                        ));
                    }
                }
            }
            if let Some(&mx) = s.windows.iter().map(|(_, e)| e).max()
                && s.windows.len() != self.experiments.first().map_or(0, |e| e.ranges.len())
                && mx > 0
            {
                rep.push(Finding::warning(
                    "window_count_mismatch",
                    format!(
                        "sample {}: {} scheduled windows for {} transitions",
                        s.number,
                        s.windows.len(),
                        self.experiments.first().map_or(0, |e| e.ranges.len())
                    ),
                ));
            }
        }
        rep.performed("scan index: whole 54-byte records; every scan inside the .wiff.scan");
        // Decode the first and last spectrum of each sample; TOF: counts sum to the index TIC.
        for (run, s) in self.samples.iter().enumerate() {
            if s.spectra.is_empty() {
                continue;
            }
            let last = s.spectra.len() as u64 - 1;
            for i in [0, last] {
                match self.spectrum_of(run as u32, i) {
                    Ok(sp) => {
                        if matches!(s.spectra[i as usize], SpectrumRef::Tof { .. }) {
                            let sum: f64 = sp.intensity.iter().map(|&v| f64::from(v)).sum();
                            let tic = sp.total_ion_current.unwrap_or(0.0);
                            if (sum - tic).abs() > 1e-6 * tic.abs().max(1.0) {
                                // TripleTOF 5600 indexes record a TIC that is not the sum of
                                // the stored counts even when every count decodes (the largest
                                // equals the index's base-peak intensity): say which case it is.
                                let max = sp.intensity.iter().copied().fold(0.0f32, f32::max);
                                let base_ok = sp
                                    .base_peak_intensity
                                    .is_some_and(|b| (f64::from(max) - b).abs() < 0.5);
                                let why = if base_ok {
                                    "; the largest count equals the index's base-peak intensity (TripleTOF 5600 files record a TIC that is not the sum of the stored counts)"
                                } else {
                                    ""
                                };
                                rep.push(Finding::warning(
                                    "tic_mismatch",
                                    format!("sample {} spectrum {i}: counts sum to {sum}, the index records TIC {tic}{why}", s.number),
                                ));
                            }
                        }
                    }
                    Err(e) => rep.push(Finding::error(
                        "undecodable",
                        format!("sample {} spectrum {i}: {e}", s.number),
                    )),
                }
            }
        }
        rep.performed("decoded the first and last spectrum of every sample (TOF: counts sum to the index TIC)");
        Ok(rep)
    }

    fn read_trace(
        &mut self,
        index: u32,
        sweep: u32,
        first_sample: u64,
        max_samples: u64,
    ) -> Result<Trace> {
        let n = self.samples.len() * 2;
        let devices = self.device_traces();
        if sweep != 0 || index as usize >= n + devices.len() {
            return Err(Error::Usage(format!(
                "trace {index} sweep {sweep} does not exist ({} traces, one sweep each)",
                n + devices.len()
            )));
        }
        if index as usize >= n {
            let (_, _, (_, data)) = devices[index as usize - n];
            let start = usize::try_from(first_sample).unwrap_or(usize::MAX);
            let take = usize::try_from(max_samples).unwrap_or(usize::MAX);
            return Ok(Trace {
                trace: index,
                sweep,
                first_sample,
                channels: data
                    .iter()
                    .map(|c| c.iter().skip(start).take(take).copied().collect())
                    .collect(),
            });
        }
        let s = &self.samples[index as usize / 2];
        let rows: Vec<(usize, &IndexRecord)> = s
            .index
            .iter()
            .enumerate()
            .filter(|(_, r)| r.byte_len > 0)
            .collect();
        let start = usize::try_from(first_sample).unwrap_or(usize::MAX);
        let take = usize::try_from(max_samples).unwrap_or(usize::MAX);
        let sel: Vec<&(usize, &IndexRecord)> = rows.iter().skip(start).take(take).collect();
        let mut values = Vec::with_capacity(sel.len());
        for &&(i, r) in &sel {
            values.push(if index.is_multiple_of(2) {
                r.tic
            } else {
                self.base_peak_of(s, i as u32, r)?
            });
        }
        Ok(Trace {
            trace: index,
            sweep,
            first_sample,
            channels: vec![
                sel.iter().map(|(_, r)| r.time_ms / 1000.0).collect(),
                values,
            ],
        })
    }

    fn visit_scan_headers(
        &mut self,
        run: u32,
        first: u64,
        visit: &mut dyn FnMut(openreadout_core::ScanHeader) -> bool,
    ) -> Result<bool> {
        let n = self.sample(run)?.spectra.len() as u64;
        for i in first..n {
            let h = openreadout_core::ScanHeader::from(self.spectrum_or_header(run, i, false)?);
            if !visit(h) {
                break;
            }
        }
        Ok(true)
    }

    fn read_spectrum(&mut self, index: u32, spectrum: u64) -> Result<Spectrum> {
        self.spectrum_of(index, spectrum)
    }

    fn member_files(&self) -> Vec<PathBuf> {
        // the .wiff and its .wiff.scan, less the one that was opened
        std::iter::once(self.path.clone())
            .chain(self.scan_path.clone())
            .filter(|p| *p != self.opened && self.fs.is_file(p))
            .collect()
    }

    fn find_spectrum(&mut self, run: u32, scan_number: u64) -> Result<Option<u64>> {
        let Ok(s) = self.sample(run) else {
            return Ok(None);
        };
        // Scan numbers are positions + 1 (see `spectrum_of`).
        Ok((scan_number >= 1 && scan_number <= s.spectra.len() as u64).then(|| scan_number - 1))
    }
}
