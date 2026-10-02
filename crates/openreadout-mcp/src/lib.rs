//! MCP (Model Context Protocol) server exposing the same operations as the CLI, over stdio.
//! Tool inputs and outputs are the same `serde`/`schemars` types the CLI uses, so an agent
//! sees one schema whichever way it calls us.
//!
//! MCP is the protocol AI assistants (Claude, Cursor, VS Code, ...) use to call local tools.
//! The server offers one tool per kind of question (`openreadout_info`, `openreadout_stats`,
//! `openreadout_analyze`, `openreadout_export`, ...), named like the CLI commands, resources (`openreadout://formats`,
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

#[cfg(feature = "http")]
mod http;
mod progress;
mod resources;
mod schema_compact;
mod tools;

#[cfg(feature = "http")]
pub use http::{HttpOptions, serve_http};
pub use resources::PREVIEW_BUDGET;

use openreadout_core::{Error, Registry};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::model::{
    CallToolResult, GetPromptRequestParams, GetPromptResponse, Implementation, ListPromptsResult,
    ListResourceTemplatesResult, ListResourcesResult, PaginatedRequestParams,
    ReadResourceRequestParams, ReadResourceResponse, ServerCapabilities, ServerConfig,
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
        data["hint"] = serde_json::Value::String(h);
    }
    McpError::new(
        rmcp::model::ErrorCode::INTERNAL_ERROR,
        e.to_string(),
        Some(data),
    )
}

/// A tool's `strict` argument over the server's default (`OPENREADOUT_STRICT`, `--strict`).
fn with_strict(reg: Registry, strict: Option<bool>) -> Registry {
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

    /// Names of the tools this server offers (for `openreadout doctor`).
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
pub const INSTRUCTIONS: &str = "OpenReadout reads raw lab-instrument files and data-set directories (microscopy, screening plates, flow cytometry, electrophysiology, NMR, mass spectrometry, chromatography, spectroscopy, plate readers, qPCR, bench instruments) without vendor software. Indices are zero-based. Values are raw as stored unless a tool says it processed them; each file reports an assurance level, and strict=true refuses values that are not validated. Only openreadout_export writes next to the data, always to new files that are read back and verified; openreadout_index, openreadout_batch and openreadout_check write only to paths you give them. Errors carry a code and a hint.";

// The trait's methods are async; resources and prompts are answered without awaiting.
#[allow(clippy::unused_async_trait_impl)]
#[tool_handler(router = self.tool_router)]
impl ServerHandler for InstrumentServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .enable_prompts()
                .build(),
        )
        .with_server_info(
            Implementation::new("openreadout", env!("CARGO_PKG_VERSION")).with_title("OpenReadout"),
        )
        .with_instructions(INSTRUCTIONS)
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        Ok(ListResourcesResult::with_all_items(resources::list()))
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
        assert_eq!(tools.len(), 15);
        for t in &tools {
            let a = t
                .annotations
                .as_ref()
                .unwrap_or_else(|| panic!("{} has no annotations", t.name));
            // `index` writes only its own index directory and `check` (report=true) only its
            // own new bundle file: not read-only, not destructive.
            let overwrites = matches!(t.name.as_ref(), "openreadout_export" | "openreadout_batch");
            let writes =
                overwrites || t.name == "openreadout_index" || t.name == "openreadout_check";
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

    fn preview_size_limit() -> u32 {
        openreadout_preview::DEFAULT_THUMBNAIL_SIZE
    }

    #[tokio::test]
    async fn preview_reports_the_ruler_mapping() {
        let s = InstrumentServer::new(synth_registry);
        let f = synth_file("preview", 3000, 2000);
        let args = |axes: Option<bool>, region: Option<openreadout_core::Region>| {
            let v = serde_json::json!({"file": f.display().to_string(), "max_size": 400});
            let mut a: PreviewArgs = serde_json::from_value(v).unwrap();
            a.axes = axes;
            a.region = region;
            Parameters(a)
        };
        let r = s.preview(args(None, None)).await.unwrap();
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
        let r = s.preview(args(None, Some(zoom))).await.unwrap();
        let v = r.structured_content.unwrap();
        assert_eq!(
            v["image"]["full_res_region"],
            serde_json::json!({"x": 1000, "y": 500, "width": 800, "height": 600})
        );
        assert_eq!(
            v["image"]["source_origin"],
            serde_json::json!({"x": 1000.0, "y": 500.0})
        );
        // axes=false: the bare plane fills the picture
        let r = s.preview(args(Some(false), None)).await.unwrap();
        let v = r.structured_content.unwrap();
        assert_eq!(v["image"]["axes"], false);
        assert_eq!(v["image"]["plot_area"]["width"], v["width"]);
        let _ = std::fs::remove_file(f);
    }

    #[test]
    fn capabilities_cover_tools_resources_prompts() {
        let info = InstrumentServer::new(Registry::new).get_info();
        assert!(info.capabilities.tools.is_some());
        assert!(info.capabilities.resources.is_some());
        assert!(info.capabilities.prompts.is_some());
        assert!(INSTRUCTIONS.contains("openreadout_export"));
        // A few sentences: the routing is in the tool names and descriptions.
        assert!(INSTRUCTIONS.len() < 1000, "{}", INSTRUCTIONS.len());
    }
}
