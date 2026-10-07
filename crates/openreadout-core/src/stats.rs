//! Pixel statistics (`openreadout stats`, MCP `openreadout_stats`): per plane, per channel
//! and per image (the whole selection), streaming one plane at a time per worker.
//!
//! Every statistic is computed over finite samples; NaN and ±∞ are counted in `non_finite`.
//! Interleaved RGB planes are summarized over all their samples together (`stats`) and per
//! colour component (`components[]`: red, green, blue), so the mean of the red samples is one
//! call.
//!
//! Percentiles use NumPy's default (`linear`) interpolation between order statistics. They
//! are exact when the samples take at most 131,072 distinct values (always for 8- and 16-bit
//! integers); otherwise the samples are counted in buckets of 2^-8 relative width (sign,
//! exponent and the top 8 mantissa bits of the `f64`), so a percentile is within 0.2 % of
//! the true value and `exact` is false. The same counts, merged exactly, give the channel
//! and image aggregates and the histograms.

use std::collections::{BTreeMap, HashMap, HashSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::model::{FileInfo, ImageInfo};
use crate::parallel::{IN_FLIGHT_BYTES, PlaneRequest, ReadContext, read_in_order};
use crate::reader::{Dataset, PlaneIndex};
use crate::region::Region;
use crate::select::Selection;
use crate::{Error, PixelType, Plane, Result};

/// Percentiles reported, in percent.
pub const PERCENTILES: [f64; 5] = [1.0, 5.0, 50.0, 95.0, 99.0];
/// Distinct values kept exactly before switching to relative buckets.
const EXACT_KEYS_MAX: usize = 1 << 17;
/// Ordered-bit shift of a bucket: 64 - 1 (sign) - 11 (exponent) - 8 (mantissa bits kept).
const BUCKET_SHIFT: u32 = 44;
/// Default number of histogram bins.
pub const DEFAULT_BINS: u32 = 64;
/// Most histogram bins accepted.
pub const MAX_BINS: u32 = 65_536;

/// Spacing of histogram bin edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum HistogramScale {
    /// Equal-width bins from `min` to `max` (integer data: to `max + 1`, so each bin holds
    /// whole values).
    #[default]
    Linear,
    /// Geometrically spaced bins from the smallest positive value to `max`; values ≤ 0 are
    /// counted in `nonpositive`. Suited to data spanning decades (photon counts, spectra).
    Log,
}

/// The axis of a maximum-intensity projection (`stats --mip z`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Projection {
    /// Along z: one projected plane per image, channel and time point.
    Z,
    /// Along t: one projected plane per image, channel and z.
    T,
}

impl Projection {
    /// `z` or `t`.
    pub fn name(self) -> &'static str {
        match self {
            Projection::Z => "z",
            Projection::T => "t",
        }
    }
}

/// What to compute.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct StatsRequest {
    /// Only this image (default: all).
    pub image: Option<u32>,
    /// Plane selection strings (`c=0`, `z=1-3`, `t=0,2`).
    pub select: Vec<String>,
    /// Pyramid level (0 = full resolution).
    pub level: u32,
    /// Only this rectangle of every plane, in the level's pixel coordinates.
    pub region: Option<crate::region::Region>,
    /// Histogram bins (0 = no histograms).
    pub bins: u32,
    /// Spacing of the histogram bins.
    pub scale: HistogramScale,
    /// Include one entry per plane (the aggregates are always present).
    pub per_plane: bool,
    /// Project the selected planes along this axis first (maximum per pixel), then compute the
    /// statistics of the projections.
    pub mip: Option<Projection>,
}

/// NumPy-style percentiles of the finite samples.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Percentiles {
    /// 1st percentile.
    pub p1: f64,
    /// 5th percentile.
    pub p5: f64,
    /// Median.
    pub p50: f64,
    /// 95th percentile.
    pub p95: f64,
    /// 99th percentile.
    pub p99: f64,
}

/// A histogram of the finite samples.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Histogram {
    /// Linear or logarithmic bin spacing.
    pub scale: HistogramScale,
    /// `bins + 1` bin edges; bin `i` is `[edges[i], edges[i+1])`, the last bin includes its
    /// upper edge.
    pub edges: Vec<f64>,
    /// Samples per bin (`bins` entries).
    pub counts: Vec<u64>,
    /// Log scale only: samples ≤ 0, which have no place on a log axis.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nonpositive: Option<u64>,
}

/// Summary statistics of a set of samples.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SampleStats {
    /// Finite samples.
    pub count: u64,
    /// NaN and infinite samples (excluded from everything else).
    #[serde(skip_serializing_if = "is_zero")]
    pub non_finite: u64,
    /// Absent when there are no finite samples (as are mean, std, percentiles).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    /// Largest finite sample.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    /// Mean of the finite samples.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mean: Option<f64>,
    /// Population standard deviation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub std: Option<f64>,
    /// 1st, 5th, 50th, 95th and 99th percentiles.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub percentiles: Option<Percentiles>,
    /// True when the percentiles and histogram are exact (see the module documentation).
    pub exact: bool,
    /// Fraction of finite samples equal to 0.
    pub zero_fraction: f64,
    /// Integer pixel types: fraction of samples at the saturation level
    /// (`saturation_level`): the detector's ceiling 2^bits − 1 when the file records its
    /// significant bits (16383 for 14-bit data stored as uint16), else the type's maximum (255
    /// for uint8, 65535 for uint16, …), i.e. clipped by the detector or the digitizer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saturated_fraction: Option<f64>,
    /// Integer pixel types: the number of samples at `saturation_level` (exact; use it for
    /// "how many pixels are saturated").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saturated_count: Option<u64>,
    /// The value a sample must equal to count as saturated (integer pixel types).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saturation_level: Option<f64>,
    /// Why that level: `significant_bits` (2^bits − 1 from the bit depth the file records;
    /// `saturation_source` names the field) or `pixel_type` (the type's maximum: no bit depth
    /// recorded, channels of different depths merged, or values above the recorded depth).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saturation_basis: Option<String>,
    /// For `significant_bits`: the `info` field the bit depth came from, e.g.
    /// `images[].extra.component_bit_count` (CZI) or `images[].extra.bits_significant`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saturation_source: Option<String>,
    /// Present when histograms were requested (`bins` > 0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub histogram: Option<Histogram>,
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde's skip_serializing_if passes a reference
fn is_zero(v: &u64) -> bool {
    *v == 0
}

/// Statistics of one plane.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PlaneStats {
    /// Image index.
    pub image: u32,
    /// Pyramid level (0 = full resolution); omitted when 0.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub level: u32,
    /// Channel index.
    pub c: u32,
    /// Z index.
    pub z: u32,
    /// Time index.
    pub t: u32,
    /// Plane width in pixels.
    pub width: u32,
    /// Plane height in pixels.
    pub height: u32,
    /// Sample type.
    pub pixel_type: PixelType,
    /// Statistics of the plane's samples.
    pub stats: SampleStats,
    /// Multi-sample (RGB) pixels: the statistics of each colour component on its own.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<ComponentStats>,
}

/// Statistics of one colour component of interleaved multi-sample (RGB) pixels.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ComponentStats {
    /// Sample index within a pixel: for RGB 0 = red, 1 = green, 2 = blue (every reader returns
    /// R, G, B order; CZI's stored B, G, R is swapped on read).
    pub sample: u32,
    /// `red`, `green`, `blue` (and `alpha`) for 3- and 4-sample pixels.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Statistics of that component's samples only.
    pub stats: SampleStats,
}

/// The name of sample `i` of a pixel with `spp` samples: `red`, `green`, `blue`, `alpha` for
/// 3- and 4-sample (RGB, RGBA) pixels, none otherwise.
pub(crate) fn component_name(spp: u32, i: u32) -> Option<&'static str> {
    if !(3..=4).contains(&spp) {
        return None;
    }
    ["red", "green", "blue", "alpha"].get(i as usize).copied()
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde's skip_serializing_if passes a reference
fn is_zero_u32(v: &u32) -> bool {
    *v == 0
}

/// Statistics of every selected plane of one channel.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ChannelStats {
    /// Image index.
    pub image: u32,
    /// Channel index.
    pub c: u32,
    /// Channel name, if recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Name of the image (scene, series, position) the channel belongs to, if recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_name: Option<String>,
    /// Number of planes aggregated.
    pub planes: u64,
    /// Statistics over those planes (RGB: every colour sample pooled).
    pub stats: SampleStats,
    /// Multi-sample (RGB) pixels: the statistics of each colour component on its own
    /// (`components[0]` = red, `[1]` = green, `[2]` = blue).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<ComponentStats>,
}

/// Statistics of every selected plane of one image (the selection aggregate).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ImageStats {
    /// Image index.
    pub image: u32,
    /// Image (scene, series, position) name, if recorded (`info` → `images[].name`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Sample type.
    pub pixel_type: PixelType,
    /// Number of planes aggregated.
    pub planes: u64,
    /// Statistics over those planes.
    pub stats: SampleStats,
    /// Multi-sample (RGB) pixels: the statistics of each colour component over those planes
    /// (`components[0]` = red, `[1]` = green, `[2]` = blue).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<ComponentStats>,
}

/// Output of `stats`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct StatsOutput {
    /// The input file.
    pub path: String,
    /// Format id of the input file.
    pub format: String,
    /// Pyramid level the planes were read at (0 = full resolution); omitted when 0.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub level: u32,
    /// The rectangle of each plane the statistics cover (level pixel coordinates), when a
    /// region was asked for; absent = whole planes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<crate::region::Region>,
    /// The selection as given (`c=0`, …); empty = every plane.
    pub select: Vec<String>,
    /// Requested histogram bins (0 = none). Integer data with fewer distinct values in its
    /// range uses fewer, one per value.
    pub bins: u32,
    /// Per-plane statistics (with `--per plane`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub planes: Vec<PlaneStats>,
    /// One aggregate per image and channel.
    pub channels: Vec<ChannelStats>,
    /// One aggregate per image over all its selected planes.
    pub images: Vec<ImageStats>,
    /// Set with `--mip`: the statistics are of maximum-intensity projections along this axis
    /// (per pixel, the largest value over the selected planes); `planes[]` entries are the
    /// projections, with the projected axis's index reported as 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mip: Option<Projection>,
}

/// Streaming accumulator. Mergeable: statistics of a union are exact merges of its parts.
#[derive(Debug, Clone)]
pub struct Accumulator {
    n: u64,
    non_finite: u64,
    mean: f64,
    m2: f64,
    min: f64,
    max: f64,
    zeros: u64,
    saturated: u64,
    /// Integer pixel types: the type's maximum.
    saturation: Option<f64>,
    /// The recorded detector ceiling (2^bits − 1) and the field it came from.
    level: Option<(f64, &'static str)>,
    /// Samples equal to `level`.
    at_level: u64,
    /// Parts with different `level`s were merged: fall back to the type's maximum.
    mixed_levels: bool,
    integer: bool,
    exact: bool,
    /// Ordered-bit key (see [`ordered`]), shifted by [`BUCKET_SHIFT`] when not exact → count.
    counts: BTreeMap<i64, u64>,
}

/// Maps an `f64` to an `i64` whose order is the numeric order (−0 is folded into +0).
fn ordered(v: f64) -> i64 {
    let v = if v == 0.0 { 0.0 } else { v };
    let b = v.to_bits() as i64;
    if b < 0 { b ^ i64::MAX } else { b }
}

fn unordered(k: i64) -> f64 {
    let b = if k < 0 { k ^ i64::MAX } else { k };
    f64::from_bits(b as u64)
}

/// `images[].extra` fields that record how many bits of each sample the detector fills, in the
/// readers' vocabularies: ND2, OIB/OIF, ZVI, LIF `bits_significant`; OME-TIFF and OIR
/// `significant_bits`; CZI `component_bit_count`.
pub const SIGNIFICANT_BITS_KEYS: [&str; 3] = [
    "bits_significant",
    "significant_bits",
    "component_bit_count",
];

/// The saturation level of an image's samples when the file records a bit depth below the
/// pixel type's (14-bit data in uint16): `(2^bits − 1, "images[].extra.<field>")`. `None`
/// for float and signed types, and when no depth, or the full type width, is recorded.
pub fn recorded_saturation(im: &crate::model::ImageInfo) -> Option<(f64, &'static str)> {
    let width = match im.pixel_type {
        PixelType::Uint8 => 8,
        PixelType::Uint16 => 16,
        PixelType::Uint32 => 32,
        _ => return None,
    };
    SIGNIFICANT_BITS_KEYS.iter().find_map(|k| {
        let bits = im.extra.get(*k)?.as_u64()?;
        (bits > 0 && bits < width).then(|| {
            let path = match *k {
                "bits_significant" => "images[].extra.bits_significant",
                "significant_bits" => "images[].extra.significant_bits",
                _ => "images[].extra.component_bit_count",
            };
            (((1u64 << bits) - 1) as f64, path)
        })
    })
}

fn saturation_of(p: PixelType) -> Option<f64> {
    match p {
        PixelType::Uint8 => Some(f64::from(u8::MAX)),
        PixelType::Uint16 => Some(f64::from(u16::MAX)),
        PixelType::Uint32 => Some(f64::from(u32::MAX)),
        PixelType::Int8 => Some(f64::from(i8::MAX)),
        PixelType::Int16 => Some(f64::from(i16::MAX)),
        PixelType::Int32 => Some(f64::from(i32::MAX)),
        #[allow(clippy::cast_precision_loss)]
        PixelType::Int64 => Some(i64::MAX as f64),
        #[allow(clippy::cast_precision_loss)]
        PixelType::Uint64 => Some(u64::MAX as f64),
        _ => None,
    }
}

impl Accumulator {
    /// An empty accumulator for samples of type `p`.
    pub fn new(p: PixelType) -> Self {
        Accumulator {
            n: 0,
            non_finite: 0,
            mean: 0.0,
            m2: 0.0,
            min: f64::INFINITY,
            max: f64::NEG_INFINITY,
            zeros: 0,
            saturated: 0,
            saturation: saturation_of(p),
            level: None,
            at_level: 0,
            mixed_levels: false,
            integer: saturation_of(p).is_some(),
            exact: true,
            counts: BTreeMap::new(),
        }
    }

    /// Count samples at `level` (a recorded detector ceiling, see [`recorded_saturation`]) as
    /// saturated instead of the type's maximum. Ignored for float types and for a level not
    /// below the type's maximum.
    #[must_use]
    pub fn with_level(mut self, level: Option<(f64, &'static str)>) -> Self {
        self.level = level.filter(|(l, _)| self.saturation.is_some_and(|m| *l < m));
        self
    }

    /// Accumulate every sample of `plane`.
    pub fn from_plane(plane: &Plane) -> Self {
        Self::from_plane_at(plane, None)
    }

    /// [`Accumulator::from_plane`] with a recorded saturation level ([`Self::with_level`]).
    pub fn from_plane_at(plane: &Plane, level: Option<(f64, &'static str)>) -> Self {
        Self::from_samples(plane, level, 0, 1)
    }

    /// One accumulator per colour component of an interleaved multi-sample (RGB) plane
    /// (`[red, green, blue]`); empty for single-sample planes. Merged, they give
    /// [`Accumulator::from_plane_at`]'s statistics of every sample.
    pub fn components_at(plane: &Plane, level: Option<(f64, &'static str)>) -> Vec<Self> {
        let spp = plane.samples_per_pixel as usize;
        if spp <= 1 {
            return Vec::new();
        }
        (0..spp)
            .map(|k| Self::from_samples(plane, level, k, spp))
            .collect()
    }

    /// Accumulate every `step`-th sample of `plane`, from sample `first`.
    fn from_samples(
        plane: &Plane,
        level: Option<(f64, &'static str)>,
        first: usize,
        step: usize,
    ) -> Self {
        let mut feed = SampleFeed::new(plane.pixel_type, level, first, step);
        feed.feed(&plane.data);
        feed.finish()
    }

    /// 8/16-bit integers: everything derived from the counts by value (exact).
    fn add_dense(&mut self, counts: &[u64], value: impl Fn(usize) -> f64) {
        let mut part = Accumulator::new(PixelType::Uint8);
        part.saturation = self.saturation;
        part.level = self.level;
        part.integer = true;
        let n: u64 = counts.iter().sum();
        if n == 0 {
            return;
        }
        let mut sum = 0.0;
        for (i, &c) in counts.iter().enumerate() {
            if c > 0 {
                sum += value(i) * c as f64;
            }
        }
        let mean = sum / n as f64;
        let mut m2 = 0.0;
        for (i, &c) in counts.iter().enumerate() {
            if c > 0 {
                let v = value(i);
                m2 += (v - mean) * (v - mean) * c as f64;
                if v == 0.0 {
                    part.zeros += c;
                }
                if Some(v) == self.saturation {
                    part.saturated += c;
                }
                if Some(v) == self.level.map(|l| l.0) {
                    part.at_level += c;
                }
                part.min = part.min.min(v);
                part.max = part.max.max(v);
                part.counts.insert(ordered(v), c);
            }
        }
        part.n = n;
        part.mean = mean;
        part.m2 = m2;
        self.merge(&part);
    }

    fn coarsen(&mut self) {
        if !self.exact {
            return;
        }
        let mut coarse = BTreeMap::new();
        for (&k, &c) in &self.counts {
            *coarse.entry(k >> BUCKET_SHIFT).or_insert(0) += c;
        }
        self.counts = coarse;
        self.exact = false;
    }

    /// Merge another accumulator into this one (Chan et al. for mean and variance).
    pub fn merge(&mut self, o: &Accumulator) {
        self.non_finite += o.non_finite;
        if o.n > 0 {
            if self.n == 0 && !self.mixed_levels {
                self.level = self.level.or(o.level);
            }
            if o.mixed_levels || self.level.map(|l| l.0) != o.level.map(|l| l.0) {
                self.mixed_levels = true;
            }
            self.at_level += o.at_level;
            let n = self.n + o.n;
            let delta = o.mean - self.mean;
            self.m2 += o.m2 + delta * delta * (self.n as f64) * (o.n as f64) / n as f64;
            self.mean += delta * o.n as f64 / n as f64;
            self.n = n;
            self.min = self.min.min(o.min);
            self.max = self.max.max(o.max);
            self.zeros += o.zeros;
            self.saturated += o.saturated;
        }
        if !o.exact {
            self.coarsen();
        }
        if self.exact == o.exact {
            for (&k, &c) in &o.counts {
                *self.counts.entry(k).or_insert(0) += c;
            }
        } else {
            for (&k, &c) in &o.counts {
                *self.counts.entry(k >> BUCKET_SHIFT).or_insert(0) += c;
            }
        }
        if self.exact && self.counts.len() > EXACT_KEYS_MAX {
            self.coarsen();
        }
    }

    /// Representative value of a count key.
    fn value(&self, k: i64) -> f64 {
        if self.exact {
            return unordered(k);
        }
        let lo = unordered(k << BUCKET_SHIFT);
        let hi = unordered(((k + 1) << BUCKET_SHIFT) - 1);
        f64::midpoint(lo, hi).clamp(self.min, self.max)
    }

    /// Values at the given zero-based ranks (sorted ascending).
    fn values_at(&self, ranks: &[u64]) -> Vec<f64> {
        let mut out = Vec::with_capacity(ranks.len());
        let mut it = self.counts.iter();
        let mut cum = 0u64;
        let mut cur: Option<i64> = None;
        for &r in ranks {
            loop {
                if let Some(k) = cur
                    && r < cum
                {
                    out.push(self.value(k));
                    break;
                }
                let Some((&k, &c)) = it.next() else {
                    out.push(self.max);
                    break;
                };
                cum += c;
                cur = Some(k);
            }
        }
        out
    }

    fn percentiles(&self) -> Option<Percentiles> {
        if self.n == 0 {
            return None;
        }
        let last = self.n - 1;
        let mut ranks = Vec::new();
        let mut spec = Vec::new();
        for p in PERCENTILES {
            let h = last as f64 * p / 100.0;
            let lo = h.floor() as u64;
            spec.push((lo, h - h.floor()));
            ranks.push(lo);
            ranks.push((lo + 1).min(last));
        }
        let mut sorted = ranks.clone();
        sorted.sort_unstable();
        sorted.dedup();
        let vals = self.values_at(&sorted);
        let at = |r: u64| vals[sorted.binary_search(&r).unwrap_or(0)];
        let v: Vec<f64> = spec
            .iter()
            .map(|&(lo, frac)| {
                let a = at(lo);
                let b = at((lo + 1).min(last));
                a + frac * (b - a)
            })
            .collect();
        Some(Percentiles {
            p1: v[0],
            p5: v[1],
            p50: v[2],
            p95: v[3],
            p99: v[4],
        })
    }

    fn histogram(&self, bins: u32, scale: HistogramScale) -> Option<Histogram> {
        if bins == 0 || self.n == 0 {
            return None;
        }
        match scale {
            HistogramScale::Linear => {
                let lo = self.min;
                let (hi, bins) = if self.integer {
                    let span = self.max - self.min + 1.0;
                    (self.max + 1.0, (f64::from(bins).min(span)) as u32)
                } else {
                    (self.max, bins)
                };
                let bins = bins.max(1);
                let edges: Vec<f64> = (0..=bins)
                    .map(|i| lo + (hi - lo) * f64::from(i) / f64::from(bins))
                    .collect();
                let mut counts = vec![0u64; bins as usize];
                let width = hi - lo;
                for (&k, &c) in &self.counts {
                    let v = self.value(k);
                    let b = if width > 0.0 {
                        ((v - lo) / width * f64::from(bins)).floor()
                    } else {
                        0.0
                    };
                    counts[(b.max(0.0) as usize).min(bins as usize - 1)] += c;
                }
                Some(Histogram {
                    scale,
                    edges,
                    counts,
                    nonpositive: None,
                })
            }
            HistogramScale::Log => {
                let lo = self
                    .counts
                    .iter()
                    .map(|(&k, _)| self.value(k))
                    .find(|&v| v > 0.0);
                let mut nonpositive = 0u64;
                for (&k, &c) in &self.counts {
                    if self.value(k) <= 0.0 {
                        nonpositive += c;
                    }
                }
                let Some(lo) = lo else {
                    return Some(Histogram {
                        scale,
                        edges: Vec::new(),
                        counts: Vec::new(),
                        nonpositive: Some(nonpositive),
                    });
                };
                let hi = self.max.max(lo);
                let (l0, l1) = (lo.ln(), hi.ln());
                let edges: Vec<f64> = (0..=bins)
                    .map(|i| (l0 + (l1 - l0) * f64::from(i) / f64::from(bins)).exp())
                    .collect();
                let mut counts = vec![0u64; bins as usize];
                for (&k, &c) in &self.counts {
                    let v = self.value(k);
                    if v <= 0.0 {
                        continue;
                    }
                    let b = if l1 > l0 {
                        ((v.ln() - l0) / (l1 - l0) * f64::from(bins)).floor()
                    } else {
                        0.0
                    };
                    counts[(b.max(0.0) as usize).min(bins as usize - 1)] += c;
                }
                Some(Histogram {
                    scale,
                    edges,
                    counts,
                    nonpositive: Some(nonpositive),
                })
            }
        }
    }

    /// The reported statistics.
    pub fn finish(&self, bins: u32, scale: HistogramScale) -> SampleStats {
        let has = self.n > 0;
        let frac = |k: u64| if has { k as f64 / self.n as f64 } else { 0.0 };
        // The recorded depth holds unless channels of different depths were merged or a
        // sample exceeds it (then the recorded depth is wrong and the type's maximum is used).
        let (saturated, level, basis, source) = match self.level {
            Some((l, src)) if !self.mixed_levels && (!has || self.max <= l) => {
                (self.at_level, Some(l), "significant_bits", Some(src))
            }
            _ => (self.saturated, self.saturation, "pixel_type", None),
        };
        SampleStats {
            count: self.n,
            non_finite: self.non_finite,
            min: has.then_some(self.min),
            max: has.then_some(self.max),
            mean: has.then_some(self.mean),
            std: has.then(|| (self.m2 / self.n as f64).max(0.0).sqrt()),
            percentiles: self.percentiles(),
            exact: self.exact,
            zero_fraction: frac(self.zeros),
            saturated_fraction: self.saturation.map(|_| frac(saturated)),
            saturated_count: self.saturation.map(|_| saturated),
            saturation_level: self.saturation.and(level),
            saturation_basis: self.saturation.map(|_| basis.to_string()),
            saturation_source: source.map(str::to_string),
            histogram: self.histogram(bins, scale),
        }
    }
}

/// The 8- and 16-bit integer types, whose samples are counted by value.
#[derive(Debug, Clone, Copy)]
enum Dense {
    U8,
    I8,
    U16,
    I16,
}

impl Dense {
    fn of(p: PixelType) -> Option<Self> {
        match p {
            PixelType::Uint8 => Some(Dense::U8),
            PixelType::Int8 => Some(Dense::I8),
            PixelType::Uint16 => Some(Dense::U16),
            PixelType::Int16 => Some(Dense::I16),
            _ => None,
        }
    }

    fn size(self) -> usize {
        match self {
            Dense::U8 | Dense::I8 => 256,
            Dense::U16 | Dense::I16 => 65_536,
        }
    }

    fn value(self, i: usize) -> f64 {
        match self {
            Dense::U8 | Dense::U16 => i as f64,
            Dense::I8 => i as f64 - 128.0,
            Dense::I16 => i as f64 - 32_768.0,
        }
    }
}

/// Where a [`SampleFeed`] keeps what it has seen so far.
#[derive(Debug)]
enum FeedState {
    /// 8/16-bit integers: a count per value.
    Dense(Dense, Vec<u64>),
    /// Any other type: a running Welford accumulator and the value counts, exact until they
    /// are too many.
    Sparse {
        part: Accumulator,
        map: HashMap<i64, u64>,
        exact: bool,
    },
}

/// Accumulates every `step`-th sample, from sample `first`, of a plane handed over in pieces of
/// whole pixels, in order (a whole plane, or its full-width strips from top to bottom). The
/// pieces see the samples in the same order as the whole plane would, and the arithmetic is
/// the same, so the result is bit for bit that of [`Accumulator::from_plane_at`] or
/// [`Accumulator::components_at`] on the whole plane.
#[derive(Debug)]
pub(crate) struct SampleFeed {
    acc: Accumulator,
    pixel_type: PixelType,
    first: usize,
    step: usize,
    state: FeedState,
}

impl SampleFeed {
    pub(crate) fn new(
        pixel_type: PixelType,
        level: Option<(f64, &'static str)>,
        first: usize,
        step: usize,
    ) -> Self {
        let acc = Accumulator::new(pixel_type).with_level(level);
        let state = if let Some(d) = Dense::of(pixel_type) {
            FeedState::Dense(d, vec![0; d.size()])
        } else {
            let mut part = Accumulator::new(PixelType::Double);
            part.saturation = acc.saturation;
            part.level = acc.level;
            part.integer = acc.integer;
            FeedState::Sparse {
                part,
                map: HashMap::new(),
                exact: true,
            }
        };
        SampleFeed {
            acc,
            pixel_type,
            first,
            step: step.max(1),
            state,
        }
    }

    /// Accumulate the samples of `d`, which holds whole pixels.
    pub(crate) fn feed(&mut self, d: &[u8]) {
        let (first, step) = (self.first, self.step);
        match &mut self.state {
            FeedState::Dense(kind, counts) => match kind {
                Dense::U8 => count_into(
                    counts,
                    d.iter().skip(first).step_by(step).map(|&b| usize::from(b)),
                ),
                Dense::I8 => count_into(
                    counts,
                    d.iter()
                        .skip(first)
                        .step_by(step)
                        .map(|&b| usize::from(b.wrapping_add(128))),
                ),
                Dense::U16 => count_into(
                    counts,
                    d.as_chunks::<2>()
                        .0
                        .iter()
                        .skip(first)
                        .step_by(step)
                        .map(|c| usize::from(u16::from_le_bytes(*c))),
                ),
                Dense::I16 => count_into(
                    counts,
                    d.as_chunks::<2>()
                        .0
                        .iter()
                        .skip(first)
                        .step_by(step)
                        .map(|c| usize::from(u16::from_le_bytes(*c) ^ 0x8000)),
                ),
            },
            FeedState::Sparse { part, map, exact } => {
                match self.pixel_type {
                    PixelType::Uint32 => add_sparse(
                        part,
                        map,
                        exact,
                        d.as_chunks::<4>()
                            .0
                            .iter()
                            .skip(first)
                            .step_by(step)
                            .map(|c| f64::from(u32::from_le_bytes(*c))),
                    ),
                    PixelType::Int32 => add_sparse(
                        part,
                        map,
                        exact,
                        d.as_chunks::<4>()
                            .0
                            .iter()
                            .skip(first)
                            .step_by(step)
                            .map(|c| f64::from(i32::from_le_bytes(*c))),
                    ),
                    PixelType::Float => add_sparse(
                        part,
                        map,
                        exact,
                        d.as_chunks::<4>()
                            .0
                            .iter()
                            .skip(first)
                            .step_by(step)
                            .map(|c| f64::from(f32::from_le_bytes(*c))),
                    ),
                    PixelType::Double => add_sparse(
                        part,
                        map,
                        exact,
                        d.as_chunks::<8>()
                            .0
                            .iter()
                            .skip(first)
                            .step_by(step)
                            .map(|c| f64::from_le_bytes(*c)),
                    ),
                    // 64-bit integers and complex samples (whose value is the modulus)
                    pt => add_sparse(
                        part,
                        map,
                        exact,
                        d.chunks_exact(pt.bytes_per_sample().max(1))
                            .skip(first)
                            .step_by(step)
                            .map(move |c| pt.sample_f64(c)),
                    ),
                }
            }
        }
    }

    /// The statistics of every sample fed.
    pub(crate) fn finish(self) -> Accumulator {
        let mut acc = self.acc;
        match self.state {
            FeedState::Dense(kind, counts) => acc.add_dense(&counts, |i| kind.value(i)),
            FeedState::Sparse {
                mut part,
                map,
                exact,
            } => {
                part.exact = exact;
                part.counts = map.into_iter().collect();
                acc.merge(&part);
            }
        }
        acc
    }
}

/// Count each index.
fn count_into(counts: &mut [u64], idx: impl Iterator<Item = usize>) {
    for i in idx {
        counts[i] += 1;
    }
}

/// Welford update per sample, counts in a hash map until they are too many.
fn add_sparse(
    part: &mut Accumulator,
    map: &mut HashMap<i64, u64>,
    exact: &mut bool,
    values: impl Iterator<Item = f64>,
) {
    for v in values {
        if !v.is_finite() {
            part.non_finite += 1;
            continue;
        }
        part.n += 1;
        let delta = v - part.mean;
        part.mean += delta / part.n as f64;
        part.m2 += delta * (v - part.mean);
        part.min = part.min.min(v);
        part.max = part.max.max(v);
        if v == 0.0 {
            part.zeros += 1;
        }
        if Some(v) == part.saturation {
            part.saturated += 1;
        }
        if Some(v) == part.level.map(|l| l.0) {
            part.at_level += 1;
        }
        let k = ordered(v);
        *map.entry(if *exact { k } else { k >> BUCKET_SHIFT })
            .or_insert(0) += 1;
        if *exact && map.len() > EXACT_KEYS_MAX {
            *exact = false;
            let mut coarse: HashMap<i64, u64> = HashMap::new();
            for (k, c) in map.drain() {
                *coarse.entry(k >> BUCKET_SHIFT).or_insert(0) += c;
            }
            *map = coarse;
        }
    }
}

/// Planes at least this large are read in strips when the reader decodes regions itself
/// (`compute_stats`).
const STREAM_MIN_BYTES: u64 = 256 << 20;
/// Target size of one strip.
const STRIP_BYTES: u64 = 8 << 20;
/// Strip edges fall on multiples of this many rows when the level's tile size is unknown
/// (NDPI restart intervals are 8 or 16 rows high).
const STRIP_ALIGN_ROWS: u32 = 64;
/// Most strips decoded at once.
const STRIPS_IN_FLIGHT: u64 = 4;

/// Decoded bytes of the plane (or region) `r` reads from `im`.
fn request_bytes(im: &ImageInfo, r: &PlaneRequest) -> u64 {
    let (w, h) = match r.region {
        Some(g) => (g.width, g.height),
        None => crate::region::level_size(im, r.level).unwrap_or((im.size_x, im.size_y)),
    };
    u64::from(w)
        * u64::from(h)
        * u64::from(im.samples_per_pixel.max(1))
        * im.pixel_type.bytes_per_sample() as u64
}

/// The full-width strips, top to bottom, that read `r` piece by piece, and the bytes of the
/// largest. `None` means `r` is read whole: the plane is under [`STREAM_MIN_BYTES`], or the
/// image is neither tiled nor a pyramid, so its reader may crop regions out of the whole
/// plane and would decode it once per strip. Strip edges fall on the tile grid, so each tile
/// is decoded once.
fn strips_of(im: &ImageInfo, r: &PlaneRequest) -> Option<(Vec<Region>, u64)> {
    let bytes = request_bytes(im, r);
    if bytes < STREAM_MIN_BYTES {
        return None;
    }
    let levels = crate::region::levels_of(im);
    let pyramid = levels.len() > 1;
    let level = levels.into_iter().find(|l| l.level == r.level)?;
    let tile_h = match level.tile_height.filter(|&t| t > 0) {
        Some(t) => t,
        None if pyramid => STRIP_ALIGN_ROWS,
        None => return None,
    };
    let area = r
        .region
        .unwrap_or_else(|| Region::full(level.size_x, level.size_y));
    if area.height == 0 {
        return None;
    }
    let row = bytes / u64::from(area.height);
    let tiles = (STRIP_BYTES / row.saturating_mul(u64::from(tile_h)).max(1)).max(1);
    let rows = u32::try_from(u64::from(tile_h).saturating_mul(tiles)).unwrap_or(u32::MAX);
    let bottom = area.y.checked_add(area.height)?;
    let mut strips = Vec::new();
    let mut y = area.y;
    while y < bottom {
        let next = (y / rows)
            .saturating_add(1)
            .saturating_mul(rows)
            .min(bottom);
        strips.push(Region::new(area.x, y, area.width, next - y));
        y = next;
    }
    let largest = strips.iter().map(|s| u64::from(s.height)).max()? * row;
    Some((strips, largest))
}

/// One read of [`compute_stats`]: a whole plane, or one strip of a plane read in strips.
struct Unit {
    /// Index of the plane request it belongs to.
    request: usize,
    /// What is read.
    read: PlaneRequest,
    /// For a strip, whether it is the plane's last.
    strip: Option<bool>,
}

/// What a worker hands back for a [`Unit`]: the accumulators of a whole plane (width,
/// height, sample type, samples per pixel, pooled, per component), or a strip's pixels.
enum Piece {
    Whole(u32, u32, PixelType, u32, Accumulator, Vec<Accumulator>),
    Strip(Plane),
}

/// The accumulators of a plane read in strips: one feed for single-sample planes, one per
/// colour component otherwise (as [`plane_accumulators`]).
struct StripFeeds {
    width: u32,
    height: u32,
    pixel_type: PixelType,
    spp: u32,
    level: Option<(f64, &'static str)>,
    feeds: Vec<SampleFeed>,
}

impl StripFeeds {
    fn new(first: &Plane, level: Option<(f64, &'static str)>) -> Self {
        let spp = first.samples_per_pixel.max(1);
        let feeds = if spp == 1 {
            vec![SampleFeed::new(first.pixel_type, level, 0, 1)]
        } else {
            (0..spp as usize)
                .map(|k| SampleFeed::new(first.pixel_type, level, k, spp as usize))
                .collect()
        };
        StripFeeds {
            width: first.width,
            height: 0,
            pixel_type: first.pixel_type,
            spp: first.samples_per_pixel,
            level,
            feeds,
        }
    }

    fn feed(&mut self, p: &Plane) -> Result<()> {
        if p.width != self.width
            || p.pixel_type != self.pixel_type
            || p.samples_per_pixel != self.spp
        {
            return Err(Error::Other(
                "stats: the strips of one plane differ in width or sample type".into(),
            ));
        }
        self.height += p.height;
        for f in &mut self.feeds {
            f.feed(&p.data);
        }
        Ok(())
    }

    /// The pooled and per-component accumulators, as [`plane_accumulators`] gives them for
    /// the whole plane.
    fn finish(self) -> (Accumulator, Vec<Accumulator>) {
        let mut accs: Vec<Accumulator> = self.feeds.into_iter().map(SampleFeed::finish).collect();
        if accs.len() == 1 {
            return (accs.remove(0), Vec::new());
        }
        let mut all = Accumulator::new(self.pixel_type).with_level(self.level);
        for c in &accs {
            all.merge(c);
        }
        (all, accs)
    }
}

/// Per image and channel: planes, pooled samples, samples per pixel, per-component samples.
type ChannelAcc = (u64, Accumulator, u32, Vec<Accumulator>);

/// The pooled and per-component accumulators of one plane.
fn plane_accumulators(
    p: &Plane,
    lvl: Option<(f64, &'static str)>,
) -> (Accumulator, Vec<Accumulator>) {
    let comps = Accumulator::components_at(p, lvl);
    let acc = if comps.is_empty() {
        Accumulator::from_plane_at(p, lvl)
    } else {
        // The pooled statistics are the exact merge of the components.
        let mut all = Accumulator::new(p.pixel_type).with_level(lvl);
        for c in &comps {
            all.merge(c);
        }
        all
    };
    (acc, comps)
}

/// One sample's value (`bytes` holds exactly one little-endian sample of type `pt`; a complex
/// sample's value is its modulus).
fn sample_value(pt: PixelType, b: &[u8]) -> f64 {
    pt.sample_f64(b)
}

/// `acc = max(acc, p)` sample by sample (NaN propagates, as NumPy's `max`).
fn max_into(acc: &mut Plane, p: &Plane) -> Result<()> {
    if acc.data.len() != p.data.len()
        || acc.pixel_type != p.pixel_type
        || acc.samples_per_pixel != p.samples_per_pixel
    {
        return Err(Error::Usage(
            "--mip: the planes of one projection differ in size or type".into(),
        ));
    }
    let bps = p.pixel_type.bytes_per_sample().max(1);
    for (a, b) in acc.data.chunks_exact_mut(bps).zip(p.data.chunks_exact(bps)) {
        let (x, y) = (sample_value(p.pixel_type, a), sample_value(p.pixel_type, b));
        if y > x || (y.is_nan() && !x.is_nan()) {
            a.copy_from_slice(b);
        }
    }
    Ok(())
}

/// The per-component statistics of accumulators from [`Accumulator::components_at`].
fn finish_components(
    comps: &[Accumulator],
    spp: u32,
    bins: u32,
    scale: HistogramScale,
) -> Vec<ComponentStats> {
    comps
        .iter()
        .zip(0u32..)
        .map(|(a, k)| ComponentStats {
            sample: k,
            name: component_name(spp, k).map(str::to_string),
            stats: a.finish(bins, scale),
        })
        .collect()
}

/// Compute statistics of the selected planes of `ds` (already opened; `info` is its summary).
pub fn compute_stats(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    req: &StatsRequest,
    ctx: &ReadContext<'_>,
) -> Result<StatsOutput> {
    if req.bins > MAX_BINS {
        return Err(Error::Usage(format!(
            "--bins {} is too many (at most {MAX_BINS})",
            req.bins
        )));
    }
    if info.images.is_empty() {
        return Err(Error::unsupported(
            "stats",
            format!("pixel statistics of a {} file", info.format.name),
            if info.traces.is_empty() {
                "This file holds no images; for tables use `openreadout export FILE --format csv`."
            } else {
                "This file holds sampled signals; `openreadout trace FILE --json` reports per-channel statistics."
            },
        ));
    }
    if let Some(i) = req.image
        && !info.images.iter().any(|im| im.index == i)
    {
        return Err(Error::Usage(format!(
            "image {i} does not exist (the file has {} images)",
            info.images.len()
        )));
    }
    let sel = Selection::parse(&req.select)?;
    let mut requests = Vec::new();
    let mut plane_bytes = 0u64;
    for im in &info.images {
        if req.image.is_some_and(|i| i != im.index) {
            continue;
        }
        let (w, h) = match req.region {
            Some(r) => (r.width, r.height),
            None => crate::region::level_size(im, req.level).unwrap_or((im.size_x, im.size_y)),
        };
        plane_bytes = plane_bytes.max(
            u64::from(w)
                * u64::from(h)
                * u64::from(im.samples_per_pixel.max(1))
                * im.pixel_type.bytes_per_sample() as u64,
        );
        // Planes the instrument never acquired (a plate channel imaged at fewer Z planes than
        // the others) read as blank; they are not data and stay out of every statistic.
        let absent = crate::plate::absent_planes(im);
        // With --mip z the projected axis is the innermost loop, so each projection's planes
        // arrive together.
        let (outer, inner) = match req.mip {
            Some(Projection::Z) => (im.size_t, im.size_z),
            _ => (im.size_z, im.size_t),
        };
        for c in 0..im.size_c {
            for a in 0..outer {
                for b in 0..inner {
                    let (z, t) = match req.mip {
                        Some(Projection::Z) => (b, a),
                        _ => (a, b),
                    };
                    if sel.contains(c, z, t) && !absent.contains(&(c, z, t)) {
                        requests.push(PlaneRequest {
                            image: im.index,
                            index: PlaneIndex { c, z, t },
                            level: req.level,
                            region: req.region,
                        });
                    }
                }
            }
        }
    }
    if requests.is_empty() {
        return Err(Error::Usage("selection matches no planes".into()));
    }
    let total = requests.len() as u64;
    let mut planes = Vec::new();
    // Per image and channel: planes, pooled samples, samples per pixel, per-component.
    let mut channels: BTreeMap<(u32, u32), ChannelAcc> = BTreeMap::new();
    // Per image: sample type, planes, pooled samples, samples per pixel, per-component.
    #[allow(clippy::type_complexity)]
    let mut images: BTreeMap<u32, (PixelType, u64, Accumulator, u32, Vec<Accumulator>)> =
        BTreeMap::new();
    let per_plane = req.per_plane;
    let (bins, scale) = (req.bins, req.scale);
    let levels: BTreeMap<u32, Option<(f64, &'static str)>> = info
        .images
        .iter()
        .map(|im| (im.index, recorded_saturation(im)))
        .collect();
    let level_of = |image: u32| levels.get(&image).copied().flatten();
    let mut add = |r: PlaneRequest,
                   width: u32,
                   height: u32,
                   pixel_type: PixelType,
                   spp: u32,
                   acc: Accumulator,
                   comps: Vec<Accumulator>| {
        if per_plane {
            planes.push(PlaneStats {
                image: r.image,
                level: r.level,
                c: r.index.c,
                z: r.index.z,
                t: r.index.t,
                width,
                height,
                pixel_type,
                stats: acc.finish(bins, scale),
                components: finish_components(&comps, spp, bins, scale),
            });
        }
        let ch = channels.entry((r.image, r.index.c)).or_insert_with(|| {
            (
                0,
                Accumulator::new(pixel_type).with_level(level_of(r.image)),
                spp,
                Vec::new(),
            )
        });
        ch.0 += 1;
        ch.1.merge(&acc);
        if ch.3.len() < comps.len() {
            ch.3.resize_with(comps.len(), || {
                Accumulator::new(pixel_type).with_level(level_of(r.image))
            });
        }
        for (sum, c) in ch.3.iter_mut().zip(&comps) {
            sum.merge(c);
        }
        let im = images.entry(r.image).or_insert_with(|| {
            (
                pixel_type,
                0,
                Accumulator::new(pixel_type).with_level(level_of(r.image)),
                spp,
                Vec::new(),
            )
        });
        im.1 += 1;
        im.2.merge(&acc);
        if im.4.len() < comps.len() {
            im.4.resize_with(comps.len(), || {
                Accumulator::new(pixel_type).with_level(level_of(r.image))
            });
        }
        for (sum, c) in im.4.iter_mut().zip(&comps) {
            sum.merge(c);
        }
    };
    if let Some(axis) = req.mip {
        // Fold each projection's planes as they arrive (in order), then measure it.
        let key = |r: &PlaneRequest| match axis {
            Projection::Z => (r.image, r.index.c, r.index.t),
            Projection::T => (r.image, r.index.c, r.index.z),
        };
        let flush = |r: PlaneRequest,
                     p: &Plane,
                     add: &mut dyn FnMut(
            PlaneRequest,
            u32,
            u32,
            PixelType,
            u32,
            Accumulator,
            Vec<Accumulator>,
        )| {
            let mut r = r;
            match axis {
                Projection::Z => r.index.z = 0,
                Projection::T => r.index.t = 0,
            }
            let (acc, comps) = plane_accumulators(p, level_of(r.image));
            add(
                r,
                p.width,
                p.height,
                p.pixel_type,
                p.samples_per_pixel,
                acc,
                comps,
            );
        };
        let mut cur: Option<(PlaneRequest, Plane)> = None;
        read_in_order(
            ds,
            ctx,
            &requests,
            plane_bytes,
            &|_, p| Ok(p),
            &mut |i, p| {
                let r = requests[i];
                match &mut cur {
                    Some((cr, acc)) if key(cr) == key(&r) => max_into(acc, &p)?,
                    _ => {
                        if let Some((cr, pl)) = cur.take() {
                            flush(cr, &pl, &mut add);
                        }
                        cur = Some((r, p));
                    }
                }
                ctx.report(i as u64 + 1, total);
                Ok(())
            },
        )?;
        if let Some((cr, pl)) = cur.take() {
            flush(cr, &pl, &mut add);
        }
    } else {
        // Large planes of tiled levels are read in full-width strips and accumulated as they
        // arrive, so memory does not grow with the plane (a whole-slide level 0); the others
        // are read whole. Either way the statistics are the same, bit for bit.
        let mut units: Vec<Unit> = Vec::with_capacity(requests.len());
        let mut unit_bytes = 0u64;
        for (i, r) in requests.iter().enumerate() {
            let im = info.images.iter().find(|im| im.index == r.image);
            if let Some((strips, bytes)) = im.and_then(|im| strips_of(im, r)) {
                let n = strips.len();
                units.extend(strips.into_iter().enumerate().map(|(k, region)| Unit {
                    request: i,
                    read: PlaneRequest {
                        region: Some(region),
                        ..*r
                    },
                    strip: Some(k + 1 == n),
                }));
                // Sized so that at most a few strips are decoded at once, and fewer when a
                // strip is large (a level stored in tall tiles).
                unit_bytes = unit_bytes.max(
                    bytes
                        .saturating_mul(STRIPS_IN_FLIGHT)
                        .max(IN_FLIGHT_BYTES / STRIPS_IN_FLIGHT),
                );
            } else {
                units.push(Unit {
                    request: i,
                    read: *r,
                    strip: None,
                });
                unit_bytes = unit_bytes.max(im.map_or(plane_bytes, |im| request_bytes(im, r)));
            }
        }
        let reads: Vec<PlaneRequest> = units.iter().map(|u| u.read).collect();
        // Requests are distinct planes, so (image, index) tells a strip from a whole plane.
        let streamed: HashSet<(u32, PlaneIndex)> = units
            .iter()
            .filter(|u| u.strip.is_some())
            .map(|u| (u.read.image, u.read.index))
            .collect();
        let mut strips: Option<StripFeeds> = None;
        read_in_order(
            ds,
            ctx,
            &reads,
            unit_bytes,
            &|r, p| {
                if streamed.contains(&(r.image, r.index)) {
                    return Ok(Piece::Strip(p));
                }
                let (acc, comps) = plane_accumulators(&p, level_of(r.image));
                Ok(Piece::Whole(
                    p.width,
                    p.height,
                    p.pixel_type,
                    p.samples_per_pixel,
                    acc,
                    comps,
                ))
            },
            &mut |k, piece| {
                let unit = &units[k];
                let r = requests[unit.request];
                match piece {
                    Piece::Whole(width, height, pixel_type, spp, acc, comps) => {
                        add(r, width, height, pixel_type, spp, acc, comps);
                    }
                    Piece::Strip(p) => {
                        let feeds =
                            strips.get_or_insert_with(|| StripFeeds::new(&p, level_of(r.image)));
                        feeds.feed(&p)?;
                        if unit.strip == Some(true) {
                            let feeds = strips.take().unwrap_or_else(|| unreachable!());
                            let (w, h, pixel_type, spp) =
                                (feeds.width, feeds.height, feeds.pixel_type, feeds.spp);
                            let (acc, comps) = feeds.finish();
                            add(r, w, h, pixel_type, spp, acc, comps);
                        } else {
                            return Ok(());
                        }
                    }
                }
                ctx.report(unit.request as u64 + 1, total);
                Ok(())
            },
        )?;
    }
    let channel_name = |image: u32, c: u32| {
        info.images
            .iter()
            .find(|im| im.index == image)
            .and_then(|im| im.channels.iter().find(|ch| ch.index == c))
            .and_then(|ch| ch.name.clone())
    };
    let image_name = |image: u32| {
        info.images
            .iter()
            .find(|im| im.index == image)
            .and_then(|im| im.name.clone())
    };
    Ok(StatsOutput {
        path: info.path.clone(),
        format: info.format.id.clone(),
        level: req.level,
        region: req.region,
        select: req.select.clone(),
        bins: req.bins,
        planes,
        channels: channels
            .into_iter()
            .map(|((image, c), (n, acc, spp, comps))| ChannelStats {
                image,
                c,
                name: channel_name(image, c),
                image_name: image_name(image),
                planes: n,
                stats: acc.finish(bins, scale),
                components: finish_components(&comps, spp, bins, scale),
            })
            .collect(),
        mip: req.mip,
        images: images
            .into_iter()
            .map(|(image, (pixel_type, n, acc, spp, comps))| ImageStats {
                image,
                name: image_name(image),
                pixel_type,
                planes: n,
                stats: acc.finish(bins, scale),
                components: finish_components(&comps, spp, bins, scale),
            })
            .collect(),
    })
}

#[cfg(test)]
#[allow(clippy::float_cmp, clippy::many_single_char_names)]
mod tests {
    use super::*;

    /// Feeding a plane in full-width strips gives the whole plane's statistics bit for bit,
    /// including a float plane whose distinct values pass the exact-count limit mid-plane.
    #[test]
    fn strips_match_the_whole_plane() {
        let (w, h) = (401u32, 400u32);
        let n = (w * h) as usize;
        let mut seed = 7u64;
        let mut next = move || {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            seed >> 33
        };
        let planes = [
            (
                PixelType::Uint8,
                3u32,
                (0..n * 3).map(|_| next() as u8).collect::<Vec<u8>>(),
            ),
            (
                PixelType::Uint16,
                1,
                (0..n)
                    .flat_map(|_| (next() as u16 >> 4).to_le_bytes())
                    .collect(),
            ),
            (
                PixelType::Float,
                1,
                (0..n)
                    .flat_map(|_| (next() as f32 / 7.0).to_le_bytes())
                    .collect(),
            ),
            (
                PixelType::Int32,
                1,
                (0..n)
                    .flat_map(|_| (next() as i32 - (1 << 30)).to_le_bytes())
                    .collect(),
            ),
        ];
        for (pixel_type, spp, data) in planes {
            let whole = Plane {
                width: w,
                height: h,
                pixel_type,
                samples_per_pixel: spp,
                data,
            };
            let level = Some((4095.0, "significant_bits"));
            let (acc, comps) = plane_accumulators(&whole, level);
            let row = whole.data.len() / h as usize;
            let mut feeds: Option<StripFeeds> = None;
            for chunk in whole.data.chunks(row * 37) {
                let strip = Plane {
                    width: w,
                    height: (chunk.len() / row) as u32,
                    pixel_type,
                    samples_per_pixel: spp,
                    data: chunk.to_vec(),
                };
                feeds
                    .get_or_insert_with(|| StripFeeds::new(&strip, level))
                    .feed(&strip)
                    .unwrap();
            }
            let feeds = feeds.unwrap();
            assert_eq!(feeds.height, h);
            let (s_acc, s_comps) = feeds.finish();
            let json = |a: &Accumulator| {
                serde_json::to_string(&a.finish(64, HistogramScale::Linear)).unwrap()
            };
            assert_eq!(json(&acc), json(&s_acc), "{pixel_type:?}");
            assert_eq!(comps.len(), s_comps.len());
            for (a, b) in comps.iter().zip(&s_comps) {
                assert_eq!(json(a), json(b), "{pixel_type:?} component");
            }
        }
    }

    fn plane_u16(v: &[u16]) -> Plane {
        Plane {
            width: v.len() as u32,
            height: 1,
            pixel_type: PixelType::Uint16,
            samples_per_pixel: 1,
            data: v.iter().flat_map(|x| x.to_le_bytes()).collect(),
        }
    }

    fn plane_f32(v: &[f32]) -> Plane {
        Plane {
            width: v.len() as u32,
            height: 1,
            pixel_type: PixelType::Float,
            samples_per_pixel: 1,
            data: v.iter().flat_map(|x| x.to_le_bytes()).collect(),
        }
    }

    #[test]
    fn ordered_keys_sort_like_numbers() {
        let vals = [-1e300, -2.5, -1.0, -1e-300, 0.0, 1e-300, 1.0, 2.5, 1e300];
        let keys: Vec<i64> = vals.iter().map(|&v| ordered(v)).collect();
        assert!(keys.windows(2).all(|w| w[0] < w[1]));
        for &v in &vals {
            assert_eq!(unordered(ordered(v)), v);
        }
        assert_eq!(ordered(-0.0), ordered(0.0));
    }

    #[test]
    fn exact_stats_match_numpy() {
        // numpy: a = [0,1,2,...,9, 65535]; np.percentile(a, [1,5,50,95,99])
        let mut v: Vec<u16> = (0..10).collect();
        v.push(65535);
        let s = Accumulator::from_plane(&plane_u16(&v)).finish(4, HistogramScale::Linear);
        assert_eq!(s.count, 11);
        assert_eq!(s.min, Some(0.0));
        assert_eq!(s.max, Some(65535.0));
        let p = s.percentiles.unwrap();
        assert!((p.p1 - 0.1).abs() < 1e-9, "{p:?}");
        assert!((p.p5 - 0.5).abs() < 1e-9);
        assert!((p.p50 - 5.0).abs() < 1e-9);
        assert!((p.p95 - 32772.0).abs() < 1e-6, "{}", p.p95); // 9 + 0.5 * (65535 - 9)
        assert!((p.p99 - 58982.4).abs() < 1e-6, "{}", p.p99);
        assert!(s.exact);
        assert!((s.zero_fraction - 1.0 / 11.0).abs() < 1e-12);
        assert!((s.saturated_fraction.unwrap() - 1.0 / 11.0).abs() < 1e-12);
        let mean = (45.0 + 65535.0) / 11.0;
        assert!((s.mean.unwrap() - mean).abs() < 1e-9);
        let h = s.histogram.unwrap();
        assert_eq!(h.counts.len(), 4);
        assert_eq!(h.counts.iter().sum::<u64>(), 11);
        assert_eq!(h.counts[0], 10);
        assert_eq!(h.counts[3], 1);
    }

    #[test]
    fn merge_equals_whole() {
        let a: Vec<u16> = (0..1000).map(|i| (i * 7 % 300) as u16).collect();
        let b: Vec<u16> = (0..500).map(|i| (i * 13 % 900) as u16).collect();
        let mut whole = a.clone();
        whole.extend(&b);
        let mut m = Accumulator::from_plane(&plane_u16(&a));
        m.merge(&Accumulator::from_plane(&plane_u16(&b)));
        let w = Accumulator::from_plane(&plane_u16(&whole));
        let (x, y) = (
            m.finish(16, HistogramScale::Linear),
            w.finish(16, HistogramScale::Linear),
        );
        assert_eq!(x.percentiles, y.percentiles);
        assert_eq!(x.histogram, y.histogram);
        assert!((x.mean.unwrap() - y.mean.unwrap()).abs() < 1e-9);
        assert!((x.std.unwrap() - y.std.unwrap()).abs() < 1e-9);
    }

    #[test]
    fn floats_non_finite_and_buckets() {
        let mut v: Vec<f32> = (0..200_000).map(|i| i as f32 * 0.37 + 0.001).collect();
        v.push(f32::NAN);
        v.push(f32::INFINITY);
        let s = Accumulator::from_plane(&plane_f32(&v)).finish(8, HistogramScale::Log);
        assert_eq!(s.count, 200_000);
        assert_eq!(s.non_finite, 2);
        assert!(!s.exact, "200k distinct floats exceed the exact budget");
        let p = s.percentiles.unwrap();
        let true_p50 = 99_999.5 * 0.37 + 0.001;
        assert!((p.p50 - true_p50).abs() / true_p50 < 0.004, "{}", p.p50);
        assert!(s.saturated_fraction.is_none());
        let h = s.histogram.unwrap();
        assert_eq!(h.scale, HistogramScale::Log);
        assert_eq!(
            h.counts.iter().sum::<u64>() + h.nonpositive.unwrap(),
            200_000
        );
    }

    #[test]
    fn signed_dense_types() {
        let p = Plane {
            width: 4,
            height: 1,
            pixel_type: PixelType::Int16,
            samples_per_pixel: 1,
            data: [-5i16, 0, 7, i16::MAX]
                .iter()
                .flat_map(|x| x.to_le_bytes())
                .collect(),
        };
        let s = Accumulator::from_plane(&p).finish(0, HistogramScale::Linear);
        assert_eq!(s.min, Some(-5.0));
        assert_eq!(s.max, Some(32767.0));
        assert_eq!(s.saturated_fraction, Some(0.25));
        assert_eq!(s.zero_fraction, 0.25);
        assert!(s.histogram.is_none());
    }

    /// 14-bit data in uint16 (CZI `component_bit_count` 14): saturation is at 16383.
    #[test]
    fn recorded_bit_depth_sets_the_saturation_level() {
        let mut im = crate::model::ImageInfo::new(0, 4, 1, PixelType::Uint16);
        im.extra
            .insert("component_bit_count".into(), serde_json::json!(14));
        let level = recorded_saturation(&im);
        assert_eq!(level, Some((16383.0, "images[].extra.component_bit_count")));
        let p = plane_u16(&[0, 16383, 16383, 100]);
        let s = Accumulator::from_plane_at(&p, level).finish(0, HistogramScale::Linear);
        assert_eq!(s.saturated_fraction, Some(0.5));
        assert_eq!(s.saturation_level, Some(16383.0));
        assert_eq!(s.saturation_basis.as_deref(), Some("significant_bits"));
        assert_eq!(
            s.saturation_source.as_deref(),
            Some("images[].extra.component_bit_count")
        );
        // merged into an aggregate created with the same level, the count carries over
        let mut ch = Accumulator::new(PixelType::Uint16).with_level(level);
        ch.merge(&Accumulator::from_plane_at(&p, level));
        ch.merge(&Accumulator::from_plane_at(&plane_u16(&[1, 2]), level));
        let s = ch.finish(0, HistogramScale::Linear);
        assert!((s.saturated_fraction.unwrap() - 2.0 / 6.0).abs() < 1e-12);
        // without a recorded depth: the type's maximum
        let s = Accumulator::from_plane(&p).finish(0, HistogramScale::Linear);
        assert_eq!(s.saturated_fraction, Some(0.0));
        assert_eq!(s.saturation_level, Some(65535.0));
        assert_eq!(s.saturation_basis.as_deref(), Some("pixel_type"));
        assert!(s.saturation_source.is_none());
    }

    /// A sample above the recorded depth proves it wrong: fall back to the type's maximum.
    #[test]
    fn values_above_the_recorded_depth_fall_back() {
        let level = Some((4095.0, "images[].extra.bits_significant"));
        let p = plane_u16(&[4095, 5000, 65535, 1]);
        let s = Accumulator::from_plane_at(&p, level).finish(0, HistogramScale::Linear);
        assert_eq!(s.saturated_fraction, Some(0.25));
        assert_eq!(s.saturation_level, Some(65535.0));
        assert_eq!(s.saturation_basis.as_deref(), Some("pixel_type"));
        // full-width, signed and float types record no level
        let mut im = crate::model::ImageInfo::new(0, 1, 1, PixelType::Uint16);
        im.extra
            .insert("bits_significant".into(), serde_json::json!(16));
        assert_eq!(recorded_saturation(&im), None);
        im.pixel_type = PixelType::Int16;
        im.extra
            .insert("bits_significant".into(), serde_json::json!(12));
        assert_eq!(recorded_saturation(&im), None);
        let f =
            Accumulator::from_plane_at(&plane_f32(&[1.0]), level).finish(0, HistogramScale::Linear);
        assert!(f.saturation_level.is_none() && f.saturation_basis.is_none());
    }

    #[test]
    fn rgb_components_are_separate_and_merge_to_the_pooled_stats() {
        // Two RGB u8 pixels: (10, 200, 255) and (30, 100, 255).
        let p = Plane {
            width: 2,
            height: 1,
            pixel_type: PixelType::Uint8,
            samples_per_pixel: 3,
            data: vec![10, 200, 255, 30, 100, 255],
        };
        let comps = Accumulator::components_at(&p, None);
        assert_eq!(comps.len(), 3);
        let s: Vec<SampleStats> = comps
            .iter()
            .map(|a| a.finish(0, HistogramScale::Linear))
            .collect();
        assert_eq!(s[0].mean, Some(20.0));
        assert_eq!(s[1].mean, Some(150.0));
        assert_eq!(
            (s[2].mean, s[2].saturated_fraction),
            (Some(255.0), Some(1.0))
        );
        assert_eq!(
            (s[0].min, s[0].max, s[0].count),
            (Some(10.0), Some(30.0), 2)
        );
        let mut pooled = Accumulator::new(PixelType::Uint8);
        for c in &comps {
            pooled.merge(c);
        }
        let whole = Accumulator::from_plane(&p).finish(4, HistogramScale::Linear);
        let merged = pooled.finish(4, HistogramScale::Linear);
        assert_eq!(merged.count, whole.count);
        assert_eq!(merged.percentiles, whole.percentiles);
        assert_eq!(merged.histogram, whole.histogram);
        assert!((merged.mean.unwrap() - whole.mean.unwrap()).abs() < 1e-9);
        assert!((merged.std.unwrap() - whole.std.unwrap()).abs() < 1e-9);
        assert_eq!(component_name(3, 0), Some("red"));
        assert_eq!(component_name(4, 3), Some("alpha"));
        assert_eq!(component_name(2, 0), None);
        assert!(Accumulator::components_at(&plane_u16(&[1, 2]), None).is_empty());
    }

    #[test]
    fn max_projection_keeps_the_largest_sample() {
        let mut acc = plane_u16(&[1, 900, 3]);
        max_into(&mut acc, &plane_u16(&[5, 2, 3])).unwrap();
        max_into(&mut acc, &plane_u16(&[4, 1, 65535])).unwrap();
        assert_eq!(acc.data, plane_u16(&[5, 900, 65535]).data);
        let mut f = plane_f32(&[1.0, f32::NAN]);
        max_into(&mut f, &plane_f32(&[f32::NAN, 2.0])).unwrap();
        assert!(sample_value(PixelType::Float, &f.data[0..4]).is_nan());
        assert!(sample_value(PixelType::Float, &f.data[4..8]).is_nan());
        assert!(max_into(&mut acc, &plane_u16(&[1])).is_err());
    }
}
