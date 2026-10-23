//! What `openreadout_plot` returns besides the picture: what was drawn, from which data, on
//! which ranges, and anything the caller should know.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The result of a plot.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct PlotOutput {
    /// Figure title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Picture size in px.
    pub width: u32,
    /// Picture size in px.
    pub height: u32,
    /// `light` or `dark`.
    pub theme: String,
    /// One entry per panel.
    pub panels: Vec<PanelOutput>,
    /// Files the data came from.
    pub files: Vec<String>,
    /// Series drawn from inline values (numbers the caller supplied, not read from a file).
    pub inline_series: usize,
    /// What was done to the data (subtractions, fits, transforms).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    /// Data that could not be drawn as asked, or values to double-check.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    /// How to save this figure to a file (MCP: the openreadout_export call).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub save: Option<String>,
}

/// One panel.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct PanelOutput {
    /// Panel title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// `plot`, `facets` or `plate`.
    pub layout: String,
    /// Small multiples drawn (facets and plates).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub facets: Option<usize>,
    /// The x axis.
    pub x: AxisOutput,
    /// The y axis.
    pub y: AxisOutput,
    /// The series, in drawing order.
    pub series: Vec<SeriesOutput>,
    /// Annotations drawn.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub annotations: Vec<String>,
}

/// An axis as drawn.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct AxisOutput {
    /// Label.
    pub label: String,
    /// Unit of the values shown (annotations use it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Range shown, `[left, right]` or `[bottom, top]`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<[f64; 2]>,
    /// The range of each small multiple, in facet order, when they do not share this axis.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub facet_ranges: Vec<[f64; 2]>,
    /// `linear`, `log`, `category` or the transform of a flow axis (`logicle`).
    pub scale: String,
    /// Categories, left to right.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub categories: Vec<String>,
}

/// One series.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct SeriesOutput {
    /// Name as labelled.
    pub name: String,
    /// The file read (absent for inline values).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// What was read: trace, sweep and channel, chromatogram, scan, wells, columns, ...
    pub data: String,
    /// `line`, `band`, `points`, `bars`, `events`, `histogram`, `sticks`, `area`.
    pub mark: String,
    /// Points (or events, wells, bars) drawn.
    pub points: usize,
    /// x range of the data, in the axis units.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x_range: Option<[f64; 2]>,
    /// y range of the data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub y_range: Option<[f64; 2]>,
    /// Series summarized in this entry (many wells of one colour class are listed once).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<usize>,
    /// Colour class (group, flag) when the series is coloured by one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
    /// Colour, #RRGGBB.
    pub color: String,
    /// Labelled spots (peaks, apex, spikes): `[x, y]` in axis units with their text.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub marked: Vec<Marked>,
}

/// A marked spot.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct Marked {
    /// x in axis units.
    pub x: f64,
    /// y in axis units.
    pub y: f64,
    /// Text (empty for spikes).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub text: String,
}
