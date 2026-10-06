//! The viewer app: an MCP App (the `io.modelcontextprotocol/ui` extension) that shows the data
//! of a file inside the chat.
//!
//! How it fits together:
//! - The viewer is one HTML page, `ui://openreadout/viewer.html`, assembled at compile time from
//!   `web/` (no build step, no network: the page loads nothing from outside).
//! - Tools that read one file ([`TOOL_VIEWS`]) carry `_meta.ui.resourceUri`, so a host that
//!   supports MCP Apps renders the viewer next to their result. Their results gain
//!   `_meta["openreadout/view"]`: the arguments the viewer should start from.
//! - The viewer fetches what it draws by calling `openreadout_view` ([`view`]), a tool only the
//!   app can call (`visibility: ["app"]`). Its results are bounded: pictures are at most
//!   [`view::MAX_IMAGE_SIZE`] px, plots at most [`view::MAX_POINTS`] points per series.
//! - `openreadout_view` is also a file viewer in hosts that open files with an app (OpenAI's
//!   `openai/ui` file entrypoint): the host passes the file's path in
//!   `_meta["openai/resource"].path`.
//!
//! The server lists the viewer tool and decorates the other tools only when the client declares
//! the extension in its capabilities ([`host_supports_ui`]); other clients see the same tools
//! as before. `OPENREADOUT_MCP_APPS=off` turns the viewer off, `on` turns it on for clients that
//! do not declare the extension.

pub(crate) mod view;

use std::sync::LazyLock;

use rmcp::ErrorData as McpError;
use rmcp::RoleServer;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ClientCapabilities, JsonObject, MetaObject,
    ReadResourceResult, Resource, ResourceContents, Tool, ToolAnnotations,
};
use rmcp::service::RequestContext;
use serde_json::{Value, json};

/// The extension's identifier in client and server capabilities.
pub const EXTENSION_ID: &str = "io.modelcontextprotocol/ui";
/// MIME type of an MCP App HTML resource.
pub const MIME_TYPE: &str = "text/html;profile=mcp-app";
/// The viewer's resource URI.
pub const RESOURCE_URI: &str = "ui://openreadout/viewer.html";
/// The tool the viewer calls for data.
pub const VIEW_TOOL: &str = "openreadout_view";
/// Result `_meta` key holding the arguments the viewer starts from.
pub const VIEW_META_KEY: &str = "openreadout/view";
/// Environment variable that turns the viewer `on` or `off` regardless of what the client declares.
pub const ENV_SWITCH: &str = "OPENREADOUT_MCP_APPS";

/// File extensions the viewer offers to open in hosts with file entrypoints. Only extensions
/// that belong to one instrument vendor's format: generic ones (`.tif`, `.csv`, `.txt`, `.xml`,
/// `.dat`, `.raw`) would take over files that other apps open better, and directory formats
/// (`.d`) are not files.
pub const FILE_EXTENSIONS: &[&str] = &[
    ".czi", ".nd2", ".lif", ".lof", ".oir", ".oib", ".oif", ".vsi", ".ims", ".zvi", ".mrxs",
    ".ndpi", ".svs", ".dm3", ".dm4", ".fcs", ".abf", ".smr", ".smrx", ".wcp", ".wiff", ".wiff2",
    ".asyr",
];

/// What the viewer shows first for a tool's result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolView {
    /// Whatever suits the file ([`view::ViewKind::Auto`]).
    Auto,
    /// A fixed view.
    Kind(view::ViewKind),
    /// `openreadout_preview`: the image, trace, spectrum or plate its arguments ask for.
    Preview,
    /// `openreadout_spectra`: one spectrum when the call named one, else the chromatogram.
    Spectra,
    /// `openreadout_analyze`: by analysis kind.
    Analyze,
}

/// The tools whose results the viewer shows, and how it starts. Tools not listed here (export,
/// batch, index, ...) have no viewer. This table is the only place that names other tools.
pub const TOOL_VIEWS: &[(&str, ToolView)] = &[
    ("openreadout_info", ToolView::Auto),
    ("openreadout_preview", ToolView::Preview),
    ("openreadout_stats", ToolView::Kind(view::ViewKind::Image)),
    ("openreadout_trace", ToolView::Kind(view::ViewKind::Trace)),
    ("openreadout_spectra", ToolView::Spectra),
    ("openreadout_analyze", ToolView::Analyze),
    ("openreadout_table", ToolView::Auto),
];

fn tool_view(name: &str) -> Option<ToolView> {
    TOOL_VIEWS.iter().find(|(n, _)| *n == name).map(|(_, v)| *v)
}

/// The switch's value: `Some(true)` for `on`, `Some(false)` for `off`, `None` to follow the client.
fn switch(value: Option<&str>) -> Option<bool> {
    match value?.trim().to_ascii_lowercase().as_str() {
        "on" | "1" | "true" | "yes" => Some(true),
        "off" | "0" | "false" | "no" => Some(false),
        _ => None,
    }
}

/// Did the client declare MCP Apps with HTML views? `switch` is [`ENV_SWITCH`]'s value.
pub fn host_supports_ui(caps: Option<&ClientCapabilities>, switch_value: Option<&str>) -> bool {
    if let Some(on) = switch(switch_value) {
        return on;
    }
    let Some(settings) = caps
        .and_then(|c| c.extensions.as_ref())
        .and_then(|e| e.get(EXTENSION_ID))
    else {
        return false;
    };
    // `mimeTypes` is required by the spec; a client that leaves it out still declared the
    // extension, and HTML is the only kind of view there is.
    match settings.get("mimeTypes") {
        None => true,
        Some(Value::Array(types)) => types
            .iter()
            .filter_map(Value::as_str)
            .any(|m| m.trim().to_ascii_lowercase().starts_with("text/html")),
        Some(_) => false,
    }
}

/// [`host_supports_ui`] for the client of this request, with the environment's switch.
pub fn ui_for(context: &RequestContext<RoleServer>) -> bool {
    let switch_value = std::env::var(ENV_SWITCH).ok();
    host_supports_ui(
        context.client_capabilities().as_ref(),
        switch_value.as_deref(),
    )
}

/// The server's extension capability entry.
pub fn server_extension() -> (String, JsonObject) {
    (EXTENSION_ID.to_string(), JsonObject::new())
}

fn meta(v: Value) -> MetaObject {
    match v {
        Value::Object(m) => MetaObject(m),
        _ => MetaObject::default(),
    }
}

/// The tools as a UI host sees them: the listed tools carry the viewer's URI, and the viewer
/// tool is added.
pub fn decorate(tools: &mut Vec<Tool>) {
    for t in tools.iter_mut() {
        if tool_view(&t.name).is_some() {
            let mut m = t.meta.take().unwrap_or_default();
            m.insert("ui".into(), json!({ "resourceUri": RESOURCE_URI }));
            t.meta = Some(m);
        }
    }
    tools.push(view_tool());
}

/// The `openreadout_view` tool definition.
pub fn view_tool() -> Tool {
    let schema = rmcp::handler::server::common::schema_for_type::<view::ViewArgs>();
    let schema = crate::schema_compact::compact(&schema);
    Tool::new(
        VIEW_TOOL,
        "Data for the OpenReadout viewer app: a picture of an image plane, or a plot of a sweep, spectrum, chromatogram, plate or flow-cytometry events, downsampled for display. Called by the viewer, not by the model.",
        std::sync::Arc::new(schema),
    )
    .with_title("OpenReadout viewer")
    .with_annotations(
        ToolAnnotations::new()
            .read_only(true)
            .destructive(false)
            .idempotent(true)
            .open_world(false),
    )
    .with_meta(meta(json!({
        "ui": { "resourceUri": RESOURCE_URI, "visibility": ["app"] },
        "openai/ui": {
            "entrypoints": [{ "type": "file", "extensions": FILE_EXTENSIONS }]
        }
    })))
}

/// The viewer's resource listing entry.
pub fn resource() -> Resource {
    let mut r = Resource::new(RESOURCE_URI, "viewer")
        .with_title("OpenReadout viewer")
        .with_description(
            "Interactive viewer for the data of an instrument file: image planes, traces, spectra, chromatograms, plates and flow-cytometry events.",
        )
        .with_mime_type(MIME_TYPE);
    r.meta = Some(resource_meta());
    r
}

fn resource_meta() -> MetaObject {
    // An empty CSP: the page loads nothing from outside and makes no requests.
    meta(json!({
        "ui": { "prefersBorder": true, "csp": {} },
        "openai/ui": { "availableDisplayModes": ["inline", "fullscreen"] }
    }))
}

/// The page, assembled from `web/` once.
pub fn html() -> &'static str {
    static HTML: LazyLock<String> = LazyLock::new(|| {
        include_str!("web/viewer.html")
            .replace("/*STYLE*/", include_str!("web/viewer.css"))
            .replace("/*SCRIPT*/", include_str!("web/viewer.js"))
    });
    &HTML
}

/// `resources/read` of the viewer, or `None` for another URI.
pub fn read(uri: &str) -> Option<ReadResourceResult> {
    (uri == RESOURCE_URI).then(|| {
        ReadResourceResult::new(vec![ResourceContents::TextResourceContents {
            uri: RESOURCE_URI.into(),
            mime_type: Some(MIME_TYPE.into()),
            text: html().into(),
            meta: Some(resource_meta()),
        }])
    })
}

/// Keys of a tool's arguments that the viewer understands under the same name.
const PASSED_KEYS: &[&str] = &[
    "image", "level", "region", "trace", "sweep", "channels", "run", "table", "column",
];

/// The arguments the viewer starts from after `tool` ran with `args`, or `None` when the tool
/// has no viewer or names no file.
pub fn view_hint(tool: &str, args: Option<&JsonObject>) -> Option<Value> {
    use view::ViewKind as K;
    let tv = tool_view(tool)?;
    let args = args?;
    let file = args.get("file")?.as_str()?;
    let mut hint = serde_json::Map::new();
    hint.insert("file".into(), json!(file));
    for k in PASSED_KEYS {
        if let Some(v) = args.get(*k).filter(|v| !v.is_null()) {
            hint.insert((*k).into(), v.clone());
        }
    }
    let opt = |k: &str| {
        args.get("options")
            .and_then(Value::as_object)
            .and_then(|o| o.get(k))
            .filter(|v| !v.is_null())
            .cloned()
    };
    // `select` (preview, stats): channel, z and t of an image
    if let Some(Value::Array(sel)) = args.get("select") {
        view::select_into(sel, &mut hint);
    }
    let kind = match tv {
        ToolView::Auto => None,
        ToolView::Kind(k) => Some(k),
        ToolView::Preview => {
            if args.get("run").is_some() || args.get("scan").is_some() {
                if let Some(s) = args.get("spectrum") {
                    hint.insert("index".into(), s.clone());
                }
                if let Some(s) = args.get("scan") {
                    hint.insert("scan".into(), s.clone());
                }
                Some(K::Spectrum)
            } else if args.get("trace").is_some() {
                Some(K::Trace)
            } else if args.get("table").is_some() {
                Some(K::Plate)
            } else {
                for k in ["composite", "contrast"] {
                    if let Some(v) = args.get(k) {
                        hint.insert(k.into(), v.clone());
                    }
                }
                if args.get("mip").and_then(Value::as_str).is_some() {
                    hint.insert("mip".into(), json!(true));
                }
                None
            }
        }
        ToolView::Spectra => {
            for k in ["scan", "index"] {
                if let Some(v) = args.get(k) {
                    hint.insert(k.into(), v.clone());
                }
            }
            Some(
                if args.get("scan").is_some() || args.get("index").is_some() {
                    K::Spectrum
                } else {
                    K::Chromatogram
                },
            )
        }
        ToolView::Analyze => match args.get("kind").and_then(Value::as_str) {
            Some("chromatogram" | "peaks") => {
                for k in ["mz", "ppm", "run"] {
                    if let Some(v) = opt(k) {
                        hint.insert(k.into(), v);
                    }
                }
                if let Some(Value::Array(t)) = opt("traces")
                    && let Some(first) = t.first()
                {
                    hint.insert("trace".into(), first.clone());
                }
                hint.insert("peaks".into(), json!(true));
                Some(K::Chromatogram)
            }
            Some("nmr-peaks") => Some(K::Nmr),
            Some("ephys-features" | "spikes") => Some(K::Trace),
            Some("gate") => Some(K::Fcs),
            Some("assay") => Some(K::Plate),
            _ => None,
        },
    };
    if let Some(k) = kind {
        hint.insert("view".into(), json!(k.name()));
    }
    Some(Value::Object(hint))
}

/// Add the viewer's starting arguments to a tool result.
pub fn attach_hint(result: &mut CallToolResult, hint: Value) {
    let m = result.meta.get_or_insert_with(MetaObject::default);
    m.insert(VIEW_META_KEY.into(), hint);
}

/// The path a host gave for an opened file: `_meta["openai/resource"].path`.
pub fn host_path(
    request: &CallToolRequestParams,
    context: &RequestContext<RoleServer>,
) -> Option<String> {
    let from = |m: &JsonObject| {
        m.get("openai/resource")
            .and_then(|r| r.get("path"))
            .and_then(Value::as_str)
            .filter(|p| !p.is_empty())
            .map(str::to_string)
    };
    request
        .meta
        .as_ref()
        .and_then(|m| from(m))
        .or_else(|| from(&context.meta))
}

/// Run `openreadout_view`.
pub async fn call_view(
    registry: fn() -> openreadout_core::Registry,
    request: CallToolRequestParams,
    context: &RequestContext<RoleServer>,
) -> Result<CallToolResult, McpError> {
    let path = host_path(&request, context);
    let args: view::ViewArgs =
        serde_json::from_value(Value::Object(request.arguments.unwrap_or_default())).map_err(
            |e| {
                McpError::invalid_params(
                    format!("bad openreadout_view arguments: {e}"),
                    Some(json!({"hint": "Pass file (a path) and optional view settings; see the tool's input schema."})),
                )
            },
        )?;
    tokio::task::spawn_blocking(move || view::call(registry, &args, path))
        .await
        .map_err(|e| McpError::internal_error(format!("view task failed: {e}"), None))?
}

#[cfg(test)]
mod tests;
