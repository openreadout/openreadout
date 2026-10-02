//! Header-only `info.rhd` / `info.rhs` plus raw `.dat` files: the "one file per signal type" and
//! "one file per channel" layouts (see `docs/formats/intan.md`). One trace per signal kind, as for
//! traditional files; every stream is sampled at the header rate (auxiliary and supply inputs are
//! written at full rate in these layouts), each a single sweep.

use openreadout_core::source::{Fs, Input};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_core::bytes::read_block;
use openreadout_core::model::{
    CheckReport, FileInfo, Finding, LsEntry, SignalChannelInfo, Trace, TraceInfo,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::{Error, Plane, Result};
use serde_json::{Value, json};

use crate::dataset::MAX_HEADER_LEN;
use crate::header::{
    Family, IntanChannel, IntanHeader, SignalKind, parse_header, scaling, stim_steps,
};
use crate::{FORMAT_ID, IntanReader};

/// How the samples are laid out on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitLayout {
    /// One `.dat` per signal kind, channels interleaved sample by sample.
    PerSignalType,
    /// One `.dat` per channel.
    PerChannel,
}

impl SplitLayout {
    /// Our name for the layout.
    pub fn name(self) -> &'static str {
        match self {
            SplitLayout::PerSignalType => "one_file_per_signal_type",
            SplitLayout::PerChannel => "one_file_per_channel",
        }
    }
}

/// Where one stream's samples are.
#[derive(Debug, Clone)]
pub enum StreamFiles {
    /// One file, `words` little-endian 16-bit words per sample (channels interleaved; a single
    /// word holding every digital line for digital streams).
    Interleaved { path: PathBuf, words: u64, len: u64 },
    /// One file per channel, each one 16-bit word per sample.
    PerChannel(Vec<(PathBuf, u64)>),
}

/// One signal kind (one trace).
#[derive(Debug, Clone)]
pub struct SplitStream {
    /// The signal kind.
    pub kind: SignalKind,
    /// The channels, in header order.
    pub channels: Vec<IntanChannel>,
    /// The files holding them.
    pub files: StreamFiles,
    /// Whole samples per channel (the shortest channel file).
    pub samples: u64,
}

/// Data file names of the one-file-per-signal-type layout.
pub fn signal_file(family: Family, kind: SignalKind) -> Option<&'static str> {
    Some(match (family, kind) {
        (_, SignalKind::Amplifier) => "amplifier.dat",
        (Family::Rhs, SignalKind::DcAmplifier) => "dcamplifier.dat",
        (Family::Rhs, SignalKind::Stimulation) => "stim.dat",
        (_, SignalKind::Auxiliary) => "auxiliary.dat",
        (_, SignalKind::Supply) => "supply.dat",
        (_, SignalKind::BoardAdc) => "analogin.dat",
        (_, SignalKind::BoardDac) => "analogout.dat",
        (_, SignalKind::DigitalIn) => "digitalin.dat",
        (_, SignalKind::DigitalOut) => "digitalout.dat",
        _ => return None,
    })
}

/// File name of one channel in the one-file-per-channel layout.
pub fn channel_file(kind: SignalKind, native_name: &str) -> Option<String> {
    let prefix = match kind {
        SignalKind::Amplifier => "amp",
        SignalKind::DcAmplifier => "dc",
        SignalKind::Stimulation => "stim",
        SignalKind::Auxiliary => "aux",
        SignalKind::Supply => "vdd",
        SignalKind::BoardAdc
        | SignalKind::BoardDac
        | SignalKind::DigitalIn
        | SignalKind::DigitalOut => "board",
        SignalKind::Temperature => return None,
    };
    Some(format!("{prefix}-{native_name}.dat"))
}

/// Signal kinds in trace order (the order of the traditional data block).
fn kinds(family: Family) -> &'static [SignalKind] {
    match family {
        Family::Rhd => &[
            SignalKind::Amplifier,
            SignalKind::Auxiliary,
            SignalKind::Supply,
            SignalKind::BoardAdc,
            SignalKind::DigitalIn,
            SignalKind::DigitalOut,
        ],
        Family::Rhs => &[
            SignalKind::Amplifier,
            SignalKind::DcAmplifier,
            SignalKind::Stimulation,
            SignalKind::BoardAdc,
            SignalKind::BoardDac,
            SignalKind::DigitalIn,
            SignalKind::DigitalOut,
        ],
    }
}

/// The header file of a split recording in `dir` (`info.rhd` or `info.rhs`), if any.
pub fn info_file(dir: &Path) -> Option<PathBuf> {
    info_file_in(&Fs::local(), dir)
}

pub(crate) fn info_file_in(fs: &Fs, dir: &Path) -> Option<PathBuf> {
    ["info.rhd", "info.rhs", "info.RHD", "info.RHS"]
        .iter()
        .map(|n| dir.join(n))
        .find(|p| fs.is_file(p))
}

/// `(scale, offset, unit, stored dtype)` for a split-layout sample of `kind`. The amplifier files
/// hold signed words centred on zero; every other kind is stored as in the traditional file.
pub fn split_scaling(
    h: &IntanHeader,
    kind: SignalKind,
) -> (f64, f64, Option<&'static str>, &'static str) {
    match kind {
        SignalKind::Amplifier => (0.195, 0.0, Some("µV"), "int16"),
        k => {
            let (scale, offset, unit) = scaling(h, k);
            (scale, offset, unit, "uint16")
        }
    }
}

/// An opened split-layout Intan recording.
#[derive(Debug)]
pub struct SplitDataset {
    path: PathBuf,
    fs: Fs,
    dir: PathBuf,
    info_path: PathBuf,
    header: IntanHeader,
    layout: SplitLayout,
    streams: Vec<SplitStream>,
    time: Option<(PathBuf, u64)>,
    first_time_index: Option<i64>,
    /// Header channels whose data file is absent (reported by `check`).
    missing: Vec<String>,
}

fn file_len(fs: &Fs, p: &Path) -> Option<u64> {
    fs.metadata(p)
        .ok()
        .filter(openreadout_core::source::EntryMeta::is_file)
        .map(|m| m.len())
}

impl SplitDataset {
    /// Open the split recording in `dir` (or whose `info.rhd` / `info.rhs` is `path`).
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    pub(crate) fn open_input(input: &Input) -> Result<Self> {
        let path = input.path();
        let fs = input.fs();
        let (dir, info_path) = if fs.is_dir(path) {
            let info = info_file_in(fs, path).ok_or_else(|| {
                Error::unsupported(
                    FORMAT_ID,
                    "an Intan directory without info.rhd / info.rhs",
                    "The one-file-per-signal-type and one-file-per-channel layouts keep the header in info.rhd (or info.rhs) next to the .dat files.",
                )
            })?;
            (path.to_path_buf(), info)
        } else {
            (
                path.parent()
                    .map_or_else(|| PathBuf::from("."), Path::to_path_buf),
                path.to_path_buf(),
            )
        };
        let mut info = fs.open(&info_path).map_err(|e| Error::io(&info_path, e))?;
        let len = info.metadata().map_err(|e| Error::io(&info_path, e))?.len();
        let head = read_block(&mut info, &info_path, 0, MAX_HEADER_LEN, len)?;
        let header = parse_header(&head).map_err(|e| Error::corrupt(FORMAT_ID, e))?;
        if header.header_len > len {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                len,
                "info file ends inside its header",
            ));
        }
        let per_signal = kinds(header.family)
            .iter()
            .any(|k| signal_file(header.family, *k).is_some_and(|n| fs.is_file(&dir.join(n))));
        let layout = if per_signal {
            SplitLayout::PerSignalType
        } else {
            SplitLayout::PerChannel
        };
        let mut streams = Vec::new();
        let mut missing = Vec::new();
        for &kind in kinds(header.family) {
            let listed: Vec<IntanChannel> = match kind {
                SignalKind::DcAmplifier | SignalKind::Stimulation => header
                    .enabled(SignalKind::Amplifier)
                    .into_iter()
                    .cloned()
                    .collect(),
                k => header.enabled(k).into_iter().cloned().collect(),
            };
            if listed.is_empty() {
                continue;
            }
            let digital = matches!(kind, SignalKind::DigitalIn | SignalKind::DigitalOut);
            match layout {
                SplitLayout::PerSignalType => {
                    let Some(name) = signal_file(header.family, kind) else {
                        continue;
                    };
                    let file = dir.join(name);
                    let Some(len) = file_len(fs, &file) else {
                        if !matches!(kind, SignalKind::DcAmplifier | SignalKind::Stimulation) {
                            missing.push(name.to_string());
                        }
                        continue;
                    };
                    let words = if digital { 1 } else { listed.len() as u64 };
                    streams.push(SplitStream {
                        kind,
                        channels: listed,
                        samples: len / (2 * words),
                        files: StreamFiles::Interleaved {
                            path: file,
                            words,
                            len,
                        },
                    });
                }
                SplitLayout::PerChannel => {
                    let mut channels = Vec::new();
                    let mut files = Vec::new();
                    for channel in listed {
                        let Some(name) = channel_file(kind, &channel.native_name) else {
                            continue;
                        };
                        let file = dir.join(&name);
                        match file_len(fs, &file) {
                            Some(len) => {
                                files.push((file, len));
                                channels.push(channel);
                            }
                            None if !matches!(
                                kind,
                                SignalKind::DcAmplifier | SignalKind::Stimulation
                            ) =>
                            {
                                missing.push(name);
                            }
                            None => {}
                        }
                    }
                    if channels.is_empty() {
                        continue;
                    }
                    let samples = files.iter().map(|(_, n)| n / 2).min().unwrap_or(0);
                    streams.push(SplitStream {
                        kind,
                        channels,
                        samples,
                        files: StreamFiles::PerChannel(files),
                    });
                }
            }
        }
        let tp = dir.join("time.dat");
        let time = file_len(fs, &tp).map(|n| (tp, n));
        let first_time_index = match &time {
            Some((time_path, time_len)) if *time_len >= 4 => {
                let mut tf = fs.open(time_path).map_err(|e| Error::io(time_path, e))?;
                let stamp = read_block(&mut tf, time_path, 0, 4, *time_len)?;
                if header.signed_timestamps() {
                    stamp.i32_at(0).map(i64::from)
                } else {
                    stamp.u32_at(0).map(i64::from)
                }
            }
            _ => None,
        };
        if streams.is_empty() && time.is_none() {
            return Err(Error::unsupported(
                FORMAT_ID,
                "an info.rhd / info.rhs without data files",
                "Keep the .dat files (time.dat, amplifier.dat or amp-*.dat, ...) next to the header file.",
            ));
        }
        Ok(SplitDataset {
            path: path.to_path_buf(),
            fs: fs.clone(),
            dir,
            info_path,
            header,
            layout,
            streams,
            time,
            first_time_index,
            missing,
        })
    }

    /// The parsed header.
    pub fn header(&self) -> &IntanHeader {
        &self.header
    }

    /// The layout.
    pub fn layout(&self) -> SplitLayout {
        self.layout
    }

    fn time_samples(&self) -> Option<u64> {
        self.time.as_ref().map(|(_, n)| n / 4)
    }

    fn rate(&self, s: &SplitStream) -> f64 {
        let fs = f64::from(self.header.sample_rate_hz);
        match self.time_samples() {
            Some(t) if t > 0 && s.samples != t && s.samples > 0 => fs * s.samples as f64 / t as f64,
            _ => fs,
        }
    }

    fn rel(&self, p: &Path) -> String {
        p.strip_prefix(&self.dir)
            .map_or_else(|_| p.display().to_string(), |r| r.display().to_string())
    }

    fn trace_info(&self, index: usize, s: &SplitStream) -> TraceInfo {
        let (scale, offset, unit, dtype) = split_scaling(&self.header, s.kind);
        let bits = matches!(s.kind, SignalKind::DigitalIn | SignalKind::DigitalOut);
        let channels = s
            .channels
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
                if s.kind == SignalKind::Amplifier {
                    e.insert("impedance_ohm".into(), json!(c.impedance_ohm));
                    e.insert("impedance_phase_deg".into(), json!(c.impedance_phase_deg));
                }
                if bits && self.layout == SplitLayout::PerSignalType {
                    e.insert("bit".into(), json!(c.native_order));
                }
                if let StreamFiles::PerChannel(files) = &s.files
                    && let Some((p, _)) = files.get(i)
                {
                    e.insert("data_file".into(), json!(self.rel(p)));
                }
                e.insert("group".into(), json!(c.group));
                SignalChannelInfo {
                    index: i as u32,
                    name: if c.custom_name.is_empty() {
                        c.native_name.clone()
                    } else {
                        c.custom_name.clone()
                    },
                    unit: unit.map(str::to_string),
                    dtype: dtype.into(),
                    scale,
                    offset,
                    extra: e,
                }
            })
            .collect();
        let mut extra = BTreeMap::new();
        extra.insert("signal".into(), json!(s.kind.name()));
        extra.insert("layout".into(), json!(self.layout.name()));
        if let StreamFiles::Interleaved { path, .. } = &s.files {
            extra.insert("data_file".into(), json!(self.rel(path)));
        }
        if let Some(t) = self.first_time_index {
            extra.insert("first_time_index".into(), json!(t));
        }
        let fs = f64::from(self.header.sample_rate_hz);
        TraceInfo {
            index: index as u32,
            name: Some(s.kind.name().into()),
            sample_rate_hz: self.rate(s),
            sample_count: s.samples,
            sweep_count: 1,
            channels,
            start_s: self
                .first_time_index
                .filter(|_| fs > 0.0)
                .map(|t| t as f64 / fs),
            extra,
        }
    }

    fn decode(
        kind: SignalKind,
        ch: &IntanChannel,
        word: u16,
        scale: f64,
        offset: f64,
        per_signal: bool,
    ) -> f64 {
        match kind {
            SignalKind::Amplifier => f64::from(word.cast_signed()) * scale + offset,
            SignalKind::DigitalIn | SignalKind::DigitalOut if per_signal => {
                f64::from((word >> (ch.native_order.clamp(0, 15).cast_unsigned())) & 1)
            }
            SignalKind::Stimulation => stim_steps(word) * scale,
            _ => f64::from(word) * scale + offset,
        }
    }
}

impl Dataset for SplitDataset {
    fn info(&self) -> Result<FileInfo> {
        let h = &self.header;
        let mut traces: Vec<TraceInfo> = self
            .streams
            .iter()
            .enumerate()
            .map(|(i, s)| self.trace_info(i, s))
            .collect();
        if let Some(first) = traces.first_mut() {
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
            e.insert(
                "notch".into(),
                json!(match h.notch_mode {
                    1 => "50 Hz",
                    2 => "60 Hz",
                    _ => "off",
                }),
            );
            if let Some(r) = h.reference.as_ref().filter(|r| !r.is_empty()) {
                e.insert("reference".into(), json!(r));
            }
            if let Some(v) = h.stim_step_a {
                e.insert("stim_step_a".into(), json!(v));
            }
        }
        let size: u64 = std::iter::once(file_len(&self.fs, &self.info_path).unwrap_or(0))
            .chain(self.time.iter().map(|(_, n)| *n))
            .chain(self.streams.iter().flat_map(|s| match &s.files {
                StreamFiles::Interleaved { len, .. } => vec![*len],
                StreamFiles::PerChannel(v) => v.iter().map(|(_, n)| *n).collect(),
            }))
            .sum();
        let mut notes = vec![format!(
            "{} layout: header in {}, samples in .dat files; one trace per signal kind ({}), each a single sweep",
            self.layout.name().replace('_', " "),
            self.rel(&self.info_path),
            self.streams
                .iter()
                .map(|s| s.kind.name())
                .collect::<Vec<_>>()
                .join(", ")
        )];
        if let Some(t) = self.time_samples()
            && self.streams.iter().any(|s| s.samples != t)
        {
            notes.push(format!(
                "time.dat holds {t} time indices but some streams hold a different number of samples; run `check`"
            ));
        }
        if !self.missing.is_empty() {
            notes.push(format!(
                "{} channel file(s) named by the header are absent: {}",
                self.missing.len(),
                self.missing
                    .iter()
                    .take(8)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: size,
            format: IntanReader.descriptor(),
            format_version: Some(format!("{}.{}", h.version.0, h.version.1)),
            images: Vec::new(),
            tables: Vec::new(),
            spectra: Vec::new(),
            traces,
            plane_count: 0,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let h = &self.header;
        Ok(json!({
            "layout": self.layout.name(),
            "info_file": self.rel(&self.info_path),
            "family": match h.family { Family::Rhd => "rhd2000", Family::Rhs => "rhs2000" },
            "version": [h.version.0, h.version.1],
            "sample_rate_hz": h.sample_rate_hz,
            "header_len": h.header_len,
            "dc_saved": h.dc_saved,
            "notes": h.notes,
            "time_indices": self.time_samples(),
            "channels": h.channels.iter().map(|c| json!({
                "native_name": c.native_name, "custom_name": c.custom_name, "native_order": c.native_order,
                "custom_order": c.custom_order, "signal_code": c.signal_code, "enabled": c.enabled,
                "chip_channel": c.chip_channel, "command_stream": c.command_stream, "board_stream": c.board_stream,
                "impedance_ohm": c.impedance_ohm, "impedance_phase_deg": c.impedance_phase_deg, "group": c.group,
            })).collect::<Vec<_>>(),
            "streams": self.streams.iter().map(|s| json!({
                "signal": s.kind.name(), "channels": s.channels.len(), "samples": s.samples,
            })).collect::<Vec<_>>(),
            "missing_files": self.missing,
        }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for (k, s) in [
            ("traces[].sample_rate_hz", Source::VendorImpl),
            ("traces[].channels[].scale", Source::VendorImpl),
            ("traces[].channels[].offset", Source::VendorImpl),
            ("traces[].channels[].name", Source::VendorImpl),
            ("traces[].sample_count", Source::Inferred),
            (
                "traces[auxiliary, supply].sample_rate_hz[split layouts]",
                Source::Inferred,
            ),
        ] {
            p.insert(k.into(), s);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let mut out = vec![LsEntry {
            kind: "header".into(),
            name: self.rel(&self.info_path),
            offset: Some(0),
            size: Some(self.header.header_len),
            image: None,
            details: json!({"channels": self.header.channels.len(), "layout": self.layout.name()}),
        }];
        if let Some((p, n)) = &self.time {
            out.push(LsEntry {
                kind: "file".into(),
                name: self.rel(p),
                offset: Some(0),
                size: Some(*n),
                image: None,
                details: json!({"time_indices": n / 4}),
            });
        }
        for s in &self.streams {
            match &s.files {
                StreamFiles::Interleaved { path, words, len } => out.push(LsEntry {
                    kind: "file".into(),
                    name: self.rel(path),
                    offset: Some(0),
                    size: Some(*len),
                    image: None,
                    details: json!({"signal": s.kind.name(), "words_per_sample": words, "samples": s.samples}),
                }),
                StreamFiles::PerChannel(files) => {
                    for ((p, n), c) in files.iter().zip(&s.channels) {
                        out.push(LsEntry {
                            kind: "file".into(),
                            name: self.rel(p),
                            offset: Some(0),
                            size: Some(*n),
                            image: None,
                            details: json!({"signal": s.kind.name(), "channel": c.native_name, "samples": n / 2}),
                        });
                    }
                }
            }
        }
        Ok(out)
    }

    fn member_files(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = Vec::new();
        if !self.fs.is_dir(&self.path) {
            v.extend(self.time.iter().map(|(p, _)| p.clone()));
            for s in &self.streams {
                match &s.files {
                    StreamFiles::Interleaved { path, .. } => v.push(path.clone()),
                    StreamFiles::PerChannel(f) => v.extend(f.iter().map(|(p, _)| p.clone())),
                }
            }
        }
        v
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::unsupported(
            FORMAT_ID,
            "image planes",
            "Intan recordings hold sampled signals: use `openreadout trace` or `openreadout export --to csv`.",
        ))
    }

    fn read_trace(
        &mut self,
        index: u32,
        sweep: u32,
        first_sample: u64,
        max_samples: u64,
    ) -> Result<Trace> {
        let stream = self.streams.get(index as usize).cloned().ok_or_else(|| {
            Error::Usage(format!(
                "trace {index} out of range (recording has {} traces)",
                self.streams.len()
            ))
        })?;
        if sweep != 0 {
            return Err(Error::Usage(format!(
                "sweep {sweep} out of range (Intan traces have one sweep)"
            )));
        }
        if first_sample > stream.samples {
            return Err(Error::Usage(format!(
                "first sample {first_sample} is past the end ({} samples)",
                stream.samples
            )));
        }
        let count = max_samples.min(stream.samples - first_sample).min(1 << 24);
        let (scale, offset, _, _) = split_scaling(&self.header, stream.kind);
        let per_signal = self.layout == SplitLayout::PerSignalType;
        let mut out: Vec<Vec<f64>> =
            vec![Vec::with_capacity(count as usize); stream.channels.len()];
        match &stream.files {
            StreamFiles::Interleaved { path, words, len } => {
                let mut f = self.fs.open(path).map_err(|e| Error::io(path, e))?;
                let row = 2 * words;
                let rows_per_chunk = ((16u64 << 20) / row).max(1);
                let mut done = 0;
                while done < count {
                    let rows = rows_per_chunk.min(count - done);
                    let at = (first_sample + done) * row;
                    let data = read_block(&mut f, path, at, rows * row, *len)?;
                    if (data.len() as u64) < rows * row {
                        return Err(Error::corrupt_at(
                            FORMAT_ID,
                            at,
                            "samples cut off (truncated .dat file)",
                        ));
                    }
                    for r in 0..rows {
                        let base = at + r * row;
                        for (c, ch) in stream.channels.iter().enumerate() {
                            let slot = if *words == 1 { 0 } else { c as u64 };
                            let word = data.u16_at(base + 2 * slot).ok_or_else(|| {
                                Error::corrupt_at(FORMAT_ID, base, "sample cut off")
                            })?;
                            out[c].push(Self::decode(
                                stream.kind,
                                ch,
                                word,
                                scale,
                                offset,
                                per_signal,
                            ));
                        }
                    }
                    done += rows;
                }
            }
            StreamFiles::PerChannel(files) => {
                for (c, ((path, len), ch)) in files.iter().zip(&stream.channels).enumerate() {
                    let mut f = self.fs.open(path).map_err(|e| Error::io(path, e))?;
                    let data = read_block(&mut f, path, first_sample * 2, count * 2, *len)?;
                    if (data.len() as u64) < count * 2 {
                        return Err(Error::corrupt_at(
                            FORMAT_ID,
                            first_sample * 2,
                            "samples cut off (truncated .dat file)",
                        ));
                    }
                    for i in 0..count {
                        let word = data
                            .u16_at(first_sample * 2 + 2 * i)
                            .ok_or_else(|| Error::corrupt(FORMAT_ID, "sample cut off"))?;
                        out[c].push(Self::decode(
                            stream.kind,
                            ch,
                            word,
                            scale,
                            offset,
                            per_signal,
                        ));
                    }
                }
            }
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
            "info file: magic number and header parsed to its end; nothing follows the header",
        );
        r.performed(
            "every .dat file named by the header exists and holds a whole number of samples",
        );
        r.performed("every stream holds as many samples as time.dat holds time indices");
        r.performed("time indices increase by exactly one sample");
        if file_len(&self.fs, &self.info_path).is_some_and(|n| n > self.header.header_len) {
            r.push(Finding::warning(
                "data_after_header",
                format!(
                    "{} holds data after its header; it looks like a traditional file, not a split-layout header",
                    self.rel(&self.info_path)
                ),
            ));
        }
        for name in &self.missing {
            r.push(Finding::warning(
                "missing_file",
                format!("{name}: named by the header but absent"),
            ));
        }
        let time_samples = self.time_samples();
        match &self.time {
            None => r.push(Finding::warning(
                "no_time_file",
                "time.dat is absent: time indices unknown",
            )),
            Some((time_path, time_len)) if time_len % 4 != 0 => r.push(
                Finding::error(
                    "truncated",
                    format!(
                        "{}: {} bytes after the last whole time index (cut off)",
                        self.rel(time_path),
                        time_len % 4
                    ),
                )
                .at(time_len - time_len % 4),
            ),
            _ => {}
        }
        for stream in &self.streams {
            let files: Vec<(PathBuf, u64, u64)> = match &stream.files {
                StreamFiles::Interleaved { path, words, len } => {
                    vec![(path.clone(), *len, 2 * words)]
                }
                StreamFiles::PerChannel(paths) => {
                    paths.iter().map(|(p, n)| (p.clone(), *n, 2)).collect()
                }
            };
            for (path, len, frame) in files {
                if len % frame != 0 {
                    r.push(
                        Finding::error(
                            "truncated",
                            format!(
                                "{}: {} bytes after the last whole sample (cut off)",
                                self.rel(&path),
                                len % frame
                            ),
                        )
                        .at(len - len % frame),
                    );
                }
                if let Some(expected) = time_samples
                    && len / frame != expected
                {
                    r.push(Finding::error(
                        "length_mismatch",
                        format!(
                            "{}: {} samples, time.dat {expected}",
                            self.rel(&path),
                            len / frame
                        ),
                    ));
                }
            }
        }
        if let Some((time_path, time_len)) = self.time.clone() {
            let mut file = self
                .fs
                .open(&time_path)
                .map_err(|e| Error::io(&time_path, e))?;
            let signed = self.header.signed_timestamps();
            let mut prev: Option<i64> = None;
            let mut gaps = 0u64;
            let mut at = 0u64;
            while at + 4 <= time_len {
                let chunk_len = (time_len - at).min(16 << 20) / 4 * 4;
                let chunk = read_block(&mut file, &time_path, at, chunk_len, time_len)?;
                for i in 0..chunk_len / 4 {
                    let pos = at + 4 * i;
                    let index = if signed {
                        chunk.i32_at(pos).map(i64::from)
                    } else {
                        chunk.u32_at(pos).map(i64::from)
                    }
                    .unwrap_or(0);
                    if prev.is_some_and(|last| index != last + 1) {
                        gaps += 1;
                    }
                    prev = Some(index);
                }
                at += chunk_len;
            }
            if gaps > 0 {
                r.push(Finding::warning(
                    "time_index_gap",
                    format!("{gaps} time indices do not follow the previous one (dropped samples or a trigger restart)"),
                ));
            }
        }
        Ok(r)
    }
}
