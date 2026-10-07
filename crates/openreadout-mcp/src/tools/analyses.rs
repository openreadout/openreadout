//! The analysis tools: `openreadout_peaks`, `openreadout_chromatogram`,
//! `openreadout_nmr_peaks`, `openreadout_ephys_features`, `openreadout_spikes`,
//! `openreadout_qpcr` and `openreadout_gate`, one per `openreadout analyze` subcommand. The
//! plate-reader assays are in `assay.rs`.
//!
//! Each tool's arguments are `file`, `strict` and the analysis's own options as top-level
//! arguments. They are run by `openreadout_batch::analyze`, which the Python and R bindings and
//! `openreadout_batch` share, so an argument the analysis does not take is an error that names
//! the ones it does.

use std::borrow::Cow;
use std::marker::PhantomData;

use base64::Engine as _;
use openreadout_batch::analyze::{AnalyzeArgs, AnalyzeKind, run};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::{ErrorData as McpError, tool, tool_router};
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};

use super::object_output;
use crate::{InstrumentServer, mcp_err, to_value, with_strict};

/// The arguments of an analysis tool: `file`, `strict`, and the analysis's options `Q` at the
/// top level. They are read as a map (so that unknown arguments get a helpful error from the
/// analysis); `Q` only shapes the input schema.
#[derive(Debug)]
pub struct AnalysisArgs<Q> {
    /// The file.
    pub file: String,
    /// The `strict` override.
    pub strict: Option<bool>,
    /// Every other argument.
    pub options: Map<String, Value>,
    _schema: PhantomData<fn() -> Q>,
}

impl<'de, Q> Deserialize<'de> for AnalysisArgs<Q> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error as _;
        let mut options = Map::<String, Value>::deserialize(d)?;
        let file = match options.remove("file") {
            Some(Value::String(s)) => s,
            Some(_) => return Err(D::Error::custom("`file` must be a string (a path)")),
            None => return Err(D::Error::missing_field("file")),
        };
        let strict = match options.remove("strict") {
            None | Some(Value::Null) => None,
            Some(Value::Bool(b)) => Some(b),
            Some(_) => return Err(D::Error::custom("`strict` must be true or false")),
        };
        Ok(Self {
            file,
            strict,
            options,
            _schema: PhantomData,
        })
    }
}

/// The input schema of an analysis tool: `file`, the options of `Q`, `strict`.
#[derive(schemars::JsonSchema)]
#[allow(dead_code)]
struct Shape<Q> {
    /// Absolute or working-directory-relative path to the instrument file or data-set directory.
    file: String,
    #[serde(flatten)]
    options: Q,
    /// true: refuse (an error with exit_code 6 and a hint) values this file's assurance does not
    /// validate. Default: the server's setting (OPENREADOUT_STRICT; off).
    strict: Option<bool>,
}

impl<Q: schemars::JsonSchema> schemars::JsonSchema for AnalysisArgs<Q> {
    fn schema_name() -> Cow<'static, str> {
        Shape::<Q>::schema_name()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        Shape::<Q>::json_schema(generator)
    }
}

/// Run analysis `kind` with `options` on `file`, as a tool result (with the PNG of a plot first,
/// when the analysis drew one).
pub(super) async fn call(
    server: &InstrumentServer,
    kind: AnalyzeKind,
    file: String,
    strict: Option<bool>,
    options: Map<String, Value>,
) -> Result<CallToolResult, McpError> {
    let registry = server.registry;
    let args = AnalyzeArgs {
        file,
        kind,
        options,
        strict,
    };
    let (out, png) =
        tokio::task::spawn_blocking(move || run(&with_strict(registry(), args.strict), &args))
            .await
            .map_err(|e| McpError::internal_error(format!("analysis task failed: {e}"), None))?
            .map_err(|e| mcp_err(&e))?;
    let mut r = CallToolResult::structured(to_value(&out)?);
    if let Some(bytes) = png {
        r.content.insert(
            0,
            ContentBlock::image(
                base64::engine::general_purpose::STANDARD.encode(&bytes),
                "image/png",
            ),
        );
    }
    Ok(r)
}

use openreadout_quant::api::{ChromatogramQuery, PeaksQuery};
use openreadout_signal::api::{EphysQuery, NmrQuery, SpikesQuery};

#[tool_router(router = analyses_router, vis = "pub(super)")]
impl InstrumentServer {
    #[tool(
        name = "openreadout_peaks",
        annotations(title = "Chromatographic peaks", read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = object_output::<openreadout_quant::analyze::PeaksOutput>(),
        description = "Detect and integrate peaks: chromatograms (detector traces, TIC, XIC, SRM) with retention time, area, height, widths, tailing, plates, resolution, S/N and area % (purity); one peak near rt; a compound list; bands and regions of IR, Raman, UV-Vis and NMR spectra (x_range, integrate). Takes the openreadout_chromatogram arguments to choose the signal."
    )]
    pub(crate) async fn peaks(
        &self,
        Parameters(a): Parameters<AnalysisArgs<PeaksQuery>>,
    ) -> Result<CallToolResult, McpError> {
        call(self, AnalyzeKind::Peaks, a.file, a.strict, a.options).await
    }

    #[tool(
        name = "openreadout_chromatogram",
        annotations(title = "Chromatograms", read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = object_output::<openreadout_quant::extract::ChromatogramOutput>(),
        description = "Chromatograms from mass spectra and detector signals: TIC, BPC, extracted-ion (mz with ppm or da), SRM/MRM transitions, stored chromatograms and UV/DAD/FID/TCD traces (traces). Per chromatogram: apex time and intensity, integral, and thinned arrays. Answers 'when does m/z X elute'."
    )]
    pub(crate) async fn chromatogram(
        &self,
        Parameters(a): Parameters<AnalysisArgs<ChromatogramQuery>>,
    ) -> Result<CallToolResult, McpError> {
        call(self, AnalyzeKind::Chromatogram, a.file, a.strict, a.options).await
    }

    #[tool(
        name = "openreadout_nmr_peaks",
        annotations(title = "NMR peaks and integrals", read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = object_output::<openreadout_signal::nmr::NmrReport>(),
        description = "NMR peak list (ppm, height, width, S/N) and region integrals, from the vendor's processed spectrum or a spectrum processed from the FID (from=fid: group delay, apodization, zero filling, FT, phase, baseline)."
    )]
    pub(crate) async fn nmr_peaks(
        &self,
        Parameters(a): Parameters<AnalysisArgs<NmrQuery>>,
    ) -> Result<CallToolResult, McpError> {
        call(self, AnalyzeKind::NmrPeaks, a.file, a.strict, a.options).await
    }

    #[tool(
        name = "openreadout_ephys_features",
        annotations(title = "Patch-clamp features", read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = object_output::<openreadout_signal::ephys::CellReport>(),
        description = "Patch-clamp features: action potentials per sweep, rheobase, f-I slope, input resistance, membrane time constant, capacitance, sag; in voltage clamp, holding current and access resistance."
    )]
    pub(crate) async fn ephys_features(
        &self,
        Parameters(a): Parameters<AnalysisArgs<EphysQuery>>,
    ) -> Result<CallToolResult, McpError> {
        call(
            self,
            AnalyzeKind::EphysFeatures,
            a.file,
            a.strict,
            a.options,
        )
        .await
    }

    #[tool(
        name = "openreadout_spikes",
        annotations(title = "Extracellular spikes", read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = object_output::<openreadout_signal::ephys::SpikesReport>(),
        description = "Extracellular spike detection per channel: band-pass, threshold at K x MAD noise, spike counts, rates and times."
    )]
    pub(crate) async fn spikes(
        &self,
        Parameters(a): Parameters<AnalysisArgs<SpikesQuery>>,
    ) -> Result<CallToolResult, McpError> {
        call(self, AnalyzeKind::Spikes, a.file, a.strict, a.options).await
    }

    #[tool(
        name = "openreadout_qpcr",
        annotations(title = "qPCR results", read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = object_output::<openreadout_qpcr::QpcrReport>(),
        description = "Real-time PCR (RDML, .eds, .rex, ...): one record per well and target with sample, task, Cq and Tm; compute_cq recomputes Cq, ddcq gives 2^-ΔΔCq fold changes, standard_curve fits efficiency per target."
    )]
    pub(crate) async fn qpcr(
        &self,
        Parameters(a): Parameters<AnalysisArgs<openreadout_batch::analyze::QpcrOptions>>,
    ) -> Result<CallToolResult, McpError> {
        call(self, AnalyzeKind::Qpcr, a.file, a.strict, a.options).await
    }

    #[tool(
        name = "openreadout_gate",
        annotations(title = "Flow-cytometry gating", read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = object_output::<openreadout_core::flow::GateOutput>(),
        description = "Flow-cytometry gating from a FlowJo workspace (workspace) or Gating-ML 2.0 file (gatingml): the event count, percentage and medians of every population of the FCS file. Without workspace or gatingml, file is the gating file and the answer describes its samples and gate tree."
    )]
    pub(crate) async fn gate(
        &self,
        Parameters(a): Parameters<AnalysisArgs<openreadout_batch::analyze::GateOptions>>,
    ) -> Result<CallToolResult, McpError> {
        call(self, AnalyzeKind::Gate, a.file, a.strict, a.options).await
    }
}
