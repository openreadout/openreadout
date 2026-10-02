//! Chromatographic peak detection and integration on one chromatogram `(t, y)`.
//!
//! Method (`book/src/guides/quantitation.md` has the full description and its validation):
//!
//! 1. **Noise** σ: the 25th percentile of the root-mean-square residuals of straight lines
//!    fitted to consecutive 1 % segments of the signal ([`crate::smooth::noise_sigma`]), or
//!    given by the caller. σₛ, the noise after smoothing, is measured the same way on the
//!    smoothed signal.
//! 2. **Smoothing**: quadratic Savitzky–Golay. The automatic window is a third of the median width
//!    at half height (in points) of the ten most prominent maxima of a lightly smoothed copy,
//!    rounded to an odd number of points; below 5 points the signal is not smoothed.
//! 3. **Detection** on the smoothed signal: every local maximum whose height above the running
//!    25 % quantile of the smoothed signal reaches the threshold is walked down on both sides.
//!    A side ends at a valley (the signal rises more than 2 σₛ above the lowest point reached; σₛ
//!    is the noise after smoothing) or where the signal has returned to the running baseline
//!    (within 2 σₛ; the walk then continues while the signal still falls). With the `auto`
//!    baseline a flank also ends where it meets a sloped background: at the first falling
//!    sample below half the peak's height where the derivative of the smoothed signal is below
//!    twice its noise, once the flank has been steeper than that, provided the signal does not
//!    rise into another peak within eight characteristic widths. The maximum is a peak
//!    when its height above the straight line between its two ends is at least
//!    `max(min_snr·σ, min_height)` and it spans at least `min_points` samples. Maxima inside a
//!    taller peak's span are not peaks (noise on its top).
//! 4. **Baselines** are straight lines anchored at the smoothed signal at the peak ends.
//!    `auto` (default) and `drop`: peaks that share a valley form a cluster with one baseline from the
//!    cluster's start to its end, split by vertical drop lines at the valleys (where the signal
//!    dips below that line the cluster is split there). `valley`: every peak gets its own line
//!    from its start to its end. `tangent`: `drop`, except that a peak smaller than `skim_ratio`
//!    times its taller neighbour in the cluster is skimmed off with a straight line between its
//!    own ends and the area under that line goes to the neighbour.
//! 5. **Quantities**: area = trapezoidal integral of (raw signal − baseline) between the ends,
//!    in signal units × `area_time_unit`; height = largest raw (signal − baseline); retention
//!    time = vertex of the parabola through the three highest smoothed points; widths at 50, 10
//!    and 5 % of the smoothed height by linear interpolation; USP tailing factor
//!    `W₀.₀₅/(2f)`; asymmetry factor `b/a` at 10 %; plates `5.54 (t_R/W½)²`; resolution to the
//!    previous peak `1.18 Δt_R/(W½₁ + W½₂)`; S/N = height / σ.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use openreadout_core::{Error, Result};

use crate::smooth::{NoiseMethod, noise_gain, noise_sigma, running_quantile, savgol};

/// Largest automatic smoothing window, in points.
pub const MAX_AUTO_WINDOW: usize = 201;
/// Default signal-to-noise threshold for detection (limit of detection convention).
pub const DEFAULT_MIN_SNR: f64 = 3.0;
/// Default height ratio below which `tangent` skims a peak off its neighbour.
pub const DEFAULT_SKIM_RATIO: f64 = 0.1;
/// `auto`: a flank ends where the smoothed signal's slope is below this many times the noise of
/// that slope.
pub const AUTO_END_SLOPE_NOISE: f64 = 2.0;
/// `auto`: … and below this fraction of the peak's typical slope (height / characteristic width
/// at half height), which only matters on noise-free signals.
pub const AUTO_END_SLOPE_FLOOR: f64 = 2e-4;
/// `auto`: the flat stretch must go on without a valley for this many characteristic peak
/// widths (otherwise it is the overlap region before a neighbouring peak).
pub const AUTO_LOOK_WIDTHS: f64 = 8.0;
/// `auto`: a rise ahead counts as a neighbouring peak when it exceeds this fraction of the
/// peak's height (and the noise hysteresis).
pub const AUTO_RISE_FRACTION: f64 = 0.02;
/// Largest `auto` look-ahead, samples.
const MAX_LOOK_POINTS: usize = 4096;

/// How the baseline under peaks is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BaselineMode {
    /// Valley-to-valley for resolved peaks; unresolved peaks share one baseline from the start
    /// of the group to its end, split by vertical drop lines at the valleys.
    Drop,
    /// Every peak gets its own straight baseline from its start to its end (valley to valley).
    Valley,
    /// As `drop`, but a small peak on a larger neighbour's flank (height below `skim_ratio` times
    /// the neighbour's) is skimmed off with a straight line between its own ends.
    Tangent,
    /// As `drop`, with peak ends that follow the background: a flank also ends where the signal
    /// has become as flat as its noise and goes on without a valley (a peak on a solvent tail or
    /// a drifting baseline ends where it meets it, instead of running down the tail). Each
    /// peak records why its baseline was drawn so (`baseline_reason`).
    #[default]
    Auto,
}

impl BaselineMode {
    /// Name used in JSON output and on the command line.
    pub fn id(self) -> &'static str {
        match self {
            BaselineMode::Drop => "drop",
            BaselineMode::Valley => "valley",
            BaselineMode::Tangent => "tangent",
            BaselineMode::Auto => "auto",
        }
    }
}

impl std::str::FromStr for BaselineMode {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "drop" | "drop-line" | "dropline" => Ok(BaselineMode::Drop),
            "valley" | "valley-to-valley" => Ok(BaselineMode::Valley),
            "tangent" | "skim" | "tangent-skim" => Ok(BaselineMode::Tangent),
            "auto" | "automatic" => Ok(BaselineMode::Auto),
            _ => Err(Error::Usage(format!(
                "unknown baseline `{s}`: use auto, drop, valley or tangent"
            ))),
        }
    }
}

/// Time unit of areas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AreaTimeUnit {
    /// Signal × minutes (retention times are in minutes).
    #[default]
    Min,
    /// Signal × seconds (what most chromatography data systems report, e.g. mAU·s).
    S,
}

impl AreaTimeUnit {
    /// `min` or `s`.
    pub fn id(self) -> &'static str {
        match self {
            AreaTimeUnit::Min => "min",
            AreaTimeUnit::S => "s",
        }
    }
    fn factor(self) -> f64 {
        match self {
            AreaTimeUnit::Min => 1.0,
            AreaTimeUnit::S => 60.0,
        }
    }
}

/// Detection and integration parameters. `Default` is the documented default method.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct PeakParams {
    /// Savitzky–Golay window in points (values below 5: no smoothing); `None`: automatic.
    pub smooth: Option<u32>,
    /// Detection threshold: height above the peak's own baseline / noise σ.
    pub min_snr: f64,
    /// Detection threshold in signal units (height above the peak's own baseline).
    pub min_height: Option<f64>,
    /// Smallest width at half height, minutes.
    pub min_width_min: Option<f64>,
    /// Fewest samples from peak start to peak end.
    pub min_points: u32,
    /// Baseline construction.
    pub baseline: BaselineMode,
    /// `tangent`: height ratio below which a peak is skimmed off its neighbour.
    pub skim_ratio: f64,
    /// Only look for peaks between these retention times (minutes).
    pub rt_range_min: Option<[f64; 2]>,
    /// Noise σ in signal units instead of the estimate.
    pub noise: Option<f64>,
    /// Window of the running baseline used to decide where peaks end, minutes (default: ten
    /// characteristic peak widths).
    pub baseline_window_min: Option<f64>,
    /// Report areas in signal × minutes (default) or signal × seconds.
    pub area_time_unit: AreaTimeUnit,
}

impl Default for PeakParams {
    fn default() -> Self {
        Self {
            smooth: None,
            min_snr: DEFAULT_MIN_SNR,
            min_height: None,
            min_width_min: None,
            min_points: 3,
            baseline: BaselineMode::Auto,
            skim_ratio: DEFAULT_SKIM_RATIO,
            rt_range_min: None,
            noise: None,
            baseline_window_min: None,
            area_time_unit: AreaTimeUnit::Min,
        }
    }
}

/// One integrated peak (a flat record: one row of a peak table).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Peak {
    /// 1-based peak number in retention-time order.
    pub number: u32,
    /// Retention time of the apex, minutes.
    pub rt_min: f64,
    /// Peak start (integration start), minutes.
    pub start_min: f64,
    /// Peak end (integration end), minutes.
    pub end_min: f64,
    /// Height of the apex above the baseline, signal units.
    pub height: f64,
    /// Area above the baseline between start and end, signal units × `area_time_unit`.
    pub area: f64,
    /// Share of the summed area of all peaks in the table, %.
    pub area_percent: f64,
    /// Width at half height, minutes (absent when the signal does not fall to half height
    /// before a drop line).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width_half_min: Option<f64>,
    /// Width at 10 % height, minutes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width_10pct_min: Option<f64>,
    /// Width at 5 % height, minutes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width_5pct_min: Option<f64>,
    /// End − start, minutes.
    pub width_base_min: f64,
    /// USP tailing factor `W₀.₀₅ / 2f` (f: apex to the leading edge at 5 % height).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tailing_factor: Option<f64>,
    /// Asymmetry factor `b/a` at 10 % height (a: leading half-width, b: trailing half-width).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asymmetry_factor: Option<f64>,
    /// Theoretical plates, half-height method `5.54 (t_R / W½)²` (t_R from time zero).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plates: Option<f64>,
    /// Resolution to the previous peak, `1.18 (t₂ − t₁) / (W½₁ + W½₂)`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution: Option<f64>,
    /// Height / noise σ (absent when the noise is 0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snr: Option<f64>,
    /// Samples from start to end.
    pub points: u32,
    /// How each end meets the baseline: `B` baseline, `V` valley (drop line), `T` tangent skim,
    /// `M` manual (given boundaries); e.g. `BB`, `BV`, `VB`, `TT`.
    pub baseline_code: String,
    /// Baseline value at the start, signal units.
    pub baseline_start: f64,
    /// Baseline value at the end, signal units.
    pub baseline_end: f64,
    /// `auto` baseline: why the baseline was drawn this way — `sloped_background` (an end was
    /// placed where the peak meets a sloped or drifting background), `drop_line` (fused with a
    /// neighbour: common baseline, vertical drop line at the valley), `baseline_penetration`
    /// (split from a neighbour where the signal dips below their common baseline), `isolated`
    /// (both ends on the baseline). The first that applies.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline_reason: Option<String>,
}

/// The method that produced a peak table, with every parameter actually used.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct PeakMethod {
    /// `savitzky_golay_quadratic` or `none`.
    pub smoothing: String,
    /// Smoothing window in points (1 = none).
    pub smooth_window_points: u32,
    /// True when the window was chosen automatically.
    pub smooth_auto: bool,
    /// Median width at half height of the most prominent maxima, points (drives the automatic
    /// smoothing and baseline windows).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub characteristic_width_points: Option<f64>,
    /// Noise σ, signal units.
    pub noise: f64,
    /// `segment_rms`, `segment_rms_nonzero`, `mad_first_difference`, `user` or `none`.
    pub noise_method: String,
    /// Detection threshold, S/N.
    pub min_snr: f64,
    /// Detection threshold, signal units.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_height: Option<f64>,
    /// Smallest width at half height, minutes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_width_min: Option<f64>,
    /// Fewest samples per peak.
    pub min_points: u32,
    /// `drop`, `valley` or `tangent`.
    pub baseline: String,
    /// `tangent` skim ratio.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skim_ratio: Option<f64>,
    /// Window of the running baseline, points.
    pub baseline_window_points: u32,
    /// `auto`: noise of the smoothed signal's derivative, signal units per minute (flanks end
    /// below [`AUTO_END_SLOPE_NOISE`] times it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slope_noise: Option<f64>,
    /// `auto`: how far a flat stretch must go on without a valley to end a peak, points.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub look_ahead_points: Option<u32>,
    /// Retention-time range searched, minutes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rt_range_min: Option<[f64; 2]>,
    /// `trapezoid`.
    pub integration: String,
    /// Time unit of `area`: `min` or `s`.
    pub area_time_unit: String,
    /// Samples analysed.
    pub points: u64,
}

/// Peaks of one chromatogram.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct PeakTable {
    /// How the peaks were found and integrated.
    pub method: PeakMethod,
    /// Peaks found.
    pub peak_count: u32,
    /// Summed area of all peaks.
    pub total_area: f64,
    /// Number of the peak with the largest area.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub main_peak: Option<u32>,
    /// Area % of that peak (chromatographic purity by area normalization).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub main_peak_area_percent: Option<f64>,
    /// One row per peak, in retention-time order.
    pub peaks: Vec<Peak>,
}

/// A candidate peak: apex, start and end sample indices.
#[derive(Debug, Clone, Copy)]
struct Span {
    apex: usize,
    l: usize,
    r: usize,
    /// `auto`: the start / end was placed on a sloped background.
    sloped: [bool; 2],
}

/// The analysed signal: finite samples in time order, restricted to the range.
struct Signal {
    t: Vec<f64>,
    y: Vec<f64>,
    ys: Vec<f64>,
    sigma: f64,
}

fn clean(t: &[f64], y: &[f64], range: Option<[f64; 2]>) -> Result<(Vec<f64>, Vec<f64>)> {
    if t.len() != y.len() {
        return Err(Error::Usage(format!(
            "time and intensity arrays differ in length ({} vs {})",
            t.len(),
            y.len()
        )));
    }
    let mut pts: Vec<(f64, f64)> = t
        .iter()
        .zip(y)
        .filter(|(a, b)| a.is_finite() && b.is_finite())
        .filter(|(a, _)| range.is_none_or(|[lo, hi]| **a >= lo && **a <= hi))
        .map(|(a, b)| (*a, *b))
        .collect();
    if pts.windows(2).any(|w| w[1].0 < w[0].0) {
        pts.sort_by(|a, b| a.0.total_cmp(&b.0));
    }
    Ok(pts.into_iter().unzip())
}

/// Characteristic peak width in points: median width at half prominence of the (up to) ten most
/// prominent maxima of `y` lightly smoothed, among maxima at least 10 σ prominent.
fn characteristic_width(y: &[f64], sigma: f64) -> Option<f64> {
    let n = y.len();
    if n < 5 {
        return None;
    }
    let ys = savgol(y, 5);
    let maxima = local_maxima(&ys);
    let mut top: Vec<usize> = maxima;
    top.sort_by(|&a, &b| ys[b].total_cmp(&ys[a]).then(a.cmp(&b)));
    top.truncate(50);
    let mut proms: Vec<(f64, f64)> = Vec::new();
    for &a in &top {
        // scipy-style prominence: lowest point between the apex and the nearest higher sample
        let (mut lmin, mut i) = (ys[a], a);
        while i > 0 {
            i -= 1;
            if ys[i] > ys[a] {
                break;
            }
            lmin = lmin.min(ys[i]);
        }
        let (mut rmin, mut j) = (ys[a], a);
        while j + 1 < n {
            j += 1;
            if ys[j] > ys[a] {
                break;
            }
            rmin = rmin.min(ys[j]);
        }
        let prom = ys[a] - lmin.max(rmin);
        if prom <= 0.0 || (sigma > 0.0 && prom < 10.0 * sigma) {
            continue;
        }
        let level = ys[a] - prom / 2.0;
        let mut l = a as f64;
        let mut i = a;
        while i > 0 && ys[i - 1] >= level {
            i -= 1;
        }
        if i > 0 {
            l = i as f64 - (ys[i] - level) / (ys[i] - ys[i - 1]);
        }
        let mut r = a as f64;
        let mut j = a;
        while j + 1 < n && ys[j + 1] >= level {
            j += 1;
        }
        if j + 1 < n {
            r = j as f64 + (ys[j] - level) / (ys[j] - ys[j + 1]);
        }
        let w = r - l;
        if w.is_finite() && w > 0.0 {
            proms.push((prom, w));
        }
    }
    if proms.is_empty() {
        return None;
    }
    proms.sort_by(|a, b| b.0.total_cmp(&a.0));
    proms.truncate(10);
    let mut widths: Vec<f64> = proms.iter().map(|p| p.1).collect();
    Some(crate::smooth::median_in_place(&mut widths))
}

/// Automatic smoothing window from the characteristic width (points): a third of it, odd.
fn auto_window(char_w: Option<f64>) -> usize {
    let Some(f) = char_w else { return 1 };
    let w = (f / 3.0).floor() as usize;
    let w = if w.is_multiple_of(2) {
        w.saturating_sub(1)
    } else {
        w
    };
    if w < 5 { 1 } else { w.min(MAX_AUTO_WINDOW) }
}

/// Indices of local maxima (the middle of a flat top), excluding the two end samples.
fn local_maxima(y: &[f64]) -> Vec<usize> {
    let n = y.len();
    let mut out = Vec::new();
    let mut i = 1;
    while i + 1 < n {
        if y[i] > y[i - 1] {
            let mut j = i;
            while j + 1 < n && y[j + 1] == y[i] {
                j += 1;
            }
            if j + 1 < n && y[j + 1] < y[i] {
                out.push(usize::midpoint(i, j));
            }
            i = j + 1;
        } else {
            i += 1;
        }
    }
    out
}

/// Where the flank of an `auto` peak meets a sloped background (see [`walk`]).
struct SlopeEnd<'a> {
    /// Derivative of the smoothed signal, signal units per minute.
    d: &'a [f64],
    /// Slope below which the flank is flat.
    min: f64,
    /// Samples the flat stretch must go on without a valley.
    look: usize,
    /// The flank counts as steep only below this level (half the apex height above the running
    /// baseline), so noise on the top of a broad peak does not end it.
    below: f64,
    /// A rise ahead larger than this is a neighbouring peak.
    rise: f64,
}

/// State of one flank walk for [`SlopeEnd::ends`].
#[derive(Default)]
struct FlankState {
    /// The flank has been steeper than the limit.
    steep: bool,
    /// A rise found ahead: the flank goes on to that neighbour, so no sample before it ends
    /// the peak (the walk only calls with new minima, whose look-ahead holds the same rise).
    rise_at: Option<isize>,
}

impl SlopeEnd<'_> {
    /// True when the falling sample `k` (a new minimum of the walk in `dir`) ends the flank:
    /// flat after a steep stretch while still above the running baseline's band (`floor`;
    /// below it the drop-line rule ends the peak), with no valley in the next `look` samples.
    fn ends(&self, ys: &[f64], k: usize, dir: isize, floor: f64, st: &mut FlankState) -> bool {
        let g = self.d.get(k).map_or(0.0, |v| v.abs());
        if g >= self.min {
            if ys[k] <= self.below {
                st.steep = true;
            }
            return false;
        }
        if !st.steep || ys[k] <= floor {
            return false;
        }
        let ki = k as isize;
        if st.rise_at.is_some_and(|r| (r - ki) * dir > 0) {
            return false;
        }
        let n = ys.len() as isize;
        let mut low = ys[k];
        let mut j = ki;
        for _ in 0..self.look {
            j += dir;
            if j < 0 || j >= n {
                break;
            }
            let v = ys[j as usize];
            if v > low + self.rise {
                st.rise_at = Some(j);
                return false;
            }
            low = low.min(v);
        }
        true
    }
}

/// Walk from `apex` in direction `dir` to the peak end (see the module docs). Returns the end
/// and whether it was placed on a sloped background (`auto` only).
fn walk(
    ys: &[f64],
    base: &[f64],
    apex: usize,
    dir: isize,
    (hyst, band): (f64, f64),
    slope: Option<&SlopeEnd<'_>>,
) -> (usize, bool) {
    let n = ys.len() as isize;
    let mut best = apex;
    let mut i = apex as isize;
    let mut st = FlankState::default();
    let mut ends = |k: usize| slope.is_some_and(|s| s.ends(ys, k, dir, base[k] + band, &mut st));
    loop {
        i += dir;
        if i < 0 || i >= n {
            break;
        }
        let k = i as usize;
        if ys[k] < ys[best] {
            best = k;
            if ends(k) {
                return (k, true);
            }
        } else if ys[k] > ys[best] + hyst {
            break;
        }
        if ys[best] <= base[best] + band {
            // back at the baseline: follow the signal while it still falls, then stop
            let mut j = best as isize + dir;
            while j >= 0 && j < n && ys[j as usize] < ys[best] {
                best = j as usize;
                if ends(best) {
                    break;
                }
                j += dir;
            }
            break;
        }
    }
    (best, false)
}

/// Central-difference derivative of `y` with respect to `t` (one-sided at the ends; 0 where
/// the times coincide).
fn derivative(t: &[f64], y: &[f64]) -> Vec<f64> {
    let n = y.len();
    (0..n)
        .map(|i| {
            let (a, b) = (i.saturating_sub(1), (i + 1).min(n - 1));
            let dt = t[b] - t[a];
            if dt > 0.0 { (y[b] - y[a]) / dt } else { 0.0 }
        })
        .collect()
}

fn line(t0: f64, y0: f64, t1: f64, y1: f64, t: f64) -> f64 {
    if t1 == t0 {
        return y0;
    }
    y0 + (y1 - y0) * (t - t0) / (t1 - t0)
}

/// Median spacing of `t` (0 for fewer than two samples).
fn median_step(t: &[f64]) -> f64 {
    let mut d: Vec<f64> = t
        .windows(2)
        .map(|w| w[1] - w[0])
        .filter(|v| *v > 0.0)
        .collect();
    if d.is_empty() {
        return 0.0;
    }
    crate::smooth::median_in_place(&mut d)
}

/// Detect and integrate the peaks of `(t, y)` (t in minutes, any signal unit).
pub fn find_peaks(t: &[f64], y: &[f64], p: &PeakParams) -> Result<PeakTable> {
    validate(p)?;
    let (t, y) = clean(t, y, p.rt_range_min)?;
    let n = y.len();
    let (sigma, noise_method) = match p.noise {
        Some(v) => (v, NoiseMethod::User),
        None => noise_sigma(&y),
    };
    let char_w = characteristic_width(&y, sigma);
    let window = match p.smooth {
        Some(w) => {
            let w = w as usize;
            if w < 5 { 1 } else { w | 1 }
        }
        None => auto_window(char_w),
    };
    let ys = savgol(&y, window);
    let sigma_s = match p.noise {
        Some(v) => v * noise_gain(window),
        None => noise_sigma(&ys).0.min(sigma),
    };
    let step = median_step(&t);
    let bw = match p.baseline_window_min {
        Some(m) if step > 0.0 => (m / step).round() as usize,
        _ => char_w.map_or(n / 10, |f| (10.0 * f).round() as usize),
    }
    .clamp(31, n.max(31));
    let mut method = PeakMethod {
        smoothing: if window >= 5 {
            "savitzky_golay_quadratic".into()
        } else {
            "none".into()
        },
        smooth_window_points: window as u32,
        smooth_auto: p.smooth.is_none(),
        characteristic_width_points: char_w.map(|v| (v * 100.0).round() / 100.0),
        noise: sigma,
        noise_method: noise_method.id().into(),
        min_snr: p.min_snr,
        min_height: p.min_height,
        min_width_min: p.min_width_min,
        min_points: p.min_points,
        baseline: p.baseline.id().into(),
        skim_ratio: (p.baseline == BaselineMode::Tangent).then_some(p.skim_ratio),
        baseline_window_points: bw as u32,
        slope_noise: None,
        look_ahead_points: None,
        rt_range_min: p.rt_range_min,
        integration: "trapezoid".into(),
        area_time_unit: p.area_time_unit.id().into(),
        points: n as u64,
    };
    if n < 3 {
        method.baseline_window_points = 0;
        return Ok(finish_table(method, Vec::new()));
    }
    let base = running_quantile(&ys, bw, 0.25);
    let thresh = (p.min_snr * sigma).max(p.min_height.unwrap_or(0.0));
    let hyst = 2.0 * sigma_s;
    let band = 2.0 * sigma_s;
    let min_points = p.min_points.max(3) as usize;
    // auto: flanks also end where they meet a sloped background
    let char_w_min = char_w.map_or(0.0, |w| w * step);
    let deriv = (p.baseline == BaselineMode::Auto && char_w_min > 0.0).then(|| derivative(&t, &ys));
    let slope_noise = deriv.as_ref().map(|d| noise_sigma(d).0);
    let look = char_w.map_or(0, |w| {
        ((AUTO_LOOK_WIDTHS * w).round() as usize).clamp(1, MAX_LOOK_POINTS)
    });
    if deriv.is_some() {
        method.slope_noise = slope_noise;
        method.look_ahead_points = Some(look as u32);
    }
    let mut cands: Vec<Span> = Vec::new();
    for a in local_maxima(&ys) {
        let h = ys[a] - base[a];
        if h < thresh || h <= 0.0 {
            continue;
        }
        let slope = deriv.as_deref().map(|d| SlopeEnd {
            d,
            min: (AUTO_END_SLOPE_NOISE * slope_noise.unwrap_or(0.0))
                .max(AUTO_END_SLOPE_FLOOR * h / char_w_min),
            look,
            below: ys[a] - 0.5 * h,
            rise: hyst.max(AUTO_RISE_FRACTION * h),
        });
        let (l, sl) = walk(&ys, &base, a, -1, (hyst, band), slope.as_ref());
        let (r, sr) = walk(&ys, &base, a, 1, (hyst, band), slope.as_ref());
        // an end counts as placed on a sloped background when the drop-line rule would have
        // anchored the baseline lower by more than 2 % of the peak's height
        let moved = |end: usize, dir: isize| {
            let (plain, _) = walk(&ys, &base, a, dir, (hyst, band), None);
            ys[end] - ys[plain] > AUTO_RISE_FRACTION * h
        };
        let (sl, sr) = (sl && moved(l, -1), sr && moved(r, 1));
        if r + 1 - l < min_points {
            continue;
        }
        let prom = ys[a] - line(t[l], ys[l], t[r], ys[r], t[a]);
        if prom < thresh || prom <= 0.0 {
            continue;
        }
        cands.push(Span {
            apex: a,
            l,
            r,
            sloped: [sl, sr],
        });
    }
    cands.sort_by(|a, b| ys[b.apex].total_cmp(&ys[a.apex]).then(a.apex.cmp(&b.apex)));
    let mut spans: Vec<Span> = Vec::new();
    for c in cands {
        if spans.iter().any(|s| s.l < c.apex && c.apex < s.r) {
            continue;
        }
        spans.push(c);
    }
    spans.sort_by_key(|s| s.apex);
    // Neighbours whose spans overlap meet at the lowest smoothed point between their apexes.
    for k in 1..spans.len() {
        if spans[k - 1].r > spans[k].l {
            let (a, b) = (spans[k - 1].apex, spans[k].apex);
            let v = (a..=b)
                .min_by(|&i, &j| ys[i].total_cmp(&ys[j]).then(i.cmp(&j)))
                .unwrap_or(a);
            spans[k - 1].r = v;
            spans[k].l = v;
            spans[k - 1].sloped[1] = false;
            spans[k].sloped[0] = false;
        }
    }
    let sig = Signal { t, y, ys, sigma };
    if let Some(min_w) = p.min_width_min {
        spans.retain(|s| {
            let bl = |tt: f64| line(sig.t[s.l], sig.ys[s.l], sig.t[s.r], sig.ys[s.r], tt);
            width_at(&sig, s, &bl, 0.5).is_none_or(|w| w >= min_w)
        });
    }
    let peaks = integrate(&sig, &spans, p);
    Ok(finish_table(method, peaks))
}

fn validate(p: &PeakParams) -> Result<()> {
    if !(p.min_snr.is_finite() && p.min_snr >= 0.0) {
        return Err(Error::Usage("min_snr must be a number >= 0".into()));
    }
    if let Some(h) = p.min_height
        && !(h.is_finite() && h >= 0.0)
    {
        return Err(Error::Usage("min_height must be a number >= 0".into()));
    }
    if let Some(n) = p.noise
        && !(n.is_finite() && n >= 0.0)
    {
        return Err(Error::Usage("noise must be a number >= 0".into()));
    }
    if !(p.skim_ratio.is_finite() && p.skim_ratio > 0.0 && p.skim_ratio < 1.0) {
        return Err(Error::Usage("skim_ratio must be between 0 and 1".into()));
    }
    if let Some([a, b]) = p.rt_range_min
        && !(a.is_finite() && b.is_finite() && a < b)
    {
        return Err(Error::Usage(
            "the retention-time range must be two numbers, low < high (minutes)".into(),
        ));
    }
    if let Some(w) = p.baseline_window_min
        && !(w.is_finite() && w > 0.0)
    {
        return Err(Error::Usage("baseline_window_min must be > 0".into()));
    }
    Ok(())
}

/// Straight baseline of one peak: two anchor points `(t, value)`.
#[derive(Debug, Clone, Copy)]
struct Base {
    t0: f64,
    b0: f64,
    t1: f64,
    b1: f64,
}

impl Base {
    fn at(&self, t: f64) -> f64 {
        line(self.t0, self.b0, self.t1, self.b1, t)
    }
}

/// Group touching spans into clusters (index ranges into `spans`).
fn clusters(spans: &[Span]) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    let mut start = 0;
    for k in 1..=spans.len() {
        if k == spans.len() || spans[k - 1].r != spans[k].l {
            if k > start {
                out.push(start..k);
            }
            start = k;
        }
    }
    out
}

fn integrate(sig: &Signal, spans: &[Span], p: &PeakParams) -> Vec<Peak> {
    let (t, ys) = (&sig.t, &sig.ys);
    let anchor = |i: usize| (t[i], ys[i]);
    let mut bases: Vec<Base> = Vec::with_capacity(spans.len());
    let mut codes: Vec<[char; 2]> = Vec::with_capacity(spans.len());
    // tangent skim: spans after skimming, and (parent, rider) pairs
    let mut skimmed: Vec<Span> = spans.to_vec();
    let mut riders: Vec<(usize, usize)> = Vec::new();
    match p.baseline {
        BaselineMode::Valley => {
            for (k, s) in spans.iter().enumerate() {
                let (t0, b0) = anchor(s.l);
                let (t1, b1) = anchor(s.r);
                bases.push(Base { t0, b0, t1, b1 });
                let shared_l = k > 0 && spans[k - 1].r == s.l;
                let shared_r = k + 1 < spans.len() && spans[k + 1].l == s.r;
                codes.push([
                    if shared_l { 'V' } else { 'B' },
                    if shared_r { 'V' } else { 'B' },
                ]);
            }
        }
        BaselineMode::Drop | BaselineMode::Tangent | BaselineMode::Auto => {
            // clusters, split where the signal at a valley dips below the cluster line
            let mut groups: Vec<std::ops::Range<usize>> = Vec::new();
            for c in clusters(spans) {
                let mut stack = vec![c];
                while let Some(g) = stack.pop() {
                    let (t0, b0) = anchor(spans[g.start].l);
                    let (t1, b1) = anchor(spans[g.end - 1].r);
                    let split = (g.start + 1..g.end)
                        .filter(|&k| {
                            let v = spans[k].l;
                            ys[v] < line(t0, b0, t1, b1, t[v])
                        })
                        .min_by(|&a, &b| {
                            let da = ys[spans[a].l] - line(t0, b0, t1, b1, t[spans[a].l]);
                            let db = ys[spans[b].l] - line(t0, b0, t1, b1, t[spans[b].l]);
                            da.total_cmp(&db)
                        });
                    match split {
                        Some(k) => {
                            stack.push(k..g.end);
                            stack.push(g.start..k);
                        }
                        None => groups.push(g),
                    }
                }
            }
            groups.sort_by_key(|g| g.start);
            for g in &groups {
                let (t0, b0) = anchor(spans[g.start].l);
                let (t1, b1) = anchor(spans[g.end - 1].r);
                let cb = Base { t0, b0, t1, b1 };
                for k in g.clone() {
                    bases.push(cb);
                    codes.push([
                        if k == g.start { 'B' } else { 'V' },
                        if k + 1 == g.end { 'B' } else { 'V' },
                    ]);
                }
            }
            if p.baseline == BaselineMode::Tangent {
                for g in &groups {
                    let cb = bases[g.start];
                    let over_cb = |k: usize| {
                        let s = spans[k];
                        (s.l..=s.r)
                            .map(|i| sig.y[i] - cb.at(t[i]))
                            .fold(f64::NEG_INFINITY, f64::max)
                    };
                    let heights: Vec<f64> = g.clone().map(over_cb).collect();
                    let mut rider = vec![false; g.len()];
                    for (off, k) in g.clone().enumerate() {
                        let neighbours = [
                            (off > 0).then(|| off - 1),
                            (off + 1 < g.len()).then_some(off + 1),
                        ];
                        let Some(o) = neighbours
                            .into_iter()
                            .flatten()
                            .filter(|&o| !rider[o] && heights[o] > heights[off])
                            .max_by(|&a, &b| heights[a].total_cmp(&heights[b]))
                        else {
                            continue;
                        };
                        let s = skimmed[k];
                        // the skim line runs from the shared valley to the point where it
                        // touches the signal on the far side of the rider (a tangent)
                        let slope = |a: usize, b: usize| (ys[b] - ys[a]) / (t[b] - t[a]);
                        let (l, r) = if o < off {
                            let v = s.l;
                            let j = (s.apex + 1..=s.r)
                                .filter(|&j| t[j] > t[v])
                                .min_by(|&a, &b| {
                                    slope(v, a).total_cmp(&slope(v, b)).then(a.cmp(&b))
                                });
                            (v, j.unwrap_or(s.r))
                        } else {
                            let v = s.r;
                            let j = (s.l..s.apex).filter(|&j| t[j] < t[v]).max_by(|&a, &b| {
                                slope(a, v).total_cmp(&slope(b, v)).then(b.cmp(&a))
                            });
                            (j.unwrap_or(s.l), v)
                        };
                        if r <= l {
                            continue;
                        }
                        let (t0, b0) = anchor(l);
                        let (t1, b1) = anchor(r);
                        let rb = Base { t0, b0, t1, b1 };
                        let own = (l..=r)
                            .map(|i| sig.y[i] - rb.at(t[i]))
                            .fold(f64::NEG_INFINITY, f64::max);
                        if own >= p.skim_ratio * heights[o] {
                            continue;
                        }
                        rider[off] = true;
                        let parent = g.start + o;
                        if o < off {
                            skimmed[parent].r = s.r;
                            codes[parent][1] = codes[k][1];
                        } else {
                            skimmed[parent].l = s.l;
                            codes[parent][0] = codes[k][0];
                        }
                        skimmed[k] = Span {
                            apex: s.apex,
                            l,
                            r,
                            sloped: [false; 2],
                        };
                        bases[k] = rb;
                        codes[k] = ['T', 'T'];
                        riders.push((parent, k));
                    }
                }
            }
        }
    }
    let f = p.area_time_unit.factor();
    let mut peaks: Vec<Peak> = skimmed
        .iter()
        .zip(&bases)
        .zip(&codes)
        .map(|((s, b), c)| measure(sig, s, b, *c, f))
        .collect();
    if p.baseline == BaselineMode::Auto {
        for (k, q) in peaks.iter_mut().enumerate() {
            let s = &spans[k];
            let shared =
                (k > 0 && spans[k - 1].r == s.l) || (k + 1 < spans.len() && spans[k + 1].l == s.r);
            let reason = if s.sloped[0] || s.sloped[1] {
                "sloped_background"
            } else if q.baseline_code.contains('V') {
                "drop_line"
            } else if shared {
                "baseline_penetration"
            } else {
                "isolated"
            };
            q.baseline_reason = Some(reason.into());
        }
    }
    // a skimmed rider's area is taken out of its parent's span
    for (parent, rider) in riders {
        peaks[parent].area -= peaks[rider].area;
    }
    // A "peak" whose signal lies below its baseline (a signal that starts mid-peak, a step)
    // is not a peak.
    peaks.retain(|q| q.area > 0.0 && q.height > 0.0);
    let total: f64 = peaks.iter().map(|q| q.area).sum();
    for (k, q) in peaks.iter_mut().enumerate() {
        q.number = k as u32 + 1;
        q.area_percent = if total > 0.0 {
            100.0 * q.area.max(0.0) / total
        } else {
            0.0
        };
    }
    for k in 1..peaks.len() {
        let (a, b) = (&peaks[k - 1], &peaks[k]);
        if let (Some(w1), Some(w2)) = (a.width_half_min, b.width_half_min)
            && w1 + w2 > 0.0
        {
            let rs = 1.18 * (b.rt_min - a.rt_min) / (w1 + w2);
            peaks[k].resolution = Some(rs);
        }
    }
    peaks
}

fn trapezoid(t: &[f64], l: usize, r: usize, v: impl Fn(usize) -> f64) -> f64 {
    let mut a = 0.0;
    for i in l..r {
        a += f64::midpoint(v(i), v(i + 1)) * (t[i + 1] - t[i]);
    }
    a
}

/// Time where `(signal − baseline)` on the smoothed signal falls to `frac` of the smoothed
/// apex height, walking outward from the apex: (left, right, apex time).
fn crossings(
    sig: &Signal,
    s: &Span,
    base: &dyn Fn(f64) -> f64,
    frac: f64,
) -> (Option<f64>, Option<f64>, usize, f64) {
    let (t, ys) = (&sig.t, &sig.ys);
    let v = |i: usize| ys[i] - base(t[i]);
    let apex = (s.l..=s.r)
        .max_by(|&a, &b| v(a).total_cmp(&v(b)).then(b.cmp(&a)))
        .unwrap_or(s.apex);
    let h = v(apex);
    if h <= 0.0 {
        return (None, None, apex, h);
    }
    let level = frac * h;
    let mut left = None;
    let mut i = apex;
    while i > s.l {
        if v(i - 1) < level {
            let (a, b) = (v(i - 1), v(i));
            left = Some(t[i - 1] + (level - a) / (b - a) * (t[i] - t[i - 1]));
            break;
        }
        i -= 1;
    }
    let mut right = None;
    let mut j = apex;
    while j < s.r {
        if v(j + 1) < level {
            let (a, b) = (v(j), v(j + 1));
            right = Some(t[j] + (a - level) / (a - b) * (t[j + 1] - t[j]));
            break;
        }
        j += 1;
    }
    (left, right, apex, h)
}

fn width_at(sig: &Signal, s: &Span, base: &dyn Fn(f64) -> f64, frac: f64) -> Option<f64> {
    let (l, r, _, _) = crossings(sig, s, base, frac);
    Some(r? - l?)
}

/// Vertex of the parabola through three points (None when not concave).
fn vertex(t: [f64; 3], v: [f64; 3]) -> Option<f64> {
    let d0 = (t[0] - t[1]) * (t[0] - t[2]);
    let d1 = (t[1] - t[0]) * (t[1] - t[2]);
    let d2 = (t[2] - t[0]) * (t[2] - t[1]);
    if d0 == 0.0 || d1 == 0.0 || d2 == 0.0 {
        return None;
    }
    let a = v[0] / d0 + v[1] / d1 + v[2] / d2;
    let b = -(v[0] * (t[1] + t[2]) / d0 + v[1] * (t[0] + t[2]) / d1 + v[2] * (t[0] + t[1]) / d2);
    if a >= 0.0 {
        return None;
    }
    let x = -b / (2.0 * a);
    x.is_finite().then_some(x.clamp(t[0], t[2]))
}

fn measure(sig: &Signal, s: &Span, b: &Base, code: [char; 2], f: f64) -> Peak {
    let t = &sig.t;
    let bl = |tt: f64| b.at(tt);
    let raw = |i: usize| sig.y[i] - b.at(t[i]);
    let top = (s.l..=s.r)
        .max_by(|&a, &c| raw(a).total_cmp(&raw(c)).then(c.cmp(&a)))
        .unwrap_or(s.apex);
    let height = raw(top);
    let area = trapezoid(t, s.l, s.r, raw) * f;
    let (l50, r50, sa, _) = crossings(sig, s, &bl, 0.5);
    let rt = if sa > 0 && sa + 1 < t.len() {
        let v = |i: usize| sig.ys[i] - bl(t[i]);
        vertex([t[sa - 1], t[sa], t[sa + 1]], [v(sa - 1), v(sa), v(sa + 1)]).unwrap_or(t[sa])
    } else {
        t[sa]
    };
    let (l10, r10, _, _) = crossings(sig, s, &bl, 0.10);
    let (l05, r05, _, _) = crossings(sig, s, &bl, 0.05);
    let w = |a: Option<f64>, c: Option<f64>| Some(c? - a?);
    let w50 = w(l50, r50);
    let w05 = w(l05, r05);
    let tailing = match (w05, l05) {
        (Some(w), Some(l)) if rt > l => Some(w / (2.0 * (rt - l))),
        _ => None,
    };
    let asym = match (l10, r10) {
        (Some(l), Some(r)) if rt > l => Some((r - rt) / (rt - l)),
        _ => None,
    };
    Peak {
        number: 0,
        rt_min: rt,
        start_min: t[s.l],
        end_min: t[s.r],
        height,
        area,
        area_percent: 0.0,
        width_half_min: w50,
        width_10pct_min: w(l10, r10),
        width_5pct_min: w05,
        width_base_min: t[s.r] - t[s.l],
        tailing_factor: tailing,
        asymmetry_factor: asym,
        plates: w50.filter(|w| *w > 0.0).map(|w| 5.54 * (rt / w).powi(2)),
        resolution: None,
        snr: (sig.sigma > 0.0).then(|| height / sig.sigma),
        points: (s.r - s.l + 1) as u32,
        baseline_code: code.iter().collect(),
        baseline_start: b.at(t[s.l]),
        baseline_end: b.at(t[s.r]),
        baseline_reason: None,
    }
}

fn finish_table(method: PeakMethod, peaks: Vec<Peak>) -> PeakTable {
    let total: f64 = peaks.iter().map(|p| p.area.max(0.0)).sum();
    let main = peaks
        .iter()
        .max_by(|a, b| a.area.total_cmp(&b.area).then(b.number.cmp(&a.number)));
    PeakTable {
        method,
        peak_count: peaks.len() as u32,
        total_area: total,
        main_peak: main.map(|p| p.number),
        main_peak_area_percent: main.map(|p| p.area_percent),
        peaks,
    }
}

/// Integrate between two given retention times (manual integration): the samples strictly
/// inside the range plus the signal linearly interpolated at `start` and `end` (clamped to the
/// chromatogram), a straight baseline from the (smoothed, as in [`find_peaks`]) signal at
/// `start` to that at `end`, or through `baseline` values when given. The peak's
/// `baseline_code` is `MM`; `area_percent` is left 0.
pub fn integrate_range(
    t: &[f64],
    y: &[f64],
    start_min: f64,
    end_min: f64,
    baseline: Option<(f64, f64)>,
    p: &PeakParams,
) -> Result<Peak> {
    validate(p)?;
    if !(start_min.is_finite() && end_min.is_finite() && start_min < end_min) {
        return Err(Error::Usage(
            "integration range must be two retention times, start < end (minutes)".into(),
        ));
    }
    let (t, y) = clean(t, y, None)?;
    let (sigma, _) = match p.noise {
        Some(v) => (v, NoiseMethod::User),
        None => noise_sigma(&y),
    };
    let window = match p.smooth {
        Some(w) if w >= 5 => (w as usize) | 1,
        Some(_) => 1,
        None => auto_window(characteristic_width(&y, sigma)),
    };
    let ys = savgol(&y, window);
    let (Some(&t_first), Some(&t_last)) = (t.first(), t.last()) else {
        return Err(Error::Usage("the chromatogram has no samples".into()));
    };
    if end_min <= t_first || start_min >= t_last {
        return Err(Error::Usage(format!(
            "no samples between {start_min} and {end_min} min (the chromatogram spans {t_first}–{t_last} min)"
        )));
    }
    // The range exactly: samples inside it, plus the signal interpolated at its two ends
    // (clamped to the chromatogram).
    let (a, z) = (start_min.max(t_first), end_min.min(t_last));
    let at = |v: &[f64], x: f64| -> f64 {
        let i = t.partition_point(|&q| q < x);
        if i == 0 {
            return v[0];
        }
        if i >= t.len() {
            return v[t.len() - 1];
        }
        let (t0, t1) = (t[i - 1], t[i]);
        if t1 == t0 {
            v[i]
        } else {
            v[i - 1] + (v[i] - v[i - 1]) * (x - t0) / (t1 - t0)
        }
    };
    let mut tt = vec![a];
    let mut yy = vec![at(&y, a)];
    let mut ss = vec![at(&ys, a)];
    for (i, &x) in t.iter().enumerate() {
        if x > a && x < z {
            tt.push(x);
            yy.push(y[i]);
            ss.push(ys[i]);
        }
    }
    tt.push(z);
    yy.push(at(&y, z));
    ss.push(at(&ys, z));
    let r = tt.len() - 1;
    let sig = Signal {
        t: tt,
        y: yy,
        ys: ss,
        sigma,
    };
    let (b0, b1) = baseline.unwrap_or((sig.ys[0], sig.ys[r]));
    let b = Base {
        t0: a,
        b0,
        t1: z,
        b1,
    };
    let s = Span {
        apex: 0,
        l: 0,
        r,
        sloped: [false; 2],
    };
    let mut peak = measure(&sig, &s, &b, ['M', 'M'], p.area_time_unit.factor());
    peak.number = 1;
    Ok(peak)
}

/// How [`pick`] chooses among the peaks inside a retention-time window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PickRule {
    /// The tallest peak whose apex lies in the window.
    #[default]
    Largest,
    /// The peak whose apex is closest to the expected retention time.
    Nearest,
}

impl std::str::FromStr for PickRule {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "largest" | "tallest" => Ok(PickRule::Largest),
            "nearest" | "closest" => Ok(PickRule::Nearest),
            _ => Err(Error::Usage(format!(
                "unknown pick rule `{s}`: use largest or nearest"
            ))),
        }
    }
}

/// The peak of `table` for an expected retention time `rt ± window` (minutes).
pub fn pick(table: &PeakTable, rt: f64, window: f64, rule: PickRule) -> Option<&Peak> {
    let inside = table
        .peaks
        .iter()
        .filter(|p| (p.rt_min - rt).abs() <= window);
    match rule {
        PickRule::Largest => inside.max_by(|a, b| {
            a.height
                .total_cmp(&b.height)
                .then((b.rt_min - rt).abs().total_cmp(&(a.rt_min - rt).abs()))
        }),
        PickRule::Nearest => inside.min_by(|a, b| {
            (a.rt_min - rt)
                .abs()
                .total_cmp(&(b.rt_min - rt).abs())
                .then(b.height.total_cmp(&a.height))
        }),
    }
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;

    fn gauss(t: f64, rt: f64, sd: f64, h: f64) -> f64 {
        h * (-0.5 * ((t - rt) / sd).powi(2)).exp()
    }

    fn noise(seed: &mut u64) -> f64 {
        let mut s = 0.0;
        for _ in 0..12 {
            *seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            s += (*seed >> 11) as f64 / (1u64 << 53) as f64;
        }
        s - 6.0
    }

    fn axis(n: usize, dt: f64) -> Vec<f64> {
        (0..n).map(|i| i as f64 * dt).collect()
    }

    #[test]
    fn single_gaussian_area_height_width() {
        let t = axis(3000, 0.01); // 30 min at 0.6 s
        let (rt, sd, h) = (12.0, 0.05, 100.0);
        let y: Vec<f64> = t.iter().map(|&x| gauss(x, rt, sd, h) + 5.0).collect();
        let tab = find_peaks(&t, &y, &PeakParams::default()).unwrap();
        assert_eq!(tab.peak_count, 1, "{tab:?}");
        let p = &tab.peaks[0];
        let area = h * sd * (2.0 * std::f64::consts::PI).sqrt();
        assert!((p.area - area).abs() / area < 0.005, "{} vs {area}", p.area);
        assert!((p.height - h).abs() < 0.5, "{}", p.height);
        assert!((p.rt_min - rt).abs() < 0.001, "{}", p.rt_min);
        let fwhm = 2.354_820_045 * sd;
        assert!((p.width_half_min.unwrap() - fwhm).abs() / fwhm < 0.01);
        assert!((p.tailing_factor.unwrap() - 1.0).abs() < 0.02);
        assert!((p.asymmetry_factor.unwrap() - 1.0).abs() < 0.02);
        assert_eq!(p.baseline_code, "BB");
        assert!((p.area_percent - 100.0).abs() < 1e-9);
        assert_eq!(tab.main_peak, Some(1));
        let plates = 5.54 * (rt / fwhm).powi(2);
        assert!((p.plates.unwrap() - plates).abs() / plates < 0.02);
    }

    #[test]
    fn noisy_peaks_and_snr_threshold() {
        let t = axis(6000, 0.005);
        let mut seed = 7;
        let y: Vec<f64> = t
            .iter()
            .map(|&x| {
                gauss(x, 5.0, 0.04, 200.0)
                    + gauss(x, 12.0, 0.06, 20.0)
                    + gauss(x, 20.0, 0.05, 3.0) // S/N 3: at the threshold edge
                    + 0.01 * x
                    + noise(&mut seed)
            })
            .collect();
        let tab = find_peaks(&t, &y, &PeakParams::default()).unwrap();
        let rts: Vec<f64> = tab.peaks.iter().map(|p| p.rt_min).collect();
        assert!(rts.iter().any(|r| (r - 5.0).abs() < 0.01), "{rts:?}");
        assert!(rts.iter().any(|r| (r - 12.0).abs() < 0.01), "{rts:?}");
        // no noise peaks at S/N 10
        let mut p10 = PeakParams::default();
        p10.min_snr = 10.0;
        let tab10 = find_peaks(&t, &y, &p10).unwrap();
        assert_eq!(tab10.peak_count, 2, "{:?}", tab10.peaks);
        let big = &tab10.peaks[0];
        let area = 200.0 * 0.04 * (2.0 * std::f64::consts::PI).sqrt();
        assert!(
            (big.area - area).abs() / area < 0.02,
            "{} vs {area}",
            big.area
        );
        assert!((tab10.method.noise - 1.0).abs() < 0.1);
        assert!(big.snr.unwrap() > 150.0);
    }

    #[test]
    fn fused_pair_drop_line_partitions_the_area() {
        // realistic detector noise (σ = 0.05): the peak ends are found where the signal meets it
        let t = axis(2000, 0.005);
        let mut seed = 11;
        let y: Vec<f64> = t
            .iter()
            .map(|&x| {
                gauss(x, 4.0, 0.05, 100.0) + gauss(x, 4.2, 0.05, 60.0) + 0.05 * noise(&mut seed)
            })
            .collect();
        let tab = find_peaks(&t, &y, &PeakParams::default()).unwrap();
        assert_eq!(tab.peak_count, 2, "{:?}", tab.peaks);
        let (a, b) = (&tab.peaks[0], &tab.peaks[1]);
        assert_eq!(a.baseline_code, "BV");
        assert_eq!(b.baseline_code, "VB");
        assert!((a.end_min - b.start_min).abs() < 1e-12);
        let total = (100.0 + 60.0) * 0.05 * (2.0 * std::f64::consts::PI).sqrt();
        assert!(((a.area + b.area) - total).abs() / total < 0.01);
        let rs = b.resolution.unwrap();
        let expect = 1.18 * 0.2 / (2.0 * 2.3548 * 0.05);
        assert!((rs - expect).abs() / expect < 0.1, "{rs} vs {expect}");
        // valley-to-valley gives less area than drop lines
        let mut pv = PeakParams::default();
        pv.baseline = BaselineMode::Valley;
        let tv = find_peaks(&t, &y, &pv).unwrap();
        assert_eq!(tv.peaks.len(), 2, "{:?}", tv.peaks);
        assert_eq!(tv.peaks[0].baseline_code, "BV");
        assert!(tv.peaks[0].area + tv.peaks[1].area < a.area + b.area);
    }

    #[test]
    fn tangent_skims_a_rider() {
        let t = axis(3000, 0.005);
        let y: Vec<f64> = t
            .iter()
            .map(|&x| {
                // tailing parent (exponentially modified) and a small rider on its tail
                let parent = if x < 5.0 {
                    gauss(x, 5.0, 0.05, 100.0)
                } else {
                    100.0 * (-(x - 5.0) / 0.3).exp()
                };
                parent + gauss(x, 5.6, 0.03, 4.0)
            })
            .collect();
        let mut p = PeakParams::default();
        p.baseline = BaselineMode::Tangent;
        let tab = find_peaks(&t, &y, &p).unwrap();
        assert_eq!(tab.peak_count, 2, "{:?}", tab.peaks);
        assert_eq!(tab.peaks[1].baseline_code, "TT");
        let mut pd = PeakParams::default();
        pd.baseline = BaselineMode::Drop;
        let drop = find_peaks(&t, &y, &pd).unwrap();
        // total area is the same, the rider is smaller when skimmed
        let sum = |x: &PeakTable| x.peaks.iter().map(|q| q.area).sum::<f64>();
        assert!((sum(&tab) - sum(&drop)).abs() / sum(&drop) < 1e-9);
        assert!(tab.peaks[1].area < drop.peaks[1].area);
        // a straight skim line cuts into the rider's foot: it keeps most, never more, of its area
        let rider = 4.0 * 0.03 * (2.0 * std::f64::consts::PI).sqrt();
        let got = tab.peaks[1].area;
        assert!(got > 0.5 * rider && got < 1.05 * rider, "{got} vs {rider}");
        assert!(tab.peaks[0].end_min > tab.peaks[1].end_min);
    }

    #[test]
    fn sparse_xic_like_signal() {
        // few points per peak, zeros between: no smoothing, noise from the non-zero steps
        let t = axis(400, 0.02);
        let mut y = vec![0.0; 400];
        for (i, v) in [(100, 1e4), (101, 5e4), (102, 9e4), (103, 6e4), (104, 2e4)] {
            y[i] = v;
        }
        y[300] = 800.0;
        let tab = find_peaks(&t, &y, &PeakParams::default()).unwrap();
        assert_eq!(tab.method.smooth_window_points, 1);
        let main = &tab.peaks[tab.main_peak.unwrap() as usize - 1];
        assert!((main.rt_min - 2.04).abs() < 0.01, "{}", main.rt_min);
        let area = trapezoid(&t, 99, 105, |i| y[i]);
        assert!((main.area - area).abs() < 1e-6, "{} vs {area}", main.area);
        assert!((main.height - 9e4).abs() < 1e-6);
    }

    #[test]
    fn manual_integration_and_pick() {
        let t = axis(1000, 0.01);
        let y: Vec<f64> = t.iter().map(|&x| gauss(x, 3.0, 0.05, 50.0) + 1.0).collect();
        let mut p = PeakParams::default();
        p.smooth = Some(0);
        let pk = integrate_range(&t, &y, 2.5, 3.5, None, &p).unwrap();
        let area = 50.0 * 0.05 * (2.0 * std::f64::consts::PI).sqrt();
        assert!((pk.area - area).abs() / area < 1e-3);
        assert_eq!(pk.baseline_code, "MM");
        let pk0 = integrate_range(&t, &y, 2.5, 3.5, Some((0.0, 0.0)), &p).unwrap();
        assert!((pk0.area - (area + 1.0)).abs() / area < 1e-3);
        let mut ps = PeakParams::default();
        ps.area_time_unit = AreaTimeUnit::S;
        let tab = find_peaks(&t, &y, &ps).unwrap();
        assert!((tab.peaks[0].area / 60.0 - area).abs() / area < 0.01);
        assert!(pick(&tab, 3.05, 0.1, PickRule::Largest).is_some());
        assert!(pick(&tab, 4.0, 0.1, PickRule::Nearest).is_none());
        assert!(integrate_range(&t, &y, 3.5, 2.5, None, &p).is_err());
    }

    fn with(mode: BaselineMode) -> PeakParams {
        let mut p = PeakParams::default();
        p.baseline = mode;
        p
    }

    /// A GC run: a large solvent peak with a long, slowly decaying tail (its slope within the
    /// noise of the slope where the riders sit), small peaks riding on the tail, and a small
    /// noise bump on the tail between them: the case the drop-line baseline gets badly wrong.
    fn solvent_tail(seed: u64) -> (Vec<f64>, Vec<f64>, [(f64, f64); 3]) {
        let t = axis(12_000, 0.001); // 12 min at 0.06 s
        let riders = [(4.0, 40.0), (6.0, 25.0), (8.5, 10.0)]; // (rt, height), σ 0.01 min
        let mut s = seed;
        let y: Vec<f64> = t
            .iter()
            .map(|&x| {
                let solvent = gauss(x, 1.0, 0.02, 5000.0).max(if x > 1.0 {
                    150.0 * (-(x - 1.0) / 2.0).exp()
                } else {
                    0.0
                });
                let r: f64 = riders.iter().map(|&(rt, h)| gauss(x, rt, 0.01, h)).sum();
                solvent + r + gauss(x, 5.0, 0.01, 0.3) + 2.0 + 0.02 * noise(&mut s)
            })
            .collect();
        let areas = riders.map(|(rt, h)| (rt, h * 0.01 * (2.0 * std::f64::consts::PI).sqrt()));
        (t, y, areas)
    }

    #[test]
    fn auto_ends_riders_where_they_meet_a_solvent_tail() {
        let (t, y, truth) = solvent_tail(3);
        let auto = find_peaks(&t, &y, &with(BaselineMode::Auto)).unwrap();
        let drop = find_peaks(&t, &y, &with(BaselineMode::Drop)).unwrap();
        for (rt, area) in truth {
            let near = |tab: &PeakTable| {
                tab.peaks
                    .iter()
                    .find(|q| (q.rt_min - rt).abs() < 0.01)
                    .cloned()
                    .unwrap_or_else(|| panic!("no peak at {rt}: {:?}", tab.peaks))
            };
            let a = near(&auto);
            assert!(
                (a.area - area).abs() / area < 0.05,
                "auto {rt}: {} vs {area} ({:?})",
                a.area,
                a
            );
            assert!(a.width_base_min < 0.2, "{a:?}");
            let d = near(&drop);
            assert!(d.baseline_reason.is_none());
            if rt == 4.0 {
                // on the steep part of the tail the drop-line walk follows the tail down to the
                // next bump, and its baseline cuts through the tail; auto ends the rider where
                // it meets the tail and says so
                assert_eq!(a.baseline_reason.as_deref(), Some("sloped_background"));
                assert!(d.end_min > 4.5, "{d:?}");
                assert!(
                    (d.area - area).abs() > 0.2 * area,
                    "drop {rt}: {} vs {area}",
                    d.area
                );
            }
        }
        assert!(auto.method.slope_noise.is_some_and(|v| v > 0.0));
        assert_eq!(auto.method.baseline, "auto");
    }

    #[test]
    fn auto_equals_drop_on_a_flat_baseline() {
        // fused pair and an isolated peak on a flat, noisy baseline: nothing to follow
        let t = axis(4000, 0.005);
        let mut seed = 11;
        let y: Vec<f64> = t
            .iter()
            .map(|&x| {
                gauss(x, 4.0, 0.05, 100.0)
                    + gauss(x, 4.2, 0.05, 60.0)
                    + gauss(x, 12.0, 0.05, 80.0)
                    + 0.05 * noise(&mut seed)
            })
            .collect();
        let auto = find_peaks(&t, &y, &with(BaselineMode::Auto)).unwrap();
        let drop = find_peaks(&t, &y, &with(BaselineMode::Drop)).unwrap();
        assert_eq!(auto.peak_count, 3, "{:?}", auto.peaks);
        assert_eq!(auto.peak_count, drop.peak_count);
        for (a, d) in auto.peaks.iter().zip(&drop.peaks) {
            assert!((a.area - d.area).abs() / d.area < 0.005, "{a:?} vs {d:?}");
            assert_eq!(a.baseline_code, d.baseline_code);
        }
        let reasons: Vec<&str> = auto
            .peaks
            .iter()
            .map(|q| q.baseline_reason.as_deref().unwrap_or(""))
            .collect();
        assert_eq!(reasons, ["drop_line", "drop_line", "isolated"]);
        let total = (100.0 + 60.0) * 0.05 * (2.0 * std::f64::consts::PI).sqrt();
        assert!(((auto.peaks[0].area + auto.peaks[1].area) - total).abs() / total < 0.01);
    }

    #[test]
    fn auto_keeps_a_broad_peak_with_noise_on_its_top() {
        // narrow peaks set the characteristic width; a broad low peak between them must not be
        // cut into pieces at the noise on its top
        let t = axis(6000, 0.005);
        let mut seed = 5;
        let y: Vec<f64> = t
            .iter()
            .map(|&x| {
                gauss(x, 5.0, 0.02, 100.0)
                    + gauss(x, 15.0, 0.1, 5.0)
                    + gauss(x, 25.0, 0.02, 100.0)
                    + 0.01 * noise(&mut seed)
            })
            .collect();
        let auto = find_peaks(&t, &y, &with(BaselineMode::Auto)).unwrap();
        let broad = auto
            .peaks
            .iter()
            .find(|q| (q.rt_min - 15.0).abs() < 0.1)
            .unwrap_or_else(|| panic!("{:?}", auto.peaks));
        let area = 5.0 * 0.1 * (2.0 * std::f64::consts::PI).sqrt();
        assert!((broad.area - area).abs() / area < 0.05, "{broad:?}");
    }

    #[test]
    fn baseline_mode_names() {
        assert_eq!(PeakParams::default().baseline, BaselineMode::Auto);
        for m in [
            BaselineMode::Auto,
            BaselineMode::Drop,
            BaselineMode::Valley,
            BaselineMode::Tangent,
        ] {
            assert_eq!(m.id().parse::<BaselineMode>().unwrap(), m);
        }
        assert!("curved".parse::<BaselineMode>().is_err());
    }

    #[test]
    fn degenerate_inputs_do_not_panic() {
        let p = PeakParams::default();
        assert_eq!(find_peaks(&[], &[], &p).unwrap().peak_count, 0);
        assert_eq!(find_peaks(&[1.0], &[1.0], &p).unwrap().peak_count, 0);
        assert!(find_peaks(&[1.0, 2.0], &[1.0], &p).is_err());
        let t = [0.0, 1.0, 1.0, 2.0, f64::NAN, 3.0];
        let y = [0.0, 5.0, 6.0, 0.0, 1.0, f64::INFINITY];
        let _ = find_peaks(&t, &y, &p).unwrap();
        let flat = vec![3.0; 100];
        let tt = axis(100, 0.1);
        assert_eq!(find_peaks(&tt, &flat, &p).unwrap().peak_count, 0);
        let mut bad = PeakParams::default();
        bad.min_snr = -1.0;
        assert!(find_peaks(&tt, &flat, &bad).is_err());
        // unsorted input is sorted
        let y2: Vec<f64> = tt.iter().map(|&x| gauss(x, 5.0, 0.3, 10.0)).collect();
        let mut pairs: Vec<(f64, f64)> = tt.iter().copied().zip(y2).collect();
        pairs.reverse();
        let (a, b): (Vec<f64>, Vec<f64>) = pairs.into_iter().unzip();
        assert_eq!(find_peaks(&a, &b, &p).unwrap().peak_count, 1);
    }
}
