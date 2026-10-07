//! `Dataset`: continuous files as one trace (segments as sweeps), event and spike files as tables.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_core::bytes::read_block;
use openreadout_core::model::{
    CheckReport, ColumnInfo, FileInfo, Finding, LsEntry, SignalChannelInfo, Table, TableInfo,
    Trace, TraceInfo,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::source::{Fs, Input, SourceFile};
use openreadout_core::{Error, Plane, Result};
use serde_json::{Value, json};

use crate::header::{HEADER_LEN, TextHeader, parse_header};
use crate::records::{
    ContinuousIndex, FileKind, NCS_RECORD_LEN, NCS_SAMPLES, NVT_RECORD_START, SPIKE_FEATURES,
    SPIKE_SAMPLES, event_record, index_continuous, spike_record, video_record,
};
use crate::{FORMAT_ID, NeuralynxReader};

/// Most event labels summarized in `info`.
pub const MAX_LABELS: usize = 10_000;
/// Rows decoded per `read_table` call at most.
pub const MAX_TABLE_READ: u64 = 1 << 20;

/// An opened Neuralynx file.
#[derive(Debug)]
pub struct NeuralynxDataset {
    path: PathBuf,
    file_len: u64,
    kind: FileKind,
    header: TextHeader,
    /// Continuous files only.
    index: Option<ContinuousIndex>,
    handle: Option<SourceFile>,
    /// Where the file is read from.
    fs: Fs,
}

fn kind_of(path: &Path, header: &TextHeader) -> Option<FileKind> {
    let by_size = header
        .number("RecordSize")
        .and_then(|n| FileKind::from_record_len(n as u64));
    let by_ext = path
        .extension()
        .and_then(|e| e.to_str())
        .and_then(FileKind::from_extension);
    let by_type = match header
        .get("FileType")
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("csc" | "ncs") => Some(FileKind::Continuous),
        Some("event") => Some(FileKind::Events),
        Some("video") => Some(FileKind::Video),
        _ => None,
    };
    by_size.or(by_ext).or(by_type)
}

impl NeuralynxDataset {
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    /// Open an [`Input`] (a local path, a buffer, a host source).
    pub(crate) fn open_input(input: &Input) -> Result<Self> {
        let (path, fs) = (input.path(), input.fs());
        let mut f = fs.open(path).map_err(|e| Error::io(path, e))?;
        let file_len = f.metadata().map_err(|e| Error::io(path, e))?.len();
        if file_len < HEADER_LEN {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                file_len,
                format!("file is {file_len} bytes, shorter than the {HEADER_LEN}-byte text header"),
            ));
        }
        let hb = read_block(&mut f, path, 0, HEADER_LEN, file_len)?;
        let header = parse_header(&hb.bytes);
        let kind = kind_of(path, &header).ok_or_else(|| {
            Error::unsupported(
                FORMAT_ID,
                "unknown Neuralynx record type",
                "Only .ncs, .nev, .nse, .nst and .ntt files (record sizes 1044, 184, 112, 176, 304) are read; video (.nvt) and raw (.nrd) files are not.",
            )
        })?;
        let index = if kind == FileKind::Continuous {
            Some(index_continuous(
                &mut f,
                path,
                file_len,
                header.number("SamplingFrequency"),
                false,
            )?)
        } else {
            None
        };
        Ok(NeuralynxDataset {
            path: path.to_path_buf(),
            file_len,
            kind,
            header,
            index,
            handle: Some(f),
            fs: fs.clone(),
        })
    }

    /// What the file holds.
    pub fn kind(&self) -> FileKind {
        self.kind
    }

    /// The parsed text header.
    pub fn text_header(&self) -> &TextHeader {
        &self.header
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

    fn record_count(&self) -> u64 {
        self.file_len.saturating_sub(HEADER_LEN) / self.kind.record_len()
    }

    /// µV per count for sub-channel `i` (ADBitVolts × 10⁶).
    fn uv_per_count(&self, i: usize) -> Option<f64> {
        let v = self.header.numbers("ADBitVolts");
        v.get(i).or(v.first()).map(|x| x * 1e6)
    }

    fn channel_extra(&self) -> BTreeMap<String, Value> {
        let h = &self.header;
        let mut e = BTreeMap::new();
        let ad = h.numbers("ADChannel");
        if !ad.is_empty() {
            e.insert(
                "ad_channel".into(),
                json!(ad.iter().map(|x| *x as i64).collect::<Vec<_>>()),
            );
        }
        let range = h.numbers("InputRange");
        if !range.is_empty() {
            e.insert("input_range_uv".into(), json!(range));
        }
        if let Some(v) = h.flag("InputInverted") {
            e.insert("input_inverted".into(), json!(v));
        }
        if let Some(v) = h.number("ADMaxValue") {
            e.insert("ad_max_value".into(), json!(v));
        }
        let mut dsp = serde_json::Map::new();
        for (k, name) in [
            ("DSPLowCutFilterEnabled", "low_cut_enabled"),
            ("DspLowCutFrequency", "low_cut_hz"),
            ("DspLowCutNumTaps", "low_cut_taps"),
            ("DspLowCutFilterType", "low_cut_type"),
            ("DSPHighCutFilterEnabled", "high_cut_enabled"),
            ("DspHighCutFrequency", "high_cut_hz"),
            ("DspHighCutNumTaps", "high_cut_taps"),
            ("DspHighCutFilterType", "high_cut_type"),
            ("DspDelayCompensation", "delay_compensation"),
            ("DspFilterDelay_µs", "filter_delay_us"),
        ] {
            if let Some(v) = h.get(k) {
                let val = h
                    .flag(k)
                    .map(Value::Bool)
                    .or_else(|| v.parse::<f64>().ok().map(|n| json!(n)))
                    .unwrap_or_else(|| json!(v));
                dsp.insert(name.into(), val);
            }
        }
        if !dsp.is_empty() {
            e.insert("dsp".into(), Value::Object(dsp));
        }
        if let Some(r) = h.text("ReferenceChannel") {
            e.insert("reference".into(), json!(r));
        }
        e
    }

    fn file_extra(&self) -> BTreeMap<String, Value> {
        let h = &self.header;
        let mut e = BTreeMap::new();
        e.insert("file_kind".into(), json!(self.kind.name()));
        e.insert("record_count".into(), json!(self.record_count()));
        if let Some((app, ver)) = h.application() {
            e.insert("application".into(), json!(app));
            if !ver.is_empty() {
                e.insert("application_version".into(), json!(ver));
            }
        }
        for (k, name) in [
            ("AcquisitionSystem", "acquisition_system"),
            ("HardwareSubSystemType", "hardware_subsystem"),
            ("FileVersion", "file_version"),
            ("FileUUID", "file_uuid"),
            ("SessionUUID", "session_uuid"),
            ("NLX_Base_Class_Type", "base_class"),
        ] {
            if let Some(v) = h.text(k) {
                e.insert(name.into(), json!(v));
            }
        }
        if let Some(v) = h.opened_at() {
            e.insert("opened_at".into(), json!(v));
        }
        if let Some(v) = h.closed_at() {
            e.insert("closed_at".into(), json!(v));
        }
        if let Some(v) = h.original_file_name() {
            e.insert("original_file_name".into(), json!(v));
        }
        e
    }

    fn entity_name(&self) -> Option<String> {
        self.header
            .text("AcqEntName")
            .or_else(|| self.header.text("NLX_Base_Class_Name"))
    }

    fn trace_info(&self, idx: &ContinuousIndex) -> TraceInfo {
        let mut extra = self.file_extra();
        let header_rate = self.header.number("SamplingFrequency");
        let rate = header_rate
            .or_else(|| idx.first.map(|f| f64::from(f.rate_hz)))
            .unwrap_or(0.0);
        if let Some(f) = idx.first {
            extra.insert("first_timestamp_us".into(), json!(f.timestamp_us));
            extra.insert("record_channel_number".into(), json!(f.channel_number));
        }
        if let Some(dt) = idx.sample_interval_us {
            extra.insert("timestamp_rate_hz".into(), json!(1e6 / dt));
        }
        let counts: Vec<u64> = idx.segments.iter().map(|s| s.sample_count).collect();
        if counts.windows(2).any(|w| w[0] != w[1]) {
            extra.insert("sweep_sample_counts".into(), json!(counts));
        }
        if let Some(f) = idx.first {
            extra.insert(
                "sweep_starts_s".into(),
                json!(
                    idx.segments
                        .iter()
                        .map(|s| (s.first_timestamp_us as f64 - f.timestamp_us as f64) / 1e6)
                        .collect::<Vec<_>>()
                ),
            );
            extra.insert(
                "sweep_start_timestamps_us".into(),
                json!(
                    idx.segments
                        .iter()
                        .map(|s| s.first_timestamp_us)
                        .collect::<Vec<_>>()
                ),
            );
        }
        extra.insert(
            "segmented_by".into(),
            json!(if idx.scanned {
                "every record timestamp"
            } else {
                "first and last record (one gap-free run; `check` reads every record)"
            }),
        );
        let scale = self.uv_per_count(0).unwrap_or(1.0);
        TraceInfo {
            index: 0,
            name: self.entity_name(),
            sample_rate_hz: rate,
            sample_count: counts.iter().copied().max().unwrap_or(0),
            sweep_count: idx.segments.len() as u32,
            channels: vec![SignalChannelInfo {
                index: 0,
                name: self.entity_name().unwrap_or_else(|| "ch0".into()),
                unit: self.uv_per_count(0).map(|_| "µV".to_string()),
                dtype: "int16".into(),
                scale,
                offset: 0.0,
                extra: self.channel_extra(),
            }],
            start_s: None,
            extra,
        }
    }

    fn table_columns(&self) -> Vec<ColumnInfo> {
        let col = |i: usize, name: String, dtype: &str, unit: Option<&str>| ColumnInfo {
            index: i as u32,
            name,
            label: None,
            dtype: dtype.into(),
            unit: unit.map(str::to_string),
            range: None,
            extra: BTreeMap::new(),
        };
        match self.kind {
            FileKind::Events => vec![
                col(0, "timestamp_us".into(), "uint64", Some("µs")),
                col(1, "event_id".into(), "int16", None),
                col(2, "ttl".into(), "int16", None),
                col(3, "packet_id".into(), "int16", None),
                col(4, "label".into(), "int32", None),
            ],
            FileKind::Spikes { electrodes } => {
                let mut v = vec![
                    col(0, "timestamp_us".into(), "uint64", Some("µs")),
                    col(1, "entity".into(), "uint32", None),
                    col(2, "cell".into(), "uint32", None),
                ];
                for k in 0..SPIKE_FEATURES {
                    v.push(col(v.len(), format!("feature_{k}"), "int32", None));
                }
                for e in 0..usize::from(electrodes) {
                    for s in 0..SPIKE_SAMPLES as usize {
                        v.push(col(v.len(), format!("w{e}_{s}"), "float64", Some("µV")));
                    }
                }
                v
            }
            FileKind::Video => vec![
                col(0, "timestamp_us".into(), "uint64", Some("µs")),
                col(1, "x".into(), "int32", Some("px")),
                col(2, "y".into(), "int32", Some("px")),
                col(3, "angle".into(), "int32", Some("°")),
                col(4, "target_count".into(), "uint8", None),
            ],
            FileKind::Continuous => Vec::new(),
        }
    }

    fn read_events(&mut self, first: u64, n: u64) -> Result<Vec<crate::records::EventRecord>> {
        let len = self.kind.record_len();
        let at = HEADER_LEN + first * len;
        let (path, file_len) = (self.path.clone(), self.file_len);
        let b = read_block(self.handle()?, &path, at, n * len, file_len)?;
        (0..n)
            .map(|k| {
                event_record(&b, at + k * len).ok_or_else(|| {
                    Error::corrupt_at(FORMAT_ID, at + k * len, "event record cut off")
                })
            })
            .collect()
    }

    fn event_labels(&mut self) -> Result<(Vec<String>, BTreeMap<String, u64>)> {
        let n = self.record_count().min(MAX_LABELS as u64 * 100);
        let evs = self.read_events(0, n)?;
        let mut order = Vec::new();
        let mut counts = BTreeMap::new();
        for e in evs {
            if !counts.contains_key(&e.label) && order.len() < MAX_LABELS {
                order.push(e.label.clone());
            }
            *counts.entry(e.label).or_insert(0u64) += 1;
        }
        Ok((order, counts))
    }

    fn table_info(&self, labels: Option<(Vec<String>, BTreeMap<String, u64>)>) -> TableInfo {
        let mut extra = self.file_extra();
        extra.insert("record_size".into(), json!(self.kind.record_len()));
        match self.kind {
            FileKind::Spikes { electrodes } => {
                extra.insert("electrodes".into(), json!(electrodes));
                extra.insert("samples_per_waveform".into(), json!(SPIKE_SAMPLES));
                if let Some(v) = self.header.number("AlignmentPt") {
                    extra.insert("alignment_sample".into(), json!(v));
                }
                if let Some(v) = self.header.number("SamplingFrequency") {
                    extra.insert("waveform_rate_hz".into(), json!(v));
                }
                let uv: Vec<f64> = (0..usize::from(electrodes))
                    .filter_map(|i| self.uv_per_count(i))
                    .collect();
                extra.insert("uv_per_count".into(), json!(uv));
                let feats: Vec<String> = self
                    .header
                    .entries
                    .iter()
                    .filter(|e| e.key.eq_ignore_ascii_case("Feature"))
                    .map(|e| e.value.clone())
                    .collect();
                if !feats.is_empty() {
                    extra.insert("features".into(), json!(feats));
                }
                extra.insert("channel".into(), json!(self.channel_extra()));
            }
            FileKind::Events => {
                if let Some((order, counts)) = labels {
                    extra.insert("labels".into(), json!(order));
                    extra.insert("label_counts".into(), json!(counts));
                }
            }
            FileKind::Video => {
                for (k, name) in [
                    ("SamplingFrequency", "frame_rate_hz"),
                    ("VideoFormat", "video_format"),
                    ("Resolution", "resolution"),
                    ("AcqEntName", "entity"),
                ] {
                    if let Some(v) = self.header.text(k) {
                        extra.insert(name.into(), json!(v));
                    }
                }
            }
            FileKind::Continuous => {}
        }
        TableInfo {
            index: 0,
            name: self
                .entity_name()
                .or_else(|| Some(self.kind.name().to_string())),
            row_count: self.record_count(),
            columns: self.table_columns(),
            extra,
        }
    }

    /// Sample positions: for sweep `sweep`, sample `first` → (record, offset within record).
    fn locate(idx: &ContinuousIndex, sweep: u32, first: u64) -> Option<(u64, u64)> {
        let seg = idx.segments.get(sweep as usize)?;
        let mut left = first;
        for r in seg.first_record..seg.first_record + seg.record_count {
            let v = idx.valid_in(r);
            if left < v {
                return Some((r, left));
            }
            left -= v;
        }
        None
    }
}

impl Dataset for NeuralynxDataset {
    fn info(&self) -> Result<FileInfo> {
        let mut notes = Vec::new();
        let (traces, tables) = match (&self.index, self.kind) {
            (Some(idx), _) => {
                notes.push("continuous channel: one trace, segments (gap-free runs of records) are sweeps; values in µV = raw × ADBitVolts × 1e6 (no sign change for -InputInverted)".into());
                (vec![self.trace_info(idx)], Vec::new())
            }
            (None, FileKind::Events) => {
                // labels need a read; `info` takes &self, so open a fresh handle
                let mut me = NeuralynxDataset {
                    path: self.path.clone(),
                    file_len: self.file_len,
                    kind: self.kind,
                    header: self.header.clone(),
                    index: None,
                    handle: None,
                    fs: self.fs.clone(),
                };
                let labels = me.event_labels().ok();
                notes.push(
                    "events: one row per record; `label` is an index into `extra.labels`".into(),
                );
                (Vec::new(), vec![self.table_info(labels)])
            }
            (None, FileKind::Video) => {
                notes.push("video tracker: one row per frame; x and y are the extracted target position in pixels (0, 0 when nothing was tracked), angle the head angle in degrees clockwise from +Y (0 when angle tracking is off; invalid before Cheetah 5)".into());
                (Vec::new(), vec![self.table_info(None)])
            }
            (None, _) => {
                notes.push(
                    "spikes: one row per waveform; w<e>_<i> is sample i of electrode e in µV"
                        .into(),
                );
                (Vec::new(), vec![self.table_info(None)])
            }
        };
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.file_len,
            format: NeuralynxReader.descriptor(),
            format_version: self.header.text("FileVersion"),
            images: Vec::new(),
            tables,
            spectra: Vec::new(),
            traces,
            plane_count: 0,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let mut m = serde_json::Map::new();
        for e in &self.header.entries {
            m.insert(e.key.clone(), json!(e.value));
        }
        Ok(json!({
            "header": m,
            "comments": self.header.comments,
            "has_header_line": self.header.has_magic,
        }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for (k, s) in [
            ("traces[].sample_rate_hz", Source::VendorImpl),
            ("traces[].channels[].scale", Source::VendorImpl),
            ("traces[].channels[].unit", Source::VendorImpl),
            ("traces[].sweep_count", Source::Inferred),
            ("traces[].extra.sweep_sample_counts", Source::VendorImpl),
            ("traces[].extra.timestamp_rate_hz", Source::Inferred),
            ("traces[].channels[].extra", Source::PriorArt),
            ("tables[].columns", Source::VendorImpl),
            ("tables[].columns[feature_*]", Source::Inferred),
            ("tables[].extra.opened_at", Source::PriorArt),
        ] {
            p.insert(k.into(), s);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let mut out = vec![LsEntry {
            kind: "header".into(),
            name: "text header".into(),
            offset: Some(0),
            size: Some(HEADER_LEN),
            image: None,
            details: json!({"entries": self.header.entries.len(), "has_header_line": self.header.has_magic}),
        }];
        let len = self.kind.record_len();
        match &self.index {
            Some(idx) => {
                for (i, s) in idx.segments.iter().enumerate() {
                    out.push(LsEntry {
                        kind: "sweep".into(),
                        name: format!("segment {i}"),
                        offset: Some(HEADER_LEN + s.first_record * len),
                        size: Some(s.record_count * len),
                        image: None,
                        details: json!({"records": s.record_count, "samples": s.sample_count, "first_timestamp_us": s.first_timestamp_us}),
                    });
                }
            }
            None => out.push(LsEntry {
                kind: "records".into(),
                name: format!("{} records", self.kind.name()),
                offset: Some(HEADER_LEN),
                size: Some(self.record_count() * len),
                image: None,
                details: json!({"records": self.record_count(), "record_size": len}),
            }),
        }
        Ok(out)
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::unsupported(
            FORMAT_ID,
            "image planes",
            "Neuralynx files hold signals, events or spikes, not images: use `openreadout trace` (NCS) or `openreadout export --format csv`.",
        ))
    }

    fn read_trace(
        &mut self,
        index: u32,
        sweep: u32,
        first_sample: u64,
        max_samples: u64,
    ) -> Result<Trace> {
        let Some(idx) = &self.index else {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!("sampled-signal reads of a .{} file", self.kind.name()),
                "Event and spike files are tables: use `openreadout export FILE --format csv` or the openreadout_table MCP tool.",
            ));
        };
        if index != 0 {
            return Err(Error::Usage(format!(
                "trace {index} out of range (an NCS file holds one trace, index 0)"
            )));
        }
        let seg = idx.segments.get(sweep as usize).cloned().ok_or_else(|| {
            Error::Usage(format!(
                "sweep {sweep} out of range (file has {} segments)",
                idx.segments.len()
            ))
        })?;
        if first_sample > seg.sample_count {
            return Err(Error::Usage(format!(
                "first sample {first_sample} is past the end of segment {sweep} ({} samples)",
                seg.sample_count
            )));
        }
        let n = max_samples
            .min(seg.sample_count - first_sample)
            .min(1 << 26);
        let scale = self.uv_per_count(0).unwrap_or(1.0);
        let mut out = Vec::with_capacity(n as usize);
        if n > 0 {
            let (mut rec, mut off) = Self::locate(idx, sweep, first_sample).ok_or_else(|| {
                Error::Other(format!(
                    "sample {first_sample} not located in segment {sweep}"
                ))
            })?;
            let valid: Vec<u64> = (rec..seg.first_record + seg.record_count)
                .map(|r| idx.valid_in(r))
                .collect();
            let start_rec = rec;
            let (path, file_len) = (self.path.clone(), self.file_len);
            while (out.len() as u64) < n {
                let batch = 256u64.min(seg.first_record + seg.record_count - rec);
                let at = HEADER_LEN + rec * NCS_RECORD_LEN;
                if at + batch * NCS_RECORD_LEN > file_len {
                    return Err(Error::corrupt_at(
                        FORMAT_ID,
                        at,
                        "record data runs past the end of the file (truncated)",
                    ));
                }
                let b = read_block(self.handle()?, &path, at, batch * NCS_RECORD_LEN, file_len)?;
                for k in 0..batch {
                    let v = valid[(rec + k - start_rec) as usize];
                    let base = at + k * NCS_RECORD_LEN + 20;
                    let mut i = off;
                    while i < v && (out.len() as u64) < n {
                        let raw = b
                            .i16_at(base + 2 * i)
                            .ok_or_else(|| Error::corrupt_at(FORMAT_ID, base, "record cut off"))?;
                        out.push(f64::from(raw) * scale);
                        i += 1;
                    }
                    off = 0;
                    if (out.len() as u64) >= n {
                        break;
                    }
                }
                rec += batch;
                if rec >= seg.first_record + seg.record_count {
                    break;
                }
            }
        }
        Ok(Trace {
            trace: 0,
            sweep,
            first_sample,
            channels: vec![out],
        })
    }

    fn read_table(&mut self, index: u32, first_row: u64, max_rows: u64) -> Result<Table> {
        if self.kind == FileKind::Continuous || index != 0 {
            return Err(if self.kind == FileKind::Continuous {
                Error::unsupported(
                    FORMAT_ID,
                    "table reads of an NCS file",
                    "NCS files are continuous signals: use `openreadout trace` or `export --format csv`.",
                )
            } else {
                Error::Usage(format!("table {index} out of range (file has 1 table)"))
            });
        }
        let total = self.record_count();
        if first_row > total {
            return Err(Error::Usage(format!(
                "first row {first_row} is past the last record ({total} records)"
            )));
        }
        let n = max_rows.min(total - first_row).min(MAX_TABLE_READ);
        let ncols = self.table_columns().len();
        let mut cols: Vec<Vec<f64>> = vec![Vec::with_capacity(n as usize); ncols];
        match self.kind {
            FileKind::Events => {
                let labels = self.event_labels()?.0;
                for e in self.read_events(first_row, n)? {
                    cols[0].push(e.timestamp_us as f64);
                    cols[1].push(f64::from(e.event_id));
                    cols[2].push(f64::from(e.ttl));
                    cols[3].push(f64::from(e.packet_id));
                    cols[4].push(
                        labels
                            .iter()
                            .position(|l| *l == e.label)
                            .map_or(-1.0, |p| p as f64),
                    );
                }
            }
            FileKind::Spikes { electrodes } => {
                let len = self.kind.record_len();
                let at = HEADER_LEN + first_row * len;
                let (path, file_len) = (self.path.clone(), self.file_len);
                let b = read_block(self.handle()?, &path, at, n * len, file_len)?;
                let uv: Vec<f64> = (0..usize::from(electrodes))
                    .map(|i| self.uv_per_count(i).unwrap_or(1.0))
                    .collect();
                for k in 0..n {
                    let s = spike_record(&b, at + k * len, electrodes).ok_or_else(|| {
                        Error::corrupt_at(FORMAT_ID, at + k * len, "spike record cut off")
                    })?;
                    cols[0].push(s.timestamp_us as f64);
                    cols[1].push(f64::from(s.entity));
                    cols[2].push(f64::from(s.cell));
                    for (j, x) in s.features.iter().enumerate() {
                        cols[3 + j].push(f64::from(*x));
                    }
                    let ne = usize::from(electrodes);
                    for e in 0..ne {
                        for p in 0..SPIKE_SAMPLES as usize {
                            // samples are stored [point, electrode]
                            cols[3 + SPIKE_FEATURES + e * SPIKE_SAMPLES as usize + p]
                                .push(f64::from(s.samples[p * ne + e]) * uv[e]);
                        }
                    }
                }
            }
            FileKind::Video => {
                let len = self.kind.record_len();
                let at = HEADER_LEN + first_row * len;
                let (path, file_len) = (self.path.clone(), self.file_len);
                let b = read_block(self.handle()?, &path, at, n * len, file_len)?;
                for k in 0..n {
                    let v = video_record(&b, at + k * len).ok_or_else(|| {
                        Error::corrupt_at(FORMAT_ID, at + k * len, "video record cut off")
                    })?;
                    cols[0].push(v.timestamp_us as f64);
                    cols[1].push(f64::from(v.x));
                    cols[2].push(f64::from(v.y));
                    cols[3].push(f64::from(v.angle));
                    cols[4].push(f64::from(v.target_count));
                }
            }
            FileKind::Continuous => unreachable!("handled above"),
        }
        Ok(Table {
            table: 0,
            first_row,
            columns: cols,
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), FORMAT_ID);
        r.performed(
            "16 KiB text header present and parsed; `-RecordSize` agrees with the record type",
        );
        r.performed("file length = header + whole records (a cut-off record is truncation)");
        let header = self.header.clone();
        if !header.has_magic {
            r.push(Finding::info(
                "no_header_line",
                "the header does not start with the Neuralynx header line",
            ));
        }
        if let Some(rs) = header.number("RecordSize")
            && rs as u64 != self.kind.record_len()
        {
            r.push(Finding::error(
                "bad_record_size",
                format!(
                    "-RecordSize {rs} does not match {}-byte .{} records",
                    self.kind.record_len(),
                    self.kind.name()
                ),
            ));
        }
        let len = self.kind.record_len();
        let data = self.file_len - HEADER_LEN;
        if self.kind == FileKind::Continuous {
            {
                r.performed("every record: valid-sample count ≤ 512, timestamps increase, one channel number");
                r.performed("segments: gaps between records; sampling rate implied by timestamps vs the header");
                let (path, file_len) = (self.path.clone(), self.file_len);
                let rate = header.number("SamplingFrequency");
                let full = index_continuous(self.handle()?, &path, file_len, rate, true)?;
                for finding in &full.findings {
                    r.push(finding.clone());
                }
                if full.segments.len() > 1 {
                    r.push(Finding::info(
                        "segments",
                        format!(
                            "{} gap-free segments (recording stopped/restarted or samples lost)",
                            full.segments.len()
                        ),
                    ));
                }
                if let (Some(dt), Some(rate)) = (full.sample_interval_us, rate) {
                    let ts_rate = 1e6 / dt;
                    if ((ts_rate - rate) / rate).abs() > 0.005 {
                        r.push(Finding::warning(
                            "rate_mismatch",
                            format!(
                                "timestamps imply {ts_rate:.3} Hz but the header says {rate} Hz"
                            ),
                        ));
                    }
                }
                if let Some(idx) = &self.index
                    && idx.segments.len() != full.segments.len()
                {
                    r.push(Finding::info(
                        "fast_index_differs",
                        "the header-only index missed segments that the full scan found",
                    ));
                }
                if full.record_count == 0 {
                    r.push(Finding::warning("no_records", "the file holds no records"));
                }
            }
        } else {
            {
                r.performed("record timestamps increase");
                if !data.is_multiple_of(len) {
                    r.push(
                        Finding::error(
                            "truncated",
                            format!("{} bytes after the last whole record", data % len),
                        )
                        .at(HEADER_LEN + (data / len) * len),
                    );
                }
                let count = self.record_count();
                let (path, file_len) = (self.path.clone(), self.file_len);
                let mut prev = 0u64;
                let mut back = 0u64;
                let mut bad_start = 0u64;
                let mut pos = 0u64;
                while pos < count {
                    let batch = 65_536u64.min(count - pos);
                    let at = HEADER_LEN + pos * len;
                    let chunk = read_block(self.handle()?, &path, at, batch * len, file_len)?;
                    for i in 0..batch {
                        let ts = match self.kind {
                            FileKind::Events | FileKind::Video => chunk.u64_at(at + i * len + 6),
                            _ => chunk.u64_at(at + i * len),
                        }
                        .unwrap_or(0);
                        if self.kind == FileKind::Video
                            && chunk.u16_at(at + i * len) != Some(NVT_RECORD_START)
                        {
                            bad_start += 1;
                        }
                        if ts < prev {
                            back += 1;
                        }
                        prev = ts;
                    }
                    pos += batch;
                }
                if back > 0 {
                    r.push(Finding::warning(
                        "timestamps_decrease",
                        format!("{back} record timestamps go backwards"),
                    ));
                }
                if bad_start > 0 {
                    r.push(Finding::warning(
                        "bad_video_record",
                        format!("{bad_start} video records do not start with 0x800"),
                    ));
                }
                if count == 0 {
                    r.push(Finding::warning("no_records", "the file holds no records"));
                }
            }
        }
        if header.number("ADBitVolts").is_none()
            && !matches!(self.kind, FileKind::Events | FileKind::Video)
        {
            r.push(Finding::warning(
                "no_scaling",
                "no -ADBitVolts in the header: values are raw counts",
            ));
        }
        let _ = NCS_SAMPLES;
        Ok(r)
    }
}
