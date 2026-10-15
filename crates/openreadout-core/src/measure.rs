//! Measurements on one image plane: an intensity profile along a line, statistics inside a
//! polygon, and objects found by a threshold (connected components with their sizes), so that
//! "how bright is the membrane", "what is in this region" and "how many nuclei" each take one
//! call.
//!
//! Coordinates are pixels of the resolution level read (level 0 = full resolution), the same
//! as `stats` regions and the rulers `preview` draws at level 0. Only the bounding rectangle
//! of what is measured is read, so a line across a whole-slide image costs the tiles it
//! crosses.
//!
//! Definitions (`book/src/reference/commands/stats.md`, "Measurements"):
//!
//! - **profile**: values sampled every pixel along the line by bilinear interpolation, each
//!   the mean over `width` pixels across the line (at most [`MAX_PROFILE_SAMPLES`] samples;
//!   longer lines are sampled more coarsely). The statistics, peak and FWHM are of those
//!   samples; `values` returns at most [`PROFILE_VALUES`] of them, each the mean of
//!   consecutive samples.
//! - **polygon**: a pixel is inside when its centre is (even-odd rule).
//! - **objects**: the plane (RGB: the mean of the components) is smoothed with a Gaussian of
//!   `smooth` pixels σ, thresholded (a value, or Otsu's threshold over a 256-bin histogram),
//!   and the pixels above it (`polarity: dark`: below it) are joined into 8-connected
//!   objects, holes filled. With `split` (default), touching objects are cut apart where the neck between
//!   them is at most three quarters as wide as the smaller one is deep (a distance-transform
//!   watershed). Objects smaller than `min_area_px` are dropped. Intensities are of the raw,
//!   unsmoothed plane.

// x, y, w, h, c, k are the natural names in image arithmetic.
#![allow(clippy::many_single_char_names)]

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::model::{FileInfo, ImageInfo};
use crate::pixel::Plane;
use crate::reader::{Dataset, PlaneIndex};
use crate::region::Region;
use crate::select::Selection;

/// Most samples taken along a profile line.
pub const MAX_PROFILE_SAMPLES: usize = 20_000;
/// Most profile values returned per channel.
pub const PROFILE_VALUES: usize = 200;
/// Most pixels a measurement reads (a 2896 × 2896 region).
pub const MAX_MEASURE_PIXELS: u64 = 1 << 23;
/// Objects listed by default.
pub const DEFAULT_MAX_OBJECTS: usize = 100;

/// Bright or dark objects.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Polarity {
    /// bright for one-channel images (fluorescence), dark for RGB (stained brightfield)
    #[default]
    Auto,
    /// objects brighter than the background
    Bright,
    /// objects darker than the background
    Dark,
}

/// How objects are found.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ObjectsRequest {
    /// Threshold in raw sample units. Default: Otsu's threshold of the smoothed plane.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold: Option<f64>,
    /// Default auto.
    #[serde(default)]
    pub polarity: Polarity,
    /// Gaussian smoothing before the threshold, σ in pixels (0 = none). Default 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smooth: Option<f64>,
    /// Smallest object kept, in pixels. Default 10.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_area_px: Option<u64>,
    /// Objects listed (largest first). Default 100; the count covers all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_objects: Option<usize>,
    /// RGB images: threshold this component (0 red, 1 green, 2 blue) instead of the mean of
    /// the three. Stained nuclei usually stand out best in red.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<u32>,
    /// Cut touching objects apart at narrow necks (distance-transform watershed). Default true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split: Option<bool>,
}

/// What to measure. Coordinates are pixels of `level`.
#[derive(Debug, Clone, Default)]
pub struct MeasureRequest {
    /// Image (default: the first).
    pub image: Option<u32>,
    /// Plane selection: channels (default all for profile and polygon, the first for objects),
    /// one z (default the middle) and one t (default 0).
    pub select: Vec<String>,
    /// Pyramid level.
    pub level: u32,
    /// The rectangle objects are counted in (default the whole plane).
    pub region: Option<Region>,
    /// Profile line `[x0, y0, x1, y1]`.
    pub line: Option<[f64; 4]>,
    /// Profile width in pixels (default 1).
    pub line_width: Option<u32>,
    /// Polygon vertices `[[x, y], ...]`.
    pub polygon: Option<Vec<[f64; 2]>>,
    /// Count objects.
    pub objects: Option<ObjectsRequest>,
}

impl MeasureRequest {
    /// True when the request asks for any measurement.
    pub fn any(&self) -> bool {
        self.line.is_some() || self.polygon.is_some() || self.objects.is_some()
    }
}

/// One channel of a profile.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ProfileChannel {
    /// Channel index (`c`), or the RGB component for RGB images.
    pub c: u32,
    /// Channel or component name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Smallest value along the line.
    pub min: f64,
    /// Largest value along the line.
    pub max: f64,
    /// Mean value along the line.
    pub mean: f64,
    /// Distance of the largest value from the start, in pixels.
    pub max_at_px: f64,
    /// The same in µm, when the pixel size is recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_at_um: Option<f64>,
    /// Full width at half maximum of the highest peak above the line's minimum, in pixels.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fwhm_px: Option<f64>,
    /// The same in µm.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fwhm_um: Option<f64>,
    /// The profile, one value every `step_px` from the start (at most 200; each the mean of the
    /// samples it covers).
    pub values: Vec<f64>,
}

/// An intensity profile along a line.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct Profile {
    /// `[x0, y0, x1, y1]` in pixels of the level read.
    pub line: [f64; 4],
    /// Pixels averaged across the line.
    pub width_px: u32,
    /// Line length in pixels.
    pub length_px: f64,
    /// Line length in µm, when the pixel size is recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub length_um: Option<f64>,
    /// Distance between the returned values in pixels.
    pub step_px: f64,
    /// Distance between the returned values in µm.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_um: Option<f64>,
    /// Samples taken along the line (one per pixel up to 20 000).
    pub samples: usize,
    /// One entry per channel.
    pub channels: Vec<ProfileChannel>,
}

/// Statistics of one channel inside a polygon.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct RoiChannel {
    /// Channel index (`c`), or the RGB component.
    pub c: u32,
    /// Channel or component name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Smallest value.
    pub min: f64,
    /// Largest value.
    pub max: f64,
    /// Mean value.
    pub mean: f64,
    /// Standard deviation.
    pub std: f64,
    /// Median value.
    pub median: f64,
    /// Sum of the values (integrated intensity).
    pub sum: f64,
}

/// Statistics inside a polygon.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct Roi {
    /// The vertices, in pixels of the level read.
    pub polygon: Vec<[f64; 2]>,
    /// Pixels inside.
    pub area_px: u64,
    /// Area in µm², when the pixel size is recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub area_um2: Option<f64>,
    /// One entry per channel.
    pub channels: Vec<RoiChannel>,
}

/// One object.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct Object {
    /// Centroid column, in pixels of the level read.
    pub x: f64,
    /// Centroid row.
    pub y: f64,
    /// Pixels.
    pub area_px: u64,
    /// Area in µm².
    #[serde(skip_serializing_if = "Option::is_none")]
    pub area_um2: Option<f64>,
    /// Mean raw value.
    pub mean: f64,
    /// Bounding rectangle.
    pub bbox: Region,
    /// True when the object touches the edge of the measured rectangle (it may be cut).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub touches_edge: bool,
}

/// Minimum, median, mean and maximum.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct Spread {
    /// Smallest.
    pub min: f64,
    /// Median.
    pub median: f64,
    /// Mean.
    pub mean: f64,
    /// Largest.
    pub max: f64,
}

/// Objects found by a threshold.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct Objects {
    /// What was thresholded: a channel index, an RGB component (`red`, `green`, `blue`) or
    /// `luminance` (the mean of the RGB components).
    pub measured: String,
    /// The rectangle searched, in pixels of the level read.
    pub region: Region,
    /// Threshold in raw units.
    pub threshold: f64,
    /// `otsu` or `given`.
    pub threshold_method: String,
    /// `bright` or `dark`.
    pub polarity: Polarity,
    /// Gaussian σ in pixels.
    pub smooth_px: f64,
    /// Smallest object kept, in pixels.
    pub min_area_px: u64,
    /// Whether touching objects were cut apart.
    pub split: bool,
    /// Objects found (all, including those touching the edge).
    pub count: usize,
    /// Objects touching the edge of the rectangle.
    pub touching_edge: usize,
    /// Objects more than twice the median area: possibly several touching objects.
    pub large: usize,
    /// Fraction of the rectangle covered by objects.
    pub area_fraction: f64,
    /// Object areas in pixels.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub area_px: Option<Spread>,
    /// Object areas in µm².
    #[serde(skip_serializing_if = "Option::is_none")]
    pub area_um2: Option<Spread>,
    /// Median equivalent diameter (of a disc of the same area) in µm.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub median_diameter_um: Option<f64>,
    /// The objects, largest first.
    pub objects: Vec<Object>,
    /// True when `objects` lists fewer than `count`.
    pub truncated: bool,
}

/// Measurements on one plane: a line profile, statistics inside a polygon or an object count.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct MeasureOutput {
    /// The file.
    pub path: String,
    /// Format id.
    pub format: String,
    /// Image index.
    pub image: u32,
    /// Pyramid level read; coordinates are its pixels.
    pub level: u32,
    /// Level-0 pixels per pixel of this level (1 at level 0).
    pub downsample: f64,
    /// The z plane read.
    pub z: u32,
    /// The time point read.
    pub t: u32,
    /// Pixel size at this level in µm, when recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pixel_size_um: Option<f64>,
    /// Profile along `line`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<Profile>,
    /// Statistics inside `polygon`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roi: Option<Roi>,
    /// Objects.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub objects: Option<Objects>,
    /// What was assumed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// The values of one plane as `f64`, one `Vec` per component (1 for grayscale, 3 for RGB).
struct Grid {
    x0: u32,
    y0: u32,
    w: usize,
    h: usize,
    comps: Vec<Vec<f64>>,
}

impl Grid {
    fn from_plane(p: &Plane, r: Region) -> Result<Grid> {
        let spp = p.samples_per_pixel.max(1) as usize;
        let bps = p.pixel_type.bytes_per_sample();
        let (w, h) = (p.width as usize, p.height as usize);
        if p.data.len() < w * h * spp * bps {
            return Err(Error::corrupt(
                "stats",
                format!(
                    "plane holds {} bytes, {w} x {h} needs {}",
                    p.data.len(),
                    w * h * spp * bps
                ),
            ));
        }
        let mut comps = vec![Vec::with_capacity(w * h); spp];
        for (i, s) in p
            .data
            .chunks_exact(bps)
            .take(w * h * spp)
            .map(|c| p.pixel_type.sample_f64(c))
            .enumerate()
        {
            comps[i % spp].push(s);
        }
        Ok(Grid {
            x0: r.x,
            y0: r.y,
            w,
            h,
            comps,
        })
    }

    /// Bilinear value of component `k` at level coordinates (x, y) (pixel centres at
    /// integer + 0.5); `None` outside.
    fn at(&self, k: usize, x: f64, y: f64) -> Option<f64> {
        let fx = x - 0.5 - f64::from(self.x0);
        let fy = y - 0.5 - f64::from(self.y0);
        if !(fx > -0.5 && fy > -0.5 && fx < self.w as f64 - 0.5 && fy < self.h as f64 - 0.5) {
            return None;
        }
        let fx = fx.clamp(0.0, (self.w - 1) as f64);
        let fy = fy.clamp(0.0, (self.h - 1) as f64);
        let (ix, iy) = (fx.floor() as usize, fy.floor() as usize);
        let (jx, jy) = ((ix + 1).min(self.w - 1), (iy + 1).min(self.h - 1));
        let (ax, ay) = (fx - ix as f64, fy - iy as f64);
        let c = &self.comps[k];
        let v = |x: usize, y: usize| c[y * self.w + x];
        Some(
            v(ix, iy) * (1.0 - ax) * (1.0 - ay)
                + v(jx, iy) * ax * (1.0 - ay)
                + v(ix, jy) * (1.0 - ax) * ay
                + v(jx, jy) * ax * ay,
        )
    }
}

fn usage(m: impl Into<String>) -> Error {
    Error::Usage(m.into())
}

/// The image, z, t and channels a request reads.
struct Target<'a> {
    im: &'a ImageInfo,
    z: u32,
    t: u32,
    channels: Vec<u32>,
    level_w: u32,
    level_h: u32,
    downsample: f64,
    pixel_um: Option<f64>,
}

fn target<'a>(info: &'a FileInfo, req: &MeasureRequest) -> Result<Target<'a>> {
    let Some(im) = (match req.image {
        Some(i) => info.images.iter().find(|im| im.index == i),
        None => info.images.first(),
    }) else {
        return Err(if info.images.is_empty() {
            Error::unsupported(
                "stats",
                format!("image measurements of a {} file", info.format.name),
                "This file holds no images; line, polygon and objects measure image planes.",
            )
        } else {
            usage(format!(
                "image {} does not exist (the file has {} images)",
                req.image.unwrap_or(0),
                info.images.len()
            ))
        });
    };
    let sel = Selection::parse(&req.select)?;
    let one = |axis: &[u32], size: u32, default: u32, name: &str| -> Result<u32> {
        match axis {
            [] => Ok(default),
            [v] if *v < size => Ok(*v),
            [v] => Err(usage(format!(
                "{name}={v} is out of range (image {} has {size})",
                im.index
            ))),
            _ => Err(usage(format!(
                "measurements read one plane: select one {name} (e.g. {name}={default})"
            ))),
        }
    };
    let z = one(&sel.z, im.size_z.max(1), im.size_z.max(1) / 2, "z")?;
    let t = one(&sel.t, im.size_t.max(1), 0, "t")?;
    let channels: Vec<u32> = if sel.c.is_empty() {
        (0..im.size_c.max(1)).collect()
    } else {
        if let Some(c) = sel.c.iter().find(|c| **c >= im.size_c.max(1)) {
            return Err(usage(format!(
                "c={c} is out of range (image {} has {} channels)",
                im.index, im.size_c
            )));
        }
        sel.c.clone()
    };
    let (level_w, level_h) = crate::region::level_size(im, req.level)?;
    let downsample = if level_w > 0 {
        f64::from(im.size_x) / f64::from(level_w)
    } else {
        1.0
    };
    let pixel_um = im.physical_size.x.map(|p| p * downsample);
    Ok(Target {
        im,
        z,
        t,
        channels,
        level_w,
        level_h,
        downsample,
        pixel_um,
    })
}

/// The rectangle of the level that holds the points, grown by `pad` pixels and clipped.
fn bounds(points: &[[f64; 2]], pad: f64, w: u32, h: u32) -> Result<Region> {
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for p in points {
        if !(p[0].is_finite() && p[1].is_finite()) {
            return Err(usage("coordinates must be finite numbers"));
        }
        x0 = x0.min(p[0]);
        y0 = y0.min(p[1]);
        x1 = x1.max(p[0]);
        y1 = y1.max(p[1]);
    }
    let cx0 = (x0 - pad - 1.0).floor().max(0.0);
    let cy0 = (y0 - pad - 1.0).floor().max(0.0);
    let cx1 = (x1 + pad + 1.0).ceil().min(f64::from(w));
    let cy1 = (y1 + pad + 1.0).ceil().min(f64::from(h));
    if cx1 <= cx0 || cy1 <= cy0 {
        return Err(usage(format!(
            "the shape lies outside the plane, which is {w} x {h} pixels at this level"
        )));
    }
    Ok(Region::new(
        cx0 as u32,
        cy0 as u32,
        (cx1 - cx0) as u32,
        (cy1 - cy0) as u32,
    ))
}

fn check_size(r: Region, what: &str) -> Result<()> {
    let n = u64::from(r.width) * u64::from(r.height);
    if n > MAX_MEASURE_PIXELS {
        return Err(Error::unsupported(
            "stats",
            format!("{what} over {} x {} pixels", r.width, r.height),
            format!(
                "Measure at most {MAX_MEASURE_PIXELS} pixels: pass a smaller region, or a coarser level (coordinates are then that level's pixels)."
            ),
        ));
    }
    Ok(())
}

/// Read channel `c` of the target in rectangle `r`.
fn read(ds: &mut dyn Dataset, tg: &Target<'_>, level: u32, c: u32, r: Region) -> Result<Grid> {
    let idx = PlaneIndex {
        c,
        z: tg.z,
        t: tg.t,
    };
    let p = if r.is_full(tg.level_w, tg.level_h) && level == 0 {
        ds.read_plane(tg.im.index, idx)?
    } else {
        ds.read_region(tg.im.index, idx, level, r)?
    };
    Grid::from_plane(&p, r)
}

/// The channels to report: (component grid index, label) per selected channel and component.
fn labels(tg: &Target<'_>, c: u32, spp: usize) -> Vec<(usize, u32, Option<String>)> {
    let name = tg
        .im
        .channels
        .iter()
        .find(|ch| ch.index == c)
        .and_then(|ch| ch.name.clone().or_else(|| ch.fluorophore.clone()));
    if spp == 1 {
        return vec![(0, c, name)];
    }
    ["R", "G", "B", "A"]
        .iter()
        .take(spp)
        .enumerate()
        .map(|(k, n)| (k, k as u32, Some((*n).to_string())))
        .collect()
}

/// Measure one plane.
pub fn measure(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    req: &MeasureRequest,
) -> Result<MeasureOutput> {
    let tg = target(info, req)?;
    let mut out = MeasureOutput {
        path: info.path.clone(),
        format: info.format.id.clone(),
        image: tg.im.index,
        level: req.level,
        downsample: tg.downsample,
        z: tg.z,
        t: tg.t,
        pixel_size_um: tg.pixel_um,
        ..MeasureOutput::default()
    };
    if tg.im.size_z > 1 && !req.select.iter().any(|s| s.contains('z')) {
        out.notes.push(format!(
            "read z={} (the middle of {}); select z=N for another",
            tg.z, tg.im.size_z
        ));
    }
    if let Some(line) = req.line {
        out.profile = Some(profile(
            ds,
            &tg,
            req.level,
            line,
            req.line_width.unwrap_or(1),
        )?);
    }
    if let Some(poly) = &req.polygon {
        out.roi = Some(roi(ds, &tg, req.level, poly)?);
    }
    if let Some(o) = &req.objects {
        out.objects = Some(objects(ds, &tg, req, o, &mut out.notes)?);
    }
    Ok(out)
}

fn profile(
    ds: &mut dyn Dataset,
    tg: &Target<'_>,
    level: u32,
    line: [f64; 4],
    width: u32,
) -> Result<Profile> {
    let [x0, y0, x1, y1] = line;
    let width = width.clamp(1, 1000);
    let len = (x1 - x0).hypot(y1 - y0);
    if !(len.is_finite() && len > 0.0) {
        return Err(usage(
            "line needs two different end points [x0, y0, x1, y1]",
        ));
    }
    let r = bounds(
        &[[x0, y0], [x1, y1]],
        f64::from(width) / 2.0 + 1.0,
        tg.level_w,
        tg.level_h,
    )?;
    check_size(r, "a profile")?;
    let n = ((len.ceil() as usize) + 1).clamp(2, MAX_PROFILE_SAMPLES);
    let step = len / (n - 1) as f64;
    let (ux, uy) = ((x1 - x0) / len, (y1 - y0) / len);
    let offsets: Vec<f64> = (0..width)
        .map(|k| f64::from(k) - f64::from(width - 1) / 2.0)
        .collect();
    let mut channels = Vec::new();
    for &c in &tg.channels {
        let g = read(ds, tg, level, c, r)?;
        for (k, label, name) in labels(tg, c, g.comps.len()) {
            let values: Vec<f64> = (0..n)
                .map(|i| {
                    let d = step * i as f64;
                    let (px, py) = (x0 + ux * d, y0 + uy * d);
                    let vals: Vec<f64> = offsets
                        .iter()
                        .filter_map(|o| g.at(k, px - uy * o, py + ux * o))
                        .collect();
                    if vals.is_empty() {
                        f64::NAN
                    } else {
                        vals.iter().sum::<f64>() / vals.len() as f64
                    }
                })
                .collect();
            let fin: Vec<(usize, f64)> = values
                .iter()
                .copied()
                .enumerate()
                .filter(|(_, v)| v.is_finite())
                .collect();
            let Some(&(imax, max)) = fin.iter().max_by(|a, b| a.1.total_cmp(&b.1)) else {
                return Err(usage("the line lies outside the plane"));
            };
            let min = fin.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
            let mean = fin.iter().map(|p| p.1).sum::<f64>() / fin.len() as f64;
            let fwhm_px = fwhm(&values, imax, min).map(|w| w * step);
            channels.push(ProfileChannel {
                c: label,
                name,
                min,
                max,
                mean,
                max_at_px: imax as f64 * step,
                max_at_um: tg.pixel_um.map(|p| imax as f64 * step * p),
                fwhm_px,
                fwhm_um: fwhm_px.zip(tg.pixel_um).map(|(w, p)| w * p),
                values: bin_means(&values, PROFILE_VALUES)
                    .iter()
                    .map(|v| round_sig(*v))
                    .collect(),
            });
        }
    }
    Ok(Profile {
        line,
        width_px: width,
        length_px: len,
        length_um: tg.pixel_um.map(|p| p * len),
        step_px: len / (n.clamp(2, PROFILE_VALUES) - 1) as f64,
        step_um: tg
            .pixel_um
            .map(|p| p * len / (n.clamp(2, PROFILE_VALUES) - 1) as f64),
        samples: n,
        channels,
    })
}

/// `v` shrunk to at most `k` values, each the mean of the finite values of an equal share.
fn bin_means(v: &[f64], k: usize) -> Vec<f64> {
    if v.len() <= k {
        return v.to_vec();
    }
    (0..k)
        .map(|b| {
            let (lo, hi) = (
                b * v.len() / k,
                ((b + 1) * v.len() / k).max(b * v.len() / k + 1),
            );
            let f: Vec<f64> = v[lo..hi]
                .iter()
                .copied()
                .filter(|x| x.is_finite())
                .collect();
            if f.is_empty() {
                f64::NAN
            } else {
                f.iter().sum::<f64>() / f.len() as f64
            }
        })
        .collect()
}

/// Width (in samples) at half height of the peak at `imax` above `base`, by linear
/// interpolation; `None` when the profile does not fall below half height on both sides.
fn fwhm(v: &[f64], imax: usize, base: f64) -> Option<f64> {
    let half = base + (v[imax] - base) / 2.0;
    if v[imax] <= base {
        return None;
    }
    let mut left = None;
    for i in (0..imax).rev() {
        if v[i] < half {
            let f = (half - v[i]) / (v[i + 1] - v[i]);
            left = Some(i as f64 + f);
            break;
        }
    }
    let mut right = None;
    for i in imax + 1..v.len() {
        if v[i] < half {
            let f = (v[i - 1] - half) / (v[i - 1] - v[i]);
            right = Some((i - 1) as f64 + f);
            break;
        }
    }
    Some(right? - left?)
}

/// Six significant digits keep the profile short without losing anything a sensor measures.
fn round_sig(v: f64) -> f64 {
    if !v.is_finite() || v == 0.0 {
        return v;
    }
    let d = 6 - 1 - v.abs().log10().floor() as i32;
    if d <= 0 {
        return v.round();
    }
    let m = 10f64.powi(d.min(15));
    (v * m).round() / m
}

fn inside(poly: &[[f64; 2]], x: f64, y: f64) -> bool {
    let mut c = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (xi, yi) = (poly[i][0], poly[i][1]);
        let (xj, yj) = (poly[j][0], poly[j][1]);
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            c = !c;
        }
        j = i;
    }
    c
}

fn roi(ds: &mut dyn Dataset, tg: &Target<'_>, level: u32, poly: &[[f64; 2]]) -> Result<Roi> {
    if poly.len() < 3 {
        return Err(usage("polygon needs at least three vertices [[x, y], ...]"));
    }
    let r = bounds(poly, 0.0, tg.level_w, tg.level_h)?;
    check_size(r, "a polygon")?;
    let mut mask = Vec::new();
    for y in 0..r.height {
        for x in 0..r.width {
            let (cx, cy) = (f64::from(r.x + x) + 0.5, f64::from(r.y + y) + 0.5);
            if inside(poly, cx, cy) {
                mask.push((y * r.width + x) as usize);
            }
        }
    }
    if mask.is_empty() {
        return Err(usage("the polygon holds no pixel centre"));
    }
    let mut channels = Vec::new();
    for &c in &tg.channels {
        let g = read(ds, tg, level, c, r)?;
        for (k, label, name) in labels(tg, c, g.comps.len()) {
            let mut v: Vec<f64> = mask.iter().map(|&i| g.comps[k][i]).collect();
            let n = v.len() as f64;
            let sum: f64 = v.iter().sum();
            let mean = sum / n;
            let var = v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1.0).max(1.0);
            v.sort_by(f64::total_cmp);
            channels.push(RoiChannel {
                c: label,
                name,
                min: v[0],
                max: v[v.len() - 1],
                mean,
                std: var.sqrt(),
                median: median_sorted(&v),
                sum,
            });
        }
    }
    let area = mask.len() as u64;
    Ok(Roi {
        polygon: poly.to_vec(),
        area_px: area,
        area_um2: tg.pixel_um.map(|p| area as f64 * p * p),
        channels,
    })
}

fn median_sorted(v: &[f64]) -> f64 {
    let n = v.len();
    if n == 0 {
        return f64::NAN;
    }
    if n % 2 == 1 {
        v[n / 2]
    } else {
        f64::midpoint(v[n / 2 - 1], v[n / 2])
    }
}

/// Separable Gaussian blur with edge clamping.
fn blur(v: &[f64], w: usize, h: usize, sigma: f64) -> Vec<f64> {
    if sigma <= 0.0 {
        return v.to_vec();
    }
    let rad = (3.0 * sigma).ceil() as isize;
    let k: Vec<f64> = (-rad..=rad)
        .map(|i| (-(i as f64).powi(2) / (2.0 * sigma * sigma)).exp())
        .collect();
    let ks: f64 = k.iter().sum();
    let k: Vec<f64> = k.iter().map(|x| x / ks).collect();
    let clamp = |i: isize, n: usize| i.clamp(0, n as isize - 1) as usize;
    let mut tmp = vec![0.0; v.len()];
    for y in 0..h {
        for x in 0..w {
            let mut s = 0.0;
            for (j, kv) in k.iter().enumerate() {
                s += kv * v[y * w + clamp(x as isize + j as isize - rad, w)];
            }
            tmp[y * w + x] = s;
        }
    }
    let mut out = vec![0.0; v.len()];
    for y in 0..h {
        for x in 0..w {
            let mut s = 0.0;
            for (j, kv) in k.iter().enumerate() {
                s += kv * tmp[clamp(y as isize + j as isize - rad, h) * w + x];
            }
            out[y * w + x] = s;
        }
    }
    out
}

/// Otsu's threshold over a 256-bin histogram of the finite values.
fn otsu(v: &[f64]) -> Option<f64> {
    let (lo, hi) = v
        .iter()
        .filter(|x| x.is_finite())
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), x| {
            (a.min(*x), b.max(*x))
        });
    if hi.partial_cmp(&lo) != Some(std::cmp::Ordering::Greater) {
        return None;
    }
    let bins = 256usize;
    let mut hist = vec![0u64; bins];
    let scale = (bins - 1) as f64 / (hi - lo);
    for x in v.iter().filter(|x| x.is_finite()) {
        hist[((x - lo) * scale) as usize] += 1;
    }
    let total: u64 = hist.iter().sum();
    let sum_all: f64 = hist
        .iter()
        .enumerate()
        .map(|(i, c)| i as f64 * *c as f64)
        .sum();
    let (mut w0, mut sum0) = (0u64, 0.0);
    let (mut best, mut best_i) = (-1.0, 0usize);
    for (i, c) in hist.iter().enumerate() {
        w0 += c;
        if w0 == 0 {
            continue;
        }
        let w1 = total - w0;
        if w1 == 0 {
            break;
        }
        sum0 += i as f64 * *c as f64;
        let m0 = sum0 / w0 as f64;
        let m1 = (sum_all - sum0) / w1 as f64;
        let between = w0 as f64 * w1 as f64 * (m0 - m1).powi(2);
        if between > best {
            best = between;
            best_i = i;
        }
    }
    Some(lo + (best_i as f64 + 0.5) / scale)
}

fn spread(mut v: Vec<f64>) -> Option<Spread> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    Some(Spread {
        min: v[0],
        median: median_sorted(&v),
        mean: v.iter().sum::<f64>() / v.len() as f64,
        max: v[v.len() - 1],
    })
}

/// Background pixels that the border cannot reach (4-connected) become foreground: holes
/// inside a nucleus or cell are part of it.
fn fill_holes(mut fg: Vec<bool>, w: usize, h: usize) -> Vec<bool> {
    let mut outside = vec![false; w * h];
    let mut stack: Vec<usize> = Vec::new();
    for x in 0..w {
        stack.push(x);
        stack.push((h - 1) * w + x);
    }
    for y in 0..h {
        stack.push(y * w);
        stack.push(y * w + w - 1);
    }
    while let Some(i) = stack.pop() {
        if fg[i] || outside[i] {
            continue;
        }
        outside[i] = true;
        let (x, y) = (i % w, i / w);
        if x > 0 {
            stack.push(i - 1);
        }
        if x + 1 < w {
            stack.push(i + 1);
        }
        if y > 0 {
            stack.push(i - w);
        }
        if y + 1 < h {
            stack.push(i + w);
        }
    }
    for (f, o) in fg.iter_mut().zip(&outside) {
        *f = !*o;
    }
    fg
}

/// City-block-and-diagonal (3-4 chamfer) distance of each foreground pixel to the background,
/// in thirds of a pixel.
fn chamfer(fg: &[bool], w: usize, h: usize) -> Vec<u16> {
    let big = u16::MAX / 2;
    let mut d: Vec<u16> = fg.iter().map(|f| if *f { big } else { 0 }).collect();
    let at = |d: &[u16], x: isize, y: isize| -> u16 {
        if x < 0 || y < 0 || x >= w as isize || y >= h as isize {
            0
        } else {
            d[y as usize * w + x as usize]
        }
    };
    for y in 0..h as isize {
        for x in 0..w as isize {
            let i = y as usize * w + x as usize;
            if d[i] == 0 {
                continue;
            }
            let m = (at(&d, x - 1, y) + 3)
                .min(at(&d, x, y - 1) + 3)
                .min(at(&d, x - 1, y - 1) + 4)
                .min(at(&d, x + 1, y - 1) + 4);
            d[i] = d[i].min(m);
        }
    }
    for y in (0..h as isize).rev() {
        for x in (0..w as isize).rev() {
            let i = y as usize * w + x as usize;
            if d[i] == 0 {
                continue;
            }
            let m = (at(&d, x + 1, y) + 3)
                .min(at(&d, x, y + 1) + 3)
                .min(at(&d, x + 1, y + 1) + 4)
                .min(at(&d, x - 1, y + 1) + 4);
            d[i] = d[i].min(m);
        }
    }
    d
}

fn find(parent: &mut [u32], mut i: u32) -> u32 {
    while parent[i as usize] != i {
        let p = parent[parent[i as usize] as usize];
        parent[i as usize] = p;
        i = p;
    }
    i
}

/// Label 8-connected foreground objects (1, 2, …; 0 = background) and how many labels there
/// are. With `split`, touching objects are cut apart at narrow necks: pixels are flooded from
/// the inside out in order of their distance to the background, and two growing regions that
/// meet stay apart when the neck is at most three quarters as wide as the smaller one is deep
/// (a distance-transform watershed with that persistence rule).
fn label_objects(fg: &[bool], w: usize, h: usize, split: bool) -> (Vec<u32>, u32) {
    const NONE: u32 = u32::MAX;
    let d = if split {
        chamfer(fg, w, h)
    } else {
        fg.iter().map(|f| u16::from(*f)).collect()
    };
    // pixels by decreasing distance (counting sort)
    let top = d.iter().copied().max().unwrap_or(0) as usize;
    let mut count = vec![0usize; top + 2];
    for &v in &d {
        if v > 0 {
            count[v as usize] += 1;
        }
    }
    let mut start = vec![0usize; top + 2];
    for v in (1..=top).rev() {
        start[v - 1] = start[v] + count[v];
    }
    let total = start[0];
    let mut order = vec![0u32; total];
    let mut fill = start.clone();
    for (i, &v) in d.iter().enumerate() {
        if v > 0 {
            order[fill[v as usize]] = i as u32;
            fill[v as usize] += 1;
        }
    }
    let mut parent = vec![NONE; w * h];
    // depth (distance at the seed) of each region root
    let mut peak = vec![0u16; w * h];
    let significant = |peak: u16, saddle: u16| -> bool {
        // the neck (saddle) at most 3/4 of the depth, and at least 1.5 px shallower
        u32::from(saddle) * 4 <= u32::from(peak) * 3 && peak.saturating_sub(saddle) >= 4
    };
    let mut roots: Vec<u32> = Vec::with_capacity(8);
    for &pi in &order {
        let i = pi as usize;
        let (x, y) = ((i % w) as isize, (i / w) as isize);
        roots.clear();
        for dy in -1isize..=1 {
            for dx in -1isize..=1 {
                let (nx, ny) = (x + dx, y + dy);
                if (dx == 0 && dy == 0) || nx < 0 || ny < 0 || nx >= w as isize || ny >= h as isize
                {
                    continue;
                }
                let j = ny as usize * w + nx as usize;
                if parent[j] != NONE {
                    let r = find(&mut parent, j as u32);
                    if !roots.contains(&r) {
                        roots.push(r);
                    }
                }
            }
        }
        if roots.is_empty() {
            parent[i] = pi;
            peak[i] = d[i];
            continue;
        }
        roots.sort_by_key(|r| std::cmp::Reverse(peak[*r as usize]));
        let main = roots[0];
        parent[i] = main;
        if !split {
            for &r in &roots[1..] {
                parent[r as usize] = main;
            }
            continue;
        }
        // merge regions that are not significant at this saddle into the deepest one
        let saddle = d[i];
        for &r in &roots[1..] {
            if !significant(peak[r as usize], saddle) {
                parent[r as usize] = main;
            }
        }
    }
    let mut label = vec![0u32; w * h];
    let mut ids = std::collections::HashMap::new();
    for i in 0..w * h {
        if parent[i] == NONE {
            continue;
        }
        let r = find(&mut parent, i as u32);
        let next = ids.len() as u32 + 1;
        label[i] = *ids.entry(r).or_insert(next);
    }
    (label, ids.len() as u32)
}

fn objects(
    ds: &mut dyn Dataset,
    tg: &Target<'_>,
    req: &MeasureRequest,
    o: &ObjectsRequest,
    notes: &mut Vec<String>,
) -> Result<Objects> {
    let r = match req.region {
        Some(r) => {
            r.check_within(
                tg.level_w,
                tg.level_h,
                &format!("image {} level {}", tg.im.index, req.level),
            )?;
            r
        }
        None => Region::full(tg.level_w, tg.level_h),
    };
    check_size(r, "an object count")?;
    let c = tg.channels.first().copied().unwrap_or(0);
    if tg.channels.len() > 1 && tg.im.size_c > 1 {
        notes.push(format!(
            "objects were counted in channel {c}; select c=N for another"
        ));
    }
    let g = read(ds, tg, req.level, c, r)?;
    let (w, h) = (g.w, g.h);
    let rgb = g.comps.len() > 1;
    let component = match o.component {
        Some(k) if !rgb => {
            return Err(usage(format!(
                "component={k} is for RGB images; this image has one sample per pixel (choose a channel with select c=N)"
            )));
        }
        Some(k) if k as usize >= g.comps.len() => {
            return Err(usage(format!(
                "component={k} is out of range (the image has {} components: 0 = red, 1 = green, 2 = blue)",
                g.comps.len()
            )));
        }
        k => k,
    };
    let raw: Vec<f64> = match component {
        Some(k) => g.comps[k as usize].clone(),
        None if rgb => {
            let k = g.comps.len().min(3);
            (0..w * h)
                .map(|i| g.comps[..k].iter().map(|c| c[i]).sum::<f64>() / k as f64)
                .collect()
        }
        None => g.comps[0].clone(),
    };
    let polarity = match o.polarity {
        Polarity::Auto if rgb => Polarity::Dark,
        Polarity::Auto => Polarity::Bright,
        p => p,
    };
    let sigma = o.smooth.unwrap_or(1.0).clamp(0.0, 50.0);
    let smooth = blur(&raw, w, h, sigma);
    let (threshold, method) = match o.threshold {
        Some(t) => (t, "given"),
        None => (
            otsu(&smooth).ok_or_else(|| {
                usage("the region is flat (one value): there is nothing to threshold")
            })?,
            "otsu",
        ),
    };
    let fg: Vec<bool> = smooth
        .iter()
        .map(|v| match polarity {
            Polarity::Dark => *v < threshold,
            _ => *v > threshold,
        })
        .collect();
    let min_area = o.min_area_px.unwrap_or(10).max(1);
    let split = o.split.unwrap_or(true);
    let fg = fill_holes(fg, w, h);
    let (label, n_labels) = label_objects(&fg, w, h, split);
    // per label: pixels, sums of x, y and value, bounding box, edge contact
    let n_labels = n_labels as usize;
    let mut n = vec![0u64; n_labels + 1];
    let mut sx = vec![0.0f64; n_labels + 1];
    let mut sy = vec![0.0f64; n_labels + 1];
    let mut sv = vec![0.0f64; n_labels + 1];
    let mut bb = vec![(usize::MAX, usize::MAX, 0usize, 0usize); n_labels + 1];
    let mut edge = vec![false; n_labels + 1];
    for (i, &l) in label.iter().enumerate() {
        if l == 0 {
            continue;
        }
        let l = l as usize;
        let (x, y) = (i % w, i / w);
        n[l] += 1;
        sx[l] += x as f64;
        sy[l] += y as f64;
        sv[l] += raw[i];
        let b = &mut bb[l];
        *b = (b.0.min(x), b.1.min(y), b.2.max(x), b.3.max(y));
        edge[l] |= x == 0 || y == 0 || x == w - 1 || y == h - 1;
    }
    let (ox, oy) = (f64::from(r.x), f64::from(r.y));
    let mut covered = 0u64;
    let mut found: Vec<Object> = Vec::new();
    for l in 1..=n_labels {
        if n[l] < min_area {
            continue;
        }
        covered += n[l];
        let (bx0, by0, bx1, by1) = bb[l];
        found.push(Object {
            x: ox + sx[l] / n[l] as f64 + 0.5,
            y: oy + sy[l] / n[l] as f64 + 0.5,
            area_px: n[l],
            area_um2: tg.pixel_um.map(|p| n[l] as f64 * p * p),
            mean: sv[l] / n[l] as f64,
            bbox: Region::new(
                r.x + bx0 as u32,
                r.y + by0 as u32,
                (bx1 - bx0 + 1) as u32,
                (by1 - by0 + 1) as u32,
            ),
            touches_edge: edge[l] && !(w == 1 || h == 1),
        });
    }
    let mut kept: Vec<Object> = found.into_iter().filter(|o| o.area_px > 0).collect();
    kept.sort_by_key(|o| std::cmp::Reverse(o.area_px));
    let areas: Vec<f64> = kept.iter().map(|o| o.area_px as f64).collect();
    let area_px = spread(areas.clone());
    let med = area_px.as_ref().map_or(0.0, |s| s.median);
    let large = kept.iter().filter(|o| o.area_px as f64 > 2.0 * med).count();
    let count = kept.len();
    let touching = kept.iter().filter(|o| o.touches_edge).count();
    let max_list = o.max_objects.unwrap_or(DEFAULT_MAX_OBJECTS);
    let truncated = count > max_list;
    kept.truncate(max_list);
    let um2 = tg.pixel_um.map(|p| p * p);
    let area_um2 = um2.and_then(|a| spread(areas.iter().map(|x| x * a).collect()));
    let median_diameter_um = area_um2
        .as_ref()
        .map(|s| 2.0 * (s.median / std::f64::consts::PI).sqrt());
    Ok(Objects {
        measured: match component {
            Some(k) => ["red", "green", "blue", "alpha"]
                .get(k as usize)
                .map_or_else(|| k.to_string(), |n| (*n).to_string()),
            None if rgb => "luminance".into(),
            None => c.to_string(),
        },
        region: r,
        threshold,
        threshold_method: method.into(),
        polarity,
        smooth_px: sigma,
        min_area_px: min_area,
        split,
        count,
        touching_edge: touching,
        large,
        area_fraction: covered as f64 / (w * h) as f64,
        area_px,
        area_um2,
        median_diameter_um,
        objects: kept,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn otsu_splits_two_levels() {
        let mut v = vec![10.0; 500];
        v.extend(vec![200.0; 100]);
        let t = otsu(&v).unwrap();
        assert!(t > 10.0 && t < 200.0, "{t}");
    }

    #[test]
    fn fwhm_of_a_triangle() {
        let v = [0.0, 0.0, 1.0, 2.0, 3.0, 4.0, 3.0, 2.0, 1.0, 0.0, 0.0];
        // half height 2 at index 3 and 7
        assert!((fwhm(&v, 5, 0.0).unwrap() - 4.0).abs() < 1e-9);
    }

    #[test]
    fn polygon_inside() {
        let sq = [[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 4.0]];
        assert!(inside(&sq, 2.0, 2.0));
        assert!(!inside(&sq, 5.0, 2.0));
    }

    #[test]
    fn rounding_keeps_six_digits() {
        assert_eq!(round_sig(1234.56789), 1234.57);
        assert_eq!(round_sig(0.000_123_456_78), 0.000_123_457);
        assert_eq!(round_sig(65535.0), 65535.0);
    }

    /// Two discs of radius 20 whose centres are 34 px apart (a narrow neck), and a separate
    /// disc with a hole.
    fn discs() -> (Vec<bool>, usize, usize) {
        let (w, h) = (140usize, 60usize);
        let mut fg = vec![false; w * h];
        for y in 0..h {
            for x in 0..w {
                let d =
                    |cx: f64, cy: f64| ((x as f64 - cx).powi(2) + (y as f64 - cy).powi(2)).sqrt();
                if d(25.0, 30.0) <= 20.0 || d(59.0, 30.0) <= 20.0 {
                    fg[y * w + x] = true;
                }
                let r = d(115.0, 30.0);
                if r <= 15.0 && r > 4.0 {
                    fg[y * w + x] = true;
                }
            }
        }
        (fg, w, h)
    }

    #[test]
    fn touching_discs_are_split_and_holes_filled() {
        let (fg, w, h) = discs();
        let fg = fill_holes(fg, w, h);
        let (labels, n) = label_objects(&fg, w, h, true);
        assert_eq!(n, 3);
        assert_ne!(labels[30 * w + 25], labels[30 * w + 59]);
        assert_ne!(labels[30 * w + 115], 0, "the hole is filled");
        let (_, n) = label_objects(&fg, w, h, false);
        assert_eq!(n, 2);
    }

    #[test]
    fn profile_values_are_bin_means() {
        let v: Vec<f64> = (0..1000).map(f64::from).collect();
        let b = bin_means(&v, 200);
        assert_eq!(b.len(), 200);
        assert!((b[0] - 2.0).abs() < 1e-9);
        assert_eq!(bin_means(&v[..10], 200).len(), 10);
    }
}
