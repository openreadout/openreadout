//! MaxWell Biosystems MaxOne and MaxTwo recordings (`.raw.h5`, HDF5). Files in the layout
//! version `20160704` keep one matrix `/sig` (rows are amplifier channels, columns frames), the
//! channel-to-electrode `/mapping` and `/settings/lsb`; files in later layouts (`20190530`) keep
//! one recording per well in `/data_store/dataNNNN` (`groups/routed/raw` with the channel of each
//! row in `groups/routed/channels`, `settings/mapping`, `settings/lsb`, `settings/sampling`).
//! Samples are unsigned levels × lsb, reported in µV. Notes and vocabulary:
//! `docs/formats/maxwell-h5.md`; provenance: `docs/provenance/maxwell-h5.md`.

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
use openreadout_hdf5::h5util::{looks_like_hdf5, open_h5_in};
use serde_json::{Value, json};

use crate::h5rec::{self, Field, Heap, Matrix, field};
use crate::mcsh5::memfind;

/// Format id of MaxWell recordings.
pub const MAXWELL_FORMAT_ID: &str = "maxwell-h5";
/// The frame rate of layout-20160704 files, which do not store one.
pub const MAXWELL_LEGACY_RATE_HZ: f64 = 20_000.0;
/// The HDF5 filter id of MaxWell's own compression.
pub const MAXWELL_COMPRESSION_FILTER: &str = "filter 401";
/// Frames decoded per `read_trace` call at most.
pub const MAX_MAXWELL_READ: u64 = 1 << 20;
/// Bytes of samples read per `read_trace` call at most.
pub const MAX_MAXWELL_READ_BYTES: u64 = 64 << 20;

/// One routed channel.
#[derive(Debug, Clone, PartialEq)]
pub struct MaxwellChannel {
    /// Amplifier channel number.
    pub channel: i64,
    /// Electrode number on the array.
    pub electrode: i64,
    /// Electrode position, µm.
    pub x_um: f64,
    /// Electrode position, µm.
    pub y_um: f64,
    /// Row of the sample matrix.
    pub row: u64,
}

/// One recording of one well.
#[derive(Debug, Clone)]
pub struct MaxwellRecording {
    /// Trace name (`sig`, or `well000 rec0000`).
    pub name: String,
    /// The sample matrix.
    pub matrix: Matrix,
    /// Routed channels, in channel order.
    pub channels: Vec<MaxwellChannel>,
    /// Frames per second.
    pub sample_rate_hz: f64,
    /// The rate is not stored (layout 20160704).
    pub rate_assumed: bool,
    /// Volts per level.
    pub lsb_v: f64,
    /// Start of the recording as stored.
    pub started_at: Option<String>,
}

/// The parsed file.
#[derive(Debug, Clone)]
pub struct MaxwellFile {
    /// File length.
    pub file_len: u64,
    /// Layout version (`/version`).
    pub version: String,
    /// MaxLab version (`/mxw_version`).
    pub mxw_version: Option<String>,
    /// Plate kind (`/wellplate/version`).
    pub plate: Option<String>,
    /// Recordings.
    pub recordings: Vec<MaxwellRecording>,
    /// Problems found while parsing.
    pub findings: Vec<Finding>,
}

fn h5err(path: &Path, e: impl std::fmt::Display) -> Error {
    Error::corrupt(
        MAXWELL_FORMAT_ID,
        format!(
            "{}: the MaxWell structure cannot be read: {e}",
            path.display()
        ),
    )
}

fn text(h5: &hdf5_pure::File, at: &str) -> Option<String> {
    let d = h5.dataset(at).ok()?;
    d.read_string().ok()?.into_iter().next().map(|s| {
        s.trim_matches(|c: char| c == '\0' || c.is_whitespace())
            .to_string()
    })
}

fn number(h5: &hdf5_pure::File, at: &str) -> Option<f64> {
    h5.dataset(at).ok()?.read_f64().ok()?.first().copied()
}

fn mapping(
    h5: &hdf5_pure::File,
    heap: &mut Heap,
    at: &str,
    path: &Path,
) -> Result<Vec<(i64, i64, f64, f64)>> {
    let d = h5.dataset(at).map_err(|e| h5err(path, e))?;
    Ok(h5rec::records(&d, heap, path)?
        .iter()
        .map(|r| {
            (
                field(r, "channel").and_then(Field::int).unwrap_or(-1),
                field(r, "electrode").and_then(Field::int).unwrap_or(-1),
                field(r, "x").and_then(Field::float).unwrap_or(f64::NAN),
                field(r, "y").and_then(Field::float).unwrap_or(f64::NAN),
            )
        })
        .filter(|m| m.0 >= 0)
        .collect())
}

fn refuse_compressed(m: &Matrix) -> Result<()> {
    if m.filters().iter().any(|f| f == MAXWELL_COMPRESSION_FILTER) {
        return Err(Error::unsupported(
            MAXWELL_FORMAT_ID,
            "samples compressed with MaxWell's own HDF5 filter (401, mxw-data)",
            "No open decoder of this filter exists; export the recording uncompressed from MaxLab Live.",
        ));
    }
    Ok(())
}

/// ISO 8601 UTC of Unix milliseconds.
fn unix_ms(ms: i64) -> String {
    openreadout_core::time::unix_to_iso8601(ms.div_euclid(1000), ms.rem_euclid(1000) as u32)
}

/// Parse a MaxWell file.
pub fn parse_maxwell(
    fs: &Fs,
    path: &Path,
    h5: &hdf5_pure::File,
    file_len: u64,
) -> Result<MaxwellFile> {
    let version = text(h5, "/version")
        .ok_or_else(|| Error::corrupt(MAXWELL_FORMAT_ID, "the file has no /version dataset"))?;
    let mut heap = Heap::open(fs, path, h5, MAXWELL_FORMAT_ID)?;
    let mut recordings = Vec::new();
    let findings = Vec::new();
    if version == "20160704" {
        let matrix = Matrix::open(h5, "/sig", MAXWELL_FORMAT_ID, file_len)?;
        refuse_compressed(&matrix)?;
        let mut channels: Vec<MaxwellChannel> = mapping(h5, &mut heap, "/mapping", path)?
            .into_iter()
            .map(|(c, e, x, y)| MaxwellChannel {
                channel: c,
                electrode: e,
                x_um: x,
                y_um: y,
                row: c as u64,
            })
            .collect();
        channels.sort_by_key(|c| c.channel);
        if let Some(c) = channels.iter().find(|c| c.row >= matrix.rows) {
            return Err(Error::corrupt(
                MAXWELL_FORMAT_ID,
                format!(
                    "channel {} is outside the {}-row /sig matrix",
                    c.channel, matrix.rows
                ),
            ));
        }
        let lsb_v = match number(h5, "/settings/lsb") {
            Some(l) => l,
            None => 3.3 / (1024.0 * number(h5, "/settings/gain").unwrap_or(512.0)),
        };
        let started_at = text(h5, "/time").and_then(|t| {
            let s = t.strip_prefix("start: ")?;
            let s = s.split(';').next()?.trim();
            (s.len() == 19).then(|| s.replacen(' ', "T", 1))
        });
        recordings.push(MaxwellRecording {
            name: "sig".into(),
            matrix,
            channels,
            sample_rate_hz: MAXWELL_LEGACY_RATE_HZ,
            rate_assumed: true,
            lsb_v,
            started_at,
        });
    } else if version.parse::<u64>().is_ok_and(|v| v > 20_160_704) {
        let store = h5.group("/data_store").map_err(|e| h5err(path, e))?;
        let mut names = store.groups().map_err(|e| h5err(path, e))?;
        names.sort();
        for n in names {
            let base = format!("/data_store/{n}");
            let Ok(raw) = Matrix::open(
                h5,
                &format!("{base}/groups/routed/raw"),
                MAXWELL_FORMAT_ID,
                file_len,
            ) else {
                continue;
            };
            refuse_compressed(&raw)?;
            let chd = h5
                .dataset(&format!("{base}/groups/routed/channels"))
                .map_err(|e| h5err(path, e))?;
            h5rec::check_backed(&chd, file_len, MAXWELL_FORMAT_ID, "routed channels")?;
            let rows = chd.read_i64().map_err(|e| h5err(path, e))?;
            let map = mapping(h5, &mut heap, &format!("{base}/settings/mapping"), path)?;
            let mut channels: Vec<MaxwellChannel> = rows
                .iter()
                .enumerate()
                .map(|(row, &c)| {
                    let m = map.iter().find(|m| m.0 == c);
                    MaxwellChannel {
                        channel: c,
                        electrode: m.map_or(-1, |m| m.1),
                        x_um: m.map_or(f64::NAN, |m| m.2),
                        y_um: m.map_or(f64::NAN, |m| m.3),
                        row: row as u64,
                    }
                })
                .collect();
            channels.sort_by_key(|c| c.channel);
            let well = number(h5, &format!("{base}/well_id")).unwrap_or(0.0) as i64;
            let rec = number(h5, &format!("{base}/recording_id")).unwrap_or(0.0) as i64;
            let rate = number(h5, &format!("{base}/settings/sampling")).ok_or_else(|| {
                Error::corrupt(MAXWELL_FORMAT_ID, format!("{base}: no sampling rate"))
            })?;
            let lsb_v = match number(h5, &format!("{base}/settings/lsb")) {
                Some(l) => l,
                None => {
                    3.3 / (1024.0 * number(h5, &format!("{base}/settings/gain")).unwrap_or(512.0))
                }
            };
            let started_at = h5
                .dataset(&format!("{base}/start_time"))
                .and_then(|d| d.read_i64())
                .ok()
                .and_then(|v| v.first().copied())
                .map(unix_ms);
            recordings.push(MaxwellRecording {
                name: format!("well{well:03} rec{rec:04}"),
                matrix: raw,
                channels,
                sample_rate_hz: rate,
                rate_assumed: false,
                lsb_v,
                started_at,
            });
        }
    } else {
        return Err(Error::unsupported(
            MAXWELL_FORMAT_ID,
            format!("MaxWell layout version {version}"),
            "Layout versions 20160704 and later dated versions (20190530) are read.",
        ));
    }
    if recordings.is_empty() {
        return Err(Error::corrupt(
            MAXWELL_FORMAT_ID,
            "the file holds no routed recording",
        ));
    }
    Ok(MaxwellFile {
        file_len,
        mxw_version: text(h5, "/mxw_version"),
        plate: text(h5, "/wellplate/version"),
        version,
        recordings,
        findings,
    })
}

/// The MaxWell reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct MaxwellReader;

fn det(confidence: DetectConfidence, note: &str) -> Detection {
    Detection {
        format_id: MAXWELL_FORMAT_ID,
        confidence,
        note: Some(note.into()),
    }
}

impl FormatReader for MaxwellReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&crate::assurance::MAXWELL)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: MAXWELL_FORMAT_ID.into(),
            name: "MaxWell Biosystems MaxOne / MaxTwo recording (.raw.h5)".into(),
            vendor: "MaxWell Biosystems AG".into(),
            extensions: vec!["h5".into()],
            family: "electrophysiology".into(),
            can_read: true,
            can_write: false,
            confidence: crate::assurance::MAXWELL.confidence,
            known_gaps: vec![
                "Recordings compressed with MaxWell's own HDF5 filter (401) are refused: no open decoder exists".into(),
                "Spike tables and frame numbers are not read".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], _path: &Path) -> Option<Detection> {
        if !looks_like_hdf5(head) {
            return None;
        }
        if memfind(head, b"Recorded with MaxLab")
            || (memfind(head, b"mxw_version") && memfind(head, b"wellplate"))
        {
            return Some(det(
                DetectConfidence::Definite,
                "HDF5 file written by MaxLab",
            ));
        }
        None
    }

    fn sniff_input(&self, head: &[u8], input: &Input) -> Option<Detection> {
        if let Some(d) = self.sniff(head, input.path()) {
            return Some(d);
        }
        if !looks_like_hdf5(head) || !has_extension(input.path(), &["h5"]) {
            return None;
        }
        let h5 = open_h5_in(input.fs(), input.path(), MAXWELL_FORMAT_ID).ok()?;
        let v = text(&h5, "/version")?;
        let root = h5.root();
        let ds = root.datasets().ok()?;
        let gs = root.groups().ok()?;
        let legacy =
            v == "20160704" && ds.iter().any(|d| d == "sig") && ds.iter().any(|d| d == "mapping");
        let current = gs.iter().any(|g| g == "data_store") && gs.iter().any(|g| g == "wells");
        (legacy || current)
            .then(|| det(DetectConfidence::Definite, "HDF5 file in a MaxWell layout"))
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(MaxwellDataset::open(path)?))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(MaxwellDataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// An opened MaxWell recording.
#[derive(Debug)]
pub struct MaxwellDataset {
    path: PathBuf,
    fs: Fs,
    h5: hdf5_pure::File,
    file: MaxwellFile,
}

impl MaxwellDataset {
    /// Open a local file.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    /// Open an [`Input`].
    pub fn open_input(input: &Input) -> Result<Self> {
        let path = input.path().to_path_buf();
        let fs = input.fs().clone();
        let file_len = fs.metadata(&path).map_err(|e| Error::io(&path, e))?.len();
        let h5 = open_h5_in(&fs, &path, MAXWELL_FORMAT_ID)?;
        let file = parse_maxwell(&fs, &path, &h5, file_len)?;
        Ok(MaxwellDataset { path, fs, h5, file })
    }

    /// The parsed file.
    pub fn maxwell(&self) -> &MaxwellFile {
        &self.file
    }

    fn trace_info(&self, i: usize) -> TraceInfo {
        let f = &self.file;
        let r = &f.recordings[i];
        let uv = r.lsb_v * 1e6;
        let channels = r
            .channels
            .iter()
            .enumerate()
            .map(|(k, c)| {
                let mut e = BTreeMap::new();
                e.insert("channel".into(), json!(c.channel));
                e.insert("electrode".into(), json!(c.electrode));
                if c.x_um.is_finite() && c.y_um.is_finite() {
                    e.insert("x_um".into(), json!(c.x_um));
                    e.insert("y_um".into(), json!(c.y_um));
                }
                SignalChannelInfo {
                    index: k as u32,
                    name: format!("ch{}", c.channel),
                    unit: Some("µV".into()),
                    dtype: h5rec::dtype_of(r.matrix.pixel_type()).into(),
                    scale: uv,
                    offset: 0.0,
                    extra: e,
                }
            })
            .collect();
        let mut extra = BTreeMap::new();
        extra.insert("layout_version".into(), json!(f.version));
        if let Some(v) = &f.mxw_version {
            extra.insert("mxw_version".into(), json!(v));
        }
        if let Some(p) = &f.plate {
            extra.insert("plate".into(), json!(p));
        }
        if let Some(t) = &r.started_at {
            extra.insert("recorded_at".into(), json!(t));
        }
        if r.rate_assumed {
            extra.insert("sample_rate_assumed".into(), json!(true));
        }
        extra.insert("lsb_v".into(), json!(r.lsb_v));
        TraceInfo {
            index: i as u32,
            name: Some(r.name.clone()),
            sample_rate_hz: r.sample_rate_hz,
            sample_count: r.matrix.cols,
            sweep_count: 1,
            channels,
            start_s: Some(0.0),
            extra,
        }
    }
}

impl Dataset for MaxwellDataset {
    fn info(&self) -> Result<FileInfo> {
        let f = &self.file;
        let mut notes = vec![format!(
            "MaxWell layout {} ({}): {} recording(s), values are stored level × lsb in µV (the level of 0 V is not subtracted)",
            f.version,
            f.plate.as_deref().unwrap_or("plate not named"),
            f.recordings.len()
        )];
        if f.recordings.iter().any(|r| r.rate_assumed) {
            notes.push(format!(
                "the file stores no frame rate: {MAXWELL_LEGACY_RATE_HZ} Hz is assumed (MaxOne's rate)"
            ));
        }
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: f.file_len,
            format: MaxwellReader.descriptor(),
            format_version: Some(f.version.clone()),
            images: Vec::new(),
            tables: Vec::new(),
            spectra: Vec::new(),
            traces: (0..f.recordings.len())
                .map(|i| self.trace_info(i))
                .collect(),
            plane_count: 0,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let f = &self.file;
        Ok(json!({
            "version": f.version,
            "mxw_version": f.mxw_version,
            "plate": f.plate,
            "recordings": f.recordings.iter().map(|r| json!({
                "name": r.name, "matrix": r.matrix.path, "rows": r.matrix.rows,
                "frames": r.matrix.cols, "lsb_v": r.lsb_v, "sample_rate_hz": r.sample_rate_hz,
                "filters": r.matrix.filters(), "channels": r.channels.len(),
            })).collect::<Vec<_>>(),
        }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for k in [
            "format_version",
            "traces[].sample_count",
            "traces[].channels[].scale",
            "traces[].channels[].unit",
        ] {
            p.insert(k.into(), Source::PriorArt);
        }
        for k in [
            "traces[].sample_rate_hz",
            "traces[].channels[].name",
            "traces[].extra.recorded_at",
        ] {
            p.insert(k.into(), Source::Inferred);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        Ok(self
            .file
            .recordings
            .iter()
            .map(|r| LsEntry {
                kind: "samples".into(),
                name: r.matrix.path.clone(),
                offset: None,
                size: None,
                image: None,
                details: json!({"rows": r.matrix.rows, "frames": r.matrix.cols, "channels": r.channels.len(), "filters": r.matrix.filters()}),
            })
            .collect())
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::unsupported(
            MAXWELL_FORMAT_ID,
            "image planes",
            "MaxWell recordings hold sampled signals: use `openreadout trace`.",
        ))
    }

    fn read_trace(&mut self, index: u32, sweep: u32, first: u64, max: u64) -> Result<Trace> {
        let Some(r) = self.file.recordings.get(index as usize) else {
            return Err(Error::Usage(format!(
                "trace index {index} out of range ({} traces)",
                self.file.recordings.len()
            )));
        };
        if sweep != 0 {
            return Err(Error::Usage(format!(
                "sweep {sweep} out of range (1 sweep)"
            )));
        }
        if first > r.matrix.cols {
            return Err(Error::Usage(format!(
                "first sample {first} is past the end ({} samples)",
                r.matrix.cols
            )));
        }
        let per = (r.matrix.pixel_type().bytes_per_sample() as u64 * r.matrix.rows).max(1);
        let count = max
            .min(r.matrix.cols - first)
            .min(MAX_MAXWELL_READ)
            .min((MAX_MAXWELL_READ_BYTES / per).max(1))
            .min((h5rec::MAX_H5_EXPANSION.saturating_mul(self.file.file_len) / per).max(1));
        let rows = r.matrix.read(
            &self.h5,
            &self.fs,
            &self.path,
            MAXWELL_FORMAT_ID,
            first,
            count,
        )?;
        let uv = r.lsb_v * 1e6;
        let channels = r
            .channels
            .iter()
            .map(|c| {
                rows[usize::try_from(c.row).unwrap_or(0)]
                    .iter()
                    .map(|v| v * uv)
                    .collect()
            })
            .collect();
        Ok(Trace {
            trace: index,
            sweep,
            first_sample: first,
            channels,
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), MAXWELL_FORMAT_ID);
        r.performed("layout version read; channel mapping against the sample matrix rows");
        for x in &self.file.findings {
            r.push(x.clone());
        }
        Ok(r)
    }
}

#[cfg(test)]
mod tests;
