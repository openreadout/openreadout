//! `openreadout_preview`: a picture of the data as image content.

use std::path::Path;

use base64::Engine as _;
use openreadout_core::Registry;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::{ErrorData as McpError, schemars, tool, tool_router};
use serde::Deserialize;

use crate::{InstrumentServer, mcp_err, resources, to_value, with_strict};

/// Arguments for `openreadout_preview`: the file, the encoding, and the preview options the
/// bindings share ([`openreadout_preview::PreviewOptions`]).
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PreviewArgs {
    /// Path to the file.
    pub file: String,
    /// Encoding. Default png (sent as JPEG when a PNG would be too large); svg also returns
    /// the vector drawing as text.
    pub format: Option<openreadout_preview::Encoding>,
    /// Refuse values the file's assurance does not validate (exit_code 6). Default: the server's.
    #[serde(default)]
    pub strict: Option<bool>,
    /// What to draw and how.
    #[serde(flatten)]
    pub options: openreadout_preview::PreviewOptions,
}

/// Default `max_size` of `openreadout_preview`.
pub const DEFAULT_MCP_PREVIEW_SIZE: u32 = 768;
/// Largest `max_size` `openreadout_preview` accepts (larger requests are clamped).
pub const MAX_MCP_PREVIEW_SIZE: u32 = 2048;

/// Render a preview and encode it within [`resources::PREVIEW_BUDGET`] bytes.
fn preview_blocking(
    registry: fn() -> Registry,
    a: &PreviewArgs,
) -> Result<(openreadout_preview::PreviewOutput, Vec<u8>, Option<String>), McpError> {
    use openreadout_preview as pv;
    fn parse<T>(r: openreadout_core::Result<T>) -> Result<T, McpError> {
        r.map_err(|e| mcp_err(&e))
    }
    let mut req = parse(
        a.options
            .request(DEFAULT_MCP_PREVIEW_SIZE, pv::Axes::Rulers),
    )?;
    req.max_size = req.max_size.min(MAX_MCP_PREVIEW_SIZE);
    // SVG goes back as text next to a PNG picture
    let encoding = match a.format.unwrap_or_default() {
        pv::Encoding::Svg => pv::Encoding::Png,
        e => e,
    };
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
    let svg = (a.format == Some(pv::Encoding::Svg)).then(|| r.figure.to_svg());
    Ok((out, bytes, svg))
}

#[tool_router(router = preview_router, vis = "pub(super)")]
impl InstrumentServer {
    #[tool(
        name = "openreadout_preview",
        annotations(title = "Draw a picture of the data", read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::common::schema_for_output::<openreadout_preview::PreviewOutput>(),
        description = "A picture of the data as image content plus what was drawn: an image plane (default channel 0, middle z), a composite, a max projection (mip=z), a region (only the tiles needed are read), a trace or spectrum plot (largest peaks labelled; marks=spikes marks action potentials), a plate heat map or, for kinetic reads, per-well growth curves coloured by layout group, or a flow-cytometry density plot. Image pictures carry rulers in full-resolution pixels and a µm scale bar, so a region read off the rulers zooms in. annotations draw your own labelled points, boxes, outlines, arrows, lines and bands in the picture's coordinates, to show the user what you found."
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
