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

const PROCESSED_NAMES: &[&str] = &[
    "1r", "1i", "2rr", "2ri", "2ir", "2ii", "3rrr", "3irr", "3rir", "3iir", "3rri", "3iri", "3rii",
    "3iii",
];

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
                let data_files = PROCESSED_NAMES
                    .iter()
                    .filter_map(|n| file_len(fs, &pdir.join(n)).map(|l| ((*n).to_string(), l)))
                    .collect();
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
        Ok(Experiment {
            fs: fs.clone(),
            dir: dir.to_path_buf(),
            acqus,
            acqu_n,
            raw_file,
            nuslist_len,
            nuslist,
            nuslist_issue,
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
    /// One, two or three processed dimensions.
    pub dims: u32,
    /// `SI` per dimension, direct dimension first.
    pub si: Vec<u64>,
    /// Submatrix edge (`XDIM`) per dimension, direct dimension first (equal to `si` when untiled).
    pub xdim: Vec<u64>,
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
        let (dims, comps): (u32, Vec<(&str, &str)>) = if has("1r").is_some() {
            (1, vec![("real", "1r"), ("imag", "1i")])
        } else if has("2rr").is_some() {
            (
                2,
                vec![("real", "2rr"), ("ri", "2ri"), ("ir", "2ir"), ("ii", "2ii")],
            )
        } else if has("3rrr").is_some() {
            (
                3,
                vec![
                    ("real", "3rrr"),
                    ("rri", "3rri"),
                    ("rir", "3rir"),
                    ("rii", "3rii"),
                    ("irr", "3irr"),
                    ("iri", "3iri"),
                    ("iir", "3iir"),
                    ("iii", "3iii"),
                ],
            )
        } else {
            return None;
        };
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
            let total = si
                .iter()
                .try_fold(1u64, |a, &b| a.checked_mul(b))
                .and_then(|v| v.checked_mul(sample_type.width()))
                .ok_or_else(|| Error::corrupt(BRUKER_FORMAT_ID, "processed dimensions overflow"))?;
            if total / sample_type.width() / si[0] > u64::from(u32::MAX) {
                return Err(Error::unsupported(
                    BRUKER_FORMAT_ID,
                    "more than u32::MAX processed sweeps",
                    "Use a smaller processed dataset.",
                ));
            }
            let components = comps
                .into_iter()
                .filter_map(|(c, f)| has(f).map(|l| (c.to_string(), f.to_string(), l)))
                .collect();
            Ok(ProcLayout {
                procno: p.procno,
                dims,
                si,
                xdim,
                sample_type,
                byte_order,
                components,
            })
        })())
    }

    /// True for processed data file names (`1r`, `2rr`, `3rrr`, …).
    pub fn is_data_name(name: &str) -> bool {
        PROCESSED_NAMES.contains(&name)
    }

    /// Values per component file.
    pub fn values(&self) -> u64 {
        self.si.iter().fold(1u64, |a, &b| a.saturating_mul(b))
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
    if proc_n || PROCESSED_NAMES.contains(&name) {
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
            sample_type: SampleType::Int32,
            byte_order: ByteOrder::Little,
            components: vec![],
        };
        // row 1, all columns: submatrix 0 row 1 (values 2..4), submatrix 1 row 1 (values 6..8)
        assert_eq!(l.row_ranges(1, 0, 4), vec![(8, 2), (24, 2)]);
        // row 2 starts the second band of submatrices (subs 2, 3)
        assert_eq!(l.row_ranges(2, 1, 2), vec![(36, 1), (48, 1)]);
    }
}
