//! Fixed and pointer-reached structures of a `.raw` file, in file order. See `docs/formats/thermo-raw.md`.
//!
//! Every name here is our own vocabulary; where a field's meaning is not known it is called
//! `words`/`texts`/`values` and kept verbatim so `info --view full` can show it.

use openreadout_core::time::{civil_from_days, days_from_civil};
use openreadout_core::{Error, Result};

use crate::FORMAT_ID;
use crate::bytes::Cursor;

/// Size of the fixed file header at offset 0.
pub const FILE_HEADER_LEN: u64 = 1356;
/// Offset of the header checksum (Adler-32 over the first [`CHECKSUM_SPAN`] bytes, this field zeroed).
pub const CHECKSUM_OFFSET: u64 = 148;
/// Number of leading bytes covered by the header checksum.
pub const CHECKSUM_SPAN: u64 = 10 * 1024 * 1024;
/// File versions whose layout we decode and have validated on corpus files.
pub const SUPPORTED_VERSIONS: [u32; 6] = [57, 61, 62, 63, 64, 66];

/// One of the two time stamps in the file header.
#[derive(Debug, Clone, Default)]
pub struct AuditStamp {
    /// Windows FILETIME (100 ns ticks since 1601-01-01), local clock of the acquisition PC.
    pub filetime: u64,
    /// Windows account (or instrument name) that wrote the file.
    pub account: String,
    pub account_2: String,
    /// In the first stamp: the header checksum. In the second: observed 1.
    pub stamp_value: u32,
}

impl AuditStamp {
    /// ISO-8601 rendering of `filetime` (no zone: the value is local time).
    pub fn iso(&self) -> Option<String> {
        filetime_iso(self.filetime)
    }
}

/// The 1356-byte header at offset 0.
#[derive(Debug, Clone, Default)]
pub struct FileHeader {
    pub version: u32,
    /// Four u32 between the signature and the version (observed 0, 0, 0, 0x80000 in top-level files).
    pub header_words: [u32; 4],
    pub created: AuditStamp,
    pub modified: AuditStamp,
    pub header_value: u32,
    /// Free text at the end of the header (usually empty).
    pub header_text: String,
}

impl FileHeader {
    pub fn header_checksum(&self) -> u32 {
        self.created.stamp_value
    }
}

/// `true` if `head` starts with the 2-byte marker and the UTF-16 word `Finnigan`.
pub fn has_signature(head: &[u8]) -> bool {
    head.len() >= crate::SIGNATURE.len() && head[..crate::SIGNATURE.len()] == crate::SIGNATURE
}

pub fn parse_file_header(c: &mut Cursor<'_>) -> Result<FileHeader> {
    let at = c.offset();
    let sig = c.take(crate::SIGNATURE.len())?;
    if sig != crate::SIGNATURE {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            at,
            "missing the Finnigan file signature",
        ));
    }
    c.skip(2)?; // terminating NUL of the signature
    let mut header_words = [0u32; 4];
    for w in &mut header_words {
        *w = c.u32()?;
    }
    let version = c.u32()?;
    let created = audit_stamp(c)?;
    let modified = audit_stamp(c)?;
    let header_value = c.u32()?;
    c.skip(60)?;
    let header_text = c.utf16_fixed(1028)?;
    Ok(FileHeader {
        version,
        header_words,
        created,
        modified,
        header_value,
        header_text,
    })
}

fn audit_stamp(c: &mut Cursor<'_>) -> Result<AuditStamp> {
    Ok(AuditStamp {
        filetime: c.u64()?,
        account: c.utf16_fixed(50)?,
        account_2: c.utf16_fixed(50)?,
        stamp_value: c.u32()?,
    })
}

/// Injection parameters at the start of the sequence row (64 bytes).
#[derive(Debug, Clone, Default)]
pub struct InjectionRecord {
    pub injection_words: [u32; 2],
    /// Row of the acquisition sequence.
    pub row_number: u32,
    pub vial_label: String,
    pub injection_volume: f64,
    pub sample_weight: f64,
    pub sample_volume: f64,
    pub internal_standard_amount: f64,
    pub dilution_factor: f64,
}

/// The acquisition-sequence row that produced this file.
#[derive(Debug, Clone, Default)]
pub struct SequenceRow {
    pub injection: InjectionRecord,
    /// Every length-prefixed string in file order (see the accessors for the known slots).
    pub texts: Vec<String>,
    /// The u32 between string 16 and string 17 (versions >= 57; observed 0).
    pub row_value: u32,
}

impl SequenceRow {
    fn slot(&self, i: usize) -> Option<&str> {
        self.texts
            .get(i)
            .map(String::as_str)
            .filter(|s| !s.is_empty())
    }
    /// Per-injection label typed into the sequence (observed `QC1`, `blanc`, `884_Caffeine_POS`).
    pub fn sample_name(&self) -> Option<&str> {
        self.slot(1)
    }
    pub fn sample_id(&self) -> Option<&str> {
        self.slot(2)
    }
    pub fn comment(&self) -> Option<&str> {
        self.slot(3)
    }
    pub fn user_labels(&self) -> Vec<&str> {
        (4..9).filter_map(|i| self.slot(i)).collect()
    }
    pub fn instrument_method(&self) -> Option<&str> {
        self.slot(9)
    }
    pub fn processing_method(&self) -> Option<&str> {
        self.slot(10)
    }
    pub fn original_file_name(&self) -> Option<&str> {
        self.slot(11)
    }
    pub fn original_path(&self) -> Option<&str> {
        self.slot(12)
    }
    pub fn vial(&self) -> Option<&str> {
        self.slot(13)
    }
}

pub fn parse_sequence_row(c: &mut Cursor<'_>, version: u32) -> Result<SequenceRow> {
    let w0 = c.u32()?;
    let row_number = c.u32()?;
    let w1 = c.u32()?;
    let injection = InjectionRecord {
        injection_words: [w0, w1],
        row_number,
        vial_label: c.utf16_fixed(12)?,
        injection_volume: c.f64()?,
        sample_weight: c.f64()?,
        sample_volume: c.f64()?,
        internal_standard_amount: c.f64()?,
        dilution_factor: c.f64()?,
    };
    let mut texts = Vec::new();
    for _ in 0..13 {
        texts.push(c.utf16_counted()?);
    }
    let mut row_value = 0;
    if version >= 57 {
        for _ in 0..3 {
            texts.push(c.utf16_counted()?);
        }
        row_value = c.u32()?;
    }
    if version >= 60 {
        for _ in 0..15 {
            texts.push(c.utf16_counted()?);
        }
    }
    Ok(SequenceRow {
        injection,
        texts,
        row_value,
    })
}

/// Autosampler block after the sequence row.
#[derive(Debug, Clone, Default)]
pub struct AutosamplerInfo {
    /// Six u32. Observed: `[?, linear vial index, vials per tray, ?, ?, ?]`, or `u32::MAX` twice when unused.
    pub numbers: [u32; 6],
    pub tray_description: String,
}

impl AutosamplerInfo {
    /// Linear vial index (tray-major), when an autosampler was used.
    pub fn vial_index(&self) -> Option<u32> {
        (self.numbers[1] != u32::MAX && self.numbers[2] != 0).then_some(self.numbers[1])
    }
}

pub fn parse_autosampler(c: &mut Cursor<'_>) -> Result<AutosamplerInfo> {
    let mut numbers = [0u32; 6];
    for n in &mut numbers {
        *n = c.u32()?;
    }
    Ok(AutosamplerInfo {
        numbers,
        tray_description: c.utf16_counted()?,
    })
}

/// The block that holds the two top-level pointers (scan data, run header).
#[derive(Debug, Clone, Default)]
pub struct FileInfoBlock {
    /// 1 when an embedded instrument-method container follows.
    pub method_file_present: u32,
    /// Acquisition start in UTC as year, month, weekday, day, hour, minute, second, millisecond
    /// (the audit stamps hold the same instant on the acquisition PC's local clock).
    pub utc_time: [u16; 8],
    pub data_address: u64,
    pub run_header_address: u64,
    /// Number of data controllers (1 = mass spectrometer only).
    pub controller_count: u32,
    pub run_header_address_2: u64,
    /// One row per data controller (versions >= 64): which device recorded it and where its run
    /// header is.
    pub controllers: Vec<ControllerRef>,
    /// Headings the operator gave the five user labels.
    pub label_headings: Vec<String>,
    /// Name of the acquisition computer.
    pub computer_name: String,
}

impl FileInfoBlock {
    /// `YYYY-MM-DDTHH:MM:SS.mmmZ`.
    pub fn utc_time_iso(&self) -> Option<String> {
        let [y, mo, _wd, d, h, mi, s, ms] = self.utc_time;
        (y > 0 && (1..=12).contains(&mo) && (1..=31).contains(&d))
            .then(|| format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{ms:03}Z"))
    }

    /// Seconds since 1970-01-01 UTC of `utc_time`.
    pub fn utc_unix_seconds(&self) -> Option<i64> {
        let [y, mo, _wd, d, h, mi, s, _ms] = self.utc_time;
        if y == 0 || !(1..=12).contains(&mo) || !(1..=31).contains(&d) {
            return None;
        }
        let days = days_from_civil(i64::from(y), u32::from(mo), u32::from(d));
        Some(days * 86_400 + i64::from(h) * 3600 + i64::from(mi) * 60 + i64::from(s))
    }
}

/// A data controller (mass spectrometer, analog/UV detector, ...) and its run header.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ControllerRef {
    /// 0 = mass spectrometer (the only kind whose scans we read); 2 was seen for the other
    /// controllers of the Exactive and Fusion Lumos files.
    pub controller_type: u32,
    pub controller_index: u32,
    pub run_header_address: u64,
}

impl FileInfoBlock {
    /// Run header of the first mass-spectrometer controller (else of the first controller).
    pub fn ms_run_header_address(&self) -> u64 {
        self.controllers
            .iter()
            .find(|c| c.controller_type == 0)
            .map_or(self.run_header_address, |c| c.run_header_address)
    }
}

/// Most controller rows we accept.
const MAX_CONTROLLERS: u32 = 64;

/// Size of the binary part of [`FileInfoBlock`] by version.
pub fn file_info_binary_len(version: u32) -> Result<usize> {
    Ok(match version {
        0..=63 => 804,
        64 => 1840,
        65 | 66 => 1856,
        v => {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!("file version {v}"),
                "Versions 63, 64 and 66 are decoded; newer layouts have not been seen in the corpus yet.",
            ));
        }
    })
}

pub fn parse_file_info(c: &mut Cursor<'_>, version: u32) -> Result<FileInfoBlock> {
    let start = c.position();
    let len = file_info_binary_len(version)?;
    let method_file_present = c.u32()?;
    let mut utc_time = [0u16; 8];
    for v in &mut utc_time {
        *v = c.u16()?;
    }
    let mut b = FileInfoBlock {
        method_file_present,
        utc_time,
        ..Default::default()
    };
    // 20 bytes consumed; the rest differs between the 32- and 64-bit address layouts.
    let _ = c.u32()?;
    let data32 = c.u32()?;
    b.controller_count = c.u32()?;
    let _ = c.u32()?;
    let rows32 = c.position();
    c.skip(8)?;
    let rh32 = c.u32()?;
    if version >= 64 {
        c.seek_to(start + 20 + 0x314)?;
        b.data_address = c.u64()?;
        // Controller rows: u32 type, u32 index, u64 run-header address.
        for i in 0..b.controller_count.clamp(1, MAX_CONTROLLERS) {
            let row = ControllerRef {
                controller_type: c.u32()?,
                controller_index: c.u32()?,
                run_header_address: c.u64()?,
            };
            match i {
                0 => b.run_header_address = row.run_header_address,
                1 => b.run_header_address_2 = row.run_header_address,
                _ => {}
            }
            b.controllers.push(row);
        }
    } else {
        b.data_address = u64::from(data32);
        b.run_header_address = u64::from(rh32);
        // Controller rows from block offset 36: u32 type, u32 index, u32 run-header address
        // (the first row's address is the one at offset 44).
        c.seek_to(rows32)?;
        for i in 0..b.controller_count.clamp(1, MAX_CONTROLLERS) {
            let row = ControllerRef {
                controller_type: c.u32()?,
                controller_index: c.u32()?,
                run_header_address: u64::from(c.u32()?),
            };
            if i == 1 {
                b.run_header_address_2 = row.run_header_address;
            }
            b.controllers.push(row);
        }
    }
    c.seek_to(start + len)?;
    for _ in 0..5 {
        b.label_headings.push(c.utf16_counted()?);
    }
    b.computer_name = c.utf16_counted()?;
    Ok(b)
}

/// Addresses of the data streams, from the run header.
#[derive(Debug, Clone, Copy, Default)]
pub struct StreamAddresses {
    pub scan_index: u64,
    pub scan_data: u64,
    pub instrument_log: u64,
    pub error_log: u64,
    pub scan_events: u64,
    pub scan_parameters: u64,
}

/// The run header: scan range, summary values and stream addresses.
#[derive(Debug, Clone, Default)]
pub struct RunHeader {
    pub address: u64,
    pub sample_words: [u32; 2],
    pub first_scan: u32,
    pub last_scan: u32,
    pub instrument_log_count: u32,
    pub error_log_count: u32,
    pub max_total_ion_current: f64,
    pub low_mz: f64,
    pub high_mz: f64,
    /// Retention time of the first and last scan, minutes.
    pub start_time_min: f64,
    pub end_time_min: f64,
    pub sample_texts: [String; 3],
    /// Thirteen fixed-width path fields (temporary device files and the original file path).
    pub device_files: Vec<String>,
    /// Two doubles between the sixth and seventh path (observed 0.5 and the method length in minutes).
    pub run_values: [f64; 2],
    pub scan_event_count: u32,
    pub scan_parameter_count: u32,
    pub segment_count: u32,
    /// Two u32 after the self-address. The second is the number of decimals the instrument
    /// software prints m/z values with in scan filters (2 or 4 in the corpus); the first is 2 everywhere.
    pub display_words: [u32; 2],
    pub streams: StreamAddresses,
    pub own_address: u64,
    /// Bytes from `address` to the end of the structure.
    pub byte_len: u64,
}

impl RunHeader {
    /// Decimals of m/z values in scan filter text.
    pub fn filter_mass_decimals(&self) -> usize {
        match self.display_words[1] {
            d @ 1..=8 => d as usize,
            _ => 2,
        }
    }

    pub fn scan_count(&self) -> u64 {
        if self.last_scan >= self.first_scan && self.first_scan > 0 {
            u64::from(self.last_scan - self.first_scan) + 1
        } else {
            0
        }
    }
}

pub fn parse_run_header(c: &mut Cursor<'_>, version: u32) -> Result<RunHeader> {
    let address = c.offset();
    let mut h = RunHeader {
        address,
        ..Default::default()
    };
    h.sample_words = [c.u32()?, c.u32()?];
    h.first_scan = c.u32()?;
    h.last_scan = c.u32()?;
    h.instrument_log_count = c.u32()?;
    h.error_log_count = c.u32()?;
    let _ = c.u32()?;
    let index32 = c.u32()?;
    let data32 = c.u32()?;
    let ilog32 = c.u32()?;
    let elog32 = c.u32()?;
    let _ = c.u32()?;
    h.max_total_ion_current = c.f64()?;
    h.low_mz = c.f64()?;
    h.high_mz = c.f64()?;
    h.start_time_min = c.f64()?;
    h.end_time_min = c.f64()?;
    c.skip(56)?;
    h.sample_texts = [c.utf16_fixed(88)?, c.utf16_fixed(40)?, c.utf16_fixed(320)?];
    for _ in 0..6 {
        h.device_files.push(c.utf16_fixed(520)?);
    }
    h.run_values = [c.f64()?, c.f64()?];
    for _ in 0..7 {
        h.device_files.push(c.utf16_fixed(520)?);
    }
    let events32 = c.u32()?;
    let params32 = c.u32()?;
    h.scan_event_count = c.u32()?;
    h.scan_parameter_count = c.u32()?;
    h.segment_count = c.u32()?;
    c.skip(8)?;
    let own32 = c.u32()?;
    h.display_words = [c.u32()?, c.u32()?];
    if version >= 64 {
        h.streams.scan_index = c.u64()?;
        h.streams.scan_data = c.u64()?;
        h.streams.instrument_log = c.u64()?;
        h.streams.error_log = c.u64()?;
        let _ = c.u64()?;
        h.streams.scan_events = c.u64()?;
        h.streams.scan_parameters = c.u64()?;
        c.skip(8)?;
        h.own_address = c.u64()?;
        c.skip(96)?;
    } else {
        h.streams = StreamAddresses {
            scan_index: u64::from(index32),
            scan_data: u64::from(data32),
            instrument_log: u64::from(ilog32),
            error_log: u64::from(elog32),
            scan_events: u64::from(events32),
            scan_parameters: u64::from(params32),
        };
        h.own_address = u64::from(own32);
    }
    h.byte_len = c.offset() - address;
    Ok(h)
}

/// Instrument identification (follows the run header).
#[derive(Debug, Clone, Default)]
pub struct InstrumentId {
    pub id_words: [u32; 3],
    pub model: String,
    pub model_2: String,
    pub serial_number: String,
    pub software_version: String,
    /// Four trailing strings (observed a revision, axis labels `m/z` and `Relative Abundance`).
    pub tags: Vec<String>,
}

pub fn parse_instrument_id(c: &mut Cursor<'_>) -> Result<InstrumentId> {
    let id_words = [c.u32()?, c.u32()?, c.u32()?];
    let model = c.utf16_counted()?;
    let model_2 = c.utf16_counted()?;
    let serial_number = c.utf16_counted()?;
    let software_version = c.utf16_counted()?;
    let mut tags = Vec::new();
    for _ in 0..4 {
        tags.push(c.utf16_counted()?);
    }
    Ok(InstrumentId {
        id_words,
        model,
        model_2,
        serial_number,
        software_version,
        tags,
    })
}

/// One column of a self-describing record stream (instrument log, per-scan parameters).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenericField {
    /// Value type code (0 = section heading without a value; see [`GenericField::byte_len`]).
    pub kind: u32,
    /// Display precision for numbers, character capacity for text kinds 12 and 13.
    pub width: u32,
    pub label: String,
}

impl GenericField {
    /// Bytes this field occupies in each record.
    pub fn byte_len(&self) -> Option<usize> {
        Some(match self.kind {
            0 => 0,
            1..=5 => 1,
            6 | 7 => 2,
            8..=10 => 4,
            11 => 8,
            12 => self.width as usize,
            13 => self.width as usize * 2,
            _ => return None,
        })
    }
}

/// Column list of a record stream.
#[derive(Debug, Clone, Default)]
pub struct GenericHeader {
    pub address: u64,
    pub fields: Vec<GenericField>,
}

impl GenericHeader {
    /// Bytes per record, if every kind is known.
    pub fn record_len(&self) -> Option<usize> {
        self.fields.iter().map(GenericField::byte_len).sum()
    }

    /// Decode one record into (label, value) pairs; section headings are skipped.
    pub fn decode(&self, c: &mut Cursor<'_>) -> Result<Vec<(String, serde_json::Value)>> {
        use serde_json::Value;
        let mut out = Vec::with_capacity(self.fields.len());
        for f in &self.fields {
            let v = match f.kind {
                0 => continue,
                1 => Value::from(c.u8()? as i8),
                2..=4 => Value::from(c.u8()? != 0),
                5 => Value::from(c.u8()?),
                6 => Value::from(c.i16()?),
                7 => Value::from(c.u16()?),
                8 => Value::from(c.i32()?),
                9 => Value::from(c.u32()?),
                10 => json_f64(f64::from(c.f32()?)),
                11 => json_f64(c.f64()?),
                12 => {
                    let raw = c.take(f.width as usize)?;
                    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
                    Value::from(String::from_utf8_lossy(&raw[..end]).into_owned())
                }
                13 => Value::from(c.utf16_fixed(f.width as usize * 2)?),
                k => {
                    return Err(Error::corrupt_at(
                        FORMAT_ID,
                        c.offset(),
                        format!("unknown record value kind {k} for '{}'", f.label),
                    ));
                }
            };
            out.push((f.label.clone(), v));
        }
        Ok(out)
    }
}

fn json_f64(v: f64) -> serde_json::Value {
    serde_json::Number::from_f64(v).map_or(serde_json::Value::Null, serde_json::Value::Number)
}

/// Longest column list we accept (the largest seen in the corpus is about 200).
pub const MAX_GENERIC_FIELDS: u32 = 10_000;

pub fn parse_generic_header(c: &mut Cursor<'_>) -> Result<GenericHeader> {
    let address = c.offset();
    let n = c.u32()?;
    if n > MAX_GENERIC_FIELDS {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            address,
            format!("implausible record column count {n}"),
        ));
    }
    let mut fields = Vec::with_capacity(n as usize);
    for _ in 0..n {
        let at = c.offset();
        let kind = c.u32()?;
        let width = c.u32()?;
        let label = c.utf16_counted()?;
        let f = GenericField { kind, width, label };
        if f.byte_len().is_none() {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                at,
                format!("unknown record value kind {kind}"),
            ));
        }
        fields.push(f);
    }
    Ok(GenericHeader { address, fields })
}

/// One row of the scan index.
#[derive(Debug, Clone, Copy, Default)]
pub struct ScanIndexEntry {
    /// Offset of the scan's data packet relative to the scan-data stream.
    pub data_offset: u64,
    /// Zero-based position of the scan.
    pub scan_index: u32,
    pub event_number: u16,
    pub segment_number: u16,
    /// Next scan number (1-based) in the linked order.
    pub next_scan: u32,
    /// Observed 21 for profile MS1, 18/19 for centroid/profile MS2 in old files.
    pub kind_code: u32,
    pub data_size: u32,
    /// Retention time, minutes.
    pub rt_min: f64,
    pub total_ion_current: f64,
    pub base_peak_intensity: f64,
    pub base_peak_mz: f64,
    pub low_mz: f64,
    pub high_mz: f64,
}

/// Bytes per scan-index row by version.
pub fn scan_index_entry_len(version: u32) -> usize {
    match version {
        0..=63 => 72,
        64 => 80,
        _ => 88,
    }
}

pub fn parse_scan_index_entry(c: &mut Cursor<'_>, version: u32) -> Result<ScanIndexEntry> {
    let start = c.position();
    let off32 = c.u32()?;
    let mut e = ScanIndexEntry {
        scan_index: c.u32()?,
        event_number: c.u16()?,
        segment_number: c.u16()?,
        next_scan: c.u32()?,
        kind_code: c.u32()?,
        data_size: c.u32()?,
        rt_min: c.f64()?,
        total_ion_current: c.f64()?,
        base_peak_intensity: c.f64()?,
        base_peak_mz: c.f64()?,
        low_mz: c.f64()?,
        high_mz: c.f64()?,
        data_offset: u64::from(off32),
    };
    if version >= 64 {
        e.data_offset = c.u64()?;
    }
    c.seek_to(start + scan_index_entry_len(version))?;
    Ok(e)
}

/// Precursor selection and activation of one MS^n stage (32 bytes; 56 in version 66).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Reaction {
    pub precursor_mz: f64,
    /// Isolation window width, m/z (observed 1.0, 2.0; the full scan range in v66 MS1 events).
    pub isolation_width: f64,
    pub energy: f64,
    /// `reaction_words[0]` is the fragmentation code (1 = trap CID, 11 = beam-type/HCD in the corpus).
    pub reaction_words: [u32; 2],
    /// Three more doubles in version 66 (observed the scan range and 0.0, or 0.0, 0.0, 0.5).
    pub reaction_values: [f64; 3],
}

impl Reaction {
    /// Fragmentation method of this stage.
    pub fn activation(&self) -> crate::event::Activation {
        crate::event::Activation::from_reaction_code(self.reaction_words[0])
    }
}

/// Per-scan acquisition descriptor from the scan-event stream.
#[derive(Debug, Clone, Default)]
pub struct ScanEvent {
    /// Leading byte array (80/120/128/136 bytes by version); see [`crate::event`] accessors.
    pub preamble: Vec<u8>,
    pub reactions: Vec<Reaction>,
    /// Acquired m/z windows `[low, high]`: one for full scans, one per product for SRM.
    pub scan_ranges: Vec<[f64; 2]>,
    /// Frequency-to-m/z coefficients for Fourier-transform scans (count 0, 4, 5 or 7).
    pub coefficients: Vec<f64>,
    /// Counted list of 8-byte items (observed zero) after the coefficients.
    pub tail_items: Vec<u64>,
    /// One u32 (two in version 66, where the second is the UTF-16 unit count of `tail_text`).
    pub tail_words: Vec<u32>,
    /// Version 66: counted UTF-16 text closing the event (empty in most files; a short label
    /// such as `L1031` in Orbitrap Exploris 120 files; meaning unknown).
    pub tail_text: String,
}

/// Bytes of the leading byte array of scan events and templates by version.
pub fn preamble_len(version: u32) -> usize {
    match version {
        0..=56 => 41,
        57..=61 => 80,
        62 => 120,
        63..=65 => 128,
        _ => 136,
    }
}

impl ScanEvent {
    /// Overall scan range: low end of the first window to high end of the last.
    pub fn scan_range(&self) -> [f64; 2] {
        match (self.scan_ranges.first(), self.scan_ranges.last()) {
            (Some(a), Some(b)) => [a[0], b[1]],
            _ => [0.0, 0.0],
        }
    }
}

/// Most reactions / coefficients we accept per event (MS^10 would need 9).
const MAX_EVENT_ITEMS: u32 = 64;
/// Most m/z windows we accept per event (SRM transitions of one precursor).
const MAX_SCAN_RANGES: u32 = 4096;

fn count(c: &mut Cursor<'_>, what: &str) -> Result<u32> {
    counted(c, what, MAX_EVENT_ITEMS)
}

fn counted(c: &mut Cursor<'_>, what: &str, max: u32) -> Result<u32> {
    let at = c.offset();
    let n = c.u32()?;
    if n > max {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            at,
            format!("implausible {what} count {n} in a scan event"),
        ));
    }
    Ok(n)
}

fn reaction(c: &mut Cursor<'_>, version: u32) -> Result<Reaction> {
    let mut r = Reaction {
        precursor_mz: c.f64()?,
        isolation_width: c.f64()?,
        energy: c.f64()?,
        reaction_words: [c.u32()?, c.u32()?],
        reaction_values: [0.0; 3],
    };
    if version >= 66 {
        r.reaction_values = [c.f64()?, c.f64()?, c.f64()?];
    }
    Ok(r)
}

pub fn parse_scan_event(c: &mut Cursor<'_>, version: u32) -> Result<ScanEvent> {
    let preamble = c.take(preamble_len(version))?.to_vec();
    let mut e = ScanEvent {
        preamble,
        ..Default::default()
    };
    let n = count(c, "reaction")?;
    for _ in 0..n {
        e.reactions.push(reaction(c, version)?);
    }
    let nr = counted(c, "scan range", MAX_SCAN_RANGES)?;
    for _ in 0..nr {
        e.scan_ranges.push([c.f64()?, c.f64()?]);
    }
    let np = count(c, "coefficient")?;
    for _ in 0..np {
        e.coefficients.push(c.f64()?);
    }
    let nt = count(c, "trailing item")?;
    for _ in 0..nt {
        e.tail_items.push(c.u64()?);
    }
    e.tail_words.push(c.u32()?);
    if version >= 66 {
        // A counted UTF-16 text: zero units in most files, so it was first read as a constant word.
        let at = c.position();
        let units = c.u32()?;
        e.tail_words.push(units);
        c.seek_to(at)?;
        e.tail_text = c.utf16_counted()?;
    }
    Ok(e)
}

/// A scan-event template from the segment/event table (what the method asked for).
#[derive(Debug, Clone, Default)]
pub struct ScanTemplate {
    pub segment: u32,
    pub event: u32,
    pub preamble: Vec<u8>,
    /// The reaction and range counts of `scan_event`; before version 66 also its coefficient
    /// and tail-item counts and closing word.
    pub template_words: Vec<u32>,
    pub scan_range: [f64; 2],
    /// The template as a complete scan event of the file's version (empty in most files; with
    /// reactions, ranges and a compound name in TSQ Altis Plus and Exploris 120 files, with the
    /// scan range in ISQ and TSQ 9610 files). Runs that store no per-scan events take each
    /// scan's event from here.
    pub scan_event: Option<ScanEvent>,
}

/// The segment/event table between the error log and the per-scan parameter header.
#[derive(Debug, Clone, Default)]
pub struct MethodTable {
    pub address: u64,
    pub templates: Vec<ScanTemplate>,
}

impl MethodTable {
    /// The complete scan event of the template at (`segment`, `event`) (version 66).
    pub fn template_event(&self, segment: u16, event: u16) -> Option<&ScanEvent> {
        self.templates
            .iter()
            .find(|t| t.segment == u32::from(segment) && t.event == u32::from(event))
            .and_then(|t| t.scan_event.as_ref())
    }
}

const MAX_TEMPLATES: u32 = 10_000;

pub fn parse_method_table(c: &mut Cursor<'_>, version: u32) -> Result<MethodTable> {
    let address = c.offset();
    let nseg = c.u32()?;
    if nseg > MAX_TEMPLATES {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            address,
            format!("implausible segment count {nseg}"),
        ));
    }
    let mut templates = Vec::new();
    for s in 0..nseg {
        let at = c.offset();
        let nev = c.u32()?;
        if nev > MAX_TEMPLATES {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                at,
                format!("implausible event count {nev}"),
            ));
        }
        for ev in 0..nev {
            // A complete scan event (`docs/formats/thermo-raw.md` § Segment/event table).
            let e = parse_scan_event(c, version)?;
            // The counts the template was first read as: reactions and ranges, then (before
            // version 66) coefficients, tail items and the closing word.
            let mut template_words = vec![e.reactions.len() as u32, e.scan_ranges.len() as u32];
            if version < 66 {
                template_words.extend([
                    e.coefficients.len() as u32,
                    e.tail_items.len() as u32,
                    e.tail_words.first().copied().unwrap_or(0),
                ]);
            }
            templates.push(ScanTemplate {
                segment: s,
                event: ev,
                preamble: e.preamble.clone(),
                template_words,
                scan_range: e.scan_ranges.first().copied().unwrap_or([0.0, 0.0]),
                scan_event: Some(e),
            });
        }
    }
    Ok(MethodTable { address, templates })
}

/// An entry of the error log.
#[derive(Debug, Clone, Default)]
pub struct ErrorLogEntry {
    pub rt_min: f32,
    pub message: String,
}

pub fn parse_error_entry(c: &mut Cursor<'_>) -> Result<ErrorLogEntry> {
    Ok(ErrorLogEntry {
        rt_min: c.f32()?,
        message: c.utf16_counted()?,
    })
}

/// FILETIME (100 ns since 1601-01-01) to `YYYY-MM-DDTHH:MM:SS.mmm`.
pub fn filetime_iso(ft: u64) -> Option<String> {
    const EPOCH_DIFF_S: i64 = 11_644_473_600;
    if ft == 0 {
        return None;
    }
    let secs = i64::try_from(ft / 10_000_000).ok()? - EPOCH_DIFF_S;
    let ms = (ft % 10_000_000) / 10_000;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    Some(format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{ms:03}",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    ))
}

/// Seconds since 1970-01-01 of a FILETIME, ignoring its time zone.
pub fn filetime_unix_seconds(ft: u64) -> Option<i64> {
    const EPOCH_DIFF_S: i64 = 11_644_473_600;
    (ft != 0).then(|| {
        i64::try_from(ft / 10_000_000)
            .ok()
            .map(|s| s - EPOCH_DIFF_S)
    })?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filetime_known() {
        // 2009-05-07 22:43:00.078 is the first audit stamp of mtbls404-Blanc04.
        assert_eq!(
            filetime_iso(0x01C9_CF65_2C6D_58E0).as_deref(),
            Some("2009-05-07T22:43:00.078")
        );
        assert_eq!(filetime_iso(0), None);
    }

    /// A method table of one segment from the observed layouts: the ISQ file's version-64
    /// template (no reaction, one range 35-900) and a TSQ Altis Plus version-66 template (one
    /// reaction, one product window, a compound name).
    #[test]
    fn method_table_templates_are_complete_events() {
        fn le32(v: &mut Vec<u8>, x: u32) {
            v.extend_from_slice(&x.to_le_bytes());
        }
        fn le64f(v: &mut Vec<u8>, x: f64) {
            v.extend_from_slice(&x.to_le_bytes());
        }
        // version 64 (mtbls758-isq-growth-93)
        let mut b = Vec::new();
        le32(&mut b, 1); // segments
        le32(&mut b, 1); // events
        let mut pre = vec![0u8; preamble_len(64)];
        pre[4] = 1; // positive
        pre[6] = 1; // MS1
        b.extend_from_slice(&pre);
        le32(&mut b, 0); // reactions
        le32(&mut b, 1); // ranges
        le64f(&mut b, 35.0);
        le64f(&mut b, 900.0);
        le32(&mut b, 0); // coefficients
        le32(&mut b, 0); // tail items
        le32(&mut b, 0); // closing word
        le32(&mut b, 0xABCD); // what follows the table
        let mut c = Cursor::new(&b, 0);
        let t = parse_method_table(&mut c, 64).unwrap();
        assert_eq!(t.templates.len(), 1);
        assert_eq!(t.templates[0].template_words, [0, 1, 0, 0, 0]);
        assert_eq!(t.templates[0].scan_range, [35.0, 900.0]);
        assert_eq!(
            t.template_event(0, 0).map(ScanEvent::scan_range),
            Some([35.0, 900.0])
        );
        assert_eq!(c.u32().unwrap(), 0xABCD);

        // version 66 (mtbls6991-tsq-altis-plus-sar11-40, transition 76.0 > 29.967 `glycine`)
        let mut b = Vec::new();
        le32(&mut b, 1);
        le32(&mut b, 1);
        let mut pre = vec![0u8; preamble_len(66)];
        pre[6] = 2;
        pre[7] = 3; // SRM
        b.extend_from_slice(&pre);
        le32(&mut b, 1); // one reaction (56 bytes in v66)
        le64f(&mut b, 76.0);
        le64f(&mut b, 0.7);
        le64f(&mut b, 0.0);
        le32(&mut b, 12);
        le32(&mut b, 0);
        for _ in 0..3 {
            le64f(&mut b, 0.0);
        }
        le32(&mut b, 1);
        le64f(&mut b, 29.966);
        le64f(&mut b, 29.968);
        le32(&mut b, 0);
        le32(&mut b, 0);
        le32(&mut b, 0); // closing word
        let name: Vec<u16> = "glycine".encode_utf16().collect();
        le32(&mut b, name.len() as u32);
        for u in name {
            b.extend_from_slice(&u.to_le_bytes());
        }
        le32(&mut b, 0xABCD);
        let mut c = Cursor::new(&b, 0);
        let t = parse_method_table(&mut c, 66).unwrap();
        let e = t.template_event(0, 0).unwrap();
        assert_eq!(e.reactions[0].precursor_mz, 76.0);
        assert_eq!(e.scan_range(), [29.966, 29.968]);
        assert_eq!(e.tail_text, "glycine");
        assert_eq!(t.templates[0].template_words, [1, 1]);
        assert_eq!(c.u32().unwrap(), 0xABCD);
    }

    #[test]
    fn generic_field_sizes() {
        let f = |kind, width| GenericField {
            kind,
            width,
            label: String::new(),
        };
        assert_eq!(f(0, 0).byte_len(), Some(0));
        assert_eq!(f(11, 2).byte_len(), Some(8));
        assert_eq!(f(12, 80).byte_len(), Some(80));
        assert_eq!(f(13, 10).byte_len(), Some(20));
        assert_eq!(f(99, 0).byte_len(), None);
    }

    /// A v66 event with no reaction, one range, no coefficients, no tail items and the given
    /// closing text (empty in older files, `L1031` in the Exploris 120 file).
    fn v66_event(text: &str) -> Vec<u8> {
        let mut b = vec![0u8; preamble_len(66)];
        b.extend_from_slice(&0u32.to_le_bytes()); // reactions
        b.extend_from_slice(&1u32.to_le_bytes()); // ranges
        b.extend_from_slice(&50.0f64.to_le_bytes());
        b.extend_from_slice(&500.0f64.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes()); // coefficients
        b.extend_from_slice(&0u32.to_le_bytes()); // tail items
        b.extend_from_slice(&0u32.to_le_bytes()); // tail word
        let units: Vec<u16> = text.encode_utf16().collect();
        b.extend_from_slice(&(units.len() as u32).to_le_bytes());
        for u in units {
            b.extend_from_slice(&u.to_le_bytes());
        }
        b
    }

    #[test]
    fn v66_event_closing_text() {
        for text in ["", "L1031"] {
            let mut data = v66_event(text);
            data.extend(v66_event(""));
            let mut c = Cursor::new(&data, 0);
            let e = parse_scan_event(&mut c, 66).unwrap();
            assert_eq!(e.tail_text, text);
            assert_eq!(e.tail_words, vec![0, text.len() as u32]);
            // the next event starts right after the text
            let e2 = parse_scan_event(&mut c, 66).unwrap();
            assert_eq!(e2.scan_ranges.len(), 1);
            assert_eq!(c.remaining(), 0);
        }
    }

    #[test]
    fn truncated_event_is_an_error() {
        let data = vec![0u8; 100];
        let mut c = Cursor::new(&data, 0);
        assert!(parse_scan_event(&mut c, 66).is_err());
    }
}
