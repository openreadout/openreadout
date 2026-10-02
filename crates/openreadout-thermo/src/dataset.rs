//! `Dataset` implementation: opening (header chain, run header, index, events), spectra,
//! metadata, listing and integrity checks.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use openreadout_core::bytes::utf16le_z;
use openreadout_core::cfb::{Cfb, CfbEntry};
use openreadout_core::model::{
    CheckReport, FileInfo, Finding, InstrumentInfo, LsEntry, SpectraInfo, Spectrum, Trace,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex, SpectrumView};
use openreadout_core::source::{Fs, Input, SourceFile};
use openreadout_core::{Error, Plane, Result};
use serde_json::{Value, json};

use crate::bytes::{Cursor, adler32, read_at};
use crate::layout::{
    AutosamplerInfo, CHECKSUM_OFFSET, CHECKSUM_SPAN, ErrorLogEntry, FILE_HEADER_LEN, FileHeader,
    FileInfoBlock, GenericHeader, InstrumentId, MethodTable, RunHeader, SUPPORTED_VERSIONS,
    ScanEvent, ScanIndexEntry, SequenceRow, parse_autosampler, parse_error_entry,
    parse_file_header, parse_file_info, parse_generic_header, parse_instrument_id,
    parse_method_table, parse_run_header, parse_scan_event, parse_scan_index_entry,
    parse_sequence_row, scan_index_entry_len,
};
use crate::packet::{
    MAX_WINDOWS, MzScale, PACKET_HEADER_LEN, Packet, WINDOW_PEAK_LEN, WINDOW_RECORD_KIND,
    parse_packet, parse_packet_header, parse_window_peaks, parse_window_record, window_record_len,
};
use crate::{FORMAT_ID, ThermoRawReader};

/// How much of the file start we read to decode the header chain.
const HEAD_READ: u64 = 4 << 20;
/// Upper bound for the small structure regions we load whole.
const MAX_REGION: u64 = 256 << 20;
/// The per-scan `Monoisotopic M/Z:` is the precursor only when it lies less than this far (m/z)
/// from the isolation target; farther values are reported as the target, as every reference
/// conversion in the corpus does (`docs/formats/thermo-raw.md` § Precursor and charge).
pub const MONOISOTOPIC_MAX_SHIFT: f64 = 3.0;

/// The instrument method embedded in the file (a compound file after a Finnigan header).
#[derive(Debug, Clone, Default)]
pub struct MethodDocument {
    /// Offset and length of the compound file.
    pub container_offset: u64,
    pub container_size: u64,
    /// Path of the temporary copy the acquisition software wrote.
    pub source_path: String,
    /// `(display name, storage name)` of every device in the method.
    pub devices: Vec<(String, String)>,
    /// `(path, bytes)` of every storage and stream.
    pub streams: Vec<(String, u64)>,
    /// `(device, text)` from each device storage's `Text` stream: the human-readable method.
    pub device_texts: Vec<(String, String)>,
}

/// Parse the method region `[start, start + len)`.
fn parse_method_document(buf: &[u8], start: u64) -> Result<MethodDocument> {
    let mut c = Cursor::new(buf, start);
    parse_file_header(&mut c)?;
    c.seek_to(FILE_HEADER_LEN as usize)?;
    let size = c.u32()?;
    let source_path = c.utf16_counted()?;
    let at = c.offset();
    let n = c.u32()?;
    if n > 1000 {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            at,
            format!("implausible method device count {n}"),
        ));
    }
    let mut devices = Vec::with_capacity(n as usize);
    for _ in 0..n {
        devices.push((c.utf16_counted()?, c.utf16_counted()?));
    }
    let container_offset = c.offset();
    let mut blob = std::io::Cursor::new(c.take(size as usize)?);
    let here = Path::new("embedded method container");
    let cf = Cfb::open(&mut blob, here, FORMAT_ID)?;
    if let Some(problem) = cf.problems.first() {
        return Err(Error::corrupt(
            FORMAT_ID,
            format!("embedded method container: {problem}"),
        ));
    }
    let entries: Vec<&CfbEntry> = cf.entries.iter().filter(|e| e.id != 0).collect();
    let mut device_texts = Vec::new();
    for e in &entries {
        if let Some(device) = e.path.strip_suffix("/Text")
            && e.is_stream
        {
            let raw = cf.read(&mut blob, here, e, u64::MAX)?;
            let text = utf16le_z(&raw).replace("\r\n", "\n");
            device_texts.push((device.to_string(), text));
        }
    }
    Ok(MethodDocument {
        container_offset,
        container_size: u64::from(size),
        source_path,
        devices,
        streams: entries.iter().map(|e| (e.path.clone(), e.size)).collect(),
        device_texts,
    })
}

/// One scan's index row joined with its event, as used by `info`, `info --view structure` and
/// exports.
#[derive(Debug, Clone)]
pub struct ScanSummary {
    pub scan_number: u64,
    pub ms_level: u32,
    pub rt_s: f64,
    pub polarity: String,
    pub scan_filter: String,
    pub data_offset: u64,
    pub data_size: u32,
    /// Bytes at `data_offset` (differs from `data_size` for window-record scans).
    pub data_len: u64,
}

/// An opened `.raw` file.
#[derive(Debug, Default)]
pub struct ThermoDataset {
    path: PathBuf,
    file_len: u64,
    pub header: FileHeader,
    pub sequence: SequenceRow,
    pub autosampler: AutosamplerInfo,
    pub file_info: FileInfoBlock,
    /// Byte range of the embedded instrument-method container, if any.
    method_file: Option<(u64, u64)>,
    pub method: Option<MethodDocument>,
    pub run_header: RunHeader,
    pub instrument: Option<InstrumentId>,
    pub instrument_log_header: Option<GenericHeader>,
    pub errors: Vec<ErrorLogEntry>,
    /// The u32 in front of the error-log entries (observed 0, or the entry count).
    pub error_log_lead: u32,
    pub method_table: Option<MethodTable>,
    pub scan_parameter_header: Option<GenericHeader>,
    pub index: Vec<ScanIndexEntry>,
    pub events: Vec<ScanEvent>,
    /// Absolute offset where parsing of the event stream stopped.
    events_end: u64,
    /// Structural problems met while opening that did not prevent reading.
    problems: Vec<Finding>,
    handle: Option<SourceFile>,
    /// Where the file is read from.
    fs: Fs,
    /// Set when the signature is right but a structure needed to read scans is damaged
    /// (typically truncation): `check` reports it, every other operation returns it.
    broken: Option<(String, Option<u64>)>,
    /// Controllers other than the mass spectrometer (UV/PDA/analog detectors).
    pub detectors: Vec<crate::detectors::Detector>,
    /// Leave peaks flagged [`crate::FLAG_EXCLUDED`] out of spectra, as conversions made before
    /// about 2021 did (see [`ThermoDataset::set_exclude_flagged_peaks`]). Off by default.
    exclude_flagged: bool,
}

fn corrupt(offset: u64, msg: impl Into<String>) -> Error {
    Error::corrupt_at(FORMAT_ID, offset, msg)
}

impl ThermoDataset {
    /// Open a file. A damaged file whose header is still readable opens in a degraded state in
    /// which `check` reports the damage (exit 4) and reads return a `Corrupt` error.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    /// Open an [`Input`] (a local path, a buffer, a host source); see [`ThermoDataset::open`].
    pub(crate) fn open_input(input: &Input) -> Result<Self> {
        let (path, fs) = (input.path(), input.fs());
        match Self::open_full(fs, path) {
            Err(Error::Corrupt { detail, offset, .. }) => {
                let mut file = fs.open(path).map_err(|e| Error::io(path, e))?;
                let file_len = file.metadata().map_err(|e| Error::io(path, e))?.len();
                let head = read_at(
                    &mut file,
                    path,
                    0,
                    file_len.min(FILE_HEADER_LEN),
                    "file start",
                )?;
                let header = parse_file_header(&mut Cursor::new(&head, 0))?;
                Ok(ThermoDataset {
                    path: path.to_path_buf(),
                    file_len,
                    header,
                    broken: Some((detail, offset)),
                    fs: fs.clone(),
                    ..Default::default()
                })
            }
            other => other,
        }
    }

    fn broken_err(&self) -> Result<()> {
        match &self.broken {
            Some((detail, Some(at))) => Err(Error::corrupt_at(FORMAT_ID, *at, detail.clone())),
            Some((detail, None)) => Err(Error::corrupt(FORMAT_ID, detail.clone())),
            None => Ok(()),
        }
    }

    #[allow(clippy::many_single_char_names)] // c = cursor, s = streams, n = scans: used throughout
    fn open_full(fs: &Fs, path: &Path) -> Result<Self> {
        let mut file = fs.open(path).map_err(|e| Error::io(path, e))?;
        let file_len = file.metadata().map_err(|e| Error::io(path, e))?.len();
        if file_len < FILE_HEADER_LEN {
            return Err(corrupt(
                0,
                format!("file is {file_len} bytes, shorter than the {FILE_HEADER_LEN}-byte header"),
            ));
        }
        let head = read_at(&mut file, path, 0, file_len.min(HEAD_READ), "file start")?;
        let mut c = Cursor::new(&head, 0);
        let header = parse_file_header(&mut c)?;
        let version = header.version;
        if !(57..=66).contains(&version) {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!("file version {version}"),
                "Versions 63, 64 and 66 are validated; 57-62 are attempted. Older (LCQ-era) and newer layouts are not decoded.",
            ));
        }
        c.seek_to(FILE_HEADER_LEN as usize)?;
        let sequence = parse_sequence_row(&mut c, version)?;
        let autosampler = parse_autosampler(&mut c)?;
        let file_info = parse_file_info(&mut c, version)?;
        let after_info = c.offset();
        let mut problems = Vec::new();
        if !SUPPORTED_VERSIONS.contains(&version) {
            problems.push(Finding::warning(
                "unvalidated_version",
                format!(
                    "file version {version} has not been validated against a reference conversion"
                ),
            ));
        }
        let method_file = (file_info.method_file_present != 0
            && file_info.data_address > after_info)
            // `then`, not `then_some`: the subtraction must only run when the condition holds.
            .then(|| (after_info, file_info.data_address - after_info));
        let method = match method_file {
            Some((off, len)) if len <= MAX_REGION => {
                match read_at(&mut file, path, off, len, "instrument method")
                    .and_then(|b| parse_method_document(&b, off))
                {
                    Ok(m) => Some(m),
                    Err(e) => {
                        problems.push(Finding::warning("instrument_method", e.to_string()));
                        None
                    }
                }
            }
            _ => None,
        };

        // Run header.
        let rh_addr = file_info.ms_run_header_address();
        let rh_len: u64 = if version >= 64 { 7576 } else { 7408 };
        let rh_buf = read_at(&mut file, path, rh_addr, rh_len, "run header")?;
        let run_header = parse_run_header(&mut Cursor::new(&rh_buf, rh_addr), version)?;
        if run_header.own_address != rh_addr {
            problems.push(
                Finding::error(
                    "run_header_address",
                    format!(
                        "run header at {rh_addr} records its own address as {}",
                        run_header.own_address
                    ),
                )
                .at(rh_addr),
            );
        }
        let s = run_header.streams;
        for (name, addr) in [
            ("scan index", s.scan_index),
            ("scan data", s.scan_data),
            ("instrument log", s.instrument_log),
            ("error log", s.error_log),
            ("scan events", s.scan_events),
            ("scan parameters", s.scan_parameters),
        ] {
            if addr > file_len {
                return Err(corrupt(
                    rh_addr,
                    format!(
                        "{name} address {addr} lies beyond the end of the {file_len}-byte file (truncated?)"
                    ),
                ));
            }
        }
        let n = run_header.scan_count();

        // Instrument id + instrument-log columns sit between the run header and the log records.
        let rh_end = rh_addr + run_header.byte_len;
        let (instrument, instrument_log_header) =
            match region(&mut file, path, rh_end, s.instrument_log, "instrument id") {
                Ok(buf) => {
                    let mut c = Cursor::new(&buf, rh_end);
                    let inst = parse_instrument_id(&mut c);
                    let log = inst.as_ref().ok().map(|_| parse_generic_header(&mut c));
                    match (inst, log) {
                        (Ok(i), Some(Ok(l))) => {
                            if c.offset() != s.instrument_log {
                                problems.push(Finding::warning(
                                    "instrument_log_header",
                                    format!(
                                        "instrument-log columns end at {} but records start at {}",
                                        c.offset(),
                                        s.instrument_log
                                    ),
                                ));
                            }
                            (Some(i), Some(l))
                        }
                        (Ok(i), _) => (Some(i), None),
                        (Err(e), _) => {
                            problems.push(Finding::warning("instrument_id", e.to_string()));
                            (None, None)
                        }
                    }
                }
                Err(e) => {
                    problems.push(Finding::warning("instrument_id", e.to_string()));
                    (None, None)
                }
            };

        // Error log, method table and the per-scan parameter columns precede the scan index.
        let mut errors = Vec::new();
        let mut error_log_lead = 0;
        let mut method_table = None;
        let mut scan_parameter_header = None;
        match region(&mut file, path, s.error_log, s.scan_index, "error log") {
            Ok(buf) => {
                let mut c = Cursor::new(&buf, s.error_log);
                let mut ok = true;
                match c.u32() {
                    Ok(v) => error_log_lead = v,
                    Err(e) => {
                        problems.push(Finding::warning("error_log", e.to_string()));
                        ok = false;
                    }
                }
                for _ in 0..if ok { run_header.error_log_count } else { 0 } {
                    match parse_error_entry(&mut c) {
                        Ok(e) => errors.push(e),
                        Err(e) => {
                            problems.push(Finding::warning("error_log", e.to_string()));
                            ok = false;
                            break;
                        }
                    }
                }
                if ok {
                    match parse_method_table(&mut c, version) {
                        Ok(t) => {
                            method_table = Some(t);
                            match parse_generic_header(&mut c) {
                                Ok(h) => scan_parameter_header = Some(h),
                                Err(e) => problems
                                    .push(Finding::warning("scan_parameter_header", e.to_string())),
                            }
                        }
                        Err(e) => problems.push(Finding::warning("method_table", e.to_string())),
                    }
                }
            }
            Err(e) => problems.push(Finding::warning("error_log", e.to_string())),
        }

        // Scan index.
        let entry_len = scan_index_entry_len(version) as u64;
        let index_len = n
            .checked_mul(entry_len)
            .ok_or_else(|| corrupt(rh_addr, "scan count overflows"))?;
        let index_buf = read_at(&mut file, path, s.scan_index, index_len, "scan index")?;
        let mut c = Cursor::new(&index_buf, s.scan_index);
        let mut index = Vec::with_capacity(n as usize);
        for _ in 0..n {
            index.push(parse_scan_index_entry(&mut c, version)?);
        }

        // Scan events: a u32 (the count before version 66, 0 in 66) then one event per scan.
        let ev_end = if s.scan_parameters > s.scan_events {
            s.scan_parameters
        } else {
            file_len
        };
        let ev_buf = region(&mut file, path, s.scan_events, ev_end, "scan events")?;
        let mut c = Cursor::new(&ev_buf, s.scan_events);
        let lead = c.u32()?;
        if version < 66 && u64::from(lead) != n {
            problems.push(
                Finding::warning(
                    "scan_event_count",
                    format!("scan-event stream says {lead} events, run header says {n} scans"),
                )
                .at(s.scan_events),
            );
        }
        let mut events = Vec::with_capacity(n as usize);
        for i in 0..n {
            let e = parse_scan_event(&mut c, version)
                .map_err(|e| corrupt(c.offset(), format!("scan event {}: {e}", i + 1)))?;
            events.push(e);
        }
        let events_end = c.offset();

        // Detector controllers (UV channels, PDA, analog): never fatal for the MS data.
        let (detectors, detector_problems) =
            crate::detectors::open_detectors(&mut file, path, version, &file_info.controllers);
        for p in detector_problems {
            problems.push(Finding::warning("detector", p));
        }

        Ok(ThermoDataset {
            path: path.to_path_buf(),
            file_len,
            header,
            sequence,
            autosampler,
            file_info,
            method_file,
            method,
            run_header,
            instrument,
            instrument_log_header,
            errors,
            error_log_lead,
            method_table,
            scan_parameter_header,
            index,
            events,
            events_end,
            problems,
            handle: None,
            fs: fs.clone(),
            broken: None,
            detectors,
            exclude_flagged: false,
        })
    }

    /// The detectors exposed as traces (those with samples), in trace order.
    fn trace_detectors(&self) -> Vec<&crate::detectors::Detector> {
        self.detectors
            .iter()
            .filter(|d| d.kind != crate::detectors::DetectorKind::NoData && d.scan_count > 0)
            .collect()
    }

    /// Every device's method text, one after the other (UV wavelengths, units, rates).
    fn method_text(&self) -> String {
        self.method.as_ref().map_or_else(String::new, |m| {
            m.device_texts
                .iter()
                .map(|(_, t)| t.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
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

    pub fn version(&self) -> u32 {
        self.header.version
    }

    pub fn scan_count(&self) -> u64 {
        self.index.len() as u64
    }

    fn check_scan(&self, run: u32, i: u64) -> Result<usize> {
        if run != 0 {
            return Err(Error::Usage(format!(
                "run {run} does not exist; this file exposes one run (index 0)"
            )));
        }
        let n = self.index.len();
        usize::try_from(i).ok().filter(|&k| k < n).ok_or_else(|| {
            Error::Usage(format!(
                "spectrum index {i} out of range (0..{n}); scan numbers are index + {}",
                self.run_header.first_scan
            ))
        })
    }

    /// Scan number (1-based in every corpus file) of index `i`.
    pub fn scan_number(&self, i: usize) -> u64 {
        u64::from(self.run_header.first_scan) + i as u64
    }

    /// Scan filter text of scan index `i`.
    pub fn filter_text(&self, i: usize) -> Option<String> {
        self.events.get(i).map(|e| self.filter_of(e))
    }

    /// How this instrument numbers its ion-trap scan rates (from its model name).
    pub fn scan_rates(&self) -> crate::event::ScanRates {
        crate::event::ScanRates::for_model(
            self.instrument
                .as_ref()
                .map(|i| format!("{} {}", i.model, i.model_2))
                .as_deref(),
        )
    }

    fn filter_of(&self, e: &ScanEvent) -> String {
        e.filter_text_for(self.run_header.filter_mass_decimals(), self.scan_rates())
    }

    /// Rows for `info` (default and structure views) and exports without reading scan data.
    pub fn scans(&self) -> Vec<ScanSummary> {
        self.index
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let ev = self.events.get(i);
                ScanSummary {
                    scan_number: self.scan_number(i),
                    ms_level: ev.map_or(0, ScanEvent::ms_level),
                    rt_s: e.rt_min * 60.0,
                    polarity: ev.map_or("unknown", |e| e.polarity().word()).into(),
                    scan_filter: self.filter_text(i).unwrap_or_default(),
                    data_offset: self
                        .run_header
                        .streams
                        .scan_data
                        .saturating_add(e.data_offset),
                    data_size: e.data_size,
                    data_len: self.scan_extent(i).1,
                }
            })
            .collect()
    }

    /// Where scan `i`'s index points and how many bytes live there: the packet, or for
    /// [`WINDOW_RECORD_KIND`] scans the window record (its peaks sit just before it).
    pub fn scan_extent(&self, i: usize) -> (u64, u64) {
        let e = self.index[i];
        let start = self
            .run_header
            .streams
            .scan_data
            .saturating_add(e.data_offset);
        let len = if e.kind_code == WINDOW_RECORD_KIND {
            window_record_len(e.data_size.saturating_sub(1))
        } else {
            u64::from(e.data_size)
        };
        (start, len)
    }

    /// Scan `i` stored as peaks followed by a window record.
    fn window_packet(&mut self, i: usize) -> Result<Packet> {
        let e = self.index[i];
        let windows = e.data_size.checked_sub(1).ok_or_else(|| {
            corrupt(
                0,
                format!("scan {}: window count missing", self.scan_number(i)),
            )
        })?;
        if windows > MAX_WINDOWS {
            return Err(corrupt(
                0,
                format!(
                    "scan {}: implausible window count {windows}",
                    self.scan_number(i)
                ),
            ));
        }
        let (at, len) = self.scan_extent(i);
        let path = self.path.clone();
        let buf = read_at(self.handle()?, &path, at, len, "window record")?;
        let record = parse_window_record(&buf, at, windows)?;
        let n = u64::from(record.peak_bytes / 8);
        let (centroids, peak_flags) = if n == 0 {
            (Vec::new(), Vec::new())
        } else {
            let first = record.windows.first().ok_or_else(|| {
                corrupt(
                    at,
                    format!("scan {}: peaks but no window", self.scan_number(i)),
                )
            })?;
            let peak_start = u64::from(first.peak_offset);
            if peak_start + n * WINDOW_PEAK_LEN != e.data_offset {
                return Err(corrupt(
                    at,
                    format!(
                        "scan {}: {n} peaks at offset {peak_start} do not end where the window record begins ({})",
                        self.scan_number(i),
                        e.data_offset
                    ),
                ));
            }
            let pat = self.run_header.streams.scan_data.saturating_add(peak_start);
            let pbuf = read_at(
                self.handle()?,
                &path,
                pat,
                n * WINDOW_PEAK_LEN,
                "window-record peaks",
            )?;
            parse_window_peaks(&pbuf, pat, n as usize)?
        };
        Ok(Packet {
            centroids,
            peak_flags,
            window_record: Some(record),
            ..Packet::default()
        })
    }

    /// Read and decode the data packet of scan index `i`.
    pub fn packet(&mut self, i: usize) -> Result<Packet> {
        let e = self.index[i];
        if e.kind_code == WINDOW_RECORD_KIND {
            return self.window_packet(i);
        }
        let at = self
            .run_header
            .streams
            .scan_data
            .checked_add(e.data_offset)
            .ok_or_else(|| corrupt(0, "scan data offset overflows"))?;
        let path = self.path.clone();
        let buf = read_at(
            self.handle()?,
            &path,
            at,
            u64::from(e.data_size),
            "scan data packet",
        )?;
        let p = parse_packet(&buf, at)?;
        if p.header.packet_len() != u64::from(e.data_size) {
            return Err(corrupt(
                at,
                format!(
                    "scan {}: packet header describes {} bytes, index says {}",
                    self.scan_number(i),
                    p.header.packet_len(),
                    e.data_size
                ),
            ));
        }
        Ok(p)
    }

    /// Decode the per-scan parameter record of scan index `i` (labels as the file writes them).
    pub fn scan_parameters(&mut self, i: usize) -> Result<Vec<(String, Value)>> {
        let Some(h) = self.scan_parameter_header.clone() else {
            return Ok(Vec::new());
        };
        let Some(len) = h.record_len() else {
            return Ok(Vec::new());
        };
        let at = self
            .run_header
            .streams
            .scan_parameters
            .saturating_add((i as u64).saturating_mul(len as u64));
        let path = self.path.clone();
        let buf = read_at(self.handle()?, &path, at, len as u64, "scan parameters")?;
        h.decode(&mut Cursor::new(&buf, at))
    }

    /// Spectra read through [`Dataset`] keep every stored centroid, including those whose
    /// descriptor carries [`crate::FLAG_EXCLUDED`] (reference and background ions), as current
    /// conversions with the vendor library do. `true` leaves them out of centroid lists and
    /// blanks them in profiles, as conversions made before October 2020 did (ProteoWizard up to
    /// 3.0.20239 in the corpus; `docs/formats/thermo-raw.md` § Centroid view).
    pub fn set_exclude_flagged_peaks(&mut self, exclude: bool) {
        self.exclude_flagged = exclude;
    }

    /// Precursor m/z and charge of scan index `i`: the last reaction's precursor, replaced by
    /// the trailer's `Monoisotopic M/Z` when that is within [`MONOISOTOPIC_MAX_SHIFT`]; the
    /// trailer's `Charge State` when non-zero (MS2 and above only).
    fn precursor_of(&mut self, i: usize) -> (Option<f64>, Option<i32>) {
        let params = self.scan_parameters(i).unwrap_or_default();
        let lookup = |label: &str| {
            params
                .iter()
                .find(|(l, _)| l.trim_end_matches(':').eq_ignore_ascii_case(label))
                .map(|(_, v)| v.clone())
        };
        let Some(r) = self.events[i].precursor_reactions().last().copied() else {
            return (None, None);
        };
        let monoisotopic = lookup("Monoisotopic M/Z")
            .and_then(|v| v.as_f64())
            .filter(|v| *v > 0.0);
        let mz = monoisotopic
            .filter(|m| (m - r.precursor_mz).abs() < MONOISOTOPIC_MAX_SHIFT)
            .unwrap_or(r.precursor_mz);
        let charge = lookup("Charge State")
            .and_then(|v| v.as_i64())
            .filter(|c| *c != 0)
            .and_then(|c| i32::try_from(c).ok());
        (Some(mz), charge)
    }

    /// Isolation window, activation, collision energy of the last reaction of scan `i`, and
    /// `extra.precursors` (every reaction's precursor m/z in filter order, when there are
    /// several: MS^n and multiplexed scans) and `extra.sps_masses` (the trailer's `SPS Masses`).
    #[allow(clippy::type_complexity)]
    fn reaction_details(
        &mut self,
        i: usize,
    ) -> (
        Option<[f64; 2]>,
        Option<String>,
        Option<f64>,
        BTreeMap<String, Value>,
    ) {
        let event = &self.events[i];
        let reactions = event.precursor_reactions().to_vec();
        let last = reactions.last().copied();
        let iso = last
            .filter(|r| r.isolation_width > 0.0 && r.precursor_mz > 0.0)
            .map(|r| {
                let h = r.isolation_width / 2.0;
                [r.precursor_mz - h, r.precursor_mz + h]
            });
        let act = last
            .and_then(|r| r.activation().filter_token())
            .map(str::to_ascii_uppercase);
        let ce = last.map(|r| r.energy).filter(|e| *e > 0.0);
        let mut extra = BTreeMap::new();
        if reactions.len() > 1 {
            extra.insert(
                "precursors".into(),
                json!(reactions.iter().map(|r| r.precursor_mz).collect::<Vec<_>>()),
            );
            // Every stage's isolation window and energy, in the same order: the top-level
            // fields are the last stage's, and exports differ in which stage they name.
            extra.insert(
                "precursor_isolation_windows_mz".into(),
                json!(
                    reactions
                        .iter()
                        .map(|r| (r.isolation_width > 0.0).then(|| {
                            let h = r.isolation_width / 2.0;
                            [r.precursor_mz - h, r.precursor_mz + h]
                        }))
                        .collect::<Vec<_>>()
                ),
            );
            extra.insert(
                "collision_energies".into(),
                json!(reactions.iter().map(|r| r.energy).collect::<Vec<_>>()),
            );
        }
        if event.is_multiplex() {
            extra.insert("multiplexed".into(), json!(true));
        }
        if let Some(e) = event.source_cid_energy() {
            extra.insert("source_cid_energy".into(), json!(e));
        }
        if event.ms_level() >= 3 {
            let params = self.scan_parameters(i).unwrap_or_default();
            if let Some((_, v)) = params
                .iter()
                .find(|(l, _)| l.trim_end_matches(':').eq_ignore_ascii_case("SPS Masses"))
                && let Some(t) = v.as_str()
            {
                let masses: Vec<f64> = t
                    .split(',')
                    .filter_map(|x| x.trim().parse::<f64>().ok())
                    .collect();
                if !masses.is_empty() {
                    extra.insert("sps_masses".into(), json!(masses));
                }
            }
        }
        (iso, act, ce, extra)
    }

    /// Header of scan index `i` without reading its data packet: the scan index row (RT, TIC,
    /// base peak), the scan event (level, polarity, filter, reactions, scan range) and the
    /// trailer record (monoisotopic m/z, charge). The isolation window is centred on the last
    /// reaction's precursor m/z with its isolation width; activation and collision energy are
    /// that reaction's.
    pub fn scan_header(&mut self, i: usize) -> Result<openreadout_core::ScanHeader> {
        let n = self.index.len().min(self.events.len());
        if i >= n {
            return Err(Error::Usage(format!(
                "spectrum index {i} out of range (0..{n})"
            )));
        }
        let (precursor_mz, precursor_charge) = self.precursor_of(i);
        let (isolation_window_mz, activation, collision_energy, extra) = self.reaction_details(i);
        let event = &self.events[i];
        let entry = self.index[i];
        Ok(openreadout_core::ScanHeader {
            index: i as u64,
            scan_number: self.scan_number(i),
            ms_level: event.ms_level(),
            rt_s: Some(entry.rt_min * 60.0),
            polarity: event.polarity().word().into(),
            centroided: event.is_profile() == Some(false),
            precursor_mz,
            precursor_charge,
            isolation_window_mz,
            activation,
            collision_energy,
            extra,
            scan_filter: Some(self.filter_of(event)),
            scan_window_mz: event.scan_ranges.first().copied(),
            total_ion_current: Some(entry.total_ion_current),
            base_peak_mz: Some(entry.base_peak_mz).filter(|v| *v > 0.0),
            base_peak_intensity: (entry.base_peak_mz > 0.0).then_some(entry.base_peak_intensity),
            ..openreadout_core::ScanHeader::default()
        })
    }

    /// Read scan index `i`. With `include_flagged` true (what `read_spectrum` does unless
    /// [`ThermoDataset::set_exclude_flagged_peaks`] was set), every stored centroid is kept;
    /// `false` leaves out the peaks whose descriptor carries [`crate::FLAG_EXCLUDED`] and blanks
    /// them in profiles.
    pub fn read_scan(
        &mut self,
        i: usize,
        view: SpectrumView,
        include_flagged: bool,
    ) -> Result<Spectrum> {
        let packet = self.packet(i)?;
        let event = self.events[i].clone();
        let entry = self.index[i];
        let use_centroids = match view {
            SpectrumView::Centroid => !packet.centroids.is_empty() || packet.profile.is_none(),
            // `Primary`, and any view added after this reader: the scan's primary data.
            _ => packet.profile.is_none(),
        };
        // Centroids whose descriptor carries FLAG_EXCLUDED (reference/background ions): their
        // positions in the returned list, or how many were left out.
        let mut flagged: Vec<usize> = Vec::new();
        let mut flagged_left_out = 0usize;
        let (mz, intensity, centroided) = if use_centroids {
            let is_flagged = |k: usize| {
                packet
                    .descriptors
                    .get(k)
                    .is_some_and(|d| d.flags & crate::packet::FLAG_EXCLUDED != 0)
            };
            let mut keep: Vec<&crate::packet::Centroid> =
                Vec::with_capacity(packet.centroids.len());
            for (k, c) in packet.centroids.iter().enumerate() {
                if is_flagged(k) {
                    if !include_flagged {
                        flagged_left_out += 1;
                        continue;
                    }
                    flagged.push(keep.len());
                }
                keep.push(c);
            }
            (
                keep.iter().map(|c| c.mz).collect(),
                keep.iter().map(|c| c.intensity).collect(),
                true,
            )
        } else {
            let profile = packet.profile.as_ref().expect("profile present");
            let scale =
                MzScale::from_event(profile.step, &event.coefficients).ok_or_else(|| {
                    Error::unsupported(
                        FORMAT_ID,
                        format!(
                            "profile m/z conversion with {} coefficients",
                            event.coefficients.len()
                        ),
                        "Read the stored centroids instead (`--centroid`).",
                    )
                })?;
            let (m, it) = crate::packet::render_profile(
                profile,
                scale,
                &packet.centroids,
                &packet.descriptors,
                include_flagged,
            );
            (m, it, false)
        };
        let (precursor_mz, precursor_charge) = self.precursor_of(i);
        let (isolation_window_mz, activation, collision_energy, mut extra) =
            self.reaction_details(i);
        if !flagged.is_empty() {
            extra.insert("flagged_peaks".into(), serde_json::json!(flagged));
        }
        if flagged_left_out > 0 {
            extra.insert(
                "flagged_peaks_left_out".into(),
                serde_json::json!(flagged_left_out),
            );
        }
        Ok(Spectrum {
            index: i as u64,
            scan_number: self.scan_number(i),
            ms_level: event.ms_level(),
            rt_s: Some(entry.rt_min * 60.0),
            polarity: event.polarity().word().into(),
            centroided,
            precursor_mz,
            precursor_charge,
            isolation_window_mz,
            activation,
            collision_energy,
            extra,
            scan_filter: Some(self.filter_of(&event)),
            total_ion_current: Some(entry.total_ion_current),
            mz,
            intensity,
            ..Spectrum::default()
        })
    }

    fn spectra_info(&self) -> SpectraInfo {
        let mut levels = BTreeSet::new();
        let mut level_counts: BTreeMap<u32, u64> = BTreeMap::new();
        let mut polarities = BTreeSet::new();
        let mut analyzers = BTreeSet::new();
        let mut sources = BTreeSet::new();
        let mut filters: BTreeMap<String, u64> = BTreeMap::new();
        let mut profile = 0u64;
        let decimals = self.run_header.filter_mass_decimals();
        for e in &self.events {
            levels.insert(e.ms_level());
            *level_counts.entry(e.ms_level()).or_default() += 1;
            polarities.insert(e.polarity().word());
            if let Some(t) = e.analyzer().filter_token() {
                analyzers.insert(t);
            }
            if let Some(t) = e.ionization().filter_token() {
                sources.insert(t);
            }
            if e.is_profile() == Some(true) {
                profile += 1;
            }
            if e.ms_level() <= 1 && filters.len() < 64 {
                *filters.entry(e.filter_text(decimals)).or_default() += 1;
            }
        }
        let rt_range_s = match (self.index.first(), self.index.last()) {
            (Some(a), Some(b)) => Some([a.rt_min * 60.0, b.rt_min * 60.0]),
            _ => None,
        };
        let instrument = self.instrument.as_ref().map(|i| {
            let software_version = Some(i.software_version.clone()).filter(|s| !s.is_empty());
            InstrumentInfo {
                manufacturer: Some("Thermo Fisher Scientific".into()),
                model: full_model(&i.model, &i.model_2, &i.serial_number),
                // Every depositor mzML names the software of this version `Xcalibur`.
                software: software_version.as_ref().map(|_| "Xcalibur".to_string()),
                software_version,
                detector: None,
            }
        });
        let mut extra = BTreeMap::new();
        let mut put = |k: &str, v: Value| {
            if !v.is_null() {
                extra.insert(k.to_string(), v);
            }
        };
        let seq = &self.sequence;
        put("sample_name", json!(seq.sample_name()));
        put("sample_id", json!(seq.sample_id()));
        put("comment", json!(seq.comment()));
        put("vial", json!(seq.vial()));
        put(
            "injection_volume",
            json!(
                (seq.injection.injection_volume != 0.0).then_some(seq.injection.injection_volume)
            ),
        );
        // The file-info block holds the start in UTC, the audit stamp the same instant on the
        // acquisition PC's clock; their difference is that PC's UTC offset.
        put(
            "acquired_at",
            json!(
                self.file_info
                    .utc_time_iso()
                    .or_else(|| self.header.created.iso())
            ),
        );
        let offset_min = match (
            self.file_info.utc_unix_seconds(),
            crate::layout::filetime_unix_seconds(self.header.created.filetime),
        ) {
            (Some(u), Some(l)) => Some(((l - u) as f64 / 900.0).round() as i64 * 15),
            _ => None,
        };
        if let (Some(off), Some(local)) = (offset_min, self.header.created.iso()) {
            let sign = if off < 0 { '-' } else { '+' };
            put(
                "acquired_at_local",
                json!(format!(
                    "{local}{sign}{:02}:{:02}",
                    off.abs() / 60,
                    off.abs() % 60
                )),
            );
        }
        put("file_modified_at", json!(self.header.modified.iso()));
        put(
            "acquisition_computer",
            json!(Some(&self.file_info.computer_name).filter(|s| !s.is_empty())),
        );
        put(
            "operator",
            json!(Some(&self.header.created.account).filter(|s| !s.is_empty())),
        );
        if let Some(i) = &self.instrument {
            put(
                "instrument_serial",
                json!(Some(&i.serial_number).filter(|s| !s.is_empty())),
            );
        }
        put("file_version", json!(self.header.version));
        let templates = self.method_table.as_ref().map(|t| {
            let mut per_segment: BTreeMap<u32, u32> = BTreeMap::new();
            for tm in &t.templates {
                *per_segment.entry(tm.segment).or_default() += 1;
            }
            per_segment.into_values().collect::<Vec<_>>()
        });
        put(
            "method_summary",
            json!({
                "instrument_method": seq.instrument_method(),
                "segments": self.run_header.segment_count,
                "events_per_segment": templates,
                "method_length_min": self.run_header.run_values[1],
                "embedded_method_bytes": self.method_file.map(|(_, len)| len),
                "devices": self.method.as_ref().map(|m| m.devices.iter().map(|(d, _)| d.clone()).collect::<Vec<_>>()),
                "ms_run_time_min": self.method.as_ref().and_then(ms_run_time),
                "gradient": self.method.as_ref()
                    .and_then(|m| crate::gradient::from_device_texts(&m.device_texts))
                    .map(|g| g.to_json()),
            }),
        );
        put("ms_level_counts", json!(level_counts));
        put("polarities", json!(polarities));
        put("analyzers", json!(analyzers));
        put("ion_sources", json!(sources));
        put("profile_scans", json!(profile));
        put("centroid_scans", json!(self.events.len() as u64 - profile));
        put("ms1_scan_filters", json!(filters));
        put(
            "mz_range",
            json!([self.run_header.low_mz, self.run_header.high_mz]),
        );
        put(
            "max_total_ion_current",
            json!(self.run_header.max_total_ion_current),
        );
        SpectraInfo {
            index: 0,
            name: seq.sample_name().map(str::to_string).or_else(|| {
                self.path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
            }),
            scan_count: self.index.len() as u64,
            ms_levels: levels.into_iter().filter(|l| *l > 0).collect(),
            rt_range_s,
            instrument,
            extra,
        }
    }
}

/// The instrument id's model, or its second model string when that one holds every word of it
/// and more (`TSQ` / `TSQ Vantage Standard`, `Exploris` / `Orbitrap Exploris 240`). A trailing
/// word equal to the serial number is not part of the model (`Orbitrap Exploris MB11229C` /
/// `Orbitrap Exploris 120`; `docs/formats/thermo-raw.md` § Instrument id).
fn full_model(model: &str, model_2: &str, serial: &str) -> Option<String> {
    let serial = serial.trim();
    let m = model.trim();
    let m = match m.rsplit_once(' ') {
        Some((head, last)) if !serial.is_empty() && last == serial => head.trim_end(),
        _ => m,
    };
    let m2 = model_2.trim();
    let words2: Vec<&str> = m2.split_whitespace().collect();
    let extends = m2.len() > m.len() && m.split_whitespace().all(|w| words2.contains(&w));
    // An instrument name that is the model family plus a site label (`Orbitrap Exploris Slot
    // #10076` / `Orbitrap Exploris 240`): same first word, and the name lacks a word of the model.
    let words: Vec<&str> = m.split_whitespace().collect();
    let labelled = !words2.is_empty()
        && words.first() == words2.first()
        && !words2.iter().all(|w| words.contains(w));
    let pick = if extends || labelled { m2 } else { m };
    (!pick.is_empty()).then(|| pick.to_string())
}

/// `MS Run Time (min): 19.00` from a device's method text, when present.
fn ms_run_time(m: &MethodDocument) -> Option<f64> {
    m.device_texts.iter().find_map(|(_, t)| {
        t.lines().find_map(|l| {
            let rest = l.trim().strip_prefix("MS Run Time (min):")?;
            rest.trim().parse::<f64>().ok()
        })
    })
}

/// Read `[start, end)` if it is a sane, bounded region.
fn region(file: &mut SourceFile, path: &Path, start: u64, end: u64, what: &str) -> Result<Vec<u8>> {
    if end < start {
        return Err(corrupt(
            start,
            format!("{what}: region end {end} precedes its start"),
        ));
    }
    let len = (end - start).min(MAX_REGION);
    read_at(file, path, start, len, what)
}

fn hex(b: &[u8]) -> String {
    b.iter()
        .fold(String::with_capacity(b.len() * 2), |mut s, x| {
            let _ = write!(s, "{x:02x}");
            s
        })
}

impl Dataset for ThermoDataset {
    fn info(&self) -> Result<FileInfo> {
        self.broken_err()?;
        let mut notes: Vec<String> = self
            .problems
            .iter()
            .map(|f| format!("{}: {}", f.code, f.message))
            .collect();
        let method_text = self.method_text();
        let traces: Vec<_> = self
            .trace_detectors()
            .into_iter()
            .enumerate()
            .map(|(i, d)| crate::detectors::trace_info(i as u32, d, &method_text))
            .collect();
        if !traces.is_empty() {
            notes.push(format!(
                "{} detector trace(s) besides the mass spectrometer ({}): read them with `trace`",
                traces.len(),
                traces
                    .iter()
                    .filter_map(|t| t.name.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if self.header.version < 63 && self.file_info.controller_count > 1 {
            notes.push(format!(
                "{} data controllers recorded; before file version 63 only the mass-spectrometer run is exposed",
                self.file_info.controller_count
            ));
        }
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.file_len,
            format: ThermoRawReader.descriptor(),
            format_version: Some(self.header.version.to_string()),
            images: Vec::new(),
            tables: Vec::new(),
            spectra: vec![self.spectra_info()],
            traces,
            plane_count: 0,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        self.broken_err()?;
        let h = &self.header;
        let stamp = |s: &crate::layout::AuditStamp| json!({"time": s.iso(), "filetime": s.filetime, "account": s.account, "account_2": s.account_2, "stamp_value": s.stamp_value});
        let seq = &self.sequence;
        let rh = &self.run_header;
        let fi = &self.file_info;
        let mut v = json!({
            "file_header": {
                "version": h.version,
                "header_words": h.header_words,
                "created": stamp(&h.created),
                "modified": stamp(&h.modified),
                "header_checksum": format!("{:08x}", h.header_checksum()),
                "header_value": h.header_value,
                "header_text": h.header_text,
            },
            "sequence_row": {
                "row_number": seq.injection.row_number,
                "injection_words": seq.injection.injection_words,
                "vial_label": seq.injection.vial_label,
                "injection_volume": seq.injection.injection_volume,
                "sample_weight": seq.injection.sample_weight,
                "sample_volume": seq.injection.sample_volume,
                "internal_standard_amount": seq.injection.internal_standard_amount,
                "dilution_factor": seq.injection.dilution_factor,
                "sample_name": seq.sample_name(),
                "sample_id": seq.sample_id(),
                "comment": seq.comment(),
                "user_labels": seq.user_labels(),
                "instrument_method": seq.instrument_method(),
                "processing_method": seq.processing_method(),
                "original_file_name": seq.original_file_name(),
                "original_path": seq.original_path(),
                "vial": seq.vial(),
                "texts": seq.texts,
                "row_value": seq.row_value,
            },
            "autosampler": {
                "numbers": self.autosampler.numbers,
                "vial_index": self.autosampler.vial_index(),
                "tray_description": self.autosampler.tray_description,
            },
            "file_info": {
                "method_file_present": fi.method_file_present,
                "utc_time": fi.utc_time_iso(),
                "data_address": fi.data_address,
                "run_header_address": fi.run_header_address,
                "run_header_address_2": fi.run_header_address_2,
                "controller_count": fi.controller_count,
                "controllers": fi.controllers.iter().map(|c| json!({"controller_type": c.controller_type, "controller_index": c.controller_index, "run_header_address": c.run_header_address})).collect::<Vec<_>>(),
                "label_headings": fi.label_headings,
                "computer_name": fi.computer_name,
            },
            "run_header": {
                "address": rh.address,
                "first_scan": rh.first_scan,
                "last_scan": rh.last_scan,
                "instrument_log_count": rh.instrument_log_count,
                "error_log_count": rh.error_log_count,
                "max_total_ion_current": rh.max_total_ion_current,
                "low_mz": rh.low_mz,
                "high_mz": rh.high_mz,
                "start_time_min": rh.start_time_min,
                "end_time_min": rh.end_time_min,
                "sample_texts": rh.sample_texts,
                "device_files": rh.device_files,
                "run_values": rh.run_values,
                "scan_event_count": rh.scan_event_count,
                "scan_parameter_count": rh.scan_parameter_count,
                "segment_count": rh.segment_count,
                "display_words": rh.display_words,
                "sample_words": rh.sample_words,
                "streams": {
                    "scan_index": rh.streams.scan_index,
                    "scan_data": rh.streams.scan_data,
                    "instrument_log": rh.streams.instrument_log,
                    "error_log": rh.streams.error_log,
                    "scan_events": rh.streams.scan_events,
                    "scan_parameters": rh.streams.scan_parameters,
                },
                "own_address": rh.own_address,
            },
        });
        if let Some(i) = &self.instrument {
            v["instrument"] = json!({
                "id_words": i.id_words,
                "model": i.model,
                "model_2": i.model_2,
                "serial_number": i.serial_number,
                "software_version": i.software_version,
                "tags": i.tags,
            });
        }
        if let Some(lh) = &self.instrument_log_header {
            let mut log = json!({
                "columns": lh.fields.iter().map(|f| json!({"label": f.label, "kind": f.kind, "width": f.width})).collect::<Vec<_>>(),
                "record_count": rh.instrument_log_count,
            });
            // First record, decoded (a later record would need another read; the first is representative).
            if let (Some(len), Ok(mut f)) = (lh.record_len(), self.fs.open(&self.path))
                && rh.instrument_log_count > 0
                && let Ok(buf) = read_at(
                    &mut f,
                    &self.path,
                    rh.streams.instrument_log,
                    4 + len as u64,
                    "instrument log",
                )
            {
                let mut c = Cursor::new(&buf, rh.streams.instrument_log);
                if let (Ok(t), Ok(rec)) = (c.f32(), lh.decode(&mut c)) {
                    let obj: serde_json::Map<String, Value> = rec.into_iter().collect();
                    log["first_record"] = json!({"rt_min": t, "values": obj});
                }
            }
            v["instrument_log"] = log;
        }
        v["error_log"] = json!({
            "lead_value": self.error_log_lead,
            "entries": self.errors
                .iter()
                .map(|e| json!({"rt_min": e.rt_min, "message": e.message}))
                .collect::<Vec<_>>(),
        });
        if let Some(t) = &self.method_table {
            v["method_table"] = json!({
                "address": t.address,
                "templates": t.templates.iter().map(|tm| json!({
                    "segment": tm.segment,
                    "event": tm.event,
                    "preamble": hex(&tm.preamble),
                    "template_words": tm.template_words,
                    "scan_range": tm.scan_range,
                })).collect::<Vec<_>>(),
            });
        }
        if let Some(ph) = &self.scan_parameter_header {
            v["scan_parameter_columns"] = json!(
                ph.fields
                    .iter()
                    .map(|f| json!({"label": f.label, "kind": f.kind, "width": f.width}))
                    .collect::<Vec<_>>()
            );
        }
        if let Some((off, len)) = self.method_file {
            v["method_file"] = json!({"offset": off, "bytes": len});
        }
        if let Some(m) = &self.method {
            v["instrument_method"] = json!({
                "container_offset": m.container_offset,
                "container_size": m.container_size,
                "source_path": m.source_path,
                "devices": m.devices.iter().map(|(d, s)| json!({"name": d, "storage": s})).collect::<Vec<_>>(),
                "streams": m.streams.iter().map(|(p, n)| json!({"path": p, "bytes": n})).collect::<Vec<_>>(),
                "text": m.device_texts.iter().map(|(d, t)| (d.clone(), Value::from(t.clone()))).collect::<serde_json::Map<_, _>>(),
            });
        }
        if let Some(e) = self.events.first() {
            v["first_scan_event"] = json!({
                "preamble": hex(&e.preamble),
                "reactions": e.reactions.iter().map(|r| json!({"precursor_mz": r.precursor_mz, "isolation_width": r.isolation_width, "energy": r.energy, "reaction_words": r.reaction_words, "reaction_values": r.reaction_values})).collect::<Vec<_>>(),
                "scan_ranges": e.scan_ranges,
                "coefficients": e.coefficients,
                "tail_items": e.tail_items,
                "tail_words": e.tail_words,
            });
        }
        Ok(v)
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut m = ProvenanceMap::new();
        for k in [
            "spectra[0].scan_count",
            "spectra[0].rt_range_s",
            "spectra[0].instrument.model",
            "spectra[0].instrument.software_version",
            "spectra[0].extra.acquired_at",
            "spectra[0].extra.instrument_serial",
            "spectra[0].extra.method_summary",
        ] {
            m.insert(k.into(), Source::PriorArt);
        }
        for k in [
            "spectra[0].ms_levels",
            "spectra[0].extra.sample_name",
            "spectra[0].extra.ms1_scan_filters",
            "spectra[0].instrument.software",
            "spectra[0].extra.method_summary.gradient",
            "spectra[*].scan_filter",
            "spectra[*].mz",
            "spectra[*].intensity",
            "spectra[*].precursor_mz",
            "spectra[*].precursor_charge",
            "spectra[*].polarity",
        ] {
            m.insert(k.into(), Source::Inferred);
        }
        m
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        self.broken_err()?;
        let mut out = Vec::new();
        let mut add = |kind: &str, name: &str, offset: u64, size: Option<u64>, details: Value| {
            out.push(LsEntry {
                kind: kind.into(),
                name: name.into(),
                offset: Some(offset),
                size,
                image: None,
                details,
            });
        };
        let rh = &self.run_header;
        let s = rh.streams;
        add(
            "header",
            "file header",
            0,
            Some(FILE_HEADER_LEN),
            json!({"version": self.header.version}),
        );
        add(
            "metadata",
            "sequence row",
            FILE_HEADER_LEN,
            None,
            Value::Null,
        );
        if let Some((off, len)) = self.method_file {
            add(
                "attachment",
                "instrument method container",
                off,
                Some(len),
                Value::Null,
            );
        }
        if let Some(m) = &self.method {
            for (p, n) in &m.streams {
                add(
                    "method",
                    p,
                    m.container_offset,
                    Some(*n),
                    json!({"within": "compound file"}),
                );
            }
        }
        let first_stream_after_data = [
            s.scan_index,
            s.instrument_log,
            s.error_log,
            s.scan_events,
            s.scan_parameters,
            rh.address,
        ]
        .into_iter()
        .filter(|a| *a > s.scan_data)
        .min()
        .unwrap_or(self.file_len);
        add(
            "stream",
            "scan data",
            s.scan_data,
            Some(first_stream_after_data - s.scan_data),
            json!({"scans": self.index.len()}),
        );
        add(
            "header",
            "run header",
            rh.address,
            Some(rh.byte_len),
            Value::Null,
        );
        if let Some(i) = &self.instrument {
            add(
                "metadata",
                "instrument id",
                rh.address + rh.byte_len,
                None,
                json!({"model": i.model, "serial_number": i.serial_number}),
            );
        }
        if let Some(lh) = &self.instrument_log_header {
            add(
                "metadata",
                "instrument log columns",
                lh.address,
                Some(s.instrument_log.saturating_sub(lh.address)),
                json!({"columns": lh.fields.len()}),
            );
        }
        add(
            "stream",
            "instrument log",
            s.instrument_log,
            Some(s.error_log.saturating_sub(s.instrument_log)),
            json!({"records": rh.instrument_log_count}),
        );
        add(
            "stream",
            "error log",
            s.error_log,
            None,
            json!({"entries": rh.error_log_count}),
        );
        if let Some(t) = &self.method_table {
            add(
                "metadata",
                "segment and event table",
                t.address,
                None,
                json!({"templates": t.templates.len(), "segments": rh.segment_count}),
            );
        }
        if let Some(ph) = &self.scan_parameter_header {
            add(
                "metadata",
                "scan parameter columns",
                ph.address,
                None,
                json!({"columns": ph.fields.len(), "record_bytes": ph.record_len()}),
            );
        }
        add(
            "index",
            "scan index",
            s.scan_index,
            Some(self.index.len() as u64 * scan_index_entry_len(self.header.version) as u64),
            json!({"entries": self.index.len()}),
        );
        add(
            "stream",
            "scan events",
            s.scan_events,
            Some(self.events_end - s.scan_events),
            json!({"events": self.events.len()}),
        );
        if let Some(len) = self
            .scan_parameter_header
            .as_ref()
            .and_then(GenericHeader::record_len)
        {
            add(
                "stream",
                "scan parameters",
                s.scan_parameters,
                Some(len as u64 * self.index.len() as u64),
                json!({"record_bytes": len}),
            );
        }
        for sc in self.scans() {
            add(
                "scan",
                &format!("scan {}", sc.scan_number),
                sc.data_offset,
                Some(sc.data_len),
                json!({"ms_level": sc.ms_level, "rt_s": sc.rt_s, "polarity": sc.polarity, "filter": sc.scan_filter}),
            );
        }
        Ok(out)
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::unsupported(
            FORMAT_ID,
            "image planes",
            "Thermo .raw files hold mass spectra; use `openreadout spectra FILE --scan N` or `export --to mzml`.",
        ))
    }

    fn read_spectrum(&mut self, index: u32, spectrum: u64) -> Result<Spectrum> {
        self.read_spectrum_view(index, spectrum, SpectrumView::Primary)
    }

    fn read_spectrum_view(
        &mut self,
        index: u32,
        spectrum: u64,
        view: SpectrumView,
    ) -> Result<Spectrum> {
        self.broken_err()?;
        let i = self.check_scan(index, spectrum)?;
        self.read_scan(i, view, !self.exclude_flagged)
    }

    fn exclude_flagged_peaks(&mut self, exclude: bool) -> bool {
        self.set_exclude_flagged_peaks(exclude);
        true
    }

    fn visit_scan_headers(
        &mut self,
        run: u32,
        first: u64,
        visit: &mut dyn FnMut(openreadout_core::ScanHeader) -> bool,
    ) -> Result<bool> {
        self.broken_err()?;
        if run != 0 {
            return Err(Error::Usage(format!(
                "run {run} does not exist; this file exposes one run (index 0)"
            )));
        }
        let n = self.index.len().min(self.events.len());
        for i in usize::try_from(first).unwrap_or(usize::MAX)..n {
            if !visit(self.scan_header(i)?) {
                break;
            }
        }
        Ok(true)
    }

    fn spectrum_ms_levels(&mut self, run: u32) -> Result<Option<Vec<u32>>> {
        self.broken_err()?;
        // One scan event per index row: the level is in the event, no packet is read.
        Ok((run == 0 && self.events.len() == self.index.len())
            .then(|| self.events.iter().map(ScanEvent::ms_level).collect()))
    }

    fn read_trace(
        &mut self,
        index: u32,
        sweep: u32,
        first_sample: u64,
        max_samples: u64,
    ) -> Result<Trace> {
        self.broken_err()?;
        let d = self
            .trace_detectors()
            .get(index as usize)
            .map(|d| (*d).clone())
            .ok_or_else(|| {
                Error::Usage(format!(
                    "trace {index} does not exist; `info` → traces[] lists {}",
                    self.trace_detectors().len()
                ))
            })?;
        if sweep != 0 {
            return Err(Error::Usage(format!(
                "trace {index} has one sweep (0); asked for {sweep}"
            )));
        }
        let first = first_sample.min(d.scan_count);
        let n = max_samples.min(d.scan_count - first);
        let path = self.path.clone();
        let (times, values) = crate::detectors::read_samples(self.handle()?, &path, &d, first, n)?;
        let mut channels = Vec::with_capacity(values.len() + 1);
        if d.kind == crate::detectors::DetectorKind::Analog {
            channels.push(times.iter().map(|t| t * 60.0).collect());
        }
        channels.extend(values);
        Ok(Trace {
            trace: index,
            sweep,
            first_sample: first,
            channels,
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), FORMAT_ID);
        if let Some((detail, at)) = &self.broken {
            r.performed("file signature and version");
            r.performed("header chain, run header and stream addresses");
            let code = if detail.contains("truncated") || detail.contains("beyond the end") {
                "truncated"
            } else {
                "structure_unreadable"
            };
            let mut f = Finding::error(code, detail.clone());
            if let Some(o) = at {
                f = f.at(*o);
            }
            r.push(f);
            return Ok(r);
        }
        for p in &self.problems {
            r.push(p.clone());
        }
        let path = self.path.clone();
        let file_len = self.file_len;
        let version = self.header.version;

        r.performed("file signature and version");
        r.performed("header checksum (Adler-32 over the first 10 MiB, checksum field zeroed)");
        let span = file_len.min(CHECKSUM_SPAN);
        let mut head = read_at(self.handle()?, &path, 0, span, "checksum span")?;
        let at = CHECKSUM_OFFSET as usize;
        head[at..at + 4].fill(0);
        let computed = adler32(0, &head);
        if computed != self.header.header_checksum() {
            r.push(
                Finding::error(
                    "checksum_mismatch",
                    format!(
                        "stored header checksum {:08x}, computed {computed:08x}: the first {span} bytes changed after writing",
                        self.header.header_checksum()
                    ),
                )
                .at(CHECKSUM_OFFSET),
            );
        }

        r.performed("run header self-address and stream addresses within the file");
        let rh = self.run_header.clone();
        let s = rh.streams;
        if u64::from(rh.scan_event_count) != self.scan_count()
            || u64::from(rh.scan_parameter_count) != self.scan_count()
        {
            r.push(Finding::warning(
                "count_mismatch",
                format!(
                    "run header: {} scans, {} scan events, {} parameter records",
                    self.scan_count(),
                    rh.scan_event_count,
                    rh.scan_parameter_count
                ),
            ));
        }

        r.performed("scan index: sequence numbers, retention-time order, packet bounds");
        let data_end = [
            s.scan_index,
            s.instrument_log,
            s.error_log,
            s.scan_events,
            s.scan_parameters,
            rh.address,
        ]
        .into_iter()
        .filter(|a| *a > s.scan_data)
        .min()
        .unwrap_or(file_len);
        let mut prev_rt = f64::NEG_INFINITY;
        let mut bad = 0usize;
        for (i, e) in self.index.iter().enumerate() {
            let scan = self.scan_number(i);
            let mut complain = |f: Finding| {
                bad += 1;
                if bad <= 20 {
                    r.push(f);
                }
            };
            if e.scan_index as usize != i {
                complain(Finding::error(
                    "index_sequence",
                    format!("index row {i} carries position {}", e.scan_index),
                ));
            }
            if e.rt_min < prev_rt {
                complain(Finding::warning(
                    "rt_order",
                    format!("scan {scan}: retention time decreases"),
                ));
            }
            prev_rt = e.rt_min;
            let (start, len) = self.scan_extent(i);
            let end = start.saturating_add(len);
            if end > data_end || end > file_len {
                complain(
                    Finding::error(
                        "packet_out_of_bounds",
                        format!(
                            "scan {scan}: packet {start}..{end} runs past the scan-data stream (ends {data_end}) or the file ({file_len})"
                        ),
                    )
                    .at(start),
                );
            }
        }
        if bad > 20 {
            r.push(Finding::error(
                "more_problems",
                format!("{} further scan-index problems not listed", bad - 20),
            ));
        }

        r.performed("scan events decode and end exactly where the per-scan parameters begin");
        if s.scan_parameters > s.scan_events && self.events_end != s.scan_parameters {
            r.push(
                Finding::error(
                    "scan_events_length",
                    format!(
                        "scan events end at {} but parameters start at {}",
                        self.events_end, s.scan_parameters
                    ),
                )
                .at(self.events_end),
            );
        }

        r.performed("per-scan parameter records fit in the file");
        if let Some(len) = self
            .scan_parameter_header
            .as_ref()
            .and_then(GenericHeader::record_len)
        {
            let end = s.scan_parameters + len as u64 * self.scan_count();
            if end > file_len {
                r.push(
                    Finding::error(
                        "truncated",
                        format!(
                            "per-scan parameters need {end} bytes, file has {file_len} (truncated?)"
                        ),
                    )
                    .at(s.scan_parameters),
                );
            }
        }

        r.performed("instrument log records fill the space before the error log");
        if let Some(len) = self
            .instrument_log_header
            .as_ref()
            .and_then(GenericHeader::record_len)
        {
            let expect = s.instrument_log + u64::from(rh.instrument_log_count) * (4 + len as u64);
            if expect != s.error_log {
                r.push(Finding::warning(
                    "instrument_log_length",
                    format!(
                        "{} log records of {} bytes end at {expect}, error log starts at {}",
                        rh.instrument_log_count,
                        4 + len,
                        s.error_log
                    ),
                ));
            }
        }

        r.performed("every scan's packet header agrees with its index size");
        let mut bad_packets = 0usize;
        for i in 0..self.index.len() {
            let e = self.index[i];
            let start = s.scan_data.saturating_add(e.data_offset);
            if e.kind_code == WINDOW_RECORD_KIND {
                if let Err(err) = self.window_packet(i) {
                    bad_packets += 1;
                    if bad_packets <= 10 {
                        r.push(Finding::error("packet_size", err.to_string()).at(start));
                    }
                }
                continue;
            }
            if start.saturating_add(PACKET_HEADER_LEN as u64) > file_len {
                bad_packets += 1;
                continue;
            }
            let buf = read_at(
                self.handle()?,
                &path,
                start,
                PACKET_HEADER_LEN as u64,
                "packet header",
            )?;
            let ph = parse_packet_header(&mut Cursor::new(&buf, start))?;
            if ph.packet_len() != u64::from(e.data_size) {
                bad_packets += 1;
                if bad_packets <= 10 {
                    r.push(
                        Finding::error(
                            "packet_size",
                            format!(
                                "scan {}: packet header describes {} bytes, index says {}",
                                self.scan_number(i),
                                ph.packet_len(),
                                e.data_size
                            ),
                        )
                        .at(start),
                    );
                }
            }
        }
        if bad_packets > 10 {
            r.push(Finding::error(
                "more_problems",
                format!("{} further packet problems not listed", bad_packets - 10),
            ));
        }
        if version < 63 {
            r.push(Finding::info(
                "unvalidated_version",
                format!("version {version} layouts are inferred, not validated"),
            ));
        }
        let dets: Vec<_> = self.trace_detectors().into_iter().cloned().collect();
        if !dets.is_empty() {
            r.performed(
                "detector controllers: index rows in order, every sample decoded, stored values (UV/analog sample, PDA total) equal to the decoded samples",
            );
        }
        for d in &dets {
            if let Err(e) = self.check_detector(d, &mut r) {
                r.push(Finding::error(
                    "detector_unreadable",
                    format!("{}: {e}", d.channel_name()),
                ));
            }
        }
        Ok(r)
    }
}

impl ThermoDataset {
    /// `check` for one detector controller: every sample decodes; the index's stored value
    /// equals the sample (UV channel, single analog value) or the spectrum's sum within its
    /// truncation (PDA: one stored unit per point); times do not decrease.
    #[allow(clippy::many_single_char_names)] // d = detector, r = report; n, k, t: counters
    fn check_detector(
        &mut self,
        d: &crate::detectors::Detector,
        r: &mut CheckReport,
    ) -> Result<()> {
        use crate::detectors::{DetectorKind, read_rows, read_samples};
        const BATCH: u64 = 4096;
        let path = self.path.clone();
        let (mut bad_value, mut bad_time, mut first_bad) = (0u64, 0u64, None);
        let mut last_t = f64::NEG_INFINITY;
        // traces report the samples on a regular grid (`axis`): the recorded times must agree
        let step = d.step_min();
        let mut off_grid = 0.0f64;
        let mut at = 0;
        while at < d.scan_count {
            let n = BATCH.min(d.scan_count - at);
            let rows = read_rows(self.handle()?, &path, d, at, n)?;
            let (times, ch) = read_samples(self.handle()?, &path, d, at, n)?;
            for (k, row) in rows.iter().enumerate() {
                let t = times[k];
                if t < last_t {
                    bad_time += 1;
                }
                last_t = t;
                let expect = d.start_min + step * (at + k as u64) as f64;
                off_grid = off_grid.max((t - expect).abs());
                let ok = match d.kind {
                    // value-only channels (before v64) store 0 in the index: nothing to compare
                    DetectorKind::Channel if !d.sample_has_time => true,
                    DetectorKind::Channel => ch[0][k].to_bits() == row.stored_value.to_bits(),
                    DetectorKind::Analog if ch.len() == 1 => {
                        ch[0][k].to_bits() == row.stored_value.to_bits()
                    }
                    DetectorKind::Pda => {
                        let g = d.pda.unwrap_or_default();
                        let sum: f64 =
                            ch.iter().map(|c| c[k]).sum::<f64>() / crate::detectors::unit_scale(&g);
                        (sum - row.stored_value).abs() <= f64::from(g.points)
                    }
                    _ => true,
                };
                if !ok {
                    bad_value += 1;
                    first_bad.get_or_insert(row.scan_number);
                }
            }
            at += n;
        }
        if bad_value > 0 {
            r.push(Finding::error(
                "detector_value_mismatch",
                format!(
                    "{}: {bad_value} of {} samples differ from the value stored in the index (first: scan {})",
                    d.channel_name(),
                    d.scan_count,
                    first_bad.unwrap_or(0)
                ),
            ));
        }
        if d.kind != DetectorKind::Analog && step > 0.0 && off_grid > step / 2.0 {
            r.push(Finding::warning(
                "detector_time_grid",
                format!(
                    "{}: recorded sample times depart from the regular grid by up to {:.4} s (half a sampling interval is {:.4} s); times from `axis` are approximate",
                    d.channel_name(),
                    off_grid * 60.0,
                    step * 30.0
                ),
            ));
        }
        if bad_time > 0 {
            r.push(Finding::warning(
                "detector_time_order",
                format!("{}: {bad_time} samples go back in time", d.channel_name()),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::full_model;

    #[test]
    fn model_name_prefers_the_longer_spelling() {
        assert_eq!(
            full_model("TSQ", "TSQ Vantage Standard", "TQU00001").as_deref(),
            Some("TSQ Vantage Standard")
        );
        assert_eq!(
            full_model("Thermo Exactive Orbitrap", "Exactive Orbitrap", "").as_deref(),
            Some("Thermo Exactive Orbitrap")
        );
        assert_eq!(full_model("LTQ", "LTQX", "").as_deref(), Some("LTQ"));
        assert_eq!(full_model("", "Orbitrap", "").as_deref(), Some("Orbitrap"));
        assert_eq!(full_model("", "", ""), None);
        assert_eq!(
            full_model("Exploris", "Orbitrap Exploris 240", "MM10076C").as_deref(),
            Some("Orbitrap Exploris 240")
        );
        assert_eq!(
            full_model(
                "Orbitrap Exploris MB11229C",
                "Orbitrap Exploris 120",
                "MB11229C"
            )
            .as_deref(),
            Some("Orbitrap Exploris 120")
        );
        assert_eq!(
            full_model("Orbitrap Exploris MA11033C", "", "MA11033C").as_deref(),
            Some("Orbitrap Exploris")
        );
        // mtbls12283: the name carries a site label; the model string is the model
        assert_eq!(
            full_model(
                "Orbitrap Exploris Slot #10076",
                "Orbitrap Exploris 240",
                "MM10076C"
            )
            .as_deref(),
            Some("Orbitrap Exploris 240")
        );
        assert_eq!(
            full_model("Q Exactive HF Orbitrap", "Q Exactive HF Orbitrap", "").as_deref(),
            Some("Q Exactive HF Orbitrap")
        );
    }
}
