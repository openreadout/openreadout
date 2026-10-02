//! `openreadout_trace`: one window of one sweep of a sampled signal or 1-D spectrum.

use std::path::Path;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData as McpError, schemars, tool, tool_router};
use serde::Deserialize;

use crate::{InstrumentServer, mcp_err, ok_json};

/// Arguments for `openreadout_trace`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TraceArgs {
    /// Absolute or working-directory-relative path to the instrument file.
    pub file: String,
    /// Trace index (see openreadout_info → traces[]). Default 0.
    #[serde(default)]
    pub trace: u32,
    /// Sweep (episode or segment) index. Default 0.
    #[serde(default)]
    pub sweep: u32,
    /// Channel indices to return. Default: all channels.
    #[serde(default)]
    pub channels: Vec<u32>,
    /// First sample of the window, zero-based within the sweep. Default 0.
    #[serde(default)]
    pub first_sample: u64,
    /// Window length in samples (statistics cover the whole window). Default: to the end of the sweep.
    pub count: Option<u64>,
    /// Window on the trace's own axis instead of first_sample/count, `[a, b]` either order: cm⁻¹,
    /// nm, ppm, a chromatogram's retention time in its axis unit; seconds for signals without an axis.
    pub x_range: Option<[f64; 2]>,
    /// Samples returned per channel. Default 200, capped at 10000.
    pub max_samples: Option<u64>,
    /// NMR: return FID traces as spectra processed by OpenReadout (group delay, apodization,
    /// zero filling, FT, stored or automatic phase, baseline, ppm axis).
    #[serde(default)]
    pub process: bool,
    /// true: refuse (an error with exit_code 6 and a hint) values this file's assurance does not
    /// validate. Default: the server's setting (OPENREADOUT_STRICT; off).
    #[serde(default)]
    pub strict: Option<bool>,
}

/// Most samples per channel `openreadout_trace` returns in one call.
pub const MAX_TRACE_SAMPLES: u64 = 10_000;

#[tool_router(router = trace_router, vis = "pub(super)")]
impl InstrumentServer {
    #[tool(
        name = "openreadout_trace",
        annotations(title = "Read a signal sweep", read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::common::schema_for_output::<openreadout_core::trace::TraceSlice>(),
        description = "One window of one sweep of a sampled signal or 1-D spectrum in physical units: electrophysiology sweeps, chromatography detector traces, ÄKTA curves, ITC thermograms, SPR sensorgrams, NMR FIDs and spectra (process=true turns an FID into a spectrum), IR/Raman/UV-Vis/CD spectra (each spectrum of a map is a sweep), qPCR curves, EPR, XRD, electrochemistry, thermal analysis. Per channel: min, max, mean, std and argmax_axis_value (the maximum's position on the axis: retention time, ppm, cm⁻¹, °2θ) over the window, plus the first max_samples values."
    )]
    pub(crate) fn trace(
        &self,
        Parameters(a): Parameters<TraceArgs>,
    ) -> Result<CallToolResult, McpError> {
        let reg = self.reg_for(a.strict);
        let (_, ds) = reg.open(Path::new(&a.file)).map_err(|e| mcp_err(&e))?;
        let mut ds: Box<dyn openreadout_core::Dataset> = if a.process {
            Box::new(
                openreadout_signal::nmr::ProcessedNmrDataset::new(
                    ds,
                    openreadout_signal::nmr::ProcessOptions::default(),
                )
                .map_err(|e| mcp_err(&e))?,
            )
        } else {
            ds
        };
        let info = ds.info().map_err(|e| mcp_err(&e))?;
        let slice = openreadout_core::trace::slice_trace(ds.as_mut(), &info, &{
            let mut trace_request = openreadout_core::trace::TraceRequest::default();
            trace_request.trace = a.trace;
            trace_request.sweep = a.sweep;
            trace_request.channels = a.channels;
            trace_request.first_sample = a.first_sample;
            trace_request.count = a.count;
            trace_request.x_range = a.x_range;
            trace_request.max_samples = a.max_samples.unwrap_or(200).min(MAX_TRACE_SAMPLES);
            trace_request
        })
        .map_err(|e| mcp_err(&e))?;
        ok_json(&slice)
    }
}
