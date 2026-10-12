//! Multi Channel Systems MC_DataTool binary exports (`.raw` written with the header option): a
//! text header ending at `EOH`, then interleaved uint16 frames of every stream. One trace.
//! Notes and vocabulary: `docs/formats/mcs-raw.md`; provenance: `docs/provenance/mcs-raw.md`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_core::bytes::{find, read_block, windows1252};
use openreadout_core::model::{
    CheckReport, DetectConfidence, FileInfo, Finding, FormatDescriptor, LsEntry, SignalChannelInfo,
    Trace, TraceInfo,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, Detection, FormatReader, PlaneIndex};
use openreadout_core::source::{Fs, Input, SourceFile};
use openreadout_core::{Error, Plane, Result};
use serde_json::{Value, json};

#[cfg(test)]
mod tests;

/// Format id of MC_DataTool binary exports.
pub const MCS_RAW_FORMAT_ID: &str = "mcs-raw";
/// First line of the header.
pub const MCS_RAW_MAGIC: &[u8] = b"MC_DataTool binary conversion";
/// Bytes searched for the end of the header.
pub const MAX_MCS_HEADER: u64 = 64 * 1024;
/// Streams accepted at most.
pub const MAX_MCS_STREAMS: usize = 4096;
/// Values (all channels) decoded per `read_trace` call at most.
pub const MAX_MCS_READ_VALUES: u64 = 1 << 27;

/// The parsed header.
#[derive(Debug, Clone, Default)]
pub struct McsRawHeader {
    /// Every `key = value` line, in file order.
    pub entries: Vec<(String, String)>,
    /// `Version`.
    pub version: Option<String>,
    /// The source recording (`MC_REC file`).
    pub source_file: Option<String>,
    /// `Sample rate`, Hz.
    pub sample_rate_hz: f64,
    /// `ADC zero`.
    pub adc_zero: f64,
    /// Electrode gain (`El`), units per count.
    pub gain: f64,
    /// Electrode unit (`El`).
    pub unit: String,
    /// Stream names (`Streams`).
    pub streams: Vec<String>,
    /// Offset of the first sample.
    pub data_offset: u64,
}

/// True when `head` starts with the MC_DataTool header line.
pub fn looks_like_mcs_raw(head: &[u8]) -> bool {
    head.starts_with(MCS_RAW_MAGIC)
}

/// Parse the header in `head` (the file's first bytes).
pub fn parse_header(head: &[u8]) -> Result<McsRawHeader> {
    let Some(eoh) = find(head, b"EOH") else {
        return Err(Error::corrupt(
            MCS_RAW_FORMAT_ID,
            "no EOH (end of header) in the first 64 KiB",
        ));
    };
    let text = windows1252(&head[..eoh]);
    let mut h = McsRawHeader {
        data_offset: eoh as u64 + 5,
        sample_rate_hz: f64::NAN,
        adc_zero: f64::NAN,
        gain: f64::NAN,
        ..McsRawHeader::default()
    };
    for line in text.split(['\r', '\n']).map(str::trim).filter(|l| !l.is_empty()) {
        let (k, v) = match line.split_once(" = ") {
            Some((k, v)) => (k.trim(), v.trim()),
            None => match line.split_once(' ') {
                Some((k, v)) if k == "Version" => (k, v.trim()),
                _ => continue,
            },
        };
        h.entries.push((k.to_string(), v.to_string()));
        match k {
            "Version" => h.version = Some(v.to_string()),
            "MC_REC file" => h.source_file = Some(v.trim_matches('"').to_string()),
            "Sample rate" => h.sample_rate_hz = v.parse().unwrap_or(f64::NAN),
            "ADC zero" => h.adc_zero = v.parse().unwrap_or(f64::NAN),
            "El" => {
                let v = v.trim_end_matches("/AD").trim();
                let split = v
                    .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == 'e' || c == 'E' || c == '+'))
                    .unwrap_or(v.len());
                h.gain = v[..split].parse().unwrap_or(f64::NAN);
                h.unit = v[split..].trim().to_string();
            }
            "Streams" => h.streams = v.split(';').map(|s| s.trim().to_string()).collect(),
            _ => {}
        }
    }
    Ok(h)
}

impl McsRawHeader {
    /// Why the samples cannot be scaled, if they cannot.
    pub fn unreadable(&self) -> Option<String> {
        if self.streams.is_empty() || self.streams.len() > MAX_MCS_STREAMS {
            return Some(format!("{} streams", self.streams.len()));
        }
        if !(self.sample_rate_hz.is_finite() && self.sample_rate_hz > 0.0) {
            return Some("no usable Sample rate".into());
        }
        if !self.adc_zero.is_finite() || !self.gain.is_finite() {
            return Some("no usable ADC zero or El gain".into());
        }
        if let Some(s) = self.streams.iter().find(|s| !s.starts_with("El_")) {
            return Some(format!(
                "stream {s} is not an electrode (El_) stream, and only the El gain is understood"
            ));
        }
        None
    }
}

/// The MC_DataTool reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct McsRawReader;

impl FormatReader for McsRawReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&crate::assurance::MCS_RAW)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: MCS_RAW_FORMAT_ID.into(),
            name: "Multi Channel Systems MC_DataTool binary export (.raw with header)".into(),
            vendor: "Multi Channel Systems (MC_DataTool)".into(),
            extensions: vec!["raw".into()],
            family: "electrophysiology".into(),
            can_read: true,
            can_write: false,
            confidence: crate::assurance::MCS_RAW.confidence,
            known_gaps: vec![
                "Only exports written with the header option are read; electrode (El_) streams only, other stream kinds are refused".into(),
                "MC_Rack .mcd recordings are not read (no public specification)".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], _path: &Path) -> Option<Detection> {
        looks_like_mcs_raw(head).then_some(Detection {
            format_id: MCS_RAW_FORMAT_ID,
            confidence: DetectConfidence::Definite,
            note: None,
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(McsRawDataset::open(path)?))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(McsRawDataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// An opened export.
#[derive(Debug)]
pub struct McsRawDataset {
    path: PathBuf,
    fs: Fs,
    handle: Option<SourceFile>,
    /// The header.
    pub header: McsRawHeader,
    /// File length.
    pub file_len: u64,
}

impl McsRawDataset {
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
        let head = read_block(&mut f, &path, 0, MAX_MCS_HEADER, file_len)?;
        if !looks_like_mcs_raw(&head.bytes) {
            return Err(Error::unsupported(
                MCS_RAW_FORMAT_ID,
                "a .raw file without the MC_DataTool header",
                "export the recording again from MC_DataTool with the header option, or read it as raw binary",
            ));
        }
        let header = parse_header(&head.bytes)?;
        if let Some(why) = header.unreadable() {
            return Err(Error::unsupported(
                MCS_RAW_FORMAT_ID,
                "this MC_DataTool export",
                format!("{why}; export electrode streams only, or open an issue with the file"),
            ));
        }
        Ok(McsRawDataset {
            path,
            fs,
            handle: Some(f),
            header,
            file_len,
        })
    }

    /// Whole frames in the file.
    pub fn frames(&self) -> u64 {
        self.file_len.saturating_sub(self.header.data_offset) / (2 * self.header.streams.len() as u64)
    }

    fn handle(&mut self) -> Result<&mut SourceFile> {
        if self.handle.is_none() {
            self.handle = Some(self.fs.open(&self.path).map_err(|e| Error::io(&self.path, e))?);
        }
        Ok(self.handle.as_mut().expect("just opened"))
    }
}

impl Dataset for McsRawDataset {
    fn info(&self) -> Result<FileInfo> {
        let h = &self.header;
        let mut extra = BTreeMap::new();
        extra.insert("application".into(), json!("MC_DataTool"));
        if let Some(v) = &h.version {
            extra.insert("application_version".into(), json!(v));
        }
        if let Some(s) = &h.source_file {
            extra.insert("source_file".into(), json!(s));
        }
        extra.insert("adc_zero".into(), json!(h.adc_zero));
        let offset = -h.gain * h.adc_zero;
        let mut notes = vec![format!(
            "MC_DataTool binary export: {} electrode stream(s), uint16 samples",
            h.streams.len()
        )];
        let extra_bytes = self
            .file_len
            .saturating_sub(h.data_offset)
            % (2 * h.streams.len() as u64);
        if extra_bytes != 0 {
            notes.push(format!("{extra_bytes} bytes after the last whole frame are ignored"));
        }
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.file_len,
            format: McsRawReader.descriptor(),
            format_version: h.version.clone(),
            images: Vec::new(),
            tables: Vec::new(),
            spectra: Vec::new(),
            traces: vec![TraceInfo {
                index: 0,
                name: Some("Signals".into()),
                sample_rate_hz: h.sample_rate_hz,
                sample_count: self.frames(),
                sweep_count: 1,
                channels: h
                    .streams
                    .iter()
                    .enumerate()
                    .map(|(i, s)| SignalChannelInfo {
                        index: i as u32,
                        name: s.clone(),
                        unit: (!h.unit.is_empty()).then(|| h.unit.clone()),
                        dtype: "uint16".into(),
                        scale: h.gain,
                        offset,
                        extra: BTreeMap::new(),
                    })
                    .collect(),
                start_s: Some(0.0),
                extra,
            }],
            plane_count: 0,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        Ok(json!({
            "header": self.header.entries.iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>(),
            "data_offset": self.header.data_offset,
        }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for k in [
            "traces[].sample_rate_hz",
            "traces[].channels[].name",
            "traces[].channels[].unit",
            "traces[].channels[].scale",
            "traces[].channels[].offset",
        ] {
            p.insert(k.into(), Source::PriorArt);
        }
        p.insert("traces[].sample_count".into(), Source::Inferred);
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        Ok(vec![
            LsEntry {
                kind: "header".into(),
                name: "text header".into(),
                offset: Some(0),
                size: Some(self.header.data_offset),
                image: None,
                details: json!({"entries": self.header.entries.len()}),
            },
            LsEntry {
                kind: "samples".into(),
                name: "interleaved uint16 frames".into(),
                offset: Some(self.header.data_offset),
                size: Some(self.frames() * 2 * self.header.streams.len() as u64),
                image: None,
                details: json!({"frames": self.frames()}),
            },
        ])
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::unsupported(
            MCS_RAW_FORMAT_ID,
            "image planes",
            "MC_DataTool exports hold sampled signals: use `openreadout trace`.",
        ))
    }

    fn read_trace(&mut self, index: u32, sweep: u32, first: u64, max: u64) -> Result<Trace> {
        if index != 0 || sweep != 0 {
            return Err(Error::Usage(format!(
                "trace {index} sweep {sweep} out of range (one trace, one sweep)"
            )));
        }
        let n = self.frames();
        if first > n {
            return Err(Error::Usage(format!(
                "first sample {first} is past the end ({n} samples)"
            )));
        }
        let nc = self.header.streams.len() as u64;
        let count = max.min(n - first).min(MAX_MCS_READ_VALUES / nc);
        let at = self.header.data_offset + first * nc * 2;
        let (path, len) = (self.path.clone(), self.file_len);
        let block = read_block(self.handle()?, &path, at, count * nc * 2, len)?;
        if (block.len() as u64) < count * nc * 2 {
            return Err(Error::corrupt_at(MCS_RAW_FORMAT_ID, at, "samples cut off"));
        }
        let (gain, offset) = (self.header.gain, -self.header.gain * self.header.adc_zero);
        let mut channels = vec![Vec::with_capacity(count as usize); nc as usize];
        for frame in block.bytes.as_chunks::<2>().0.chunks_exact(nc as usize) {
            for (c, v) in frame.iter().enumerate() {
                channels[c].push(f64::from(u16::from_le_bytes(*v)) * gain + offset);
            }
        }
        Ok(Trace {
            trace: 0,
            sweep: 0,
            first_sample: first,
            channels,
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), MCS_RAW_FORMAT_ID);
        r.performed("header read to EOH: Sample rate, ADC zero, El gain and Streams present");
        r.performed("file length against whole frames of every stream");
        let h = &self.header;
        let extra = self.file_len.saturating_sub(h.data_offset) % (2 * h.streams.len() as u64);
        if extra != 0 {
            r.push(Finding::warning(
                "partial_frame",
                format!("{extra} bytes after the last whole frame"),
            ));
        }
        let mut seen = std::collections::BTreeSet::new();
        for s in &h.streams {
            if !seen.insert(s) {
                r.push(Finding::warning("duplicate_stream", format!("stream {s} is listed twice")));
            }
        }
        Ok(r)
    }
}
