//! `Dataset` for a Varian/Agilent VnmrJ data directory (`<name>.fid/` with `fid`, `procpar`,
//! `text`, `log`): one time-domain trace whose sweeps are the traces of `fid` in disk order.

use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use openreadout_core::Source;
use openreadout_core::experiment::{Acquisition, Experiment, Origin, Sample};
use openreadout_core::model::{
    CheckReport, FileInfo, Finding, LsEntry, SignalChannelInfo, Trace, TraceInfo,
};
use openreadout_core::provenance::ProvenanceMap;
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::source::{Fs, Input};
use openreadout_core::{Error, Plane, Result};
use serde_json::{Value, json};

use crate::varian_layout::{
    BLOCK_HEADER_BYTES, BlockHeader, FILE_HEADER_BYTES, FidHeader, VarianSampleType, decode_varian,
    find_varian_experiments_in, resolve_varian_dir_in,
};
use crate::varian_procpar::{Procpar, ProcparValues, parse_procpar};
use crate::{VARIAN_FORMAT_ID, VarianReader};

/// Largest `procpar` read (real ones are < 100 KiB).
const MAX_PROCPAR: u64 = 16 << 20;
/// Largest `text` file read.
const MAX_TEXT: u64 = 64 << 10;
/// Most values decoded by one `read_trace` call.
const MAX_READ_VALUES: u64 = 1 << 28;
/// Most block headers `check` walks.
const MAX_CHECKED_BLOCKS: u64 = 1 << 20;
/// Most files listed by `entries`.
const MAX_LISTED: usize = 4096;

/// An opened VnmrJ data directory.
#[derive(Debug)]
pub struct VarianDataset {
    path: PathBuf,
    /// Where the data directory is read from.
    fs: Fs,
    dir: PathBuf,
    header: FidHeader,
    /// `Err` holds why the header cannot be used to read data.
    sample_type: std::result::Result<VarianSampleType, String>,
    fid_len: u64,
    procpar: Option<Procpar>,
    text: Option<String>,
    first_block: Option<BlockHeader>,
    files: Vec<(String, u64)>,
}

fn read_limited(fs: &Fs, path: &Path, limit: u64) -> Option<Vec<u8>> {
    let meta = fs.metadata(path).ok()?;
    if !meta.is_file() || meta.len() > limit {
        return None;
    }
    fs.read(path).ok()
}

fn list_files(fs: &Fs, dir: &Path) -> Vec<(String, u64)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs.read_dir(&d) else { continue };
        for e in rd.filter_map(std::result::Result::ok) {
            if out.len() >= MAX_LISTED {
                return out;
            }
            let p = e.path();
            match e.file_type() {
                Ok(t) if t.is_dir() => stack.push(p),
                Ok(t) if t.is_file() => {
                    let rel = p
                        .strip_prefix(dir)
                        .unwrap_or(&p)
                        .to_string_lossy()
                        .replace('\\', "/");
                    out.push((rel, e.metadata().map_or(0, |m| m.len())));
                }
                _ => {}
            }
        }
    }
    out.sort();
    out
}

fn put(m: &mut BTreeMap<String, Value>, k: &str, v: Option<Value>) {
    if let Some(v) = v {
        m.insert(k.into(), v);
    }
}

/// `"VnmrJ VERSION 4.2 REVISION A"` → (`VnmrJ`, `4.2 REVISION A`).
fn software(pp: &Procpar) -> Option<(String, Option<String>)> {
    let v = pp.text("parver")?;
    Some(match v.split_once(" VERSION ") {
        Some((n, ver)) => (n.trim().to_string(), Some(ver.trim().to_string())),
        None => (v.to_string(), None),
    })
}

/// `20070606T193417` → `2007-06-06T19:34:17` (local time of the spectrometer; no zone).
fn vnmr_time(s: &str) -> Option<String> {
    let s = s.trim();
    let b = s.as_bytes();
    if b.len() < 15 || b[8] != b'T' || !b[..8].iter().chain(&b[9..15]).all(u8::is_ascii_digit) {
        return None;
    }
    Some(format!(
        "{}-{}-{}T{}:{}:{}",
        &s[0..4],
        &s[4..6],
        &s[6..8],
        &s[9..11],
        &s[11..13],
        &s[13..15]
    ))
}

/// A Varian nucleus name (`H1`, `C13`, `P31`) in the usual order (`1H`, `13C`, `31P`).
fn nucleus(tn: &str) -> String {
    let t = tn.trim();
    let split = t.find(|c: char| c.is_ascii_digit());
    match split {
        Some(i) if i > 0 && t[i..].chars().all(|c| c.is_ascii_digit()) => {
            format!("{}{}", &t[i..], &t[..i])
        }
        _ => t.to_string(),
    }
}

impl VarianDataset {
    /// Open a data directory (or its `fid`, `procpar`, `text` or `log`).
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    /// Open an [`Input`] (a local path, or a data directory held in memory).
    pub(crate) fn open_input(input: &Input) -> Result<Self> {
        let (path, fs) = (input.path(), input.fs());
        let Some(dir) = resolve_varian_dir_in(fs, path) else {
            if fs.is_dir(path) {
                let exps = find_varian_experiments_in(fs, path);
                if !exps.is_empty() {
                    let names: Vec<String> = exps
                        .iter()
                        .take(12)
                        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
                        .collect();
                    return Err(Error::Usage(format!(
                        "{} holds {} VnmrJ data directories ({}{}); pass one of them, e.g. {}",
                        path.display(),
                        exps.len(),
                        names.join(", "),
                        if exps.len() > 12 { ", …" } else { "" },
                        exps[0].display()
                    )));
                }
            }
            return Err(Error::UnknownFormat {
                path: path.to_path_buf(),
            });
        };
        let fid = dir.join("fid");
        let mut f = fs.open(&fid).map_err(|e| Error::io(&fid, e))?;
        let fid_len = f.metadata().map_err(|e| Error::io(&fid, e))?.len();
        let mut head = [0u8; 60];
        let n = f.read(&mut head).map_err(|e| Error::io(&fid, e))?;
        let header = FidHeader::parse(&head[..n]).ok_or_else(|| {
            Error::corrupt(VARIAN_FORMAT_ID, "fid is shorter than its 32-byte header")
        })?;
        let first_block = (header.block_headers >= 1 && header.block_count >= 1)
            .then(|| BlockHeader::parse(head.get(32..n).unwrap_or(&[])))
            .flatten();
        let procpar =
            read_limited(fs, &dir.join("procpar"), MAX_PROCPAR).map(|b| parse_procpar(&b));
        let text = read_limited(fs, &dir.join("text"), MAX_TEXT)
            .map(|b| crate::text::decode_text(&b).0)
            .filter(|t| !t.trim().is_empty());
        let sample_type = if header.problems().is_empty() {
            header.sample_type()
        } else {
            Err(header.problems().join("; "))
        };
        Ok(VarianDataset {
            path: path.to_path_buf(),
            fs: fs.clone(),
            files: list_files(fs, &dir),
            dir,
            header,
            sample_type,
            fid_len,
            procpar,
            text,
            first_block,
        })
    }

    /// The parsed `procpar`, when present.
    pub fn procpar(&self) -> Option<&Procpar> {
        self.procpar.as_ref()
    }

    fn sweeps(&self) -> u64 {
        u64::try_from(self.header.block_count).unwrap_or(0)
            * u64::try_from(self.header.traces_per_block).unwrap_or(0)
    }

    /// Complete blocks present in the file.
    fn blocks_in_file(&self) -> u64 {
        let bb = u64::try_from(self.header.block_bytes).unwrap_or(0);
        if bb == 0 {
            return 0;
        }
        (self.fid_len.saturating_sub(FILE_HEADER_BYTES) / bb)
            .min(u64::try_from(self.header.block_count).unwrap_or(0))
    }

    fn pp_real(&self, k: &str) -> Option<f64> {
        self.procpar.as_ref()?.real(k)
    }
    fn pp_text(&self, k: &str) -> Option<&str> {
        self.procpar.as_ref()?.text(k)
    }

    fn trace_info(&self) -> TraceInfo {
        let dtype = self
            .sample_type
            .as_ref()
            .map_or("float32", |t| t.dtype())
            .to_string();
        let channels = ["real", "imag"]
            .iter()
            .enumerate()
            .map(|(i, n)| SignalChannelInfo {
                index: i as u32,
                name: (*n).into(),
                unit: None,
                dtype: dtype.clone(),
                scale: 1.0,
                offset: 0.0,
                extra: BTreeMap::new(),
            })
            .collect();
        let samples = u64::try_from(self.header.points).unwrap_or(0) / 2;
        let sw = self.pp_real("sw").filter(|v| *v > 0.0);
        let mut e = BTreeMap::new();
        e.insert("kind".into(), json!("time_domain"));
        e.insert("file".into(), json!("fid"));
        if let Some(sw) = sw {
            e.insert(
                "axis".into(),
                json!({"quantity": "time", "unit": "s", "first": 0.0, "step": 1.0 / sw, "size": samples}),
            );
        }
        let text = |k: &str| self.pp_text(k).map(|s| json!(s));
        let real = |k: &str| self.pp_real(k).map(|v| json!(v));
        let int = |k: &str| {
            self.pp_real(k)
                .filter(|v| v.fract() == 0.0 && v.abs() < 9e15)
                .map(|v| json!(v as i64))
        };
        put(
            &mut e,
            "nucleus",
            self.pp_text("tn").map(|t| json!(nucleus(t))),
        );
        put(&mut e, "nucleus_name", text("tn"));
        put(&mut e, "spectrometer_frequency_mhz", real("sfrq"));
        put(
            &mut e,
            "decoupler_nucleus",
            self.pp_text("dn").map(|t| json!(nucleus(t))),
        );
        put(&mut e, "decoupler_frequency_mhz", real("dfrq"));
        put(&mut e, "spectral_width_hz", real("sw"));
        if let (Some(sw), Some(sf)) = (sw, self.pp_real("sfrq").filter(|v| *v > 0.0)) {
            e.insert("spectral_width_ppm".into(), json!(sw / sf));
        }
        put(&mut e, "time_domain_size", int("np"));
        put(&mut e, "scans", int("nt"));
        put(&mut e, "completed_scans", int("ct"));
        put(&mut e, "dummy_scans", int("ss"));
        put(&mut e, "receiver_gain", real("gain"));
        put(&mut e, "pulse_program", text("seqfil"));
        put(&mut e, "experiment", text("pslabel"));
        put(&mut e, "solvent", text("solvent"));
        put(&mut e, "temperature_c", real("temp"));
        put(
            &mut e,
            "acquired_at",
            self.pp_text("time_run")
                .and_then(vnmr_time)
                .map(|t| json!(t)),
        );
        put(
            &mut e,
            "completed_at",
            self.pp_text("time_complete")
                .and_then(vnmr_time)
                .map(|t| json!(t)),
        );
        put(&mut e, "acquisition_date", text("date"));
        put(&mut e, "console", text("console"));
        put(&mut e, "instrument", text("console"));
        put(&mut e, "system_name", text("systemname_"));
        put(&mut e, "probe", text("probe_"));
        put(&mut e, "operator", text("operator_"));
        put(&mut e, "sample_name", text("samplename"));
        put(&mut e, "comment", text("comment"));
        if let Some(t) = &self.text {
            e.insert("title".into(), json!(t.trim()));
        }
        if let Some((name, version)) = self.procpar.as_ref().and_then(software) {
            e.insert("software".into(), json!(name));
            put(&mut e, "software_version", version.map(|v| json!(v)));
        }
        if let Some(a) = self.procpar.as_ref().and_then(|p| p.text("array")) {
            e.insert("arrayed_parameters".into(), json!(a));
        }
        put(&mut e, "array_size", int("arraydim"));
        let mut dims = Vec::new();
        for (k, (ni, sw_n, phase, dn)) in [
            ("ni", "sw1", "phase", "dn"),
            ("ni2", "sw2", "phase2", "dn2"),
            ("ni3", "sw3", "phase3", "dn3"),
        ]
        .iter()
        .enumerate()
        {
            let Some(n) = self.pp_real(ni).filter(|v| *v >= 1.0) else {
                continue;
            };
            let mut d = BTreeMap::new();
            d.insert("dimension".to_string(), json!(k + 2));
            d.insert("increments".to_string(), json!(n as i64));
            put(
                &mut d,
                "spectral_width_hz",
                self.pp_real(sw_n).map(|v| json!(v)),
            );
            if let Some(ProcparValues::Real(v)) = self
                .procpar
                .as_ref()
                .and_then(|p| p.get(phase))
                .map(|p| &p.values)
            {
                d.insert("phase_values".to_string(), json!(v));
            }
            put(
                &mut d,
                "nucleus",
                self.pp_text(dn).map(|t| json!(nucleus(t))),
            );
            dims.push(json!(d));
        }
        if !dims.is_empty() {
            e.insert("indirect_dimensions".into(), json!(dims));
        }
        e.insert("block_count".into(), json!(self.header.block_count));
        e.insert(
            "traces_per_block".into(),
            json!(self.header.traces_per_block),
        );
        e.insert(
            "block_headers_per_block".into(),
            json!(self.header.block_headers),
        );
        e.insert("status_code".into(), json!(self.header.status as u16));
        e.insert("status_flags".into(), json!(self.header.status_names()));
        e.insert("byte_order".into(), json!("big-endian"));
        if let Ok(t) = &self.sample_type {
            e.insert("sample_type".into(), json!(t.dtype()));
        }
        if let Some(b) = &self.first_block {
            e.insert("block_scale".into(), json!(b.scale));
            e.insert("block_completed_scans".into(), json!(b.completed_scans));
        }
        TraceInfo {
            index: 0,
            name: Some("fid".into()),
            sample_rate_hz: sw.unwrap_or(0.0),
            sample_count: samples,
            sweep_count: u32::try_from(self.sweeps()).unwrap_or(u32::MAX),
            channels,
            start_s: Some(0.0),
            extra: e,
        }
    }

    /// Other processed or companion data in the directory (listed, not decoded).
    fn processed_files(&self) -> Vec<&str> {
        self.files
            .iter()
            .map(|(n, _)| n.as_str())
            .filter(|n| {
                let leaf = n.rsplit('/').next().unwrap_or(n);
                matches!(leaf, "phasefile" | "data") || n.starts_with("datdir/")
            })
            .collect()
    }
}

impl Dataset for VarianDataset {
    fn experiment(&self) -> Option<Experiment> {
        let mut e = Experiment::default();
        let origin = |from: &str| Origin {
            source: Source::Inferred,
            from: from.to_string(),
        };
        let placeholder = |s: &str| {
            let t = s.trim();
            t.is_empty() || t.eq_ignore_ascii_case("none") || t.eq_ignore_ascii_case("unknown")
        };
        let mut s = Sample::default();
        if let Some(v) = self.pp_text("samplename").filter(|v| !placeholder(v)) {
            s.id = Some(v.to_string());
            s.source_field = Some("procpar samplename".into());
            e.provenance
                .insert("sample.id".into(), origin("procpar samplename"));
        }
        if let Some(t) = &self.text {
            let line = t
                .lines()
                .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
                .find(|l| !l.is_empty());
            if let Some(line) = line
                && s.id.as_deref() != Some(line.as_str())
            {
                s.name = Some(line);
                e.provenance.insert("sample.name".into(), origin("text"));
            }
        }
        if s != Sample::default() {
            e.sample = Some(s);
        }
        if let Some(op) = self.pp_text("operator_").filter(|v| !placeholder(v)) {
            e.acquisition = Some(Acquisition {
                operator: Some(op.to_string()),
                ..Acquisition::default()
            });
            e.provenance
                .insert("acquisition.operator".into(), origin("procpar operator_"));
        }
        (!e.is_empty()).then_some(e)
    }

    fn info(&self) -> Result<FileInfo> {
        let mut notes = Vec::new();
        if self.procpar.is_none() {
            notes.push("no procpar: acquisition parameters (spectral width, nucleus, frequency) are unknown; the fid header alone gives the layout".into());
        } else if self.procpar.as_ref().is_some_and(|p| !p.issues.is_empty()) {
            notes.push("procpar has syntax problems; run `check`".into());
        }
        if let Err(e) = &self.sample_type {
            notes.push(format!("fid header not usable: {e}"));
        }
        let blocks = self.blocks_in_file();
        if blocks < u64::try_from(self.header.block_count).unwrap_or(0) {
            notes.push(format!(
                "fid holds {blocks} of {} blocks (truncated or a stopped acquisition); run `check`",
                self.header.block_count
            ));
        }
        if self.header.traces_per_block > 1 || self.sweeps() > 1 {
            notes.push("sweeps are the fid's traces in disk order (block by block); arrayed and multidimensional data are not reordered by phase or array".into());
        }
        notes.push("values are returned as stored: block scale factors and drift corrections are reported, not applied".into());
        let processed = self.processed_files();
        if !processed.is_empty() {
            notes.push(format!(
                "processed data ({}) listed by `info --view structure`, not decoded",
                processed.join(", ")
            ));
        }
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.files.iter().map(|(_, s)| *s).sum(),
            format: VarianReader.descriptor(),
            format_version: self.pp_text("parver").map(str::to_string).or_else(|| {
                self.pp_real("parversion")
                    .map(|v| format!("procpar version {v}"))
            }),
            images: Vec::new(),
            tables: Vec::new(),
            spectra: Vec::new(),
            traces: vec![self.trace_info()],
            plane_count: 0,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let h = &self.header;
        Ok(json!({
            "directory": self.dir.display().to_string(),
            "file_header": {
                "block_count": h.block_count, "traces_per_block": h.traces_per_block,
                "points": h.points, "element_bytes": h.element_bytes, "trace_bytes": h.trace_bytes,
                "block_bytes": h.block_bytes, "version_code": h.version_code,
                "status": h.status as u16, "status_flags": h.status_names(),
                "block_headers": h.block_headers,
            },
            "first_block_header": self.first_block.map(|b| json!({
                "scale": b.scale, "status": b.status as u16, "index": b.index, "mode": b.mode as u16,
                "completed_scans": b.completed_scans, "left_phase": b.left_phase,
                "right_phase": b.right_phase, "level": b.level, "tilt": b.tilt,
            })),
            "procpar": self.procpar.as_ref().map(Procpar::to_json),
            "text": self.text,
        }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for (k, s) in [
            ("format_version", Source::Inferred),
            ("traces[0].sample_count", Source::PriorArt),
            ("traces[0].sweep_count", Source::PriorArt),
            ("traces[0].sample_rate_hz", Source::PriorArt),
            ("traces[0].channels[].dtype", Source::PriorArt),
            ("traces[0].extra.axis", Source::PriorArt),
            ("traces[0].extra.nucleus", Source::Inferred),
            (
                "traces[0].extra.spectrometer_frequency_mhz",
                Source::Inferred,
            ),
            ("traces[0].extra.spectral_width_hz", Source::PriorArt),
            ("traces[0].extra.spectral_width_ppm", Source::Inferred),
            ("traces[0].extra.scans", Source::Inferred),
            ("traces[0].extra.pulse_program", Source::Inferred),
            ("traces[0].extra.solvent", Source::Inferred),
            ("traces[0].extra.temperature_c", Source::Inferred),
            ("traces[0].extra.acquired_at", Source::Inferred),
            ("traces[0].extra.software", Source::Inferred),
            ("traces[0].extra.indirect_dimensions", Source::PriorArt),
            ("traces[0].extra.status_flags", Source::PriorArt),
            ("traces[0].extra.block_scale", Source::PriorArt),
        ] {
            p.insert(k.into(), s);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let mut out = Vec::new();
        for (name, size) in &self.files {
            let leaf = name.rsplit('/').next().unwrap_or(name);
            let kind = match name.as_str() {
                "fid" => "time-domain",
                "procpar" => "parameters",
                "text" => "title",
                "log" => "log",
                _ if matches!(leaf, "phasefile" | "data") || name.starts_with("datdir/") => {
                    "processed-data"
                }
                _ => "file",
            };
            let details = match kind {
                "time-domain" => json!({
                    "blocks": self.header.block_count, "blocks_in_file": self.blocks_in_file(),
                    "traces_per_block": self.header.traces_per_block, "points": self.header.points,
                    "sample_type": self.sample_type.as_ref().ok().map(|t| t.dtype()),
                }),
                "parameters" => json!({
                    "parameters": self.procpar.as_ref().map_or(0, |p| p.params.len()),
                    "issues": self.procpar.as_ref().map_or(0, |p| p.issues.len()),
                }),
                "processed-data" => json!({"decoded": false}),
                _ => Value::Null,
            };
            out.push(LsEntry {
                kind: kind.into(),
                name: name.clone(),
                offset: None,
                size: Some(*size),
                image: None,
                details,
            });
        }
        Ok(out)
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::unsupported(
            VARIAN_FORMAT_ID,
            "image planes",
            "VnmrJ NMR data are traces (FIDs), not images: use `openreadout trace`, `openreadout export --to csv`, or the openreadout_trace MCP tool.",
        ))
    }

    fn read_trace(&mut self, index: u32, sweep: u32, first: u64, max: u64) -> Result<Trace> {
        if index != 0 {
            return Err(Error::Usage(format!(
                "trace index {index} out of range (0..1)"
            )));
        }
        let ty = self
            .sample_type
            .clone()
            .map_err(|e| Error::corrupt(VARIAN_FORMAT_ID, format!("fid header not usable: {e}")))?;
        let sweeps = self.sweeps();
        if u64::from(sweep) >= sweeps {
            return Err(Error::Usage(format!(
                "sweep {sweep} out of range ({sweeps} sweeps)"
            )));
        }
        let n = u64::try_from(self.header.points).unwrap_or(0) / 2;
        if first > n {
            return Err(Error::Usage(format!(
                "first sample {first} is past the end ({n} samples)"
            )));
        }
        let count = max.min(n - first).min(MAX_READ_VALUES);
        let ntr = u64::try_from(self.header.traces_per_block)
            .unwrap_or(1)
            .max(1);
        let (block, tr) = (u64::from(sweep) / ntr, u64::from(sweep) % ntr);
        let w = ty.width();
        let offset = FILE_HEADER_BYTES
            .checked_add(
                block
                    .checked_mul(u64::try_from(self.header.block_bytes).unwrap_or(0))
                    .ok_or_else(|| Error::corrupt(VARIAN_FORMAT_ID, "offset overflows"))?,
            )
            .and_then(|o| {
                o.checked_add(
                    u64::try_from(self.header.block_headers).unwrap_or(0) * BLOCK_HEADER_BYTES,
                )
            })
            .and_then(|o| o.checked_add(tr * u64::try_from(self.header.trace_bytes).unwrap_or(0)))
            .and_then(|o| o.checked_add(first * 2 * w))
            .ok_or_else(|| Error::corrupt(VARIAN_FORMAT_ID, "offset overflows"))?;
        let len = count * 2 * w;
        let end = offset.saturating_add(len);
        if end > self.fid_len {
            return Err(Error::corrupt_at(
                VARIAN_FORMAT_ID,
                offset,
                format!(
                    "fid needs bytes up to {end} for sweep {sweep} but has {} (truncated)",
                    self.fid_len
                ),
            ));
        }
        let fid = self.dir.join("fid");
        let mut f = self.fs.open(&fid).map_err(|e| Error::io(&fid, e))?;
        f.seek(SeekFrom::Start(offset))
            .map_err(|e| Error::io(&fid, e))?;
        let mut buf = vec![
            0u8;
            usize::try_from(len)
                .map_err(|_| Error::corrupt(VARIAN_FORMAT_ID, "read too large"))?
        ];
        f.read_exact(&mut buf).map_err(|e| Error::io(&fid, e))?;
        let vals = decode_varian(&buf, ty);
        let mut re = Vec::with_capacity(count as usize);
        let mut im = Vec::with_capacity(count as usize);
        for pair in vals.as_chunks::<2>().0 {
            re.push(pair[0]);
            im.push(pair[1]);
        }
        Ok(Trace {
            trace: 0,
            sweep,
            first_sample: first,
            channels: vec![re, im],
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), VARIAN_FORMAT_ID);
        r.performed("fid file header: counts and sizes consistent (trace = points × element size, block = traces + 28-byte block headers), status bits agree with the element size");
        r.performed("fid length against the declared blocks");
        r.performed("block headers: 1-based sequential block numbers, scale factors");
        r.performed("procpar parses (header, values and enumeration lines) and agrees with the fid header (np, arraydim)");
        let h = self.header;
        for p in h.problems() {
            r.push(Finding::error("bad_header", format!("fid header: {p}")));
        }
        let declared = h.declared_len();
        if h.problems().is_empty() {
            if self.fid_len < declared {
                let bb = u64::try_from(h.block_bytes).unwrap_or(0);
                let at = FILE_HEADER_BYTES + self.blocks_in_file() * bb;
                r.push(
                    Finding::error(
                        "truncated",
                        format!(
                            "fid has {} bytes; {} blocks of {} bytes need {declared}: {} complete blocks (a truncated copy or a stopped acquisition)",
                            self.fid_len, h.block_count, h.block_bytes, self.blocks_in_file()
                        ),
                    )
                    .at(at),
                );
            } else if self.fid_len > declared {
                r.push(Finding::warning(
                    "extra_bytes",
                    format!(
                        "fid has {} bytes after the declared blocks",
                        self.fid_len - declared
                    ),
                ));
            }
            // block headers
            if h.block_headers >= 1 {
                let fid = self.dir.join("fid");
                let mut f = self.fs.open(&fid).map_err(|e| Error::io(&fid, e))?;
                let bb = u64::try_from(h.block_bytes).unwrap_or(0);
                let n = self.blocks_in_file().min(MAX_CHECKED_BLOCKS);
                let (mut out_of_order, mut scaled) = (0u64, 0u64);
                let mut first_bad = None;
                let mut buf = [0u8; 28];
                for b in 0..n {
                    f.seek(SeekFrom::Start(FILE_HEADER_BYTES + b * bb))
                        .map_err(|e| Error::io(&fid, e))?;
                    f.read_exact(&mut buf).map_err(|e| Error::io(&fid, e))?;
                    let Some(bh) = BlockHeader::parse(&buf) else {
                        break;
                    };
                    if i64::from(bh.index) != (b as i64 + 1) & 0xffff
                        && i64::from(bh.index as u16) != (b as i64 + 1) & 0xffff
                    {
                        out_of_order += 1;
                        first_bad.get_or_insert(b);
                    }
                    if bh.scale != 0 {
                        scaled += 1;
                    }
                }
                if out_of_order > 0 {
                    r.push(Finding::warning(
                        "block_index",
                        format!("{out_of_order} block headers do not carry their 1-based block number (first: block {})", first_bad.unwrap_or(0) + 1),
                    ));
                }
                if scaled > 0 {
                    r.push(Finding::info(
                        "block_scale",
                        format!("{scaled} blocks have a non-zero scale factor; values are returned as stored"),
                    ));
                }
            }
        }
        match &self.procpar {
            None => r.push(Finding::warning(
                "missing_parameters",
                "no procpar next to fid: acquisition parameters are unknown",
            )),
            Some(pp) => {
                for m in &pp.issues {
                    r.push(Finding::warning(
                        "parameter_syntax",
                        format!("procpar: {m}"),
                    ));
                }
                if pp.latin1 {
                    r.push(Finding::info(
                        "non_utf8",
                        "procpar is not UTF-8; read as Latin-1",
                    ));
                }
                if let Some(np) = pp.real("np")
                    && (np - f64::from(h.points)).abs() > 0.5
                {
                    r.push(Finding::warning(
                        "np_mismatch",
                        format!(
                            "procpar np = {np} but the fid header has {} points per trace",
                            h.points
                        ),
                    ));
                }
                if let Some(ad) = pp.real("arraydim")
                    && (ad - self.sweeps() as f64).abs() > 0.5
                {
                    r.push(Finding::warning(
                        "array_mismatch",
                        format!(
                            "procpar arraydim = {ad} but the fid holds {} traces",
                            self.sweeps()
                        ),
                    ));
                }
                if pp.real("sw").is_none() {
                    r.push(Finding::warning(
                        "missing_parameter",
                        "procpar has no sw: no sampling rate",
                    ));
                }
            }
        }
        if h.points % 2 != 0 {
            r.push(Finding::error(
                "odd_points",
                format!(
                    "{} values per trace cannot be real/imaginary pairs",
                    h.points
                ),
            ));
        }
        Ok(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_times() {
        assert_eq!(nucleus("H1"), "1H");
        assert_eq!(nucleus("C13"), "13C");
        assert_eq!(nucleus("P31"), "31P");
        assert_eq!(nucleus("lk"), "lk");
        assert_eq!(
            vnmr_time("20070606T193417").as_deref(),
            Some("2007-06-06T19:34:17")
        );
        assert_eq!(
            vnmr_time("20070606T193416X07PMWedJun").as_deref(),
            Some("2007-06-06T19:34:16")
        );
        assert_eq!(vnmr_time("junk"), None);
    }
}
