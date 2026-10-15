//! The tools, one module per tool or tool family. Each module holds the tool's arguments,
//! its `#[tool]` method and the helpers only it uses; [`router`] joins their routers.

pub(crate) mod analyses;
pub(crate) mod assay;
pub(crate) mod batch;
pub(crate) mod check;
pub(crate) mod export;
pub(crate) mod index;
pub(crate) mod info;
pub(crate) mod plot;
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
    InstrumentServer::info_router()
        + InstrumentServer::analyses_router()
        + InstrumentServer::assay_router()
        + InstrumentServer::check_router()
        + InstrumentServer::export_router()
        + InstrumentServer::spectra_router()
        + InstrumentServer::table_router()
        + InstrumentServer::trace_router()
        + InstrumentServer::preview_router()
        + InstrumentServer::plot_router()
        + InstrumentServer::stats_router()
        + InstrumentServer::batch_router()
        + InstrumentServer::index_router()
        + InstrumentServer::watch_router()
}

/// The output schema of a tool: how `T` is serialized (a field skipped when empty is not
/// required), with `"type": "object"` at the top as MCP requires (a result with several shapes
/// is an `anyOf` under it). Numbers may be `null`, because serde_json writes NaN and infinity as
/// `null`. Clients such as Claude Code check every result against this schema and turn a
/// mismatch into an error, so it must describe what the tool sends, not what it would accept.
pub(crate) fn output_schema<T: schemars::JsonSchema>() -> std::sync::Arc<rmcp::model::JsonObject> {
    let schema = schemars::generate::SchemaSettings::draft2020_12()
        .for_serialize()
        .into_generator()
        .into_root_schema_for::<T>();
    let mut v = serde_json::to_value(schema).unwrap_or_default();
    nullable_numbers(&mut v);
    let mut o = match v {
        serde_json::Value::Object(o) => o,
        _ => rmcp::model::JsonObject::new(),
    };
    o.remove("title");
    o.remove("description");
    o.insert("type".into(), serde_json::json!("object"));
    std::sync::Arc::new(o)
}

/// Let every `"type": "number"` in a schema also be `null`.
fn nullable_numbers(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::Object(o) => {
            match o.get_mut("type") {
                Some(t) if t == "number" => *t = serde_json::json!(["number", "null"]),
                Some(serde_json::Value::Array(a))
                    if a.iter().any(|x| x == "number") && !a.iter().any(|x| x == "null") =>
                {
                    a.push(serde_json::json!("null"));
                }
                _ => {}
            }
            for x in o.values_mut() {
                nullable_numbers(x);
            }
        }
        serde_json::Value::Array(a) => a.iter_mut().for_each(nullable_numbers),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::output_schema;

    #[derive(serde::Serialize, schemars::JsonSchema)]
    #[allow(dead_code)]
    struct Shape {
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        flag: bool,
        value: f64,
    }

    #[test]
    fn output_schemas_describe_what_is_sent() {
        let s = output_schema::<Shape>();
        // A field left out when false is not required; a number may be NaN, sent as null.
        assert_eq!(s["required"], serde_json::json!(["value"]));
        assert_eq!(
            s["properties"]["value"]["type"],
            serde_json::json!(["number", "null"])
        );
        assert_eq!(s["type"], "object");
    }
}
