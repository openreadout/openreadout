//! `PlxDataset`: continuous channels as traces (channels on one sample grid share a trace, gap-free
//! runs are sweeps), spike waveforms and events as two tables.

use openreadout_core::source::SourceFile as File;
use openreadout_core::source::{Fs, Input};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_core::bytes::read_block;
use openreadout_core::model::{
    CheckReport, ColumnInfo, FileInfo, Finding, LsEntry, SignalChannelInfo, Table, TableInfo,
    Trace, TraceInfo,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::{Error, Plane, Result};
use serde_json::{Value, json};

use crate::plx::{
    BLOCK_HEADER_LEN, ContinuousChannel, PlxFile, Run, SampleBlock, block_header, parse_plx, runs,
};
use crate::{FORMAT_ID, PlexonReader};

/// Rows decoded per `read_table` call at most.
pub const MAX_TABLE_READ: u64 = 1 << 20;

/// One trace: continuous channels (indices into the header list) on one sample grid.
#[derive(Debug, Clone)]
pub struct PlxTrace {
    /// Continuous-channel header indices, in header order.
    pub channels: Vec<usize>,
    /// Sampling rate, Hz.
    pub rate_hz: f64,
    /// Gap-free runs (sweeps), identical for every channel of the trace.
    pub runs: Vec<Run>,
}

/// An opened PLX file.
#[derive(Debug)]
pub struct PlxDataset {
    path: PathBuf,
    fs: Fs,
    plx: PlxFile,
    traces: Vec<PlxTrace>,
    /// Continuous channels with blocks but no header (channel numbers).
    orphans: Vec<u16>,
    handle: Option<File>,
}

fn not_images() -> Error {
    Error::unsupported(
        FORMAT_ID,
        "image planes",
        "Plexon files hold continuous signals, spikes and events: use `openreadout trace` for signals and `openreadout export --to csv --table N` for tables.",
    )
}

/// The alphabetic prefix of a channel name (`WB01` → `WB`).
fn prefix(name: &str) -> &str {
    name.trim_end_matches(|c: char| c.is_ascii_digit())
}

impl PlxDataset {
    /// Open and index a PLX file.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    pub(crate) fn open_input(input: &Input) -> Result<Self> {
        let path = input.path();
        let fs = input.fs();
        let mut f = fs.open(path).map_err(|e| Error::io(path, e))?;
        let len = f.metadata().map_err(|e| Error::io(path, e))?.len();
        let plx = parse_plx(&mut f, path, len)?;
        let clock = f64::from(plx.header.clock_hz);
        let mut traces: Vec<PlxTrace> = Vec::new();
        for (i, c) in plx.continuous_channels.iter().enumerate() {
            let Some(blocks) = u16::try_from(c.channel)
                .ok()
                .and_then(|ch| plx.index.continuous.get(&ch))
            else {
                continue;
            };
            let rate = f64::from(c.rate_hz);
            let r = runs(blocks, clock, rate);
            let grid: Vec<(u64, u64)> = r.iter().map(|x| (x.timestamp, x.samples)).collect();
            match traces.iter_mut().find(|t| {
                t.rate_hz.to_bits() == rate.to_bits()
                    && t.runs
                        .iter()
                        .map(|x| (x.timestamp, x.samples))
                        .eq(grid.iter().copied())
            }) {
                Some(t) => t.channels.push(i),
                None => traces.push(PlxTrace {
                    channels: vec![i],
                    rate_hz: rate,
                    runs: r,
                }),
            }
        }
        let known: Vec<i32> = plx.continuous_channels.iter().map(|c| c.channel).collect();
        let orphans = plx
            .index
            .continuous
            .keys()
            .filter(|ch| !known.contains(&i32::from(**ch)))
            .copied()
            .collect();
        Ok(PlxDataset {
            path: path.to_path_buf(),
            fs: fs.clone(),
            plx,
            traces,
            orphans,
            handle: Some(f),
        })
    }

    /// The parsed file.
    pub fn plx(&self) -> &PlxFile {
        &self.plx
    }

    fn clock(&self) -> f64 {
        f64::from(self.plx.header.clock_hz.max(1))
    }

    fn handle(&mut self) -> Result<&mut File> {
        if self.handle.is_none() {
            self.handle = Some(
                self.fs
                    .open(&self.path)
                    .map_err(|e| Error::io(&self.path, e))?,
            );
        }
        Ok(self.handle.as_mut().expect("just opened"))
    }

    fn blocks(&self, c: &ContinuousChannel) -> &[SampleBlock] {
        u16::try_from(c.channel)
            .ok()
            .and_then(|ch| self.plx.index.continuous.get(&ch))
            .map_or(&[], Vec::as_slice)
    }

    fn trace_info(&self, index: usize, t: &PlxTrace) -> TraceInfo {
        let h = &self.plx.header;
        let clock = self.clock();
        let channels: Vec<SignalChannelInfo> = t
            .channels
            .iter()
            .enumerate()
            .map(|(k, &i)| {
                let c = &self.plx.continuous_channels[i];
                let mut e = BTreeMap::new();
                e.insert("channel_number".into(), json!(c.channel));
                e.insert("gain".into(), json!(c.gain));
                e.insert("preamp_gain".into(), json!(c.preamp_gain));
                e.insert("enabled".into(), json!(c.enabled));
                if !c.comment.is_empty() {
                    e.insert("comment".into(), json!(c.comment));
                }
                SignalChannelInfo {
                    index: k as u32,
                    name: c.name.clone(),
                    unit: Some("mV".into()),
                    dtype: "int16".into(),
                    scale: h.continuous_scale(c),
                    offset: 0.0,
                    extra: e,
                }
            })
            .collect();
        let names: Vec<&str> = t
            .channels
            .iter()
            .map(|&i| prefix(&self.plx.continuous_channels[i].name))
            .collect();
        let name = names
            .first()
            .filter(|p| !p.is_empty() && names.iter().all(|x| x == *p))
            .map(|p| (*p).to_string());
        let mut extra = BTreeMap::new();
        let counts: Vec<u64> = t.runs.iter().map(|r| r.samples).collect();
        if counts.windows(2).any(|w| w[0] != w[1]) {
            extra.insert("sweep_sample_counts".into(), json!(counts));
        }
        extra.insert(
            "sweep_start_timestamps".into(),
            json!(t.runs.iter().map(|r| r.timestamp).collect::<Vec<_>>()),
        );
        extra.insert(
            "sweep_starts_s".into(),
            json!(
                t.runs
                    .iter()
                    .map(|r| r.timestamp as f64 / clock)
                    .collect::<Vec<_>>()
            ),
        );
        extra.insert("timestamp_clock_hz".into(), json!(h.clock_hz));
        TraceInfo {
            index: index as u32,
            name,
            sample_rate_hz: t.rate_hz,
            sample_count: counts.iter().copied().max().unwrap_or(0),
            sweep_count: t.runs.len() as u32,
            channels,
            start_s: t.runs.first().map(|r| r.timestamp as f64 / clock),
            extra,
        }
    }

    fn spike_columns(&self) -> Vec<ColumnInfo> {
        let col = |i: usize, name: String, dtype: &str, unit: Option<&str>| ColumnInfo {
            index: i as u32,
            name,
            label: None,
            dtype: dtype.into(),
            unit: unit.map(str::to_string),
            range: None,
            extra: BTreeMap::new(),
        };
        let mut v = vec![
            col(0, "time_s".into(), "float64", Some("s")),
            col(1, "timestamp".into(), "uint64", None),
            col(2, "channel".into(), "uint16", None),
            col(3, "unit".into(), "uint16", None),
        ];
        for i in 0..self.plx.index.max_waveform as usize {
            v.push(col(v.len(), format!("w{i}"), "float64", Some("mV")));
        }
        v
    }

    fn event_columns() -> Vec<ColumnInfo> {
        let col = |i: u32, name: &str, dtype: &str, unit: Option<&str>| ColumnInfo {
            index: i,
            name: name.into(),
            label: None,
            dtype: dtype.into(),
            unit: unit.map(str::to_string),
            range: None,
            extra: BTreeMap::new(),
        };
        vec![
            col(0, "time_s", "float64", Some("s")),
            col(1, "timestamp", "uint64", None),
            col(2, "channel", "uint16", None),
            col(3, "value", "uint16", None),
        ]
    }

    /// (spike table index, event table index).
    fn table_ids(&self) -> (Option<u32>, Option<u32>) {
        let spikes =
            (!self.plx.spike_channels.is_empty() || !self.plx.index.spikes.is_empty()).then_some(0);
        let events = (!self.plx.event_channels.is_empty() || !self.plx.index.events.is_empty())
            .then_some(u32::from(spikes.is_some()));
        (spikes, events)
    }

    fn tables(&self) -> Vec<TableInfo> {
        let h = &self.plx.header;
        let (s, e) = self.table_ids();
        let mut out = Vec::new();
        if let Some(i) = s {
            let mut extra = BTreeMap::new();
            extra.insert("timestamp_clock_hz".into(), json!(h.clock_hz));
            extra.insert("waveform_rate_hz".into(), json!(h.waveform_rate_hz));
            extra.insert("waveform_points".into(), json!(h.waveform_points));
            extra.insert("pre_threshold_points".into(), json!(h.pre_threshold_points));
            extra.insert(
                "channels".into(),
                json!(self.plx.spike_channels.iter().map(|c| json!({
                    "channel_number": c.channel, "name": c.name, "signal_name": c.signal_name,
                    "gain": c.gain, "mv_per_count": h.spike_scale(c), "threshold": c.threshold,
                    "filter": c.filter, "unit_count": c.unit_count,
                })).collect::<Vec<_>>()),
            );
            out.push(TableInfo {
                index: i,
                name: Some("spikes".into()),
                row_count: self.plx.index.spikes.len() as u64,
                columns: self.spike_columns(),
                extra,
            });
        }
        if let Some(i) = e {
            let mut extra = BTreeMap::new();
            extra.insert("timestamp_clock_hz".into(), json!(h.clock_hz));
            extra.insert(
                "channels".into(),
                json!(
                    self.plx
                        .event_channels
                        .iter()
                        .map(|c| json!({
                            "channel_number": c.channel, "name": c.name,
                        }))
                        .collect::<Vec<_>>()
                ),
            );
            out.push(TableInfo {
                index: i,
                name: Some("events".into()),
                row_count: self.plx.index.events.len() as u64,
                columns: Self::event_columns(),
                extra,
            });
        }
        out
    }

    fn read_spikes(&mut self, first: u64, n: u64) -> Result<Table> {
        let width = self.plx.index.max_waveform as usize;
        let mut cols: Vec<Vec<f64>> = (0..4 + width)
            .map(|_| Vec::with_capacity(n as usize))
            .collect();
        let clock = self.clock();
        let scales: BTreeMap<i32, f64> = self
            .plx
            .spike_channels
            .iter()
            .map(|c| (c.channel, self.plx.header.spike_scale(c)))
            .collect();
        let offsets: Vec<u64> =
            self.plx.index.spikes[first as usize..(first + n) as usize].to_vec();
        let (path, len) = (self.path.clone(), self.plx.file_len);
        let mut buf = openreadout_core::bytes::Block::default();
        let max_block = BLOCK_HEADER_LEN + 2 * u64::from(self.plx.index.max_waveform);
        for at in offsets {
            if at < buf.origin || at + max_block > buf.end() {
                buf = read_block(self.handle()?, &path, at, (4 << 20).max(max_block), len)?;
            }
            let (_, ts, ch, unit, waves, words) = block_header(&buf, at)
                .ok_or_else(|| Error::corrupt_at(FORMAT_ID, at, "spike block cut off"))?;
            let scale = scales.get(&i32::from(ch)).copied().unwrap_or(1.0);
            cols[0].push(ts as f64 / clock);
            cols[1].push(ts as f64);
            cols[2].push(f64::from(ch));
            cols[3].push(f64::from(unit));
            let k = usize::from(waves) * usize::from(words);
            for w in 0..width {
                let v = if w < k {
                    let raw = buf
                        .i16_at(at + BLOCK_HEADER_LEN + 2 * w as u64)
                        .ok_or_else(|| {
                            Error::corrupt_at(FORMAT_ID, at, "spike waveform cut off")
                        })?;
                    f64::from(raw) * scale
                } else {
                    f64::NAN
                };
                cols[4 + w].push(v);
            }
        }
        Ok(Table {
            table: 0,
            first_row: first,
            columns: cols,
        })
    }
}

impl Dataset for PlxDataset {
    fn info(&self) -> Result<FileInfo> {
        let h = &self.plx.header;
        let mut traces: Vec<TraceInfo> = self
            .traces
            .iter()
            .enumerate()
            .map(|(i, t)| self.trace_info(i, t))
            .collect();
        let mut tables = self.tables();
        let first = traces
            .first_mut()
            .map(|t| &mut t.extra)
            .or_else(|| tables.first_mut().map(|t| &mut t.extra));
        if let Some(e) = first {
            if let Some(d) = &h.recorded_at {
                e.insert("recorded_at".into(), json!(d));
            }
            if !h.acquired_with.is_empty() {
                e.insert("application".into(), json!(h.acquired_with));
            }
        }
        let mut notes = vec![format!(
            "PLX version {}: continuous channels on one sample grid form a trace (gap-free runs of blocks are sweeps; values in mV = raw × scale); spike waveforms and events are tables; timestamps count a {} Hz clock",
            h.version, h.clock_hz
        )];
        let empty = self
            .plx
            .continuous_channels
            .iter()
            .filter(|c| self.blocks(c).is_empty())
            .count();
        if empty > 0 {
            notes.push(format!(
                "{empty} continuous channel header(s) without data blocks are not listed as channels"
            ));
        }
        if self
            .plx
            .index
            .findings
            .iter()
            .any(|f| f.severity == openreadout_core::model::Severity::Error)
        {
            notes.push("structural problems found (e.g. truncation); run `check`".into());
        }
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.plx.file_len,
            format: PlexonReader.descriptor(),
            format_version: Some(format!("PLX {}", h.version)),
            images: Vec::new(),
            tables,
            spectra: Vec::new(),
            traces,
            plane_count: 0,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let p = &self.plx;
        let h = &p.header;
        Ok(json!({
            "container": "plx",
            "version": h.version, "comment": h.comment, "clock_hz": h.clock_hz,
            "waveform_points": h.waveform_points, "pre_threshold_points": h.pre_threshold_points,
            "recorded_at": h.recorded_at, "waveform_rate_hz": h.waveform_rate_hz,
            "last_timestamp": h.last_timestamp, "electrodes_per_channel": h.electrodes_per_channel,
            "data_electrodes_per_channel": h.data_electrodes_per_channel, "spike_bits": h.spike_bits,
            "continuous_bits": h.continuous_bits, "spike_max_mv": h.spike_max_mv,
            "continuous_max_mv": h.continuous_max_mv, "spike_preamp_gain": h.spike_preamp_gain,
            "acquired_with": h.acquired_with, "processed_with": h.processed_with,
            "spike_channels": p.spike_channels.iter().map(|c| json!({
                "name": c.name, "signal_name": c.signal_name, "channel": c.channel, "gain": c.gain,
                "filter": c.filter, "threshold": c.threshold, "unit_count": c.unit_count, "comment": c.comment,
            })).collect::<Vec<_>>(),
            "event_channels": p.event_channels.iter().map(|c| json!({
                "name": c.name, "channel": c.channel, "comment": c.comment,
            })).collect::<Vec<_>>(),
            "continuous_channels": p.continuous_channels.iter().map(|c| json!({
                "name": c.name, "channel": c.channel, "rate_hz": c.rate_hz, "gain": c.gain,
                "enabled": c.enabled, "preamp_gain": c.preamp_gain, "comment": c.comment,
                "blocks": self.blocks(c).len(),
            })).collect::<Vec<_>>(),
            "block_counts": p.index.block_counts,
            "data_start": p.data_start,
        }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for (k, s) in [
            ("traces[].sample_rate_hz", Source::PriorArt),
            ("traces[].channels[].scale", Source::PriorArt),
            ("traces[].channels[].unit", Source::Inferred),
            ("traces[].sweep_count", Source::Inferred),
            ("traces[].start_s", Source::Inferred),
            ("tables[spikes].columns", Source::PriorArt),
            ("tables[events].columns", Source::PriorArt),
            ("traces[].extra.recorded_at", Source::PriorArt),
            ("tables[].extra.recorded_at", Source::PriorArt),
            ("traces[].extra.application", Source::PriorArt),
            ("tables[].extra.application", Source::PriorArt),
        ] {
            p.insert(k.into(), s);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let p = &self.plx;
        let mut out = vec![LsEntry {
            kind: "header".into(),
            name: "PLX file header".into(),
            offset: Some(0),
            size: Some(crate::plx::FILE_HEADER_LEN),
            image: None,
            details: json!({"version": p.header.version}),
        }];
        out.push(LsEntry {
            kind: "header".into(),
            name: "channel headers".into(),
            offset: Some(crate::plx::FILE_HEADER_LEN),
            size: Some(p.data_start - crate::plx::FILE_HEADER_LEN),
            image: None,
            details: json!({"spike": p.spike_channels.len(), "event": p.event_channels.len(), "continuous": p.continuous_channels.len()}),
        });
        out.push(LsEntry {
            kind: "records".into(),
            name: "data blocks".into(),
            offset: Some(p.data_start),
            size: Some(p.index.data_end - p.data_start),
            image: None,
            details: json!({"blocks_by_type": p.index.block_counts}),
        });
        Ok(out)
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(not_images())
    }

    fn read_trace(
        &mut self,
        index: u32,
        sweep: u32,
        first_sample: u64,
        max_samples: u64,
    ) -> Result<Trace> {
        let t = self.traces.get(index as usize).cloned().ok_or_else(|| {
            Error::Usage(format!(
                "trace {index} out of range (file has {} traces)",
                self.traces.len()
            ))
        })?;
        let run = *t.runs.get(sweep as usize).ok_or_else(|| {
            Error::Usage(format!(
                "sweep {sweep} out of range (trace {index} has {} sweeps)",
                t.runs.len()
            ))
        })?;
        if first_sample > run.samples {
            return Err(Error::Usage(format!(
                "first sample {first_sample} is past the end of sweep {sweep} ({} samples)",
                run.samples
            )));
        }
        let n = max_samples.min(run.samples - first_sample).min(1 << 24);
        let (path, len) = (self.path.clone(), self.plx.file_len);
        let mut out = Vec::with_capacity(t.channels.len());
        for &ci in &t.channels {
            let c = self.plx.continuous_channels[ci].clone();
            let scale = self.plx.header.continuous_scale(&c);
            let blocks: Vec<SampleBlock> =
                self.blocks(&c)[run.first_block..run.first_block + run.block_count].to_vec();
            let mut v = Vec::with_capacity(n as usize);
            let mut skip = first_sample;
            for b in blocks {
                if v.len() as u64 >= n {
                    break;
                }
                let bs = u64::from(b.samples);
                if skip >= bs {
                    skip -= bs;
                    continue;
                }
                let take = (bs - skip).min(n - v.len() as u64);
                let at = b.offset + 2 * skip;
                let blk = read_block(self.handle()?, &path, at, 2 * take, len)?;
                if (blk.len() as u64) < 2 * take {
                    return Err(Error::corrupt_at(FORMAT_ID, at, "continuous block cut off"));
                }
                for k in 0..take {
                    let raw = blk
                        .i16_at(at + 2 * k)
                        .ok_or_else(|| Error::corrupt_at(FORMAT_ID, at, "sample cut off"))?;
                    v.push(f64::from(raw) * scale);
                }
                skip = 0;
            }
            out.push(v);
        }
        Ok(Trace {
            trace: index,
            sweep,
            first_sample,
            channels: out,
        })
    }

    fn read_table(&mut self, index: u32, first_row: u64, max_rows: u64) -> Result<Table> {
        let (s, e) = self.table_ids();
        let total = if Some(index) == s {
            self.plx.index.spikes.len() as u64
        } else if Some(index) == e {
            self.plx.index.events.len() as u64
        } else {
            return Err(Error::Usage(format!(
                "table {index} out of range (file has {} tables)",
                u32::from(s.is_some()) + u32::from(e.is_some())
            )));
        };
        if first_row > total {
            return Err(Error::Usage(format!(
                "first row {first_row} is past the last row ({total} rows)"
            )));
        }
        let n = max_rows.min(total - first_row).min(MAX_TABLE_READ);
        if Some(index) == s {
            let mut t = self.read_spikes(first_row, n)?;
            t.table = index;
            return Ok(t);
        }
        let clock = self.clock();
        let mut cols: Vec<Vec<f64>> = (0..4).map(|_| Vec::with_capacity(n as usize)).collect();
        for ev in &self.plx.index.events[first_row as usize..(first_row + n) as usize] {
            cols[0].push(ev.timestamp as f64 / clock);
            cols[1].push(ev.timestamp as f64);
            cols[2].push(f64::from(ev.channel));
            cols[3].push(f64::from(ev.value));
        }
        Ok(Table {
            table: index,
            first_row,
            columns: cols,
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), FORMAT_ID);
        r.performed("PLEX signature; file header and channel headers inside the file");
        r.performed("every data block: known type (spike, event, continuous) and its samples inside the file (truncation)");
        r.performed("continuous blocks: timestamps per channel never go backwards; gaps reported");
        for f in &self.plx.index.findings {
            r.push(f.clone());
        }
        let h = &self.plx.header;
        if h.clock_hz <= 0 {
            r.push(Finding::error(
                "bad_clock",
                format!("timestamp clock {} Hz", h.clock_hz),
            ));
        }
        if !(100..=107).contains(&h.version) {
            r.push(Finding::warning(
                "unknown_version",
                format!(
                    "PLX version {} (versions 100–106 are documented by prior art)",
                    h.version
                ),
            ));
        }
        for c in &self.plx.continuous_channels {
            let blocks = self.blocks(c);
            if blocks.is_empty() {
                continue;
            }
            if c.rate_hz <= 0 {
                r.push(Finding::error(
                    "bad_rate",
                    format!("continuous channel {} has rate {} Hz", c.name, c.rate_hz),
                ));
            }
            let back = blocks
                .windows(2)
                .filter(|w| w[1].timestamp < w[0].timestamp)
                .count();
            if back > 0 {
                r.push(Finding::warning(
                    "timestamps_decrease",
                    format!(
                        "continuous channel {}: {back} block timestamps go backwards",
                        c.name
                    ),
                ));
            }
        }
        for t in &self.traces {
            if t.runs.len() > 1 {
                r.push(Finding::info(
                    "segments",
                    format!(
                        "trace of {} channel(s) at {} Hz: {} gap-free sweeps (recording paused)",
                        t.channels.len(),
                        t.rate_hz,
                        t.runs.len()
                    ),
                ));
            }
        }
        if !self.orphans.is_empty() {
            r.push(Finding::warning(
                "unlisted_channel",
                format!(
                    "continuous blocks for channel number(s) {:?} that have no channel header (not read)",
                    self.orphans
                ),
            ));
        }
        let known: Vec<i32> = self.plx.spike_channels.iter().map(|c| c.channel).collect();
        let (path, len) = (self.path.clone(), self.plx.file_len);
        let mut unknown = 0u64;
        let offsets = self.plx.index.spikes.clone();
        let mut buf = openreadout_core::bytes::Block::default();
        for at in offsets {
            if at < buf.origin || at + BLOCK_HEADER_LEN > buf.end() {
                buf = read_block(self.handle()?, &path, at, 4 << 20, len)?;
            }
            if let Some((_, _, ch, ..)) = block_header(&buf, at)
                && !known.contains(&i32::from(ch))
            {
                unknown += 1;
            }
        }
        if unknown > 0 {
            r.push(Finding::warning(
                "unlisted_spike_channel",
                format!("{unknown} spike blocks name a channel without a header (waveforms left in counts)"),
            ));
        }
        Ok(r)
    }
}
