//! `openreadout_assay`: the plate-reader analyses of `openreadout analyze` (`assay-wells`,
//! `assay-curve`, `dose-response`, `kinetics`, `growth`, `assay-qc`) as one tool with an
//! `analysis` argument. They share the plate arguments ([`Plate`]): the layout and the control
//! wells at the top level, the rarely needed ones under `plate_options`. The arguments of one
//! analysis only say so in their description, and a call with an argument its analysis does not
//! take is an error that lists the ones it does. The request runs `openreadout_batch::analyze`
//! kind `assay`.

use std::collections::BTreeMap;

use openreadout_assay::fit::{Model, Weighting};
use openreadout_assay::{Analysis, BlankMode, FitOn, Normalize, OutlierRule, Reduce};
use openreadout_batch::analyze::AnalyzeKind;
use openreadout_core::Error;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, tool, tool_router};
use serde_json::Value;

use super::analyses::{AnalysisArgs, Lean, call};

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
    /// Blank subtraction. Default `auto`.
    blank_subtraction: Option<BlankMode>,
    /// Outlier rule within a replicate group. Default `grubbs`.
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
    /// wells, curve, dose-response, qc on a kinetic read: how each well's time course becomes
    /// one value.
    reduce: Option<Reduce>,
    /// Points per sliding window (reduce=max-slope, kinetics, growth). Default: 5 or a tenth of
    /// the time points, whichever is larger.
    window: Option<usize>,
}

/// Curve fits.
#[derive(schemars::JsonSchema)]
#[allow(dead_code)]
pub struct Fit {
    /// curve, dose-response: the model. Default `4pl`.
    model: Option<Model>,
    /// curve, dose-response: fit weighting. Default `none`.
    weighting: Option<Weighting>,
    /// curve, dose-response: confidence level of intervals. Default 0.95.
    confidence: Option<f64>,
    /// curve, dose-response: also return a PNG of the points and the fitted curve.
    plot: Option<bool>,
}

/// Every argument of `openreadout_assay`: the schema the model sees.
#[derive(schemars::JsonSchema)]
#[allow(dead_code)]
pub struct AssayArgs {
    /// What to compute.
    analysis: Analysis,
    #[serde(flatten)]
    plate: Plate,
    #[serde(flatten)]
    endpoint: Endpoint,
    #[serde(flatten)]
    fit: Fit,
    /// wells, dose-response, qc: percent effect from the controls. Default `none`.
    normalize: Option<Normalize>,
    /// curve: standards as `WELLS=CONCENTRATION` items separated by `;`
    /// (`A1,A2=100;B1:B2=50`). Default: the layout's.
    standards: Option<String>,
    /// curve: fit every standard replicate or the level means. Default `replicates`.
    fit_on: Option<FitOn>,
    /// curve: lower limit of quantification (overrides the recovery rule).
    lloq: Option<f64>,
    /// curve: upper limit of quantification (overrides the recovery rule).
    uloq: Option<f64>,
    /// Only these wells (`A1:H6`); the layout's blanks and controls are still used.
    wells: Option<String>,
    /// growth: values at or below this are left out of the log-scale fit. Default 5 % of each
    /// well's maximum.
    growth_threshold: Option<f64>,
    /// Fields of the per-well and per-group rows. Default: the main ones, rounded to 4
    /// significant digits; ["all"] for every field at full precision.
    fields: Option<Vec<String>>,
}

/// The arguments each analysis takes (besides `file`, `strict` and `analysis`).
#[derive(schemars::JsonSchema)]
#[allow(dead_code)]
struct WellsArgs {
    #[serde(flatten)]
    plate: Plate,
    #[serde(flatten)]
    endpoint: Endpoint,
    normalize: Option<Normalize>,
    wells: Option<String>,
}

#[derive(schemars::JsonSchema)]
#[allow(dead_code)]
struct CurveArgs {
    #[serde(flatten)]
    plate: Plate,
    #[serde(flatten)]
    endpoint: Endpoint,
    #[serde(flatten)]
    fit: Fit,
    standards: Option<String>,
    fit_on: Option<FitOn>,
    lloq: Option<f64>,
    uloq: Option<f64>,
    wells: Option<String>,
}

#[derive(schemars::JsonSchema)]
#[allow(dead_code)]
struct DoseResponseArgs {
    #[serde(flatten)]
    plate: Plate,
    #[serde(flatten)]
    endpoint: Endpoint,
    #[serde(flatten)]
    fit: Fit,
    normalize: Option<Normalize>,
    wells: Option<String>,
}

#[derive(schemars::JsonSchema)]
#[allow(dead_code)]
struct KineticsArgs {
    #[serde(flatten)]
    plate: Plate,
    window: Option<usize>,
    wells: Option<String>,
}

#[derive(schemars::JsonSchema)]
#[allow(dead_code)]
struct GrowthArgs {
    #[serde(flatten)]
    kinetics: KineticsArgs,
    growth_threshold: Option<f64>,
}

/// The arguments `analysis` takes.
fn accepted(analysis: Analysis) -> std::collections::BTreeSet<String> {
    use openreadout_batch::measures::accepted_keys;
    let mut k = match analysis {
        Analysis::Wells | Analysis::Qc => accepted_keys::<WellsArgs>(),
        Analysis::Curve => accepted_keys::<CurveArgs>(),
        Analysis::DoseResponse => accepted_keys::<DoseResponseArgs>(),
        Analysis::Kinetics => accepted_keys::<KineticsArgs>(),
        Analysis::Growth => accepted_keys::<GrowthArgs>(),
    };
    k.insert("analysis".into());
    k.insert("fields".into());
    k
}

#[tool_router(router = assay_router, vis = "pub(super)")]
impl InstrumentServer {
    #[tool(
        name = "openreadout_assay",
        annotations(title = "Analyse a plate-reader assay", read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = super::lean_output_schema::<openreadout_assay::AssayOutput>(crate::lean::ASSAY),
        description = "Plate-reader exports, with the layout the file stores or `layout`. analysis: wells (per-well values, blank subtraction, replicate mean/SD/CV, outliers), curve (standard curve, back-calculated concentrations, LLOQ/ULOQ), dose-response (IC50/EC50 per compound with CI and Hill slope), kinetics (Vmax, lag, AUC per well), growth (growth rate, doubling time, lag, logistic fit, and whether each well grew, crashed or regrew, with times), qc (Z′, S/B, S/N, SSMD). An argument that names analyses applies to those only."
    )]
    pub(crate) async fn assay(
        &self,
        Parameters(a): Parameters<AnalysisArgs<AssayArgs>>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let analysis: Analysis = match a.options.get("analysis") {
            Some(v) => serde_json::from_value(v.clone()).map_err(|_| {
                mcp_err(&Error::Usage(format!(
                    "analysis must be wells, curve, dose-response, kinetics, growth or qc, not {v}"
                )))
            })?,
            None => {
                return Err(mcp_err(&Error::Usage(
                    "openreadout_assay needs analysis: wells, curve, dose-response, kinetics, growth or qc".into(),
                )));
            }
        };
        let ok = accepted(analysis);
        let unknown: Vec<&String> = a.options.keys().filter(|k| !ok.contains(*k)).collect();
        if !unknown.is_empty() {
            return Err(mcp_err(&Error::Usage(format!(
                "analysis={} takes no {}; it takes: {}",
                analysis.id(),
                unknown
                    .iter()
                    .map(|k| format!("`{k}`"))
                    .collect::<Vec<_>>()
                    .join(", "),
                ok.iter().cloned().collect::<Vec<_>>().join(", ")
            ))));
        }
        let mut options = a.options;
        let lean = Lean::take(
            &mut options,
            crate::lean::ASSAY,
            &["growth_summary"],
            &["growth", "kinetics", "wells"],
        )?;
        options.insert("analysis".into(), Value::String(analysis.id().into()));
        call(
            self,
            AnalyzeKind::Assay,
            a.file,
            a.strict,
            options,
            Some(lean),
            &ctx,
        )
        .await
    }
}
