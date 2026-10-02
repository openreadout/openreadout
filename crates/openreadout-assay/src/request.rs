//! What to analyse and how: the request shared by the CLI (`openreadout analyze assay …`), the
//! MCP tool (`openreadout_analyze` kind `assay`) and the Python binding (`openreadout.assay`).

use serde::{Deserialize, Serialize};

use crate::fit::{Model, Weighting};

/// The analysis to run.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum Analysis {
    /// Per-well and per-replicate-group values: roles, blank subtraction, mean, SD, CV %,
    /// outliers.
    #[default]
    Wells,
    /// Standard curve (linear, 4PL, 5PL) and back-calculated concentrations of every well.
    Curve,
    /// Dose-response per compound: IC50/EC50 with confidence interval, Hill slope, top, bottom.
    DoseResponse,
    /// Per-well kinetic metrics: max slope (Vmax), lag time, time to max, AUC.
    Kinetics,
    /// Per-well growth curves: growth rate, doubling time, lag, logistic fit.
    Growth,
    /// Assay quality from control wells: Z′, signal/background, signal/noise, CVs.
    Qc,
}

impl Analysis {
    /// The id (`wells`, `curve`, `dose-response`, `kinetics`, `growth`, `qc`).
    pub fn id(self) -> &'static str {
        match self {
            Analysis::Wells => "wells",
            Analysis::Curve => "curve",
            Analysis::DoseResponse => "dose-response",
            Analysis::Kinetics => "kinetics",
            Analysis::Growth => "growth",
            Analysis::Qc => "qc",
        }
    }
}

/// How a kinetic read becomes one value per well for endpoint analyses (wells, curve,
/// dose-response, qc).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Reduce {
    /// The first time point.
    First,
    /// The last time point.
    Last,
    /// The largest value.
    Max,
    /// The smallest value.
    Min,
    /// The mean over all time points.
    Mean,
    /// The largest windowed slope (value units per minute; `window` points).
    MaxSlope,
    /// The least-squares slope over all time points (value units per minute).
    MeanSlope,
    /// Trapezoidal area under the curve (value × s).
    Auc,
}

impl Reduce {
    /// The id.
    pub fn id(self) -> &'static str {
        match self {
            Reduce::First => "first",
            Reduce::Last => "last",
            Reduce::Max => "max",
            Reduce::Min => "min",
            Reduce::Mean => "mean",
            Reduce::MaxSlope => "max-slope",
            Reduce::MeanSlope => "mean-slope",
            Reduce::Auc => "auc",
        }
    }
}

/// Blank subtraction.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum BlankMode {
    /// Subtract the mean of the blank wells when the layout has blanks, else nothing.
    #[default]
    Auto,
    /// Subtract the mean of the blank wells (an error without blanks).
    Mean,
    /// Subtract the median of the blank wells (an error without blanks).
    Median,
    /// Never subtract.
    None,
}

/// Outlier rule within a replicate group.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum OutlierRule {
    /// Grubbs' test (groups of at least three wells): the value farthest from the mean is an
    /// outlier when |x − mean| / SD exceeds the two-sided critical value at alpha = threshold
    /// (default 0.05); one outlier per group.
    #[default]
    Grubbs,
    /// Modified z-score (Iglewicz–Hoaglin; groups of at least five wells, the MAD of fewer is
    /// degenerate): |0.6745 (x − median) / MAD| > threshold (default 3.5).
    Mad,
    /// No outlier flagging.
    None,
}

/// What the standard curve is fitted to.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum FitOn {
    /// Every standard well (replicates as separate points).
    #[default]
    Replicates,
    /// The mean of each standard level.
    Means,
}

/// Dose-response normalisation.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum Normalize {
    /// Fit the (blank-subtracted) signal.
    #[default]
    None,
    /// Percent effect: 100 (y − mean(negative)) / (mean(positive) − mean(negative)); the
    /// negative control is no effect (vehicle), the positive control full effect.
    Controls,
}

/// An assay-analysis request. Every field has a default; `analysis` picks what to compute.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct AssayRequest {
    /// The analysis.
    pub analysis: Analysis,
    /// Table (plate) index of the export (see `info` → `tables[]`). Default 0.
    pub table: u32,
    /// Read to analyse: its 1-based number or its label (exact, else a unique substring).
    /// Default: the first measured (not calculated) read.
    pub read: Option<String>,
    /// For spectral reads: the wavelength (nm) to analyse.
    pub wavelength_nm: Option<f64>,
    /// Kinetic reads in endpoint analyses: how a well's time course becomes one value.
    pub reduce: Option<Reduce>,
    /// Plate-layout file (CSV/TSV, plate-map grid or long form).
    pub layout: Option<String>,
    /// Plate layout as CSV/TSV text (instead of, or on top of, `layout`).
    pub layout_text: Option<String>,
    /// Use the layout the export embeds (Gen5 `Well ID`/`Conc/Dil`, SkanIt `Sample`, BMG
    /// `Content`/`Concentration`). Default true; a layout file or well flags override it well by
    /// well.
    pub embedded_layout: bool,
    /// Wells to mark as blanks (`H1,H2`, `H1:H12`).
    pub blank_wells: Option<String>,
    /// Wells to mark as positive controls (full effect / maximum signal).
    pub positive_wells: Option<String>,
    /// Wells to mark as negative controls (no effect / vehicle).
    pub negative_wells: Option<String>,
    /// Wells to ignore.
    pub empty_wells: Option<String>,
    /// Standards as `WELLS=CONCENTRATION` items separated by `;` (`A1,A2=100;B1:B2=50`).
    pub standards: Option<String>,
    /// Role of wells by the name the layout gives them (their role text or sample name, as
    /// written or without a trailing index): `{"DMSO": "negative", "STAU": "positive"}`. For names
    /// the layout cannot place (a vehicle is no effect in one assay and full signal in another).
    pub roles: std::collections::BTreeMap<String, String>,
    /// Blank subtraction. Default `auto`.
    pub blank: BlankMode,
    /// Outlier rule within replicate groups. Default `grubbs`.
    pub outliers: OutlierRule,
    /// Outlier threshold: alpha (`grubbs`, default 0.05) or modified z-score cut-off (`mad`,
    /// default 3.5).
    pub outlier_threshold: Option<f64>,
    /// Leave flagged outliers out of means, fits and controls. Default false (flag only).
    pub exclude_outliers: bool,
    /// Curve model: `linear`, `4pl`, `5pl`. Default `4pl`.
    pub model: Option<Model>,
    /// Fit weighting. Default `none`.
    pub weighting: Weighting,
    /// Standard curve: fit every replicate or the level means. Default `replicates`.
    pub fit_on: FitOn,
    /// Normalisation to the controls: the dose-response fit, and every well's
    /// `percent_effect` in `wells`. Default `none`.
    pub normalize: Normalize,
    /// Lower limit of quantification (overrides the recovery rule).
    pub lloq: Option<f64>,
    /// Upper limit of quantification (overrides the recovery rule).
    pub uloq: Option<f64>,
    /// Confidence level of intervals. Default 0.95.
    pub confidence: f64,
    /// Kinetics and growth: points per sliding window. Default 5 (kinetics: max slope; growth:
    /// the log-scale window).
    pub window: Option<usize>,
    /// Growth: values at or below this are left out of the log-scale fit. Default 5 % of each
    /// well's maximum.
    pub growth_threshold: Option<f64>,
    /// Kinetics and growth: only these wells (`A1:H6`). Default: every well with values.
    pub wells: Option<String>,
}

impl Default for AssayRequest {
    fn default() -> Self {
        AssayRequest {
            analysis: Analysis::Wells,
            table: 0,
            read: None,
            wavelength_nm: None,
            reduce: None,
            layout: None,
            layout_text: None,
            embedded_layout: true,
            blank_wells: None,
            positive_wells: None,
            negative_wells: None,
            empty_wells: None,
            standards: None,
            roles: std::collections::BTreeMap::new(),
            blank: BlankMode::Auto,
            outliers: OutlierRule::Grubbs,
            outlier_threshold: None,
            exclude_outliers: false,
            model: None,
            weighting: Weighting::None,
            fit_on: FitOn::Replicates,
            normalize: Normalize::None,
            lloq: None,
            uloq: None,
            confidence: 0.95,
            window: None,
            growth_threshold: None,
            wells: None,
        }
    }
}
