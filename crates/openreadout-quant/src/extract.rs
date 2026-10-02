//! Chromatograms from any reader: total-ion (TIC), base-peak (BPC), extracted-ion (XIC) and
//! SRM/MRM transition chromatograms computed from mass spectra in one streaming pass, and
//! stored chromatograms or detector signals read from traces and tables.
//!
//! Definitions (`book/src/guides/quantitation.md`):
//!
//! - **XIC** at m/z `M` with tolerance `±δ` (`δ = M·ppm·10⁻⁶` or a fixed Da value): for every
//!   selected spectrum, the sum (or, with [`Aggregate::Max`], the largest) of the intensities of
//!   the points with `M − δ ≤ m/z ≤ M + δ`; 0 when none. Spectra are read as the instrument's
//!   stored centroid lists where a scan has one ([`SpectrumView::Centroid`]), otherwise as
//!   stored (profile points are then summed within the window); `profile: true` reads the
//!   profile. Each chromatogram reports how many of its scans were centroid or profile.
//! - **TIC**: the spectrum's recorded total ion current when the reader has one, else the sum
//!   of its intensities; with an m/z range, the sum inside the range. **BPC**: likewise the
//!   recorded base-peak intensity, else the largest intensity.
//! - **SRM/MRM** `Q1 > Q3`: MS/MS scans whose precursor is within the transition tolerance of
//!   Q1; in each, the intensity of the point nearest Q3 within the tolerance. When several
//!   distinct precursors (or products) fall within the tolerance, only the one nearest the
//!   requested value is kept (the others are named in `notes`). Files that store transitions
//!   as chromatograms (mzML) or as table columns (Waters MRM functions) are read from those.

use std::sync::atomic::{AtomicU64, Ordering};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use openreadout_core::model::{FileInfo, Spectrum, TraceInfo};
use openreadout_core::parallel::ReadContext;
use openreadout_core::{Dataset, Error, Result, SpectrumView};

/// Spectra decoded per worker between progress reports.
const PROGRESS_EVERY: u64 = 256;
/// Fewest spectra per worker thread.
const MIN_SCANS_PER_WORKER: u64 = 200;
/// Samples read from a trace per call.
const TRACE_CHUNK: u64 = 1 << 20;
/// Default XIC tolerance, ppm.
pub const DEFAULT_PPM: f64 = 10.0;
/// Default SRM/MRM and precursor tolerance, Da (unit resolution).
pub const DEFAULT_TRANSITION_TOLERANCE_DA: f64 = 0.5;

/// One chromatogram to extract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Target {
    /// Total ion current.
    Tic,
    /// Base-peak intensity.
    Bpc,
    /// Extracted-ion chromatogram at `mz`.
    Xic {
        /// Target m/z.
        mz: f64,
    },
    /// Selected/multiple reaction monitoring transition `q1 > q3`.
    Srm {
        /// Precursor (Q1) m/z.
        q1: f64,
        /// Product (Q3) m/z.
        q3: f64,
    },
    /// A stored chromatogram or detector signal (`info` → `traces[]`).
    Trace {
        /// Trace index.
        trace: u32,
        /// Channel index (default: the first channel that is not `time`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        channel: Option<u32>,
    },
}

/// m/z tolerance of an XIC (half-width of the window).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Tolerance {
    /// Parts per million of the target m/z.
    Ppm(f64),
    /// Absolute, in m/z units (Da for singly charged ions).
    Da(f64),
}

impl Default for Tolerance {
    fn default() -> Self {
        Tolerance::Ppm(DEFAULT_PPM)
    }
}

impl Tolerance {
    /// Half-width of the window around `mz`.
    pub fn half_width(self, mz: f64) -> f64 {
        match self {
            Tolerance::Ppm(p) => mz * p * 1e-6,
            Tolerance::Da(d) => d,
        }
    }
    fn label(self) -> String {
        match self {
            Tolerance::Ppm(p) => format!("±{} ppm", num(p)),
            Tolerance::Da(d) => format!("±{} Da", num(d)),
        }
    }
}

/// How the points inside an XIC window are combined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Aggregate {
    /// Sum of intensities (default; what OpenMS, pyteomics-based scripts and most vendor
    /// software do).
    #[default]
    Sum,
    /// Largest intensity.
    Max,
}

/// Polarity filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Polarity {
    /// Positive-ion scans.
    Positive,
    /// Negative-ion scans.
    Negative,
}

impl std::str::FromStr for Polarity {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "positive" | "pos" | "+" => Ok(Polarity::Positive),
            "negative" | "neg" | "-" => Ok(Polarity::Negative),
            _ => Err(Error::Usage(format!(
                "unknown polarity `{s}`: use positive or negative"
            ))),
        }
    }
}

impl Polarity {
    fn id(self) -> &'static str {
        match self {
            Polarity::Positive => "positive",
            Polarity::Negative => "negative",
        }
    }
}

/// What to extract and from which scans.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ChromRequest {
    /// Chromatograms to extract (empty: the TIC).
    pub targets: Vec<Target>,
    /// XIC tolerance.
    pub tolerance: Tolerance,
    /// SRM/MRM Q1 and Q3 tolerance, Da.
    pub transition_tolerance_da: f64,
    /// Spectra run.
    pub run: u32,
    /// MS level of the scans (default 1, or 2 with `precursor_mz`; SRM: any MS/MS level).
    pub ms_level: Option<u32>,
    /// Only scans of this polarity.
    pub polarity: Option<Polarity>,
    /// Only scans whose scan filter / description contains this text (case-insensitive).
    pub scan_filter: Option<String>,
    /// Only MS/MS scans whose precursor m/z is within `precursor_tolerance_da` of this.
    pub precursor_mz: Option<f64>,
    /// Precursor tolerance, Da.
    pub precursor_tolerance_da: f64,
    /// Only scans (or trace samples) in this retention-time range, minutes.
    pub rt_range_min: Option<[f64; 2]>,
    /// TIC/BPC over this m/z range only.
    pub mz_range: Option<[f64; 2]>,
    /// Read profile data where a scan has both profile and centroids.
    pub profile: bool,
    /// Combination of the points inside an XIC window.
    pub aggregate: Aggregate,
    /// Sweep of trace targets.
    pub sweep: u32,
}

impl Default for ChromRequest {
    fn default() -> Self {
        Self {
            targets: Vec::new(),
            tolerance: Tolerance::default(),
            transition_tolerance_da: DEFAULT_TRANSITION_TOLERANCE_DA,
            run: 0,
            ms_level: None,
            polarity: None,
            scan_filter: None,
            precursor_mz: None,
            precursor_tolerance_da: DEFAULT_TRANSITION_TOLERANCE_DA,
            rt_range_min: None,
            mz_range: None,
            profile: false,
            aggregate: Aggregate::Sum,
            sweep: 0,
        }
    }
}

/// One chromatogram: retention times (minutes) and intensities, with how they were obtained
/// and a summary.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct Chromatogram {
    /// Short label, e.g. `TIC`, `XIC 301.1410 ±10 ppm`, `SRM 279.2 > 179.2`, `DAD1 A`.
    pub label: String,
    /// `tic`, `bpc`, `xic`, `srm` or `trace`.
    pub kind: String,
    /// Where the points come from: `spectra` (computed from the mass spectra), `trace N`
    /// (a stored chromatogram or detector signal) or `table N` (an MRM table column).
    pub source: String,
    /// XIC target m/z.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mz: Option<f64>,
    /// XIC window `[low, high]` m/z.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mz_window: Option<[f64; 2]>,
    /// TIC/BPC m/z range.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mz_range: Option<[f64; 2]>,
    /// SRM: the precursor (Q1) m/z matched in the file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub precursor_mz: Option<f64>,
    /// SRM: the product (Q3) m/z matched in the file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub product_mz: Option<f64>,
    /// MS level of the scans used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ms_level: Option<u32>,
    /// Polarity filter applied.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub polarity: Option<String>,
    /// Scan-filter text required.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scan_filter: Option<String>,
    /// `sum` or `max` (XIC).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aggregation: Option<String>,
    /// Scans read as centroids.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub centroid_scans: Option<u64>,
    /// Scans read as profiles (their points inside the window are summed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_scans: Option<u64>,
    /// TIC/BPC: points from the scan's recorded value (`recorded`), from its points
    /// (`computed`), or both (`mixed`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intensity_source: Option<String>,
    /// Unit of `intensity` (`counts` for mass spectra, the detector's unit for traces).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intensity_unit: Option<String>,
    /// Points in the chromatogram.
    pub points: u64,
    /// Retention time of the most intense point, minutes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub apex_rt_min: Option<f64>,
    /// Intensity of the most intense point.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub apex_intensity: Option<f64>,
    /// Trapezoidal integral of the whole chromatogram (no baseline), intensity × minutes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub integral: Option<f64>,
    /// Retention times, minutes (ascending).
    pub rt_min: Vec<f64>,
    /// Intensities.
    pub intensity: Vec<f64>,
    /// True when `rt_min`/`intensity` were thinned to `max_points` (the most intense point of
    /// each of `max_points` equal slices is kept); the summary covers every point.
    #[serde(default)]
    pub decimated: bool,
    /// Anything the caller should know.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

impl Chromatogram {
    /// Fill the summary fields from the points.
    pub fn summarize(&mut self) {
        self.points = self.rt_min.len() as u64;
        let apex = self
            .intensity
            .iter()
            .enumerate()
            .filter(|(_, v)| v.is_finite())
            .max_by(|a, b| a.1.total_cmp(b.1).then(b.0.cmp(&a.0)));
        self.apex_rt_min = apex.map(|(i, _)| self.rt_min[i]);
        self.apex_intensity = apex.map(|(_, v)| *v);
        self.integral = (self.rt_min.len() > 1).then(|| {
            self.rt_min
                .windows(2)
                .zip(self.intensity.windows(2))
                .map(|(t, y)| f64::midpoint(y[0], y[1]) * (t[1] - t[0]))
                .filter(|v| v.is_finite())
                .sum()
        });
    }

    /// Keep at most `max` points: the most intense point of each of `max` equal slices.
    pub fn decimate(&mut self, max: usize) {
        let n = self.rt_min.len();
        if max == 0 || n <= max {
            return;
        }
        let mut t = Vec::with_capacity(max);
        let mut y = Vec::with_capacity(max);
        for b in 0..max {
            let lo = b * n / max;
            let hi = ((b + 1) * n / max).max(lo + 1).min(n);
            let k = (lo..hi)
                .max_by(|&i, &j| {
                    self.intensity[i]
                        .total_cmp(&self.intensity[j])
                        .then(j.cmp(&i))
                })
                .unwrap_or(lo);
            t.push(self.rt_min[k]);
            y.push(self.intensity[k]);
        }
        self.rt_min = t;
        self.intensity = y;
        self.decimated = true;
    }
}

/// Output of `analyze chromatogram`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ChromatogramOutput {
    /// The input file.
    pub path: String,
    /// Format id of the input file.
    pub format: String,
    /// Spectra run used, when any chromatogram was computed from spectra.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run: Option<u32>,
    /// Spectra decoded.
    pub spectra_read: u64,
    /// One entry per requested chromatogram.
    pub chromatograms: Vec<Chromatogram>,
    /// File written (`-o`), when any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// Anything the caller should know.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

fn num(v: f64) -> String {
    let s = format!("{v:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.to_string()
}

fn validate(req: &ChromRequest) -> Result<()> {
    let pos = |v: f64, what: &str| -> Result<()> {
        if v.is_finite() && v > 0.0 {
            Ok(())
        } else {
            Err(Error::Usage(format!(
                "{what} must be a number > 0 (got {v})"
            )))
        }
    };
    match req.tolerance {
        Tolerance::Ppm(p) => pos(p, "--ppm")?,
        Tolerance::Da(d) => pos(d, "--da")?,
    }
    pos(req.transition_tolerance_da, "the transition tolerance")?;
    pos(req.precursor_tolerance_da, "the precursor tolerance")?;
    for t in &req.targets {
        match t {
            Target::Xic { mz } => pos(*mz, "an XIC m/z")?,
            Target::Srm { q1, q3 } => {
                pos(*q1, "Q1")?;
                pos(*q3, "Q3")?;
            }
            _ => {}
        }
    }
    if let Some(p) = req.precursor_mz {
        pos(p, "the precursor m/z")?;
    }
    for (r, what) in [
        (req.rt_range_min, "the retention-time range"),
        (req.mz_range, "the m/z range"),
    ] {
        if let Some([a, b]) = r
            && !(a.is_finite() && b.is_finite() && a < b)
        {
            return Err(Error::Usage(format!(
                "{what} must be two numbers, low < high"
            )));
        }
    }
    Ok(())
}

/// Extract the requested chromatograms. Spectra are streamed once for all spectrum-derived
/// targets, split across threads when `ctx.opener` can open extra handles (the result does not
/// depend on the number of threads).
pub fn extract(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    req: &ChromRequest,
    ctx: &ReadContext<'_>,
) -> Result<ChromatogramOutput> {
    validate(req)?;
    let targets: Vec<Target> = if req.targets.is_empty() {
        vec![if info.spectra.is_empty() && !info.traces.is_empty() {
            Target::Trace {
                trace: 0,
                channel: None,
            }
        } else {
            Target::Tic
        }]
    } else {
        req.targets.clone()
    };
    let mut out = ChromatogramOutput {
        path: info.path.clone(),
        format: info.format.id.clone(),
        ..ChromatogramOutput::default()
    };
    let run = info.spectra.iter().find(|s| s.index == req.run);
    if !info.spectra.is_empty() && run.is_none() {
        return Err(Error::Usage(format!(
            "run {} out of range (file has {} spectra runs)",
            req.run,
            info.spectra.len()
        )));
    }
    let has_scans = run.is_some_and(|r| r.scan_count > 0);
    // A run without MS1 scans (SRM/MRM-only, all-MS/MS): without an explicit level, TIC, BPC and
    // XICs use the lowest level present, as the instrument's own TIC does.
    let lowered: ChromRequest;
    let req = match run {
        Some(r)
            if req.ms_level.is_none()
                && req.precursor_mz.is_none()
                && !r.ms_levels.is_empty()
                && !r.ms_levels.contains(&1) =>
        {
            let mut q = req.clone();
            q.ms_level = r.ms_levels.iter().copied().min();
            out.notes.push(format!(
                "the run has no MS1 scans: TIC/BPC/XIC use its MS{} scans",
                q.ms_level.unwrap_or(2)
            ));
            lowered = q;
            &lowered
        }
        _ => req,
    };
    // decide where each target comes from
    let mut slots: Vec<Option<Chromatogram>> = vec![None; targets.len()];
    let mut from_spectra: Vec<(usize, Target)> = Vec::new();
    for (k, t) in targets.iter().enumerate() {
        match t {
            Target::Trace { trace, channel } => {
                slots[k] = Some(from_trace(ds, info, *trace, *channel, req)?);
            }
            Target::Tic | Target::Bpc if !has_scans => {
                slots[k] = Some(stored_total(ds, info, t, req)?);
            }
            Target::Srm { q1, q3 } if !has_scans || !has_msn(info, req.run) => {
                slots[k] = Some(stored_transition(ds, info, *q1, *q3, req)?);
            }
            Target::Xic { .. } if !has_scans => {
                return Err(Error::unsupported(
                    "chromatogram",
                    format!("extracted-ion chromatograms of a {} file", info.format.name),
                    "This file holds no mass spectra (see `openreadout info` → spectra[]); \
                     XICs need spectra. Use --trace for its stored chromatograms.",
                ));
            }
            _ => from_spectra.push((k, t.clone())),
        }
    }
    if !from_spectra.is_empty() {
        let run = run.ok_or_else(|| Error::Other("no spectra run".into()))?;
        let (chroms, read) = stream_spectra(ds, run.scan_count, &from_spectra, req, ctx)?;
        out.run = Some(req.run);
        out.spectra_read = read;
        for ((k, _), c) in from_spectra.iter().zip(chroms) {
            slots[*k] = Some(c);
        }
    }
    out.chromatograms = slots.into_iter().flatten().collect();
    for c in &mut out.chromatograms {
        c.summarize();
        if c.points == 0 && !c.notes.iter().any(|n| n.contains("no retention time")) {
            c.notes.push(
                "no points: no scan matched the filters (see `info` → spectra[].extra for the \
                 MS levels, polarities and scan filters present)"
                    .into(),
            );
        }
    }
    Ok(out)
}

fn has_msn(info: &FileInfo, run: u32) -> bool {
    info.spectra
        .iter()
        .find(|s| s.index == run)
        .is_some_and(|r| r.ms_levels.is_empty() || r.ms_levels.iter().any(|&l| l >= 2))
}

fn default_level(req: &ChromRequest) -> u32 {
    req.ms_level
        .unwrap_or(if req.precursor_mz.is_some() { 2 } else { 1 })
}

/// A point with the matched precursor/product (SRM only).
#[derive(Debug, Clone, Copy)]
struct Pt {
    rt: f64,
    v: f64,
    q1: f64,
    q3: f64,
}

#[derive(Debug, Default, Clone)]
struct Acc {
    pts: Vec<Pt>,
    centroid: u64,
    profile: u64,
    recorded: u64,
    computed: u64,
    /// Scans that matched but whose file states no retention time: left out.
    no_rt: u64,
}

impl Acc {
    fn merge(&mut self, o: Acc) {
        self.pts.extend(o.pts);
        self.centroid += o.centroid;
        self.profile += o.profile;
        self.recorded += o.recorded;
        self.computed += o.computed;
        self.no_rt += o.no_rt;
    }
}

fn polarity_ok(sp: &Spectrum, req: &ChromRequest) -> bool {
    req.polarity.is_none_or(|p| sp.polarity == p.id())
}

/// A scan without a retention time passes the time window here: it is counted (and left out)
/// where a point would be placed, so the note can say how many matching scans had none.
fn common_ok(sp: &Spectrum, req: &ChromRequest) -> bool {
    let in_window = match sp.rt_s {
        Some(t) => req
            .rt_range_min
            .is_none_or(|[a, b]| t / 60.0 >= a && t / 60.0 <= b),
        None => true,
    };
    in_window
        && polarity_ok(sp, req)
        && req.scan_filter.as_ref().is_none_or(|f| {
            sp.scan_filter
                .as_ref()
                .is_some_and(|s| s.to_ascii_lowercase().contains(&f.to_ascii_lowercase()))
        })
}

/// Sum and max of the intensities with `lo ≤ m/z ≤ hi` (m/z ascending).
fn window(sp: &Spectrum, lo: f64, hi: f64) -> (f64, f64) {
    let start = sp.mz.partition_point(|&m| m < lo);
    let mut sum = 0.0;
    let mut max = 0.0f64;
    for (m, v) in sp.mz[start..]
        .iter()
        .zip(sp.intensity.get(start..).unwrap_or(&[]))
    {
        if *m > hi {
            break;
        }
        let v = f64::from(*v);
        sum += v;
        max = max.max(v);
    }
    (sum, max)
}

fn visit(sp: &Spectrum, targets: &[(usize, Target)], req: &ChromRequest, accs: &mut [Acc]) {
    if !common_ok(sp, req) {
        return;
    }
    let rt = sp.rt_s.map(|t| t / 60.0);
    let level = default_level(req);
    let precursor_ok = |sp: &Spectrum| {
        req.precursor_mz.is_none_or(|p| {
            sp.precursor_mz
                .is_some_and(|x| (x - p).abs() <= req.precursor_tolerance_da)
        })
    };
    for ((_, t), acc) in targets.iter().zip(accs.iter_mut()) {
        let v = match t {
            Target::Tic | Target::Bpc | Target::Xic { .. } => {
                if sp.ms_level != level || !precursor_ok(sp) {
                    continue;
                }
                if rt.is_none() {
                    acc.no_rt += 1;
                    continue;
                }
                match t {
                    Target::Tic => {
                        if let Some([a, b]) = req.mz_range {
                            acc.computed += 1;
                            window(sp, a, b).0
                        } else if let Some(v) = sp.total_ion_current {
                            acc.recorded += 1;
                            v
                        } else {
                            acc.computed += 1;
                            sp.intensity.iter().map(|&v| f64::from(v)).sum()
                        }
                    }
                    Target::Bpc => {
                        if let Some([a, b]) = req.mz_range {
                            acc.computed += 1;
                            window(sp, a, b).1
                        } else if let Some(v) = sp.base_peak_intensity {
                            acc.recorded += 1;
                            v
                        } else {
                            acc.computed += 1;
                            sp.intensity
                                .iter()
                                .fold(0.0f64, |m, &v| m.max(f64::from(v)))
                        }
                    }
                    Target::Xic { mz } => {
                        let d = req.tolerance.half_width(*mz);
                        let (s, m) = window(sp, mz - d, mz + d);
                        match req.aggregate {
                            Aggregate::Sum => s,
                            Aggregate::Max => m,
                        }
                    }
                    _ => unreachable!("matched above"),
                }
            }
            Target::Srm { q1, q3 } => {
                if sp.ms_level < 2 || req.ms_level.is_some_and(|l| sp.ms_level != l) {
                    continue;
                }
                let tol = req.transition_tolerance_da;
                let Some(p) = sp.precursor_mz.filter(|p| (p - q1).abs() <= tol) else {
                    continue;
                };
                let start = sp.mz.partition_point(|&m| m < q3 - tol);
                let best = sp.mz[start..]
                    .iter()
                    .zip(sp.intensity.get(start..).unwrap_or(&[]))
                    .take_while(|(m, _)| **m <= q3 + tol)
                    .min_by(|a, b| (a.0 - q3).abs().total_cmp(&(b.0 - q3).abs()));
                let Some((m, v)) = best else { continue };
                let Some(rt) = rt else {
                    acc.no_rt += 1;
                    continue;
                };
                if sp.centroided {
                    acc.centroid += 1;
                } else {
                    acc.profile += 1;
                }
                acc.pts.push(Pt {
                    rt,
                    v: f64::from(*v),
                    q1: p,
                    q3: *m,
                });
                continue;
            }
            Target::Trace { .. } => continue,
        };
        // counted as `no_rt` above, before any value was taken
        let Some(rt) = rt else { continue };
        if sp.centroided {
            acc.centroid += 1;
        } else {
            acc.profile += 1;
        }
        acc.pts.push(Pt {
            rt,
            v,
            q1: 0.0,
            q3: 0.0,
        });
    }
}

/// Stream every spectrum of the run once.
fn stream_spectra(
    ds: &mut dyn Dataset,
    scan_count: u64,
    targets: &[(usize, Target)],
    req: &ChromRequest,
    ctx: &ReadContext<'_>,
) -> Result<(Vec<Chromatogram>, u64)> {
    let view = if req.profile {
        SpectrumView::Primary
    } else {
        SpectrumView::Centroid
    };
    let levels = ds.spectrum_ms_levels(req.run)?;
    let want_level = default_level(req);
    let needed = |i: u64| -> bool {
        let Some(lv) = &levels else { return true };
        let Some(&l) = lv.get(i as usize) else {
            return true;
        };
        targets.iter().any(|(_, t)| match t {
            Target::Srm { .. } => l >= 2 && req.ms_level.is_none_or(|x| x == l),
            _ => l == want_level,
        })
    };
    let workers = if ctx.opener.is_some() {
        (rayon::current_num_threads() as u64)
            .min(scan_count / MIN_SCANS_PER_WORKER)
            .max(1)
    } else {
        1
    };
    let done = AtomicU64::new(0);
    let read = AtomicU64::new(0);
    let run_chunk = |d: &mut dyn Dataset, lo: u64, hi: u64| -> Result<Vec<Acc>> {
        let mut accs = vec![Acc::default(); targets.len()];
        for i in lo..hi {
            if needed(i) {
                let sp = d.read_spectrum_view(req.run, i, view)?;
                read.fetch_add(1, Ordering::Relaxed);
                visit(&sp, targets, req, &mut accs);
            }
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            if n.is_multiple_of(PROGRESS_EVERY) {
                ctx.report(n, scan_count);
            }
        }
        Ok(accs)
    };
    let bounds: Vec<(u64, u64)> = (0..workers)
        .map(|w| (w * scan_count / workers, (w + 1) * scan_count / workers))
        .collect();
    let mut results: Vec<Option<Result<Vec<Acc>>>> = (0..workers).map(|_| None).collect();
    if workers == 1 {
        results[0] = Some(run_chunk(ds, 0, scan_count));
    } else {
        let opener = ctx.opener.ok_or_else(|| Error::Other("no opener".into()))?;
        let (first, rest) = results.split_at_mut(1);
        let run_chunk = &run_chunk;
        rayon::scope(|s| {
            let (lo, hi) = bounds[0];
            let slot = &mut first[0];
            s.spawn(move |_| *slot = Some(run_chunk(ds, lo, hi)));
            for (slot, &(lo, hi)) in rest.iter_mut().zip(&bounds[1..]) {
                s.spawn(move |_| {
                    *slot = Some(opener().and_then(|mut d| run_chunk(d.as_mut(), lo, hi)));
                });
            }
        });
    }
    let mut accs = vec![Acc::default(); targets.len()];
    for r in results {
        let part = r.ok_or_else(|| Error::Other("worker did not run".into()))??;
        for (a, p) in accs.iter_mut().zip(part) {
            a.merge(p);
        }
    }
    ctx.report(scan_count, scan_count);
    let chroms = targets
        .iter()
        .zip(accs)
        .map(|((_, t), a)| finish_spectra(t, a, req))
        .collect();
    Ok((chroms, read.load(Ordering::Relaxed)))
}

/// Keep the points whose `key` (precursor or product) is the distinct value nearest `target`.
fn nearest_group(
    pts: &mut Vec<Pt>,
    key: fn(&Pt) -> f64,
    target: f64,
    what: &str,
) -> Option<String> {
    let mut values: Vec<f64> = pts.iter().map(key).collect();
    values.sort_by(f64::total_cmp);
    values.dedup_by(|a, b| (*a - *b).abs() < 1e-4);
    if values.len() < 2 {
        return None;
    }
    let best = values
        .iter()
        .copied()
        .min_by(|a, b| (a - target).abs().total_cmp(&(b - target).abs()))?;
    pts.retain(|p| (key(p) - best).abs() < 1e-4);
    let others: Vec<String> = values
        .iter()
        .filter(|v| (**v - best).abs() >= 1e-4)
        .map(|v| num(*v))
        .collect();
    Some(format!(
        "{what} {} matched; also within the tolerance and left out: {}",
        num(best),
        others.join(", ")
    ))
}

fn finish_spectra(t: &Target, mut a: Acc, req: &ChromRequest) -> Chromatogram {
    let mut c = Chromatogram {
        source: "spectra".into(),
        polarity: req.polarity.map(|p| p.id().to_string()),
        scan_filter: req.scan_filter.clone(),
        intensity_unit: Some("counts".into()),
        ..Chromatogram::default()
    };
    let level = default_level(req);
    let mut suffix = String::new();
    if let Some(p) = req.precursor_mz {
        suffix.push_str(&format!(" (MS{level} of {})", num(p)));
    }
    if let Some(p) = req.polarity {
        suffix.push_str(if p == Polarity::Positive {
            " +"
        } else {
            " −"
        });
    }
    match t {
        Target::Tic | Target::Bpc => {
            let tic = matches!(t, Target::Tic);
            c.kind = if tic { "tic" } else { "bpc" }.into();
            c.label = format!("{}{suffix}", if tic { "TIC" } else { "BPC" });
            if let Some([lo, hi]) = req.mz_range {
                c.label.push_str(&format!(" m/z {}–{}", num(lo), num(hi)));
                c.mz_range = req.mz_range;
            }
            c.ms_level = Some(level);
            c.intensity_source = Some(
                match (a.recorded > 0, a.computed > 0) {
                    (true, false) => "recorded",
                    (false, true) => "computed",
                    (true, true) => "mixed",
                    (false, false) => "none",
                }
                .into(),
            );
        }
        Target::Xic { mz } => {
            let d = req.tolerance.half_width(*mz);
            c.kind = "xic".into();
            c.label = format!("XIC {} {}{suffix}", num(*mz), req.tolerance.label());
            c.mz = Some(*mz);
            c.mz_window = Some([mz - d, mz + d]);
            c.ms_level = Some(level);
            c.aggregation = Some(
                match req.aggregate {
                    Aggregate::Sum => "sum",
                    Aggregate::Max => "max",
                }
                .into(),
            );
        }
        Target::Srm { q1, q3 } => {
            c.kind = "srm".into();
            c.label = format!("SRM {} > {}", num(*q1), num(*q3));
            c.ms_level = req.ms_level;
            if let Some(n) = nearest_group(&mut a.pts, |p| p.q1, *q1, "precursor") {
                c.notes.push(n);
            }
            if let Some(n) = nearest_group(&mut a.pts, |p| p.q3, *q3, "product") {
                c.notes.push(n);
            }
            c.precursor_mz = a.pts.first().map(|p| p.q1);
            c.product_mz = a.pts.first().map(|p| p.q3);
        }
        Target::Trace { .. } => {}
    }
    c.centroid_scans = Some(a.centroid);
    c.profile_scans = Some(a.profile);
    if a.no_rt > 0 {
        c.notes.push(format!(
            "{} matching scan{} left out: the file states no retention time for {}",
            a.no_rt,
            if a.no_rt == 1 { " is" } else { "s are" },
            if a.no_rt == 1 { "it" } else { "them" }
        ));
    }
    if a.pts.windows(2).any(|w| w[1].rt < w[0].rt) {
        a.pts.sort_by(|x, y| x.rt.total_cmp(&y.rt));
    }
    c.rt_min = a.pts.iter().map(|p| p.rt).collect();
    c.intensity = a.pts.iter().map(|p| p.v).collect();
    c
}

/// Retention time in minutes of every sample of a trace sweep, and the intensity channel.
fn from_trace(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    trace: u32,
    channel: Option<u32>,
    req: &ChromRequest,
) -> Result<Chromatogram> {
    let t = info
        .traces
        .iter()
        .find(|t| t.index == trace)
        .ok_or_else(|| {
            if info.traces.is_empty() {
                Error::unsupported(
                    "chromatogram",
                    format!("stored chromatograms of a {} file", info.format.name),
                    "This file holds no traces; see `openreadout info`.",
                )
            } else {
                Error::Usage(format!(
                    "trace {trace} out of range (file has {} traces)",
                    info.traces.len()
                ))
            }
        })?;
    if req.sweep >= t.sweep_count {
        return Err(Error::Usage(format!(
            "sweep {} out of range (trace {trace} has {} sweeps)",
            req.sweep, t.sweep_count
        )));
    }
    let time_ch = t
        .channels
        .iter()
        .position(|c| c.name == "time" && t.sample_rate_hz == 0.0);
    let ch = match channel {
        Some(c) => {
            if c as usize >= t.channels.len() {
                return Err(Error::Usage(format!(
                    "channel {c} out of range (trace {trace} has {} channels)",
                    t.channels.len()
                )));
            }
            c as usize
        }
        None => (0..t.channels.len())
            .find(|&c| Some(c) != time_ch)
            .ok_or_else(|| Error::Usage(format!("trace {trace} has no signal channel")))?,
    };
    let axis = time_axis(t);
    if time_ch.is_none() && matches!(axis, Axis::None) {
        return Err(Error::unsupported(
            "chromatogram",
            format!(
                "trace {trace} ({}) as a chromatogram: it has no time axis",
                t.name.as_deref().unwrap_or("unnamed")
            ),
            "Only chromatograms and time signals can be integrated; see `info` → traces[].extra.axis.",
        ));
    }
    let n = openreadout_core::trace::sweep_samples(t, req.sweep);
    let mut rt = Vec::with_capacity(n.min(1 << 24) as usize);
    let mut y = Vec::with_capacity(n.min(1 << 24) as usize);
    let time_scale = time_ch.map_or(1.0 / 60.0, |c| {
        if t.channels[c].unit.as_deref() == Some("min") {
            1.0
        } else {
            1.0 / 60.0
        }
    });
    let mut pos = 0u64;
    while pos < n {
        let want = TRACE_CHUNK.min(n - pos);
        let tr = ds.read_trace(trace, req.sweep, pos, want)?;
        let col = tr
            .channels
            .get(ch)
            .ok_or_else(|| Error::Other(format!("reader returned no channel {ch}")))?;
        let got = col.len() as u64;
        for (i, &v) in col.iter().enumerate() {
            let k = pos + i as u64;
            let tm = match (time_ch, &axis) {
                (Some(c), _) => tr
                    .channels
                    .get(c)
                    .and_then(|tc| tc.get(i))
                    .map_or(f64::NAN, |s| s * time_scale),
                (None, Axis::Minutes { first, step }) => first + step * k as f64,
                (None, _) => f64::NAN,
            };
            if req.rt_range_min.is_none_or(|[a, b]| tm >= a && tm <= b) && tm.is_finite() {
                rt.push(tm);
                y.push(v);
            }
        }
        pos += got;
        if got < want || got == 0 {
            break;
        }
    }
    let name = t.name.clone().unwrap_or_else(|| format!("trace {trace}"));
    let chan = &t.channels[ch];
    let label = if t.channels.len() > 1 + usize::from(time_ch.is_some()) {
        format!("{name}: {}", chan.name)
    } else {
        name
    };
    let mut c = Chromatogram {
        label,
        kind: "trace".into(),
        source: format!("trace {trace}"),
        intensity_unit: chan.unit.clone(),
        rt_min: rt,
        intensity: y,
        ..Chromatogram::default()
    };
    if let Some(p) = t
        .extra
        .get("precursor_mz")
        .and_then(serde_json::Value::as_f64)
    {
        c.precursor_mz = Some(p);
    }
    if let Some(p) = t
        .extra
        .get("product_mz")
        .and_then(serde_json::Value::as_f64)
    {
        c.product_mz = Some(p);
    }
    Ok(c)
}

enum Axis {
    Minutes { first: f64, step: f64 },
    None,
}

fn time_axis(t: &TraceInfo) -> Axis {
    if let Some(a) = t.extra.get("axis") {
        let q = a.get("quantity").and_then(|v| v.as_str()).unwrap_or("");
        let unit = a.get("unit").and_then(|v| v.as_str()).unwrap_or("");
        let first = a.get("first").and_then(serde_json::Value::as_f64);
        let step = a.get("step").and_then(serde_json::Value::as_f64);
        if matches!(q, "retention_time" | "time")
            && let (Some(first), Some(step)) = (first, step)
        {
            let k = match unit {
                "min" => 1.0,
                "s" => 1.0 / 60.0,
                "ms" => 1.0 / 60_000.0,
                "h" => 60.0,
                _ => return Axis::None,
            };
            return Axis::Minutes {
                first: first * k,
                step: step * k,
            };
        }
        // a retention-volume axis (ÄKTA/UNICORN) rides on a regular time base: fall through
        if !q.is_empty() && !matches!(q, "retention_time" | "time" | "retention_volume") {
            return Axis::None;
        }
    }
    if t.sample_rate_hz > 0.0 && t.sample_rate_hz.is_finite() {
        return Axis::Minutes {
            first: t.start_s.unwrap_or(0.0) / 60.0,
            step: 1.0 / t.sample_rate_hz / 60.0,
        };
    }
    Axis::None
}

/// A stored TIC/BPC trace, for files without usable spectra.
fn stored_total(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    t: &Target,
    req: &ChromRequest,
) -> Result<Chromatogram> {
    let (name, kind) = match t {
        Target::Bpc => ("BPC", "bpc"),
        _ => ("TIC", "tic"),
    };
    let found = info.traces.iter().find(|tr| {
        tr.name.as_deref().is_some_and(|n| {
            let u = n.to_ascii_uppercase();
            u == name || u.starts_with(&format!("{name} "))
        }) || tr
            .extra
            .get("chromatogram_type")
            .and_then(|v| v.as_str())
            .is_some_and(|v| {
                v.starts_with(if kind == "tic" {
                    "total ion current"
                } else {
                    "base peak"
                })
            })
    });
    if let Some(tr) = found {
        let mut c = from_trace(ds, info, tr.index, None, req)?;
        c.kind = kind.into();
        c.label = name.into();
        c.notes.push(format!(
            "read from the file's stored {name} (trace {}); no usable spectra",
            tr.index
        ));
        return Ok(c);
    }
    // an MRM table with a stored TIC column
    for tab in &info.tables {
        if kind == "tic"
            && let (Some(tcol), Some(vcol)) = (
                tab.columns.iter().position(|c| c.name == "rt_min"),
                tab.columns.iter().position(|c| c.name == "tic_stored"),
            )
        {
            let mut c = from_table(ds, tab.index, tab.row_count, tcol, vcol, req)?;
            c.kind = "tic".into();
            c.label = "TIC".into();
            c.source = format!("table {}", tab.index);
            return Ok(c);
        }
    }
    Err(Error::unsupported(
        "chromatogram",
        format!("a {name} of a {} file", info.format.name),
        "This file has no spectra and no stored chromatogram of that kind; see `openreadout info`.",
    ))
}

fn from_table(
    ds: &mut dyn Dataset,
    table: u32,
    rows: u64,
    tcol: usize,
    vcol: usize,
    req: &ChromRequest,
) -> Result<Chromatogram> {
    let tab = ds.read_table(table, 0, rows)?;
    let (Some(tc), Some(vc)) = (tab.columns.get(tcol), tab.columns.get(vcol)) else {
        return Err(Error::Other(format!("table {table}: missing columns")));
    };
    let mut c = Chromatogram {
        source: format!("table {table}"),
        intensity_unit: Some("counts".into()),
        ..Chromatogram::default()
    };
    for (&t, &v) in tc.iter().zip(vc) {
        if t.is_finite() && req.rt_range_min.is_none_or(|[a, b]| t >= a && t <= b) {
            c.rt_min.push(t);
            c.intensity.push(v);
        }
    }
    Ok(c)
}

/// Parse an MRM column name `Q1 > Q3`.
fn transition_of(name: &str) -> Option<(f64, f64)> {
    let (a, b) = name.split_once('>')?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
}

/// An SRM transition from stored chromatograms (traces carrying `precursor_mz`/`product_mz`) or
/// MRM table columns named `Q1 > Q3`.
fn stored_transition(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    q1: f64,
    q3: f64,
    req: &ChromRequest,
) -> Result<Chromatogram> {
    let tol = req.transition_tolerance_da;
    let dist = |a: f64, b: f64| (a - q1).abs().max((b - q3).abs());
    let mut traces: Vec<(&TraceInfo, f64, f64)> = info
        .traces
        .iter()
        .filter_map(|t| {
            let p = t.extra.get("precursor_mz")?.as_f64()?;
            let d = t.extra.get("product_mz")?.as_f64()?;
            (dist(p, d) <= tol).then_some((t, p, d))
        })
        .collect();
    if !traces.is_empty() {
        let best = traces
            .iter()
            .map(|x| dist(x.1, x.2))
            .fold(f64::INFINITY, f64::min);
        traces.retain(|x| (dist(x.1, x.2) - best).abs() < 1e-9);
        let (mut rt, mut y) = (Vec::new(), Vec::new());
        let mut sources = Vec::new();
        let mut unit = None;
        for (t, _, _) in &traces {
            let c = from_trace(ds, info, t.index, None, req)?;
            rt.extend(c.rt_min);
            y.extend(c.intensity);
            unit = c.intensity_unit;
            sources.push(t.index.to_string());
        }
        let mut pts: Vec<(f64, f64)> = rt.into_iter().zip(y).collect();
        pts.sort_by(|a, b| a.0.total_cmp(&b.0));
        let (p, d) = (traces[0].1, traces[0].2);
        let mut c = Chromatogram {
            label: format!("SRM {} > {}", num(q1), num(q3)),
            kind: "srm".into(),
            source: format!("trace {}", sources.join("+")),
            precursor_mz: Some(p),
            product_mz: Some(d),
            intensity_unit: unit.or(Some("counts".into())),
            ..Chromatogram::default()
        };
        if traces.len() > 1 {
            c.notes.push(format!(
                "{} stored chromatograms of this transition (scheduled windows) merged",
                traces.len()
            ));
        }
        (c.rt_min, c.intensity) = pts.into_iter().unzip();
        return Ok(c);
    }
    let mut best: Option<(u32, u64, usize, usize, f64, f64)> = None;
    for tab in &info.tables {
        let Some(tcol) = tab.columns.iter().position(|c| c.name == "rt_min") else {
            continue;
        };
        for (k, col) in tab.columns.iter().enumerate() {
            if let Some((a, b)) = transition_of(&col.name)
                && dist(a, b) <= tol
                && best.is_none_or(|x| dist(a, b) < dist(x.4, x.5))
            {
                best = Some((tab.index, tab.row_count, tcol, k, a, b));
            }
        }
    }
    if let Some((table, rows, tcol, vcol, a, b)) = best {
        let mut c = from_table(ds, table, rows, tcol, vcol, req)?;
        c.label = format!("SRM {} > {}", num(q1), num(q3));
        c.kind = "srm".into();
        c.precursor_mz = Some(a);
        c.product_mz = Some(b);
        return Ok(c);
    }
    let mut hint = String::from(
        "No MS/MS spectra, stored SRM chromatogram or MRM table column matches this transition",
    );
    let known: Vec<String> = info
        .traces
        .iter()
        .filter_map(|t| {
            Some(format!(
                "{}>{}",
                num(t.extra.get("precursor_mz")?.as_f64()?),
                num(t.extra.get("product_mz")?.as_f64()?)
            ))
        })
        .chain(
            info.tables
                .iter()
                .flat_map(|t| t.columns.iter())
                .filter_map(|c| {
                    transition_of(&c.name).map(|(a, b)| format!("{}>{}", num(a), num(b)))
                }),
        )
        .take(12)
        .collect();
    if !known.is_empty() {
        hint.push_str(&format!(" (the file has e.g. {})", known.join(", ")));
    }
    hint.push_str("; widen --transition-tol or check `openreadout info`.");
    Err(Error::Unsupported {
        format: "chromatogram",
        feature: format!("SRM transition {} > {}", num(q1), num(q3)),
        hint: Some(hint),
    })
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;

    fn sp(i: u64, rt_s: f64, level: u32, mz: Vec<f64>, it: Vec<f32>) -> Spectrum {
        Spectrum {
            index: i,
            scan_number: i + 1,
            ms_level: level,
            rt_s: Some(rt_s),
            polarity: "positive".into(),
            centroided: true,
            mz,
            intensity: it,
            ..Spectrum::default()
        }
    }

    #[test]
    fn xic_window_and_aggregate() {
        let s = sp(
            0,
            60.0,
            1,
            vec![100.0, 100.0005, 100.002, 200.0],
            vec![1.0, 2.0, 4.0, 8.0],
        );
        let mut req = ChromRequest::default();
        req.targets = vec![
            Target::Xic { mz: 100.0 },
            Target::Tic,
            Target::Bpc,
            Target::Xic { mz: 150.0 },
        ];
        let t: Vec<(usize, Target)> = req.targets.iter().cloned().enumerate().collect();
        let mut accs = vec![Acc::default(); 4];
        visit(&s, &t, &req, &mut accs);
        assert_eq!(accs[0].pts[0].v, 3.0); // 10 ppm of 100 = ±0.001
        assert_eq!(accs[1].pts[0].v, 15.0);
        assert_eq!(accs[2].pts[0].v, 8.0);
        assert_eq!(accs[3].pts[0].v, 0.0);
        assert!((accs[0].pts[0].rt - 1.0).abs() < 1e-12);
        req.aggregate = Aggregate::Max;
        req.tolerance = Tolerance::Da(0.01);
        let mut accs = vec![Acc::default(); 4];
        visit(&s, &t, &req, &mut accs);
        assert_eq!(accs[0].pts[0].v, 4.0);
        // MS2 scans are not in an MS1 XIC; polarity filter
        let s2 = sp(1, 61.0, 2, vec![100.0], vec![5.0]);
        let mut accs = vec![Acc::default(); 4];
        visit(&s2, &t, &req, &mut accs);
        assert!(accs.iter().all(|a| a.pts.is_empty()));
        req.polarity = Some(Polarity::Negative);
        let mut accs = vec![Acc::default(); 4];
        visit(&s, &t, &req, &mut accs);
        assert!(accs.iter().all(|a| a.pts.is_empty()));
    }

    /// A scan whose file states no retention time is left out of every chromatogram (never
    /// placed at 0), and the chromatogram says how many were.
    #[test]
    fn scans_without_retention_time_are_left_out() {
        let mut req = ChromRequest::default();
        req.targets = vec![Target::Tic, Target::Xic { mz: 100.0 }];
        req.rt_range_min = Some([0.0, 10.0]);
        let t: Vec<(usize, Target)> = req.targets.iter().cloned().enumerate().collect();
        let mut accs = vec![Acc::default(); 2];
        visit(&sp(0, 60.0, 1, vec![100.0], vec![2.0]), &t, &req, &mut accs);
        let mut none = sp(1, 0.0, 1, vec![100.0], vec![5.0]);
        none.rt_s = None;
        visit(&none, &t, &req, &mut accs);
        // an MS2 scan without a time is not a matching scan of an MS1 chromatogram
        let mut ms2 = sp(2, 0.0, 2, vec![100.0], vec![7.0]);
        ms2.rt_s = None;
        visit(&ms2, &t, &req, &mut accs);
        for a in &accs {
            assert_eq!(a.pts.len(), 1);
            assert_eq!(a.no_rt, 1);
        }
        assert_eq!(
            accs[0].computed, 1,
            "the TIC counts only the scan it placed"
        );
        let c = finish_spectra(&t[0].1, accs.remove(0), &req);
        assert_eq!(c.rt_min, vec![1.0]);
        assert!(
            c.notes
                .iter()
                .any(|n| n.starts_with("1 matching scan is left out")),
            "{:?}",
            c.notes
        );
    }

    #[test]
    fn srm_nearest_precursor_and_product() {
        let mut req = ChromRequest::default();
        req.targets = vec![Target::Srm {
            q1: 115.0,
            q3: 71.1,
        }];
        let t: Vec<(usize, Target)> = req.targets.iter().cloned().enumerate().collect();
        let mut accs = vec![Acc::default()];
        for (i, (p, prods)) in [
            (115.007, vec![(27.28, 1.0f32), (71.103, 10.0)]),
            (115.05, vec![(71.143, 20.0)]),
            (115.007, vec![(27.28, 2.0), (71.103, 30.0)]),
        ]
        .into_iter()
        .enumerate()
        {
            let mut s = sp(i as u64, i as f64 * 6.0, 2, Vec::new(), Vec::new());
            s.precursor_mz = Some(p);
            (s.mz, s.intensity) = prods.into_iter().unzip();
            visit(&s, &t, &req, &mut accs);
        }
        let c = finish_spectra(&t[0].1, accs.remove(0), &req);
        assert_eq!(c.intensity, vec![10.0, 30.0]);
        assert_eq!(c.precursor_mz, Some(115.007));
        assert_eq!(c.product_mz, Some(71.103));
        assert!(c.notes[0].contains("115.05"), "{:?}", c.notes);
    }

    #[test]
    fn decimate_keeps_maxima() {
        let mut c = Chromatogram {
            rt_min: (0..100).map(f64::from).collect(),
            intensity: (0..100).map(|i| if i == 37 { 99.0 } else { 1.0 }).collect(),
            ..Chromatogram::default()
        };
        c.summarize();
        c.decimate(10);
        assert_eq!(c.rt_min.len(), 10);
        assert!(c.intensity.contains(&99.0));
        assert_eq!(c.points, 100);
        assert_eq!(c.apex_rt_min, Some(37.0));
    }

    #[test]
    fn column_names_parse_as_transitions() {
        assert_eq!(transition_of("207.82 > 99.66"), Some((207.82, 99.66)));
        assert_eq!(transition_of("rt_min"), None);
    }
}
