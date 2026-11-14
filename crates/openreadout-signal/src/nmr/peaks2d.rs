//! Peak picking and volume integration on 2D (and higher) spectra stored as a row-major matrix,
//! one [`PpmAxis`] per dimension, the slowest dimension first (F1, F2 for a 2D spectrum).
//!
//! [`pick_cross_peaks`] takes plain inputs, so it works on a stored spectrum
//! ([`crate::nmr::source2d`]) and on any matrix computed elsewhere.
//!
//! **Noise.** [`matrix_noise_sd`]: runs of 64 points along the direct (last) dimension, at most
//! 16384 of them spread evenly over the matrix; in each, a least-squares line is removed and
//! `1.4826 · MAD` of the residual taken; the noise SD is the 25th percentile over runs (as the
//! 1D [`crate::nmr::noise_sd`]) times [`NOISE_P25_SCALE`], which makes it unbiased on Gaussian
//! noise. Runs that are exactly constant (blanked regions) are skipped. Data without negative
//! values are magnitude-mode spectra: their noise floor is Rayleigh-distributed, and the noise
//! SD is multiplied by [`MAGNITUDE_NOISE_SCALE`] so it describes the noise on a line.
//! **Noise from the maxima**: processed 2D spectra often have noise with heavier tails than
//! Gaussian (non-uniform sampling, t1 noise, truncation wiggles). The 90th percentile of the
//! heights of all local maxima of |value| (nearly all of them noise) divided by its value for
//! Gaussian noise ([`MAXIMA_P90_RATIO`]) gives a second estimate; the larger of the two is the
//! noise SD the thresholds use.
//! **Line noise**: for every line of the matrix along each dimension (every column and every row
//! of a 2D spectrum), `1.4826 · MAD` about the line's median. The local noise of a point is the
//! largest of the noise SD and the noise of the lines through it, so a column of t1 noise under
//! a strong signal, or the residual solvent column, raises the bar for the points on it.
//!
//! **Maxima.** A point is a maximum when it is at least as high as all of its 3ᴺ − 1 neighbours
//! (strictly higher than the ones before it in memory order, so a plateau gives one maximum),
//! is not on the edge of the matrix, and has height ≥ `min_snr` × noise SD and
//! ≥ `min_height_fraction` × the largest absolute value in the searched range. Its prominence
//! along each dimension (height above the higher of the two lowest points reached walking along
//! that dimension to a higher point or the edge, as in 1D) must be ≥ `min_prominence_snr` × the
//! noise SD: this rejects ripples riding on a larger line. With `include_negative` the same is
//! done on the negated matrix. A maximum that passes these tests but is below `min_local_snr` ×
//! its local noise (or its prominence below the prominence test scaled the same way) sits on a
//! noise ridge: it is left out and counted (`ridge_suppressed`), or kept and flagged with
//! `keep_ridge_peaks`. Kept maxima on a line whose noise is ≥ `ridge_factor` × the noise SD and
//! less than 2 × `min_snr` × their local noise are flagged `ridge` too.
//!
//! **Position, height, width.** Per dimension, the parabola through the maximum and its two
//! neighbours along that dimension gives the sub-point position; the height is the maximum
//! value plus the parabolas' corrections. The full width at half height per dimension is found
//! by walking along that dimension to the first points below half height (linear
//! interpolation); `None` when the signal rises again first (overlap).
//!
//! **Cross peaks.** Maxima of either sign closer than `group_gap_hz` (or `group_gap_points`,
//! whichever is more points) in every dimension belong to one cross peak (single linkage), so a
//! multiplet's fine structure, the antiphase lobes of a phase-sensitive COSY or a split line
//! form one cross peak with its member count. A cross peak's position (`ppm`) is the
//! |height|-weighted mean of its members' positions; its apex is its tallest member.
//!
//! **Footprints and volumes.** A cross peak's footprint is the set of points reached from its
//! members by flooding outwards, highest point first, through face neighbours of the same sign
//! whose height is ≥ `footprint_snr` × their local noise; where two floods meet the first to
//! arrive keeps the point (a watershed). The volume is the sum of the footprint's values times
//! the cell size (the product of the point spacings in ppm): intensity·ppmᴺ. Cross peaks with
//! fewer than `min_footprint_points` footprint points are isolated spikes (artefacts of
//! non-uniform-sampling reconstruction): they are left out and counted. `max_peaks` then keeps
//! the tallest.
//! [`integrate_regions`] sums boxes given in ppm, normalised to a reference box as the 1D
//! integrals are.
//!
//! **Annotations** ([`annotate`]). When two dimensions observe the same nucleus over
//! overlapping ranges (COSY, TOCSY, NOESY, ROESY), cross peaks within `diagonal_tolerance_hz`
//! of the diagonal are flagged `diagonal`; every other cross peak is paired with the cross peak
//! nearest its mirror image, within `symmetry_tolerance_hz` (`symmetric_partner`,
//! `partner_height_ratio`), or flagged `partner_missing`. Cross peaks on a line whose noise is
//! ≥ `ridge_factor` × the noise SD are flagged `ridge`; cross peaks within a point of an
//! indirect dimension's carrier are flagged `axial`. Every cross peak has a `sign`.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use openreadout_core::{Error, Result};

use crate::nmr::process::PpmAxis;

/// Length of the runs used by [`matrix_noise_sd`].
const NOISE_RUN: usize = 64;
/// The 25th percentile of the run estimates is this much below the noise SD on Gaussian noise
/// (measured with 64-point runs: `noise_estimate_is_close_to_the_true_sd`).
pub const NOISE_P25_SCALE: f64 = 1.12;
/// Magnitude-mode data (no negative values): the noise floor is Rayleigh-distributed and its
/// robust spread is 0.66 of the noise riding on a signal, so the noise SD is scaled by 1/0.66.
pub const MAGNITUDE_NOISE_SCALE: f64 = 1.52;
/// 90th percentile of the heights of the local maxima of |noise| (3ᴺ − 1 neighbours) in units of
/// the noise SD, 2D Gaussian noise (2.39 to 2.53 from white to strongly correlated noise,
/// measured by simulation); per extra dimension add [`MAXIMA_P90_PER_DIMENSION`].
pub const MAXIMA_P90_RATIO: f64 = 2.5;
/// Increase of [`MAXIMA_P90_RATIO`] per dimension beyond two (2.88 measured in 3D).
pub const MAXIMA_P90_PER_DIMENSION: f64 = 0.4;
/// The same percentile for magnitude-mode noise, in units of the scaled noise SD (2.95).
pub const MAXIMA_P90_MAGNITUDE: f64 = 2.95;
/// Most points [`maxima_p90`] examines (whole rows, spread evenly).
const MAXIMA_SAMPLE_POINTS: usize = 1 << 22;
/// Most runs sampled by [`matrix_noise_sd`].
const NOISE_RUNS_MAX: usize = 16384;
/// Most points all footprints together may hold (a guard against floods through broad signal).
pub const MAX_FOOTPRINT_POINTS: usize = 1 << 26;
/// Largest number of dimensions [`pick_cross_peaks`] accepts.
pub const MAX_DIMENSIONS: usize = 4;

/// Thresholds and annotation tolerances for [`pick_cross_peaks`] and [`annotate`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct CrossPeakOptions {
    /// Minimum height as a multiple of the noise SD.
    pub min_snr: f64,
    /// Minimum height as a multiple of the local noise (the noise of the lines through the
    /// maximum, when larger than the noise SD); the local prominence test scales by
    /// `min_local_snr / min_snr`.
    pub min_local_snr: f64,
    /// Minimum height as a fraction of the largest absolute value in the searched range
    /// (0 = off).
    pub min_height_fraction: f64,
    /// Minimum prominence along every dimension as a multiple of the noise SD (and of the
    /// local noise).
    pub min_prominence_snr: f64,
    /// Also report negative peaks (edited HSQC CH2, phase-sensitive NOESY/ROESY/COSY).
    pub include_negative: bool,
    /// Only search these shift ranges, one per dimension, slowest first (ppm, either order;
    /// `None` or a missing entry = the whole dimension).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ranges_ppm: Vec<Option<(f64, f64)>>,
    /// Leave out cross peaks whose footprint has fewer points (isolated spikes).
    pub min_footprint_points: usize,
    /// Keep at most this many cross peaks (the tallest).
    pub max_peaks: usize,
    /// Maxima closer than this in every dimension belong to one cross peak, Hz.
    pub group_gap_hz: f64,
    /// ... or closer than this many points, when that is more.
    pub group_gap_points: f64,
    /// Footprint floor as a multiple of the local noise.
    pub footprint_snr: f64,
    /// A line whose noise is at least this multiple of the noise SD is a noise ridge.
    pub ridge_factor: f64,
    /// A cross peak on the indirect-dimension line of one this many times taller is flagged
    /// `ridge` (t1 noise, truncation wiggles), unless a homonuclear spectrum gives it a
    /// symmetric partner.
    pub ridge_ratio: f64,
    /// Keep maxima that pass the global threshold but not the local one (flagged `ridge`).
    pub keep_ridge_peaks: bool,
    /// Homonuclear spectra: cross peaks this close to the diagonal are diagonal peaks, Hz.
    pub diagonal_tolerance_hz: f64,
    /// Homonuclear spectra: largest distance of a symmetric partner from the mirror image, Hz.
    pub symmetry_tolerance_hz: f64,
    /// Also list every maximum (`maxima`), with the cross peak it belongs to.
    pub list_maxima: bool,
}

impl Default for CrossPeakOptions {
    fn default() -> Self {
        Self {
            min_snr: 10.0,
            min_local_snr: 6.0,
            min_height_fraction: 0.0,
            min_prominence_snr: 5.0,
            include_negative: true,
            ranges_ppm: Vec::new(),
            min_footprint_points: 3,
            max_peaks: 1000,
            group_gap_hz: 15.0,
            group_gap_points: 1.5,
            footprint_snr: 3.0,
            ridge_factor: 3.0,
            ridge_ratio: 20.0,
            keep_ridge_peaks: false,
            diagonal_tolerance_hz: 15.0,
            symmetry_tolerance_hz: 25.0,
            list_maxima: false,
        }
    }
}

/// Sign of a cross peak's members.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PeakSign {
    /// All members positive.
    Positive,
    /// All members negative.
    Negative,
    /// Positive and negative members (antiphase multiplets).
    Mixed,
}

/// What [`annotate`] and the picker noticed about a cross peak.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PeakFlag {
    /// On the diagonal of a homonuclear spectrum.
    Diagonal,
    /// Off the diagonal of a homonuclear spectrum with no cross peak at its mirror image.
    PartnerMissing,
    /// On a line of raised noise (t1 noise under a strong signal, a solvent column).
    Ridge,
    /// At the carrier of an indirect dimension (a possible axial peak).
    Axial,
}

/// One maximum (the member view).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Maximum {
    /// Interpolated position per dimension, ppm (slowest first).
    pub ppm: Vec<f64>,
    /// Point per dimension.
    pub index: Vec<usize>,
    /// Interpolated height (negative for negative maxima).
    pub height: f64,
    /// |height| / noise SD (`None` when the noise SD is 0).
    pub snr: Option<f64>,
    /// Index of the cross peak it belongs to in `peaks`.
    pub peak: usize,
}

/// One cross peak: a group of maxima with its footprint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CrossPeak {
    /// Position per dimension, ppm: the |height|-weighted mean of the members (slowest first).
    pub ppm: Vec<f64>,
    /// The same position in Hz from 0 ppm.
    pub hz: Vec<f64>,
    /// Interpolated position of the apex (the tallest member), ppm.
    pub apex_ppm: Vec<f64>,
    /// Point of the apex per dimension.
    pub index: Vec<usize>,
    /// Interpolated height of the apex (negative for a negative apex).
    pub height: f64,
    /// Sign of the members.
    pub sign: PeakSign,
    /// |height| / noise SD (`None` when the noise SD is 0).
    pub snr: Option<f64>,
    /// |height| / the local noise at the apex (`None` when it is 0).
    pub local_snr: Option<f64>,
    /// Full width at half height of the apex per dimension, Hz (`None` where an overlapping
    /// line hides the half-height point).
    pub width_hz: Vec<Option<f64>>,
    /// The same widths in ppm.
    pub width_ppm: Vec<Option<f64>>,
    /// Number of maxima in the cross peak.
    pub maxima: usize,
    /// Sum of the footprint's values × the cell size (intensity·ppmᴺ).
    pub volume: f64,
    /// Sum of the footprint's absolute values × the cell size.
    pub absolute_volume: f64,
    /// Points in the footprint.
    pub footprint_points: usize,
    /// Extent of the footprint per dimension (high ppm, low ppm).
    pub footprint_ppm: Vec<(f64, f64)>,
    /// Annotations.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<PeakFlag>,
    /// Homonuclear spectra: index in `peaks` of the cross peak at the mirror image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symmetric_partner: Option<usize>,
    /// |height| / |partner height|.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partner_height_ratio: Option<f64>,
}

impl CrossPeak {
    /// Does it carry `flag`?
    pub fn has(&self, flag: PeakFlag) -> bool {
        self.flags.contains(&flag)
    }
}

/// What [`pick_cross_peaks`] found.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CrossPeakPick {
    /// Cross peaks, high shift of the direct dimension first, then high shift of the slower ones.
    pub peaks: Vec<CrossPeak>,
    /// Every maximum kept (filled when `list_maxima`; `maxima_count` counts them anyway).
    pub maxima: Vec<Maximum>,
    /// Number of maxima kept.
    pub maxima_count: usize,
    /// Noise SD the thresholds and `snr` use: the larger of `run_noise_sd` and
    /// `maxima_noise_sd` (module docs).
    pub noise_sd: f64,
    /// Noise SD from runs along the direct dimension ([`matrix_noise_sd`]).
    pub run_noise_sd: f64,
    /// Noise SD implied by the 90th percentile of the local maxima's heights.
    pub maxima_noise_sd: f64,
    /// Maxima left out because they did not stand out from the noise of their line.
    pub ridge_suppressed: usize,
    /// Cross peaks left out for a footprint smaller than `min_footprint_points`.
    pub small_footprint_suppressed: usize,
    /// Remarks.
    pub notes: Vec<String>,
}

/// Row-major strides of `shape`.
fn strides(shape: &[usize]) -> Vec<usize> {
    let mut s = vec![1usize; shape.len()];
    for d in (0..shape.len().saturating_sub(1)).rev() {
        s[d] = s[d + 1] * shape[d + 1];
    }
    s
}

fn coords(mut idx: usize, shape: &[usize], out: &mut [usize]) {
    for d in (0..shape.len()).rev() {
        out[d] = idx % shape[d];
        idx /= shape[d];
    }
}

/// Number of points of `shape`, or an error when it overflows or does not match `len`.
fn check_shape(len: usize, shape: &[usize], axes: &[PpmAxis]) -> Result<()> {
    if shape.is_empty() || shape.len() > MAX_DIMENSIONS {
        return Err(Error::Usage(format!(
            "a spectrum matrix needs 1 to {MAX_DIMENSIONS} dimensions, got {}",
            shape.len()
        )));
    }
    if axes.len() != shape.len() {
        return Err(Error::Usage(format!(
            "{} axes for a {}-dimensional matrix",
            axes.len(),
            shape.len()
        )));
    }
    let n = shape
        .iter()
        .try_fold(1usize, |a, &b| a.checked_mul(b))
        .ok_or_else(|| Error::Usage("matrix shape overflows".into()))?;
    if n != len {
        return Err(Error::Usage(format!(
            "matrix of shape {shape:?} needs {n} values, got {len}"
        )));
    }
    Ok(())
}

fn median_in_place(v: &mut [f64]) -> f64 {
    let n = v.len();
    if n == 0 {
        return 0.0;
    }
    let (_, m, _) = v.select_nth_unstable_by(n / 2, f64::total_cmp);
    let hi = *m;
    if n % 2 == 1 {
        hi
    } else {
        let lo = v[..n / 2].iter().copied().fold(f64::NEG_INFINITY, f64::max);
        f64::midpoint(lo, hi)
    }
}

/// 1.4826 · MAD about the median.
fn mad_sd(v: &mut [f64]) -> f64 {
    let med = median_in_place(v);
    for x in v.iter_mut() {
        *x = (*x - med).abs();
    }
    1.4826 * median_in_place(v)
}

/// Robust noise SD of a row-major matrix whose last dimension has `row_len` points (module docs).
/// 0 when there are too few points.
pub fn matrix_noise_sd(data: &[f64], row_len: usize) -> f64 {
    if row_len < 8 || data.len() < 16 {
        return 0.0;
    }
    let run = NOISE_RUN.min(row_len);
    let per_row = row_len / run;
    let rows = data.len() / row_len;
    let total = per_row * rows;
    if total == 0 {
        return 0.0;
    }
    let step = total.div_ceil(NOISE_RUNS_MAX).max(1);
    let mut sds = Vec::with_capacity(total / step + 1);
    let mut dev = Vec::with_capacity(run);
    let mut k = 0;
    while k < total {
        let (r, j) = (k / per_row, k % per_row);
        let start = r * row_len + j * run;
        k += step;
        let Some(c) = data.get(start..start + run) else {
            continue;
        };
        if !c.iter().all(|v| v.is_finite()) {
            continue;
        }
        let m = c.len() as f64;
        let xm = (m - 1.0) / 2.0;
        let ym = c.iter().sum::<f64>() / m;
        let (mut sxy, mut sxx) = (0.0, 0.0);
        for (i, v) in c.iter().enumerate() {
            let dx = i as f64 - xm;
            sxy += dx * (v - ym);
            sxx += dx * dx;
        }
        let slope = if sxx > 0.0 { sxy / sxx } else { 0.0 };
        dev.clear();
        dev.extend(
            c.iter()
                .enumerate()
                .map(|(i, v)| v - ym - slope * (i as f64 - xm)),
        );
        let sd = mad_sd(&mut dev);
        if sd > 0.0 {
            sds.push(sd);
        }
    }
    if sds.is_empty() {
        return 0.0;
    }
    sds.sort_by(f64::total_cmp);
    sds[(sds.len() - 1) / 4] * NOISE_P25_SCALE
}

/// Noise of every line along each dimension: `out[d][line]`, where the line of a point along
/// `d` is `(idx / (stride_d · n_d)) · stride_d + idx % stride_d`. Lines shorter than 16 points
/// get 0 (no estimate).
fn line_noise(data: &[f64], shape: &[usize], st: &[usize]) -> Vec<Vec<f64>> {
    let n = data.len();
    let mut out = Vec::with_capacity(shape.len());
    let mut buf = Vec::new();
    for d in 0..shape.len() {
        let nd = shape[d];
        let lines = n / nd.max(1);
        if nd < 16 {
            out.push(vec![0.0; lines]);
            continue;
        }
        let sd = st[d];
        let mut v = Vec::with_capacity(lines);
        for line in 0..lines {
            let (hi, lo) = (line / sd, line % sd);
            let base = hi * sd * nd + lo;
            buf.clear();
            buf.extend(
                (0..nd)
                    .filter_map(|k| data.get(base + k * sd))
                    .filter(|x| x.is_finite()),
            );
            v.push(if buf.len() >= 8 { mad_sd(&mut buf) } else { 0.0 });
        }
        out.push(v);
    }
    out
}

/// Shared state of one picking run.
struct Grid<'a> {
    data: &'a [f64],
    shape: &'a [usize],
    st: Vec<usize>,
    sd: f64,
    lines: Vec<Vec<f64>>,
}

impl Grid<'_> {
    fn line_of(&self, idx: usize, d: usize) -> usize {
        let (sd, nd) = (self.st[d], self.shape[d]);
        (idx / (sd * nd)) * sd + idx % sd
    }
    /// Largest noise of the lines through `idx` (0 when no line has an estimate).
    fn line_sd(&self, idx: usize) -> f64 {
        (0..self.shape.len())
            .filter_map(|d| self.lines[d].get(self.line_of(idx, d)).copied())
            .fold(0.0, f64::max)
    }
    fn local_sd(&self, idx: usize) -> f64 {
        self.sd.max(self.line_sd(idx))
    }
    fn at(&self, idx: usize, sign: f64) -> f64 {
        self.data.get(idx).map_or(f64::NEG_INFINITY, |v| sign * v)
    }
    /// Values along dimension `d` through `idx`: (position of idx on the line, getter).
    fn walk(&self, idx: usize, d: usize, sign: f64) -> (usize, impl Fn(usize) -> f64 + '_) {
        let (sd, nd) = (self.st[d], self.shape[d]);
        let pos = (idx / sd) % nd;
        let base = idx - pos * sd;
        (pos, move |k: usize| {
            if k < nd {
                self.at(base + k * sd, sign)
            } else {
                f64::NEG_INFINITY
            }
        })
    }
    /// Prominence of `idx` along dimension `d` (1D rule).
    fn prominence(&self, idx: usize, d: usize, sign: f64) -> f64 {
        let nd = self.shape[d];
        let (pos, v) = self.walk(idx, d, sign);
        let h = v(pos);
        let mut left = h;
        for k in (0..pos).rev() {
            let x = v(k);
            if x > h {
                break;
            }
            left = left.min(x);
        }
        let mut right = h;
        for k in pos + 1..nd {
            let x = v(k);
            if x > h {
                break;
            }
            right = right.min(x);
        }
        h - left.max(right)
    }
    /// Full width at half of `h` along `d`, in points.
    fn half_width(&self, idx: usize, d: usize, sign: f64, h: f64) -> Option<f64> {
        let nd = self.shape[d];
        let (pos, v) = self.walk(idx, d, sign);
        let half = h / 2.0;
        if !(v(pos) >= half) {
            return None;
        }
        let mut k = pos;
        let left = loop {
            if k == 0 {
                return None;
            }
            let (cur, nxt) = (v(k), v(k - 1));
            if nxt < half {
                break (k - 1) as f64 + (half - nxt) / (cur - nxt);
            }
            if nxt > cur {
                return None;
            }
            k -= 1;
        };
        let mut k = pos;
        let right = loop {
            if k + 1 >= nd {
                return None;
            }
            let (cur, nxt) = (v(k), v(k + 1));
            if nxt < half {
                break k as f64 + (cur - half) / (cur - nxt);
            }
            if nxt > cur {
                return None;
            }
            k += 1;
        };
        Some(right - left)
    }
}

/// A maximum before grouping.
#[derive(Debug, Clone)]
struct Raw {
    idx: usize,
    pos: Vec<f64>,
    height: f64,
    ridge: bool,
    local_sd: f64,
}

/// Offsets of the 3ᴺ − 1 neighbours, as (per-dimension step in −1..=1) lists.
fn neighbour_steps(dims: usize) -> Vec<Vec<isize>> {
    let total = 3usize.pow(dims as u32);
    (0..total)
        .filter(|&k| k != total / 2)
        .map(|mut k| {
            let mut s = vec![0isize; dims];
            for x in s.iter_mut().rev() {
                *x = (k % 3) as isize - 1;
                k /= 3;
            }
            s
        })
        .collect()
}

/// Index window per dimension `[lo, hi]` (inclusive, off the edges) from `ranges_ppm`.
fn windows(shape: &[usize], axes: &[PpmAxis], ranges: &[Option<(f64, f64)>]) -> Vec<(usize, usize)> {
    shape
        .iter()
        .enumerate()
        .map(|(d, &n)| {
            let (lo, hi) = (1usize, n.saturating_sub(2));
            match ranges.get(d).copied().flatten() {
                Some((a, b)) => {
                    let (i, j) = (axes[d].index_of(a), axes[d].index_of(b));
                    let (i, j) = if i <= j { (i, j) } else { (j, i) };
                    let i = i.floor().clamp(lo as f64, hi as f64) as usize;
                    let j = j.ceil().clamp(lo as f64, hi as f64) as usize;
                    (i, j)
                }
                None => (lo, hi),
            }
        })
        .collect()
}

/// Calls `f` with the flat index of every point of the box `win` (odometer order).
fn for_box(win: &[(usize, usize)], st: &[usize], mut f: impl FnMut(usize)) {
    if win.iter().any(|&(a, b)| a > b) {
        return;
    }
    let mut c: Vec<usize> = win.iter().map(|w| w.0).collect();
    loop {
        f(c.iter().zip(st).map(|(a, b)| a * b).sum());
        let mut d = c.len();
        loop {
            if d == 0 {
                return;
            }
            d -= 1;
            if c[d] < win[d].1 {
                c[d] += 1;
                break;
            }
            c[d] = win[d].0;
        }
    }
}

fn in_ranges(pos: &[f64], axes: &[PpmAxis], ranges: &[Option<(f64, f64)>]) -> bool {
    pos.iter().enumerate().all(|(d, &p)| {
        ranges.get(d).copied().flatten().is_none_or(|(a, b)| {
            let ppm = axes[d].ppm(p);
            (a.min(b)..=a.max(b)).contains(&ppm)
        })
    })
}

struct UnionFind(Vec<usize>);
impl UnionFind {
    fn find(&mut self, mut x: usize) -> usize {
        while self.0[x] != x {
            self.0[x] = self.0[self.0[x]];
            x = self.0[x];
        }
        x
    }
    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.0[ra.max(rb)] = ra.min(rb);
        }
    }
}

/// Heap entry for the footprint flood: highest value first.
#[derive(PartialEq)]
struct Flood(f64, usize, u32);
impl Eq for Flood {}
impl PartialOrd for Flood {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Flood {
    fn cmp(&self, o: &Self) -> Ordering {
        self.0.total_cmp(&o.0).then(o.1.cmp(&self.1))
    }
}

/// 90th percentile of |value| over the local maxima of |value| (3ᴺ − 1 neighbours, off the
/// edges), on rows spread evenly so at most [`MAXIMA_SAMPLE_POINTS`] points are examined.
/// `None` with fewer than 100 maxima.
fn maxima_p90(data: &[f64], shape: &[usize], st: &[usize]) -> Option<f64> {
    let dims = shape.len();
    let row_len = shape[dims - 1];
    let rows = data.len() / row_len.max(1);
    let step = (data.len() / MAXIMA_SAMPLE_POINTS).max(1);
    let steps = neighbour_steps(dims);
    let offsets: Vec<isize> = steps
        .iter()
        .map(|s| s.iter().zip(st).map(|(a, &b)| a * b as isize).sum())
        .collect();
    let mut c = vec![0usize; dims];
    let mut h = Vec::new();
    let mut r = 0;
    while r < rows {
        let base = r * row_len;
        coords(base, shape, &mut c);
        r += step;
        if c[..dims - 1]
            .iter()
            .zip(shape)
            .any(|(&x, &n)| x == 0 || x + 1 >= n)
        {
            continue;
        }
        for k in 1..row_len.saturating_sub(1) {
            let i = base + k;
            let Some(v) = data.get(i).map(|x| x.abs()) else {
                continue;
            };
            if !(v > 0.0 && v.is_finite()) {
                continue;
            }
            if offsets.iter().all(|&o| {
                data.get((i as isize + o) as usize)
                    .is_none_or(|w| !(w.abs() > v))
            }) {
                h.push(v);
            }
        }
    }
    if h.len() < 100 {
        return None;
    }
    let k = h.len() * 9 / 10;
    let (_, x, _) = h.select_nth_unstable_by(k, f64::total_cmp);
    Some(*x)
}

/// Footprint sums of one cross peak.
struct Acc {
    vol: f64,
    abs: f64,
    n: usize,
    lo: Vec<usize>,
    hi: Vec<usize>,
}

/// Pick cross peaks in the row-major matrix `data` of `shape` (slowest dimension first), one
/// axis per dimension (module docs). Annotations other than `ridge` come from [`annotate`].
// exact ties decide which point of a plateau is its maximum; per-dimension loops index
// several parallel arrays
#[allow(clippy::float_cmp, clippy::needless_range_loop, clippy::too_many_lines)]
pub fn pick_cross_peaks(
    data: &[f64],
    shape: &[usize],
    axes: &[PpmAxis],
    opts: &CrossPeakOptions,
) -> Result<CrossPeakPick> {
    check_shape(data.len(), shape, axes)?;
    let dims = shape.len();
    let mut out = CrossPeakPick::default();
    let row_len = shape[dims - 1];
    let mut sd = matrix_noise_sd(data, row_len);
    let magnitude = data.iter().any(|v| *v > 0.0) && !data.iter().any(|v| *v < 0.0);
    if magnitude {
        sd *= MAGNITUDE_NOISE_SCALE;
        out.notes.push(format!(
            "no negative values: treated as magnitude-mode data, noise SD scaled by {MAGNITUDE_NOISE_SCALE} (the noise on a line is that much larger than the spread of the noise floor)"
        ));
    }
    out.run_noise_sd = sd;
    out.noise_sd = sd;
    if shape.iter().any(|&n| n < 3) {
        out.notes
            .push("a dimension has fewer than 3 points: no peaks picked".into());
        return Ok(out);
    }
    let st = strides(shape);
    let ratio = if magnitude {
        MAXIMA_P90_MAGNITUDE
    } else {
        MAXIMA_P90_RATIO + MAXIMA_P90_PER_DIMENSION * (dims as f64 - 2.0)
    };
    if let Some(p90) = maxima_p90(data, shape, &st) {
        let from_maxima = p90 / ratio;
        out.maxima_noise_sd = from_maxima;
        if from_maxima > sd {
            out.notes.push(format!(
                "the local maxima of the noise are {:.1} times higher than Gaussian noise of the measured SD gives (non-uniform sampling, t1 noise, truncation): thresholds use the noise level from the maxima",
                from_maxima / sd
            ));
            sd = from_maxima;
            out.noise_sd = sd;
        }
    }
    let lines = line_noise(data, shape, &st);
    let g = Grid {
        data,
        shape,
        st,
        sd,
        lines,
    };
    let win = windows(shape, axes, &opts.ranges_ppm);
    let mut tallest = 0.0_f64;
    for_box(&win, &g.st, |i| {
        if let Some(v) = data.get(i)
            && v.is_finite()
        {
            tallest = tallest.max(v.abs());
        }
    });
    let threshold = (opts.min_snr * sd).max(opts.min_height_fraction * tallest);
    if !(threshold > 0.0) {
        out.notes
            .push("the spectrum has no noise and no signal: no peaks picked".into());
        return Ok(out);
    }
    let steps = neighbour_steps(dims);
    let offsets: Vec<isize> = steps
        .iter()
        .map(|s| s.iter().zip(&g.st).map(|(a, &b)| a * b as isize).sum())
        .collect();
    let mut raw: Vec<Raw> = Vec::new();
    let mut c = vec![0usize; dims];
    for sign in [1.0, -1.0] {
        if sign < 0.0 && !opts.include_negative {
            break;
        }
        for_box(&win, &g.st, |i| {
            let v = g.at(i, sign);
            if !(v >= threshold && v.is_finite()) {
                return;
            }
            for &o in &offsets {
                let j = (i as isize + o) as usize;
                let w = g.at(j, sign);
                if w > v || (w == v && j < i) {
                    return;
                }
            }
            let local = g.local_sd(i);
            let prom = (0..dims)
                .map(|d| g.prominence(i, d, sign))
                .fold(f64::INFINITY, f64::min);
            if prom < opts.min_prominence_snr * sd {
                return;
            }
            // on a noisy line and less than twice the threshold above its noise
            let ridge = local >= opts.ridge_factor * sd
                && local > sd
                && v < 2.0 * opts.min_snr * local;
            let scale = if opts.min_snr > 0.0 {
                opts.min_local_snr / opts.min_snr
            } else {
                1.0
            };
            if v < opts.min_local_snr * local || prom < opts.min_prominence_snr * scale * local {
                out.ridge_suppressed += 1;
                if !opts.keep_ridge_peaks {
                    return;
                }
            }
            coords(i, shape, &mut c);
            let mut h = v;
            let mut pos = Vec::with_capacity(dims);
            for d in 0..dims {
                // a neighbour of the other sign (a dispersive lobe, an antiphase partner) counts
                // as 0: the parabola would otherwise overshoot far above the maximum
                let a = g.at(i - g.st[d], sign).max(0.0);
                let b = g.at(i + g.st[d], sign).max(0.0);
                let den = a - 2.0 * v + b;
                let off = if den < 0.0 {
                    (0.5 * (a - b) / den).clamp(-0.5, 0.5)
                } else {
                    0.0
                };
                h -= 0.25 * (a - b) * off;
                pos.push(c[d] as f64 + off);
            }
            if !in_ranges(&pos, axes, &opts.ranges_ppm) {
                return;
            }
            raw.push(Raw {
                idx: i,
                pos,
                height: sign * h,
                ridge,
                local_sd: local,
            });
        });
    }
    if out.ridge_suppressed > 0 && !opts.keep_ridge_peaks {
        out.notes.push(format!(
            "{} maxima above the noise threshold were left out because they do not stand out from the noise of their row or column (t1 noise, solvent): pass keep_ridge_peaks to list them, flagged ridge",
            out.ridge_suppressed
        ));
    }
    // group: single linkage within the gap in every dimension, via a grid of gap-sized cells
    let gap: Vec<f64> = axes
        .iter()
        .map(|a| {
            let hzpp = a.hz_per_point();
            let by_hz = if hzpp > 0.0 {
                opts.group_gap_hz / hzpp
            } else {
                0.0
            };
            by_hz.max(opts.group_gap_points).max(1.0)
        })
        .collect();
    let cell = |p: &[f64]| -> Vec<i64> {
        p.iter()
            .zip(&gap)
            .map(|(x, gp)| (x / gp).floor() as i64)
            .collect()
    };
    let mut cells: HashMap<Vec<i64>, Vec<usize>> = HashMap::new();
    for (k, r) in raw.iter().enumerate() {
        cells.entry(cell(&r.pos)).or_default().push(k);
    }
    let mut uf = UnionFind((0..raw.len()).collect());
    let near = neighbour_steps(dims);
    for (k, r) in raw.iter().enumerate() {
        let base = cell(&r.pos);
        let mut keys = vec![base.clone()];
        keys.extend(
            near.iter()
                .map(|s| base.iter().zip(s).map(|(a, &b)| a + b as i64).collect()),
        );
        for key in keys {
            if let Some(list) = cells.get(&key) {
                for &m in list {
                    if m > k
                        && raw[m]
                            .pos
                            .iter()
                            .zip(&r.pos)
                            .zip(&gap)
                            .all(|((a, b), gp)| (a - b).abs() <= *gp)
                    {
                        uf.union(k, m);
                    }
                }
            }
        }
    }
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for k in 0..raw.len() {
        let root = uf.find(k);
        groups.entry(root).or_default().push(k);
    }
    let mut groups: Vec<Vec<usize>> = groups.into_values().collect();
    for gr in &mut groups {
        gr.sort_by(|&a, &b| raw[b].height.abs().total_cmp(&raw[a].height.abs()));
    }
    groups.sort_by(|a, b| raw[b[0]].height.abs().total_cmp(&raw[a[0]].height.abs()));
    let maxima_total_groups = groups.len();
    // footprints: watershed flood per sign from the members
    let mut label: HashMap<usize, u32> = HashMap::new();
    for sign in [1.0, -1.0] {
        let mut heap = BinaryHeap::new();
        for (gi, gr) in groups.iter().enumerate() {
            for &m in gr {
                let r = &raw[m];
                if r.height * sign > 0.0 {
                    label.insert(r.idx, gi as u32);
                    heap.push(Flood(g.at(r.idx, sign), r.idx, gi as u32));
                }
            }
        }
        let mut full = false;
        while let Some(Flood(_, i, gi)) = heap.pop() {
            coords(i, shape, &mut c);
            for d in 0..dims {
                for up in [false, true] {
                    let j = if up {
                        if c[d] + 1 >= shape[d] {
                            continue;
                        }
                        i + g.st[d]
                    } else {
                        if c[d] == 0 {
                            continue;
                        }
                        i - g.st[d]
                    };
                    if label.contains_key(&j) {
                        continue;
                    }
                    let v = g.at(j, sign);
                    if !(v.is_finite() && v >= opts.footprint_snr * g.local_sd(j) && v > 0.0) {
                        continue;
                    }
                    if label.len() >= MAX_FOOTPRINT_POINTS {
                        full = true;
                        continue;
                    }
                    label.insert(j, gi);
                    heap.push(Flood(v, j, gi));
                }
            }
        }
        if full {
            out.notes.push(format!(
                "footprints were cut at {MAX_FOOTPRINT_POINTS} points: volumes of broad peaks are too small; raise footprint_snr or narrow ranges_ppm"
            ));
        }
    }
    let cell_size: f64 = axes.iter().map(|a| a.step_ppm.abs()).product();
    let mut acc: Vec<Acc> = (0..groups.len())
        .map(|_| Acc {
            vol: 0.0,
            abs: 0.0,
            n: 0,
            lo: vec![usize::MAX; dims],
            hi: vec![0; dims],
        })
        .collect();
    for (&i, &gi) in &label {
        let Some(a) = acc.get_mut(gi as usize) else {
            continue;
        };
        let v = data.get(i).copied().unwrap_or(0.0);
        a.vol += v;
        a.abs += v.abs();
        a.n += 1;
        coords(i, shape, &mut c);
        for d in 0..dims {
            a.lo[d] = a.lo[d].min(c[d]);
            a.hi[d] = a.hi[d].max(c[d]);
        }
    }
    drop(label);
    // single-point spikes (reconstruction artefacts) cannot be a resolved line
    let mut kept: Vec<(Vec<usize>, Acc)> = groups
        .into_iter()
        .zip(acc)
        .filter(|(_, a)| a.n >= opts.min_footprint_points)
        .collect();
    let small = maxima_total_groups - kept.len();
    if small > 0 {
        out.small_footprint_suppressed = small;
        out.notes.push(format!(
            "{small} cross peaks with fewer than {} footprint points (isolated spikes) were left out (min_footprint_points)",
            opts.min_footprint_points
        ));
    }
    if kept.len() > opts.max_peaks {
        out.notes.push(format!(
            "{} cross peaks found, the {} tallest kept (max_peaks)",
            kept.len(),
            opts.max_peaks
        ));
        kept.truncate(opts.max_peaks);
    }
    let (groups, acc): (Vec<Vec<usize>>, Vec<Acc>) = kept.into_iter().unzip();
    // assemble
    let mut peaks: Vec<(CrossPeak, Vec<usize>)> = Vec::with_capacity(groups.len());
    for (gi, gr) in groups.iter().enumerate() {
        let apex = &raw[gr[0]];
        let sign = if apex.height >= 0.0 { 1.0 } else { -1.0 };
        let wsum: f64 = gr.iter().map(|&m| raw[m].height.abs()).sum();
        let centre: Vec<f64> = (0..dims)
            .map(|d| gr.iter().map(|&m| raw[m].pos[d] * raw[m].height.abs()).sum::<f64>() / wsum)
            .collect();
        let pos_n = gr.iter().filter(|&&m| raw[m].height > 0.0).count();
        let peak_sign = match (pos_n, gr.len() - pos_n) {
            (_, 0) => PeakSign::Positive,
            (0, _) => PeakSign::Negative,
            _ => PeakSign::Mixed,
        };
        let h = apex.height.abs();
        let widths: Vec<Option<f64>> = (0..dims)
            .map(|d| g.half_width(apex.idx, d, sign, h))
            .collect();
        coords(apex.idx, shape, &mut c);
        let a = &acc[gi];
        let ppm: Vec<f64> = centre.iter().zip(axes).map(|(p, ax)| ax.ppm(*p)).collect();
        let mut flags = Vec::new();
        if gr.iter().any(|&m| raw[m].ridge) {
            flags.push(PeakFlag::Ridge);
        }
        peaks.push((
            CrossPeak {
                hz: ppm
                    .iter()
                    .zip(axes)
                    .map(|(p, ax)| p * ax.reference_frequency_mhz)
                    .collect(),
                ppm,
                apex_ppm: apex.pos.iter().zip(axes).map(|(p, ax)| ax.ppm(*p)).collect(),
                index: c.clone(),
                height: apex.height,
                sign: peak_sign,
                snr: (sd > 0.0).then(|| h / sd),
                local_snr: (apex.local_sd > 0.0).then(|| h / apex.local_sd),
                width_hz: widths
                    .iter()
                    .zip(axes)
                    .map(|(w, ax)| w.map(|w| w * ax.hz_per_point()))
                    .collect(),
                width_ppm: widths
                    .iter()
                    .zip(axes)
                    .map(|(w, ax)| w.map(|w| w * ax.step_ppm.abs()))
                    .collect(),
                maxima: gr.len(),
                volume: a.vol * cell_size,
                absolute_volume: a.abs * cell_size,
                footprint_points: a.n,
                footprint_ppm: (0..dims)
                    .map(|d| {
                        let (p, q) = if a.n == 0 {
                            (axes[d].ppm(c[d] as f64), axes[d].ppm(c[d] as f64))
                        } else {
                            (axes[d].ppm(a.lo[d] as f64), axes[d].ppm(a.hi[d] as f64))
                        };
                        (p.max(q), p.min(q))
                    })
                    .collect(),
                flags,
                symmetric_partner: None,
                partner_height_ratio: None,
            },
            gr.clone(),
        ));
    }
    // order: direct dimension high shift first, then the slower ones
    peaks.sort_by(|(a, _), (b, _)| {
        for d in (0..dims).rev() {
            match b.ppm[d].total_cmp(&a.ppm[d]) {
                Ordering::Equal => {}
                o => return o,
            }
        }
        Ordering::Equal
    });
    out.maxima_count = peaks.iter().map(|p| p.1.len()).sum();
    if opts.list_maxima {
        for (pi, (_, gr)) in peaks.iter().enumerate() {
            for &m in gr {
                let r = &raw[m];
                let mut ci = vec![0usize; dims];
                coords(r.idx, shape, &mut ci);
                out.maxima.push(Maximum {
                    ppm: r.pos.iter().zip(axes).map(|(p, ax)| ax.ppm(*p)).collect(),
                    index: ci,
                    height: r.height,
                    snr: (sd > 0.0).then(|| r.height.abs() / sd),
                    peak: pi,
                });
            }
        }
    }
    out.peaks = peaks.into_iter().map(|p| p.0).collect();
    Ok(out)
}

/// What [`annotate`] needs to know about the spectrum besides its axes.
#[derive(Debug, Clone, Default)]
pub struct SpectrumContext {
    /// Nucleus per dimension (`1H`, `13C`), slowest first.
    pub nuclei: Vec<Option<String>>,
    /// Point of the carrier per dimension (axial peaks), slowest first.
    pub carrier_points: Vec<Option<f64>>,
}

/// The pair of dimensions that observe the same nucleus over overlapping shift ranges
/// (at least half of the narrower range), if any.
pub fn homonuclear_pair(axes: &[PpmAxis], nuclei: &[Option<String>]) -> Option<(usize, usize)> {
    let range = |a: &PpmAxis| {
        let (p, q) = (a.first_ppm, a.last_ppm());
        (p.min(q), p.max(q))
    };
    for i in 0..axes.len() {
        for j in i + 1..axes.len() {
            let (Some(a), Some(b)) = (nuclei.get(i).cloned().flatten(), nuclei.get(j).cloned().flatten()) else {
                continue;
            };
            if !a.eq_ignore_ascii_case(&b) {
                continue;
            }
            let (ri, rj) = (range(&axes[i]), range(&axes[j]));
            let overlap = ri.1.min(rj.1) - ri.0.max(rj.0);
            let narrow = (ri.1 - ri.0).min(rj.1 - rj.0);
            if narrow > 0.0 && overlap >= 0.5 * narrow {
                return Some((i, j));
            }
        }
    }
    None
}

/// Annotate cross peaks (module docs): diagonal and symmetric partners on homonuclear spectra,
/// axial peaks. Returns the homonuclear dimension pair used, if any.
pub fn annotate(
    peaks: &mut [CrossPeak],
    axes: &[PpmAxis],
    ctx: &SpectrumContext,
    opts: &CrossPeakOptions,
) -> Option<(usize, usize)> {
    let dims = axes.len();
    // axial: within a point of an indirect dimension's carrier
    for p in peaks.iter_mut() {
        let axial = (0..dims.saturating_sub(1)).any(|d| {
            ctx.carrier_points
                .get(d)
                .copied()
                .flatten()
                .is_some_and(|c| (axes[d].index_of(p.apex_ppm[d]) - c).abs() <= 1.0)
        });
        if axial && !p.has(PeakFlag::Axial) {
            p.flags.push(PeakFlag::Axial);
        }
    }
    let Some(pair) = homonuclear_pair(axes, &ctx.nuclei) else {
        streaks(peaks, axes, opts, false);
        return None;
    };
    let (i, j) = pair;
    let tol = |hz: f64, pts: f64| {
        [i, j]
            .iter()
            .map(|&d| {
                let a = &axes[d];
                let by_hz = if a.reference_frequency_mhz > 0.0 {
                    hz / a.reference_frequency_mhz
                } else {
                    0.0
                };
                by_hz.max(pts * a.step_ppm.abs())
            })
            .fold(0.0, f64::max)
    };
    let diag = tol(opts.diagonal_tolerance_hz, 1.5);
    let sym = tol(opts.symmetry_tolerance_hz, 2.0);
    // other dimensions of an nD spectrum must agree for a mirror image
    let others: Vec<(usize, f64)> = (0..dims)
        .filter(|&d| d != i && d != j)
        .map(|d| (d, 2.0 * axes[d].step_ppm.abs()))
        .collect();
    for p in peaks.iter_mut() {
        if (p.ppm[i] - p.ppm[j]).abs() <= diag && !p.has(PeakFlag::Diagonal) {
            p.flags.push(PeakFlag::Diagonal);
        }
    }
    let n = peaks.len();
    let inside = |r: (f64, f64), x: f64| x <= r.0 + sym && x >= r.1 - sym;
    for a in 0..n {
        if peaks[a].has(PeakFlag::Diagonal) {
            continue;
        }
        let pa = &peaks[a];
        let mut best: Option<(usize, f64)> = None;
        for (b, q) in peaks.iter().enumerate() {
            if b == a || q.has(PeakFlag::Diagonal) {
                continue;
            }
            if others.iter().any(|&(d, t)| (q.ppm[d] - pa.ppm[d]).abs() > t) {
                continue;
            }
            // the mirror image of one centre lies in the other's footprint (or within the
            // tolerance of it): multiplets split into maxima differently on the two sides
            let a_in_b = inside(q.footprint_ppm[i], pa.ppm[j]) && inside(q.footprint_ppm[j], pa.ppm[i]);
            let b_in_a = inside(pa.footprint_ppm[j], q.ppm[i]) && inside(pa.footprint_ppm[i], q.ppm[j]);
            if !(a_in_b || b_in_a) {
                continue;
            }
            let dist = (q.ppm[i] - pa.ppm[j]).abs().max((q.ppm[j] - pa.ppm[i]).abs());
            if best.is_none_or(|(_, bd)| dist < bd) {
                best = Some((b, dist));
            }
        }
        match best {
            Some((b, _)) => {
                let ratio = peaks[a].height.abs() / peaks[b].height.abs();
                peaks[a].symmetric_partner = Some(b);
                peaks[a].partner_height_ratio = ratio.is_finite().then_some(ratio);
            }
            None => {
                if !peaks[a].has(PeakFlag::PartnerMissing) {
                    peaks[a].flags.push(PeakFlag::PartnerMissing);
                }
            }
        }
    }
    streaks(peaks, axes, opts, true);
    Some(pair)
}

/// Flag `ridge` on cross peaks that lie on an indirect-dimension line (same direct-dimension
/// position within the stronger peak's half width, at least a point) of a cross peak at least
/// `ridge_ratio` times taller: t1 noise and truncation wiggles. On homonuclear spectra
/// (`homonuclear`) a cross peak with a symmetric partner is kept unflagged.
fn streaks(peaks: &mut [CrossPeak], axes: &[PpmAxis], opts: &CrossPeakOptions, homonuclear: bool) {
    let dims = axes.len();
    if dims < 2 || !(opts.ridge_ratio > 1.0) {
        return;
    }
    let last = dims - 1;
    let step = axes[last].step_ppm.abs();
    let mut flag = vec![false; peaks.len()];
    for (a, p) in peaks.iter().enumerate() {
        if p.has(PeakFlag::Ridge) || p.has(PeakFlag::Diagonal) || (homonuclear && p.symmetric_partner.is_some()) {
            continue;
        }
        flag[a] = peaks.iter().any(|q| {
            let tol = (q.width_ppm[last].unwrap_or(0.0) / 2.0).max(step);
            q.height.abs() >= opts.ridge_ratio * p.height.abs()
                && (q.apex_ppm[last] - p.apex_ppm[last]).abs() <= tol
                && (0..last).all(|d| (0..last).filter(|&e| e != d).all(|e| (q.ppm[e] - p.ppm[e]).abs() <= 2.0 * axes[e].step_ppm.abs()))
        });
    }
    for (p, f) in peaks.iter_mut().zip(flag) {
        if f {
            p.flags.push(PeakFlag::Ridge);
        }
    }
}

/// One volume integral over a box.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct VolumeIntegral {
    /// The box per dimension (high ppm, low ppm), slowest first.
    pub ranges_ppm: Vec<(f64, f64)>,
    /// Sum of the points in the box × the cell size (intensity·ppmᴺ).
    pub value: f64,
    /// `value` rescaled so the reference box has the reference value (`None` when the
    /// reference integral is 0 or missing).
    pub normalized: Option<f64>,
    /// Points summed.
    pub points: usize,
}

/// Integrate boxes (one ppm pair per dimension, either order) of the matrix. `reference` =
/// (box index, value) for the normalisation (default: box 0 → 1).
pub fn integrate_regions(
    data: &[f64],
    shape: &[usize],
    axes: &[PpmAxis],
    regions: &[Vec<(f64, f64)>],
    reference: Option<(usize, f64)>,
) -> Result<Vec<VolumeIntegral>> {
    check_shape(data.len(), shape, axes)?;
    let st = strides(shape);
    let cell_size: f64 = axes.iter().map(|a| a.step_ppm.abs()).product();
    let mut out = Vec::with_capacity(regions.len());
    for r in regions {
        if r.len() != shape.len() {
            return Err(Error::Usage(format!(
                "an integration box needs one ppm range per dimension ({}), got {}",
                shape.len(),
                r.len()
            )));
        }
        let mut win = Vec::with_capacity(shape.len());
        let mut empty = false;
        for (d, &(a, b)) in r.iter().enumerate() {
            let (i, j) = (axes[d].index_of(a), axes[d].index_of(b));
            let (i, j) = if i <= j { (i, j) } else { (j, i) };
            let start = i.ceil().max(0.0);
            let end = j.floor().min(shape[d] as f64 - 1.0);
            if end < start {
                empty = true;
                win.push((0, 0));
            } else {
                win.push((start as usize, end as usize));
            }
        }
        let (mut sum, mut points) = (0.0, 0usize);
        if !empty {
            for_box(&win, &st, |k| {
                if let Some(v) = data.get(k)
                    && v.is_finite()
                {
                    sum += v;
                    points += 1;
                }
            });
        }
        out.push(VolumeIntegral {
            ranges_ppm: r.iter().map(|&(a, b)| (a.max(b), a.min(b))).collect(),
            value: sum * cell_size,
            normalized: None,
            points,
        });
    }
    let (ri, rv) = reference.unwrap_or((0, 1.0));
    let base = out.get(ri).map_or(f64::NAN, |r| r.value);
    for r in &mut out {
        r.normalized = (base != 0.0 && base.is_finite()).then(|| r.value / base * rv);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Standard normal noise from a hash of `i` (Box–Muller).
    fn gauss(i: usize) -> f64 {
        let u = |k: u64| {
            let mut z = k.wrapping_add(0x9E37_79B9_7F4A_7C15);
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^= z >> 31;
            ((z >> 11) as f64 + 0.5) / (1u64 << 53) as f64
        };
        let (a, b) = (u(2 * i as u64), u(2 * i as u64 + 1));
        (-2.0 * a.ln()).sqrt() * (std::f64::consts::TAU * b).cos()
    }

    fn axis(first: f64, n: usize, width_ppm: f64, mhz: f64) -> PpmAxis {
        PpmAxis {
            first_ppm: first,
            step_ppm: -width_ppm / n as f64,
            size: n,
            reference_frequency_mhz: mhz,
        }
    }

    /// (row, col, height, row FWHM, col FWHM) Lorentzian peaks on a noise floor of SD `sd`.
    fn spectrum(shape: [usize; 2], peaks: &[(f64, f64, f64, f64, f64)], sd: f64) -> Vec<f64> {
        let mut v = Vec::with_capacity(shape[0] * shape[1]);
        for r in 0..shape[0] {
            for c in 0..shape[1] {
                let mut x = sd * gauss(r * shape[1] + c);
                for &(pr, pc, h, wr, wc) in peaks {
                    let a = (r as f64 - pr) / (wr / 2.0);
                    let b = (c as f64 - pc) / (wc / 2.0);
                    x += h / ((1.0 + a * a) * (1.0 + b * b));
                }
                v.push(x);
            }
        }
        v
    }

    fn hetero() -> Vec<PpmAxis> {
        vec![axis(160.0, 256, 160.0, 100.0), axis(10.0, 512, 10.0, 400.0)]
    }

    #[test]
    fn noise_estimate_is_close_to_the_true_sd() {
        let v = spectrum([256, 512], &[(100.0, 200.0, 500.0, 3.0, 4.0)], 2.0);
        let sd = matrix_noise_sd(&v, 512);
        assert!((sd - 2.0).abs() < 0.1, "{sd}");
        let pure = spectrum([512, 512], &[], 1.0);
        let sd = matrix_noise_sd(&pure, 512);
        assert!((sd - 1.0).abs() < 0.03, "{sd}");
        // the maxima give the same level on Gaussian noise
        let p = pick_cross_peaks(&pure, &[512, 512], &[axis(10.0, 512, 10.0, 400.0); 2], &CrossPeakOptions::default()).unwrap();
        assert!((p.maxima_noise_sd / p.run_noise_sd - 1.0).abs() < 0.1, "{p:?}");
        // heavy-tailed noise raises the noise level the thresholds use
        let heavy: Vec<f64> = pure.iter().enumerate().map(|(i, x)| x * if i % 7 == 0 { 4.0 } else { 1.0 }).collect();
        let p = pick_cross_peaks(&heavy, &[512, 512], &[axis(10.0, 512, 10.0, 400.0); 2], &CrossPeakOptions::default()).unwrap();
        assert!(p.noise_sd > 1.3 * p.run_noise_sd, "{} {}", p.noise_sd, p.run_noise_sd);
        assert!(p.peaks.is_empty());
        // blanked (exactly zero) rows are ignored
        let mut w = v.clone();
        w[..512 * 128].fill(0.0);
        assert!((matrix_noise_sd(&w, 512) - 2.0).abs() < 0.3);
        assert!(matrix_noise_sd(&[0.0; 4], 2).abs() < f64::EPSILON);
    }

    #[test]
    fn finds_peaks_with_position_width_and_sign() {
        let ax = hetero();
        let v = spectrum(
            [256, 512],
            &[
                (100.3, 200.6, 100.0, 3.0, 5.0),
                (50.0, 400.0, -60.0, 3.0, 5.0),
            ],
            1.0,
        );
        let p = pick_cross_peaks(&v, &[256, 512], &ax, &CrossPeakOptions::default()).unwrap();
        assert_eq!(p.peaks.len(), 2, "{:?}", p.peaks);
        let a = p.peaks.iter().find(|x| x.height > 0.0).unwrap();
        assert!((ax[0].index_of(a.ppm[0]) - 100.3).abs() < 0.1, "{a:?}");
        assert!((ax[1].index_of(a.ppm[1]) - 200.6).abs() < 0.1);
        assert!((a.height - 100.0).abs() < 4.0);
        assert_eq!(a.index, vec![100, 201]);
        let w0 = a.width_ppm[0].unwrap() / ax[0].step_ppm.abs();
        let w1 = a.width_hz[1].unwrap() / ax[1].hz_per_point();
        assert!((w0 - 3.0).abs() < 0.5 && (w1 - 5.0).abs() < 0.5, "{w0} {w1}");
        assert!(a.snr.unwrap() > 80.0);
        assert_eq!(a.sign, PeakSign::Positive);
        let b = p.peaks.iter().find(|x| x.height < 0.0).unwrap();
        assert_eq!(b.sign, PeakSign::Negative);
        assert!(b.volume < 0.0 && b.absolute_volume > 0.0);
        // direct dimension high shift first
        assert!(p.peaks[0].ppm[1] > p.peaks[1].ppm[1]);
        let pos_only = CrossPeakOptions {
            include_negative: false,
            ..CrossPeakOptions::default()
        };
        assert_eq!(
            pick_cross_peaks(&v, &[256, 512], &ax, &pos_only)
                .unwrap()
                .peaks
                .len(),
            1
        );
    }

    #[test]
    fn noise_alone_gives_no_peaks() {
        let v = spectrum([256, 512], &[], 1.0);
        let p = pick_cross_peaks(&v, &[256, 512], &hetero(), &CrossPeakOptions::default()).unwrap();
        assert!(p.peaks.is_empty(), "{:?}", p.peaks.first());
    }

    #[test]
    fn multiplet_fine_structure_is_one_cross_peak() {
        // antiphase 2×2 pattern (J = 7 Hz in both dimensions of a 1H/1H spectrum at 400 MHz,
        // 1.95 Hz per point), as in a phase-sensitive DQF-COSY
        let ax = vec![axis(10.0, 2048, 10.0, 400.0), axis(10.0, 2048, 10.0, 400.0)];
        let j = 7.0 / ax[0].hz_per_point();
        let (r, c) = (600.0, 1400.0);
        let mut pk = Vec::new();
        for (dr, dc, s) in [(-1.0, -1.0, 1.0), (-1.0, 1.0, -1.0), (1.0, -1.0, -1.0), (1.0, 1.0, 1.0)] {
            pk.push((r + dr * j / 2.0, c + dc * j / 2.0, 80.0 * s, 1.5, 1.5));
        }
        // and a separate singlet 100 Hz away
        pk.push((r, c + 100.0 / ax[1].hz_per_point(), 50.0, 1.5, 1.5));
        let shape = [2048, 2048];
        let v = spectrum(shape, &pk, 1.0);
        let opts = CrossPeakOptions {
            list_maxima: true,
            ..CrossPeakOptions::default()
        };
        let p = pick_cross_peaks(&v, &shape, &ax, &opts).unwrap();
        assert_eq!(p.peaks.len(), 2, "{:?}", p.peaks);
        let m = p.peaks.iter().find(|x| x.maxima == 4).expect("the multiplet");
        assert_eq!(m.sign, PeakSign::Mixed);
        assert!((ax[0].index_of(m.ppm[0]) - r).abs() < 0.3);
        assert!((ax[1].index_of(m.ppm[1]) - c).abs() < 0.3);
        // antiphase lobes cancel in the volume, not in the absolute volume
        assert!(m.volume.abs() < 0.2 * m.absolute_volume, "{m:?}");
        assert_eq!(p.maxima.len(), 5);
        assert_eq!(p.maxima_count, 5);
        let mi = p.peaks.iter().position(|x| x.maxima == 4).unwrap();
        assert_eq!(p.maxima.iter().filter(|x| x.peak == mi).count(), 4);
    }

    #[test]
    fn ridges_do_not_flood_the_list() {
        // a strong line with a column of t1 noise (noise SD 30 along F1 at its F2 position)
        let shape = [256, 512];
        let ax = hetero();
        let mut v = spectrum(shape, &[(128.0, 300.0, 20000.0, 2.0, 3.0)], 1.0);
        for r in 0..256 {
            for c in 298..=302 {
                v[r * 512 + c] += 30.0 * gauss(1_000_000 + r * 7 + c);
            }
        }
        let p = pick_cross_peaks(&v, &shape, &ax, &CrossPeakOptions::default()).unwrap();
        assert_eq!(p.peaks.len(), 1, "{:?}", p.peaks);
        // the strong line stands far above the noise of its column: not flagged
        assert!(!p.peaks[0].has(PeakFlag::Ridge));
        assert!(p.ridge_suppressed > 3, "{}", p.ridge_suppressed);
        assert!(p.notes.iter().any(|n| n.contains("keep_ridge_peaks")));
        let keep = CrossPeakOptions {
            keep_ridge_peaks: true,
            ..CrossPeakOptions::default()
        };
        let q = pick_cross_peaks(&v, &shape, &ax, &keep).unwrap();
        assert!(q.peaks.len() > 3);
        assert!(q.peaks.iter().filter(|x| x.has(PeakFlag::Ridge)).count() == q.peaks.len() - 1);
        // the footprint of the strong line does not run along the ridge
        let fp = p.peaks[0].footprint_ppm[0];
        assert!((fp.0 - fp.1) / ax[0].step_ppm.abs() < 120.0, "{fp:?}");
    }

    #[test]
    fn volumes_scale_with_area_and_boxes_normalize() {
        let shape = [256, 512];
        let ax = hetero();
        // same height, twice the width in F2: twice the volume
        let v = spectrum(
            shape,
            &[(60.0, 100.0, 200.0, 3.0, 4.0), (180.0, 400.0, 200.0, 3.0, 8.0)],
            0.5,
        );
        let opts = CrossPeakOptions {
            footprint_snr: 1.0,
            ..CrossPeakOptions::default()
        };
        let p = pick_cross_peaks(&v, &shape, &ax, &opts).unwrap();
        assert_eq!(p.peaks.len(), 2);
        let (a, b) = (&p.peaks[0], &p.peaks[1]);
        let (a, b) = if a.index[1] < b.index[1] { (a, b) } else { (b, a) };
        let ratio = b.volume / a.volume;
        assert!((ratio - 2.0).abs() < 0.35, "{ratio}");
        let boxes = vec![
            vec![(ax[0].ppm(30.0), ax[0].ppm(90.0)), (ax[1].ppm(40.0), ax[1].ppm(160.0))],
            vec![(ax[0].ppm(150.0), ax[0].ppm(210.0)), (ax[1].ppm(280.0), ax[1].ppm(511.0))],
            vec![(500.0, 400.0), (50.0, 40.0)],
        ];
        let r = integrate_regions(&v, &shape, &ax, &boxes, None).unwrap();
        assert!((r[0].normalized.unwrap() - 1.0).abs() < 1e-12);
        assert!((r[1].normalized.unwrap() - 2.0).abs() < 0.25, "{r:?}");
        assert_eq!(r[2].points, 0);
        let r = integrate_regions(&v, &shape, &ax, &boxes[..2], Some((1, 10.0))).unwrap();
        assert!((r[1].normalized.unwrap() - 10.0).abs() < 1e-9);
        assert!(integrate_regions(&v, &shape, &ax, &[vec![(1.0, 2.0)]], None).is_err());
    }

    #[test]
    fn homonuclear_diagonal_and_symmetric_partners() {
        // 1H/1H, 1024 × 1024 over 10 ppm at 400 MHz
        let n = 1024;
        let ax = vec![axis(10.0, n, 10.0, 400.0), axis(10.0, n, 10.0, 400.0)];
        let at = |ppm: f64| ax[0].index_of(ppm);
        let (a, b, c, d) = (7.2, 3.5, 1.2, 4.6);
        let v = spectrum(
            [n, n],
            &[
                (at(a), at(a), 500.0, 2.0, 2.0),
                (at(b), at(b), 400.0, 2.0, 2.0),
                (at(a), at(b), 80.0, 2.0, 2.0),
                (at(b), at(a), 60.0, 2.0, 2.0),
                (at(c), at(d), 50.0, 2.0, 2.0),
            ],
            1.0,
        );
        let opts = CrossPeakOptions::default();
        let mut p = pick_cross_peaks(&v, &[n, n], &ax, &opts).unwrap();
        assert_eq!(p.peaks.len(), 5);
        let ctx = SpectrumContext {
            nuclei: vec![Some("1H".into()), Some("1H".into())],
            carrier_points: vec![None, None],
        };
        let pair = annotate(&mut p.peaks, &ax, &ctx, &opts);
        assert_eq!(pair, Some((0, 1)));
        let find = |x: f64, y: f64| {
            p.peaks
                .iter()
                .position(|q| (q.ppm[0] - x).abs() < 0.02 && (q.ppm[1] - y).abs() < 0.02)
                .unwrap()
        };
        assert!(p.peaks[find(a, a)].has(PeakFlag::Diagonal));
        assert!(p.peaks[find(b, b)].has(PeakFlag::Diagonal));
        let (ab, ba) = (find(a, b), find(b, a));
        assert_eq!(p.peaks[ab].symmetric_partner, Some(ba));
        assert_eq!(p.peaks[ba].symmetric_partner, Some(ab));
        let r = p.peaks[ab].partner_height_ratio.unwrap();
        assert!((r - 80.0 / 60.0).abs() < 0.1, "{r}");
        assert!(p.peaks[find(c, d)].has(PeakFlag::PartnerMissing));
        // heteronuclear: no diagonal
        let mut q = pick_cross_peaks(&v, &[n, n], &ax, &opts).unwrap();
        let hetero = SpectrumContext {
            nuclei: vec![Some("13C".into()), Some("1H".into())],
            carrier_points: vec![None, None],
        };
        assert_eq!(annotate(&mut q.peaks, &ax, &hetero, &opts), None);
        assert!(q.peaks.iter().all(|x| x.flags.is_empty()));
    }

    #[test]
    fn axial_peaks_at_the_indirect_carrier() {
        let shape = [256, 512];
        let ax = hetero();
        let v = spectrum(
            shape,
            &[(128.0, 100.0, 100.0, 2.0, 3.0), (60.0, 300.0, 100.0, 2.0, 3.0)],
            1.0,
        );
        let opts = CrossPeakOptions::default();
        let mut p = pick_cross_peaks(&v, &shape, &ax, &opts).unwrap();
        let ctx = SpectrumContext {
            nuclei: vec![Some("13C".into()), Some("1H".into())],
            carrier_points: vec![Some(128.0), None],
        };
        annotate(&mut p.peaks, &ax, &ctx, &opts);
        assert_eq!(p.peaks.iter().filter(|x| x.has(PeakFlag::Axial)).count(), 1);
        let k = p.peaks.iter().position(|x| x.has(PeakFlag::Axial)).unwrap();
        assert_eq!(p.peaks[k].index[0], 128);
    }

    #[test]
    fn magnitude_mode_spectrum() {
        // |complex noise| (Rayleigh) with two broad lines, all values positive
        let shape = [256, 512];
        let ax = hetero();
        let clean = spectrum(
            shape,
            &[(80.0, 120.0, 200.0, 6.0, 8.0), (200.0, 300.0, 120.0, 6.0, 8.0)],
            0.0,
        );
        let v: Vec<f64> = clean
            .iter()
            .enumerate()
            .map(|(i, &x)| (x + gauss(3 * i)).hypot(gauss(3 * i + 1)))
            .collect();
        let p = pick_cross_peaks(&v, &shape, &ax, &CrossPeakOptions::default()).unwrap();
        assert_eq!(p.peaks.len(), 2, "{:?}", p.peaks);
        assert!(p.peaks.iter().all(|x| x.sign == PeakSign::Positive));
    }

    #[test]
    fn three_dimensions() {
        let shape = [32, 48, 128];
        let ax = vec![
            axis(130.0, 32, 30.0, 40.5),
            axis(70.0, 48, 60.0, 100.0),
            axis(10.0, 128, 6.0, 400.0),
        ];
        let (pc, h) = ([10.4, 20.0, 64.7], 300.0);
        let mut v = Vec::with_capacity(32 * 48 * 128);
        for a in 0..32 {
            for b in 0..48 {
                for c in 0..128 {
                    let d = [a as f64 - pc[0], b as f64 - pc[1], c as f64 - pc[2]];
                    let g: f64 = d.iter().map(|x| (-(x * x) / 2.0).exp()).product();
                    v.push(h * g + gauss((a * 48 + b) * 128 + c));
                }
            }
        }
        let p = pick_cross_peaks(&v, &shape, &ax, &CrossPeakOptions::default()).unwrap();
        assert_eq!(p.peaks.len(), 1, "{:?}", p.peaks);
        for (d, a) in ax.iter().enumerate() {
            assert!((a.index_of(p.peaks[0].ppm[d]) - pc[d]).abs() < 0.15, "dim {d}");
        }
        assert_eq!(p.peaks[0].ppm.len(), 3);
        // a range in the slowest dimension that excludes the peak
        let opts = CrossPeakOptions {
            ranges_ppm: vec![Some((ax[0].ppm(20.0), ax[0].ppm(30.0)))],
            ..CrossPeakOptions::default()
        };
        assert!(pick_cross_peaks(&v, &shape, &ax, &opts).unwrap().peaks.is_empty());
    }

    #[test]
    fn weak_peaks_on_a_strong_column_are_flagged() {
        // a 100x weaker maximum on the F1 line of a strong peak (t1 noise, truncation wiggle)
        let shape = [256, 512];
        let ax = hetero();
        let v = spectrum(
            shape,
            &[(60.0, 200.0, 10000.0, 2.0, 3.0), (180.0, 200.0, 100.0, 2.0, 3.0), (180.0, 400.0, 100.0, 2.0, 3.0)],
            1.0,
        );
        let opts = CrossPeakOptions::default();
        let mut p = pick_cross_peaks(&v, &shape, &ax, &opts).unwrap();
        assert_eq!(p.peaks.len(), 3);
        let ctx = SpectrumContext {
            nuclei: vec![Some("13C".into()), Some("1H".into())],
            carrier_points: vec![None, None],
        };
        annotate(&mut p.peaks, &ax, &ctx, &opts);
        let flagged: Vec<usize> = p
            .peaks
            .iter()
            .filter(|x| x.has(PeakFlag::Ridge))
            .map(|x| x.index[1])
            .collect();
        assert_eq!(flagged.len(), 1, "{:?}", p.peaks);
        let weak = p.peaks.iter().find(|x| x.has(PeakFlag::Ridge)).unwrap();
        assert_eq!(weak.index, vec![180, 200]);
    }

    #[test]
    fn bad_and_tiny_input() {
        let ax = hetero();
        let o = CrossPeakOptions::default();
        assert!(pick_cross_peaks(&[0.0; 10], &[2, 4], &ax, &o).is_err());
        assert!(pick_cross_peaks(&[0.0; 8], &[2, 4], &ax[..1], &o).is_err());
        assert!(pick_cross_peaks(&[1.0; 8], &[2, 4], &ax, &o).unwrap().peaks.is_empty());
        let flat = vec![0.0; 64 * 64];
        assert!(pick_cross_peaks(&flat, &[64, 64], &ax, &o).unwrap().peaks.is_empty());
        let mut nan = spectrum([64, 64], &[(30.0, 30.0, 100.0, 2.0, 2.0)], 1.0);
        nan[5] = f64::NAN;
        nan[700] = f64::INFINITY;
        let p = pick_cross_peaks(&nan, &[64, 64], &ax, &o).unwrap();
        assert_eq!(p.peaks.len(), 1, "{:?}", p.peaks);
        // a single-point spike is not a line
        let mut spike = spectrum([64, 64], &[], 1.0);
        spike[32 * 64 + 20] = 100.0;
        let p = pick_cross_peaks(&spike, &[64, 64], &ax, &o).unwrap();
        assert!(p.peaks.is_empty(), "{:?}", p.peaks);
        assert_eq!(p.small_footprint_suppressed, 1);
    }
}
