//! The result of an assay analysis: tidy tables (one row per well, per replicate group, per
//! compound) plus the fit report. Field names and units are documented in
//! book/src/guides/plate-analysis.md; every time is in seconds unless the name says otherwise.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// `openreadout analyze assay-wells|assay-curve|dose-response|kinetics|growth|assay-qc --json` data.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AssayOutput {
    /// The input file.
    pub path: String,
    /// Format id of the input (`plate`, or `long-csv`).
    pub format: String,
    /// The analysis run (`wells`, `curve`, `dose-response`, `kinetics`, `growth`, `qc`).
    pub analysis: String,
    /// Table (plate) index.
    pub table: u32,
    /// Plate (table) name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plate: Option<String>,
    /// The read analysed.
    pub read: ReadSummary,
    /// How kinetic time courses were reduced to one value per well (endpoint analyses).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reduce: Option<String>,
    /// Where the layout came from and what it assigns.
    pub layout: LayoutSummary,
    /// Blank subtraction applied.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blank: Option<BlankSummary>,
    /// Outlier rule and flagged wells.
    pub outliers: OutlierSummary,
    /// One row per well.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wells: Vec<WellRow>,
    /// One row per replicate group (sample, standard level, control, dose).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub samples: Vec<SampleRow>,
    /// Standard-curve fit report (`curve`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub curve: Option<CurveReport>,
    /// One row per compound (`dose-response`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub compounds: Vec<CompoundRow>,
    /// One row per well (`kinetics`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kinetics: Vec<KineticRow>,
    /// One row per well (`growth`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub growth: Vec<GrowthRow>,
    /// Assay quality from control wells (whenever positive and negative controls exist).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<QualityReport>,
    /// Files written (`--csv`, `--plot`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub written: Vec<String>,
    /// What was assumed or left out.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    /// What may be wrong with the layout: role names read as samples, controls of unknown sign,
    /// role-map entries that matched no well.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// The read analysed.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ReadSummary {
    /// 1-based read number.
    pub number: u32,
    /// Label as the file writes it.
    pub label: String,
    /// Detection mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// Unit of the values.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Values the vendor software calculated (not measured).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub calculated: bool,
    /// Whether the read has several time points per well.
    pub kinetic: bool,
    /// Time points per well (kinetic reads).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_points: Option<usize>,
    /// The wavelength selected (spectral reads).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wavelength_nm: Option<f64>,
    /// Wells with values.
    pub wells_measured: usize,
}

/// Layout summary.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LayoutSummary {
    /// Sources merged, in order (later ones win well by well): `embedded:<title>`, a file path,
    /// `text`, `--blank-wells`, ….
    pub sources: Vec<String>,
    /// Measured wells with a role.
    pub wells_assigned: usize,
    /// Measured wells per role (`unassigned` for wells the layout does not name).
    pub roles: BTreeMap<String, usize>,
    /// Unit of the concentrations, when the layout states one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub concentration_unit: Option<String>,
}

/// Blank subtraction applied.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct BlankSummary {
    /// `mean` or `median`.
    pub method: String,
    /// Blank wells used.
    pub wells: Vec<String>,
    /// Number of blank wells used.
    pub n: usize,
    /// The value subtracted (endpoint), or its mean over time points (kinetic: subtracted per
    /// time point).
    pub value: f64,
    /// SD of the blank wells (endpoint).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sd: Option<f64>,
    /// Whether the blank was subtracted per time point.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub per_time_point: bool,
}

/// Outlier rule and result.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct OutlierSummary {
    /// `mad`, `grubbs` or `none`.
    pub rule: String,
    /// Modified z-score cut-off (mad) or alpha (grubbs).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threshold: Option<f64>,
    /// Wells flagged.
    pub flagged: Vec<String>,
    /// Whether flagged wells were left out of means, fits and controls.
    pub excluded: bool,
}

/// One well.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WellRow {
    /// Well name (`A1`).
    pub well: String,
    /// 1-based row.
    pub row: u32,
    /// 1-based column.
    pub col: u32,
    /// Role (`blank`, `standard`, `sample`, `positive`, `negative`, `control`, `empty`), or
    /// `unassigned`.
    pub role: String,
    /// Replicate group this well belongs to.
    pub group: String,
    /// Sample name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample: Option<String>,
    /// Compound.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compound: Option<String>,
    /// Nominal concentration (standards, doses).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub concentration: Option<f64>,
    /// Dilution factor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dilution: Option<f64>,
    /// The value as read (after `reduce` for kinetic reads); null when the cell was not a
    /// number.
    pub raw: Option<f64>,
    /// After blank subtraction (equal to `raw` without one).
    pub value: Option<f64>,
    /// Flagged as an outlier in its replicate group.
    pub outlier: bool,
    /// Left out of means and fits.
    pub excluded: bool,
    /// Concentration back-calculated from the standard curve (`curve`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub back_calculated: Option<f64>,
    /// `back_calculated` × `dilution`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_concentration: Option<f64>,
    /// `curve`: `in_range`, `below_range`, `above_range` (outside the standards'
    /// concentrations), `below_curve`, `above_curve` (beyond an asymptote: no concentration) or
    /// `not_computable`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flag: Option<String>,
    /// `curve`: within [LLOQ, ULOQ].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quantifiable: Option<bool>,
    /// Standards: back-calculated / nominal × 100.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery_percent: Option<f64>,
    /// `dose-response` with `normalize: controls`: percent effect.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub percent_effect: Option<f64>,
    /// Other layout columns.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, String>,
}

/// One replicate group.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SampleRow {
    /// Group name (the sample name, else role and concentration, else the well).
    pub group: String,
    /// Role of the group.
    pub role: String,
    /// Sample name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample: Option<String>,
    /// Compound.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compound: Option<String>,
    /// Nominal concentration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub concentration: Option<f64>,
    /// Dilution factor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dilution: Option<f64>,
    /// Wells of the group.
    pub wells: Vec<String>,
    /// What `mean`/`sd`/`cv_percent` summarise: `value` (blank-subtracted), or a kinetic/growth
    /// metric (`max_slope_per_min`, `doubling_time_h`).
    pub metric: String,
    /// Wells in the group.
    pub n: usize,
    /// Wells used (numbers, not excluded).
    pub n_used: usize,
    /// Mean of the used wells.
    pub mean: Option<f64>,
    /// Sample SD (n − 1).
    pub sd: Option<f64>,
    /// SD / |mean| × 100.
    pub cv_percent: Option<f64>,
    /// Outlier wells.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outliers: Vec<String>,
    /// `curve`: mean of the wells' back-calculated concentrations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub back_calculated_mean: Option<f64>,
    /// `curve`: SD of the wells' back-calculated concentrations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub back_calculated_sd: Option<f64>,
    /// `curve`: CV % of the wells' back-calculated concentrations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub back_calculated_cv_percent: Option<f64>,
    /// `curve`: concentration back-calculated from the group's mean signal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub back_calculated_of_mean: Option<f64>,
    /// `back_calculated_mean` × dilution.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_concentration: Option<f64>,
    /// `curve`: range flag of `back_calculated_of_mean`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flag: Option<String>,
    /// Standards: `back_calculated_mean` / nominal × 100.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery_percent: Option<f64>,
}

/// One fitted parameter.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ParamRow {
    /// Name (`a`, `b`, `c`, `d`, `g`; `slope`, `intercept`).
    pub name: String,
    /// Estimate.
    pub value: f64,
    /// Standard error (Wald).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub se: Option<f64>,
    /// Lower confidence limit: value − t·SE.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ci_low: Option<f64>,
    /// Upper confidence limit: value + t·SE.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ci_high: Option<f64>,
}

/// One point of a fit.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CurvePoint {
    /// Well (replicate fits) or group (fits on means).
    pub label: String,
    /// Concentration.
    pub x: f64,
    /// Response.
    pub y: f64,
    /// Fitted response.
    pub fitted: f64,
    /// y − fitted.
    pub residual: f64,
}

/// A fitted curve.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct FitReport {
    /// `linear`, `4pl`, `5pl`.
    pub model: String,
    /// The formula in the parameter names.
    pub formula: String,
    /// `none`, `1/y`, `1/y2`, `1/x`, `1/x2`.
    pub weighting: String,
    /// Parameters with standard errors and confidence limits.
    pub parameters: Vec<ParamRow>,
    /// Points fitted.
    pub n_points: usize,
    /// Distinct concentrations.
    pub n_levels: usize,
    /// Residual degrees of freedom.
    pub df: usize,
    /// 1 − SSE/SST.
    pub r_squared: f64,
    /// Residual standard error √(SSE/df).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub residual_se: Option<f64>,
    /// Weighted sum of squared residuals.
    pub sse: f64,
    /// Whether the optimiser converged.
    pub converged: bool,
    /// Optimiser iterations.
    pub iterations: usize,
    /// Confidence level of the limits.
    pub confidence: f64,
    /// `increasing` or `decreasing` response with concentration.
    pub direction: String,
    /// The points with fitted values and residuals.
    pub points: Vec<CurvePoint>,
}

/// Standard-curve report.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CurveReport {
    /// The fit.
    pub fit: FitReport,
    /// `replicates` or `means`.
    pub fit_on: String,
    /// Lowest standard concentration.
    pub range_low: f64,
    /// Highest standard concentration.
    pub range_high: f64,
    /// Lower limit of quantification.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lloq: Option<f64>,
    /// Upper limit of quantification.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uloq: Option<f64>,
    /// How LLOQ/ULOQ were set.
    pub loq_rule: String,
    /// Unit of the concentrations, when the layout states one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub concentration_unit: Option<String>,
}

/// Dose-response of one compound.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CompoundRow {
    /// Compound name.
    pub compound: String,
    /// `IC50` (response falls with dose) or `EC50`.
    pub kind: String,
    /// Concentration at the curve's midpoint (the 4PL/5PL parameter c).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ec50: Option<f64>,
    /// Standard error of `ec50` (Wald, delta method).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ec50_se: Option<f64>,
    /// Lower confidence limit of `ec50`, from ln(c) ± t·SE(ln c).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ec50_ci_low: Option<f64>,
    /// Upper confidence limit of `ec50`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ec50_ci_high: Option<f64>,
    /// log10 of `ec50`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log10_ec50: Option<f64>,
    /// Concentration where the fitted curve crosses 50 % effect (`normalize: controls`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub absolute_ec50: Option<f64>,
    /// Hill slope, GraphPad sign convention: positive when the response rises with dose.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hill_slope: Option<f64>,
    /// The upper plateau: max(a, d).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top: Option<f64>,
    /// The lower plateau: min(a, d).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bottom: Option<f64>,
    /// Response at zero dose (a).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_at_zero: Option<f64>,
    /// Response at infinite dose (d).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_at_infinity: Option<f64>,
    /// Doses (distinct concentrations).
    pub n_concentrations: usize,
    /// Lowest and highest dose tested.
    pub min_concentration: f64,
    /// Highest dose tested.
    pub max_concentration: f64,
    /// `ec50` lies outside the tested doses.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub extrapolated: bool,
    /// The fit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fit: Option<FitReport>,
    /// Why no fit was made.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Kinetic metrics of one well.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct KineticRow {
    /// Well name.
    pub well: String,
    /// Role, or `unassigned`.
    pub role: String,
    /// Sample name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample: Option<String>,
    /// Time points used.
    pub n_points: usize,
    /// Points per window.
    pub window: usize,
    /// Largest windowed slope, value units per minute (Vmax).
    pub max_slope_per_min: f64,
    /// The same per second.
    pub max_slope_per_s: f64,
    /// r² of the max-slope window.
    pub max_slope_r_squared: f64,
    /// Mean time of the max-slope window (s).
    pub time_at_max_slope_s: f64,
    /// First time of the max-slope window (s).
    pub window_start_s: f64,
    /// Last time of the max-slope window (s).
    pub window_end_s: f64,
    /// Lag time (s).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lag_time_s: Option<f64>,
    /// Least-squares slope over all points, value units per minute.
    pub mean_slope_per_min: f64,
    /// r² of the all-points line.
    pub mean_slope_r_squared: f64,
    /// Largest value.
    pub max_value: f64,
    /// Time of the largest value (s).
    pub time_to_max_s: f64,
    /// Smallest value.
    pub min_value: f64,
    /// First value.
    pub initial_value: f64,
    /// Last value.
    pub final_value: f64,
    /// Trapezoidal area under the curve (value × s).
    pub auc: f64,
}

/// Growth metrics of one well.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct GrowthRow {
    /// Well name.
    pub well: String,
    /// Role, or `unassigned`.
    pub role: String,
    /// Sample name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample: Option<String>,
    /// Time points used.
    pub n_points: usize,
    /// The well's own minimum, subtracted as background when the layout has no blank wells
    /// (growthcurver's default); absent when blanks were subtracted or subtraction is off.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background: Option<f64>,
    /// Points per window of the log-scale fit.
    pub window: usize,
    /// Values at or below this were left out of the log-scale fit.
    pub threshold: f64,
    /// Maximum specific growth rate µmax (per hour).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub growth_rate_per_h: Option<f64>,
    /// ln 2 / µmax, hours.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doubling_time_h: Option<f64>,
    /// ln 2 / µmax, minutes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doubling_time_min: Option<f64>,
    /// First time of the exponential phase (s).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exp_phase_start_s: Option<f64>,
    /// Last time of the exponential phase (s).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exp_phase_end_s: Option<f64>,
    /// r² of ln(value) against time in the exponential phase.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exp_r_squared: Option<f64>,
    /// Lag time (s).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lag_time_s: Option<f64>,
    /// Largest value.
    pub max_value: f64,
    /// Time of the largest value (s).
    pub time_to_max_s: f64,
    /// Trapezoidal area under the curve (value × h).
    pub auc_h: f64,
    /// Logistic fit: carrying capacity K.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logistic_k: Option<f64>,
    /// Logistic fit: N0 at the first time point.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logistic_n0: Option<f64>,
    /// Logistic fit: r (per hour).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logistic_r_per_h: Option<f64>,
    /// Logistic fit: ln 2 / r (hours).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logistic_doubling_time_h: Option<f64>,
    /// Logistic fit: inflection time (s).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logistic_t_mid_s: Option<f64>,
    /// Logistic fit: residual SD.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logistic_sigma: Option<f64>,
}

/// Statistics of a group of control wells.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct GroupStats {
    /// Wells used.
    pub wells: Vec<String>,
    /// Count.
    pub n: usize,
    /// Mean.
    pub mean: f64,
    /// Sample SD.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sd: Option<f64>,
    /// CV %.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cv_percent: Option<f64>,
    /// Smallest value.
    pub min: f64,
    /// Largest value.
    pub max: f64,
}

/// Assay quality of the plate (values as read, before blank subtraction).
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct QualityReport {
    /// Positive-control wells.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub positive: Option<GroupStats>,
    /// Negative-control wells.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub negative: Option<GroupStats>,
    /// Blank wells.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blank: Option<GroupStats>,
    /// Sample wells.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub samples: Option<GroupStats>,
    /// Z′ = 1 − 3 (SDpos + SDneg) / |mean pos − mean neg| (Zhang, Chung & Oldenburg 1999).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub z_prime: Option<f64>,
    /// Z = 1 − 3 (SDsamples + SDneg) / |mean samples − mean neg|.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub z_factor: Option<f64>,
    /// Higher control mean / lower control mean.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signal_to_background: Option<f64>,
    /// |mean pos − mean neg| / SDneg.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signal_to_noise: Option<f64>,
    /// Strictly standardized mean difference (mean pos − mean neg) / √(SDpos² + SDneg²).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ssmd: Option<f64>,
    /// Median CV % over replicate groups of two or more wells.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub median_replicate_cv_percent: Option<f64>,
    /// `excellent` (Z′ ≥ 0.5), `marginal` (0 < Z′ < 0.5) or `unusable` (Z′ ≤ 0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assessment: Option<String>,
}
