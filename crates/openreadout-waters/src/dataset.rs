//! `Dataset` for Waters MassLynx `.raw` directories: text metadata (`_HEADER.TXT`,
//! `_extern.inf`, `_INLET.INF`), the per-function scan index (`_FUNCnnn.IDX`), full-scan spectra
//! (12-byte values; 6- and 8-byte values as documented by rainbow), MRM/SIR intensities (4- and
//! 2-byte values) as tables, per-scan statistics (`_FUNCnnn.STS`) and analog channels
//! (`_CHROnnn.DAT`, `_CHROMS.INF`).
//! Vocabulary and derivation: `docs/formats/waters-raw.md`, `docs/provenance/waters-raw.md`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use openreadout_core::bytes::{latin1, le_f32, le_u16, le_u32, read_range};
use openreadout_core::model::{
    CheckReport, ColumnInfo, FileInfo, Finding, InstrumentInfo, LsEntry, SignalChannelInfo,
    SpectraInfo, Spectrum, Table, TableInfo, Trace, TraceInfo,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::source::{Fs, Input};
use openreadout_core::{Error, Plane, Result};
use serde_json::{Value, json};

use crate::drift::{DriftIndex, decode_drift_scan, parse_drift_index};
use crate::layout::{
    CHRO_HEADER, Calibration, ExternInfo, FLAG_WIDE_CALIBRATED, FUNCTION_BLOCK, FUNCTION_DAUGHTER,
    FUNCTION_PDA, FUNCTION_TOF_MS, FUNCTION_TOF_MSMS, FunctionBlock, ScanIndex, StatsLayout,
    decode_packed16, decode_packed32, decode_value6, decode_value8, decode_value12,
    header_datetime, idx_record_len_for, parse_chroms, parse_extern, parse_functions,
    parse_header_txt, parse_idx, parse_stats_layout,
};
use crate::{WATERS_ID, WatersRawReader};

/// Largest `.INF`/`.TXT` sidecar of a `.raw` folder that is read (they are a few KB).
const MAX_SIDECAR: u64 = 16 << 20;
/// Largest `_FUNCnnn.IDX` / `_FUNCnnn.STS` loaded whole.
const MAX_INDEX: u64 = 1 << 30;
/// Largest single scan read from a DAT file.
const MAX_SCAN_BYTES: u64 = 1 << 30;

/// An analog channel (`_CHROnnn.DAT` with its `_CHROMS.INF` description).
#[derive(Debug, Clone)]
pub struct AnalogChannel {
    /// Channel number (`nnn`).
    pub number: u32,
    /// File name of the channel's `_CHROnnn.DAT`.
    pub file: String,
    /// Description from `_CHROMS.INF`.
    pub description: Option<String>,
    /// Unit from the `$CC$` string.
    pub unit: Option<String>,
    /// The `$CC$,...` conversion string after the description, verbatim.
    pub conversion: Option<String>,
    /// (time in minutes, value) pairs.
    pub points: Vec<(f32, f32)>,
    /// Bytes after the header that do not form a whole pair.
    pub trailing: u64,
}

/// A function: `_FUNCnnn.IDX` + `_FUNCnnn.DAT` (+ `_FUNCnnn.STS`).
#[derive(Debug, Clone)]
pub struct WatersFunction {
    /// Function number (`nnn`, from 1).
    pub number: u32,
    /// The scan index.
    pub scans: Vec<ScanIndex>,
    /// Size of `_FUNCnnn.IDX`.
    pub idx_len: u64,
    /// Bytes per index record (22 or 30).
    pub idx_record_len: usize,
    /// Size of `_FUNCnnn.DAT`, when present.
    pub dat_len: Option<u64>,
    /// DAT bytes per stored value (DAT length / total values), when whole.
    pub bytes_per_value: Option<u64>,
    /// The function's `_FUNCTNS.INF` block.
    pub block: Option<FunctionBlock>,
    /// `positive` / `negative`, from `_extern.inf` or the function code.
    pub polarity: Option<String>,
    /// The function's type as `_extern.inf` names it (`TOF MS FUNCTION`, `REFERENCE`, …).
    pub kind: Option<String>,
    /// `Data Format` of the function in `_extern.inf` (`Centroid`, `Continuum`).
    pub data_format: Option<String>,
    /// The `Cal Function n` line of `_HEADER.TXT`.
    pub calibration: Option<Calibration>,
    /// Layout of `_FUNCnnn.STS`, when present and self-consistent.
    pub stats_layout: Option<StatsLayout>,
    /// MSe high-energy function: its `_extern.inf` section ramps a high collision energy.
    pub high_energy: bool,
    /// The function has drift-resolved data (`_funcNNN.cdt` + `.ind`, ion mobility or SONAR
    /// quadrupole bins); the `.DAT` spectra are those bins summed.
    pub drift_resolved: bool,
    /// The parsed `_funcNNN.ind` and the name of the `_funcNNN.cdt`, when both are present and
    /// the index parses.
    pub drift: Option<(DriftIndex, String)>,
    /// Bytes of `_FUNCnnn.STS`.
    pub stats: Vec<u8>,
}

impl WatersFunction {
    fn label(&self) -> String {
        format!("FUNC{:03}", self.number)
    }
    /// Channel names of the stored values (MRM transitions / SIR masses), when every scan
    /// stores as many values as the function block lists masses.
    fn value_names(&self) -> Option<Vec<String>> {
        let b = self.block.as_ref()?;
        let n = self.scans.first()?.count as usize;
        if n == 0 || b.set_mz.len() != n || self.scans.iter().any(|s| s.count as usize != n) {
            return None;
        }
        Some(
            (0..n)
                .map(|i| match b.second_mz.get(i) {
                    Some(p) => format!("{} > {}", b.set_mz[i], p),
                    None => format!("{}", b.set_mz[i]),
                })
                .collect(),
        )
    }
    /// Values that decode as table columns: 4-byte (MRM) or 2-byte (SIR) layouts.
    fn decodable(&self) -> bool {
        matches!(self.bytes_per_value, Some(4 | 2)) && self.value_names().is_some()
    }
    /// A scanning function whose values are (mass, intensity) pairs.
    fn is_spectral(&self) -> bool {
        matches!(self.bytes_per_value, Some(12 | 8 | 6)) && !self.is_pda()
    }
    /// A photodiode-array (UV/Vis) function: block type 0x0C, or a PDA/UV type text.
    fn is_pda(&self) -> bool {
        self.block
            .as_ref()
            .is_some_and(|b| b.function_type() == FUNCTION_PDA)
            || self.kind.as_deref().is_some_and(|k| {
                let k = k.to_ascii_uppercase();
                k.contains("PDA") || k.contains("UV") || k.contains("DIODE")
            })
    }
    /// MS level: from the function type of the `_FUNCTNS.INF` block (TOF MS/MS and product-ion
    /// scans are MS2; an MSe high-energy TOF MS function, one whose `_extern.inf` section ramps
    /// a high collision energy, is MS2 of the whole scan window), else from the `_extern.inf`
    /// type text (`MSMS` → 2). The type text alone misleads: fast-DDA acquisitions name their
    /// MS/MS function `TOF SURVEY FUNCTION`.
    fn ms_level(&self) -> u32 {
        if let Some(b) = &self.block
            && !self.is_reference()
        {
            return match b.function_type() {
                FUNCTION_TOF_MSMS | FUNCTION_DAUGHTER => 2,
                FUNCTION_TOF_MS if self.high_energy => 2,
                _ => 1,
            };
        }
        match self.kind.as_deref().map(str::to_ascii_uppercase) {
            Some(k) if k.contains("MSMS") || k.contains("DAUGHTER") || k.contains("MS/MS") => 2,
            _ => 1,
        }
    }
    /// The lock-spray reference function (block code bit 0x8000, or `REFERENCE` in its type).
    fn is_reference(&self) -> bool {
        self.block
            .as_ref()
            .is_some_and(FunctionBlock::is_reference_code)
            || self
                .kind
                .as_deref()
                .is_some_and(|k| k.to_ascii_uppercase().contains("REFERENCE"))
    }
    /// Values stored already calibrated (flag 0x100 on the index records).
    fn stored_calibrated(&self) -> bool {
        self.scans
            .first()
            .is_some_and(|s| s.flags & FLAG_WIDE_CALIBRATED != 0)
    }
    fn stat(&self, scan: usize, name: &str) -> Option<f64> {
        let l = self.stats_layout.as_ref()?;
        let f = l.field(name).or_else(|| {
            (name == "Set Mass")
                .then(|| l.field_by_id(crate::layout::STAT_SET_MASS))
                .flatten()
                .filter(|f| f.name.is_empty())
        })?;
        l.value(&self.stats, scan, f).filter(|v| v.is_finite())
    }
    fn scan_window(&self) -> Option<[f64; 2]> {
        let b = self.block.as_ref()?;
        let lo = f64::from(*b.set_mz.first()?);
        let hi = f64::from(*b.second_mz.first()?);
        (hi > lo && b.set_mz.len() == 1).then_some([lo, hi])
    }
}

/// One scan as stored: (mass, intensity) pairs and, for 12-byte values, each point's flags.
type RawScan = (Vec<(f64, f64)>, Vec<u16>);

/// What a trace index refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TraceKind {
    Function(usize),
    Analog(usize),
    /// A photodiode-array function: time, then one channel per wavelength.
    Pda(usize),
    Tic,
    Bpc,
}

/// An opened `.raw` directory.
#[derive(Debug)]
pub struct WatersRawDataset {
    path: PathBuf,
    /// Where the directory is read from.
    fs: Fs,
    files: Vec<(String, u64)>,
    total_size: u64,
    header: Vec<(String, String)>,
    extern_inf: Option<String>,
    extern_info: ExternInfo,
    inlet_inf: Option<String>,
    functions: Vec<WatersFunction>,
    analog: Vec<AnalogChannel>,
    functions_inf_len: Option<u64>,
    /// Spectral scans merged by retention time: (function position, scan position).
    order: Vec<(usize, usize)>,
    /// The scans of `order` whose function has drift data, in the same order: (function
    /// position, scan position, first run-1 spectrum index, run-0 spectrum index). Run 1 lists
    /// one spectrum per drift bin.
    drift_order: Vec<(usize, usize, u64, u64)>,
    /// The last scan whose drift bins were decoded: (function, scan) → per bin (m/z, intensity).
    drift_cache: Option<((usize, usize), DriftSpectra)>,
}

/// One scan's drift bins as spectra: per bin, m/z and intensity arrays.
type DriftSpectra = Vec<(Vec<f64>, Vec<f32>)>;

/// Is `path` a MassLynx `.raw` directory (has `_HEADER.TXT` or a `_FUNCnnn.IDX`)?
pub fn is_waters_dir(path: &Path) -> bool {
    is_waters_dir_in(&Fs::local(), path)
}

/// [`is_waters_dir`] in the namespace `fs`.
pub fn is_waters_dir_in(fs: &Fs, path: &Path) -> bool {
    if !fs.is_dir(path) {
        return false;
    }
    let Ok(rd) = fs.read_dir(path) else {
        return false;
    };
    rd.filter_map(std::result::Result::ok).any(|e| {
        let n = e.file_name().to_string_lossy().to_ascii_uppercase();
        n == "_HEADER.TXT"
            || (n.starts_with("_FUNC")
                && Path::new(&n)
                    .extension()
                    .is_some_and(|x| x.eq_ignore_ascii_case("idx")))
    })
}

fn find(files: &[(String, u64)], name: &str) -> Option<String> {
    files
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(n, _)| n.clone())
}

/// A sidecar's bytes, or `None` when it is unreadable or larger than [`MAX_SIDECAR`].
fn read_sidecar(fs: &Fs, p: &Path) -> Option<Vec<u8>> {
    openreadout_core::bytes::read_file_capped_in(fs, p, MAX_SIDECAR, WATERS_ID, "sidecar").ok()
}

fn read_text(fs: &Fs, p: &Path) -> Option<String> {
    let b = read_sidecar(fs, p)?;
    Some(latin1(&b).replace('\r', ""))
}

fn number_in(name: &str, prefix: &str, suffix: &str) -> Option<u32> {
    let u = name.to_ascii_uppercase();
    u.strip_prefix(prefix)?.strip_suffix(suffix)?.parse().ok()
}

/// Evenly spaced times: `(first, step)` when every step is within 1 % of the mean step.
fn uniform_times(t: &[f64]) -> Option<(f64, f64)> {
    if t.len() < 2 {
        return None;
    }
    let step = (t[t.len() - 1] - t[0]) / (t.len() - 1) as f64;
    if step.is_nan() || step <= 0.0 {
        return None;
    }
    let ok = t
        .windows(2)
        .all(|w| ((w[1] - w[0]) - step).abs() <= 0.01 * step);
    ok.then_some((t[0], step))
}

/// DAT bytes per stored value: DAT length / Σ counts when whole; otherwise (a cut-off DAT)
/// the spacing of the first two consecutive scans that hold values.
fn infer_bytes_per_value(scans: &[ScanIndex], dat_len: Option<u64>) -> Option<u64> {
    let total: u64 = scans.iter().map(|s| u64::from(s.count)).sum();
    if let Some(l) = dat_len
        && total > 0
        && l % total == 0
    {
        return Some(l / total);
    }
    dat_len?;
    scans.windows(2).find_map(|w| {
        let n = u64::from(w[0].count);
        let d = w[1].offset.checked_sub(w[0].offset)?;
        // `then`, not `then_some`: the quotient must not be evaluated when `n` is 0.
        (n > 0 && d % n == 0 && matches!(d / n, 2 | 4 | 6 | 8 | 12)).then(|| d / n)
    })
}

/// Round to 12 significant digits (hides f32→f64 noise in derived rates and axes).
fn tidy(v: f64) -> f64 {
    if !v.is_finite() || v == 0.0 {
        return v;
    }
    let mag = v.abs().log10().floor() as i32;
    let p = 10f64.powi(11 - mag);
    if !p.is_finite() || p == 0.0 {
        return v;
    }
    (v * p).round() / p
}

fn read_capped(fs: &Fs, path: &Path, what: &str) -> Result<Vec<u8>> {
    openreadout_core::bytes::read_file_capped_in(fs, path, MAX_INDEX, WATERS_ID, what)
}

impl WatersRawDataset {
    /// Open a `.raw` directory.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    /// Open an [`Input`]: a local `.raw` directory, or one held in memory or by the host.
    pub fn open_input(input: &Input) -> Result<Self> {
        let (path, fs) = (input.path(), input.fs());
        if !fs.is_dir(path) {
            return Err(Error::Usage(format!(
                "{} is not a directory: open the whole MassLynx .raw directory",
                path.display()
            )));
        }
        let mut files: Vec<(String, u64)> = fs
            .read_dir(path)
            .map_err(|e| Error::io(path, e))?
            .filter_map(std::result::Result::ok)
            .filter_map(|e| {
                let m = e.metadata().ok()?;
                m.is_file()
                    .then(|| (e.file_name().to_string_lossy().into_owned(), m.len()))
            })
            .collect();
        files.sort();
        let total_size = files.iter().map(|(_, s)| s).sum();
        let header = find(&files, "_HEADER.TXT")
            .and_then(|n| read_text(fs, &path.join(n)))
            .map(|t| parse_header_txt(&t))
            .unwrap_or_default();
        let extern_inf = find(&files, "_extern.inf").and_then(|n| read_text(fs, &path.join(n)));
        let extern_info = extern_inf.as_deref().map(parse_extern).unwrap_or_default();
        let inlet_inf = find(&files, "_INLET.INF").and_then(|n| read_text(fs, &path.join(n)));
        let functions_inf = find(&files, "_FUNCTNS.INF");
        let blocks = functions_inf
            .as_ref()
            .and_then(|n| read_sidecar(fs, &path.join(n)))
            .map(|b| parse_functions(&b))
            .unwrap_or_default();
        let functions_inf_len = functions_inf
            .as_ref()
            .and_then(|n| files.iter().find(|(f, _)| f == n).map(|(_, s)| *s));
        let calibrations: BTreeMap<u32, Calibration> = header
            .iter()
            .filter_map(|(k, v)| {
                let n = k.strip_prefix("Cal Function ")?.trim().parse().ok()?;
                Some((n, Calibration::parse(v)?))
            })
            .collect();
        let mut functions = Vec::new();
        for (name, len) in &files {
            let Some(n) = number_in(name, "_FUNC", ".IDX") else {
                continue;
            };
            if n == 0 {
                continue;
            }
            let b = read_capped(fs, &path.join(name), "scan index")?;
            let dat = find(&files, &format!("_FUNC{n:03}.DAT"));
            let dat_len = dat.and_then(|d| files.iter().find(|(f, _)| *f == d).map(|(_, s)| *s));
            let record_len = idx_record_len_for(&b, dat_len);
            let scans = parse_idx(&b, record_len);
            let bytes_per_value = infer_bytes_per_value(&scans, dat_len);
            let ext = extern_info.functions.get(&n);
            let block = blocks.get(n as usize - 1).cloned();
            // Polarity: the function's section, the acquisition-wide line, then the function
            // code (0x20 set in negative-ion functions).
            let polarity = ext
                .and_then(|e| e.polarity.clone())
                .or_else(|| extern_info.polarity.clone())
                .or_else(|| {
                    block.as_ref().map(|b| {
                        if b.code & 0x20 != 0 {
                            "negative".to_string()
                        } else {
                            "positive".to_string()
                        }
                    })
                });
            let (stats_layout, stats) = match find(&files, &format!("_FUNC{n:03}.STS")) {
                Some(s) => match read_capped(fs, &path.join(&s), "scan statistics") {
                    Ok(b) => (parse_stats_layout(&b), b),
                    Err(_) => (None, Vec::new()),
                },
                None => (None, Vec::new()),
            };
            // MSe high energy: the function's section ramps a high collision energy.
            let high_energy = ext.is_some_and(|e| {
                e.parameters.iter().any(|(k, _)| {
                    let k = k.to_ascii_uppercase();
                    k.starts_with("RAMP HIGH ENERGY") || k.contains("COLLISION ENERGY HIGH")
                })
            });
            let cdt = find(&files, &format!("_func{n:03}.cdt"));
            let drift_resolved = cdt.is_some();
            let drift = cdt.and_then(|c| {
                let ind = find(&files, &format!("_func{n:03}.ind"))?;
                let b = read_capped(fs, &path.join(ind), "drift index").ok()?;
                Some((parse_drift_index(&b)?, c))
            });
            functions.push(WatersFunction {
                number: n,
                high_energy,
                drift_resolved,
                drift,
                scans,
                idx_len: *len,
                idx_record_len: record_len,
                dat_len,
                bytes_per_value,
                block,
                polarity,
                kind: ext.and_then(|e| e.kind.clone()),
                data_format: ext.and_then(|e| e.data_format.clone()),
                calibration: calibrations.get(&n).cloned(),
                stats_layout,
                stats,
            });
        }
        functions.sort_by_key(|f| f.number);
        let chroms = find(&files, "_CHROMS.INF")
            .and_then(|n| read_sidecar(fs, &path.join(n)))
            .map(|b| parse_chroms(&b))
            .unwrap_or_default();
        let mut analog = Vec::new();
        for (name, len) in &files {
            let Some(n) = number_in(name, "_CHRO", ".DAT") else {
                continue;
            };
            if *len > MAX_INDEX || n == 0 {
                continue;
            }
            let b = fs
                .read(&path.join(name))
                .map_err(|e| Error::io(path.join(name), e))?;
            let body = b.get(CHRO_HEADER..).unwrap_or(&[]);
            let points = body
                .as_chunks::<8>()
                .0
                .iter()
                .filter_map(|c| Some((le_f32(c, 0)?, le_f32(c, 4)?)))
                .collect();
            let (description, unit, conversion) =
                chroms.get(n as usize - 1).cloned().unwrap_or_default();
            analog.push(AnalogChannel {
                number: n,
                file: name.clone(),
                description,
                unit,
                conversion,
                points,
                trailing: (body.len() % 8) as u64,
            });
        }
        analog.sort_by_key(|a| a.number);
        if functions.is_empty() && header.is_empty() {
            return Err(Error::corrupt(
                WATERS_ID,
                "no _HEADER.TXT and no _FUNCnnn.IDX in this directory",
            ));
        }
        // Spectral scans of all functions, merged by retention time (ties: function number).
        let mut order: Vec<(usize, usize)> = functions
            .iter()
            .enumerate()
            .filter(|(_, f)| f.is_spectral())
            .flat_map(|(fi, f)| (0..f.scans.len()).map(move |si| (fi, si)))
            .collect();
        order.sort_by(|a, b| {
            let ra = functions[a.0].scans[a.1].rt_min;
            let rb = functions[b.0].scans[b.1].rt_min;
            ra.total_cmp(&rb)
                .then(functions[a.0].number.cmp(&functions[b.0].number))
                .then(a.1.cmp(&b.1))
        });
        let mut drift_order = Vec::new();
        let mut next = 0u64;
        for (run0, &(fi, si)) in order.iter().enumerate() {
            if let Some((ix, _)) = &functions[fi].drift
                && si < ix.scans.len()
            {
                drift_order.push((fi, si, next, run0 as u64));
                next += ix.bins as u64;
            }
        }
        Ok(WatersRawDataset {
            drift_order,
            drift_cache: None,
            path: path.to_path_buf(),
            fs: fs.clone(),
            files,
            total_size,
            header,
            extern_inf,
            extern_info,
            inlet_inf,
            functions,
            analog,
            functions_inf_len,
            order,
        })
    }

    /// The functions, in number order.
    pub fn functions(&self) -> &[WatersFunction] {
        &self.functions
    }

    fn dat_path(&self, number: u32) -> PathBuf {
        let want = format!("_FUNC{number:03}.DAT");
        self.path.join(find(&self.files, &want).unwrap_or(want))
    }

    fn header_value(&self, key: &str) -> Option<String> {
        self.header
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.clone())
            .filter(|v| !v.is_empty())
    }

    fn base_extra(&self) -> BTreeMap<String, Value> {
        let mut m = BTreeMap::new();
        for (key, ours) in [
            ("Acquired Name", "sample_name"),
            ("Sample Description", "description"),
            ("SampleID", "sample_id"),
            ("User Name", "operator"),
            ("Instrument", "instrument"),
            ("Job Code", "job_code"),
            ("Bottle Number", "vial"),
            ("MS Method", "method"),
            ("Inlet Method", "inlet_method"),
            ("Tune Method", "tune_method"),
            ("Version", "header_version"),
        ] {
            if let Some(v) = self.header_value(key) {
                m.insert(ours.into(), json!(v));
            }
        }
        // `Instrument` is `MODEL#SERIAL` on Xevo/SYNAPT systems (e.g. `XEVO-TQS#WAA049`).
        if let Some((model, serial)) = m
            .get("instrument")
            .and_then(Value::as_str)
            .and_then(|s| s.split_once('#'))
            .map(|(a, b)| (a.trim().to_string(), b.trim().to_string()))
            .filter(|(a, b)| !a.is_empty() && !b.is_empty())
        {
            m.insert("instrument".into(), json!(model));
            // `NotSet` is the software's placeholder when no serial was configured.
            if !serial.eq_ignore_ascii_case("notset") {
                m.insert("instrument_serial".into(), json!(serial));
            }
        }
        if let Some(iso) = self.acquired_at() {
            m.insert("acquired_at".into(), json!(iso));
        }
        m
    }

    /// `Acquired Date` (`16-Jun-2021`) + `Acquired Time` (`22:01:20`) as ISO-8601.
    fn acquired_at(&self) -> Option<String> {
        let d = self.header_value("Acquired Date")?;
        let t = self
            .header_value("Acquired Time")
            .unwrap_or_else(|| "00:00:00".into());
        header_datetime(&d, &t)
    }

    fn function_trace(&self, index: u32, f: &WatersFunction) -> Option<TraceInfo> {
        let times: Vec<f64> = f.scans.iter().map(|s| f64::from(s.rt_min) * 60.0).collect();
        let (first, step) = uniform_times(&times)?;
        let mut extra = self.base_extra();
        extra.insert("function".into(), json!(f.number));
        extra.insert("x_start_min".into(), json!(tidy(first / 60.0)));
        extra.insert(
            "x_end_min".into(),
            json!(tidy(times[times.len() - 1] / 60.0)),
        );
        extra.insert(
            "axis".into(),
            json!({"quantity": "retention_time", "unit": "min", "first": tidy(first / 60.0), "step": tidy(step / 60.0)}),
        );
        extra.insert(
            "sampling".into(),
            json!("scan times evenly spaced within 1 % (actual times: _FUNCnnn.IDX)"),
        );
        if let Some(p) = &f.polarity {
            extra.insert("polarity".into(), json!(p));
        }
        if let Some(b) = f.bytes_per_value {
            extra.insert("bytes_per_value".into(), json!(b));
        }
        if let Some(k) = &f.kind {
            extra.insert("function_kind".into(), json!(k));
        }
        let mut channels = vec![SignalChannelInfo {
            index: 0,
            name: "TIC (stored)".into(),
            unit: Some("counts".into()),
            dtype: "float32".into(),
            scale: 1.0,
            offset: 0.0,
            extra: BTreeMap::new(),
        }];
        if f.decodable()
            && let Some(names) = f.value_names()
        {
            let b = f.block.as_ref().expect("value_names needs a block");
            for (i, n) in names.into_iter().enumerate() {
                let mut e = BTreeMap::new();
                e.insert("set_mz".into(), json!(f64::from(b.set_mz[i])));
                if let Some(p) = b.second_mz.get(i) {
                    e.insert("second_mz".into(), json!(f64::from(*p)));
                }
                channels.push(SignalChannelInfo {
                    index: i as u32 + 1,
                    name: n,
                    unit: Some("counts".into()),
                    dtype: if f.bytes_per_value == Some(2) {
                        "uint16"
                    } else {
                        "uint32"
                    }
                    .into(),
                    scale: 1.0,
                    offset: 0.0,
                    extra: e,
                });
            }
        } else {
            extra.insert("values_decoded".into(), json!(false));
        }
        Some(TraceInfo {
            index,
            name: Some(f.label()),
            sample_rate_hz: tidy(1.0 / step),
            sample_count: f.scans.len() as u64,
            sweep_count: 1,
            channels,
            start_s: Some(tidy(first)),
            extra,
        })
    }

    /// The wavelength grid (nm) of a PDA function: its first scan's 6-byte values, when every
    /// scan stores as many values.
    fn pda_wavelengths(&self, f: &WatersFunction) -> Option<Vec<f64>> {
        if f.bytes_per_value != Some(6) {
            return None;
        }
        let first = f.scans.first()?;
        if first.count == 0 || f.scans.iter().any(|s| s.count != first.count) {
            return None;
        }
        let (pairs, _) = self.raw_pairs(f, first).ok()?;
        Some(pairs.iter().map(|p| tidy(p.0)).collect())
    }

    fn pda_trace(&self, index: u32, f: &WatersFunction) -> Option<TraceInfo> {
        let wl = self.pda_wavelengths(f)?;
        let chan = |i: usize, name: String, unit: &str| SignalChannelInfo {
            index: i as u32,
            name,
            unit: Some(unit.into()),
            dtype: "float64".into(),
            scale: 1.0,
            offset: 0.0,
            extra: BTreeMap::new(),
        };
        let mut channels = vec![chan(0, "time".into(), "s")];
        for (k, w) in wl.iter().enumerate() {
            let mut c = chan(k + 1, format!("{w} nm"), "counts");
            c.extra.insert("wavelength_nm".into(), json!(w));
            channels.push(c);
        }
        let mut extra = self.base_extra();
        extra.insert("function".into(), json!(f.number));
        extra.insert("detector".into(), json!("photodiode array"));
        extra.insert("wavelength_range_nm".into(), json!([wl.first(), wl.last()]));
        extra.insert(
            "values".into(),
            json!("the stored signed absorbance counts of the 6-byte values (unscaled)"),
        );
        extra.insert(
            "irregular_sampling".into(),
            json!("sample_rate_hz is 0: channel `time` holds each sample's time in seconds"),
        );
        Some(TraceInfo {
            index,
            name: Some(format!("PDA ({})", f.label())),
            sample_rate_hz: 0.0,
            sample_count: f.scans.len() as u64,
            sweep_count: 1,
            channels,
            start_s: f.scans.first().map(|s| f64::from(s.rt_min) * 60.0),
            extra,
        })
    }

    fn analog_trace(&self, index: u32, a: &AnalogChannel) -> Option<TraceInfo> {
        let times: Vec<f64> = a.points.iter().map(|p| f64::from(p.0) * 60.0).collect();
        let (first, step) = uniform_times(&times)?;
        let mut extra = self.base_extra();
        extra.insert("analog_channel".into(), json!(a.number));
        extra.insert("file".into(), json!(a.file));
        extra.insert("x_start_min".into(), json!(tidy(first / 60.0)));
        extra.insert(
            "x_end_min".into(),
            json!(tidy(times[times.len() - 1] / 60.0)),
        );
        extra.insert(
            "axis".into(),
            json!({"quantity": "retention_time", "unit": "min", "first": tidy(first / 60.0), "step": tidy(step / 60.0)}),
        );
        let name = a
            .description
            .clone()
            .unwrap_or_else(|| format!("CHRO{:03}", a.number));
        Some(TraceInfo {
            index,
            name: Some(name.clone()),
            sample_rate_hz: tidy(1.0 / step),
            sample_count: a.points.len() as u64,
            sweep_count: 1,
            channels: vec![SignalChannelInfo {
                index: 0,
                name,
                unit: a.unit.clone(),
                dtype: "float32".into(),
                scale: 1.0,
                offset: 0.0,
                extra: BTreeMap::new(),
            }],
            start_s: Some(tidy(first)),
            extra,
        })
    }

    fn ms_trace(&self, index: u32, bpc: bool) -> TraceInfo {
        let chan = |i: u32, name: &str, unit: &str| SignalChannelInfo {
            index: i,
            name: name.into(),
            unit: Some(unit.into()),
            dtype: "float64".into(),
            scale: 1.0,
            offset: 0.0,
            extra: BTreeMap::new(),
        };
        let mut extra = self.base_extra();
        extra.insert(
            "kind".into(),
            json!(if bpc {
                "base peak"
            } else {
                "total ion current"
            }),
        );
        extra.insert(
            "source".into(),
            json!(if bpc {
                "the most intense value of each decoded spectrum"
            } else {
                "the TIC stored in _FUNCnnn.IDX"
            }),
        );
        extra.insert("irregular_sampling".into(), json!(true));
        extra.insert(
            "spectra".into(),
            json!("every spectrum of run 0, all functions"),
        );
        TraceInfo {
            index,
            name: Some(if bpc { "BPC" } else { "TIC" }.into()),
            sample_rate_hz: 0.0,
            sample_count: self.order.len() as u64,
            sweep_count: 1,
            channels: vec![chan(0, "time", "s"), chan(1, "intensity", "counts")],
            start_s: self
                .order
                .first()
                .map(|&(f, s)| f64::from(self.functions[f].scans[s].rt_min) * 60.0),
            extra,
        }
    }

    /// Stored TIC and (when decodable) the MRM/SIR values of scans `[a, a + count)`, column-major.
    fn function_values(&self, f: &WatersFunction, a: usize, count: usize) -> Result<Vec<Vec<f64>>> {
        let scans = &f.scans[a..a + count];
        let mut channels = vec![
            scans
                .iter()
                .map(|scan| f64::from(scan.stored_tic))
                .collect::<Vec<_>>(),
        ];
        if f.decodable() {
            let bpv = f.bytes_per_value.unwrap_or(4);
            let width = f.scans[0].count as usize;
            let mut cols = vec![Vec::with_capacity(count); width];
            if let (Some(first), Some(last)) = (scans.first(), scans.last()) {
                let dat = self.dat_path(f.number);
                let mut file = self.fs.open(&dat).map_err(|e| Error::io(&dat, e))?;
                let start = first.offset;
                let end = last.offset + bpv * u64::from(last.count);
                let bytes = read_range(&mut file, &dat, start, end.saturating_sub(start))?;
                for scan in scans {
                    for (c, col) in cols.iter_mut().enumerate() {
                        let pos = (scan.offset.saturating_sub(start) + bpv * c as u64) as usize;
                        let value = if bpv == 2 {
                            le_u16(&bytes, pos).map(decode_packed16)
                        } else {
                            le_u32(&bytes, pos).map(decode_packed32)
                        };
                        col.push(value.ok_or_else(|| {
                            Error::corrupt_at(
                                WATERS_ID,
                                scan.offset,
                                "scan values run past the end of the DAT file (truncated)",
                            )
                        })?);
                    }
                }
            }
            channels.extend(cols);
        }
        Ok(channels)
    }

    fn function_table(&self, index: u32, f: &WatersFunction) -> TableInfo {
        let mut columns = vec![
            ColumnInfo {
                index: 0,
                name: "rt_min".into(),
                dtype: "float32".into(),
                unit: Some("min".into()),
                ..ColumnInfo::default()
            },
            ColumnInfo {
                index: 1,
                name: "tic_stored".into(),
                dtype: "float32".into(),
                unit: Some("counts".into()),
                ..ColumnInfo::default()
            },
        ];
        if f.decodable()
            && let Some(names) = f.value_names()
        {
            for (i, n) in names.into_iter().enumerate() {
                columns.push(ColumnInfo {
                    index: i as u32 + 2,
                    name: n,
                    dtype: if f.bytes_per_value == Some(2) {
                        "uint16"
                    } else {
                        "uint32"
                    }
                    .into(),
                    unit: Some("counts".into()),
                    ..ColumnInfo::default()
                });
            }
        }
        // Header fields (sample, instrument, operator, date) as on traces, so an MRM-only run
        // without an evenly spaced trace still carries them.
        let mut extra = self.base_extra();
        extra.insert("function".into(), json!(f.number));
        if let Some(p) = &f.polarity {
            extra.insert("polarity".into(), json!(p));
        }
        if let Some(b) = f.bytes_per_value {
            extra.insert("bytes_per_value".into(), json!(b));
        }
        if let Some(k) = &f.kind {
            extra.insert("function_kind".into(), json!(k));
        }
        if f.is_spectral() {
            extra.insert(
                "spectra".into(),
                json!("this function's scans are spectra of run 0 (openreadout spectra)"),
            );
        }
        TableInfo {
            index,
            name: Some(f.label()),
            row_count: f.scans.len() as u64,
            columns,
            extra,
        }
    }

    /// Trace index → what it reads: uniformly sampled functions, analog channels, then TIC and
    /// BPC over all spectra.
    fn trace_map(&self) -> Vec<TraceKind> {
        let mut out = Vec::new();
        for (i, f) in self.functions.iter().enumerate() {
            let times: Vec<f64> = f.scans.iter().map(|s| f64::from(s.rt_min)).collect();
            if uniform_times(&times).is_some() {
                out.push(TraceKind::Function(i));
            }
        }
        for (i, a) in self.analog.iter().enumerate() {
            let times: Vec<f64> = a.points.iter().map(|p| f64::from(p.0)).collect();
            if uniform_times(&times).is_some() {
                out.push(TraceKind::Analog(i));
            }
        }
        for (i, f) in self.functions.iter().enumerate() {
            if f.is_pda() && self.pda_wavelengths(f).is_some() {
                out.push(TraceKind::Pda(i));
            }
        }
        if !self.order.is_empty() {
            out.push(TraceKind::Tic);
            out.push(TraceKind::Bpc);
        }
        out
    }

    /// (stored mass, intensity) pairs of one scan, as stored, and (12-byte layout only) each
    /// point's flags ([`crate::layout::value12_flags`]; empty for other layouts).
    fn raw_pairs(&self, f: &WatersFunction, s: &ScanIndex) -> Result<RawScan> {
        let bpv = f.bytes_per_value.unwrap_or(0);
        let len = bpv * u64::from(s.count);
        if len > MAX_SCAN_BYTES {
            return Err(Error::unsupported(
                WATERS_ID,
                format!("a scan of {len} bytes"),
                "Scans above 1 GiB are not read.",
            ));
        }
        let dat = self.dat_path(f.number);
        let mut h = self.fs.open(&dat).map_err(|e| Error::io(&dat, e))?;
        let bytes = read_range(&mut h, &dat, s.offset, len)?;
        if (bytes.len() as u64) < len {
            return Err(Error::corrupt_at(
                WATERS_ID,
                s.offset,
                format!(
                    "{}: scan values run past the end of the DAT file (truncated)",
                    f.label()
                ),
            ));
        }
        let step = bpv as usize;
        let decode: fn(&[u8]) -> Option<(f64, f64)> = match bpv {
            12 => decode_value12,
            8 => decode_value8,
            6 => decode_value6,
            _ => {
                return Err(Error::unsupported(
                    WATERS_ID,
                    format!("{bpv}-byte values as spectra"),
                    "Only the 6-, 8- and 12-byte layouts hold (mass, intensity) pairs.",
                ));
            }
        };
        let flags = if bpv == 12 {
            bytes
                .chunks_exact(step)
                .map(crate::layout::value12_flags)
                .collect()
        } else {
            Vec::new()
        };
        Ok((bytes.chunks_exact(step).filter_map(decode).collect(), flags))
    }

    /// Function and scan position of spectrum `index`.
    fn locate(&self, index: u64) -> Result<(usize, usize)> {
        usize::try_from(index)
            .ok()
            .and_then(|i| self.order.get(i))
            .copied()
            .ok_or_else(|| {
                Error::Usage(format!(
                    "spectrum {index} does not exist ({} spectra)",
                    self.order.len()
                ))
            })
    }

    /// Metadata of spectrum `index` from the function's index, `.STS` statistics and header
    /// alone (no scan values read); TIC and base peak are left to the caller.
    fn meta_at(&self, index: u64) -> Result<Spectrum> {
        let (fi, si) = self.locate(index)?;
        let f = &self.functions[fi];
        let s = &f.scans[si];
        let calibrate = !f.stored_calibrated();
        let cal = f
            .calibration
            .as_ref()
            .filter(|c| calibrate && !c.is_identity());
        let ms_level = f.ms_level();
        let ms2 = ms_level >= 2;
        let mut extra = BTreeMap::new();
        extra.insert("function".into(), json!(f.number));
        if let Some(k) = &f.kind {
            extra.insert("function_kind".into(), json!(k));
        }
        if f.is_reference() {
            extra.insert("lock_mass_reference".into(), json!(true));
        }
        extra.insert("stored_tic".into(), json!(f64::from(s.stored_tic)));
        extra.insert(
            "calibration".into(),
            json!(match (f.stored_calibrated(), cal) {
                (true, _) => "stored calibrated (index flag 0x100)".to_string(),
                (false, Some(c)) => format!(
                    "Cal Function {} ({})",
                    f.number,
                    match c.kind {
                        crate::layout::CalibrationKind::Linear => "T0",
                        crate::layout::CalibrationKind::SquareRoot => "T1",
                    }
                ),
                (false, None) => "none".to_string(),
            }),
        );
        for (name, key) in [
            ("Cone", "cone_v"),
            ("Collision Energy2", "collision_energy2"),
        ] {
            if let Some(v) = f.stat(si, name) {
                extra.insert(key.into(), json!(v));
            }
        }
        // Precursor: a product-ion scan's block set mass; a TOF MS/MS scan's `Set Mass`
        // statistic; an MSe high-energy scan fragments its whole scan window (reported as that
        // window, centred, as the vendor library's conversions do).
        let block_type = f.block.as_ref().map(FunctionBlock::function_type);
        let mut isolation = None;
        let precursor = if !ms2 {
            None
        } else if block_type == Some(FUNCTION_DAUGHTER) {
            f.block
                .as_ref()
                .map(|b| f64::from(b.set_mass))
                .filter(|v| v.is_finite() && *v > 0.0)
        } else if block_type == Some(FUNCTION_TOF_MS) && f.high_energy {
            isolation = f.scan_window();
            isolation.map(|[a, b]| f64::midpoint(a, b))
        } else {
            f.stat(si, "Set Mass").filter(|&v| v > 0.0)
        };
        if f.high_energy && ms2 {
            extra.insert("mse_high_energy".into(), json!(true));
        }
        if f.drift_resolved {
            extra.insert(
                "drift_bins".into(),
                json!(if f.drift.is_some() {
                    "summed: the stored sum of the scan's drift bins; run 1 lists one spectrum per bin"
                } else {
                    "summed: the stored sum of the scan's drift bins; the drift index (_funcNNN.ind) is missing or unreadable, so the bins are not listed"
                }),
            );
        }
        Ok(Spectrum {
            index,
            scan_number: si as u64 + 1,
            ms_level,
            rt_s: Some(f64::from(s.rt_min) * 60.0),
            polarity: f.polarity.clone().unwrap_or_else(|| "unknown".into()),
            centroided: f.bytes_per_value == Some(12)
                || f.data_format
                    .as_deref()
                    .is_some_and(|d| d.eq_ignore_ascii_case("centroid")),
            precursor_mz: precursor,
            precursor_charge: None,
            scan_filter: None,
            total_ion_current: None,
            native_id: Some(format!("function={} process=0 scan={}", f.number, si + 1)),
            base_peak_mz: None,
            base_peak_intensity: None,
            precursor_intensity: None,
            isolation_window_mz: isolation,
            // The collision cells of Waters TOF instruments fragment by beam-type CID
            // (MS:1000422, the term our mzML writer uses for `HCD`), as the exports record.
            activation: ms2.then(|| "HCD".to_string()),
            collision_energy: f.stat(si, "Collision Energy").filter(|_| ms2),
            inverse_reduced_mobility: None,
            scan_window_mz: f.scan_window(),
            extra,
            mz: Vec::new(),
            intensity: Vec::new(),
        })
    }

    /// Header of spectrum `index`: [`Self::meta_at`] with the TIC the index stores for the scan.
    fn header_at(&self, index: u64) -> Result<openreadout_core::ScanHeader> {
        let (fi, si) = self.locate(index)?;
        let mut h = openreadout_core::ScanHeader::from(self.meta_at(index)?);
        h.total_ion_current = Some(f64::from(self.functions[fi].scans[si].stored_tic));
        Ok(h)
    }

    fn spectrum_at(&self, index: u64) -> Result<Spectrum> {
        let (fi, si) = self.locate(index)?;
        let mut sp = self.meta_at(index)?;
        let f = &self.functions[fi];
        let s = &f.scans[si];
        let (pairs, flags) = self.raw_pairs(f, s)?;
        let calibrate = !f.stored_calibrated();
        let cal = f
            .calibration
            .as_ref()
            .filter(|c| calibrate && !c.is_identity());
        // The 12-byte layout is validated bit for bit with the calibrated mass rounded to single
        // precision; the 6- and 8-byte layouts keep double precision.
        let round32 = f.bytes_per_value == Some(12);
        let mut pts: Vec<(f64, f32)> = pairs
            .iter()
            .map(|&(m, i)| {
                let m = cal.map_or(m, |c| c.apply(m));
                let m = if round32 { f64::from(m as f32) } else { m };
                (m, i as f32)
            })
            .collect();
        // Flagged points: the lock-mass peak of a reference scan (0x40), others counted.
        let lock_peaks: Vec<f64> = flags
            .iter()
            .zip(&pts)
            .filter(|(fl, _)| **fl & crate::layout::FLAG_LOCK_MASS_PEAK != 0)
            .map(|(_, p)| p.0)
            .collect();
        if !lock_peaks.is_empty() {
            sp.extra
                .insert("lock_mass_peak_mz".into(), json!(lock_peaks));
        }
        let other_flags = flags
            .iter()
            .filter(|fl| **fl & !crate::layout::FLAG_LOCK_MASS_PEAK != 0)
            .count();
        if other_flags > 0 {
            sp.extra.insert("flagged_points".into(), json!(other_flags));
        }
        if !pts.windows(2).all(|w| w[0].0 <= w[1].0) {
            pts.sort_by(|a, b| a.0.total_cmp(&b.0));
        }
        let tic: f64 = pts.iter().map(|p| f64::from(p.1)).sum();
        let base = pts
            .iter()
            .copied()
            .fold(None::<(f64, f32)>, |acc, p| match acc {
                Some(a) if a.1 >= p.1 => Some(a),
                _ => Some(p),
            });
        sp.total_ion_current = Some(tic);
        sp.base_peak_mz = base.map(|b| b.0);
        sp.base_peak_intensity = base.map(|b| f64::from(b.1));
        sp.mz = pts.iter().map(|p| p.0).collect();
        sp.intensity = pts.iter().map(|p| p.1).collect();
        Ok(sp)
    }

    /// Spectra in run 1: one per drift bin of every drift-resolved scan.
    fn drift_count(&self) -> u64 {
        self.drift_order.last().map_or(0, |&(fi, _, start, _)| {
            start
                + self.functions[fi]
                    .drift
                    .as_ref()
                    .map_or(0, |(ix, _)| ix.bins as u64)
        })
    }

    /// Run-1 spectrum `k`: its entry in `drift_order` and its drift bin.
    fn drift_locate(&self, k: u64) -> Result<(usize, usize)> {
        let n = self.drift_count();
        if k >= n {
            return Err(Error::Usage(format!(
                "spectrum {k} of run 1 does not exist ({n} drift-bin spectra)"
            )));
        }
        let i = self
            .drift_order
            .partition_point(|&(_, _, start, _)| start <= k)
            .saturating_sub(1);
        let start = self.drift_order[i].2;
        Ok((i, usize::try_from(k - start).unwrap_or(usize::MAX)))
    }

    /// Header of run-1 spectrum `k`, without its points: the drift-summed scan's metadata with
    /// the bin's number, native id and drift time.
    fn drift_meta(&self, k: u64) -> Result<Spectrum> {
        let (i, bin) = self.drift_locate(k)?;
        let (fi, si, _, run0) = self.drift_order[i];
        let f = &self.functions[fi];
        let bins = f.drift.as_ref().map_or(0, |(ix, _)| ix.bins);
        let mut sp = self.meta_at(run0)?;
        sp.index = k;
        // The vendor library numbers a drift-resolved function's spectra per bin.
        sp.scan_number = (si * bins + bin + 1) as u64;
        sp.native_id = Some(format!(
            "function={} process=0 scan={}",
            f.number, sp.scan_number
        ));
        sp.extra.remove("drift_bins");
        sp.extra.remove("stored_tic");
        sp.extra.insert("scan".into(), json!(si + 1));
        if self.extern_info.sonar {
            sp.extra.insert("sonar_bin".into(), json!(bin));
        } else {
            sp.extra.insert("drift_bin".into(), json!(bin));
            // A bin lasts `ADC Pushes Per IMS Increment` pusher periods.
            if let (Some(hz), Some(pushes)) = (
                f.stat(si, "Pusher Frequency").filter(|&v| v > 0.0),
                self.extern_info.pushes_per_drift_bin.filter(|&p| p > 0),
            ) {
                sp.extra.insert(
                    "drift_time_ms".into(),
                    json!(tidy(bin as f64 * f64::from(pushes) * 1000.0 / hz)),
                );
            }
        }
        Ok(sp)
    }

    /// Decode the drift bins of one scan. Each time-of-flight index takes the m/z of the
    /// drift-summed spectrum's point of the same rank, after checking that the bins sum to that
    /// spectrum point for point (docs/provenance/waters-raw.md, 2026-10-06).
    fn decode_drift(&self, fi: usize, si: usize, run0: u64) -> Result<DriftSpectra> {
        let f = &self.functions[fi];
        let Some((ix, cdt)) = &f.drift else {
            return Err(Error::Usage(format!("{} has no drift index", f.label())));
        };
        let ds = ix.scans.get(si).ok_or_else(|| {
            Error::Usage(format!("{} scan {} has no drift data", f.label(), si + 1))
        })?;
        let len = ds.byte_len();
        if len > MAX_SCAN_BYTES {
            return Err(Error::unsupported(
                WATERS_ID,
                format!("drift data of {len} bytes in one scan"),
                "Scans above 1 GiB are not read.",
            ));
        }
        let path = self.path.join(cdt);
        let mut h = self.fs.open(&path).map_err(|e| Error::io(&path, e))?;
        let bytes = read_range(&mut h, &path, ds.offset, len)?;
        if (bytes.len() as u64) < len {
            return Err(Error::corrupt_at(
                WATERS_ID,
                ds.offset,
                format!(
                    "{cdt}: scan {} runs past the end of the file (truncated)",
                    si + 1
                ),
            ));
        }
        let what = format!("drift bins of {} scan {}", f.label(), si + 1);
        let bins = decode_drift_scan(&bytes, ds).map_err(|e| {
            Error::unsupported(
                WATERS_ID,
                format!("{what}: {e}"),
                "Read the drift-summed spectra of run 0 instead.",
            )
        })?;
        let summed = self.spectrum_at(run0)?;
        let stored: Vec<(f64, f32)> = summed
            .mz
            .iter()
            .zip(&summed.intensity)
            .filter(|(_, i)| **i != 0.0)
            .map(|(&m, &i)| (m, i))
            .collect();
        let mut sums: BTreeMap<u64, i64> = BTreeMap::new();
        for b in &bins {
            for (&t, &v) in b.tof.iter().zip(&b.intensity) {
                *sums.entry(t).or_default() += v;
            }
        }
        if sums.len() != stored.len()
            || sums
                .values()
                .zip(&stored)
                .any(|(&s, &(_, i))| (s as f32).to_bits() != i.to_bits())
        {
            return Err(Error::unsupported(
                WATERS_ID,
                format!(
                    "{what}: the bins ({} points) do not sum to the stored drift-summed spectrum ({} points)",
                    sums.len(),
                    stored.len()
                ),
                "Read the drift-summed spectra of run 0 instead.",
            ));
        }
        let mz_of: BTreeMap<u64, f64> = sums
            .keys()
            .copied()
            .zip(stored.iter().map(|p| p.0))
            .collect();
        Ok(bins
            .iter()
            .map(|b| {
                (
                    b.tof.iter().filter_map(|t| mz_of.get(t).copied()).collect(),
                    b.intensity.iter().map(|&v| v as f32).collect(),
                )
            })
            .collect())
    }

    /// Run-1 spectrum `k`: one drift bin of a scan, with its points.
    fn drift_spectrum(&mut self, k: u64) -> Result<Spectrum> {
        let mut sp = self.drift_meta(k)?;
        let (i, bin) = self.drift_locate(k)?;
        let (fi, si, _, run0) = self.drift_order[i];
        if self
            .drift_cache
            .as_ref()
            .is_none_or(|(key, _)| *key != (fi, si))
        {
            let d = self.decode_drift(fi, si, run0)?;
            self.drift_cache = Some(((fi, si), d));
        }
        let (mz, intensity) = self
            .drift_cache
            .as_ref()
            .and_then(|(_, d)| d.get(bin))
            .cloned()
            .unwrap_or_default();
        let tic: f64 = intensity.iter().map(|&v| f64::from(v)).sum();
        let base = mz
            .iter()
            .zip(&intensity)
            .fold(None::<(f64, f32)>, |acc, (&m, &v)| match acc {
                Some(a) if a.1 >= v => Some(a),
                _ => Some((m, v)),
            });
        sp.total_ion_current = Some(tic);
        sp.base_peak_mz = base.map(|b| b.0);
        sp.base_peak_intensity = base.map(|b| f64::from(b.1));
        sp.mz = mz;
        sp.intensity = intensity;
        Ok(sp)
    }

    /// Run 1: the drift bins of every drift-resolved scan, one spectrum each.
    fn drift_spectra_info(&self) -> SpectraInfo {
        let mut info = self.spectra_info();
        let funcs: Vec<&WatersFunction> = self
            .functions
            .iter()
            .filter(|f| f.is_spectral() && f.drift.is_some())
            .collect();
        info.index = 1;
        info.name = Some("drift bins".into());
        info.scan_count = self.drift_count();
        info.ms_levels = funcs
            .iter()
            .map(|f| f.ms_level())
            .collect::<BTreeSet<u32>>()
            .into_iter()
            .collect();
        let rts: Vec<f64> = self
            .drift_order
            .iter()
            .map(|&(fi, si, _, _)| f64::from(self.functions[fi].scans[si].rt_min) * 60.0)
            .collect();
        info.rt_range_s = match (rts.first(), rts.last()) {
            (Some(&a), Some(&b)) => Some([a, b]),
            _ => None,
        };
        info.extra.remove("stored_spectra");
        info.extra.insert(
            "drift_resolved".into(),
            json!(if self.extern_info.sonar {
                "SONAR quadrupole steps"
            } else {
                "ion mobility drift time"
            }),
        );
        info.extra.insert(
            "functions".into(),
            json!(
                funcs
                    .iter()
                    .map(|f| json!({
                        "function": f.number,
                        "kind": f.kind,
                        "ms_level": f.ms_level(),
                        "scans": f.drift.as_ref().map_or(0, |(ix, _)| ix.scans.len()),
                        "bins_per_scan": f.drift.as_ref().map_or(0, |(ix, _)| ix.bins),
                    }))
                    .collect::<Vec<_>>()
            ),
        );
        info
    }

    fn spectra_info(&self) -> SpectraInfo {
        let rts: Vec<f64> = self
            .order
            .iter()
            .map(|&(f, s)| f64::from(self.functions[f].scans[s].rt_min) * 60.0)
            .collect();
        let rt_range_s = match (rts.first(), rts.last()) {
            (Some(&a), Some(&b)) => Some([a, b]),
            _ => None,
        };
        let spectral: Vec<&WatersFunction> =
            self.functions.iter().filter(|f| f.is_spectral()).collect();
        let levels: BTreeSet<u32> = spectral.iter().map(|f| f.ms_level()).collect();
        let mut extra = self.base_extra();
        let pols: BTreeSet<String> = spectral
            .iter()
            .map(|f| f.polarity.clone().unwrap_or_else(|| "unknown".into()))
            .collect();
        extra.insert("polarities".into(), json!(pols));
        extra.insert(
            "functions".into(),
            json!(
                spectral
                    .iter()
                    .map(|f| json!({
                        "function": f.number,
                        "kind": f.kind,
                        "ms_level": f.ms_level(),
                        "scans": f.scans.len(),
                        "bytes_per_value": f.bytes_per_value,
                        "lock_mass_reference": f.is_reference(),
                        "scan_window_mz": f.scan_window(),
                    }))
                    .collect::<Vec<_>>()
            ),
        );
        if let Some(m) = self.extern_info.lock_mass {
            extra.insert("lock_mass".into(), json!(m));
        }
        extra.insert(
            "stored_spectra".into(),
            json!(if spectral.iter().all(|f| f.bytes_per_value == Some(12)) {
                "centroid"
            } else {
                "mixed"
            }),
        );
        let instrument = self.header_value("Instrument");
        let (model, serial) = match instrument.as_deref().and_then(|i| i.split_once('#')) {
            Some((m, s)) => (Some(m.trim().to_string()), Some(s.trim().to_string())),
            None => (instrument.clone(), None),
        };
        if let Some(s) = serial.filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("NotSet")) {
            extra.insert("instrument_serial".into(), json!(s));
        }
        extra.insert(
            "method_summary".into(),
            json!({
                "instrument_method": self.header_value("MS Method"),
                "inlet_method": self.header_value("Inlet Method"),
            }),
        );
        SpectraInfo {
            index: 0,
            name: self.header_value("Acquired Name").or_else(|| {
                self.path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
            }),
            scan_count: self.order.len() as u64,
            ms_levels: levels.into_iter().collect(),
            rt_range_s,
            instrument: Some(InstrumentInfo {
                manufacturer: Some("Waters".into()),
                model,
                software: Some("MassLynx".into()),
                software_version: self.extern_info.software_version.clone(),
                detector: None,
            }),
            extra,
        }
    }
}

impl Dataset for WatersRawDataset {
    fn info(&self) -> Result<FileInfo> {
        let traces: Vec<TraceInfo> = self
            .trace_map()
            .iter()
            .enumerate()
            .filter_map(|(t, k)| match *k {
                TraceKind::Function(i) => self.function_trace(t as u32, &self.functions[i]),
                TraceKind::Analog(i) => self.analog_trace(t as u32, &self.analog[i]),
                TraceKind::Pda(i) => self.pda_trace(t as u32, &self.functions[i]),
                TraceKind::Tic => Some(self.ms_trace(t as u32, false)),
                TraceKind::Bpc => Some(self.ms_trace(t as u32, true)),
            })
            .collect();
        let mut notes = vec![
            "one table per function (retention time, stored TIC from _FUNCnnn.IDX, and one column per MRM transition or SIR mass when the 4- or 2-byte value layout is recognised); the same as a trace when the scan times are evenly spaced; one trace per analog channel (_CHROnnn.DAT)".to_string(),
        ];
        if !self.order.is_empty() {
            notes.push(format!(
                "{} spectra from the scanning functions, merged by retention time into run 0 (native ids function=F process=0 scan=S); TIC and BPC traces over all of them",
                self.order.len()
            ));
        }
        if self.drift_count() > 0 {
            notes.push(format!(
                "{} drift-bin spectra in run 1: each drift-resolved scan of run 0 split into its bins (_funcNNN.cdt), numbered per bin as the vendor does (scan = (S - 1) x bins + bin + 1)",
                self.drift_count()
            ));
        }
        let undecoded: Vec<String> = self
            .functions
            .iter()
            .filter(|f| !f.decodable() && !f.is_spectral())
            .map(|f| {
                format!(
                    "{} ({} bytes per value)",
                    f.label(),
                    f.bytes_per_value.map_or("?".into(), |b| b.to_string())
                )
            })
            .collect();
        if !undecoded.is_empty() {
            notes.push(format!(
                "scan values not decoded (only the stored TIC and times are read): {}",
                undecoded.join(", ")
            ));
        }
        let expected = self.functions.len() + self.analog.len();
        let skipped = expected
            - traces
                .iter()
                .filter(|t| t.name.as_deref() != Some("TIC") && t.name.as_deref() != Some("BPC"))
                .count()
                .min(expected);
        if skipped > 0 {
            notes.push(format!(
                "{skipped} functions/analog channels have unevenly spaced times: read them as tables (exact retention times), not traces"
            ));
        }
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.total_size,
            format: WatersRawReader.descriptor(),
            format_version: self.header_value("Version"),
            images: Vec::new(),
            tables: self
                .functions
                .iter()
                .enumerate()
                .map(|(i, f)| self.function_table(i as u32, f))
                .collect(),
            spectra: if self.order.is_empty() {
                Vec::new()
            } else if self.drift_count() > 0 {
                vec![self.spectra_info(), self.drift_spectra_info()]
            } else {
                vec![self.spectra_info()]
            },
            traces,
            plane_count: 0,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let header: serde_json::Map<String, Value> = self
            .header
            .iter()
            .map(|(k, v)| (k.clone(), json!(v)))
            .collect();
        let funcs: Vec<Value> = self
            .functions
            .iter()
            .map(|f| {
                json!({
                    "function": f.number,
                    "kind": f.kind,
                    "data_format": f.data_format,
                    "scans": f.scans.len(),
                    "idx_bytes": f.idx_len,
                    "idx_record_bytes": f.idx_record_len,
                    "index_flags": f.scans.first().map(|s| s.flags),
                    "dat_bytes": f.dat_len,
                    "bytes_per_value": f.bytes_per_value,
                    "polarity": f.polarity,
                    "calibration": f.calibration.as_ref().map(|c| json!({
                        "coefficients": c.coefficients,
                        "kind": match c.kind {
                            crate::layout::CalibrationKind::Linear => "T0",
                            crate::layout::CalibrationKind::SquareRoot => "T1",
                        },
                    })),
                    "stored_calibrated": f.stored_calibrated(),
                    "rt_min": [f.scans.first().map(|s| s.rt_min), f.scans.last().map(|s| s.rt_min)],
                    "block": f.block.as_ref().map(|b| json!({
                        "code": format!("0x{:04X}", b.code),
                        "start_min": b.start_min,
                        "end_min": b.end_min,
                        "set_mz": b.set_mz,
                        "second_mz": b.second_mz,
                    })),
                    "statistics_fields": f.stats_layout.as_ref().map(|l| l.fields.iter().map(|x| x.name.clone()).collect::<Vec<_>>()),
                })
            })
            .collect();
        let analog: Vec<Value> = self
            .analog
            .iter()
            .map(|a| json!({"channel": a.number, "file": a.file, "description": a.description, "unit": a.unit, "conversion": a.conversion, "points": a.points.len()}))
            .collect();
        Ok(json!({
            "header": header,
            "functions": funcs,
            "analog_channels": analog,
            "extern_inf": self.extern_inf,
            "inlet_inf": self.inlet_inf,
            "files": self.files.iter().map(|(n, s)| json!({"name": n, "size": s})).collect::<Vec<_>>(),
        }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for (k, s) in [
            ("format_version", Source::Inferred),
            ("traces[].extra.sample_name", Source::Inferred),
            ("traces[].extra.acquired_at", Source::Inferred),
            ("traces[].sample_count", Source::PriorArt),
            ("traces[].sample_rate_hz", Source::PriorArt),
            ("traces[].channels[].unit", Source::Inferred),
            ("tables[].row_count", Source::PriorArt),
            ("tables[].columns[].name", Source::Inferred),
            ("tables[].extra.polarity", Source::Inferred),
        ] {
            p.insert(k.into(), s);
        }
        if !self.order.is_empty() {
            for k in [
                "spectra[0].scan_count",
                "spectra[0].ms_levels",
                "spectra[0].rt_range_s",
                "spectra[0].instrument.model",
                "spectra[0].instrument.software_version",
                "spectra[0].extra.sample_name",
                "spectra[0].extra.acquired_at",
                "spectra[0].extra.polarities",
            ] {
                p.insert(k.into(), Source::Inferred);
            }
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let mut out = Vec::new();
        for (name, size) in &self.files {
            let u = name.to_ascii_uppercase();
            let (kind, details) = if let Some(n) = number_in(name, "_FUNC", ".IDX") {
                let f = self.functions.iter().find(|f| f.number == n);
                (
                    "scan-index",
                    json!({"function": n, "scans": f.map(|f| f.scans.len()), "record_bytes": f.map(|f| f.idx_record_len)}),
                )
            } else if let Some(n) = number_in(name, "_FUNC", ".DAT") {
                let f = self.functions.iter().find(|f| f.number == n);
                (
                    "scan-data",
                    json!({"function": n, "bytes_per_value": f.and_then(|f| f.bytes_per_value), "kind": f.and_then(|f| f.kind.clone())}),
                )
            } else if let Some(n) = number_in(name, "_FUNC", ".STS") {
                let f = self.functions.iter().find(|f| f.number == n);
                (
                    "scan-statistics",
                    json!({"function": n, "fields": f.and_then(|f| f.stats_layout.as_ref()).map(|l| l.fields.len())}),
                )
            } else if let Some(n) = number_in(name, "_CHRO", ".DAT") {
                let a = self.analog.iter().find(|a| a.number == n);
                (
                    "analog",
                    json!({"channel": n, "description": a.and_then(|a| a.description.clone()), "points": a.map(|a| a.points.len())}),
                )
            } else if Path::new(&u)
                .extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("txt") || x.eq_ignore_ascii_case("inf"))
            {
                ("metadata", json!({}))
            } else {
                ("file", json!({}))
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
        if !self.order.is_empty() {
            out.push(LsEntry {
                kind: "run".into(),
                name: "MS run".into(),
                offset: None,
                size: None,
                image: None,
                details: json!({
                    "spectra": self.order.len(),
                    "functions": self.functions.iter().filter(|f| f.is_spectral()).map(|f| f.number).collect::<Vec<_>>(),
                }),
            });
        }
        Ok(out)
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::unsupported(
            WATERS_ID,
            "image planes",
            "Waters .raw directories hold spectra and chromatograms: use `openreadout spectra` or `openreadout trace`.",
        ))
    }

    fn read_trace(
        &mut self,
        index: u32,
        sweep: u32,
        first_sample: u64,
        max_samples: u64,
    ) -> Result<Trace> {
        let map = self.trace_map();
        let &kind = map.get(index as usize).ok_or_else(|| {
            Error::Usage(format!("trace {index} out of range ({} traces)", map.len()))
        })?;
        if sweep != 0 {
            return Err(Error::Usage(format!(
                "sweep {sweep} out of range (one sweep)"
            )));
        }
        let total = match kind {
            TraceKind::Function(i) => self.functions[i].scans.len(),
            TraceKind::Analog(i) => self.analog[i].points.len(),
            TraceKind::Pda(i) => self.functions[i].scans.len(),
            TraceKind::Tic | TraceKind::Bpc => self.order.len(),
        } as u64;
        if first_sample > total {
            return Err(Error::Usage(format!(
                "first sample {first_sample} past the end ({total})"
            )));
        }
        let n = max_samples.min(total - first_sample) as usize;
        let a = first_sample as usize;
        let channels = match kind {
            TraceKind::Analog(i) => {
                vec![
                    self.analog[i].points[a..a + n]
                        .iter()
                        .map(|p| f64::from(p.1))
                        .collect(),
                ]
            }
            TraceKind::Function(i) => {
                let f = self.functions[i].clone();
                self.function_values(&f, a, n)?
            }
            TraceKind::Pda(i) => {
                let f = &self.functions[i];
                let width = self.pda_wavelengths(f).map_or(0, |w| w.len());
                let mut ch = vec![Vec::with_capacity(n); width + 1];
                for s in &f.scans[a..a + n] {
                    ch[0].push(f64::from(s.rt_min) * 60.0);
                    let (pairs, _) = self.raw_pairs(f, s)?;
                    for (k, c) in ch.iter_mut().skip(1).enumerate() {
                        c.push(pairs.get(k).map_or(f64::NAN, |p| p.1));
                    }
                }
                ch
            }
            TraceKind::Tic => {
                let rows = &self.order[a..a + n];
                vec![
                    rows.iter()
                        .map(|&(f, s)| f64::from(self.functions[f].scans[s].rt_min) * 60.0)
                        .collect(),
                    rows.iter()
                        .map(|&(f, s)| f64::from(self.functions[f].scans[s].stored_tic))
                        .collect(),
                ]
            }
            TraceKind::Bpc => {
                let mut t = Vec::with_capacity(n);
                let mut v = Vec::with_capacity(n);
                for i in a..a + n {
                    let sp = self.spectrum_at(i as u64)?;
                    // every MassLynx scan index row stores its time
                    t.push(sp.rt_s.unwrap_or(f64::NAN));
                    v.push(sp.base_peak_intensity.unwrap_or(0.0));
                }
                vec![t, v]
            }
        };
        Ok(Trace {
            trace: index,
            sweep: 0,
            first_sample,
            channels,
        })
    }

    fn read_table(&mut self, index: u32, first_row: u64, max_rows: u64) -> Result<Table> {
        let f = self.functions.get(index as usize).cloned().ok_or_else(|| {
            Error::Usage(format!(
                "table {index} out of range ({} functions)",
                self.functions.len()
            ))
        })?;
        let total = f.scans.len() as u64;
        if first_row > total {
            return Err(Error::Usage(format!(
                "first row {first_row} past the end ({total} scans)"
            )));
        }
        let n = max_rows.min(total - first_row) as usize;
        let a = first_row as usize;
        let mut columns = vec![
            f.scans[a..a + n]
                .iter()
                .map(|s| f64::from(s.rt_min))
                .collect(),
        ];
        columns.extend(self.function_values(&f, a, n)?);
        Ok(Table {
            table: index,
            first_row,
            columns,
        })
    }

    fn visit_scan_headers(
        &mut self,
        run: u32,
        first: u64,
        visit: &mut dyn FnMut(openreadout_core::ScanHeader) -> bool,
    ) -> Result<bool> {
        if run == 1 && self.drift_count() > 0 {
            for k in first..self.drift_count() {
                let h = openreadout_core::ScanHeader::from(self.drift_meta(k)?);
                if !visit(h) {
                    break;
                }
            }
            return Ok(true);
        }
        if run != 0 {
            return Err(Error::Usage(format!(
                "run {run} out of range (this MassLynx .raw holds {} runs)",
                if self.drift_count() > 0 { 2 } else { 1 }
            )));
        }
        for i in first..self.order.len() as u64 {
            if !visit(self.header_at(i)?) {
                break;
            }
        }
        Ok(true)
    }

    fn read_spectrum(&mut self, index: u32, spectrum: u64) -> Result<Spectrum> {
        if index == 1 && self.drift_count() > 0 {
            return self.drift_spectrum(spectrum);
        }
        if index != 0 || self.order.is_empty() {
            return Err(Error::Usage(if self.order.is_empty() {
                "this .raw directory holds no scanning (full-scan) function; MRM/SIR values are tables".to_string()
            } else {
                format!(
                    "run {index} does not exist (run 0: spectra; run 1, when the data are drift-resolved: drift bins)"
                )
            }));
        }
        self.spectrum_at(spectrum)
    }

    fn find_spectrum(&mut self, run: u32, scan_number: u64) -> Result<Option<u64>> {
        if run == 1 {
            // Per-bin scan numbers count within a function; the lowest-numbered one wins.
            let Some(&(first, ..)) = self
                .drift_order
                .iter()
                .min_by_key(|e| self.functions[e.0].number)
            else {
                return Ok(None);
            };
            let bins = self.functions[first]
                .drift
                .as_ref()
                .map_or(0, |(ix, _)| ix.bins) as u64;
            if bins == 0 || scan_number == 0 {
                return Ok(None);
            }
            let (si, bin) = ((scan_number - 1) / bins, (scan_number - 1) % bins);
            return Ok(self
                .drift_order
                .iter()
                .find(|&&(f, s, _, _)| f == first && s as u64 == si)
                .map(|&(_, _, start, _)| start + bin));
        }
        if run != 0 {
            return Ok(None);
        }
        // Scan numbers count per function; the lowest-numbered spectral function wins.
        let Some(first) = self.functions.iter().position(WatersFunction::is_spectral) else {
            return Ok(None);
        };
        Ok(self
            .order
            .iter()
            .position(|&(f, s)| f == first && s as u64 + 1 == scan_number)
            .map(|i| i as u64))
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), WATERS_ID);
        r.performed("_HEADER.TXT present and parsed ($$ key: value lines)");
        r.performed("_FUNCnnn.IDX: whole 22- or 30-byte records; scan offsets/counts inside _FUNCnnn.DAT; retention times non-decreasing");
        r.performed("_FUNCTNS.INF holds one 416-byte block per function");
        r.performed("4-byte MRM values: each scan's decoded values sum to the stored TIC");
        r.performed("_CHROnnn.DAT: whole (time, value) pairs after the 128-byte header");
        if self.header.is_empty() {
            r.push(Finding::warning(
                "missing_header",
                "_HEADER.TXT is missing or has no $$ lines",
            ));
        }
        if let Some(len) = self.functions_inf_len {
            if len % FUNCTION_BLOCK as u64 != 0 {
                r.push(Finding::warning(
                    "bad_functions_inf",
                    format!("_FUNCTNS.INF is {len} bytes, not a multiple of {FUNCTION_BLOCK}"),
                ));
            } else if len / FUNCTION_BLOCK as u64 != self.functions.len() as u64 {
                r.push(Finding::warning(
                    "function_count_mismatch",
                    format!(
                        "_FUNCTNS.INF describes {} functions, {} _FUNCnnn.IDX files found",
                        len / FUNCTION_BLOCK as u64,
                        self.functions.len()
                    ),
                ));
            }
        }
        for f in self.functions.clone() {
            let name = f.label();
            if f.idx_len % f.idx_record_len as u64 != 0 {
                r.push(Finding::error(
                    "truncated",
                    format!(
                        "_{name}.IDX is {} bytes, not whole {}-byte records",
                        f.idx_len, f.idx_record_len
                    ),
                ));
            }
            let Some(dat_len) = f.dat_len else {
                r.push(Finding::error(
                    "missing_file",
                    format!("_{name}.DAT is missing"),
                ));
                continue;
            };
            let bpv = f.bytes_per_value.unwrap_or(0);
            let mut outside = 0;
            for s in &f.scans {
                let end = s
                    .offset
                    .saturating_add(u64::from(s.count).saturating_mul(bpv.max(1)));
                if end > dat_len {
                    outside += 1;
                }
            }
            let total: u64 = f.scans.iter().map(|s| u64::from(s.count)).sum();
            if (f.bytes_per_value.is_none() || (total > 0 && dat_len % total != 0))
                && !f.scans.is_empty()
                && outside == 0
            {
                r.push(Finding::error(
                    "truncated",
                    format!("_{name}.DAT is {dat_len} bytes, not a whole number of bytes for the {total} values the index lists"),
                ));
            } else if outside > 0 {
                r.push(Finding::error(
                    "truncated",
                    format!("{outside} scans of {name} point past the end of _{name}.DAT"),
                ));
            }
            if f.scans.windows(2).any(|w| w[1].rt_min < w[0].rt_min) {
                r.push(Finding::warning(
                    "time_not_monotonic",
                    format!("{name}: retention times decrease somewhere"),
                ));
            }
            if let Some(l) = &f.stats_layout {
                let n = l.record_count(f.stats.len());
                if n != f.scans.len() {
                    r.push(Finding::warning(
                        "statistics_count_mismatch",
                        format!("_{name}.STS holds {n} records for {} scans", f.scans.len()),
                    ));
                }
            }
            if f.decodable() && bpv == 4 && outside == 0 {
                let path = self.dat_path(f.number);
                if let Ok(mut h) = self.fs.open(&path) {
                    let bytes = read_range(&mut h, &path, 0, dat_len)?;
                    let mut bad = 0u64;
                    for s in &f.scans {
                        let sum: f64 = (0..s.count as usize)
                            .filter_map(|c| le_u32(&bytes, s.offset as usize + 4 * c))
                            .map(decode_packed32)
                            .sum();
                        let t = f64::from(s.stored_tic);
                        if (sum - t).abs() > 1e-5 * t.abs().max(1.0) {
                            bad += 1;
                        }
                    }
                    if bad > 0 {
                        r.push(Finding::warning("tic_mismatch", format!("{name}: {bad} scans whose decoded values do not sum to the stored TIC")));
                    }
                }
            }
        }
        if !self.order.is_empty() {
            r.performed("spectra: decoded the first and last spectrum of every scanning function");
            let mut probes = Vec::new();
            for (fi, f) in self.functions.iter().enumerate() {
                if !f.is_spectral() || f.scans.is_empty() {
                    continue;
                }
                for si in [0, f.scans.len() - 1] {
                    if let Some(i) = self.order.iter().position(|&x| x == (fi, si)) {
                        probes.push(i as u64);
                    }
                }
            }
            for i in probes {
                if let Err(e) = self.spectrum_at(i) {
                    r.push(Finding::error("undecodable", format!("spectrum {i}: {e}")));
                }
            }
        }
        if self.drift_count() > 0 {
            r.performed("drift bins (_funcNNN.cdt): decoded the first and last scan of every drift-resolved function and checked that its bins sum to the stored drift-summed spectrum");
            let mut probes = Vec::new();
            for (fi, f) in self.functions.iter().enumerate() {
                let Some((ix, _)) = &f.drift else { continue };
                if ix.trailing != 0 {
                    r.push(Finding::error(
                        "truncated",
                        format!(
                            "_func{:03}.ind: {} bytes after the last whole scan block",
                            f.number, ix.trailing
                        ),
                    ));
                }
                let scans: Vec<(usize, u64)> = self
                    .drift_order
                    .iter()
                    .filter(|e| e.0 == fi)
                    .map(|e| (e.1, e.3))
                    .collect();
                if let (Some(&a), Some(&b)) = (scans.first(), scans.last()) {
                    probes.push((fi, a.0, a.1));
                    if b != a {
                        probes.push((fi, b.0, b.1));
                    }
                }
            }
            for (fi, si, run0) in probes {
                if let Err(e) = self.decode_drift(fi, si, run0) {
                    r.push(Finding::error("drift_undecodable", e.to_string()));
                }
            }
        }
        for a in &self.analog {
            if a.trailing != 0 {
                r.push(Finding::error(
                    "truncated",
                    format!(
                        "{}: {} bytes after the last whole (time, value) pair",
                        a.file, a.trailing
                    ),
                ));
            }
        }
        Ok(r)
    }
}
