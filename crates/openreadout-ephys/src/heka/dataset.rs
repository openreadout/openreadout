//! `Dataset` for a PatchMaster bundle: every series is a trace (sweeps × channels); channels of a
//! series whose sample interval or point counts differ form further traces of that series.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_core::bytes::read_block;
use openreadout_core::model::{
    CheckReport, FileInfo, Finding, LsEntry, SignalChannelInfo, Trace, TraceInfo,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::source::{Fs, Input, SourceFile};
use openreadout_core::{Error, Plane, Result};
use serde_json::{Value, json};

use super::bundle::{BUNDLE_HEADER_LEN, Bundle, parse_bundle};
use super::tree::{
    PulsedTree, SampleType, TraceRecord, heka_time_iso, parse_pulsed, recording_mode_name,
};
use super::{HEKA_FORMAT_ID, HekaReader};

/// Samples per channel decoded per `read_trace` call at most.
pub const MAX_HEKA_READ: u64 = 1 << 26;
/// Largest pulsed tree read into memory, bytes.
pub const MAX_PUL_LEN: u64 = 256 << 20;

/// One trace of the normalized view: channels of one series sharing a sample grid.
#[derive(Debug, Clone)]
pub struct HekaTrace {
    /// Group index in the file.
    pub group: usize,
    /// Series index within the group.
    pub series: usize,
    /// Trace-record indices (within each sweep) that are this trace's channels.
    pub channels: Vec<usize>,
    /// The series' sweeps in this trace, with their sample counts.
    pub sweeps: Vec<(usize, u64)>,
    /// Sample interval, seconds.
    pub interval_s: f64,
    /// Part number when a series is split into several traces (0 otherwise).
    pub part: usize,
}

/// An opened PatchMaster bundle.
#[derive(Debug)]
pub struct HekaDataset {
    path: PathBuf,
    fs: Fs,
    handle: Option<SourceFile>,
    file_len: u64,
    bundle: Bundle,
    tree: PulsedTree,
    traces: Vec<HekaTrace>,
}

impl HekaDataset {
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
        let head = read_block(&mut f, &path, 0, BUNDLE_HEADER_LEN, file_len)?;
        let bundle = parse_bundle(&head, file_len)?;
        let pul = bundle.item(".pul").cloned().ok_or_else(|| {
            Error::corrupt(HEKA_FORMAT_ID, "the bundle has no .pul (pulsed tree) item")
        })?;
        if pul.end() > file_len {
            return Err(Error::corrupt_at(
                HEKA_FORMAT_ID,
                pul.start,
                format!(
                    "the pulsed tree (bytes {}..{}) runs past the end of the file ({file_len} bytes): the file is truncated",
                    pul.start,
                    pul.end()
                ),
            ));
        }
        if pul.length > MAX_PUL_LEN {
            return Err(Error::unsupported(
                HEKA_FORMAT_ID,
                format!("pulsed trees larger than {MAX_PUL_LEN} bytes"),
                "the file's pulsed tree is larger than this reader accepts; it may be damaged.",
            ));
        }
        let block = read_block(&mut f, &path, pul.start, pul.length, file_len)?;
        let tree = parse_pulsed(&block.bytes)?;
        let traces = layout(&tree);
        Ok(HekaDataset {
            path,
            fs,
            handle: Some(f),
            file_len,
            bundle,
            tree,
            traces,
        })
    }

    /// The parsed pulsed tree.
    pub fn tree(&self) -> &PulsedTree {
        &self.tree
    }

    /// The normalized traces.
    pub fn heka_traces(&self) -> &[HekaTrace] {
        &self.traces
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

    fn record(&self, t: &HekaTrace, sweep: usize, channel: usize) -> Option<&TraceRecord> {
        self.tree
            .groups
            .get(t.group)?
            .series
            .get(t.series)?
            .sweeps
            .get(sweep)?
            .traces
            .get(channel)
    }
}

/// The sweeps a channel is in, with their point counts.
type SweepCounts = Vec<(usize, u64)>;

/// Group the channels of every series into traces that share a sample grid.
fn layout(tree: &PulsedTree) -> Vec<HekaTrace> {
    let mut out = Vec::new();
    for (gi, g) in tree.groups.iter().enumerate() {
        for (si, s) in g.series.iter().enumerate() {
            let width = s.sweeps.iter().map(|w| w.traces.len()).max().unwrap_or(0);
            // per channel and interval: the sweeps it is in, with their point counts
            let mut keyed: Vec<(u64, SweepCounts, usize)> = Vec::new();
            for c in 0..width {
                let mut by_dx: Vec<(u64, SweepCounts)> = Vec::new();
                for (wi, w) in s.sweeps.iter().enumerate() {
                    if let Some(tr) = w.traces.get(c) {
                        let dx = tr.x_interval.to_bits();
                        match by_dx.iter_mut().find(|(d, _)| *d == dx) {
                            Some((_, v)) => v.push((wi, tr.points)),
                            None => by_dx.push((dx, vec![(wi, tr.points)])),
                        }
                    }
                }
                for (dx, sweeps) in by_dx {
                    keyed.push((dx, sweeps, c));
                }
            }
            let mut groups: Vec<HekaTrace> = Vec::new();
            for (dx, sweeps, c) in keyed {
                if let Some(t) = groups
                    .iter_mut()
                    .find(|t| t.interval_s.to_bits() == dx && t.sweeps == sweeps)
                {
                    t.channels.push(c);
                } else {
                    groups.push(HekaTrace {
                        group: gi,
                        series: si,
                        channels: vec![c],
                        sweeps,
                        interval_s: f64::from_bits(dx),
                        part: 0,
                    });
                }
            }
            let parts = groups.len();
            for (k, mut t) in groups.into_iter().enumerate() {
                if parts > 1 {
                    t.part = k + 1;
                }
                out.push(t);
            }
        }
    }
    out
}

fn finite(v: f64) -> Value {
    if v.is_finite() { json!(v) } else { Value::Null }
}

impl HekaDataset {
    fn trace_info(&self, index: usize, t: &HekaTrace) -> TraceInfo {
        let g = &self.tree.groups[t.group];
        let s = &g.series[t.series];
        let first_sweep = t.sweeps.first().map_or(0, |x| x.0);
        let mut channels = Vec::new();
        for (k, &c) in t.channels.iter().enumerate() {
            let Some(r) = self.record(t, first_sweep, c) else {
                continue;
            };
            let st = r.sample_type();
            let mut extra = BTreeMap::new();
            extra.insert("trace_record".into(), json!(c));
            if let Some(a) = r.adc_channel {
                extra.insert("adc_channel".into(), json!(a));
            }
            extra.insert(
                "recording_mode".into(),
                json!(recording_mode_name(r.recording_mode)),
            );
            extra.insert("zero_offset".into(), finite(r.zero_offset));
            if r.time_offset_s != 0.0 {
                extra.insert("time_offset_s".into(), finite(r.time_offset_s));
            }
            if r.x_start != 0.0 {
                extra.insert("x_start_s".into(), finite(r.x_start));
            }
            if let Some(v) = r.bandwidth_hz.filter(|v| *v > 0.0) {
                extra.insert("bandwidth_hz".into(), json!(v));
            }
            if let Some(v) = r.linked_dac {
                extra.insert("linked_dac".into(), json!(v));
            }
            for (key, bit) in [
                ("leak", 1),
                ("virtual", 2),
                ("imon", 3),
                ("vmon", 4),
                ("clipped", 5),
            ] {
                if r.kind_bit(bit) {
                    extra.insert(key.into(), json!(true));
                }
            }
            for (key, v) in [
                ("series_resistance_ohm", r.rs_ohm),
                ("c_slow_f", r.c_slow_f),
                ("seal_resistance_ohm", r.seal_ohm),
                ("pipette_resistance_ohm", r.pipette_ohm),
                ("holding", r.holding),
            ] {
                if let Some(v) = v.filter(|v| *v != 0.0) {
                    extra.insert(key.into(), json!(v));
                }
            }
            let varies = t.sweeps.iter().any(|&(w, _)| {
                self.record(t, w, c)
                    .is_some_and(|x| x.scaler.to_bits() != r.scaler.to_bits())
            });
            if varies {
                extra.insert("scale_varies".into(), json!(true));
            }
            let integer = st.is_some_and(SampleType::is_integer);
            channels.push(SignalChannelInfo {
                index: k as u32,
                name: if r.label.is_empty() {
                    format!("ch{c}")
                } else {
                    r.label.clone()
                },
                unit: (!r.unit.is_empty()).then(|| r.unit.clone()),
                dtype: st.map_or("unknown", SampleType::dtype).into(),
                scale: if integer { r.scaler } else { 1.0 },
                offset: 0.0,
                extra,
            });
        }
        let mut extra = BTreeMap::new();
        extra.insert("group_index".into(), json!(t.group));
        extra.insert("group_label".into(), json!(g.label));
        extra.insert("series_index".into(), json!(t.series));
        extra.insert("series_label".into(), json!(s.label));
        if !s.comment.is_empty() {
            extra.insert("series_comment".into(), json!(s.comment));
        }
        if t.part > 0 {
            extra.insert("series_part".into(), json!(t.part));
        }
        let idx: Vec<usize> = t.sweeps.iter().map(|x| x.0).collect();
        if idx.len() != s.sweeps.len() || idx.iter().enumerate().any(|(k, &w)| k != w) {
            // the series' sweeps this trace holds, when some lack its channels
            extra.insert("sweep_indices".into(), json!(idx));
        }
        let counts: Vec<u64> = t.sweeps.iter().map(|x| x.1).collect();
        if counts.windows(2).any(|w| w[0] != w[1]) {
            extra.insert("sweep_sample_counts".into(), json!(counts));
        }
        let t0 = self.tree.start_time;
        let starts: Vec<Value> = t
            .sweeps
            .iter()
            .map(|&(w, _)| {
                let x = s.sweeps[w].time - t0;
                if x.is_finite() && t0.is_finite() && t0 != 0.0 {
                    json!(x)
                } else {
                    Value::Null
                }
            })
            .collect();
        let start_s = starts.first().and_then(Value::as_f64);
        if t.sweeps.len() <= 10_000 && starts.iter().any(|v| !v.is_null()) {
            extra.insert("sweep_starts_s".into(), json!(starts));
        }
        if let Some(iso) = s
            .sweeps
            .get(first_sweep)
            .and_then(|w| heka_time_iso(w.time))
        {
            extra.insert("recorded_at".into(), json!(iso));
        }
        if let Some(temp) = s
            .sweeps
            .get(first_sweep)
            .and_then(|w| w.temperature)
            .filter(|v| *v != 0.0)
        {
            extra.insert("temperature_c".into(), json!(temp));
        }
        if index == 0 {
            extra.insert("application".into(), json!("PatchMaster"));
            extra.insert("application_version".into(), json!(self.bundle.version));
            extra.insert("pulsed_tree_version".into(), json!(self.tree.root_version));
        }
        if !s.method.is_empty() {
            extra.insert("method".into(), json!(s.method));
        }
        let name = if t.part > 0 {
            format!("{} (part {})", s.label, t.part)
        } else {
            s.label.clone()
        };
        TraceInfo {
            index: index as u32,
            name: Some(name),
            sample_rate_hz: if t.interval_s > 0.0 {
                1.0 / t.interval_s
            } else {
                0.0
            },
            sample_count: counts.iter().copied().max().unwrap_or(0),
            sweep_count: t.sweeps.len() as u32,
            channels,
            start_s,
            extra,
        }
    }

    /// Why samples of this trace cannot be read, if they cannot.
    fn unreadable(&self, t: &HekaTrace) -> Option<(String, String)> {
        for &(w, _) in &t.sweeps {
            for &c in &t.channels {
                let r = self.record(t, w, c)?;
                let Some(st) = r.sample_type() else {
                    return Some((
                        format!("sample format code {}", r.format_code),
                        "the trace stores samples in a format PatchMaster's documents do not define; the file may be damaged.".into(),
                    ));
                };
                if r.y_offset != 0.0 {
                    return Some((
                        "traces with a non-zero Y offset".into(),
                        format!(
                            "trace '{}' has a Y offset of {}; no development file has one, so how PatchMaster applies it is not validated. Export the series from PatchMaster.",
                            r.label, r.y_offset
                        ),
                    ));
                }
                if !st.is_integer() && r.scaler.to_bits() != 1.0f64.to_bits() {
                    return Some((
                        "float traces with a data scaler".into(),
                        format!(
                            "trace '{}' stores {} samples with a data scaler of {}; whether PatchMaster applies it to float samples is not validated.",
                            r.label,
                            st.dtype(),
                            r.scaler
                        ),
                    ));
                }
                if st.is_integer() && !(r.scaler.is_finite() && r.scaler != 0.0) {
                    return Some((
                        "traces without a usable data scaler".into(),
                        format!(
                            "trace '{}' has data scaler {}; its samples cannot be scaled to {}.",
                            r.label, r.scaler, r.unit
                        ),
                    ));
                }
                if r.interleave_size != 0
                    && (r.interleave_size % st.width() != 0
                        || r.interleave_skip < r.interleave_size)
                {
                    return Some((
                        "irregular interleaving".into(),
                        format!(
                            "trace '{}' is interleaved in blocks of {} bytes every {} bytes, which does not divide into {}-byte samples.",
                            r.label,
                            r.interleave_size,
                            r.interleave_skip,
                            st.width()
                        ),
                    ));
                }
            }
        }
        None
    }

    /// Byte ranges (offset, length) holding samples `first..first+n` of a record.
    fn ranges(r: &TraceRecord, width: u64, first: u64, n: u64) -> Option<Vec<(u64, u64)>> {
        if r.interleave_size == 0 {
            let at = r.data_offset.checked_add(first.checked_mul(width)?)?;
            return Some(vec![(at, n.checked_mul(width)?)]);
        }
        let per = r.interleave_size / width; // samples per block
        let mut out = Vec::new();
        let mut i = first;
        let end = first.checked_add(n)?;
        while i < end {
            let block = i / per;
            let within = i % per;
            let take = (per - within).min(end - i);
            let at = r
                .data_offset
                .checked_add(block.checked_mul(r.interleave_skip)?)?
                .checked_add(within.checked_mul(width)?)?;
            out.push((at, take.checked_mul(width)?));
            i += take;
        }
        Some(out)
    }
}

fn decode(bytes: &[u8], st: SampleType, scale: f64, out: &mut Vec<f64>) {
    match st {
        SampleType::Int16 => out.extend(
            bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| f64::from(i16::from_le_bytes(*c)) * scale),
        ),
        SampleType::Int32 => out.extend(
            bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| f64::from(i32::from_le_bytes(*c)) * scale),
        ),
        SampleType::Float32 => out.extend(
            bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| f64::from(f32::from_le_bytes(*c))),
        ),
        SampleType::Float64 => out.extend(
            bytes
                .as_chunks::<8>()
                .0
                .iter()
                .map(|c| f64::from_le_bytes(*c)),
        ),
    }
}

impl Dataset for HekaDataset {
    fn info(&self) -> Result<FileInfo> {
        let series: usize = self.tree.groups.iter().map(|g| g.series.len()).sum();
        let mut notes = vec![format!(
            "{} group(s), {series} series: every series is a trace (sweeps × channels); read samples with `openreadout trace FILE --trace N --sweep M`",
            self.tree.groups.len()
        )];
        notes.push("values are raw × data scaler in the unit the file records (A, V); PatchMaster's zero level (`zero_offset` per channel) is not subtracted".into());
        let split = self.traces.iter().filter(|t| t.part > 0).count();
        if split > 0 {
            notes.push(format!(
                "{split} trace(s) are parts of a series whose channels differ in sample interval or length"
            ));
        }
        let refused: Vec<String> = self
            .traces
            .iter()
            .enumerate()
            .filter_map(|(i, t)| self.unreadable(t).map(|(what, _)| format!("{i} ({what})")))
            .collect();
        if !refused.is_empty() {
            notes.push(format!(
                "samples of trace(s) {} are not read: features never validated",
                refused.join(", ")
            ));
        }
        let dat = self.bundle.item(".dat");
        let out_of_bounds = self.traces.iter().any(|t| {
            t.sweeps.iter().any(|&(w, _)| {
                t.channels.iter().any(|&c| {
                    self.record(t, w, c).is_some_and(|r| {
                        let width = r.sample_type().map_or(2, SampleType::width);
                        let end = if r.interleave_size == 0 {
                            r.data_offset.saturating_add(r.points.saturating_mul(width))
                        } else {
                            r.data_offset
                        };
                        end > self.file_len || dat.is_some_and(|d| end > d.end())
                    })
                })
            })
        });
        if out_of_bounds {
            notes.push("structural problems found (samples outside the data item or past the end of the file); run `check`".into());
        }
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.file_len,
            format: HekaReader.descriptor(),
            format_version: Some(
                self.bundle
                    .version
                    .trim_start_matches(['v', 'V'])
                    .to_string(),
            ),
            images: Vec::new(),
            tables: Vec::new(),
            spectra: Vec::new(),
            traces: self
                .traces
                .iter()
                .enumerate()
                .map(|(i, t)| self.trace_info(i, t))
                .collect(),
            plane_count: 0,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let b = &self.bundle;
        let groups: Vec<Value> = self
            .tree
            .groups
            .iter()
            .map(|g| {
                json!({
                    "label": g.label,
                    "text": g.text,
                    "experiment_number": g.experiment_number,
                    "series": g.series.iter().map(|s| json!({
                        "label": s.label,
                        "comment": s.comment,
                        "series_number": s.series_count,
                        "time": heka_time_iso(s.time),
                        "method": s.method,
                        "username": s.username,
                        "sweep_count": s.sweeps.len(),
                        "sweeps": s.sweeps.iter().take(10_000).map(|w| json!({
                            "label": w.label,
                            "time": heka_time_iso(w.time),
                            "timer_s": finite(w.timer),
                            "stimulus": w.stim_count,
                            "sweep_number": w.sweep_count,
                            "temperature_c": w.temperature,
                            "traces": w.traces.iter().map(|t| json!({
                                "label": t.label,
                                "data_offset": t.data_offset,
                                "points": t.points,
                                "format": t.sample_type().map(SampleType::dtype),
                                "data_kind": t.data_kind,
                                "recording_mode": recording_mode_name(t.recording_mode),
                                "scaler": finite(t.scaler),
                                "zero_offset": finite(t.zero_offset),
                                "unit": t.unit,
                                "x_interval": finite(t.x_interval),
                                "x_start": finite(t.x_start),
                                "x_unit": t.x_unit,
                                "time_offset_s": finite(t.time_offset_s),
                                "y_offset": finite(t.y_offset),
                                "adc_channel": t.adc_channel,
                                "interleave_size": t.interleave_size,
                                "interleave_skip": t.interleave_skip,
                            })).collect::<Vec<_>>(),
                        })).collect::<Vec<_>>(),
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        Ok(json!({
            "bundle": {
                "signature": b.signature,
                "version": b.version,
                "modified": heka_time_iso(b.modified),
                "item_count": b.item_count,
                "little_endian": b.little_endian,
                "items": b.items.iter().map(|i| json!({
                    "slot": i.slot, "extension": i.extension, "start": i.start, "length": i.length,
                })).collect::<Vec<_>>(),
            },
            "root": {
                "version": self.tree.root_version,
                "version_name": self.tree.version_name,
                "text": self.tree.root_text,
                "start_time": heka_time_iso(self.tree.start_time),
                "record_sizes": self.tree.record_sizes,
            },
            "groups": groups,
        }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for (k, s) in [
            ("format_version", Source::Spec),
            ("traces[].sample_rate_hz", Source::Spec),
            ("traces[].sample_count", Source::Spec),
            ("traces[].sweep_count", Source::Spec),
            ("traces[].channels[].name", Source::Spec),
            ("traces[].channels[].unit", Source::Spec),
            ("traces[].channels[].scale", Source::Spec),
            ("traces[].channels[].extra", Source::Spec),
            ("traces[].name", Source::Spec),
            ("traces[].start_s", Source::Spec),
            ("traces[].extra.recorded_at", Source::Spec),
            ("traces[].extra.sweep_starts_s", Source::Spec),
            ("traces[].extra.series_label", Source::Spec),
            ("traces[].extra.group_label", Source::Spec),
            ("traces[].extra.series_part", Source::Inferred),
            ("traces[].channels[].extra.zero_offset", Source::Spec),
        ] {
            p.insert(k.into(), s);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let mut out: Vec<LsEntry> = self
            .bundle
            .items
            .iter()
            .map(|i| LsEntry {
                kind: "bundle-item".into(),
                name: i.extension.clone(),
                offset: Some(i.start),
                size: Some(i.length),
                image: None,
                details: json!({"slot": i.slot}),
            })
            .collect();
        for (k, t) in self.traces.iter().enumerate() {
            let s = &self.tree.groups[t.group].series[t.series];
            out.push(LsEntry {
                kind: "series".into(),
                name: format!("trace {k}: {} / {}", self.tree.groups[t.group].label, s.label),
                offset: None,
                size: None,
                image: None,
                details: json!({"group": t.group, "series": t.series, "sweeps": t.sweeps.len(), "channels": t.channels.len()}),
            });
        }
        Ok(out)
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::unsupported(
            HEKA_FORMAT_ID,
            "image planes",
            "PatchMaster files hold sampled signals, not images: use `openreadout trace FILE --trace N --sweep M` or `export --to csv`.",
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
                "trace index {index} out of range (the file has {} traces, 0..{})",
                self.traces.len(),
                self.traces.len()
            ))
        })?;
        let &(w, count) = t.sweeps.get(sweep as usize).ok_or_else(|| {
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
        if let Some((what, hint)) = self.unreadable(&t) {
            return Err(Error::unsupported(HEKA_FORMAT_ID, what, hint));
        }
        let n = max_samples.min(count - first_sample).min(MAX_HEKA_READ);
        let dat_end = self
            .bundle
            .item(".dat")
            .map_or(self.file_len, super::bundle::BundleItem::end);
        let limit = dat_end.min(self.file_len);
        let file_len = self.file_len;
        let path = self.path.clone();
        let records: Vec<TraceRecord> = t
            .channels
            .iter()
            .map(|&c| {
                self.record(&t, w, c).cloned().ok_or_else(|| {
                    Error::corrupt(HEKA_FORMAT_ID, "trace record missing from the pulsed tree")
                })
            })
            .collect::<Result<_>>()?;
        let mut trace = Trace {
            trace: index,
            sweep,
            first_sample,
            channels: Vec::with_capacity(records.len()),
        };
        for r in &records {
            let st = r
                .sample_type()
                .ok_or_else(|| Error::corrupt(HEKA_FORMAT_ID, "unknown sample format"))?;
            let scale = if st.is_integer() { r.scaler } else { 1.0 };
            let mut col = Vec::with_capacity(n as usize);
            let ranges = Self::ranges(r, st.width(), first_sample, n)
                .ok_or_else(|| Error::corrupt(HEKA_FORMAT_ID, "sample offset overflow"))?;
            for (at, len) in ranges {
                if at.saturating_add(len) > limit {
                    return Err(Error::corrupt_at(
                        HEKA_FORMAT_ID,
                        at,
                        format!(
                            "samples of '{}' (sweep {sweep}) need bytes up to {} but the data item ends at byte {limit} (truncated or damaged)",
                            r.label,
                            at.saturating_add(len)
                        ),
                    ));
                }
                let block = read_block(self.handle()?, &path, at, len, file_len)?;
                decode(&block.bytes, st, scale, &mut col);
            }
            trace.channels.push(col);
        }
        Ok(trace)
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), HEKA_FORMAT_ID);
        r.performed(
            "bundle signature (DAT2), version text, endian flag; every bundle item inside the file",
        );
        r.performed("pulsed tree: magic, five levels, stored record sizes, every record and child count inside the tree, the walk ends where the .pul item ends");
        r.performed("every trace record: sample format known, data scaler usable, samples inside the .dat item");
        r.performed("sweeps of a series hold the same channels on one sample grid");
        for i in &self.bundle.items {
            if i.end() > self.file_len {
                r.push(
                    Finding::error(
                        "truncated",
                        format!(
                            "bundle item {} ({}) ends at byte {} but the file has {} bytes",
                            i.slot,
                            i.extension,
                            i.end(),
                            self.file_len
                        ),
                    )
                    .at(i.start),
                );
            }
        }
        if let Some(p) = self.bundle.item(".pul")
            && self.tree.walked_len != p.length
        {
            r.push(Finding::warning(
                "tree_length_mismatch",
                format!(
                    "the pulsed tree walk ends after {} bytes; the .pul item holds {}",
                    self.tree.walked_len, p.length
                ),
            ));
        }
        let dat = self.bundle.item(".dat").cloned();
        let mut outside = 0usize;
        let mut first_outside: Option<u64> = None;
        let mut total = 0usize;
        for t in &self.traces {
            for &(w, _) in &t.sweeps {
                for &c in &t.channels {
                    let Some(rec) = self.record(t, w, c) else {
                        continue;
                    };
                    total += 1;
                    let width = rec.sample_type().map_or(2, SampleType::width);
                    let end = match Self::ranges(rec, width, 0, rec.points) {
                        Some(v) => v
                            .iter()
                            .map(|(a, l)| a.saturating_add(*l))
                            .max()
                            .unwrap_or(0),
                        None => u64::MAX,
                    };
                    let start_ok = dat.as_ref().is_none_or(|d| rec.data_offset >= d.start);
                    let end_ok =
                        end <= self.file_len && dat.as_ref().is_none_or(|d| end <= d.end());
                    if !(start_ok && end_ok) && rec.points > 0 {
                        outside += 1;
                        first_outside.get_or_insert(rec.data_offset);
                    }
                }
            }
        }
        if outside > 0 {
            r.push(
                Finding::error(
                    "truncated",
                    format!(
                        "{outside} of {total} trace records point at samples outside the .dat item or past the end of the file"
                    ),
                )
                .at(first_outside.unwrap_or(0)),
            );
        }
        for (i, t) in self.traces.iter().enumerate() {
            if let Some((what, _)) = self.unreadable(t) {
                r.push(Finding::warning(
                    "unsupported_trace",
                    format!("trace {i}: {what}; its samples are not read"),
                ));
            }
            if !(t.interval_s.is_finite() && t.interval_s > 0.0) {
                r.push(Finding::error(
                    "bad_interval",
                    format!("trace {i}: sample interval {} s", t.interval_s),
                ));
            }
        }
        let split = self.traces.iter().filter(|t| t.part > 0).count();
        if split > 0 {
            r.push(Finding::info(
                "series_split",
                format!("{split} trace(s) are parts of series whose channels differ in sample interval or length"),
            ));
        }
        Ok(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(points: u64, isize: u64, iskip: u64) -> TraceRecord {
        TraceRecord {
            label: "Imon-1".into(),
            data_offset: 1000,
            points,
            data_kind: 1,
            recording_mode: 3,
            format_code: 0,
            scaler: 1e-12,
            time_offset_s: 0.0,
            zero_offset: 0.0,
            unit: "A".into(),
            x_interval: 2e-5,
            x_start: 0.0,
            x_unit: "s".into(),
            y_offset: 0.0,
            bandwidth_hz: None,
            linked_dac: None,
            adc_channel: None,
            interleave_size: isize,
            interleave_skip: iskip,
            rs_ohm: None,
            c_slow_f: None,
            seal_ohm: None,
            pipette_ohm: None,
            holding: None,
        }
    }

    #[test]
    fn contiguous_and_interleaved_ranges() {
        let r = rec(100, 0, 0);
        assert_eq!(HekaDataset::ranges(&r, 2, 10, 5), Some(vec![(1020, 10)]));
        // 8 samples per 16-byte block, blocks 48 bytes apart
        let r = rec(100, 16, 48);
        assert_eq!(
            HekaDataset::ranges(&r, 2, 6, 5),
            Some(vec![(1012, 4), (1048, 6)])
        );
    }

    #[test]
    fn decodes_scaled_integers_and_floats() {
        let mut out = Vec::new();
        decode(&[0x10, 0x00, 0xF0, 0xFF], SampleType::Int16, 0.5, &mut out);
        assert_eq!(out, vec![8.0, -8.0]);
        out.clear();
        decode(&1.5f32.to_le_bytes(), SampleType::Float32, 1.0, &mut out);
        assert_eq!(out, vec![1.5]);
    }

    /// The minimal bundle of `fuzz/seeds.py` (`mini_heka`): one group, series, sweep and trace
    /// of eight int16 samples −4..3 scaled by 1e-12 A.
    pub(crate) fn mini_bundle() -> Vec<u8> {
        let sizes = [640usize, 144, 1408, 288, 512];
        let samples: Vec<u8> = (-4i16..4).flat_map(i16::to_le_bytes).collect();
        let dat_at = 256i32;
        let pul_at = dat_at + samples.len() as i32;
        let mut rec: Vec<Vec<u8>> = sizes.iter().map(|&s| vec![0u8; s]).collect();
        rec[0][520..528].copy_from_slice(&5_190_109_783.0f64.to_le_bytes());
        rec[1][4..6].copy_from_slice(b"E1");
        rec[2][4..6].copy_from_slice(b"S1");
        rec[3][48..56].copy_from_slice(&5_190_109_790.0f64.to_le_bytes());
        let tr = &mut rec[4];
        tr[4..10].copy_from_slice(b"Imon-1");
        tr[40..44].copy_from_slice(&dat_at.to_le_bytes());
        tr[44..48].copy_from_slice(&8i32.to_le_bytes());
        tr[64..66].copy_from_slice(&1u16.to_le_bytes());
        tr[68] = 3;
        tr[72..80].copy_from_slice(&1e-12f64.to_le_bytes());
        tr[96] = b'A';
        tr[104..112].copy_from_slice(&2e-5f64.to_le_bytes());
        tr[120] = b's';
        let mut tree = b"eerT".to_vec();
        tree.extend(5i32.to_le_bytes());
        for s in sizes {
            tree.extend((s as i32).to_le_bytes());
        }
        for (r, n) in rec.iter().zip([1i32, 1, 1, 1, 0]) {
            tree.extend(r);
            tree.extend(n.to_le_bytes());
        }
        let mut head = vec![0u8; 256];
        head[..4].copy_from_slice(b"DAT2");
        head[8..28].copy_from_slice(b"v2x90.2, 22-Nov-2016");
        head[48..52].copy_from_slice(&2i32.to_le_bytes());
        head[52] = 1;
        head[64..68].copy_from_slice(&dat_at.to_le_bytes());
        head[68..72].copy_from_slice(&(samples.len() as i32).to_le_bytes());
        head[72..76].copy_from_slice(b".dat");
        head[80..84].copy_from_slice(&pul_at.to_le_bytes());
        head[84..88].copy_from_slice(&(tree.len() as i32).to_le_bytes());
        head[88..92].copy_from_slice(b".pul");
        [head, samples, tree].concat()
    }

    #[test]
    fn reads_the_minimal_bundle() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("mini.dat");
        std::fs::write(&p, mini_bundle()).unwrap();
        let mut ds = HekaDataset::open(&p).unwrap();
        let info = ds.info().unwrap();
        assert_eq!(info.traces.len(), 1);
        let t = &info.traces[0];
        assert_eq!((t.sweep_count, t.sample_count), (1, 8));
        assert!((t.sample_rate_hz - 50_000.0).abs() < 1e-6);
        assert_eq!(t.channels[0].name, "Imon-1");
        assert_eq!(t.channels[0].unit.as_deref(), Some("A"));
        assert_eq!(t.start_s, Some(7.0));
        let tr = ds.read_trace(0, 0, 2, 3).unwrap();
        assert_eq!(tr.channels[0], vec![-2e-12, -1e-12, 0.0]);
        assert!(ds.check().unwrap().ok);
        assert!(ds.read_trace(0, 1, 0, 1).is_err());
        assert!(ds.read_trace(1, 0, 0, 1).is_err());
    }

    #[test]
    fn truncated_bundles_fail_cleanly() {
        let full = mini_bundle();
        let dir = tempfile::tempdir().unwrap();
        for cut in [0, 3, 8, 100, 255, 256, 270, 300, full.len() - 1] {
            let p = dir.path().join(format!("cut{cut}.dat"));
            std::fs::write(&p, &full[..cut]).unwrap();
            if let Ok(mut ds) = HekaDataset::open(&p) {
                let _ = ds.info();
                let _ = ds.check();
                let _ = ds.read_trace(0, 0, 0, 100);
            }
        }
        // samples cut off: the tree is intact but points at bytes past the .dat item
        let mut bad = full.clone();
        bad[68..72].copy_from_slice(&4i32.to_le_bytes());
        let p = dir.path().join("short.dat");
        std::fs::write(&p, &bad).unwrap();
        let mut ds = HekaDataset::open(&p).unwrap();
        assert!(ds.read_trace(0, 0, 0, 8).is_err());
        assert!(!ds.check().unwrap().ok);
    }
}
