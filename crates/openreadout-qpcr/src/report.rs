//! `openreadout analyze qpcr`: named per-well records, and the analyses — our own threshold Cq
//! compared with the vendor's, ΔΔCq relative quantification, standard curves.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use openreadout_core::{Error, Result};

use crate::analysis::{Baseline, linear_fit, mean_sd, threshold_cq};
use crate::dataset::QpcrDataset;
use crate::model::{Assay, Dialect, Reaction, Run};

/// What `qpcr_report` returns: filters, and which analyses to run.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct QpcrReportRequest {
    /// Only this well (`A1`, `B3`; case-insensitive).
    pub well: Option<String>,
    /// Only this target (exact name, case-insensitive).
    pub target: Option<String>,
    /// Only this sample (exact name, case-insensitive).
    pub sample: Option<String>,
    /// Only this run (RDML files with several plates).
    pub run: Option<String>,
    /// Compute our own threshold Cq for every curve and compare it with the vendor's.
    pub compute_cq: bool,
    /// Threshold for our Cq (baseline-corrected units); default: the file's own threshold where
    /// it records the one in force, else automatic (10 SD of the baseline).
    pub threshold: Option<f64>,
    /// Baseline window for our Cq (1-based cycles, inclusive); default: the file's, else 3-15.
    pub baseline: Option<(u32, u32)>,
    /// ΔΔCq: reference (endogenous control) targets; default: the file's.
    pub reference_targets: Vec<String>,
    /// ΔΔCq: control (calibrator) sample; default: the file's.
    pub control_sample: Option<String>,
    /// Run ΔΔCq relative quantification.
    pub relative: bool,
    /// Fit a standard curve per target from the standard wells.
    pub standard_curve: bool,
    /// At most this many records (default: all).
    pub max_records: Option<usize>,
    /// Cq to use for undetermined results in the aggregates (`targets[]` means, ΔΔCq), e.g.
    /// the cycle count. Default `None`: undetermined results are left out and counted.
    pub undetermined_cq: Option<f64>,
}

/// `cq_status` values: the vendor gave a Cq.
pub const CQ_DETERMINED: &str = "determined";
/// `cq_status` values: the vendor (or our analysis) found no amplification ("Undetermined",
/// RDML `-1`, `NaN`, or an SDS/7500 `Ct` equal to the cycle count).
pub const CQ_UNDETERMINED: &str = "undetermined";
/// `cq_status` values: the file holds no result for this well × target (not analysed, setup
/// only).
pub const CQ_NO_RESULT: &str = "no result";

/// One well × target, with names resolved.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct QpcrAssayRecord {
    /// Run (plate) name.
    pub run: String,
    /// Well name (`A1`).
    pub well: String,
    /// Plate row, 1-based.
    pub row: u32,
    /// Plate column, 1-based.
    pub col: u32,
    /// Sample name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample: Option<String>,
    /// Target (assay, detector, gene).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Reporter dye.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dye: Option<String>,
    /// Role of the well for this target: `unknown`, `standard`, `ntc`, `nac`, `ntp`, `nrt`,
    /// `positive`, ...
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    /// Given quantity (standards).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quantity: Option<f64>,
    /// Cq (Ct) as the vendor software or RDML file reports it.
    pub cq: Option<f64>,
    /// True when the file says there is no Cq (Undetermined, `-1`, `NaN`, or an SDS/7500 `Ct`
    /// equal to the cycle count).
    pub cq_undetermined: bool,
    /// `determined` (a Cq), `undetermined` (no amplification: `cq` is null) or `no result`.
    pub cq_status: String,
    /// The number the file stores for an undetermined result (SDS/7500 `.eds`: the cycle
    /// count, e.g. 40), kept for reference; never a Cq.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cq_stored: Option<f64>,
    /// Mean Cq of the replicate group (vendor).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cq_mean: Option<f64>,
    /// Standard deviation of the replicate group's Cq (vendor).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cq_sd: Option<f64>,
    /// Melting temperatures of the product, °C (vendor; several when the curve has several
    /// peaks).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tm: Vec<f64>,
    /// Threshold the Cq was called at (vendor).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threshold: Option<f64>,
    /// Whether the vendor threshold was set automatically.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_threshold: Option<bool>,
    /// First cycle of the baseline window (vendor).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline_start: Option<u32>,
    /// Last cycle of the baseline window (vendor).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline_end: Option<u32>,
    /// Amplification status (vendor).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amp_status: Option<String>,
    /// Cq confidence (vendor).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cq_confidence: Option<f64>,
    /// Quantity calculated by the vendor software from a standard curve.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub calculated_quantity: Option<f64>,
    /// Amplification efficiency of this reaction (fold per cycle), when the file has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub efficiency: Option<f64>,
    /// Why the result is excluded (omitted wells, RDML `excl`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub excluded: Option<String>,
    /// Vendor QC flags.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<String>,
    /// ΔCq to the reference target as the vendor computed it (ΔΔCt experiments).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vendor_delta_cq: Option<f64>,
    /// Relative quantity as the vendor computed it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vendor_rq: Option<f64>,
    /// Our threshold Cq (with `compute_cq`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub computed_cq: Option<f64>,
    /// Threshold our Cq used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub computed_threshold: Option<f64>,
    /// `determined` or `undetermined` for our Cq (with `compute_cq`; a curve that never crosses
    /// the threshold, or crosses it only at the last read, is undetermined).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub computed_cq_status: Option<String>,
    /// Number of cycles in the amplification curve.
    pub cycles: u32,
}

/// Our threshold Cq against the vendor's, over every curve with a vendor result.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct CqComparison {
    /// How our Cq was computed.
    pub method: String,
    /// Curves compared.
    pub curves: u64,
    /// Both give a Cq.
    pub both_cq: u64,
    /// Both say there is no Cq.
    pub both_undetermined: u64,
    /// Only the vendor gives a Cq.
    pub only_vendor: u64,
    /// Only we give a Cq.
    pub only_ours: u64,
    /// Curves where the threshold came from the file (the vendor's own).
    pub vendor_threshold_used: u64,
    /// Mean of (ours − vendor), cycles.
    pub mean_difference: Option<f64>,
    /// Mean absolute difference, cycles.
    pub mean_abs_difference: Option<f64>,
    /// Median absolute difference, cycles.
    pub median_abs_difference: Option<f64>,
    /// Largest absolute difference, cycles.
    pub max_abs_difference: Option<f64>,
    /// Fraction of `both_cq` within 0.1 cycles.
    pub within_0_1: Option<f64>,
    /// Fraction of `both_cq` within 0.5 cycles.
    pub within_0_5: Option<f64>,
    /// Pearson correlation of the two Cq lists.
    pub pearson_r: Option<f64>,
}

/// ΔΔCq relative quantification of one sample × target.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct RelativeQuantity {
    /// Sample.
    pub sample: String,
    /// Target of interest.
    pub target: String,
    /// Replicate wells averaged: those with a Cq (plus the undetermined ones when
    /// `undetermined_cq` is given).
    pub replicates: u32,
    /// Replicate wells of this sample × target without a Cq (undetermined): left out of the
    /// mean, or counted at `undetermined_cq` when the request gives one.
    pub undetermined: u32,
    /// Mean Cq of the target in this sample.
    pub cq_mean: f64,
    /// Standard deviation of those Cqs.
    pub cq_sd: Option<f64>,
    /// Mean Cq of the reference target(s) in this sample (arithmetic mean over references).
    pub reference_cq_mean: f64,
    /// ΔCq = target − reference.
    pub delta_cq: f64,
    /// SD of ΔCq (target and reference SDs combined in quadrature).
    pub delta_cq_sd: Option<f64>,
    /// ΔΔCq = ΔCq(sample) − ΔCq(control sample); `None` without a control.
    pub delta_delta_cq: Option<f64>,
    /// 2^−ΔΔCq.
    pub rq: Option<f64>,
    /// 2^−(ΔΔCq + SD).
    pub rq_min: Option<f64>,
    /// 2^−(ΔΔCq − SD).
    pub rq_max: Option<f64>,
    /// Efficiency-corrected ratio (Pfaffl) when every efficiency involved is known.
    pub efficiency_corrected_rq: Option<f64>,
    /// ΔCq the vendor software computed for these wells, when stored.
    pub vendor_delta_cq: Option<f64>,
    /// Relative quantity the vendor software computed, when stored.
    pub vendor_rq: Option<f64>,
}

/// Cq statuses of a set of records.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct CqCounts {
    /// Records (well × target).
    pub records: u64,
    /// With a Cq.
    pub determined: u64,
    /// Without a Cq because nothing amplified ("Undetermined").
    pub undetermined: u64,
    /// Without any result (not analysed, setup only).
    pub no_result: u64,
    /// Excluded or omitted in the file (counted in the other fields too).
    pub excluded: u64,
}

/// Per-target Cq summary over the records that match the filters.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct TargetCqSummary {
    /// Target.
    pub target: String,
    /// Record counts by status (every task, excluded records included in the counts).
    pub counts: CqCounts,
    /// Records averaged: not excluded, with a Cq (plus the undetermined ones when
    /// `undetermined_cq` is given).
    pub averaged: u64,
    /// Mean Cq of those records; undetermined results are left out unless `undetermined_cq`
    /// is given.
    pub cq_mean: Option<f64>,
    /// Sample standard deviation of those Cqs.
    pub cq_sd: Option<f64>,
    /// Smallest Cq.
    pub cq_min: Option<f64>,
    /// Largest Cq.
    pub cq_max: Option<f64>,
}

/// A standard curve fitted to the standard wells of one target.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct StandardCurveFit {
    /// Target.
    pub target: String,
    /// Standard wells used.
    pub points: u32,
    /// Distinct quantities.
    pub levels: u32,
    /// Slope of Cq against log10(quantity).
    pub slope: f64,
    /// Cq at quantity 1.
    pub intercept: f64,
    /// Coefficient of determination.
    pub r2: f64,
    /// Efficiency, percent: (10^(−1/slope) − 1) × 100.
    pub efficiency_percent: f64,
    /// The vendor's slope for this target, when the file stores its standard curve.
    pub vendor_slope: Option<f64>,
    /// The vendor's efficiency, percent.
    pub vendor_efficiency_percent: Option<f64>,
    /// The vendor's r².
    pub vendor_r2: Option<f64>,
}

/// Output of `qpcr_report`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct QpcrReport {
    /// The input file.
    pub path: String,
    /// Format id.
    pub format: String,
    /// Dialect (`rdml`, `eds-sds`, `eds-7500`, `eds-json`, `rex`).
    pub dialect: String,
    /// Experiment or run name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub experiment: Option<String>,
    /// Instrument model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instrument: Option<String>,
    /// Temperature of the step that reads fluorescence during cycling (the annealing /
    /// extension temperature of a two-step PCR), °C.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acquisition_temperature_c: Option<f64>,
    /// Number of PCR cycles in the program.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cycles: Option<u32>,
    /// Reference targets used or recorded (endogenous controls).
    pub reference_targets: Vec<String>,
    /// Control (calibrator) sample used or recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub control_sample: Option<String>,
    /// Records matching the filters.
    pub record_count: u64,
    /// True when `max_records` cut the list.
    pub truncated: bool,
    /// The records.
    pub records: Vec<QpcrAssayRecord>,
    /// Cq statuses over every record that matches the filters (not cut by `max_records`).
    pub cq_counts: CqCounts,
    /// Per target: counts by Cq status and the mean Cq without the undetermined results.
    pub targets: Vec<TargetCqSummary>,
    /// Cq used for undetermined results in the aggregates, when the request gave one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub undetermined_cq: Option<f64>,
    /// Our Cq against the vendor's (with `compute_cq`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cq_comparison: Option<CqComparison>,
    /// ΔΔCq results (with `relative`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub relative_quantities: Vec<RelativeQuantity>,
    /// Standard curves (with `standard_curve`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub standard_curves: Vec<StandardCurveFit>,
    /// What to keep in mind.
    pub notes: Vec<String>,
}

fn eq(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

/// The threshold and baseline in force for our Cq on this assay: the request's, else the
/// file's when it records the one the vendor used, else automatic.
fn settings(a: &Assay, dialect: Dialect, req: &QpcrReportRequest) -> (Option<f64>, Baseline, bool) {
    let vendor_threshold = if a.threshold_used {
        a.threshold
    } else {
        match dialect {
            Dialect::EdsSds | Dialect::Eds7500 => a.estimated_threshold,
            _ => None,
        }
    };
    let vendor_baseline = match (dialect, a.baseline_start, a.baseline_end) {
        (Dialect::EdsJson, Some(s), Some(e)) => Some(Baseline::Window(s, e)),
        (Dialect::EdsSds | Dialect::Eds7500, Some(s), Some(e))
            if a.auto_baseline == Some(false) =>
        {
            Some(Baseline::Window(s, e))
        }
        // RDML bgFluor/bgFluorSlp; SDS/7500: the vendor's fitted line, recovered as Rn − ΔRn
        (Dialect::Rdml | Dialect::EdsSds | Dialect::Eds7500, _, _) => a
            .background
            .map(|b| Baseline::Line(b, a.background_slope.unwrap_or(0.0))),
        _ => None,
    };
    let baseline = match req.baseline {
        Some((s, e)) => Baseline::Window(s, e),
        None => vendor_baseline.unwrap_or(Baseline::Auto),
    };
    (
        req.threshold.or(vendor_threshold),
        baseline,
        req.threshold.is_none() && vendor_threshold.is_some(),
    )
}

fn record(run: &Run, rx: &Reaction, a: Option<&Assay>) -> QpcrAssayRecord {
    let mut r = QpcrAssayRecord {
        run: run.name.clone(),
        well: run.well_name(rx.row, rx.column),
        row: rx.row + 1,
        col: rx.column + 1,
        sample: rx.sample.clone(),
        excluded: rx.omitted.then(|| "omitted".to_string()),
        ..QpcrAssayRecord::default()
    };
    if let Some(a) = a {
        r.target.clone_from(&a.target);
        r.dye.clone_from(&a.dye);
        r.task.clone_from(&a.task);
        r.quantity = a.quantity;
        r.cq = a.cq;
        r.cq_undetermined = a.cq_undetermined;
        r.cq_stored = a.cq_stored;
        r.cq_mean = a.cq_mean;
        r.cq_sd = a.cq_sd;
        r.tm.clone_from(&a.tm);
        r.threshold = a.threshold;
        r.auto_threshold = a.auto_threshold;
        r.baseline_start = a.baseline_start;
        r.baseline_end = a.baseline_end;
        r.amp_status.clone_from(&a.amp_status);
        r.cq_confidence = a.cq_confidence;
        r.calculated_quantity = a.calculated_quantity;
        r.efficiency = a.efficiency;
        if a.excluded.is_some() {
            r.excluded.clone_from(&a.excluded);
        }
        r.flags.clone_from(&a.flags);
        r.vendor_delta_cq = a.vendor_delta_cq;
        r.vendor_rq = a.vendor_rq;
        r.cycles = a
            .amplification
            .as_ref()
            .map_or(0, |c| c.fluorescence.len() as u32);
    }
    r.cq_status = cq_status(a).to_string();
    r
}

/// `determined`, `undetermined` or `no result`.
fn cq_status(a: Option<&Assay>) -> &'static str {
    match a {
        Some(a) if a.cq.is_some() => CQ_DETERMINED,
        Some(a) if a.cq_undetermined => CQ_UNDETERMINED,
        _ => CQ_NO_RESULT,
    }
}

/// Mean, sample SD, min and max.
fn describe(v: &[f64]) -> (Option<f64>, Option<f64>, Option<f64>, Option<f64>) {
    let (m, sd, _) = mean_sd(v);
    (
        m,
        sd,
        v.iter().copied().reduce(f64::min),
        v.iter().copied().reduce(f64::max),
    )
}

fn median(v: &mut [f64]) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    let n = v.len();
    Some(if n % 2 == 1 {
        v[n / 2]
    } else {
        f64::midpoint(v[n / 2 - 1], v[n / 2])
    })
}

fn pearson(x: &[f64], y: &[f64]) -> Option<f64> {
    let n = x.len() as f64;
    if x.len() < 3 {
        return None;
    }
    let (mx, my) = (x.iter().sum::<f64>() / n, y.iter().sum::<f64>() / n);
    let sxy: f64 = x.iter().zip(y).map(|(a, b)| (a - mx) * (b - my)).sum();
    let sxx: f64 = x.iter().map(|a| (a - mx).powi(2)).sum();
    let syy: f64 = y.iter().map(|b| (b - my).powi(2)).sum();
    (sxx > 0.0 && syy > 0.0).then(|| sxy / (sxx * syy).sqrt())
}

/// Records, analyses and notes for one opened file.
pub fn qpcr_report(ds: &QpcrDataset, req: &QpcrReportRequest) -> Result<QpcrReport> {
    let data = &ds.data;
    let mut out = QpcrReport {
        path: ds.path().display().to_string(),
        format: ds.format_id().to_string(),
        dialect: data.dialect.id().to_string(),
        experiment: data
            .name
            .clone()
            .or_else(|| data.runs.first().and_then(|r| r.experiment.clone())),
        instrument: data
            .instrument
            .model
            .clone()
            .or_else(|| data.runs.first().and_then(|r| r.instrument.clone())),
        notes: data.notes.clone(),
        ..QpcrReport::default()
    };
    if let Some(p) = data.programs.first() {
        out.acquisition_temperature_c = p.acquisition_temperature();
        out.cycles = p.cycles();
        let reads = p.acquisition_temperatures();
        if reads.len() > 1 {
            out.notes.push(format!(
                "fluorescence is read at {} steps of each cycle ({} °C); acquisition_temperature_c is the first",
                reads.len(),
                reads.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
            ));
        }
    }
    // ---- records
    let mut rows: Vec<(QpcrAssayRecord, Option<&Assay>)> = Vec::new();
    for run in &data.runs {
        if req.run.as_deref().is_some_and(|r| !eq(r, &run.name)) {
            continue;
        }
        for rx in &run.reactions {
            let well = run.well_name(rx.row, rx.column);
            if req.well.as_deref().is_some_and(|w| !eq(w, &well)) {
                continue;
            }
            if req
                .sample
                .as_deref()
                .is_some_and(|s| !rx.sample.as_deref().is_some_and(|x| eq(x, s)))
            {
                continue;
            }
            if rx.assays.is_empty() {
                if req.target.is_none() {
                    rows.push((record(run, rx, None), None));
                }
                continue;
            }
            for assay in &rx.assays {
                if req
                    .target
                    .as_deref()
                    .is_some_and(|t| !assay.target.as_deref().is_some_and(|x| eq(x, t)))
                {
                    continue;
                }
                rows.push((record(run, rx, Some(assay)), Some(assay)));
            }
        }
    }
    if rows.is_empty() && (req.well.is_some() || req.target.is_some() || req.sample.is_some()) {
        let wells: Vec<String> = data
            .runs
            .iter()
            .flat_map(|r| {
                r.reactions
                    .iter()
                    .map(move |x| r.well_name(x.row, x.column))
            })
            .take(12)
            .collect();
        return Err(Error::Usage(format!(
            "no well × target matches the filters; targets: {}; samples: {}; wells include {}",
            data.targets
                .iter()
                .map(|t| t.name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            data.samples
                .iter()
                .take(20)
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            wells.join(", ")
        )));
    }
    // ---- our Cq
    if req.compute_cq {
        let mut cmp = CqComparison::default();
        let mut diffs = Vec::new();
        let (mut xs, mut ys) = (Vec::new(), Vec::new());
        for (rec, assay) in &mut rows {
            let Some(assay) = assay else { continue };
            let Some(c) = &assay.amplification else {
                continue;
            };
            let (thr, base, vendor_thr) = settings(assay, data.dialect, req);
            let Some(mut call) = threshold_cq(&c.cycles, &c.fluorescence, thr, base) else {
                continue;
            };
            // the vendor's rule: a Cq at (or past) the last cycle is no Cq
            let last = c
                .cycles
                .iter()
                .copied()
                .filter(|x| x.is_finite())
                .reduce(f64::max);
            if let (Some(q), Some(l)) = (call.cq, last)
                && q >= l
            {
                call.cq = None;
            }
            rec.computed_cq = call.cq;
            rec.computed_threshold = Some(call.threshold);
            rec.computed_cq_status = Some(
                if call.cq.is_some() {
                    CQ_DETERMINED
                } else {
                    CQ_UNDETERMINED
                }
                .to_string(),
            );
            if assay.cq.is_none() && !assay.cq_undetermined {
                continue;
            }
            cmp.curves += 1;
            if vendor_thr {
                cmp.vendor_threshold_used += 1;
            }
            match (assay.cq, call.cq) {
                (Some(v), Some(o)) => {
                    cmp.both_cq += 1;
                    diffs.push(o - v);
                    xs.push(v);
                    ys.push(o);
                }
                (None, None) => cmp.both_undetermined += 1,
                (Some(_), None) => cmp.only_vendor += 1,
                (None, Some(_)) => cmp.only_ours += 1,
            }
        }
        if !diffs.is_empty() {
            let n = diffs.len() as f64;
            let mut abs: Vec<f64> = diffs.iter().map(|x| x.abs()).collect();
            cmp.mean_difference = Some(diffs.iter().sum::<f64>() / n);
            cmp.mean_abs_difference = Some(abs.iter().sum::<f64>() / n);
            cmp.max_abs_difference = abs.iter().copied().reduce(f64::max);
            cmp.within_0_1 = Some(abs.iter().filter(|x| **x <= 0.1).count() as f64 / n);
            cmp.within_0_5 = Some(abs.iter().filter(|x| **x <= 0.5).count() as f64 / n);
            cmp.median_abs_difference = median(&mut abs);
            cmp.pearson_r = pearson(&xs, &ys);
        }
        cmp.method = format!(
            "threshold cycle on the stored fluorescence after subtracting a least-squares baseline line; threshold = {}; baseline = {}",
            match req.threshold {
                Some(t) => format!("{t} (given)"),
                None => "the file's own where it records the threshold in force (JSON-layout .eds per well, RDML quantFluor), for SDS/7500-layout .eds the vendor's automatic threshold recovered as the median of its ΔRn at its Cq per target (log-interpolated), else 10 x SD of the baseline residuals".into(),
            },
            match req.baseline {
                Some((s, e)) => format!("cycles {s}-{e} (given)"),
                None => "the file's own: the per-well window (JSON-layout .eds, refitted by us), the background line (RDML bgFluor/bgFluorSlp; SDS/7500-layout .eds: the vendor's fitted line recovered as Rn - ΔRn); else a least-squares line through cycles 3-15 ended 3 cycles before the curve's Cq; crossings interpolated in log(fluorescence)".into(),
            }
        );
        out.cq_comparison = Some(cmp);
    }
    // ---- Cq statuses and per-target means (undetermined left out unless substituted)
    let sub = req.undetermined_cq.filter(|v| v.is_finite());
    out.undetermined_cq = sub;
    let mut per_target: BTreeMap<String, (CqCounts, Vec<f64>)> = BTreeMap::new();
    for (rec, _) in &rows {
        let excluded = rec.excluded.is_some();
        let tally = |c: &mut CqCounts| {
            c.records += 1;
            match rec.cq_status.as_str() {
                CQ_DETERMINED => c.determined += 1,
                CQ_UNDETERMINED => c.undetermined += 1,
                _ => c.no_result += 1,
            }
            if excluded {
                c.excluded += 1;
            }
        };
        tally(&mut out.cq_counts);
        let Some(t) = &rec.target else { continue };
        let e = per_target.entry(t.clone()).or_default();
        tally(&mut e.0);
        if excluded {
            continue;
        }
        match (rec.cq, rec.cq_undetermined, sub) {
            (Some(c), _, _) => e.1.push(c),
            (None, true, Some(v)) => e.1.push(v),
            _ => {}
        }
    }
    out.targets = per_target
        .into_iter()
        .map(|(target, (counts, v))| {
            let (cq_mean, cq_sd, cq_min, cq_max) = describe(&v);
            TargetCqSummary {
                target,
                counts,
                averaged: v.len() as u64,
                cq_mean,
                cq_sd,
                cq_min,
                cq_max,
            }
        })
        .collect();
    if out.cq_counts.undetermined > 0 {
        out.notes.push(match sub {
            None => format!(
                "{} well x target results are undetermined (no amplification): cq is null and they are left out of targets[] means and ΔΔCq (counted in cq_counts, targets[].counts and relative_quantities[].undetermined); pass undetermined_cq (e.g. the cycle count) to count them at a fixed Cq",
                out.cq_counts.undetermined
            ),
            Some(v) => format!(
                "{} undetermined results are counted at Cq {v} in targets[] means and ΔΔCq (undetermined_cq)",
                out.cq_counts.undetermined
            ),
        });
    }
    // ---- ΔΔCq
    let refs: Vec<String> = if req.reference_targets.is_empty() {
        let mut r = data.reference_targets.clone();
        if r.is_empty() {
            r = data
                .targets
                .iter()
                .filter(|t| t.kind.as_deref() == Some("reference"))
                .map(|t| t.name.clone())
                .collect();
        }
        r
    } else {
        req.reference_targets.clone()
    };
    let control = req
        .control_sample
        .clone()
        .or_else(|| data.calibrator_sample.clone());
    out.reference_targets.clone_from(&refs);
    out.control_sample.clone_from(&control);
    if req.relative {
        out.relative_quantities = relative(ds, &refs, control.as_deref(), sub, &mut out.notes)?;
    }
    if req.standard_curve {
        out.standard_curves = standard_curves(ds, &mut out.notes);
    }
    out.record_count = rows.len() as u64;
    let limit = req.max_records.unwrap_or(usize::MAX);
    out.truncated = rows.len() > limit;
    out.records = rows.into_iter().take(limit).map(|(r, _)| r).collect();
    Ok(out)
}

/// Is this a result to use in calculations (not excluded, not a control well)?
fn counted(rx: &Reaction, assay: &Assay) -> bool {
    !(rx.omitted
        || assay.excluded.is_some()
        || matches!(
            assay.task.as_deref(),
            Some("ntc" | "nac" | "ntp" | "nrt" | "negative" | "optical calibrator")
        ))
}

/// The Cq of a result to use in calculations (a Cq, not excluded, not a control well).
fn usable(rx: &Reaction, assay: &Assay) -> Option<f64> {
    if counted(rx, assay) { assay.cq } else { None }
}

fn relative(
    ds: &QpcrDataset,
    refs: &[String],
    control: Option<&str>,
    undetermined_cq: Option<f64>,
    notes: &mut Vec<String>,
) -> Result<Vec<RelativeQuantity>> {
    let data = &ds.data;
    if refs.is_empty() {
        return Err(Error::Usage(format!(
            "ΔΔCq needs a reference target (endogenous control): pass --reference-target TARGET (MCP: reference_targets); this file's targets: {}",
            data.targets
                .iter()
                .map(|t| t.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    for r in refs {
        if !data.targets.iter().any(|t| eq(&t.name, r)) {
            return Err(Error::Usage(format!(
                "reference target {r:?} is not in this file; targets: {}",
                data.targets
                    .iter()
                    .map(|t| t.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
    }
    // (sample, target) → Cqs; vendor ΔCq/RQ per group
    let mut cqs: BTreeMap<(String, String), Vec<f64>> = BTreeMap::new();
    let mut vendor: BTreeMap<(String, String), (Option<f64>, Option<f64>)> = BTreeMap::new();
    let mut undetermined: BTreeMap<(String, String), u32> = BTreeMap::new();
    for run in &data.runs {
        for rx in &run.reactions {
            let Some(s) = &rx.sample else { continue };
            for assay in &rx.assays {
                let Some(t) = &assay.target else { continue };
                if assay.task.as_deref() == Some("standard") || !counted(rx, assay) {
                    continue;
                }
                let cq = match (assay.cq, assay.cq_undetermined, undetermined_cq) {
                    (Some(c), _, _) => c,
                    (None, true, sub) => {
                        *undetermined.entry((s.clone(), t.clone())).or_default() += 1;
                        match sub {
                            Some(v) => v,
                            None => continue,
                        }
                    }
                    _ => continue,
                };
                cqs.entry((s.clone(), t.clone())).or_default().push(cq);
                let v = vendor.entry((s.clone(), t.clone())).or_default();
                v.0 = v.0.or(assay.vendor_delta_cq);
                v.1 = v.1.or(assay.vendor_rq);
            }
        }
    }
    let all_undetermined: Vec<String> = undetermined
        .iter()
        .filter(|(k, _)| !cqs.contains_key(*k))
        .map(|((s, t), n)| format!("{s} / {t} ({n})"))
        .collect();
    if !all_undetermined.is_empty() {
        notes.push(format!(
            "every replicate is undetermined (no Cq) for sample / target {}: no ΔCq for them",
            all_undetermined.join(", ")
        ));
    }
    let is_ref = |t: &str| refs.iter().any(|r| eq(r, t));
    let samples: Vec<String> = {
        let mut s: Vec<String> = cqs.keys().map(|k| k.0.clone()).collect();
        s.dedup();
        s
    };
    if let Some(c) = control
        && !samples.iter().any(|s| eq(s, c))
    {
        return Err(Error::Usage(format!(
            "control sample {c:?} has no usable Cq; samples: {}",
            samples.join(", ")
        )));
    }
    // per sample: reference mean Cq (mean over reference targets of their mean) and SD of all
    // reference wells
    let mut ref_stats: BTreeMap<String, (f64, Option<f64>)> = BTreeMap::new();
    for s in &samples {
        let mut means = Vec::new();
        let mut all = Vec::new();
        for r in refs {
            if let Some((_, v)) = cqs.iter().find(|((ss, t), _)| ss == s && eq(t, r)) {
                if let (Some(m), _, _) = mean_sd(v) {
                    means.push(m);
                }
                all.extend(v.iter().copied());
            }
        }
        if means.len() == refs.len() {
            let m = means.iter().sum::<f64>() / means.len() as f64;
            ref_stats.insert(s.clone(), (m, mean_sd(&all).1));
        }
    }
    let efficiency = |t: &str| {
        data.targets
            .iter()
            .find(|x| eq(&x.name, t))
            .and_then(|x| x.efficiency)
            .filter(|e| *e > 1.0 && *e <= 2.5)
    };
    let mut out = Vec::new();
    let mut missing_ref = Vec::new();
    for ((s, t), v) in &cqs {
        if is_ref(t) {
            continue;
        }
        let Some(&(ref_mean, ref_sd)) = ref_stats.get(s) else {
            missing_ref.push(s.clone());
            continue;
        };
        let (Some(m), sd, n) = mean_sd(v) else {
            continue;
        };
        let delta = m - ref_mean;
        let delta_sd = match (sd, ref_sd) {
            (Some(assay), Some(b)) => Some((assay * assay + b * b).sqrt()),
            (Some(assay), None) | (None, Some(assay)) => Some(assay),
            _ => None,
        };
        let vd = vendor
            .get(&(s.clone(), t.clone()))
            .copied()
            .unwrap_or_default();
        out.push(RelativeQuantity {
            sample: s.clone(),
            target: t.clone(),
            replicates: n as u32,
            undetermined: undetermined
                .get(&(s.clone(), t.clone()))
                .copied()
                .unwrap_or(0),
            cq_mean: m,
            cq_sd: sd,
            reference_cq_mean: ref_mean,
            delta_cq: delta,
            delta_cq_sd: delta_sd,
            vendor_delta_cq: vd.0,
            vendor_rq: vd.1,
            ..RelativeQuantity::default()
        });
    }
    missing_ref.sort();
    missing_ref.dedup();
    if !missing_ref.is_empty() {
        notes.push(format!(
            "no usable Cq of every reference target in sample(s) {}; they are left out",
            missing_ref.join(", ")
        ));
    }
    let Some(control) = control else {
        notes.push(
            "no control (calibrator) sample: ΔCq only; pass --control-sample SAMPLE (MCP: control_sample) for ΔΔCq and RQ"
                .into(),
        );
        return Ok(out);
    };
    let control_delta: BTreeMap<String, f64> = out
        .iter()
        .filter(|r| eq(&r.sample, control))
        .map(|r| (r.target.clone(), r.delta_cq))
        .collect();
    let control_cq: BTreeMap<String, f64> = cqs
        .iter()
        .filter(|((s, _), _)| eq(s, control))
        .filter_map(|((_, t), v)| mean_sd(v).0.map(|m| (t.clone(), m)))
        .collect();
    for r in &mut out {
        let Some(cd) = control_delta.get(&r.target) else {
            continue;
        };
        let dd = r.delta_cq - cd;
        r.delta_delta_cq = Some(dd);
        r.rq = Some(2f64.powf(-dd));
        if let Some(sd) = r.delta_cq_sd {
            r.rq_min = Some(2f64.powf(-(dd + sd)));
            r.rq_max = Some(2f64.powf(-(dd - sd)));
        }
        // Pfaffl: E_t^(Cq_t,control − Cq_t,sample) / geometric mean over references of
        // E_r^(Cq_r,control − Cq_r,sample).
        let et = efficiency(&r.target);
        let mut ref_ratio = Vec::new();
        for rf in refs {
            let (Some(e), Some(c_ctrl)) = (
                efficiency(rf),
                control_cq.iter().find(|(t, _)| eq(t, rf)).map(|x| *x.1),
            ) else {
                break;
            };
            let Some(c_s) = cqs
                .iter()
                .find(|((s, t), _)| eq(s, &r.sample) && eq(t, rf))
                .and_then(|(_, v)| mean_sd(v).0)
            else {
                break;
            };
            ref_ratio.push(e.powf(c_ctrl - c_s));
        }
        if let (Some(et), Some(ct)) = (et, control_cq.get(&r.target))
            && ref_ratio.len() == refs.len()
        {
            let geo = ref_ratio.iter().map(|x| x.ln()).sum::<f64>() / ref_ratio.len() as f64;
            r.efficiency_corrected_rq = Some(et.powf(ct - r.cq_mean) / geo.exp());
        }
    }
    Ok(out)
}

fn standard_curves(ds: &QpcrDataset, notes: &mut Vec<String>) -> Vec<StandardCurveFit> {
    let data = &ds.data;
    let mut pts: BTreeMap<String, Vec<(f64, f64)>> = BTreeMap::new();
    for run in &data.runs {
        for rx in &run.reactions {
            for assay in &rx.assays {
                if assay.task.as_deref() != Some("standard") {
                    continue;
                }
                let (Some(t), Some(q), Some(cq)) =
                    (&assay.target, assay.quantity, usable(rx, assay))
                else {
                    continue;
                };
                if q > 0.0 {
                    pts.entry(t.clone()).or_default().push((q.log10(), cq));
                }
            }
        }
    }
    if pts.is_empty() {
        notes.push("no standard wells with a quantity and a Cq: no standard curve".into());
    }
    let mut out = Vec::new();
    for (t, p) in pts {
        let x: Vec<f64> = p.iter().map(|v| v.0).collect();
        let y: Vec<f64> = p.iter().map(|v| v.1).collect();
        let mut levels = x.clone();
        levels.sort_by(f64::total_cmp);
        levels.dedup_by(|assay, b| (*assay - *b).abs() < 1e-9);
        if levels.len() < 2 {
            notes.push(format!(
                "target {t}: standards at a single quantity; no curve"
            ));
            continue;
        }
        let Some((assay, b, r2)) = linear_fit(&x, &y) else {
            continue;
        };
        let vendor = data.standard_curves.iter().find(|s| eq(&s.target, &t));
        out.push(StandardCurveFit {
            target: t,
            points: p.len() as u32,
            levels: levels.len() as u32,
            slope: b,
            intercept: assay,
            r2,
            efficiency_percent: (10f64.powf(-1.0 / b) - 1.0) * 100.0,
            vendor_slope: vendor.and_then(|v| v.slope),
            vendor_efficiency_percent: vendor.and_then(|v| v.efficiency_percent),
            vendor_r2: vendor.and_then(|v| v.r2),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_helpers() {
        assert_eq!(median(&mut [3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(&mut [4.0, 1.0, 2.0, 3.0]), Some(2.5));
        assert_eq!(median(&mut []), None);
        let r = pearson(&[1.0, 2.0, 3.0], &[2.0, 4.0, 6.0]).unwrap();
        assert!((r - 1.0).abs() < 1e-12);
    }
}
