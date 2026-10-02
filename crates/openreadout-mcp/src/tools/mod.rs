//! The tools, one module per tool or tool family. Each module holds the tool's arguments,
//! its `#[tool]` method and the helpers only it uses; [`router`] joins their routers.

pub(crate) mod analyze;
pub(crate) mod batch;
pub(crate) mod check;
pub(crate) mod export;
pub(crate) mod index;
pub(crate) mod info;
pub(crate) mod preview;
pub(crate) mod spectra;
pub(crate) mod stats;
pub(crate) mod table;
pub(crate) mod trace;
pub(crate) mod watch;

use rmcp::handler::server::router::tool::ToolRouter;

use crate::InstrumentServer;

/// Every tool of the server.
pub(crate) fn router() -> ToolRouter<InstrumentServer> {
    InstrumentServer::analyze_router()
        + InstrumentServer::info_router()
        + InstrumentServer::check_router()
        + InstrumentServer::export_router()
        + InstrumentServer::spectra_router()
        + InstrumentServer::table_router()
        + InstrumentServer::trace_router()
        + InstrumentServer::preview_router()
        + InstrumentServer::stats_router()
        + InstrumentServer::batch_router()
        + InstrumentServer::index_router()
        + InstrumentServer::watch_router()
}

/// The output schema of a tool whose result has several shapes (an untagged enum): its
/// `anyOf`, under `"type": "object"` as MCP requires of an output schema.
fn object_output<T: schemars::JsonSchema + std::any::Any>()
-> std::sync::Arc<rmcp::model::JsonObject> {
    let mut o = rmcp::handler::server::common::schema_for_output::<T>()
        .as_ref()
        .clone();
    o.insert("type".into(), serde_json::json!("object"));
    std::sync::Arc::new(o)
}
