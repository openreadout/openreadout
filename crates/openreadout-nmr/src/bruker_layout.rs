//! Bruker experiment directory layout: which files exist, how `fid`/`ser` and processed files
//! are laid out on disk. See `docs/formats/bruker-nmr.md`.

use std::path::{Path, PathBuf};

use openreadout_core::source::{DirEntry, EntryMeta, Fs};
use openreadout_core::{Error, Result};

use crate::BRUKER_FORMAT_ID;
use crate::bruker_params::{ParamFile, parse_param_file};

/// Largest parameter file we read (real ones are < 64 KiB).
const MAX_PARAM_FILE: u64 = 16 << 20;
/// Most files listed by `entries` (a `pdata` tree can hold many plots and reports).
const MAX_LISTED_FILES: usize = 4096;

/// Stored sample type (`DTYPA` / `DTYPP`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleType {
    /// `0`: 32-bit two's-complement integers.
    Int32,
    /// `2`: IEEE-754 binary64.
    Float64,
}

impl SampleType {
    /// From the parameter value; absent means int32 (nmrglue's default).
    pub fn from_code(code: Option<i64>) -> std::result::Result<Self, i64> {
        match code {
            None | Some(0) => Ok(SampleType::Int32),
            Some(2) => Ok(SampleType::Float64),
            Some(other) => Err(other),
        }
    }
    /// Bytes per stored value.
    pub fn width(self) -> u64 {
        match self {
            SampleType::Int32 => 4,
            SampleType::Float64 => 8,
        }
    }
    /// NumPy dtype name.
    pub fn dtype(self) -> &'static str {
        match self {
            SampleType::Int32 => "int32",
            SampleType::Float64 => "float64",
        }
    }
}

/// Byte order (`BYTORDA` / `BYTORDP`: 0 little-endian, 1 big-endian).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByteOrder {
    Little,
    Big,
}

impl ByteOrder {
    /// From the parameter value; anything but 1 is little-endian (nmrglue's rule).
    pub fn from_code(code: Option<i64>) -> Self {
        if code == Some(1) {
            ByteOrder::Big
        } else {
            ByteOrder::Little
        }
    }
    /// `little-endian` / `big-endian`.
    pub fn name(self) -> &'static str {
        match self {
            ByteOrder::Little => "little-endian",
            ByteOrder::Big => "big-endian",
        }
    }
}

/// Decode stored values to f64 (unscaled). Trailing bytes that do not fill a value are ignored.
pub fn decode_samples(bytes: &[u8], ty: SampleType, order: ByteOrder) -> Vec<f64> {
    match ty {
        SampleType::Int32 => bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&a| {
                f64::from(match order {
                    ByteOrder::Little => i32::from_le_bytes(a),
                    ByteOrder::Big => i32::from_be_bytes(a),
                })
            })
            .collect(),
        SampleType::Float64 => bytes
            .as_chunks::<8>()
            .0
            .iter()
            .map(|&a| match order {
                ByteOrder::Little => f64::from_le_bytes(a),
                ByteOrder::Big => f64::from_be_bytes(a),
            })
            .collect(),
    }
}

/// Digital-filter group delay in points for firmware (`DSPFVS`) 10–13, keyed by decimation
/// (`DECIM`). Values as tabulated by nmrglue (`bruker_dsp_table`, BSD-3-Clause), which credits
/// W. M. Westler and F. Abildgaard's public processing note. Values keep nmrglue's precision.
#[allow(clippy::excessive_precision)]
pub const DSP_GROUP_DELAY: &[(i64, &[(i64, f64)])] = &[
    (
        10,
        &[
            (2, 44.75),
            (3, 33.5),
            (4, 66.625),
            (6, 59.083_333_333_333_333),
            (8, 68.5625),
            (12, 60.375),
            (16, 69.531_25),
            (24, 61.020_833_333_333_333),
            (32, 70.015_625),
            (48, 61.343_75),
            (64, 70.257_812_5),
            (96, 61.505_208_333_333_333),
            (128, 70.378_906_25),
            (192, 61.585_937_5),
            (256, 70.439_453_125),
            (384, 61.626_302_083_333_333),
            (512, 70.469_726_562_5),
            (768, 61.646_484_375),
            (1024, 70.484_863_281_25),
            (1536, 61.656_575_520_833_333),
            (2048, 70.492_431_640_625),
        ],
    ),
    (
        11,
        &[
            (2, 46.0),
            (3, 36.5),
            (4, 48.0),
            (6, 50.166_666_666_666_667),
            (8, 53.25),
            (12, 69.5),
            (16, 72.25),
            (24, 70.166_666_666_666_667),
            (32, 72.75),
            (48, 70.5),
            (64, 73.0),
            (96, 70.666_666_666_666_667),
            (128, 72.5),
            (192, 71.333_333_333_333_333),
            (256, 72.25),
            (384, 71.666_666_666_666_667),
            (512, 72.125),
            (768, 71.833_333_333_333_333),
            (1024, 72.0625),
            (1536, 71.916_666_666_666_667),
            (2048, 72.031_25),
        ],
    ),
    (
        12,
        &[
            (2, 46.0),
            (3, 36.5),
            (4, 48.0),
            (6, 50.166_666_666_666_667),
            (8, 53.25),
            (12, 69.5),
            (16, 71.625),
            (24, 70.166_666_666_666_667),
            (32, 72.125),
            (48, 70.5),
            (64, 72.375),
            (96, 70.666_666_666_666_667),
            (128, 72.5),
            (192, 71.333_333_333_333_333),
            (256, 72.25),
            (384, 71.666_666_666_666_667),
            (512, 72.125),
            (768, 71.833_333_333_333_333),
            (1024, 72.0625),
            (1536, 71.916_666_666_666_667),
            (2048, 72.031_25),
        ],
    ),
    (
        13,
        &[
            (2, 2.75),
            (3, 2.833_333_333_333_333_3),
            (4, 2.875),
            (6, 2.916_666_666_666_666_7),
            (8, 2.9375),
            (12, 2.958_333_333_333_333_3),
            (16, 2.968_75),
            (24, 2.979_166_666_666_666_7),
            (32, 2.984_375),
            (48, 2.989_583_333_333_333_3),
            (64, 2.992_187_5),
            (96, 2.994_791_666_666_666_7),
        ],
    ),
];

/// One `pdata/<procno>` directory.
#[derive(Debug, Clone)]
pub struct Processing {
    pub procno: u32,
    pub dir: PathBuf,
    /// `procs` (F2 / direct dimension), then `proc2s`, `proc3s`, … in order.
    pub procs: Vec<ParamFile>,
    /// Processed data files present, with sizes (`1r`, `1i`, `2rr`, …).
    pub data_files: Vec<(String, u64)>,
}

/// An opened experiment directory.
#[derive(Debug, Clone)]
pub struct Experiment {
    /// Where the directory is read from.
    pub(crate) fs: Fs,
    pub dir: PathBuf,
    /// `acqus` (or `acqu` when `acqus` is missing).
    pub acqus: ParamFile,
    /// `acqu2s`, `acqu3s`, … present, in dimension order (index 0 = dimension 2).
    pub acqu_n: Vec<ParamFile>,
    /// `fid` or `ser` and its size.
    pub raw_file: Option<(String, u64)>,
    /// Number of lines in `nuslist`, when present.
    pub nuslist_len: Option<u64>,
    /// `nuslist` rows (one sampled increment per row, one index per indirect dimension) in file
    /// order; they are reported, not used to reconstruct the full grid.
    pub nuslist: Option<Vec<Vec<u32>>>,
    /// Why `nuslist` could not be read as a table (too large, not integers, ragged rows).
    pub nuslist_issue: Option<String>,
    /// List files the experiment used (`vdlist`, `lists/vp/<VPLIST>`, `fq1list`, …).
    pub lists: Vec<ExperimentList>,
    pub processing: Vec<Processing>,
    /// Every file under the directory (relative path, size), up to a limit.
    pub files: Vec<(String, u64)>,
    pub total_size: u64,
    /// True when more files existed than were listed.
    pub files_truncated: bool,
}

fn read_param(fs: &Fs, dir: &Path, name: &str) -> Result<Option<ParamFile>> {
    let p = dir.join(name);
    let Ok(meta) = fs.metadata(&p) else {
        return Ok(None);
    };
    if !meta.is_file() {
        return Ok(None);
    }
    if meta.len() > MAX_PARAM_FILE {
        return Err(Error::corrupt(
            BRUKER_FORMAT_ID,
            format!("{name} is {} bytes; not a parameter file", meta.len()),
        ));
    }
    let bytes = fs.read(&p).map_err(|e| Error::io(&p, e))?;
    Ok(Some(parse_param_file(name, &bytes)))
}

fn file_len(fs: &Fs, p: &Path) -> Option<u64> {
    fs.metadata(p)
        .ok()
        .filter(EntryMeta::is_file)
        .map(|m| m.len())
}

/// Most processed dimensions decoded (`8rrrrrrrr`); TopSpin parameter files go up to `proc8s`.
pub const MAX_PROCESSED_DIMS: u32 = 8;

/// Dimensions of a processed data file name: `<N>` followed by N letters `r`/`i` (`1r`, `2ri`,
/// `3rrr`, `4rrri`, …), N from 1 to [`MAX_PROCESSED_DIMS`].
pub fn processed_dims(name: &str) -> Option<u32> {
    let mut chars = name.chars();
    let n = chars.next()?.to_digit(10)?;
    let rest = chars.as_str();
    ((1..=MAX_PROCESSED_DIMS).contains(&n)
        && rest.len() == n as usize
        && rest.chars().all(|c| c == 'r' || c == 'i'))
    .then_some(n)
}

/// The component files of an N-dimensional processed spectrum, as (channel name, file name):
/// every combination of `r`/`i`, the first letter for the direct dimension and the last letter
/// changing fastest (`3rrr`, `3rri`, `3rir`, …). The all-real file is channel `real`; in 1D the
/// imaginary file is `imag`; other channels are named by their letters (`ri`, `rri`).
pub fn processed_components(dims: u32) -> Vec<(String, String)> {
    let n = dims.clamp(1, MAX_PROCESSED_DIMS);
    (0..1u32 << n)
        .map(|mask| {
            let letters: String = (0..n)
                .map(|k| if mask & (1 << (n - 1 - k)) != 0 { 'i' } else { 'r' })
                .collect();
            let channel = match (mask, n) {
                (0, _) => "real".to_string(),
                (_, 1) => "imag".to_string(),
                _ => letters.clone(),
            };
            (channel, format!("{n}{letters}"))
        })
        .collect()
}

/// One list file an experiment used (`vdlist`, `vplist`, `fq1list`, …): values one per line.
#[derive(Debug, Clone, PartialEq)]
pub struct ExperimentList {
    /// The list kind: `vdlist`, `vclist`, `vplist`, `valist`, `vtlist`, `fq1list` … `fq8list`,
    /// `difflist`.
    pub kind: String,
    /// The `acqus` parameter naming it (`VDLIST`, `FQ1LIST`), when it has one.
    pub parameter: Option<String>,
    /// Path of the file relative to the experiment directory.
    pub file: String,
    /// A first line that is not a value (`dB`, `O 114.530000`), as written.
    pub header: Option<String>,
    /// The values in file order, with a unit letter (`100u`, `5m`) applied.
    pub values: Vec<f64>,
    /// Unit of the values when the kind or the header says it (`s`, `dB`, `ppm`, `Hz`).
    pub unit: Option<String>,
    /// Lines that are not values.
    pub issues: Vec<String>,
}

/// List kinds, the `acqus` parameter naming the file, and its folder under `lists/`.
const LIST_KINDS: &[(&str, Option<&str>, &str)] = &[
    ("vdlist", Some("VDLIST"), "vd"),
    ("vclist", Some("VCLIST"), "vc"),
    ("vplist", Some("VPLIST"), "vp"),
    ("valist", Some("VALIST"), "va"),
    ("vtlist", Some("VTLIST"), "vt"),
    ("fq1list", Some("FQ1LIST"), "f1"),
    ("fq2list", Some("FQ2LIST"), "f1"),
    ("fq3list", Some("FQ3LIST"), "f1"),
    ("fq4list", Some("FQ4LIST"), "f1"),
    ("fq5list", Some("FQ5LIST"), "f1"),
    ("fq6list", Some("FQ6LIST"), "f1"),
    ("fq7list", Some("FQ7LIST"), "f1"),
    ("fq8list", Some("FQ8LIST"), "f1"),
    ("difflist", None, "diff"),
];

/// Most values read from one list file.
const MAX_LIST_VALUES: usize = 1 << 20;

/// Parse a list file. Time lists (`vdlist`, `vplist`) take a unit letter after the number
/// (`n`, `u`, `m`, `s`); values are then in seconds.
pub fn parse_experiment_list(kind: &str, file: &str, bytes: &[u8]) -> ExperimentList {
    let (text, _) = crate::text::decode_text(bytes);
    let timed = matches!(kind, "vdlist" | "vplist");
    let mut out = ExperimentList {
        kind: kind.to_string(),
        parameter: None,
        file: file.to_string(),
        header: None,
        values: Vec::new(),
        unit: None,
        issues: Vec::new(),
    };
    for (n, line) in text.lines().enumerate() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if out.values.len() >= MAX_LIST_VALUES {
            out.issues.push(format!("more than {MAX_LIST_VALUES} values: the rest is not read"));
            break;
        }
        let split = t
            .find(|c: char| c.is_ascii_alphabetic() && c != 'e' && c != 'E')
            .unwrap_or(t.len());
        let (num, suffix) = t.split_at(split);
        // divisor of the value for its unit letter (a division keeps 100u exactly 1e-4)
        let divisor = match (timed, suffix.trim()) {
            (_, "") => Some(1.0),
            (true, "s") => Some(1.0),
            (true, "m") => Some(1e3),
            (true, "u") => Some(1e6),
            (true, "n") => Some(1e9),
            _ => None,
        };
        match (num.trim().parse::<f64>(), divisor) {
            (Ok(v), Some(d)) if v.is_finite() => out.values.push(v / d),
            _ if out.values.is_empty() && out.header.is_none() => out.header = Some(t.to_string()),
            _ => out.issues.push(format!("line {}: {t:?} is not a value", n + 1)),
        }
    }
    let header = out.header.as_deref().map(str::to_ascii_lowercase);
    out.unit = match kind {
        "vdlist" | "vplist" => Some("s".into()),
        _ if header.as_deref().is_some_and(|h| h.split_whitespace().any(|w| w == "ppm")) => {
            Some("ppm".into())
        }
        _ if header.as_deref().is_some_and(|h| h.split_whitespace().any(|w| w == "hz")) => {
            Some("Hz".into())
        }
        "valist" if header.as_deref() == Some("db") => Some("dB".into()),
        _ => None,
    };
    out
}

fn read_lists(fs: &Fs, dir: &Path, acqus: &ParamFile) -> Vec<ExperimentList> {
    let mut out = Vec::new();
    for &(kind, param, folder) in LIST_KINDS {
        let named = param.and_then(|p| acqus.text(p)).filter(|n| {
            !n.contains(['/', '\\']) && *n != ".." && *n != "."
        });
        let mut candidates = vec![kind.to_string()];
        if let Some(n) = named {
            candidates.push(format!("lists/{folder}/{n}"));
        }
        for rel in candidates {
            let p = dir.join(&rel);
            let Some(len) = file_len(fs, &p) else { continue };
            let mut l = if len > MAX_PARAM_FILE {
                ExperimentList {
                    kind: kind.to_string(),
                    parameter: None,
                    file: rel.clone(),
                    header: None,
                    values: Vec::new(),
                    unit: None,
                    issues: vec![format!("{rel} is {len} bytes: not read")],
                }
            } else {
                match fs.read(&p) {
                    Ok(b) => parse_experiment_list(kind, &rel, &b),
                    Err(_) => continue,
                }
            };
            l.parameter = param.map(str::to_string);
            out.push(l);
            break;
        }
    }
    out
}

fn list_files(
    fs: &Fs,
    root: &Path,
    rel: &Path,
    depth: u32,
    out: &mut Vec<(String, u64)>,
    total: &mut u64,
) -> bool {
    let Ok(rd) = fs.read_dir(&root.join(rel)) else {
        return false;
    };
    let mut names: Vec<_> = rd.filter_map(std::result::Result::ok).collect();
    names.sort_by_key(DirEntry::file_name);
    let mut truncated = false;
    for e in names {
        let Ok(ft) = e.file_type() else { continue };
        let r = rel.join(e.file_name());
        if ft.is_dir() {
            if depth < 6 {
                truncated |= list_files(fs, root, &r, depth + 1, out, total);
            }
        } else if ft.is_file() {
            let len = e.metadata().map_or(0, |m| m.len());
            *total = total.saturating_add(len);
            if out.len() < MAX_LISTED_FILES {
                out.push((r.to_string_lossy().replace('\\', "/"), len));
            } else {
                truncated = true;
            }
        }
    }
    truncated
}

impl Experiment {
    /// Read the parameter files and list the directory. Binary data is not read.
    pub fn load(dir: &Path) -> Result<Self> {
        Self::load_in(&Fs::local(), dir)
    }

    /// [`Experiment::load`] in the namespace `fs`.
    pub(crate) fn load_in(fs: &Fs, dir: &Path) -> Result<Self> {
        let acqus = match read_param(fs, dir, "acqus")? {
            Some(p) => p,
            None => read_param(fs, dir, "acqu")?.ok_or_else(|| {
                Error::corrupt(
                    BRUKER_FORMAT_ID,
                    format!("{} has no acqus parameter file", dir.display()),
                )
            })?,
        };
        let mut acqu_n = Vec::new();
        for k in 2..=8 {
            match read_param(fs, dir, &format!("acqu{k}s"))? {
                Some(p) => acqu_n.push(p),
                None => match read_param(fs, dir, &format!("acqu{k}"))? {
                    Some(p) => acqu_n.push(p),
                    None => break,
                },
            }
        }
        let raw_file = ["fid", "ser"]
            .iter()
            .find_map(|n| file_len(fs, &dir.join(n)).map(|l| ((*n).to_string(), l)));
        let (nuslist, nuslist_issue, nuslist_len) = match file_len(fs, &dir.join("nuslist")) {
            None => (None, None, None),
            Some(len) if len > MAX_PARAM_FILE => (
                None,
                Some("nuslist exceeds 16 MiB: not read".to_string()),
                None,
            ),
            Some(_) => {
                let bytes = fs.read(&dir.join("nuslist")).unwrap_or_default();
                let lines = bytes
                    .split(|&c| c == b'\n')
                    .filter(|l| l.iter().any(|c| !c.is_ascii_whitespace()))
                    .count() as u64;
                match parse_nuslist(&bytes) {
                    Ok(rows) => (Some(rows), None, Some(lines)),
                    Err(m) => (None, Some(m), Some(lines)),
                }
            }
        };
        let mut processing = Vec::new();
        if let Ok(rd) = fs.read_dir(&dir.join("pdata")) {
            let mut procnos: Vec<(u32, PathBuf)> = rd
                .filter_map(std::result::Result::ok)
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .filter_map(|e| {
                    e.file_name()
                        .to_str()
                        .and_then(|n| n.parse::<u32>().ok())
                        .map(|n| (n, e.path()))
                })
                .collect();
            procnos.sort();
            for (procno, pdir) in procnos {
                let mut procs = Vec::new();
                if let Some(p) = read_param(fs, &pdir, "procs")? {
                    procs.push(p);
                    for k in 2..=8 {
                        match read_param(fs, &pdir, &format!("proc{k}s"))? {
                            Some(p) => procs.push(p),
                            None => break,
                        }
                    }
                }
                let mut data_files: Vec<(String, u64)> = fs
                    .read_dir(&pdir)
                    .map(|rd| {
                        rd.filter_map(std::result::Result::ok)
                            .filter_map(|e| {
                                let name = e.file_name().to_str()?.to_string();
                                processed_dims(&name)?;
                                file_len(fs, &pdir.join(&name)).map(|l| (name, l))
                            })
                            .take(1 << 12)
                            .collect()
                    })
                    .unwrap_or_default();
                data_files.sort();
                processing.push(Processing {
                    procno,
                    dir: pdir,
                    procs,
                    data_files,
                });
            }
        }
        let mut files = Vec::new();
        let mut total_size = 0u64;
        let files_truncated = list_files(fs, dir, Path::new(""), 0, &mut files, &mut total_size);
        let lists = read_lists(fs, dir, &acqus);
        Ok(Experiment {
            fs: fs.clone(),
            dir: dir.to_path_buf(),
            acqus,
            acqu_n,
            raw_file,
            nuslist_len,
            nuslist,
            nuslist_issue,
            lists,
            processing,
            files,
            total_size,
            files_truncated,
        })
    }
}

/// How `fid`/`ser` is laid out.
#[derive(Debug, Clone, PartialEq)]
pub struct RawLayout {
    /// `fid` or `ser`.
    pub file_name: String,
    pub file_len: u64,
    pub sample_type: SampleType,
    pub byte_order: ByteOrder,
    /// Real/imaginary interleaved (`AQ_mod` 1 or 3).
    pub complex: bool,
    /// `TD` of `acqus`: stored values per row (real + imaginary counted separately).
    pub td: u64,
    /// Bytes from the start of one row to the next.
    pub row_stride: u64,
    /// Rows the parameters declare (`TD` of every `acquNs` multiplied), or the file's rows when
    /// dimensions are missing.
    pub rows: u64,
    /// Whole rows present in the file.
    pub rows_in_file: u64,
    /// True when `ser` rows are not padded to 1024 bytes.
    pub unpadded_rows: bool,
    /// Notes about how the layout was determined.
    pub notes: Vec<String>,
}

impl RawLayout {
    /// Samples per row (complex points for complex data).
    pub fn samples_per_row(&self) -> u64 {
        if self.complex { self.td / 2 } else { self.td }
    }
    /// Bytes of data in one row (without padding).
    pub fn row_bytes(&self) -> u64 {
        self.td.saturating_mul(self.sample_type.width())
    }

    /// Derive the layout from `acqus`/`acquNs` and the file size.
    pub fn from_experiment(exp: &Experiment) -> std::result::Result<Self, Error> {
        let (file_name, file_len) = exp.raw_file.clone().ok_or_else(|| {
            Error::corrupt(
                BRUKER_FORMAT_ID,
                "no fid or ser file in the experiment directory",
            )
        })?;
        let a = &exp.acqus;
        let sample_type = SampleType::from_code(a.int("DTYPA")).map_err(|c| {
            Error::unsupported(
                BRUKER_FORMAT_ID,
                format!("DTYPA = {c}"),
                "Only DTYPA 0 (int32) and 2 (float64) time-domain data are decoded.",
            )
        })?;
        let byte_order = ByteOrder::from_code(a.int("BYTORDA"));
        let complex = matches!(a.int("AQ_mod"), Some(1 | 3));
        let td = a
            .int("TD")
            .and_then(|t| u64::try_from(t).ok())
            .ok_or_else(|| Error::corrupt(BRUKER_FORMAT_ID, "acqus has no valid TD"))?;
        let width = sample_type.width();
        let row_bytes = td
            .checked_mul(width)
            .ok_or_else(|| Error::corrupt(BRUKER_FORMAT_ID, "TD overflows"))?;
        let mut notes = Vec::new();
        if row_bytes == 0 {
            return Err(Error::corrupt(BRUKER_FORMAT_ID, "TD is 0"));
        }
        if complex && td % 2 == 1 {
            notes.push(format!(
                "TD {td} is odd for complex data; the last value is ignored"
            ));
        }
        if file_name == "fid" {
            return Ok(RawLayout {
                file_name,
                file_len,
                sample_type,
                byte_order,
                complex,
                td,
                row_stride: row_bytes,
                rows: 1,
                rows_in_file: u64::from(file_len >= row_bytes),
                unpadded_rows: false,
                notes,
            });
        }
        // ser: rows start on 1024-byte boundaries (nmrglue, quoting the NBL documentation)
        let padded = row_bytes.div_ceil(1024).saturating_mul(1024);
        let mut declared: Option<u64> = None;
        for p in &exp.acqu_n {
            let t = p
                .int("TD")
                .and_then(|t| u64::try_from(t).ok())
                .filter(|&t| t > 0);
            match t {
                Some(t) => declared = Some(declared.unwrap_or(1).saturating_mul(t)),
                None => notes.push(format!("{} has no valid TD", p.name)),
            }
        }
        let (row_stride, unpadded_rows) = match declared {
            Some(d) if padded != row_bytes && d.checked_mul(row_bytes) == Some(file_len) => {
                notes.push(
                    "rows are not padded to 1024 bytes (file length equals TD × rows exactly)"
                        .into(),
                );
                (row_bytes, true)
            }
            _ => (padded, false),
        };
        // rows whose values (not necessarily their trailing padding) are in the file
        let rows_in_file = if file_len >= row_bytes {
            (file_len - row_bytes) / row_stride + 1
        } else {
            0
        };
        let rows = match declared {
            None => {
                notes.push("no acqu2s: row count taken from the file size".into());
                rows_in_file
            }
            Some(d) if rows_in_file > d && rows_in_file % d == 0 => {
                notes.push(format!(
                    "the file holds {rows_in_file} rows but the acquNs files declare {d}; a dimension's parameter file (e.g. acqu{}s) is probably missing, so the file's row count is used",
                    exp.acqu_n.len() + 2
                ));
                rows_in_file
            }
            Some(d) => d,
        };
        Ok(RawLayout {
            file_name,
            file_len,
            sample_type,
            byte_order,
            complex,
            td,
            row_stride,
            rows,
            rows_in_file,
            unpadded_rows,
            notes,
        })
    }
}

/// How one `pdata/<procno>` spectrum is laid out.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcLayout {
    pub procno: u32,
    /// Processed dimensions (1 to [`MAX_PROCESSED_DIMS`]).
    pub dims: u32,
    /// `SI` per dimension, direct dimension first.
    pub si: Vec<u64>,
    /// Submatrix edge (`XDIM`) per dimension, direct dimension first (equal to `si` when untiled).
    pub xdim: Vec<u64>,
    /// Rows of direct-dimension points: the product of the indirect `SI` (1 for 1D).
    pub rows: u64,
    pub sample_type: SampleType,
    pub byte_order: ByteOrder,
    /// Channel files in order: (`real`|`imag`, file name, size).
    pub components: Vec<(String, String, u64)>,
}

impl ProcLayout {
    /// Derive from `procs`/`proc2s` and the processed files present. `None` when nothing is decodable.
    pub fn from_processing(p: &Processing) -> Option<std::result::Result<Self, Error>> {
        let procs = p.procs.first()?;
        let has = |n: &str| p.data_files.iter().find(|(f, _)| f == n).map(|(_, l)| *l);
        // the lowest dimension whose all-real file is present
        let dims = (1..=MAX_PROCESSED_DIMS).find(|&n| has(&format!("{n}{}", "r".repeat(n as usize))).is_some())?;
        let comps = processed_components(dims);
        Some((|| {
            let sample_type = SampleType::from_code(procs.int("DTYPP")).map_err(|c| {
                Error::unsupported(
                    BRUKER_FORMAT_ID,
                    format!("DTYPP = {c}"),
                    "Only DTYPP 0 (int32) and 2 (float64) processed data are decoded.",
                )
            })?;
            let byte_order = ByteOrder::from_code(procs.int("BYTORDP"));
            let mut si = Vec::new();
            let mut xdim = Vec::new();
            for k in 0..dims as usize {
                let pf = p.procs.get(k).ok_or_else(|| {
                    Error::corrupt(
                        BRUKER_FORMAT_ID,
                        format!("pdata/{}: proc{}s is missing", p.procno, k + 1),
                    )
                })?;
                let s = pf
                    .int("SI")
                    .and_then(|v| u64::try_from(v).ok())
                    .filter(|&v| v > 0)
                    .ok_or_else(|| {
                        Error::corrupt(
                            BRUKER_FORMAT_ID,
                            format!("pdata/{}: {} has no valid SI", p.procno, pf.name),
                        )
                    })?;
                let x = pf
                    .int("XDIM")
                    .and_then(|v| u64::try_from(v).ok())
                    .filter(|&v| v > 0 && v <= s)
                    .unwrap_or(s);
                si.push(s);
                xdim.push(x);
            }
            if dims >= 2 {
                for k in 0..dims as usize {
                    if si[k] % xdim[k] != 0 {
                        return Err(Error::corrupt(
                            BRUKER_FORMAT_ID,
                            format!(
                                "pdata/{}: SI {} is not a multiple of XDIM {}",
                                p.procno, si[k], xdim[k]
                            ),
                        ));
                    }
                }
            }
            let si_text = si.iter().map(u64::to_string).collect::<Vec<_>>().join(" × ");
            let total = si
                .iter()
                .try_fold(1u64, |a, &b| a.checked_mul(b))
                .and_then(|v| v.checked_mul(sample_type.width()))
                .ok_or_else(|| {
                    Error::corrupt(
                        BRUKER_FORMAT_ID,
                        format!("pdata/{}: SI {si_text} overflows the file size", p.procno),
                    )
                })?;
            let rows = total / sample_type.width() / si[0];
            if rows > u64::from(u32::MAX) {
                return Err(Error::unsupported(
                    BRUKER_FORMAT_ID,
                    format!("more than u32::MAX processed rows (SI {si_text})"),
                    "Use a smaller processed dataset.",
                ));
            }
            let components = comps
                .into_iter()
                .filter_map(|(c, f)| has(&f).map(|l| (c, f, l)))
                .collect();
            Ok(ProcLayout {
                procno: p.procno,
                dims,
                si,
                xdim,
                rows,
                sample_type,
                byte_order,
                components,
            })
        })())
    }

    /// True for processed data file names (`1r`, `2rr`, `3rrr`, `4rrrr`, …).
    pub fn is_data_name(name: &str) -> bool {
        processed_dims(name).is_some()
    }

    /// Values per component file (`SI` multiplied; it fits in u64, `from_processing` checks).
    pub fn values(&self) -> u64 {
        self.si.iter().fold(1u64, |a, &b| a.saturating_mul(b))
    }

    /// Bytes per component file, `None` when the product overflows.
    pub fn file_bytes(&self) -> Option<u64> {
        self.si
            .iter()
            .try_fold(1u64, |a, &b| a.checked_mul(b))?
            .checked_mul(self.sample_type.width())
    }

    /// Byte ranges `(offset, values)` holding samples `[first, first+count)` of row `row`,
    /// accounting for 2D submatrix tiling.
    pub fn row_ranges(&self, row: u64, first: u64, count: u64) -> Vec<(u64, u64)> {
        // saturating arithmetic: absurd SI/XDIM values give offsets past the end of the file,
        // which the read then reports as a corrupt file instead of overflowing
        let w = self.sample_type.width();
        if self.dims == 1 {
            return vec![(first.saturating_mul(w), count)];
        }
        let x2 = self.xdim[0].max(1);
        let mut remaining = row;
        let mut tile_row = 0u64;
        let mut within_row = 0u64;
        let mut tile_stride = 1u64;
        let mut within_stride = x2;
        for k in 1..self.si.len() {
            let coordinate = remaining % self.si[k];
            remaining /= self.si[k];
            tile_row =
                tile_row.saturating_add((coordinate / self.xdim[k]).saturating_mul(tile_stride));
            within_row = within_row
                .saturating_add((coordinate % self.xdim[k]).saturating_mul(within_stride));
            tile_stride = tile_stride.saturating_mul(self.si[k] / self.xdim[k]);
            within_stride = within_stride.saturating_mul(self.xdim[k]);
        }
        let mut out = Vec::new();
        let end = first.saturating_add(count);
        let mut pos = first;
        while pos < end {
            let b = pos / x2;
            let within = pos % x2;
            let n = (x2 - within).min(end - pos);
            let sub = tile_row.saturating_mul(self.si[0] / x2).saturating_add(b);
            let off = sub
                .saturating_mul(within_stride)
                .saturating_add(within_row)
                .saturating_add(within)
                .saturating_mul(w);
            out.push((off, n));
            pos += n;
        }
        out
    }
}

/// Parse `nuslist` text: one row per non-blank line, whitespace-separated unsigned integers,
/// the same count on every row (one per indirect dimension, at most 7).
pub fn parse_nuslist(bytes: &[u8]) -> std::result::Result<Vec<Vec<u32>>, String> {
    let (text, _) = crate::text::decode_text(bytes);
    let mut rows: Vec<Vec<u32>> = Vec::new();
    for (n, line) in text
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
    {
        let row = line
            .split_whitespace()
            .map(str::parse::<u32>)
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|_| {
                format!(
                    "nuslist line {}: {:?} is not a list of indices",
                    n + 1,
                    line.trim()
                )
            })?;
        if row.len() > 7 || rows.first().is_some_and(|r| r.len() != row.len()) {
            return Err(format!(
                "nuslist line {}: {} indices (other rows have {})",
                n + 1,
                row.len(),
                rows.first().map_or(0, Vec::len)
            ));
        }
        rows.push(row);
    }
    Ok(rows)
}

/// Whether `d` holds a Bruker flex (XMASS) mass-spectrometry parameter file rather than a
/// TopSpin one: such directories also hold `acqus` and `fid`, and belong to the flex reader.
fn is_xmass(fs: &Fs, d: &Path) -> bool {
    ["acqus", "acqu"].iter().any(|n| {
        let p = d.join(n);
        fs.open(&p).is_ok_and(|f| {
            let mut head = [0u8; 256];
            let n = f.read_at(0, &mut head).unwrap_or(0);
            let t = String::from_utf8_lossy(&head[..n]);
            t.lines().take(4).any(|l| {
                (l.starts_with("##TITLE=") || l.starts_with("##ORIGIN=")) && l.contains("XMASS")
            })
        })
    })
}

/// Resolve a path to its experiment directory: the directory itself when it holds `acqus`
/// (or `acqu`) and `fid`/`ser`/`pdata`; the parent of `fid`/`ser`/`acqus`/`acquNs`/`pulseprogram`;
/// or the experiment above `pdata/<procno>` and its files.
pub fn resolve_experiment_dir(path: &Path) -> Option<PathBuf> {
    resolve_experiment_dir_in(&Fs::local(), path)
}

/// [`resolve_experiment_dir`] in the namespace `fs`.
pub(crate) fn resolve_experiment_dir_in(fs: &Fs, path: &Path) -> Option<PathBuf> {
    let is_exp = |d: &Path| {
        (fs.is_file(&d.join("acqus")) || fs.is_file(&d.join("acqu")))
            && (fs.is_file(&d.join("fid"))
                || fs.is_file(&d.join("ser"))
                || fs.is_dir(&d.join("pdata")))
            && !is_xmass(fs, d)
    };
    let meta = fs.metadata(path).ok()?;
    if meta.is_dir() {
        if is_exp(path) {
            return Some(path.to_path_buf());
        }
        // pdata/<procno> or pdata
        let parent = path.parent()?;
        if parent.file_name().is_some_and(|n| n == "pdata") {
            let exp = parent.parent()?;
            if is_exp(exp) {
                return Some(exp.to_path_buf());
            }
        }
        if path.file_name().is_some_and(|n| n == "pdata") && is_exp(parent) {
            return Some(parent.to_path_buf());
        }
        return None;
    }
    let name = path.file_name()?.to_str()?;
    let parent = path.parent()?;
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    let raw_names = ["fid", "ser", "acqus", "acqu", "pulseprogram", "nuslist"];
    let acqu_n = name.starts_with("acqu") && name[4..].trim_end_matches('s').parse::<u32>().is_ok();
    if (raw_names.contains(&name) || acqu_n) && is_exp(parent) {
        return Some(parent.to_path_buf());
    }
    let proc_n = name.starts_with("proc")
        && (name == "procs"
            || name == "proc"
            || name[4..].trim_end_matches('s').parse::<u32>().is_ok());
    if proc_n || processed_dims(name).is_some() {
        let pdata = parent.parent()?;
        if pdata.file_name().is_some_and(|n| n == "pdata") {
            let exp = pdata.parent()?;
            if is_exp(exp) {
                return Some(exp.to_path_buf());
            }
        }
    }
    None
}

/// Experiment directories directly inside `dir` (a data set's "name" directory), sorted by
/// experiment number.
pub fn find_experiments(dir: &Path) -> Vec<PathBuf> {
    find_experiments_in(&Fs::local(), dir)
}

/// [`find_experiments`] in the namespace `fs`.
pub(crate) fn find_experiments_in(fs: &Fs, dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = fs.read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(u64, PathBuf)> = rd
        .filter_map(std::result::Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| {
            let p = e.path();
            let n = e.file_name().to_str()?.parse::<u64>().ok()?;
            resolve_experiment_dir_in(fs, &p)
                .filter(|r| r == &p)
                .map(|_| (n, p))
        })
        .take(100_000)
        .collect();
    out.sort();
    out.into_iter().map(|(_, p)| p).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_both_types() {
        let le = [1u8, 0, 0, 0, 0xff, 0xff, 0xff, 0xff];
        assert_eq!(
            decode_samples(&le, SampleType::Int32, ByteOrder::Little),
            vec![1.0, -1.0]
        );
        let be = [0u8, 0, 0, 2];
        assert_eq!(
            decode_samples(&be, SampleType::Int32, ByteOrder::Big),
            vec![2.0]
        );
        let f = 1.5f64.to_be_bytes();
        assert_eq!(
            decode_samples(&f, SampleType::Float64, ByteOrder::Big),
            vec![1.5]
        );
    }

    #[test]
    fn submatrix_ranges() {
        // 4 x 4 spectrum stored as 2 x 2 submatrices of 2 x 2
        let l = ProcLayout {
            procno: 1,
            dims: 2,
            si: vec![4, 4],
            xdim: vec![2, 2],
            rows: 4,
            sample_type: SampleType::Int32,
            byte_order: ByteOrder::Little,
            components: vec![],
        };
        // row 1, all columns: submatrix 0 row 1 (values 2..4), submatrix 1 row 1 (values 6..8)
        assert_eq!(l.row_ranges(1, 0, 4), vec![(8, 2), (24, 2)]);
        // row 2 starts the second band of submatrices (subs 2, 3)
        assert_eq!(l.row_ranges(2, 1, 2), vec![(36, 1), (48, 1)]);
    }

    #[test]
    fn processed_names_of_any_dimension() {
        assert_eq!(processed_dims("1r"), Some(1));
        assert_eq!(processed_dims("4rrir"), Some(4));
        assert_eq!(processed_dims("8iiiiiiii"), Some(8));
        assert_eq!(processed_dims("4rrr"), None);
        assert_eq!(processed_dims("9rrrrrrrrr"), None);
        assert_eq!(processed_dims("2rx"), None);
        assert_eq!(processed_dims("title"), None);
        let c = processed_components(1);
        assert_eq!(c[1], ("imag".to_string(), "1i".to_string()));
        let c = processed_components(3);
        let files: Vec<&str> = c.iter().map(|(_, f)| f.as_str()).collect();
        assert_eq!(
            files,
            ["3rrr", "3rri", "3rir", "3rii", "3irr", "3iri", "3iir", "3iii"]
        );
        let c = processed_components(4);
        assert_eq!(c.len(), 16);
        assert_eq!(c[0].0, "real");
        assert_eq!(c[1], ("rrri".to_string(), "4rrri".to_string()));
        assert_eq!(c[15].1, "4iiii");
    }

    fn processing(dims: &[(u64, u64)], files: &[(&str, u64)]) -> Processing {
        let procs = dims
            .iter()
            .enumerate()
            .map(|(k, (si, xdim))| {
                let name = if k == 0 {
                    "procs".to_string()
                } else {
                    format!("proc{}s", k + 1)
                };
                let text = format!("##$SI= {si}\n##$XDIM= {xdim}\n##END=\n");
                parse_param_file(&name, text.as_bytes())
            })
            .collect();
        Processing {
            procno: 1,
            dir: PathBuf::from("pdata/1"),
            procs,
            data_files: files.iter().map(|(n, l)| ((*n).to_string(), *l)).collect(),
        }
    }

    #[test]
    fn four_dimensional_layout() {
        // 4 × 4 × 4 × 8 values in 2 × 2 × 2 × 4 submatrices
        let p = processing(
            &[(8, 4), (4, 2), (4, 2), (4, 2)],
            &[("4rrrr", 2048), ("4iiii", 2048)],
        );
        let l = ProcLayout::from_processing(&p).unwrap().unwrap();
        assert_eq!((l.dims, l.rows, l.values()), (4, 64, 512));
        assert_eq!(l.file_bytes(), Some(2048));
        let names: Vec<&str> = l.components.iter().map(|c| c.0.as_str()).collect();
        assert_eq!(names, ["real", "iiii"]);
        // the 64 rows × 8 points cover every value exactly once
        let mut seen = vec![0u32; 512];
        for row in 0..64 {
            for (off, n) in l.row_ranges(row, 0, 8) {
                for v in off / 4..off / 4 + n {
                    seen[v as usize] += 1;
                }
            }
        }
        assert!(seen.iter().all(|&c| c == 1));
    }

    #[test]
    fn inconsistent_si_and_xdim_are_errors() {
        let p = processing(&[(8, 4), (6, 4), (4, 2)], &[("3rrr", 768)]);
        let e = ProcLayout::from_processing(&p).unwrap().unwrap_err();
        assert!(e.to_string().contains("not a multiple of XDIM"), "{e}");
        let big = u64::from(u32::MAX);
        let p = processing(&[(big, big), (big, big), (big, big)], &[("3rrr", 8)]);
        let e = ProcLayout::from_processing(&p).unwrap().unwrap_err();
        assert!(e.to_string().contains("overflows"), "{e}");
        // a missing procNs file for a declared dimension
        let p = processing(&[(8, 8), (4, 4)], &[("3rrr", 8)]);
        let e = ProcLayout::from_processing(&p).unwrap().unwrap_err();
        assert!(e.to_string().contains("proc3s is missing"), "{e}");
    }

    #[test]
    fn list_files() {
        let l = parse_experiment_list("vplist", "lists/vp/x", b"100u\n5m\n\n10m\n");
        assert_eq!(l.values, vec![1e-4, 5e-3, 1e-2]);
        assert_eq!((l.header, l.unit.as_deref()), (None, Some("s")));
        let l = parse_experiment_list("valist", "lists/va/x", b"dB\n36.82\n-0.08\n");
        assert_eq!(l.values, vec![36.82, -0.08]);
        assert_eq!((l.header.as_deref(), l.unit.as_deref()), (Some("dB"), Some("dB")));
        let l = parse_experiment_list("fq1list", "fq1list", b"O 114.530000\n-2000000\n-1992172\n");
        assert_eq!(l.values, vec![-2_000_000.0, -1_992_172.0]);
        assert_eq!(l.header.as_deref(), Some("O 114.530000"));
        assert_eq!(l.unit, None);
        let l = parse_experiment_list("vclist", "vclist", b"1\n2\nthree\n4\n");
        assert_eq!(l.values, vec![1.0, 2.0, 4.0]);
        assert_eq!(l.issues.len(), 1);
        // unit letters apply to time lists only
        let l = parse_experiment_list("vclist", "vclist", b"2m\n");
        assert_eq!(l.header.as_deref(), Some("2m"));
    }
}
