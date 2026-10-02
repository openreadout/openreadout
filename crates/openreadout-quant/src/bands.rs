//! Bands and regions of spectra: peak picking and integration on any trace whose `axis` is not
//! time — IR/Raman wavenumber or Raman shift, UV-Vis wavelength, NMR chemical shift, m/z of a
//! profile spectrum, interferogram points.
//!
//! Method (`book/src/guides/quantitation.md` "Spectral bands and regions"):
//!
//! - The spectrum is read whole (one sweep of one trace), its samples ordered by increasing x
//!   (wavenumber and ppm axes are usually stored descending) with non-finite samples left out.
//! - **Regions** (`x_ranges`): the samples with `lo ≤ x ≤ hi` (no interpolation at the ends; at
//!   least two are needed). Baseline `linear` (default): the straight line through the raw
//!   signal at the first and last of those samples; `none`: zero. `area` = trapezoidal integral
//!   of (signal − baseline) over them, in signal units × axis units; `area_no_baseline` = the
//!   integral of the signal itself; the region maximum is the sample of largest
//!   (signal − baseline) and the centroid the (signal − baseline)-weighted mean x. This is
//!   `numpy.trapezoid(y[m] - b, x[m])` with `m = (x >= lo) & (x <= hi)`, the way band areas and
//!   indices (carbonyl, sulfoxide) are usually scripted. `peaks_area` sums the detected bands
//!   whose apex lies in the region (their own baselines: the `auto` detection baseline).
//! - **Bands**: the chromatographic detector ([`crate::peaks::find_peaks`]) run on the whole
//!   spectrum with x in place of time (widths and windows are in axis units), so noise, smoothing,
//!   S/N and baselines follow the same documented rules. With regions, only bands whose apex lies
//!   in one are listed.
//! - **Transmittance and reflectance** spectra (the signal's quantity or unit says so) have their
//!   bands as minima: bands are detected on the negated signal and their heights and areas are
//!   depths below the baseline (positive); region areas stay signed integrals of the signal minus
//!   the baseline (negative over an absorption band).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use openreadout_core::model::{FileInfo, TraceInfo};
use openreadout_core::{Dataset, Error, Result};

use crate::peaks::{Peak, PeakParams, find_peaks};

/// Samples read per `read_trace` call.
const CHUNK: u64 = 1 << 20;
/// Most samples one spectrum may have (a larger trace is a signal, not a spectrum).
const MAX_POINTS: u64 = 1 << 26;

/// Baseline under a region.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RegionBaseline {
    /// A straight line through the signal at the first and last sample of the region.
    #[default]
    Linear,
    /// No baseline: the integral of the signal itself.
    None,
}

impl RegionBaseline {
    /// `linear` or `none`.
    pub fn id(self) -> &'static str {
        match self {
            RegionBaseline::Linear => "linear",
            RegionBaseline::None => "none",
        }
    }
}

/// The quantity of a trace's axis, when it is not time (`wavenumber`, `raman_shift`,
/// `wavelength`, `chemical_shift`, `points`, …). A chromatogram whose axis is the retention
/// volume (ÄKTA/UNICORN curves, sampled in time) is not a spectrum.
pub fn spectral_axis(t: &TraceInfo) -> Option<String> {
    let a = t.extra.get("axis")?;
    let q = a.get("quantity").and_then(|v| v.as_str()).unwrap_or("");
    if matches!(q, "time" | "retention_time" | "retention_volume" | "") {
        return None;
    }
    Some(q.to_string())
}

/// One spectrum read for band analysis: x ascending, finite samples only.
#[derive(Debug, Clone, Default)]
pub struct Spectrum {
    /// Label: the trace name (and channel name when the trace has several signals).
    pub label: String,
    /// Trace index.
    pub trace: u32,
    /// Sweep index.
    pub sweep: u32,
    /// Channel index of the signal.
    pub channel: u32,
    /// Axis quantity.
    pub x_quantity: String,
    /// Axis unit.
    pub x_unit: Option<String>,
    /// Signal channel name.
    pub y_quantity: String,
    /// Signal unit.
    pub y_unit: Option<String>,
    /// Axis values, ascending.
    pub x: Vec<f64>,
    /// Signal values.
    pub y: Vec<f64>,
    /// Bands are minima (transmittance, reflectance).
    pub minima: bool,
}

fn is_minima(names: &[&str]) -> bool {
    names.iter().any(|n| {
        let l = n.to_ascii_lowercase();
        l.contains("transmittance") || l.contains("reflectance") || l == "%t" || l == "%r"
    })
}

/// Read sweep `sweep` of trace `trace` (signal `channel`, default the first channel that is not
/// the axis) as a spectrum.
pub fn read_spectrum(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    trace: u32,
    channel: Option<u32>,
    sweep: u32,
) -> Result<Spectrum> {
    let t = info
        .traces
        .iter()
        .find(|t| t.index == trace)
        .ok_or_else(|| {
            Error::Usage(format!(
                "trace {trace} out of range (file has {} traces)",
                info.traces.len()
            ))
        })?;
    let x_quantity = spectral_axis(t).ok_or_else(|| {
        Error::unsupported(
            "bands",
            format!(
                "trace {trace} ({}) as a spectrum: its axis is time",
                t.name.as_deref().unwrap_or("unnamed")
            ),
            "Time signals and chromatograms are integrated by retention time (`analyze peaks --integrate A-B`).",
        )
    })?;
    if sweep >= t.sweep_count {
        return Err(Error::Usage(format!(
            "sweep {sweep} out of range (trace {trace} has {} sweeps)",
            t.sweep_count
        )));
    }
    let axis = &t.extra["axis"];
    let num = |k: &str| axis.get(k).and_then(serde_json::Value::as_f64);
    let irregular = axis.get("irregular").and_then(serde_json::Value::as_bool) == Some(true);
    let axis_ch = if irregular {
        let c = axis
            .get("channel")
            .and_then(serde_json::Value::as_u64)
            .and_then(|c| usize::try_from(c).ok())
            .filter(|&c| c < t.channels.len())
            .ok_or_else(|| {
                Error::corrupt(
                    "bands",
                    format!("trace {trace}: irregular axis without a valid channel"),
                )
            })?;
        Some(c)
    } else {
        None
    };
    let regular = if irregular {
        None
    } else {
        match (num("first"), num("step")) {
            (Some(f), Some(s)) if f.is_finite() && s.is_finite() && s != 0.0 => Some((f, s)),
            _ => {
                return Err(Error::unsupported(
                    "bands",
                    format!("trace {trace}: an axis without first and step"),
                    "See `info` → traces[].extra.axis.",
                ));
            }
        }
    };
    let ch = match channel {
        Some(c) => {
            if c as usize >= t.channels.len() || Some(c as usize) == axis_ch {
                return Err(Error::Usage(format!(
                    "channel {c} is not a signal channel of trace {trace} ({} channels{})",
                    t.channels.len(),
                    axis_ch.map_or(String::new(), |a| format!(", {a} is the axis"))
                )));
            }
            c as usize
        }
        None => (0..t.channels.len())
            .find(|&c| Some(c) != axis_ch)
            .ok_or_else(|| Error::Usage(format!("trace {trace} has no signal channel")))?,
    };
    let n = openreadout_core::trace::sweep_samples(t, sweep);
    if n > MAX_POINTS {
        return Err(Error::unsupported(
            "bands",
            format!("a spectrum of {n} points"),
            "Narrow the data first (`trace --x-range`), or analyse fewer points.",
        ));
    }
    let mut pts: Vec<(f64, f64)> = Vec::with_capacity(n.min(1 << 20) as usize);
    let mut pos = 0u64;
    while pos < n {
        let want = CHUNK.min(n - pos);
        let tr = ds.read_trace(trace, sweep, pos, want)?;
        let ys = tr
            .channels
            .get(ch)
            .ok_or_else(|| Error::Other(format!("reader returned no channel {ch}")))?;
        let xs = axis_ch.and_then(|a| tr.channels.get(a));
        for (i, &v) in ys.iter().enumerate() {
            let x = match (regular, xs) {
                (Some((f, s)), _) => f + s * (pos + i as u64) as f64,
                (None, Some(xc)) => xc.get(i).copied().unwrap_or(f64::NAN),
                (None, None) => f64::NAN,
            };
            if x.is_finite() && v.is_finite() {
                pts.push((x, v));
            }
        }
        let got = ys.len() as u64;
        pos += got;
        if got < want || got == 0 {
            break;
        }
    }
    if pts.windows(2).any(|w| w[1].0 < w[0].0) {
        pts.sort_by(|a, b| a.0.total_cmp(&b.0));
    }
    let chan = &t.channels[ch];
    let name = t.name.clone().unwrap_or_else(|| format!("trace {trace}"));
    let signals = t.channels.len() - usize::from(axis_ch.is_some());
    let y_quantity = t
        .extra
        .get("y_quantity")
        .and_then(|v| v.as_str())
        .map_or_else(|| chan.name.clone(), str::to_string);
    let minima = is_minima(&[&chan.name, chan.unit.as_deref().unwrap_or(""), &y_quantity]);
    let (x, y) = pts.into_iter().unzip();
    Ok(Spectrum {
        label: if signals > 1 {
            format!("{name}: {}", chan.name)
        } else {
            name
        },
        trace,
        sweep,
        channel: ch as u32,
        x_quantity,
        x_unit: axis
            .get("unit")
            .and_then(|v| v.as_str())
            .filter(|u| !u.is_empty())
            .map(str::to_string),
        y_quantity,
        y_unit: chan.unit.clone().filter(|u| !u.is_empty()),
        x,
        y,
        minima,
    })
}

/// One integrated region of a spectrum.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Region {
    /// 1-based region number in request order.
    pub number: u32,
    /// Lower end asked for, axis units.
    pub from: f64,
    /// Upper end asked for, axis units.
    pub to: f64,
    /// Axis value of the first sample used.
    pub x_first: f64,
    /// Axis value of the last sample used.
    pub x_last: f64,
    /// Samples used.
    pub points: u32,
    /// `linear` or `none`.
    pub baseline: String,
    /// Baseline at `x_first`, signal units.
    pub baseline_start: f64,
    /// Baseline at `x_last`, signal units.
    pub baseline_end: f64,
    /// Trapezoidal integral of (signal − baseline), signal units × axis units.
    pub area: f64,
    /// Trapezoidal integral of the signal itself.
    pub area_no_baseline: f64,
    /// Axis value of the region maximum (the sample of largest signal − baseline; of largest
    /// baseline − signal when bands are minima).
    pub max_x: f64,
    /// Height of the maximum above the baseline (depth below it when bands are minima).
    pub max_height: f64,
    /// Signal at the maximum.
    pub max_value: f64,
    /// (signal − baseline)-weighted mean axis value (absent when that integral is not positive).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub centroid: Option<f64>,
    /// Numbers of the detected bands whose apex lies in the region.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub peaks: Vec<u32>,
    /// Summed area of those bands (each above its own detection baseline).
    pub peaks_area: f64,
}

/// One detected band.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Band {
    /// 1-based band number in order of increasing x.
    pub number: u32,
    /// Apex position (vertex of the parabola through the three highest smoothed points), axis
    /// units.
    pub x: f64,
    /// Band start, axis units.
    pub start: f64,
    /// Band end, axis units.
    pub end: f64,
    /// Height above the baseline (depth below it when bands are minima), signal units.
    pub height: f64,
    /// Area above the baseline (below it when bands are minima), signal units × axis units.
    pub area: f64,
    /// Share of the summed area of all bands of the spectrum, %.
    pub area_percent: f64,
    /// (signal − baseline)-weighted mean position.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub centroid: Option<f64>,
    /// Full width at half height, axis units.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width_half: Option<f64>,
    /// Height / noise σ.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snr: Option<f64>,
    /// Samples from start to end.
    pub points: u32,
    /// How each end meets the baseline (as `analyze peaks`: `B`, `V`, `T`).
    pub baseline_code: String,
    /// Baseline at the start, signal units.
    pub baseline_start: f64,
    /// Baseline at the end, signal units.
    pub baseline_end: f64,
    /// `auto` baseline: why the baseline was drawn this way (as `analyze peaks`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline_reason: Option<String>,
}

/// How the bands were found.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct BandMethod {
    /// `savitzky_golay_quadratic` or `none`.
    pub smoothing: String,
    /// Smoothing window, points (1 = none).
    pub smooth_window_points: u32,
    /// Noise σ, signal units.
    pub noise: f64,
    /// How the noise was estimated.
    pub noise_method: String,
    /// Detection threshold, S/N.
    pub min_snr: f64,
    /// Detection threshold, signal units.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_height: Option<f64>,
    /// Smallest width at half height, axis units.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_width: Option<f64>,
    /// Baseline under detected bands: `auto`, `drop`, `valley` or `tangent`.
    pub baseline: String,
    /// Baseline under regions: `linear` or `none`.
    pub region_baseline: String,
    /// `trapezoid`.
    pub integration: String,
}

/// Bands and regions of one spectrum.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct SpectrumBands {
    /// Label (trace name).
    pub label: String,
    /// Always `spectrum`.
    pub kind: String,
    /// `trace N sweep S`.
    pub source: String,
    /// Trace index.
    pub trace: u32,
    /// Sweep index.
    pub sweep: u32,
    /// Channel of the signal.
    pub channel: u32,
    /// Axis quantity: `wavenumber`, `raman_shift`, `wavelength`, `chemical_shift`, `points`, ….
    pub x_quantity: String,
    /// Axis unit (`1/cm`, `nm`, `ppm`, …).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x_unit: Option<String>,
    /// Signal quantity (`absorbance`, `intensity`, `transmittance`, …).
    pub y_quantity: String,
    /// Signal unit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intensity_unit: Option<String>,
    /// Unit of areas: signal unit (or quantity) × axis unit.
    pub area_unit: String,
    /// Samples in the spectrum.
    pub points: u64,
    /// Smallest axis value.
    pub x_min: f64,
    /// Largest axis value.
    pub x_max: f64,
    /// Bands are minima (transmittance, reflectance): detected on the negated signal.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bands_are_minima: bool,
    /// How bands were found.
    pub method: BandMethod,
    /// Bands listed.
    pub peak_count: u32,
    /// Detected bands (with regions: only those with the apex in one), in order of x.
    pub peaks: Vec<Band>,
    /// Integrated regions, in request order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub regions: Vec<Region>,
    /// Anything the caller should know.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

fn trapz(x: &[f64], v: impl Fn(usize) -> f64) -> f64 {
    let mut s = 0.0;
    for i in 1..x.len() {
        s += f64::midpoint(v(i - 1), v(i)) * (x[i] - x[i - 1]);
    }
    s
}

/// Integrate the region `[lo, hi]` (either order) of `(x, y)` (x ascending).
pub fn integrate_region(
    x: &[f64],
    y: &[f64],
    range: [f64; 2],
    baseline: RegionBaseline,
    minima: bool,
) -> Result<Region> {
    let (lo, hi) = (range[0].min(range[1]), range[0].max(range[1]));
    if !(lo.is_finite() && hi.is_finite()) || lo == hi {
        return Err(Error::Usage(format!(
            "region {}:{} needs two different finite axis values",
            range[0], range[1]
        )));
    }
    let a = x.partition_point(|&v| v < lo);
    let b = x.partition_point(|&v| v <= hi);
    if b < a + 2 {
        let (first, last) = (x.first().copied(), x.last().copied());
        return Err(Error::Usage(format!(
            "region {lo}:{hi} holds {} sample(s); at least 2 are needed (the spectrum spans {}..{})",
            b.saturating_sub(a),
            first.map_or("-".into(), |v| format!("{v}")),
            last.map_or("-".into(), |v| format!("{v}"))
        )));
    }
    let (xs, ys) = (&x[a..b], &y[a..b]);
    let last = xs.len() - 1;
    let (b0, b1) = match baseline {
        RegionBaseline::Linear => (ys[0], ys[last]),
        RegionBaseline::None => (0.0, 0.0),
    };
    let base = |i: usize| {
        if xs[last] == xs[0] {
            b0
        } else {
            b0 + (b1 - b0) * (xs[i] - xs[0]) / (xs[last] - xs[0])
        }
    };
    let area = trapz(xs, |i| ys[i] - base(i));
    let raw = trapz(xs, |i| ys[i]);
    let sign = if minima { -1.0 } else { 1.0 };
    let (mut k, mut best) = (0usize, f64::NEG_INFINITY);
    for (i, &yv) in ys.iter().enumerate() {
        let h = sign * (yv - base(i));
        if h > best {
            best = h;
            k = i;
        }
    }
    let weight = trapz(xs, |i| sign * (ys[i] - base(i)));
    let centroid = (weight > 0.0)
        .then(|| trapz(xs, |i| xs[i] * sign * (ys[i] - base(i))) / weight)
        .filter(|c| c.is_finite());
    Ok(Region {
        number: 0,
        from: lo,
        to: hi,
        x_first: xs[0],
        x_last: xs[last],
        points: xs.len() as u32,
        baseline: baseline.id().into(),
        baseline_start: b0,
        baseline_end: b1,
        area,
        area_no_baseline: raw,
        max_x: xs[k],
        max_height: best,
        max_value: ys[k],
        centroid,
        peaks: Vec::new(),
        peaks_area: 0.0,
    })
}

fn band_of(p: &Peak, x: &[f64], y: &[f64], sign: f64) -> Band {
    let a = x.partition_point(|&v| v < p.start_min);
    let b = x.partition_point(|&v| v <= p.end_min);
    let centroid = (b >= a + 2)
        .then(|| {
            let (xs, ys) = (&x[a..b], &y[a..b]);
            let span = p.end_min - p.start_min;
            let base = |i: usize| {
                if span > 0.0 {
                    p.baseline_start
                        + (p.baseline_end - p.baseline_start) * (xs[i] - p.start_min) / span
                } else {
                    p.baseline_start
                }
            };
            let w = trapz(xs, |i| sign * ys[i] - base(i));
            (w > 0.0).then(|| trapz(xs, |i| xs[i] * (sign * ys[i] - base(i))) / w)
        })
        .flatten()
        .filter(|c| c.is_finite());
    Band {
        number: p.number,
        x: p.rt_min,
        start: p.start_min,
        end: p.end_min,
        height: p.height,
        area: p.area,
        area_percent: p.area_percent,
        centroid,
        width_half: p.width_half_min,
        snr: p.snr,
        points: p.points,
        baseline_code: p.baseline_code.clone(),
        // back in signal units (bands of minima were found on the negated signal)
        baseline_start: sign * p.baseline_start,
        baseline_end: sign * p.baseline_end,
        baseline_reason: p.baseline_reason.clone(),
    }
}

/// Detect bands in `s` and integrate `ranges` (axis units, either order).
pub fn analyse(
    s: &Spectrum,
    params: &PeakParams,
    ranges: &[[f64; 2]],
    baseline: RegionBaseline,
) -> Result<SpectrumBands> {
    let (Some(&x_min), Some(&x_max)) = (s.x.first(), s.x.last()) else {
        return Err(Error::Usage(format!(
            "{}: the spectrum has no finite samples",
            s.label
        )));
    };
    let sign = if s.minima { -1.0 } else { 1.0 };
    let signal: Vec<f64> = s.y.iter().map(|v| sign * v).collect();
    let mut p = params.clone();
    p.rt_range_min = None;
    let table = find_peaks(&s.x, &signal, &p)?;
    let mut regions = Vec::with_capacity(ranges.len());
    for (k, r) in ranges.iter().enumerate() {
        let mut g = integrate_region(&s.x, &s.y, *r, baseline, s.minima)?;
        g.number = k as u32 + 1;
        for pk in &table.peaks {
            if pk.rt_min >= g.from && pk.rt_min <= g.to {
                g.peaks.push(pk.number);
                g.peaks_area += pk.area;
            }
        }
        regions.push(g);
    }
    let inside =
        |x: f64| ranges.is_empty() || regions.iter().any(|g: &Region| x >= g.from && x <= g.to);
    let peaks: Vec<Band> = table
        .peaks
        .iter()
        .filter(|pk| inside(pk.rt_min))
        .map(|pk| band_of(pk, &s.x, &s.y, sign))
        .collect();
    let y_label = s.y_unit.clone().unwrap_or_else(|| s.y_quantity.clone());
    let mut notes = Vec::new();
    if s.minima {
        notes.push(format!(
            "{}: bands are minima; band heights and areas are depths below the baseline, region areas are signed (negative over an absorption band). Convert to absorbance for quantitative band areas.",
            s.y_quantity
        ));
    }
    Ok(SpectrumBands {
        label: s.label.clone(),
        kind: "spectrum".into(),
        source: format!("trace {} sweep {}", s.trace, s.sweep),
        trace: s.trace,
        sweep: s.sweep,
        channel: s.channel,
        x_quantity: s.x_quantity.clone(),
        x_unit: s.x_unit.clone(),
        y_quantity: s.y_quantity.clone(),
        intensity_unit: s.y_unit.clone(),
        area_unit: format!("{y_label}·{}", s.x_unit.as_deref().unwrap_or("x")),
        points: s.x.len() as u64,
        x_min,
        x_max,
        bands_are_minima: s.minima,
        method: BandMethod {
            smoothing: table.method.smoothing.clone(),
            smooth_window_points: table.method.smooth_window_points,
            noise: table.method.noise,
            noise_method: table.method.noise_method.clone(),
            min_snr: table.method.min_snr,
            min_height: table.method.min_height,
            min_width: table.method.min_width_min,
            baseline: table.method.baseline.clone(),
            region_baseline: baseline.id().into(),
            integration: "trapezoid".into(),
        },
        peak_count: peaks.len() as u32,
        peaks,
        regions,
        notes,
    })
}

/// The spectrum as a [`crate::extract::Chromatogram`] (x in place of retention time), for
/// plots.
pub fn as_chromatogram(s: &Spectrum) -> crate::extract::Chromatogram {
    let mut c = crate::extract::Chromatogram {
        label: s.label.clone(),
        kind: "spectrum".into(),
        source: format!("trace {} sweep {}", s.trace, s.sweep),
        intensity_unit: s.y_unit.clone().or_else(|| Some(s.y_quantity.clone())),
        rt_min: s.x.clone(),
        intensity: s.y.clone(),
        ..crate::extract::Chromatogram::default()
    };
    c.summarize();
    c
}

/// What a plot shades over a spectrum: its regions when there are any, else its bands.
pub fn plot_overlay(b: &SpectrumBands) -> crate::analyze::ChromatogramPeaks {
    let sign = if b.bands_are_minima { -1.0 } else { 1.0 };
    let peaks: Vec<Peak> = if b.regions.is_empty() {
        b.peaks
            .iter()
            .map(|p| Peak {
                number: p.number,
                rt_min: p.x,
                start_min: p.start,
                end_min: p.end,
                height: p.height,
                area: p.area,
                baseline_start: p.baseline_start,
                baseline_end: p.baseline_end,
                ..Peak::default()
            })
            .collect()
    } else {
        b.regions
            .iter()
            .map(|g| Peak {
                number: g.number,
                rt_min: g.max_x,
                start_min: g.x_first,
                end_min: g.x_last,
                height: sign * g.max_height,
                area: g.area,
                baseline_start: g.baseline_start,
                baseline_end: g.baseline_end,
                ..Peak::default()
            })
            .collect()
    };
    crate::analyze::ChromatogramPeaks {
        label: b.label.clone(),
        kind: "spectrum".into(),
        source: b.source.clone(),
        area_unit: b.area_unit.clone(),
        points: b.points,
        peak_count: peaks.len() as u32,
        peaks,
        ..crate::analyze::ChromatogramPeaks::default()
    }
}

/// Axis label of a spectrum plot, e.g. `WAVENUMBER (1/CM)`.
pub fn axis_label(b: &SpectrumBands) -> String {
    let q = b.x_quantity.replace('_', " ");
    match &b.x_unit {
        Some(u) => format!("{q} ({u})").to_uppercase(),
        None => q.to_uppercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gauss(x: f64, c: f64, w: f64, h: f64) -> f64 {
        h * (-0.5 * ((x - c) / w).powi(2)).exp()
    }

    fn spectrum(minima: bool) -> Spectrum {
        let x: Vec<f64> = (0..=1000).map(|i| 1000.0 + f64::from(i)).collect();
        let y: Vec<f64> = x
            .iter()
            .map(|&v| {
                let s = 0.1
                    + 1e-4 * (v - 1000.0)
                    + gauss(v, 1300.0, 8.0, 1.0)
                    + gauss(v, 1700.0, 12.0, 0.5);
                if minima { 1.0 - s } else { s }
            })
            .collect();
        Spectrum {
            label: "test".into(),
            x_quantity: "wavenumber".into(),
            x_unit: Some("1/cm".into()),
            y_quantity: if minima {
                "transmittance"
            } else {
                "absorbance"
            }
            .into(),
            x,
            y,
            minima,
            ..Spectrum::default()
        }
    }

    #[test]
    fn region_matches_trapezoid_with_a_linear_baseline() {
        let s = spectrum(false);
        let r =
            integrate_region(&s.x, &s.y, [1340.0, 1260.0], RegionBaseline::Linear, false).unwrap();
        assert_eq!((r.from, r.to), (1260.0, 1340.0));
        assert_eq!(r.points, 81);
        // the band's Gaussian area h·w·√(2π) (its tails beyond ±5σ are negligible)
        let want = 1.0 * 8.0 * (2.0 * std::f64::consts::PI).sqrt();
        assert!((r.area - want).abs() < 1e-3 * want, "{}", r.area);
        assert_eq!(r.max_x, 1300.0);
        assert!((r.centroid.unwrap() - 1300.0).abs() < 1e-6);
        let none =
            integrate_region(&s.x, &s.y, [1260.0, 1340.0], RegionBaseline::None, false).unwrap();
        assert_eq!(none.area, none.area_no_baseline);
        assert!(none.area > r.area);
        // the sloped background under the band: (0.126+0.134)/2 × 80
        assert!((none.area - r.area - 0.13 * 80.0).abs() < 1e-3);
    }

    #[test]
    fn regions_need_two_samples() {
        let s = spectrum(false);
        assert!(
            integrate_region(&s.x, &s.y, [1300.2, 1300.8], RegionBaseline::Linear, false).is_err()
        );
        assert!(integrate_region(&s.x, &s.y, [5.0, 5.0], RegionBaseline::Linear, false).is_err());
        assert!(
            integrate_region(&s.x, &s.y, [f64::NAN, 5.0], RegionBaseline::Linear, false).is_err()
        );
        assert!(integrate_region(&[], &[], [1.0, 2.0], RegionBaseline::Linear, false).is_err());
    }

    #[test]
    fn bands_are_found_and_limited_to_regions() {
        let s = spectrum(false);
        let out = analyse(&s, &PeakParams::default(), &[], RegionBaseline::Linear).unwrap();
        assert_eq!(out.peak_count, 2, "{:?}", out.peaks);
        assert!((out.peaks[0].x - 1300.0).abs() < 0.5);
        assert!((out.peaks[1].x - 1700.0).abs() < 0.5);
        let w = out.peaks[0].width_half.unwrap();
        assert!((w - 2.3548 * 8.0).abs() < 1.0, "{w}");
        let out = analyse(
            &s,
            &PeakParams::default(),
            &[[1650.0, 1750.0]],
            RegionBaseline::Linear,
        )
        .unwrap();
        assert_eq!(out.peak_count, 1);
        assert_eq!(out.regions[0].peaks, vec![out.peaks[0].number]);
        assert!(out.regions[0].peaks_area > 0.0);
        assert_eq!(out.area_unit, "absorbance·1/cm");
    }

    #[test]
    fn transmittance_bands_are_minima() {
        let s = spectrum(true);
        let out = analyse(
            &s,
            &PeakParams::default(),
            &[[1260.0, 1340.0]],
            RegionBaseline::Linear,
        )
        .unwrap();
        assert!(out.bands_are_minima);
        assert_eq!(out.peak_count, 1);
        assert!(out.peaks[0].height > 0.9 && out.peaks[0].area > 0.0);
        let r = &out.regions[0];
        assert!(r.area < 0.0);
        assert_eq!(r.max_x, 1300.0);
        assert!(r.max_height > 0.9);
        assert!((r.centroid.unwrap() - 1300.0).abs() < 1e-6);
        assert!(!out.notes.is_empty());
    }
}
