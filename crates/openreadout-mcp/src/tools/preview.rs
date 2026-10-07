//! `openreadout_preview`: a picture of the data as image content.

use std::path::Path;

use base64::Engine as _;
use openreadout_core::Registry;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::{ErrorData as McpError, schemars, tool, tool_router};
use serde::Deserialize;

use crate::{InstrumentServer, mcp_err, resources, to_value, with_strict};

/// Arguments for `openreadout_preview`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PreviewArgs {
    /// Absolute or working-directory-relative path to the instrument file.
    pub file: String,
    /// Image index (default 0).
    pub image: Option<u32>,
    /// Plane selection such as `c=1`, `z=4`, `t=0` or combined `c=0,1,z=2`. Default: c=0, the
    /// middle z, t=0. Several channels imply a composite.
    #[serde(default)]
    pub select: Vec<String>,
    /// Maximum-intensity projection, over the selected range if any.
    pub mip: Option<openreadout_preview::Axis>,
    /// Blend all (or the selected) channels additively in their colours.
    #[serde(default)]
    pub composite: bool,
    /// Pyramid level to read (default: the level nearest max_size, or the level at which
    /// `region` renders at about max_size).
    pub level: Option<u32>,
    /// Zoom: only this rectangle, `{x, y, width, height}` in full-resolution pixels (or in the
    /// pixels of `level` when a level is given). Whole-slide images: look at the overview
    /// first, then at regions of it.
    pub region: Option<openreadout_core::Region>,
    /// Longest side in pixels, rulers included (default 768, max 2048).
    pub max_size: Option<u32>,
    /// Image previews: what frames the picture.
    #[serde(default)]
    pub axes: openreadout_preview::Axes,
    /// `auto` (default), `min-max`, `percentile:LO,HI` (e.g. `percentile:1,99`) or `raw`.
    pub contrast: Option<String>,
    /// Colour lookup. Default: gray for one channel, channel colours for composites.
    pub lut: Option<openreadout_preview::Lut>,
    /// Encoding. Default png (sent as JPEG when a PNG would be too large).
    pub format: Option<openreadout_preview::Encoding>,
    /// Trace preview: trace index (see openreadout_info traces[]).
    pub trace: Option<u32>,
    /// Trace preview: sweep index.
    pub sweep: Option<u32>,
    /// Trace preview: channel indices (default: the first 8).
    #[serde(default)]
    pub channels: Vec<u32>,
    /// Spectrum preview: run index.
    pub run: Option<u32>,
    /// Spectrum preview: zero-based spectrum index.
    pub spectrum: Option<u64>,
    /// Spectrum preview: instrument scan number instead of an index.
    pub scan: Option<u64>,
    /// Spectrum preview: the instrument's centroid list instead of the profile.
    #[serde(default)]
    pub centroid: bool,
    /// Plate preview: table index.
    pub table: Option<u32>,
    /// Plate preview (long layout): value column name.
    pub column: Option<String>,
    /// true: refuse (an error with exit_code 6 and a hint) values this file's assurance does not
    /// validate. Default: the server's setting (OPENREADOUT_STRICT; off).
    #[serde(default)]
    pub strict: Option<bool>,
}

/// Default `max_size` of `openreadout_preview`.
pub const DEFAULT_MCP_PREVIEW_SIZE: u32 = 768;
/// Largest `max_size` `openreadout_preview` accepts (larger requests are clamped).
pub const MAX_MCP_PREVIEW_SIZE: u32 = 2048;

/// Render a preview and encode it within [`resources::PREVIEW_BUDGET`] bytes.
fn preview_blocking(
    registry: fn() -> Registry,
    a: &PreviewArgs,
) -> Result<(openreadout_preview::PreviewOutput, Vec<u8>), McpError> {
    use openreadout_preview as pv;
    fn parse<T>(r: openreadout_core::Result<T>) -> Result<T, McpError> {
        r.map_err(|e| mcp_err(&e))
    }
    let req = {
        let mut preview_request = pv::PreviewRequest::default();
        preview_request.image = a.image;
        preview_request.select = a.select.clone();
        preview_request.mip = a.mip;
        preview_request.composite = a.composite;
        preview_request.level = a.level;
        preview_request.region = a.region;
        preview_request.max_size = a
            .max_size
            .unwrap_or(DEFAULT_MCP_PREVIEW_SIZE)
            .min(MAX_MCP_PREVIEW_SIZE);
        preview_request.contrast = parse(
            a.contrast
                .as_deref()
                .unwrap_or("auto")
                .parse::<pv::Contrast>(),
        )?;
        preview_request.lut = a.lut;
        preview_request.trace = a.trace;
        preview_request.sweep = a.sweep;
        preview_request.channels = a.channels.clone();
        preview_request.run = a.run;
        preview_request.spectrum = a.spectrum;
        preview_request.scan = a.scan;
        preview_request.centroid = a.centroid;
        preview_request.table = a.table;
        preview_request.column = a.column.clone();
        a.axes.apply(&mut preview_request);
        preview_request
    };
    let encoding = a.format.unwrap_or_default();
    let reg = with_strict(registry(), a.strict);
    let (_, mut ds) = reg.open(Path::new(&a.file)).map_err(|e| mcp_err(&e))?;
    let info = ds.info().map_err(|e| mcp_err(&e))?;
    let r = pv::render(ds.as_mut(), &info, &req).map_err(|e| mcp_err(&e))?;
    let (mut out, bytes) = pv::finish_within(
        &r,
        encoding,
        pv::DEFAULT_JPEG_QUALITY,
        resources::PREVIEW_BUDGET,
    )
    .map_err(|e| mcp_err(&e))?;
    if let Some(im) = &out.image {
        out.hint = Some(format!(
            "showing full-res region {}; zoom: call again with region {{x, y, width, height}} in full-res px read off the rulers; numbers come from openreadout_stats",
            im.full_res_region
        ));
    }
    Ok((out, bytes))
}

#[tool_router(router = preview_router, vis = "pub(super)")]
impl InstrumentServer {
    #[tool(
        name = "openreadout_preview",
        annotations(title = "Look at the data", read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::common::schema_for_output::<openreadout_preview::PreviewOutput>(),
        description = "A picture of the data as image content plus what was drawn: an image plane (default channel 0, middle z), a composite, a max projection (mip=z), a region (only the tiles needed are read), a trace or spectrum plot, or a plate heat map. Image pictures carry rulers in full-resolution pixels and a µm scale bar, so a region read off the rulers zooms in."
    )]
    pub(crate) async fn preview(
        &self,
        Parameters(a): Parameters<PreviewArgs>,
    ) -> Result<CallToolResult, McpError> {
        let registry = self.registry;
        let (out, bytes) = tokio::task::spawn_blocking(move || preview_blocking(registry, &a))
            .await
            .map_err(|e| McpError::internal_error(format!("preview task failed: {e}"), None))??;
        let value = to_value(&out)?;
        let mime = if out.encoding == "jpeg" {
            "image/jpeg"
        } else {
            "image/png"
        };
        let mut r = CallToolResult::structured(value);
        r.content.insert(
            0,
            ContentBlock::image(
                base64::engine::general_purpose::STANDARD.encode(&bytes),
                mime,
            ),
        );
        Ok(r)
    }
}
