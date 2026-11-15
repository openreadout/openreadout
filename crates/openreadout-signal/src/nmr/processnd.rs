//! Processing of multidimensional (2D, 3D, …) FIDs into spectra, and pseudo-2D series.
//!
//! [`process_nd`] takes the raw rows of a data set (the reader's sweeps, in acquisition order),
//! the direct dimension's acquisition parameters ([`FidParameters`]), one
//! [`DimensionParameters`] per indirect dimension and [`NdOptions`], and returns a
//! [`SpectrumNd`]. [`crate::nmr::indirect::load_processed_nd`] does all of that from a
//! `Dataset`, with the parameters and stored processing the vendor software kept.
//!
//! **Layout.** Indirect dimensions are listed fastest-varying first, as the readers list them
//! (`extra.indirect_dimensions`: Bruker `acqu2s` then `acqu3s`): row `r` of the input is at
//! `r = i₁ + R₁·(i₂ + R₂·…)` with `iₖ` the row index along indirect dimension `k` and `Rₖ` its
//! row count ([`DimensionParameters::rows`], two rows per complex point for hypercomplex
//! encodings). The output is row-major with the slowest dimension first: `shape[0]` is F1 (the
//! slowest indirect dimension), the last entry is the direct dimension, and `axes` follows the
//! same order. For 2D data `real[i·n₂ + j]` is the point at F1 index `i`, F2 index `j`.
//!
//! **Steps.**
//!
//! 1. direct dimension, each row: the 1D steps of [`crate::nmr::process`] (stored `TDeff`,
//!    complex conjugation for Varian/JEOL/Spinsolve, digital-filter group delay, optional linear
//!    prediction, window from the true time zero, first point × factor, zero filling, transform,
//!    the zero-order term of a fractional group delay), then phase;
//! 2. each indirect dimension, fastest first: the rows along it are combined into complex
//!    interferograms according to its [`Encoding`] (below), optionally extended by linear
//!    prediction ([`crate::nmr::lp`]), windowed, first point × factor (default 0.5), zero filled,
//!    transformed and ordered like the direct dimension (index 0 = high frequency), then phased;
//!    a `Series` dimension (pseudo-2D: T1/T2, diffusion, kinetics) is kept as it is;
//! 3. magnitude or power over the dimensions that ask for it, axis reversal, strip (a stored
//!    region of the direct dimension), optional tilt (J-resolved) and symmetrisation, baseline
//!    per dimension, ppm axes.
//!
//! **Encodings** (rows `2k` = `u`, `2k+1` = `v` of a pair; `i₂` the direct dimension's imaginary
//! unit; conventions fixed by reproducing TopSpin's `2rr`, docs/provenance/nmr-processing.md):
//!
//! | encoding | interferogram point `k` |
//! | --- | --- |
//! | `Complex` (Bruker QF) | `conj(row k)` (each row is a complex point; no hypercomplex data) |
//! | `States` | `Re₂(u) + i·Re₂(v)` (and the same for the direct imaginary parts) |
//! | `StatesTppi` | as `States`, × (−1)^k |
//! | `EchoAntiecho` | `Re₂(C) − i·Re₂(S)`, `C = i₂·(u + v)`, `S = u − v` |
//! | `Tppi`, `Sequential` | row k is a real point at twice the rate; `2·N` real points are transformed and the first `N` bins, conjugated, kept |
//! | `Series` | not transformed |
//!
//! Data are held as hypercomplex components: `2^d` real arrays for `d` dimensions, component
//! `m` holding the part that is imaginary in the dimensions whose bit is set in `m` (bit 0 = the
//! direct dimension, bit `k` = indirect dimension `k − 1` in input order). Only `Complex`
//! dimensions leave no imaginary part in the direct dimension.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use openreadout_core::{Error, Result};

use crate::fft::{Complex, fft_in_place, next_pow2};
use crate::nmr::baseline::{self, BaselineMode};
use crate::nmr::lp::{LinearPrediction, predict};
use crate::nmr::phase;
use crate::nmr::process::{FidParameters, PhaseMode, PpmAxis, ppm_axis};
use crate::nmr::window::Window;

/// Largest number of points of one hypercomplex component of the result (2^26 = 64 Mi points).
pub const MAX_ND_POINTS: usize = 1 << 26;

/// How an indirect dimension was sampled (module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Encoding {
    /// Each row is one complex point (Bruker QF: frequency discrimination by coherence
    /// selection, usually shown in magnitude).
    #[default]
    Complex,
    /// Sequential (real) sampling, transformed like `Tppi` (Bruker QSEQ; not verified on vendor
    /// spectra).
    Sequential,
    /// Time-proportional phase increments: real points at twice the rate.
    Tppi,
    /// Hypercomplex pairs (cosine, sine).
    States,
    /// Hypercomplex pairs with the receiver phase alternated by 180° per increment.
    StatesTppi,
    /// Echo/antiecho pairs (gradient selection).
    EchoAntiecho,
    /// Not a Fourier dimension: a series of 1D acquisitions with one parameter varied.
    Series,
}

impl Encoding {
    /// Raw rows per complex point (2 for hypercomplex pairs, else 1).
    pub fn rows_per_point(self) -> usize {
        match self {
            Encoding::States | Encoding::StatesTppi | Encoding::EchoAntiecho => 2,
            _ => 1,
        }
    }
    /// Is the dimension Fourier transformed?
    pub fn is_fourier(self) -> bool {
        self != Encoding::Series
    }
}

/// The values a pseudo-2D series varied from row to row.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
pub struct Series {
    /// What was varied, e.g. `difflist` (gradient strengths), `vdlist` (delays), `vclist` (loop
    /// counters), `delays`.
    pub name: String,
    /// One value per row (may be shorter than the rows when the list is incomplete).
    pub values: Vec<f64>,
    /// Unit of the values, when known (`s`, `ms`, `G/cm`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Where the values came from (a file name or parameter).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// How the stored spectrum was phased in a dimension (Bruker `PH_mod`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StoredPhaseMode {
    /// Phase-sensitive with the stored phases.
    Phased,
    /// Magnitude `|S|`.
    Magnitude,
    /// Power `|S|²`.
    Power,
}

/// Processing values the vendor software stored for one dimension, in this crate's
/// conventions (Bruker `procs`/`proc2s`/…, VnmrJ `procpar`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
pub struct StoredDimension {
    /// Where they came from, e.g. `pdata/1/proc2s`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Transform size in complex points.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<usize>,
    /// Window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<Window>,
    /// Zero-order phase, degrees.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase0_deg: Option<f64>,
    /// First-order phase, degrees.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase1_deg: Option<f64>,
    /// Phase-sensitive, magnitude or power.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase_mode: Option<StoredPhaseMode>,
    /// Factor applied to the first point; `None`: 0.5.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_point_factor: Option<f64>,
    /// Points used: complex points for the direct dimension, raw rows for an indirect one
    /// (Bruker `TDeff`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_domain_points: Option<usize>,
    /// Linear prediction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linear_prediction: Option<LinearPrediction>,
    /// The axis is reversed (Bruker `REVERSE`).
    #[serde(default)]
    pub reverse: bool,
    /// Region kept of the transformed direct dimension: first point and number of points
    /// (Bruker `STSR`, `STSI`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strip: Option<(usize, usize)>,
    /// True when the vendor software also stored a spectrum made with these values.
    #[serde(default)]
    pub spectrum_stored: bool,
}

/// Acquisition parameters of one indirect dimension.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
pub struct DimensionParameters {
    /// Nucleus, e.g. `13C`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nucleus: Option<String>,
    /// Spectral width, Hz (complex sampling rate; for `Tppi` the full width).
    pub spectral_width_hz: f64,
    /// Carrier frequency, MHz.
    pub carrier_frequency_mhz: f64,
    /// Frequency of 0 ppm, MHz.
    pub reference_frequency_mhz: f64,
    /// Raw rows along this dimension (Bruker `TD` of `acqu2s`, VnmrJ `2·ni` for hypercomplex).
    pub rows: usize,
    /// Sampling.
    pub encoding: Encoding,
    /// Conjugate the interferogram (reverses the frequency axis).
    #[serde(default)]
    pub conjugate: bool,
    /// The varied parameter of a `Series` dimension.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series: Option<Series>,
    /// Stored processing of this dimension.
    #[serde(default)]
    pub stored: StoredDimension,
}

/// Linear prediction choice for one dimension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LpMode {
    /// The stored linear prediction, if any.
    #[default]
    Default,
    /// None.
    None,
    /// This one.
    Manual {
        /// The prediction.
        prediction: LinearPrediction,
    },
}

/// Choices for one dimension. `Default` gives the documented defaults.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DimensionOptions {
    /// Transform size in complex points (rounded up to a power of two). `None`: stored size,
    /// else the next power of two ≥ 2 × the points.
    #[serde(default)]
    pub size: Option<usize>,
    /// Window. `None`: the stored window; else a sine bell shifted by π/2 (cosine) for an
    /// indirect dimension and the 1D default (exponential, 0.3 Hz for 1H/19F, 1 Hz otherwise)
    /// for the direct dimension of a series, and a cosine bell for the direct dimension of 2D
    /// data.
    #[serde(default)]
    pub window: Option<Window>,
    /// Phasing (`Default`: the stored mode and phases; else magnitude for `Complex` dimensions
    /// and automatic phasing for the others).
    #[serde(default)]
    pub phase: PhaseMode,
    /// With `PhaseMode::Magnitude`: power `|S|²` instead of `|S|`.
    #[serde(default)]
    pub power: bool,
    /// Baseline correction of the real result along this dimension (default none).
    #[serde(default = "no_baseline")]
    pub baseline: BaselineMode,
    /// Linear prediction.
    #[serde(default)]
    pub linear_prediction: LpMode,
    /// Factor for the first point. `None`: stored, else 0.5.
    #[serde(default)]
    pub first_point_factor: Option<f64>,
    /// Override the encoding (indirect dimensions only).
    #[serde(default)]
    pub encoding: Option<Encoding>,
    /// Reverse the axis. `None`: stored.
    #[serde(default)]
    pub reverse: Option<bool>,
}

fn no_baseline() -> BaselineMode {
    BaselineMode::None
}

impl Default for DimensionOptions {
    fn default() -> Self {
        Self {
            size: None,
            window: None,
            phase: PhaseMode::Default,
            power: false,
            baseline: BaselineMode::None,
            linear_prediction: LpMode::Default,
            first_point_factor: None,
            encoding: None,
            reverse: None,
        }
    }
}

/// Processing choices for a multidimensional data set.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
pub struct NdOptions {
    /// The direct dimension.
    #[serde(default)]
    pub direct: DimensionOptions,
    /// Indirect dimensions, in input order (fastest first); missing entries take defaults.
    #[serde(default)]
    pub indirect: Vec<DimensionOptions>,
    /// Ignore the digital-filter group delay.
    #[serde(default)]
    pub ignore_group_delay: bool,
    /// Symmetrise a square homonuclear 2D spectrum about its diagonal: each point and its mirror
    /// image take the value of smaller magnitude (TopSpin `sym`).
    #[serde(default)]
    pub symmetrize: bool,
    /// Tilt a J-resolved spectrum by 45° (each F1 row shifted along F2 by its F1 offset) and
    /// symmetrise it about F1 = 0 (TopSpin `tilt` and `symj`).
    #[serde(default)]
    pub tilt: bool,
    /// Keep the imaginary (hypercomplex) components in the result.
    #[serde(default)]
    pub keep_components: bool,
}

/// One hypercomplex component of a result.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Component {
    /// Which dimensions this part is imaginary in: bit `k` set for the axis `ndim − 1 − k`
    /// (bit 0 = the direct dimension, the last axis).
    pub mask: u32,
    /// Values, laid out like [`SpectrumNd::real`].
    pub values: Vec<f64>,
}

/// What was done in one dimension.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
pub struct DimensionRecord {
    /// `F1`, `F2`, … (the direct dimension is the last).
    pub label: String,
    /// Nucleus.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nucleus: Option<String>,
    /// Sampling (`None` for the direct dimension).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoding: Option<Encoding>,
    /// Points (complex, or real for `Tppi`) that entered the transform, after linear prediction.
    pub points: usize,
    /// Size of the result along this dimension.
    pub size: usize,
    /// Window applied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<Window>,
    /// Where the window came from: `stored`, `default`, `option`.
    pub window_source: String,
    /// Phase mode used: `stored`, `auto`, `manual`, `magnitude`, `power`, `none`.
    pub phase_mode: String,
    /// Zero-order phase applied, degrees.
    pub phase0_deg: f64,
    /// First-order phase applied, degrees.
    pub phase1_deg: f64,
    /// Linear prediction applied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linear_prediction: Option<LinearPrediction>,
    /// First-point factor.
    pub first_point_factor: f64,
    /// Group delay removed (direct dimension), points.
    #[serde(default)]
    pub group_delay_points: f64,
    /// The axis was reversed.
    #[serde(default)]
    pub reversed: bool,
    /// Baseline correction applied.
    pub baseline: BaselineMode,
    /// Stored values found (may be unused).
    pub stored: StoredDimension,
}

/// What [`process_nd`] did.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
pub struct NdRecord {
    /// Raw rows read.
    pub rows: usize,
    /// One entry per output axis, slowest first.
    pub dimensions: Vec<DimensionRecord>,
    /// Steps on the whole data set, in order (`magnitude`, `tilt`, `symmetrize`, …).
    pub steps: Vec<String>,
    /// Remarks (fallbacks, conventions not verified, clamping).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// A processed multidimensional spectrum (or pseudo-2D stack of spectra).
#[derive(Debug, Clone, Default)]
pub struct SpectrumNd {
    /// Size per axis, slowest first (2D: `[F1, F2]`).
    pub shape: Vec<usize>,
    /// The real (absorptive, or magnitude) part, row-major.
    pub real: Vec<f64>,
    /// Imaginary hypercomplex components (with `NdOptions::keep_components`, phase-sensitive
    /// results only).
    pub components: Vec<Component>,
    /// One axis per entry of `shape`. The axis of a `Series` dimension has `step_ppm` 0 and
    /// `reference_frequency_mhz` 0 (its points are the rows; see `series`).
    pub axes: Vec<PpmAxis>,
    /// The varied parameter of a pseudo-2D series (the slowest axis).
    pub series: Option<Series>,
    /// What was done.
    pub record: NdRecord,
}

/// How [`SpectrumNd::projection`] combines the points it collapses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Projection {
    /// Largest value (positive skyline).
    #[default]
    Skyline,
    /// Smallest value (negative skyline).
    NegativeSkyline,
    /// Sum.
    Sum,
}

impl SpectrumNd {
    /// Number of dimensions.
    pub fn ndim(&self) -> usize {
        self.shape.len()
    }

    fn strides(&self) -> Vec<usize> {
        let mut s = vec![1; self.shape.len()];
        for a in (0..self.shape.len().saturating_sub(1)).rev() {
            s[a] = s[a + 1] * self.shape[a + 1];
        }
        s
    }

    /// The value at `index` (one entry per axis), if inside.
    pub fn get(&self, index: &[usize]) -> Option<f64> {
        if index.len() != self.shape.len() || index.iter().zip(&self.shape).any(|(i, n)| i >= n) {
            return None;
        }
        let off: usize = index.iter().zip(self.strides()).map(|(i, s)| i * s).sum();
        self.real.get(off).copied()
    }

    /// Nearest point index of `ppm` on `axis` (clamped), or `None` for an axis out of range.
    pub fn point_of(&self, axis: usize, ppm: f64) -> Option<usize> {
        let a = self.axes.get(axis)?;
        let n = *self.shape.get(axis)?;
        if n == 0 {
            return None;
        }
        let k = a.index_of(ppm).round();
        Some(if k.is_finite() {
            (k.max(0.0) as usize).min(n - 1)
        } else {
            0
        })
    }

    /// The 1D trace along `axis` through the point `at` (indices of the other axes, in axis
    /// order, skipping `axis`).
    pub fn trace(&self, axis: usize, at: &[usize]) -> Option<Vec<f64>> {
        let nd = self.ndim();
        if axis >= nd || at.len() + 1 != nd {
            return None;
        }
        let strides = self.strides();
        let mut base = 0;
        let mut it = at.iter();
        for (a, (&n, &s)) in self.shape.iter().zip(&strides).enumerate() {
            if a == axis {
                continue;
            }
            let i = *it.next()?;
            if i >= n {
                return None;
            }
            base += i * s;
        }
        Some(
            (0..self.shape[axis])
                .map(|k| self.real[base + k * strides[axis]])
                .collect(),
        )
    }

    /// The 1D trace along `axis` through the point nearest to the shifts `at_ppm` (one per other
    /// axis, in axis order). For 2D data, `trace_at_ppm(1, &[f1])` is the row (F2 trace) at F1
    /// shift `f1`, `trace_at_ppm(0, &[f2])` the column at F2 shift `f2`.
    pub fn trace_at_ppm(&self, axis: usize, at_ppm: &[f64]) -> Option<Vec<f64>> {
        let mut idx = Vec::with_capacity(at_ppm.len());
        let mut it = at_ppm.iter();
        for a in 0..self.ndim() {
            if a == axis {
                continue;
            }
            idx.push(self.point_of(a, *it.next()?)?);
        }
        self.trace(axis, &idx)
    }

    /// Projection onto `axis`: every point along `axis` combines all points with that index.
    pub fn projection(&self, axis: usize, how: Projection) -> Option<Vec<f64>> {
        if axis >= self.ndim() {
            return None;
        }
        let strides = self.strides();
        let n = self.shape[axis];
        let mut out = vec![
            match how {
                Projection::Skyline => f64::NEG_INFINITY,
                Projection::NegativeSkyline => f64::INFINITY,
                Projection::Sum => 0.0,
            };
            n
        ];
        for (off, &v) in self.real.iter().enumerate() {
            let k = (off / strides[axis]) % n;
            match how {
                Projection::Skyline => out[k] = out[k].max(v),
                Projection::NegativeSkyline => out[k] = out[k].min(v),
                Projection::Sum => out[k] += v,
            }
        }
        for v in &mut out {
            if !v.is_finite() {
                *v = 0.0;
            }
        }
        Some(out)
    }
}

/// Hypercomplex data being processed: `comps[m]` row-major over `shape`.
struct Cube {
    shape: Vec<usize>,
    comps: Vec<Vec<f64>>,
}

impl Cube {
    fn ndim(&self) -> usize {
        self.shape.len()
    }
    fn bit(&self, axis: usize) -> usize {
        1 << (self.ndim() - 1 - axis)
    }
    /// Replace axis `axis` (length `L`) by `new_len`: `f(lanes_in, lanes_out)` gets every
    /// component's lane (`comps × L`) and fills `comps × new_len`.
    fn map_axis(
        &mut self,
        axis: usize,
        new_len: usize,
        mut f: impl FnMut(&[Vec<f64>], &mut [Vec<f64>]),
    ) {
        let len = self.shape[axis];
        let inner: usize = self.shape[axis + 1..].iter().product();
        let outer: usize = self.shape[..axis].iter().product();
        let nc = self.comps.len();
        let total = outer * new_len * inner;
        let mut out = vec![vec![0.0; total]; nc];
        let mut lin = vec![vec![0.0; len]; nc];
        let mut lout = vec![vec![0.0; new_len]; nc];
        for o in 0..outer {
            for i in 0..inner {
                for (c, l) in lin.iter_mut().enumerate() {
                    let src = &self.comps[c];
                    for (k, v) in l.iter_mut().enumerate() {
                        *v = src[(o * len + k) * inner + i];
                    }
                }
                for l in &mut lout {
                    l.fill(0.0);
                }
                f(&lin, &mut lout);
                for (c, l) in lout.iter().enumerate() {
                    let dst = &mut out[c];
                    for (k, v) in l.iter().enumerate() {
                        dst[(o * new_len + k) * inner + i] = *v;
                    }
                }
            }
        }
        self.comps = out;
        self.shape[axis] = new_len;
    }
}

fn bad(what: impl Into<String>) -> Error {
    Error::unsupported(
        "nmr-processing",
        format!("multidimensional processing: {}", what.into()),
        "The acquisition parameters of a dimension are missing or invalid; run `openreadout info <file>` (traces[].extra.indirect_dimensions) to see them, or process one row with `--sweep`.",
    )
}

fn pow2_size(want: usize, label: &str, notes: &mut Vec<String>) -> Result<usize> {
    let n = next_pow2(want.max(2)).unwrap_or(usize::MAX);
    if n != want {
        notes.push(format!(
            "{label}: transform size {want} rounded up to {n} (power of two)"
        ));
    }
    if n > crate::nmr::process::MAX_TRANSFORM_SIZE {
        return Err(Error::Usage(format!(
            "{label}: transform size {n} exceeds the maximum {} points",
            crate::nmr::process::MAX_TRANSFORM_SIZE
        )));
    }
    Ok(n)
}

/// Transform `x` (already windowed, any length ≤ n) into our order: index 0 = high frequency,
/// zero frequency at `n/2`.
fn fft_ordered(x: &[Complex], n: usize) -> Result<Vec<Complex>> {
    let mut buf = vec![Complex::default(); n];
    for (d, s) in buf.iter_mut().zip(x) {
        *d = *s;
    }
    fft_in_place(&mut buf, false).map_err(Error::Other)?;
    let half = n / 2;
    Ok((0..n).map(|k| buf[(half + n - k) % n]).collect())
}

fn phase_lane(re: &mut [f64], im: &mut [f64], p0: f64, p1: f64) {
    let mut s: Vec<Complex> = re
        .iter()
        .zip(im.iter())
        .map(|(a, b)| Complex::new(*a, *b))
        .collect();
    phase::apply_phase(&mut s, p0, p1);
    for ((a, b), v) in re.iter_mut().zip(im.iter_mut()).zip(s) {
        *a = v.re;
        *b = v.im;
    }
}

struct PhaseChoice {
    mode: &'static str,
    p0: f64,
    p1: f64,
}

/// Decide the phase of a dimension; `auto` computes automatic phases when needed.
fn choose_phase(
    opt: &DimensionOptions,
    stored: &StoredDimension,
    default_magnitude: bool,
    label: &str,
    auto: &mut dyn FnMut() -> (f64, f64),
) -> Result<PhaseChoice> {
    let stored_phases = stored
        .phase0_deg
        .map(|p0| (p0, stored.phase1_deg.unwrap_or(0.0)));
    let usable = stored_phases.filter(|(a, b)| {
        a.is_finite() && b.is_finite() && (stored.spectrum_stored || *a != 0.0 || *b != 0.0)
    });
    let mag = |power: bool| PhaseChoice {
        mode: if power { "power" } else { "magnitude" },
        p0: 0.0,
        p1: 0.0,
    };
    Ok(match opt.phase {
        PhaseMode::Default => match stored.phase_mode {
            Some(StoredPhaseMode::Magnitude) => mag(false),
            Some(StoredPhaseMode::Power) => mag(true),
            _ => match usable {
                Some((a, b)) => PhaseChoice {
                    mode: "stored",
                    p0: a,
                    p1: b,
                },
                None if default_magnitude => mag(false),
                None => {
                    let (a, b) = auto();
                    PhaseChoice {
                        mode: "auto",
                        p0: a,
                        p1: b,
                    }
                }
            },
        },
        PhaseMode::Stored => match stored_phases {
            Some((a, b)) => PhaseChoice {
                mode: "stored",
                p0: a,
                p1: b,
            },
            None => {
                return Err(Error::unsupported(
                    "nmr-processing",
                    format!("stored phases of {label}: this data set has none"),
                    "Use automatic phasing, magnitude mode or explicit phases for that dimension.",
                ));
            }
        },
        PhaseMode::Auto => {
            let (a, b) = auto();
            PhaseChoice {
                mode: "auto",
                p0: a,
                p1: b,
            }
        }
        PhaseMode::Manual {
            phase0_deg,
            phase1_deg,
        } => PhaseChoice {
            mode: "manual",
            p0: phase0_deg,
            p1: phase1_deg,
        },
        PhaseMode::Magnitude => mag(opt.power),
        PhaseMode::None => PhaseChoice {
            mode: "none",
            p0: 0.0,
            p1: 0.0,
        },
    })
}

fn first_point(opt: &DimensionOptions, stored: &StoredDimension) -> f64 {
    opt.first_point_factor
        .or(stored.first_point_factor)
        .filter(|v| v.is_finite() && (0.0..=2.0).contains(v))
        .unwrap_or(0.5)
}

fn lp_choice(opt: &DimensionOptions, stored: &StoredDimension) -> Option<LinearPrediction> {
    match opt.linear_prediction {
        LpMode::Default => stored.linear_prediction,
        LpMode::None => None,
        LpMode::Manual { prediction } => Some(prediction),
    }
    .filter(|l| l.coefficients > 0)
}

fn label(axis: usize) -> String {
    format!("F{}", axis + 1)
}

/// Direct-dimension transform of one row (1D steps without phase).
struct DirectPlan {
    used: usize,
    n: usize,
    gi: usize,
    frac: f64,
    weights: Vec<f64>,
    fcor: f64,
    conjugate: bool,
    lp: Option<(LinearPrediction, usize)>,
}

impl DirectPlan {
    fn run(&self, row: &[Complex]) -> Result<Vec<Complex>> {
        let n = self.n;
        let mut x = vec![Complex::default(); n];
        for (d, s) in x.iter_mut().zip(row.iter().take(self.used)) {
            let v = if self.conjugate { s.conj() } else { *s };
            *d = if v.re.is_finite() && v.im.is_finite() {
                v
            } else {
                Complex::default()
            };
        }
        if self.gi > 0 {
            x.rotate_left(self.gi);
        }
        let live = n - self.gi;
        if let Some((lp, points)) = &self.lp {
            let have = self.used.saturating_sub(self.gi).min(live);
            let ext = predict(&x[..have], lp, *points);
            for (d, s) in x.iter_mut().take(live).zip(ext) {
                *d = s;
            }
        }
        for (i, v) in x.iter_mut().take(live).enumerate() {
            *v = v.scale(self.weights.get(i).copied().unwrap_or(0.0));
        }
        x[0] = x[0].scale(self.fcor);
        fft_in_place(&mut x, false).map_err(Error::Other)?;
        if self.frac > 0.0 {
            let w = 2.0 * std::f64::consts::PI * self.frac / n as f64;
            let z = std::f64::consts::PI * (1.0 - self.frac);
            let half = n / 2;
            for (j, v) in x.iter_mut().enumerate() {
                let f = if j < half {
                    j as f64
                } else {
                    j as f64 - n as f64
                };
                *v *= Complex::from_phase(w * f + z);
            }
        }
        let half = n / 2;
        Ok((0..n).map(|k| x[(half + n - k) % n]).collect())
    }
}

fn default_window_direct(p: &FidParameters, series: bool) -> Window {
    if series {
        let lb = match p.nucleus.as_deref().map(str::trim) {
            Some("1H" | "3H" | "19F" | "H1" | "F19") => 0.3,
            _ => 1.0,
        };
        Window::Exponential {
            line_broadening_hz: lb,
        }
    } else {
        Window::Sine {
            offset: 0.5,
            power: 2.0,
        }
    }
}

/// Representative F2 spectrum for automatic direct-dimension phasing: the `t = 0` point of the
/// first indirect dimension after its encoding is applied (a series: its strongest row).
fn direct_reference(cube: &Cube, enc: Option<Encoding>) -> Vec<Complex> {
    let n2 = *cube.shape.last().unwrap_or(&0);
    let row = |r: usize| -> Vec<Complex> {
        (0..n2)
            .map(|j| Complex::new(cube.comps[0][r * n2 + j], cube.comps[1][r * n2 + j]))
            .collect()
    };
    let rows = cube.comps[0].len() / n2.max(1);
    if rows == 0 {
        return Vec::new();
    }
    match enc {
        Some(Encoding::EchoAntiecho) if rows >= 2 => {
            let (u, v) = (row(0), row(1));
            u.iter()
                .zip(&v)
                .map(|(a, b)| {
                    let s = *a + *b;
                    Complex::new(-s.im, s.re)
                })
                .collect()
        }
        Some(Encoding::Series) => {
            let best = (0..rows)
                .max_by(|&a, &b| {
                    let e = |r: usize| -> f64 {
                        (0..n2)
                            .map(|j| {
                                cube.comps[0][r * n2 + j].abs() + cube.comps[1][r * n2 + j].abs()
                            })
                            .sum()
                    };
                    e(a).total_cmp(&e(b))
                })
                .unwrap_or(0);
            row(best)
        }
        _ => row(0),
    }
}

/// Process a multidimensional FID (module docs).
///
/// `rows`: the raw complex rows (direct-dimension FIDs) in acquisition order; `direct`,
/// `direct_stored`: the direct dimension; `indirect`: one entry per indirect dimension,
/// fastest-varying first. Rows missing at the end (an interrupted acquisition) are zero; extra
/// rows are ignored. For non-uniformly sampled data, place the measured rows on the full grid
/// first (zeros or reconstructed values) and pass the full grid: see
/// [`crate::nmr::indirect::place_on_grid`].
pub fn process_nd(
    rows: &[Vec<Complex>],
    direct: &FidParameters,
    direct_stored: &StoredDimension,
    indirect: &[DimensionParameters],
    opts: &NdOptions,
) -> Result<SpectrumNd> {
    let mut rec = NdRecord::default();
    let nd = indirect.len() + 1;
    if nd > 4 {
        return Err(bad("more than four dimensions"));
    }
    if !(direct.spectral_width_hz.is_finite() && direct.spectral_width_hz > 0.0) {
        return Err(bad("the direct dimension has no spectral width"));
    }
    if !(direct.reference_frequency_mhz.is_finite() && direct.reference_frequency_mhz > 0.0) {
        return Err(bad("the direct dimension has no spectrometer frequency"));
    }
    if !direct.group_delay_points.is_finite() || direct.group_delay_points < 0.0 {
        return Err(bad("negative or non-finite group delay"));
    }
    let enc: Vec<Encoding> = indirect
        .iter()
        .enumerate()
        .map(|(k, d)| {
            opts.indirect
                .get(k)
                .and_then(|o| o.encoding)
                .unwrap_or(d.encoding)
        })
        .collect();
    for (k, d) in indirect.iter().enumerate() {
        if d.rows == 0 {
            return Err(bad(format!("indirect dimension {} has no rows", k + 1)));
        }
        if enc[k].is_fourier() && !(d.spectral_width_hz.is_finite() && d.spectral_width_hz > 0.0) {
            return Err(bad(format!(
                "indirect dimension {} has no spectral width",
                k + 1
            )));
        }
    }
    let want_rows: usize = indirect
        .iter()
        .try_fold(1usize, |a, d| a.checked_mul(d.rows))
        .ok_or_else(|| bad("too many rows"))?;
    if rows.is_empty() || rows.iter().all(Vec::is_empty) {
        return Err(bad("no data"));
    }
    rec.rows = rows.len();
    if rows.len() < want_rows {
        rec.notes.push(format!(
            "{} of {want_rows} rows acquired; the missing rows are zero",
            rows.len()
        ));
    } else if rows.len() > want_rows {
        rec.notes.push(format!(
            "{} rows read, {want_rows} used (the dimensions' row counts)",
            rows.len()
        ));
    }
    let series_any = enc.contains(&Encoding::Series);

    // ---- direct dimension
    let dopt = &opts.direct;
    let fid_len = rows.iter().map(Vec::len).max().unwrap_or(0);
    let used = direct_stored
        .time_domain_points
        .filter(|&k| k > 0 && k < fid_len)
        .unwrap_or(fid_len);
    if used < fid_len {
        rec.notes.push(format!(
            "direct dimension: {used} of {fid_len} points used (stored processing)"
        ));
    }
    let lp_d = lp_choice(dopt, direct_stored);
    let lp_points = lp_d.map_or(0, |l| match l.direction {
        crate::nmr::lp::LpDirection::Backward => 0,
        _ => l.points,
    });
    let n2 = pow2_size(
        dopt.size
            .or(direct_stored.size)
            .unwrap_or_else(|| used.max(lp_points).saturating_mul(2)),
        "direct dimension",
        &mut rec.notes,
    )?;
    let g = if opts.ignore_group_delay {
        0.0
    } else {
        direct.group_delay_points
    };
    let gi = g.floor() as usize;
    let (gi, frac) = if gi >= n2.min(used) {
        rec.notes.push(format!(
            "group delay {g} points is not shorter than the data; not removed"
        ));
        (0, 0.0)
    } else {
        (gi, g - gi as f64)
    };
    let (window_d, wsrc_d) = match dopt.window {
        Some(w) => (w, "option"),
        None => match direct_stored.window {
            Some(w) => (w, "stored"),
            None => (default_window_direct(direct, series_any), "default"),
        },
    };
    let live_points = match lp_d {
        Some(l) if l.direction != crate::nmr::lp::LpDirection::Backward => {
            l.points.saturating_sub(gi).max(used.saturating_sub(gi))
        }
        _ => used.saturating_sub(gi),
    }
    .min(n2 - gi);
    let plan = DirectPlan {
        used,
        n: n2,
        gi,
        frac,
        weights: window_d.weights(live_points, 1.0 / direct.spectral_width_hz),
        fcor: first_point(dopt, direct_stored),
        conjugate: direct.conjugate,
        lp: lp_d.map(|l| (l, l.points.saturating_sub(gi))),
    };
    let mut shape: Vec<usize> = indirect.iter().rev().map(|d| d.rows).collect();
    shape.push(n2);
    let per_comp = want_rows
        .checked_mul(n2)
        .filter(|&v| v <= MAX_ND_POINTS)
        .ok_or_else(|| {
            Error::Usage(format!(
                "the data set needs more than {MAX_ND_POINTS} points per component; use smaller transform sizes"
            ))
        })?;
    let ncomp = 1usize << nd;
    let mut cube = Cube {
        shape,
        comps: vec![vec![0.0; per_comp]; ncomp],
    };
    for (r, row) in rows.iter().take(want_rows).enumerate() {
        let s = plan.run(row)?;
        let base = r * n2;
        for (j, v) in s.iter().enumerate() {
            cube.comps[0][base + j] = v.re;
            cube.comps[1][base + j] = v.im;
        }
    }
    let mut drec = DimensionRecord {
        label: label(nd - 1),
        nucleus: direct.nucleus.clone(),
        encoding: None,
        points: live_points,
        size: n2,
        window: Some(window_d),
        window_source: wsrc_d.into(),
        linear_prediction: lp_d,
        first_point_factor: plan.fcor,
        group_delay_points: gi as f64 + frac,
        baseline: dopt.baseline,
        stored: direct_stored.clone(),
        ..DimensionRecord::default()
    };
    // direct phase
    let first_enc = enc.first().copied();
    let default_mag_d = false;
    let mut auto_d = || {
        let v = direct_reference(&cube, first_enc);
        phase::autophase(&v)
    };
    let mut pc = choose_phase(
        dopt,
        direct_stored,
        default_mag_d,
        "the direct dimension",
        &mut auto_d,
    )?;
    if dopt.phase == PhaseMode::Default && pc.mode == "auto" && first_enc == Some(Encoding::Complex)
    {
        // a magnitude F1 makes the F2 phase irrelevant; skip the search
        pc = PhaseChoice {
            mode: "none",
            p0: 0.0,
            p1: 0.0,
        };
    }
    let mut magnitude_bits: usize = 0;
    let mut power = false;
    if pc.mode == "stored" || pc.mode == "auto" || pc.mode == "manual" {
        let (p0, p1) = (pc.p0, pc.p1);
        let last = nd - 1;
        cube.map_axis(last, n2, |lin, lout| {
            for c in (0..ncomp).filter(|c| c & 1 == 0) {
                let (mut re, mut im) = (lin[c].clone(), lin[c | 1].clone());
                phase_lane(&mut re, &mut im, p0, p1);
                lout[c].copy_from_slice(&re);
                lout[c | 1].copy_from_slice(&im);
            }
        });
    } else if pc.mode == "magnitude" || pc.mode == "power" {
        magnitude_bits |= 1;
        power |= pc.mode == "power";
    }
    drec.phase_mode = pc.mode.into();
    drec.phase0_deg = pc.p0;
    drec.phase1_deg = pc.p1;
    let mut records: Vec<DimensionRecord> = Vec::new();

    // ---- indirect dimensions, fastest first
    for (k, d) in indirect.iter().enumerate() {
        let axis = nd - 2 - k;
        let bit = cube.bit(axis);
        let e = enc[k];
        let default_opt = DimensionOptions::default();
        let o = opts.indirect.get(k).unwrap_or(&default_opt);
        let st = &d.stored;
        let mut drk = DimensionRecord {
            label: label(axis),
            nucleus: d.nucleus.clone(),
            encoding: Some(e),
            baseline: o.baseline,
            stored: st.clone(),
            ..DimensionRecord::default()
        };
        let lp = if e.is_fourier() {
            lp_choice(o, st)
        } else {
            None
        };
        let rows_k = d.rows;
        let used_rows = match lp {
            Some(_) => rows_k,
            None => st
                .time_domain_points
                .filter(|&t| t > 0 && t < rows_k)
                .unwrap_or(rows_k),
        };
        if used_rows < rows_k {
            rec.notes.push(format!(
                "{}: {used_rows} of {rows_k} rows used (stored processing)",
                drk.label
            ));
        }
        if e == Encoding::Series {
            cube.map_axis(axis, used_rows, |lin, lout| {
                for (c, l) in lout.iter_mut().enumerate() {
                    l.copy_from_slice(&lin[c][..used_rows]);
                }
            });
            drk.points = used_rows;
            drk.size = used_rows;
            drk.phase_mode = "none".into();
            drk.window_source = "none".into();
            records.push(drk);
            continue;
        }
        let rpp = e.rows_per_point();
        let m = used_rows / rpp;
        if m == 0 {
            return Err(bad(format!("{}: fewer rows than one point", drk.label)));
        }
        if used_rows % rpp != 0 {
            rec.notes.push(format!(
                "{}: odd number of rows ({used_rows}) for pairs; the last row is ignored",
                drk.label
            ));
        }
        let tppi = matches!(e, Encoding::Tppi | Encoding::Sequential);
        if e == Encoding::Sequential {
            rec.notes.push(format!(
                "{}: sequential sampling transformed like TPPI (not verified on vendor spectra)",
                drk.label
            ));
        }
        let lp_m = lp.map(|l| match l.direction {
            crate::nmr::lp::LpDirection::Backward => l.points / rpp,
            _ => (l.points / rpp).max(m),
        });
        let points = match (lp, lp_m) {
            (Some(l), Some(p)) if l.direction != crate::nmr::lp::LpDirection::Backward => p,
            _ => m,
        };
        let n = pow2_size(
            o.size
                .or(st.size)
                .unwrap_or_else(|| points.saturating_mul(2)),
            &drk.label,
            &mut rec.notes,
        )?;
        let (win, wsrc) = match o.window {
            Some(w) => (w, "option"),
            None => match st.window {
                Some(w) => (w, "stored"),
                None => (
                    Window::Sine {
                        offset: 0.5,
                        power: 2.0,
                    },
                    "default",
                ),
            },
        };
        let dwell = if tppi {
            0.5 / d.spectral_width_hz
        } else {
            1.0 / d.spectral_width_hz
        };
        let wlen = points.min(if tppi { 2 * n } else { n });
        let weights = win.weights(wlen, dwell);
        let fcor = first_point(o, st);
        let conj = d.conjugate;
        let ncomp_l = ncomp;
        let mut err: Option<Error> = None;
        cube.map_axis(axis, n, |lin, lout| {
            if err.is_some() {
                return;
            }
            for c in (0..ncomp_l).filter(|c| c & bit == 0) {
                // the interferogram (complex along this axis) of component c
                let z: Vec<Complex> = match e {
                    Encoding::Complex => {
                        if c & 1 != 0 {
                            continue;
                        }
                        (0..m)
                            .map(|i| Complex::new(lin[c][i], -lin[c | 1][i]))
                            .collect()
                    }
                    Encoding::States | Encoding::StatesTppi => (0..m)
                        .map(|i| {
                            let s = if e == Encoding::StatesTppi && i % 2 == 1 {
                                -1.0
                            } else {
                                1.0
                            };
                            Complex::new(lin[c][2 * i], lin[c][2 * i + 1]).scale(s)
                        })
                        .collect(),
                    Encoding::EchoAntiecho => (0..m)
                        .map(|i| {
                            let w = |cc: usize| lin[cc][2 * i] + lin[cc][2 * i + 1];
                            let x = lin[c][2 * i] - lin[c][2 * i + 1];
                            let cre = if c & 1 == 0 { -w(c | 1) } else { w(c & !1) };
                            Complex::new(cre, -x)
                        })
                        .collect(),
                    Encoding::Tppi | Encoding::Sequential => {
                        (0..m).map(|i| Complex::new(lin[c][i], 0.0)).collect()
                    }
                    Encoding::Series => continue,
                };
                let mut z = if conj && !tppi {
                    z.into_iter().map(Complex::conj).collect()
                } else {
                    z
                };
                if let (Some(l), Some(p)) = (lp, lp_m) {
                    z = predict(&z, &l, p);
                }
                if tppi {
                    // real transform: 2n real points, first n bins conjugated
                    let mut buf: Vec<Complex> = z
                        .iter()
                        .take(wlen)
                        .zip(&weights)
                        .map(|(v, w)| Complex::new(v.re * w, 0.0))
                        .collect();
                    if let Some(f) = buf.first_mut() {
                        *f = f.scale(fcor);
                    }
                    buf.resize(2 * n, Complex::default());
                    if let Err(x) = fft_in_place(&mut buf, false) {
                        err = Some(Error::Other(x));
                        return;
                    }
                    for i in 0..n {
                        // conjugate: the frequency axis mirrored (ν → SW − ν)
                        let v = if conj { buf[n - i] } else { buf[i] }.conj();
                        lout[c][i] = v.re;
                        lout[c | bit][i] = v.im;
                    }
                } else {
                    let mut w: Vec<Complex> = z
                        .iter()
                        .take(wlen)
                        .zip(&weights)
                        .map(|(v, w)| v.scale(*w))
                        .collect();
                    if let Some(f) = w.first_mut() {
                        *f = f.scale(fcor);
                    }
                    match fft_ordered(&w, n) {
                        Ok(s) => {
                            for (i, v) in s.iter().enumerate() {
                                lout[c][i] = v.re;
                                lout[c | bit][i] = v.im;
                            }
                        }
                        Err(x) => {
                            err = Some(x);
                            return;
                        }
                    }
                }
            }
        });
        if let Some(x) = err {
            return Err(x);
        }
        if e == Encoding::Complex {
            // the direct imaginary part went into this dimension
            for c in (0..ncomp).filter(|c| c & 1 != 0) {
                cube.comps[c].iter_mut().for_each(|v| *v = 0.0);
            }
        }
        drk.points = points;
        drk.size = n;
        drk.window = Some(win);
        drk.window_source = wsrc.into();
        drk.linear_prediction = lp;
        drk.first_point_factor = fcor;
        // phase
        let mut auto_k = || {
            // the column (along this axis) through the strongest point of the real part
            let inner: usize = cube.shape[axis + 1..].iter().product();
            let (best, _) = cube.comps[0]
                .iter()
                .zip(&cube.comps[bit])
                .enumerate()
                .map(|(i, (a, b))| (i, a.hypot(*b)))
                .fold((0, -1.0), |acc, x| if x.1 > acc.1 { x } else { acc });
            let o_idx = best / (n * inner);
            let i_idx = best % inner;
            let v: Vec<Complex> = (0..n)
                .map(|kk| {
                    let off = (o_idx * n + kk) * inner + i_idx;
                    Complex::new(cube.comps[0][off], cube.comps[bit][off])
                })
                .collect();
            phase::autophase(&v)
        };
        let pc = choose_phase(o, st, e == Encoding::Complex, &drk.label, &mut auto_k)?;
        if matches!(pc.mode, "stored" | "auto" | "manual") {
            let (p0, p1) = (pc.p0, pc.p1);
            cube.map_axis(axis, n, |lin, lout| {
                for c in 0..ncomp {
                    lout[c].copy_from_slice(&lin[c]);
                }
                for c in (0..ncomp).filter(|c| c & bit == 0) {
                    let (mut re, mut im) = (lin[c].clone(), lin[c | bit].clone());
                    phase_lane(&mut re, &mut im, p0, p1);
                    lout[c].copy_from_slice(&re);
                    lout[c | bit].copy_from_slice(&im);
                }
            });
        } else if matches!(pc.mode, "magnitude" | "power") {
            magnitude_bits |= bit;
            power |= pc.mode == "power";
        }
        drk.phase_mode = pc.mode.into();
        drk.phase0_deg = pc.p0;
        drk.phase1_deg = pc.p1;
        records.push(drk);
    }

    // ---- result: magnitude over the flagged dimensions
    let Cube { shape, comps } = cube;
    let mut real = if magnitude_bits != 0 {
        rec.steps.push(if power {
            "power".into()
        } else {
            "magnitude".into()
        });
        let subs: Vec<usize> = (0..ncomp).filter(|m| m & !magnitude_bits == 0).collect();
        (0..comps[0].len())
            .map(|i| {
                let s: f64 = subs.iter().map(|&m| comps[m][i] * comps[m][i]).sum();
                if power { s } else { s.sqrt() }
            })
            .collect()
    } else {
        comps[0].clone()
    };
    let mut components: Vec<Component> = if opts.keep_components && magnitude_bits == 0 {
        comps
            .into_iter()
            .enumerate()
            .skip(1)
            .filter(|(m, v)| (!enc.contains(&Encoding::Complex) || m & 1 == 0) && !v.is_empty())
            .map(|(m, values)| Component {
                mask: m as u32,
                values,
            })
            .collect()
    } else {
        Vec::new()
    };
    let mut shape = shape;

    // records in axis order (slowest first): indirect reversed, then direct
    records.reverse();
    records.push(drec);

    // axes
    let mut axes: Vec<PpmAxis> = Vec::with_capacity(nd);
    for (a, r) in records.iter().enumerate() {
        let size = shape[a];
        if a == nd - 1 {
            axes.push(ppm_axis(direct, size));
        } else {
            let d = &indirect[nd - 2 - a];
            if enc[nd - 2 - a] == Encoding::Series {
                axes.push(PpmAxis {
                    first_ppm: 0.0,
                    step_ppm: 0.0,
                    size,
                    reference_frequency_mhz: 0.0,
                });
            } else {
                let p = FidParameters {
                    spectral_width_hz: d.spectral_width_hz,
                    carrier_frequency_mhz: d.carrier_frequency_mhz,
                    reference_frequency_mhz: d.reference_frequency_mhz,
                    ..FidParameters::default()
                };
                let ax = ppm_axis(&p, size);
                if !(d.reference_frequency_mhz > 0.0 && d.carrier_frequency_mhz > 0.0) {
                    rec.notes.push(format!(
                        "{}: no carrier or reference frequency; its ppm axis is not meaningful",
                        r.label
                    ));
                }
                axes.push(ax);
            }
        }
    }

    // reverse
    let default_opt = DimensionOptions::default();
    for a in 0..nd {
        let k_opt = if a == nd - 1 {
            &opts.direct
        } else {
            opts.indirect.get(nd - 2 - a).unwrap_or(&default_opt)
        };
        let stored_rev = records[a].stored.reverse;
        if k_opt.reverse.unwrap_or(stored_rev) {
            reverse_axis(&mut real, &shape, a);
            for c in &mut components {
                reverse_axis(&mut c.values, &shape, a);
            }
            let ax = &mut axes[a];
            ax.first_ppm = ax.last_ppm();
            ax.step_ppm = -ax.step_ppm;
            records[a].reversed = true;
        }
    }

    // strip of the direct dimension
    if let Some((start, len)) = direct_stored.strip {
        let n = shape[nd - 1];
        if len > 0 && start < n && len < n {
            let len = len.min(n - start);
            strip_last(&mut real, &shape, start, len);
            for c in &mut components {
                strip_last(&mut c.values, &shape, start, len);
            }
            shape[nd - 1] = len;
            let ax = &mut axes[nd - 1];
            ax.first_ppm = ax.ppm(start as f64);
            ax.size = len;
            records[nd - 1].size = len;
            rec.steps.push(format!("strip:{start}:{len}"));
        }
    }

    // tilt and symmetrisation (2D only)
    if opts.tilt {
        if nd == 2 && enc[0] != Encoding::Series {
            tilt(&mut real, &shape, &axes);
            rec.steps.push("tilt".into());
            symmetrize_j(&mut real, &shape);
            rec.steps.push("symmetrize_j".into());
            components.clear();
        } else {
            rec.notes
                .push("tilt needs a 2D Fourier spectrum; not done".into());
        }
    }
    if opts.symmetrize {
        if nd == 2 && shape[0] == shape[1] && enc[0] != Encoding::Series {
            let n = shape[0];
            for i in 0..n {
                for j in i + 1..n {
                    let (a, b) = (real[i * n + j], real[j * n + i]);
                    let v = if a.abs() <= b.abs() { a } else { b };
                    real[i * n + j] = v;
                    real[j * n + i] = v;
                }
            }
            rec.steps.push("symmetrize".into());
            components.clear();
        } else {
            rec.notes.push(
                "symmetrisation needs a square 2D spectrum (equal sizes in both dimensions); not done"
                    .into(),
            );
        }
    }

    // baseline per dimension
    for a in 0..nd {
        let mode = records[a].baseline;
        if mode.order().is_none() {
            continue;
        }
        let mut done = 0usize;
        let strides: Vec<usize> = {
            let mut s = vec![1; nd];
            for x in (0..nd.saturating_sub(1)).rev() {
                s[x] = s[x + 1] * shape[x + 1];
            }
            s
        };
        let n = shape[a];
        let inner = strides[a];
        let outer = real.len() / (n * inner).max(1);
        let mut lane = vec![0.0; n];
        for o in 0..outer {
            for i in 0..inner {
                for (k, v) in lane.iter_mut().enumerate() {
                    *v = real[(o * n + k) * inner + i];
                }
                if baseline::correct(&mut lane, mode).is_some() {
                    done += 1;
                    for (k, v) in lane.iter().enumerate() {
                        real[(o * n + k) * inner + i] = *v;
                    }
                }
            }
        }
        rec.steps
            .push(format!("baseline:{}:{done} traces", records[a].label));
    }

    let series = indirect
        .iter()
        .zip(&enc)
        .rev()
        .find(|(_, e)| **e == Encoding::Series)
        .and_then(|(d, _)| d.series.clone());
    if series_any
        && nd == 2
        && let Some(s) = &series
        && s.values.len() < shape[0]
    {
        rec.notes.push(format!(
            "the {} list has {} values for {} rows",
            s.name,
            s.values.len(),
            shape[0]
        ));
    }
    rec.dimensions = records;
    Ok(SpectrumNd {
        shape,
        real,
        components,
        axes,
        series,
        record: rec,
    })
}

fn reverse_axis(v: &mut [f64], shape: &[usize], axis: usize) {
    let n = shape[axis];
    let inner: usize = shape[axis + 1..].iter().product();
    let outer: usize = shape[..axis].iter().product();
    for o in 0..outer {
        for i in 0..inner {
            for k in 0..n / 2 {
                v.swap((o * n + k) * inner + i, (o * n + (n - 1 - k)) * inner + i);
            }
        }
    }
}

fn strip_last(v: &mut Vec<f64>, shape: &[usize], start: usize, len: usize) {
    let n = *shape.last().unwrap_or(&0);
    if n == 0 {
        return;
    }
    let rows = v.len() / n;
    let mut out = Vec::with_capacity(rows * len);
    for r in 0..rows {
        out.extend_from_slice(&v[r * n + start..r * n + start + len]);
    }
    *v = out;
}

/// A line at F1 offset +f (Hz) sits f Hz higher in F2 (towards index 0); the shear moves it
/// back. Not verified on a vendor spectrum.
const TILT_SIGN: f64 = -1.0;

/// Shift each F1 row along F2 by its F1 offset from the centre (Hz), by linear interpolation.
fn tilt(real: &mut [f64], shape: &[usize], axes: &[PpmAxis]) {
    let (n1, n2) = (shape[0], shape[1]);
    let hz1 = axes[0].hz_per_point();
    let hz2 = axes[1].hz_per_point();
    if !(hz1 > 0.0 && hz2 > 0.0) {
        return;
    }
    let mut row = vec![0.0; n2];
    for i in 0..n1 {
        // F1 offset from the centre, Hz (index 0 = highest frequency)
        let f1 = (n1 as f64 / 2.0 - i as f64) * hz1;
        let shift = f1 / hz2;
        row.copy_from_slice(&real[i * n2..(i + 1) * n2]);
        for j in 0..n2 {
            let x = j as f64 + TILT_SIGN * shift;
            let x0 = x.floor();
            let t = x - x0;
            let at = |k: f64| -> f64 {
                if k < 0.0 || k >= n2 as f64 {
                    0.0
                } else {
                    row[k as usize]
                }
            };
            real[i * n2 + j] = (1.0 - t) * at(x0) + t * at(x0 + 1.0);
        }
    }
}

/// Symmetrise about F1 = 0 (the centre row): each pair takes the value of smaller magnitude.
fn symmetrize_j(real: &mut [f64], shape: &[usize]) {
    let (n1, n2) = (shape[0], shape[1]);
    for i in 1..n1 / 2 {
        let k = n1 - i;
        for j in 0..n2 {
            let (a, b) = (real[i * n2 + j], real[k * n2 + j]);
            let v = if a.abs() <= b.abs() { a } else { b };
            real[i * n2 + j] = v;
            real[k * n2 + j] = v;
        }
    }
}
