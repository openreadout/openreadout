//! `openreadout_stats`: pixel statistics per channel, plane, image or plate well.

use std::path::Path;

use openreadout_core::Error;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData as McpError, schemars, tool, tool_router};
use serde::{Deserialize, Serialize};

use super::object_output;
use crate::{InstrumentServer, mcp_err, ok_json};

/// Rows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum StatsPer {
    /// channel and image aggregates (default)
    #[default]
    Channel,
    /// also one entry per plane (image, c, z, t)
    Plane,
    /// a screening plate (pass the plate folder or index, never one TIFF): one row per well × channel over the well's fields
    Well,
    /// a screening plate: one row per well × field × channel
    Field,
}

/// Arguments for `openreadout_stats`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct StatsArgs {
    /// Absolute or working-directory-relative path to the instrument file, or a plate's index
    /// file or folder (Harmony `Index.idx.xml`, ImageXpress `.HTD`, CellVoyager
    /// `MeasurementData.mlf`, OME-Zarr plate).
    pub file: String,
    /// Rows.
    #[serde(default)]
    pub per: StatsPer,
    /// per=well|field: only these wells (`C05`, `c5`).
    #[serde(default)]
    pub wells: Vec<String>,
    /// Only this image index.
    pub image: Option<u32>,
    /// Plane selection strings such as `c=0`, `z=2-5`, `t=0,3`. The aggregates cover exactly the selection.
    #[serde(default)]
    pub select: Vec<String>,
    /// Pyramid level: 0 = full resolution (default).
    #[serde(default)]
    pub level: u32,
    /// Only this rectangle of each plane, `{x, y, width, height}` in the pixel coordinates of
    /// `level` (e.g. the 512 x 512 centre of a whole-slide image).
    pub region: Option<openreadout_core::Region>,
    /// Histogram bins (0 = none). Default 32, at most 65536.
    pub bins: Option<u32>,
    /// Histogram spacing: `linear` (default) or `log`.
    #[serde(default)]
    pub scale: openreadout_core::stats::HistogramScale,
    /// Maximum-intensity projection first: `z` (per image, channel and time point: the largest
    /// value of each pixel over the selected z planes) or `t`; the statistics are of the
    /// projections.
    pub mip: Option<openreadout_core::stats::Projection>,
    /// true: refuse (an error with exit_code 6 and a hint) values this file's assurance does not
    /// validate. Default: the server's setting (OPENREADOUT_STRICT; off).
    #[serde(default)]
    pub strict: Option<bool>,
}

/// Default histogram bins for `openreadout_stats` (smaller than the CLI's to keep answers short).
pub const MCP_STATS_BINS: u32 = 32;

/// What `openreadout_stats` returns.
#[derive(Debug, Serialize, schemars::JsonSchema)]
#[serde(untagged)]
#[allow(dead_code, clippy::large_enum_variant)]
enum StatsToolOutput {
    /// per=channel|plane.
    Images(openreadout_core::stats::StatsOutput),
    /// per=well|field.
    Wells(openreadout_core::plate::WellStatsOutput),
}

#[tool_router(router = stats_router, vis = "pub(super)")]
impl InstrumentServer {
    #[tool(
        name = "openreadout_stats",
        annotations(title = "Pixel statistics", read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = object_output::<StatsToolOutput>(),
        description = "Pixel statistics per channel and image: count, min, max, mean, std, percentiles p1–p99, zero and saturated fractions (at the detector's 2^bits−1 when recorded), histogram; RGB images also per component. per=plane adds every plane; per=well or field gives rows per well of a screening plate. mip='z' measures the maximum-intensity projection; whole slides: a coarser level and/or a region."
    )]
    pub(crate) fn stats(
        &self,
        Parameters(a): Parameters<StatsArgs>,
    ) -> Result<CallToolResult, McpError> {
        let reg = self.reg_for(a.strict);
        let path = Path::new(&a.file);
        let (_, mut ds) = reg.open(path).map_err(|e| mcp_err(&e))?;
        let info = ds.info().map_err(|e| mcp_err(&e))?;
        let opener = || -> openreadout_core::Result<Box<dyn openreadout_core::Dataset>> {
            reg.open(path).map(|(_, d)| d)
        };
        let ctx = openreadout_core::parallel::ReadContext {
            opener: Some(&opener),
            progress: None,
        };
        if matches!(a.per, StatsPer::Well | StatsPer::Field) {
            let mut req = openreadout_core::plate::WellStatsRequest::default();
            req.select = a.select;
            req.wells = a.wells;
            req.per_field = a.per == StatsPer::Field;
            req.level = a.level;
            let out = openreadout_core::plate::well_stats(ds.as_mut(), &info, &req, &ctx)
                .map_err(|e| mcp_err(&e))?;
            return ok_json(&out);
        }
        if !a.wells.is_empty() {
            return Err(mcp_err(&Error::Usage(
                "wells goes with per=well or per=field".into(),
            )));
        }
        let out = openreadout_core::stats::compute_stats(
            ds.as_mut(),
            &info,
            &{
                let mut stats_request = openreadout_core::stats::StatsRequest::default();
                stats_request.image = a.image;
                stats_request.select = a.select;
                stats_request.level = a.level;
                stats_request.region = a.region;
                stats_request.bins = a.bins.unwrap_or(MCP_STATS_BINS);
                stats_request.scale = a.scale;
                stats_request.per_plane = a.per == StatsPer::Plane;
                stats_request.mip = a.mip;
                stats_request
            },
            &ctx,
        )
        .map_err(|e| mcp_err(&e))?;
        ok_json(&out)
    }
}
