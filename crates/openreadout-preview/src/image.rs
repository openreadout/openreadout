//! Image previews: plane selection, pyramid level choice, projection, block-average
//! downsampling, contrast and colour mapping, then the rulers frame ([`crate::rulers`]).

use openreadout_core::model::{FileInfo, ImageInfo};
use openreadout_core::pixel::{PixelType, Plane};
use openreadout_core::reader::{Dataset, PlaneIndex};
use openreadout_core::region::Region;
use openreadout_core::select::Selection;
use openreadout_core::{Error, Result};

use crate::canvas::Canvas;
use crate::color::{self, Rgb};
use crate::rulers::{self, Layout, Mapping};
use crate::{
    Axis, Contrast, ImagePreview, Lut, PreviewChannel, PreviewOutput, PreviewRequest, SourcePoint,
    split_select,
};

/// Most channels blended into one composite.
const MAX_COMPOSITE_CHANNELS: usize = 16;
/// Percentiles used by `auto` contrast.
const AUTO_PERCENTILES: (f64, f64) = (0.1, 99.9);

/// One downsampled band of values (NaN where a block had no finite sample).
struct Band {
    w: usize,
    h: usize,
    v: Vec<f32>,
}

/// All samples of a plane as f32, interleaved as stored (`samples_per_pixel` per pixel).
fn samples(p: &Plane) -> Result<Vec<f32>> {
    let n = p.width as usize * p.height as usize * p.samples_per_pixel as usize;
    let bps = p.pixel_type.bytes_per_sample();
    if p.data.len() < n * bps {
        return Err(Error::Other(format!(
            "plane holds {} bytes, expected {}",
            p.data.len(),
            n * bps
        )));
    }
    let d = &p.data[..n * bps];
    Ok(match p.pixel_type {
        PixelType::Uint8 => d.iter().map(|&b| f32::from(b)).collect(),
        PixelType::Int8 => d.iter().map(|&b| f32::from(b as i8)).collect(),
        PixelType::Uint16 => d
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| f32::from(u16::from_le_bytes([c[0], c[1]])))
            .collect(),
        PixelType::Int16 => d
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| f32::from(i16::from_le_bytes([c[0], c[1]])))
            .collect(),
        PixelType::Uint32 => d
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]) as f32)
            .collect(),
        PixelType::Int32 => d
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]]) as f32)
            .collect(),
        PixelType::Float => d
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect(),
        PixelType::Double => d
            .as_chunks::<8>()
            .0
            .iter()
            .map(|c| f64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]) as f32)
            .collect(),
        // 64-bit integers, and complex samples drawn by their modulus
        other => d
            .chunks_exact(bps)
            .map(|c| other.sample_f64(c) as f32)
            .collect(),
    })
}

/// Average `f`×`f` blocks of sample `s` (of `spp`) into a band; non-finite samples are skipped.
fn block_average(src: &[f32], w: usize, h: usize, spp: usize, s: usize, f: usize) -> Band {
    let (ow, oh) = (w.div_ceil(f), h.div_ceil(f));
    if f == 1 {
        return Band {
            w,
            h,
            v: (0..w * h).map(|i| src[i * spp + s]).collect(),
        };
    }
    let mut sum = vec![0f64; ow * oh];
    let mut cnt = vec![0u32; ow * oh];
    for y in 0..h {
        let row = (y / f) * ow;
        for x in 0..w {
            let v = src[(y * w + x) * spp + s];
            if v.is_finite() {
                let o = row + x / f;
                sum[o] += f64::from(v);
                cnt[o] += 1;
            }
        }
    }
    Band {
        w: ow,
        h: oh,
        v: sum
            .iter()
            .zip(&cnt)
            .map(|(&s, &n)| {
                if n > 0 {
                    (s / f64::from(n)) as f32
                } else {
                    f32::NAN
                }
            })
            .collect(),
    }
}

/// `(level, width, height)` of every pyramid level the reader describes (level 0 first).
/// Levels with fewer z planes than the image (Imaris downsamples z too) are left out: the
/// picture's z index counts level-0 planes.
fn pyramid(im: &ImageInfo) -> Vec<(u32, u32, u32)> {
    if !im.resolution_levels.is_empty() {
        return im
            .resolution_levels
            .iter()
            .filter(|l| l.level < im.pyramid_levels.max(1))
            .filter(|l| l.size_z.is_none_or(|z| z == im.size_z))
            .map(|l| (l.level, l.size_x, l.size_y))
            .collect();
    }
    let mut out = vec![(0, im.size_x, im.size_y)];
    if let Some(levels) = im.extra.get("pyramid").and_then(|v| v.as_array()) {
        for l in levels {
            let get = |k: &str| l.get(k).and_then(serde_json::Value::as_u64);
            if let (Some(level), Some(w), Some(h)) = (get("level"), get("size_x"), get("size_y"))
                && level > 0
                && level < u64::from(im.pyramid_levels.max(1))
            {
                out.push((level as u32, w as u32, h as u32));
            }
        }
    }
    out
}

/// The most downsampled level whose longer side is still ≥ `max_size` (level 0 if none is).
fn choose_level(im: &ImageInfo, max_size: u32) -> u32 {
    pyramid(im)
        .into_iter()
        .filter(|&(_, w, h)| w.max(h) >= max_size)
        .min_by_key(|&(l, w, h)| (w.max(h), l))
        .map_or(0, |(l, _, _)| l)
}

/// The most downsampled level at which the full-resolution `region` still spans ≥ `max_size`
/// pixels on its longer side (level 0 if none does), and the region mapped onto it.
fn choose_region_level(im: &ImageInfo, region: Region, max_size: u32) -> (u32, Region) {
    let base = (im.size_x, im.size_y);
    pyramid(im)
        .into_iter()
        .map(|(l, w, h)| (l, region.rescale(base, (w, h))))
        .filter(|(l, r)| *l == 0 || r.width.max(r.height) >= max_size)
        .min_by_key(|(l, r)| (r.width.max(r.height), *l))
        .unwrap_or((0, region))
}

/// Size of pyramid level `level` of `im`, when the reader describes it.
fn level_dims(im: &ImageInfo, level: u32) -> Option<(u32, u32)> {
    pyramid(im)
        .into_iter()
        .find(|&(l, _, _)| l == level)
        .map(|(_, w, h)| (w, h))
}

fn resolve_contrast(c: Contrast, pt: PixelType, rgb: bool) -> Contrast {
    match c {
        Contrast::Auto if rgb && pt == PixelType::Uint8 => Contrast::Raw,
        Contrast::Auto => Contrast::Percentile(AUTO_PERCENTILES.0, AUTO_PERCENTILES.1),
        other => other,
    }
}

fn raw_range(pt: PixelType) -> (f64, f64) {
    match pt {
        PixelType::Uint8 => (0.0, 255.0),
        PixelType::Int8 => (-128.0, 127.0),
        PixelType::Uint16 => (0.0, 65_535.0),
        PixelType::Int16 => (-32_768.0, 32_767.0),
        PixelType::Uint32 => (0.0, f64::from(u32::MAX)),
        PixelType::Int32 => (f64::from(i32::MIN), f64::from(i32::MAX)),
        // Floats, and any pixel type added to the core model after this renderer.
        _ => (0.0, 1.0),
    }
}

/// Linear-interpolated percentile of sorted values.
fn percentile(sorted: &[f64], p: f64) -> f64 {
    let pos = p / 100.0 * (sorted.len() - 1) as f64;
    let lo = pos.floor() as usize;
    let hi = (lo + 1).min(sorted.len() - 1);
    sorted[lo] + (sorted[hi] - sorted[lo]) * (pos - lo as f64)
}

/// What [`screen_samples`] leaves out of a channel's auto contrast (the samples stay drawn).
#[derive(Debug, Default, Clone, Copy, PartialEq)]
struct Screened {
    /// Values above this (the recorded bit depth's ceiling) do not count.
    ceiling: Option<f64>,
    /// Values equal to this (the pixel type's maximum) do not count.
    skip: Option<f64>,
}

/// Most samples above the recorded bit depth that are still read as blended fill (left out of
/// the auto contrast) rather than as a sign the recorded depth is wrong, in percent.
const MAX_OVER_RANGE_PERCENT: usize = 1;

/// The largest value of an unsigned integer pixel type: what readers and acquisition software
/// write where nothing was acquired, and what a detector clips to.
fn type_max(pt: PixelType) -> Option<f32> {
    match pt {
        PixelType::Uint8 => Some(f32::from(u8::MAX)),
        PixelType::Uint16 => Some(f32::from(u16::MAX)),
        PixelType::Uint32 => Some(u32::MAX as f32),
        _ => None,
    }
}

/// What [`screen_samples`] found in one channel, for the report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum FindingKind {
    /// Samples above the recorded depth, mostly the type's maximum: fill, now no data.
    Fill,
    /// A few samples above the recorded depth: left out of the auto contrast.
    Blended,
    /// Many samples at the type's maximum, no recorded depth: left out of the auto contrast.
    AtMaximum,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Finding {
    kind: FindingKind,
    /// Percent of the channel's samples concerned.
    share: f64,
    /// `Fill`: percent of those at the type's maximum.
    at_top: f64,
}

fn share(n: usize, of: usize) -> f64 {
    100.0 * n as f64 / of.max(1) as f64
}

/// Screen the samples of one channel for values that are not measurements before they are
/// drawn. Tiled scans store a fill where nothing was acquired: readers composite uncovered
/// canvas as 0, but the pyramid tiles ZEN writes for slide scans (Axioscan RAC files) carry
/// 65535 past the scanned tiles and, at their edges, 65535 averaged with data. When the file
/// records a bit depth below the pixel type's (`images[].extra.component_bit_count` 12):
/// - if most samples above its ceiling are exactly the type's maximum, all samples above the
///   ceiling are fill: they become no data (NaN), drawn as background like the uncovered
///   canvas of level 0, and do not count for the contrast;
/// - else, if at most 1% of the samples lie above it (edge pixels blended with fill), auto
///   contrast leaves them out but they are drawn;
/// - else the recorded depth does not hold (as `stats` decides) and nothing is screened.
///
/// Without a recorded depth, auto contrast leaves out the type's maximum when it holds more
/// samples than the upper percentile allows (it would otherwise set the white point).
#[allow(clippy::float_cmp)] // integer samples are exact in f32; the fill is exactly the maximum
fn screen_samples(acc: &mut [f32], im: &ImageInfo, auto: bool) -> (Screened, Option<Finding>) {
    let none = (Screened::default(), None);
    let Some(top) = type_max(im.pixel_type) else {
        return none;
    };
    let finite = acc.iter().filter(|v| v.is_finite()).count();
    if finite == 0 {
        return none;
    }
    if let Some((ceiling, _)) = openreadout_core::stats::recorded_saturation(im) {
        let ceil = ceiling as f32;
        let above = acc.iter().filter(|&&v| v > ceil).count();
        if above > 0 {
            let at_top = acc.iter().filter(|&&v| v == top).count();
            if at_top * 2 >= above {
                for v in acc.iter_mut().filter(|v| **v > ceil) {
                    *v = f32::NAN;
                }
                let f = Finding {
                    kind: FindingKind::Fill,
                    share: share(above, finite),
                    at_top: share(at_top, above),
                };
                return (Screened::default(), Some(f));
            }
            if above * 100 <= finite * MAX_OVER_RANGE_PERCENT {
                if !auto {
                    return none;
                }
                let f = Finding {
                    kind: FindingKind::Blended,
                    share: share(above, finite),
                    at_top: 0.0,
                };
                let s = Screened {
                    ceiling: Some(ceiling),
                    skip: None,
                };
                return (s, Some(f));
            }
        }
    }
    if auto {
        let at_top = acc.iter().filter(|&&v| v == top).count();
        let allowed = (100.0 - AUTO_PERCENTILES.1) / 100.0 * finite as f64;
        if at_top < finite && at_top as f64 > allowed {
            let f = Finding {
                kind: FindingKind::AtMaximum,
                share: share(at_top, finite),
                at_top: 100.0,
            };
            let s = Screened {
                ceiling: None,
                skip: Some(f64::from(top)),
            };
            return (s, Some(f));
        }
    }
    none
}

/// `12.30%`, or `12.30-13.10%` over several channels.
fn percent_range(v: &[f64]) -> String {
    let lo = v.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = v.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let (a, b) = (format!("{lo:.2}"), format!("{hi:.2}"));
    if a == b {
        format!("{a}%")
    } else {
        format!("{a}-{b}%")
    }
}

/// One note per kind of finding, naming the channels it concerns.
fn finding_notes(im: &ImageInfo, found: &[(u32, Finding)]) -> Vec<String> {
    let top = type_max(im.pixel_type).unwrap_or(f32::MAX);
    let (bits, field) = openreadout_core::stats::recorded_saturation(im).map_or_else(
        || (String::new(), ""),
        |(ceiling, field)| (format!("{}-bit", (ceiling + 1.0).log2().round()), field),
    );
    let mut kinds: Vec<FindingKind> = found.iter().map(|(_, f)| f.kind).collect();
    kinds.sort_unstable();
    kinds.dedup();
    kinds
        .into_iter()
        .map(|kind| {
            let of: Vec<(u32, Finding)> = found.iter().copied().filter(|(_, f)| f.kind == kind).collect();
            let chans = of
                .iter()
                .map(|(c, _)| format!("c={c}"))
                .collect::<Vec<_>>()
                .join(", ");
            let shares = percent_range(&of.iter().map(|(_, f)| f.share).collect::<Vec<_>>());
            match kind {
                FindingKind::Fill => format!(
                    "{chans}: {shares} of the samples lie above the {bits} range the file records ({field}), {} of those at {top}: the fill written where nothing was acquired (pyramid tiles past the scanned tiles); drawn as background, like uncovered canvas, and left out of the contrast",
                    percent_range(&of.iter().map(|(_, f)| f.at_top).collect::<Vec<_>>()),
                ),
                FindingKind::Blended => format!(
                    "{chans}: {shares} of the samples lie above the {bits} range the file records ({field}) (tile edges blended with fill): drawn, but left out of the contrast"
                ),
                FindingKind::AtMaximum => format!(
                    "{chans}: {shares} of the samples are {top}, the pixel type's maximum (fill where nothing was acquired, or clipping): the contrast is computed without them"
                ),
            }
        })
        .collect()
}

/// Display range `(black, white)` of the bands under a contrast rule; values `screened` out
/// do not count (unless nothing else is left).
#[allow(clippy::float_cmp)] // `skip` is an exact integer value
fn display_range(bands: &[&Band], c: Contrast, pt: PixelType, screened: Screened) -> (f64, f64) {
    let (lo, hi) = match c {
        Contrast::Raw => raw_range(pt),
        Contrast::Auto | Contrast::MinMax | Contrast::Percentile(..) => {
            let finite = || {
                bands
                    .iter()
                    .flat_map(|b| b.v.iter())
                    .filter(|v| v.is_finite())
                    .map(|&v| f64::from(v))
            };
            let counts = |v: &f64| {
                screened.ceiling.is_none_or(|m| *v <= m) && screened.skip.is_none_or(|s| *v != s)
            };
            let mut vals: Vec<f64> = finite().filter(counts).collect();
            if vals.is_empty() {
                vals = finite().collect();
            }
            if vals.is_empty() {
                return (0.0, 1.0);
            }
            vals.sort_by(f64::total_cmp);
            match c {
                Contrast::Percentile(a, b) => (percentile(&vals, a), percentile(&vals, b)),
                _ => (vals[0], vals[vals.len() - 1]),
            }
        }
    };
    if hi > lo { (lo, hi) } else { (lo, lo + 1.0) }
}

fn level_of(v: f32, lo: f64, hi: f64) -> f32 {
    if !v.is_finite() {
        return 0.0;
    }
    ((f64::from(v) - lo) / (hi - lo)).clamp(0.0, 1.0) as f32
}

/// Pick the single index of an axis, or all of them for a projection axis.
fn axis_indices(
    name: &str,
    sel: &[u32],
    size: u32,
    default: u32,
    project: bool,
) -> Result<Vec<u32>> {
    for &i in sel {
        if i >= size {
            return Err(Error::Usage(format!(
                "{name}={i} out of range (image has {size}, 0..{size})"
            )));
        }
    }
    if project {
        return Ok(if sel.is_empty() {
            (0..size).collect()
        } else {
            sel.to_vec()
        });
    }
    match sel {
        [] => Ok(vec![default]),
        [one] => Ok(vec![*one]),
        _ => Err(Error::Usage(format!(
            "a preview shows one {name} at a time; pass --mip {name} to project several"
        ))),
    }
}

/// Decoded bytes of one plane (or `region` of it) at `level`.
fn plane_bytes(im: &ImageInfo, level: u32, region: Option<Region>) -> u64 {
    let (w, h) = match region {
        Some(r) => (r.width, r.height),
        None if level == 0 => (im.size_x, im.size_y),
        None => level_dims(im, level).unwrap_or((im.size_x, im.size_y)),
    };
    u64::from(w)
        .saturating_mul(u64::from(h))
        .saturating_mul(u64::from(im.samples_per_pixel.max(1)))
        .saturating_mul(im.pixel_type.bytes_per_sample() as u64)
}

/// A read budget: the image and the most decoded bytes one plane read may take.
#[derive(Clone, Copy)]
struct Budget<'a> {
    im: &'a ImageInfo,
    per_plane: u64,
    planes: u64,
}

impl Budget<'_> {
    fn check(&self, level: u32, region: Option<Region>) -> Result<()> {
        let need = plane_bytes(self.im, level, region);
        if need <= self.per_plane {
            return Ok(());
        }
        let mib = |b: u64| b.div_ceil(1 << 20);
        Err(Error::unsupported(
            "preview",
            format!(
                "a quick look decoding {} MiB of pixels ({} plane(s) at level {level}; budget {} MiB)",
                mib(need.saturating_mul(self.planes)),
                self.planes,
                mib(self.per_plane.saturating_mul(self.planes))
            ),
            "Preview a region (full-resolution pixels) or a smaller pyramid level to look at part of the image.",
        ))
    }
}

/// Read one plane (or its `region`, in the coordinates of `level`) at `level`, falling back to
/// level 0 when the level was chosen automatically and the reader does not serve it (the region
/// then becomes `region0`, its full-resolution form).
#[allow(clippy::too_many_arguments)]
fn read_plane(
    ds: &mut dyn Dataset,
    image: u32,
    idx: PlaneIndex,
    level: &mut u32,
    region: &mut Option<Region>,
    region0: Option<Region>,
    auto: bool,
    budget: Option<Budget<'_>>,
    notes: &mut Vec<String>,
) -> Result<Plane> {
    if let Some(b) = budget {
        b.check(*level, *region)?;
    }
    let got = match *region {
        Some(r) => ds.read_region(image, idx, *level, r),
        None => ds.read_plane_level(image, idx, *level),
    };
    match got {
        Err(Error::Unsupported { .. }) if auto && *level > 0 => {
            notes.push(format!(
                "pyramid level {level} is listed but not readable by this reader; used level 0",
                level = *level
            ));
            if let Some(b) = budget {
                b.check(0, region0)?;
            }
            *level = 0;
            *region = region0;
            match region0 {
                Some(r) => ds.read_region(image, idx, 0, r),
                None => ds.read_plane(image, idx),
            }
        }
        other => other,
    }
}

pub(crate) fn render(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    req: &PreviewRequest,
    out: &mut PreviewOutput,
) -> Result<Canvas> {
    let image = req.image.unwrap_or(0);
    let im = info
        .images
        .iter()
        .find(|i| i.index == image)
        .ok_or_else(|| {
            Error::Usage(format!(
                "image {image} not found (file has {} images)",
                info.images.len()
            ))
        })?
        .clone();
    if im.size_c == 0 || im.size_z == 0 || im.size_t == 0 || im.size_x == 0 || im.size_y == 0 {
        return Err(Error::Usage(format!("image {image} has no pixels")));
    }
    let sel = Selection::parse(&split_select(&req.select))?;
    let rgb = im.samples_per_pixel == 3;
    let mut composite = req.composite;
    if sel.c.len() > 1 && !composite {
        composite = true;
        out.notes
            .push("several channels selected: rendered as a composite".into());
    }
    if rgb && composite {
        return Err(Error::Usage(
            "composites blend single-sample channels; this image stores RGB planes, preview one channel (c=N) at a time".into(),
        ));
    }
    let lut = req.lut.unwrap_or(if composite {
        Lut::ChannelColor
    } else {
        Lut::Gray
    });
    if composite && lut == Lut::Gray {
        return Err(Error::Usage(
            "--lut gray cannot show a composite; drop --lut or preview one channel".into(),
        ));
    }
    let c_list: Vec<u32> = if composite {
        for &c in &sel.c {
            if c >= im.size_c {
                return Err(Error::Usage(format!(
                    "c={c} out of range (image has {}, 0..{})",
                    im.size_c, im.size_c
                )));
            }
        }
        let mut all: Vec<u32> = if sel.c.is_empty() {
            (0..im.size_c).collect()
        } else {
            sel.c.clone()
        };
        if all.len() > MAX_COMPOSITE_CHANNELS {
            out.notes.push(format!(
                "composite limited to the first {MAX_COMPOSITE_CHANNELS} of {} channels",
                all.len()
            ));
            all.truncate(MAX_COMPOSITE_CHANNELS);
        }
        all
    } else {
        axis_indices("c", &sel.c, im.size_c, 0, false)?
    };
    let z_list = axis_indices(
        "z",
        &sel.z,
        im.size_z,
        im.size_z / 2,
        req.mip == Some(Axis::Z),
    )?;
    let t_list = axis_indices("t", &sel.t, im.size_t, 0, req.mip == Some(Axis::T))?;

    // Rulers: `max_size` bounds the whole picture, so the data gets what the margins leave.
    let mut layout = req
        .axes
        .then(|| Layout::new(req.max_size, im.size_x, im.size_y));
    if let Some(l) = layout {
        let room = req.max_size.saturating_sub(l.h().max(l.v()));
        if room < 16 || room < req.max_size / 2 {
            out.notes.push(format!(
                "max size {} is too small for rulers; drawn without them",
                req.max_size
            ));
            layout = None;
        }
    }
    if req.grid && layout.is_none() {
        out.notes
            .push("grid lines are drawn only with the rulers (axes)".into());
    }
    // The level is chosen for `max_size` as without rulers (a level just under it would be
    // halved by the integer block average); the block size then fits what the margins leave.
    let fit = layout.map_or(req.max_size, |l| req.max_size - l.h().max(l.v()));
    let planes = (c_list.len() * z_list.len() * t_list.len()) as u64;
    let budget = req.max_read_bytes.map(|b| Budget {
        im: &im,
        per_plane: b / planes.max(1),
        planes,
    });

    let auto_level = req.level.is_none();
    // A region is in level-0 coordinates unless a level is given (then in that level's).
    let (mut level, mut region) = match (req.level, req.region) {
        (Some(l), r) => (l, r),
        (None, Some(r)) => {
            r.check_within(im.size_x, im.size_y, &format!("image {image} (level 0)"))?;
            let (l, rl) = choose_region_level(&im, r, req.max_size);
            (l, Some(rl))
        }
        (None, None) => (choose_level(&im, req.max_size), None),
    };
    let region0 = if auto_level { req.region } else { None };
    let spp = im.samples_per_pixel.max(1) as usize;
    let mut first_dims: Option<(u32, u32)> = None;
    let mut factor = 1usize;
    let contrast = resolve_contrast(req.contrast, im.pixel_type, rgb);
    // Auto contrast (not what it resolved to for RGB) may leave fill and clipping out.
    let auto_contrast = req.contrast == Contrast::Auto && contrast != Contrast::Raw;
    let mut per_channel: Vec<(u32, Vec<Band>, Screened)> = Vec::new();
    let mut found: Vec<(u32, Finding)> = Vec::new();
    for &c in &c_list {
        let mut acc: Option<Vec<f32>> = None;
        let mut dims = (0u32, 0u32);
        for &z in &z_list {
            for &t in &t_list {
                let p = read_plane(
                    ds,
                    image,
                    PlaneIndex { c, z, t },
                    &mut level,
                    &mut region,
                    region0,
                    auto_level,
                    budget,
                    &mut out.notes,
                )?;
                let s = samples(&p)?;
                dims = (p.width, p.height);
                if p.samples_per_pixel as usize != spp {
                    return Err(Error::Other(format!(
                        "plane has {} samples per pixel, image declares {spp}",
                        p.samples_per_pixel
                    )));
                }
                acc = Some(match acc {
                    None => s,
                    Some(mut a) => {
                        if a.len() != s.len() {
                            return Err(Error::Other(
                                "planes of one image differ in size; cannot project".into(),
                            ));
                        }
                        for (x, y) in a.iter_mut().zip(s) {
                            // NaN-aware maximum (f32::max ignores a NaN operand)
                            *x = x.max(y);
                        }
                        a
                    }
                });
            }
        }
        let mut acc = acc.unwrap_or_default();
        let (screened, finding) = screen_samples(&mut acc, &im, auto_contrast);
        found.extend(finding.map(|f| (c, f)));
        match first_dims {
            None => {
                first_dims = Some(dims);
                factor = (dims.0.max(dims.1).div_ceil(fit)).max(1) as usize;
            }
            Some(d) if d != dims => {
                return Err(Error::Other(format!(
                    "channels of image {image} differ in size ({}x{} vs {}x{})",
                    d.0, d.1, dims.0, dims.1
                )));
            }
            Some(_) => {}
        }
        let (w, h) = (dims.0 as usize, dims.1 as usize);
        let bands = (0..spp)
            .map(|s| block_average(&acc, w, h, spp, s, factor))
            .collect();
        per_channel.push((c, bands, screened));
    }
    out.notes.extend(finding_notes(&im, &found));
    let (sw, sh) = match region {
        // The plane size at the level read (the region is part of it).
        Some(_) => level_dims(&im, level).unwrap_or((0, 0)),
        None => first_dims.unwrap_or((0, 0)),
    };
    let (ow, oh) = per_channel
        .first()
        .and_then(|(_, b, _)| b.first())
        .map_or((0, 0), |b| (b.w, b.h));
    let mut canvas = Canvas::new(ow as u32, oh as u32, [0, 0, 0]);
    let mut channels = Vec::new();
    let mut acc = vec![0f32; ow * oh * 3];
    for (pos, (c, bands, screened)) in per_channel.iter().enumerate() {
        let ch = im.channels.iter().find(|x| x.index == *c);
        let refs: Vec<&Band> = bands.iter().collect();
        let (lo, hi) = display_range(&refs, contrast, im.pixel_type, *screened);
        let (col, source): (Rgb, &str) = if rgb {
            ([255, 255, 255], "rgb")
        } else if lut == Lut::Gray {
            ([255, 255, 255], "lut")
        } else {
            let (c, s) = color::channel_color(ch, pos);
            (c, s.as_str())
        };
        if rgb {
            for i in 0..ow * oh {
                for (k, band) in bands.iter().take(3).enumerate() {
                    acc[i * 3 + k] = 255.0 * level_of(band.v[i], lo, hi);
                }
            }
        } else {
            let band = &bands[0];
            for i in 0..ow * oh {
                let l = level_of(band.v[i], lo, hi);
                for k in 0..3 {
                    acc[i * 3 + k] += f32::from(col[k]) * l;
                }
            }
        }
        channels.push(PreviewChannel {
            index: *c,
            name: ch.and_then(|x| x.name.clone()),
            color: color::to_hex(col),
            color_source: source.into(),
            display_min: lo,
            display_max: hi,
        });
    }
    for (o, a) in canvas.rgb.iter_mut().zip(&acc) {
        *o = a.round().clamp(0.0, 255.0) as u8;
    }
    // Map drawn pixels back to full-resolution pixels: level scale × block size.
    let level_size = if level == 0 {
        Some((im.size_x, im.size_y))
    } else {
        level_dims(&im, level).or(if region.is_none() { first_dims } else { None })
    };
    let (kx, ky) = match level_size {
        Some((w, h)) if w > 0 && h > 0 => (
            f64::from(im.size_x) / f64::from(w),
            f64::from(im.size_y) / f64::from(h),
        ),
        _ => {
            out.notes.push(format!(
                "the size of level {level} is not known: rulers and source_* fields count level-{level} pixels"
            ));
            (1.0, 1.0)
        }
    };
    let map = Mapping {
        origin: region.map_or((0.0, 0.0), |r| (f64::from(r.x) * kx, f64::from(r.y) * ky)),
        per_pixel: (factor as f64 * kx, factor as f64 * ky),
    };
    let read_end = match (region, level_size) {
        (Some(r), Some(ls)) => {
            let r0 = r.rescale(ls, (im.size_x, im.size_y));
            (r0.x + r0.width, r0.y + r0.height)
        }
        _ => (im.size_x, im.size_y),
    };
    let full_res_region = map.covered(ow as u32, oh as u32, read_end);
    let (canvas, plot_area, scale_bar) = match layout {
        Some(l) => {
            let d = rulers::decorate(&canvas, l, map, im.physical_size.x, req.grid);
            (d.canvas, d.plot_area, d.scale_bar)
        }
        None => (canvas, Region::new(0, 0, ow as u32, oh as u32), None),
    };
    out.image = Some(ImagePreview {
        image,
        level,
        source_width: sw,
        source_height: sh,
        region,
        downsample: factor as u32,
        c: c_list,
        z: z_list,
        t: t_list,
        projection: req.mip.map(|a| {
            match a {
                Axis::Z => "max-z",
                Axis::T => "max-t",
            }
            .to_string()
        }),
        composite,
        rgb,
        contrast: contrast.to_string(),
        lut: if rgb {
            "rgb".into()
        } else if lut == Lut::Gray {
            "gray".into()
        } else {
            "channel-color".into()
        },
        channels,
        axes: layout.is_some(),
        grid: layout.is_some() && req.grid,
        plot_area,
        source_origin: SourcePoint {
            x: map.origin.0,
            y: map.origin.1,
        },
        source_per_pixel: map.per_pixel.0,
        source_per_pixel_y: map.per_pixel.1,
        full_res_region,
        scale_bar,
    });
    Ok(canvas)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_average_handles_edges_and_nan() {
        // 3x2 image, f=2 → 2x1: [(1+2+4+5)/4, (3+6)/2]
        let src = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        let b = block_average(&src, 3, 2, 1, 0, 2);
        assert_eq!((b.w, b.h), (2, 1));
        assert_eq!(b.v, vec![3.0, 4.5]);
        let b = block_average(&[f32::NAN, 2.0], 2, 1, 1, 0, 2);
        assert_eq!(b.v, vec![2.0]);
    }

    #[test]
    fn percentiles_and_ranges() {
        let s: Vec<f64> = (0..=100).map(f64::from).collect();
        assert!((percentile(&s, 1.0) - 1.0).abs() < 1e-12);
        assert!((percentile(&s, 99.5) - 99.5).abs() < 1e-12);
        let b = Band {
            w: 2,
            h: 1,
            v: vec![5.0, 5.0],
        };
        assert_eq!(
            display_range(
                &[&b],
                Contrast::MinMax,
                PixelType::Uint16,
                Screened::default()
            ),
            (5.0, 6.0)
        );
        assert_eq!(
            display_range(&[&b], Contrast::Raw, PixelType::Uint8, Screened::default()),
            (0.0, 255.0)
        );
    }

    fn twelve_bit() -> ImageInfo {
        let mut im = ImageInfo::new(0, 10, 10, PixelType::Uint16);
        im.extra
            .insert("component_bit_count".into(), serde_json::json!(12));
        im
    }

    #[test]
    fn fill_above_the_recorded_depth_becomes_no_data() {
        // 60 data samples, 30 at the fill 65535, 2 blends of fill and data, 8 uncovered zeros.
        let mut acc: Vec<f32> = (0..60).map(|i| 100.0 + i as f32).collect();
        acc.extend([65535.0; 30]);
        acc.extend([30_000.0, 9_000.0]);
        acc.extend([0.0; 8]);
        let (s, f) = screen_samples(&mut acc, &twelve_bit(), true);
        assert_eq!(s, Screened::default());
        let f = f.unwrap();
        assert_eq!(f.kind, FindingKind::Fill);
        assert!((f.share - 32.0).abs() < 1e-9 && (f.at_top - 93.75).abs() < 1e-9);
        assert_eq!(acc.iter().filter(|v| v.is_nan()).count(), 32);
        assert!(acc.iter().all(|v| v.is_nan() || *v <= 4095.0));
        // Also without auto contrast: fill is not data whatever the contrast rule.
        let mut acc2 = vec![100.0, 65535.0];
        let (_, f) = screen_samples(&mut acc2, &twelve_bit(), false);
        assert!(acc2[1].is_nan() && f.is_some());
    }

    #[test]
    fn blended_edges_leave_the_contrast_only() {
        // 1000 samples, 5 blends above 4095 and none at 65535: drawn, not counted.
        let mut acc: Vec<f32> = (0..995).map(|i| 1000.0 + (i % 100) as f32).collect();
        acc.extend([20_000.0, 18_000.0, 9_000.0, 5_000.0, 30_000.0]);
        let before = acc.clone();
        let (s, f) = screen_samples(&mut acc, &twelve_bit(), true);
        assert_eq!(acc, before, "blends stay drawn");
        assert_eq!(s.ceiling, Some(4095.0));
        assert_eq!(f.map(|f| f.kind), Some(FindingKind::Blended));
        let band = Band {
            w: 1000,
            h: 1,
            v: acc,
        };
        let p = Contrast::Percentile(0.1, 99.9);
        let (_, hi) = display_range(&[&band], p, PixelType::Uint16, s);
        assert!(
            hi <= 1099.0,
            "white point {hi} from the data, not the blends"
        );
        let (_, hi) = display_range(&[&band], p, PixelType::Uint16, Screened::default());
        assert!(hi > 4095.0, "unscreened, the blends set the white point");
        // Not in auto contrast: nothing is left out.
        let (s, f) = screen_samples(&mut before.clone(), &twelve_bit(), false);
        assert_eq!((s, f), (Screened::default(), None));
    }

    #[test]
    fn a_wrong_recorded_depth_screens_nothing() {
        // Most samples above 4095, spread out (14-bit data recorded as 12-bit).
        let mut acc: Vec<f32> = (0..100).map(|i| (i * 160) as f32).collect();
        let before = acc.clone();
        let (s, f) = screen_samples(&mut acc, &twelve_bit(), true);
        assert_eq!((acc, s, f), (before, Screened::default(), None));
    }

    #[test]
    fn dominant_type_maximum_is_left_out_of_auto_contrast() {
        // No recorded depth; 20% of the samples at 65535.
        let im = ImageInfo::new(0, 10, 10, PixelType::Uint16);
        let mut acc: Vec<f32> = (0..80).map(|i| 200.0 + i as f32).collect();
        acc.extend([65535.0; 20]);
        let (s, f) = screen_samples(&mut acc, &im, true);
        assert_eq!(s.skip, Some(65535.0));
        assert_eq!(f.map(|f| f.kind), Some(FindingKind::AtMaximum));
        let band = Band {
            w: 100,
            h: 1,
            v: acc.clone(),
        };
        let p = Contrast::Percentile(0.1, 99.9);
        let (lo, hi) = display_range(&[&band], p, PixelType::Uint16, s);
        assert!(lo >= 200.0 && hi < 280.0, "{lo}..{hi}");
        // A handful at the maximum (under 0.1%) or only the maximum: nothing changes.
        let mut few: Vec<f32> = (0..10_000).map(|i| (i % 500) as f32).collect();
        few[0] = 65535.0;
        assert_eq!(screen_samples(&mut few, &im, true).1, None);
        let mut all = vec![65535.0f32; 10];
        assert_eq!(screen_samples(&mut all, &im, true).1, None);
        // Only everything left out: the range falls back to all samples.
        let only = Band {
            w: 2,
            h: 1,
            v: vec![65535.0, 65535.0],
        };
        assert_eq!(
            display_range(&[&only], p, PixelType::Uint16, s),
            (65535.0, 65536.0)
        );
        // Floats have no fill value.
        let fim = ImageInfo::new(0, 10, 10, PixelType::Float);
        assert_eq!(screen_samples(&mut acc, &fim, true).1, None);
    }

    #[test]
    fn findings_make_one_note_per_kind() {
        let fill = |share, at_top| Finding {
            kind: FindingKind::Fill,
            share,
            at_top,
        };
        let blended = Finding {
            kind: FindingKind::Blended,
            share: 0.29,
            at_top: 0.0,
        };
        let found = [
            (0, fill(13.27, 97.23)),
            (1, blended),
            (3, fill(13.29, 97.07)),
        ];
        let notes = finding_notes(&twelve_bit(), &found);
        assert_eq!(notes.len(), 2, "{notes:?}");
        assert!(
            notes[0].starts_with("c=0, c=3: 13.27-13.29% of the samples lie above the 12-bit range the file records (images[].extra.component_bit_count), 97.07-97.23% of those at 65535"),
            "{}",
            notes[0]
        );
        assert!(notes[1].starts_with("c=1: 0.29% "), "{}", notes[1]);
    }

    #[test]
    fn level_choice_prefers_smallest_sufficient() {
        let mut im = ImageInfo::new(0, 8000, 6000, PixelType::Uint8);
        im.pyramid_levels = 3;
        im.extra.insert(
            "pyramid".into(),
            serde_json::json!([
                {"level": 1, "size_x": 2000, "size_y": 1500},
                {"level": 2, "size_x": 500, "size_y": 375}
            ]),
        );
        assert_eq!(choose_level(&im, 1024), 1);
        assert_eq!(choose_level(&im, 256), 2);
        assert_eq!(choose_level(&im, 9000), 0);
        // A 2000 px wide region renders from level 1 (4x smaller) at 500 px, from level 0
        // at 1024 px.
        let r = Region::new(4000, 3000, 2000, 1000);
        assert_eq!(
            choose_region_level(&im, r, 500),
            (1, Region::new(1000, 750, 500, 250))
        );
        assert_eq!(choose_region_level(&im, r, 1024), (0, r));
        // resolution_levels, when filled, take precedence over extra.pyramid.
        im.resolution_levels = vec![
            openreadout_core::ResolutionLevel::new(0, 8000, 6000, 8000, 6000),
            openreadout_core::ResolutionLevel::new(1, 4000, 3000, 8000, 6000),
            openreadout_core::ResolutionLevel::new(2, 2000, 1500, 8000, 6000),
        ];
        assert_eq!(choose_level(&im, 1024), 2);
        assert_eq!(level_dims(&im, 1), Some((4000, 3000)));
        // A level with fewer z planes (Imaris) is not chosen: the middle z of level 0 may not
        // exist there (zenodo4433202-ovule-732).
        im.size_z = 219;
        im.resolution_levels[2].size_z = Some(109);
        assert_eq!(choose_level(&im, 1024), 1);
    }
}
