//! Mass spectrometry: `openreadout_scans` (the scan headers of a run) and
//! `openreadout_spectrum` (one spectrum).

use std::path::Path;

use openreadout_core::Error;
use openreadout_core::model::SpectrumOutput;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData as McpError, schemars, tool, tool_router};
use serde::Deserialize;

use crate::{InstrumentServer, mcp_err, ok_json};

/// Arguments for `openreadout_scans`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ScansArgs {
    /// Absolute or working-directory-relative path to the mass-spectrometry file.
    pub file: String,
    /// Run index for files with several runs (Sciex samples; default 0).
    #[serde(default)]
    pub run: u32,
    /// Only scans of this MS level (1 = full scans, 2 = MS/MS).
    pub ms_level: Option<u32>,
    /// Only `positive` or `negative` scans.
    pub polarity: Option<String>,
    /// Retention-time window `[start, end]` in minutes.
    pub rt_range: Option<[f64; 2]>,
    /// Only MS/MS scans whose precursor m/z is within the tolerance of this.
    pub precursor: Option<f64>,
    /// Precursor tolerance in m/z units (default 0.01).
    pub precursor_tol: Option<f64>,
    /// Precursor tolerance in ppm instead.
    pub precursor_ppm: Option<f64>,
    /// Only precursors of this charge state.
    pub charge: Option<i32>,
    /// Only this activation: HCD, CID, ETD, ... (case-insensitive).
    pub activation: Option<String>,
    /// Only scans whose filter string contains this text (case-insensitive).
    pub scan_filter: Option<String>,
    /// Skip this many matching scans (paging; default 0).
    #[serde(default)]
    pub offset: u64,
    /// List at most this many matching scans (default 100, at most 5000); all are counted.
    pub limit: Option<u64>,
    /// Only count the matching scans (matched, ms_level_counts); list none.
    #[serde(default)]
    pub count: bool,
    /// true: refuse (an error with exit_code 6 and a hint) values this file's assurance does not
    /// validate. Default: the server's setting (OPENREADOUT_STRICT; off).
    #[serde(default)]
    pub strict: Option<bool>,
}

/// Arguments for `openreadout_spectrum`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SpectrumArgs {
    /// Absolute or working-directory-relative path to the mass-spectrometry file.
    pub file: String,
    /// Run index for files with several runs (Sciex samples; default 0).
    #[serde(default)]
    pub run: u32,
    /// The scan number as the instrument counts it (1-based in Thermo files; mzML `scan=N`
    /// native ids, mzXML `num`, timsTOF spectrum position + 1).
    pub scan: Option<u64>,
    /// The zero-based spectrum index.
    pub spectrum: Option<u64>,
    /// With ms_level: the nth spectrum of that level, counting from 1 (ms_level=2, nth=1 = the
    /// first MS/MS scan; MS1 and MS/MS scans interleave).
    pub nth: Option<u64>,
    /// The MS level nth counts in (1 = full scans, 2 = MS/MS).
    pub ms_level: Option<u32>,
    /// The stored centroid list instead of the profile when a scan has both.
    #[serde(default)]
    pub centroid: bool,
    /// At most this many points (default 2000; point_count reports the full size).
    pub max_points: Option<usize>,
    /// true: refuse (an error with exit_code 6 and a hint) values this file's assurance does not
    /// validate. Default: the server's setting (OPENREADOUT_STRICT; off).
    #[serde(default)]
    pub strict: Option<bool>,
}

/// Most scans one `openreadout_scans` call lists.
const MAX_SCANS_LISTED: u64 = 5000;

#[tool_router(router = spectra_router, vis = "pub(super)")]
impl InstrumentServer {
    #[tool(
        name = "openreadout_scans",
        annotations(title = "Mass-spectrometry scans", read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::common::schema_for_output::<openreadout_core::ScanList>(),
        description = "The scans of a mass-spectrometry run, without decoding peaks: scan number, MS level, retention time, polarity, precursor m/z and charge, isolation window, activation, collision energy, filter, stored TIC; filtered and paged, and every match counted (matched, ms_level_counts). IR, Raman, UV-Vis and NMR spectra are traces: openreadout_trace."
    )]
    pub(crate) fn scans(
        &self,
        Parameters(a): Parameters<ScansArgs>,
    ) -> Result<CallToolResult, McpError> {
        let reg = self.reg_for(a.strict);
        let filter = openreadout_core::ScanFilter {
            ms_level: a.ms_level,
            polarity: a.polarity.map(|p| p.to_ascii_lowercase()),
            rt_min_s: a.rt_range.map(|r| r[0] * 60.0),
            rt_max_s: a.rt_range.map(|r| r[1] * 60.0),
            precursor_mz: a.precursor,
            precursor_tol_mz: a.precursor_tol,
            precursor_tol_ppm: a.precursor_ppm,
            charge: a.charge,
            activation: a.activation,
            filter_contains: a.scan_filter,
        };
        filter.validate().map_err(|e| mcp_err(&e))?;
        let (det, mut ds) = reg.open(Path::new(&a.file)).map_err(|e| mcp_err(&e))?;
        let limit = if a.count {
            0
        } else {
            a.limit.unwrap_or(100).min(MAX_SCANS_LISTED)
        };
        let list = openreadout_core::scans::scan_list(
            ds.as_mut(),
            &a.file,
            det.format_id,
            a.run,
            &filter,
            a.offset,
            limit,
        )
        .map_err(|e| mcp_err(&e))?;
        ok_json(&list)
    }

    #[tool(
        name = "openreadout_spectrum",
        annotations(title = "One mass spectrum", read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::common::schema_for_output::<SpectrumOutput>(),
        description = "One mass spectrum's mz[] and intensity[] with its metadata, picked by scan number, by zero-based spectrum index, or as the nth spectrum of an MS level."
    )]
    pub(crate) fn spectrum(
        &self,
        Parameters(a): Parameters<SpectrumArgs>,
    ) -> Result<CallToolResult, McpError> {
        use openreadout_core::SpectrumView;
        let reg = self.reg_for(a.strict);
        let (det, mut ds) = reg.open(Path::new(&a.file)).map_err(|e| mcp_err(&e))?;
        let view = if a.centroid {
            SpectrumView::Centroid
        } else {
            SpectrumView::Primary
        };
        let mut sp = match (a.nth, a.scan, a.spectrum) {
            (Some(nth), _, _) => {
                let level = a.ms_level.ok_or_else(|| {
                    mcp_err(&Error::Usage(
                        "nth counts within ms_level: give ms_level too (2 = MS/MS)".into(),
                    ))
                })?;
                let count = ds
                    .info()
                    .map_err(|e| mcp_err(&e))?
                    .spectra
                    .iter()
                    .find(|s| s.index == a.run)
                    .map_or(0, |s| s.scan_count);
                openreadout_core::reader::spectrum_by_level(
                    ds.as_mut(),
                    a.run,
                    level,
                    nth,
                    count,
                    view,
                )
            }
            (None, _, Some(i)) => ds.read_spectrum_view(a.run, i, view),
            (None, Some(n), None) => {
                openreadout_core::reader::spectrum_by_scan(ds.as_mut(), a.run, n, view)
            }
            (None, None, None) => Err(Error::Usage(
                "give scan, spectrum, or ms_level with nth".into(),
            )),
        }
        .map_err(|e| mcp_err(&e))?;
        if a.nth.is_none() && a.scan.is_some_and(|n| n != sp.scan_number) {
            return Err(mcp_err(&Error::Usage(format!(
                "scan numbers are not contiguous (got scan {}); use `spectrum` (the zero-based index)",
                sp.scan_number
            ))));
        }
        let point_count = sp.mz.len() as u64;
        let cap = a.max_points.unwrap_or(2000);
        let truncated = sp.mz.len() > cap;
        sp.mz.truncate(cap);
        sp.intensity.truncate(cap);
        ok_json(&SpectrumOutput {
            path: a.file.clone(),
            format: det.format_id.into(),
            run: a.run,
            view: if a.centroid { "centroid" } else { "primary" }.into(),
            point_count,
            truncated,
            spectrum: sp,
        })
    }
}
