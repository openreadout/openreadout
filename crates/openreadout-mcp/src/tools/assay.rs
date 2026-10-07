//! The plate-reader assay tools, one per analysis of `openreadout analyze`:
//! `openreadout_assay_wells`, `openreadout_assay_curve`, `openreadout_dose_response`,
//! `openreadout_kinetics`, `openreadout_growth` and `openreadout_assay_qc`.
//!
//! They share the plate arguments ([`Plate`]): the layout and the control wells at the top level,
//! the rarely needed ones under `plate_options`. Each tool adds the arguments of its analysis
//! and runs `openreadout_batch::analyze` kind `assay` with `analysis` set. Everything assay-
//! specific in this server is in this file, so folding the six tools into one
//! `openreadout_assay` with an `analysis` argument means replacing this file only.

use std::collections::BTreeMap;

use openreadout_assay::fit::{Model, Weighting};
use openreadout_assay::{Analysis, BlankMode, FitOn, Normalize, OutlierRule, Reduce};
use openreadout_batch::analyze::AnalyzeKind;
use openreadout_core::Error;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData as McpError, tool, tool_router};
use serde_json::Value;

use super::analyses::{AnalysisArgs, call};
use super::object_output;
use crate::{InstrumentServer, mcp_err};

/// Plate layout and control wells, shared by every assay tool.
#[derive(schemars::JsonSchema)]
#[allow(dead_code)]
pub struct Plate {
    /// Plate-layout file: CSV/TSV, a plate-map grid (blocks whose corner cell names the field:
    /// role, sample, compound, concentration, dilution) or a long table with a `well` column.
    layout: Option<String>,
    /// Wells to mark as blanks (`H1,H2`, `H1:H12`).
    blank_wells: Option<String>,
    /// Wells to mark as positive controls (full effect / maximum signal).
    positive_wells: Option<String>,
    /// Wells to mark as negative controls (no effect / vehicle).
    negative_wells: Option<String>,
    /// Rarely needed plate options.
    plate_options: Option<PlateOptions>,
}

/// Rarely needed plate options.
#[derive(schemars::JsonSchema)]
#[allow(dead_code)]
pub struct PlateOptions {
    /// Table (plate) index of the export (see openreadout_info → tables[]). Default 0.
    table: Option<u32>,
    /// Read to analyse: its 1-based number or its label. Default: the first measured read.
    read: Option<String>,
    /// For spectral reads: the wavelength (nm) to analyse.
    wavelength_nm: Option<f64>,
    /// Use the layout the export embeds (Gen5 Well ID and Conc/Dil, SkanIt Sample, BMG
    /// Content/Concentration). Default true.
    embedded_layout: Option<bool>,
    /// Plate layout as CSV/TSV text (instead of, or on top of, `layout`).
    layout_text: Option<String>,
    /// Wells to ignore.
    empty_wells: Option<String>,
    /// Role of wells by the name the layout gives them: `{"DMSO": "negative", "STAU": "positive"}`.
    roles: Option<BTreeMap<String, String>>,
    /// Default `auto`.
    blank_subtraction: Option<BlankMode>,
    /// Default `grubbs`.
    outliers: Option<OutlierRule>,
    /// Outlier threshold: alpha (`grubbs`, default 0.05) or modified z-score cut-off (`mad`,
    /// default 3.5).
    outlier_threshold: Option<f64>,
    /// Leave flagged outliers out of means, fits and controls. Default false (flag only).
    exclude_outliers: Option<bool>,
}

/// Endpoint analyses of a kinetic read.
#[derive(schemars::JsonSchema)]
#[allow(dead_code)]
pub struct Endpoint {
    /// Kinetic reads.
    reduce: Option<Reduce>,
    /// Points per window for reduce=max-slope.
    window: Option<usize>,
}

/// Curve fits.
#[derive(schemars::JsonSchema)]
#[allow(dead_code)]
pub struct Fit {
    /// Curve model: `linear`, `4pl`, `5pl`. Default `4pl`.
    model: Option<Model>,
    /// Fit weighting. Default `none`.
    weighting: Option<Weighting>,
    /// Confidence level of intervals. Default 0.95.
    confidence: Option<f64>,
    /// Also return a PNG of the points and the fitted curve as image content.
    plot: Option<bool>,
}

/// `openreadout_assay_wells` and `openreadout_assay_qc`.
#[derive(schemars::JsonSchema)]
#[allow(dead_code)]
pub struct WellsArgs {
    #[serde(flatten)]
    plate: Plate,
    #[serde(flatten)]
    endpoint: Endpoint,
    /// Every well's percent effect. Default `none`.
    normalize: Option<Normalize>,
}

/// `openreadout_assay_curve`.
#[derive(schemars::JsonSchema)]
#[allow(dead_code)]
pub struct CurveArgs {
    #[serde(flatten)]
    plate: Plate,
    #[serde(flatten)]
    endpoint: Endpoint,
    #[serde(flatten)]
    fit: Fit,
    /// Standards as `WELLS=CONCENTRATION` items separated by `;` (`A1,A2=100;B1:B2=50`); default:
    /// the layout's.
    standards: Option<String>,
    /// Fit every standard replicate or the level means. Default `replicates`.
    fit_on: Option<FitOn>,
    /// Lower limit of quantification (overrides the recovery rule).
    lloq: Option<f64>,
    /// Upper limit of quantification (overrides the recovery rule).
    uloq: Option<f64>,
}

/// `openreadout_dose_response`.
#[derive(schemars::JsonSchema)]
#[allow(dead_code)]
pub struct DoseResponseArgs {
    #[serde(flatten)]
    plate: Plate,
    #[serde(flatten)]
    endpoint: Endpoint,
    #[serde(flatten)]
    fit: Fit,
    /// What is fitted. Default `none`.
    normalize: Option<Normalize>,
}

/// `openreadout_kinetics`.
#[derive(schemars::JsonSchema)]
#[allow(dead_code)]
pub struct KineticsArgs {
    #[serde(flatten)]
    plate: Plate,
    /// Points per sliding window. Default: 5 or a tenth of the time points, whichever is larger.
    window: Option<usize>,
    /// Only these wells (`A1:H6`). Default: every well with values.
    wells: Option<String>,
}

/// `openreadout_growth`.
#[derive(schemars::JsonSchema)]
#[allow(dead_code)]
pub struct GrowthArgs {
    #[serde(flatten)]
    kinetics: KineticsArgs,
    /// Values at or below this are left out of the log-scale fit. Default 5 % of each well's
    /// maximum.
    growth_threshold: Option<f64>,
}

/// Run one assay analysis: check that every argument is one of `Q`'s, then hand the request to
/// the shared dispatcher with `analysis` set.
async fn assay<Q: schemars::JsonSchema>(
    server: &InstrumentServer,
    tool: &str,
    analysis: Analysis,
    a: AnalysisArgs<Q>,
) -> Result<CallToolResult, McpError> {
    let accepted = openreadout_batch::measures::accepted_keys::<AnalysisArgs<Q>>();
    let unknown: Vec<&String> = a
        .options
        .keys()
        .filter(|k| !accepted.contains(*k))
        .collect();
    if !unknown.is_empty() {
        return Err(mcp_err(&Error::Usage(format!(
            "{tool} has no argument {}; it takes: {}",
            unknown
                .iter()
                .map(|k| format!("`{k}`"))
                .collect::<Vec<_>>()
                .join(", "),
            accepted.iter().cloned().collect::<Vec<_>>().join(", ")
        ))));
    }
    let mut options = a.options;
    options.insert("analysis".into(), Value::String(analysis.id().into()));
    call(server, AnalyzeKind::Assay, a.file, a.strict, options).await
}

#[tool_router(router = assay_router, vis = "pub(super)")]
impl InstrumentServer {
    #[tool(
        name = "openreadout_assay_wells",
        annotations(title = "Plate wells and replicates", read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = object_output::<openreadout_assay::AssayOutput>(),
        description = "Plate-reader export: per-well values with roles from the layout, blank subtraction, and per replicate group the mean, SD, CV % and outlier flags (plus Z′ when the layout has positive and negative controls)."
    )]
    pub(crate) async fn assay_wells(
        &self,
        Parameters(a): Parameters<AnalysisArgs<WellsArgs>>,
    ) -> Result<CallToolResult, McpError> {
        assay(self, "openreadout_assay_wells", Analysis::Wells, a).await
    }

    #[tool(
        name = "openreadout_assay_curve",
        annotations(title = "Standard curve", read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = object_output::<openreadout_assay::AssayOutput>(),
        description = "Plate-reader export: fit a standard curve (linear, 4PL, 5PL) to the standard wells and back-calculate the concentration of every well, with range flags, LLOQ/ULOQ, R² and residuals."
    )]
    pub(crate) async fn assay_curve(
        &self,
        Parameters(a): Parameters<AnalysisArgs<CurveArgs>>,
    ) -> Result<CallToolResult, McpError> {
        assay(self, "openreadout_assay_curve", Analysis::Curve, a).await
    }

    #[tool(
        name = "openreadout_dose_response",
        annotations(title = "Dose-response", read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = object_output::<openreadout_assay::AssayOutput>(),
        description = "Plate-reader export: a dose-response fit (4PL/5PL) per compound: IC50/EC50 with confidence interval, Hill slope, top and bottom; optionally normalised to the controls (percent effect)."
    )]
    pub(crate) async fn dose_response(
        &self,
        Parameters(a): Parameters<AnalysisArgs<DoseResponseArgs>>,
    ) -> Result<CallToolResult, McpError> {
        assay(self, "openreadout_dose_response", Analysis::DoseResponse, a).await
    }

    #[tool(
        name = "openreadout_kinetics",
        annotations(title = "Kinetic reads", read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = object_output::<openreadout_assay::AssayOutput>(),
        description = "Plate-reader kinetic read: per well the max slope (Vmax) over a sliding window, lag time, time to max, mean slope and AUC."
    )]
    pub(crate) async fn kinetics(
        &self,
        Parameters(a): Parameters<AnalysisArgs<KineticsArgs>>,
    ) -> Result<CallToolResult, McpError> {
        assay(self, "openreadout_kinetics", Analysis::Kinetics, a).await
    }

    #[tool(
        name = "openreadout_growth",
        annotations(title = "Growth curves", read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = object_output::<openreadout_assay::AssayOutput>(),
        description = "Plate-reader growth curves (OD over time): per well the maximum growth rate and doubling time from the exponential phase, lag time, and a logistic fit (K, r, N0)."
    )]
    pub(crate) async fn growth(
        &self,
        Parameters(a): Parameters<AnalysisArgs<GrowthArgs>>,
    ) -> Result<CallToolResult, McpError> {
        assay(self, "openreadout_growth", Analysis::Growth, a).await
    }

    #[tool(
        name = "openreadout_assay_qc",
        annotations(title = "Assay quality", read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = object_output::<openreadout_assay::AssayOutput>(),
        description = "Plate-reader assay quality from the control wells: Z′, signal/background, signal/noise, SSMD and CVs."
    )]
    pub(crate) async fn assay_qc(
        &self,
        Parameters(a): Parameters<AnalysisArgs<WellsArgs>>,
    ) -> Result<CallToolResult, McpError> {
        assay(self, "openreadout_assay_qc", Analysis::Qc, a).await
    }
}
