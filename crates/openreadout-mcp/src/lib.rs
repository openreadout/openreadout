//! MCP (Model Context Protocol) server exposing the same operations as the CLI, over stdio.
//! Tool inputs and outputs are the same `serde`/`schemars` types the CLI uses, so an agent
//! sees one schema whichever way it calls us.
//!
//! MCP is the protocol AI assistants (Claude, Cursor, VS Code, ...) use to call local tools.
//! The server offers one tool per operation (`openreadout_info`, `openreadout_stats`,
//! `openreadout_peaks`, `openreadout_export`, ...), named like the CLI commands and taking the
//! CLI flags' names as arguments, resources (`openreadout://formats`,
//! `openreadout://file/{path}`) and prompts; `book/src/reference/mcp.md` in the repository lists them.
//!
//! # Cargo features
//!
//! | feature | default | what it adds |
//! | --- | --- | --- |
//! | `http` | no | `serve_http` and `HttpOptions`: MCP over Streamable HTTP on a loopback address (optional bearer token). Off by default so that the stdio server contains no networking code. |
//!
//! # Example
//!
//! ```no_run
//! use openreadout_core::Registry;
//!
//! /// The readers the server may use; called once per request so that every call gets fresh
//! /// readers. Real servers register the format crates' readers here.
//! fn registry() -> Registry {
//!     Registry::new()
//! }
//!
//! // Serve on stdin/stdout until the client disconnects.
//! openreadout_mcp::serve_stdio(registry)?;
//! # Ok::<(), anyhow::Error>(())
//! ```
#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod app;
mod budget;
mod coerce;
#[cfg(feature = "http")]
mod http;
mod lean;
mod progress;
mod resources;
mod schema_compact;
mod tools;
mod wording;

#[cfg(feature = "http")]
pub use http::{HttpOptions, serve_http};
pub use resources::PREVIEW_BUDGET;

use openreadout_core::{Error, Registry};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, GetPromptRequestParams,
    GetPromptResponse, Implementation, ListPromptsResult, ListResourceTemplatesResult,
    ListResourcesResult, ListToolsResult, PaginatedRequestParams, ReadResourceRequestParams,
    ReadResourceResponse, ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler, ServiceExt, tool_handler};
use serde::Serialize;

/// The server. Holds a registry factory so each call gets fresh readers.
#[derive(Clone)]
pub struct InstrumentServer {
    registry: fn() -> Registry,
    tool_router: ToolRouter<Self>,
    watches: tools::watch::Watches,
}

impl std::fmt::Debug for InstrumentServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("InstrumentServer")
    }
}

fn mcp_err(e: &Error) -> McpError {
    let mut data = serde_json::json!({"code": e.code(), "exit_code": e.exit_code()});
    if let Some(h) = e.hint() {
        data["hint"] = serde_json::Value::String(wording::for_tools(&h));
    }
    McpError::new(
        rmcp::model::ErrorCode::INTERNAL_ERROR,
        wording::for_tools(&e.to_string()),
        Some(data),
    )
}

/// A failed tool call as a result with `isError`, its message and hint in the text (the model
/// reads tool results; clients often pass on only the message of a protocol error), and
/// `{code, exit_code, hint}` under `_meta["openreadout/error"]`.
fn error_result(e: McpError) -> CallToolResult {
    let mut text = e.message.to_string();
    if let Some(h) = e
        .data
        .as_ref()
        .and_then(|d| d.get("hint"))
        .and_then(|h| h.as_str())
    {
        text.push_str("\nhint: ");
        text.push_str(h);
    }
    let mut r = CallToolResult::error(vec![rmcp::model::ContentBlock::text(text)]);
    // the code and exit code for clients that branch on them, as the CLI's JSON errors have
    if let Some(d) = e.data {
        r.meta
            .get_or_insert_with(rmcp::model::MetaObject::default)
            .insert(ERROR_META_KEY.into(), d);
    }
    r
}

/// Result `_meta` key of a failed call: `{code, exit_code, hint}`.
pub const ERROR_META_KEY: &str = "openreadout/error";

/// A tool's `strict` argument over the server's default (`OPENREADOUT_STRICT`, `--strict`).
pub(crate) fn with_strict(reg: Registry, strict: Option<bool>) -> Registry {
    match strict {
        Some(on) => reg.strict(on),
        None => reg,
    }
}

fn to_value<T: Serialize>(v: &T) -> Result<serde_json::Value, McpError> {
    serde_json::to_value(v).map_err(|e| McpError::internal_error(e.to_string(), None))
}

/// A successful result: the JSON as `structuredContent` and, for clients that only read
/// `content`, the same JSON serialized as one text block.
fn ok_json<T: Serialize>(v: &T) -> Result<CallToolResult, McpError> {
    Ok(CallToolResult::structured(to_value(v)?))
}

impl InstrumentServer {
    /// A fresh registry, strict when the call asks for it (`strict`) or the server runs strict.
    fn reg_for(&self, strict: Option<bool>) -> Registry {
        with_strict((self.registry)(), strict)
    }

    /// Names of the tools this server offers (for `openreadout self doctor`).
    pub fn tool_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .tool_router
            .list_all()
            .into_iter()
            .map(|t| t.name.to_string())
            .collect();
        names.sort();
        names
    }
}

impl InstrumentServer {
    /// A server whose tools open files with readers from `registry` (called once per request).
    pub fn new(registry: fn() -> Registry) -> Self {
        let mut tool_router = tools::router();
        for route in tool_router.map.values_mut() {
            route.attr.input_schema =
                std::sync::Arc::new(schema_compact::compact(&route.attr.input_schema));
        }
        Self {
            registry,
            tool_router,
            watches: tools::watch::Watches::default(),
        }
    }
}

/// Server instructions sent at initialization.
pub const INSTRUCTIONS: &str = "OpenReadout reads raw lab-instrument files and data-set directories without vendor software: microscopy, slides, screening plates, electron microscopy, flow cytometry, electrophysiology, NMR, spectroscopy, mass spectrometry, chromatography, plate readers, qPCR, DNA sequencers and bench instruments, in vendor binary formats and their text and XML exports. Its readers know each format's layout, units and conventions, so questions about these files need no hand-written parser. Each file reports an assurance level that says how far its variant was checked against an independent reader, and strict=true refuses values that are not validated. Indices are zero-based. Values are raw as stored unless a tool says it processed them. Only openreadout_export (data, and figures from openreadout_plot or openreadout_preview) and openreadout_extract write next to the data, to new files that are read back and verified. openreadout_index, openreadout_batch, openreadout_summarize and openreadout_report write only to paths you give them. Tools are the CLI's commands and arguments are its flags (analyze nmr-peaks --range-ppm is openreadout_nmr_peaks with range_ppm). Errors carry a code and a hint.";

// The trait's methods are async; resources and prompts are answered without awaiting.
#[allow(clippy::unused_async_trait_impl)]
#[tool_handler(router = self.tool_router)]
impl ServerHandler for InstrumentServer {
    fn get_info(&self) -> ServerConfig {
        let mut capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_resources()
            .enable_prompts()
            .build();
        capabilities.extensions = Some([app::server_extension()].into_iter().collect());
        ServerConfig::new(capabilities)
            .with_server_info(
                Implementation::new("openreadout", env!("CARGO_PKG_VERSION"))
                    .with_title("OpenReadout"),
            )
            .with_instructions(INSTRUCTIONS)
    }

    // The viewer app (`app`) hooks in here: tools/list, tools/call and the `ui://` resource.
    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let mut tools = self.tool_router.list_all();
        if app::ui_for(&context) {
            app::decorate(&mut tools);
        }
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(
        &self,
        mut request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        if let (Some(tool), Some(args)) = (
            self.tool_router.get(&request.name),
            request.arguments.as_mut(),
        ) {
            coerce::args(&tool.input_schema, args);
            for (key, inner_tool) in coerce::NESTED_TOOL_ARGS {
                if let (Some(serde_json::Value::Object(inner)), Some(t)) =
                    (args.get_mut(key), self.tool_router.get(inner_tool))
                {
                    coerce::args(&t.input_schema, inner);
                }
            }
        }
        if request.name == app::VIEW_TOOL {
            return app::call_view(self.registry, request, &context)
                .await
                .map(Into::into);
        }
        let hint = app::ui_for(&context)
            .then(|| app::view_hint(&request.name, request.arguments.as_ref()))
            .flatten();
        let name = request.name.clone();
        let max_chars = budget::max_chars(&name, request.arguments.as_ref());
        let tcc = rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
        let mut response = match coerce::with_argument_hint(
            self.tool_router.call(tcc).await,
            self.tool_router.get(&name),
        ) {
            Err(e) if self.tool_router.get(&name).is_some() => error_result(e).into(),
            r => r?,
        };
        if let CallToolResponse::Complete(result) = &mut response {
            wording::result_notes(result);
            budget::fit(result, &name, max_chars);
            if let Some(hint) = hint {
                app::attach_hint(result, hint);
            }
        }
        Ok(response)
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        if name == app::VIEW_TOOL {
            return Some(app::view_tool());
        }
        self.tool_router.get(name).cloned()
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        let mut list = resources::list();
        if app::ui_for(&context) {
            list.push(app::resource());
        }
        Ok(ListResourcesResult::with_all_items(list))
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, McpError> {
        Ok(ListResourceTemplatesResult::with_all_items(
            resources::templates(),
        ))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        if let Some(r) = app::read(&request.uri) {
            return Ok(r.into());
        }
        let registry = self.registry;
        let uri = request.uri;
        tokio::task::spawn_blocking(move || resources::read(registry, &uri))
            .await
            .map_err(|e| McpError::internal_error(format!("resource task failed: {e}"), None))?
            .map(Into::into)
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, McpError> {
        Ok(ListPromptsResult::with_all_items(resources::prompts()))
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, McpError> {
        resources::get_prompt(&request.name, request.arguments.as_ref()).map(Into::into)
    }
}

/// Serve over stdio until the client disconnects.
pub fn serve_stdio(registry: fn() -> Registry) -> anyhow::Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(async {
        let service = InstrumentServer::new(registry)
            .serve(rmcp::transport::stdio())
            .await?;
        service.waiting().await?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use base64::Engine as _;
    use openreadout_core::model::FileInfo;
    use rmcp::handler::server::wrapper::Parameters;

    use super::*;
    use crate::tools::info::{InfoArgs, InfoView};
    use crate::tools::preview::PreviewArgs;
    use openreadout_core::reader::PlaneIndex;

    #[test]
    fn every_tool_is_annotated() {
        let s = InstrumentServer::new(Registry::new);
        let tools = s.tool_router.list_all();
        let mut names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            [
                "openreadout_assay",
                "openreadout_batch",
                "openreadout_check",
                "openreadout_chromatogram",
                "openreadout_compare",
                "openreadout_ephys_features",
                "openreadout_export",
                "openreadout_extract",
                "openreadout_gate",
                "openreadout_health",
                "openreadout_index",
                "openreadout_info",
                "openreadout_link",
                "openreadout_nmr_peaks",
                "openreadout_peaks",
                "openreadout_plot",
                "openreadout_preview",
                "openreadout_qpcr",
                "openreadout_report",
                "openreadout_scans",
                "openreadout_search",
                "openreadout_spectrum",
                "openreadout_spikes",
                "openreadout_stats",
                "openreadout_summarize",
                "openreadout_table",
                "openreadout_trace",
                "openreadout_watch",
            ]
        );
        for t in &tools {
            let a = t
                .annotations
                .as_ref()
                .unwrap_or_else(|| panic!("{} has no annotations", t.name));
            // `index` writes only its own index directory and `report` only its own new bundle
            // file: not read-only, not destructive.
            let overwrites = matches!(
                t.name.as_ref(),
                "openreadout_export"
                    | "openreadout_extract"
                    | "openreadout_batch"
                    | "openreadout_summarize"
            );
            let writes =
                overwrites || t.name == "openreadout_index" || t.name == "openreadout_report";
            assert!(a.title.is_some(), "{}", t.name);
            assert_eq!(a.read_only_hint, Some(!writes), "{}", t.name);
            assert_eq!(a.destructive_hint, Some(overwrites), "{}", t.name);
            // `watch` returns new events on every call.
            let idempotent = t.name != "openreadout_watch";
            assert_eq!(a.idempotent_hint, Some(idempotent), "{}", t.name);
            assert_eq!(a.open_world_hint, Some(false), "{}", t.name);
            if t.name != "openreadout_export" {
                let o = t
                    .output_schema
                    .as_ref()
                    .unwrap_or_else(|| panic!("{} has no outputSchema", t.name));
                assert_eq!(
                    o.get("type"),
                    Some(&serde_json::json!("object")),
                    "{}",
                    t.name
                );
            }
        }
    }

    /// A synthetic image format for tool tests: `SYNTHIMG`, then width and height (u32 LE);
    /// the plane is a uint8 gradient generated on read.
    #[derive(Debug)]
    struct SynthReader;

    #[derive(Debug)]
    struct SynthImage {
        path: String,
        w: u32,
        h: u32,
    }

    impl openreadout_core::FormatReader for SynthReader {
        fn descriptor(&self) -> openreadout_core::FormatDescriptor {
            openreadout_core::FormatDescriptor {
                id: "synth".into(),
                name: "Synthetic".into(),
                vendor: "test".into(),
                extensions: vec![],
                family: "microscopy".into(),
                can_read: true,
                can_write: false,
                confidence: openreadout_core::Confidence::High,
                known_gaps: vec![],
            }
        }
        fn sniff(&self, head: &[u8], _: &Path) -> Option<openreadout_core::Detection> {
            head.starts_with(b"SYNTHIMG")
                .then_some(openreadout_core::Detection {
                    format_id: "synth",
                    confidence: openreadout_core::DetectConfidence::Definite,
                    note: None,
                })
        }
        fn open(
            &self,
            path: &Path,
        ) -> openreadout_core::Result<Box<dyn openreadout_core::Dataset>> {
            let b = std::fs::read(path).map_err(|e| Error::io(path, e))?;
            let u = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
            Ok(Box::new(SynthImage {
                path: path.display().to_string(),
                w: u(8),
                h: u(12),
            }))
        }
    }

    impl openreadout_core::Dataset for SynthImage {
        fn info(&self) -> openreadout_core::Result<FileInfo> {
            let mut im = openreadout_core::ImageInfo::new(
                0,
                self.w,
                self.h,
                openreadout_core::PixelType::Uint8,
            );
            im.physical_size =
                openreadout_core::PhysicalSize::micrometres(Some(0.5), Some(0.5), None);
            let im = im.finish();
            Ok(FileInfo {
                path: self.path.clone(),
                size_bytes: 16,
                format: openreadout_core::FormatReader::descriptor(&SynthReader),
                format_version: None,
                plane_count: im.plane_count,
                images: vec![im],
                tables: vec![],
                spectra: vec![],
                traces: vec![],
                notes: vec![],
            })
        }
        fn vendor_metadata(&self) -> openreadout_core::Result<serde_json::Value> {
            Ok(serde_json::Value::Null)
        }
        fn provenance(&self) -> openreadout_core::ProvenanceMap {
            openreadout_core::ProvenanceMap::default()
        }
        fn entries(&self) -> openreadout_core::Result<Vec<openreadout_core::LsEntry>> {
            Ok(vec![])
        }
        fn read_plane(
            &mut self,
            _: u32,
            _: PlaneIndex,
        ) -> openreadout_core::Result<openreadout_core::Plane> {
            assert!(
                u64::from(self.w) * u64::from(self.h) <= 1 << 24,
                "the thumbnail budget must stop this read"
            );
            let data = (0..self.h)
                .flat_map(|y| (0..self.w).map(move |x| ((x + y) % 256) as u8))
                .collect();
            Ok(openreadout_core::Plane {
                width: self.w,
                height: self.h,
                pixel_type: openreadout_core::PixelType::Uint8,
                samples_per_pixel: 1,
                data,
            })
        }
        fn check(&mut self) -> openreadout_core::Result<openreadout_core::CheckReport> {
            Ok(openreadout_core::CheckReport::new(&self.path, "synth"))
        }
    }

    fn synth_registry() -> Registry {
        Registry::new().with(Box::new(SynthReader))
    }

    fn synth_file(name: &str, w: u32, h: u32) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "openreadout-mcp-{}-{name}.synth",
            std::process::id()
        ));
        let mut b = b"SYNTHIMG".to_vec();
        b.extend_from_slice(&w.to_le_bytes());
        b.extend_from_slice(&h.to_le_bytes());
        std::fs::write(&p, b).unwrap();
        p
    }

    fn info_args(file: &Path, thumbnail: Option<bool>) -> Parameters<InfoArgs> {
        Parameters(InfoArgs {
            file: file.display().to_string(),
            view: InfoView::Summary,
            ask: None,
            max_images: None,
            thumbnail,
            vendor: false,
            max_frames: None,
            max_entries: None,
            kind: None,
            first_entry: None,
            strict: None,
        })
    }

    fn images(r: &CallToolResult) -> Vec<Vec<u8>> {
        r.content
            .iter()
            .filter_map(|c| c.as_image())
            .map(|i| {
                base64::engine::general_purpose::STANDARD
                    .decode(&i.data)
                    .unwrap()
            })
            .collect()
    }

    fn texts(r: &CallToolResult) -> Vec<String> {
        r.content
            .iter()
            .filter_map(|c| c.as_text())
            .map(|t| t.text.clone())
            .collect()
    }

    #[test]
    fn info_attaches_a_bounded_thumbnail() {
        let s = InstrumentServer::new(synth_registry);
        let f = synth_file("small", 2000, 1000);
        let with = s.info(info_args(&f, None)).unwrap();
        let without = s.info(info_args(&f, Some(false))).unwrap();
        assert_eq!(with.structured_content, without.structured_content);
        assert!(images(&without).is_empty());
        assert_eq!(without.content.len(), 1);
        let img = images(&with);
        assert_eq!(img.len(), 1);
        let t = texts(&with);
        assert!(
            t.iter().any(|x| x.contains("rulers in full-res px")),
            "{t:?}"
        );
        // PNG or JPEG, at most 384 px, identical across calls
        let png = &img[0];
        let (width, height) = if png.starts_with(b"\x89PNG") {
            (
                u32::from_be_bytes([png[16], png[17], png[18], png[19]]),
                u32::from_be_bytes([png[20], png[21], png[22], png[23]]),
            )
        } else {
            assert!(png.starts_with(&[0xFF, 0xD8]), "PNG or JPEG");
            (0, 0)
        };
        assert!(
            width.max(height) <= preview_size_limit(),
            "{width}x{height}"
        );
        assert_eq!(images(&s.info(info_args(&f, None)).unwrap()), img);

        // over the decoded-pixel budget: no picture, a note pointing to openreadout_preview
        let big = synth_file("big", 40_000, 30_000);
        let r = s.info(info_args(&big, None)).unwrap();
        assert!(images(&r).is_empty());
        let t = texts(&r);
        assert!(
            t.iter()
                .any(|x| x.contains("no thumbnail") && x.contains("openreadout_preview")),
            "{t:?}"
        );
        assert_eq!(
            r.structured_content,
            s.info(info_args(&big, Some(false)))
                .unwrap()
                .structured_content
        );
        let _ = std::fs::remove_file(f);
        let _ = std::fs::remove_file(big);
    }

    /// A preview with annotations, as the agent saw it, saved by openreadout_export at twice
    /// its size; the preview's result says how.
    #[tokio::test]
    async fn export_saves_an_annotated_preview() {
        let server = InstrumentServer::new(synth_registry);
        let file = synth_file("save", 3000, 2000);
        let preview = serde_json::json!({
            "file": file.display().to_string(),
            "max_size": 400,
            "dark": true,
            "annotations": [{"type": "point", "x": 1500, "y": 1000, "label": "here"}],
        });
        let result = server
            .preview(Parameters(serde_json::from_value(preview.clone()).unwrap()))
            .await
            .unwrap();
        let seen = result.structured_content.unwrap();
        let save = seen["save"].as_str().unwrap();
        assert!(save.contains("preview = these arguments") && save.contains(".preview.png"));
        let out = file.with_extension("annotated.png");
        let args = serde_json::from_value(serde_json::json!({
            "preview": preview, "output": out.display().to_string(), "dpi": 300,
        }))
        .unwrap();
        let (written, shown) =
            crate::tools::export::export_blocking(synth_registry, args, None).unwrap();
        assert!(
            shown.is_some_and(|(bytes, _)| !bytes.is_empty()),
            "the saved picture comes back"
        );
        assert_eq!(written["drawn"], "image");
        // the image is read again at twice max_size (whole downsampling steps, so about twice)
        let saved_width = written["width"].as_u64().unwrap();
        let seen_width = seen["width"].as_u64().unwrap();
        assert!(
            saved_width > seen_width * 3 / 2 && saved_width <= seen_width * 2 + 2,
            "{saved_width} vs {seen_width}"
        );
        let png = std::fs::read(&out).unwrap();
        assert!(png.starts_with(b"\x89PNG") && png.windows(4).any(|x| x == b"pHYs"));
        // unknown preview arguments are refused, not ignored
        let err =
            serde_json::from_value::<PreviewArgs>(serde_json::json!({"file": "x", "zoom": 2}))
                .unwrap_err();
        assert!(err.to_string().contains("zoom"), "{err}");
        let _ = std::fs::remove_file(out);
        let _ = std::fs::remove_file(file);
    }

    fn preview_size_limit() -> u32 {
        openreadout_preview::DEFAULT_THUMBNAIL_SIZE
    }

    #[tokio::test]
    async fn preview_reports_the_ruler_mapping() {
        let s = InstrumentServer::new(synth_registry);
        let f = synth_file("preview", 3000, 2000);
        let args = |axes: openreadout_preview::Axes, region: Option<openreadout_core::Region>| {
            let v = serde_json::json!({"file": f.display().to_string(), "max_size": 400});
            let mut a: PreviewArgs = serde_json::from_value(v).unwrap();
            a.options.axes = Some(axes);
            a.options.region = region;
            Parameters(a)
        };
        let r = s
            .preview(args(openreadout_preview::Axes::Rulers, None))
            .await
            .unwrap();
        let v = r.structured_content.clone().unwrap();
        let im = &v["image"];
        assert_eq!(im["axes"], true);
        assert_eq!(
            im["full_res_region"],
            serde_json::json!({"x": 0, "y": 0, "width": 3000, "height": 2000})
        );
        assert!(im["scale_bar"]["pixels"].as_u64().unwrap() > 0);
        assert!(v["hint"].as_str().unwrap().contains("region"));
        assert!(v["width"].as_u64().unwrap() <= 400);
        // a zoom read off the rulers comes back with the same numbers
        let zoom = openreadout_core::Region::new(1000, 500, 800, 600);
        let r = s
            .preview(args(openreadout_preview::Axes::Rulers, Some(zoom)))
            .await
            .unwrap();
        let v = r.structured_content.unwrap();
        assert_eq!(
            v["image"]["full_res_region"],
            serde_json::json!({"x": 1000, "y": 500, "width": 800, "height": 600})
        );
        assert_eq!(
            v["image"]["source_origin"],
            serde_json::json!({"x": 1000.0, "y": 500.0})
        );
        // axes=none: the bare plane fills the picture
        let r = s
            .preview(args(openreadout_preview::Axes::None, None))
            .await
            .unwrap();
        let v = r.structured_content.unwrap();
        assert_eq!(v["image"]["axes"], false);
        assert_eq!(v["image"]["plot_area"]["width"], v["width"]);
        let _ = std::fs::remove_file(f);
    }

    #[tokio::test]
    async fn plot_returns_the_figure_and_what_it_drew() {
        let s = InstrumentServer::new(Registry::new);
        let args: crate::tools::plot::PlotArgs = serde_json::from_value(serde_json::json!({
            "title": "per tube",
            "data": {"values": {"x": ["E01", "E03"], "y": [0.004, 2.6]}},
            "annotate": [{"y": 1.0, "label": "threshold"}],
        }))
        .unwrap();
        let r = s.plot(Parameters(args)).await.unwrap();
        let first = serde_json::to_value(&r.content[0]).unwrap();
        assert_eq!(first["type"], "image");
        assert_eq!(first["mimeType"], "image/png");
        let v = r.structured_content.clone().unwrap();
        assert_eq!(v["inline_series"], 1);
        assert_eq!(
            v["panels"][0]["x"]["categories"],
            serde_json::json!(["E01", "E03"])
        );
        let meta = r.meta.unwrap();
        assert!(meta.get(crate::tools::plot::FIGURE_META_KEY).is_some());
        // it says how to save it, and only openreadout_export writes
        assert!(
            v["save"]
                .as_str()
                .is_some_and(|s| s.starts_with("openreadout_export with plot")),
            "{v}"
        );
        let e = serde_json::from_value::<crate::tools::plot::PlotArgs>(
            serde_json::json!({"data": {"values": {"y": [1]}}, "output": "f.png"}),
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("openreadout_export"), "{e}");
        // an unknown key names the known ones
        let e =
            serde_json::from_value::<crate::tools::plot::PlotArgs>(serde_json::json!({"dta": {}}))
                .unwrap_err()
                .to_string();
        assert!(e.contains("unknown plot argument `dta`"), "{e}");
    }

    #[test]
    fn capabilities_cover_tools_resources_prompts() {
        let info = InstrumentServer::new(Registry::new).get_info();
        assert!(info.capabilities.tools.is_some());
        assert!(info.capabilities.resources.is_some());
        assert!(info.capabilities.prompts.is_some());
        assert!(INSTRUCTIONS.contains("openreadout_export"));
        // A few sentences: the routing is in the tool names and descriptions.
        assert!(INSTRUCTIONS.len() < 1200, "{}", INSTRUCTIONS.len());
    }
}
