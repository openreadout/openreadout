//! 3Brain BrainWave raw data files (`.brw`, HDF5) of BioCAM high-density MEA systems. BRW 3.x
//! files keep the recording variables in `/3BRecInfo` and the samples in `/3BData/Raw` (a
//! frames × channels matrix in data layout 100, or the same values as one interleaved vector);
//! BRW 4.x files keep the settings as JSON in `/ExperimentSettings` and the samples of each well
//! in `/Well_<name>/Raw`. Samples are unsigned digital levels mapped linearly to µV. Notes and
//! vocabulary: `docs/formats/brw.md`; provenance: `docs/provenance/brw.md`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_core::model::{
    CheckReport, DetectConfidence, FileInfo, Finding, FormatDescriptor, LsEntry, SignalChannelInfo,
    Trace, TraceInfo,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, Detection, FormatReader, PlaneIndex, has_extension};
use openreadout_core::source::{Fs, Input};
use openreadout_core::{Error, Plane, Result};
use openreadout_hdf5::h5util::{attr_text, looks_like_hdf5, open_h5_in};
use serde_json::{Value, json};

use crate::h5rec::{self, Field, Heap, field};
use crate::mcsh5::memfind;

/// Format id of 3Brain BRW files.
pub const BRW_FORMAT_ID: &str = "brw";
/// Frames decoded per `read_trace` call at most.
pub const MAX_BRW_READ: u64 = 1 << 22;
/// Bytes of samples read per `read_trace` call at most.
pub const MAX_BRW_READ_BYTES: u64 = 64 << 20;
/// Bytes of the settings JSON read at most.
pub const MAX_BRW_SETTINGS: usize = 16 << 20;

/// The BRW generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrwGeneration {
    /// BRW 3.x (`/3BRecInfo`, `/3BData`).
    V3,
    /// BRW 4.x (`/ExperimentSettings`, `/Well_*`).
    V4,
}

/// One trace: a well (BRW 4.x) or the whole chip (BRW 3.x).
#[derive(Debug, Clone, PartialEq)]
pub struct BrwStream {
    /// Trace name (`Raw`, or the well group name).
    pub name: String,
    /// Path of the sample dataset.
    pub data_path: String,
    /// The samples are a frames × channels matrix (data layout 100), not one vector.
    pub matrix: bool,
    /// Channel names.
    pub channels: Vec<String>,
    /// Frames (samples per channel).
    pub frames: u64,
}

/// The parsed file.
#[derive(Debug, Clone)]
pub struct BrwFile {
    /// File length.
    pub file_len: u64,
    /// BRW generation.
    pub generation: BrwGeneration,
    /// `3BData` version attribute (BRW 3.x: 100, 101, 102), or the well group's (BRW 4.x).
    pub data_version: Option<i64>,
    /// Frames per second.
    pub sample_rate_hz: f64,
    /// µV per digital level.
    pub gain: f64,
    /// µV at digital level 0.
    pub offset: f64,
    /// Digital levels (2^bit depth) when stored values are inverted (BRW 3.x signal inversion −1).
    pub inverted_levels: Option<f64>,
    /// Firmware version (`major.minor.build.revision`).
    pub firmware: Option<String>,
    /// Hardware version.
    pub hardware: Option<String>,
    /// Recording date (BRW 4.x `ExperimentDateTime`).
    pub recorded_at: Option<String>,
    /// Plate or chip model.
    pub model: Option<String>,
    /// The sample streams.
    pub streams: Vec<BrwStream>,
    /// Parsed BRW 4.x settings.
    pub settings: Option<Value>,
    /// Problems found while parsing.
    pub findings: Vec<Finding>,
}

fn h5err(path: &Path, e: impl std::fmt::Display) -> Error {
    Error::corrupt(
        BRW_FORMAT_ID,
        format!("{}: the BRW structure cannot be read: {e}", path.display()),
    )
}

fn scalar_f64(h5: &hdf5_pure::File, at: &str, path: &Path) -> Result<f64> {
    let ds = h5.dataset(at).map_err(|e| h5err(path, e))?;
    ds.read_f64()
        .map_err(|e| h5err(path, e))?
        .first()
        .copied()
        .ok_or_else(|| Error::corrupt(BRW_FORMAT_ID, format!("{at} is empty")))
}

fn version_text(r: &[(String, Field)]) -> Option<String> {
    let parts: Vec<String> = ["Major", "Minor", "Build", "Revision"]
        .iter()
        .filter_map(|k| field(&r.to_vec(), k).and_then(Field::int))
        .map(|v| v.to_string())
        .collect();
    (!parts.is_empty()).then(|| parts.join("."))
}

fn json_version(v: &Value) -> Option<String> {
    let parts: Vec<String> = ["Major", "Minor", "Build", "Revision"]
        .iter()
        .filter_map(|k| v.get(*k).and_then(Value::as_i64))
        .map(|x| x.to_string())
        .collect();
    (!parts.is_empty()).then(|| parts.join("."))
}

fn parse_v3(fs: &Fs, path: &Path, h5: &hdf5_pure::File, file_len: u64) -> Result<BrwFile> {
    let vars = "/3BRecInfo/3BRecVars";
    let bit_depth = scalar_f64(h5, &format!("{vars}/BitDepth"), path)?;
    let max_v = scalar_f64(h5, &format!("{vars}/MaxVolt"), path)?;
    let min_v = scalar_f64(h5, &format!("{vars}/MinVolt"), path)?;
    let frames = scalar_f64(h5, &format!("{vars}/NRecFrames"), path)?;
    let rate = scalar_f64(h5, &format!("{vars}/SamplingRate"), path)?;
    let inversion = scalar_f64(h5, &format!("{vars}/SignalInversion"), path)?;
    if !(1.0..=16.0).contains(&bit_depth)
        || frames.is_nan()
        || frames < 0.0
        || rate.is_nan()
        || rate <= 0.0
    {
        return Err(Error::corrupt(
            BRW_FORMAT_ID,
            format!("recording variables: bit depth {bit_depth}, {frames} frames at {rate} Hz"),
        ));
    }
    let levels = (1u32 << (bit_depth as u32)) as f64;
    let mut heap = Heap::open(fs, path, h5, BRW_FORMAT_ID)?;
    let chs_ds = h5
        .dataset("/3BRecInfo/3BMeaStreams/Raw/Chs")
        .map_err(|e| h5err(path, e))?;
    let chs = h5rec::records(&chs_ds, &mut heap, path)?;
    let names: Vec<String> = chs
        .iter()
        .map(|r| {
            let row = field(r, "Row").and_then(Field::int).unwrap_or(0);
            let col = field(r, "Col").and_then(Field::int).unwrap_or(0);
            format!("ch{row}-{col}")
        })
        .collect();
    let data_version = h5
        .group("/3BData")
        .ok()
        .and_then(|g| g.attrs().ok())
        .and_then(|a| a.get("Version").and_then(attr_text))
        .and_then(|t| t.parse().ok());
    let raw = h5.dataset("/3BData/Raw").map_err(|e| h5err(path, e))?;
    let shape = raw.shape().map_err(|e| h5err(path, e))?;
    let frames = frames as u64;
    let (matrix, nch, stored_frames) = match (data_version, shape.as_slice()) {
        (Some(100), [f, c]) => (true, *c, *f),
        (Some(100), _) => {
            return Err(Error::corrupt(
                BRW_FORMAT_ID,
                format!("3BData version 100 with a raw dataset of shape {shape:?}"),
            ));
        }
        (Some(101 | 102) | None, [n]) => {
            let nch = n.checked_div(frames).unwrap_or(0);
            (false, nch, frames)
        }
        (v, s) => {
            return Err(Error::unsupported(
                BRW_FORMAT_ID,
                format!("3BData version {v:?} with a raw dataset of shape {s:?}"),
                "BRW 3.x data layouts 100, 101 and 102 are read.",
            ));
        }
    };
    let mut findings = Vec::new();
    if nch as usize != names.len() {
        return Err(Error::corrupt(
            BRW_FORMAT_ID,
            format!(
                "the raw data hold {nch} channel(s), the channel table {}",
                names.len()
            ),
        ));
    }
    if !matrix && frames.checked_mul(nch).is_none_or(|t| t != shape[0]) {
        findings.push(Finding::warning(
            "trailing_samples",
            format!(
                "{} raw values for {nch} channels × {frames} frames",
                shape[0]
            ),
        ));
    }
    let sys = "/3BRecInfo/3BMeaSystem";
    let mut ver = |name: &str| -> Option<String> {
        let d = h5.dataset(&format!("{sys}/{name}")).ok()?;
        let r = h5rec::records(&d, &mut heap, path).ok()?;
        r.first().and_then(|r| version_text(r))
    };
    let firmware = ver("FwVersion");
    let hardware = ver("HwVersion");
    Ok(BrwFile {
        file_len,
        generation: BrwGeneration::V3,
        data_version,
        sample_rate_hz: rate,
        gain: (max_v - min_v) / levels,
        offset: min_v,
        inverted_levels: (inversion < 0.0).then_some(levels),
        firmware,
        hardware,
        recorded_at: None,
        model: None,
        streams: vec![BrwStream {
            name: "Raw".into(),
            data_path: "/3BData/Raw".into(),
            matrix,
            channels: names,
            frames: stored_frames,
        }],
        settings: None,
        findings,
    })
}

fn parse_v4(path: &Path, h5: &hdf5_pure::File, file_len: u64) -> Result<BrwFile> {
    let ds = h5
        .dataset("/ExperimentSettings")
        .map_err(|e| h5err(path, e))?;
    let text = ds
        .read_string()
        .map_err(|e| h5err(path, e))?
        .into_iter()
        .next()
        .unwrap_or_default();
    if text.len() > MAX_BRW_SETTINGS {
        return Err(Error::unsupported(
            BRW_FORMAT_ID,
            "experiment settings over 16 MiB",
            "The file may be damaged; the limit is MAX_BRW_SETTINGS.",
        ));
    }
    let settings: Value = serde_json::from_str(&text).map_err(|e| {
        Error::corrupt(
            BRW_FORMAT_ID,
            format!("the experiment settings are not JSON: {e}"),
        )
    })?;
    let vc = &settings["ValueConverter"];
    let num = |v: &Value, k: &str| v.get(k).and_then(Value::as_f64);
    let (Some(min_a), Some(max_a), Some(min_d), Some(max_d)) = (
        num(vc, "MinAnalogValue"),
        num(vc, "MaxAnalogValue"),
        num(vc, "MinDigitalValue"),
        num(vc, "MaxDigitalValue"),
    ) else {
        return Err(Error::corrupt(
            BRW_FORMAT_ID,
            "the experiment settings have no value converter",
        ));
    };
    let scale = num(vc, "ScaleFactor").unwrap_or(1.0);
    let Some(rate) = num(&settings["TimeConverter"], "FrameRate").filter(|r| *r > 0.0) else {
        return Err(Error::corrupt(
            BRW_FORMAT_ID,
            "the experiment settings have no frame rate",
        ));
    };
    if (max_d - min_d).abs() < f64::EPSILON {
        return Err(Error::corrupt(
            BRW_FORMAT_ID,
            "the value converter's digital range is empty",
        ));
    }
    let root = h5.root();
    let mut wells: Vec<String> = root
        .groups()
        .map_err(|e| h5err(path, e))?
        .into_iter()
        .filter(|g| g.starts_with("Well_"))
        .collect();
    wells.sort();
    let mut streams = Vec::new();
    let mut data_version = None;
    for w in &wells {
        let g = root.group(w).map_err(|e| h5err(path, e))?;
        if data_version.is_none() {
            data_version = g
                .attrs()
                .ok()
                .and_then(|a| a.get("Version").and_then(attr_text))
                .and_then(|t| t.parse().ok());
        }
        let names = g.datasets().map_err(|e| h5err(path, e))?;
        if !names.iter().any(|n| n == "Raw") {
            if names.iter().any(|n| n == "EventsBasedSparseRaw") {
                return Err(Error::unsupported(
                    BRW_FORMAT_ID,
                    "event-based sparse raw data (BrainWave's compressed recordings)",
                    "Only stored samples are read; this file keeps samples only around detected events. Export the full raw data from BrainWave.",
                ));
            }
            continue;
        }
        let idd = g.dataset("StoredChIdxs").map_err(|e| h5err(path, e))?;
        h5rec::check_backed(&idd, file_len, BRW_FORMAT_ID, "StoredChIdxs")?;
        let idx = idd.read_i64().map_err(|e| h5err(path, e))?;
        let raw = g.dataset("Raw").map_err(|e| h5err(path, e))?;
        let n: u64 = raw.shape().map_err(|e| h5err(path, e))?.iter().product();
        let nch = idx.len() as u64;
        if nch == 0 || !n.is_multiple_of(nch) {
            return Err(Error::corrupt(
                BRW_FORMAT_ID,
                format!("{w}: {n} raw values are not whole frames of {nch} channels"),
            ));
        }
        streams.push(BrwStream {
            name: w.clone(),
            data_path: format!("/{w}/Raw"),
            matrix: false,
            channels: idx.iter().map(|i| format!("ch{i}")).collect(),
            frames: n / nch,
        });
    }
    if streams.is_empty() {
        return Err(Error::corrupt(
            BRW_FORMAT_ID,
            "the file has no well with raw data",
        ));
    }
    let ms = &settings["MeaSystem"];
    Ok(BrwFile {
        file_len,
        generation: BrwGeneration::V4,
        data_version,
        sample_rate_hz: rate,
        gain: scale * (max_a - min_a) / (max_d - min_d),
        offset: min_a,
        inverted_levels: None,
        firmware: json_version(&ms["FwVersion"]),
        hardware: json_version(&ms["HwVersion"]),
        recorded_at: settings["ExperimentDateTime"].as_str().map(str::to_string),
        model: settings["MeaPlate"]["Model"].as_str().map(str::to_string),
        streams,
        settings: Some(settings),
        findings: Vec::new(),
    })
}

/// Parse a BRW file.
pub fn parse_brw(fs: &Fs, path: &Path, h5: &hdf5_pure::File, file_len: u64) -> Result<BrwFile> {
    let root = h5.root();
    let groups = root.groups().map_err(|e| h5err(path, e))?;
    if groups.iter().any(|g| g == "3BRecInfo") {
        return parse_v3(fs, path, h5, file_len);
    }
    let datasets = root.datasets().map_err(|e| h5err(path, e))?;
    if datasets.iter().any(|d| d == "ExperimentSettings") {
        return parse_v4(path, h5, file_len);
    }
    Err(Error::unsupported(
        BRW_FORMAT_ID,
        "HDF5 files without BRW recording information",
        "This is not a BrainWave raw data file (no /3BRecInfo or /ExperimentSettings); BXR result files are not read.",
    ))
}

impl BrwFile {
    /// `BRW 3.x` or `BRW 4.x`.
    pub fn version_text(&self) -> &'static str {
        match self.generation {
            BrwGeneration::V3 => "BRW 3.x",
            BrwGeneration::V4 => "BRW 4.x",
        }
    }

    /// The layout of the samples, for the assurance profile.
    pub fn layout(&self) -> String {
        match (self.generation, self.data_version) {
            (BrwGeneration::V3, Some(100)) => "frame matrix (3BData 100)".into(),
            (BrwGeneration::V3, Some(v)) => format!("interleaved (3BData {v})"),
            (BrwGeneration::V3, None) => "interleaved (3BData unversioned)".into(),
            (BrwGeneration::V4, _) => "interleaved per well".into(),
        }
    }
}

/// The BRW reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct BrwReader;

impl FormatReader for BrwReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&crate::assurance::BRW)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: BRW_FORMAT_ID.into(),
            name: "3Brain BrainWave raw data (BRW 3.x and 4.x)".into(),
            vendor: "3Brain AG".into(),
            extensions: vec!["brw".into()],
            family: "electrophysiology".into(),
            can_read: true,
            can_write: false,
            confidence: crate::assurance::BRW.confidence,
            known_gaps: vec![
                "Event-based sparse raw data (BrainWave 5 compressed recordings) are refused".into(),
                "BRW 4.x channel names are the stored channel indices; their place on the chip is not derived".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if !looks_like_hdf5(head) {
            return None;
        }
        let marked = memfind(head, b"3BRecInfo") || memfind(head, b"BRW-File");
        (marked || has_extension(path, &["brw"])).then(|| Detection {
            format_id: BRW_FORMAT_ID,
            confidence: DetectConfidence::Definite,
            note: Some(if marked {
                "HDF5 file with BRW recording information".into()
            } else {
                "HDF5 file with the .brw extension".into()
            }),
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(BrwDataset::open(path)?))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(BrwDataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// An opened BRW file.
#[derive(Debug)]
pub struct BrwDataset {
    path: PathBuf,
    h5: hdf5_pure::File,
    file: BrwFile,
}

impl BrwDataset {
    /// Open a local file.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    /// Open an [`Input`].
    pub fn open_input(input: &Input) -> Result<Self> {
        let path = input.path().to_path_buf();
        let fs = input.fs().clone();
        let file_len = fs.metadata(&path).map_err(|e| Error::io(&path, e))?.len();
        let h5 = open_h5_in(&fs, &path, BRW_FORMAT_ID)?;
        let file = parse_brw(&fs, &path, &h5, file_len)?;
        Ok(BrwDataset { path, h5, file })
    }

    /// The parsed file.
    pub fn brw(&self) -> &BrwFile {
        &self.file
    }

    fn trace_info(&self, i: usize) -> TraceInfo {
        let f = &self.file;
        let s = &f.streams[i];
        let channels = s
            .channels
            .iter()
            .enumerate()
            .map(|(k, n)| SignalChannelInfo {
                index: k as u32,
                name: n.clone(),
                unit: Some("µV".into()),
                dtype: "uint16".into(),
                scale: if f.inverted_levels.is_some() {
                    -f.gain
                } else {
                    f.gain
                },
                offset: match f.inverted_levels {
                    Some(l) => l * f.gain + f.offset,
                    None => f.offset,
                },
                extra: BTreeMap::new(),
            })
            .collect();
        let mut extra = BTreeMap::new();
        extra.insert("layout".into(), json!(f.layout()));
        if let Some(v) = &f.firmware {
            extra.insert("firmware_version".into(), json!(v));
        }
        if let Some(v) = &f.hardware {
            extra.insert("hardware_version".into(), json!(v));
        }
        if i == 0 {
            if let Some(t) = &f.recorded_at {
                extra.insert("recorded_at".into(), json!(t));
            }
            if let Some(m) = &f.model {
                extra.insert("model".into(), json!(m));
            }
        }
        if f.inverted_levels.is_some() {
            extra.insert("signal_inversion".into(), json!(-1));
        }
        TraceInfo {
            index: i as u32,
            name: Some(s.name.clone()),
            sample_rate_hz: f.sample_rate_hz,
            sample_count: s.frames,
            sweep_count: 1,
            channels,
            start_s: Some(0.0),
            extra,
        }
    }
}

impl Dataset for BrwDataset {
    fn info(&self) -> Result<FileInfo> {
        let f = &self.file;
        let mut notes = vec![format!(
            "{} file, {}: {} trace(s), {} channel(s) at {} Hz",
            f.version_text(),
            f.layout(),
            f.streams.len(),
            f.streams.first().map_or(0, |s| s.channels.len()),
            f.sample_rate_hz
        )];
        notes.push(match f.inverted_levels {
            Some(l) => format!(
                "values are ({l} − stored level) × {} + {} µV (signal inversion)",
                f.gain, f.offset
            ),
            None => format!("values are stored level × {} + {} µV", f.gain, f.offset),
        });
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: f.file_len,
            format: BrwReader.descriptor(),
            format_version: Some(f.version_text().into()),
            images: Vec::new(),
            tables: Vec::new(),
            spectra: Vec::new(),
            traces: (0..f.streams.len()).map(|i| self.trace_info(i)).collect(),
            plane_count: 0,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let f = &self.file;
        Ok(json!({
            "generation": f.version_text(),
            "data_version": f.data_version,
            "sample_rate_hz": f.sample_rate_hz,
            "gain": f.gain,
            "offset": f.offset,
            "inverted_levels": f.inverted_levels,
            "firmware_version": f.firmware,
            "hardware_version": f.hardware,
            "streams": f.streams.iter().map(|s| json!({
                "name": s.name, "data": s.data_path, "frames": s.frames,
                "channels": s.channels.len(), "matrix": s.matrix,
            })).collect::<Vec<_>>(),
            "settings": f.settings,
        }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for k in [
            "format_version",
            "traces[].sample_rate_hz",
            "traces[].sample_count",
            "traces[].channels[].scale",
            "traces[].channels[].offset",
            "traces[].channels[].unit",
        ] {
            p.insert(k.into(), Source::PriorArt);
        }
        for k in ["traces[].channels[].name", "traces[].extra.recorded_at"] {
            p.insert(k.into(), Source::Inferred);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        Ok(self
            .file
            .streams
            .iter()
            .map(|s| LsEntry {
                kind: "samples".into(),
                name: s.data_path.clone(),
                offset: None,
                size: Some(s.frames * s.channels.len() as u64 * 2),
                image: None,
                details: json!({"frames": s.frames, "channels": s.channels.len(), "matrix": s.matrix}),
            })
            .collect())
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::unsupported(
            BRW_FORMAT_ID,
            "image planes",
            "BRW recordings hold sampled signals: use `openreadout trace`.",
        ))
    }

    fn read_trace(&mut self, index: u32, sweep: u32, first: u64, max: u64) -> Result<Trace> {
        let Some(s) = self.file.streams.get(index as usize) else {
            return Err(Error::Usage(format!(
                "trace index {index} out of range ({} traces)",
                self.file.streams.len()
            )));
        };
        if sweep != 0 {
            return Err(Error::Usage(format!(
                "sweep {sweep} out of range (1 sweep)"
            )));
        }
        if first > s.frames {
            return Err(Error::Usage(format!(
                "first sample {first} is past the end ({} samples)",
                s.frames
            )));
        }
        let nch = s.channels.len() as u64;
        let count = max
            .min(s.frames - first)
            .min(MAX_BRW_READ)
            .min((MAX_BRW_READ_BYTES / (2 * nch).max(1)).max(1))
            .min(
                (h5rec::MAX_H5_EXPANSION.saturating_mul(self.file.file_len) / (2 * nch).max(1))
                    .max(1),
            );
        let ds = self
            .h5
            .dataset(&s.data_path)
            .map_err(|e| h5err(&self.path, e))?;
        let values = if count == 0 {
            Vec::new()
        } else if s.matrix {
            ds.read_u16_rows(first, count)
                .map_err(|e| h5err(&self.path, e))?
        } else {
            ds.read_u16_rows(first * nch, count * nch)
                .map_err(|e| h5err(&self.path, e))?
        };
        if (values.len() as u64) < count * nch {
            return Err(Error::corrupt(
                BRW_FORMAT_ID,
                format!("{}: fewer samples than the recording declares", s.data_path),
            ));
        }
        let n = usize::try_from(nch).unwrap_or(0);
        let mut channels: Vec<Vec<f64>> =
            vec![Vec::with_capacity(usize::try_from(count).unwrap_or(0)); n];
        let (gain, offset, inv) = (self.file.gain, self.file.offset, self.file.inverted_levels);
        for (k, &v) in values.iter().enumerate() {
            let level = match inv {
                Some(l) => l - f64::from(v),
                None => f64::from(v),
            };
            channels[k % n].push(level * gain + offset);
        }
        Ok(Trace {
            trace: index,
            sweep,
            first_sample: first,
            channels,
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), BRW_FORMAT_ID);
        r.performed("recording variables or experiment settings read; channel table against the raw data size");
        for x in &self.file.findings {
            r.push(x.clone());
        }
        Ok(r)
    }
}

#[cfg(test)]
mod tests;
