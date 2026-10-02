//! `analyze nmr-peaks`, and `--process` for `trace`, `export` and `preview` (NMR FIDs as spectra).

use std::path::PathBuf;

use openreadout_core::{Dataset, Error, Registry, Result};
use openreadout_signal::nmr::{
    Apodization, BaselineMode, NmrReport, NmrRequest, PeakOptions, PhaseMode, ProcessOptions,
    ProcessedNmrDataset, SpectrumSource,
};

use crate::output::{emit, fail};

/// FID processing choices shared by `nmr-peaks` (unprefixed) and `--process` (prefixed).
#[derive(Debug, Clone, Default)]
pub struct ProcessChoice {
    pub phase: Option<String>,
    pub lb: Option<f64>,
    pub gb: Option<f64>,
    pub size: Option<usize>,
    pub baseline: Option<String>,
    pub no_group_delay: bool,
}

/// `--process` and its options on `trace`, `export` and `preview`.
#[derive(Debug, Clone, Default, clap::Args)]
pub struct ProcessArgs {
    /// NMR: read FID traces as spectra processed by OpenReadout (group-delay removal,
    /// apodization, zero filling, FT, phasing, baseline, ppm axis). Stored spectra are unchanged.
    #[arg(long)]
    pub process: bool,
    /// With `--process`: `default` (stored phases, else automatic), `stored`, `auto`,
    /// `magnitude`, `none`, or `P0,P1` in degrees.
    #[arg(long, value_name = "MODE", requires = "process")]
    pub process_phase: Option<String>,
    /// With `--process`: exponential line broadening, Hz (default: stored, else 0.3 Hz for 1H
    /// and 1 Hz otherwise).
    #[arg(
        long,
        value_name = "HZ",
        requires = "process",
        allow_negative_numbers = true
    )]
    pub process_lb: Option<f64>,
    /// With `--process`: transform size in complex points (power of two).
    #[arg(long, value_name = "N", requires = "process")]
    pub process_size: Option<usize>,
    /// With `--process`: baseline correction `default` (polynomial order 1), `none`, or
    /// `poly:N`.
    #[arg(long, value_name = "MODE", requires = "process")]
    pub process_baseline: Option<String>,
}

impl ProcessArgs {
    fn choice(&self) -> ProcessChoice {
        ProcessChoice {
            phase: self.process_phase.clone(),
            lb: self.process_lb,
            size: self.process_size,
            baseline: self.process_baseline.clone(),
            ..ProcessChoice::default()
        }
    }

    /// Wrap `ds` when `--process` was given.
    pub fn wrap(&self, ds: Box<dyn Dataset>) -> Result<Box<dyn Dataset>> {
        if !self.process {
            return Ok(ds);
        }
        let opts = process_options(&self.choice())?;
        Ok(Box::new(ProcessedNmrDataset::new(ds, opts)?))
    }
}

fn parse_phase(s: &str) -> Result<PhaseMode> {
    Ok(match s.trim().to_ascii_lowercase().as_str() {
        "default" => PhaseMode::Default,
        "stored" => PhaseMode::Stored,
        "auto" | "automatic" => PhaseMode::Auto,
        "magnitude" | "mc" => PhaseMode::Magnitude,
        "none" => PhaseMode::None,
        other => {
            let parts: Vec<&str> = other.split(',').map(str::trim).collect();
            let nums: Option<Vec<f64>> = parts.iter().map(|p| p.parse::<f64>().ok()).collect();
            match nums.as_deref() {
                Some([p0]) if p0.is_finite() => PhaseMode::Manual {
                    phase0_deg: *p0,
                    phase1_deg: 0.0,
                },
                Some([p0, p1]) if p0.is_finite() && p1.is_finite() => PhaseMode::Manual {
                    phase0_deg: *p0,
                    phase1_deg: *p1,
                },
                _ => {
                    return Err(Error::Usage(format!(
                        "--phase {s:?}: expected default, stored, auto, magnitude, none or P0,P1 (degrees)"
                    )));
                }
            }
        }
    })
}

fn parse_baseline(s: &str) -> Result<BaselineMode> {
    let t = s.trim().to_ascii_lowercase();
    Ok(match t.as_str() {
        "default" => BaselineMode::Default,
        "none" | "off" => BaselineMode::None,
        _ => match t
            .strip_prefix("poly:")
            .or_else(|| t.strip_prefix("polynomial:"))
        {
            Some(n) => match n.parse::<u32>() {
                Ok(order) if order <= 8 => BaselineMode::Polynomial { order },
                _ => {
                    return Err(Error::Usage(format!(
                        "--baseline {s:?}: the polynomial order must be 0 to 8"
                    )));
                }
            },
            None => {
                return Err(Error::Usage(format!(
                    "--baseline {s:?}: expected default, none or poly:N"
                )));
            }
        },
    })
}

/// Turn CLI choices into [`ProcessOptions`].
pub fn process_options(c: &ProcessChoice) -> Result<ProcessOptions> {
    let mut o = ProcessOptions::default();
    if let Some(p) = &c.phase {
        o.phase = parse_phase(p)?;
    }
    if let Some(b) = &c.baseline {
        o.baseline = parse_baseline(b)?;
    }
    if let Some(n) = c.size {
        if n < 2 {
            return Err(Error::Usage("--size must be at least 2".into()));
        }
        o.size = Some(n);
    }
    for (name, v) in [("--lb", c.lb), ("--gb", c.gb)] {
        if v.is_some_and(|v| !v.is_finite()) {
            return Err(Error::Usage(format!(
                "{name} must be a finite number of Hz"
            )));
        }
    }
    o.apodization = match (c.lb, c.gb) {
        (_, Some(gb)) => Some(Apodization::Gaussian {
            line_broadening_hz: gb,
        }),
        (Some(0.0), None) => Some(Apodization::None),
        (Some(lb), None) => Some(Apodization::Exponential {
            line_broadening_hz: lb,
        }),
        (None, None) => None,
    };
    o.ignore_group_delay = c.no_group_delay;
    Ok(o)
}

/// `nmr-peaks`.
#[derive(Debug, Clone, clap::Args)]
pub struct NmrPeaksArgs {
    /// An NMR data set: a Bruker experiment directory, a Varian `.fid` directory, a JEOL `.jdf`
    /// or a JCAMP-DX NMR spectrum.
    pub file: PathBuf,
    /// Spectrum source: `auto` (the vendor's processed spectrum when there is one, else the
    /// FID processed here), `fid` (always process the FID), `processed` (stored spectrum only).
    #[arg(long = "from", value_enum, default_value = "auto")]
    pub from: FromArg,
    /// Trace index (see `info`); default: chosen by `--from`.
    #[arg(long)]
    pub trace: Option<u32>,
    /// Row of a `ser`/arrayed FID to process (each row is processed as a 1-D FID).
    #[arg(long, default_value_t = 0)]
    pub sweep: u32,
    /// Phase: `default` (stored phases, else automatic), `stored`, `auto`, `magnitude`, `none`,
    /// or `P0,P1` in degrees (correction `e^{-i(P0 + P1*k/N)}`, k from the high-ppm edge).
    #[arg(long, value_name = "MODE")]
    pub phase: Option<String>,
    /// Exponential line broadening, Hz (0 = no window; default: stored, else 0.3 Hz for 1H and
    /// 1 Hz otherwise).
    #[arg(long, value_name = "HZ", allow_negative_numbers = true)]
    pub lb: Option<f64>,
    /// Gaussian window instead: Gaussian line width, Hz.
    #[arg(long, value_name = "HZ", conflicts_with = "lb")]
    pub gb: Option<f64>,
    /// Transform size in complex points (power of two; default: stored, else 2 × the FID).
    #[arg(long, value_name = "N")]
    pub size: Option<usize>,
    /// Baseline correction: `default` (polynomial order 1 through automatically found baseline
    /// blocks), `none`, `poly:N`. Stored spectra are used as stored unless `--baseline` is given.
    #[arg(long, value_name = "MODE")]
    pub baseline: Option<String>,
    /// Do not remove the digital-filter group delay.
    #[arg(long)]
    pub no_group_delay: bool,
    /// Minimum peak height as a multiple of the noise SD.
    #[arg(long, value_name = "X", default_value_t = 10.0)]
    pub min_snr: f64,
    /// Minimum prominence (height above the higher neighbouring minimum) in noise SDs.
    #[arg(long, value_name = "X", default_value_t = 5.0)]
    pub min_prominence: f64,
    /// Minimum height as a fraction of the tallest point (e.g. 0.01 = 1 %).
    #[arg(long, value_name = "F", default_value_t = 0.0)]
    pub min_height_fraction: f64,
    /// Also report negative peaks (DEPT, APT).
    #[arg(long)]
    pub negative: bool,
    /// Only pick peaks between two shifts, `A:B` in ppm.
    #[arg(long, value_name = "A:B", allow_hyphen_values = true)]
    pub range: Option<String>,
    /// Keep at most this many peaks (the tallest).
    #[arg(long, value_name = "N", default_value_t = 1000)]
    pub max_peaks: usize,
    /// Integrate a region `A:B` (ppm); repeatable.
    #[arg(long = "integrate", value_name = "A:B", allow_hyphen_values = true)]
    pub integrate: Vec<String>,
    /// Normalise integrals so region I (0-based) equals V: `I=V` (default `0=1`).
    #[arg(long, value_name = "I=V")]
    pub integral_reference: Option<String>,
    #[arg(long)]
    pub json: bool,
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

fn request(a: &NmrPeaksArgs) -> Result<NmrRequest> {
    let process = process_options(&ProcessChoice {
        phase: a.phase.clone(),
        lb: a.lb,
        gb: a.gb,
        size: a.size,
        baseline: a.baseline.clone(),
        no_group_delay: a.no_group_delay,
    })?;
    for (name, v) in [
        ("--min-snr", a.min_snr),
        ("--min-prominence", a.min_prominence),
        ("--min-height-fraction", a.min_height_fraction),
    ] {
        if !(v.is_finite() && v >= 0.0) {
            return Err(Error::Usage(format!("{name} must be a number >= 0")));
        }
    }
    let integrals = a
        .integrate
        .iter()
        .map(|s| parse_range(s, "--integrate"))
        .collect::<Result<Vec<_>>>()?;
    let integral_reference = match &a.integral_reference {
        None => None,
        Some(s) => {
            let err = || Error::Usage(format!("--integral-reference {s:?}: expected I=V"));
            let (i, v) = s.split_once('=').ok_or_else(err)?;
            let i: usize = i.trim().parse().map_err(|_| err())?;
            let v: f64 = v.trim().parse().map_err(|_| err())?;
            if i >= integrals.len() || !v.is_finite() {
                return Err(Error::Usage(format!(
                    "--integral-reference {s:?}: region {i} does not exist or the value is not finite"
                )));
            }
            Some((i, v))
        }
    };
    Ok(NmrRequest {
        trace: a.trace,
        sweep: a.sweep,
        source: match a.from {
            FromArg::Auto => SpectrumSource::Auto,
            FromArg::Fid => SpectrumSource::Fid,
            FromArg::Processed => SpectrumSource::Processed,
        },
        process,
        baseline_on_stored: a.baseline.is_some(),
        peaks: PeakOptions {
            min_snr: a.min_snr,
            min_height_fraction: a.min_height_fraction,
            min_prominence_snr: a.min_prominence,
            include_negative: a.negative,
            range_ppm: a
                .range
                .as_deref()
                .map(|r| parse_range(r, "--range"))
                .transpose()?,
            max_peaks: a.max_peaks,
        },
        integrals,
        integral_reference,
    })
}

/// Run `nmr-peaks` on one file.
pub fn nmr_peaks(reg: &Registry, a: &NmrPeaksArgs) -> Result<NmrReport> {
    let req = request(a)?;
    let (_, mut ds) = reg.open(&a.file)?;
    let info = ds.info()?;
    openreadout_signal::nmr::analyze(ds.as_mut(), &info, &req)
}

fn render(r: &NmrReport) -> String {
    let mut s = format!(
        "{} spectrum ({}{}), {} points, {:.4} to {:.4} ppm; noise SD {:.4e}; {} peaks",
        r.nucleus.as_deref().unwrap_or("NMR"),
        r.source,
        r.processing
            .as_ref()
            .map(|p| format!(
                ": phase {} {:.1}/{:.1} deg, size {}",
                p.phase_mode, p.phase0_deg, p.phase1_deg, p.size
            ))
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
        s.push_str(&format!("\n  … {} more (use --json)", r.peaks.len() - 50));
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

pub fn run_nmr_peaks(reg: &Registry, a: &NmrPeaksArgs) -> i32 {
    match nmr_peaks(reg, a) {
        Ok(r) => emit(a.json, &r, render),
        Err(e) => fail(a.json, &e),
    }
}
