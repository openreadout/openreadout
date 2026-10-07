//! Deterministic previews of `OpenReadout` datasets, encoded as PNG or JPEG.
//!
//! - **Images**: one plane, a channel composite (additive, channel colours from the normalized
//!   model) or a maximum-intensity projection over z or t, read from the pyramid level nearest the
//!   requested size when the reader exposes levels, then block-averaged to fit and mapped to 8 bits
//!   with a contrast rule (`auto`, `min-max`, `percentile:LO,HI`, `raw`). By default the picture
//!   is framed by coordinate rulers in full-resolution pixels (and a scale bar when the pixel
//!   size is known), and [`ImagePreview`] reports how PNG pixels map back to source pixels, so a
//!   viewer can read coordinates off the image and ask for a `region` of it.
//! - **Traces**: a sparkline (min/max envelope per pixel column) of one sweep, one panel per
//!   channel, with the x axis labelled (time, NMR chemical shift, …).
//! - **Spectra**: one mass spectrum, as sticks (centroided) or a profile line, with its m/z range.
//! - **Plates**: a heat map of a multi-well plate table (columns named like wells `A1`…`P24`, or
//!   `well` / `row` + `column` columns).
//!
//! Every step uses fixed-order arithmetic and no threads, so the same request produces the same
//! bytes on every run.
//!
//! # Example
//!
//! Previewing a file is [`render`] (read and draw) followed by [`finish`] (encode):
//!
//! ```no_run
//! use openreadout_core::{Dataset, Result};
//! use openreadout_preview::{Encoding, PreviewRequest, finish, render};
//!
//! fn png_preview(dataset: &mut dyn Dataset) -> Result<Vec<u8>> {
//!     let info = dataset.info()?;
//!     let mut request = PreviewRequest::default(); // image 0, channel 0, middle Z, 1024 px
//!     request.max_size = 512;
//!     let rendered = render(dataset, &info, &request)?;
//!     let (output, png) = finish(&rendered, Encoding::Png, 90)?;
//!     println!("{} x {} px, xxh3 {}", output.width, output.height, output.xxh3);
//!     Ok(png)
//! }
//! ```
//!
//! The drawing and encoding layers work without a file:
//!
//! ```
//! use openreadout_preview::{Canvas, Encoding, encode};
//!
//! let mut canvas = Canvas::new(4, 2, [0, 0, 0]);
//! canvas.fill_rect(0, 0, 2, 2, [255, 255, 255]);
//! let png = encode(&canvas, Encoding::Png, 90)?;
//! assert_eq!(&png[1..4], b"PNG");
//! # Ok::<(), openreadout_core::Error>(())
//! ```
#![forbid(unsafe_code)]
#![warn(missing_docs)]
// Pixel and DCT arithmetic reads best with the conventional short names (x, y, w, h, r, g, b).
#![allow(clippy::many_single_char_names)]

pub mod canvas;
pub mod color;
mod image;
pub mod jpeg;
mod overlay;
mod plate;
mod rulers;
mod spectrum;
mod trace;

use std::path::{Path, PathBuf};
use std::str::FromStr;

use openreadout_core::model::FileInfo;
use openreadout_core::reader::Dataset;
use openreadout_core::{Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use canvas::Canvas;
pub use plate::{PlateGrid, plate_grid};

/// Largest `max_size` accepted (pixels on the longer side).
pub const MAX_PREVIEW_SIZE: u32 = 8192;
/// Default `max_size`.
pub const DEFAULT_PREVIEW_SIZE: u32 = 1024;
/// Default JPEG quality.
pub const DEFAULT_JPEG_QUALITY: u8 = 90;
/// Default longest side of a [`thumbnail`].
pub const DEFAULT_THUMBNAIL_SIZE: u32 = 384;
/// Default budget of decoded pixel bytes a [`thumbnail`] may read (32 MiB).
pub const DEFAULT_THUMBNAIL_READ_BYTES: u64 = 32 << 20;
/// A file with both traces and spectra previews its first trace by default only when some trace
/// has at least this many samples (a chromatogram of a real run); otherwise spectrum 0.
pub const MIN_TRACE_SAMPLES_OVER_SPECTRUM: u64 = 100;

/// How sample values map to display brightness.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[non_exhaustive]
pub enum Contrast {
    /// `percentile:0.1,99.9` for most images; `raw` for 8-bit RGB (already display-ready).
    #[default]
    Auto,
    /// Darkest sample → black, brightest → full colour.
    MinMax,
    /// The given percentiles of the (downsampled) samples → black and full colour.
    Percentile(f64, f64),
    /// The pixel type's full range (0–255 for uint8, 0–65535 for uint16, 0–1 for floats).
    Raw,
}

impl FromStr for Contrast {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        let s = s.trim().to_ascii_lowercase();
        match s.as_str() {
            "auto" => Ok(Contrast::Auto),
            "min-max" | "minmax" => Ok(Contrast::MinMax),
            "raw" => Ok(Contrast::Raw),
            _ => {
                let bad = || {
                    Error::Usage(format!(
                        "bad contrast '{s}': use auto, min-max, raw or percentile:LO,HI (e.g. percentile:1,99)"
                    ))
                };
                let spec = s.strip_prefix("percentile:").ok_or_else(bad)?;
                let (a, b) = spec.split_once(',').ok_or_else(bad)?;
                let (a, b): (f64, f64) = (
                    a.trim().parse().map_err(|_| bad())?,
                    b.trim().parse().map_err(|_| bad())?,
                );
                if !(0.0..=100.0).contains(&a) || !(0.0..=100.0).contains(&b) || a >= b {
                    return Err(bad());
                }
                Ok(Contrast::Percentile(a, b))
            }
        }
    }
}

impl std::fmt::Display for Contrast {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Contrast::Auto => f.write_str("auto"),
            Contrast::MinMax => f.write_str("min-max"),
            Contrast::Percentile(a, b) => write!(f, "percentile:{a},{b}"),
            Contrast::Raw => f.write_str("raw"),
        }
    }
}

/// Colour lookup for a single channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum Lut {
    /// black to white
    #[serde(alias = "grey")]
    Gray,
    /// black to the channel's colour
    #[serde(alias = "channel-colour", alias = "color")]
    ChannelColor,
}

impl FromStr for Lut {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "gray" | "grey" => Ok(Lut::Gray),
            "channel-color" | "channel-colour" | "color" => Ok(Lut::ChannelColor),
            other => Err(Error::Usage(format!(
                "bad lut '{other}': use gray or channel-color"
            ))),
        }
    }
}

/// Axis of a maximum-intensity projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Axis {
    /// along z (focal planes)
    Z,
    /// along t (time points)
    T,
}

/// What frames an image preview.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Axes {
    /// coordinate rulers in full-resolution pixels and a scale bar around the picture (default)
    #[default]
    Rulers,
    /// rulers, plus faint grid lines over the data at the ruler ticks
    Grid,
    /// the bare downsampled plane
    None,
}

impl Axes {
    /// Set `axes` and `grid` of a request.
    pub fn apply(self, req: &mut PreviewRequest) {
        req.axes = self != Axes::None;
        req.grid = self == Axes::Grid;
    }
}

impl FromStr for Axes {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "rulers" => Ok(Axes::Rulers),
            "grid" => Ok(Axes::Grid),
            "none" => Ok(Axes::None),
            other => Err(Error::Usage(format!(
                "bad axes '{other}': use rulers, grid or none"
            ))),
        }
    }
}

impl FromStr for Axis {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "z" => Ok(Axis::Z),
            "t" => Ok(Axis::T),
            other => Err(Error::Usage(format!(
                "bad projection axis '{other}': use z or t"
            ))),
        }
    }
}

/// Output encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Encoding {
    /// lossless (default)
    #[default]
    Png,
    /// lossy and smaller
    #[serde(alias = "jpg")]
    Jpeg,
}

impl Encoding {
    /// The lowercase name (`png`, `jpeg`).
    pub fn as_str(self) -> &'static str {
        match self {
            Encoding::Png => "png",
            Encoding::Jpeg => "jpeg",
        }
    }
    /// The MIME type (`image/png`, `image/jpeg`).
    pub fn mime(self) -> &'static str {
        match self {
            Encoding::Png => "image/png",
            Encoding::Jpeg => "image/jpeg",
        }
    }
    /// The usual file extension (`png`, `jpg`).
    pub fn extension(self) -> &'static str {
        match self {
            Encoding::Png => "png",
            Encoding::Jpeg => "jpg",
        }
    }
    /// From a file name: `.jpg`/`.jpeg` → JPEG, anything else → PNG.
    pub fn from_path(p: &Path) -> Self {
        match p
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("jpg" | "jpeg") => Encoding::Jpeg,
            _ => Encoding::Png,
        }
    }
}

impl FromStr for Encoding {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "png" => Ok(Encoding::Png),
            "jpeg" | "jpg" => Ok(Encoding::Jpeg),
            other => Err(Error::Usage(format!(
                "bad preview format '{other}': use png or jpeg"
            ))),
        }
    }
}

/// What to render. `Default` previews image 0 (middle z, t 0, channel 0) at 1024 px.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct PreviewRequest {
    /// Image index (default 0).
    pub image: Option<u32>,
    /// Plane selection (`c=1`, `z=3`, `t=0`, or a combined `c=0,1,z=2`); see [`split_select`].
    pub select: Vec<String>,
    /// Maximum-intensity projection axis.
    pub mip: Option<Axis>,
    /// Blend all (or the selected) channels additively in their colours.
    pub composite: bool,
    /// Pyramid level (default: the level nearest `max_size`).
    pub level: Option<u32>,
    /// Only this rectangle of the image: in the pixel coordinates of `level` when a level is
    /// given, else in full-resolution (level 0) coordinates, and the level is then chosen so
    /// that the region renders at about `max_size`.
    pub region: Option<openreadout_core::Region>,
    /// Longest side of the output in pixels.
    pub max_size: u32,
    /// How sample values map to brightness.
    pub contrast: Contrast,
    /// Default: `gray` for one channel, `channel-color` for composites.
    pub lut: Option<Lut>,
    /// Trace preview: trace index (default 0).
    pub trace: Option<u32>,
    /// Trace preview: sweep (default 0).
    pub sweep: Option<u32>,
    /// Trace preview: channel indices (default: the first 8).
    pub channels: Vec<u32>,
    /// Spectrum preview: run index (default 0).
    pub run: Option<u32>,
    /// Spectrum preview: zero-based spectrum index (default 0).
    pub spectrum: Option<u64>,
    /// Spectrum preview: instrument scan number instead of an index.
    pub scan: Option<u64>,
    /// Spectrum preview: the instrument's centroid list instead of the profile.
    pub centroid: bool,
    /// Plate preview: table index (default 0).
    pub table: Option<u32>,
    /// Plate preview (long layout): the value column (default: the first non-position column).
    pub column: Option<String>,
    /// Image preview: frame the picture with coordinate rulers (full-resolution pixels) and a
    /// scale bar (default `true`); `false` gives the bare downsampled plane. `max_size` bounds
    /// the whole picture, rulers included.
    pub axes: bool,
    /// Image preview: faint grid lines over the data at the ruler ticks (default `false`; only
    /// with `axes`).
    pub grid: bool,
    /// Image preview: refuse (an `unsupported` error) to read more than this many bytes of
    /// decoded pixels in total, so a quick look never decodes a huge plane.
    pub max_read_bytes: Option<u64>,
}

impl Default for PreviewRequest {
    fn default() -> Self {
        PreviewRequest {
            image: None,
            select: Vec::new(),
            mip: None,
            composite: false,
            level: None,
            region: None,
            max_size: DEFAULT_PREVIEW_SIZE,
            contrast: Contrast::Auto,
            lut: None,
            trace: None,
            sweep: None,
            channels: Vec::new(),
            run: None,
            spectrum: None,
            scan: None,
            centroid: false,
            table: None,
            column: None,
            axes: true,
            grid: false,
            max_read_bytes: None,
        }
    }
}

/// One channel of an image preview.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct PreviewChannel {
    /// C index.
    pub index: u32,
    /// Channel name, if recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Display colour `#RRGGBB` (`#FFFFFF` with the gray LUT).
    pub color: String,
    /// Where the colour came from: `file` (`channels[].color`), `wavelength`, `palette` or `lut`.
    pub color_source: String,
    /// Sample value shown as black.
    pub display_min: f64,
    /// Sample value shown at full brightness.
    pub display_max: f64,
}

/// A point in full-resolution (level 0) source pixel coordinates.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SourcePoint {
    /// Column (may be fractional on a downsampled pyramid level).
    pub x: f64,
    /// Row.
    pub y: f64,
}

/// The scale bar drawn under an image preview.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ScaleBar {
    /// Length as labelled (e.g. `50`).
    pub length: f64,
    /// Unit of the label: `nm`, `µm` or `mm`.
    pub unit: String,
    /// Length in µm.
    pub length_um: f64,
    /// Length in PNG pixels.
    pub pixels: u32,
}

/// Details of an image preview.
///
/// PNG pixel `(px, py)` inside `plot_area` shows full-resolution source pixel
/// `x = source_origin.x + (px - plot_area.x) * source_per_pixel`,
/// `y = source_origin.y + (py - plot_area.y) * source_per_pixel_y` (left/top edges of the PNG
/// pixel), whatever pyramid level, downsample or region was drawn; the ruler labels use the same
/// numbers, so a `region` read off them can be passed straight back.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ImagePreview {
    /// Image index.
    pub image: u32,
    /// Pyramid level read (0 = full resolution).
    pub level: u32,
    /// Plane size at that level.
    pub source_width: u32,
    /// Plane height at that level.
    pub source_height: u32,
    /// The rectangle drawn, in the pixel coordinates of `level`, when a region was asked for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<openreadout_core::Region>,
    /// Block size of the averaging downsample (1 = none).
    pub downsample: u32,
    /// Channels, z-planes and time points used.
    pub c: Vec<u32>,
    /// Z indices used.
    pub z: Vec<u32>,
    /// Time indices used.
    pub t: Vec<u32>,
    /// `max-z` or `max-t` for a maximum-intensity projection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projection: Option<String>,
    /// True when channels were blended into one colour image.
    pub composite: bool,
    /// True when the planes are RGB (samples_per_pixel 3).
    pub rgb: bool,
    /// Contrast rule applied (`auto` resolved to what it did, e.g. `percentile:0.1,99.9`).
    pub contrast: String,
    /// `gray`, `channel-color` or `rgb`.
    pub lut: String,
    /// One entry per channel drawn.
    pub channels: Vec<PreviewChannel>,
    /// True when the picture is framed by coordinate rulers (labelled in full-resolution
    /// pixels), with a scale bar when the pixel size is known.
    #[serde(default)]
    pub axes: bool,
    /// True when grid lines were drawn over the data at the ruler ticks.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub grid: bool,
    /// The data's rectangle inside the PNG, in PNG pixels (the whole PNG without rulers).
    #[serde(default)]
    pub plot_area: openreadout_core::Region,
    /// Full-resolution source coordinate of the plot area's top-left corner.
    #[serde(default)]
    pub source_origin: SourcePoint,
    /// Full-resolution source pixels per PNG pixel, horizontally (pyramid level scale ×
    /// `downsample`).
    #[serde(default)]
    pub source_per_pixel: f64,
    /// Full-resolution source pixels per PNG pixel, vertically (equal to `source_per_pixel` up
    /// to the rounding of pyramid level sizes).
    #[serde(default)]
    pub source_per_pixel_y: f64,
    /// The full-resolution (level 0) rectangle the plot area shows: pass it, or a part of it,
    /// back as `region` to zoom.
    #[serde(default)]
    pub full_res_region: openreadout_core::Region,
    /// The scale bar drawn, when the physical pixel size is known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale_bar: Option<ScaleBar>,
}

impl ImagePreview {
    /// Update the pixel mapping for a canvas halved by [`Canvas::half`].
    fn halve(&mut self) {
        let (plot, origin, per) = rulers::halve(
            self.plot_area,
            self.source_origin,
            (self.source_per_pixel, self.source_per_pixel_y),
        );
        self.plot_area = plot;
        self.source_origin = origin;
        (self.source_per_pixel, self.source_per_pixel_y) = per;
        if let Some(b) = &mut self.scale_bar {
            b.pixels = b.pixels.div_ceil(2);
        }
    }
}

/// One channel of a trace preview.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct TracePreviewChannel {
    /// Channel index.
    pub index: u32,
    /// Channel name.
    pub name: String,
    /// Physical unit of the samples.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Line colour `#RRGGBB`.
    pub color: String,
    /// Bottom and top of the panel's y axis (the channel's finite range, padded by 5 %).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    /// Top of the panel's y axis.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
}

/// The horizontal axis of a trace or spectrum preview (the labels at its two ends).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct XAxis {
    /// `time`, `chemical_shift`, `mz`, `sample`, … (the reader's `extra.axis.quantity` when set).
    pub quantity: String,
    /// `s`, `min`, `ppm`, `m/z`, … (empty for sample indices).
    pub unit: String,
    /// Value at the left edge.
    pub first: f64,
    /// Value at the right edge (smaller than `first` for NMR chemical shift, which runs right to left).
    pub last: f64,
}

/// Details of a trace preview.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct TracePreview {
    /// Trace index.
    pub trace: u32,
    /// Sweep index.
    pub sweep: u32,
    /// Samples per second.
    pub sample_rate_hz: f64,
    /// Samples in the sweep.
    pub sweep_sample_count: u64,
    /// Samples drawn (from the start of the sweep).
    pub sample_count: u64,
    /// True when the sweep is longer than the samples drawn.
    pub truncated: bool,
    /// Label of the horizontal axis, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x_axis: Option<XAxis>,
    /// One panel per channel, top to bottom.
    pub channels: Vec<TracePreviewChannel>,
}

/// Details of a mass-spectrum preview.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct SpectrumPreview {
    /// Run index.
    pub run: u32,
    /// Zero-based spectrum index.
    pub index: u64,
    /// Scan number as the instrument counts it.
    pub scan_number: u64,
    /// MS level (1 = full scan, 2 = fragment scan, ...).
    pub ms_level: u32,
    /// Retention time in seconds; null when the file states none.
    pub rt_s: Option<f64>,
    /// `sticks` (centroided) or `profile` (a line through the points).
    pub style: String,
    /// Points in the spectrum.
    pub point_count: u64,
    /// m/z range of the x axis.
    pub x_axis: XAxis,
    /// Largest intensity (the top of the plot).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_intensity: Option<f64>,
    /// Precursor m/z of a fragment (MS2+) scan.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub precursor_mz: Option<f64>,
}

/// Details of a plate heat map.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct PlatePreview {
    /// Table index.
    pub table: u32,
    /// `wide` (one column per well) or `long` (`well` or `row` + `column` columns).
    pub layout: String,
    /// Plate geometry, e.g. 8 × 12.
    pub rows: u32,
    /// Plate columns (e.g. 12 for a 96-well plate).
    pub columns: u32,
    /// Wells with a finite value.
    pub wells: u32,
    /// What each cell shows (a column name, or `mean of N rows`).
    pub value: String,
    /// Colour-scale ends (dark = min, yellow = max).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    /// Value shown at the yellow end of the scale.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
}

/// Output of `preview`: what was rendered, and where it went.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct PreviewOutput {
    /// The input file.
    pub path: String,
    /// Format id of the input file.
    pub format: String,
    /// `image`, `trace`, `spectrum` or `plate`.
    pub kind: String,
    /// The written file (CLI); absent when the preview is returned inline (MCP image content).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// `png` or `jpeg`.
    pub encoding: String,
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// Size of the encoded image.
    pub bytes: u64,
    /// xxh3-128 of the encoded bytes, 32 hex chars (identical across runs for the same request).
    pub xxh3: String,
    /// True once a written file was read back and matched.
    pub verified: bool,
    /// Present for image previews.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<ImagePreview>,
    /// Present for trace previews.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace: Option<TracePreview>,
    /// Present for spectrum previews.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spectrum: Option<SpectrumPreview>,
    /// Present for plate previews.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plate: Option<PlatePreview>,
    /// Anything the caller should know (fallbacks, caps, implied options).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    /// What to do next with the picture (look at it; zoom with a region read off the rulers).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

impl PreviewOutput {
    /// Halve the recorded geometry along with the canvas.
    fn halved(&mut self) {
        if let Some(im) = &mut self.image {
            im.halve();
        }
    }
}

/// A rendered preview before encoding.
#[derive(Debug, Clone)]
pub struct Rendered {
    /// The rendered pixels.
    pub canvas: Canvas,
    /// What was rendered (without the encoding fields filled in).
    pub output: PreviewOutput,
}

/// Split combined selections: `["c=0,1,z=2"]` → `["c=0,1", "z=2"]` (a comma followed by
/// `axis=` starts a new selection). Plain repeatable `c=0` arguments pass through.
pub fn split_select(args: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for a in args {
        let mut cur = String::new();
        for part in a.split(',') {
            if part.contains('=') && !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            if !cur.is_empty() {
                cur.push(',');
            }
            cur.push_str(part.trim());
        }
        if !cur.is_empty() {
            out.push(cur);
        }
    }
    out
}

/// Which kind of preview a request asks for.
fn kind(info: &FileInfo, req: &PreviewRequest) -> Result<&'static str> {
    if req.table.is_some() || req.column.is_some() {
        return Ok("plate");
    }
    if req.trace.is_some() || req.sweep.is_some() || !req.channels.is_empty() {
        return Ok("trace");
    }
    if req.run.is_some() || req.spectrum.is_some() || req.scan.is_some() || req.centroid {
        return Ok("spectrum");
    }
    // Mass-spectrometry files also carry chromatograms (TIC, BPC) as traces: prefer them when
    // they are long enough to show a run, else a spectrum says more.
    let short_traces = info
        .traces
        .iter()
        .all(|t| t.sample_count < MIN_TRACE_SAMPLES_OVER_SPECTRUM);
    if !info.images.is_empty() {
        Ok("image")
    } else if !info.traces.is_empty() && (!short_traces || info.spectra.is_empty()) {
        Ok("trace")
    } else if !info.spectra.is_empty() {
        Ok("spectrum")
    } else if !info.tables.is_empty() {
        Ok("plate")
    } else {
        Err(Error::unsupported(
            "preview",
            format!("preview of a {} file", info.format.name),
            "This file holds no images, traces, spectra or tables to draw; see `openreadout info`.",
        ))
    }
}

/// Render a preview (no encoding yet).
pub fn render(ds: &mut dyn Dataset, info: &FileInfo, req: &PreviewRequest) -> Result<Rendered> {
    if req.max_size < 16 || req.max_size > MAX_PREVIEW_SIZE {
        return Err(Error::Usage(format!(
            "max size {} out of range (16..={MAX_PREVIEW_SIZE})",
            req.max_size
        )));
    }
    let k = kind(info, req)?;
    let mut out = PreviewOutput {
        path: info.path.clone(),
        format: info.format.id.clone(),
        kind: k.into(),
        ..PreviewOutput::default()
    };
    let canvas = match k {
        "image" => image::render(ds, info, req, &mut out)?,
        "trace" => trace::render(ds, info, req, &mut out)?,
        "spectrum" => spectrum::render(ds, info, req, &mut out)?,
        _ => plate::render(ds, info, req, &mut out)?,
    };
    // Plots and heat maps have a minimum legible layout; below it, shrink to honour `max_size`.
    let mut canvas = canvas;
    while canvas.width.max(canvas.height) > req.max_size {
        canvas = canvas.half();
        out.halved();
        out.notes.push(format!(
            "layout larger than max size {}; halved to {}x{}",
            req.max_size, canvas.width, canvas.height
        ));
    }
    Ok(Rendered {
        canvas,
        output: out,
    })
}

/// Encode a canvas. PNG is stored as 8-bit greyscale when every pixel is grey, else RGB.
pub fn encode(c: &Canvas, encoding: Encoding, jpeg_quality: u8) -> Result<Vec<u8>> {
    match encoding {
        Encoding::Jpeg => {
            if c.width > 65_535 || c.height > 65_535 {
                return Err(Error::Usage("JPEG previews are limited to 65535 px".into()));
            }
            Ok(jpeg::encode(c, jpeg_quality))
        }
        Encoding::Png => {
            let mut buf = Vec::new();
            let gray = c.is_gray();
            {
                let mut e = png::Encoder::new(&mut buf, c.width, c.height);
                e.set_color(if gray {
                    png::ColorType::Grayscale
                } else {
                    png::ColorType::Rgb
                });
                e.set_depth(png::BitDepth::Eight);
                e.set_compression(png::Compression::Balanced);
                let mut w = e
                    .write_header()
                    .map_err(|e| Error::Other(format!("PNG encoding failed: {e}")))?;
                let data: Vec<u8> = if gray {
                    c.rgb.as_chunks::<3>().0.iter().map(|p| p[0]).collect()
                } else {
                    c.rgb.clone()
                };
                w.write_image_data(&data)
                    .map_err(|e| Error::Other(format!("PNG encoding failed: {e}")))?;
                w.finish()
                    .map_err(|e| Error::Other(format!("PNG encoding failed: {e}")))?;
            }
            Ok(buf)
        }
    }
}

fn xxh3_hex(b: &[u8]) -> String {
    format!("{:032x}", xxhash_rust::xxh3::xxh3_128(b))
}

/// Encode a rendered preview and fill in the size, encoding and hash.
pub fn finish(
    r: &Rendered,
    encoding: Encoding,
    jpeg_quality: u8,
) -> Result<(PreviewOutput, Vec<u8>)> {
    let bytes = encode(&r.canvas, encoding, jpeg_quality)?;
    let mut out = r.output.clone();
    out.encoding = encoding.as_str().into();
    out.width = r.canvas.width;
    out.height = r.canvas.height;
    out.bytes = bytes.len() as u64;
    out.xxh3 = xxh3_hex(&bytes);
    Ok((out, bytes))
}

/// Encode within a byte budget (for inline MCP image content): the requested encoding first,
/// then JPEG at quality 85, then halving the image until it fits. Notes record what changed.
pub fn finish_within(
    r: &Rendered,
    encoding: Encoding,
    jpeg_quality: u8,
    max_bytes: usize,
) -> Result<(PreviewOutput, Vec<u8>)> {
    let (mut out, mut bytes) = finish(r, encoding, jpeg_quality)?;
    if bytes.len() <= max_bytes {
        return Ok((out, bytes));
    }
    let mut cur = r.clone();
    let mut enc = encoding;
    if enc == Encoding::Png {
        enc = Encoding::Jpeg;
        (out, bytes) = finish(&cur, enc, 85)?;
        out.notes.push(format!(
            "PNG exceeded the {max_bytes}-byte budget; re-encoded as JPEG (quality 85)"
        ));
    }
    while bytes.len() > max_bytes && cur.canvas.width.max(cur.canvas.height) > 32 {
        cur.canvas = cur.canvas.half();
        cur.output.halved();
        let notes = out.notes.clone();
        (out, bytes) = finish(&cur, enc, 85)?;
        out.notes = notes;
        out.notes.push(format!(
            "halved to {}x{} to fit the {max_bytes}-byte budget",
            cur.canvas.width, cur.canvas.height
        ));
    }
    Ok((out, bytes))
}

/// A small look at a file's first image for a summary ("auto-screenshot"): channel 0 (or a
/// composite of 2–4 channels), middle z, t 0, from the smallest pyramid level that still covers
/// `max_size`, with rulers. Fails (`unsupported`) instead of decoding more than `max_read_bytes`
/// of pixels, and on any read error: the caller skips the thumbnail and points to [`render`]
/// with a region or level.
pub fn thumbnail(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    max_size: u32,
    max_read_bytes: u64,
) -> Result<Rendered> {
    let im = info.images.first().ok_or_else(|| {
        Error::unsupported(
            "preview",
            "thumbnail of a file without images",
            "Use `preview` for traces, spectra and plates.",
        )
    })?;
    let mut req = PreviewRequest {
        image: Some(im.index),
        max_size,
        max_read_bytes: Some(max_read_bytes),
        composite: im.samples_per_pixel != 3 && (2..=4).contains(&im.size_c),
        ..PreviewRequest::default()
    };
    match render(ds, info, &req) {
        // a composite reads one plane per channel: one channel may still fit the budget
        Err(Error::Unsupported { .. }) if req.composite => {
            req.composite = false;
            render(ds, info, &req)
        }
        other => other,
    }
}

/// Default output path: `<input stem>.preview.<png|jpg>` next to the input.
pub fn default_output(input: &Path, encoding: Encoding) -> PathBuf {
    let stem = input
        .file_stem()
        .map_or_else(|| "preview".into(), |s| s.to_string_lossy().to_string());
    input.with_file_name(format!("{stem}.preview.{}", encoding.extension()))
}

/// Write `bytes` to `output` through a temporary file that is read back and compared before it is
/// renamed into place. Refuses to replace an existing file unless `overwrite`, and refuses to
/// write over the input. Returns `true` (verified).
pub fn write_verified(input: &Path, output: &Path, bytes: &[u8], overwrite: bool) -> Result<bool> {
    if output == input
        || (output.exists()
            && std::fs::canonicalize(output).ok() == std::fs::canonicalize(input).ok())
    {
        return Err(Error::Usage(
            "output path must differ from the input; raw files are never modified".into(),
        ));
    }
    if output.exists() && !overwrite {
        return Err(Error::Usage(format!(
            "{} exists; pass --overwrite to replace it",
            output.display()
        )));
    }
    let tmp = output.with_file_name(format!(
        ".{}.partial-{}",
        output
            .file_name()
            .map_or_else(|| "preview".into(), |s| s.to_string_lossy().to_string()),
        std::process::id()
    ));
    std::fs::write(&tmp, bytes).map_err(|e| Error::io(&tmp, e))?;
    let same = std::fs::read(&tmp).is_ok_and(|b| b == bytes);
    if !same {
        let _ = std::fs::remove_file(&tmp);
        return Err(Error::Other(
            "read-back verification failed; preview discarded".into(),
        ));
    }
    std::fs::rename(&tmp, output).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        Error::io(output, e)
    })?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_options() {
        assert_eq!("auto".parse::<Contrast>().unwrap(), Contrast::Auto);
        assert_eq!(
            "percentile:1,99".parse::<Contrast>().unwrap(),
            Contrast::Percentile(1.0, 99.0)
        );
        assert!("percentile:99,1".parse::<Contrast>().is_err());
        assert!("bright".parse::<Contrast>().is_err());
        assert_eq!("channel-color".parse::<Lut>().unwrap(), Lut::ChannelColor);
        assert_eq!("Z".parse::<Axis>().unwrap(), Axis::Z);
        assert_eq!(Encoding::from_path(Path::new("a.JPG")), Encoding::Jpeg);
        assert_eq!(Encoding::from_path(Path::new("a")), Encoding::Png);
    }

    #[test]
    fn splits_combined_selection() {
        assert_eq!(
            split_select(&["c=0,1,z=2".into(), "t=3".into()]),
            vec!["c=0,1", "z=2", "t=3"]
        );
        assert_eq!(split_select(&["z=1-3".into()]), vec!["z=1-3"]);
    }
}
