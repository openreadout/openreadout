//! Reconstruction of non-uniformly sampled (NUS) indirect dimensions.
//!
//! A non-uniformly sampled experiment measures only some increments of the indirect
//! dimensions (Bruker `FnTYPE` 2 lists them in `nuslist`). Transforming the grid with zeros at
//! the other increments gives every line a pattern of sampling artefacts. [`reconstruct`] fills
//! the missing increments of the grid that [`crate::nmr::indirect::place_on_grid`] made, before
//! any indirect transform, so that [`crate::nmr::process_nd`] can process the full grid as if
//! it had been measured.
//!
//! **Method: iterative soft thresholding** (Stern, Donoho, Hoch, J. Magn. Reson. 188 (2007)
//! 295; Hyberts et al., J. Biomol. NMR 52 (2012) 315; docs/provenance/nmr-processing.md).
//!
//! 1. The direct dimension of every measured row is Fourier transformed (no window, no zero
//!    filling beyond the next power of two), so that each direct-dimension point (a "column")
//!    holds an indirect time signal of its own.
//! 2. The rows that belong to one measured increment are combined into complex sequences with
//!    one exponential per line: a `Complex` (QF) row as it is, echo and antiecho rows each on
//!    their own, a `States` or `StatesTppi` pair (u, v) as u + i·v and u − i·v. With two
//!    indirect dimensions the combination is done in each of them.
//! 3. Each sequence of each column is reconstructed on its own. The spectrum `S` starts at zero;
//!    each iteration takes the difference between the measured points and the inverse
//!    transform of `S` (zero at the points not measured), adds its transform to `S`, and soft
//!    thresholds the sum (`S ← S·max(0, 1 − τ/|S|)`). The threshold τ starts just below the
//!    tallest point of the zero-filled spectrum and drops by [`THRESHOLD_DECAY`] per iteration
//!    to a floor at the noise level, then stays there until the spectrum changes by less than
//!    the convergence threshold (relative, root-mean-square) or the iteration cap is reached.
//!    The transform has twice the grid's points (rounded up to a power of two) in each
//!    indirect dimension.
//! 4. The points that were not measured take the reconstructed values; the measured rows are
//!    left exactly as they were. The combination of step 2 is undone and the direct dimension
//!    transformed back.
//!
//! The noise level is the 25th percentile, over all columns, of the tallest point of each
//! column's zero-filled spectrum (at least 10⁻⁶ of the tallest column, for noise-free data). Columns whose tallest point is below [`SIGNAL_FACTOR`] times
//! that level hold no signal worth reconstructing: their missing points stay zero. Columns are
//! independent, so they are spread over threads; the result does not depend on the number of
//! threads.
//!
//! `Tppi`, `Sequential` and `Series` dimensions are not reconstructed.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use openreadout_core::{Error, Result};

use crate::fft::{Complex, fft_in_place, next_pow2};
use crate::nmr::processnd::{DimensionParameters, Encoding};

/// Default iteration cap of [`NusMode::IterativeSoftThreshold`].
pub const DEFAULT_ITERATIONS: usize = 1000;
/// Default convergence threshold: the relative root-mean-square change of a column's spectrum
/// in one iteration, once the threshold has reached its floor.
pub const DEFAULT_CONVERGENCE: f64 = 1e-4;
/// Factor by which the threshold drops in each iteration.
pub const THRESHOLD_DECAY: f64 = 0.93;
/// A column is reconstructed when its tallest zero-filled point is at least this many times
/// the noise level (module docs).
pub const SIGNAL_FACTOR: f64 = 2.0;
/// Lowest noise level relative to the tallest column (module docs).
const DYNAMIC_RANGE_FLOOR: f64 = 1e-6;
/// Most threads [`reconstruct`] uses.
const MAX_THREADS: usize = 8;

/// How the missing increments of non-uniformly sampled data are filled.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NusMode {
    /// Iterative soft thresholding with [`DEFAULT_ITERATIONS`] and [`DEFAULT_CONVERGENCE`];
    /// zeros (with a note) for encodings it does not handle.
    #[default]
    Default,
    /// Zeros: the spectrum keeps the sampling artefacts.
    ZeroFill,
    /// Iterative soft thresholding (module docs).
    IterativeSoftThreshold {
        /// Iteration cap per column.
        iterations: usize,
        /// Convergence threshold (relative change of a column's spectrum per iteration).
        convergence: f64,
    },
}

/// What was done with the missing increments.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
pub struct NusRecord {
    /// `iterative_soft_threshold` or `zero_fill`.
    pub method: String,
    /// Increments measured (grid points of the indirect dimensions).
    pub sampled_points: usize,
    /// Increments of the full grid.
    pub grid_points: usize,
    /// `sampled_points / grid_points`.
    pub sampled_fraction: f64,
    /// Iteration cap per column (0 for zero filling).
    pub max_iterations: usize,
    /// Convergence threshold (0 for zero filling).
    pub convergence: f64,
    /// Most iterations any column took.
    pub iterations_used: usize,
    /// Direct-dimension points (columns) reconstructed.
    pub columns_reconstructed: usize,
    /// Direct-dimension points (columns) in all.
    pub columns: usize,
}

/// Can [`reconstruct`] handle this encoding?
pub fn reconstructable(e: Encoding) -> bool {
    matches!(
        e,
        Encoding::Complex | Encoding::States | Encoding::StatesTppi | Encoding::EchoAntiecho
    )
}

fn nus_error(what: impl Into<String>) -> Error {
    Error::unsupported(
        "nmr-processing",
        format!("non-uniform sampling: {}", what.into()),
        "The sampling schedule (Bruker `nuslist`) does not fit the acquisition parameters (`NusTD` and the number of indirect dimensions in acqu2s/acqu3s); run `openreadout check <experiment>` to see the mismatch, or process the rows as measured with `--sweep`.",
    )
}

/// Check a sampling schedule against the full grid: one index per indirect dimension, each
/// below the full size. Returns the number of entries that repeat an earlier one (TopSpin
/// schedules can measure an increment more than once).
pub fn check_schedule(schedule: &[Vec<usize>], full: &[usize]) -> Result<usize> {
    let total = full
        .iter()
        .try_fold(1usize, |a, f| a.checked_mul(*f))
        .ok_or_else(|| nus_error("the full grid is too large"))?;
    if total == 0 {
        return Err(nus_error("the full grid has no points"));
    }
    let mut seen = vec![false; total];
    let mut repeats = 0;
    for (j, inc) in schedule.iter().enumerate() {
        if inc.len() != full.len() {
            return Err(nus_error(format!(
                "entry {j} has {} indices for {} indirect dimensions",
                inc.len(),
                full.len()
            )));
        }
        let mut g = 0usize;
        let mut stride = 1usize;
        for (k, (&i, &f)) in inc.iter().zip(full).enumerate() {
            if i >= f {
                return Err(nus_error(format!(
                    "entry {j} samples point {i} of indirect dimension {}, which has {f} points",
                    k + 1
                )));
            }
            g += i * stride;
            stride *= f;
        }
        if std::mem::replace(&mut seen[g], true) {
            repeats += 1;
        }
    }
    Ok(repeats)
}

/// Per-dimension mixing of the rows of one increment into complex sequences (module docs,
/// step 2): `forward[c][p]` and its inverse.
fn mixing(e: Encoding) -> (Vec<Vec<Complex>>, Vec<Vec<Complex>>) {
    let one = Complex::new(1.0, 0.0);
    let zero = Complex::default();
    match e {
        Encoding::States | Encoding::StatesTppi => {
            let i = Complex::new(0.0, 1.0);
            let h = Complex::new(0.5, 0.0);
            (
                vec![vec![one, i], vec![one, -i]],
                vec![
                    vec![h, h],
                    vec![Complex::new(0.0, -0.5), Complex::new(0.0, 0.5)],
                ],
            )
        }
        Encoding::EchoAntiecho => (
            vec![vec![one, zero], vec![zero, one]],
            vec![vec![one, zero], vec![zero, one]],
        ),
        _ => (vec![vec![one]], vec![vec![one]]),
    }
}

/// Transform along every axis of `buf` (axis 0 fastest).
fn fft_nd(buf: &mut [Complex], shape: &[usize], inverse: bool, lane: &mut Vec<Complex>) {
    let mut stride = 1usize;
    for &n in shape {
        if n > 1 {
            let outer = buf.len() / (n * stride);
            lane.resize(n, Complex::default());
            for o in 0..outer {
                for s in 0..stride {
                    let base = o * n * stride + s;
                    for (k, v) in lane.iter_mut().enumerate() {
                        *v = buf[base + k * stride];
                    }
                    // lengths are powers of two by construction
                    let _ = fft_in_place(lane, inverse);
                    for (k, v) in lane.iter().enumerate() {
                        buf[base + k * stride] = *v;
                    }
                }
            }
        }
        stride *= n;
    }
}

/// The geometry shared by every column.
struct Plan<'a> {
    /// Grid points (increments) of the full grid.
    grid: usize,
    /// Padded transform shape and its size.
    pad_shape: Vec<usize>,
    pad: usize,
    /// Padded index of each grid point.
    pad_of: Vec<usize>,
    /// Is the grid point measured?
    measured: &'a [bool],
    /// Forward mixing, `B × B` (channel, part).
    forward: Vec<Vec<Complex>>,
}

impl Plan<'_> {
    /// One reconstruction of channel `y` (length `grid`, zero where not measured), returning
    /// the values at every grid point and the iterations taken.
    fn ist(
        &self,
        y: &[Complex],
        tau0: f64,
        floor: f64,
        iterations: usize,
        convergence: f64,
        scratch: &mut Scratch,
    ) -> (Vec<Complex>, usize) {
        let p = self.pad;
        let inv_p = 1.0 / p as f64;
        let mut s = vec![Complex::default(); p];
        let mut tau = (tau0 * THRESHOLD_DECAY).max(floor);
        let mut used = iterations;
        let t = &mut scratch.time;
        for it in 0..iterations {
            // residual at the measured points
            t.clear();
            t.extend_from_slice(&s);
            fft_nd(t, &self.pad_shape, true, &mut scratch.lane);
            let r = &mut scratch.freq;
            r.clear();
            r.resize(p, Complex::default());
            for (g, yv) in y.iter().enumerate().take(self.grid) {
                if self.measured[g] {
                    let k = self.pad_of[g];
                    r[k] = *yv - t[k].scale(inv_p);
                }
            }
            fft_nd(r, &self.pad_shape, false, &mut scratch.lane);
            let (mut change, mut norm) = (0.0, 0.0);
            for (sv, rv) in s.iter_mut().zip(r.iter()) {
                let gv = *sv + *rv;
                let a = gv.abs();
                let nv = if a > tau {
                    gv.scale(1.0 - tau / a)
                } else {
                    Complex::default()
                };
                let d = nv - *sv;
                change += d.re * d.re + d.im * d.im;
                norm += nv.re * nv.re + nv.im * nv.im;
                *sv = nv;
            }
            if tau <= floor && change <= convergence * convergence * norm {
                used = it + 1;
                break;
            }
            tau = (tau * THRESHOLD_DECAY).max(floor);
        }
        t.clear();
        t.extend_from_slice(&s);
        fft_nd(t, &self.pad_shape, true, &mut scratch.lane);
        let out = (0..self.grid)
            .map(|g| {
                if self.measured[g] {
                    y[g]
                } else {
                    t[self.pad_of[g]].scale(inv_p)
                }
            })
            .collect();
        (out, used)
    }

    /// The tallest point of the zero-filled spectrum of `y`.
    fn tallest(&self, y: &[Complex], scratch: &mut Scratch) -> f64 {
        let r = &mut scratch.freq;
        r.clear();
        r.resize(self.pad, Complex::default());
        for g in 0..self.grid {
            if self.measured[g] {
                r[self.pad_of[g]] = y[g];
            }
        }
        fft_nd(r, &self.pad_shape, false, &mut scratch.lane);
        r.iter().map(|v| v.abs()).fold(0.0, f64::max)
    }
}

#[derive(Default)]
struct Scratch {
    time: Vec<Complex>,
    freq: Vec<Complex>,
    lane: Vec<Complex>,
}

/// Fill the points of `grid` that `schedule` does not list (module docs).
///
/// `grid`: the full grid of raw rows, as [`crate::nmr::indirect::place_on_grid`] returns it for
/// the same `schedule`, `indirect` and `full`; the rows of grid points not in `schedule` are
/// replaced. `schedule`: the measured increments only (one complex-point index per indirect
/// dimension, fastest first). `iterations`: cap per column; `convergence`: relative change of a
/// column's spectrum below which it stops.
pub fn reconstruct(
    grid: &mut [Vec<Complex>],
    schedule: &[Vec<usize>],
    indirect: &[DimensionParameters],
    full: &[usize],
    iterations: usize,
    convergence: f64,
) -> Result<NusRecord> {
    if full.len() != indirect.len() || full.is_empty() {
        return Err(nus_error("one full size per indirect dimension is needed"));
    }
    if let Some(d) = indirect.iter().find(|d| !reconstructable(d.encoding)) {
        return Err(Error::unsupported(
            "nmr-processing",
            format!(
                "reconstruction of non-uniformly sampled {:?} dimensions",
                d.encoding
            ),
            "Reconstruction handles QF, States, States-TPPI and echo/antiecho dimensions; process this data set with zero filling (NusMode::ZeroFill).",
        ));
    }
    if iterations == 0 || !(convergence.is_finite() && convergence >= 0.0) {
        return Err(Error::Usage(
            "NUS reconstruction needs at least one iteration and a finite, non-negative convergence threshold".into(),
        ));
    }
    check_schedule(schedule, full)?;
    let rpp: Vec<usize> = indirect
        .iter()
        .map(|d| d.encoding.rows_per_point())
        .collect();
    let block: usize = rpp.iter().product();
    let points: usize = full.iter().product();
    let rows_total = points
        .checked_mul(block)
        .ok_or_else(|| nus_error("the full grid is too large"))?;
    if grid.len() != rows_total {
        return Err(Error::Usage(format!(
            "NUS reconstruction: the grid has {} rows, the dimensions need {rows_total}",
            grid.len()
        )));
    }
    // row of grid point g, part b
    let mut row_stride = Vec::with_capacity(full.len());
    let mut acc = 1usize;
    for (f, r) in full.iter().zip(&rpp) {
        row_stride.push(acc);
        acc *= f * r;
    }
    let row_of = |g: usize, b: usize| -> usize {
        let (mut gr, mut br, mut row) = (g, b, 0usize);
        for k in 0..full.len() {
            let i = gr % full[k];
            gr /= full[k];
            let part = br % rpp[k];
            br /= rpp[k];
            row += (i * rpp[k] + part) * row_stride[k];
        }
        row
    };
    // mixing over all dimensions (tensor product)
    let mixes: Vec<_> = indirect.iter().map(|d| mixing(d.encoding)).collect();
    let mut forward = vec![vec![Complex::default(); block]; block];
    let mut inverse = vec![vec![Complex::default(); block]; block];
    for c in 0..block {
        for b in 0..block {
            let (mut f, mut i) = (Complex::new(1.0, 0.0), Complex::new(1.0, 0.0));
            let (mut cr, mut br) = (c, b);
            for (k, (fw, inv)) in mixes.iter().enumerate() {
                let (ck, bk) = (cr % rpp[k], br % rpp[k]);
                cr /= rpp[k];
                br /= rpp[k];
                f *= fw[ck][bk];
                i *= inv[bk][ck];
            }
            forward[c][b] = f;
            inverse[b][c] = i;
        }
    }
    let mut measured = vec![false; points];
    for inc in schedule {
        let (mut g, mut stride) = (0usize, 1usize);
        for (&i, &f) in inc.iter().zip(full) {
            g += i * stride;
            stride *= f;
        }
        measured[g] = true;
    }
    let sampled = measured.iter().filter(|m| **m).count();
    let mut rec = NusRecord {
        method: "iterative_soft_threshold".into(),
        sampled_points: sampled,
        grid_points: points,
        sampled_fraction: sampled as f64 / points as f64,
        max_iterations: iterations,
        convergence,
        ..NusRecord::default()
    };
    let len = grid.iter().map(Vec::len).max().unwrap_or(0);
    if sampled == 0 || sampled == points || len == 0 {
        return Ok(rec);
    }
    let n2 = next_pow2(len).ok_or_else(|| nus_error("rows too long"))?;
    rec.columns = n2;
    let pad_shape: Vec<usize> = full
        .iter()
        .map(|&f| next_pow2(f.saturating_mul(2)).unwrap_or(usize::MAX))
        .collect();
    let pad = pad_shape
        .iter()
        .try_fold(1usize, |a, f| a.checked_mul(*f))
        .filter(|&p| p <= crate::nmr::processnd::MAX_ND_POINTS)
        .ok_or_else(|| nus_error("the full grid is too large"))?;
    let mut pad_of = Vec::with_capacity(points);
    for g in 0..points {
        let (mut gr, mut idx, mut stride) = (g, 0usize, 1usize);
        for (f, p) in full.iter().zip(&pad_shape) {
            idx += (gr % f) * stride;
            gr /= f;
            stride *= p;
        }
        pad_of.push(idx);
    }
    // direct-dimension spectra of the measured rows: meas[(mi·B + b)·n2 + j]
    let meas_g: Vec<usize> = (0..points).filter(|&g| measured[g]).collect();
    let mut meas = vec![Complex::default(); sampled * block * n2];
    for (mi, &g) in meas_g.iter().enumerate() {
        for b in 0..block {
            let dst = &mut meas[(mi * block + b) * n2..(mi * block + b + 1) * n2];
            let src = &grid[row_of(g, b)];
            for (d, s) in dst.iter_mut().zip(src) {
                *d = if s.re.is_finite() && s.im.is_finite() {
                    *s
                } else {
                    Complex::default()
                };
            }
            fft_in_place(dst, false).map_err(Error::Other)?;
        }
    }
    let plan = Plan {
        grid: points,
        pad_shape,
        pad,
        pad_of,
        measured: &measured,
        forward,
    };
    let channel = |j: usize, c: usize, y: &mut Vec<Complex>| {
        y.clear();
        y.resize(points, Complex::default());
        for (mi, &g) in meas_g.iter().enumerate() {
            let mut v = Complex::default();
            for b in 0..block {
                v += plan.forward[c][b] * meas[(mi * block + b) * n2 + j];
            }
            y[g] = v;
        }
    };
    // pass 1: the tallest zero-filled point of every column
    let threads = std::thread::available_parallelism()
        .map_or(1, std::num::NonZero::get)
        .clamp(1, MAX_THREADS);
    let tallest: Vec<f64> = run_columns(threads, (0..n2).collect(), |j, scratch| {
        let mut y = Vec::new();
        (0..block)
            .map(|c| {
                channel(j, c, &mut y);
                plan.tallest(&y, scratch)
            })
            .fold(0.0, f64::max)
    });
    let mut sorted = tallest.clone();
    sorted.sort_by(f64::total_cmp);
    // the noise level, and at least 10⁻⁶ of the tallest column (noise-free data)
    let top = sorted.last().copied().unwrap_or(0.0);
    let noise = sorted[(sorted.len() - 1) / 4].max(top * DYNAMIC_RANGE_FLOOR);
    let todo: Vec<usize> = (0..n2)
        .filter(|&j| tallest[j] > 0.0 && tallest[j] >= SIGNAL_FACTOR * noise)
        .collect();
    rec.columns_reconstructed = todo.len();
    // pass 2: reconstruct those columns; unmeasured values per part, (ui·B + b)
    let results = run_columns(threads, todo.clone(), |j, scratch| {
        let mut y = Vec::new();
        let mut chans = Vec::with_capacity(block);
        let mut used = 0;
        for c in 0..block {
            channel(j, c, &mut y);
            let t0 = plan.tallest(&y, scratch);
            if t0 <= 0.0 {
                chans.push(vec![Complex::default(); points]);
                continue;
            }
            let (x, u) = plan.ist(&y, t0, noise, iterations, convergence, scratch);
            used = used.max(u);
            chans.push(x);
        }
        (chans, used)
    });
    // unmeasured rows, transformed back along the direct dimension
    let mut unm: Vec<Vec<Complex>> = Vec::new();
    let unm_g: Vec<usize> = (0..points).filter(|&g| !measured[g]).collect();
    unm.resize(unm_g.len() * block, Vec::new());
    for v in &mut unm {
        *v = vec![Complex::default(); n2];
    }
    for (&j, (chans, used)) in todo.iter().zip(&results) {
        rec.iterations_used = rec.iterations_used.max(*used);
        for (ui, &g) in unm_g.iter().enumerate() {
            for b in 0..block {
                let mut v = Complex::default();
                for (c, ch) in chans.iter().enumerate() {
                    v += inverse[b][c] * ch[g];
                }
                unm[ui * block + b][j] = v;
            }
        }
    }
    let inv_n2 = 1.0 / n2 as f64;
    for (ui, &g) in unm_g.iter().enumerate() {
        for b in 0..block {
            let s = &mut unm[ui * block + b];
            fft_in_place(s, true).map_err(Error::Other)?;
            let row = &mut grid[row_of(g, b)];
            row.clear();
            row.extend(s.iter().take(len).map(|v| v.scale(inv_n2)));
        }
    }
    Ok(rec)
}

/// Run `f` on every column of `cols` over `threads` threads; results in the order of `cols`.
fn run_columns<T: Send>(
    threads: usize,
    cols: Vec<usize>,
    f: impl Fn(usize, &mut Scratch) -> T + Sync,
) -> Vec<T> {
    let threads = threads.min(cols.len()).max(1);
    if threads == 1 {
        let mut scratch = Scratch::default();
        return cols.iter().map(|&j| f(j, &mut scratch)).collect();
    }
    let f = &f;
    let cols = &cols;
    let mut parts: Vec<Vec<(usize, T)>> = std::thread::scope(|sc| {
        let handles: Vec<_> = (0..threads)
            .map(|t| {
                sc.spawn(move || {
                    let mut scratch = Scratch::default();
                    (t..cols.len())
                        .step_by(threads)
                        .map(|k| (k, f(cols[k], &mut scratch)))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or_default())
            .collect()
    });
    let mut out: Vec<Option<T>> = (0..cols.len()).map(|_| None).collect();
    for part in &mut parts {
        for (k, v) in part.drain(..) {
            out[k] = Some(v);
        }
    }
    out.into_iter().flatten().collect()
}

/// A Poisson-gap sampling schedule (Hyberts, Takeuchi, Wagner, J. Am. Chem. Soc. 132 (2010)
/// 2145): `keep` of `points` increments, ascending, always including 0, with gaps drawn from a
/// Poisson distribution whose mean grows as sin(θ) from the start to the end. The same `seed`
/// gives the same schedule.
pub fn poisson_gap_schedule(points: usize, keep: usize, seed: u64) -> Vec<usize> {
    if keep >= points {
        return (0..points).collect();
    }
    if keep == 0 {
        return Vec::new();
    }
    let mut state = seed;
    let mut uniform = move || {
        // SplitMix64
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        ((z >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    };
    let mut lambda = (points as f64 / keep as f64 - 1.0) * std::f64::consts::FRAC_PI_2;
    let mut best: Vec<usize> = Vec::new();
    for _ in 0..10_000 {
        let mut out = Vec::with_capacity(keep + 1);
        let mut i = 0usize;
        while i < points {
            out.push(i);
            let theta = (i as f64 + 0.5) / points as f64 * std::f64::consts::FRAC_PI_2;
            let mean = (lambda * theta.sin()).clamp(0.0, 500.0);
            // Knuth's method
            let limit = (-mean).exp();
            let (mut k, mut prod) = (0usize, uniform());
            while prod > limit {
                k += 1;
                prod *= uniform();
            }
            i = i.saturating_add(1 + k);
        }
        if out.len() == keep {
            return out;
        }
        if best.is_empty() || out.len().abs_diff(keep) < best.len().abs_diff(keep) {
            best.clone_from(&out);
        }
        lambda *= if out.len() > keep { 1.02 } else { 0.98 };
    }
    best.truncate(keep);
    let missing: Vec<usize> = (0..points).filter(|i| !best.contains(i)).collect();
    let short = keep.saturating_sub(best.len());
    best.extend(missing.into_iter().take(short));
    best.sort_unstable();
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nmr::indirect::place_on_grid;

    /// Lines: (F1 frequency in cycles per increment, F2 frequency in cycles per point, amplitude).
    const LINES: [(f64, f64, f64); 3] = [(0.11, 0.2, 1.0), (-0.17, -0.13, 0.6), (0.31, 0.2, 0.3)];

    fn dim(rows: usize, encoding: Encoding) -> DimensionParameters {
        DimensionParameters {
            rows,
            encoding,
            spectral_width_hz: 1000.0,
            ..DimensionParameters::default()
        }
    }

    /// F2 signal of the lines at F1 time `k`; `part(phase)` gives the F1 modulation.
    fn row(len: usize, part: impl Fn(f64) -> Complex, k: f64) -> Vec<Complex> {
        let tau = std::f64::consts::TAU;
        (0..len)
            .map(|t| {
                let mut v = Complex::default();
                for (f1, f2, a) in LINES {
                    let decay = (-(k * 0.06 + t as f64 * 0.03)).exp();
                    let x = Complex::from_phase(tau * f2 * t as f64).scale(a * decay);
                    v += x * part(tau * f1 * k);
                }
                v
            })
            .collect()
    }

    /// The full grid of `m` increments for `e`, rows in `place_on_grid` order.
    fn full_grid(m: usize, len: usize, e: Encoding) -> Vec<Vec<Complex>> {
        let mut out = Vec::new();
        for k in 0..m {
            let k = k as f64;
            match e {
                Encoding::States => {
                    out.push(row(len, |p| Complex::new(p.cos(), 0.0), k));
                    out.push(row(len, |p| Complex::new(p.sin(), 0.0), k));
                }
                Encoding::EchoAntiecho => {
                    out.push(row(len, Complex::from_phase, k));
                    out.push(row(len, |p| Complex::from_phase(-p), k));
                }
                _ => out.push(row(len, Complex::from_phase, k)),
            }
        }
        out
    }

    fn rel_err(a: &[Vec<Complex>], b: &[Vec<Complex>]) -> f64 {
        let (mut d, mut n) = (0.0, 0.0);
        for (x, y) in a.iter().zip(b) {
            for (p, q) in x.iter().zip(y) {
                d += (*p - *q).abs().powi(2);
                n += q.abs().powi(2);
            }
        }
        (d / n).sqrt()
    }

    /// `row` plus white noise of SD `NOISE` (a fixed sequence from `seed`).
    fn noisy(row: &[Complex], seed: usize) -> Vec<Complex> {
        let u = |k: usize| {
            let z = (k as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
            let z = (z ^ (z >> 29)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            ((z >> 11) as f64 / (1u64 << 53) as f64 - 0.5) * 12f64.sqrt()
        };
        row.iter()
            .enumerate()
            .map(|(t, v)| {
                let k = (seed * 4096 + t) * 2;
                *v + Complex::new(u(k), u(k + 1)).scale(NOISE)
            })
            .collect()
    }

    const NOISE: f64 = 0.002;

    fn sampled(full: &[Vec<Complex>], sched: &[usize], rpp: usize) -> Vec<Vec<Complex>> {
        sched
            .iter()
            .flat_map(|&i| (0..rpp).map(move |p| noisy(&full[i * rpp + p], i * rpp + p)))
            .collect()
    }

    #[test]
    fn reconstructs_each_encoding() {
        let (m, len) = (64, 256);
        for e in [
            Encoding::Complex,
            Encoding::States,
            Encoding::StatesTppi,
            Encoding::EchoAntiecho,
        ] {
            let rpp = e.rows_per_point();
            let full = full_grid(
                m,
                len,
                if e == Encoding::StatesTppi {
                    Encoding::States
                } else {
                    e
                },
            );
            let flat = poisson_gap_schedule(m, 24, 7);
            let sched: Vec<Vec<usize>> = flat.iter().map(|&i| vec![i]).collect();
            let d = [dim(m * rpp, e)];
            let mut grid = place_on_grid(&sampled(&full, &flat, rpp), &sched, &d, &[m]).unwrap();
            let zero = rel_err(&grid, &full);
            let rec = reconstruct(
                &mut grid,
                &sched,
                &d,
                &[m],
                DEFAULT_ITERATIONS,
                DEFAULT_CONVERGENCE,
            )
            .unwrap();
            let err = rel_err(&grid, &full);
            // the error includes noise and the truncated F1 decay: a sparse model of
            // Lorentzian lines from 24 of 64 points (zero filled: above 0.5)
            assert!(
                err < 0.2 && err < zero / 3.0,
                "{e:?}: {zero} -> {err} ({rec:?})"
            );
            assert_eq!(rec.sampled_points, 24);
            assert_eq!(rec.grid_points, m);
            assert!(rec.iterations_used > 0 && rec.iterations_used <= DEFAULT_ITERATIONS);
            // measured rows are untouched
            for &i in &flat {
                assert_eq!(grid[i * rpp], noisy(&full[i * rpp], i * rpp));
            }
        }
    }

    #[test]
    fn reconstructs_two_indirect_dimensions_and_is_deterministic() {
        // indirect dimension 1 QF (fastest), indirect dimension 2 States: row
        // i₁ + m1·(part + 2·i₂), as place_on_grid lays them out
        let (m1, m2, len) = (16usize, 16usize, 128usize);
        let tau = std::f64::consts::TAU;
        let mut full = vec![Vec::new(); m1 * m2 * 2];
        for a in 0..m2 {
            for pa in 0..2 {
                for b in 0..m1 {
                    let ph1 = tau * 0.19 * b as f64;
                    let ph2 = tau * -0.23 * a as f64;
                    let m = if pa == 0 { ph2.cos() } else { ph2.sin() };
                    full[b + m1 * (pa + 2 * a)] = (0..len)
                        .map(|t| {
                            Complex::from_phase(ph1 + tau * 0.1 * t as f64)
                                .scale(m * (-(t as f64) * 0.05 - 0.2 * (a + b) as f64).exp())
                        })
                        .collect();
                }
            }
        }
        let d = [dim(m1, Encoding::Complex), dim(2 * m2, Encoding::States)];
        let mut sched = Vec::new();
        for a in 0..m2 {
            for b in 0..m1 {
                if (a * 31 + b * 17 + a * b * 7).wrapping_mul(2_654_435_761) >> 7 & 3 != 0
                    || a + b == 0
                {
                    sched.push(vec![b, a]);
                }
            }
        }
        let rows: Vec<Vec<Complex>> = sched
            .iter()
            .flat_map(|s| {
                (0..2).map(|p| {
                    let r = s[0] + m1 * (p + 2 * s[1]);
                    noisy(&full[r], r)
                })
            })
            .collect();
        let grid0 = place_on_grid(&rows, &sched, &d, &[m1, m2]).unwrap();
        let (mut g1, mut g2) = (grid0.clone(), grid0.clone());
        let zero = rel_err(&grid0, &full);
        reconstruct(&mut g1, &sched, &d, &[m1, m2], 400, 1e-4).unwrap();
        reconstruct(&mut g2, &sched, &d, &[m1, m2], 400, 1e-4).unwrap();
        assert_eq!(g1, g2);
        let err = rel_err(&g1, &full);
        assert!(err < 0.2 && err < zero / 3.0, "{zero} -> {err}");
    }

    #[test]
    fn skips_columns_without_signal() {
        let (m, len) = (32, 256);
        // one line on an F2 grid point: one column holds all of it
        let full: Vec<Vec<Complex>> = (0..m)
            .map(|k| {
                (0..len)
                    .map(|t| {
                        let ph = 0.25 * t as f64 + 0.1 * k as f64;
                        Complex::from_phase(std::f64::consts::TAU * ph)
                    })
                    .collect()
            })
            .collect();
        let flat = poisson_gap_schedule(m, 12, 1);
        let sched: Vec<Vec<usize>> = flat.iter().map(|&i| vec![i]).collect();
        let d = [dim(m, Encoding::Complex)];
        let mut grid = place_on_grid(&sampled(&full, &flat, 1), &sched, &d, &[m]).unwrap();
        let rec = reconstruct(&mut grid, &sched, &d, &[m], 400, 1e-4).unwrap();
        assert_eq!(rec.columns, len);
        assert!(
            rec.columns_reconstructed >= 1 && rec.columns_reconstructed < len / 4,
            "{rec:?}"
        );
        assert!(rel_err(&grid, &full) < 0.05);
    }

    #[test]
    fn bad_schedules_are_refused() {
        assert_eq!(check_schedule(&[vec![0], vec![3]], &[4]).unwrap(), 0);
        assert_eq!(
            check_schedule(&[vec![0], vec![3], vec![0]], &[4]).unwrap(),
            1
        );
        for (s, full) in [
            (vec![vec![0], vec![4]], vec![4]),
            (vec![vec![0, 1]], vec![4]),
            (vec![vec![0]], vec![0]),
        ] {
            let e = check_schedule(&s, &full).unwrap_err();
            assert!(e.hint().is_some_and(|h| h.contains("nuslist")), "{e}");
        }
        let d = [dim(8, Encoding::Tppi)];
        let mut g = vec![vec![Complex::default(); 4]; 8];
        assert!(reconstruct(&mut g, &[vec![0]], &d, &[8], 10, 1e-4).is_err());
        let d = [dim(4, Encoding::Complex)];
        let mut g = vec![vec![Complex::default(); 4]; 4];
        assert!(reconstruct(&mut g, &[vec![0]], &d, &[4], 0, 1e-4).is_err());
        assert!(reconstruct(&mut g, &[vec![0]], &d, &[5], 10, 1e-4).is_err());
    }

    #[test]
    fn poisson_gap_schedules() {
        let s = poisson_gap_schedule(128, 32, 42);
        assert_eq!(s.len(), 32);
        assert_eq!(s[0], 0);
        assert!(s.windows(2).all(|w| w[0] < w[1]) && *s.last().unwrap() < 128);
        assert_eq!(s, poisson_gap_schedule(128, 32, 42));
        assert_ne!(s, poisson_gap_schedule(128, 32, 43));
        // denser at the start than at the end
        let early = s.iter().filter(|&&i| i < 64).count();
        assert!(early > s.len() - early);
        assert_eq!(poisson_gap_schedule(10, 20, 1).len(), 10);
        assert!(poisson_gap_schedule(10, 0, 1).is_empty());
    }
}
