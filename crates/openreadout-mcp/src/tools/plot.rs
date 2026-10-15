//! `openreadout_plot`: a figure composed from lab files, as image content plus a summary of
//! what was drawn.

use std::borrow::Cow;

use base64::Engine as _;
use openreadout_plot::PlotSpec;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, MetaObject};
use rmcp::{ErrorData as McpError, tool, tool_router};
use serde::{Deserialize, Deserializer};

use crate::{InstrumentServer, mcp_err, resources, to_value, with_strict};

/// Result `_meta` key with the figure's panels and thinned series, for hover in the viewer.
pub const FIGURE_META_KEY: &str = "openreadout/figure";

/// The arguments: a [`PlotSpec`], read as JSON first so unknown keys get an error naming the
/// known ones.
#[derive(Debug)]
pub struct PlotArgs(pub PlotSpec);

impl<'de> Deserialize<'de> for PlotArgs {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error as _;
        let v = serde_json::Value::deserialize(d)?;
        openreadout_plot::parse_spec(v)
            .map(PlotArgs)
            .map_err(|e| D::Error::custom(e.to_string()))
    }
}

impl schemars::JsonSchema for PlotArgs {
    fn schema_name() -> Cow<'static, str> {
        PlotSpec::schema_name()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        PlotSpec::json_schema(generator)
    }
}

/// Draw the figure; PNG bytes (JPEG when a PNG would be over the budget) and the result.
fn plot_blocking(
    registry: fn() -> openreadout_core::Registry,
    spec: &PlotSpec,
) -> Result<(openreadout_plot::Plotted, Vec<u8>, &'static str), McpError> {
    let reg = with_strict(registry(), spec.strict);
    let p = openreadout_plot::run(&reg, spec).map_err(|e| mcp_err(&e))?;
    let raster = p.figure.to_raster(1.0);
    let png = raster.to_png().map_err(|e| mcp_err(&e))?;
    if png.len() <= resources::PREVIEW_BUDGET {
        return Ok((p, png, "image/png"));
    }
    let jpg = raster.to_jpeg(88).map_err(|e| mcp_err(&e))?;
    Ok((p, jpg, "image/jpeg"))
}

#[tool_router(router = plot_router, vis = "pub(super)")]
impl InstrumentServer {
    #[tool(
        name = "openreadout_plot",
        annotations(title = "Draw a figure", read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::common::schema_for_output::<openreadout_plot::PlotOutput>(),
        description = "A finished figure from lab files in one call, as image content plus what was drawn (series, data read, ranges, units, marked peaks). Name data by file and selector, never by pasting numbers: traces and sweeps, chromatograms (TIC, XIC) and the spectrum at an rt or the TIC apex, plate wells by range or layout group (curves per well, or mean ± spread per group), FCS dot plots coloured by gate population and histograms, f–I curves, dose-response and standard-curve fits, a table file's columns (openreadout_batch output); small inline values are allowed. Several files overlay; facet=series gives small multiples, facet=plate plate positions; panels side by side; annotate with bands, lines, labelled points, arrows. output writes PNG, SVG or PDF."
    )]
    pub(crate) async fn plot(
        &self,
        Parameters(PlotArgs(spec)): Parameters<PlotArgs>,
    ) -> Result<CallToolResult, McpError> {
        let registry = self.registry;
        let (p, bytes, mime) = tokio::task::spawn_blocking(move || plot_blocking(registry, &spec))
            .await
            .map_err(|e| McpError::internal_error(format!("plot task failed: {e}"), None))??;
        let mut r = CallToolResult::structured(to_value(&p.output)?);
        r.content.insert(
            0,
            ContentBlock::image(
                base64::engine::general_purpose::STANDARD.encode(&bytes),
                mime,
            ),
        );
        let m = r.meta.get_or_insert_with(MetaObject::default);
        m.insert(
            FIGURE_META_KEY.into(),
            serde_json::json!({"width": p.output.width, "height": p.output.height, "panels": p.hover}),
        );
        Ok(r)
    }
}
