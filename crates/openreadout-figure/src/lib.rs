//! Vector figures for `OpenReadout`.
//!
//! A [`Figure`] is a list of shapes, text and raster images in pixel coordinates, drawn in
//! order. It renders in-process to a [`Raster`] (and so PNG or JPEG) with anti-aliasing, or to
//! SVG text. Text is set in the embedded IBM Plex Sans and Mono with their real metrics and
//! kerning, so labels can be measured and placed exactly.
//!
//! On top of the scene:
//! - [`Theme`]: the OpenReadout look (paper and ink, a warm red accent, quiet axes), light or
//!   dark, with type sizes that follow the figure size;
//! - [`Scale`] and [`Domain`]: linear and log scales with nice ticks and short labels;
//! - [`Panel`]: a plot rectangle with marks in data units (lines, areas, bands, rules,
//!   markers, bars, heat-map cells, text, arrows) and quiet axes;
//! - [`Envelope`]: dense data reduced to a min/max line per pixel column;
//! - [`grid`]: the frames of small multiples;
//! - [`LabelPlacer`], [`spread`] and [`end_labels`]: labels that do not overlap;
//! - [`Pill`]: a label on a rounded background that reads on any image.
//!
//! Every step is deterministic: the same figure gives the same bytes.
//!
//! ```
//! use openreadout_figure::{Figure, Theme, TextStyle, Anchor, Baseline};
//!
//! let theme = Theme::light();
//! let mut fig = Figure::new(200.0, 60.0, Some(theme.paper));
//! let style = TextStyle::new(16.0, theme.ink).align(Anchor::Middle, Baseline::Middle);
//! fig.text(100.0, 30.0, "5 µm", &style);
//! let svg = fig.to_svg();
//! assert!(svg.contains("5 µm"));
//! let png = fig.to_png(2.0)?; // 400 × 120 px
//! assert_eq!(&png[1..4], b"PNG");
//! # Ok::<(), openreadout_core::Error>(())
//! ```
#![forbid(unsafe_code)]
#![warn(missing_docs)]
// Geometry and pixel arithmetic read best with the conventional short names (x, y, w, h, r, g, b).
#![allow(clippy::many_single_char_names)]
// Colours are written 0xRRGGBB, the way designers write them.
#![allow(clippy::unreadable_literal)]

mod chart;
mod color;
mod font;
mod geom;
pub mod jpeg;
mod label;
mod path;
mod raster;
mod render;
mod scale;
mod scene;
mod svg;
mod theme;

pub use chart::{
    AxesDrawn, AxesSpec, Domain, EndLabel, Envelope, GridLines, Marker, Panel, Shape, arrow_parts,
    arrow_px, color_bar, draw_marker, end_labels, key_row, marker_path, spread,
};
pub use color::{Color, Colormap};
pub use font::{Face, Metrics, metrics, text_path, text_width};
pub use geom::{Rect, grid};
pub use label::{LabelPlacer, Pill};
pub use path::{Path, Segment};
pub use raster::Raster;
pub use scale::{Scale, ScaleKind, Tick, format_tick, format_value, nice_step};
pub use scene::{Anchor, Baseline, Cap, Figure, Item, Stroke, TextStyle};
pub use theme::{Mode, Theme};
