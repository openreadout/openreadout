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
//!   samples; `values` returns at most [`PROFILE_VALUES`] of them, evenly spaced from the start
//!   to the end, each the mean of the samples within half a step of it.
//! - **polygon**: a pixel is inside when its centre is (even-odd rule).
//! - **objects**: see [`objects`] and `segment.rs`.

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

mod objects;
mod segment;

pub use objects::{
    CountConfidence, DEFAULT_MAX_OBJECTS, Kind, LOW_FIT, MAX_OBJECT_PIXELS, Object, ObjectChannel,
    Objects, ObjectsRequest, Polarity, Positive, Stain, ThresholdMethod, label_plane,
};

/// Most samples taken along a profile line.
pub const MAX_PROFILE_SAMPLES: usize = 20_000;
/// Most profile values returned per channel.
pub const PROFILE_VALUES: usize = 200;
/// Most pixels a measurement reads (a 2896 × 2896 region).
pub const MAX_MEASURE_PIXELS: u64 = 1 << 23;

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
    /// The rectangle objects are counted in (default the whole plane, or the polygon).
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
    /// Distance between the returned values in pixels (the first is at the start, the last
    /// at the end).
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
    /// Pixel height in µm when it differs from the width.
    pixel_um_y: Option<f64>,
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
    let pixel_um_y = im
        .physical_size
        .y
        .map(|p| p * f64::from(im.size_y) / f64::from(level_h.max(1)))
        .filter(|py| pixel_um.is_some_and(|px| (py - px).abs() > px * 1e-6));
    Ok(Target {
        im,
        z,
        t,
        channels,
        level_w,
        level_h,
        downsample,
        pixel_um,
        pixel_um_y,
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
        out.objects = Some(objects::objects(ds, &tg, req, o, &mut out.notes)?);
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
    // µm per pixel along this line (pixels may be taller than wide)
    let um_per_px = tg.pixel_um.map(|px| {
        let py = tg.pixel_um_y.unwrap_or(px);
        ((x1 - x0) * px).hypot((y1 - y0) * py) / len
    });
    let k_out = n.min(PROFILE_VALUES);
    let out_step = len / (k_out - 1) as f64;
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
                max_at_um: um_per_px.map(|p| imax as f64 * step * p),
                fwhm_px,
                fwhm_um: fwhm_px.zip(um_per_px).map(|(w, p)| w * p),
                values: resample(&values, k_out)
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
        length_um: um_per_px.map(|p| p * len),
        step_px: out_step,
        step_um: um_per_px.map(|p| p * out_step),
        samples: n,
        channels,
    })
}

/// `k` values evenly spaced from the first sample of `v` to the last, each the mean of the
/// finite samples within half a step of it (or as close to the ends as it is).
fn resample(v: &[f64], k: usize) -> Vec<f64> {
    if v.len() <= k || k < 2 {
        return v.to_vec();
    }
    let last = (v.len() - 1) as f64;
    let step = last / (k - 1) as f64;
    (0..k)
        .map(|i| {
            let c = i as f64 * step;
            // a window centred on the value, narrower at the ends of the line
            let half = (step / 2.0).min(c).min(last - c);
            let lo = (c - half).ceil().max(0.0) as usize;
            let hi = ((c + half).floor() as usize).min(v.len() - 1);
            let f: Vec<f64> = v[lo..=hi.max(lo)]
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
        if !v[i].is_finite() {
            return None;
        }
        if v[i] < half {
            let f = (half - v[i]) / (v[i + 1] - v[i]);
            left = Some(i as f64 + f);
            break;
        }
    }
    let mut right = None;
    for i in imax + 1..v.len() {
        if !v[i].is_finite() {
            return None;
        }
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
        area_um2: tg
            .pixel_um
            .map(|p| area as f64 * p * tg.pixel_um_y.unwrap_or(p)),
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

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn profile_values_are_evenly_spaced() {
        let v: Vec<f64> = (0..1001).map(f64::from).collect();
        let b = resample(&v, 201);
        assert_eq!(b.len(), 201);
        // each value is centred on its position, the ends included
        assert!((b[0] - 0.0).abs() < 1e-9, "{}", b[0]);
        assert!((b[1] - 5.0).abs() < 1e-9, "{}", b[1]);
        assert!((b[200] - 1000.0).abs() < 1e-9, "{}", b[200]);
        assert_eq!(resample(&v[..10], 200).len(), 10);
    }
}
