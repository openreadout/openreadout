//! `analyze qpcr`: named per-well results of a real-time PCR file (RDML, Applied Biosystems `.eds`,
//! Rotor-Gene `.rex`, LightCycler 480 `.ixo`), and the analyses: our own threshold Cq against the vendor's, ΔΔCq
//! relative quantification, standard curves (`openreadout_qpcr::qpcr_report`).

use std::path::{Path, PathBuf};

use openreadout_core::{Error, Registry, Result};
use openreadout_qpcr::{QpcrDataset, QpcrReport, QpcrReportRequest, qpcr_report};

use crate::output::{emit, fail};

/// Arguments of `qpcr`.
#[derive(Debug, clap::Args)]
pub struct QpcrArgs {
    /// A qPCR file: RDML (`.rdml`, LightCycler 96 `.lc96p`), Applied Biosystems `.eds`,
    /// Rotor-Gene `.rex`, LightCycler 480 `.ixo`.
    #[arg(value_name = "FILE")]
    pub file: PathBuf,
    /// Only this well (`A1`, `B3`).
    #[arg(long)]
    pub well: Option<String>,
    /// Only this target (gene, assay, detector).
    #[arg(long)]
    pub target: Option<String>,
    /// Only this sample.
    #[arg(long)]
    pub sample: Option<String>,
    /// Only this run (RDML files with several plates).
    #[arg(long)]
    pub run: Option<String>,
    /// Also compute our own threshold Cq for every curve and compare it with the vendor's
    /// (agreement statistics under `cq_comparison`).
    #[arg(long)]
    pub compute_cq: bool,
    /// With `--compute-cq`: threshold in baseline-corrected fluorescence units (default: the
    /// file's own where it records the threshold in force, else 10 SD of the baseline).
    #[arg(long, requires = "compute_cq")]
    pub threshold: Option<f64>,
    /// With `--compute-cq`: first cycle of the baseline window (default: the file's, else 3).
    #[arg(long, requires_all = ["compute_cq", "baseline_end"], value_name = "CYCLE")]
    pub baseline_start: Option<u32>,
    /// With `--compute-cq`: last cycle of the baseline window (default: the file's, else 15).
    #[arg(long, requires_all = ["compute_cq", "baseline_start"], value_name = "CYCLE")]
    pub baseline_end: Option<u32>,
    /// ΔΔCq relative quantification (2^-ΔΔCq) per sample and target.
    #[arg(long)]
    pub ddcq: bool,
    /// ΔΔCq reference (endogenous control) target; repeatable. Default: the file's.
    #[arg(long = "reference-target", value_name = "TARGET")]
    pub reference_targets: Vec<String>,
    /// ΔΔCq control (calibrator) sample. Default: the file's.
    #[arg(long, value_name = "SAMPLE")]
    pub control_sample: Option<String>,
    /// Fit a standard curve per target from the standard wells (slope, R², efficiency).
    #[arg(long)]
    pub standard_curve: bool,
    /// Return at most this many records.
    #[arg(long, value_name = "N")]
    pub max_records: Option<usize>,
    /// Count undetermined wells (no Cq) at this Cq in the per-target means and ΔΔCq (e.g. the
    /// cycle count). Default: they are left out and counted.
    #[arg(long, value_name = "CQ")]
    pub undetermined_cq: Option<f64>,
    #[arg(long)]
    pub json: bool,
}

fn baseline(start: Option<u32>, end: Option<u32>) -> Result<Option<(u32, u32)>> {
    match (start, end) {
        (None, None) => Ok(None),
        (Some(a), Some(b)) if a >= 1 && b > a => Ok(Some((a, b))),
        _ => Err(Error::Usage(
            "--baseline-start and --baseline-end go together, with 1 <= start < end".into(),
        )),
    }
}

/// Open a qPCR file through the registry (so `.pcrd` and non-qPCR files get their own errors).
pub fn open(reg: &Registry, file: &Path) -> Result<QpcrDataset> {
    openreadout_qpcr::open_qpcr(reg, file)
}

fn report(reg: &Registry, a: &QpcrArgs) -> Result<QpcrReport> {
    let ds = open(reg, &a.file)?;
    let mut req = QpcrReportRequest::default();
    req.well.clone_from(&a.well);
    req.target.clone_from(&a.target);
    req.sample.clone_from(&a.sample);
    req.run.clone_from(&a.run);
    req.compute_cq = a.compute_cq;
    req.threshold = a.threshold;
    req.baseline = baseline(a.baseline_start, a.baseline_end)?;
    req.relative = a.ddcq;
    req.reference_targets.clone_from(&a.reference_targets);
    req.control_sample.clone_from(&a.control_sample);
    req.standard_curve = a.standard_curve;
    req.max_records = a.max_records;
    req.undetermined_cq = a.undetermined_cq;
    qpcr_report(&ds, &req)
}

fn num(v: Option<f64>, digits: usize) -> String {
    v.map_or_else(|| "-".into(), |x| format!("{x:.digits$}"))
}

fn render(r: &QpcrReport) -> String {
    let mut s = format!(
        "{} ({}, {}){}",
        r.path,
        r.format,
        r.dialect,
        r.experiment
            .as_deref()
            .map(|e| format!(": {e}"))
            .unwrap_or_default()
    );
    if let Some(i) = &r.instrument {
        s.push_str(&format!("\ninstrument: {i}"));
    }
    if let Some(t) = r.acquisition_temperature_c {
        s.push_str(&format!(
            "\nprogram: {} cycles, reads at {t} °C",
            r.cycles.map_or_else(|| "?".into(), |c| c.to_string())
        ));
    }
    s.push_str(&format!(
        "\n{} well x target records{}\n",
        r.record_count,
        if r.truncated { " (truncated)" } else { "" }
    ));
    s.push_str("well   sample                 target            task        Cq        Tm\n");
    for x in &r.records {
        let cq = match (x.cq, x.cq_undetermined) {
            (Some(c), _) => format!("{c:.3}"),
            (None, true) => x
                .cq_stored
                .map_or_else(|| "undet.".into(), |v| format!("undet.({v})")),
            _ => "-".into(),
        };
        s.push_str(&format!(
            "{:<6} {:<22} {:<17} {:<10} {:>7} {:>9}\n",
            x.well,
            x.sample.as_deref().unwrap_or("-"),
            x.target.as_deref().unwrap_or("-"),
            x.task.as_deref().unwrap_or("-"),
            cq,
            x.tm.first()
                .map_or_else(|| "-".into(), |t| format!("{t:.2}"))
        ));
    }
    let c = &r.cq_counts;
    s.push_str(&format!(
        "\nCq: {} determined, {} undetermined, {} no result ({} excluded)\n",
        c.determined, c.undetermined, c.no_result, c.excluded
    ));
    if r.targets.len() > 1 || c.undetermined > 0 {
        s.push_str("target            records  Cq  undet.  mean Cq     SD\n");
        for t in &r.targets {
            s.push_str(&format!(
                "{:<17} {:>7} {:>3} {:>7} {:>8} {:>6}\n",
                t.target,
                t.counts.records,
                t.counts.determined,
                t.counts.undetermined,
                num(t.cq_mean, 3),
                num(t.cq_sd, 3)
            ));
        }
    }
    if let Some(c) = &r.cq_comparison {
        s.push_str(&format!(
            "\nour Cq vs vendor: {} curves, {} both with Cq, {} both undetermined, {} only vendor, {} only ours; mean diff {}, median |diff| {}, max |diff| {}, within 0.5 cycles {}, r = {}\n  {}\n",
            c.curves,
            c.both_cq,
            c.both_undetermined,
            c.only_vendor,
            c.only_ours,
            num(c.mean_difference, 3),
            num(c.median_abs_difference, 3),
            num(c.max_abs_difference, 3),
            num(c.within_0_5, 3),
            num(c.pearson_r, 5),
            c.method
        ));
    }
    if !r.relative_quantities.is_empty() {
        s.push_str(&format!(
            "\nΔΔCq (reference: {}; control: {})\nsample                 target            n   meanCq    ΔCq     ΔΔCq      RQ\n",
            r.reference_targets.join(", "),
            r.control_sample.as_deref().unwrap_or("-")
        ));
        for q in &r.relative_quantities {
            s.push_str(&format!(
                "{:<22} {:<17} {:>2} {:>8.3} {:>7.3} {:>8} {:>7}\n",
                q.sample,
                q.target,
                q.replicates,
                q.cq_mean,
                q.delta_cq,
                num(q.delta_delta_cq, 3),
                num(q.rq, 3)
            ));
        }
    }
    for c in &r.standard_curves {
        s.push_str(&format!(
            "\nstandard curve {}: slope {:.4}, intercept {:.3}, R² {:.4}, efficiency {:.1} % ({} wells, {} levels){}",
            c.target,
            c.slope,
            c.intercept,
            c.r2,
            c.efficiency_percent,
            c.points,
            c.levels,
            match (c.vendor_slope, c.vendor_efficiency_percent) {
                (Some(vs), Some(ve)) => format!("; vendor: slope {vs:.4}, efficiency {ve:.1} %"),
                _ => String::new(),
            }
        ));
    }
    for n in &r.notes {
        s.push_str(&format!("\nnote: {n}"));
    }
    s
}

pub fn run(reg: &Registry, a: &QpcrArgs) -> i32 {
    match report(reg, a) {
        Ok(r) => emit(a.json, &r, render),
        Err(e) => fail(a.json, &e),
    }
}
