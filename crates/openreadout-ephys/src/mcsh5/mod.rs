//! Multi Channel Systems HDF5 files (`.h5`, MCS-HDF5 protocol type `RawData`), as Multi Channel
//! DataManager writes them when it converts MC_Rack, Multi Channel Experimenter and Multi Channel
//! Analyzer recordings: `/Data/Recording_x/` groups with analog streams (a channel × sample matrix
//! with one information record per channel and a table of segments), event streams and time-stamp
//! streams. Laid out in Multi Channel Systems' published format definition. Notes and vocabulary:
//! `docs/formats/mcs-hdf5.md`; provenance: `docs/provenance/mcs-hdf5.md`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use hdf5_pure::Group;
use openreadout_core::model::{
    CheckReport, ColumnInfo, DetectConfidence, FileInfo, Finding, FormatDescriptor, LsEntry,
    SignalChannelInfo, Table, TableInfo, Trace, TraceInfo,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, Detection, FormatReader, PlaneIndex, has_extension};
use openreadout_core::source::{Fs, Input};
use openreadout_core::{Error, Plane, Result};
use openreadout_hdf5::h5util::{attr_text, looks_like_hdf5, open_h5_in};
use serde_json::{Value, json};

use crate::h5rec::{self, Field, Heap, Matrix, Record, field};

/// Format id of MCS HDF5 files.
pub const MCS_HDF5_FORMAT_ID: &str = "mcs-hdf5";
/// Samples per channel decoded per `read_trace` call at most.
pub const MAX_MCS_HDF5_READ: u64 = 1 << 22;
/// Bytes of samples decoded per `read_trace` call at most.
pub const MAX_MCS_HDF5_READ_BYTES: u64 = 64 << 20;
/// Events and time stamps kept at most.
pub const MAX_MCS_HDF5_EVENTS: usize = 4_000_000;

/// One channel of an analog stream (one information record).
#[derive(Debug, Clone, PartialEq)]
pub struct McsChannel {
    /// Channel id the recording software gave.
    pub id: i64,
    /// Row of the sample matrix that holds the channel.
    pub row: u64,
    /// Channel group id.
    pub group: i64,
    /// Channel label (electrode name).
    pub label: String,
    /// Stored sample type as the record names it (`Int`, …).
    pub raw_data_type: String,
    /// Physical unit (`V`; `NoUnit` for digital channels).
    pub unit: String,
    /// Power of ten of the values.
    pub exponent: i32,
    /// Stored number of the zero point.
    pub ad_zero: i64,
    /// Microseconds between samples.
    pub tick_us: i64,
    /// Stored step to value factor.
    pub conversion_factor: i64,
    /// Bits of the AD converter (when the record has them).
    pub adc_bits: Option<i64>,
    /// High-pass filter: type, corner frequency text and order.
    pub high_pass: (String, String, i64),
    /// Low-pass filter: type, corner frequency text and order.
    pub low_pass: (String, String, i64),
}

impl McsChannel {
    /// Value of one stored step: conversion factor × 10^exponent.
    pub fn step(&self) -> f64 {
        // 10^exponent parsed from its decimal form: correctly rounded
        let p: f64 = format!("1e{}", self.exponent).parse().unwrap_or(1.0);
        self.conversion_factor as f64 * p
    }
}

/// One segment (a sweep) of an analog stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McsSegment {
    /// Time of the first sample, µs.
    pub timestamp_us: i64,
    /// First column of the sample matrix.
    pub first: u64,
    /// Last column (inclusive).
    pub last: u64,
}

/// An analog stream: a sample matrix and its channels.
#[derive(Debug, Clone)]
pub struct McsAnalogStream {
    /// Recording number.
    pub recording: u32,
    /// Stream number.
    pub stream: u32,
    /// Stream label.
    pub label: String,
    /// Stream sub-type (`Electrode`, `Auxiliary`, `Digital`, …).
    pub data_subtype: String,
    /// Channels in record order.
    pub channels: Vec<McsChannel>,
    /// Segments in table order.
    pub segments: Vec<McsSegment>,
    /// The sample matrix.
    pub matrix: Matrix,
}

impl McsAnalogStream {
    /// Digital streams hold bit patterns: their stored numbers are the values.
    pub fn digital(&self) -> bool {
        self.data_subtype.eq_ignore_ascii_case("Digital")
    }
}

/// One event or time stamp (an event entity row).
#[derive(Debug, Clone, PartialEq)]
pub struct McsEvent {
    /// Seconds (from µs).
    pub time_s: f64,
    /// Seconds (from µs; 0 for time stamps).
    pub duration_s: f64,
    /// Index into the table's entity names.
    pub entity: usize,
}

/// A table built from the event streams or the time-stamp streams.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct McsEventTable {
    /// `events` or `timestamps`.
    pub name: String,
    /// `stream label / entity label` per entity.
    pub entities: Vec<String>,
    /// Rows sorted by time.
    pub rows: Vec<McsEvent>,
}

/// The parsed file.
#[derive(Debug, Clone)]
pub struct McsFile {
    /// File length.
    pub file_len: u64,
    /// Protocol type (`RawData`).
    pub protocol_type: String,
    /// Protocol version.
    pub protocol_version: Option<i64>,
    /// Root attributes and `/Data` attributes as text.
    pub attributes: BTreeMap<String, String>,
    /// Analog streams of every recording.
    pub analog: Vec<McsAnalogStream>,
    /// Event and time-stamp tables.
    pub tables: Vec<McsEventTable>,
    /// Streams found and not read (frame and segment streams).
    pub not_read: Vec<String>,
    /// Problems found while parsing.
    pub findings: Vec<Finding>,
}

fn group_attrs(g: &Group) -> BTreeMap<String, String> {
    g.attrs()
        .map(|a| {
            a.iter()
                .filter_map(|(k, v)| attr_text(v).map(|t| (k.clone(), t)))
                .collect()
        })
        .unwrap_or_default()
}

fn h5err(path: &Path, e: impl std::fmt::Display) -> Error {
    Error::corrupt(
        MCS_HDF5_FORMAT_ID,
        format!(
            "{}: the MCS HDF5 structure cannot be read: {e}",
            path.display()
        ),
    )
}

/// `Stream_3` → 3.
fn numbered(name: &str, prefix: &str) -> Option<u32> {
    name.strip_prefix(prefix)?.parse().ok()
}

fn sorted_numbered(names: Vec<String>, prefix: &str) -> Vec<(u32, String)> {
    let mut v: Vec<(u32, String)> = names
        .into_iter()
        .filter_map(|n| numbered(&n, prefix).map(|k| (k, n)))
        .collect();
    v.sort();
    v
}

fn channel(r: &Record) -> McsChannel {
    let int = |k: &str| field(r, k).and_then(Field::int).unwrap_or(0);
    let text = |k: &str| field(r, k).map(Field::text).unwrap_or_default();
    McsChannel {
        id: int("ChannelID"),
        row: u64::try_from(int("RowIndex")).unwrap_or(u64::MAX),
        group: int("GroupID"),
        label: text("Label"),
        raw_data_type: text("RawDataType"),
        unit: text("Unit"),
        exponent: i32::try_from(int("Exponent")).unwrap_or(0),
        ad_zero: int("ADZero"),
        tick_us: int("Tick"),
        conversion_factor: int("ConversionFactor"),
        adc_bits: field(r, "ADCBits").and_then(Field::int),
        high_pass: (
            text("HighPassFilterType"),
            text("HighPassFilterCutOffFrequency"),
            int("HighPassFilterOrder"),
        ),
        low_pass: (
            text("LowPassFilterType"),
            text("LowPassFilterCutOffFrequency"),
            int("LowPassFilterOrder"),
        ),
    }
}

/// The MCS-HDF5 protocol type of an opened HDF5 file (root attribute).
pub fn protocol_type(h5: &hdf5_pure::File) -> Option<String> {
    group_attrs(&h5.root()).remove("McsHdf5ProtocolType")
}

/// Parse an MCS HDF5 file.
#[allow(clippy::too_many_lines)]
pub fn parse_mcs_hdf5(
    fs: &Fs,
    path: &Path,
    h5: &hdf5_pure::File,
    file_len: u64,
) -> Result<McsFile> {
    let root = h5.root();
    let mut attributes = group_attrs(&root);
    let protocol_type = attributes
        .get("McsHdf5ProtocolType")
        .cloned()
        .unwrap_or_default();
    if protocol_type != "RawData" {
        return Err(Error::unsupported(
            MCS_HDF5_FORMAT_ID,
            format!("MCS HDF5 files of protocol type {protocol_type:?}"),
            "Only the RawData protocol (Multi Channel DataManager conversions) is read; CMOS-MEA files are not.",
        ));
    }
    let protocol_version = attributes
        .get("McsHdf5ProtocolVersion")
        .and_then(|v| v.parse().ok());
    let data = h5.group("/Data").map_err(|e| h5err(path, e))?;
    for (k, v) in group_attrs(&data) {
        attributes.insert(format!("Data.{k}"), v);
    }
    let mut heap = Heap::open(fs, path, h5, MCS_HDF5_FORMAT_ID)?;
    let mut analog = Vec::new();
    let mut not_read = Vec::new();
    let mut findings = Vec::new();
    let mut events = McsEventTable {
        name: "events".into(),
        ..Default::default()
    };
    let mut stamps = McsEventTable {
        name: "timestamps".into(),
        ..Default::default()
    };
    let recordings = sorted_numbered(data.groups().map_err(|e| h5err(path, e))?, "Recording_");
    for (rec_no, rec_name) in &recordings {
        let rec = data.group(rec_name).map_err(|e| h5err(path, e))?;
        let kinds = rec.groups().map_err(|e| h5err(path, e))?;
        for kind in &kinds {
            let kg = rec.group(kind).map_err(|e| h5err(path, e))?;
            let streams = sorted_numbered(kg.groups().map_err(|e| h5err(path, e))?, "Stream_");
            for (s_no, s_name) in &streams {
                let sg = kg.group(s_name).map_err(|e| h5err(path, e))?;
                let sa = group_attrs(&sg);
                let label = sa.get("Label").cloned().unwrap_or_default();
                let where_ = format!("/Data/{rec_name}/{kind}/{s_name}");
                match kind.as_str() {
                    "AnalogStream" => {
                        let info = sg.dataset("InfoChannel").map_err(|e| h5err(path, e))?;
                        let channels: Vec<McsChannel> = h5rec::records(&info, &mut heap, path)?
                            .iter()
                            .map(channel)
                            .collect();
                        let matrix = Matrix::open(
                            h5,
                            &format!("{where_}/ChannelData"),
                            MCS_HDF5_FORMAT_ID,
                            file_len,
                        )?;
                        let tsd = sg
                            .dataset("ChannelDataTimeStamps")
                            .map_err(|e| h5err(path, e))?;
                        h5rec::check_backed(&tsd, file_len, MCS_HDF5_FORMAT_ID, &where_)?;
                        let ts = tsd.read_i64().map_err(|e| h5err(path, e))?;
                        let mut segments = Vec::new();
                        for t in ts.as_chunks::<3>().0 {
                            let (Ok(first), Ok(last)) = (u64::try_from(t[1]), u64::try_from(t[2]))
                            else {
                                findings.push(Finding::error(
                                    "bad_segment",
                                    format!("{where_}: a segment with negative indices {t:?}"),
                                ));
                                continue;
                            };
                            if last < first || last >= matrix.cols {
                                findings.push(Finding::error(
                                    "bad_segment",
                                    format!(
                                        "{where_}: segment columns {first}..={last} outside the {} stored samples",
                                        matrix.cols
                                    ),
                                ));
                                continue;
                            }
                            segments.push(McsSegment {
                                timestamp_us: t[0],
                                first,
                                last,
                            });
                        }
                        if let Some(c) = channels.iter().find(|c| c.row >= matrix.rows) {
                            return Err(Error::corrupt(
                                MCS_HDF5_FORMAT_ID,
                                format!(
                                    "{where_}: channel {:?} is in row {} of a {}-row sample matrix",
                                    c.label, c.row, matrix.rows
                                ),
                            ));
                        }
                        let ticks: std::collections::BTreeSet<i64> =
                            channels.iter().map(|c| c.tick_us).collect();
                        if ticks.len() > 1 || ticks.iter().any(|&t| t <= 0) {
                            return Err(Error::unsupported(
                                MCS_HDF5_FORMAT_ID,
                                format!("{where_}: channels with sample ticks {ticks:?} µs"),
                                "Every channel of an analog stream must share one positive sample tick.",
                            ));
                        }
                        analog.push(McsAnalogStream {
                            recording: *rec_no,
                            stream: *s_no,
                            label,
                            data_subtype: sa.get("DataSubType").cloned().unwrap_or_default(),
                            channels,
                            segments,
                            matrix,
                        });
                    }
                    "EventStream" | "TimeStampStream" => {
                        let (info_name, prefix, table) = if kind == "EventStream" {
                            ("InfoEvent", "EventEntity_", &mut events)
                        } else {
                            ("InfoTimeStamp", "TimeStampEntity_", &mut stamps)
                        };
                        let infos = match sg.dataset(info_name) {
                            Ok(d) => h5rec::records(&d, &mut heap, path)?,
                            Err(_) => Vec::new(),
                        };
                        let id_key = if kind == "EventStream" {
                            "EventID"
                        } else {
                            "TimeStampEntityID"
                        };
                        for (e_no, e_name) in
                            sorted_numbered(sg.datasets().map_err(|e| h5err(path, e))?, prefix)
                        {
                            let d = sg.dataset(&e_name).map_err(|e| h5err(path, e))?;
                            let shape = d.shape().map_err(|e| h5err(path, e))?;
                            h5rec::check_backed(&d, file_len, MCS_HDF5_FORMAT_ID, &e_name)?;
                            let v = d.read_i64().map_err(|e| h5err(path, e))?;
                            let entity_label = infos
                                .iter()
                                .find(|r| {
                                    field(r, id_key).and_then(Field::int) == Some(i64::from(e_no))
                                })
                                .and_then(|r| field(r, "Label"))
                                .map(Field::text)
                                .filter(|t| !t.is_empty())
                                .unwrap_or_else(|| e_name.clone());
                            let ent = table.entities.len();
                            table
                                .entities
                                .push(format!("{} / {entity_label}", label.trim()));
                            let n = shape.last().copied().unwrap_or(0) as usize;
                            let rows = shape.first().copied().unwrap_or(1);
                            for i in 0..n.min(v.len()) {
                                if table.rows.len() >= MAX_MCS_HDF5_EVENTS {
                                    findings.push(Finding::warning(
                                        "events_capped",
                                        format!("more than {MAX_MCS_HDF5_EVENTS} events; the rest are left out"),
                                    ));
                                    break;
                                }
                                let dur = if rows >= 2 && shape.len() == 2 {
                                    v.get(n + i).copied().unwrap_or(0)
                                } else {
                                    0
                                };
                                table.rows.push(McsEvent {
                                    time_s: v[i] as f64 / 1e6,
                                    duration_s: dur as f64 / 1e6,
                                    entity: ent,
                                });
                            }
                        }
                    }
                    other => not_read.push(format!("{where_} ({other}, {label})")),
                }
            }
        }
    }
    let mut tables = Vec::new();
    for mut t in [events, stamps] {
        if !t.rows.is_empty() {
            t.rows
                .sort_by(|a, b| a.time_s.total_cmp(&b.time_s).then(a.entity.cmp(&b.entity)));
            tables.push(t);
        }
    }
    Ok(McsFile {
        file_len,
        protocol_type,
        protocol_version,
        attributes,
        analog,
        tables,
        not_read,
        findings,
    })
}

impl McsFile {
    /// `RawData <version>`.
    pub fn version_text(&self) -> String {
        match self.protocol_version {
            Some(v) => format!("{} {v}", self.protocol_type),
            None => self.protocol_type.clone(),
        }
    }

    /// The writing application and its version (protocol version 2 on), else the recording
    /// program.
    pub fn writer(&self) -> (Option<String>, Option<String>) {
        let a = &self.attributes;
        match a.get("GeneratingApplicationName") {
            Some(n) => (
                Some(n.clone()),
                a.get("GeneratingApplicationVersion").cloned(),
            ),
            None => (
                a.get("Data.ProgramName").cloned(),
                a.get("Data.ProgramVersion").cloned(),
            ),
        }
    }

    /// The recording date (`/Data` `DateInTicks`, .NET ticks of 100 ns since year 1) as ISO 8601
    /// without a zone: the recording computer's local time.
    pub fn recorded_at(&self) -> Option<String> {
        let ticks: i64 = self.attributes.get("Data.DateInTicks")?.parse().ok()?;
        if ticks <= 0 {
            return None;
        }
        let secs = ticks / 10_000_000 - 62_135_596_800;
        let micros = (ticks % 10_000_000) / 10;
        let days = secs.div_euclid(86_400);
        let sod = secs.rem_euclid(86_400);
        let (y, m, d) = openreadout_core::time::civil_from_days(days);
        Some(format!(
            "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{micros:06}",
            sod / 3600,
            (sod % 3600) / 60,
            sod % 60
        ))
    }
}

/// The MCS HDF5 reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct McsHdf5Reader;

fn det(confidence: DetectConfidence, note: &str) -> Detection {
    Detection {
        format_id: MCS_HDF5_FORMAT_ID,
        confidence,
        note: Some(note.into()),
    }
}

impl FormatReader for McsHdf5Reader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&crate::assurance::MCS_HDF5)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: MCS_HDF5_FORMAT_ID.into(),
            name: "Multi Channel Systems HDF5 (Multi Channel DataManager)".into(),
            vendor: "Multi Channel Systems MCS GmbH".into(),
            extensions: vec!["h5".into()],
            family: "electrophysiology".into(),
            can_read: true,
            can_write: false,
            confidence: crate::assurance::MCS_HDF5.confidence,
            known_gaps: vec![
                "Segment streams (spike cutouts and averages) and frame streams (CMOS sensor frames) are listed, not read".into(),
                "CMOS-MEA protocol files are refused".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], _path: &Path) -> Option<Detection> {
        if !looks_like_hdf5(head) {
            return None;
        }
        memfind(head, b"McsHdf5ProtocolType").then(|| {
            det(
                DetectConfidence::Definite,
                "HDF5 file with an MCS-HDF5 protocol attribute",
            )
        })
    }

    fn sniff_input(&self, head: &[u8], input: &Input) -> Option<Detection> {
        if let Some(d) = self.sniff(head, input.path()) {
            return Some(d);
        }
        if !looks_like_hdf5(head) || !has_extension(input.path(), &["h5", "hdf5"]) {
            return None;
        }
        let h5 = open_h5_in(input.fs(), input.path(), MCS_HDF5_FORMAT_ID).ok()?;
        (protocol_type(&h5).as_deref() == Some("RawData")).then(|| {
            det(
                DetectConfidence::Definite,
                "HDF5 file of the MCS-HDF5 RawData protocol",
            )
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(McsHdf5Dataset::open(path)?))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(McsHdf5Dataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// `needle` occurs in `hay`.
pub fn memfind(hay: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && hay.windows(needle.len()).any(|w| w == needle)
}

/// An opened MCS HDF5 file.
#[derive(Debug)]
pub struct McsHdf5Dataset {
    path: PathBuf,
    fs: Fs,
    h5: hdf5_pure::File,
    file: McsFile,
}

impl McsHdf5Dataset {
    /// Open a local file.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    /// Open an [`Input`].
    pub fn open_input(input: &Input) -> Result<Self> {
        let path = input.path().to_path_buf();
        let fs = input.fs().clone();
        let file_len = fs.metadata(&path).map_err(|e| Error::io(&path, e))?.len();
        let h5 = open_h5_in(&fs, &path, MCS_HDF5_FORMAT_ID)?;
        let file = parse_mcs_hdf5(&fs, &path, &h5, file_len)?;
        Ok(McsHdf5Dataset { path, fs, h5, file })
    }

    /// The parsed file.
    pub fn mcs(&self) -> &McsFile {
        &self.file
    }

    fn trace_info(&self, i: usize) -> TraceInfo {
        let s = &self.file.analog[i];
        let tick = s.channels.first().map_or(1, |c| c.tick_us);
        let rate = 1e6 / tick as f64;
        let dtype = h5rec::dtype_of(s.matrix.pixel_type());
        let channels = s
            .channels
            .iter()
            .enumerate()
            .map(|(k, c)| {
                let mut e = BTreeMap::new();
                e.insert("channel_id".into(), json!(c.id));
                e.insert("row".into(), json!(c.row));
                e.insert("group_id".into(), json!(c.group));
                e.insert("ad_zero".into(), json!(c.ad_zero));
                e.insert("conversion_factor".into(), json!(c.conversion_factor));
                e.insert("exponent".into(), json!(c.exponent));
                if let Some(b) = c.adc_bits {
                    e.insert("adc_bits".into(), json!(b));
                }
                for (name, (ty, f, order)) in
                    [("high_pass", &c.high_pass), ("low_pass", &c.low_pass)]
                {
                    if !ty.is_empty() {
                        e.insert(
                            format!("{name}_filter"),
                            json!({"type": ty, "cutoff_hz": f.parse::<f64>().ok(), "order": order}),
                        );
                    }
                }
                let (scale, offset) = if s.digital() {
                    (1.0, 0.0)
                } else {
                    (c.step(), -(c.ad_zero as f64) * c.step())
                };
                SignalChannelInfo {
                    index: k as u32,
                    name: if c.label.is_empty() {
                        format!("ch{k}")
                    } else {
                        c.label.clone()
                    },
                    unit: Some(if c.unit == "NoUnit" {
                        String::new()
                    } else {
                        c.unit.clone()
                    }),
                    dtype: dtype.into(),
                    scale,
                    offset,
                    extra: e,
                }
            })
            .collect();
        let counts: Vec<u64> = s.segments.iter().map(|g| g.last - g.first + 1).collect();
        let mut extra = BTreeMap::new();
        extra.insert("recording".into(), json!(s.recording));
        extra.insert("stream".into(), json!(s.stream));
        extra.insert("data_subtype".into(), json!(s.data_subtype));
        if i == 0 {
            if let Some(t) = self.file.recorded_at() {
                extra.insert("recorded_at".into(), json!(t));
            }
            let (w, v) = self.file.writer();
            if let Some(w) = w {
                extra.insert("writer".into(), json!(w));
            }
            if let Some(v) = v {
                extra.insert("writer_version".into(), json!(v));
            }
            for (k, ours) in [
                ("Data.ProgramName", "recording_program"),
                ("Data.ProgramVersion", "recording_program_version"),
                ("Data.MeaName", "mea_name"),
                ("Data.MeaLayout", "mea_layout"),
            ] {
                if let Some(v) = self.file.attributes.get(k) {
                    extra.insert(ours.into(), json!(v));
                }
            }
        }
        let t0 = s.segments.first().map_or(0, |g| g.timestamp_us);
        if s.segments.len() > 1 {
            extra.insert(
                "sweep_starts_s".into(),
                json!(
                    s.segments
                        .iter()
                        .map(|g| (g.timestamp_us - t0) as f64 / 1e6)
                        .collect::<Vec<_>>()
                ),
            );
            extra.insert("sweep_sample_counts".into(), json!(counts));
        }
        TraceInfo {
            index: i as u32,
            name: Some(if s.label.trim().is_empty() {
                format!("Stream_{}", s.stream)
            } else {
                s.label.trim().to_string()
            }),
            sample_rate_hz: rate,
            sample_count: counts.iter().copied().max().unwrap_or(0),
            sweep_count: u32::try_from(s.segments.len()).unwrap_or(u32::MAX),
            channels,
            start_s: Some(t0 as f64 / 1e6),
            extra,
        }
    }

    fn table_info(&self, i: usize) -> TableInfo {
        let t = &self.file.tables[i];
        let col = |k: u32, name: &str, dtype: &str, unit: Option<&str>| ColumnInfo {
            index: k,
            name: name.into(),
            label: None,
            dtype: dtype.into(),
            unit: unit.map(str::to_string),
            range: None,
            extra: BTreeMap::new(),
        };
        let mut ent = col(2, "entity", "int32", None);
        ent.extra.insert("categories".into(), json!(t.entities));
        TableInfo {
            index: i as u32,
            name: Some(t.name.clone()),
            row_count: t.rows.len() as u64,
            columns: vec![
                col(0, "time_s", "float64", Some("s")),
                col(1, "duration_s", "float64", Some("s")),
                ent,
            ],
            extra: BTreeMap::new(),
        }
    }
}

impl Dataset for McsHdf5Dataset {
    fn info(&self) -> Result<FileInfo> {
        let f = &self.file;
        let (w, v) = f.writer();
        let mut notes = vec![format!(
            "MCS HDF5 ({}) written by {} {}: {} analog stream(s) as traces",
            f.version_text(),
            w.as_deref().unwrap_or("an unnamed program"),
            v.as_deref().unwrap_or(""),
            f.analog.len()
        )];
        notes.push(
            "values are (stored − ADZero) × ConversionFactor × 10^Exponent in each channel's unit; digital streams keep their stored numbers"
                .into(),
        );
        if f.analog.iter().any(|s| s.segments.len() > 1) {
            notes.push(
                "streams with several segments: each segment is a sweep (`extra.sweep_starts_s`)"
                    .into(),
            );
        }
        if !f.not_read.is_empty() {
            notes.push(format!("streams not read: {}", f.not_read.join("; ")));
        }
        if f.findings
            .iter()
            .any(|x| x.severity == openreadout_core::model::Severity::Error)
        {
            notes.push("structural problems found; run `check`".into());
        }
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: f.file_len,
            format: McsHdf5Reader.descriptor(),
            format_version: Some(f.version_text()),
            images: Vec::new(),
            tables: (0..f.tables.len()).map(|i| self.table_info(i)).collect(),
            spectra: Vec::new(),
            traces: (0..f.analog.len()).map(|i| self.trace_info(i)).collect(),
            plane_count: 0,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let f = &self.file;
        Ok(json!({
            "attributes": f.attributes,
            "analog_streams": f.analog.iter().map(|s| json!({
                "recording": s.recording, "stream": s.stream, "label": s.label,
                "data_subtype": s.data_subtype, "matrix": s.matrix.path,
                "rows": s.matrix.rows, "columns": s.matrix.cols,
                "filters": s.matrix.filters(),
                "segments": s.segments.iter().map(|g| json!([g.timestamp_us, g.first, g.last])).collect::<Vec<_>>(),
                "channels": s.channels.iter().map(|c| json!({
                    "id": c.id, "row": c.row, "group": c.group, "label": c.label,
                    "raw_data_type": c.raw_data_type, "unit": c.unit, "exponent": c.exponent,
                    "ad_zero": c.ad_zero, "tick_us": c.tick_us, "conversion_factor": c.conversion_factor,
                })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "not_read": f.not_read,
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
            "traces[].extra.sweep_starts_s",
            "tables[].row_count",
        ] {
            p.insert(k.into(), Source::Spec);
        }
        p.insert("traces[].extra.recorded_at".into(), Source::Inferred);
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let f = &self.file;
        let mut out = Vec::new();
        for s in &f.analog {
            out.push(LsEntry {
                kind: "analog-stream".into(),
                name: s.matrix.path.clone(),
                offset: None,
                size: None,
                image: None,
                details: json!({
                    "label": s.label, "channels": s.channels.len(), "rows": s.matrix.rows,
                    "columns": s.matrix.cols, "segments": s.segments.len(),
                    "sample_type": h5rec::dtype_of(s.matrix.pixel_type()), "filters": s.matrix.filters(),
                }),
            });
        }
        for t in &f.tables {
            out.push(LsEntry {
                kind: format!("{}-table", t.name),
                name: t.name.clone(),
                offset: None,
                size: None,
                image: None,
                details: json!({"entities": t.entities.len(), "rows": t.rows.len()}),
            });
        }
        for n in &f.not_read {
            out.push(LsEntry {
                kind: "not-read".into(),
                name: n.clone(),
                offset: None,
                size: None,
                image: None,
                details: json!({}),
            });
        }
        Ok(out)
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::unsupported(
            MCS_HDF5_FORMAT_ID,
            "image planes",
            "MCS HDF5 recordings hold sampled signals: use `openreadout trace`.",
        ))
    }

    fn read_trace(&mut self, index: u32, sweep: u32, first: u64, max: u64) -> Result<Trace> {
        let Some(s) = self.file.analog.get(index as usize) else {
            return Err(Error::Usage(format!(
                "trace index {index} out of range ({} traces)",
                self.file.analog.len()
            )));
        };
        let Some(g) = s.segments.get(sweep as usize).copied() else {
            return Err(Error::Usage(format!(
                "sweep {sweep} out of range ({} sweeps)",
                s.segments.len()
            )));
        };
        let n = g.last - g.first + 1;
        if first > n {
            return Err(Error::Usage(format!(
                "first sample {first} is past the end ({n} samples)"
            )));
        }
        let bps = s.matrix.pixel_type().bytes_per_sample() as u64;
        let per_sample = (bps * s.matrix.rows).max(1);
        let count = max
            .min(n - first)
            .min(MAX_MCS_HDF5_READ)
            .min((MAX_MCS_HDF5_READ_BYTES / per_sample).max(1))
            .min((h5rec::MAX_H5_EXPANSION.saturating_mul(self.file.file_len) / per_sample).max(1));
        let rows = s.matrix.read(
            &self.h5,
            &self.fs,
            &self.path,
            MCS_HDF5_FORMAT_ID,
            g.first + first,
            count,
        )?;
        let digital = s.digital();
        let channels = s
            .channels
            .iter()
            .map(|c| {
                let r = &rows[usize::try_from(c.row).unwrap_or(0)];
                if digital {
                    r.clone()
                } else {
                    let step = c.step();
                    let zero = c.ad_zero as f64;
                    r.iter().map(|&v| (v - zero) * step).collect()
                }
            })
            .collect();
        Ok(Trace {
            trace: index,
            sweep,
            first_sample: first,
            channels,
        })
    }

    fn read_table(&mut self, index: u32, first_row: u64, max_rows: u64) -> Result<Table> {
        let Some(t) = self.file.tables.get(index as usize) else {
            return Err(Error::Usage(format!("table index {index} out of range")));
        };
        let a = usize::try_from(first_row)
            .unwrap_or(usize::MAX)
            .min(t.rows.len());
        let b = a
            .saturating_add(usize::try_from(max_rows).unwrap_or(usize::MAX))
            .min(t.rows.len());
        let rows = &t.rows[a..b];
        Ok(Table {
            table: index,
            first_row: a as u64,
            columns: vec![
                rows.iter().map(|r| r.time_s).collect(),
                rows.iter().map(|r| r.duration_s).collect(),
                rows.iter().map(|r| r.entity as f64).collect(),
            ],
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let f = &self.file;
        let mut r = CheckReport::new(self.path.display().to_string(), MCS_HDF5_FORMAT_ID);
        r.performed("protocol type and version read; every recording's streams listed");
        r.performed("analog streams: channel records against the sample matrix rows, segments against its columns");
        for x in &f.findings {
            r.push(x.clone());
        }
        for s in &f.analog {
            let mut rows = std::collections::BTreeSet::new();
            for c in &s.channels {
                if !rows.insert(c.row) {
                    r.push(Finding::warning(
                        "shared_row",
                        format!("{}: two channel records name row {}", s.matrix.path, c.row),
                    ));
                }
            }
            if !s.digital() && s.channels.iter().any(|c| c.conversion_factor == 0) {
                r.push(Finding::warning(
                    "zero_conversion",
                    format!(
                        "{}: a channel has conversion factor 0: its values read as 0",
                        s.matrix.path
                    ),
                ));
            }
            for w in s.segments.windows(2) {
                if w[1].first <= w[0].last {
                    r.push(Finding::warning(
                        "overlapping_segments",
                        format!(
                            "{}: segments overlap at column {}",
                            s.matrix.path, w[1].first
                        ),
                    ));
                }
            }
        }
        Ok(r)
    }
}

#[cfg(test)]
mod tests;
