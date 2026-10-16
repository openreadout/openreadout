//! `openreadout_view`: what the viewer draws, sized for display.
//!
//! One call returns one view of one file: a picture of an image plane (as image content), or a
//! plot (sweeps, an NMR spectrum, a chromatogram, a mass spectrum), a plate's per-well values or
//! a sample of flow-cytometry events (in `structuredContent`). Every view is bounded: pictures
//! by [`MAX_IMAGE_SIZE`], plots by [`MAX_POINTS`] points per series (long signals become a
//! min/max envelope so spikes stay visible), events by [`MAX_EVENTS`]. `limits` in the result
//! says what was reduced.

use std::path::Path;

use base64::Engine as _;
use openreadout_core::model::{FileInfo, TableInfo, TraceInfo};
use openreadout_core::{Error, Registry};
use rmcp::ErrorData as McpError;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::schemars;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::mcp_err;

/// Longest side of a picture, in pixels.
pub const MAX_IMAGE_SIZE: u32 = 1600;
/// Longest side of a picture when the viewer does not ask for a size.
pub const DEFAULT_IMAGE_SIZE: u32 = 768;
/// Most points per plotted series.
pub const MAX_POINTS: u32 = 4000;
/// Points per series when the viewer does not ask.
pub const DEFAULT_POINTS: u32 = 1200;
/// Most channels plotted at once in stacked panels.
pub const MAX_CHANNELS: usize = 8;
/// Most channels of a trace whose channels are overlaid in one panel (qPCR wells).
pub const MAX_OVERLAY_CHANNELS: usize = 384;
/// Most plotted values over all channels of one plot.
pub const MAX_PLOT_VALUES: usize = 200_000;
/// Most samples per channel read for one plot (the rest of a longer window is left out).
pub const MAX_SAMPLES_READ: u64 = 20_000_000;
/// Most flow-cytometry events sampled.
pub const MAX_EVENTS: u64 = 50_000;
/// Events sampled when the viewer does not ask.
pub const DEFAULT_EVENTS: u64 = 10_000;
/// A population view gates every event of files with at most this many; larger files are gated
/// in [`GATED_WINDOWS`] windows spread over the run, this many events in all.
pub const MAX_GATED_READ: u64 = 2_000_000;
/// Events gated per read.
pub const GATED_CHUNK: u64 = 250_000;
/// Windows of a larger file gated for a population view.
pub const GATED_WINDOWS: u64 = 16;
/// Most child populations coloured in a gated view.
pub const MAX_GATED_POPULATIONS: usize = 8;
/// Most peaks marked on a plot.
pub const MAX_PEAKS: usize = 60;
/// A chromatogram peak is labelled when it is at least this share of the tallest peak's height
/// (smaller ones can still be hovered).
pub const LABEL_SHARE: f64 = 0.2;
/// A labelled chromatogram peak is also at least this many noise SDs tall, by the peak picker's
/// S/N and by the trace's point-to-point noise.
pub const LABEL_SNR: f64 = 10.0;
/// Most images, traces or tables listed in the outline.
pub const MAX_OUTLINE_ITEMS: usize = 256;
/// Most columns listed per table in the outline.
pub const MAX_OUTLINE_COLUMNS: usize = 128;
/// Most time points per well of a kinetic plate (longer runs are averaged into equal bins).
pub const MAX_TIME_POINTS: usize = 240;
/// Most rows of a plate table read for its time course.
pub const MAX_PLATE_ROWS: u64 = 1_000_000;
/// Most layout maps (samples, groups, ...) and wells per map returned with a plate.
const MAX_LAYOUT_MAPS: usize = 8;
const MAX_LAYOUT_WELLS: usize = 1536;

/// What to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ViewKind {
    /// Pick from what the file holds.
    #[default]
    Auto,
    /// An image plane.
    Image,
    /// Sweeps of a sampled signal or 1-D spectrum.
    Trace,
    /// An NMR spectrum (an FID is processed first) with its peaks.
    Nmr,
    /// A chromatogram: TIC or XIC of a mass-spectrometry run, or a detector trace.
    Chromatogram,
    /// One mass spectrum.
    Spectrum,
    /// A plate's per-well values.
    Plate,
    /// Flow-cytometry events: a scatter plot or histogram.
    Fcs,
    /// Only the outline (the file holds nothing the viewer draws).
    Summary,
    /// An `openreadout_plot` figure drawn again from its arguments (`spec`): in the host's
    /// theme, at the viewer's size, over a zoomed x range.
    Figure,
}

impl ViewKind {
    /// The lowercase name.
    pub fn name(self) -> &'static str {
        match self {
            ViewKind::Auto => "auto",
            ViewKind::Image => "image",
            ViewKind::Trace => "trace",
            ViewKind::Nmr => "nmr",
            ViewKind::Chromatogram => "chromatogram",
            ViewKind::Spectrum => "spectrum",
            ViewKind::Plate => "plate",
            ViewKind::Fcs => "fcs",
            ViewKind::Summary => "summary",
            ViewKind::Figure => "figure",
        }
    }
}

/// The file to view.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum FileRef {
    /// Absolute or working-directory-relative path.
    Path(String),
    /// A file the host opened for the viewer; the host sends its path in
    /// `_meta["openai/resource"].path`.
    Opened {
        /// File name.
        name: String,
        /// The host's URI for the file.
        #[serde(rename = "resourceUri")]
        resource_uri: String,
    },
}

/// Arguments of `openreadout_view`.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct ViewArgs {
    /// The file (every view but `figure`).
    pub file: Option<FileRef>,
    /// What to show (default: picked from the file).
    #[serde(default)]
    pub view: ViewKind,
    /// Also return the file's outline (images, traces, runs, tables). Default true.
    #[serde(default = "yes")]
    pub outline: bool,
    /// Image index; image: default 0.
    pub image: Option<u32>,
    /// Image: channel (default 0).
    pub c: Option<u32>,
    /// Image: z plane (default the middle one).
    pub z: Option<u32>,
    /// Image: time point (default 0).
    pub t: Option<u32>,
    /// Image: pyramid level (default: the one nearest max_size).
    pub level: Option<u32>,
    /// Image: blend the channels (all, or those in `channels`) in their colours.
    #[serde(default)]
    pub composite: bool,
    /// Image: maximum-intensity projection over z.
    #[serde(default)]
    pub mip: bool,
    /// Image volumes: `axial`, `coronal`, `sagittal`, `orthogonal`, `xz`, `yz` or `native`
    /// (default: medical images in their acquisition plane). `z` then counts slices of it.
    pub plane: Option<String>,
    /// Image: `auto`, `min-max`, `percentile:LO,HI`, `raw` or `window:CENTER,WIDTH` (`window:file`,
    /// `window:bone`, ...).
    pub contrast: Option<String>,
    /// Image: only this rectangle, in full-resolution pixels.
    pub region: Option<openreadout_core::Region>,
    /// Image: longest side in pixels (default 768, at most 1600).
    pub max_size: Option<u32>,
    /// Trace, chromatogram from a detector, NMR: trace index.
    pub trace: Option<u32>,
    /// Sweep index (default 0).
    pub sweep: Option<u32>,
    /// Trace: channels to plot (default the first 8); image composite: channels to blend.
    #[serde(default)]
    pub channels: Vec<u32>,
    /// Trace, NMR: first sample of the window (default 0).
    pub first_sample: Option<u64>,
    /// Trace, NMR: samples in the window (default to the end).
    pub count: Option<u64>,
    /// Points per plotted series (default 1200, at most 4000).
    pub points: Option<u32>,
    /// Mass spectrometry: run index (default 0).
    pub run: Option<u32>,
    /// Spectrum: zero-based spectrum index.
    pub index: Option<u64>,
    /// Spectrum: scan number.
    pub scan: Option<u64>,
    /// Spectrum: the scan nearest this retention time, minutes.
    pub rt_min: Option<f64>,
    /// Spectrum: MS level of the scan nearest `rt_min` (default 1).
    pub ms_level: Option<u32>,
    /// Chromatogram: extracted-ion chromatograms at these m/z (default: the TIC).
    #[serde(default)]
    pub mz: Vec<f64>,
    /// Chromatogram: XIC tolerance in ppm (default 10).
    pub ppm: Option<f64>,
    /// Chromatogram: retention-time window; spectrum: m/z window. Axis units, either order.
    pub x_range: Option<[f64; 2]>,
    /// Chromatogram: detect and mark peaks (default: on for detector traces, off for MS runs).
    pub peaks: Option<bool>,
    /// Plate, FCS: table index (default 0).
    pub table: Option<u32>,
    /// Plate: value column of a long table.
    pub column: Option<String>,
    /// Plate: the read (1-based `read` column) whose time course is returned (default the first).
    pub read: Option<u32>,
    /// FCS: x parameter ($PnN).
    pub x: Option<String>,
    /// FCS: y parameter ($PnN); empty for a histogram of x.
    pub y: Option<String>,
    /// FCS: events to sample (default 10000, at most 50000).
    pub events: Option<u64>,
    /// FCS: FlowJo workspace whose populations colour the events.
    pub workspace: Option<String>,
    /// FCS: Gating-ML 2.0 file (instead of a workspace).
    pub gatingml: Option<String>,
    /// FCS: workspace sample (default matched by file name).
    pub sample: Option<String>,
    /// FCS with gating: show the events of this population (path or unique name), coloured by
    /// its child populations (default: all events, coloured by the top populations).
    pub population: Option<String>,
    /// Image: outline the objects counted as openreadout_stats objects counts them.
    pub objects: Option<openreadout_core::measure::ObjectsRequest>,
    /// Image: draw the lanes and bands of a gel or blot as openreadout_stats lanes finds them.
    pub lanes: Option<openreadout_core::measure::LanesRequest>,
    /// Figure: the `openreadout_plot` arguments, as the agent sent them.
    pub spec: Option<Value>,
    /// Figure: draw in the dark theme.
    #[serde(default)]
    pub dark: bool,
    /// Figure: width in CSS pixels (default the spec's), and pixels per CSS pixel (default 1,
    /// at most 3), so the picture is sharp on the viewer's screen.
    pub width: Option<u32>,
    /// Figure: pixels per CSS pixel.
    pub scale: Option<f32>,
    /// Figure: the panel `x_range` zooms (its index in the spec's `panels`; default every
    /// panel). Panels that share x (`share` x or both), facets and plate grids zoom together.
    pub panel: Option<usize>,
    /// Figure: the host's background (`#RRGGBB`), drawn in place of the theme's paper so the
    /// figure blends into the page.
    pub paper: Option<String>,
}

fn yes() -> bool {
    true
}

/// `select` strings of `openreadout_preview` (`c=1`, `z=4`, `c=0-2`, `c=0,1,z=2`) as view
/// arguments, parsed as `--select` is. Strings that do not parse are left out.
pub fn select_into(select: &[Value], out: &mut Map<String, Value>) {
    let strings: Vec<String> = select
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    let Ok(sel) =
        openreadout_core::select::Selection::parse(&openreadout_preview::split_select(&strings))
    else {
        return;
    };
    match sel.c.as_slice() {
        [] => {}
        [one] => {
            out.insert("c".into(), json!(one));
        }
        many => {
            out.insert("composite".into(), json!(true));
            out.insert("channels".into(), json!(many));
        }
    }
    if let [one] = sel.z.as_slice() {
        out.insert("z".into(), json!(one));
    }
    if let [one] = sel.t.as_slice() {
        out.insert("t".into(), json!(one));
    }
}

fn err(e: &Error) -> McpError {
    mcp_err(e)
}

fn usage(msg: impl Into<String>) -> McpError {
    mcp_err(&Error::Usage(msg.into()))
}

/// Notes on a view. The viewer shows `shown` under the picture. `model` holds the notes of the
/// shared rendering and analysis code, worded for the model: they go into the tool result's
/// text and `model_notes`, and the viewer leaves them out.
#[derive(Default)]
struct Notes {
    shown: Vec<String>,
    model: Vec<String>,
}

impl Notes {
    fn push(&mut self, note: String) {
        self.shown.push(note);
    }
}

/// Run the view tool. `host_path` is the path a host gave for an opened file.
pub fn call(
    registry: fn() -> Registry,
    a: &ViewArgs,
    host_path: Option<String>,
) -> Result<CallToolResult, McpError> {
    if a.view == ViewKind::Figure {
        return figure(registry, a);
    }
    let path = match (&a.file, host_path) {
        (_, Some(p)) => p,
        (None, None) => return Err(usage("file is required for this view")),
        (Some(FileRef::Path(p)), None) => p.clone(),
        (Some(FileRef::Opened { name, resource_uri }), None) => {
            // The host's first call may come without the path; the viewer calls again and the
            // host adds it to calls from the viewer.
            let v = json!({
                "view": "pending",
                "file": {"name": name, "resourceUri": resource_uri},
                "notes": ["the host did not send this file's path; the viewer asks again"],
            });
            let mut r = CallToolResult::structured(v);
            r.content = vec![ContentBlock::text(format!(
                "waiting for the path of {name}"
            ))];
            return Ok(r);
        }
    };
    let reg = registry();
    let (det, mut ds) = reg.open(Path::new(&path)).map_err(|e| err(&e))?;
    let info = ds.info().map_err(|e| err(&e))?;
    // a sequencing run or read file: its overview is a figure (quality by cycle per lane, read
    // lengths), drawn by the figure view from plot arguments made here
    if a.view == ViewKind::Auto
        && let Some(spec) = sequencing_figure(&path, &info)
    {
        drop(ds);
        let mut b = a.clone();
        b.view = ViewKind::Figure;
        b.spec = Some(spec);
        return figure(registry, &b);
    }
    let kind = match a.view {
        ViewKind::Auto => auto_kind(&info),
        k => k,
    };
    let mut out = Map::new();
    out.insert("view".into(), json!(kind.name()));
    out.insert(
        "file".into(),
        json!({
            "path": path,
            "name": Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned()),
            "format": det.format_id,
            "format_name": info.format.name,
            "family": info.format.family,
            "size_bytes": info.size_bytes,
        }),
    );
    if a.outline {
        out.insert("outline".into(), outline(&info));
    }
    let mut notes = Notes::default();
    let mut limits = Map::new();
    let mut picture: Option<(Vec<u8>, &'static str)> = None;
    let summary = match kind {
        ViewKind::Summary if is_samples(&info) => {
            picture = Some(samples_picture(ds.as_mut(), &info)?);
            format!(
                "{} samples ({}), as the preview draws them",
                info.tables[0].row_count, info.format.name
            )
        }
        ViewKind::Auto | ViewKind::Summary | ViewKind::Figure => {
            format!("outline of {}", info.format.name)
        }
        ViewKind::Image => {
            let (v, bytes, mime) = image(ds.as_mut(), &info, a, &mut notes)?;
            let s = format!(
                "image {} {}x{} {}",
                v["image"]["image"], v["width"], v["height"], mime
            );
            out.extend(v.as_object().cloned().unwrap_or_default());
            picture = Some((bytes, mime));
            limits.insert("max_image_size".into(), json!(MAX_IMAGE_SIZE));
            s
        }
        ViewKind::Trace => {
            let plot = trace(ds.as_mut(), &info, a, &mut notes, &mut limits)?;
            let s = format!("trace {} sweep {}", plot["trace"], plot["sweep"]);
            out.insert("plot".into(), plot);
            s
        }
        ViewKind::Nmr => {
            let plot = nmr(ds.as_mut(), &info, a, &mut notes, &mut limits)?;
            let s = format!(
                "NMR spectrum, {} peaks",
                plot["peaks"].as_array().map_or(0, Vec::len)
            );
            out.insert("plot".into(), plot);
            s
        }
        ViewKind::Chromatogram => {
            drop(ds);
            let plot = chromatogram(&reg, &path, &info, a, &mut notes, &mut limits)?;
            let s = format!(
                "{} chromatogram(s)",
                plot["series"].as_array().map_or(0, Vec::len)
            );
            out.insert("plot".into(), plot);
            s
        }
        ViewKind::Spectrum => {
            let plot = spectrum(ds.as_mut(), &info, a, &mut notes, &mut limits)?;
            let s = format!("spectrum index {}", plot["spectrum"]["index"]);
            out.insert("plot".into(), plot);
            s
        }
        ViewKind::Plate => {
            let g =
                openreadout_preview::plate_grid(ds.as_mut(), &info, a.table, a.column.as_deref())
                    .map_err(|e| err(&e))?;
            notes.model.extend(g.notes.iter().cloned());
            let cells: Vec<Value> = g
                .cells
                .iter()
                .map(|((r, c), v)| json!([r, c, *v as f32]))
                .collect();
            let s = format!("plate {}x{}, {} wells", g.rows, g.columns, cells.len());
            let mut plate = json!({
                "table": g.table, "layout": g.layout, "rows": g.rows, "columns": g.columns,
                "value": g.value, "cells": cells,
            });
            if let Some(t) = info.tables.iter().find(|t| t.index == g.table) {
                if let Some(groups) = well_groups(t) {
                    plate["groups"] = groups;
                }
                if g.layout == "long" {
                    match kinetics(ds.as_mut(), t, a.read, &mut limits) {
                        Ok(Some(k)) => plate["kinetic"] = k,
                        Ok(None) => {}
                        Err(e) => notes.push(format!("no time course: {}", e.message)),
                    }
                }
            }
            out.insert("plate".into(), plate);
            s
        }
        ViewKind::Fcs => {
            let ev = if a.workspace.is_some() || a.gatingml.is_some() {
                drop(ds);
                fcs_gated(&reg, &path, a, &mut notes, &mut limits)?
            } else {
                fcs(ds.as_mut(), &info, a, &mut notes, &mut limits)?
            };
            let s = format!("{} events sampled", ev["sampled"]);
            out.insert("events".into(), ev);
            s
        }
    };
    if !notes.shown.is_empty() {
        out.insert("notes".into(), json!(notes.shown));
    }
    let mut text = format!("viewer data: {summary}");
    if !notes.model.is_empty() {
        text.push_str("; notes: ");
        text.push_str(&notes.model.join("; "));
        out.insert("model_notes".into(), json!(notes.model));
    }
    if !limits.is_empty() {
        out.insert("limits".into(), Value::Object(limits));
    }
    let mut r = CallToolResult::structured(Value::Object(out));
    r.content = vec![ContentBlock::text(text)];
    if let Some((bytes, mime)) = picture {
        r.content.push(ContentBlock::image(
            base64::engine::general_purpose::STANDARD.encode(&bytes),
            mime,
        ));
    }
    Ok(r)
}

/// Most CSS pixels wide a figure is drawn again at.
const MAX_FIGURE_WIDTH: u32 = 2400;

/// `openreadout_plot` again, for the viewer: the agent's spec in the host's theme, at the
/// viewer's width and pixel density, over `x_range` when the figure is a single plot. It never
/// writes a file.
fn figure(registry: fn() -> Registry, a: &ViewArgs) -> Result<CallToolResult, McpError> {
    let raw = a
        .spec
        .clone()
        .ok_or_else(|| usage("view figure needs spec: the openreadout_plot arguments"))?;
    let mut spec = openreadout_plot::parse_spec(raw).map_err(|e| err(&e))?;
    spec.theme = Some(if a.dark {
        openreadout_plot::ThemeName::Dark
    } else {
        openreadout_plot::ThemeName::Light
    });
    spec.paper.clone_from(&a.paper);
    if let Some(w) = a.width {
        spec.width = Some(w.clamp(320, MAX_FIGURE_WIDTH));
        // the height follows the content at the new width
        spec.height = None;
    }
    let mut notes = Vec::new();
    if let Some([x0, x1]) = a.x_range {
        let range = Some([x0.min(x1), x0.max(x1)]);
        if spec.panels.is_empty() {
            // one plot, or its facets or wells, which share their axes
            spec.panel.x.get_or_insert_with(Default::default).range = range;
        } else {
            let shared = matches!(
                spec.share,
                Some(openreadout_plot::Share::X | openreadout_plot::Share::Both)
            );
            let n = spec.panels.len();
            match a.panel {
                Some(i) if i >= n => notes.push(format!(
                    "panel {i} is not in the figure (it has {n}); zoom left out"
                )),
                Some(i) if !shared => {
                    spec.panels[i]
                        .panel
                        .x
                        .get_or_insert_with(Default::default)
                        .range = range;
                }
                _ => {
                    for p in &mut spec.panels {
                        p.panel.x.get_or_insert_with(Default::default).range = range;
                    }
                }
            }
        }
    }
    let reg = crate::with_strict(registry(), spec.strict);
    let p = openreadout_plot::plot(&reg, &spec).map_err(|e| err(&e))?;
    let scale = a.scale.unwrap_or(1.0).clamp(1.0, 3.0);
    let raster = p.figure.to_raster(scale);
    let mut bytes = raster.to_png().map_err(|e| err(&e))?;
    let mut mime = "image/png";
    if bytes.len() > crate::resources::PREVIEW_BUDGET {
        bytes = raster.to_jpeg(88).map_err(|e| err(&e))?;
        mime = "image/jpeg";
    }
    let v = json!({
        "view": "figure",
        "figure": {"width": p.output.width, "height": p.output.height, "panels": p.hover},
        "notes": notes,
    });
    let mut r = CallToolResult::structured(v);
    r.content = vec![
        ContentBlock::text(format!(
            "viewer data: figure {}x{} at {scale}x",
            p.output.width, p.output.height
        )),
        ContentBlock::image(
            base64::engine::general_purpose::STANDARD.encode(&bytes),
            mime,
        ),
    ];
    Ok(r)
}

// ---------------------------------------------------------------------------------------------
// What the file holds

fn axis_of(t: &TraceInfo) -> Option<&Value> {
    t.extra.get("axis")
}

fn axis_str<'a>(t: &'a TraceInfo, key: &str) -> Option<&'a str> {
    axis_of(t).and_then(|a| a.get(key)).and_then(Value::as_str)
}

fn is_retention(t: &TraceInfo) -> bool {
    axis_str(t, "quantity") == Some("retention_time")
}

/// An FID, or a spectrum with a chemical-shift axis (ppm, or Hz with the spectrometer frequency).
fn is_nmr(t: &TraceInfo) -> bool {
    use openreadout_signal::nmr::source;
    source::is_fid(t) || source::spectrum_axis(t).is_some()
}

fn is_plate_table(t: &TableInfo) -> bool {
    let named = |n: &str| {
        t.columns
            .iter()
            .any(|c| c.name.trim().eq_ignore_ascii_case(n))
    };
    let wells = t
        .columns
        .iter()
        .filter(|c| well_name(&c.name))
        .take(2)
        .count();
    wells >= 2 || named("well") || (named("row") && (named("column") || named("col")))
}

fn well_name(n: &str) -> bool {
    let n = n.trim();
    let mut ch = n.chars();
    let Some(r) = ch.next() else { return false };
    let d = ch.as_str();
    r.is_ascii_alphabetic()
        && r.to_ascii_uppercase() <= 'P'
        && (1..=2).contains(&d.len())
        && d.bytes().all(|b| b.is_ascii_digit())
}

fn is_fcs(info: &FileInfo) -> bool {
    info.format.family == "flow-cytometry" && !info.tables.is_empty()
}

/// Does the file hold mass spectra (a run with at least one scan)?
fn has_scans(info: &FileInfo) -> bool {
    info.spectra.iter().any(|s| s.scan_count > 0)
}

/// The most intense `max` of `items` (by `height`), in their original order.
fn tallest<T>(items: Vec<T>, max: usize, height: impl Fn(&T) -> f64) -> Vec<T> {
    if items.len() <= max {
        return items;
    }
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by(|&i, &j| height(&items[j]).total_cmp(&height(&items[i])));
    order.truncate(max);
    order.sort_unstable();
    let mut keep = vec![false; items.len()];
    for i in order {
        keep[i] = true;
    }
    items
        .into_iter()
        .zip(keep)
        .filter_map(|(x, k)| k.then_some(x))
        .collect()
}

/// The views this file supports, best first.
pub fn available(info: &FileInfo) -> Vec<ViewKind> {
    let mut v = Vec::new();
    if !info.images.is_empty() {
        v.push(ViewKind::Image);
    }
    if info.traces.iter().any(is_nmr) {
        v.push(ViewKind::Nmr);
    }
    if has_scans(info) || info.traces.iter().any(is_retention) {
        v.push(ViewKind::Chromatogram);
    }
    if has_scans(info) {
        v.push(ViewKind::Spectrum);
    }
    if is_fcs(info) {
        v.push(ViewKind::Fcs);
    }
    if info.tables.iter().any(is_plate_table) && !is_fcs(info) {
        v.push(ViewKind::Plate);
    }
    if !info.traces.is_empty() {
        v.push(ViewKind::Trace);
    }
    v
}

/// The plot arguments of a sequencing file's overview: an Illumina run's % ≥ Q30, mean quality
/// and error rate by cycle with one line per lane; a FASTQ or BAM file's read lengths and read
/// quality. `None` for other files (nanopore signal opens in the trace view: one sweep per read).
pub fn sequencing_figure(path: &str, info: &FileInfo) -> Option<Value> {
    if info.format.family != "sequencing" {
        return None;
    }
    let kind = |t: &TableInfo| {
        t.extra
            .get("kind")
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    if let Some(t) = info.traces.first()
        && t.extra.get("kind").and_then(Value::as_str) == Some("run_quality_by_cycle")
    {
        let sweeps: Vec<u32> = (0..t.sweep_count.min(8)).collect();
        let has = |c: &str| t.channels.iter().any(|ch| ch.name == c);
        let mut panels = vec![json!({
            "title": "% of base calls ≥ Q30 by cycle",
            "data": {"file": path, "trace": 0, "sweep": sweeps, "y": "percent_q30"},
        })];
        if has("error_rate") {
            panels.push(json!({
                "title": "error rate by cycle (PhiX)",
                "data": {"file": path, "trace": 0, "sweep": sweeps, "y": "error_rate"},
            }));
        }
        panels.push(json!({
            "title": "mean Q score by cycle",
            "data": {"file": path, "trace": 0, "sweep": sweeps, "y": "mean_qscore"},
        }));
        return Some(json!({"panels": panels, "columns": 1, "share": "x"}));
    }
    if info
        .tables
        .first()
        .and_then(kind)
        .is_some_and(|k| k == "fastq_reads" || k == "bam_records")
    {
        return Some(json!({
            "panels": [
                {"title": "read lengths", "data": {"file": path, "show": "histogram", "x": "length"}},
                {"title": "read quality", "data": {"file": path, "show": "histogram", "x": "mean_quality"}},
            ],
            "columns": 2,
        }));
    }
    None
}

/// A table of sample measurements (NanoDrop, Qubit): `tables[0].extra.kind` is `samples`.
fn is_samples(info: &FileInfo) -> bool {
    info.tables
        .first()
        .is_some_and(|t| t.extra.get("kind").and_then(Value::as_str) == Some("samples"))
}

/// The preview of a samples table, for the summary view.
fn samples_picture(
    ds: &mut dyn openreadout_core::Dataset,
    info: &FileInfo,
) -> Result<(Vec<u8>, &'static str), McpError> {
    use openreadout_preview as pv;
    let mut req = pv::PreviewRequest::default();
    req.max_size = DEFAULT_IMAGE_SIZE.clamp(64, MAX_IMAGE_SIZE);
    let r = pv::render(ds, info, &req).map_err(|e| err(&e))?;
    let (out, bytes) = pv::finish_within(
        &r,
        pv::Encoding::Png,
        pv::DEFAULT_JPEG_QUALITY,
        crate::resources::PREVIEW_BUDGET,
    )
    .map_err(|e| err(&e))?;
    Ok((
        bytes,
        if out.encoding == "jpeg" {
            "image/jpeg"
        } else {
            "image/png"
        },
    ))
}

/// The view for [`ViewKind::Auto`].
pub fn auto_kind(info: &FileInfo) -> ViewKind {
    let views = available(info);
    // NanoDrop, Qubit: the samples picture (spectra beside concentrations and ratios) first.
    if is_samples(info) {
        return ViewKind::Summary;
    }
    // qPCR runs: the amplification curves say more than a plate of one number; so do the
    // electropherograms of a chip or tape run.
    if matches!(info.format.family.as_str(), "qpcr" | "electrophoresis")
        && views.contains(&ViewKind::Trace)
    {
        return ViewKind::Trace;
    }
    views.first().copied().unwrap_or(ViewKind::Summary)
}

fn outline(info: &FileInfo) -> Value {
    let images: Vec<Value> = info
        .images
        .iter()
        .take(MAX_OUTLINE_ITEMS)
        .map(|im| {
            let levels: Vec<Value> = if im.resolution_levels.is_empty() {
                vec![json!([im.size_x, im.size_y])]
            } else {
                im.resolution_levels
                    .iter()
                    .map(|l| json!([l.size_x, l.size_y]))
                    .collect()
            };
            let channels: Vec<Value> = (0..im.size_c)
                .take(64)
                .map(|c| {
                    let ch = im.channels.iter().find(|ch| ch.index == c);
                    let (rgb, _) = openreadout_preview::color::channel_color(ch, c as usize);
                    json!({
                        "name": ch.and_then(|ch| ch.name.clone().or_else(|| ch.fluorophore.clone())),
                        "color": openreadout_preview::color::to_hex(rgb),
                    })
                })
                .collect();
            json!({
                "index": im.index, "name": im.name,
                "size_x": im.size_x, "size_y": im.size_y, "size_z": im.size_z,
                "size_c": im.size_c, "size_t": im.size_t,
                "pixel_type": im.pixel_type, "samples_per_pixel": im.samples_per_pixel,
                "physical_size": im.physical_size, "levels": levels, "channels": channels,
                // medical images: display windows, value unit and anatomical orientation, for
                // the window presets and the view menu
                "windows": im.extra.get("windows"),
                "value_unit": im.extra.get("value_unit"),
                "plane": im.extra.get("orientation").and_then(|o| o.get("plane")),
            })
        })
        .collect();
    let traces: Vec<Value> = info
        .traces
        .iter()
        .take(MAX_OUTLINE_ITEMS)
        .map(|t| {
            let channels: Vec<Value> = t
                .channels
                .iter()
                .take(64)
                .map(|c| json!({"name": c.name, "unit": c.unit}))
                .collect();
            json!({
                "index": t.index, "name": t.name, "sweep_count": t.sweep_count,
                "sample_count": t.sample_count, "sample_rate_hz": t.sample_rate_hz,
                "channel_count": t.channels.len(), "channels": channels, "axis": axis_of(t),
                "retention": is_retention(t), "nmr": is_nmr(t),
            })
        })
        .collect();
    let spectra: Vec<Value> = info
        .spectra
        .iter()
        .take(MAX_OUTLINE_ITEMS)
        .map(|s| {
            json!({
                "index": s.index, "name": s.name, "scan_count": s.scan_count,
                "ms_levels": s.ms_levels, "rt_range_s": s.rt_range_s,
            })
        })
        .collect();
    let tables: Vec<Value> = info
        .tables
        .iter()
        .take(MAX_OUTLINE_ITEMS)
        .map(|t| {
            let columns: Vec<Value> = t
                .columns
                .iter()
                .take(MAX_OUTLINE_COLUMNS)
                .map(|c| json!({"name": c.name, "label": c.label, "range": c.range}))
                .collect();
            json!({
                "index": t.index, "name": t.name, "row_count": t.row_count,
                "column_count": t.columns.len(), "columns": columns,
                "plate": is_plate_table(t),
            })
        })
        .collect();
    let views: Vec<&str> = available(info).into_iter().map(ViewKind::name).collect();
    json!({
        "views": views,
        "images": images, "images_total": info.images.len(),
        "traces": traces, "traces_total": info.traces.len(),
        "spectra": spectra,
        "tables": tables, "tables_total": info.tables.len(),
        "notes": info.notes.iter().take(8).collect::<Vec<_>>(),
    })
}

// ---------------------------------------------------------------------------------------------
// Image

fn image(
    ds: &mut dyn openreadout_core::Dataset,
    info: &FileInfo,
    a: &ViewArgs,
    notes: &mut Notes,
) -> Result<(Value, Vec<u8>, &'static str), McpError> {
    use openreadout_preview as pv;
    let mut req = pv::PreviewRequest::default();
    req.image = a.image;
    let mut select = Vec::new();
    if a.composite {
        req.composite = true;
        if !a.channels.is_empty() {
            let list: Vec<String> = a.channels.iter().map(u32::to_string).collect();
            select.push(format!("c={}", list.join(",")));
        }
    } else {
        select.push(format!("c={}", a.c.unwrap_or(0)));
    }
    if let Some(z) = a.z.filter(|_| !a.mip) {
        select.push(format!("z={z}"));
    }
    if let Some(t) = a.t {
        select.push(format!("t={t}"));
    }
    req.select = select;
    req.mip = a.mip.then_some(pv::Axis::Z);
    req.view = a
        .plane
        .as_deref()
        .map(str::parse::<pv::View>)
        .transpose()
        .map_err(|e| err(&e))?;
    req.level = a.level;
    req.region = a.region;
    req.max_size = a
        .max_size
        .unwrap_or(DEFAULT_IMAGE_SIZE)
        .clamp(64, MAX_IMAGE_SIZE);
    req.contrast = a
        .contrast
        .as_deref()
        .unwrap_or("auto")
        .parse()
        .map_err(|e| err(&e))?;
    req.axes = false;
    if let Some(o) = &a.objects {
        req.marks = pv::Marks::Objects;
        req.objects = Some(o.clone());
    }
    if let Some(l) = &a.lanes {
        req.marks = pv::Marks::Lanes;
        req.lanes = Some(l.clone());
    }
    let r = pv::render(ds, info, &req).map_err(|e| err(&e))?;
    let (out, bytes) = pv::finish_within(
        &r,
        pv::Encoding::Png,
        pv::DEFAULT_JPEG_QUALITY,
        crate::resources::PREVIEW_BUDGET,
    )
    .map_err(|e| err(&e))?;
    // The rendering's own notes are worded for the model (rulers, contrast screening).
    // Fitting the byte budget only shrinks the picture further, and the viewer loads full
    // detail when the user zooms in, so that is not news to anyone.
    notes.model.extend(r.output.notes.iter().cloned());
    let mime = if out.encoding == "jpeg" {
        "image/jpeg"
    } else {
        "image/png"
    };
    let v = json!({
        "image": out.image,
        "width": out.width,
        "height": out.height,
        "encoding": out.encoding,
    });
    Ok((v, bytes, mime))
}

// ---------------------------------------------------------------------------------------------
// Plots

/// Points per series for this call.
fn points(a: &ViewArgs) -> usize {
    a.points.unwrap_or(DEFAULT_POINTS).clamp(16, MAX_POINTS) as usize
}

/// Min/max of the samples that fall in each of `buckets` equal slices of `n` samples; when
/// `n <= buckets` every sample is its own slice.
struct Envelope {
    n: u64,
    lo: Vec<f64>,
    hi: Vec<f64>,
}

impl Envelope {
    fn new(n: u64, buckets: usize) -> Self {
        let b = (n.min(buckets as u64)).max(1) as usize;
        Envelope {
            n: n.max(1),
            lo: vec![f64::INFINITY; b],
            hi: vec![f64::NEG_INFINITY; b],
        }
    }
    fn bucket(&self, i: u64) -> usize {
        let b = self.lo.len() as u128;
        ((u128::from(i) * b / u128::from(self.n)) as usize).min(self.lo.len() - 1)
    }
    fn push(&mut self, i: u64, v: f64) {
        if !v.is_finite() {
            return;
        }
        let k = self.bucket(i);
        self.lo[k] = self.lo[k].min(v);
        self.hi[k] = self.hi[k].max(v);
    }
    /// Samples per slice.
    fn per_bucket(&self) -> f64 {
        self.n as f64 / self.lo.len() as f64
    }
    fn series(&self, name: &str, unit: Option<&str>) -> Value {
        let f = |v: &Vec<f64>| -> Vec<Option<f32>> {
            v.iter()
                .map(|x| x.is_finite().then_some(*x as f32))
                .collect()
        };
        if self.per_bucket() <= 1.0 {
            json!({"name": name, "unit": unit, "y": f(&self.lo)})
        } else {
            json!({"name": name, "unit": unit, "lo": f(&self.lo), "hi": f(&self.hi)})
        }
    }
}

fn trace(
    ds: &mut dyn openreadout_core::Dataset,
    info: &FileInfo,
    a: &ViewArgs,
    notes: &mut Notes,
    limits: &mut Map<String, Value>,
) -> Result<Value, McpError> {
    let ti = a
        .trace
        .unwrap_or_else(|| info.traces.first().map_or(0, |t| t.index));
    let t = info
        .traces
        .iter()
        .find(|t| t.index == ti)
        .ok_or_else(|| {
            usage(if info.traces.is_empty() {
                "this file holds no traces".to_string()
            } else {
                format!("trace {ti} out of range ({} traces)", info.traces.len())
            })
        })?
        .clone();
    let sweep = a.sweep.unwrap_or(0);
    if sweep >= t.sweep_count.max(1) {
        return Err(usage(format!(
            "sweep {sweep} out of range (trace {ti} has {} sweeps)",
            t.sweep_count
        )));
    }
    // Voltammetry reads as current against potential, as the preview draws it.
    if a.channels.is_empty()
        && a.first_sample.is_none()
        && a.count.is_none()
        && let Some(v) = openreadout_preview::electrochem::voltammogram(ds, info, Some(ti), a.sweep)
            .map_err(|e| err(&e))?
    {
        return Ok(voltammogram_plot(&v, &t, a, notes));
    }
    let nch = t.channels.len();
    if nch == 0 {
        return Err(usage(format!("trace {ti} has no channels")));
    }
    // An irregular abscissa stored as a channel is the x axis, not a signal.
    let x_channel = axis_of(&t)
        .filter(|ax| ax.get("irregular").and_then(Value::as_bool) == Some(true))
        .and_then(|ax| ax.get("channel"))
        .and_then(Value::as_u64)
        .and_then(|c| u32::try_from(c).ok());
    // Overlaid curves (qPCR wells, spectra of a series) are drawn in one panel, so many fit.
    // A Sanger trace's four bases are read together, in one panel.
    let kind = t.extra.get("kind").and_then(Value::as_str);
    let overlay = t.extra.get("plot").and_then(Value::as_str) == Some("overlay")
        || kind == Some("sanger_trace");
    // electrophoresis channels are coloured by their base or dye, as analysis software does
    let electrophoresis = matches!(kind, Some("sanger_trace" | "electropherogram"));
    let max_channels = if overlay {
        MAX_OVERLAY_CHANNELS
    } else {
        MAX_CHANNELS
    };
    let channels: Vec<u32> = if a.channels.is_empty() {
        (0..nch as u32)
            .filter(|c| Some(*c) != x_channel || nch == 1)
            // a channel holding each sample's own position (migration time) is not a signal
            .filter(|c| {
                t.channels[*c as usize]
                    .extra
                    .get("role")
                    .and_then(Value::as_str)
                    != Some("position")
            })
            .take(max_channels)
            .collect()
    } else {
        a.channels
            .iter()
            .copied()
            .filter(|c| (*c as usize) < nch)
            .take(max_channels)
            .collect()
    };
    if channels.is_empty() {
        return Err(usage(format!(
            "no such channel in trace {ti} ({nch} channels)"
        )));
    }
    let signal_channels = nch - usize::from(x_channel.is_some() && nch > 1);
    if a.channels.is_empty() && signal_channels > max_channels {
        notes.push(format!(
            "plotting the first {max_channels} of {signal_channels} channels"
        ));
    }
    let sweep_len = openreadout_core::trace::sweep_samples(&t, sweep);
    // A whole Sanger read is unreadable at once: open on about 80 bases where the calls
    // become reliable, as the preview does; zooming out shows the rest.
    let read_window =
        if kind == Some("sanger_trace") && a.first_sample.is_none() && a.count.is_none() {
            sanger_window(ds, info, &t)
        } else {
            None
        };
    let first = a
        .first_sample
        .or(read_window.map(|w| w.0))
        .unwrap_or(0)
        .min(sweep_len);
    let count = a
        .count
        .or(read_window.map(|w| w.1))
        .unwrap_or(sweep_len - first)
        .min(sweep_len - first);
    if read_window.is_some() {
        notes.push(
            "showing about 80 bases where the calls become reliable; zoom out for the whole read"
                .into(),
        );
    }
    let n = count.min(MAX_SAMPLES_READ);
    if n < count {
        notes.push(format!(
            "read the first {n} of {count} samples of the window; zoom in to see the rest"
        ));
    }
    // Many channels share the point budget.
    let buckets = points(a).min((MAX_PLOT_VALUES / channels.len()).max(16));
    let mut env: Vec<Envelope> = channels.iter().map(|_| Envelope::new(n, buckets)).collect();
    let mut xs: Option<Vec<Option<f32>>> =
        x_channel.map(|_| vec![None; env.first().map_or(0, |e| e.lo.len())]);
    let batch = ((1u64 << 23) / nch as u64).clamp(4096, 1 << 20);
    let mut pos = 0u64;
    while pos < n {
        let want = batch.min(n - pos);
        let tr = ds
            .read_trace(ti, sweep, first + pos, want)
            .map_err(|e| err(&e))?;
        let got = tr.channels.first().map_or(0, Vec::len) as u64;
        for (k, &c) in channels.iter().enumerate() {
            let data = tr
                .channels
                .get(c as usize)
                .ok_or_else(|| err(&Error::Other(format!("reader returned no channel {c}"))))?;
            for (i, &v) in data.iter().enumerate() {
                env[k].push(pos + i as u64, v);
            }
        }
        if let (Some(xc), Some(xs)) = (x_channel, xs.as_mut())
            && let Some(data) = tr.channels.get(xc as usize)
            && let Some(e) = env.first()
        {
            for (i, &v) in data.iter().enumerate() {
                let k = e.bucket(pos + i as u64);
                if xs[k].is_none() && v.is_finite() {
                    xs[k] = Some(v as f32);
                }
            }
        }
        if got == 0 {
            break;
        }
        pos += got;
    }
    let per_bucket = env.first().map_or(1.0, Envelope::per_bucket);
    if per_bucket > 1.0 {
        limits.insert(
            "samples_per_point".into(),
            json!((per_bucket * 1000.0).round() / 1000.0),
        );
    }
    let series: Vec<Value> = channels
        .iter()
        .zip(&env)
        .map(|(&c, e)| {
            let ch = &t.channels[c as usize];
            let mut s = e.series(&ch.name, ch.unit.as_deref());
            s["channel"] = json!(c);
            let dye = ch.extra.get("dye").and_then(Value::as_str);
            if electrophoresis
                && let Some(i) = openreadout_preview::series_index_of(if ch.name.len() == 1 {
                    &ch.name
                } else {
                    dye.unwrap_or(&ch.name)
                })
            {
                s["palette"] = json!(i);
            }
            s
        })
        .collect();
    // x of sample i of the sweep: axis.first + i * axis.step for a regular axis, else time.
    let x = match axis_of(&t) {
        Some(ax) if ax.get("irregular").and_then(Value::as_bool) != Some(true) => json!({
            "quantity": ax.get("quantity"), "unit": ax.get("unit"),
            "first": ax.get("first"), "step": ax.get("step"),
            "reversed": ax.get("unit").and_then(Value::as_str).is_some_and(|u| u.eq_ignore_ascii_case("ppm")),
        }),
        Some(ax) => json!({
            "quantity": ax.get("quantity"), "unit": ax.get("unit"), "irregular": true,
        }),
        None if t.sample_rate_hz > 0.0 => {
            let start = if t.sweep_count > 1 {
                0.0
            } else {
                t.start_s.unwrap_or(0.0)
            };
            json!({"quantity": "time", "unit": "s", "first": start, "step": 1.0 / t.sample_rate_hz})
        }
        None => json!({"quantity": "sample", "unit": null, "first": 0, "step": 1}),
    };
    let mut plot = json!({
        "kind": "trace",
        "trace": ti, "sweep": sweep, "sweep_count": t.sweep_count,
        "sweep_sample_count": sweep_len, "first_sample": first, "count": count, "read": n,
        "samples_per_point": per_bucket, "x": x, "series": series,
        "overlay": overlay,
    });
    if let Some(xs) = xs {
        plot["xs"] = json!(xs);
    }
    Ok(plot)
}

/// The scans of about 80 bases of a Sanger read, from the first run of 20 calls averaging
/// quality 20 or more: (first scan, scan count). `None` without a base-call table.
fn sanger_window(
    ds: &mut dyn openreadout_core::Dataset,
    info: &FileInfo,
    t: &TraceInfo,
) -> Option<(u64, u64)> {
    let k = t.extra.get("base_calls_table").and_then(Value::as_u64)?;
    let tab = info.tables.iter().find(|x| u64::from(x.index) == k)?;
    let col = |name: &str| tab.columns.iter().position(|c| c.name == name);
    let (sc, qc) = (col("scan")?, col("quality"));
    let rows = ds.read_table(tab.index, 0, tab.row_count).ok()?;
    let scans = rows.columns.get(sc)?;
    let q: Vec<f64> = qc
        .and_then(|c| rows.columns.get(c).cloned())
        .unwrap_or_default();
    let n = scans.len();
    if n < 2 {
        return None;
    }
    let start = (0..n.saturating_sub(20))
        .find(|&i| {
            q.get(i..i + 20)
                .is_some_and(|w| w.iter().sum::<f64>() / 20.0 >= 20.0)
        })
        .unwrap_or(0);
    let end = (start + 80).min(n - 1);
    let a = scans[start].max(0.0) as u64;
    let b = scans[end].max(0.0) as u64;
    (b > a).then_some((a.saturating_sub(10), b - a + 20))
}

/// A voltammogram as a plot: one series per cycle, current against potential.
fn voltammogram_plot(
    v: &openreadout_preview::electrochem::Voltammogram,
    t: &TraceInfo,
    a: &ViewArgs,
    notes: &mut Notes,
) -> Value {
    for n in &v.notes {
        notes.push(n.clone());
    }
    let cap = points(a);
    let series: Vec<Value> = v
        .cycles
        .iter()
        .map(|(name, pts)| {
            let step = pts.len().div_ceil(cap).max(1);
            let (xs, ys): (Vec<f32>, Vec<f32>) = pts
                .iter()
                .step_by(step)
                .map(|p| (p.0 as f32, p.1 as f32))
                .unzip();
            json!({
                "name": if name.is_empty() { v.current.1.clone() } else { format!("cycle {name}") },
                "unit": v.current.2, "x": xs, "y": ys,
            })
        })
        .collect();
    json!({
        "kind": "trace",
        "voltammogram": true,
        "trace": v.trace, "sweep": v.sweep, "sweep_count": t.sweep_count,
        "x": {"quantity": "potential", "unit": v.potential.2, "irregular": true},
        "y_unit": v.current.2,
        "series": series,
        "overlay": true,
    })
}

fn nmr(
    ds: &mut dyn openreadout_core::Dataset,
    info: &FileInfo,
    a: &ViewArgs,
    notes: &mut Notes,
    limits: &mut Map<String, Value>,
) -> Result<Value, McpError> {
    use openreadout_signal::nmr;
    let mut q = json!({});
    if let Some(t) = a.trace {
        q["trace"] = json!(t);
    }
    if let Some(s) = a.sweep {
        q["sweep"] = json!(s);
    }
    let q: openreadout_signal::api::NmrQuery =
        serde_json::from_value(q).map_err(|e| usage(e.to_string()))?;
    let req = q.request().map_err(|e| err(&e))?;
    let report = nmr::analyze(ds, info, &req).map_err(|e| err(&e))?;
    let chosen = nmr::load(ds, info, &req).map_err(|e| err(&e))?;
    notes.model.extend(chosen.notes.iter().take(4).cloned());
    let y = &chosen.spectrum.real;
    let ax = &chosen.spectrum.axis;
    let total = y.len() as u64;
    let first = a.first_sample.unwrap_or(0).min(total);
    let count = a.count.unwrap_or(total - first).min(total - first);
    let mut env = Envelope::new(count, points(a));
    for i in 0..count {
        env.push(i, y[(first + i) as usize]);
    }
    if env.per_bucket() > 1.0 {
        limits.insert(
            "samples_per_point".into(),
            json!((env.per_bucket() * 1000.0).round() / 1000.0),
        );
    }
    if report.peaks.len() > MAX_PEAKS {
        notes.push(format!(
            "marked the {MAX_PEAKS} tallest of {} peaks",
            report.peaks.len()
        ));
    }
    let peaks: Vec<Value> = tallest(report.peaks.clone(), MAX_PEAKS, |p| p.height.abs())
        .iter()
        .map(|p| json!({"x": p.ppm, "height": p.height as f32, "label": format!("{:.3}", p.ppm)}))
        .collect();
    Ok(json!({
        "kind": "nmr",
        "trace": chosen.trace, "sweep": req.sweep, "source": chosen.source,
        "nucleus": chosen.nucleus,
        "sweep_sample_count": total, "first_sample": first, "count": count, "read": count,
        "samples_per_point": env.per_bucket(),
        "x": {"quantity": "chemical shift", "unit": "ppm", "first": ax.first_ppm, "step": ax.step_ppm, "reversed": true},
        "series": [env.series(&chosen.nucleus.clone().unwrap_or_else(|| "spectrum".into()), None)],
        "peaks": peaks,
    }))
}

/// Run one analysis kind on `file`, as JSON.
fn analyze(reg: &Registry, file: &str, kind: &str, options: Value) -> Result<Value, McpError> {
    let args: openreadout_batch::analyze::AnalyzeArgs =
        serde_json::from_value(json!({"file": file, "kind": kind, "options": options}))
            .map_err(|e| usage(e.to_string()))?;
    let (out, _) = openreadout_batch::analyze::run(reg, &args).map_err(|e| err(&e))?;
    serde_json::to_value(out).map_err(|e| McpError::internal_error(e.to_string(), None))
}

fn chromatogram(
    reg: &Registry,
    file: &str,
    info: &FileInfo,
    a: &ViewArgs,
    notes: &mut Notes,
    limits: &mut Map<String, Value>,
) -> Result<Value, McpError> {
    let detector = a.trace.or_else(|| {
        (!has_scans(info))
            .then(|| {
                info.traces
                    .iter()
                    .find(|t| is_retention(t))
                    .map(|t| t.index)
            })
            .flatten()
    });
    let mut source = Map::new();
    if let Some(t) = detector {
        source.insert("traces".into(), json!([t]));
        if let Some(c) = a.channels.first() {
            source.insert("channel".into(), json!(c));
        }
        if let Some(s) = a.sweep {
            source.insert("sweep".into(), json!(s));
        }
    } else {
        if a.mz.is_empty() {
            source.insert("tic".into(), json!(true));
        } else {
            source.insert("mz".into(), json!(a.mz));
            if let Some(p) = a.ppm {
                source.insert("ppm".into(), json!(p));
            }
        }
        if let Some(r) = a.run {
            source.insert("run".into(), json!(r));
        }
    }
    if let Some([x0, x1]) = a.x_range {
        source.insert("rt_range".into(), json!([x0.min(x1), x0.max(x1)]));
    }
    let mut copts = source.clone();
    copts.insert("max_points".into(), json!(points(a)));
    let out = analyze(reg, file, "chromatogram", Value::Object(copts))?;
    let mut series = Vec::new();
    let mut unit = None;
    for c in out["chromatograms"].as_array().into_iter().flatten() {
        let f32s = |k: &str| -> Vec<Option<f32>> {
            c[k].as_array()
                .into_iter()
                .flatten()
                .map(|v| v.as_f64().map(|x| x as f32))
                .collect()
        };
        if c["decimated"].as_bool() == Some(true) {
            limits.insert("chromatogram_points".into(), json!(points(a)));
        }
        unit = unit.or_else(|| c["intensity_unit"].as_str().map(str::to_string));
        series.push(json!({
            "name": c["label"], "unit": c["intensity_unit"], "kind": c["kind"],
            "x": c["rt_min"].as_array().map(|v| v.iter().map(|x| x.as_f64().map(|x| (x * 1e5).round() / 1e5)).collect::<Vec<_>>()),
            "y": f32s("intensity"),
            "apex_rt_min": c["apex_rt_min"], "apex_intensity": c["apex_intensity"],
            "points": c["points"],
        }));
    }
    notes.model.extend(
        out["notes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .take(4)
            .map(str::to_string),
    );
    // the point-to-point noise of the first chromatogram, for which peaks get a label
    let step_noise = series
        .first()
        .and_then(|c| c["y"].as_array())
        .map_or(0.0, |ys| {
            let v: Vec<f64> = ys.iter().filter_map(Value::as_f64).collect();
            let mut d: Vec<f64> = v.windows(2).map(|w| (w[1] - w[0]).abs()).collect();
            d.sort_by(f64::total_cmp);
            d.get(d.len() / 2)
                .map_or(0.0, |m| 1.4826 * m / std::f64::consts::SQRT_2)
        });
    let want_peaks = a.peaks.unwrap_or(detector.is_some());
    let mut peaks = Vec::new();
    let mut area_unit = Value::Null;
    if want_peaks {
        match analyze(reg, file, "peaks", Value::Object(source)) {
            Ok(p) => {
                if let Some(c) = p["chromatograms"].as_array().and_then(|c| c.first()) {
                    area_unit = c["area_unit"].clone();
                    let all = c["peaks"].as_array().cloned().unwrap_or_default();
                    if all.len() > MAX_PEAKS {
                        notes.push(format!(
                            "marked the {MAX_PEAKS} tallest of {} peaks",
                            all.len()
                        ));
                    }
                    let all = tallest(all, MAX_PEAKS, |p| p["height"].as_f64().unwrap_or(0.0));
                    let top = all
                        .iter()
                        .filter_map(|p| p["height"].as_f64())
                        .fold(0.0, f64::max);
                    for pk in &all {
                        let h = pk["height"].as_f64().unwrap_or(0.0);
                        // the picker's S/N, and the height over the trace's own wiggle
                        let snr =
                            pk["snr"]
                                .as_f64()
                                .unwrap_or(f64::INFINITY)
                                .min(if step_noise > 0.0 {
                                    h / step_noise
                                } else {
                                    f64::INFINITY
                                });
                        peaks.push(json!({
                            "x": pk["rt_min"], "from": pk["start_min"], "to": pk["end_min"],
                            "height": pk["height"], "area": pk["area"],
                            "area_percent": pk["area_percent"],
                            "label": pk["number"],
                            // every peak can be hovered; only the prominent ones are labelled
                            "labelled": h >= top * LABEL_SHARE && snr >= LABEL_SNR,
                        }));
                    }
                }
            }
            Err(e) => notes.push(format!("no peaks: {}", e.message)),
        }
    }
    Ok(json!({
        "kind": "chromatogram",
        "source": if detector.is_some() { "trace" } else { "spectra" },
        "trace": detector, "run": out["run"], "spectra_read": out["spectra_read"],
        "x": {"quantity": "retention time", "unit": "min"},
        "y_unit": unit,
        "series": series, "peaks": peaks, "area_unit": area_unit,
        "x_range": a.x_range,
    }))
}

fn spectrum(
    ds: &mut dyn openreadout_core::Dataset,
    info: &FileInfo,
    a: &ViewArgs,
    notes: &mut Notes,
    limits: &mut Map<String, Value>,
) -> Result<Value, McpError> {
    use openreadout_core::SpectrumView;
    let run = a.run.unwrap_or(0);
    let scan_count = info
        .spectra
        .iter()
        .find(|s| s.index == run)
        .map(|s| s.scan_count)
        .ok_or_else(|| {
            usage(if info.spectra.is_empty() {
                "this file holds no mass spectra".to_string()
            } else {
                format!("run {run} out of range ({} runs)", info.spectra.len())
            })
        })?;
    let index = if let Some(i) = a.index {
        i
    } else if let Some(rt) = a.rt_min {
        let level = a.ms_level.unwrap_or(1);
        let target = rt * 60.0;
        let mut best: Option<(f64, u64)> = None;
        openreadout_core::scans::visit_scans(ds, run, &mut |h| {
            if h.ms_level == level
                && let Some(t) = h.rt_s
            {
                let d = (t - target).abs();
                if best.is_none_or(|(bd, _)| d < bd) {
                    best = Some((d, h.index));
                }
            }
            true
        })
        .map_err(|e| err(&e))?;
        best.map(|(_, i)| i).ok_or_else(|| {
            usage(format!(
                "no MS{level} scan with a retention time in run {run}"
            ))
        })?
    } else if let Some(n) = a.scan {
        openreadout_core::reader::spectrum_by_scan(ds, run, n, SpectrumView::Primary)
            .map_err(|e| err(&e))?
            .index
    } else {
        0
    };
    if index >= scan_count {
        return Err(usage(format!(
            "spectrum {index} out of range (run {run} has {scan_count})"
        )));
    }
    let sp = ds
        .read_spectrum_view(run, index, SpectrumView::Primary)
        .map_err(|e| err(&e))?;
    let total = sp.mz.len();
    let max = points(a);
    let (lo_mz, hi_mz) = match a.x_range {
        Some([x0, x1]) => (x0.min(x1), x0.max(x1)),
        None => (f64::NEG_INFINITY, f64::INFINITY),
    };
    let inside: Vec<usize> = (0..total)
        .filter(|&i| sp.mz[i] >= lo_mz && sp.mz[i] <= hi_mz && sp.intensity.get(i).is_some())
        .collect();
    let round = |x: f64| (x * 1e5).round() / 1e5;
    let (style, xs, ys): (&str, Vec<f64>, Vec<f32>) = if sp.centroided {
        // Sticks: the most intense `max` peaks, in m/z order.
        let mut keep = inside.clone();
        if keep.len() > max {
            keep.sort_by(|&i, &j| sp.intensity[j].total_cmp(&sp.intensity[i]));
            keep.truncate(max);
            keep.sort_unstable();
            limits.insert("peaks_shown".into(), json!(max));
            notes.push(format!(
                "showing the {max} most intense of {} centroids",
                inside.len()
            ));
        }
        (
            "sticks",
            keep.iter().map(|&i| round(sp.mz[i])).collect(),
            keep.iter().map(|&i| sp.intensity[i]).collect(),
        )
    } else if inside.len() > max {
        // Profile: the most intense point of each of `max` equal m/z slices.
        let (a0, a1) = (sp.mz[inside[0]], sp.mz[inside[inside.len() - 1]]);
        let w = (a1 - a0).max(f64::MIN_POSITIVE);
        let mut best: Vec<Option<usize>> = vec![None; max];
        for &i in &inside {
            let k = (((sp.mz[i] - a0) / w * max as f64) as usize).min(max - 1);
            if best[k].is_none_or(|j| sp.intensity[i] > sp.intensity[j]) {
                best[k] = Some(i);
            }
        }
        limits.insert("profile_points".into(), json!(max));
        let keep: Vec<usize> = best.into_iter().flatten().collect();
        (
            "profile",
            keep.iter().map(|&i| round(sp.mz[i])).collect(),
            keep.iter().map(|&i| sp.intensity[i]).collect(),
        )
    } else {
        (
            "profile",
            inside.iter().map(|&i| round(sp.mz[i])).collect(),
            inside.iter().map(|&i| sp.intensity[i]).collect(),
        )
    };
    Ok(json!({
        "kind": "spectrum",
        "run": run, "scan_count": scan_count,
        "spectrum": {
            "index": sp.index, "scan_number": sp.scan_number, "ms_level": sp.ms_level,
            "rt_min": sp.rt_s.map(|s| s / 60.0), "polarity": sp.polarity,
            "centroided": sp.centroided, "precursor_mz": sp.precursor_mz,
            "precursor_charge": sp.precursor_charge, "scan_filter": sp.scan_filter,
            "base_peak_mz": sp.base_peak_mz, "point_count": total,
        },
        "x": {"quantity": "m/z", "unit": null},
        "x_range": a.x_range,
        "series": [{"name": format!("scan {}", sp.scan_number), "style": style, "x": xs, "y": ys}],
    }))
}

// ---------------------------------------------------------------------------------------------
// Plates

/// The plate layout a reader stored with a table (`extra.layout`: one map of well name → label
/// per kind, such as samples and groups), capped to [`MAX_LAYOUT_MAPS`] maps.
fn well_groups(t: &TableInfo) -> Option<Value> {
    let maps = t.extra.get("layout")?.as_object()?;
    let out: Map<String, Value> = maps
        .iter()
        .filter_map(|(name, m)| {
            let wells: Map<String, Value> = m
                .as_object()?
                .iter()
                .filter(|(_, v)| v.is_string())
                .take(MAX_LAYOUT_WELLS)
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            (!wells.is_empty()).then(|| (name.clone(), Value::Object(wells)))
        })
        .take(MAX_LAYOUT_MAPS)
        .collect();
    (!out.is_empty()).then_some(Value::Object(out))
}

/// The value of row `i` of column `c`, NaN when absent.
fn cell(cols: &[Vec<f64>], c: Option<usize>, i: usize) -> f64 {
    c.and_then(|c| cols.get(c))
        .and_then(|v| v.get(i))
        .copied()
        .unwrap_or(f64::NAN)
}

/// The time course of every well of a long plate table (`row`/`col` or `well`, `time_s`,
/// `value`, optionally `read` and `wavelength_nm`), on at most [`MAX_TIME_POINTS`] time points.
/// `None` when the table holds one time point.
fn kinetics(
    ds: &mut dyn openreadout_core::Dataset,
    table: &TableInfo,
    read: Option<u32>,
    limits: &mut Map<String, Value>,
) -> Result<Option<Value>, McpError> {
    let idx = |names: &[&str]| {
        table.columns.iter().position(|c| {
            names
                .iter()
                .any(|nrows| c.name.trim().eq_ignore_ascii_case(nrows))
        })
    };
    let (Some(ti), Some(vi)) = (idx(&["time_s"]), idx(&["value"])) else {
        return Ok(None);
    };
    let (ri, ci, wi) = (idx(&["row"]), idx(&["col", "column"]), idx(&["well"]));
    // Well names of a `well` index column (`extra.categories`), when there is no row and column.
    let well_names: Option<Vec<Option<(u32, u32)>>> = wi
        .filter(|_| ri.is_none() || ci.is_none())
        .and_then(|w| table.columns[w].extra.get("categories"))
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .map(|nrows| nrows.as_str().and_then(parse_well))
                .collect()
        });
    if (ri.is_none() || ci.is_none()) && well_names.is_none() {
        return Ok(None);
    }
    let (readi, wli) = (idx(&["read"]), idx(&["wavelength_nm"]));
    let total = table.row_count.min(MAX_PLATE_ROWS);
    if total < table.row_count {
        limits.insert("plate_rows_read".into(), json!(total));
    }
    let mut cols: Vec<Vec<f64>> = vec![Vec::new(); table.columns.len()];
    let mut pos = 0u64;
    while pos < total {
        let tab = ds
            .read_table(table.index, pos, (total - pos).min(262_144))
            .map_err(|e| err(&e))?;
        let got = tab.columns.first().map_or(0, Vec::len) as u64;
        if got == 0 {
            break;
        }
        let needed = [Some(ti), Some(vi), ri, ci, wi, readi, wli];
        for (j, (dst, src)) in cols.iter_mut().zip(tab.columns).enumerate() {
            if needed.contains(&Some(j)) {
                dst.extend(src);
            }
        }
        pos += got;
    }
    let nrows = cols.get(vi).map_or(0, Vec::len);
    // Base of row and column numbers: 1 unless a 0 occurs.
    let base = |c: Option<usize>| {
        if c.and_then(|c| cols.get(c))
            .is_some_and(|v| v.contains(&0.0))
        {
            0.0
        } else {
            1.0
        }
    };
    let (rb, cb) = (base(ri), base(ci));
    let position = |i: usize| -> Option<(u32, u32)> {
        if let Some(names) = &well_names {
            let w = cell(&cols, wi, i);
            return (w.is_finite() && w >= 0.0)
                .then(|| names.get(w as usize).copied().flatten())
                .flatten();
        }
        let (r, c) = (cell(&cols, ri, i) - rb, cell(&cols, ci, i) - cb);
        (r >= 0.0 && c >= 0.0 && r < 64.0 && c < 64.0 && r.fract() == 0.0 && c.fract() == 0.0)
            .then_some((r as u32, c as u32))
    };
    // Which read, and its first wavelength.
    let mut reads: Vec<u32> = (0..nrows)
        .map(|i| cell(&cols, readi, i))
        .filter(|r| r.is_finite() && *r >= 0.0)
        .map(|r| r as u32)
        .collect();
    reads.sort_unstable();
    reads.dedup();
    let chosen = match read {
        Some(r) if reads.contains(&r) || reads.is_empty() => Some(r),
        Some(r) => {
            return Err(usage(format!(
                "read {r} is not in table {} (reads {reads:?})",
                table.index
            )));
        }
        None => reads.first().copied(),
    };
    let of_read = |i: usize| {
        chosen.is_none_or(|r| readi.is_none() || (cell(&cols, readi, i) - f64::from(r)).abs() < 0.5)
    };
    let wavelength = (0..nrows)
        .filter(|&i| of_read(i))
        .map(|i| cell(&cols, wli, i))
        .filter(|w| w.is_finite())
        .fold(f64::INFINITY, f64::min);
    let keep = |i: usize| {
        of_read(i) && {
            let w = cell(&cols, wli, i);
            !wavelength.is_finite() || !w.is_finite() || w.to_bits() == wavelength.to_bits()
        }
    };
    let mut times: Vec<f64> = (0..nrows)
        .filter(|&i| keep(i))
        .map(|i| cell(&cols, Some(ti), i))
        .filter(|x| x.is_finite())
        .collect();
    times.sort_by(f64::total_cmp);
    times.dedup();
    if times.len() < 2 {
        return Ok(None);
    }
    // The time grid: the time points themselves, or equal bins when there are too many.
    let (t0, t1) = (times[0], times[times.len() - 1]);
    let bucketed = times.len() > MAX_TIME_POINTS;
    let bins = times.len().min(MAX_TIME_POINTS);
    let slot = |x: f64| -> usize {
        if bucketed {
            (((x - t0) / (t1 - t0) * bins as f64) as usize).min(bins - 1)
        } else {
            times.partition_point(|t| *t < x).min(bins - 1)
        }
    };
    let grid: Vec<f64> = if bucketed {
        (0..bins)
            .map(|j| t0 + (j as f64 + 0.5) * (t1 - t0) / bins as f64)
            .collect()
    } else {
        times.clone()
    };
    if bucketed {
        limits.insert("time_points".into(), json!(bins));
    }
    let mut wells: std::collections::BTreeMap<(u32, u32), Vec<(f64, u32)>> = Default::default();
    for i in 0..nrows {
        if !keep(i) {
            continue;
        }
        let (x, v) = (cell(&cols, Some(ti), i), cell(&cols, Some(vi), i));
        let Some(p) = position(i) else { continue };
        if !x.is_finite() || !v.is_finite() {
            continue;
        }
        let acc = wells.entry(p).or_insert_with(|| vec![(0.0, 0); bins]);
        let s = &mut acc[slot(x)];
        s.0 += v;
        s.1 += 1;
    }
    let mut values: Vec<f32> = Vec::with_capacity(wells.len() * bins);
    for acc in wells.values() {
        values.extend(acc.iter().map(|(s, c)| {
            if *c > 0 {
                (*s / f64::from(*c)) as f32
            } else {
                f32::NAN
            }
        }));
    }
    let bytes: Vec<u8> = values.iter().flat_map(|x| x.to_le_bytes()).collect();
    let described = table.extra.get("reads").and_then(Value::as_array);
    let reads_json: Vec<Value> = reads
        .iter()
        .map(|r| {
            let d = described.and_then(|d| {
                d.iter()
                    .find(|x| x.get("read").and_then(Value::as_u64) == Some(u64::from(*r)))
            });
            json!({"read": r, "label": d.and_then(|d| d.get("label")), "unit": d.and_then(|d| d.get("unit"))})
        })
        .collect();
    let unit = reads_json
        .iter()
        .find(|r| chosen.is_some_and(|c| r["read"] == json!(c)))
        .and_then(|r| r["unit"].as_str().map(str::to_string));
    Ok(Some(json!({
        "read": chosen, "reads": reads_json,
        "wavelength_nm": wavelength.is_finite().then_some(wavelength),
        "unit": unit,
        "time_s": grid.iter().map(|x| (x * 1000.0).round() / 1000.0).collect::<Vec<_>>(),
        "time_points": times.len(),
        "wells": wells.keys().map(|(r, c)| json!([r, c])).collect::<Vec<_>>(),
        "encoding": "f32le-base64",
        "values": base64::engine::general_purpose::STANDARD.encode(bytes),
    })))
}

/// `A1` / `a01` / `P24` → zero-based (row, column).
fn parse_well(name: &str) -> Option<(u32, u32)> {
    let name = name.trim();
    let mut chars = name.chars();
    let r = chars.next()?.to_ascii_uppercase();
    let digits = chars.as_str();
    if !r.is_ascii_uppercase()
        || digits.is_empty()
        || digits.len() > 2
        || !digits.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let c: u32 = digits.parse().ok()?;
    (c >= 1).then(|| (u32::from(r as u8 - b'A'), c - 1))
}

// ---------------------------------------------------------------------------------------------
// Flow cytometry

fn fcs(
    ds: &mut dyn openreadout_core::Dataset,
    info: &FileInfo,
    a: &ViewArgs,
    notes: &mut Notes,
    limits: &mut Map<String, Value>,
) -> Result<Value, McpError> {
    let ti = a.table.unwrap_or(0);
    let t = info.tables.iter().find(|t| t.index == ti).ok_or_else(|| {
        usage(format!(
            "table {ti} out of range ({} tables)",
            info.tables.len()
        ))
    })?;
    if t.columns.is_empty() {
        return Err(usage(format!("table {ti} has no columns")));
    }
    let find = |name: &str| {
        t.columns
            .iter()
            .position(|c| c.name == name)
            .ok_or_else(|| usage(format!("no parameter named '{name}' in table {ti}")))
    };
    let named = |n: &str| {
        t.columns
            .iter()
            .position(|c| c.name.eq_ignore_ascii_case(n))
    };
    let xi = match &a.x {
        Some(x) => find(x)?,
        None => named("FSC-A").or(named("FSC-H")).unwrap_or(0),
    };
    let yi = match a.y.as_deref() {
        Some("") => None,
        Some(y) => Some(find(y)?),
        None => named("SSC-A")
            .or(named("SSC-H"))
            .or_else(|| (t.columns.len() > 1).then(|| usize::from(xi == 0))),
    };
    let want = a.events.unwrap_or(DEFAULT_EVENTS).clamp(100, MAX_EVENTS);
    let rows = t.row_count;
    let stride = rows.div_ceil(want).max(1);
    if stride > 1 {
        limits.insert("event_stride".into(), json!(stride));
        notes.push(format!(
            "1 in {stride} of {rows} events; values as stored (no compensation)"
        ));
    }
    let mut xs: Vec<f32> = Vec::new();
    let mut ys: Vec<f32> = Vec::new();
    let chunk = 65_536u64;
    let mut pos = 0u64;
    while pos < rows && (xs.len() as u64) < want {
        let tab = ds
            .read_table(ti, pos, chunk.min(rows - pos))
            .map_err(|e| err(&e))?;
        let got = tab.columns.first().map_or(0, Vec::len) as u64;
        if got == 0 {
            break;
        }
        let xcol = tab.columns.get(xi);
        let ycol = yi.and_then(|y| tab.columns.get(y));
        for r in 0..got {
            if !(pos + r).is_multiple_of(stride) {
                continue;
            }
            let x = xcol
                .and_then(|c| c.get(r as usize))
                .copied()
                .unwrap_or(f64::NAN);
            xs.push(x as f32);
            if let Some(c) = ycol {
                ys.push(c.get(r as usize).copied().unwrap_or(f64::NAN) as f32);
            }
        }
        pos += got;
    }
    let b64 = |v: &[f32]| {
        let bytes: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
        base64::engine::general_purpose::STANDARD.encode(bytes)
    };
    let col = |i: usize| {
        let c = &t.columns[i];
        json!({"name": c.name, "label": c.label, "range": c.range})
    };
    Ok(json!({
        "table": ti, "total": rows, "sampled": xs.len(), "stride": stride,
        "x": col(xi), "y": yi.map(col),
        "encoding": "f32le-base64",
        "xs": b64(&xs), "ys": yi.map(|_| b64(&ys)),
    }))
}

/// Flow-cytometry events coloured by the populations of a FlowJo workspace or Gating-ML file:
/// the events of one population (all events by default), with a bit per child population.
fn fcs_gated(
    reg: &Registry,
    path: &str,
    a: &ViewArgs,
    notes: &mut Notes,
    limits: &mut Map<String, Value>,
) -> Result<Value, McpError> {
    use openreadout_fcs::analysis as fa;
    let gating = a
        .workspace
        .as_ref()
        .or(a.gatingml.as_ref())
        .cloned()
        .unwrap_or_default();
    let ti = a.table.unwrap_or(0);
    let mut opts = json!({"table": ti});
    if let Some(w) = &a.workspace {
        opts["workspace"] = json!(w);
    } else {
        opts["gatingml"] = json!(gating);
    }
    if let Some(s) = &a.sample {
        opts["sample"] = json!(s);
    }
    let report = analyze(reg, path, "gate", opts)?;
    let pops = report["populations"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let field = |p: &Value, k: &str| p[k].as_str().unwrap_or_default().to_string();
    let shown: Option<&Value> = match a.population.as_deref().map(str::trim) {
        None | Some("" | "/") => None,
        Some(q) => {
            let want = if q.starts_with('/') {
                q.to_string()
            } else {
                format!("/{q}")
            };
            let by_path = pops.iter().find(|p| field(p, "path") == want);
            let by_name: Vec<&Value> = pops.iter().filter(|p| field(p, "name") == q).collect();
            Some(match (by_path, by_name.as_slice()) {
                (Some(p), _) => p,
                (None, [p]) => *p,
                _ => {
                    return Err(usage(format!(
                        "population '{q}' is not in {gating}, or its name is not unique (give its path)"
                    )));
                }
            })
        }
    };
    let shown_path = shown.map(|p| field(p, "path"));
    let children: Vec<&Value> = pops
        .iter()
        .filter(|p| p["parent"].as_str().map(str::to_string) == shown_path)
        .take(MAX_GATED_POPULATIONS)
        .collect();
    let dims_of = |p: &Value| -> Vec<String> {
        p["dimensions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|d| d["parameter"].as_str().map(str::to_string))
            .collect()
    };
    // Axes: the ones the child gates are drawn on, else the population's own.
    let axes = children
        .first()
        .map(|c| dims_of(c))
        .filter(|d| !d.is_empty())
        .or_else(|| shown.map(dims_of))
        .unwrap_or_default();
    let mut members: Vec<String> = shown_path.iter().cloned().collect();
    members.extend(children.iter().map(|c| field(c, "path")));
    let total_rows = report["event_count"].as_u64().unwrap_or(0);
    // Every event is gated, chunk by chunk, when the file holds at most MAX_GATED_READ of them;
    // a larger file is gated in GATED_WINDOWS windows spread evenly over the acquisition.
    let windows: Vec<(u64, u64)> = if total_rows <= MAX_GATED_READ {
        (0..total_rows.div_ceil(GATED_CHUNK).max(1))
            .map(|k| {
                (
                    k * GATED_CHUNK,
                    GATED_CHUNK.min(total_rows - k * GATED_CHUNK),
                )
            })
            .collect()
    } else {
        let per = MAX_GATED_READ / GATED_WINDOWS;
        (0..GATED_WINDOWS)
            .map(|k| (k * total_rows / GATED_WINDOWS, per))
            .collect()
    };
    let read: u64 = windows.iter().map(|w| w.1).sum();
    if read < total_rows {
        limits.insert("events_read".into(), json!(read));
        notes.push(format!(
            "{read} of {total_rows} events gated, in {} windows spread over the run",
            windows.len()
        ));
    }
    let mut topts = fa::TableOptions::default();
    topts.gating_file = Some(std::path::PathBuf::from(&gating));
    topts.sample = a.sample.clone();
    topts.populations = members;
    topts.compensation = Some(fa::CompensationChoice::Auto);
    // the events of the shown population expected among those read, for an even sample
    let shown_count = shown
        .and_then(|p| p["count"].as_u64())
        .unwrap_or(total_rows);
    let expected = shown_count
        .saturating_mul(read)
        .checked_div(total_rows)
        .unwrap_or(0);
    let want = a.events.unwrap_or(DEFAULT_EVENTS).clamp(100, MAX_EVENTS);
    let stride = expected.div_ceil(want).max(1);
    if stride > 1 {
        limits.insert("event_stride".into(), json!(stride));
    }
    let (mut xs, mut ys, mut bits) = (Vec::new(), Vec::new(), Vec::new());
    let mut in_shown = 0u64;
    let mut first: Option<fa::ProcessedRows> = None;
    let mut axes_at: Option<(usize, Option<usize>)> = None;
    for &(start, count) in &windows {
        let rows = match fa::processed_rows(Path::new(path), ti, start, count, &topts) {
            Ok(r) => r,
            Err(_) if first.is_none() && topts.compensation.is_some() => {
                // no matrix to compensate with: the values as stored
                topts.compensation = None;
                notes.push("values as stored (no compensation matrix)".into());
                fa::processed_rows(Path::new(path), ti, start, count, &topts)
                    .map_err(|e| err(&e))?
            }
            Err(e) => return Err(err(&e)),
        };
        let col = |name: &str| rows.names.iter().position(|n| n == name);
        // the axes, found by name in the first window
        if axes_at.is_none() {
            let named = |n: &str| {
                col(n).ok_or_else(|| usage(format!("no parameter named '{n}' in table {ti}")))
            };
            let xi = match a.x.as_deref() {
                Some(n) => named(n)?,
                None => axes
                    .first()
                    .and_then(|n| col(n))
                    .or_else(|| col("FSC-A"))
                    .unwrap_or(0),
            };
            let yi = match a.y.as_deref() {
                Some("") => None,
                Some(n) => Some(named(n)?),
                None if axes.len() == 1 => None,
                None => axes.get(1).and_then(|n| col(n)).or_else(|| col("SSC-A")),
            };
            axes_at = Some((xi, yi));
        }
        let (xi, yi) = axes_at.unwrap_or((0, None));
        let gate_col = |p: &str| col(&format!("gate:{p}")).and_then(|i| rows.columns.get(i));
        let inside = shown_path.as_deref().and_then(gate_col);
        let child_cols: Vec<Option<&Vec<f64>>> = children
            .iter()
            .map(|c| gate_col(&field(c, "path")))
            .collect();
        let n = rows.columns.first().map_or(0, Vec::len);
        let is_in = |i: usize| inside.is_none_or(|c| c.get(i).copied() == Some(1.0));
        for i in (0..n).filter(|&i| is_in(i)) {
            let k = in_shown;
            in_shown += 1;
            if !k.is_multiple_of(stride) || xs.len() as u64 >= want {
                continue;
            }
            let at = |c: usize| {
                rows.columns
                    .get(c)
                    .and_then(|v| v.get(i))
                    .copied()
                    .unwrap_or(f64::NAN) as f32
            };
            xs.push(at(xi));
            if let Some(y) = yi {
                ys.push(at(y));
            }
            let mut b = 0u8;
            for (j, c) in child_cols.iter().enumerate() {
                if c.and_then(|c| c.get(i)).copied() == Some(1.0) {
                    b |= 1 << j;
                }
            }
            bits.push(b);
        }
        if first.is_none() {
            first = Some(rows);
        }
    }
    let rows = first.ok_or_else(|| usage(format!("table {ti} holds no events")))?;
    let (xi, yi) = axes_at.unwrap_or((0, None));
    let b64 = |v: &[f32]| {
        let bytes: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
        base64::engine::general_purpose::STANDARD.encode(bytes)
    };
    let param =
        |i: usize| json!({"name": rows.names[i], "label": rows.labels.get(i).cloned().flatten()});
    let summary = |p: &Value| {
        json!({
            "path": p["path"], "name": p["name"], "gate_type": p["gate_type"],
            "count": p["count"], "percent_of_parent": p["percent_of_parent"],
            "dimensions": dims_of(p),
        })
    };
    let all = json!({"path": "/", "name": "all events", "count": total_rows});
    let listed: Vec<Value> = pops
        .iter()
        .take(MAX_OUTLINE_ITEMS)
        .map(|p| json!({"path": p["path"], "name": p["name"], "count": p["count"]}))
        .collect();
    Ok(json!({
        "table": ti, "total": total_rows, "sampled": xs.len(), "stride": stride,
        "x": param(xi), "y": yi.map(param),
        "encoding": "f32le-base64",
        "xs": b64(&xs), "ys": yi.map(|_| b64(&ys)),
        "compensated": rows.processing.compensation.is_some(),
        "gating": {
            "file": gating,
            "sample": report["sample"]["name"],
            "shown": shown.map_or(all, summary),
            "in_shown_read": in_shown,
            "children": children.iter().map(|c| summary(c)).collect::<Vec<_>>(),
            "populations": listed,
            "members": base64::engine::general_purpose::STANDARD.encode(&bits),
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_keeps_extremes() {
        let mut e = Envelope::new(10_000, 100);
        for i in 0..10_000u64 {
            e.push(i, if i == 4321 { 99.0 } else { (i % 7) as f64 });
        }
        assert_eq!(e.lo.len(), 100);
        assert!((e.per_bucket() - 100.0).abs() < 1e-9);
        assert_eq!(e.hi[43], 99.0);
        assert_eq!(e.lo[43], 0.0);
        let s = e.series("a", Some("mV"));
        assert_eq!(s["hi"].as_array().unwrap().len(), 100);
        // short signals are plotted as they are
        let mut e = Envelope::new(5, 100);
        for i in 0..5 {
            e.push(i, i as f64);
        }
        assert_eq!(e.series("b", None)["y"], json!([0.0, 1.0, 2.0, 3.0, 4.0]));
    }

    #[test]
    fn select_strings_become_arguments() {
        let mut m = Map::new();
        select_into(&[json!("c=0,2,z=4"), json!("t=1")], &mut m);
        assert_eq!(m["composite"], json!(true));
        assert_eq!(m["channels"], json!([0, 2]));
        assert_eq!(m["z"], json!(4));
        assert_eq!(m["t"], json!(1));
        let mut m = Map::new();
        select_into(&[json!("c=3")], &mut m);
        assert_eq!(m["c"], json!(3));
        // ranges, as `--select` takes them
        let mut m = Map::new();
        select_into(&[json!("c=0-2")], &mut m);
        assert_eq!(m["composite"], json!(true));
        assert_eq!(m["channels"], json!([0, 1, 2]));
        let mut m = Map::new();
        select_into(&[json!("c=0-1,3"), json!("z=2-2")], &mut m);
        assert_eq!(m["channels"], json!([0, 1, 3]));
        assert_eq!(m["z"], json!(2));
    }

    #[test]
    fn wells_are_recognized() {
        assert!(well_name("A1") && well_name("p24") && well_name("H12"));
        assert!(!well_name("FSC-A") && !well_name("Q1") && !well_name("A123"));
    }
}
