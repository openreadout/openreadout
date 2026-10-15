//! What a figure is made of, as the caller writes it: panels of series (data named by file and
//! selector, a table file's columns, or a few inline values), axes, facets and annotations.
//! These types are the input schema of `openreadout_plot` and of `openreadout plot`.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One value or a list of them.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum OneOrMany<T> {
    /// One.
    One(T),
    /// Several.
    Many(Vec<T>),
}

impl<T: Clone> OneOrMany<T> {
    /// The values as a list.
    pub fn to_vec(&self) -> Vec<T> {
        match self {
            OneOrMany::One(v) => vec![v.clone()],
            OneOrMany::Many(v) => v.clone(),
        }
    }
}

/// A figure: one panel (its fields at the top level) or several (`panels`).
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct PlotSpec {
    /// Title over the figure.
    pub title: Option<String>,
    /// Small print under the figure (a source credit, a method note).
    pub caption: Option<String>,
    /// The single panel.
    #[serde(flatten)]
    pub panel: PanelSpec,
    /// Several panels, side by side (see columns); instead of a top-level `data`.
    #[serde(default)]
    pub panels: Vec<PanelItem>,
    /// Panels (or facets of a single panel) per row. Default: all panels in one row; facets in
    /// a near-square grid.
    pub columns: Option<u32>,
    /// Panels that share an axis range. Default none.
    pub share: Option<Share>,
    /// Width in px (default 1200, at most 3000).
    pub width: Option<u32>,
    /// Height in px (default from the content, at most 3000).
    pub height: Option<u32>,
    /// Colour scheme. Default light.
    pub theme: Option<ThemeName>,
    /// Also write the figure to this file (.png, .svg or .pdf; png at 2× the pixel size),
    /// verified after writing.
    pub output: Option<String>,
    /// Replace an existing output file.
    #[serde(default)]
    pub overwrite: bool,
    /// true: refuse values this file's assurance does not validate (exit_code 6, with a hint).
    pub strict: Option<bool>,
}

/// A panel in `panels`.
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct PanelItem {
    /// Title over the panel.
    pub title: Option<String>,
    /// The panel.
    #[serde(flatten)]
    pub panel: PanelSpec,
}

/// One plot area (or a grid of small multiples of it, with `facet`).
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct PanelSpec {
    /// What to draw: one series spec or a list of them, drawn in order (first at the back).
    pub data: Option<OneOrMany<SeriesSpec>>,
    /// Small multiples: `series` = one small plot per series (sweeps, files, groups, wells);
    /// `plate` = plate wells in their plate positions.
    pub facet: Option<Facet>,
    /// The x axis.
    pub x: Option<AxisSpec>,
    /// The y axis.
    pub y: Option<AxisSpec>,
    /// Bands, lines, labelled points, arrows and notes, in the units the axes show.
    #[serde(default)]
    pub annotate: Vec<Annotation>,
    /// How series are named: `direct` labels at the line ends, a `key` row, or `none`. Default
    /// direct when it fits, else key.
    pub legend: Option<Legend>,
    /// Column headings of a plate facet (e.g. doses), left to right.
    #[serde(default)]
    pub column_labels: Vec<String>,
    /// Row headings of a plate facet, top to bottom. Default: the layout group of each block of
    /// rows, else the row letters.
    #[serde(default)]
    pub row_labels: Vec<String>,
    /// Offset each series upward by this fraction of the tallest one (stacked spectra).
    pub stack: Option<f64>,
}

/// Small multiples.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Facet {
    /// One small plot per series.
    Series,
    /// Plate wells in plate positions.
    Plate,
}

/// Shared axis ranges across panels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Share {
    /// Same x range.
    X,
    /// Same y range.
    Y,
    /// Both.
    Both,
    /// Each panel its own.
    None,
}

/// Colour scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ThemeName {
    /// Ink on paper.
    #[default]
    Light,
    /// Light ink on a dark ground.
    Dark,
}

/// Legend style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Legend {
    /// Labels at the line ends.
    Direct,
    /// A key row above the plot.
    Key,
    /// No labels.
    None,
}

/// An axis.
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AxisSpec {
    /// Axis label (default from the data, with its unit).
    pub label: Option<String>,
    /// Log scale.
    #[serde(default)]
    pub log: bool,
    /// Range shown, `[low, high]`.
    pub range: Option<[f64; 2]>,
    /// High to low (default for NMR ppm and IR wavenumbers).
    pub reverse: Option<bool>,
    /// Time unit to show a time axis in (`s`, `min`, `h`). Default chosen from the span.
    pub unit: Option<String>,
}

/// What a series shows. Default from the file: traces for signals and spectra, the TIC for mass
/// spectrometry, per-well curves for plates, events for FCS (with x and y), rows for tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Show {
    /// A sampled signal or 1-D spectrum (trace, sweep, channel).
    Trace,
    /// Patch-clamp sweeps (sweep = which; spikes marks action potentials).
    Sweeps,
    /// A chromatogram: TIC (default), BPC, XIC (mz) or a detector trace.
    Chromatogram,
    /// One mass spectrum (rt, or scan).
    Spectrum,
    /// Plate-reader wells: kinetic curves, or values per well.
    Wells,
    /// FCS events: x against y (points or density), coloured by population.
    Events,
    /// Histogram of one parameter or column (x).
    Histogram,
    /// Patch-clamp f–I: spikes in the stimulus (or y=rate_hz) against injected current.
    Fi,
    /// Dose-response points and fitted curves per compound (openreadout_assay).
    DoseResponse,
    /// Standard curve: standards, fit and back-calculated samples (openreadout_assay).
    StandardCurve,
    /// qPCR amplification curves.
    Amplification,
    /// qPCR melt curves (−dF/dT).
    Melt,
}

/// How a series is drawn. Default from what it shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MarkKind {
    /// Lines.
    Line,
    /// Dots.
    Points,
    /// Lines with dots.
    LinePoints,
    /// Bars (categories or x positions).
    Bars,
    /// Filled to zero.
    Area,
    /// Vertical sticks (centroided spectra).
    Sticks,
    /// 2-D density of events.
    Density,
}

/// How plate wells or table rows are grouped into series.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Spread {
    /// Mean ± standard deviation.
    Sd,
    /// Mean ± standard error.
    Sem,
    /// Mean with the min–max range.
    Range,
    /// Mean only.
    None,
}

/// Plate values before plotting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Subtract {
    /// Raw values.
    None,
    /// Minus the mean of the layout's blank wells (per time point).
    Blank,
    /// Minus each well's first reading (gain over the run).
    First,
}

/// The data of one series.
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SeriesSpec {
    /// Instrument file(s); several files give one series each.
    pub file: Option<OneOrMany<String>>,
    /// A table file (.csv, .tsv, .parquet, .jsonl; e.g. openreadout_batch output) to plot
    /// columns x, y (and error) of.
    pub table: Option<String>,
    /// Small inline data the agent computed: `{x: [...], y: [...], error?: [...]}`; x may be
    /// category names.
    pub values: Option<InlineValues>,
    /// What to show (default from the file).
    pub show: Option<Show>,
    /// How to draw it (default from what it shows).
    pub mark: Option<MarkKind>,
    /// Series name (default from the data).
    pub label: Option<String>,
    /// Colour: ink, accent, muted, faint, blue, ochre, green, purple, teal, brown, or #RRGGBB.
    pub color: Option<String>,

    /// Trace index (info → traces[]). Default 0.
    pub trace: Option<u32>,
    /// Sweep index, or a list (sweeps). Default 0 (Sweeps: all).
    pub sweep: Option<OneOrMany<u32>>,
    /// Channel index. Default 0 (Sweeps: the voltage channel).
    pub channel: Option<u32>,
    /// Only this window of the x axis, `[a, b]` in the axis' units.
    pub x_range: Option<[f64; 2]>,
    /// NMR: process FIDs into spectra.
    #[serde(default)]
    pub process: bool,

    /// Chromatogram: `tic` (default) or `bpc`; ignored when mz is given.
    pub chromatogram: Option<String>,
    /// Chromatogram: extracted-ion m/z value(s), one series each.
    pub mz: Option<OneOrMany<f64>>,
    /// XIC tolerance in ppm (default 10).
    pub ppm: Option<f64>,
    /// Spectrum: retention time in minutes, or "apex" (the TIC's highest point).
    pub rt: Option<serde_json::Value>,
    /// Spectrum: instrument scan number.
    pub scan: Option<u64>,
    /// Label the N highest peaks (chromatograms: rt; spectra: m/z or x).
    pub peaks: Option<u32>,
    /// Sweeps: mark detected action potentials.
    #[serde(default)]
    pub spikes: bool,

    /// Plates and qPCR: wells such as `A1:H11` or `A1,B2`.
    pub wells: Option<String>,
    /// Plates: only wells of these layout groups (`*` matches any text).
    #[serde(default)]
    pub groups: Vec<String>,
    /// Plates: `well` (one curve per well, default) or `group` (mean and spread per layout
    /// group). Tables: the column whose values split rows into series.
    pub by: Option<String>,
    /// Spread drawn around group means. Default sd.
    pub spread: Option<Spread>,
    /// Plates: subtract blanks or each well's first reading. Default none.
    pub subtract: Option<Subtract>,
    /// Plates: read number or label (default the first).
    pub read: Option<String>,
    /// Plate wells: colour by `group`, `row` or `column`; by classes `{"crashed": "E3:H11"}`; or
    /// by a rule `{"at": 6, "below": 0.1, "label": "crashed", "else": "grew"}` (at in x units).
    pub color_by: Option<ColorBy>,

    /// FCS parameter, table column, or f–I measure for the x axis.
    pub x: Option<String>,
    /// FCS parameter, table column, or f–I measure (`spike_count`, `rate_hz`) for the y axis.
    pub y: Option<String>,
    /// Table column with error bar half-widths.
    pub error: Option<String>,
    /// Tables: row filters `COLUMN=VALUE` or `COLUMN!=VALUE`.
    #[serde(rename = "where", default)]
    pub where_: Vec<String>,
    /// FCS: populations to colour (paths or unique names; needs workspace or gatingml).
    #[serde(default)]
    pub populations: Vec<String>,
    /// FCS: FlowJo workspace (.wsp).
    pub workspace: Option<String>,
    /// FCS: Gating-ML 2.0 file.
    pub gatingml: Option<String>,
    /// FCS: transform (`logicle` default for fluorescence, `arcsinh`, `log`, `linear`,
    /// `workspace`).
    pub transform: Option<String>,
    /// FCS: compensate `auto`, `fcs` or `gating`.
    pub compensate: Option<String>,
    /// qPCR: target name.
    pub target: Option<String>,
    /// DoseResponse, StandardCurve: openreadout_assay arguments without file (layout, standards,
    /// model, normalize, reduce, …).
    pub assay: Option<serde_json::Map<String, serde_json::Value>>,
}

/// Inline values.
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InlineValues {
    /// x values: numbers, or category names.
    pub x: Vec<serde_json::Value>,
    /// y values.
    pub y: Vec<f64>,
    /// Error bar half-widths.
    #[serde(default)]
    pub error: Vec<f64>,
    /// Point labels.
    #[serde(default)]
    pub labels: Vec<String>,
    /// Unit of x (axis label).
    pub x_unit: Option<String>,
    /// Unit of y.
    pub y_unit: Option<String>,
}

/// How plate wells are coloured.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum ColorBy {
    /// `group`, `row`, `column` or `none`.
    Field(String),
    /// A rule on each well's value at one x.
    Rule(ColorRule),
    /// Named classes of wells: `{"crashed": "E3:H11,A9:D11"}`; other wells are the rest.
    Classes(BTreeMap<String, String>),
}

/// A per-well rule.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ColorRule {
    /// Where to look, in x-axis units (time in the unit the axis shows).
    pub at: f64,
    /// Flag wells whose value is below this...
    pub below: Option<f64>,
    /// ...or above this.
    pub above: Option<f64>,
    /// Name of flagged wells. Default "flagged".
    pub label: Option<String>,
    /// Name of the other wells. Default "other".
    #[serde(rename = "else")]
    pub otherwise: Option<String>,
}

/// One annotation. `x` and `y` are in the units the axes show: a number on one axis draws a
/// line, `[a, b]` a band, numbers on both a labelled point (with `to`: an arrow from the label
/// to that point).
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Annotation {
    /// x position or `[a, b]`.
    pub x: Option<OneOrMany<f64>>,
    /// y position or `[a, b]`.
    pub y: Option<OneOrMany<f64>>,
    /// Text.
    pub label: Option<String>,
    /// Arrow head position `[x, y]`; the arrow starts at the label.
    pub to: Option<[f64; 2]>,
    /// Colour name or #RRGGBB (default accent).
    pub color: Option<String>,
    /// Draw a dot at a labelled point. Default true.
    pub marker: Option<bool>,
    /// Only in this facet (its title, e.g. `B3` or `sweep 6`).
    pub facet: Option<String>,
}
