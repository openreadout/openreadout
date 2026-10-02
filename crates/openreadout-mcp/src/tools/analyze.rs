//! `openreadout_analyze`: one analysis picked by kind; the dispatcher is
//! `openreadout_batch::analyze`, shared with the Python and R bindings.

use base64::Engine as _;
use openreadout_batch::analyze::{AnalyzeArgs, AnalyzeOutput, run};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::{ErrorData as McpError, tool, tool_router};

use super::object_output;
use crate::{InstrumentServer, mcp_err, to_value, with_strict};

#[tool_router(router = analyze_router, vis = "pub(super)")]
impl InstrumentServer {
    #[tool(
        name = "openreadout_analyze",
        annotations(title = "Analyze", read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = object_output::<AnalyzeOutput>(),
        description = "An analysis with a documented method, picked by kind. options holds that kind's settings: the schema lists every kind's, and an option the kind does not take is an error naming the ones it does. openreadout_batch runs the same kinds and options over many files."
    )]
    pub(crate) async fn analyze(
        &self,
        Parameters(a): Parameters<AnalyzeArgs>,
    ) -> Result<CallToolResult, McpError> {
        let registry = self.registry;
        let (out, png) =
            tokio::task::spawn_blocking(move || run(&with_strict(registry(), a.strict), &a))
                .await
                .map_err(|e| McpError::internal_error(format!("analyze task failed: {e}"), None))?
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
}
