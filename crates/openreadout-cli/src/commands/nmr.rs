//! `analyze nmr-peaks` (1D peaks, or the cross peaks of a 2D/3D spectrum), `--process` for
//! `trace`, `export` and `preview` (NMR FIDs as spectra), and `--slice` (rows, columns and
//! projections of a 2D spectrum as 1-D traces).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_core::{Dataset, Error, Registry, Result};
use openreadout_signal::api::NmrQuery;
use openreadout_signal::nmr::{
    Nmr2dReport, NmrMatrixDataset, NmrPeaksReport, NmrReport, SliceSpec,
};

use crate::output::{emit, fail};

/// A processing flag value with an optional dimension: `fN=VALUE` or `VALUE` (the direct
/// dimension, or the only one of a 1D FID).
fn split_dimension(s: &str) -> (Option<String>, &str) {
    match s.split_once('=') {
        Some((d, v)) if d.trim().to_ascii_lowercase().starts_with('f') => {
            (Some(d.trim().to_ascii_lowercase()), v.trim())
        }
        _ => (None, s.trim()),
    }
}

/// The processing flags (`--phase`, `--lb`, `--gb`, `--size`, `--baseline`, each repeatable
/// with `fN=` for one dimension of a 2D FID) as the fields of an [`NmrQuery`].
#[derive(Debug, Clone, Default)]
pub struct ProcessFlags<'a> {
    pub phase: &'a [String],
    pub lb: &'a [String],
    pub gb: &'a [String],
    pub size: &'a [String],
    pub baseline: &'a [String],
    pub no_group_delay: bool,
    pub nus: Option<String>,
}

impl ProcessFlags<'_> {
    /// Fill the processing fields of `q`; `prefix` names the flags in errors (`--` or
    /// `--process-`).
    pub fn apply(&self, q: &mut NmrQuery, prefix: &str) -> Result<()> {
        use openreadout_signal::api::DimensionQuery;
        let num = |flag: &str, v: &str| -> Result<f64> {
            v.parse::<f64>()
                .ok()
                .filter(|x| x.is_finite())
                .ok_or_else(|| {
                    Error::Usage(format!("{prefix}{flag} {v:?}: expected a number of Hz"))
                })
        };
        let count = |flag: &str, v: &str| -> Result<usize> {
            v.parse::<usize>().map_err(|_| {
                Error::Usage(format!(
                    "{prefix}{flag} {v:?}: expected a whole number of points"
                ))
            })
        };
        let mut seen: BTreeMap<(String, &str), ()> = BTreeMap::new();
        let mut once = |dim: &Option<String>, flag: &'static str| -> Result<()> {
            let key = (dim.clone().unwrap_or_default(), flag);
            if seen.insert(key, ()).is_some() {
                return Err(Error::Usage(format!(
                    "{prefix}{flag} is given twice for {}",
                    dim.as_deref().unwrap_or("the direct dimension")
                )));
            }
            Ok(())
        };
        let mut dims: BTreeMap<String, DimensionQuery> = BTreeMap::new();
        for s in self.phase {
            let (d, v) = split_dimension(s);
            once(&d, "phase")?;
            match d {
                Some(d) => dims.entry(d).or_default().phase = Some(v.into()),
                None => q.phase = Some(v.into()),
            }
        }
        for s in self.lb {
            let (d, v) = split_dimension(s);
            once(&d, "lb")?;
            let x = num("lb", v)?;
            match d {
                Some(d) => dims.entry(d).or_default().lb = Some(x),
                None => q.lb = Some(x),
            }
        }
        for s in self.gb {
            let (d, v) = split_dimension(s);
            once(&d, "gb")?;
            let x = num("gb", v)?;
            match d {
                Some(d) => dims.entry(d).or_default().gb = Some(x),
                None => q.gb = Some(x),
            }
        }
        for s in self.size {
            let (d, v) = split_dimension(s);
            once(&d, "size")?;
            let n = count("size", v)?;
            match d {
                Some(d) => dims.entry(d).or_default().size = Some(n),
                None => q.size = Some(n),
            }
        }
        for s in self.baseline {
            let (d, v) = split_dimension(s);
            once(&d, "baseline")?;
            match d {
                Some(d) => dims.entry(d).or_default().baseline = Some(v.into()),
                None => q.baseline = Some(v.into()),
            }
        }
        q.dimensions = dims;
        q.no_group_delay = self.no_group_delay;
        q.nus.clone_from(&self.nus);
        Ok(())
    }
}

/// `--process` and its options on `trace`, `export`, `preview` and `plot`.
#[derive(Debug, Clone, Default, clap::Args)]
pub struct ProcessArgs {
    /// NMR: read FID traces as spectra processed by OpenReadout (group-delay removal,
    /// apodization, zero filling, FT, phasing, baseline, ppm axis); a 2D/3D FID becomes its
    /// spectrum in every dimension (a matrix, or one spectrum per row of a pseudo-2D series).
    /// Stored spectra are unchanged.
    #[arg(long)]
    pub process: bool,
    /// With `--process`: `default` (stored phases, else automatic), `stored`, `auto`,
    /// `magnitude`, `none`, or `P0,P1` in degrees. 2D: `f1=MODE` for one dimension
    /// (repeatable); a bare value is the direct dimension.
    #[arg(long, value_name = "[fN=]MODE", requires = "process")]
    pub process_phase: Vec<String>,
    /// With `--process`: exponential line broadening, Hz (default: stored, else 0.3 Hz for 1H
    /// and 1 Hz otherwise). 2D: `f1=HZ` for one dimension.
    #[arg(
        long,
        value_name = "[fN=]HZ",
        requires = "process",
        allow_hyphen_values = true
    )]
    pub process_lb: Vec<String>,
    /// With `--process`: Gaussian broadening instead, Hz. 2D: `f1=HZ` for one dimension.
    #[arg(long, value_name = "[fN=]HZ", requires = "process")]
    pub process_gb: Vec<String>,
    /// With `--process`: transform size in complex points (power of two). 2D: `f1=N`.
    #[arg(long, value_name = "[fN=]N", requires = "process")]
    pub process_size: Vec<String>,
    /// With `--process`: baseline correction `default` (polynomial order 1), `none`, or
    /// `poly:N`. 2D: `f2=MODE` for one dimension (default none in 2D).
    #[arg(long, value_name = "[fN=]MODE", requires = "process")]
    pub process_baseline: Vec<String>,
    /// With `--process`, 2D FIDs sampled non-uniformly: `default` or `zero-fill` (the measured
    /// rows on the full grid, zeros elsewhere).
    #[arg(long, value_name = "MODE", requires = "process")]
    pub nus: Option<String>,
}

impl ProcessArgs {
    /// The processing request these flags describe.
    fn request(&self) -> Result<openreadout_signal::nmr::NmrPeaksRequest> {
        let mut q = NmrQuery::default();
        ProcessFlags {
            phase: &self.process_phase,
            lb: &self.process_lb,
            gb: &self.process_gb,
            size: &self.process_size,
            baseline: &self.process_baseline,
            no_group_delay: false,
            nus: self.nus.clone(),
        }
        .apply(&mut q, "--process-")?;
        q.peaks_request()
    }

    /// Wrap `ds` when `--process` was given: a 2D/3D FID is transformed in every dimension,
    /// 1D FIDs row by row.
    pub fn wrap(&self, ds: Box<dyn Dataset>) -> Result<Box<dyn Dataset>> {
        if !self.process {
            return Ok(ds);
        }
        let req = self.request()?;
        openreadout_signal::nmr::process_any(ds, req.nd, req.spectrum.process)
    }
}

/// `nmr-peaks`.
#[derive(Debug, Clone, clap::Args)]
pub struct NmrPeaksArgs {
    /// An NMR data set: a Bruker experiment directory, a Varian `.fid` directory, a JEOL `.jdf`
    /// or a JCAMP-DX NMR spectrum. A stored 2D/3D spectrum gives cross peaks.
    pub file: PathBuf,
    /// Spectrum source: `auto` (the vendor's processed spectrum when there is one, else the
    /// FID processed here), `fid` (always process the FID), `processed` (stored spectrum only).
    #[arg(long = "from", value_enum, default_value = "auto")]
    pub from: FromArg,
    /// Trace index (see `info`); default: chosen by `--from`.
    #[arg(long)]
    pub trace: Option<u32>,
    /// 1D peaks of one row: a row of a `ser`/arrayed FID (processed as a 1-D FID) or of a stored
    /// 2D spectrum (its F2 spectrum at one F1 point).
    #[arg(long)]
    pub sweep: Option<u32>,
    /// 2D spectra: 1D peaks of a slice: `row:PPM` (the F2 trace at an F1 shift), `column:PPM`
    /// (the F1 trace at an F2 shift), or a projection `skyline:f1|f2`, `min:f1|f2`, `sum:f1|f2`.
    #[arg(long, value_name = "SPEC", conflicts_with = "sweep")]
    pub slice: Option<String>,
    /// Phase: `default` (stored phases, else automatic), `stored`, `auto`, `magnitude`, `none`,
    /// or `P0,P1` in degrees (correction `e^{-i(P0 + P1*k/N)}`, k from the high-ppm edge).
    /// 2D FIDs: `f1=MODE` for one dimension (repeatable); a bare value is the direct dimension.
    #[arg(long, value_name = "[fN=]MODE")]
    pub phase: Vec<String>,
    /// Exponential line broadening, Hz (0 = no window; default: stored, else 0.3 Hz for 1H and
    /// 1 Hz otherwise). 2D FIDs: `f1=HZ`.
    #[arg(long, value_name = "[fN=]HZ", allow_hyphen_values = true)]
    pub lb: Vec<String>,
    /// Gaussian window instead: Gaussian line width, Hz. 2D FIDs: `f1=HZ`.
    #[arg(long, value_name = "[fN=]HZ")]
    pub gb: Vec<String>,
    /// Transform size in complex points (power of two; default: stored, else 2 × the FID).
    /// 2D FIDs: `f1=N`.
    #[arg(long, value_name = "[fN=]N")]
    pub size: Vec<String>,
    /// Baseline correction: `default` (polynomial order 1 through automatically found baseline
    /// blocks), `none`, `poly:N`. Stored spectra are used as stored unless `--baseline` is given.
    /// 2D FIDs: `f2=MODE` (default none).
    #[arg(long, value_name = "[fN=]MODE")]
    pub baseline: Vec<String>,
    /// Do not remove the digital-filter group delay.
    #[arg(long)]
    pub no_group_delay: bool,
    /// 2D FIDs sampled non-uniformly: `default` or `zero-fill` (the measured rows on the full
    /// grid, zeros elsewhere).
    #[arg(long, value_name = "MODE")]
    pub nus: Option<String>,
    /// Minimum peak height as a multiple of the noise SD.
    #[arg(long, value_name = "X", default_value_t = 10.0)]
    pub min_snr: f64,
    /// Minimum prominence (height above the higher neighbouring minimum) in noise SDs; 2D: along
    /// every dimension.
    #[arg(long, value_name = "X", default_value_t = 5.0)]
    pub min_prominence: f64,
    /// Minimum height as a fraction of the tallest point (e.g. 0.01 = 1 %).
    #[arg(long, value_name = "F", default_value_t = 0.0)]
    pub min_height_fraction: f64,
    /// Report negative peaks (DEPT, APT). 2D spectra report them by default (edited HSQC CH₂,
    /// phase-sensitive NOESY, ROESY and COSY).
    #[arg(long, overrides_with = "no_negative")]
    pub negative: bool,
    /// Leave out negative peaks (2D).
    #[arg(long, overrides_with = "negative")]
    pub no_negative: bool,
    /// Only pick peaks between two shifts, `A:B` in ppm. 2D: one per dimension, `f1=A:B` and
    /// `f2=C:D` (repeat the flag); a bare `A:B` is F2. Only that part of a 2D spectrum is read.
    #[arg(long, value_name = "[fN=]A:B", allow_hyphen_values = true)]
    pub range_ppm: Vec<String>,
    /// Keep at most this many peaks (the tallest).
    #[arg(long, value_name = "N", default_value_t = 1000)]
    pub max_peaks: usize,
    /// Integrate a region `A:B` (ppm); repeatable. 2D: a box with one range per dimension,
    /// `f1=A:B,f2=C:D`.
    #[arg(long = "integrate", value_name = "A:B", allow_hyphen_values = true)]
    pub integrate: Vec<String>,
    /// Without --integrate: integrate every multiplet (peaks closer than 15 Hz, with 10 Hz on
    /// each side). 1D only.
    #[arg(long, conflicts_with = "integrate")]
    pub auto_integrate: bool,
    /// Normalise integrals so region (or box) I (0-based) equals V: `I=V` (default `0=1`).
    #[arg(long, value_name = "I=V")]
    pub integral_reference: Option<String>,
    /// Write the peak rows (2D: cross-peak rows, one column per dimension) to a file: `.csv`,
    /// `.tsv`, `.jsonl` or `.parquet`. With --integrate, the integral rows.
    #[arg(short, long, value_name = "FILE")]
    pub output: Option<PathBuf>,
    /// Draw the spectrum with its peaks to a `.png`, `.jpg` or `.svg` (2D: a contour plot with
    /// the cross peaks marked).
    #[arg(long, value_name = "FILE")]
    pub plot: Option<PathBuf>,
    /// Plot width in pixels.
    #[arg(long, default_value_t = 1200)]
    pub width: u32,
    /// Replace existing output files.
    #[arg(long)]
    pub overwrite: bool,
    #[command(flatten)]
    pub cross: CrossPeakArgs,
    #[arg(long)]
    pub json: bool,
}

/// The 2D picking options (`CrossPeakOptions`), one flag each.
#[derive(Debug, Clone, Default, clap::Args)]
#[command(next_help_heading = "2D spectra")]
pub struct CrossPeakArgs {
    /// Minimum height in units of the local noise (of the row and column through the peak)
    /// [default: 6].
    #[arg(long, value_name = "X")]
    pub min_local_snr: Option<f64>,
    /// Leave out cross peaks with fewer footprint points (isolated spikes) [default: 3].
    #[arg(long, value_name = "N")]
    pub min_footprint_points: Option<usize>,
    /// Maxima closer than this in every dimension form one cross peak, Hz [default: 15].
    #[arg(long, value_name = "HZ")]
    pub group_gap_hz: Option<f64>,
    /// ... or closer than this many points, when that is more [default: 1.5].
    #[arg(long, value_name = "N")]
    pub group_gap_points: Option<f64>,
    /// Footprint floor in local-noise units [default: 3].
    #[arg(long, value_name = "X")]
    pub footprint_snr: Option<f64>,
    /// A row or column whose noise is at least this many noise SDs is a noise ridge
    /// [default: 3].
    #[arg(long, value_name = "X")]
    pub ridge_factor: Option<f64>,
    /// Flag cross peaks on the F1 line of one this many times taller (t1 noise) [default: 20].
    #[arg(long, value_name = "X")]
    pub ridge_ratio: Option<f64>,
    /// Keep maxima that stand out from the noise but not from their ridge (flagged `ridge`).
    #[arg(long)]
    pub keep_ridge_peaks: bool,
    /// Homonuclear spectra: cross peaks this close to the diagonal are diagonal peaks, Hz
    /// [default: 15].
    #[arg(long, value_name = "HZ")]
    pub diagonal_tolerance_hz: Option<f64>,
    /// Homonuclear spectra: largest distance of a symmetric partner from the mirror image, Hz
    /// [default: 25].
    #[arg(long, value_name = "HZ")]
    pub symmetry_tolerance_hz: Option<f64>,
    /// Also list every maximum of every cross peak (`maxima`).
    #[arg(long)]
    pub list_maxima: bool,
    /// Refuse to read more points than this (default 268435456, 2 GiB of values); narrow the
    /// read with `--range-ppm` instead where you can.
    #[arg(long, value_name = "N")]
    pub max_points: Option<usize>,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum FromArg {
    Auto,
    Fid,
    Processed,
}

fn parse_range(s: &str, what: &str) -> Result<(f64, f64)> {
    let err = || Error::Usage(format!("{what} {s:?}: expected A:B (two numbers in ppm)"));
    let (a, b) = s.split_once(':').ok_or_else(err)?;
    let a: f64 = a.trim().parse().map_err(|_| err())?;
    let b: f64 = b.trim().parse().map_err(|_| err())?;
    if !(a.is_finite() && b.is_finite()) {
        return Err(err());
    }
    Ok((a, b))
}

/// `f1=A:B` → ("f1", (A, B)); `A:B` → (None, (A, B)).
fn parse_dim_range(s: &str, what: &str) -> Result<(Option<String>, (f64, f64))> {
    match s.split_once('=') {
        Some((d, r)) => Ok((
            Some(d.trim().to_ascii_lowercase()),
            parse_range(r.trim(), what)?,
        )),
        None => Ok((None, parse_range(s, what)?)),
    }
}

/// The `openreadout_nmr_peaks` query these flags describe: the CLI and the MCP tool share one
/// parser and one set of defaults.
pub fn query(a: &NmrPeaksArgs) -> Result<NmrQuery> {
    use openreadout_signal::api::{IntegrateArg, RangeArg};
    let mut bare: Option<(f64, f64)> = None;
    let mut dims: BTreeMap<String, (f64, f64)> = BTreeMap::new();
    for r in &a.range_ppm {
        match parse_dim_range(r, "--range-ppm")? {
            (None, p) if bare.is_none() => bare = Some(p),
            (None, _) => {
                return Err(Error::Usage(
                    "--range-ppm A:B is given twice; 2D ranges are f1=A:B and f2=C:D".into(),
                ));
            }
            (Some(d), p) => {
                if dims.insert(d.clone(), p).is_some() {
                    return Err(Error::Usage(format!("--range-ppm {d}= is given twice")));
                }
            }
        }
    }
    let range_ppm = match (bare, dims.is_empty()) {
        (None, true) => None,
        (Some(p), true) => Some(RangeArg::Pair(p)),
        (None, false) => Some(RangeArg::PerDimension(dims)),
        (Some(p), false) => {
            // a bare range next to named ones is the direct dimension (F2 of a 2D spectrum)
            if dims.contains_key("f2") {
                return Err(Error::Usage(
                    "--range-ppm: a bare A:B is F2, and f2= is given too".into(),
                ));
            }
            dims.insert("f2".into(), p);
            Some(RangeArg::PerDimension(dims))
        }
    };
    let mut integrate = Vec::new();
    for g in &a.integrate {
        let parts: Vec<&str> = g.split(',').map(str::trim).collect();
        if parts.len() == 1 && !parts[0].contains('=') {
            integrate.push(IntegrateArg::Pair(parse_range(parts[0], "--integrate")?));
            continue;
        }
        let mut named = BTreeMap::new();
        let mut ordered = Vec::new();
        for p in parts {
            match parse_dim_range(p, "--integrate")? {
                (Some(d), r) => {
                    named.insert(d, r);
                }
                (None, r) => ordered.push(r),
            }
        }
        integrate.push(match (named.is_empty(), ordered.is_empty()) {
            (false, true) => IntegrateArg::PerDimension(named),
            (true, false) => IntegrateArg::Box(ordered),
            _ => {
                return Err(Error::Usage(format!(
                    "--integrate {g:?}: name every range (f1=A:B,f2=C:D) or none (A:B,C:D, F1 first)"
                )));
            }
        });
    }
    let integral_reference = match &a.integral_reference {
        None => None,
        Some(s) => {
            let err = || Error::Usage(format!("--integral-reference {s:?}: expected I=V"));
            let (i, v) = s.split_once('=').ok_or_else(err)?;
            let i: usize = i.trim().parse().map_err(|_| err())?;
            let v: f64 = v.trim().parse().map_err(|_| err())?;
            Some((i, v))
        }
    };
    let c = &a.cross;
    let mut q = NmrQuery {
        from: Some(
            match a.from {
                FromArg::Auto => "auto",
                FromArg::Fid => "fid",
                FromArg::Processed => "processed",
            }
            .into(),
        ),
        trace: a.trace,
        sweep: a.sweep,
        slice: a.slice.clone(),
        min_snr: Some(a.min_snr),
        min_prominence: Some(a.min_prominence),
        min_height_fraction: Some(a.min_height_fraction),
        negative: if a.negative {
            Some(true)
        } else if a.no_negative {
            Some(false)
        } else {
            None
        },
        range_ppm,
        max_peaks: Some(a.max_peaks),
        integrate,
        auto_integrate: a.auto_integrate,
        integral_reference,
        min_local_snr: c.min_local_snr,
        min_footprint_points: c.min_footprint_points,
        group_gap_hz: c.group_gap_hz,
        group_gap_points: c.group_gap_points,
        footprint_snr: c.footprint_snr,
        ridge_factor: c.ridge_factor,
        ridge_ratio: c.ridge_ratio,
        keep_ridge_peaks: c.keep_ridge_peaks,
        diagonal_tolerance_hz: c.diagonal_tolerance_hz,
        symmetry_tolerance_hz: c.symmetry_tolerance_hz,
        list_maxima: c.list_maxima,
        max_points: c.max_points,
        ..NmrQuery::default()
    };
    ProcessFlags {
        phase: &a.phase,
        lb: &a.lb,
        gb: &a.gb,
        size: &a.size,
        baseline: &a.baseline,
        no_group_delay: a.no_group_delay,
        nus: a.nus.clone(),
    }
    .apply(&mut q, "--")?;
    Ok(q)
}

/// Run `nmr-peaks` on one file.
pub fn nmr_peaks(reg: &Registry, a: &NmrPeaksArgs) -> Result<NmrPeaksReport> {
    let q = query(a)?;
    // flags are checked before the file is opened
    q.peaks_request()?;
    let (_, mut ds) = reg.open(&a.file)?;
    let info = ds.info()?;
    let report = q.run(ds.as_mut(), &info)?;
    if let Some(o) = &a.output {
        write_rows(&a.file, o, &report, !q.integrate.is_empty(), a.overwrite)?;
    }
    if let Some(p) = &a.plot {
        let figure = openreadout_preview::nmr_peaks_figure(ds.as_mut(), &info, &report, a.width)?;
        let bytes = openreadout_preview::encode_figure(
            &figure,
            openreadout_preview::Encoding::from_path(p),
            openreadout_preview::DEFAULT_JPEG_QUALITY,
        )?;
        openreadout_preview::write_verified(&a.file, p, &bytes, a.overwrite)?;
    }
    Ok(report)
}

/// Write the peak (or integral) rows of `r` to `o`, read back and verified.
fn write_rows(
    input: &Path,
    o: &Path,
    r: &NmrPeaksReport,
    integrals: bool,
    overwrite: bool,
) -> Result<()> {
    use openreadout_batch::table::{Column, ColumnType, Role, Table, Value};
    let (peaks, regions) = r.rows();
    let rows = if integrals { regions } else { peaks };
    let mut names: Vec<String> = Vec::new();
    for m in &rows {
        for k in m.keys() {
            if !names.contains(k) {
                names.push(k.clone());
            }
        }
    }
    let cell = |v: Option<&serde_json::Value>| match v {
        None | Some(serde_json::Value::Null) => Value::Null,
        Some(serde_json::Value::Bool(b)) => Value::Bool(*b),
        Some(serde_json::Value::Number(n)) => n
            .as_i64()
            .map_or_else(|| Value::Float(n.as_f64().unwrap_or(f64::NAN)), Value::Int),
        Some(serde_json::Value::String(s)) => Value::Text(s.clone()),
        Some(other) => Value::Text(other.to_string()),
    };
    let table_rows: Vec<Vec<Value>> = rows
        .iter()
        .map(|m| names.iter().map(|k| cell(m.get(k))).collect())
        .collect();
    let columns = names
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let kind = table_rows
                .iter()
                .map(|r| &r[i])
                .find(|v| !v.is_null())
                .map_or(ColumnType::Float, |v| match v {
                    Value::Int(_) => ColumnType::Integer,
                    Value::Bool(_) => ColumnType::Boolean,
                    Value::Text(_) => ColumnType::String,
                    _ => ColumnType::Float,
                });
            // a column mixing whole and real numbers is real
            let kind = if kind == ColumnType::Integer
                && table_rows.iter().any(|r| matches!(r[i], Value::Float(_)))
            {
                ColumnType::Float
            } else {
                kind
            };
            Column {
                name: n.clone(),
                kind,
                unit: n
                    .ends_with("_ppm")
                    .then(|| "ppm".to_string())
                    .or_else(|| n.ends_with("_hz").then(|| "Hz".to_string())),
                role: if n == "peak" || n == "region" {
                    Role::Key
                } else {
                    Role::Value
                },
                description: None,
            }
        })
        .collect();
    let t = Table {
        columns,
        rows: table_rows,
    };
    if openreadout_batch::write::TableFormat::from_path(o)
        .is_none_or(|f| matches!(f, openreadout_batch::write::TableFormat::Json))
    {
        return Err(Error::Usage(format!(
            "-o {}: use a .csv, .tsv, .jsonl or .parquet file name",
            o.display()
        )));
    }
    let w = openreadout_batch::write::save(
        &t,
        o,
        None,
        &openreadout_batch::write::TableMeta {
            measure: "nmr-peaks".into(),
            grain: vec![if integrals { "region" } else { "peak" }.into()],
        },
        overwrite,
        &[input],
    )?;
    if !crate::ui::quiet() {
        eprintln!("wrote {} rows to {}", w.rows, w.path);
    }
    Ok(())
}

fn render(r: &NmrPeaksReport) -> String {
    match r {
        NmrPeaksReport::Spectrum(r) => render_1d(r),
        NmrPeaksReport::CrossPeaks(r) => render_2d(r),
    }
}

fn render_1d(r: &NmrReport) -> String {
    let mut s = format!(
        "{} spectrum ({}{}{}), {} points, {:.4} to {:.4} ppm; noise SD {:.4e}; {} peaks",
        r.nucleus.as_deref().unwrap_or("NMR"),
        r.source,
        r.processing
            .as_ref()
            .map(|p| format!(
                ": phase {} {:.1}/{:.1} deg, size {}",
                p.phase_mode, p.phase0_deg, p.phase1_deg, p.size
            ))
            .unwrap_or_default(),
        r.slice
            .as_ref()
            .map(|sl| format!(", slice {sl}"))
            .unwrap_or_default(),
        r.axis.size,
        r.axis.first_ppm,
        r.axis.last_ppm(),
        r.noise_sd,
        r.peak_count
    );
    for p in r.peaks.iter().take(50) {
        s.push_str(&format!(
            "\n  {:>10.4} ppm  height {:>12.4e}  S/N {:>9}  width {}",
            p.ppm,
            p.height,
            p.snr.map_or_else(|| "-".into(), |v| format!("{v:.1}")),
            p.width_hz
                .map_or_else(|| "-".into(), |w| format!("{w:.2} Hz"))
        ));
    }
    if r.peaks.len() > 50 {
        s.push_str(&format!(
            "\n  … {} more (use --json or -o)",
            r.peaks.len() - 50
        ));
    }
    for (i, g) in r.integrals.iter().enumerate() {
        s.push_str(&format!(
            "\n  integral {i}: {:.4}..{:.4} ppm = {:.6e} (normalized {})",
            g.from_ppm,
            g.to_ppm,
            g.value,
            g.normalized
                .map_or_else(|| "-".into(), |v| format!("{v:.4}"))
        ));
    }
    s
}

/// The cross-peak table: one line per cross peak, a column per dimension.
fn render_2d(r: &Nmr2dReport) -> String {
    let dims = r.shape.len();
    let label = |d: usize| -> String {
        let nu = r.nuclei.get(d).cloned().flatten();
        let unit = r
            .dimensions
            .get(d)
            .filter(|a| !a.is_shift())
            .map_or("ppm".to_string(), |a| a.unit.clone());
        match nu {
            Some(n) => format!("F{} {n} ({unit})", d + 1),
            None => format!("F{} ({unit})", d + 1),
        }
    };
    let shape = r
        .shape
        .iter()
        .map(usize::to_string)
        .collect::<Vec<_>>()
        .join(" × ");
    let mut s = format!(
        "{}D spectrum {} ({}{}), {shape} points; noise SD {:.4e}; {} cross peaks",
        dims,
        r.trace_name.as_deref().unwrap_or("-"),
        r.source,
        r.pulse_program
            .as_ref()
            .map(|p| format!(", {p}"))
            .unwrap_or_default(),
        r.noise_sd,
        r.peak_count
    );
    if r.ridge_suppressed > 0 || r.small_footprint_suppressed > 0 {
        s.push_str(&format!(
            " (left out: {} maxima on noise ridges, {} isolated spikes)",
            r.ridge_suppressed, r.small_footprint_suppressed
        ));
    }
    s.push_str("\n  ");
    let heads: Vec<String> = (0..dims).map(label).collect();
    for h in &heads {
        s.push_str(&format!("{h:>16}  "));
    }
    s.push_str(&format!(
        "{:>12}  {:>8}  {:>12}  flags",
        "height", "S/N", "volume"
    ));
    let rows = r.cross_peak_rows();
    for (p, row) in r.peaks.iter().zip(&rows).take(50) {
        s.push_str("\n  ");
        for (d, v) in p.ppm.iter().enumerate() {
            let shown = r.dimensions.get(d).map_or(*v, |a| a.show(*v));
            s.push_str(&format!("{shown:>16.4}  "));
        }
        s.push_str(&format!(
            "{:>12.4e}  {:>8}  {:>12.4e}  {}",
            p.height,
            p.snr.map_or_else(|| "-".into(), |v| format!("{v:.1}")),
            p.volume,
            row.get("flags").and_then(|v| v.as_str()).unwrap_or("")
        ));
    }
    if r.peaks.len() > 50 {
        s.push_str(&format!(
            "\n  … {} more (use --json or -o peaks.csv)",
            r.peaks.len() - 50
        ));
    }
    for (i, g) in r.integrals.iter().enumerate() {
        let b = g
            .ranges_ppm
            .iter()
            .enumerate()
            .map(|(d, (a, z))| format!("F{} {a:.3}..{z:.3}", d + 1))
            .collect::<Vec<_>>()
            .join(", ");
        s.push_str(&format!(
            "\n  integral {i}: {b} = {:.6e} (normalized {})",
            g.value,
            g.normalized
                .map_or_else(|| "-".into(), |v| format!("{v:.4}"))
        ));
    }
    for n in r.notes.iter().take(3) {
        s.push_str(&format!("\n  note: {n}"));
    }
    s
}

pub fn run_nmr_peaks(reg: &Registry, a: &NmrPeaksArgs) -> i32 {
    match nmr_peaks(reg, a) {
        Ok(r) => emit(a.json, &r, render),
        Err(e) => fail(a.json, &e),
    }
}

/// `--slice` on `trace`, `export`, `preview` and `plot`: read a slice or projection of a
/// stored 2D spectrum as a 1-D trace.
#[derive(Debug, Clone, Default, clap::Args)]
pub struct SliceArgs {
    /// NMR 2D spectra: read a slice as a 1-D spectrum: `row:PPM` (the F2 trace at an F1
    /// shift), `column:PPM` (the F1 trace at an F2 shift), or a projection `skyline:f1|f2`,
    /// `min:f1|f2` (negative peaks), `sum:f1|f2`. `--trace` picks the 2D spectrum.
    #[arg(long, value_name = "SPEC")]
    pub slice: Option<String>,
}

impl SliceArgs {
    /// Wrap `ds` when `--slice` was given; the trace to read is then the slice's.
    pub fn wrap(
        &self,
        ds: Box<dyn Dataset>,
        trace: Option<u32>,
    ) -> Result<(Box<dyn Dataset>, Option<u32>)> {
        let Some(s) = &self.slice else {
            return Ok((ds, trace));
        };
        let spec = SliceSpec::parse(s)?;
        let w = NmrMatrixDataset::new(ds, trace, &[spec])?;
        let index = w.slice_trace(0);
        Ok((Box::new(w), index))
    }
}
