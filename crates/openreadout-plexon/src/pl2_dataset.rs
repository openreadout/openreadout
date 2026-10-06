//! `Pl2Dataset`: analog channels as traces (channels on one sample grid share a trace, gap-free
//! runs of records are sweeps), spike waveforms and digital events as two tables.

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

use crate::dataset::MAX_TABLE_READ;
use crate::pl2::{PL2_FILE_HEADER_LEN, Pl2Channel, Pl2File, REC_END, RECORD_HEADER_LEN, parse_pl2};
use crate::plx::{Run, SampleBlock, runs};
use crate::{FORMAT_ID, PlexonReader};

/// One trace: analog channels (indices into the header list) on one sample grid.
#[derive(Debug, Clone)]
pub struct Pl2Trace {
    /// Analog-channel header indices, in header order.
    pub channels: Vec<usize>,
    /// Sampling rate, Hz.
    pub rate_hz: f64,
    /// Gap-free runs (sweeps), identical for every channel of the trace.
    pub runs: Vec<Run>,
}

/// An opened PL2 file.
#[derive(Debug)]
pub struct Pl2Dataset {
    path: PathBuf,
    fs: Fs,
    pl2: Pl2File,
    traces: Vec<Pl2Trace>,
    handle: Option<File>,
}

fn unit_name(units: &str) -> Option<String> {
    match units.trim() {
        "" => None,
        "Volts" | "volts" => Some("V".into()),
        u => Some(u.to_string()),
    }
}

impl Pl2Dataset {
    /// Open and index a PL2 file.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    pub(crate) fn open_input(input: &Input) -> Result<Self> {
        let path = input.path();
        let fs = input.fs();
        let mut f = fs.open(path).map_err(|e| Error::io(path, e))?;
        let len = f.metadata().map_err(|e| Error::io(path, e))?.len();
        let pl2 = parse_pl2(&mut f, path, len)?;
        let clock = pl2.header.clock_hz;
        let mut traces: Vec<Pl2Trace> = Vec::new();
        for (i, c) in pl2.analog_channels.iter().enumerate() {
            let Some(blocks) = Self::blocks_of(&pl2, c) else {
                continue;
            };
            let r = runs(blocks.iter().copied(), clock, c.rate_hz);
            let grid: Vec<(u64, u64)> = r.iter().map(|x| (x.timestamp, x.samples)).collect();
            match traces.iter_mut().find(|t| {
                t.rate_hz.to_bits() == c.rate_hz.to_bits()
                    && t.runs
                        .iter()
                        .map(|x| (x.timestamp, x.samples))
                        .eq(grid.iter().copied())
            }) {
                Some(t) => t.channels.push(i),
                None => traces.push(Pl2Trace {
                    channels: vec![i],
                    rate_hz: c.rate_hz,
                    runs: r,
                }),
            }
        }
        Ok(Pl2Dataset {
            path: path.to_path_buf(),
            fs: fs.clone(),
            pl2,
            traces,
            handle: Some(f),
        })
    }

    fn blocks_of<'a>(pl2: &'a Pl2File, c: &Pl2Channel) -> Option<&'a Vec<SampleBlock>> {
        let ch = u16::try_from(c.channel).ok()?;
        pl2.index
            .analog
            .get(&(c.source, ch))
            .filter(|v| !v.is_empty())
    }

    /// The parsed file.
    pub fn pl2(&self) -> &Pl2File {
        &self.pl2
    }

    fn clock(&self) -> f64 {
        if self.pl2.header.clock_hz > 0.0 {
            self.pl2.header.clock_hz
        } else {
            1.0
        }
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

    fn spike_scale(&self, source: u8, channel: u16) -> f64 {
        self.pl2
            .spike_channels
            .iter()
            .find(|c| c.source == source && c.channel == u32::from(channel))
            .map_or(1.0, |c| c.units_per_count)
    }

    fn trace_info(&self, index: usize, t: &Pl2Trace) -> TraceInfo {
        let clock = self.clock();
        let channels: Vec<SignalChannelInfo> = t
            .channels
            .iter()
            .enumerate()
            .map(|(k, &i)| {
                let c = &self.pl2.analog_channels[i];
                let mut e = BTreeMap::new();
                e.insert("source".into(), json!(c.source));
                e.insert("channel_number".into(), json!(c.channel));
                if !c.device.is_empty() {
                    e.insert("device".into(), json!(c.device));
                }
                SignalChannelInfo {
                    index: k as u32,
                    name: c.name.clone(),
                    unit: unit_name(&c.units),
                    dtype: "int16".into(),
                    scale: c.units_per_count,
                    offset: 0.0,
                    extra: e,
                }
            })
            .collect();
        let prefixes: Vec<&str> = t
            .channels
            .iter()
            .map(|&i| {
                self.pl2.analog_channels[i]
                    .name
                    .trim_end_matches(|c: char| c.is_ascii_digit())
            })
            .collect();
        let name = prefixes
            .first()
            .filter(|p| !p.is_empty() && prefixes.iter().all(|x| x == *p))
            .map(|p| (*p).to_string());
        let counts: Vec<u64> = t.runs.iter().map(|r| r.samples).collect();
        let mut extra = BTreeMap::new();
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
        extra.insert("timestamp_clock_hz".into(), json!(self.pl2.header.clock_hz));
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

    /// (spike table index, event table index).
    fn table_ids(&self) -> (Option<u32>, Option<u32>) {
        let spikes =
            (!self.pl2.spike_channels.is_empty() || !self.pl2.index.spikes.is_empty()).then_some(0);
        let events = (!self.pl2.digital_channels.is_empty() || !self.pl2.index.events.is_empty())
            .then_some(u32::from(spikes.is_some()));
        (spikes, events)
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

    fn tables(&self) -> Vec<TableInfo> {
        let (s, e) = self.table_ids();
        let mut out = Vec::new();
        let clock = self.pl2.header.clock_hz;
        if let Some(i) = s {
            let unit = self
                .pl2
                .spike_channels
                .first()
                .and_then(|c| unit_name(&c.units));
            let mut cols = vec![
                Self::column(0, "time_s", "float64", Some("s")),
                Self::column(1, "timestamp", "uint64", None),
                Self::column(2, "source", "uint8", None),
                Self::column(3, "channel", "uint16", None),
                Self::column(4, "unit", "uint16", None),
            ];
            for w in 0..usize::from(self.pl2.index.max_waveform) {
                cols.push(Self::column(
                    cols.len(),
                    &format!("w{w}"),
                    "float64",
                    unit.as_deref(),
                ));
            }
            let mut extra = BTreeMap::new();
            extra.insert("timestamp_clock_hz".into(), json!(clock));
            extra.insert(
                "channels".into(),
                json!(self.pl2.spike_channels.iter().filter(|c| c.recording).map(|c| json!({
                    "name": c.name, "source": c.source, "channel_number": c.channel,
                    "rate_hz": c.rate_hz, "units_per_count": c.units_per_count,
                    "waveform_samples": c.waveform_samples, "pre_threshold": c.pre_threshold,
                    "threshold": c.threshold, "unit_counts": c.unit_counts,
                })).collect::<Vec<_>>()),
            );
            out.push(TableInfo {
                index: i,
                name: Some("spikes".into()),
                row_count: self.pl2.index.spike_rows,
                columns: cols,
                extra,
            });
        }
        if let Some(i) = e {
            let mut extra = BTreeMap::new();
            extra.insert("timestamp_clock_hz".into(), json!(clock));
            extra.insert(
                "channels".into(),
                json!(
                    self.pl2
                        .digital_channels
                        .iter()
                        .filter(|c| c.recording)
                        .map(|c| json!({
                            "name": c.name, "source": c.source, "channel_number": c.channel,
                        }))
                        .collect::<Vec<_>>()
                ),
            );
            out.push(TableInfo {
                index: i,
                name: Some("events".into()),
                row_count: self.pl2.index.event_rows,
                columns: vec![
                    Self::column(0, "time_s", "float64", Some("s")),
                    Self::column(1, "timestamp", "uint64", None),
                    Self::column(2, "source", "uint8", None),
                    Self::column(3, "channel", "uint16", None),
                    Self::column(4, "value", "uint16", None),
                ],
                extra,
            });
        }
        out
    }

    fn read_spikes(&mut self, first: u64, n: u64) -> Result<Table> {
        let width = usize::from(self.pl2.index.max_waveform);
        let mut cols: Vec<Vec<f64>> = (0..5 + width)
            .map(|_| Vec::with_capacity(n as usize))
            .collect();
        let clock = self.clock();
        let (path, len) = (self.path.clone(), self.pl2.file_len);
        let end = first + n;
        let runs: Vec<_> = self
            .pl2
            .index
            .spikes
            .iter()
            .filter(|r| r.first_row + u64::from(r.count) > first && r.first_row < end)
            .copied()
            .collect();
        for r in runs {
            let scale = self.spike_scale(r.source, r.channel);
            let cnt = u64::from(r.count);
            let wf = u64::from(r.waveform_samples);
            let payload = cnt * (10 + 2 * wf);
            let base = r.offset + RECORD_HEADER_LEN;
            let b = read_block(self.handle()?, &path, base, payload, len)?;
            if (b.len() as u64) < payload {
                return Err(Error::corrupt_at(
                    FORMAT_ID,
                    r.offset,
                    "spike record cut off",
                ));
            }
            let lo = first.saturating_sub(r.first_row);
            let hi = (end - r.first_row).min(cnt);
            for k in lo..hi {
                let ts = b
                    .u64_at(base + 8 * k)
                    .ok_or_else(|| Error::corrupt_at(FORMAT_ID, base, "spike cut off"))?;
                let unit = b.u16_at(base + 8 * cnt + 2 * k).unwrap_or(0);
                cols[0].push(ts as f64 / clock);
                cols[1].push(ts as f64);
                cols[2].push(f64::from(r.source));
                cols[3].push(f64::from(r.channel));
                cols[4].push(f64::from(unit));
                let w0 = base + 10 * cnt + 2 * wf * k;
                for w in 0..width as u64 {
                    cols[5 + w as usize].push(if w < wf {
                        f64::from(b.i16_at(w0 + 2 * w).unwrap_or(0)) * scale
                    } else {
                        f64::NAN
                    });
                }
            }
        }
        Ok(Table {
            table: 0,
            first_row: first,
            columns: cols,
        })
    }

    fn read_events(&mut self, first: u64, n: u64) -> Result<Vec<Vec<f64>>> {
        let mut cols: Vec<Vec<f64>> = (0..5).map(|_| Vec::with_capacity(n as usize)).collect();
        let clock = self.clock();
        let (path, len) = (self.path.clone(), self.pl2.file_len);
        let end = first + n;
        let runs: Vec<_> = self
            .pl2
            .index
            .events
            .iter()
            .filter(|r| r.first_row + u64::from(r.count) > first && r.first_row < end)
            .copied()
            .collect();
        for r in runs {
            let cnt = u64::from(r.count);
            let base = r.offset + RECORD_HEADER_LEN;
            let b = read_block(self.handle()?, &path, base, 10 * cnt, len)?;
            if (b.len() as u64) < 10 * cnt {
                return Err(Error::corrupt_at(
                    FORMAT_ID,
                    r.offset,
                    "event record cut off",
                ));
            }
            for k in first.saturating_sub(r.first_row)..(end - r.first_row).min(cnt) {
                let ts = b.u64_at(base + 8 * k).unwrap_or(0);
                cols[0].push(ts as f64 / clock);
                cols[1].push(ts as f64);
                cols[2].push(f64::from(r.source));
                cols[3].push(f64::from(r.channel));
                cols[4].push(f64::from(b.u16_at(base + 8 * cnt + 2 * k).unwrap_or(0)));
            }
        }
        Ok(cols)
    }
}

impl Dataset for Pl2Dataset {
    fn info(&self) -> Result<FileInfo> {
        let h = &self.pl2.header;
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
            if !h.application.is_empty() {
                e.insert("application".into(), json!(h.application));
            }
            if !h.application_version.is_empty() {
                e.insert("application_version".into(), json!(h.application_version));
            }
        }
        let mut notes = vec![format!(
            "PL2: analog channels on one sample grid form a trace (values in V = raw × volts per count); spike waveforms and digital events are tables; timestamps count a {} Hz clock from the recording start",
            h.clock_hz
        )];
        notes.push("PL2 layout was derived from hex dumps of public files; spike waveforms and events are confirmed against Neo on a PLX of the same recording for one offline-written file, analog records only for internal consistency".into());
        if self
            .pl2
            .index
            .findings
            .iter()
            .any(|f| f.severity == openreadout_core::model::Severity::Error)
        {
            notes.push("structural problems found (e.g. truncation); run `check`".into());
        }
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.pl2.file_len,
            format: PlexonReader.descriptor(),
            format_version: Some("PL2".into()),
            images: Vec::new(),
            tables,
            spectra: Vec::new(),
            traces,
            plane_count: 0,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let p = &self.pl2;
        let h = &p.header;
        let ch = |c: &Pl2Channel| {
            json!({
                "name": c.name, "source": c.source, "channel": c.channel, "enabled": c.enabled,
                "recording": c.recording, "units": c.units, "rate_hz": c.rate_hz,
                "units_per_count": c.units_per_count, "waveform_samples": c.waveform_samples,
                "threshold": c.threshold, "pre_threshold": c.pre_threshold,
                "unit_counts": c.unit_counts, "device": c.device,
            })
        };
        Ok(json!({
            "container": "pl2",
            "comment": h.comment, "application": h.application,
            "application_version": h.application_version, "recorded_at": h.recorded_at,
            "clock_hz": h.clock_hz, "start_count": h.start_count, "duration_ticks": h.duration_ticks,
            "channel_counts": h.channel_counts, "data_start": h.data_start,
            "footer_start": h.footer_start,
            "spike_channels": p.spike_channels.iter().map(ch).collect::<Vec<_>>(),
            "analog_channels": p.analog_channels.iter().map(ch).collect::<Vec<_>>(),
            "digital_channels": p.digital_channels.iter().map(ch).collect::<Vec<_>>(),
            "record_counts": p.index.record_counts.iter().map(|(k, v)| (format!("{k:#04x}"), *v)).collect::<BTreeMap<_, _>>(),
        }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for k in [
            "traces[].sample_rate_hz",
            "traces[].channels[].scale",
            "traces[].channels[].unit",
            "traces[].sweep_count",
            "traces[].start_s",
            "tables[spikes].columns",
            "tables[events].columns",
            "traces[].extra.recorded_at",
            "tables[].extra.recorded_at",
            "traces[].extra.application",
            "tables[].extra.application",
            "traces[].extra.application_version",
            "tables[].extra.application_version",
        ] {
            p.insert(k.into(), Source::Inferred);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let p = &self.pl2;
        Ok(vec![
            LsEntry {
                kind: "header".into(),
                name: "PL2 file header".into(),
                offset: Some(0),
                size: Some(PL2_FILE_HEADER_LEN),
                image: None,
                details: json!({"channel_counts": p.header.channel_counts}),
            },
            LsEntry {
                kind: "header".into(),
                name: "channel headers".into(),
                offset: Some(PL2_FILE_HEADER_LEN),
                size: Some(p.header.headers_end.saturating_sub(PL2_FILE_HEADER_LEN)),
                image: None,
                details: json!({"spike": p.spike_channels.len(), "analog": p.analog_channels.len(), "digital": p.digital_channels.len()}),
            },
            LsEntry {
                kind: "records".into(),
                name: "data records".into(),
                offset: Some(p.header.data_start),
                size: Some(p.index.walk_end.saturating_sub(p.header.data_start)),
                image: None,
                details: json!({"records_by_type": p.index.record_counts.iter().map(|(k, v)| (format!("{k:#04x}"), *v)).collect::<BTreeMap<_, _>>()}),
            },
            LsEntry {
                kind: "footer".into(),
                name: "footer (settings and index; not read)".into(),
                offset: Some(p.header.footer_start),
                size: Some(p.file_len.saturating_sub(p.header.footer_start)),
                image: None,
                details: Value::Null,
            },
        ])
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::unsupported(
            FORMAT_ID,
            "image planes",
            "Plexon files hold continuous signals, spikes and events: use `openreadout trace` for signals and `openreadout export --to csv --table N` for tables.",
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
        let (path, len) = (self.path.clone(), self.pl2.file_len);
        let mut out = Vec::with_capacity(t.channels.len());
        for &ci in &t.channels {
            let c = self.pl2.analog_channels[ci].clone();
            let blocks: Vec<SampleBlock> = Self::blocks_of(&self.pl2, &c)
                .map(|v| v[run.first_block..run.first_block + run.block_count].to_vec())
                .unwrap_or_default();
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
                    return Err(Error::corrupt_at(FORMAT_ID, at, "analog record cut off"));
                }
                for k in 0..take {
                    let raw = blk
                        .i16_at(at + 2 * k)
                        .ok_or_else(|| Error::corrupt_at(FORMAT_ID, at, "sample cut off"))?;
                    v.push(f64::from(raw) * c.units_per_count);
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
            self.pl2.index.spike_rows
        } else if Some(index) == e {
            self.pl2.index.event_rows
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
        Ok(Table {
            table: index,
            first_row,
            columns: self.read_events(first_row, n)?,
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), FORMAT_ID);
        r.performed("PL2 signature; file header and every channel-header record (type and length)");
        r.performed("every data record inside the file (truncation); the walk ends on the end-of-recording record at the footer offset");
        r.performed(
            "spikes per unit in each spike-channel header equal the spikes in its data records",
        );
        r.performed("analog records of a channel: timestamps never go backwards; gaps reported");
        for f in &self.pl2.index.findings {
            r.push(f.clone());
        }
        let p = &self.pl2;
        if p.header.clock_hz <= 0.0 || !p.header.clock_hz.is_finite() {
            r.push(Finding::error(
                "bad_clock",
                format!("timestamp clock {} Hz", p.header.clock_hz),
            ));
        }
        let ended = p.index.record_counts.contains_key(&REC_END);
        if !ended {
            r.push(Finding::warning(
                "no_end_record",
                "no end-of-recording record before the footer (the recording may have been cut short)",
            ));
        }
        if p.index.walk_end != p.header.footer_start && p.index.walk_end < p.file_len {
            r.push(
                Finding::warning(
                    "footer_mismatch",
                    format!(
                        "the data records end at byte {}, the header puts the footer at {}",
                        p.index.walk_end, p.header.footer_start
                    ),
                )
                .at(p.index.walk_end),
            );
        }
        if let (Some(a), b) = (p.index.end_duration, p.header.duration_ticks)
            && a != b
        {
            r.push(Finding::warning(
                "duration_mismatch",
                format!("the end-of-recording record says {a} ticks, the header {b}"),
            ));
        }
        let mut found: BTreeMap<(u8, u16), u64> = BTreeMap::new();
        for s in &p.index.spikes {
            *found.entry((s.source, s.channel)).or_default() += u64::from(s.count);
        }
        for c in &p.spike_channels {
            let want: u64 = c.unit_counts.iter().sum();
            let got = u16::try_from(c.channel)
                .ok()
                .and_then(|ch| found.get(&(c.source, ch)))
                .copied()
                .unwrap_or(0);
            if want != got {
                r.push(Finding::warning(
                    "spike_count_mismatch",
                    format!(
                        "{}: the header counts {want} spikes, the data records hold {got}",
                        c.name
                    ),
                ));
            }
        }
        let known: Vec<(u8, u32)> = p
            .analog_channels
            .iter()
            .map(|c| (c.source, c.channel))
            .collect();
        for ((src, ch), blocks) in &p.index.analog {
            if !known.contains(&(*src, u32::from(*ch))) {
                r.push(Finding::warning(
                    "unlisted_channel",
                    format!("analog records for source {src} channel {ch}, which has no channel header (not read)"),
                ));
            }
            let back = blocks
                .windows(2)
                .filter(|w| w[1].timestamp < w[0].timestamp)
                .count();
            if back > 0 {
                r.push(Finding::warning(
                    "timestamps_decrease",
                    format!("source {src} channel {ch}: {back} record timestamps go backwards"),
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
        Ok(r)
    }
}
