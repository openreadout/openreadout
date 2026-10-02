//! JSON queries for `analyze chromatogram` and `analyze peaks`, shared by the MCP tool
//! (`openreadout_analyze` kinds `chromatogram` and `peaks`) and the Python bindings: the same
//! field names and defaults everywhere.

use openreadout_core::model::FileInfo;
use openreadout_core::parallel::ReadContext;
use openreadout_core::{Dataset, Error, Result};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::analyze::{PeaksOutput, PeaksRequest, run_peaks};
use crate::bands::RegionBaseline;
use crate::extract::{
    Aggregate, ChromRequest, ChromatogramOutput, Polarity, Target, Tolerance, extract,
};
use crate::peaks::{AreaTimeUnit, BaselineMode, PeakParams, PickRule};
use crate::targets::Compound;

/// Points per chromatogram a query returns by default.
pub const DEFAULT_CHROMATOGRAM_POINTS: usize = 500;
/// Most points per chromatogram a query returns.
pub const MAX_CHROMATOGRAM_POINTS: usize = 20_000;

/// Which chromatograms, from which scans (shared by both tools).
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct SourceArgs {
    /// Total-ion chromatogram (the default for mass-spectrometry files when nothing else is chosen).
    #[serde(default)]
    pub tic: bool,
    /// Base-peak chromatogram.
    #[serde(default)]
    pub bpc: bool,
    /// Extracted-ion chromatograms at these m/z values (one streaming pass for all).
    #[serde(default)]
    pub mz: Vec<f64>,
    /// XIC tolerance (half-width) in ppm. Default 10.
    pub ppm: Option<f64>,
    /// XIC tolerance in m/z units instead of ppm.
    pub da: Option<f64>,
    /// SRM/MRM transitions as `"Q1>Q3"` strings, e.g. `"279.2>179.2"`.
    #[serde(default)]
    pub transitions: Vec<String>,
    /// Q1/Q3 tolerance of transitions, m/z units. Default 0.5.
    pub transition_tol: Option<f64>,
    /// Stored chromatograms or detector signals (UV/DAD, FID, TCD, stored TIC/SRM): trace indices from openreadout_info → traces[].
    #[serde(default)]
    pub traces: Vec<u32>,
    /// Channel of the traces (e.g. one DAD wavelength). Default: the first signal channel.
    pub channel: Option<u32>,
    /// Sweep of the traces (one spectrum of a map or series, one injection). Default 0.
    pub sweep: Option<u32>,
    /// Spectra run. Default 0.
    #[serde(default)]
    pub run: u32,
    /// MS level of the scans. Default 1 (2 with `precursor`).
    pub ms_level: Option<u32>,
    /// `positive` or `negative`: only scans of this polarity.
    pub polarity: Option<String>,
    /// Only scans whose scan filter/description contains this text (case-insensitive).
    pub scan_filter: Option<String>,
    /// Only MS/MS scans of this precursor m/z (product-ion XIC).
    pub precursor: Option<f64>,
    /// Precursor tolerance, m/z units. Default 0.5.
    pub precursor_tol: Option<f64>,
    /// Retention-time range `[start, end]`, minutes.
    pub rt_range: Option<[f64; 2]>,
    /// TIC/BPC over this m/z range only, `[low, high]`.
    pub mz_range: Option<[f64; 2]>,
    /// Use profile data where scans store both profile and centroids. Default false (the
    /// instrument's centroids).
    #[serde(default)]
    pub profile: bool,
    /// `sum` (default) or `max` of the points inside an XIC window.
    pub aggregate: Option<String>,
}

impl SourceArgs {
    /// The extraction request.
    pub fn request(&self) -> Result<ChromRequest> {
        let mut r = ChromRequest::default();
        if self.tic {
            r.targets.push(Target::Tic);
        }
        if self.bpc {
            r.targets.push(Target::Bpc);
        }
        r.targets
            .extend(self.mz.iter().map(|&mz| Target::Xic { mz }));
        for t in &self.transitions {
            let bad = || Error::Usage(format!("transition {t:?}: expected \"Q1>Q3\""));
            let (a, b) = t
                .split_once('>')
                .or_else(|| t.split_once('/'))
                .ok_or_else(bad)?;
            r.targets.push(Target::Srm {
                q1: a.trim().parse().map_err(|_| bad())?,
                q3: b.trim().parse().map_err(|_| bad())?,
            });
        }
        r.targets
            .extend(self.traces.iter().map(|&trace| Target::Trace {
                trace,
                channel: self.channel,
            }));
        if self.traces.is_empty() && self.channel.is_some() {
            r.targets.push(Target::Trace {
                trace: 0,
                channel: self.channel,
            });
        }
        if let Some(p) = self.ppm {
            r.tolerance = Tolerance::Ppm(p);
        }
        if let Some(d) = self.da {
            r.tolerance = Tolerance::Da(d);
        }
        if let Some(t) = self.transition_tol {
            r.transition_tolerance_da = t;
        }
        r.run = self.run;
        r.sweep = self.sweep.unwrap_or(0);
        r.ms_level = self.ms_level;
        r.polarity = self
            .polarity
            .as_deref()
            .map(str::parse::<Polarity>)
            .transpose()?;
        r.scan_filter.clone_from(&self.scan_filter);
        r.precursor_mz = self.precursor;
        if let Some(t) = self.precursor_tol {
            r.precursor_tolerance_da = t;
        }
        r.rt_range_min = self.rt_range;
        r.mz_range = self.mz_range;
        r.profile = self.profile;
        r.aggregate = match self.aggregate.as_deref() {
            None | Some("sum") => Aggregate::Sum,
            Some("max") => Aggregate::Max,
            Some(o) => return Err(Error::Usage(format!("aggregate {o:?}: use sum or max"))),
        };
        Ok(r)
    }
}

/// An `analyze chromatogram` query.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ChromatogramQuery {
    /// Which chromatograms.
    #[serde(flatten)]
    pub source: SourceArgs,
    /// Points returned per chromatogram (the most intense point of each of N equal slices).
    /// Default 500, max 20000; the summary (apex, integral) always covers every point.
    pub max_points: Option<usize>,
}

/// An `analyze peaks` query.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct PeaksQuery {
    /// Which chromatograms (default: the first detector trace, else the TIC).
    #[serde(flatten)]
    pub source: SourceArgs,
    /// Savitzky–Golay window in points (below 5: none). Default automatic.
    pub smooth: Option<u32>,
    /// Detection threshold, height / noise σ. Default 3.
    pub min_snr: Option<f64>,
    /// Detection threshold in signal units.
    pub min_height: Option<f64>,
    /// Smallest width at half height, minutes.
    pub min_width: Option<f64>,
    /// Baseline under detected peaks: `auto` (default), `drop`, `valley` or `tangent`; or the
    /// baseline under `x_range` windows and manual integrations: `linear` (default: a straight
    /// line between the window's ends) or `none` (area of the signal itself).
    pub baseline: Option<String>,
    /// Areas in signal × seconds instead of signal × minutes. Default false.
    #[serde(default)]
    pub area_seconds: bool,
    /// Expected retention time (minutes): also report the peak there as `picked`.
    pub rt: Option<f64>,
    /// Half-width of the `rt` window (and of compounds without one), minutes. Default 0.5.
    pub window: Option<f64>,
    /// `largest` (default) or `nearest` peak in the window.
    pub pick: Option<String>,
    /// Manual integrations `[[start, end], ...]` in minutes.
    #[serde(default)]
    pub integrate: Vec<[f64; 2]>,
    /// Axis windows `[[a, b], ...]` to integrate, in the axis's own units (either order): on a
    /// spectrum (IR/Raman cm⁻¹, UV nm, NMR ppm) each is a region under `spectra[].regions` (area
    /// with the `baseline`, area without it, maximum, centroid, the bands inside); on a
    /// chromatogram a manual integration in minutes.
    #[serde(default)]
    pub x_range: Vec<[f64; 2]>,
    /// Targeted compounds: `[{name, mz? | q1+q3? | trace?, rt?, window?, ppm?, polarity?, ms_level?, precursor?}]`; one result row each under `compounds`.
    #[serde(default)]
    pub compounds: Vec<Compound>,
    /// Leave out the full peak tables (keep compounds, picked and manual results). Default false.
    #[serde(default)]
    pub summary_only: bool,
}

/// Run an `analyze chromatogram` query on an open file.
pub fn chromatogram(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    q: &ChromatogramQuery,
    ctx: &ReadContext<'_>,
) -> Result<ChromatogramOutput> {
    let req = q.source.request()?;
    let mut out = extract(ds, info, &req, ctx)?;
    let max = q
        .max_points
        .unwrap_or(DEFAULT_CHROMATOGRAM_POINTS)
        .clamp(1, MAX_CHROMATOGRAM_POINTS);
    for c in &mut out.chromatograms {
        c.decimate(max);
    }
    Ok(out)
}

/// Run an `analyze peaks` query on an open file.
pub fn peaks(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    a: &PeaksQuery,
    ctx: &ReadContext<'_>,
) -> Result<PeaksOutput> {
    let mut r = PeaksRequest {
        chrom: a.source.request()?,
        default_window_min: a.window,
        integrate: a.integrate.clone(),
        compounds: a.compounds.clone(),
        x_ranges: a.x_range.clone(),
        ..PeaksRequest::default()
    };
    let mut p = PeakParams {
        smooth: a.smooth,
        min_height: a.min_height,
        min_width_min: a.min_width,
        rt_range_min: a.source.rt_range,
        ..PeakParams::default()
    };
    if let Some(s) = a.min_snr {
        p.min_snr = s;
    }
    if let Some(b) = &a.baseline {
        match b.to_ascii_lowercase().as_str() {
            "linear" | "straight" => r.region_baseline = RegionBaseline::Linear,
            "none" | "zero" | "off" => r.region_baseline = RegionBaseline::None,
            _ => p.baseline = b.parse::<BaselineMode>()?,
        }
    }
    if a.area_seconds {
        p.area_time_unit = AreaTimeUnit::S;
    }
    r.params = p;
    if let Some(k) = &a.pick {
        r.pick = k.parse::<PickRule>()?;
    }
    if let Some(rt) = a.rt {
        r.expect = Some((
            rt,
            a.window.unwrap_or(crate::analyze::DEFAULT_RT_WINDOW_MIN),
        ));
    }
    let (mut out, _) = run_peaks(ds, info, &r, ctx)?;
    if a.summary_only {
        for c in &mut out.chromatograms {
            c.peaks.clear();
        }
        for s in &mut out.spectra {
            s.peaks.clear();
        }
    }
    Ok(out)
}
