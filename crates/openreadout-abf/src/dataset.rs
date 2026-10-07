//! `Dataset` implementation: one trace per file, sweeps × channels, scaled reads, checks.

use std::collections::BTreeMap;
use std::path::PathBuf;

use openreadout_core::model::{
    CheckReport, FileInfo, Finding, LsEntry, SignalChannelInfo, Trace, TraceInfo,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::source::{Fs, Input, SourceFile};
use openreadout_core::{Error, Plane, Result};
use serde_json::{Value, json};

use crate::command::{CommandPlan, command_plan, command_sweep};
use crate::file::{AbfFile, Generation, InputChannel, OutputChannel, SampleFormat, usable_factor};
use crate::{AbfReader, FORMAT_ID};
use openreadout_core::bytes::read_block;

/// Samples per channel decoded per read call at most (bounds memory of one `read_trace`).
pub const MAX_READ_SAMPLES: u64 = 1 << 26;

/// Sweeps `entries` lists one by one; the rest are summed up in one entry (a damaged header can
/// declare millions).
const MAX_SWEEP_ENTRIES: usize = 10_000;

/// An opened ABF file.
#[derive(Debug)]
pub struct AbfDataset {
    path: PathBuf,
    file: AbfFile,
    /// Command waveforms synthesized as trace 1 (when any).
    command: CommandPlan,
    handle: Option<SourceFile>,
    /// Where the file is read from.
    fs: Fs,
}

impl AbfDataset {
    pub fn open(path: &std::path::Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    /// Open an [`Input`] (a local path, a buffer, a host source).
    pub(crate) fn open_input(input: &Input) -> Result<Self> {
        let file = AbfFile::open_in(input.fs(), input.path())?;
        Ok(AbfDataset {
            path: input.path().to_path_buf(),
            command: command_plan(&file),
            file,
            handle: None,
            fs: input.fs().clone(),
        })
    }

    /// The parsed header (for library users).
    pub fn header(&self) -> &AbfFile {
        &self.file
    }

    fn read_command(&self, sweep: u32, first_sample: u64, max_samples: u64) -> Result<Trace> {
        let file = &self.file;
        let s = file.sweeps.get(sweep as usize).ok_or_else(|| {
            Error::Usage(format!(
                "sweep {sweep} out of range (file has {} sweeps)",
                file.sweeps.len()
            ))
        })?;
        if first_sample > s.sample_count {
            return Err(Error::Usage(format!(
                "first sample {first_sample} is past the end of sweep {sweep} ({} samples)",
                s.sample_count
            )));
        }
        let n = max_samples
            .min(s.sample_count - first_sample)
            .min(MAX_READ_SAMPLES);
        let lo = usize::try_from(first_sample).unwrap_or(usize::MAX);
        let hi = usize::try_from(first_sample + n).unwrap_or(usize::MAX);
        let mut channels = Vec::with_capacity(self.command.outputs.len());
        for &i in &self.command.outputs {
            let out = &file.outputs[i];
            let wave = command_sweep(out, sweep, s.sample_count).ok_or_else(|| {
                Error::unsupported(
                    FORMAT_ID,
                    format!("DAC {} epochs run past the end of sweep {sweep}", out.index),
                    "The command waveform is not synthesized for this sweep; the epoch table is in `info` (trace 0, extra.outputs).",
                )
            })?;
            channels.push(wave.get(lo..hi).map(<[f64]>::to_vec).unwrap_or_default());
        }
        Ok(Trace {
            trace: 1,
            sweep,
            first_sample,
            channels,
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
}

fn finite(v: f32) -> Value {
    if v.is_finite() { json!(v) } else { Value::Null }
}

fn channel_info(c: &InputChannel, fmt: SampleFormat) -> SignalChannelInfo {
    let mut extra = BTreeMap::new();
    extra.insert("adc_number".into(), json!(c.adc_number));
    if fmt == SampleFormat::Int16 {
        extra.insert("programmable_gain".into(), finite(c.programmable_gain));
        extra.insert("instrument_scale".into(), finite(c.instrument_scale));
        extra.insert("instrument_offset".into(), finite(c.instrument_offset));
        extra.insert("signal_gain".into(), finite(c.signal_gain));
        extra.insert("signal_offset".into(), finite(c.signal_offset));
    }
    if let Some(v) = c.lowpass_hz.filter(|v| v.is_finite()) {
        extra.insert("lowpass_hz".into(), json!(v));
    }
    if let Some(v) = c.highpass_hz.filter(|v| v.is_finite()) {
        extra.insert("highpass_hz".into(), json!(v));
    }
    if let Some(t) = &c.telegraph {
        extra.insert(
            "telegraph".into(),
            json!({
                "enabled": t.enabled,
                "instrument_code": t.instrument_code,
                "additional_gain": finite(t.additional_gain),
                "filter_hz": finite(t.filter_hz),
                "membrane_capacitance": finite(t.membrane_capacitance),
                "clamp_mode_code": t.clamp_mode_code,
            }),
        );
    }
    SignalChannelInfo {
        index: c.index,
        name: if c.name.is_empty() {
            format!("ch{}", c.index)
        } else {
            c.name.clone()
        },
        unit: (!c.unit.is_empty()).then(|| c.unit.clone()),
        dtype: fmt.dtype().into(),
        scale: c.scale,
        offset: c.offset,
        extra,
    }
}

fn output_json(o: &OutputChannel) -> Value {
    let holding =
        (o.holding_level.is_finite() && o.holding_level.abs() < 1e6).then_some(o.holding_level);
    json!({
        "index": o.index,
        "name": o.name,
        "unit": o.unit,
        "holding_level": holding,
        "waveform_enabled": o.waveform_enabled,
        "waveform_source_code": o.waveform_source_code,
        "stimulus_file": o.stimulus_file,
        "epochs": o.epochs.iter().map(|e| json!({
            "index": e.index,
            "kind": e.kind.name(),
            "kind_code": e.kind_code,
            "level": finite(e.level),
            "level_step": finite(e.level_step),
            "duration": e.duration,
            "duration_step": e.duration_step,
            "pulse_period": e.pulse_period,
            "pulse_width": e.pulse_width,
        })).collect::<Vec<_>>(),
    })
}

/// The normalized trace description of a parsed file.
pub fn trace_info(f: &AbfFile) -> TraceInfo {
    let mut extra = BTreeMap::new();
    extra.insert("abf_version".into(), json!(f.version));
    extra.insert("generation".into(), json!(f.generation.name()));
    extra.insert("acquisition_mode".into(), json!(f.mode.name()));
    extra.insert("acquisition_mode_code".into(), json!(f.mode.code()));
    extra.insert("sample_format".into(), json!(f.sample_format.dtype()));
    extra.insert("sample_interval_us".into(), json!(f.sample_interval_us));
    extra.insert("adc_range_v".into(), finite(f.adc_range_v));
    extra.insert("adc_resolution".into(), json!(f.adc_resolution));
    if let Some(v) = f.sweep_interval_s {
        extra.insert("sweep_interval_s".into(), json!(v));
    }
    if f.variable_length() {
        extra.insert(
            "sweep_sample_counts".into(),
            json!(f.sweeps.iter().map(|s| s.sample_count).collect::<Vec<_>>()),
        );
    }
    if !f.synch.is_empty() && f.sweeps.len() <= 10_000 {
        extra.insert(
            "sweep_starts_s".into(),
            json!(f.sweeps.iter().map(|s| s.start_s).collect::<Vec<_>>()),
        );
    }
    let put = |m: &mut BTreeMap<String, Value>, k: &str, v: Option<String>| {
        if let Some(v) = v {
            m.insert(k.into(), Value::String(v));
        }
    };
    put(&mut extra, "created_at", f.created_at.clone());
    put(&mut extra, "creator", f.creator.clone());
    put(&mut extra, "creator_version", f.creator_version.clone());
    put(&mut extra, "protocol", f.protocol_name());
    put(&mut extra, "protocol_path", f.protocol_path.clone());
    put(&mut extra, "comment", f.comment.clone());
    put(&mut extra, "guid", f.guid.clone());
    if let Some(c) = f.experiment_kind_code {
        extra.insert("experiment_kind_code".into(), json!(c));
    }
    if let Some(c) = f.digitizer_code {
        extra.insert("digitizer_code".into(), json!(c));
    }
    if !f.tags.is_empty() {
        extra.insert(
            "tags".into(),
            json!(
                f.tags
                    .iter()
                    .map(|t| json!({
                        "time_s": t.time_s,
                        "comment": t.comment,
                        "kind_code": t.kind_code,
                    }))
                    .collect::<Vec<_>>()
            ),
        );
    }
    let outputs: Vec<Value> = f
        .outputs
        .iter()
        .filter(|o| o.waveform_enabled || !o.epochs.is_empty() || !o.name.is_empty())
        .map(output_json)
        .collect();
    if !outputs.is_empty() {
        extra.insert("outputs".into(), json!(outputs));
    }
    if !f.digital.is_empty() {
        extra.insert(
            "digital_outputs".into(),
            json!(
                f.digital
                    .iter()
                    .map(|d| json!({
                        "epoch": d.epoch,
                        "pattern": format!("{:08b}", d.pattern & 0xFF),
                    }))
                    .collect::<Vec<_>>()
            ),
        );
    }
    TraceInfo {
        index: 0,
        name: f.protocol_name(),
        sample_rate_hz: f.sample_rate_hz,
        sample_count: f.max_sweep_len(),
        sweep_count: f.sweeps.len() as u32,
        channels: f
            .channels
            .iter()
            .map(|c| channel_info(c, f.sample_format))
            .collect(),
        start_s: Some(0.0),
        extra,
    }
}

/// Trace 1: the command (DAC) waveforms synthesized from the epoch table.
pub fn command_trace_info(f: &AbfFile, plan: &CommandPlan) -> Option<TraceInfo> {
    if plan.outputs.is_empty() {
        return None;
    }
    let mut extra = BTreeMap::new();
    extra.insert("synthesized".into(), json!(true));
    extra.insert(
        "source".into(),
        json!("epoch table (holding level for the first 1/64 of each sweep, then the epochs)"),
    );
    if !plan.refused.is_empty() {
        extra.insert("not_synthesized".into(), json!(plan.refused));
    }
    let channels = plan
        .outputs
        .iter()
        .enumerate()
        .map(|(k, &i)| {
            let o = &f.outputs[i];
            let mut e = BTreeMap::new();
            e.insert("dac".into(), json!(o.index));
            e.insert("holding_level".into(), finite(o.holding_level));
            SignalChannelInfo {
                index: k as u32,
                name: if o.name.is_empty() {
                    format!("DAC{}", o.index)
                } else {
                    o.name.clone()
                },
                unit: (!o.unit.is_empty()).then(|| o.unit.clone()),
                dtype: "float64".into(),
                scale: 1.0,
                offset: 0.0,
                extra: e,
            }
        })
        .collect();
    Some(TraceInfo {
        index: 1,
        name: Some("command".into()),
        sample_rate_hz: f.sample_rate_hz,
        sample_count: f.max_sweep_len(),
        sweep_count: f.sweeps.len() as u32,
        channels,
        start_s: Some(0.0),
        extra,
    })
}

impl Dataset for AbfDataset {
    fn info(&self) -> Result<FileInfo> {
        let f = &self.file;
        let mut notes = vec![format!(
            "{} sweep(s) × {} channel(s); read samples with `openreadout trace` or export them with `export --to csv`; values are scaled to physical units (value = raw × scale + offset)",
            f.sweeps.len(),
            f.channels.len()
        )];
        if f.findings
            .iter()
            .any(|x| x.severity == openreadout_core::model::Severity::Error)
        {
            notes.push("structural problems found (e.g. truncation); run `check`".into());
        }
        if !self.command.outputs.is_empty() {
            notes.push("trace 1 (`command`): the DAC command waveforms synthesized from the epoch table, one sweep per recorded sweep".into());
        }
        for r in &self.command.refused {
            notes.push(format!("command waveform: {r}"));
        }
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: f.file_len,
            format: AbfReader.descriptor(),
            format_version: Some(f.version.clone()),
            images: Vec::new(),
            tables: Vec::new(),
            spectra: Vec::new(),
            traces: std::iter::once(trace_info(f))
                .chain(command_trace_info(f, &self.command))
                .collect(),
            plane_count: 0,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let f = &self.file;
        let channels: Vec<Value> = f
            .channels
            .iter()
            .map(|c| {
                json!({
                    "index": c.index, "adc_number": c.adc_number, "name": c.name, "unit": c.unit,
                    "programmable_gain": finite(c.programmable_gain),
                    "instrument_scale": finite(c.instrument_scale),
                    "instrument_offset": finite(c.instrument_offset),
                    "signal_gain": finite(c.signal_gain), "signal_offset": finite(c.signal_offset),
                    "lowpass_hz": c.lowpass_hz.map(finite), "highpass_hz": c.highpass_hz.map(finite),
                })
            })
            .collect();
        Ok(json!({
            "generation": f.generation.name(),
            "version": f.version,
            "header_len": f.header_len,
            "acquisition_mode_code": f.mode.code(),
            "header_sweep_count": f.header_sweep_count,
            "data_offset": f.data_offset,
            "total_samples": f.total_samples,
            "sample_format": f.sample_format.dtype(),
            "synch_time_unit_us": finite(f.synch_time_unit_us),
            "sections": f.sections.iter().map(|s| json!({
                "name": s.name, "block": s.block, "entry_size": s.entry_size,
                "entry_count": s.entry_count, "offset": s.offset, "byte_len": s.byte_len,
            })).collect::<Vec<_>>(),
            "strings": f.strings,
            "input_channels": channels,
            "outputs": f.outputs.iter().map(output_json).collect::<Vec<_>>(),
            "synch_array": f.synch.iter().take(100_000).map(|(s, l)| json!([s, l])).collect::<Vec<_>>(),
            "tags": f.tags.iter().map(|t| json!({"raw_time": t.raw_time, "comment": t.comment, "kind_code": t.kind_code})).collect::<Vec<_>>(),
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
            ("traces[].channels[].extra", Source::PriorArt),
            ("traces[].name", Source::PriorArt),
            ("traces[].extra.acquisition_mode", Source::PriorArt),
            ("traces[].extra.created_at", Source::PriorArt),
            ("traces[].extra.creator", Source::PriorArt),
            ("traces[].extra.protocol_path", Source::PriorArt),
            ("traces[].extra.comment", Source::PriorArt),
            ("traces[].extra.tags", Source::PriorArt),
            ("traces[].extra.outputs", Source::PriorArt),
            ("traces[].extra.digital_outputs", Source::PriorArt),
            ("traces[].extra.sweep_sample_counts", Source::PriorArt),
            ("traces[].extra.sweep_starts_s", Source::Inferred),
            ("traces[].channels[].unit[µ]", Source::Inferred),
            ("vendor.strings", Source::Inferred),
        ] {
            p.insert(k.into(), s);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let f = &self.file;
        let mut out: Vec<LsEntry> = f
            .sections
            .iter()
            .map(|s| LsEntry {
                kind: if s.name == "header" {
                    "header".into()
                } else {
                    "section".into()
                },
                name: s.name.into(),
                offset: Some(s.offset),
                size: Some(s.byte_len),
                image: None,
                details: json!({"block": s.block, "entry_size": s.entry_size, "entry_count": s.entry_count}),
            })
            .collect();
        let row = f.channels.len() as u64 * f.sample_format.width();
        for (i, s) in f.sweeps.iter().enumerate().take(MAX_SWEEP_ENTRIES) {
            out.push(LsEntry {
                kind: "sweep".into(),
                name: format!("sweep {i}"),
                offset: Some(f.data_offset + s.first_sample * row),
                size: Some(s.sample_count * row),
                image: None,
                details: json!({"samples_per_channel": s.sample_count, "start_s": s.start_s}),
            });
        }
        if f.sweeps.len() > MAX_SWEEP_ENTRIES {
            out.push(LsEntry {
                kind: "sweep".into(),
                name: format!(
                    "{} more sweeps (not listed)",
                    f.sweeps.len() - MAX_SWEEP_ENTRIES
                ),
                offset: None,
                size: None,
                image: None,
                details: json!({"sweep_count": f.sweeps.len(), "listed": MAX_SWEEP_ENTRIES}),
            });
        }
        Ok(out)
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::unsupported(
            FORMAT_ID,
            "image planes",
            "ABF files hold sampled signals, not images: use `openreadout trace FILE --sweep N`, `openreadout export FILE --to csv`, or the openreadout_trace MCP tool.",
        ))
    }

    fn read_trace(
        &mut self,
        index: u32,
        sweep: u32,
        first_sample: u64,
        max_samples: u64,
    ) -> Result<Trace> {
        if index == 1 && !self.command.outputs.is_empty() {
            return self.read_command(sweep, first_sample, max_samples);
        }
        if index != 0 {
            return Err(Error::Usage(format!(
                "trace index {index} out of range (this file holds trace 0{})",
                if self.command.outputs.is_empty() {
                    ""
                } else {
                    " and the command trace 1"
                }
            )));
        }
        let (s, nch, width, fmt, data_offset, file_len) = {
            let f = &self.file;
            let s = f.sweeps.get(sweep as usize).cloned().ok_or_else(|| {
                Error::Usage(format!(
                    "sweep {sweep} out of range (file has {} sweeps, 0..{})",
                    f.sweeps.len(),
                    f.sweeps.len()
                ))
            })?;
            (
                s,
                f.channels.len() as u64,
                f.sample_format.width(),
                f.sample_format,
                f.data_offset,
                f.file_len,
            )
        };
        if first_sample > s.sample_count {
            return Err(Error::Usage(format!(
                "first sample {first_sample} is past the end of sweep {sweep} ({} samples)",
                s.sample_count
            )));
        }
        let n = max_samples
            .min(s.sample_count - first_sample)
            .min(MAX_READ_SAMPLES);
        let mut trace = Trace {
            trace: 0,
            sweep,
            first_sample,
            channels: vec![Vec::new(); nch as usize],
        };
        if n == 0 {
            return Ok(trace);
        }
        let row = nch * width;
        let begin = s
            .first_sample
            .checked_add(first_sample)
            .and_then(|x| x.checked_mul(row))
            .and_then(|x| x.checked_add(data_offset))
            .ok_or_else(|| Error::corrupt(FORMAT_ID, "sample offset overflow"))?;
        let len = n
            .checked_mul(row)
            .ok_or_else(|| Error::corrupt(FORMAT_ID, "sample length overflow"))?;
        if begin.saturating_add(len) > file_len {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                begin,
                format!(
                    "sweep {sweep} samples {first_sample}..{} need bytes up to {} but the file has {file_len} bytes (truncated)",
                    first_sample + n,
                    begin + len
                ),
            ));
        }
        let path = self.path.clone();
        let block = read_block(self.handle()?, &path, begin, len, file_len)?;
        let bytes = block.bytes;
        let chans: Vec<(f64, f64)> = self
            .file
            .channels
            .iter()
            .map(|c| (c.scale, c.offset))
            .collect();
        for (c, col) in trace.channels.iter_mut().enumerate() {
            col.reserve(n as usize);
            let (scale, offset) = chans[c];
            for r in 0..n as usize {
                let at = (r * nch as usize + c) * width as usize;
                let v = match fmt {
                    SampleFormat::Int16 => {
                        f64::from(i16::from_le_bytes([bytes[at], bytes[at + 1]])) * scale + offset
                    }
                    SampleFormat::Float32 => f64::from(f32::from_le_bytes([
                        bytes[at],
                        bytes[at + 1],
                        bytes[at + 2],
                        bytes[at + 3],
                    ])),
                };
                col.push(v);
            }
        }
        Ok(trace)
    }

    fn check(&mut self) -> Result<CheckReport> {
        let f = &self.file;
        let mut r = CheckReport::new(self.path.display().to_string(), FORMAT_ID);
        r.performed(
            "signature (`ABF ` / `ABF2`), version, operation mode, sample format and interval",
        );
        r.performed(match f.generation {
            Generation::Abf2 => "ABF 2 section map: every section lies inside the file; required sections (protocol, ADC, data) present",
            Generation::Abf1 => "ABF 1 header length (2048 or 6144 bytes) inside the file; data, synch array and tag regions inside the file",
        });
        r.performed("data size: samples = channels × sweeps × samples per sweep (or the synch-array lengths), and the data region ends inside the file");
        r.performed(
            "per-channel scaling factors finite and non-zero; channel names and units present",
        );
        r.performed("tags fall inside the recording");
        for x in &f.findings {
            r.push(x.clone());
        }
        for s in &f.sections {
            if s.name == "header" || s.byte_len == 0 {
                continue;
            }
            if s.end() > f.file_len {
                let (code, what) = if s.offset >= f.file_len {
                    ("section_out_of_bounds", "starts past the end of the file")
                } else {
                    ("truncated", "is cut off by the end of the file")
                };
                r.push(
                    Finding::error(
                        code,
                        format!(
                            "section {} (bytes {}..{}) {what} ({} bytes)",
                            s.name,
                            s.offset,
                            s.end(),
                            f.file_len
                        ),
                    )
                    .at(s.offset),
                );
            }
        }
        if f.generation == Generation::Abf1 && f.data_offset < f.header_len {
            r.push(Finding::error(
                "bad_offset",
                format!(
                    "data starts at byte {} inside the {}-byte header",
                    f.data_offset, f.header_len
                ),
            ));
        }
        let data_end = f.data_offset.saturating_add(f.data_len());
        if data_end > f.file_len {
            let have = f.file_len.saturating_sub(f.data_offset) / f.sample_format.width();
            r.push(
                Finding::error(
                    "truncated",
                    format!(
                        "the header declares {} samples ({} bytes from byte {}) but the file ends at byte {}: {} samples ({:.1}%) are missing",
                        f.total_samples,
                        f.data_len(),
                        f.data_offset,
                        f.file_len,
                        f.total_samples.saturating_sub(have),
                        100.0 * f.total_samples.saturating_sub(have) as f64
                            / f.total_samples.max(1) as f64
                    ),
                )
                .at(f.file_len),
            );
        }
        let per_sweep: u64 = f.sweeps.iter().map(|s| s.sample_count).sum();
        if per_sweep * f.channels.len() as u64 != f.total_samples
            && !f.findings.iter().any(|x| {
                matches!(
                    x.code.as_str(),
                    "sweep_length_mismatch" | "synch_length_mismatch" | "partial_sample_group"
                )
            })
        {
            r.push(Finding::warning(
                "data_length_mismatch",
                format!(
                    "{} sweeps × {} channels cover {} of the {} samples in the data",
                    f.sweeps.len(),
                    f.channels.len(),
                    per_sweep * f.channels.len() as u64,
                    f.total_samples
                ),
            ));
        }
        if f.total_samples == 0 {
            r.push(Finding::warning("no_samples", "the data section is empty"));
        }
        if f.header_sweep_count > 0
            && !f.synch.is_empty()
            && f.synch.len() as i64 != f.header_sweep_count
            && f.mode != crate::file::AcquisitionMode::GapFree
        {
            r.push(Finding::info(
                "synch_count_mismatch",
                format!(
                    "the synch array has {} entries for {} sweeps",
                    f.synch.len(),
                    f.header_sweep_count
                ),
            ));
        }
        if f.sample_format == SampleFormat::Int16 {
            if !usable_factor(f.adc_range_v) || f.adc_resolution <= 0 {
                r.push(Finding::error(
                    "bad_scaling",
                    format!(
                        "ADC range {} V / resolution {} cannot scale samples",
                        f.adc_range_v, f.adc_resolution
                    ),
                ));
            }
            for c in &f.channels {
                let bad: Vec<&str> = [
                    ("instrument_scale", c.instrument_scale),
                    ("signal_gain", c.signal_gain),
                    ("programmable_gain", c.programmable_gain),
                ]
                .iter()
                .filter(|(_, v)| !usable_factor(*v))
                .map(|(n, _)| *n)
                .collect();
                if !bad.is_empty() || !c.scale.is_finite() || !c.offset.is_finite() {
                    r.push(Finding::warning(
                        "bad_scaling",
                        format!(
                            "channel {} ({}): {} unusable; scaled values are not finite",
                            c.index,
                            c.name,
                            if bad.is_empty() {
                                "offset".to_string()
                            } else {
                                bad.join(", ")
                            }
                        ),
                    ));
                }
            }
        }
        for c in &f.channels {
            if c.name.is_empty() {
                r.push(Finding::info(
                    "unnamed_channel",
                    format!(
                        "channel {} has no name (reported as ch{})",
                        c.index, c.index
                    ),
                ));
            }
        }
        let duration = if f.sample_rate_hz > 0.0 {
            f.sweeps.last().map_or(0.0, |s| {
                s.start_s.unwrap_or(0.0) + s.sample_count as f64 / f.sample_rate_hz
            })
        } else {
            0.0
        };
        for t in &f.tags {
            if t.time_s < 0.0 || (duration > 0.0 && t.time_s > duration * 1.01 + 1.0) {
                r.push(Finding::warning(
                    "tag_outside_recording",
                    format!(
                        "tag \"{}\" at {:.3} s lies outside the recording (0..{duration:.3} s)",
                        t.comment, t.time_s
                    ),
                ));
            }
        }
        Ok(r)
    }
}
