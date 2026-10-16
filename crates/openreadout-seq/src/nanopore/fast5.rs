//! FAST5 (`.fast5`): HDF5 files of nanopore reads. Multi-read files (FAST5 1.0 and later) hold
//! one `read_<id>` group per read; single-read files (FAST5 0.6) hold `Raw/Reads/Read_<n>` and
//! `UniqueGlobalKey`. Each read has `Raw/Signal` (int16 ADC values, deflate or VBZ
//! compressed), `Raw` attributes (read id, number, start, duration, median before),
//! `channel_id` (channel number, digitisation, offset, range, sampling rate) and the run's
//! `tracking_id` and `context_tags`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use hdf5_pure::{AttrValue, Layout};
use openreadout_core::source::{Fs, Input};
use openreadout_core::{Error, Result};
use openreadout_hdf5::h5util::{attr_text, open_h5_in};

use super::{ReadMeta, RunMeta, SignalSource, vbz};

/// Format id of FAST5 files.
pub const FORMAT_ID: &str = "fast5";

/// HDF5 filter id of VBZ.
pub const FILTER_VBZ: u16 = 32020;

/// Most reads listed from one file.
const MAX_READS: usize = 1 << 20;

fn corrupt(d: impl Into<String>) -> Error {
    Error::corrupt(FORMAT_ID, d)
}

fn attrs(file: &hdf5_pure::File, group: &str) -> BTreeMap<String, AttrValue> {
    file.group(group)
        .and_then(|g| g.attrs())
        .map(|a| a.into_iter().collect())
        .unwrap_or_default()
}

fn num(a: &BTreeMap<String, AttrValue>, k: &str) -> Option<f64> {
    a.get(k)
        .and_then(attr_text)
        .and_then(|t| t.trim().parse::<f64>().ok())
        .filter(|x| x.is_finite())
}

fn text(a: &BTreeMap<String, AttrValue>, k: &str) -> Option<String> {
    a.get(k).and_then(attr_text).filter(|t| !t.is_empty())
}

/// Where one read's pieces are.
#[derive(Debug, Clone)]
struct ReadPaths {
    signal: String,
    raw: String,
    channel: String,
}

/// The signal of an open FAST5 file.
pub struct Fast5Signal {
    file: hdf5_pure::File,
    fs: Fs,
    path: PathBuf,
    reads: Vec<ReadPaths>,
}

impl std::fmt::Debug for Fast5Signal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Fast5Signal")
            .field("path", &self.path)
            .field("reads", &self.reads.len())
            .finish_non_exhaustive()
    }
}

impl SignalSource for Fast5Signal {
    fn raw(&mut self, read: usize) -> Result<Vec<i16>> {
        let p = self
            .reads
            .get(read)
            .ok_or_else(|| Error::Usage(format!("read {read} does not exist")))?;
        let ds = self
            .file
            .dataset(&p.signal)
            .map_err(|e| corrupt(format!("{}: {e}", p.signal)))?;
        let filters = ds.filter_pipeline();
        if !filters.iter().any(|f| f.id == FILTER_VBZ) {
            return ds
                .read_i16()
                .map_err(|e| corrupt(format!("{}: {e}", p.signal)));
        }
        if filters.len() != 1 {
            return Err(Error::unsupported(
                FORMAT_ID,
                "VBZ combined with other HDF5 filters",
                "Only signal compressed with VBZ alone (as MinKNOW writes it) or with deflate is read.",
            ));
        }
        let params: Vec<u32> = filters[0].client_data.clone();
        let n = ds
            .shape()
            .ok()
            .and_then(|s| s.first().copied())
            .unwrap_or(0) as usize;
        if n > vbz::MAX_SAMPLES {
            return Err(corrupt(format!("{}: {n} samples", p.signal)));
        }
        let Ok(Layout::Chunked { .. }) = ds.layout() else {
            return Err(corrupt(format!(
                "{}: VBZ data that is not chunked",
                p.signal
            )));
        };
        let mut chunks = ds
            .chunks()
            .map_err(|e| corrupt(format!("{}: {e}", p.signal)))?;
        chunks.sort_by_key(|c| c.offset.first().copied().unwrap_or(0));
        let f = self
            .fs
            .open(&self.path)
            .map_err(|e| Error::io(&self.path, e))?;
        let flen = f.size().map_err(|e| Error::io(&self.path, e))?;
        let mut out = Vec::with_capacity(n);
        for c in chunks {
            if c.address.saturating_add(c.storage_size) > flen || c.storage_size > 1 << 30 {
                return Err(corrupt(format!(
                    "{}: a chunk lies past the end of the file (truncated)",
                    p.signal
                )));
            }
            let mut raw = vec![0u8; c.storage_size as usize];
            f.read_exact_at(c.address, &mut raw)
                .map_err(|e| Error::io(&self.path, e))?;
            let v = if c.filter_mask & 1 == 1 {
                raw.as_chunks::<2>()
                    .0
                    .iter()
                    .map(|b| i16::from_le_bytes([b[0], b[1]]))
                    .collect()
            } else {
                vbz::fast5_chunk(&raw, &params)
                    .map_err(|e| corrupt(format!("{}: {e}", p.signal)))?
            };
            let at = c.offset.first().copied().unwrap_or(0) as usize;
            if at != out.len() {
                return Err(corrupt(format!("{}: chunks leave a gap", p.signal)));
            }
            out.extend(v);
        }
        out.truncate(n);
        Ok(out)
    }
}

/// An open FAST5 file.
#[derive(Debug)]
pub struct Fast5 {
    /// `file_version` of the root group.
    pub version: Option<String>,
    /// Reads.
    pub reads: Vec<ReadMeta>,
    /// The run (from the first read's `tracking_id` and `context_tags`).
    pub runs: Vec<RunMeta>,
    /// Signal access.
    pub signal: Fast5Signal,
    /// Signal compression of the first read (`vbz`, `gzip`, `none`).
    pub compression: String,
    /// Reads whose group holds base calls (`Analyses/Basecall_*`).
    pub basecalled: usize,
    /// File size.
    pub size: u64,
}

/// Open a FAST5 file.
pub fn open(input: &Input) -> Result<Fast5> {
    let fs = input.fs().clone();
    let path = input.path().to_path_buf();
    let size = input.metadata()?.len();
    let file = open_h5_in(&fs, &path, FORMAT_ID)?;
    let root_attrs = attrs(&file, "/");
    let version = text(&root_attrs, "file_version");
    let groups = file
        .root()
        .groups()
        .map_err(|e| corrupt(format!("root group: {e}")))?;
    let mut paths: Vec<(String, ReadPaths, String)> = Vec::new();
    let multi: Vec<&String> = groups.iter().filter(|g| g.starts_with("read_")).collect();
    let mut tracking_group = None;
    let mut context_group = None;
    if multi.is_empty() {
        // single-read FAST5: Raw/Reads/Read_<n>
        let reads = file
            .group("/Raw/Reads")
            .and_then(|g| g.groups())
            .map_err(|_| {
                Error::corrupt(
                    FORMAT_ID,
                    "neither read_<id> groups (multi-read FAST5) nor Raw/Reads (single-read FAST5)",
                )
            })?;
        for r in reads {
            let raw = format!("/Raw/Reads/{r}");
            paths.push((
                r.clone(),
                ReadPaths {
                    signal: format!("{raw}/Signal"),
                    raw,
                    channel: "/UniqueGlobalKey/channel_id".into(),
                },
                "/".into(),
            ));
        }
        tracking_group = Some("/UniqueGlobalKey/tracking_id".to_string());
        context_group = Some("/UniqueGlobalKey/context_tags".to_string());
    } else {
        if multi.len() > MAX_READS {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!("{} reads in one file", multi.len()),
                "Split the file (ont_fast5_api's multi_to_single_fast5 or a POD5 conversion) before reading it.",
            ));
        }
        for g in multi {
            let base = format!("/{g}");
            paths.push((
                g.trim_start_matches("read_").to_string(),
                ReadPaths {
                    signal: format!("{base}/Raw/Signal"),
                    raw: format!("{base}/Raw"),
                    channel: format!("{base}/channel_id"),
                },
                base.clone(),
            ));
            if tracking_group.is_none() {
                tracking_group = Some(format!("{base}/tracking_id"));
                context_group = Some(format!("{base}/context_tags"));
            }
        }
    }
    let tracking = tracking_group.map(|g| attrs(&file, &g)).unwrap_or_default();
    let context = context_group.map(|g| attrs(&file, &g)).unwrap_or_default();
    let mut reads = Vec::with_capacity(paths.len());
    let mut rate_any = None;
    let mut basecalled = 0;
    let mut channel_cache: BTreeMap<String, BTreeMap<String, AttrValue>> = BTreeMap::new();
    for (name, p, base) in &paths {
        let raw = attrs(&file, &p.raw);
        let ch = channel_cache
            .entry(p.channel.clone())
            .or_insert_with(|| attrs(&file, &p.channel))
            .clone();
        let digitisation = num(&ch, "digitisation").filter(|d| *d != 0.0);
        let range = num(&ch, "range");
        let offset = num(&ch, "offset").unwrap_or(0.0);
        let scale = match (digitisation, range) {
            (Some(d), Some(r)) => r / d,
            _ => 1.0,
        };
        let rate = num(&ch, "sampling_rate");
        rate_any = rate_any.or(rate);
        let n = file
            .dataset(&p.signal)
            .ok()
            .and_then(|d| d.shape().ok())
            .and_then(|s| s.first().copied());
        if base != "/" && file.group(&format!("{base}/Analyses")).is_ok() {
            basecalled += 1;
        }
        reads.push(ReadMeta {
            read_id: text(&raw, "read_id").unwrap_or_else(|| name.clone()),
            channel: text(&ch, "channel_number").and_then(|c| c.trim().parse().ok()),
            well: num(&raw, "start_mux").map(|m| m as u32),
            read_number: num(&raw, "read_number").map(|v| v as u64),
            start_sample: num(&raw, "start_time").map(|v| v as u64),
            num_samples: n.or_else(|| num(&raw, "duration").map(|v| v as u64)),
            median_before: num(&raw, "median_before"),
            open_pore_level: None,
            // FAST5 2.x stores a numeric code without its names: kept as the code
            end_reason: raw.get("end_reason").and_then(attr_text),
            end_reason_forced: None,
            pore_type: None,
            num_events: None,
            offset,
            scale,
            run: Some(0),
            run_name: text(&tracking, "run_id"),
            sample_rate: rate,
        });
    }
    let compression = paths
        .first()
        .and_then(|(_, p, _)| file.dataset(&p.signal).ok())
        .map_or_else(
            || "none".into(),
            |d| {
                let f = d.filter_pipeline();
                if f.iter().any(|x| x.id == FILTER_VBZ) {
                    "vbz"
                } else if f.iter().any(|x| x.id == 1) {
                    "gzip"
                } else {
                    "none"
                }
                .to_string()
            },
        );
    let tags = |a: &BTreeMap<String, AttrValue>| -> Vec<(String, String)> {
        a.iter()
            .filter_map(|(k, v)| attr_text(v).map(|t| (k.clone(), t)))
            .collect()
    };
    let run = RunMeta {
        acquisition_id: text(&tracking, "run_id").unwrap_or_default(),
        started_ms: None,
        protocol_started_ms: None,
        experiment: text(&tracking, "protocol_group_id"),
        flow_cell_id: text(&tracking, "flow_cell_id"),
        flow_cell_product: text(&tracking, "flow_cell_product_code"),
        protocol: text(&tracking, "exp_script_name"),
        protocol_run_id: text(&tracking, "protocol_run_id"),
        sample_id: text(&tracking, "sample_id"),
        sample_rate: rate_any.or_else(|| num(&context, "sample_frequency")),
        sequencing_kit: text(&context, "sequencing_kit"),
        position: text(&tracking, "device_id"),
        position_type: text(&tracking, "device_type"),
        software: text(&tracking, "version").map(|v| format!("MinKNOW {v}")),
        system_name: text(&tracking, "hostname"),
        system_type: None,
        adc_min: None,
        adc_max: None,
        context_tags: tags(&context),
        tracking_id: tags(&tracking),
    };
    let started = text(&tracking, "exp_start_time");
    let mut run = run;
    if let Some(s) = started {
        run.tracking_id.push(("exp_start_time".into(), s));
    }
    Ok(Fast5 {
        version,
        reads,
        runs: vec![run],
        signal: Fast5Signal {
            file,
            fs,
            path,
            reads: paths.into_iter().map(|(_, p, _)| p).collect(),
        },
        compression,
        basecalled,
        size,
    })
}
