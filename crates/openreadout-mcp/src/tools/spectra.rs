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
    /// Path to the mass-spectrometry file.
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
    /// Refuse values the file's assurance does not validate (exit_code 6). Default: the server's.
    #[serde(default)]
    pub strict: Option<bool>,
}

/// Arguments for `openreadout_spectrum`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SpectrumArgs {
    /// Path to the mass-spectrometry file.
    pub file: String,
    /// Run index for files with several runs (Sciex samples; default 0).
    #[serde(default)]
    pub run: u32,
    /// The scan number as the instrument counts it (1-based in Thermo files; mzML `scan=N`
    /// native ids, mzXML `num`, timsTOF spectrum position + 1).
    pub scan: Option<u64>,
    /// The zero-based spectrum index, counted within ms_level when it is given.
    pub spectrum: Option<u64>,
    /// Count spectrum among the spectra of this MS level only (1 = full scans, 2 = MS/MS). MS1 and
    /// MS/MS scans interleave, and ms_level=2 with spectrum=0 is the first MS/MS scan.
    pub ms_level: Option<u32>,
    /// The stored centroid list instead of the profile when a scan has both.
    #[serde(default)]
    pub centroid: bool,
    /// Waters .raw: correct m/z with the lock-spray reference scans, given the lock compound's
    /// m/z (e.g. 556.2771), or "auto" for the Lock Mass the file records. Default: m/z as stored.
    pub lock_mass: Option<String>,
    /// The MS scan (of ms_level, default 1) nearest this retention time, minutes: the spectrum
    /// at a chromatogram's apex.
    pub rt: Option<f64>,
    /// Most intense peaks listed under peaks[] with charge and neutral mass (default 20).
    pub top_peaks: Option<usize>,
    /// At most this many points, the most intense kept (default 1000; point_count reports the
    /// full size).
    pub max_points: Option<usize>,
    /// Refuse values the file's assurance does not validate (exit_code 6). Default: the server's.
    #[serde(default)]
    pub strict: Option<bool>,
}

/// Most scans one `openreadout_scans` call lists.
const MAX_SCANS_LISTED: u64 = 5000;

#[tool_router(router = spectra_router, vis = "pub(super)")]
impl InstrumentServer {
    #[tool(
        name = "openreadout_scans",
        annotations(title = "List mass-spectrometry scans", read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
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
        annotations(title = "Read a mass spectrum", read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::common::schema_for_output::<SpectrumOutput>(),
        description = "One mass spectrum, picked by retention time (rt, minutes: the scan nearest it), scan number or zero-based spectrum index: metadata, peaks[] (the most intense, with charge from the isotope spacing and neutral mass) and mz[]/intensity[]."
    )]
    pub(crate) fn spectrum(
        &self,
        Parameters(a): Parameters<SpectrumArgs>,
    ) -> Result<CallToolResult, McpError> {
        use openreadout_core::SpectrumView;
        let reg = self.reg_for(a.strict);
        let (det, mut ds) = reg.open(Path::new(&a.file)).map_err(|e| mcp_err(&e))?;
        if let Some(lm) = a.lock_mass.as_deref() {
            let reference = if lm.eq_ignore_ascii_case("auto") {
                None
            } else {
                Some(lm.parse::<f64>().map_err(|_| {
                    mcp_err(&openreadout_core::Error::Usage(format!(
                        "lock_mass {lm:?}: expected the lock compound's m/z or \"auto\""
                    )))
                })?)
            };
            ds.set_lock_mass(reference).map_err(|e| mcp_err(&e))?;
        }
        let view = if a.centroid {
            SpectrumView::Centroid
        } else {
            SpectrumView::Primary
        };
        let rt_pick = match a.rt {
            Some(rt) => {
                if a.scan.is_some() || a.spectrum.is_some() {
                    return Err(mcp_err(&Error::Usage(
                        "give one of rt, scan and spectrum".into(),
                    )));
                }
                let level = a.ms_level.unwrap_or(1);
                let hit =
                    openreadout_core::scans::nearest_scan(ds.as_mut(), a.run, rt * 60.0, level)
                        .map_err(|e| mcp_err(&e))?
                        .ok_or_else(|| {
                            mcp_err(&Error::Usage(format!(
                                "run {} has no MS{level} scan with a retention time",
                                a.run
                            )))
                        })?;
                Some(hit.0)
            }
            None => None,
        };
        let mut sp = match (
            a.spectrum.or(rt_pick),
            a.ms_level.filter(|_| rt_pick.is_none()),
            a.scan,
        ) {
            (Some(i), Some(level), _) => {
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
                    i,
                    count,
                    view,
                )
            }
            (Some(i), None, _) => ds.read_spectrum_view(a.run, i, view),
            (None, _, Some(n)) => {
                openreadout_core::reader::spectrum_by_scan(ds.as_mut(), a.run, n, view)
            }
            (None, _, None) => Err(Error::Usage(
                "give rt (minutes), scan, or spectrum (with ms_level to count within one MS level)"
                    .into(),
            )),
        }
        .map_err(|e| mcp_err(&e))?;
        if a.spectrum.is_none() && a.scan.is_some_and(|n| n != sp.scan_number) {
            return Err(mcp_err(&Error::Usage(format!(
                "scan numbers are not contiguous (got scan {}); use `spectrum` (the zero-based index)",
                sp.scan_number
            ))));
        }
        let point_count = sp.mz.len() as u64;
        let peaks = openreadout_core::scans::top_peaks(&sp, a.top_peaks.unwrap_or(20));
        let cap = a.max_points.unwrap_or(1000);
        let truncated = sp.mz.len() > cap;
        openreadout_core::scans::keep_most_intense(&mut sp, cap);
        ok_json(&SpectrumOutput {
            path: a.file.clone(),
            format: det.format_id.into(),
            run: a.run,
            view: if a.centroid { "centroid" } else { "primary" }.into(),
            point_count,
            truncated,
            peaks,
            spectrum: sp,
        })
    }
}
