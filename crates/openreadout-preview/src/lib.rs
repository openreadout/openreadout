//! Deterministic previews of `OpenReadout` datasets, drawn with the figure engine
//! (`openreadout-figure`) and encoded as PNG, JPEG or SVG.
//!
//! - **Images**: one plane, a channel composite (additive, channel colours from the normalized
//!   model) or a maximum-intensity projection over z or t, read from the pyramid level nearest the
//!   requested size when the reader exposes levels, then resampled to the requested size (area
//!   average; a small image is enlarged by repeating its pixels) and mapped to 8 bits
//!   with a contrast rule (`auto`, `min-max`, `percentile:LO,HI`, `raw`). By default the picture
//!   is framed by coordinate rulers in full-resolution pixels (and a scale bar when the pixel
//!   size is known), and [`ImagePreview`] reports how PNG pixels map back to source pixels, so a
//!   viewer can read coordinates off the image and ask for a `region` of it.
//! - **Traces**: one sweep, one panel per channel (min/max envelope per pixel column, so spikes
//!   survive any downsampling), with ticked axes in the trace's units (time, NMR chemical shift,
//!   wavenumber, …). Spectra and chromatograms get their largest peaks labelled; current-clamp
//!   sweeps can get their action potentials marked ([`Marks`]). qPCR curves are overlaid.
//! - **Spectra**: one mass spectrum, as sticks (centroided) or a profile line, the largest peaks
//!   labelled with their m/z.
//! - **Plates**: a heat map of a multi-well plate table (columns named like wells `A1`…`P24`, or
//!   `well` / `row` + `column` columns); a kinetic read becomes a grid of small growth curves in
//!   plate positions, coloured by the plate layout's groups.
//! - **Events**: flow cytometry events as a density plot of two parameters (or a histogram of
//!   one).
//!
//! Callers can draw their own [`Annotation`]s on any preview (points, boxes, outlines, arrows,
//! text, lines and bands), in full-resolution pixels on images and in axis units on plots;
//! [`PreviewOutput::annotations`] says where each one landed.
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
//! use openreadout_preview::{Encoding, Raster, encode};
//!
//! let mut raster = Raster::new(4, 2, [0, 0, 0]);
//! raster.set(0, 0, [255, 255, 255]);
//! let png = encode(&raster, Encoding::Png, 90)?;
//! assert_eq!(&png[1..4], b"PNG");
//! # Ok::<(), openreadout_core::Error>(())
//! ```
#![forbid(unsafe_code)]
#![warn(missing_docs)]
// Pixel and DCT arithmetic reads best with the conventional short names (x, y, w, h, r, g, b).
#![allow(clippy::many_single_char_names)]

mod annotate;
pub mod color;
pub mod electrochem;
mod electrophoresis;
mod events;
mod figure_frame;
mod image;
mod kinetic;
mod lanes;
mod marks;
mod nmr;
mod objects;
mod options;
mod overlay;
mod plate;
mod rulers;
mod sequencing;
mod samples;
mod spectrum;
mod tic;
mod trace;
mod volume;

use std::path::{Path, PathBuf};
use std::str::FromStr;

use openreadout_core::model::FileInfo;
use openreadout_core::reader::Dataset;
use openreadout_core::{Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use annotate::{Annotation, DrawnAnnotation};
pub use electrophoresis::series_index_of;
pub use openreadout_figure::{Figure, Raster};
pub use options::PreviewOptions;
pub use plate::{PlateGrid, plate_grid};
pub use volume::View;

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
    /// A display window: values from `center - width / 2` (black) to `center + width / 2`
    /// (white), in the image's values (Hounsfield units for CT).
    Window(f64, f64),
    /// The n-th display window the file stores (`images[].extra.windows`).
    StoredWindow(usize),
}

/// Named CT display windows: `(name, center, width)` in Hounsfield units.
pub const WINDOW_PRESETS: &[(&str, f64, f64)] = &[
    ("soft-tissue", 40.0, 400.0),
    ("brain", 40.0, 80.0),
    ("subdural", 75.0, 215.0),
    ("stroke", 40.0, 40.0),
    ("bone", 400.0, 1800.0),
    ("lung", -600.0, 1500.0),
    ("mediastinum", 50.0, 350.0),
    ("liver", 60.0, 150.0),
];

impl FromStr for Contrast {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        let s = s.trim().to_ascii_lowercase();
        match s.as_str() {
            "auto" => Ok(Contrast::Auto),
            "min-max" | "minmax" => Ok(Contrast::MinMax),
            "raw" => Ok(Contrast::Raw),
            _ if s.starts_with("window") => {
                let bad = || {
                    let names: Vec<&str> = WINDOW_PRESETS.iter().map(|p| p.0).collect();
                    Error::Usage(format!(
                        "bad contrast '{s}': use window:CENTER,WIDTH (e.g. window:40,400), window:file (the file's own window; window:file:2 for the third), or a preset: window:{}",
                        names.join(", window:")
                    ))
                };
                let spec = s.strip_prefix("window:").ok_or_else(bad)?.trim();
                if let Some(rest) = spec.strip_prefix("file") {
                    let n = match rest.strip_prefix(':') {
                        Some(n) => n.trim().parse().map_err(|_| bad())?,
                        None if rest.is_empty() => 0,
                        None => return Err(bad()),
                    };
                    return Ok(Contrast::StoredWindow(n));
                }
                if let Some((_, c, w)) = WINDOW_PRESETS
                    .iter()
                    .find(|p| p.0 == spec.replace('_', "-"))
                {
                    return Ok(Contrast::Window(*c, *w));
                }
                let (c, w) = spec.split_once(',').ok_or_else(bad)?;
                let (c, w): (f64, f64) = (
                    c.trim().parse().map_err(|_| bad())?,
                    w.trim().parse().map_err(|_| bad())?,
                );
                if !c.is_finite() || !w.is_finite() || w <= 0.0 {
                    return Err(bad());
                }
                Ok(Contrast::Window(c, w))
            }
            _ => {
                let bad = || {
                    Error::Usage(format!(
                        "bad contrast '{s}': use auto, min-max, raw, percentile:LO,HI (e.g. percentile:1,99) or window:CENTER,WIDTH (window:brain, window:bone, ...)"
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
            Contrast::Window(c, w) => write!(f, "window:{c},{w}"),
            Contrast::StoredWindow(n) => write!(f, "window:file:{n}"),
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
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Default,
    serde::Deserialize,
    serde::Serialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum Axes {
    /// coordinate rulers in full-resolution pixels and a scale bar around the picture (default)
    #[default]
    Rulers,
    /// rulers, plus faint grid lines over the data at the ruler ticks
    Grid,
    /// a figure for people: the picture with a scale bar, the title and the caption; no rulers
    ScaleBar,
    /// the bare downsampled plane
    None,
}

impl Axes {
    /// Set `axes`, `grid` and `figure` of a request.
    pub fn apply(self, req: &mut PreviewRequest) {
        req.axes = self != Axes::None;
        req.grid = self == Axes::Grid;
        req.figure = self == Axes::ScaleBar;
    }
}

impl FromStr for Axes {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "rulers" => Ok(Axes::Rulers),
            "grid" => Ok(Axes::Grid),
            "scale-bar" | "scalebar" => Ok(Axes::ScaleBar),
            "none" => Ok(Axes::None),
            other => Err(Error::Usage(format!(
                "bad axes '{other}': use rulers, grid, scale-bar or none"
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
    /// vector: text stays text, image data is embedded as PNG
    Svg,
}

impl Encoding {
    /// The lowercase name (`png`, `jpeg`, `svg`).
    pub fn as_str(self) -> &'static str {
        match self {
            Encoding::Png => "png",
            Encoding::Jpeg => "jpeg",
            Encoding::Svg => "svg",
        }
    }
    /// The MIME type (`image/png`, `image/jpeg`, `image/svg+xml`).
    pub fn mime(self) -> &'static str {
        match self {
            Encoding::Png => "image/png",
            Encoding::Jpeg => "image/jpeg",
            Encoding::Svg => "image/svg+xml",
        }
    }
    /// The usual file extension (`png`, `jpg`, `svg`).
    pub fn extension(self) -> &'static str {
        match self {
            Encoding::Png => "png",
            Encoding::Jpeg => "jpg",
            Encoding::Svg => "svg",
        }
    }
    /// From a file name: `.jpg`/`.jpeg` → JPEG, `.svg` → SVG, anything else → PNG.
    pub fn from_path(p: &Path) -> Self {
        match p
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("jpg" | "jpeg") => Encoding::Jpeg,
            Some("svg") => Encoding::Svg,
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
            "svg" => Ok(Encoding::Svg),
            other => Err(Error::Usage(format!(
                "bad preview format '{other}': use png, jpeg or svg"
            ))),
        }
    }
}

/// Marks drawn from the data itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Marks {
    /// label the largest peaks of spectra and chromatograms (default)
    #[default]
    Auto,
    /// label the largest peaks of any trace
    Peaks,
    /// mark the action potentials of current-clamp sweeps
    Spikes,
    /// images: outline and number the nuclei, cells or spots found (`stats` objects)
    Objects,
    /// gels and blots: box and number the lanes, draw their profiles and bands (`stats` lanes)
    Lanes,
    /// no marks
    None,
}

impl FromStr for Marks {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" => Ok(Marks::Auto),
            "peaks" => Ok(Marks::Peaks),
            "spikes" => Ok(Marks::Spikes),
            "objects" => Ok(Marks::Objects),
            "lanes" => Ok(Marks::Lanes),
            "none" => Ok(Marks::None),
            other => Err(Error::Usage(format!(
                "bad marks '{other}': use auto, peaks, spikes, objects, lanes or none"
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
    /// Which plane of a volume to show (default: the stored planes, or for medical images the
    /// anatomical plane they were acquired in, oriented the radiology way).
    pub view: Option<View>,
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
    /// Trace preview: only this window of the x axis, `[a, b]` in its units (bp of a sized electropherogram, base positions of a Sanger read, minutes, ppm, …).
    pub x_range: Option<[f64; 2]>,
    /// Spectrum preview: run index (default 0).
    pub run: Option<u32>,
    /// Spectrum preview: zero-based spectrum index (default 0).
    pub spectrum: Option<u64>,
    /// Spectrum preview: instrument scan number instead of an index.
    pub scan: Option<u64>,
    /// Spectrum preview: the instrument's centroid list instead of the profile.
    pub centroid: bool,
    /// Plate or event preview: table index (default 0).
    pub table: Option<u32>,
    /// Table columns to draw: a plate's value column (long layout; default the first that is not
    /// a position), or the x and y parameters of an event preview (one gives a histogram;
    /// default FSC-A and SSC-A, else the first two).
    pub columns: Vec<String>,
    /// Marks drawn from the data (peak labels, action potentials, objects).
    pub marks: Marks,
    /// Image preview: how objects are found for `marks: objects` (setting it implies them).
    pub objects: Option<openreadout_core::measure::ObjectsRequest>,
    /// Image preview: how lanes are found for `marks: lanes` (setting it implies them).
    pub lanes: Option<openreadout_core::measure::LanesRequest>,
    /// The caller's own marks, drawn on top.
    pub annotations: Vec<Annotation>,
    /// Draw in the dark theme (light ink on a dark ground).
    pub dark: bool,
    /// Image preview: frame the picture with coordinate rulers (full-resolution pixels) and a
    /// scale bar (default `true`); `false` gives the bare downsampled plane. `max_size` bounds
    /// the whole picture, rulers included.
    pub axes: bool,
    /// Image preview: faint grid lines over the data at the ruler ticks (default `false`; only
    /// with `axes`).
    pub grid: bool,
    /// Image preview: frame the picture as a figure for people instead of with rulers: a scale
    /// bar under it, and `title` and `caption` (default `false`; only with `axes`).
    pub figure: bool,
    /// Image preview: a title above the picture (drawn with `figure`).
    pub title: Option<String>,
    /// Image preview: a caption under the picture (drawn with `figure`).
    pub caption: Option<String>,
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
            view: None,
            composite: false,
            level: None,
            region: None,
            max_size: DEFAULT_PREVIEW_SIZE,
            contrast: Contrast::Auto,
            lut: None,
            trace: None,
            sweep: None,
            channels: Vec::new(),
            x_range: None,
            run: None,
            spectrum: None,
            scan: None,
            centroid: false,
            table: None,
            columns: Vec::new(),
            marks: Marks::Auto,
            objects: None,
            lanes: None,
            annotations: Vec::new(),
            dark: false,
            axes: true,
            grid: false,
            figure: false,
            title: None,
            caption: None,
            max_read_bytes: None,
        }
    }
}

/// An image preview, through a view of the volume when one applies ([`volume`]).
fn render_image(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    req: &PreviewRequest,
    out: &mut PreviewOutput,
) -> Result<(Figure, Frame)> {
    let image = req.image.unwrap_or(0);
    let Some(im) = info.images.iter().find(|i| i.index == image) else {
        return image::render(ds, info, req, out);
    };
    let Some(view) = volume::effective(info, im, req.view) else {
        return image::render(ds, info, req, out);
    };
    let sel = openreadout_core::select::Selection::parse(&split_select(&req.select))?;
    let mut req = req.clone();
    let select_z = sel.z.first().copied();
    if view == View::Orthogonal {
        // the z selection places the crosshair; the composite itself is one plane
        let join = |a: &str, v: &[u32]| {
            (!v.is_empty()).then(|| {
                format!(
                    "{a}={}",
                    v.iter().map(u32::to_string).collect::<Vec<_>>().join(",")
                )
            })
        };
        req.select = [join("c", &sel.c), join("t", &sel.t)]
            .into_iter()
            .flatten()
            .collect();
        if req.mip.is_some() {
            return Err(Error::Usage(
                "an orthogonal view shows three slices; project with view axial, coronal or sagittal and mip z".into(),
            ));
        }
        if req.axes && !req.figure {
            // rulers would count composite pixels; frame it as a figure with a scale bar
            req.figure = true;
            req.grid = false;
            out.notes
                .push("orthogonal views are framed with a scale bar, not rulers".into());
        }
    }
    let (mut vds, vim, marks, notes) = volume::ViewDataset::new(ds, info, image, view, select_z)?;
    out.notes.extend(notes);
    let slices = vim.size_z;
    let mut vinfo = info.clone();
    if let Some(slot) = vinfo.images.iter_mut().find(|i| i.index == image) {
        *slot = vim;
    }
    let (mut figure, frame) = image::render(&mut vds, &vinfo, &req, out)?;
    // the view's own marks (panel names, crosshairs), under the caller's annotations
    if !marks.is_empty() {
        let theme = theme(&req);
        annotate::draw(&mut figure, &frame, &marks, &theme, &mut Vec::new())?;
    }
    let drawn = (figure, frame);
    if let Some(p) = out.image.as_mut() {
        p.view = Some(
            match vds.view() {
                View::Axial => "axial",
                View::Coronal => "coronal",
                View::Sagittal => "sagittal",
                View::Orthogonal => "orthogonal",
                View::Xz => "xz",
                View::Yz => "yz",
                View::Native => "native",
            }
            .into(),
        );
        p.crosshair = vds.point();
        p.view_slices = Some(slices);
    }
    if req.view.is_none() {
        out.notes.push(format!(
            "shown as a radiological {} view (view: native shows the stored orientation)",
            out.image
                .as_ref()
                .and_then(|p| p.view.clone())
                .unwrap_or_default()
        ));
    }
    let _ = vds.ortho_step();
    Ok(drawn)
}

/// The theme a request draws in, sized for its `max_size`.
pub(crate) fn theme(req: &PreviewRequest) -> openreadout_figure::Theme {
    let t = if req.dark {
        openreadout_figure::Theme::dark()
    } else {
        openreadout_figure::Theme::light()
    };
    t.for_size(req.max_size as f32)
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
    /// Pixels of the plane at `level` per drawn pixel, horizontally: above 1 when the plane was
    /// made smaller (an area average), below 1 when it was enlarged (each source pixel drawn as
    /// a square), 1 when drawn as is.
    pub downsample: f64,
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
    /// The plane of the volume shown, when it is not the stored planes: `axial`, `coronal`,
    /// `sagittal`, `orthogonal`, `xz` or `yz`. Rulers, regions and annotations then count the
    /// pixels of that view.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<String>,
    /// Orthogonal views: the voxel (x, y, z of the stored planes) the three planes cross at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crosshair: Option<[u32; 3]>,
    /// With a view: how many slices it has (the range of `z`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view_slices: Option<u32>,
}

impl ImagePreview {
    /// Update the pixel mapping for a raster halved by [`Raster::half`].
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
    /// Bottom of the panel's y axis.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    /// Top of the panel's y axis.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
}

/// The horizontal axis of a trace or spectrum preview.
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
    /// One panel per channel, top to bottom (one shared panel for overlays).
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
    /// Largest intensity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_intensity: Option<f64>,
    /// Precursor m/z of a fragment (MS2+) scan.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub precursor_mz: Option<f64>,
}

/// Details of a chromatogram preview of a mass-spectrometry run (its first look).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ChromatogramPreview {
    /// Run index.
    pub run: u32,
    /// `tic` (the total ion current each scan header records) or `bpc` (base-peak intensity).
    pub signal: String,
    /// MS1 scans drawn.
    pub ms1_scans: u64,
    /// Scans in the run (all MS levels).
    pub scans: u64,
    /// Retention time of the tallest point, minutes.
    pub apex_rt_min: f64,
    /// Its intensity.
    pub apex_intensity: f64,
}

/// One layout group of a plate preview (a SoftMax Pro group, a sample name, …).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct PlateGroup {
    /// Group name as the file records it.
    pub name: String,
    /// Colour it is drawn in, `#RRGGBB`.
    pub color: String,
    /// Wells in the group.
    pub wells: u32,
    /// True for blank wells (drawn faint).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub blank: bool,
}

/// Details of a plate preview.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct PlatePreview {
    /// Table index.
    pub table: u32,
    /// `wide` (one column per well) or `long` (`well` or `row` + `column` columns).
    pub layout: String,
    /// `heatmap` (one value per well) or `curves` (a kinetic read, one small plot per well).
    pub view: String,
    /// Plate geometry, e.g. 8 × 12.
    pub rows: u32,
    /// Plate columns (e.g. 12 for a 96-well plate).
    pub columns: u32,
    /// Wells with a finite value.
    pub wells: u32,
    /// What each cell shows (a column name, or `mean of N rows`).
    pub value: String,
    /// Heat map: value at the light end of the colour scale; curves: bottom of every card's y
    /// axis.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    /// Heat map: value at the dark end of the colour scale; curves: top of every card's y axis.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    /// Curves: the time axis every card shares, in `time_unit`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<[f64; 2]>,
    /// Curves: `s`, `min` or `h`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_unit: Option<String>,
    /// The layout field the colours follow (e.g. `SoftMax group`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grouped_by: Option<String>,
    /// The groups, in the order of the key.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<PlateGroup>,
}

/// Details of an event (flow cytometry) preview.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct EventsPreview {
    /// Table index.
    pub table: u32,
    /// Parameter on the x axis.
    pub x: String,
    /// Parameter on the y axis (absent for a histogram).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<String>,
    /// Events drawn.
    pub events: u64,
    /// Events in the table.
    pub total_events: u64,
    /// `linear` or `asinh` (each axis, x first).
    pub scales: Vec<String>,
}

/// One plot panel inside the picture, for mapping PNG pixels to data values and back: the
/// value at the panel's left edge is `x[0]`, at its right edge `x[1]`, at its bottom `y[0]`
/// and at its top `y[1]`, along the scale named in `x_scale` / `y_scale`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct PlotPanel {
    /// The panel's rectangle in PNG pixels.
    pub plot_area: PxRect,
    /// Values at the left and right edges.
    pub x: [f64; 2],
    /// Values at the bottom and top edges.
    pub y: [f64; 2],
    /// `linear`, `log` or `asinh`.
    pub x_scale: String,
    /// `linear`, `log` or `asinh`.
    pub y_scale: String,
    /// Trace previews: the channel drawn in this panel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<u32>,
    /// Plate curve previews: the well of this card.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub well: Option<String>,
}

/// A rectangle in PNG pixels (fractional).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PxRect {
    /// Left edge.
    pub x: f64,
    /// Top edge.
    pub y: f64,
    /// Width.
    pub width: f64,
    /// Height.
    pub height: f64,
}

impl From<openreadout_figure::Rect> for PxRect {
    fn from(r: openreadout_figure::Rect) -> Self {
        let q = |v: f32| (f64::from(v) * 100.0).round() / 100.0;
        PxRect {
            x: q(r.x),
            y: q(r.y),
            width: q(r.w),
            height: q(r.h),
        }
    }
}

/// A mark drawn from the data: a labelled peak or a detected action potential.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct DataMark {
    /// `peak` or `spike`.
    pub kind: String,
    /// Position along the x axis, in its unit.
    pub x: f64,
    /// Value there, in the panel's unit.
    pub y: f64,
    /// The label drawn (e.g. the m/z or ppm), if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Trace previews: the channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<u32>,
}

/// Output of `preview`: what was rendered, and where it went.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct PreviewOutput {
    /// The input file.
    pub path: String,
    /// Format id of the input file.
    pub format: String,
    /// `image`, `trace`, `chromatogram`, `spectrum`, `plate` or `events`.
    pub kind: String,
    /// The written file (CLI); absent when the preview is returned inline (MCP image content).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// `png`, `jpeg` or `svg`.
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
    /// Present for chromatogram previews (a mass-spectrometry run's total-ion chromatogram).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chromatogram: Option<ChromatogramPreview>,
    /// Present for plate previews.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plate: Option<PlatePreview>,
    /// Present for event previews.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub events: Option<EventsPreview>,
    /// Plot panels (traces, spectra, plate curves, events): where each sits in the picture and
    /// the data range it shows.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub panels: Vec<PlotPanel>,
    /// Marks drawn from the data (labelled peaks, action potentials).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub marks: Vec<DataMark>,
    /// `marks: objects`: the objects counted in the area shown, as `stats` objects (the first
    /// 20 listed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<serde_json::Map<String, serde_json::Value>>")]
    pub objects: Option<openreadout_core::measure::Objects>,
    /// `marks: lanes`: the lanes and bands found in the area shown, as `stats` lanes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<serde_json::Map<String, serde_json::Value>>")]
    pub lanes: Option<openreadout_core::measure::Lanes>,
    /// The caller's annotations as drawn: each as given, plus where it landed in PNG pixels.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub annotations: Vec<DrawnAnnotation>,
    /// Anything the caller should know (fallbacks, caps, implied options).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    /// What to do next with the picture (look at it; zoom with a region read off the rulers).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// How to save this picture to a file (MCP: the openreadout_export call).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub save: Option<String>,
}

impl PreviewOutput {
    /// Halve the recorded geometry along with the raster.
    fn halved(&mut self) {
        if let Some(im) = &mut self.image {
            im.halve();
        }
        let h = |r: &mut PxRect| {
            r.x /= 2.0;
            r.y /= 2.0;
            r.width /= 2.0;
            r.height /= 2.0;
        };
        for p in &mut self.panels {
            h(&mut p.plot_area);
        }
        for a in &mut self.annotations {
            a.halve();
        }
    }
}

/// How figure pixels relate to data: what the annotations are placed in.
#[derive(Debug, Clone)]
pub(crate) enum Frame {
    /// An image: the plot area and the full-resolution mapping.
    Image {
        plot_area: openreadout_core::Region,
        map: rulers::Mapping,
    },
    /// Plot panels, each with the channel or well it shows, and where the figure's key may
    /// take more entries (a plate grid's annotations are labelled there).
    Plots {
        panels: Vec<FramePanel>,
        key: Option<KeySlot>,
        /// Labels the preview drew itself (peak labels), which the caller's labels keep off.
        taken: Vec<openreadout_figure::Rect>,
    },
    /// A plate heat map: each well's cell.
    Wells(Vec<((u32, u32), openreadout_figure::Rect)>),
}

impl Frame {
    /// Plot panels without a key.
    pub(crate) fn plots(panels: Vec<FramePanel>) -> Frame {
        Frame::Plots {
            panels,
            key: None,
            taken: Vec::new(),
        }
    }

    /// Plot panels whose own labels occupy `taken`.
    pub(crate) fn plots_labelled(
        panels: Vec<FramePanel>,
        taken: &[openreadout_figure::Rect],
    ) -> Frame {
        Frame::Plots {
            panels,
            key: None,
            taken: taken.to_vec(),
        }
    }
}

/// Room at the end of a figure's key row: the next entry starts at `x`, centred on `y`, and
/// must end before `max_x`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct KeySlot {
    pub x: f32,
    pub y: f32,
    pub max_x: f32,
}

/// A plot panel and what it shows.
#[derive(Debug, Clone)]
pub(crate) struct FramePanel {
    pub panel: openreadout_figure::Panel,
    pub channel: Option<u32>,
    pub well: Option<(u32, u32)>,
}

impl FramePanel {
    pub(crate) fn new(panel: openreadout_figure::Panel) -> Self {
        FramePanel {
            panel,
            channel: None,
            well: None,
        }
    }
}

/// A rendered preview before encoding.
#[derive(Debug, Clone)]
pub struct Rendered {
    /// The rendered pixels.
    pub canvas: Raster,
    /// The figure the pixels were rendered from (for SVG output).
    pub figure: Figure,
    /// What was rendered (without the encoding fields filled in).
    pub output: PreviewOutput,
}

impl Rendered {
    /// Give this picture the pixels of `big`, the same image read at `scale` times the size,
    /// so that saving it at `scale` draws a sharper version of this picture: type, lines,
    /// outlines and labels keep their size relative to the image, and the image has real
    /// pixels. Returns false (and changes nothing) when `big` has no image of about that size.
    pub fn take_pixels(&mut self, big: &Rendered, scale: f32) -> bool {
        use openreadout_figure::Item;
        fn largest(f: &Figure) -> Option<usize> {
            f.items
                .iter()
                .enumerate()
                .filter_map(|(i, it)| match it {
                    Item::Image { rect, .. } => Some((i, rect.w * rect.h)),
                    _ => None,
                })
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .map(|x| x.0)
        }
        let (Some(i), Some(j)) = (largest(&self.figure), largest(&big.figure)) else {
            return false;
        };
        let Item::Image { raster, .. } = &big.figure.items[j] else {
            return false;
        };
        let raster = raster.clone();
        let Item::Image {
            rect,
            raster: mine,
            smooth,
        } = &mut self.figure.items[i]
        else {
            return false;
        };
        // the same region (the same shape), with more pixels than this picture shows. The
        // larger picture's frame does not grow in proportion, so its image is near, not exactly
        // at, `scale` times the size; it is then resampled smoothly to the saved size.
        let (w, h) = (raster.width as f32, raster.height as f32);
        let same_shape = (w / h - rect.w / rect.h).abs() <= 0.02 * rect.w / rect.h;
        if !same_shape || w <= mine.width as f32 {
            return false;
        }
        if (w - rect.w * scale).abs() > 1.0 {
            *smooth = true;
        }
        *mine = raster;
        true
    }
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
    if req.table.is_some() || !req.columns.is_empty() {
        return Ok(if events::is_events(info, req.table.unwrap_or(0)) {
            "events"
        } else {
            "plate"
        });
    }
    if req.trace.is_some()
        || req.sweep.is_some()
        || !req.channels.is_empty()
        || req.x_range.is_some()
    {
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
        Ok(if events::is_events(info, 0) {
            "events"
        } else {
            "plate"
        })
    } else {
        Err(Error::unsupported(
            "preview",
            format!("preview of a {} file", info.format.name),
            "This file holds no images, traces, spectra or tables to draw; see `openreadout info`.",
        ))
    }
}

/// Most annotations one picture takes.
pub const MAX_ANNOTATIONS: usize = 500;
/// Most trace channels one picture draws.
pub const MAX_CHANNELS: usize = 64;
/// Longest title, in characters.
pub const MAX_TITLE_CHARS: usize = 300;
/// Longest caption, in characters.
pub const MAX_CAPTION_CHARS: usize = 2000;

/// Refuse requests too large to draw: many annotations or channels, long titles and captions.
fn check_limits(req: &PreviewRequest) -> Result<()> {
    if req.max_size < 16 || req.max_size > MAX_PREVIEW_SIZE {
        return Err(Error::Usage(format!(
            "max size {} is out of range: use 16 to {MAX_PREVIEW_SIZE}",
            req.max_size
        )));
    }
    let over = |what: &str, n: usize, max: usize, how: &str| {
        Err(Error::Usage(format!(
            "{n} {what} are over the limit of {max}; {how}"
        )))
    };
    if req.annotations.len() > MAX_ANNOTATIONS {
        return over(
            "annotations",
            req.annotations.len(),
            MAX_ANNOTATIONS,
            "mark the most telling ones, or zoom in with region and annotate each part",
        );
    }
    let mut channels = req.channels.clone();
    channels.sort_unstable();
    channels.dedup();
    if channels.len() > MAX_CHANNELS {
        return over(
            "channels",
            channels.len(),
            MAX_CHANNELS,
            "draw fewer channels per picture",
        );
    }
    for (what, text, max) in [
        ("title", &req.title, MAX_TITLE_CHARS),
        ("caption", &req.caption, MAX_CAPTION_CHARS),
    ] {
        let n = text.as_deref().map_or(0, |t| t.chars().count());
        if n > max {
            return Err(Error::Usage(format!(
                "a {what} of {n} characters is over the limit of {max}; shorten it, and give the full text in your reply"
            )));
        }
    }
    Ok(())
}

/// Render a preview (no encoding yet).
pub fn render(ds: &mut dyn Dataset, info: &FileInfo, req: &PreviewRequest) -> Result<Rendered> {
    check_limits(req)?;
    let k = kind(info, req)?;
    let mut out = PreviewOutput {
        path: info.path.clone(),
        format: info.format.id.clone(),
        kind: k.into(),
        ..PreviewOutput::default()
    };
    // An NMR data set shows its spectrum unless the caller picked a trace, sweep or channel.
    let nmr_default = k == "trace"
        && info.format.family == "nmr"
        && req.trace.is_none()
        && req.sweep.is_none()
        && req.channels.is_empty();
    // A sequencing data set shows its own view (run quality, a read's squiggle, read lengths)
    // unless the caller picked a table, a trace other than a nanopore read, or channels.
    let seq_default = info.format.family == "sequencing"
        && req.table.is_none()
        && req.columns.is_empty()
        && req.channels.is_empty()
        && req.trace.is_none_or(|t| t == 0);
    let mut drawn = if nmr_default {
        nmr::render(ds, info, req, &mut out)
    } else if seq_default {
        sequencing::render(ds, info, req, &mut out)
    } else {
        None
    };
    // A Sanger read or a sized fragment run shows its chromatogram or electropherogram.
    if drawn.is_none() && k == "trace" && info.format.family == "electrophoresis" {
        drawn = electrophoresis::render(ds, info, req, &mut out)?;
    }
    // A mass-spectrometry run shows its chromatogram unless a spectrum was asked for.
    if drawn.is_none()
        && k == "spectrum"
        && req.run.is_none()
        && req.spectrum.is_none()
        && req.scan.is_none()
        && !req.centroid
    {
        drawn = tic::render(ds, info, req, &mut out)?;
        if drawn.is_some() {
            out.kind = "chromatogram".into();
        }
    }
    if drawn.is_none() && samples::applies(info, req) {
        drawn = Some(samples::render(ds, info, req, &mut out)?);
        out.kind = "samples".into();
    }
    let (mut figure, frame) = match (k, drawn) {
        (_, Some(drawn)) => drawn,
        ("image", _) => render_image(ds, info, req, &mut out)?,
        ("trace", _) if electrochem::is_voltammetry(info, req) => {
            electrochem::render(ds, info, req, &mut out)?
        }
        ("trace", _) => trace::render(ds, info, req, &mut out)?,
        ("spectrum", _) => spectrum::render(ds, info, req, &mut out)?,
        ("events", _) => events::render(ds, info, req, &mut out)?,
        _ => plate::render(ds, info, req, &mut out)?,
    };
    if out.kind != "image" && (req.title.is_some() || req.caption.is_some()) {
        out.notes.push(
            "title and caption are drawn on image previews; openreadout_plot titles and captions plots"
                .into(),
        );
    }
    if let Frame::Plots { panels, .. } = &frame {
        out.panels = panels.iter().map(annotate::plot_panel).collect();
    }
    if k == "image" && (req.marks == Marks::Objects || req.objects.is_some()) {
        objects::draw(ds, info, req, &frame, &mut figure, &mut out)?;
    }
    if k == "image" && (req.marks == Marks::Lanes || req.lanes.is_some()) {
        lanes::draw(ds, info, req, &frame, &mut figure, &mut out);
    }
    if !req.annotations.is_empty() {
        let theme = theme(req);
        out.annotations = annotate::draw(
            &mut figure,
            &frame,
            &req.annotations,
            &theme,
            &mut out.notes,
        )?;
    }
    figure.title = Some(title(&out));
    let mut canvas = figure.to_raster(1.0);
    // Plots and heat maps have a minimum legible layout; below it, shrink to honour `max_size`.
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
        figure,
        output: out,
    })
}

/// A one-line description of the picture (the SVG title).
fn title(o: &PreviewOutput) -> String {
    let name = Path::new(&o.path)
        .file_name()
        .map_or_else(|| o.path.clone(), |n| n.to_string_lossy().to_string());
    format!("{} preview of {name}", o.kind)
}

/// Encode a raster as PNG (8-bit greyscale when every pixel is grey, else RGB) or JPEG. SVG
/// needs the figure: use [`finish`].
pub fn encode(c: &Raster, encoding: Encoding, jpeg_quality: u8) -> Result<Vec<u8>> {
    match encoding {
        Encoding::Jpeg => c.to_jpeg(jpeg_quality),
        Encoding::Png => c.to_png(),
        Encoding::Svg => Err(Error::Usage(
            "SVG is drawn from the figure, not a raster; use finish()".into(),
        )),
    }
}

/// Encode a figure at its own size: PNG, JPEG or SVG.
pub fn encode_figure(f: &Figure, encoding: Encoding, jpeg_quality: u8) -> Result<Vec<u8>> {
    match encoding {
        Encoding::Svg => Ok(f.to_svg().into_bytes()),
        e => encode(&f.to_raster(1.0), e, jpeg_quality),
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
    let bytes = match encoding {
        Encoding::Svg => r.figure.to_svg().into_bytes(),
        e => encode(&r.canvas, e, jpeg_quality)?,
    };
    let mut out = r.output.clone();
    out.encoding = encoding.as_str().into();
    out.width = r.canvas.width;
    out.height = r.canvas.height;
    out.bytes = bytes.len() as u64;
    out.xxh3 = xxh3_hex(&bytes);
    Ok((out, bytes))
}

/// Encode within a byte budget (for inline MCP image content): the requested encoding first,
/// then JPEG at quality 85, then halving the image until it fits. `encoding` in the output
/// says when the image became a JPEG. Halving adds one note, worded for the person looking
/// at the picture.
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
    if enc != Encoding::Jpeg {
        enc = Encoding::Jpeg;
        (out, bytes) = finish(&cur, enc, 85)?;
    }
    let mut halved = false;
    while bytes.len() > max_bytes && cur.canvas.width.max(cur.canvas.height) > 32 {
        cur.canvas = cur.canvas.half();
        cur.output.halved();
        (out, bytes) = finish(&cur, enc, 85)?;
        halved = true;
    }
    if halved {
        out.notes.push(format!(
            "shown at reduced resolution ({}x{} px); preview a region for full detail",
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

/// Default output path: `<input stem>.preview.<png|jpg|svg>` next to the input.
pub fn default_output(input: &Path, encoding: Encoding) -> PathBuf {
    let stem = input
        .file_stem()
        .map_or_else(|| "preview".into(), |s| s.to_string_lossy().to_string());
    input.with_file_name(format!("{stem}.preview.{}", encoding.extension()))
}

/// Write `bytes` to `output` through a temporary file that is read back and compared before it is
/// renamed into place. Refuses to replace an existing file unless `overwrite`, and refuses to
/// write over the input however its path is spelled. Returns `true` (verified).
pub fn write_verified(input: &Path, output: &Path, bytes: &[u8], overwrite: bool) -> Result<bool> {
    openreadout_core::output::write_verified(output, bytes, overwrite, &[input])?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Saving at a larger scale keeps the picture as drawn and takes only the image's pixels
    /// from the larger reading, so type and outlines grow with the save.
    #[test]
    fn a_scaled_save_takes_only_the_pixels() {
        use std::sync::Arc;
        let picture = |w: u32, h: u32, rect: openreadout_figure::Rect| {
            let mut figure = Figure::new(rect.w + 20.0, rect.h + 20.0, None);
            figure.image(rect, Arc::new(Raster::new(w, h, [9, 9, 9])), false);
            figure.text(
                5.0,
                5.0,
                "12 nuclei",
                &openreadout_figure::TextStyle::new(12.0, openreadout_figure::Color::rgb(0, 0, 0)),
            );
            Rendered {
                canvas: Raster::new(1, 1, [0, 0, 0]),
                figure,
                output: PreviewOutput::default(),
            }
        };
        let mut small = picture(
            100,
            50,
            openreadout_figure::Rect::new(10.0, 10.0, 100.0, 50.0),
        );
        let big = picture(
            300,
            150,
            openreadout_figure::Rect::new(10.0, 10.0, 300.0, 150.0),
        );
        assert!(small.take_pixels(&big, 3.0));
        assert_eq!(small.figure.width, 120.0, "the layout stays the small one");
        let sizes: Vec<(u32, u32)> = small
            .figure
            .items
            .iter()
            .filter_map(|i| match i {
                openreadout_figure::Item::Image { raster, .. } => {
                    Some((raster.width, raster.height))
                }
                _ => None,
            })
            .collect();
        assert_eq!(sizes, vec![(300, 150)]);
        let mut other = picture(
            100,
            50,
            openreadout_figure::Rect::new(10.0, 10.0, 100.0, 50.0),
        );
        assert!(!other.take_pixels(
            &picture(
                120,
                50,
                openreadout_figure::Rect::new(0.0, 0.0, 120.0, 50.0)
            ),
            3.0
        ));
    }

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
        assert_eq!("spikes".parse::<Marks>().unwrap(), Marks::Spikes);
        assert_eq!(Encoding::from_path(Path::new("a.JPG")), Encoding::Jpeg);
        assert_eq!(Encoding::from_path(Path::new("a.svg")), Encoding::Svg);
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
