//! `Dataset`: one trace per stored signal kind (amplifier, auxiliary, supply, board ADC,
//! digital, ...), each a single sweep read out of the interleaved data blocks.

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

use crate::header::{
    BlockPart, Family, IntanChannel, IntanHeader, SignalKind, block_layout, parse_header, scaling,
    stim_steps,
};
use crate::{FORMAT_ID, IntanReader};

/// Largest header read.
pub const MAX_HEADER_LEN: u64 = 4 << 20;

/// An opened traditional-format Intan file.
#[derive(Debug)]
pub struct IntanDataset {
    path: PathBuf,
    header: IntanHeader,
    parts: Vec<BlockPart>,
    block_len: u64,
    file_len: u64,
    first_time_index: Option<i64>,
    handle: Option<SourceFile>,
    /// Where the file is read from.
    fs: Fs,
}

impl IntanDataset {
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    /// Open an [`Input`] (a local path, a buffer, a host source).
    pub(crate) fn open_input(input: &Input) -> Result<Self> {
        let (path, fs) = (input.path(), input.fs());
        let mut f = fs.open(path).map_err(|e| Error::io(path, e))?;
        let file_len = f.metadata().map_err(|e| Error::io(path, e))?.len();
        let b = read_block(&mut f, path, 0, MAX_HEADER_LEN, file_len)?;
        let header = parse_header(&b).map_err(|e| Error::corrupt(FORMAT_ID, e))?;
        if header.header_len > file_len {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                file_len,
                "the file ends inside its header",
            ));
        }
        let (parts, block_len) = block_layout(&header);
        let first_time_index = if file_len >= header.header_len + 4 {
            let t = read_block(&mut f, path, header.header_len, 4, file_len)?;
            if header.signed_timestamps() {
                t.i32_at(header.header_len).map(i64::from)
            } else {
                t.u32_at(header.header_len).map(i64::from)
            }
        } else {
            None
        };
        Ok(IntanDataset {
            path: path.to_path_buf(),
            header,
            parts,
            block_len,
            file_len,
            first_time_index,
            handle: Some(f),
            fs: fs.clone(),
        })
    }

    /// The parsed header.
    pub fn header(&self) -> &IntanHeader {
        &self.header
    }

    fn blocks(&self) -> u64 {
        self.file_len.saturating_sub(self.header.header_len) / self.block_len.max(1)
    }

    /// Logical channels of a part: enabled channels, or one per enabled digital line.
    fn logical(&self, p: &BlockPart) -> Vec<IntanChannel> {
        match p.kind {
            SignalKind::DcAmplifier | SignalKind::Stimulation => self
                .header
                .enabled(SignalKind::Amplifier)
                .into_iter()
                .cloned()
                .collect(),
            SignalKind::Temperature => (1..=p.channels)
                .map(|i| IntanChannel {
                    native_name: format!("T{i}"),
                    custom_name: String::new(),
                    native_order: i as i16 - 1,
                    custom_order: i as i16 - 1,
                    signal_code: -1,
                    enabled: true,
                    chip_channel: -1,
                    command_stream: None,
                    board_stream: -1,
                    impedance_ohm: 0.0,
                    impedance_phase_deg: 0.0,
                    group: "temperature".into(),
                })
                .collect(),
            k => self.header.enabled(k).into_iter().cloned().collect(),
        }
    }

    fn trace_info(&self, index: usize, p: &BlockPart) -> TraceInfo {
        let fs = f64::from(self.header.sample_rate_hz);
        let n = self.header.block_samples() as f64;
        let rate = fs * p.samples as f64 / n;
        let (scale, offset, unit) = scaling(&self.header, p.kind);
        let bits = matches!(p.kind, SignalKind::DigitalIn | SignalKind::DigitalOut);
        let channels = self
            .logical(p)
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let mut e = BTreeMap::new();
                e.insert("native_name".into(), json!(c.native_name));
                if !c.custom_name.is_empty() {
                    e.insert("custom_name".into(), json!(c.custom_name));
                }
                e.insert("native_order".into(), json!(c.native_order));
                if c.chip_channel >= 0 {
                    e.insert("chip_channel".into(), json!(c.chip_channel));
                    e.insert("board_stream".into(), json!(c.board_stream));
                }
                if let Some(s) = c.command_stream {
                    e.insert("command_stream".into(), json!(s));
                }
                if p.kind == SignalKind::Amplifier {
                    e.insert("impedance_ohm".into(), json!(c.impedance_ohm));
                    e.insert("impedance_phase_deg".into(), json!(c.impedance_phase_deg));
                }
                if bits {
                    e.insert("bit".into(), json!(c.native_order));
                }
                if p.kind == SignalKind::Stimulation {
                    e.insert("decoding".into(), json!("bits 0-7 magnitude, bit 8 sign; value = ±magnitude × stim step (A); settle/charge-recovery/compliance bits are not returned"));
                }
                e.insert("group".into(), json!(c.group));
                SignalChannelInfo {
                    index: i as u32,
                    name: if c.custom_name.is_empty() { c.native_name.clone() } else { c.custom_name.clone() },
                    unit: unit.map(str::to_string),
                    dtype: if p.kind == SignalKind::Temperature { "int16".into() } else { "uint16".into() },
                    scale,
                    offset,
                    extra: e,
                }
            })
            .collect();
        let mut extra = BTreeMap::new();
        extra.insert("signal".into(), json!(p.kind.name()));
        extra.insert("samples_per_block".into(), json!(p.samples));
        if let Some(t) = self.first_time_index {
            extra.insert("first_time_index".into(), json!(t));
        }
        TraceInfo {
            index: index as u32,
            name: Some(p.kind.name().into()),
            sample_rate_hz: rate,
            sample_count: self.blocks() * p.samples,
            sweep_count: 1,
            channels,
            start_s: self
                .first_time_index
                .filter(|_| fs > 0.0)
                .map(|t| t as f64 / fs),
            extra,
        }
    }
}

impl Dataset for IntanDataset {
    fn info(&self) -> Result<FileInfo> {
        let h = &self.header;
        let traces: Vec<TraceInfo> = self
            .parts
            .iter()
            .enumerate()
            .map(|(i, p)| self.trace_info(i, p))
            .collect();
        let mut notes = vec![format!(
            "{} data blocks of {} samples; one trace per signal kind ({}), each a single sweep",
            self.blocks(),
            h.block_samples(),
            self.parts
                .iter()
                .map(|p| p.kind.name())
                .collect::<Vec<_>>()
                .join(", ")
        )];
        let rest = self.file_len.saturating_sub(h.header_len) % self.block_len.max(1);
        if rest != 0 {
            notes.push(format!(
                "{rest} bytes after the last whole data block (truncated); run `check`"
            ));
        }
        let mut t0 = traces;
        if let Some(first) = t0.first_mut() {
            let e = &mut first.extra;
            e.insert(
                "family".into(),
                json!(match h.family {
                    Family::Rhd => "rhd2000",
                    Family::Rhs => "rhs2000",
                }),
            );
            e.insert("board_mode".into(), json!(h.board_mode));
            e.insert("dsp_enabled".into(), json!(h.dsp_enabled));
            e.insert("dsp_cutoff_hz".into(), json!(h.dsp_cutoff_hz));
            e.insert(
                "bandwidth_hz".into(),
                json!([h.lower_bandwidth_hz, h.upper_bandwidth_hz]),
            );
            if let Some(v) = h.lower_settle_bandwidth_hz {
                e.insert("lower_settle_bandwidth_hz".into(), json!(v));
            }
            e.insert(
                "notch".into(),
                json!(match h.notch_mode {
                    1 => "50 Hz",
                    2 => "60 Hz",
                    _ => "off",
                }),
            );
            e.insert("impedance_test_hz".into(), json!(h.impedance_test_hz));
            if let Some(r) = h.reference.as_ref().filter(|r| !r.is_empty()) {
                e.insert("reference".into(), json!(r));
            }
            let notes: Vec<&String> = h.notes.iter().filter(|n| !n.is_empty()).collect();
            if !notes.is_empty() {
                e.insert("notes".into(), json!(notes));
            }
            if let Some(v) = h.stim_step_a {
                e.insert("stim_step_a".into(), json!(v));
            }
            if let Some(v) = h.charge_recovery_limit_a {
                e.insert("charge_recovery_limit_a".into(), json!(v));
            }
            if let Some(v) = h.charge_recovery_target_v {
                e.insert("charge_recovery_target_v".into(), json!(v));
            }
            if let Some(v) = h.amp_settle_mode {
                e.insert("amp_settle_mode".into(), json!(v));
            }
            if let Some(v) = h.charge_recovery_mode {
                e.insert("charge_recovery_mode".into(), json!(v));
            }
        }
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.file_len,
            format: IntanReader.descriptor(),
            format_version: Some(format!("{}.{}", h.version.0, h.version.1)),
            images: Vec::new(),
            tables: Vec::new(),
            spectra: Vec::new(),
            traces: t0,
            plane_count: 0,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let h = &self.header;
        Ok(json!({
            "family": match h.family { Family::Rhd => "rhd2000", Family::Rhs => "rhs2000" },
            "version": [h.version.0, h.version.1],
            "sample_rate_hz": h.sample_rate_hz,
            "header_len": h.header_len,
            "block_len": self.block_len,
            "block_samples": h.block_samples(),
            "temperature_sensors": h.temperature_sensors,
            "dc_saved": h.dc_saved,
            "notes": h.notes,
            "channels": h.channels.iter().map(|c| json!({
                "native_name": c.native_name, "custom_name": c.custom_name, "native_order": c.native_order,
                "custom_order": c.custom_order, "signal_code": c.signal_code, "enabled": c.enabled,
                "chip_channel": c.chip_channel, "command_stream": c.command_stream, "board_stream": c.board_stream,
                "impedance_ohm": c.impedance_ohm, "impedance_phase_deg": c.impedance_phase_deg, "group": c.group,
            })).collect::<Vec<_>>(),
            "block_parts": self.parts.iter().map(|p| json!({"signal": p.kind.name(), "offset": p.offset, "channels": p.channels, "samples": p.samples})).collect::<Vec<_>>(),
        }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for (k, s) in [
            ("traces[].sample_rate_hz", Source::VendorImpl),
            ("traces[].channels[].scale", Source::VendorImpl),
            ("traces[].channels[].offset", Source::VendorImpl),
            ("traces[].channels[].name", Source::VendorImpl),
            ("traces[].sample_count", Source::VendorImpl),
            ("traces[digital_out, rhd]", Source::PriorArt),
            (
                "traces[].extra.samples_per_block[rhd version rule]",
                Source::PriorArt,
            ),
        ] {
            p.insert(k.into(), s);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let mut out = vec![LsEntry {
            kind: "header".into(),
            name: "Intan header".into(),
            offset: Some(0),
            size: Some(self.header.header_len),
            image: None,
            details: json!({"channels": self.header.channels.len()}),
        }];
        out.push(LsEntry {
            kind: "records".into(),
            name: "data blocks".into(),
            offset: Some(self.header.header_len),
            size: Some(self.blocks() * self.block_len),
            image: None,
            details: json!({"blocks": self.blocks(), "block_len": self.block_len, "parts": self.parts.iter().map(|p| json!({"signal": p.kind.name(), "offset": p.offset, "channels": p.channels, "samples": p.samples})).collect::<Vec<_>>()}),
        });
        Ok(out)
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::unsupported(
            FORMAT_ID,
            "image planes",
            "Intan files hold sampled signals: use `openreadout trace` or `openreadout export --to csv`.",
        ))
    }

    fn read_trace(
        &mut self,
        index: u32,
        sweep: u32,
        first_sample: u64,
        max_samples: u64,
    ) -> Result<Trace> {
        let part = self.parts.get(index as usize).cloned().ok_or_else(|| {
            Error::Usage(format!(
                "trace {index} out of range (file has {} traces)",
                self.parts.len()
            ))
        })?;
        if sweep != 0 {
            return Err(Error::Usage(format!(
                "sweep {sweep} out of range (Intan traces have one sweep)"
            )));
        }
        let total = self.blocks() * part.samples;
        if first_sample > total {
            return Err(Error::Usage(format!(
                "first sample {first_sample} is past the end ({total} samples)"
            )));
        }
        let count = max_samples.min(total - first_sample).min(1 << 24);
        let logical = self.logical(&part);
        let (scale, offset, _) = scaling(&self.header, part.kind);
        let mut out: Vec<Vec<f64>> = vec![Vec::with_capacity(count as usize); logical.len()];
        let (hl, bl) = (self.header.header_len, self.block_len);
        let (path, file_len) = (self.path.clone(), self.file_len);
        if self.handle.is_none() {
            self.handle = Some(self.fs.open(&path).map_err(|e| Error::io(&path, e))?);
        }
        let mut sample = first_sample;
        let end = first_sample + count;
        while sample < end {
            let b0 = sample / part.samples;
            let b1 = ((end - 1) / part.samples).min(b0 + (32 << 20) / bl.max(1));
            let at = hl + b0 * bl;
            let f = self.handle.as_mut().expect("opened above");
            let data = read_block(f, &path, at, (b1 - b0 + 1) * bl, file_len)?;
            if (data.len() as u64) < (b1 - b0 + 1) * bl {
                return Err(Error::corrupt_at(
                    FORMAT_ID,
                    at,
                    "data blocks cut off (truncated)",
                ));
            }
            for block in b0..=b1 {
                let lo = if block == b0 {
                    sample - block * part.samples
                } else {
                    0
                };
                let hi = ((end - block * part.samples).min(part.samples)).max(lo);
                let base = at + (block - b0) * bl + part.offset;
                for (c, ch) in logical.iter().enumerate() {
                    for i in lo..hi {
                        let v = match part.kind {
                            SignalKind::DigitalIn | SignalKind::DigitalOut => {
                                let word = data.u16_at(base + 2 * i).unwrap_or(0);
                                f64::from((word >> (ch.native_order.clamp(0, 15) as u16)) & 1)
                            }
                            SignalKind::Temperature => {
                                f64::from(
                                    data.i16_at(base + 2 * (c as u64 * part.samples + i))
                                        .unwrap_or(0),
                                ) * scale
                                    + offset
                            }
                            SignalKind::Stimulation => {
                                stim_steps(
                                    data.u16_at(base + 2 * (c as u64 * part.samples + i))
                                        .unwrap_or(0),
                                ) * scale
                            }
                            _ => {
                                f64::from(
                                    data.u16_at(base + 2 * (c as u64 * part.samples + i))
                                        .unwrap_or(0),
                                ) * scale
                                    + offset
                            }
                        };
                        out[c].push(v);
                    }
                }
            }
            sample = (b1 + 1) * part.samples;
        }
        Ok(Trace {
            trace: index,
            sweep: 0,
            first_sample,
            channels: out,
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), FORMAT_ID);
        r.performed(
            "magic number, header (Qt strings, signal groups, channel records) parsed to its end",
        );
        r.performed(
            "data length is a whole number of data blocks (block size from the enabled channels)",
        );
        r.performed("time indices increase by exactly one sample within and across blocks");
        let (hl, bl) = (self.header.header_len, self.block_len);
        let data = self.file_len - hl;
        if !data.is_multiple_of(bl) {
            r.push(
                Finding::error(
                    "truncated",
                    format!(
                        "{} bytes after the last whole {bl}-byte data block (a block is cut off)",
                        data % bl
                    ),
                )
                .at(hl + (data / bl) * bl),
            );
        }
        if self.header.sample_rate_hz <= 0.0 || !self.header.sample_rate_hz.is_finite() {
            r.push(Finding::error(
                "bad_rate",
                format!("sample rate {}", self.header.sample_rate_hz),
            ));
        }
        let per_block = self.header.block_samples();
        let blocks = self.blocks();
        let signed = self.header.signed_timestamps();
        let (path, file_len) = (self.path.clone(), self.file_len);
        let mut prev: Option<i64> = None;
        let mut gaps = 0u64;
        let mut block = 0;
        while block < blocks {
            let count = (blocks - block).min(((16 << 20) / bl).max(1));
            let at = hl + block * bl;
            if self.handle.is_none() {
                self.handle = Some(self.fs.open(&path).map_err(|e| Error::io(&path, e))?);
            }
            let chunk = read_block(
                self.handle.as_mut().expect("opened"),
                &path,
                at,
                count * bl,
                file_len,
            )?;
            for j in 0..count {
                for i in 0..per_block {
                    let pos = at + j * bl + 4 * i;
                    let time = if signed {
                        chunk.i32_at(pos).map(i64::from)
                    } else {
                        chunk.u32_at(pos).map(i64::from)
                    }
                    .unwrap_or(0);
                    if prev.is_some_and(|last| time != last + 1) {
                        gaps += 1;
                    }
                    prev = Some(time);
                }
            }
            block += count;
        }
        if gaps > 0 {
            r.push(Finding::warning("time_index_gap", format!("{gaps} time indices do not follow the previous one (dropped samples or a trigger restart)")));
        }
        if blocks == 0 {
            r.push(Finding::warning(
                "no_samples",
                "the file holds no whole data block",
            ));
        }
        if self.header.family == Family::Rhd
            && !matches!(self.header.board_mode, 0 | 1 | 13)
            && !self.header.enabled(SignalKind::BoardAdc).is_empty()
        {
            r.push(Finding::warning(
                "unknown_board_mode",
                format!(
                    "board mode {}: board ADC values are raw counts",
                    self.header.board_mode
                ),
            ));
        }
        Ok(r)
    }
}
