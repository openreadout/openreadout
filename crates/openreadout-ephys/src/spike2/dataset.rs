//! `Dataset` for a Spike2 `.smr` or `.smrx`: every waveform channel is a trace (pauses split sweeps);
//! events, markers and text marks form the `events` table, AdcMark/RealMark waveforms the
//! `spikes` table.

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

use super::file::{ChannelKind, SmrChannel, SmrFile, item_text, parse_smr, segments};
use super::file64::parse_smrx;
use super::{SPIKE2_FORMAT_ID, Spike2Reader};

/// Samples per channel decoded per `read_trace` call at most.
pub const MAX_SMR_READ: u64 = 1 << 26;
/// Rows returned per `read_table` call at most.
pub const MAX_SMR_TABLE_READ: u64 = 1 << 22;

/// One waveform trace: a channel and its sweeps (first block, block count, samples).
#[derive(Debug, Clone)]
pub struct SmrTrace {
    /// Index into the file's channels.
    pub channel: usize,
    /// Sweeps: (first block, block count, samples).
    pub sweeps: Vec<(usize, usize, u64)>,
}

/// An opened `.smr`.
#[derive(Debug)]
pub struct Spike2Dataset {
    path: PathBuf,
    fs: Fs,
    handle: Option<SourceFile>,
    file: SmrFile,
    traces: Vec<SmrTrace>,
    /// Channel indices of the events table, spikes table.
    event_channels: Vec<usize>,
    spike_channels: Vec<usize>,
}

impl Spike2Dataset {
    /// Open a local file.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    /// Open an [`Input`].
    pub fn open_input(input: &Input) -> Result<Self> {
        let path = input.path().to_path_buf();
        let fs = input.fs().clone();
        let mut f = fs.open(&path).map_err(|e| Error::io(&path, e))?;
        let file_len = f.size().map_err(|e| Error::io(&path, e))?;
        let head = read_block(&mut f, &path, 0, 16, file_len)?;
        let file = if head.slice(0, 3) == Some(b"S64") {
            parse_smrx(&mut f, &path, file_len)?
        } else {
            parse_smr(&mut f, &path, file_len)?
        };
        let traces = file
            .channels
            .iter()
            .enumerate()
            .filter(|(_, c)| c.kind.is_waveform())
            .filter_map(|(i, c)| {
                let sweeps = segments(c);
                (!sweeps.is_empty() && c.interval_ticks.is_some())
                    .then_some(SmrTrace { channel: i, sweeps })
            })
            .collect();
        let event_channels = (0..file.channels.len())
            .filter(|&i| file.channels[i].kind.is_events())
            .collect();
        let spike_channels = (0..file.channels.len())
            .filter(|&i| file.channels[i].kind.is_spikes())
            .collect();
        Ok(Spike2Dataset {
            path,
            fs,
            handle: Some(f),
            file,
            traces,
            event_channels,
            spike_channels,
        })
    }

    /// The parsed file.
    pub fn smr(&self) -> &SmrFile {
        &self.file
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

    fn table_ids(&self) -> (Option<u32>, Option<u32>) {
        let e = (!self.event_channels.is_empty()).then_some(0);
        let s = (!self.spike_channels.is_empty()).then_some(u32::from(e.is_some()));
        (e, s)
    }

    fn rows(&self, chans: &[usize]) -> u64 {
        chans
            .iter()
            .map(|&i| self.file.channels[i].item_count())
            .sum()
    }

    fn max_wave(&self) -> u64 {
        self.spike_channels
            .iter()
            .map(|&i| self.file.channels[i].wave_points())
            .max()
            .unwrap_or(0)
    }
}

fn finite(v: f64) -> Value {
    if v.is_finite() { json!(v) } else { Value::Null }
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

fn channel_json(c: &SmrChannel) -> Value {
    json!({
        "channel_number": c.number,
        "kind": c.kind.name(),
        "title": c.title,
        "comment": c.comment,
        "physical_channel": c.physical,
        "ideal_rate_hz": finite(c.ideal_rate),
        "unit": c.unit,
        "items": c.item_count(),
        "blocks": c.blocks.len(),
    })
}

impl Spike2Dataset {
    fn trace_info(&self, index: usize, t: &SmrTrace) -> TraceInfo {
        let f = &self.file;
        let c = &f.channels[t.channel];
        let interval_s = c.interval_ticks.map_or(f64::NAN, |v| v as f64 * f.tick_s);
        let mut extra = BTreeMap::new();
        extra.insert("channel_number".into(), json!(c.number));
        extra.insert("kind".into(), json!(c.kind.name()));
        if c.physical >= 0 {
            extra.insert("physical_channel".into(), json!(c.physical));
        }
        if !c.comment.is_empty() {
            extra.insert("comment".into(), json!(c.comment));
        }
        extra.insert("ideal_rate_hz".into(), finite(c.ideal_rate));
        extra.insert(
            "sample_interval_ticks".into(),
            json!(c.interval_ticks.unwrap_or(0)),
        );
        extra.insert("tick_s".into(), json!(f.tick_s));
        let starts: Vec<f64> = t
            .sweeps
            .iter()
            .map(|&(b, _, _)| c.blocks[b].start as f64 * f.tick_s)
            .collect();
        if t.sweeps.len() > 1 {
            extra.insert("sweep_starts_s".into(), json!(starts));
            let counts: Vec<u64> = t.sweeps.iter().map(|s| s.2).collect();
            if counts.windows(2).any(|w| w[0] != w[1]) {
                extra.insert("sweep_sample_counts".into(), json!(counts));
            }
        }
        if index == 0 {
            if let Some(d) = &f.recorded_at {
                extra.insert("recorded_at".into(), json!(d));
            }
            extra.insert("application".into(), json!("Spike2"));
            if !f.creator.is_empty() {
                extra.insert("application_version".into(), json!(f.creator));
            }
            match f.son64 {
                Some(v) => {
                    extra.insert("son64_version".into(), json!(format!("{}.{}", v[0], v[1])));
                }
                None => {
                    extra.insert("file_version".into(), json!(f.system_id));
                }
            }
        }
        let (dtype, scale, offset) = match c.kind {
            ChannelKind::Adc => ("int16", c.gain(), c.offset),
            _ => ("float32", 1.0, 0.0),
        };
        let mut cextra = BTreeMap::new();
        cextra.insert("channel_number".into(), json!(c.number));
        TraceInfo {
            index: index as u32,
            name: Some(if c.title.is_empty() {
                format!("ch{}", c.number)
            } else {
                c.title.clone()
            }),
            sample_rate_hz: 1.0 / interval_s,
            sample_count: t.sweeps.iter().map(|s| s.2).max().unwrap_or(0),
            sweep_count: t.sweeps.len() as u32,
            channels: vec![SignalChannelInfo {
                index: 0,
                name: if c.title.is_empty() {
                    format!("ch{}", c.number)
                } else {
                    c.title.clone()
                },
                unit: (!c.unit.is_empty()).then(|| c.unit.clone()),
                dtype: dtype.into(),
                scale,
                offset,
                extra: cextra,
            }],
            start_s: starts.first().copied(),
            extra,
        }
    }

    fn tables(&self) -> Vec<TableInfo> {
        let f = &self.file;
        let (events_at, spikes_at) = self.table_ids();
        let tick_dtype = if f.son64.is_some() { "int64" } else { "int32" };
        let mut out = Vec::new();
        if let Some(i) = events_at {
            let mut extra = BTreeMap::new();
            extra.insert(
                "channels".into(),
                json!(
                    self.event_channels
                        .iter()
                        .map(|&k| {
                            let c = &f.channels[k];
                            let mut v = channel_json(c);
                            if let Some(low) = c.initially_low {
                                v["initially_low"] = json!(low);
                            }
                            v
                        })
                        .collect::<Vec<_>>()
                ),
            );
            if !f.texts.is_empty() {
                extra.insert("texts".into(), json!(f.texts));
            }
            extra.insert("tick_s".into(), json!(f.tick_s));
            if self.traces.is_empty() {
                if let Some(d) = &f.recorded_at {
                    extra.insert("recorded_at".into(), json!(d));
                }
                extra.insert("application".into(), json!("Spike2"));
            }
            out.push(TableInfo {
                index: i,
                name: Some("events".into()),
                row_count: self.rows(&self.event_channels),
                columns: vec![
                    column(0, "time_s", "float64", Some("s")),
                    column(1, "tick", tick_dtype, None),
                    column(2, "channel", "uint16", None),
                    column(3, "code0", "uint8", None),
                    column(4, "code1", "uint8", None),
                    column(5, "code2", "uint8", None),
                    column(6, "code3", "uint8", None),
                    column(7, "text", "int32", None),
                ],
                extra,
            });
        }
        if let Some(i) = spikes_at {
            let unit = self
                .spike_channels
                .first()
                .map(|&k| f.channels[k].unit.clone())
                .filter(|u| !u.is_empty());
            let mut cols = vec![
                column(0, "time_s", "float64", Some("s")),
                column(1, "tick", tick_dtype, None),
                column(2, "channel", "uint16", None),
                column(3, "unit", "uint8", None),
                column(4, "code1", "uint8", None),
                column(5, "code2", "uint8", None),
                column(6, "code3", "uint8", None),
            ];
            for k in 0..self.max_wave() {
                cols.push(column(
                    cols.len(),
                    &format!("w{k}"),
                    "float64",
                    unit.as_deref(),
                ));
            }
            let mut extra = BTreeMap::new();
            extra.insert(
                "channels".into(),
                json!(
                    self.spike_channels
                        .iter()
                        .map(|&k| {
                            let c = &f.channels[k];
                            let mut v = channel_json(c);
                            v["waveform_points"] = json!(c.wave_points());
                            v["pre_trigger_points"] = json!(c.pre_trigger);
                            v["interleave"] = json!(c.interleave);
                            v["waveform_rate_hz"] = c
                                .interval_ticks
                                .map_or(Value::Null, |t| json!(1.0 / (t as f64 * f.tick_s)));
                            if c.kind == ChannelKind::AdcMark {
                                v["scale"] = json!(c.gain());
                                v["offset"] = finite(c.offset);
                            }
                            v
                        })
                        .collect::<Vec<_>>()
                ),
            );
            extra.insert("tick_s".into(), json!(f.tick_s));
            out.push(TableInfo {
                index: i,
                name: Some("spikes".into()),
                row_count: self.rows(&self.spike_channels),
                columns: cols,
                extra,
            });
        }
        out
    }

    /// Rows `first..first+n` of the table over `chans` (channel order, file order).
    fn read_rows(
        &mut self,
        chans: &[usize],
        first: u64,
        n: u64,
        spikes: bool,
    ) -> Result<Vec<Vec<f64>>> {
        let width = if spikes {
            7 + self.max_wave() as usize
        } else {
            8
        };
        let mut cols: Vec<Vec<f64>> = vec![Vec::with_capacity(n as usize); width];
        let tick_s = self.file.tick_s;
        let file_len = self.file.file_len;
        let path = self.path.clone();
        let mut skip = first;
        let mut left = n;
        for &ci in chans {
            if left == 0 {
                break;
            }
            let c = self.file.channels[ci].clone();
            let item = c.item_len();
            for b in &c.blocks {
                if left == 0 {
                    break;
                }
                if skip >= b.items {
                    skip -= b.items;
                    continue;
                }
                let take = (b.items - skip).min(left);
                let at = b.offset + skip * item;
                let block = read_block(self.handle()?, &path, at, take * item, file_len)?;
                for k in 0..take {
                    let o = at + k * item;
                    let tick = if c.wide_times {
                        block.i64_at(o).unwrap_or(0) as f64
                    } else {
                        f64::from(block.i32_at(o).unwrap_or(0))
                    };
                    let (tl, hl) = (c.time_len(), c.head_len());
                    cols[0].push(tick * tick_s);
                    cols[1].push(tick);
                    cols[2].push(f64::from(c.number));
                    let has_codes = !matches!(
                        c.kind,
                        ChannelKind::EventFall | ChannelKind::EventRise | ChannelKind::EventBoth
                    );
                    for j in 0..4u64 {
                        cols[3 + j as usize].push(if has_codes {
                            block.u8_at(o + tl + j).map_or(f64::NAN, f64::from)
                        } else {
                            f64::NAN
                        });
                    }
                    if spikes {
                        let pts = c.wave_points();
                        for w in 0..(width as u64 - 7) {
                            let v = if w >= pts {
                                f64::NAN
                            } else if c.kind == ChannelKind::AdcMark {
                                block
                                    .i16_at(o + hl + 2 * w)
                                    .map_or(f64::NAN, |x| f64::from(x) * c.gain() + c.offset)
                            } else {
                                block.f32_at(o + hl + 4 * w).map_or(f64::NAN, f64::from)
                            };
                            cols[7 + w as usize].push(v);
                        }
                    } else {
                        let text = if c.kind == ChannelKind::TextMark {
                            block
                                .slice(o + hl, usize::from(c.extra_bytes))
                                .map(item_text)
                                .and_then(|t| self.file.texts.iter().position(|x| *x == t))
                                .map_or(f64::NAN, |p| p as f64)
                        } else {
                            f64::NAN
                        };
                        cols[7].push(text);
                    }
                }
                left -= take;
                skip = 0;
            }
        }
        Ok(cols)
    }
}

impl Dataset for Spike2Dataset {
    fn info(&self) -> Result<FileInfo> {
        let f = &self.file;
        let version = match f.son64 {
            Some(_) => "64-bit Spike2 file (.smrx)".to_string(),
            None => format!("Spike2 file version {}", f.system_id),
        };
        let mut notes = vec![format!(
            "{version}: every waveform channel is a trace (pauses split sweeps); events, markers and text marks are the `events` table, spike shapes (AdcMark/RealMark) the `spikes` table; times count a {:e} s tick",
            f.tick_s
        )];
        let empty: Vec<String> = f
            .channels
            .iter()
            .filter(|c| c.kind.is_waveform() && c.item_count() == 0)
            .map(|c| c.number.to_string())
            .collect();
        if !empty.is_empty() {
            notes.push(format!(
                "waveform channel(s) {} hold no samples and are not listed",
                empty.join(", ")
            ));
        }
        if f.findings
            .iter()
            .any(|x| x.severity == openreadout_core::model::Severity::Error)
        {
            notes.push("structural problems found (e.g. truncation); run `check`".into());
        }
        let mut traces: Vec<TraceInfo> = self
            .traces
            .iter()
            .enumerate()
            .map(|(i, t)| self.trace_info(i, t))
            .collect();
        if traces.is_empty() && self.event_channels.is_empty() && self.spike_channels.is_empty() {
            notes.push("the file holds no channels with data".into());
        }
        let tables = self.tables();
        if let Some(t) = traces.first_mut()
            && !f.comments.is_empty()
        {
            t.extra.insert("file_comments".into(), json!(f.comments));
        }
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: f.file_len,
            format: Spike2Reader.descriptor(),
            format_version: Some(match f.son64 {
                Some(v) => format!("smrx {}.{}", v[0], v[1]),
                None => f.system_id.to_string(),
            }),
            images: Vec::new(),
            tables,
            spectra: Vec::new(),
            traces,
            plane_count: 0,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let f = &self.file;
        Ok(json!({
            "file_version": f.system_id,
            "son64_version": f.son64.map(|v| format!("{}.{}", v[0], v[1])),
            "creator": f.creator,
            "us_per_time": f.us_per_time,
            "time_per_adc": f.time_per_adc,
            "time_base_s": f.time_base_s,
            "tick_s": f.tick_s,
            "max_time_ticks": f.max_time,
            "recorded_at": f.recorded_at,
            "comments": f.comments,
            "channel_slots": f.channel_slots,
            "channels": f.channels.iter().map(|c| {
                let mut v = channel_json(c);
                v["extra_bytes"] = json!(c.extra_bytes);
                v["pre_trigger_points"] = json!(c.pre_trigger);
                v["scale"] = finite(c.scale);
                v["offset"] = finite(c.offset);
                v["interleave"] = json!(c.interleave);
                v["sample_interval_ticks"] = json!(c.interval_ticks);
                v["header_blocks"] = json!(c.header_blocks);
                v["first_times"] = json!(c.blocks.first().map(|b| b.start));
                v["last_times"] = json!(c.blocks.last().map(|b| b.end));
                v
            }).collect::<Vec<_>>(),
        }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for (k, s) in [
            ("format_version", Source::PriorArt),
            ("traces[].sample_rate_hz", Source::PriorArt),
            ("traces[].sample_count", Source::PriorArt),
            ("traces[].sweep_count", Source::PriorArt),
            ("traces[].channels[].name", Source::PriorArt),
            ("traces[].channels[].unit", Source::PriorArt),
            ("traces[].channels[].scale", Source::PriorArt),
            ("traces[].channels[].offset", Source::PriorArt),
            ("traces[].start_s", Source::PriorArt),
            ("traces[].extra.recorded_at", Source::Inferred),
            ("traces[].extra.sweep_starts_s", Source::PriorArt),
            ("traces[].extra.application_version", Source::Inferred),
            ("tables[].columns", Source::PriorArt),
            ("tables[].extra.channels", Source::PriorArt),
            ("tables[].extra.texts", Source::Inferred),
        ] {
            p.insert(k.into(), s);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let mut out = vec![LsEntry {
            kind: "header".into(),
            name: "file header".into(),
            offset: Some(0),
            size: Some(if self.file.son64.is_some() {
                65536
            } else {
                512
            }),
            image: None,
            details: json!({"file_version": self.file.system_id, "son64_version": self.file.son64.map(|v| format!("{}.{}", v[0], v[1]))}),
        }];
        for c in &self.file.channels {
            out.push(LsEntry {
                kind: "channel".into(),
                name: format!("channel {} ({}) {}", c.number, c.kind.name(), c.title),
                offset: c.blocks.first().map(|b| b.offset),
                size: Some(c.item_count() * c.item_len()),
                image: None,
                details: json!({"blocks": c.blocks.len(), "items": c.item_count()}),
            });
        }
        Ok(out)
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::unsupported(
            SPIKE2_FORMAT_ID,
            "image planes",
            "Spike2 files hold sampled signals and events, not images: use `openreadout trace` or `openreadout table`.",
        ))
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
                "trace index {index} out of range (the file has {} traces)",
                self.traces.len()
            ))
        })?;
        let &(b0, nb, count) = t.sweeps.get(sweep as usize).ok_or_else(|| {
            Error::Usage(format!(
                "sweep {sweep} out of range (trace {index} has {} sweeps)",
                t.sweeps.len()
            ))
        })?;
        if first_sample > count {
            return Err(Error::Usage(format!(
                "first sample {first_sample} is past the end of sweep {sweep} ({count} samples)"
            )));
        }
        let c = self.file.channels[t.channel].clone();
        let n = max_samples.min(count - first_sample).min(MAX_SMR_READ);
        let width = c.item_len();
        let gain = c.gain();
        let offset = c.offset;
        let file_len = self.file.file_len;
        let path = self.path.clone();
        let mut col = Vec::with_capacity(n as usize);
        let mut skip = first_sample;
        let mut left = n;
        for b in &c.blocks[b0..b0 + nb] {
            if left == 0 {
                break;
            }
            if skip >= b.items {
                skip -= b.items;
                continue;
            }
            let take = (b.items - skip).min(left);
            let at = b.offset + skip * width;
            let block = read_block(self.handle()?, &path, at, take * width, file_len)?;
            if block.len() as u64 != take * width {
                return Err(Error::corrupt_at(
                    SPIKE2_FORMAT_ID,
                    at,
                    "samples cut off by the end of the file",
                ));
            }
            match c.kind {
                ChannelKind::Adc => col.extend(
                    block
                        .bytes
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|x| f64::from(i16::from_le_bytes(*x)) * gain + offset),
                ),
                _ => col.extend(
                    block
                        .bytes
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|x| f64::from(f32::from_le_bytes(*x))),
                ),
            }
            left -= take;
            skip = 0;
        }
        Ok(Trace {
            trace: index,
            sweep,
            first_sample,
            channels: vec![col],
        })
    }

    fn read_table(&mut self, index: u32, first_row: u64, max_rows: u64) -> Result<Table> {
        let (e, s) = self.table_ids();
        let (chans, spikes) = if Some(index) == e {
            (self.event_channels.clone(), false)
        } else if Some(index) == s {
            (self.spike_channels.clone(), true)
        } else {
            return Err(Error::Usage(format!(
                "table {index} out of range (file has {} tables)",
                u32::from(e.is_some()) + u32::from(s.is_some())
            )));
        };
        let total = self.rows(&chans);
        if first_row > total {
            return Err(Error::Usage(format!(
                "first row {first_row} is past the last row ({total} rows)"
            )));
        }
        let n = max_rows.min(total - first_row).min(MAX_SMR_TABLE_READ);
        Ok(Table {
            table: index,
            first_row,
            columns: self.read_rows(&chans, first_row, n, spikes)?,
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let f = &self.file;
        let mut r = CheckReport::new(self.path.display().to_string(), SPIKE2_FORMAT_ID);
        if self.file.son64.is_some() {
            r.performed("signature `S64`, usable clock, header blocks, channel table and string table inside the header");
            r.performed("every channel's index tree: index and data blocks inside the file, runs and items inside their blocks, live block count agrees with the channel record");
        } else {
            r.performed("signature `(C) CED 87`, file version 1–9, usable clock");
            r.performed("every channel header inside the file; every block of each channel's chain inside the file; chain length and last block agree with the header");
        }
        r.performed("waveform channels: positive sample interval; blocks never start before the previous block's last sample (overlaps reported)");
        for x in &f.findings {
            r.push(x.clone());
        }
        for c in f.channels.iter().filter(|c| c.kind.is_waveform()) {
            let interval = c.interval_ticks.map_or(0, |v| v as i64);
            let overlaps = c
                .blocks
                .windows(2)
                .filter(|w| w[1].items > 0 && w[1].start - w[0].end < interval)
                .count();
            if overlaps > 0 {
                r.push(Finding::warning(
                    "overlapping_blocks",
                    format!(
                        "channel {}: {overlaps} block(s) start less than one sample interval after the previous block",
                        c.number
                    ),
                ));
            }
            let pauses = segments(c).len().saturating_sub(1);
            if pauses > 0 {
                r.push(Finding::info(
                    "pauses",
                    format!(
                        "channel {}: {pauses} pause(s) split the recording into sweeps",
                        c.number
                    ),
                ));
            }
        }
        Ok(r)
    }
}
