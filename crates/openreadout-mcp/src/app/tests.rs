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

/// `web/theme.json` is openreadout-figure's theme as the viewer reads it. When the theme
/// changes, this test rewrites the file and fails once; commit the new file.
#[test]
fn viewer_colours_are_the_figure_engines() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/app/web/theme.json");
    let want = openreadout_figure::Theme::tokens_json();
    let have = std::fs::read_to_string(&path).unwrap_or_default();
    if have != want {
        std::fs::write(&path, &want).unwrap();
        panic!(
            "{} was out of date and has been rewritten; commit it",
            path.display()
        );
    }
    assert!(!html().contains("/*TOKENS*/"));
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
        "openreadout_spectrum",
        Some(&args(json!({"file": "a.mzML", "scan": 12}))),
    )
    .unwrap();
    assert_eq!(h["view"], "spectrum");
    assert_eq!(h["scan"], 12);
    let h = view_hint(
        "openreadout_spectrum",
        Some(&args(json!({"file": "a.mzML", "spectrum": 3}))),
    )
    .unwrap();
    assert_eq!(h["view"], "spectrum");
    assert_eq!(h["index"], 3);
    let h = view_hint(
        "openreadout_scans",
        Some(&args(json!({"file": "a.mzML", "ms_level": 2}))),
    )
    .unwrap();
    assert_eq!(h["view"], "chromatogram");
    let h = view_hint(
        "openreadout_spectrum",
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
        "openreadout_assay",
        Some(&args(
            json!({"file": "plate.xlsx", "analysis": "dose-response"}),
        )),
    )
    .unwrap();
    assert_eq!(h["view"], "plate");
    // the agent's annotations reach the viewer as they were given
    let ann = json!([{"kind": "point", "x": 10, "y": 20, "label": "here"}]);
    let h = view_hint(
        "openreadout_preview",
        Some(&args(
            json!({"file": "/d/a.czi", "annotations": ann.clone()}),
        )),
    )
    .unwrap();
    assert_eq!(h["annotations"], ann);
    // gating: the population asked about, with its workspace
    let h = view_hint(
        "openreadout_gate",
        Some(&args(
            json!({"file": "a.fcs", "workspace": "w.wsp", "populations": ["CD3+", "CD4+"]}),
        )),
    )
    .unwrap();
    assert_eq!(h["view"], "fcs");
    assert_eq!(h["workspace"], "w.wsp");
    assert_eq!(h["population"], "CD3+");
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
            b'P' => {
                // a kinetic plate read: 96 wells, two reads, `n` time points, long layout
                format.family = "plate-reader".into();
                let col = |i: u32, name: &str| ColumnInfo {
                    index: i,
                    name: name.into(),
                    dtype: "f64".into(),
                    ..Default::default()
                };
                let names = ["well", "row", "col", "read", "time_s", "value"];
                let mut t = TableInfo {
                    index: 0,
                    row_count: u64::from(self.n) * 2 * 96,
                    columns: (0..6).map(|i| col(i, names[i as usize])).collect(),
                    ..Default::default()
                };
                let mut group = serde_json::Map::new();
                for w in 0..96u32 {
                    let name = format!("{}{}", char::from(b'A' + (w / 12) as u8), w % 12 + 1);
                    group.insert(name, json!(if w % 12 == 11 { "Blank" } else { "cells" }));
                }
                t.extra.insert("layout".into(), json!({"Group": group}));
                t.extra.insert("plate_rows".into(), json!(8));
                t.extra.insert("plate_columns".into(), json!(12));
                t.extra.insert(
                    "reads".into(),
                    json!([{"read": 1, "label": "OD600", "unit": "OD"}, {"read": 2, "label": "GFP"}]),
                );
                info.tables = vec![t];
            }
            b'O' => {
                // many overlaid curves, like the wells of a qPCR run
                let ch = |name: String| SignalChannelInfo {
                    name,
                    dtype: "f64".into(),
                    scale: 1.0,
                    ..Default::default()
                };
                let mut t = TraceInfo {
                    index: 0,
                    sample_rate_hz: 1.0,
                    sample_count: u64::from(self.n),
                    sweep_count: 1,
                    channels: (0..96).map(|i| ch(format!("W{i} GAPDH"))).collect(),
                    ..Default::default()
                };
                t.extra.insert("plot".into(), json!("overlay"));
                info.traces = vec![t];
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
        let nch = if self.kind == b'O' { 96 } else { 10 };
        let channels = (0..nch)
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
        if self.kind == b'P' {
            let total = u64::from(self.n) * 2 * 96;
            let end = (first + max.min(1000)).min(total);
            let mut cols = vec![Vec::new(); 6];
            for i in first..end {
                let (t, rest) = (i / 192, i % 192);
                let (read, w) = (rest / 96 + 1, rest % 96);
                let v = [
                    w as f64,
                    (w / 12 + 1) as f64,
                    (w % 12 + 1) as f64,
                    read as f64,
                    t as f64 * 600.0,
                    read as f64 * 100.0 + w as f64 + t as f64 * 0.5,
                ];
                for (c, x) in cols.iter_mut().zip(v) {
                    c.push(x);
                }
            }
            return Ok(Table {
                table: 0,
                first_row: first,
                columns: cols,
            });
        }
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

/// A figure drawn again for the viewer: on the host's background, and zoomed on the panel the
/// user dragged on, with y fitted to what the range shows; the other panel keeps its range.
#[test]
fn figure_views_zoom_one_panel_on_the_hosts_background() {
    let line = |k: f64| {
        json!({"values": {"x": (0..=10).map(|i| json!(i)).collect::<Vec<_>>(),
                          "y": (0..=10).map(|i| f64::from(i) * k).collect::<Vec<_>>()}})
    };
    let spec = json!({"panels": [{"data": line(1.0)}, {"data": line(10.0)}]});
    let panels = |r: &CallToolResult| r.structured_content.as_ref().unwrap()["figure"]["panels"].clone();
    let r = run(
        json!({"view": "figure", "spec": spec, "width": 600, "x_range": [2, 4], "panel": 1,
               "paper": "#262624", "dark": true}),
        None,
    );
    let p = panels(&r);
    let dom = |i: usize, k: &str| p[i][k]["domain"].clone();
    assert_eq!(p[1]["spec_panel"], 1);
    assert_eq!(dom(1, "x"), json!([2.0, 4.0]), "{p}");
    let y = dom(1, "y");
    assert!(y[1].as_f64().unwrap() < 60.0, "y fits the zoomed range: {y}");
    assert!(dom(0, "x")[1].as_f64().unwrap() >= 10.0, "panel 0 keeps its range");
    // the figure is drawn on the paper the viewer passes
    let mut s = openreadout_plot::parse_spec(spec.clone()).unwrap();
    s.paper = Some("#262624".into());
    let p = openreadout_plot::plot(&Registry::new(), &s).unwrap();
    assert_eq!(
        p.figure.background,
        Some(openreadout_figure::Color::hex(0x26_2624))
    );
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
    // fitting the byte budget is not something the viewer tells the user
    assert!(v.get("notes").is_none(), "{}", v["notes"]);
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
fn kinetic_plates_carry_time_courses_and_layout() {
    let file = fake_file("plate", b'P', 500);
    let res = run(json!({"file": file, "view": "plate"}), None);
    let out = res.structured_content.unwrap();
    let pl = &out["plate"];
    assert_eq!(pl["groups"]["Group"]["A12"], "Blank");
    let kin = &pl["kinetic"];
    assert_eq!(kin["read"], 1);
    assert_eq!(kin["unit"], "OD");
    assert_eq!(kin["time_points"], 500);
    let times = kin["time_s"].as_array().unwrap();
    assert_eq!(times.len(), view::MAX_TIME_POINTS);
    assert_eq!(kin["wells"].as_array().unwrap().len(), 96);
    assert_eq!(out["limits"]["time_points"], json!(view::MAX_TIME_POINTS));
    let vals = base64::engine::general_purpose::STANDARD
        .decode(kin["values"].as_str().unwrap())
        .unwrap();
    assert_eq!(vals.len(), 96 * view::MAX_TIME_POINTS * 4);
    // well A2 (index 1), first bin: read 1 values 101, 101.5, 102 averaged
    let first = f32::from_le_bytes(vals[view::MAX_TIME_POINTS * 4..][..4].try_into().unwrap());
    assert!((100.5..102.5).contains(&first), "{first}");
    // the other read
    let res = run(
        json!({"file": file, "view": "plate", "read": 2, "outline": false}),
        None,
    );
    assert_eq!(
        res.structured_content.unwrap()["plate"]["kinetic"]["read"],
        2
    );
    // an unknown read leaves the heat map and says why there is no time course
    let res = run(json!({"file": file, "view": "plate", "read": 7}), None);
    let out = res.structured_content.unwrap();
    assert!(out["plate"].get("kinetic").is_none());
    assert!(
        out["notes"].to_string().contains("read 7"),
        "{}",
        out["notes"]
    );
    // a short run keeps its time points as they are
    let short = fake_file("plate-short", b'P', 3);
    let res = run(json!({"file": short, "view": "plate"}), None);
    let kin = &res.structured_content.unwrap()["plate"]["kinetic"];
    assert_eq!(kin["time_s"], json!([0.0, 600.0, 1200.0]));
    let _ = std::fs::remove_file(file);
    let _ = std::fs::remove_file(short);
}

#[test]
fn overlaid_curves_are_not_capped_at_eight() {
    let f = fake_file("overlay", b'O', 40);
    let r = run(json!({"file": f, "view": "trace"}), None);
    let plot = &r.structured_content.unwrap()["plot"];
    assert_eq!(plot["overlay"], true);
    assert_eq!(plot["series"].as_array().unwrap().len(), 96);
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
        let mut s = Session::connect();
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

    /// A running server with no handshake yet.
    fn connect() -> Session {
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
        Session {
            lines: tokio::io::BufReader::new(cr).lines(),
            write: cw,
            id: 0,
        }
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
    assert_eq!(tools["tools"].as_array().unwrap().len(), 28);
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

// Models sometimes send a nested object as a JSON string: the `preview` of openreadout_export,
// and the arguments inside it, are read as the objects they hold.
#[tokio::test]
async fn nested_arguments_sent_as_json_strings_are_read() {
    let f = fake_file("nested-json", b'I', 256);
    let out = f.with_extension("saved.png");
    let mut s = Session::start(json!({
        "extensions": {EXTENSION_ID: {"mimeTypes": [MIME_TYPE]}}
    }))
    .await;
    let preview = json!({
        "file": f.display().to_string(),
        "max_size": 128,
        "annotations": json!([{"type": "point", "x": 10, "y": 10, "label": "a"}]).to_string(),
    })
    .to_string();
    let r = s
        .call(
            "tools/call",
            json!({"name": "openreadout_export", "arguments": {
                "preview": preview, "output": out.display().to_string(),
            }}),
        )
        .await;
    assert_ne!(r["isError"], json!(true), "{r}");
    assert_eq!(r["structuredContent"]["verified"], json!(true), "{r}");
    assert!(out.exists());
    let _ = std::fs::remove_file(out);
    let _ = std::fs::remove_file(f);
}

// Protocol 2026-07-28 has no initialize. The client sends `server/discover`, every request carries
// the version in `_meta`, and list results must carry `ttlMs` and `cacheScope`. Claude Code 2.1.292
// drops a server's whole tool list when they are missing, as it did with rmcp 3.5.0.
#[tokio::test]
async fn sessions_without_initialize_get_cache_hints() {
    let meta = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": {"name": "test-host", "version": "1"},
        "io.modelcontextprotocol/clientCapabilities": {},
    });
    let mut s = Session::connect();
    let discover = s.call("server/discover", json!({"_meta": meta})).await;
    assert!(
        discover["supportedVersions"]
            .to_string()
            .contains("2026-07-28"),
        "{discover}"
    );
    for method in ["tools/list", "prompts/list", "resources/list"] {
        let list = s.call(method, json!({"_meta": meta})).await;
        assert!(list["ttlMs"].is_number(), "{method}: {list}");
        assert!(
            matches!(list["cacheScope"].as_str(), Some("public" | "private")),
            "{method}: {list}"
        );
    }
}
