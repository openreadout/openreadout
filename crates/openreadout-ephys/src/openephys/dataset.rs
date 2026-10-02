//! `Dataset` for Open Ephys recordings (binary and legacy formats).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_core::bytes::{be_i16, read_block};
use openreadout_core::model::{
    CheckReport, ColumnInfo, FileInfo, Finding, LsEntry, SignalChannelInfo, Table, TableInfo,
    Trace, TraceInfo,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::source::{Fs, Input};
use openreadout_core::{Error, Plane, Result};
use serde_json::{Value, json};

use super::binary::{self, EventRows, OeRecording, OeTrace};
use super::legacy::{
    self, EVENT_RECORD_LEN, LEGACY_HEADER_LEN, LegacyChannel, LegacySpikes, LegacyTrace,
    RECORD_LEN, RECORD_SAMPLES,
};
use super::{OPEN_EPHYS_FORMAT_ID, OpenEphysReader};

/// Samples per channel decoded per `read_trace` call at most.
pub const MAX_OE_READ: u64 = 1 << 24;
/// Rows returned per `read_table` call at most.
pub const MAX_OE_TABLE_READ: u64 = 1 << 22;

/// The two layouts.
#[derive(Debug)]
enum Kind {
    Binary {
        recordings: Vec<OeRecording>,
        traces: Vec<OeTrace>,
        events: EventRows,
    },
    Legacy {
        nodes: Vec<String>,
        channels: Vec<LegacyChannel>,
        traces: Vec<LegacyTrace>,
        events: Vec<PathBuf>,
        spikes: Vec<LegacySpikes>,
    },
}

/// An opened Open Ephys recording directory.
#[derive(Debug)]
pub struct OpenEphysDataset {
    root: PathBuf,
    fs: Fs,
    kind: Kind,
    findings: Vec<Finding>,
}

/// The directory an input names: itself, or the directory of a `structure.oebin` /
/// `.continuous` file.
fn root_of(input: &Input) -> PathBuf {
    if input.is_dir() {
        input.path().to_path_buf()
    } else {
        input
            .path()
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
    }
}

impl OpenEphysDataset {
    /// Open a local path.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    /// Open an [`Input`] (a directory, a `structure.oebin`, or a `.continuous` file).
    pub fn open_input(input: &Input) -> Result<Self> {
        let fs = input.fs().clone();
        let root = root_of(input);
        let legacy_input = input
            .path()
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("continuous"));
        let mut oebins = Vec::new();
        if !legacy_input {
            oebins_below(&fs, &root, 0, &mut oebins);
        }
        if !oebins.is_empty() {
            let (recordings, mut findings) = binary::discover(&fs, &root)?;
            let traces = binary::layout(&recordings);
            let events = binary::events(&fs, &recordings)?;
            for u in &events.undecoded {
                findings.push(Finding::info(
                    "undecoded_events",
                    format!("binary event stream {u} is listed, not decoded"),
                ));
            }
            return Ok(OpenEphysDataset {
                root,
                fs,
                kind: Kind::Binary {
                    recordings,
                    traces,
                    events,
                },
                findings,
            });
        }
        let (cont, events, spikes_files) = legacy::discover(&fs, &root)?;
        if cont.is_empty() {
            return Err(Error::corrupt(
                OPEN_EPHYS_FORMAT_ID,
                format!(
                    "{} holds neither structure.oebin (binary format) nor .continuous files (Open Ephys format)",
                    root.display()
                ),
            ));
        }
        let mut findings = Vec::new();
        let mut nodes = Vec::new();
        let mut channels = Vec::new();
        for (node, p) in &cont {
            channels.push(legacy::channel(&fs, p, &mut findings)?);
            nodes.push(node.clone());
        }
        let traces = legacy::layout(&nodes, &channels);
        let mut spikes = Vec::new();
        for p in &spikes_files {
            spikes.push(legacy::spikes(&fs, p, &mut findings)?);
        }
        Ok(OpenEphysDataset {
            root,
            fs,
            kind: Kind::Legacy {
                nodes,
                channels,
                traces,
                events,
                spikes,
            },
            findings,
        })
    }
}

fn oebins_below(fs: &Fs, dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if !out.is_empty() {
        return;
    }
    if fs.is_file(&dir.join("structure.oebin")) {
        out.push(dir.join("structure.oebin"));
        return;
    }
    if depth >= binary::MAX_OEBIN_DEPTH {
        return;
    }
    if let Ok(rd) = fs.read_dir(dir) {
        let mut subs: Vec<PathBuf> = rd
            .filter_map(std::result::Result::ok)
            .filter(|e| e.metadata().is_ok_and(|m| m.is_dir()))
            .map(|e| e.path())
            .collect();
        subs.sort();
        for s in subs {
            oebins_below(fs, &s, depth + 1, out);
        }
    }
}

/// Unit of a channel `structure.oebin` gives none for: µV for headstage channels (`CH`, `AP`,
/// `LFP`), V for ADC and analog-input channels (Open Ephys docs); unknown otherwise.
fn default_unit(name: &str) -> Option<&'static str> {
    if name.starts_with("CH") || name.starts_with("AP") || name.starts_with("LFP") {
        Some("uV")
    } else if name.contains("ADC") || name.starts_with("AI") {
        Some("V")
    } else {
        None
    }
}

fn column(i: usize, name: &str, dtype: &str, unit: Option<&str>) -> ColumnInfo {
    ColumnInfo {
        index: i as u32,
        name: name.into(),
        label: None,
        dtype: dtype.into(),
        unit: unit.map(str::to_string),
        range: None,
        extra: BTreeMap::new(),
    }
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root).unwrap_or(p).display().to_string()
}

impl OpenEphysDataset {
    fn binary_trace_info(&self, recs: &[OeRecording], i: usize, t: &OeTrace) -> TraceInfo {
        let r0 = &recs[t.recordings[0]];
        let s0 = &r0.streams[t.stream[0]];
        let channels = s0
            .channels
            .iter()
            .enumerate()
            .map(|(k, c)| SignalChannelInfo {
                index: k as u32,
                name: c.name.clone(),
                unit: if c.units.is_empty() {
                    default_unit(&c.name).map(str::to_string)
                } else {
                    Some(c.units.clone())
                },
                dtype: "int16".into(),
                scale: c.bit_volts,
                offset: 0.0,
                extra: if c.units.is_empty() {
                    BTreeMap::from([("unit_assumed".to_string(), json!(true))])
                } else {
                    BTreeMap::new()
                },
            })
            .collect();
        let mut extra = BTreeMap::new();
        if !r0.node.is_empty() {
            extra.insert("record_node".into(), json!(r0.node));
        }
        extra.insert("experiment".into(), json!(r0.experiment));
        extra.insert(
            "recordings".into(),
            json!(
                t.recordings
                    .iter()
                    .map(|&k| recs[k].recording)
                    .collect::<Vec<_>>()
            ),
        );
        extra.insert("stream_folder".into(), json!(s0.folder));
        if !s0.stream_name.is_empty() {
            extra.insert("stream_name".into(), json!(s0.stream_name));
        }
        if !s0.processor.is_empty() {
            extra.insert("source_processor".into(), json!(s0.processor));
        }
        let rate = s0.sample_rate;
        let streams: Vec<&binary::OeStream> = t
            .recordings
            .iter()
            .zip(&t.stream)
            .map(|(&r, &s)| &recs[r].streams[s])
            .collect();
        let counts: Vec<u64> = streams.iter().map(|s| s.samples).collect();
        let starts: Vec<Value> = streams
            .iter()
            .map(|s| {
                s.first_sample
                    .map_or(Value::Null, |v| json!(v as f64 / rate))
            })
            .collect();
        if counts.windows(2).any(|w| w[0] != w[1]) {
            extra.insert("sweep_sample_counts".into(), json!(counts));
        }
        extra.insert("sweep_starts_s".into(), json!(starts));
        extra.insert(
            "first_sample_numbers".into(),
            json!(streams.iter().map(|s| s.first_sample).collect::<Vec<_>>()),
        );
        if streams.iter().any(|s| s.first_timestamp_s.is_some()) {
            extra.insert(
                "first_timestamps_s".into(),
                json!(
                    streams
                        .iter()
                        .map(|s| s.first_timestamp_s)
                        .collect::<Vec<_>>()
                ),
            );
        }
        if i == 0 {
            extra.insert("application".into(), json!("Open Ephys GUI"));
            if !r0.gui_version.is_empty() {
                extra.insert("application_version".into(), json!(r0.gui_version));
            }
        }
        let name = if r0.node.is_empty() {
            s0.folder.clone()
        } else {
            format!("{}/{}", r0.node, s0.folder)
        };
        TraceInfo {
            index: i as u32,
            name: Some(name),
            sample_rate_hz: rate,
            sample_count: counts.iter().copied().max().unwrap_or(0),
            sweep_count: t.recordings.len() as u32,
            channels,
            start_s: starts.first().and_then(Value::as_f64),
            extra,
        }
    }

    fn legacy_trace_info(
        &self,
        channels: &[LegacyChannel],
        i: usize,
        t: &LegacyTrace,
    ) -> TraceInfo {
        let c0 = &channels[t.channels[0]];
        let rate = c0.sample_rate;
        let chans = t
            .channels
            .iter()
            .enumerate()
            .map(|(k, &ci)| {
                let c = &channels[ci];
                SignalChannelInfo {
                    index: k as u32,
                    name: c.name.clone(),
                    unit: Some(default_unit_legacy(&c.name).to_string()),
                    dtype: "int16".into(),
                    scale: c.bit_volts,
                    offset: 0.0,
                    extra: BTreeMap::from([("file".to_string(), json!(rel(&self.root, &c.path)))]),
                }
            })
            .collect();
        let counts: Vec<u64> = c0.runs.iter().map(|r| r.records * RECORD_SAMPLES).collect();
        let starts: Vec<f64> = c0.runs.iter().map(|r| r.timestamp as f64 / rate).collect();
        let mut extra = BTreeMap::new();
        if !t.node.is_empty() {
            extra.insert("record_node".into(), json!(t.node));
        }
        extra.insert("source".into(), json!(c0.source));
        extra.insert("segment".into(), json!(c0.segment));
        extra.insert("sweep_starts_s".into(), json!(starts));
        extra.insert(
            "sweep_recordings".into(),
            json!(c0.runs.iter().map(|r| r.recording).collect::<Vec<_>>()),
        );
        if counts.windows(2).any(|w| w[0] != w[1]) {
            extra.insert("sweep_sample_counts".into(), json!(counts));
        }
        if i == 0 {
            extra.insert("application".into(), json!("Open Ephys GUI"));
            extra.insert("format_version".into(), json!(c0.version));
            if !c0.date_created.is_empty() {
                extra.insert("date_created".into(), json!(c0.date_created));
            }
        }
        TraceInfo {
            index: i as u32,
            name: Some(if t.node.is_empty() {
                format!("{} segment {}", c0.source, c0.segment)
            } else {
                format!("{}/{} segment {}", t.node, c0.source, c0.segment)
            }),
            sample_rate_hz: rate,
            sample_count: counts.iter().copied().max().unwrap_or(0),
            sweep_count: c0.runs.len() as u32,
            channels: chans,
            start_s: starts.first().copied(),
            extra,
        }
    }

    fn tables(&self) -> Vec<TableInfo> {
        let mut out = Vec::new();
        match &self.kind {
            Kind::Binary {
                recordings, events, ..
            } => {
                if events.stream_list.is_empty() {
                    return out;
                }
                let streams: Vec<Value> = events
                    .stream_list
                    .iter()
                    .map(|&(r, e)| {
                        let rec = &recordings[r];
                        let ev = &rec.events[e];
                        json!({"record_node": rec.node, "experiment": rec.experiment,
                               "recording": rec.recording, "folder": ev.folder,
                               "channel_name": ev.channel_name, "type": ev.kind,
                               "sample_rate_hz": ev.sample_rate})
                    })
                    .collect();
                let mut extra = BTreeMap::new();
                extra.insert("streams".into(), json!(streams));
                if !events.texts.is_empty() {
                    extra.insert("texts".into(), json!(events.texts));
                }
                out.push(TableInfo {
                    index: 0,
                    name: Some("events".into()),
                    row_count: events.sample_number.len() as u64,
                    columns: vec![
                        column(0, "stream", "uint32", None),
                        column(1, "sample_number", "int64", None),
                        column(2, "time_s", "float64", Some("s")),
                        column(3, "timestamp_s", "float64", Some("s")),
                        column(4, "state", "int16", None),
                        column(5, "line", "int16", None),
                        column(6, "full_word", "uint64", None),
                        column(7, "text", "int32", None),
                    ],
                    extra,
                });
            }
            Kind::Legacy { events, spikes, .. } => {
                let rows: u64 = events
                    .iter()
                    .map(|p| {
                        self.fs.metadata(p).map_or(0, |m| {
                            m.len().saturating_sub(LEGACY_HEADER_LEN) / EVENT_RECORD_LEN
                        })
                    })
                    .sum();
                if !events.is_empty() {
                    let mut extra = BTreeMap::new();
                    extra.insert(
                        "files".into(),
                        json!(
                            events
                                .iter()
                                .map(|p| rel(&self.root, p))
                                .collect::<Vec<_>>()
                        ),
                    );
                    out.push(TableInfo {
                        index: 0,
                        name: Some("events".into()),
                        row_count: rows,
                        columns: vec![
                            column(0, "file", "uint32", None),
                            column(1, "timestamp", "int64", None),
                            column(2, "time_s", "float64", Some("s")),
                            column(3, "sample_position", "int16", None),
                            column(4, "event_type", "uint8", None),
                            column(5, "processor_id", "uint8", None),
                            column(6, "event_id", "uint8", None),
                            column(7, "channel", "uint8", None),
                            column(8, "recording", "uint16", None),
                        ],
                        extra,
                    });
                }
                for s in spikes {
                    let mut cols = vec![
                        column(0, "timestamp", "int64", None),
                        column(1, "time_s", "float64", Some("s")),
                        column(2, "sorted_id", "uint16", None),
                        column(3, "electrode_id", "uint16", None),
                        column(4, "channel", "uint16", None),
                        column(5, "recording", "uint16", None),
                    ];
                    for c in 0..s.channels {
                        for k in 0..s.samples {
                            cols.push(column(
                                cols.len(),
                                &format!("w{c}_{k}"),
                                "float64",
                                Some("uV"),
                            ));
                        }
                    }
                    let mut extra = BTreeMap::new();
                    extra.insert("file".into(), json!(rel(&self.root, &s.path)));
                    extra.insert("electrode".into(), json!(s.electrode));
                    extra.insert("sample_rate_hz".into(), json!(s.sample_rate));
                    extra.insert("channels_per_spike".into(), json!(s.channels));
                    extra.insert("samples_per_channel".into(), json!(s.samples));
                    out.push(TableInfo {
                        index: out.len() as u32,
                        name: Some(format!("spikes {}", s.electrode)),
                        row_count: s.records,
                        columns: cols,
                        extra,
                    });
                }
            }
        }
        out
    }

    fn read_legacy_events(
        &self,
        events: &[PathBuf],
        first: u64,
        max_rows: u64,
    ) -> Result<Vec<Vec<f64>>> {
        let mut cols = vec![Vec::new(); 9];
        let mut skip = first;
        let mut left = max_rows;
        for (fi, path) in events.iter().enumerate() {
            if left == 0 {
                break;
            }
            let mut file = self.fs.open(path).map_err(|e| Error::io(path, e))?;
            let len = file.size().map_err(|e| Error::io(path, e))?;
            let head = read_block(&mut file, path, 0, LEGACY_HEADER_LEN, len)?;
            let rate = legacy::parse_header(&head.bytes)
                .get("sampleRate")
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(f64::NAN);
            let count = len.saturating_sub(LEGACY_HEADER_LEN) / EVENT_RECORD_LEN;
            if skip >= count {
                skip -= count;
                continue;
            }
            let take = (count - skip).min(left);
            let at = LEGACY_HEADER_LEN + skip * EVENT_RECORD_LEN;
            let records = read_block(&mut file, path, at, take * EVENT_RECORD_LEN, len)?;
            for k in 0..take {
                let pos = at + k * EVENT_RECORD_LEN;
                let ts = records.i64_at(pos).unwrap_or(0);
                cols[0].push(fi as f64);
                cols[1].push(ts as f64);
                cols[2].push(ts as f64 / rate);
                cols[3].push(f64::from(records.i16_at(pos + 8).unwrap_or(0)));
                for (j, field) in [10u64, 11, 12, 13].iter().enumerate() {
                    cols[4 + j].push(f64::from(records.u8_at(pos + field).unwrap_or(0)));
                }
                cols[8].push(f64::from(records.u16_at(pos + 14).unwrap_or(0)));
            }
            left -= take;
            skip = 0;
        }
        Ok(cols)
    }

    fn read_legacy_spikes(
        &self,
        s: &LegacySpikes,
        first: u64,
        max_rows: u64,
    ) -> Result<Vec<Vec<f64>>> {
        let nch = u64::from(s.channels);
        let ns = u64::from(s.samples);
        let width = 6 + (nch * ns) as usize;
        let mut cols = vec![Vec::new(); width];
        if first >= s.records {
            return Ok(cols);
        }
        let take = max_rows.min(s.records - first);
        let path = &s.path;
        let mut file = self.fs.open(path).map_err(|e| Error::io(path, e))?;
        let len = file.size().map_err(|e| Error::io(path, e))?;
        let at = LEGACY_HEADER_LEN + first * s.record_len;
        let records = read_block(&mut file, path, at, take * s.record_len, len)?;
        for k in 0..take {
            let pos = at + k * s.record_len;
            let ts = records.i64_at(pos + 1).unwrap_or(0);
            cols[0].push(ts as f64);
            cols[1].push(ts as f64 / s.sample_rate);
            cols[2].push(f64::from(records.u16_at(pos + 23).unwrap_or(0)));
            cols[3].push(f64::from(records.u16_at(pos + 25).unwrap_or(0)));
            cols[4].push(f64::from(records.u16_at(pos + 27).unwrap_or(0)));
            let wave_at = pos + 42;
            let gains_at = wave_at + 2 * nch * ns;
            let rec_at = gains_at + 6 * nch;
            cols[5].push(f64::from(records.u16_at(rec_at).unwrap_or(0)));
            for ch in 0..nch {
                let gain = f64::from(records.f32_at(gains_at + 4 * ch).unwrap_or(f32::NAN));
                for j in 0..ns {
                    let raw = f64::from(records.u16_at(wave_at + 2 * (ch * ns + j)).unwrap_or(0));
                    cols[6 + (ch * ns + j) as usize].push((raw - 32768.0) / gain * 1000.0);
                }
            }
        }
        Ok(cols)
    }
}

/// Legacy channel units: µV for `CH` channels, V otherwise (as Neo and the Open Ephys docs).
fn default_unit_legacy(name: &str) -> &'static str {
    if name.starts_with("CH") { "uV" } else { "V" }
}

impl Dataset for OpenEphysDataset {
    fn info(&self) -> Result<FileInfo> {
        let mut notes = Vec::new();
        let (traces, version): (Vec<TraceInfo>, String) = match &self.kind {
            Kind::Binary {
                recordings,
                traces,
                events,
            } => {
                notes.push(format!(
                    "Open Ephys binary format: {} recording(s); every continuous stream is a trace with one sweep per recording of its experiment; values are raw × bit_volts",
                    recordings.len()
                ));
                if !events.undecoded.is_empty() {
                    notes.push(format!(
                        "{} binary event stream(s) are listed, not decoded",
                        events.undecoded.len()
                    ));
                }
                let spikes: usize = recordings.iter().map(|r| r.spike_groups).sum();
                if spikes > 0 {
                    notes.push(format!(
                        "{spikes} spike group(s) in the recordings are not decoded"
                    ));
                }
                let v = recordings
                    .first()
                    .map(|r| format!("binary (GUI {})", r.gui_version))
                    .unwrap_or_default();
                (
                    traces
                        .iter()
                        .enumerate()
                        .map(|(i, t)| self.binary_trace_info(recordings, i, t))
                        .collect(),
                    v,
                )
            }
            Kind::Legacy {
                channels, traces, ..
            } => {
                notes.push(format!(
                    "Open Ephys format (one file per channel): {} continuous file(s); channels on the same sample grid form a trace, gaps and recording numbers split sweeps; values are raw × bitVolts",
                    channels.len()
                ));
                let refused: Vec<String> = channels
                    .iter()
                    .filter(|c| c.refused.is_some())
                    .map(|c| rel(&self.root, &c.path))
                    .collect();
                if !refused.is_empty() {
                    notes.push(format!(
                        "samples of {} are not read: records that are not a gap-free series (run `check`)",
                        refused.join(", ")
                    ));
                }
                let v = channels
                    .first()
                    .map(|c| format!("Open Ephys format {}", c.version))
                    .unwrap_or_default();
                (
                    traces
                        .iter()
                        .enumerate()
                        .map(|(i, t)| self.legacy_trace_info(channels, i, t))
                        .collect(),
                    v,
                )
            }
        };
        if self
            .findings
            .iter()
            .any(|f| f.code == "missing_stream" || f.code == "bad_stream")
        {
            notes.push(
                "structural problems found (streams missing or unusable); run `check`".into(),
            );
        }
        let mut tables = self.tables();
        if traces.is_empty()
            && let Some(t) = tables.first_mut()
        {
            t.extra
                .insert("application".into(), json!("Open Ephys GUI"));
        }
        let size = match &self.kind {
            Kind::Binary { recordings, .. } => recordings
                .iter()
                .flat_map(|r| &r.streams)
                .map(|s| s.samples * 2 * s.channels.len() as u64)
                .sum(),
            Kind::Legacy { channels, .. } => channels
                .iter()
                .map(|c| self.fs.metadata(&c.path).map_or(0, |m| m.len()))
                .sum(),
        };
        Ok(FileInfo {
            path: self.root.display().to_string(),
            size_bytes: size,
            format: OpenEphysReader.descriptor(),
            format_version: Some(version),
            images: Vec::new(),
            tables,
            spectra: Vec::new(),
            traces,
            plane_count: 0,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        Ok(match &self.kind {
            Kind::Binary { recordings, .. } => json!({
                "layout": "binary",
                "recordings": recordings.iter().map(|r| json!({
                    "record_node": r.node, "experiment": r.experiment, "recording": r.recording,
                    "dir": rel(&self.root, &r.dir), "gui_version": r.gui_version,
                    "continuous": r.streams.iter().map(|s| json!({
                        "folder": s.folder, "sample_rate": s.sample_rate, "channels": s.channels.len(),
                        "samples": s.samples, "first_sample": s.first_sample,
                        "first_timestamp_s": s.first_timestamp_s, "processor": s.processor,
                        "stream_name": s.stream_name,
                    })).collect::<Vec<_>>(),
                    "events": r.events.iter().map(|e| json!({
                        "folder": e.folder, "channel_name": e.channel_name, "type": e.kind,
                        "sample_rate": e.sample_rate,
                    })).collect::<Vec<_>>(),
                    "spike_groups": r.spike_groups,
                })).collect::<Vec<_>>(),
            }),
            Kind::Legacy {
                channels, nodes, ..
            } => json!({
                "layout": "legacy",
                "channels": channels.iter().zip(nodes).map(|(c, n)| json!({
                    "file": rel(&self.root, &c.path), "record_node": n, "name": c.name,
                    "source": c.source, "segment": c.segment, "sample_rate": c.sample_rate,
                    "bit_volts": c.bit_volts, "version": c.version, "date_created": c.date_created,
                    "runs": c.runs.iter().map(|r| json!({"first_record": r.first_record,
                        "records": r.records, "timestamp": r.timestamp, "recording": r.recording}))
                        .collect::<Vec<_>>(),
                    "refused": c.refused,
                })).collect::<Vec<_>>(),
            }),
        })
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for (k, s) in [
            ("format_version", Source::Spec),
            ("traces[].sample_rate_hz", Source::Spec),
            ("traces[].sample_count", Source::Spec),
            ("traces[].sweep_count", Source::Inferred),
            ("traces[].channels[].name", Source::Spec),
            ("traces[].channels[].unit", Source::Spec),
            ("traces[].channels[].scale", Source::Spec),
            ("traces[].start_s", Source::PriorArt),
            ("traces[].extra.sweep_starts_s", Source::PriorArt),
            ("tables[].columns", Source::Spec),
        ] {
            p.insert(k.into(), s);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let mut out = Vec::new();
        match &self.kind {
            Kind::Binary { recordings, .. } => {
                for r in recordings {
                    for s in &r.streams {
                        out.push(LsEntry {
                            kind: "continuous".into(),
                            name: rel(&self.root, &s.data_path),
                            offset: Some(0),
                            size: Some(s.samples * 2 * s.channels.len() as u64),
                            image: None,
                            details: json!({"channels": s.channels.len(), "samples": s.samples}),
                        });
                    }
                    for e in &r.events {
                        out.push(LsEntry {
                            kind: "events".into(),
                            name: rel(&self.root, &e.dir),
                            offset: None,
                            size: None,
                            image: None,
                            details: json!({"type": e.kind}),
                        });
                    }
                }
            }
            Kind::Legacy {
                channels,
                events,
                spikes,
                ..
            } => {
                for c in channels {
                    out.push(LsEntry {
                        kind: "continuous".into(),
                        name: rel(&self.root, &c.path),
                        offset: Some(LEGACY_HEADER_LEN),
                        size: Some(c.runs.iter().map(|r| r.records).sum::<u64>() * RECORD_LEN),
                        image: None,
                        details: json!({"channel": c.name, "runs": c.runs.len(), "refused": c.refused}),
                    });
                }
                for p in events {
                    out.push(LsEntry {
                        kind: "events".into(),
                        name: rel(&self.root, p),
                        offset: Some(LEGACY_HEADER_LEN),
                        size: None,
                        image: None,
                        details: Value::Null,
                    });
                }
                for s in spikes {
                    out.push(LsEntry {
                        kind: "spikes".into(),
                        name: rel(&self.root, &s.path),
                        offset: Some(LEGACY_HEADER_LEN),
                        size: Some(s.records * s.record_len),
                        image: None,
                        details: json!({"records": s.records}),
                    });
                }
            }
        }
        Ok(out)
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::unsupported(
            OPEN_EPHYS_FORMAT_ID,
            "image planes",
            "Open Ephys recordings hold sampled signals and events: use `openreadout trace` or `openreadout table`.",
        ))
    }

    fn read_trace(
        &mut self,
        index: u32,
        sweep: u32,
        first_sample: u64,
        max_samples: u64,
    ) -> Result<Trace> {
        let info = self.info()?;
        let listed = info.traces.get(index as usize).ok_or_else(|| {
            Error::Usage(format!(
                "trace index {index} out of range (the recording has {} traces)",
                info.traces.len()
            ))
        })?;
        if sweep >= listed.sweep_count {
            return Err(Error::Usage(format!(
                "sweep {sweep} out of range (trace {index} has {} sweeps)",
                listed.sweep_count
            )));
        }
        let mut out = Trace {
            trace: index,
            sweep,
            first_sample,
            channels: Vec::new(),
        };
        match &self.kind {
            Kind::Binary {
                recordings, traces, ..
            } => {
                let tr = &traces[index as usize];
                let stream =
                    &recordings[tr.recordings[sweep as usize]].streams[tr.stream[sweep as usize]];
                if first_sample > stream.samples {
                    return Err(Error::Usage(format!(
                        "first sample {first_sample} is past the end of sweep {sweep} ({} samples)",
                        stream.samples
                    )));
                }
                let count = max_samples
                    .min(stream.samples - first_sample)
                    .min(MAX_OE_READ);
                let nch = stream.channels.len() as u64;
                let mut file = self
                    .fs
                    .open(&stream.data_path)
                    .map_err(|e| Error::io(&stream.data_path, e))?;
                let len = file.size().map_err(|e| Error::io(&stream.data_path, e))?;
                let at = first_sample * nch * 2;
                let data = read_block(&mut file, &stream.data_path, at, count * nch * 2, len)?;
                if data.len() as u64 != count * nch * 2 {
                    return Err(Error::corrupt(
                        OPEN_EPHYS_FORMAT_ID,
                        format!("{} is shorter than its samples", stream.data_path.display()),
                    ));
                }
                out.channels = stream
                    .channels
                    .iter()
                    .enumerate()
                    .map(|(c, ch)| {
                        (0..count)
                            .map(|k| {
                                let pos = ((k * nch + c as u64) * 2) as usize;
                                f64::from(i16::from_le_bytes([
                                    data.bytes[pos],
                                    data.bytes[pos + 1],
                                ])) * ch.bit_volts
                            })
                            .collect()
                    })
                    .collect();
            }
            Kind::Legacy {
                channels, traces, ..
            } => {
                let tr = &traces[index as usize];
                let c0 = &channels[tr.channels[0]];
                let run = c0.runs[sweep as usize];
                let records = run.records * RECORD_SAMPLES;
                if first_sample > records {
                    return Err(Error::Usage(format!(
                        "first sample {first_sample} is past the end of sweep {sweep} ({records} samples)"
                    )));
                }
                let count = max_samples.min(records - first_sample).min(MAX_OE_READ);
                for &ci in &tr.channels {
                    let chan = &channels[ci];
                    let mut col = Vec::with_capacity(count as usize);
                    if count > 0 {
                        let r0 = run.first_record + first_sample / RECORD_SAMPLES;
                        let r1 = run.first_record + (first_sample + count - 1) / RECORD_SAMPLES;
                        let mut file = self
                            .fs
                            .open(&chan.path)
                            .map_err(|e| Error::io(&chan.path, e))?;
                        let len = file.size().map_err(|e| Error::io(&chan.path, e))?;
                        let at = LEGACY_HEADER_LEN + r0 * RECORD_LEN;
                        let data =
                            read_block(&mut file, &chan.path, at, (r1 - r0 + 1) * RECORD_LEN, len)?;
                        let mut i = first_sample;
                        while i < first_sample + count {
                            let rec = run.first_record + i / RECORD_SAMPLES;
                            let pos = LEGACY_HEADER_LEN
                                + rec * RECORD_LEN
                                + 12
                                + 2 * (i % RECORD_SAMPLES);
                            let value = data
                                .slice(pos, 2)
                                .and_then(|two| be_i16(two, 0))
                                .ok_or_else(|| {
                                    Error::corrupt(OPEN_EPHYS_FORMAT_ID, "record cut off")
                                })?;
                            col.push(f64::from(value) * chan.bit_volts);
                            i += 1;
                        }
                    }
                    out.channels.push(col);
                }
            }
        }
        Ok(out)
    }

    fn read_table(&mut self, index: u32, first_row: u64, max_rows: u64) -> Result<Table> {
        let n = max_rows.min(MAX_OE_TABLE_READ);
        let columns = match &self.kind {
            Kind::Binary { events, .. } => {
                if index != 0 || events.stream_list.is_empty() {
                    return Err(Error::Usage(format!("table {index} out of range")));
                }
                let total = events.sample_number.len() as u64;
                let a = first_row.min(total) as usize;
                let b = first_row.saturating_add(n).min(total) as usize;
                [
                    &events.stream,
                    &events.sample_number,
                    &events.time_s,
                    &events.timestamp_s,
                    &events.state,
                    &events.line,
                    &events.full_word,
                    &events.text,
                ]
                .iter()
                .map(|c| c[a..b].to_vec())
                .collect()
            }
            Kind::Legacy { events, spikes, .. } => {
                let has_events = !events.is_empty();
                if index == 0 && has_events {
                    self.read_legacy_events(events, first_row, n)?
                } else {
                    let k = index as usize - usize::from(has_events);
                    let s = spikes
                        .get(k)
                        .ok_or_else(|| Error::Usage(format!("table {index} out of range")))?;
                    self.read_legacy_spikes(s, first_row, n)?
                }
            }
        };
        Ok(Table {
            table: index,
            first_row,
            columns,
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.root.display().to_string(), OPEN_EPHYS_FORMAT_ID);
        r.performed("binary: every structure.oebin parses; every listed stream folder exists; continuous.dat holds whole samples; .npy headers and lengths");
        r.performed("legacy: every .continuous header; every record's marker, sample count and timestamp (gap-free runs)");
        for f in &self.findings {
            r.push(f.clone());
        }
        Ok(r)
    }
}
