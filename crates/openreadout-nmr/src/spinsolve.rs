//! `Dataset` for a Magritek Spinsolve experiment directory (`<yymmdd-hhmmss> <protocol>
//! (<suffix>)/` with `acqu.par`, optional `proc.par` and `processing.script`, and Prospa data
//! files `data.1d`/`data.2d`, `spectrum.1d`, …): one trace per data file, the FID first.
//!
//! Prospa data files: a 32-byte header of eight little-endian u32 (the ASCII words `PROS`,
//! `DATA`, `V1.1` stored byte-reversed, a data type code and the x/y/z/q sizes), then float32
//! values: type 501 interleaved complex pairs, 503 an x block then real values, 504 an x block
//! then interleaved complex pairs, row after row. Notes and vocabulary:
//! `docs/formats/magritek-spinsolve.md`; provenance: `docs/provenance/magritek-spinsolve.md`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_core::Source;
use openreadout_core::bytes::le_u32;
use openreadout_core::experiment::{Acquisition, Experiment, Origin, Sample};
use openreadout_core::model::{
    CheckReport, FileInfo, Finding, LsEntry, SignalChannelInfo, Trace, TraceInfo,
};
use openreadout_core::provenance::ProvenanceMap;
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::source::{Fs, Input};
use openreadout_core::{Error, Plane, Result};
use serde_json::{Value, json};

use crate::{SPINSOLVE_FORMAT_ID, SpinsolveReader};

/// First eight bytes of a Prospa data file (`PROS` `DATA` as little-endian words).
pub const DATA_MAGIC: &[u8; 8] = b"SORPATAD";
/// First four bytes of every Prospa file (data and plot files).
pub const PROSPA_MAGIC: &[u8; 4] = b"SORP";
/// Header length of a Prospa data file.
pub const HEADER_BYTES: u64 = 32;
/// Data type: interleaved complex float32 pairs, no x axis.
pub const TYPE_COMPLEX: u32 = 501;
/// Data type: an x block of float32 then real float32 values.
pub const TYPE_XY_REAL: u32 = 503;
/// Data type: an x block of float32 then interleaved complex float32 pairs.
pub const TYPE_XY_COMPLEX: u32 = 504;
/// Names of the time-domain data file, in the order they are preferred.
pub const FID_NAMES: [&str; 5] = ["data.1d", "fid.1d", "data.2d", "data.3d", "data.4d"];

/// Largest parameter file read.
const MAX_PAR: u64 = 4 << 20;
/// Most values decoded by one `read_trace` call.
const MAX_READ_VALUES: u64 = 1 << 26;
/// Most files listed.
const MAX_LISTED: usize = 4096;
/// Deepest directory level searched for experiments under a folder.
const SEARCH_DEPTH: usize = 4;

/// The 32-byte header of a Prospa data file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProspaHeader {
    /// Version text (`V1.1`), from the byte-reversed third word.
    pub version: [u8; 4],
    /// Data type code (501, 503, 504 seen).
    pub data_type: u32,
    /// x, y, z, q sizes.
    pub dims: [u32; 4],
}

impl ProspaHeader {
    /// Parse the header; `None` unless the bytes start with the data magic.
    pub fn parse(b: &[u8]) -> Option<Self> {
        if b.len() < 32 || &b[..8] != DATA_MAGIC {
            return None;
        }
        let u = |i: usize| le_u32(b, i).unwrap_or(0);
        let mut version = [b[11], b[10], b[9], b[8]];
        if !version.iter().all(u8::is_ascii_graphic) {
            version = [b[8], b[9], b[10], b[11]];
        }
        Some(ProspaHeader {
            version,
            data_type: u(12),
            dims: [u(16), u(20), u(24), u(28)],
        })
    }

    /// Version text, e.g. `V1.1`.
    pub fn version_text(&self) -> String {
        String::from_utf8_lossy(&self.version).into_owned()
    }

    /// Points per row (x size).
    pub fn points(&self) -> u64 {
        u64::from(self.dims[0])
    }

    /// Rows (y·z·q; a zero size counts as 1).
    pub fn rows(&self) -> Option<u64> {
        self.dims[1..]
            .iter()
            .try_fold(1u64, |a, &d| a.checked_mul(u64::from(d.max(1))))
    }

    /// Bytes per point of a row (x value included), for the known data types.
    pub fn point_bytes(&self) -> Option<u64> {
        match self.data_type {
            TYPE_COMPLEX => Some(8),
            TYPE_XY_REAL => Some(8),
            TYPE_XY_COMPLEX => Some(12),
            _ => None,
        }
    }

    /// Whether the values are complex pairs.
    pub fn is_complex(&self) -> bool {
        matches!(self.data_type, TYPE_COMPLEX | TYPE_XY_COMPLEX)
    }

    /// Whether rows start with an x block.
    pub fn has_x(&self) -> bool {
        matches!(self.data_type, TYPE_XY_REAL | TYPE_XY_COMPLEX)
    }

    /// Bytes of one row.
    pub fn row_bytes(&self) -> Option<u64> {
        self.points().checked_mul(self.point_bytes()?)
    }

    /// File length the header declares.
    pub fn declared_len(&self) -> Option<u64> {
        self.row_bytes()?
            .checked_mul(self.rows()?)?
            .checked_add(HEADER_BYTES)
    }

    /// Problems that make the values unreadable.
    pub fn problems(&self) -> Vec<String> {
        let mut v = Vec::new();
        if self.point_bytes().is_none() {
            v.push(format!(
                "data type {} is not one of the decoded types (501 complex, 503 x + real, 504 x + complex)",
                self.data_type
            ));
        }
        if self.dims[0] == 0 {
            v.push("x size is 0".into());
        }
        if self.declared_len().is_none() && self.point_bytes().is_some() {
            v.push(format!("sizes {:?} overflow", self.dims));
        }
        v
    }
}

/// A parsed `acqu.par`/`proc.par`: `name = value` lines in file order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParFile {
    /// Parameters in file order (a repeated name keeps its last value in [`ParFile::get`]).
    pub params: Vec<(String, Value)>,
    /// Lines that are not `name = value`.
    pub issues: Vec<String>,
}

/// One parameter value: a quoted text, a number, a `[a,b]` list of numbers, else the raw text.
pub fn parse_par_value(raw: &str) -> Value {
    let v = raw.trim();
    if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
        return json!(&v[1..v.len() - 1]);
    }
    if let Ok(i) = v.parse::<i64>() {
        return json!(i);
    }
    if let Ok(f) = v.parse::<f64>()
        && f.is_finite()
    {
        return json!(f);
    }
    if let Some(inner) = v.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        let parts: Vec<Option<f64>> = inner
            .split(',')
            .map(|p| p.trim().parse::<f64>().ok().filter(|f| f.is_finite()))
            .collect();
        if !inner.trim().is_empty() && parts.iter().all(Option::is_some) {
            return json!(parts.into_iter().flatten().collect::<Vec<f64>>());
        }
    }
    json!(v)
}

/// Parse `name = value` lines (UTF-8, else Latin-1).
pub fn parse_par(bytes: &[u8]) -> ParFile {
    let (text, _) = crate::text::decode_text(bytes);
    let mut out = ParFile::default();
    for (i, line) in text.lines().enumerate() {
        let l = line.trim();
        if l.is_empty() {
            continue;
        }
        match l.split_once('=') {
            Some((k, v)) if !k.trim().is_empty() => {
                out.params.push((k.trim().to_string(), parse_par_value(v)));
            }
            _ => out
                .issues
                .push(format!("line {}: not `name = value`", i + 1)),
        }
    }
    out
}

impl ParFile {
    /// The last value stored under `name`.
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.params
            .iter()
            .rev()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v)
    }

    /// A number (numeric text accepted).
    pub fn num(&self, name: &str) -> Option<f64> {
        match self.get(name)? {
            Value::Number(n) => n.as_f64(),
            Value::String(s) => s.trim().parse().ok(),
            _ => None,
        }
        .filter(|v: &f64| v.is_finite())
    }

    /// A text (numbers as written).
    pub fn text(&self, name: &str) -> Option<String> {
        match self.get(name)? {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        }
    }

    /// The parameters as a JSON object (last value wins).
    pub fn to_json(&self) -> Value {
        let mut m = serde_json::Map::new();
        for (k, v) in &self.params {
            m.insert(k.clone(), v.clone());
        }
        Value::Object(m)
    }
}

/// `Phase(p0, p1)` in a `processing.script` (degrees, the software's sign).
pub fn script_phase(script: &str) -> Option<(f64, f64)> {
    let i = script.find("Phase(")?;
    let rest = &script[i + 6..];
    let args = &rest[..rest.find(')')?];
    let mut it = args.split(',').map(|s| s.trim().parse::<f64>().ok());
    let p0 = it.next()??;
    let p1 = it.next().flatten().unwrap_or(0.0);
    (p0.is_finite() && p1.is_finite()).then_some((p0, p1))
}

/// `expName` `yymmdd-hhmmss <protocol> (<suffix>)` → (start `20yy-mm-ddThh:mm:ss`, suffix).
pub fn split_exp_name(name: &str) -> (Option<String>, Option<String>) {
    let n = name.trim();
    let b = n.as_bytes();
    let stamp =
        (b.len() >= 13 && b[6] == b'-' && b[..6].iter().chain(&b[7..13]).all(u8::is_ascii_digit))
            .then(|| {
                format!(
                    "20{}-{}-{}T{}:{}:{}",
                    &n[0..2],
                    &n[2..4],
                    &n[4..6],
                    &n[7..9],
                    &n[9..11],
                    &n[11..13]
                )
            })
            .filter(|s| {
                let mo: u32 = s[5..7].parse().unwrap_or(0);
                let d: u32 = s[8..10].parse().unwrap_or(0);
                let h: u32 = s[11..13].parse().unwrap_or(99);
                let mi: u32 = s[14..16].parse().unwrap_or(99);
                let se: u32 = s[17..19].parse().unwrap_or(99);
                (1..=12).contains(&mo) && (1..=31).contains(&d) && h < 24 && mi < 60 && se < 61
            });
    let suffix = n
        .ends_with(')')
        .then(|| {
            n.rfind('(')
                .map(|i| n[i + 1..n.len() - 1].trim().to_string())
        })
        .flatten()
        .filter(|s| !s.is_empty());
    (stamp, suffix)
}

/// What the x block of a data file measures, judged from its values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum XAxis {
    /// Evenly spaced time in milliseconds (step = the dwell time).
    TimeMs {
        /// First value (ms).
        first: f64,
        /// Step (ms).
        step: f64,
    },
    /// Evenly spaced chemical shift in ppm (span = the spectral width over `b1Freq`).
    Ppm {
        /// First value (ppm).
        first: f64,
        /// Step (ppm).
        step: f64,
    },
}

/// One Prospa data file of the directory.
#[derive(Debug, Clone)]
pub struct DataFile {
    /// File name relative to the experiment directory.
    pub name: String,
    /// Its header.
    pub header: ProspaHeader,
    /// File length.
    pub len: u64,
    /// Whether this is the experiment's FID.
    pub fid: bool,
    /// The x block, when it is a recognised axis.
    pub axis: Option<XAxis>,
}

impl DataFile {
    /// Complete rows present in the file.
    fn rows_present(&self) -> u64 {
        match self.header.row_bytes() {
            Some(rb) if rb > 0 => {
                (self.len.saturating_sub(HEADER_BYTES) / rb).min(self.header.rows().unwrap_or(0))
            }
            _ => 0,
        }
    }
    /// Whether x values are returned as a channel.
    fn x_channel(&self) -> bool {
        self.header.has_x() && self.axis.is_none()
    }
}

/// The experiment directory `path` names (the directory itself, or the directory of a file in
/// it): one holding `acqu.par` or a Prospa data file.
pub fn resolve_spinsolve_dir_in(fs: &Fs, path: &Path) -> Option<PathBuf> {
    let dir = if fs.is_dir(path) {
        path.to_path_buf()
    } else {
        let name = path.file_name()?.to_string_lossy().to_ascii_lowercase();
        let known = name == "acqu.par"
            || name == "proc.par"
            || name == "processing.script"
            || is_data_name(&name)
            || is_plot_name(&name);
        let parent = path.parent()?.to_path_buf();
        // a Prospa data file under any name (renamed, copied out of its directory)
        if !known && has_data_magic(fs, path) {
            return Some(parent);
        }
        if !known {
            return None;
        }
        parent
    };
    is_experiment_dir(fs, &dir).then_some(dir)
}

fn is_plot_name(name: &str) -> bool {
    name.rsplit_once('.')
        .is_some_and(|(_, e)| e.eq_ignore_ascii_case("pt1") || e.eq_ignore_ascii_case("pt2"))
}

fn is_data_name(lower: &str) -> bool {
    [".1d", ".2d", ".3d", ".4d"]
        .iter()
        .any(|e| lower.ends_with(e))
}

fn has_data_magic(fs: &Fs, p: &Path) -> bool {
    let Ok(f) = fs.open(p) else { return false };
    let mut b = [0u8; 8];
    f.read_exact_at(0, &mut b).is_ok() && &b == DATA_MAGIC
}

/// A directory with an `acqu.par` that parses as Spinsolve parameters, or with a Prospa data
/// file.
fn is_experiment_dir(fs: &Fs, dir: &Path) -> bool {
    let Ok(rd) = fs.read_dir(dir) else {
        return false;
    };
    let mut names = Vec::new();
    for e in rd.filter_map(std::result::Result::ok).take(MAX_LISTED) {
        if e.file_type().is_ok_and(|t| t.is_file()) {
            names.push(e.file_name().to_string_lossy().to_string());
        }
    }
    if names.iter().any(|n| n == "acqu.par") {
        let par = dir.join("acqu.par");
        if let Ok(m) = fs.metadata(&par)
            && m.len() <= MAX_PAR
            && let Ok(b) = fs.read(&par)
        {
            let p = parse_par(&b);
            if p.get("nrPnts").is_some()
                && (p.get("b1Freq").is_some() || p.get("b1Freq1H").is_some())
            {
                return true;
            }
        }
    }
    names
        .iter()
        .filter(|n| is_data_name(&n.to_ascii_lowercase()))
        .any(|n| has_data_magic(fs, &dir.join(n)))
}

/// Spinsolve experiment directories under `dir` (at most a few levels deep), sorted.
pub fn find_spinsolve_experiments_in(fs: &Fs, dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![(dir.to_path_buf(), 0usize)];
    let mut visited = 0usize;
    while let Some((d, depth)) = stack.pop() {
        visited += 1;
        if visited > 2_000 || out.len() >= 10_000 {
            break;
        }
        let Ok(rd) = fs.read_dir(&d) else { continue };
        for e in rd.filter_map(std::result::Result::ok) {
            if !e.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let p = e.path();
            if e.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            if is_experiment_dir(fs, &p) {
                out.push(p);
            } else if depth + 1 < SEARCH_DEPTH {
                stack.push((p, depth + 1));
            }
        }
    }
    out.sort();
    out
}

fn list_files(fs: &Fs, dir: &Path) -> Vec<(String, u64)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs.read_dir(&d) else { continue };
        for e in rd.filter_map(std::result::Result::ok) {
            if out.len() >= MAX_LISTED {
                out.sort();
                return out;
            }
            let p = e.path();
            match e.file_type() {
                // sub-experiments (e.g. `1D_T1IRT2/0000/`) are listed, not opened
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

/// An opened Spinsolve experiment directory.
#[derive(Debug)]
pub struct SpinsolveDataset {
    path: PathBuf,
    fs: Fs,
    dir: PathBuf,
    acqu: Option<ParFile>,
    proc: Option<ParFile>,
    script: Option<String>,
    files: Vec<(String, u64)>,
    data: Vec<DataFile>,
    /// Data files whose header could not be read (name, why).
    skipped: Vec<(String, String)>,
}

impl SpinsolveDataset {
    /// Open an experiment directory (or a file in it).
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    /// Open an [`Input`] (a local path, or a directory held in memory).
    pub(crate) fn open_input(input: &Input) -> Result<Self> {
        let (path, fs) = (input.path(), input.fs());
        let Some(dir) = resolve_spinsolve_dir_in(fs, path) else {
            if fs.is_dir(path) {
                let exps = find_spinsolve_experiments_in(fs, path);
                if !exps.is_empty() {
                    let names: Vec<String> = exps
                        .iter()
                        .take(8)
                        .filter_map(|p| {
                            p.strip_prefix(path)
                                .ok()
                                .map(|r| r.to_string_lossy().to_string())
                        })
                        .collect();
                    return Err(Error::Usage(format!(
                        "{} holds {} Spinsolve experiment directories ({}{}); pass one of them, e.g. {}",
                        path.display(),
                        exps.len(),
                        names.join(", "),
                        if exps.len() > 8 { ", …" } else { "" },
                        exps[0].display()
                    )));
                }
            }
            return Err(Error::UnknownFormat {
                path: path.to_path_buf(),
            });
        };
        let read_par = |name: &str| {
            let p = dir.join(name);
            let m = fs.metadata(&p).ok()?;
            if !m.is_file() || m.len() > MAX_PAR {
                return None;
            }
            fs.read(&p).ok()
        };
        let acqu = read_par("acqu.par").map(|b| parse_par(&b));
        let proc = read_par("proc.par").map(|b| parse_par(&b));
        let script = read_par("processing.script").map(|b| crate::text::decode_text(&b).0);
        let files = list_files(fs, &dir);
        let mut data = Vec::new();
        let mut skipped = Vec::new();
        let opened = (!fs.is_dir(path))
            .then(|| path.file_name().map(|n| n.to_string_lossy().to_string()))
            .flatten()
            .filter(|n| !is_data_name(&n.to_ascii_lowercase()) && has_data_magic(fs, path));
        for (name, len) in &files {
            if name.contains('/')
                || !(is_data_name(&name.to_ascii_lowercase()) || opened.as_ref() == Some(name))
            {
                continue;
            }
            let p = dir.join(name);
            let mut head = [0u8; 32];
            let ok = fs
                .open(&p)
                .and_then(|f| f.read_exact_at(0, &mut head))
                .is_ok();
            match ok.then(|| ProspaHeader::parse(&head)).flatten() {
                Some(header) => data.push(DataFile {
                    name: name.clone(),
                    header,
                    len: *len,
                    fid: false,
                    axis: None,
                }),
                None => skipped.push((
                    name.clone(),
                    "not a Prospa data file (no PROS DATA header)".to_string(),
                )),
            }
        }
        let mut ds = SpinsolveDataset {
            path: path.to_path_buf(),
            fs: fs.clone(),
            dir,
            acqu,
            proc,
            script,
            files,
            data,
            skipped,
        };
        ds.classify();
        Ok(ds)
    }

    fn acq_num(&self, k: &str) -> Option<f64> {
        self.acqu.as_ref()?.num(k)
    }
    fn acq_text(&self, k: &str) -> Option<String> {
        self.acqu
            .as_ref()?
            .text(k)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    /// Spectral width in Hz (`bandwidth` kHz).
    pub fn spectral_width_hz(&self) -> Option<f64> {
        self.acq_num("bandwidth")
            .map(|v| v * 1000.0)
            .filter(|v| *v > 0.0)
    }

    /// The 0 ppm frequency in MHz (`b1Freq`).
    pub fn reference_frequency_mhz(&self) -> Option<f64> {
        self.acq_num("b1Freq").filter(|v| *v > 0.0)
    }

    /// Carrier offset from the 0 ppm frequency in Hz (`lowestFrequency` + width/2).
    pub fn carrier_offset_hz(&self) -> Option<f64> {
        Some(self.acq_num("lowestFrequency")? + self.spectral_width_hz()? / 2.0)
    }

    /// Mark the FID and recognise x blocks; FID first, then the rest by name.
    fn classify(&mut self) {
        let points = self
            .acq_num("nrPnts")
            .filter(|v| *v >= 1.0)
            .map(|v| v as u64);
        let fid_idx = FID_NAMES.iter().find_map(|want| {
            self.data.iter().position(|d| {
                d.name.eq_ignore_ascii_case(want)
                    && d.header.is_complex()
                    && d.header.problems().is_empty()
                    && points.is_none_or(|p| p == d.header.points())
            })
        });
        if let Some(i) = fid_idx {
            self.data[i].fid = true;
            let d = self.data.remove(i);
            self.data.insert(0, d);
        }
        let dwell_ms = self.acq_num("dwellTime").map(|us| us / 1000.0);
        let ppm_step = match (self.spectral_width_hz(), self.reference_frequency_mhz()) {
            (Some(sw), Some(f)) => Some(sw / f),
            _ => None,
        };
        for i in 0..self.data.len() {
            let d = &self.data[i];
            if !d.header.has_x() || !d.header.problems().is_empty() || d.header.rows() != Some(1) {
                continue;
            }
            let n = d.header.points();
            if n < 2 || d.len < HEADER_BYTES + 4 * n {
                continue;
            }
            let Some(x) = self.read_x(d) else { continue };
            let Some((first, step)) = linear(&x) else {
                continue;
            };
            let close = |a: f64, b: f64| (a - b).abs() <= 1e-4 * b.abs().max(1e-12);
            let axis = if dwell_ms.is_some_and(|w| close(step, w)) {
                Some(XAxis::TimeMs { first, step })
            } else if ppm_step.is_some_and(|s| close(step.abs() * n as f64, s)) {
                Some(XAxis::Ppm { first, step })
            } else {
                None
            };
            self.data[i].axis = axis;
        }
    }

    fn read_x(&self, d: &DataFile) -> Option<Vec<f64>> {
        let n = usize::try_from(d.header.points()).ok()?;
        let f = self.fs.open(&self.dir.join(&d.name)).ok()?;
        let mut buf = vec![0u8; n.checked_mul(4)?];
        f.read_exact_at(HEADER_BYTES, &mut buf).ok()?;
        Some(
            buf.as_chunks::<4>()
                .0
                .iter()
                .map(|c| f64::from(f32::from_le_bytes(*c)))
                .collect(),
        )
    }

    fn fid(&self) -> Option<&DataFile> {
        self.data.first().filter(|d| d.fid)
    }

    fn channels(d: &DataFile) -> Vec<SignalChannelInfo> {
        let mut names: Vec<&str> = Vec::new();
        if d.x_channel() {
            names.push("x");
        }
        if d.header.is_complex() {
            names.extend(["real", "imag"]);
        } else {
            names.push("y");
        }
        names
            .iter()
            .enumerate()
            .map(|(i, n)| SignalChannelInfo {
                index: i as u32,
                name: (*n).into(),
                unit: None,
                dtype: "float32".into(),
                scale: 1.0,
                offset: 0.0,
                extra: BTreeMap::new(),
            })
            .collect()
    }

    fn acquisition_extra(&self, e: &mut BTreeMap<String, Value>) {
        let text = |k: &str| self.acq_text(k).map(|s| json!(s));
        let num = |k: &str| self.acq_num(k).map(|v| json!(v));
        let int = |k: &str| {
            self.acq_num(k)
                .filter(|v| v.fract() == 0.0 && v.abs() < 9e15)
                .map(|v| json!(v as i64))
        };
        put(e, "nucleus", text("nucleus").or_else(|| text("rxChannel")));
        put(e, "receiver_channel", text("rxChannel"));
        let sw = self.spectral_width_hz();
        let reference = self.reference_frequency_mhz();
        put(e, "spectral_width_hz", sw.map(|v| json!(v)));
        put(e, "reference_frequency_mhz", reference.map(|v| json!(v)));
        if let (Some(r), Some(off)) = (reference, self.carrier_offset_hz()) {
            e.insert("spectrometer_frequency_mhz".into(), json!(r + off * 1e-6));
            e.insert("carrier_offset_hz".into(), json!(off));
            e.insert("carrier_offset_ppm".into(), json!(off / r));
        }
        if let (Some(sw), Some(r)) = (sw, reference) {
            e.insert("spectral_width_ppm".into(), json!(sw / r));
        }
        put(e, "proton_frequency_mhz", num("b1Freq1H"));
        put(e, "time_domain_size", int("nrPnts"));
        put(e, "scans", int("nrScans"));
        put(e, "steps", int("nrSteps"));
        put(e, "receiver_gain", num("rxGain"));
        put(e, "receiver_phase_deg", num("rxPhase"));
        put(e, "dwell_time_us", num("dwellTime"));
        put(e, "acquisition_time_ms", num("acqTime"));
        put(e, "repetition_time_ms", num("repTime"));
        put(e, "duration_s", num("duration"));
        put(e, "pulse_program", text("experiment"));
        put(e, "experiment_name", text("expName"));
        put(e, "instrument", text("specType"));
        put(e, "instrument_serial", text("specID"));
        put(e, "software_version", text("softwareVersion"));
        if self.acqu.is_some() {
            e.insert("software".into(), json!("Spinsolve"));
        }
        if let Some(name) = self.acq_text("expName") {
            let (at, suffix) = split_exp_name(&name);
            put(e, "acquired_at", at.map(|v| json!(v)));
            put(e, "sample_name", suffix.map(|v| json!(v)));
        }
        put(
            e,
            "apodization",
            text("filterType").filter(|_| {
                self.acq_text("filter")
                    .is_some_and(|f| f.eq_ignore_ascii_case("yes"))
            }),
        );
    }

    fn trace_info(&self, index: u32, d: &DataFile) -> TraceInfo {
        let mut e = BTreeMap::new();
        e.insert("file".into(), json!(d.name));
        e.insert("data_type".into(), json!(d.header.data_type));
        e.insert("dimensions".into(), json!(d.header.dims));
        let n = d.header.points();
        let sw = self.spectral_width_hz();
        let mut rate = 0.0;
        if d.fid {
            e.insert("kind".into(), json!("time_domain"));
            // the dwell time from the spectral width; the float32 x block only without one
            let step = match (sw, d.axis) {
                (Some(s), _) => Some(1.0 / s),
                (None, Some(XAxis::TimeMs { step, .. })) => Some(step / 1000.0),
                _ => None,
            };
            if let Some(step) = step {
                e.insert(
                    "axis".into(),
                    json!({"quantity": "time", "unit": "s", "first": 0.0, "step": step, "size": n}),
                );
            }
            rate = step.map_or(0.0, |s| 1.0 / s);
            self.acquisition_extra(&mut e);
        } else {
            match d.axis {
                Some(XAxis::Ppm { first, step }) => {
                    e.insert("kind".into(), json!("spectrum"));
                    let last = first + step * n.saturating_sub(1) as f64;
                    let mut a = json!({"quantity": "chemical_shift", "unit": "ppm", "first": first, "last": last, "step": step, "size": n});
                    if let Some(f) = self.reference_frequency_mhz() {
                        a["spectrometer_frequency_mhz"] = json!(f);
                    }
                    e.insert("axis".into(), a);
                    put(
                        &mut e,
                        "nucleus",
                        self.acq_text("nucleus").map(|v| json!(v)),
                    );
                }
                Some(XAxis::TimeMs { first, step }) => {
                    e.insert(
                        "axis".into(),
                        json!({"quantity": "time", "unit": "s", "first": first / 1000.0, "step": step / 1000.0, "size": n}),
                    );
                    if step > 0.0 {
                        rate = 1000.0 / step;
                    }
                }
                None => {}
            }
        }
        let rows = d.header.rows().unwrap_or(0);
        TraceInfo {
            index,
            name: Some(d.name.clone()),
            sample_rate_hz: rate,
            sample_count: n,
            sweep_count: u32::try_from(rows).unwrap_or(u32::MAX),
            channels: Self::channels(d),
            start_s: match (d.fid, d.axis) {
                (true, _) => Some(0.0),
                (false, Some(XAxis::TimeMs { first, .. })) => Some(first / 1000.0),
                _ => None,
            },
            extra: e,
        }
    }

    fn file_kind(&self, name: &str) -> &'static str {
        let lower = name.to_ascii_lowercase();
        let leaf = lower.rsplit('/').next().unwrap_or(&lower);
        if !name.contains('/')
            && let Some(d) = self.data.iter().find(|d| d.name == name)
        {
            return if d.fid { "time-domain" } else { "data" };
        }
        match leaf {
            "acqu.par" | "proc.par" | "acqu.par.bak" | "protocol.par" | "gradients.par" => {
                "parameters"
            }
            "processing.script" => "processing-script",
            _ if is_plot_name(leaf) => "plot",
            _ if lower.starts_with("ppcode/") => "pulse-program",
            _ if leaf.rsplit_once('.').is_some_and(|(_, e)| e == "mnova") => "processed-document",
            _ => "file",
        }
    }
}

/// `(first, step)` when the values are evenly spaced (relative 1e-3 of the step, float32 x).
fn linear(x: &[f64]) -> Option<(f64, f64)> {
    let n = x.len();
    if n < 2 || !x.iter().all(|v| v.is_finite()) {
        return None;
    }
    let step = (x[n - 1] - x[0]) / (n - 1) as f64;
    if step == 0.0 {
        return None;
    }
    let tol = 1e-3 * step.abs() + 1e-6 * x[0].abs().max(x[n - 1].abs());
    x.iter()
        .enumerate()
        .all(|(i, v)| (v - (x[0] + step * i as f64)).abs() <= tol.max(step.abs() * 1e-3))
        .then_some((x[0], step))
}

impl Dataset for SpinsolveDataset {
    fn experiment(&self) -> Option<Experiment> {
        let mut e = Experiment::default();
        let origin = |from: &str| Origin {
            source: Source::Inferred,
            from: from.to_string(),
        };
        let name = self.acq_text("expName")?;
        let (at, suffix) = split_exp_name(&name);
        if let Some(s) = suffix {
            e.sample = Some(Sample {
                name: Some(s),
                source_field: Some("acqu.par expName (suffix)".into()),
                ..Sample::default()
            });
            e.provenance
                .insert("sample.name".into(), origin("acqu.par expName"));
        }
        if let Some(at) = at {
            e.acquisition = Some(Acquisition {
                started_at: Some(at),
                duration_s: self.acq_num("duration").filter(|v| *v >= 0.0),
                ..Acquisition::default()
            });
            e.provenance
                .insert("acquisition.started_at".into(), origin("acqu.par expName"));
            if e.acquisition
                .as_ref()
                .is_some_and(|a| a.duration_s.is_some())
            {
                e.provenance
                    .insert("acquisition.duration_s".into(), origin("acqu.par duration"));
            }
        }
        (!e.is_empty()).then_some(e)
    }

    fn info(&self) -> Result<FileInfo> {
        let mut notes = Vec::new();
        if self.acqu.is_none() {
            notes.push("no acqu.par: spectral width, frequencies and nucleus are unknown; the data files' headers alone give the layout".into());
        }
        if self.fid().is_none() {
            notes.push("no complex time-domain file (data.1d, fid.1d, data.2d) with nrPnts points: FID processing is not available".into());
        }
        for d in &self.data {
            for p in d.header.problems() {
                notes.push(format!("{}: {p}; listed, not decoded", d.name));
            }
            if d.header.problems().is_empty() {
                let rows = d.header.rows().unwrap_or(0);
                if d.rows_present() < rows {
                    notes.push(format!(
                        "{} holds {} of {rows} rows (truncated or a stopped acquisition); run `check`",
                        d.name,
                        d.rows_present()
                    ));
                }
                if d.header.has_x() && rows > 1 {
                    notes.push(format!(
                        "{}: {rows} rows with an x block are not decoded (no public file shows the layout)",
                        d.name
                    ));
                }
            }
        }
        if let Some(d) = self.fid()
            && d.header.rows().unwrap_or(1) > 1
        {
            notes.push(format!(
                "{}: sweeps are the {} rows of the series (nrSteps) in file order",
                d.name,
                d.header.rows().unwrap_or(1)
            ));
        }
        for (n, why) in &self.skipped {
            notes.push(format!("{n}: {why}"));
        }
        let plots: Vec<&str> = self
            .files
            .iter()
            .map(|(n, _)| n.as_str())
            .filter(|n| self.file_kind(n) == "plot")
            .collect();
        if !plots.is_empty() {
            notes.push(format!(
                "plot files ({}) listed by `info --view structure`, not decoded",
                plots.join(", ")
            ));
        }
        let subs: std::collections::BTreeSet<&str> = self
            .files
            .iter()
            .filter_map(|(n, _)| {
                let lower = n.to_ascii_lowercase();
                let leaf = lower.rsplit('/').next().unwrap_or(&lower);
                (n.contains('/') && is_data_name(leaf))
                    .then(|| n.rsplit_once('/').map_or("", |x| x.0))
            })
            .collect();
        if !subs.is_empty() {
            notes.push(format!(
                "sub-experiments in {} {} are not read from here; open each directory",
                subs.len(),
                if subs.len() == 1 { "folder" } else { "folders" }
            ));
        }
        notes.push("values are float32 as stored; the frequency sense is the vendor's (FID processing conjugates, see docs/formats/magritek-spinsolve.md)".into());
        let traces = self
            .data
            .iter()
            .filter(|d| d.header.problems().is_empty())
            .enumerate()
            .map(|(i, d)| self.trace_info(i as u32, d))
            .collect();
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.files.iter().map(|(_, s)| *s).sum(),
            format: SpinsolveReader.descriptor(),
            format_version: self
                .acq_text("softwareVersion")
                .map(|v| format!("Spinsolve {v}"))
                .or_else(|| self.data.first().map(|d| d.header.version_text())),
            images: Vec::new(),
            tables: Vec::new(),
            spectra: Vec::new(),
            traces,
            plane_count: 0,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let proc_phase = self
            .proc
            .as_ref()
            .and_then(|p| Some((p.num("p0Phase")?, p.num("p1Phase").unwrap_or(0.0))));
        Ok(json!({
            "directory": self.dir.display().to_string(),
            "acqu_par": self.acqu.as_ref().map(ParFile::to_json),
            "proc_par": self.proc.as_ref().map(ParFile::to_json),
            "processing_script": self.script,
            "script_phase_deg": self.script.as_deref().and_then(script_phase),
            "proc_phase_deg": proc_phase,
            "data_files": self.data.iter().map(|d| json!({
                "name": d.name, "version": d.header.version_text(), "data_type": d.header.data_type,
                "dimensions": d.header.dims, "bytes": d.len, "fid": d.fid,
                "x_axis": match d.axis {
                    Some(XAxis::TimeMs { first, step }) => json!({"unit": "ms", "first": first, "step": step}),
                    Some(XAxis::Ppm { first, step }) => json!({"unit": "ppm", "first": first, "step": step}),
                    None => Value::Null,
                },
            })).collect::<Vec<_>>(),
        }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for (k, s) in [
            ("format_version", Source::Inferred),
            ("traces[].sample_count", Source::PriorArt),
            ("traces[].sweep_count", Source::Inferred),
            ("traces[].channels[].dtype", Source::PriorArt),
            ("traces[0].sample_rate_hz", Source::PriorArt),
            ("traces[0].extra.axis", Source::PriorArt),
            ("traces[0].extra.spectral_width_hz", Source::PriorArt),
            ("traces[0].extra.reference_frequency_mhz", Source::PriorArt),
            ("traces[0].extra.carrier_offset_hz", Source::PriorArt),
            (
                "traces[0].extra.spectrometer_frequency_mhz",
                Source::Inferred,
            ),
            ("traces[0].extra.nucleus", Source::Inferred),
            ("traces[0].extra.scans", Source::Inferred),
            ("traces[0].extra.pulse_program", Source::Inferred),
            ("traces[0].extra.acquired_at", Source::Inferred),
            ("traces[0].extra.sample_name", Source::Inferred),
            ("traces[0].extra.instrument", Source::Inferred),
            ("traces[0].extra.instrument_serial", Source::Inferred),
            ("traces[0].extra.software_version", Source::Inferred),
            ("traces[].extra.axis", Source::Inferred),
        ] {
            p.insert(k.into(), s);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let mut out = Vec::new();
        for (name, size) in &self.files {
            let kind = self.file_kind(name);
            let details = match (kind, self.data.iter().find(|d| &d.name == name)) {
                ("time-domain" | "data", Some(d)) => json!({
                    "data_type": d.header.data_type, "dimensions": d.header.dims,
                    "rows_in_file": d.rows_present(),
                    "decoded": d.header.problems().is_empty(),
                }),
                ("parameters", _) => {
                    if name == "acqu.par" {
                        json!({"parameters": self.acqu.as_ref().map_or(0, |p| p.params.len())})
                    } else if name == "proc.par" {
                        json!({"parameters": self.proc.as_ref().map_or(0, |p| p.params.len())})
                    } else {
                        Value::Null
                    }
                }
                ("plot" | "processed-document", _) => json!({"decoded": false}),
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
            SPINSOLVE_FORMAT_ID,
            "image planes",
            "Spinsolve NMR data are traces (FIDs and spectra), not images: use `openreadout trace`, `openreadout analyze nmr-peaks`, or `openreadout export --format csv`.",
        ))
    }

    fn read_trace(&mut self, index: u32, sweep: u32, first: u64, max: u64) -> Result<Trace> {
        let decoded: Vec<&DataFile> = self
            .data
            .iter()
            .filter(|d| d.header.problems().is_empty())
            .collect();
        let d = decoded.get(index as usize).copied().ok_or_else(|| {
            Error::Usage(format!(
                "trace index {index} out of range (0..{})",
                decoded.len()
            ))
        })?;
        let h = d.header;
        let rows = h.rows().unwrap_or(0);
        if u64::from(sweep) >= rows {
            return Err(Error::Usage(format!(
                "sweep {sweep} out of range ({rows} sweeps)"
            )));
        }
        if h.has_x() && rows > 1 {
            return Err(Error::unsupported(
                SPINSOLVE_FORMAT_ID,
                format!("{}: several rows with an x block", d.name),
                "Only single-row x/y files and complex series without an x block are decoded; open an issue with the file.",
            ));
        }
        let n = h.points();
        if first > n {
            return Err(Error::Usage(format!(
                "first sample {first} is past the end ({n} samples)"
            )));
        }
        let count = max.min(n - first).min(MAX_READ_VALUES);
        let row_bytes = h
            .row_bytes()
            .ok_or_else(|| Error::corrupt(SPINSOLVE_FORMAT_ID, "row size overflows"))?;
        let row0 = HEADER_BYTES
            .checked_add(
                u64::from(sweep)
                    .checked_mul(row_bytes)
                    .ok_or_else(|| Error::corrupt(SPINSOLVE_FORMAT_ID, "offset overflows"))?,
            )
            .ok_or_else(|| Error::corrupt(SPINSOLVE_FORMAT_ID, "offset overflows"))?;
        let (x_off, y_off, y_width) = if h.has_x() {
            (Some(row0), row0 + n * 4, if h.is_complex() { 8 } else { 4 })
        } else {
            (None, row0, 8)
        };
        let end = y_off.saturating_add(n.saturating_mul(y_width));
        if end > d.len {
            return Err(Error::corrupt_at(
                SPINSOLVE_FORMAT_ID,
                row0,
                format!(
                    "{} needs bytes up to {end} for row {sweep} but has {} (truncated)",
                    d.name, d.len
                ),
            ));
        }
        let path = self.dir.join(&d.name);
        let f = self.fs.open(&path).map_err(|e| Error::io(&path, e))?;
        let read = |off: u64, len: u64| -> Result<Vec<f64>> {
            let mut buf =
                vec![
                    0u8;
                    usize::try_from(len)
                        .map_err(|_| Error::corrupt(SPINSOLVE_FORMAT_ID, "read too large"))?
                ];
            f.read_exact_at(off, &mut buf)
                .map_err(|e| Error::io(&path, e))?;
            Ok(buf
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| f64::from(f32::from_le_bytes(*c)))
                .collect())
        };
        let mut channels = Vec::new();
        if let Some(xo) = x_off
            && d.x_channel()
        {
            channels.push(read(xo + first * 4, count * 4)?);
        }
        let vals = read(y_off + first * y_width, count * y_width)?;
        if h.is_complex() {
            let mut re = Vec::with_capacity(vals.len() / 2);
            let mut im = Vec::with_capacity(vals.len() / 2);
            for p in vals.as_chunks::<2>().0 {
                re.push(p[0]);
                im.push(p[1]);
            }
            channels.push(re);
            channels.push(im);
        } else {
            channels.push(vals);
        }
        Ok(Trace {
            trace: index,
            sweep,
            first_sample: first,
            channels,
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), SPINSOLVE_FORMAT_ID);
        r.performed("acqu.par and proc.par parse as `name = value` lines");
        r.performed(
            "Prospa data headers: PROS DATA magic, known data type, sizes that do not overflow",
        );
        r.performed("data file lengths against the declared rows");
        r.performed("the FID's size against nrPnts and its rows against nrSteps");
        match &self.acqu {
            None => r.push(Finding::warning(
                "missing_parameters",
                "no acqu.par in the experiment directory: acquisition parameters are unknown",
            )),
            Some(p) => {
                for m in &p.issues {
                    r.push(Finding::warning(
                        "parameter_syntax",
                        format!("acqu.par: {m}"),
                    ));
                }
                for k in ["bandwidth", "b1Freq", "lowestFrequency", "nrPnts"] {
                    if p.get(k).is_none() {
                        r.push(Finding::info(
                            "missing_parameter",
                            format!("acqu.par has no {k}"),
                        ));
                    }
                }
            }
        }
        if let Some(p) = &self.proc {
            for m in &p.issues {
                r.push(Finding::warning(
                    "parameter_syntax",
                    format!("proc.par: {m}"),
                ));
            }
        }
        for d in &self.data {
            for p in d.header.problems() {
                r.push(Finding::error("bad_header", format!("{}: {p}", d.name)));
            }
            if let Some(declared) = d.header.declared_len() {
                if d.len < declared {
                    r.push(
                        Finding::error(
                            "truncated",
                            format!(
                                "{} has {} bytes; its header declares {declared} ({} complete rows of {})",
                                d.name,
                                d.len,
                                d.rows_present(),
                                d.header.rows().unwrap_or(0)
                            ),
                        )
                        .at(d.len),
                    );
                } else if d.len > declared {
                    r.push(Finding::warning(
                        "extra_bytes",
                        format!(
                            "{} has {} bytes after the declared data",
                            d.name,
                            d.len - declared
                        ),
                    ));
                }
            }
        }
        if let Some(d) = self.fid() {
            if let Some(steps) = self.acq_num("nrSteps")
                && d.header.rows().is_some_and(|r| r > 1)
                && (steps - d.header.rows().unwrap_or(0) as f64).abs() > 0.5
            {
                r.push(Finding::warning(
                    "rows_mismatch",
                    format!(
                        "acqu.par nrSteps = {steps} but {} holds {} rows",
                        d.name,
                        d.header.rows().unwrap_or(0)
                    ),
                ));
            }
        } else if self.acqu.is_some() && !self.data.is_empty() {
            r.push(Finding::info(
                "no_fid",
                "no complex data file with nrPnts points: the data files are not an FID",
            ));
        }
        for (n, why) in &self.skipped {
            r.push(Finding::warning("unreadable_data", format!("{n}: {why}")));
        }
        Ok(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_and_names() {
        assert_eq!(parse_par_value("\"1H\""), json!("1H"));
        assert_eq!(parse_par_value(" 4096"), json!(4096));
        assert_eq!(parse_par_value("43.45168"), json!(43.451_68));
        assert_eq!(parse_par_value("[-5,15]"), json!([-5.0, 15.0]));
        assert_eq!(parse_par_value("2.00.27 Alpha"), json!("2.00.27 Alpha"));
        let (at, s) = split_exp_name("230213-131158 Proton (lyogel)");
        assert_eq!(at.as_deref(), Some("2023-02-13T13:11:58"));
        assert_eq!(s.as_deref(), Some("lyogel"));
        let (at, s) = split_exp_name("241220-104035 T1_mincodeSAVE ()");
        assert_eq!(at.as_deref(), Some("2024-12-20T10:40:35"));
        assert_eq!(s, None);
        assert_eq!(split_exp_name("991399-000000 x").0, None);
        assert_eq!(
            script_phase("Phase(-0.351563,0);\nZoom(-5,15);"),
            Some((-0.351_563, 0.0))
        );
        assert_eq!(script_phase("Zoom(-5,15);"), None);
    }

    #[test]
    fn header() {
        let mut b = Vec::new();
        b.extend_from_slice(b"SORPATAD1.1V");
        for v in [501u32, 4096, 1, 1, 1] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        let h = ProspaHeader::parse(&b).unwrap();
        assert_eq!(h.version_text(), "V1.1");
        assert_eq!(h.declared_len(), Some(32 + 4096 * 8));
        assert!(h.problems().is_empty());
        b[12] = 0xf4; // 500
        assert!(!ProspaHeader::parse(&b).unwrap().problems().is_empty());
        assert!(ProspaHeader::parse(b"SORPPLD1").is_none());
        let mut big = b.clone();
        big[12..16].copy_from_slice(&504u32.to_le_bytes());
        for i in 0..4 {
            big[16 + 4 * i..20 + 4 * i].copy_from_slice(&u32::MAX.to_le_bytes());
        }
        let h = ProspaHeader::parse(&big).unwrap();
        assert_eq!(h.declared_len(), None);
        assert!(!h.problems().is_empty());
    }

    #[test]
    fn linear_axes() {
        let x: Vec<f64> = (0..100u16)
            .map(|i| f64::from(0.2f32 * f32::from(i)))
            .collect();
        let (f, s) = linear(&x).unwrap();
        assert!(f.abs() < 1e-9 && (s - 0.2).abs() < 1e-6);
        assert!(linear(&[1.0, 2.0, 4.0]).is_none());
        assert!(linear(&[1.0, 1.0]).is_none());
    }
}
