//! `openreadout_stats`: pixel statistics per channel, plane, image or plate well.

use std::path::Path;

use openreadout_core::Error;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData as McpError, schemars, tool, tool_router};
use serde::{Deserialize, Serialize};

use super::output_schema;
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
    /// Intensity profile along the line `[x0, y0, x1, y1]` (pixels of `level`, as on the
    /// preview rulers): values per channel, peak position and FWHM in px and µm.
    pub line: Option<[f64; 4]>,
    /// Profile width in pixels, averaged across the line. Default 1.
    pub line_width: Option<u32>,
    /// Statistics inside this polygon `[[x, y], ...]` (pixels of `level`): area, mean, std,
    /// median, sum per channel.
    pub polygon: Option<Vec<[f64; 2]>>,
    /// Count objects in `region`, `polygon` or the whole plane: count, density, sizes,
    /// centroids, each object's mean in every channel and how many are positive per channel.
    /// `{}` counts nuclei with settings taken from the image (size, threshold, H&E colour
    /// deconvolution). Found in a DNA-stain channel unless `select` names one.
    pub objects: Option<openreadout_core::measure::ObjectsRequest>,
    /// Lanes and bands of a gel or blot in `region` or the whole plane: each lane's total and
    /// each band's row and background-subtracted volume (with `ladder`, its size). `{}` finds
    /// the lanes; preview marks=lanes draws them.
    pub lanes: Option<openreadout_core::measure::LanesRequest>,
    /// Refuse values the file's assurance does not validate (exit_code 6). Default: the server's.
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
    /// line, polygon, objects or lanes.
    Measure(openreadout_core::measure::MeasureOutput),
}

#[tool_router(router = stats_router, vis = "pub(super)")]
impl InstrumentServer {
    #[tool(
        name = "openreadout_stats",
        annotations(title = "Measure pixels", read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = output_schema::<StatsToolOutput>(),
        description = "Pixel statistics per channel and image: count, min, max, mean, std, percentiles p1–p99, zero and saturated fractions (at the detector's 2^bits−1 when recorded), histogram; RGB images also per component. per=plane adds every plane; per=well or field gives rows per well of a screening plate. mip='z' measures the maximum-intensity projection; whole slides: a coarser level and/or a region. Measurements on one plane: line (intensity profile, FWHM), polygon (statistics inside), objects (count nuclei, cells or spots; preview marks=objects shows them), lanes (gel and blot lanes, band volumes and sizes; preview marks=lanes)."
    )]
    pub(crate) async fn stats(
        &self,
        Parameters(a): Parameters<StatsArgs>,
        rc: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let reg = self.reg_for(a.strict);
        let (notifier, forward) = crate::progress::Notifier::for_call(&rc);
        let r = tokio::task::spawn_blocking(move || {
            let report = notifier.map(|n| n.shared("Reading", "planes"));
            stats_blocking(
                &reg,
                a,
                report
                    .as_deref()
                    .map(|r| r as &openreadout_core::parallel::Progress<'_>),
            )
        })
        .await
        .map_err(|e| McpError::internal_error(format!("stats task failed: {e}"), None))?;
        forward.finish().await;
        r
    }
}

/// Refuse arguments a measurement (line, polygon, objects) would ignore: `ignored` names the
/// pixel-statistics arguments that are set. A region bounds objects only, and `line_width`
/// needs a line.
pub(crate) fn check_measure_args(
    m: &openreadout_core::measure::MeasureRequest,
    ignored: &[(&str, bool)],
    line_width: bool,
) -> openreadout_core::Result<()> {
    if !m.any() {
        if line_width {
            return Err(Error::Usage(
                "line_width is the width of a line profile: give line as well".into(),
            ));
        }
        return Ok(());
    }
    let set: Vec<&str> = ignored.iter().filter(|x| x.1).map(|x| x.0).collect();
    if !set.is_empty() {
        return Err(Error::Usage(format!(
            "{} apply to pixel statistics, not to line, polygon or objects, which measure one plane: pick the plane with select (`z=4`, `c=1`), or call again without line, polygon and objects",
            set.join(", ")
        )));
    }
    if m.region.is_some() && m.objects.is_none() && m.lanes.is_none() {
        return Err(Error::Usage(
            "region bounds an objects count or lanes; a line or polygon is given in full-plane pixels of level, so drop region".into(),
        ));
    }
    if line_width && m.line.is_none() {
        return Err(Error::Usage(
            "line_width is the width of a line profile: give line as well".into(),
        ));
    }
    Ok(())
}

/// `openreadout_stats` (blocking), reporting planes read to `report`.
fn stats_blocking(
    reg: &openreadout_core::Registry,
    a: StatsArgs,
    report: Option<&openreadout_core::parallel::Progress<'_>>,
) -> Result<CallToolResult, McpError> {
    let path = Path::new(&a.file);
    let (_, mut ds) = reg.open(path).map_err(|e| mcp_err(&e))?;
    let info = ds.info().map_err(|e| mcp_err(&e))?;
    let opener = || -> openreadout_core::Result<Box<dyn openreadout_core::Dataset>> {
        reg.open(path).map(|(_, d)| d)
    };
    let ctx = openreadout_core::parallel::ReadContext {
        opener: Some(&opener),
        progress: report,
    };
    let measure = openreadout_core::measure::MeasureRequest {
        image: a.image,
        select: a.select.clone(),
        level: a.level,
        region: a.region,
        line: a.line,
        line_width: a.line_width,
        polygon: a.polygon.clone(),
        objects: a.objects.clone(),
        lanes: a.lanes.clone(),
    };
    let ignored = [
        ("mip", a.mip.is_some()),
        ("per", a.per != StatsPer::Channel),
        ("bins", a.bins.is_some()),
        (
            "scale",
            a.scale != openreadout_core::stats::HistogramScale::Linear,
        ),
        ("wells", !a.wells.is_empty()),
    ];
    check_measure_args(&measure, &ignored, a.line_width.is_some()).map_err(|e| mcp_err(&e))?;
    if measure.any() {
        let out = openreadout_core::measure::measure(ds.as_mut(), &info, &measure)
            .map_err(|e| mcp_err(&e))?;
        return ok_json(&out);
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use openreadout_core::measure::MeasureRequest;

    /// Arguments a measurement would ignore are refused as usage errors.
    #[test]
    fn measurements_refuse_arguments_they_ignore() {
        let line = MeasureRequest {
            line: Some([0.0, 0.0, 10.0, 10.0]),
            ..MeasureRequest::default()
        };
        let e = check_measure_args(&line, &[("mip", true), ("bins", false)], false).unwrap_err();
        assert_eq!(e.exit_code(), 2);
        assert!(e.to_string().contains("mip apply"), "{e}");
        check_measure_args(&line, &[("mip", false)], true).unwrap();
        let region = MeasureRequest {
            region: Some(openreadout_core::Region::new(0, 0, 4, 4)),
            ..line.clone()
        };
        assert!(check_measure_args(&region, &[], false).is_err());
        assert!(check_measure_args(&MeasureRequest::default(), &[], true).is_err());
        check_measure_args(&MeasureRequest::default(), &[("mip", true)], false).unwrap();
    }
}
