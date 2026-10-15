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
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewArgs {
    /// Path to the file.
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
    /// Encoding. Default png (sent as JPEG when a PNG would be too large); svg also returns
    /// the vector drawing as text.
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
    /// Plate or flow-cytometry preview: table index.
    pub table: Option<u32>,
    /// Table columns: a plate's value column, or the x and y parameters of a flow-cytometry
    /// density plot (one gives a histogram; default FSC-A, SSC-A).
    #[serde(default)]
    pub columns: Vec<String>,
    /// Marks from the data. Default auto: the largest peaks of spectra and chromatograms are
    /// labelled.
    pub marks: Option<openreadout_preview::Marks>,
    /// Your own marks, drawn on top and reported back with their pixel positions. Coordinates
    /// as the picture shows them: full-resolution pixels on images (the ruler numbers), axis
    /// units on plots, wells on plates.
    #[serde(default)]
    pub annotations: Vec<openreadout_preview::Annotation>,
    /// Draw in the dark theme.
    #[serde(default)]
    pub dark: bool,
    /// Refuse values the file's assurance does not validate (exit_code 6). Default: the server's.
    #[serde(default)]
    pub strict: Option<bool>,
}

/// Default `max_size` of `openreadout_preview`.
pub const DEFAULT_MCP_PREVIEW_SIZE: u32 = 768;
/// Largest `max_size` `openreadout_preview` accepts (larger requests are clamped).
pub const MAX_MCP_PREVIEW_SIZE: u32 = 2048;

/// The library request of `a`, with `max_size` at most `max_size_cap`.
fn request(
    a: &PreviewArgs,
    max_size_cap: u32,
) -> Result<openreadout_preview::PreviewRequest, McpError> {
    use openreadout_preview as pv;
    let mut r = pv::PreviewRequest::default();
    r.image = a.image;
    r.select = a.select.clone();
    r.mip = a.mip;
    r.composite = a.composite;
    r.level = a.level;
    r.region = a.region;
    r.max_size = a
        .max_size
        .unwrap_or(DEFAULT_MCP_PREVIEW_SIZE)
        .min(max_size_cap);
    r.contrast = a
        .contrast
        .as_deref()
        .unwrap_or("auto")
        .parse::<pv::Contrast>()
        .map_err(|e| mcp_err(&e))?;
    r.lut = a.lut;
    r.trace = a.trace;
    r.sweep = a.sweep;
    r.channels = a.channels.clone();
    r.run = a.run;
    r.spectrum = a.spectrum;
    r.scan = a.scan;
    r.centroid = a.centroid;
    r.table = a.table;
    r.columns = a.columns.clone();
    r.marks = a.marks.unwrap_or_default();
    r.annotations = a.annotations.clone();
    r.dark = a.dark;
    a.axes.apply(&mut r);
    Ok(r)
}

/// Render the preview `a` asks for, with `max_size` at most `max_size_cap`.
pub(crate) fn render(
    registry: fn() -> Registry,
    a: &PreviewArgs,
    max_size_cap: u32,
) -> Result<openreadout_preview::Rendered, McpError> {
    let req = request(a, max_size_cap)?;
    let reg = with_strict(registry(), a.strict);
    let (_, mut ds) = reg.open(Path::new(&a.file)).map_err(|e| mcp_err(&e))?;
    let info = ds.info().map_err(|e| mcp_err(&e))?;
    openreadout_preview::render(ds.as_mut(), &info, &req).map_err(|e| mcp_err(&e))
}

/// Where `openreadout_export` saves a preview of `file` by default: `<stem>.preview.png` next
/// to it.
pub(crate) fn default_figure_path(file: &str) -> String {
    openreadout_preview::default_output(Path::new(file), openreadout_preview::Encoding::Png)
        .display()
        .to_string()
}

/// Render a preview and encode it within [`resources::PREVIEW_BUDGET`] bytes.
fn preview_blocking(
    registry: fn() -> Registry,
    a: &PreviewArgs,
) -> Result<(openreadout_preview::PreviewOutput, Vec<u8>, Option<String>), McpError> {
    use openreadout_preview as pv;
    let r = render(registry, a, MAX_MCP_PREVIEW_SIZE)?;
    // SVG goes back as text next to a PNG picture
    let encoding = match a.format.unwrap_or_default() {
        pv::Encoding::Svg => pv::Encoding::Png,
        e => e,
    };
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
    out.save = Some(crate::tools::export::save_hint(
        "preview",
        &default_figure_path(&a.file),
    ));
    let svg = (a.format == Some(pv::Encoding::Svg)).then(|| r.figure.to_svg());
    Ok((out, bytes, svg))
}

#[tool_router(router = preview_router, vis = "pub(super)")]
impl InstrumentServer {
    #[tool(
        name = "openreadout_preview",
        annotations(title = "Draw a picture of the data", read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = crate::tools::output_schema::<openreadout_preview::PreviewOutput>(),
        description = "A picture of the data as image content plus what was drawn: an image plane (default channel 0, middle z), a composite, a max projection (mip=z), a region (only the tiles needed are read), a trace or spectrum plot (largest peaks labelled; marks=spikes marks action potentials), a plate heat map or, for kinetic reads, per-well growth curves coloured by layout group, or a flow-cytometry density plot. Image pictures carry rulers in full-resolution pixels and a µm scale bar, so a region read off the rulers zooms in. annotations draw your own labelled points, boxes, outlines, arrows, lines and bands in the picture's coordinates, to show the user what you found. Draws only; openreadout_export saves the picture."
    )]
    pub(crate) async fn preview(
        &self,
        Parameters(a): Parameters<PreviewArgs>,
    ) -> Result<CallToolResult, McpError> {
        let registry = self.registry;
        let (out, bytes, svg) = tokio::task::spawn_blocking(move || preview_blocking(registry, &a))
            .await
            .map_err(|e| McpError::internal_error(format!("preview task failed: {e}"), None))??;
        let value = to_value(&out)?;
        let mime = if out.encoding == "jpeg" {
            "image/jpeg"
        } else {
            "image/png"
        };
        let mut r = CallToolResult::structured(value);
        if let Some(svg) = svg {
            r.content.push(ContentBlock::text(svg));
        }
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
