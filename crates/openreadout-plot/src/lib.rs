//! Figures from lab files for `OpenReadout`.
//!
//! A [`PlotSpec`] names the data of each series by file and selector (a trace, sweep and channel;
//! a chromatogram or the spectrum at a retention time; plate wells by range or layout group; FCS
//! parameters and populations; an analysis result such as f–I curves or dose-response fits; a
//! table file's columns) or gives a few inline values, and says how to arrange them: panels,
//! small multiples, plate positions, shared and log axes, bands, lines, labelled points, arrows.
//! [`plot`] reads the data through the readers and analyses, picks chart types, ranges, units and
//! labels from it, and draws the figure with `openreadout-figure` in the OpenReadout look. The
//! result is the [`Figure`] (PNG, SVG or PDF) and a [`PlotOutput`] that says what was drawn.
//!
//! ```no_run
//! use openreadout_core::Registry;
//! use openreadout_plot::{PlotSpec, plot};
//!
//! let spec: PlotSpec = serde_json::from_value(serde_json::json!({
//!     "data": {"file": "run.mzML", "peaks": 1},
//!     "title": "Total ion current"
//! }))?;
//! let plotted = plot(&Registry::new(), &spec)?;
//! let png = plotted.figure.to_png(1.0)?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![allow(clippy::many_single_char_names, clippy::float_cmp)]

mod draw;
mod load;
mod model;
mod output;
mod pdf;
mod spec;

use std::path::Path;

use openreadout_core::{Error, Registry, Result};
use openreadout_figure::{Anchor, Baseline, Face, Figure, Rect, TextStyle, Theme};
use serde_json::Value;

pub use output::{AxisOutput, Marked, PanelOutput, PlotOutput, SeriesOutput, WrittenFile};
pub use spec::{
    Annotation, AxisSpec, ColorBy, ColorRule, Facet, InlineValues, Legend, MarkKind, OneOrMany,
    PanelItem, PanelSpec, PlotSpec, SeriesSpec, Share, Show, Spread, Subtract, ThemeName,
};

/// Default figure width in px.
pub const DEFAULT_WIDTH: u32 = 1200;
/// Largest width or height in px.
pub const MAX_SIZE: u32 = 3000;

/// A drawn figure.
#[derive(Debug)]
pub struct Plotted {
    /// The figure (render with `to_png`, `to_svg`, or [`to_pdf`]).
    pub figure: Figure,
    /// What was drawn.
    pub output: PlotOutput,
    /// Per panel: its pixel rectangle, axis ranges and thinned series, for hover read-outs.
    pub hover: Value,
}

/// Fields of [`PlotSpec`] and [`PanelSpec`] at the top level, and of a `panels` item.
const TOP_KEYS: &[&str] = &[
    "title", "caption", "panels", "columns", "share", "width", "height", "theme", "output",
    "overwrite", "strict",
];
const PANEL_KEYS: &[&str] = &[
    "data", "facet", "x", "y", "annotate", "legend", "column_labels", "row_labels", "stack",
];

/// Parse a plot spec from JSON, refusing unknown keys with the list of known ones.
pub fn parse_spec(v: Value) -> Result<PlotSpec> {
    if let Value::Object(m) = &v {
        for k in m.keys() {
            if !TOP_KEYS.contains(&k.as_str()) && !PANEL_KEYS.contains(&k.as_str()) {
                return Err(Error::Usage(format!(
                    "unknown plot argument `{k}`; known: {}, {}",
                    PANEL_KEYS.join(", "),
                    TOP_KEYS.join(", ")
                )));
            }
        }
        if let Some(Value::Array(ps)) = m.get("panels") {
            for (i, p) in ps.iter().enumerate() {
                if let Value::Object(pm) = p {
                    for k in pm.keys() {
                        if k != "title" && !PANEL_KEYS.contains(&k.as_str()) {
                            return Err(Error::Usage(format!(
                                "panels[{i}]: unknown key `{k}`; a panel takes title, {}",
                                PANEL_KEYS.join(", ")
                            )));
                        }
                    }
                }
            }
        }
    }
    serde_json::from_value(v).map_err(|e| Error::Usage(format!("plot spec: {e}")))
}

fn theme_for(spec: &PlotSpec) -> Theme {
    match spec.theme.unwrap_or_default() {
        ThemeName::Light => Theme::light(),
        ThemeName::Dark => Theme::dark(),
    }
}

/// Read the data a spec names and draw the figure.
pub fn plot(reg: &Registry, spec: &PlotSpec) -> Result<Plotted> {
    let panels: Vec<(Option<String>, PanelSpec)> = if spec.panels.is_empty() {
        if spec.panel.data.is_none() {
            return Err(Error::Usage(
                "nothing to plot: give `data` (a series or a list of them), or `panels`".into(),
            ));
        }
        vec![(None, spec.panel.clone())]
    } else {
        if spec.panel.data.is_some() {
            return Err(Error::Usage(
                "give either top-level `data` (one panel) or `panels`, not both".into(),
            ));
        }
        spec.panels.iter().map(|p| (p.title.clone(), p.panel.clone())).collect()
    };
    let width = spec.width.unwrap_or(DEFAULT_WIDTH).clamp(320, MAX_SIZE) as f32;
    let theme = theme_for(spec).for_size(width.max(spec.height.unwrap_or(0) as f32).min(1600.0));
    let mut ctx = load::Ctx::new(reg);
    let mut ins: Vec<draw::PanelIn> = Vec::new();
    let mut files: Vec<String> = Vec::new();
    let mut inline_series = 0;
    for (i, (title, ps)) in panels.into_iter().enumerate() {
        draw::check_annotations(&ps.annotate)?;
        ctx.plate = None;
        ctx.x_unit = ps.x.as_ref().and_then(|x| x.unit.clone());
        let items = ps.data.as_ref().map(OneOrMany::to_vec).unwrap_or_default();
        if items.is_empty() {
            return Err(Error::Usage(format!("panel {i}: no `data`")));
        }
        let mut series = Vec::new();
        for item in &items {
            let mut s = load::load(&mut ctx, item).map_err(|e| prefix(e, i, items.len() > 1))?;
            let colour = item.color.as_deref().map(|c| {
                draw::named_color(&theme, c).ok_or_else(|| {
                    Error::Usage(format!(
                        "color `{c}`: ink, accent, muted, faint, blue, ochre, green, purple, teal, brown or #RRGGBB"
                    ))
                })
            });
            let colour = colour.transpose()?;
            for x in &mut s {
                if colour.is_some() {
                    x.color = colour;
                }
                if let Some(l) = &item.label
                    && item.color_by.is_none()
                    && s_many(&items, item)
                {
                    x.class = Some(l.clone());
                }
                match &x.source.file {
                    Some(f) => {
                        if !files.contains(f) {
                            files.push(f.clone());
                        }
                    }
                    None => inline_series += 1,
                }
            }
            series.extend(s);
        }
        if series.is_empty() {
            return Err(Error::Usage(format!("panel {i}: the data selected nothing to draw")));
        }
        if ps.facet == Some(Facet::Plate) && series.iter().all(|s| s.well.is_none()) {
            return Err(Error::Usage(
                "facet=plate lays out plate wells; this data has no wells (facet=series makes one small plot per series)".into(),
            ));
        }
        ins.push(draw::PanelIn { title, spec: ps, series, plate: ctx.plate.take() });
    }
    share_axes(&mut ins, spec.share)?;
    // layout
    let n = ins.len();
    let cols = spec.columns.map_or(n.min(3), |c| (c as usize).clamp(1, n));
    let rows = n.div_ceil(cols);
    let fs = theme.font_size;
    let margin = (fs * 1.3).round();
    let title_h = if spec.title.is_some() { fs * 2.6 } else { 0.0 };
    let caption_h = if spec.caption.is_some() { fs * 2.0 } else { 0.0 };
    let gap = fs * 2.2;
    let inner_w = width - 2.0 * margin;
    let col_w = (inner_w - gap * (cols as f32 - 1.0)) / cols as f32;
    let facet_cols = if n == 1 { spec.columns } else { None };
    let mut row_h = Vec::new();
    for r in 0..rows {
        let h = ins[r * cols..((r + 1) * cols).min(n)]
            .iter()
            .map(|p| draw::preferred_height(p, col_w, facet_cols) + if p.title.is_some() { fs * 1.7 } else { 0.0 })
            .fold(0.0f32, f32::max);
        row_h.push(h);
    }
    let natural = margin * 2.0 + title_h + caption_h + row_h.iter().sum::<f32>() + gap * (rows as f32 - 1.0);
    let height = spec.height.map_or(natural, |h| h as f32).clamp(200.0, MAX_SIZE as f32);
    // stretch or squeeze the rows to the height asked for
    let k = (height - margin * 2.0 - title_h - caption_h - gap * (rows as f32 - 1.0)) / row_h.iter().sum::<f32>().max(1.0);
    let mut fig = Figure::new(width, height.round(), Some(theme.paper));
    fig.title.clone_from(&spec.title);
    if let Some(t) = &spec.title {
        let st = TextStyle::new(theme.title_size(), theme.ink).face(Face::SansMedium);
        fig.text(margin, margin + theme.title_size() * 0.9, t, &st);
    }
    let mut out = PlotOutput {
        title: spec.title.clone(),
        width: width as u32,
        height: height.round() as u32,
        theme: match spec.theme.unwrap_or_default() {
            ThemeName::Light => "light".into(),
            ThemeName::Dark => "dark".into(),
        },
        ..PlotOutput::default()
    };
    let mut hover = Vec::new();
    let mut y = margin + title_h;
    let mut it = ins.into_iter();
    for h in &row_h {
        let rh = h * k;
        for c in 0..cols {
            let Some(p) = it.next() else { break };
            let x = margin + c as f32 * (col_w + gap);
            let drawn = draw::draw_panel(&mut fig, &theme, Rect::new(x, y, col_w, rh), p, facet_cols)?;
            out.panels.push(drawn.summary);
            hover.push(Value::Array(drawn.hover));
        }
        y += rh + gap;
    }
    if let Some(c) = &spec.caption {
        let st = TextStyle::new(theme.small_size(), theme.muted).align(Anchor::Start, Baseline::Alphabetic);
        fig.text(margin, height - margin * 0.75, c, &st);
    }
    out.files = files;
    out.inline_series = inline_series;
    out.notes = ctx.notes;
    out.warnings = ctx.warnings;
    out.notes.dedup();
    out.warnings.dedup();
    Ok(Plotted {
        figure: fig,
        output: out,
        hover: Value::Array(hover),
    })
}

/// One data item that gives several series (wells, files) is named once, by its label.
fn s_many(items: &[SeriesSpec], item: &SeriesSpec) -> bool {
    let _ = items;
    item.label.is_some()
}

fn prefix(e: Error, panel: usize, many: bool) -> Error {
    let _ = many;
    match e {
        Error::Usage(m) if panel > 0 => Error::Usage(format!("panel {panel}: {m}")),
        other => other,
    }
}

/// Give panels the same x and/or y range.
fn share_axes(ins: &mut [draw::PanelIn], share: Option<Share>) -> Result<()> {
    let Some(share) = share.filter(|s| *s != Share::None) else {
        return Ok(());
    };
    if ins.len() < 2 {
        return Ok(());
    }
    let mut xr: Option<[f64; 2]> = None;
    let mut yr: Option<[f64; 2]> = None;
    let mut doms = Vec::new();
    for p in ins.iter() {
        let mut s = p.series.clone();
        let (ax, ay) = draw::prepare_axes(&mut s, &p.spec)?;
        doms.push((ax.domain, ay.domain));
        let u = |r: Option<[f64; 2]>, d: openreadout_figure::Domain| {
            let (lo, hi) = (d.lo.min(d.hi), d.lo.max(d.hi));
            Some(r.map_or([lo, hi], |r| [r[0].min(lo), r[1].max(hi)]))
        };
        xr = u(xr, ax.domain);
        yr = u(yr, ay.domain);
    }
    for p in ins.iter_mut() {
        if matches!(share, Share::X | Share::Both) {
            let x = p.spec.x.get_or_insert_with(AxisSpec::default);
            if x.range.is_none() {
                x.range = xr;
            }
        }
        if matches!(share, Share::Y | Share::Both) {
            let y = p.spec.y.get_or_insert_with(AxisSpec::default);
            if y.range.is_none() {
                y.range = yr;
            }
        }
    }
    Ok(())
}

/// Encodings for `output`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileFormat {
    /// PNG at 2× the pixel size.
    Png,
    /// SVG (vector; text as text in IBM Plex).
    Svg,
    /// PDF (vector; text as outlines).
    Pdf,
}

impl FileFormat {
    /// From a path's extension.
    pub fn from_path(p: &Path) -> Result<FileFormat> {
        match p
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("png") => Ok(FileFormat::Png),
            Some("svg") => Ok(FileFormat::Svg),
            Some("pdf") => Ok(FileFormat::Pdf),
            _ => Err(Error::Usage(format!(
                "output `{}`: name it .png, .svg or .pdf",
                p.display()
            ))),
        }
    }

    /// `png`, `svg` or `pdf`.
    pub fn as_str(self) -> &'static str {
        match self {
            FileFormat::Png => "png",
            FileFormat::Svg => "svg",
            FileFormat::Pdf => "pdf",
        }
    }
}

/// The figure as a PDF document (vector, one page the size of the figure at 72 px per inch).
pub fn to_pdf(fig: &Figure) -> Vec<u8> {
    pdf::pdf(fig)
}

/// Encode `fig` as `format`.
pub fn encode(fig: &Figure, format: FileFormat) -> Result<Vec<u8>> {
    Ok(match format {
        FileFormat::Png => fig.to_png(2.0)?,
        FileFormat::Svg => fig.to_svg().into_bytes(),
        FileFormat::Pdf => to_pdf(fig),
    })
}

/// Write the figure to `path` (format from the extension) through a temporary file that is read
/// back and compared before it is renamed into place.
pub fn write(fig: &Figure, path: &Path, overwrite: bool) -> Result<WrittenFile> {
    let format = FileFormat::from_path(path)?;
    let bytes = encode(fig, format)?;
    openreadout_assay::write_verified(path, &bytes, overwrite)?;
    Ok(WrittenFile {
        path: path.display().to_string(),
        format: format.as_str().into(),
        bytes: bytes.len() as u64,
        verified: true,
    })
}

/// Plot and, when the spec names an `output`, write it.
pub fn run(reg: &Registry, spec: &PlotSpec) -> Result<Plotted> {
    let mut p = plot(reg, spec)?;
    if let Some(o) = &spec.output {
        p.output.output = Some(write(&p.figure, Path::new(o), spec.overwrite)?);
    }
    Ok(p)
}
