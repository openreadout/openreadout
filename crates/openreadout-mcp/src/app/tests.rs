use std::path::{Path, PathBuf};

use base64::Engine as _;
use openreadout_core::model::{
    ColumnInfo, FileInfo, SignalChannelInfo, Table, TableInfo, Trace, TraceInfo,
};
use openreadout_core::{Error, Registry};
use rmcp::model::ExtensionCapabilities;
use serde_json::json;

use super::*;

fn caps(ext: Option<Value>) -> ClientCapabilities {
    let mut c = ClientCapabilities::default();
    if let Some(Value::Object(settings)) = ext {
        let mut e = ExtensionCapabilities::new();
        e.insert(EXTENSION_ID.into(), settings);
        c.extensions = Some(e);
    }
    c
}

#[test]
fn ui_only_for_clients_that_declare_it() {
    let html = caps(Some(json!({"mimeTypes": [MIME_TYPE]})));
    assert!(host_supports_ui(Some(&html), None));
    // Codex's tests declare plain text/html; the profile is the only kind there is.
    let plain = caps(Some(json!({"mimeTypes": ["text/html"]})));
    assert!(host_supports_ui(Some(&plain), None));
    let no_list = caps(Some(json!({})));
    assert!(host_supports_ui(Some(&no_list), None));
    let other = caps(Some(json!({"mimeTypes": ["application/x-future"]})));
    assert!(!host_supports_ui(Some(&other), None));
    assert!(!host_supports_ui(Some(&caps(None)), None));
    assert!(!host_supports_ui(None, None));
    // the switch wins either way; anything else follows the client
    assert!(!host_supports_ui(Some(&html), Some("off")));
    assert!(host_supports_ui(None, Some("on")));
    assert!(host_supports_ui(Some(&html), Some("auto")));
}

#[test]
fn listed_tools_point_at_the_viewer() {
    let server = crate::InstrumentServer::new(Registry::new);
    let mut tools = server.tool_router.list_all();
    let before = tools.len();
    decorate(&mut tools);
    assert_eq!(tools.len(), before + 1);
    for t in &tools {
        let ui = t.meta.as_ref().and_then(|m| m.get("ui"));
        let listed = tool_view(&t.name).is_some();
        if t.name == VIEW_TOOL {
            let ui = ui.unwrap();
            assert_eq!(ui["resourceUri"], RESOURCE_URI);
            assert_eq!(ui["visibility"], json!(["app"]));
        } else if listed {
            assert_eq!(ui.unwrap()["resourceUri"], RESOURCE_URI, "{}", t.name);
        } else {
            assert!(ui.is_none(), "{} has no viewer", t.name);
        }
    }
    // every tool in the table exists (a renamed tool must be renamed here too)
    for (name, _) in TOOL_VIEWS {
        assert!(
            server.tool_router.get(name).is_some(),
            "{name} is in TOOL_VIEWS but not a tool"
        );
    }
}

#[test]
fn view_tool_claims_only_vendor_extensions() {
    let t = view_tool();
    let m = t.meta.unwrap();
    let entry = &m["openai/ui"]["entrypoints"][0];
    assert_eq!(entry["type"], "file");
    let exts: Vec<&str> = entry["extensions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(exts.contains(&".czi") && exts.contains(&".fcs"));
    for generic in [
        ".tif", ".tiff", ".csv", ".txt", ".xml", ".dat", ".raw", ".d", ".h5", ".json",
    ] {
        assert!(!exts.contains(&generic), "{generic} is not vendor-specific");
    }
    for e in &exts {
        assert!(e.starts_with('.') && e.len() > 2, "{e}");
    }
    // the input schema takes a path or a host-opened file
    let schema = serde_json::to_string(&t.input_schema).unwrap();
    assert!(schema.contains("resourceUri"), "{schema}");
    assert_eq!(t.annotations.unwrap().read_only_hint, Some(true));
}

#[test]
fn the_page_is_self_contained() {
    let r = read(RESOURCE_URI).unwrap();
    let ResourceContents::TextResourceContents {
        mime_type,
        text,
        meta,
        ..
    } = &r.contents[0]
    else {
        panic!("text contents expected");
    };
    assert_eq!(mime_type.as_deref(), Some(MIME_TYPE));
    assert!(text.starts_with("<!doctype html>"));
    assert!(!text.contains("/*STYLE*/") && !text.contains("/*SCRIPT*/"));
    assert!(text.contains("ui/initialize") && text.contains(VIEW_TOOL));
    // nothing loads from elsewhere: the default CSP of MCP Apps allows no external origin
    for needle in ["http://", "https://", "<link", "import(", "fetch("] {
        assert!(!text.contains(needle), "the page mentions {needle}");
    }
    assert!(text.len() < 300_000, "{} bytes", text.len());
    let meta = meta.as_ref().unwrap();
    assert_eq!(meta["ui"]["csp"], json!({}));
    assert!(read("ui://openreadout/other.html").is_none());
    assert_eq!(resource().mime_type.as_deref(), Some(MIME_TYPE));
}

#[test]
fn hints_follow_the_tool_table() {
    let args = |v: Value| v.as_object().cloned().unwrap();
    let h = view_hint("openreadout_info", Some(&args(json!({"file": "/d/a.czi"})))).unwrap();
    assert_eq!(h, json!({"file": "/d/a.czi"}));
    let h = view_hint(
        "openreadout_preview",
        Some(&args(
            json!({"file": "/d/a.czi", "select": ["c=1,z=4"], "mip": "z"}),
        )),
    )
    .unwrap();
    assert_eq!(h["c"], 1);
    assert_eq!(h["z"], 4);
    assert_eq!(h["mip"], true);
    let h = view_hint(
        "openreadout_trace",
        Some(&args(json!({"file": "a.abf", "sweep": 3, "channels": [1]}))),
    )
    .unwrap();
    assert_eq!(h["view"], "trace");
    assert_eq!(h["sweep"], 3);
    assert_eq!(h["channels"], json!([1]));
    let h = view_hint(
        "openreadout_chromatogram",
        Some(&args(json!({"file": "a.raw", "mz": [301.14]}))),
    )
    .unwrap();
    assert_eq!(h["view"], "chromatogram");
    assert_eq!(h["mz"], json!([301.14]));
    let h = view_hint(
        "openreadout_spectra",
        Some(&args(json!({"file": "a.mzML", "scan": 12}))),
    )
    .unwrap();
    assert_eq!(h["view"], "spectrum");
    assert_eq!(h["scan"], 12);
    let h = view_hint(
        "openreadout_spectra",
        Some(&args(json!({"file": "a.mzML", "spectrum": 3}))),
    )
    .unwrap();
    assert_eq!(h["view"], "spectrum");
    assert_eq!(h["index"], 3);
    let h = view_hint(
        "openreadout_nmr_peaks",
        Some(&args(json!({"file": "a.fid"}))),
    )
    .unwrap();
    assert_eq!(h["view"], "nmr");
    let h = view_hint(
        "openreadout_dose_response",
        Some(&args(json!({"file": "plate.xlsx"}))),
    )
    .unwrap();
    assert_eq!(h["view"], "plate");
    // tools without a viewer, or calls without a file
    assert!(view_hint("openreadout_export", Some(&args(json!({"file": "a"})))).is_none());
    assert!(view_hint("openreadout_info", Some(&args(json!({})))).is_none());
    assert!(view_hint("openreadout_info", None).is_none());
}

// ---------------------------------------------------------------------------------------------
// The view tool against synthetic data sets: `FAKE` + kind byte + u32 size.

#[derive(Debug)]
struct FakeReader;

#[derive(Debug)]
struct Fake {
    path: String,
    kind: u8,
    n: u32,
}

impl openreadout_core::FormatReader for FakeReader {
    fn descriptor(&self) -> openreadout_core::FormatDescriptor {
        openreadout_core::FormatDescriptor {
            id: "fake".into(),
            name: "Fake".into(),
            vendor: "test".into(),
            extensions: vec![],
            family: "test".into(),
            can_read: true,
            can_write: false,
            confidence: openreadout_core::Confidence::High,
            known_gaps: vec![],
        }
    }
    fn sniff(&self, head: &[u8], _: &Path) -> Option<openreadout_core::Detection> {
        head.starts_with(b"FAKE")
            .then_some(openreadout_core::Detection {
                format_id: "fake",
                confidence: openreadout_core::DetectConfidence::Definite,
                note: None,
            })
    }
    fn open(&self, path: &Path) -> openreadout_core::Result<Box<dyn openreadout_core::Dataset>> {
        let b = std::fs::read(path).map_err(|e| Error::io(path, e))?;
        Ok(Box::new(Fake {
            path: path.display().to_string(),
            kind: b[4],
            n: u32::from_le_bytes([b[5], b[6], b[7], b[8]]),
        }))
    }
}

impl openreadout_core::Dataset for Fake {
    fn info(&self) -> openreadout_core::Result<FileInfo> {
        let mut format = openreadout_core::FormatReader::descriptor(&FakeReader);
        let mut info = FileInfo {
            path: self.path.clone(),
            size_bytes: 9,
            format: format.clone(),
            format_version: None,
            plane_count: 0,
            images: vec![],
            tables: vec![],
            spectra: vec![],
            traces: vec![],
            notes: vec![],
        };
        match self.kind {
            b'I' => {
                format.family = "microscopy".into();
                let im = openreadout_core::ImageInfo::new(
                    0,
                    self.n,
                    self.n / 2,
                    openreadout_core::PixelType::Uint8,
                )
                .finish();
                info.plane_count = im.plane_count;
                info.images = vec![im];
            }
            b'T' => {
                let ch = |name: &str| SignalChannelInfo {
                    name: name.into(),
                    unit: Some("mV".into()),
                    dtype: "f64".into(),
                    scale: 1.0,
                    ..Default::default()
                };
                info.traces = vec![TraceInfo {
                    index: 0,
                    sample_rate_hz: 10_000.0,
                    sample_count: u64::from(self.n),
                    sweep_count: 2,
                    channels: (0..10).map(|i| ch(&format!("ch{i}"))).collect(),
                    ..Default::default()
                }];
            }
            _ => {
                format.family = "flow-cytometry".into();
                let col = |i: u32, name: &str| ColumnInfo {
                    index: i,
                    name: name.into(),
                    dtype: "f32".into(),
                    ..Default::default()
                };
                info.tables = vec![TableInfo {
                    index: 0,
                    row_count: u64::from(self.n),
                    columns: vec![col(0, "FSC-A"), col(1, "SSC-A"), col(2, "FITC-A")],
                    ..Default::default()
                }];
            }
        }
        info.format = format;
        Ok(info)
    }
    fn vendor_metadata(&self) -> openreadout_core::Result<Value> {
        Ok(Value::Null)
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
        _: openreadout_core::reader::PlaneIndex,
    ) -> openreadout_core::Result<openreadout_core::Plane> {
        let (w, h) = (self.n, self.n / 2);
        Ok(openreadout_core::Plane {
            width: w,
            height: h,
            pixel_type: openreadout_core::PixelType::Uint8,
            samples_per_pixel: 1,
            data: (0..h)
                .flat_map(|y| (0..w).map(move |x| ((x ^ y) % 256) as u8))
                .collect(),
        })
    }
    fn read_trace(
        &mut self,
        trace: u32,
        sweep: u32,
        first: u64,
        count: u64,
    ) -> openreadout_core::Result<Trace> {
        // pages of at most 100 000 samples, like a real reader
        let end = (first + count.min(100_000)).min(u64::from(self.n));
        let channels = (0..10)
            .map(|c| {
                (first..end)
                    .map(|i| {
                        if i == 123_456 {
                            1000.0
                        } else {
                            f64::from(c) + (i % 10) as f64
                        }
                    })
                    .collect()
            })
            .collect();
        Ok(Trace {
            trace,
            sweep,
            first_sample: first,
            channels,
        })
    }
    fn read_table(&mut self, _: u32, first: u64, max: u64) -> openreadout_core::Result<Table> {
        let end = (first + max).min(u64::from(self.n));
        let col = |k: f64| (first..end).map(|i| i as f64 * k).collect();
        Ok(Table {
            table: 0,
            first_row: first,
            columns: vec![col(1.0), col(2.0), col(3.0)],
        })
    }
    fn check(&mut self) -> openreadout_core::Result<openreadout_core::CheckReport> {
        Ok(openreadout_core::CheckReport::new(&self.path, "fake"))
    }
}

fn fake_registry() -> Registry {
    Registry::new().with(Box::new(FakeReader))
}

fn fake_file(name: &str, kind: u8, n: u32) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "openreadout-app-{}-{name}.fake",
        std::process::id()
    ));
    let mut b = b"FAKE".to_vec();
    b.push(kind);
    b.extend_from_slice(&n.to_le_bytes());
    std::fs::write(&p, b).unwrap();
    p
}

fn run(args: Value, host_path: Option<String>) -> CallToolResult {
    let a: view::ViewArgs = serde_json::from_value(args).unwrap();
    view::call(fake_registry, &a, host_path).unwrap()
}

#[test]
fn image_views_are_bounded_pictures() {
    let f = fake_file("image", b'I', 6000);
    let r = run(json!({"file": f, "max_size": 99999}), None);
    let v = r.structured_content.as_ref().unwrap();
    assert_eq!(v["view"], "image");
    assert_eq!(v["outline"]["views"], json!(["image"]));
    assert!(v["width"].as_u64().unwrap() <= u64::from(view::MAX_IMAGE_SIZE));
    let img = r.content.iter().find_map(|c| c.as_image()).unwrap();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&img.data)
        .unwrap();
    assert!(bytes.len() <= crate::resources::PREVIEW_BUDGET);
    // a zoomed region maps back to full-resolution pixels
    let r = run(
        json!({"file": f, "outline": false, "region": {"x": 100, "y": 50, "width": 400, "height": 200}}),
        None,
    );
    let v = r.structured_content.unwrap();
    assert!(v.get("outline").is_none());
    assert_eq!(
        v["image"]["full_res_region"],
        json!({"x": 100, "y": 50, "width": 400, "height": 200})
    );
    let _ = std::fs::remove_file(f);
}

#[test]
fn trace_views_are_envelopes_that_keep_spikes() {
    let file = fake_file("trace", b'T', 1_000_000);
    let r = run(json!({"file": file, "points": 999_999}), None);
    let out = r.structured_content.unwrap();
    assert_eq!(out["view"], "trace");
    let plot = &out["plot"];
    let series = plot["series"].as_array().unwrap();
    assert_eq!(series.len(), view::MAX_CHANNELS);
    assert!(
        out["notes"].to_string().contains("first 8 of 10 channels"),
        "{}",
        out["notes"]
    );
    for s in series {
        assert_eq!(s["hi"].as_array().unwrap().len(), view::MAX_POINTS as usize);
    }
    // the one-sample spike survives 250x downsampling
    let hi = series[0]["hi"].as_array().unwrap();
    assert!(hi.iter().any(|x| x.as_f64() == Some(1000.0)));
    assert!(out["limits"]["samples_per_point"].as_f64().unwrap() > 1.0);
    // a short window comes back as the samples themselves
    let r = run(
        json!({"file": file, "outline": false, "first_sample": 500, "count": 100, "channels": [3], "sweep": 1}),
        None,
    );
    let plot = &r.structured_content.unwrap()["plot"];
    assert_eq!(plot["sweep"], 1);
    assert_eq!(plot["first_sample"], 500);
    assert_eq!(plot["series"][0]["y"].as_array().unwrap().len(), 100);
    assert_eq!(plot["series"][0]["channel"], 3);
    // out-of-range requests are usage errors with the counts
    let args: view::ViewArgs = serde_json::from_value(json!({"file": file, "sweep": 5})).unwrap();
    let err = view::call(fake_registry, &args, None).unwrap_err();
    assert!(err.message.contains("2 sweeps"), "{}", err.message);
    let _ = std::fs::remove_file(file);
}

#[test]
fn events_are_sampled_and_encoded() {
    let f = fake_file("fcs", b'F', 200_000);
    let r = run(json!({"file": f, "events": 1_000_000}), None);
    let v = r.structured_content.unwrap();
    assert_eq!(v["view"], "fcs");
    let ev = &v["events"];
    assert_eq!(ev["x"]["name"], "FSC-A");
    assert_eq!(ev["y"]["name"], "SSC-A");
    assert_eq!(ev["sampled"].as_u64().unwrap(), view::MAX_EVENTS);
    let xs = base64::engine::general_purpose::STANDARD
        .decode(ev["xs"].as_str().unwrap())
        .unwrap();
    assert_eq!(xs.len() as u64, view::MAX_EVENTS * 4);
    // every 4th event: FSC-A of event 4 is 4
    assert_eq!(f32::from_le_bytes([xs[4], xs[5], xs[6], xs[7]]), 4.0);
    // histogram: no y
    let r = run(json!({"file": f, "x": "FITC-A", "y": ""}), None);
    let ev = &r.structured_content.unwrap()["events"];
    assert!(ev["y"].is_null() && ev["ys"].is_null());
    let a: view::ViewArgs = serde_json::from_value(json!({"file": f, "x": "nope"})).unwrap();
    assert!(view::call(fake_registry, &a, None).is_err());
    let _ = std::fs::remove_file(f);
}

#[test]
fn opened_files_use_the_host_path() {
    let f = fake_file("opened", b'I', 64);
    let opened = json!({"file": {"name": "a.czi", "resourceUri": "host-resource://1"}});
    // no path yet: the viewer is told to ask again
    let r = run(opened.clone(), None);
    let v = r.structured_content.unwrap();
    assert_eq!(v["view"], "pending");
    assert_eq!(v["file"]["name"], "a.czi");
    // with the host's path the file is read
    let r = run(opened, Some(f.display().to_string()));
    assert_eq!(r.structured_content.unwrap()["view"], "image");
    let _ = std::fs::remove_file(f);
}

#[test]
fn summary_when_nothing_to_draw() {
    let info = FileInfo {
        path: "x".into(),
        size_bytes: 0,
        format: openreadout_core::FormatReader::descriptor(&FakeReader),
        format_version: None,
        plane_count: 0,
        images: vec![],
        tables: vec![],
        spectra: vec![],
        traces: vec![],
        notes: vec![],
    };
    assert_eq!(view::auto_kind(&info), view::ViewKind::Summary);
    assert!(view::available(&info).is_empty());
}

// ---------------------------------------------------------------------------------------------
// A scripted MCP session over a byte stream (what stdio carries), with and without the extension.

struct Session {
    lines: tokio::io::Lines<tokio::io::BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>>,
    write: tokio::io::WriteHalf<tokio::io::DuplexStream>,
    id: u64,
}

impl Session {
    async fn start(capabilities: Value) -> Session {
        use rmcp::ServiceExt as _;
        use tokio::io::AsyncBufReadExt as _;
        let (client, server) = tokio::io::duplex(1 << 22);
        let (sr, sw) = tokio::io::split(server);
        tokio::spawn(async move {
            if let Ok(s) = crate::InstrumentServer::new(fake_registry)
                .serve((sr, sw))
                .await
            {
                let _ = s.waiting().await;
            }
        });
        let (cr, cw) = tokio::io::split(client);
        let mut s = Session {
            lines: tokio::io::BufReader::new(cr).lines(),
            write: cw,
            id: 0,
        };
        let init = s
            .call(
                "initialize",
                json!({
                    "protocolVersion": "2025-06-18",
                    "capabilities": capabilities,
                    "clientInfo": {"name": "test-host", "version": "1"},
                }),
            )
            .await;
        assert!(
            init["capabilities"]["extensions"][EXTENSION_ID].is_object(),
            "{init}"
        );
        s.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .await;
        s
    }

    async fn send(&mut self, v: Value) {
        use tokio::io::AsyncWriteExt as _;
        let mut line = v.to_string();
        line.push('\n');
        self.write.write_all(line.as_bytes()).await.unwrap();
    }

    async fn call(&mut self, method: &str, params: Value) -> Value {
        self.id += 1;
        let id = self.id;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await;
        loop {
            let line = self
                .lines
                .next_line()
                .await
                .unwrap()
                .expect("server closed");
            let v: Value = serde_json::from_str(&line).unwrap();
            if v["id"] == json!(id) {
                assert!(v.get("error").is_none(), "{method}: {v}");
                return v["result"].clone();
            }
        }
    }
}

#[tokio::test]
async fn sessions_offer_the_viewer_only_when_negotiated() {
    let f = fake_file("session", b'I', 256);
    let path = f.display().to_string();

    let mut ui = Session::start(json!({
        "extensions": {EXTENSION_ID: {"mimeTypes": [MIME_TYPE]}}
    }))
    .await;
    let tools = ui.call("tools/list", json!({})).await;
    let find = |name: &str| {
        tools["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == name)
            .cloned()
    };
    assert_eq!(
        find("openreadout_info").unwrap()["_meta"]["ui"]["resourceUri"],
        RESOURCE_URI
    );
    assert_eq!(
        find(VIEW_TOOL).unwrap()["_meta"]["ui"]["visibility"],
        json!(["app"])
    );
    let list = ui.call("resources/list", json!({})).await;
    assert!(list["resources"].to_string().contains(RESOURCE_URI));
    let page = ui
        .call("resources/read", json!({"uri": RESOURCE_URI}))
        .await;
    assert_eq!(page["contents"][0]["mimeType"], MIME_TYPE);
    // the model's call carries where the viewer starts; the viewer's call returns the picture
    let info = ui
        .call(
            "tools/call",
            json!({"name": "openreadout_info", "arguments": {"file": path}}),
        )
        .await;
    let hint = info["_meta"][VIEW_META_KEY].clone();
    assert_eq!(hint, json!({"file": path}));
    let view = ui
        .call("tools/call", json!({"name": VIEW_TOOL, "arguments": hint}))
        .await;
    assert_eq!(view["structuredContent"]["view"], "image");
    assert!(
        view["content"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["type"] == "image")
    );
    // a host-opened file: the path arrives in _meta
    let opened = ui
        .call(
            "tools/call",
            json!({
                "name": VIEW_TOOL,
                "arguments": {"file": {"name": "x.czi", "resourceUri": "host-resource://x"}},
                "_meta": {"openai/resource": {"path": path}},
            }),
        )
        .await;
    assert_eq!(opened["structuredContent"]["view"], "image");

    // a client without the extension sees today's server
    let mut plain = Session::start(json!({})).await;
    let tools = plain.call("tools/list", json!({})).await;
    let text = tools.to_string();
    assert!(!text.contains(VIEW_TOOL) && !text.contains(RESOURCE_URI));
    assert_eq!(tools["tools"].as_array().unwrap().len(), 29);
    let list = plain.call("resources/list", json!({})).await;
    assert!(!list.to_string().contains(RESOURCE_URI));
    let info = plain
        .call(
            "tools/call",
            json!({"name": "openreadout_info", "arguments": {"file": path}}),
        )
        .await;
    assert!(info.get("_meta").is_none(), "{info}");
    let _ = std::fs::remove_file(f);
}
