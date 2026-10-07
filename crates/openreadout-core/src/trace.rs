//! Sampled-signal slices for JSON consumers (`openreadout trace`, the `openreadout_trace`
//! MCP tool): a window of one sweep, per-channel statistics over the whole window, and at most a
//! capped number of the samples themselves.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::model::{FileInfo, TraceInfo};
use crate::reader::Dataset;

/// Samples read from the reader per batch while computing statistics.
const CHUNK: u64 = 1 << 20;

/// Largest `max_samples` a slice returns per channel.
pub const MAX_SLICE_SAMPLES: u64 = 100_000;

/// What to read.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct TraceRequest {
    /// Trace index (see `info` → `traces[]`).
    pub trace: u32,
    /// Sweep index within the trace.
    pub sweep: u32,
    /// Channel indices; empty = all.
    pub channels: Vec<u32>,
    /// First sample of the window, zero-based within the sweep.
    pub first_sample: u64,
    /// Window length in samples; `None` = to the end of the sweep.
    pub count: Option<u64>,
    /// Samples returned per channel (statistics always cover the whole window).
    pub max_samples: u64,
    /// Window on the trace's own axis instead of `first_sample`/`count`: `[a, b]` (either
    /// order) in the axis's units (`extra.axis`: cm⁻¹, nm, ppm, retention time in its unit),
    /// or in seconds on the `start_s` clock for signals without an axis. The window is the
    /// samples whose abscissa lies in it.
    pub x_range: Option<[f64; 2]>,
}

/// Summary statistics of one channel over the requested window.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct TraceStats {
    /// Samples in the window.
    pub count: u64,
    /// Finite samples (NaN/inf are skipped by min/max/mean/std).
    pub finite: u64,
    /// Smallest finite sample.
    pub min: Option<f64>,
    /// Largest finite sample.
    pub max: Option<f64>,
    /// Mean of the finite samples.
    pub mean: Option<f64>,
    /// Population standard deviation.
    pub std: Option<f64>,
    /// Sample index of the minimum, counted from the start of the sweep (not of the window:
    /// the index within `samples` is `argmin - first_sample`).
    pub argmin: Option<u64>,
    /// Sample index of the maximum, counted from the start of the sweep (not of the window:
    /// the index within `samples` is `argmax - first_sample`).
    pub argmax: Option<u64>,
    /// Time of the minimum in seconds, on the same clock as the slice's `start_s`. Absent when
    /// the trace has no time base (a spectrum; see `argmin_axis_value`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub argmin_time_s: Option<f64>,
    /// Time of the maximum in seconds, on the same clock as the slice's `start_s` (for a
    /// chromatogram: the retention time of the tallest point, in seconds).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub argmax_time_s: Option<f64>,
    /// Position of the minimum on the slice's `axis`, in `axis.unit` (retention time in
    /// minutes, chemical shift in ppm, wavenumber, …). Present only when the slice has an `axis`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub argmin_axis_value: Option<f64>,
    /// Position of the maximum on the slice's `axis`, in `axis.unit`: `axis.first + argmax *
    /// axis.step`, or the abscissa channel's value for an irregular axis.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub argmax_axis_value: Option<f64>,
}

/// One channel of a `TraceSlice`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct TraceChannelSlice {
    /// Channel index.
    pub index: u32,
    /// Channel name.
    pub name: String,
    /// Physical unit of the samples.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Statistics over the whole window.
    pub stats: TraceStats,
    /// The first `max_samples` samples of the window, scaled to `unit`.
    pub samples: Vec<f64>,
}

/// Output of `trace`: a window of one sweep of one trace.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct TraceSlice {
    /// The input file.
    pub path: String,
    /// Format id of the input file.
    pub format: String,
    /// Trace index (see `info` → `traces[]`).
    pub trace: u32,
    /// Sweep index.
    pub sweep: u32,
    /// Sweeps in the trace.
    pub sweep_count: u32,
    /// Samples per second.
    pub sample_rate_hz: f64,
    /// Samples in this sweep.
    pub sweep_sample_count: u64,
    /// First sample of the window (zero-based, within the sweep).
    pub first_sample: u64,
    /// Samples in the window (statistics cover all of them).
    pub sample_count: u64,
    /// Time of the first window sample in seconds. Single-sweep traces use the trace's own
    /// clock: `info.traces[].start_s + first_sample / sample_rate_hz` (a chromatogram's
    /// retention time, negative when acquisition began before injection; an electrophysiology
    /// recording's clock). Multi-sweep traces are timed from the start of the sweep
    /// (`first_sample / sample_rate_hz`; `sweep_start_s` places the sweep in the recording).
    /// Irregularly sampled traces (`sample_rate_hz` 0 with a `time` channel) report that
    /// channel's first window value; traces without a time base (spectra) report 0.
    pub start_s: f64,
    /// Time of the sweep's first sample on the recording clock, seconds, when the file
    /// records it (`info.traces[].start_s` for a single sweep, `extra.sweep_starts_s[sweep]`
    /// for multi-sweep traces).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sweep_start_s: Option<f64>,
    /// One entry per requested channel.
    pub channels: Vec<TraceChannelSlice>,
    /// True when fewer samples are returned than the window holds.
    pub truncated: bool,
    /// The trace's abscissa (`info` → `traces[].extra.axis`: `{quantity, unit, first, step,
    /// …}`), e.g. the ppm axis of an NMR spectrum or a chromatogram's retention time in
    /// minutes; sample `i` of the sweep is at `first + i * step`. For a time axis this agrees
    /// with `start_s` and `sample_rate_hz` (`first` = `start_s` at `first_sample` 0, `step` =
    /// 1 / `sample_rate_hz`, in `unit`). Irregular axes (`irregular: true`) name the channel
    /// holding each sample's abscissa instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub axis: Option<serde_json::Value>,
    /// The axis window asked for (`x_range`, low to high): axis units, or seconds for signals
    /// without an axis. `first_sample`/`sample_count` are the samples inside it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x_range: Option<[f64; 2]>,
}

/// Where an extreme sits: (sample index, time in s, axis value); the last two only when the
/// trace has channels holding them (irregular sampling).
type At = (u64, Option<f64>, Option<f64>);

#[derive(Default)]
struct Acc {
    count: u64,
    finite: u64,
    sum: f64,
    sumsq: f64,
    min: Option<(f64, At)>,
    max: Option<(f64, At)>,
    shift: Option<f64>,
}

impl Acc {
    #[cfg(test)]
    fn push(&mut self, v: f64, at: u64) {
        self.push_at(v, (at, None, None));
    }

    fn push_at(&mut self, v: f64, at: At) {
        self.count += 1;
        if !v.is_finite() {
            return;
        }
        self.finite += 1;
        // shifted sums keep the variance accurate for large DC offsets
        let k = *self.shift.get_or_insert(v);
        self.sum += v - k;
        self.sumsq += (v - k) * (v - k);
        if self.min.is_none_or(|(m, _)| v < m) {
            self.min = Some((v, at));
        }
        if self.max.is_none_or(|(m, _)| v > m) {
            self.max = Some((v, at));
        }
    }
    fn stats(&self) -> TraceStats {
        let n = self.finite as f64;
        let (mean, std) = if self.finite > 0 {
            let k = self.shift.unwrap_or(0.0);
            let m = self.sum / n;
            let var = (self.sumsq / n - m * m).max(0.0);
            (Some(k + m), Some(var.sqrt()))
        } else {
            (None, None)
        };
        TraceStats {
            count: self.count,
            finite: self.finite,
            min: self.min.map(|m| m.0),
            max: self.max.map(|m| m.0),
            mean,
            std,
            argmin: self.min.map(|m| m.1.0),
            argmax: self.max.map(|m| m.1.0),
            ..TraceStats::default()
        }
    }

    /// Statistics with the extremes placed in time and on the axis.
    fn stats_on(&self, x: &Abscissa) -> TraceStats {
        let mut s = self.stats();
        let place = |e: Option<(f64, At)>| {
            e.map_or((None, None), |(_, (i, t, a))| {
                (
                    t.or_else(|| x.time_at(i)).map(tidy_s),
                    a.or_else(|| x.axis_at(i)).map(tidy_s),
                )
            })
        };
        (s.argmin_time_s, s.argmin_axis_value) = place(self.min);
        (s.argmax_time_s, s.argmax_axis_value) = place(self.max);
        s
    }
}

/// Factor to seconds of a time unit as readers write it (`s`, `min`, `ms`, JCAMP-DX
/// `SECONDS`, …); `None` for anything else.
pub fn seconds_per_unit(unit: &str) -> Option<f64> {
    match unit.trim().to_ascii_lowercase().as_str() {
        "s" | "sec" | "secs" | "second" | "seconds" => Some(1.0),
        "min" | "mins" | "minute" | "minutes" => Some(60.0),
        "ms" | "msec" | "millisecond" | "milliseconds" => Some(1e-3),
        "us" | "µs" | "microsecond" | "microseconds" => Some(1e-6),
        "h" | "hour" | "hours" => Some(3600.0),
        _ => None,
    }
}

/// Time of sample 0 of a sweep on the clock `trace` reports (`TraceSlice::start_s` at
/// `first_sample` 0 of a regularly sampled trace): the trace's `start_s` for a single-sweep
/// trace, 0 for multi-sweep traces (timed from the start of each sweep).
pub fn sweep_origin_s(t: &TraceInfo) -> f64 {
    if t.sweep_count <= 1 {
        t.start_s.filter(|s| s.is_finite()).unwrap_or(0.0)
    } else {
        0.0
    }
}

/// Time of the sweep's first sample on the recording clock, when recorded.
fn sweep_start_s(t: &TraceInfo, sweep: u32) -> Option<f64> {
    if t.sweep_count <= 1 {
        return t.start_s;
    }
    t.extra
        .get("sweep_starts_s")
        .and_then(|v| v.get(sweep as usize))
        .and_then(serde_json::Value::as_f64)
}

/// How a sample index maps to time and to the trace's `axis`.
#[derive(Debug, Default)]
struct Abscissa {
    /// Regular sampling: (time of sample 0 in s, samples per second).
    regular: Option<(f64, f64)>,
    /// Irregular sampling: (channel holding each sample's time, its factor to seconds).
    time_channel: Option<(usize, f64)>,
    /// Regular axis: (first, step).
    axis: Option<(f64, f64)>,
    /// Irregular axis: channel holding each sample's abscissa.
    axis_channel: Option<usize>,
}

impl Abscissa {
    fn of(t: &TraceInfo) -> Self {
        let mut x = Abscissa::default();
        if t.sample_rate_hz > 0.0 && t.sample_rate_hz.is_finite() {
            x.regular = Some((sweep_origin_s(t), t.sample_rate_hz));
        } else {
            x.time_channel = t
                .channels
                .iter()
                .position(|c| c.name.eq_ignore_ascii_case("time"))
                .and_then(|k| {
                    let f = t.channels[k].unit.as_deref().and_then(seconds_per_unit)?;
                    Some((k, f))
                });
        }
        if let Some(a) = t.extra.get("axis") {
            let num = |k: &str| a.get(k).and_then(serde_json::Value::as_f64);
            if a.get("irregular").and_then(serde_json::Value::as_bool) == Some(true) {
                x.axis_channel = a
                    .get("channel")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|c| usize::try_from(c).ok())
                    .filter(|&c| c < t.channels.len());
            } else if let (Some(first), Some(step)) = (num("first"), num("step")) {
                x.axis = Some((first, step));
            }
        }
        x
    }

    fn time_at(&self, i: u64) -> Option<f64> {
        self.regular.map(|(t0, rate)| t0 + i as f64 / rate)
    }

    fn axis_at(&self, i: u64) -> Option<f64> {
        self.axis.map(|(first, step)| first + i as f64 * step)
    }
}

/// Round to 12 significant digits, so `-2.38 + 802 / 2.5` does not print binary noise.
fn tidy_s(v: f64) -> f64 {
    if v == 0.0 || !v.is_finite() {
        return v;
    }
    let digits = 11 - v.abs().log10().floor() as i32;
    if !(-300..=300).contains(&digits) {
        return v;
    }
    let mag = 10f64.powi(digits);
    let r = (v * mag).round() / mag;
    if r.is_finite() { r } else { v }
}

/// Read a window of a sweep: statistics over the whole window, the first `max_samples` values.
pub fn slice_trace(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    req: &TraceRequest,
) -> Result<TraceSlice> {
    let t = info
        .traces
        .iter()
        .find(|t| t.index == req.trace)
        .ok_or_else(|| {
            if info.traces.is_empty() {
                Error::unsupported(
                    "trace",
                    format!("sampled-signal reads of a {} file", info.format.name),
                    "This file holds no traces; see `openreadout info` for what it contains.",
                )
            } else {
                Error::Usage(format!(
                    "trace {} out of range (file has {} traces)",
                    req.trace,
                    info.traces.len()
                ))
            }
        })?;
    if req.sweep >= t.sweep_count {
        return Err(Error::Usage(format!(
            "sweep {} out of range (trace {} has {} sweeps, 0..{})",
            req.sweep, req.trace, t.sweep_count, t.sweep_count
        )));
    }
    let channels: Vec<u32> = if req.channels.is_empty() {
        (0..t.channels.len() as u32).collect()
    } else {
        for &c in &req.channels {
            if c as usize >= t.channels.len() {
                return Err(Error::Usage(format!(
                    "channel {c} out of range (trace {} has {} channels)",
                    req.trace,
                    t.channels.len()
                )));
            }
        }
        req.channels.clone()
    };
    let max_samples = req.max_samples.min(MAX_SLICE_SAMPLES);
    let mut accs: Vec<Acc> = channels.iter().map(|_| Acc::default()).collect();
    let mut kept: Vec<Vec<f64>> = channels.iter().map(|_| Vec::new()).collect();
    let x = Abscissa::of(t);
    let x_range = req.x_range.map(|[lo, hi]| [lo.min(hi), lo.max(hi)]);
    let (first_sample, count) = match x_range {
        Some(window) => {
            let (start, len) = axis_window(ds, t, req, &x, window)?;
            (start, Some(len))
        }
        None => (req.first_sample, req.count),
    };
    let mut first_time: Option<f64> = None;
    let mut pos = first_sample;
    let mut remaining = count.unwrap_or(u64::MAX);
    let mut sweep_len = None;
    loop {
        let want = CHUNK.min(remaining);
        let tr = ds.read_trace(req.trace, req.sweep, pos, want)?;
        let got = tr.channels.first().map_or(0, Vec::len) as u64;
        let times = x
            .time_channel
            .and_then(|(k, f)| tr.channels.get(k).map(|col| (col, f)));
        let xs = x.axis_channel.and_then(|k| tr.channels.get(k));
        if first_time.is_none() {
            first_time = times.and_then(|(col, f)| col.first().map(|v| v * f));
        }
        for (k, &c) in channels.iter().enumerate() {
            let col = tr
                .channels
                .get(c as usize)
                .ok_or_else(|| Error::Other(format!("reader returned no channel {c}")))?;
            if times.is_none() && xs.is_none() {
                // regular sampling: extremes are placed from their index afterwards
                for (i, &v) in col.iter().enumerate() {
                    accs[k].push_at(v, (pos + i as u64, None, None));
                }
            } else {
                for (i, &v) in col.iter().enumerate() {
                    let tm = times.and_then(|(tc, f)| tc.get(i).map(|t| t * f));
                    let ax = xs.and_then(|a| a.get(i).copied());
                    accs[k].push_at(v, (pos + i as u64, tm, ax));
                }
            }
            let room = max_samples.saturating_sub(kept[k].len() as u64) as usize;
            kept[k].extend(col.iter().take(room));
        }
        pos += got;
        remaining = remaining.saturating_sub(got);
        if got < want || remaining == 0 || got == 0 {
            if got < want {
                sweep_len = Some(pos);
            }
            break;
        }
    }
    let sample_count = pos - first_sample;
    let sweep_sample_count = sweep_len.unwrap_or_else(|| sweep_samples(t, req.sweep));
    let rate = t.sample_rate_hz;
    Ok(TraceSlice {
        path: info.path.clone(),
        format: info.format.id.clone(),
        trace: req.trace,
        sweep: req.sweep,
        sweep_count: t.sweep_count,
        sample_rate_hz: rate,
        sweep_sample_count,
        first_sample,
        sample_count,
        start_s: x.time_at(first_sample).or(first_time).map_or(0.0, tidy_s),
        sweep_start_s: sweep_start_s(t, req.sweep),
        truncated: kept.iter().any(|k| (k.len() as u64) < sample_count),
        axis: t.extra.get("axis").cloned(),
        x_range,
        channels: channels
            .iter()
            .zip(accs.iter().zip(kept))
            .map(|(&c, (a, samples))| {
                let ch = &t.channels[c as usize];
                TraceChannelSlice {
                    index: c,
                    name: ch.name.clone(),
                    unit: ch.unit.clone(),
                    stats: a.stats_on(&x),
                    samples,
                }
            })
            .collect(),
    })
}

/// The samples `(first, count)` of a sweep whose abscissa lies in `[lo, hi]`: from the axis's
/// `first`/`step` or, for irregular axes and time channels, by reading that channel; for
/// signals without an axis, seconds on the `start_s` clock.
#[allow(clippy::many_single_char_names, clippy::float_cmp)] // lo == hi: an empty window as given
fn axis_window(
    ds: &mut dyn Dataset,
    t: &TraceInfo,
    req: &TraceRequest,
    x: &Abscissa,
    [lo, hi]: [f64; 2],
) -> Result<(u64, u64)> {
    if !(lo.is_finite() && hi.is_finite()) || lo == hi {
        return Err(Error::Usage(format!(
            "x range {lo}:{hi}: expected two different finite axis values"
        )));
    }
    let n = sweep_samples(t, req.sweep);
    let none = || {
        Error::Usage(format!(
            "x range {lo}:{hi} holds no sample of trace {} (see `info` → traces[].extra.axis)",
            req.trace
        ))
    };
    if n == 0 {
        return Err(none());
    }
    // regular abscissa: the axis's first/step, else the time base (seconds)
    let regular = x
        .axis
        .or_else(|| {
            (x.axis_channel.is_none() && x.time_channel.is_none())
                .then(|| x.regular.map(|(t0, rate)| (t0, 1.0 / rate)))
                .flatten()
        })
        .filter(|(f, s)| f.is_finite() && s.is_finite() && *s != 0.0);
    if let Some((first, step)) = regular {
        let (ia, ib) = ((lo - first) / step, (hi - first) / step);
        let a = (ia.min(ib) - 1e-9).ceil().max(0.0);
        let b = (ia.max(ib) + 1e-9).floor().min((n - 1) as f64);
        if !(a.is_finite() && b.is_finite()) || a > b {
            return Err(none());
        }
        return Ok((a as u64, b as u64 - a as u64 + 1));
    }
    let (col, factor) = match (x.axis_channel, x.time_channel) {
        (Some(k), _) => (k, 1.0),
        (None, Some((k, f))) => (k, f),
        (None, None) => {
            return Err(Error::unsupported(
                "trace",
                format!(
                    "an axis window on trace {}: it has no axis and no time base",
                    req.trace
                ),
                "Use --first-sample/--count (sample indices).",
            ));
        }
    };
    let (mut first, mut last) = (None::<u64>, None::<u64>);
    let mut pos = 0u64;
    while pos < n {
        let want = CHUNK.min(n - pos);
        let tr = ds.read_trace(req.trace, req.sweep, pos, want)?;
        let Some(c) = tr.channels.get(col) else { break };
        for (i, v) in c.iter().enumerate() {
            let v = v * factor;
            if v >= lo && v <= hi {
                let k = pos + i as u64;
                first.get_or_insert(k);
                last = Some(k);
            }
        }
        let got = c.len() as u64;
        pos += got;
        if got < want || got == 0 {
            break;
        }
    }
    match (first, last) {
        (Some(a), Some(b)) => Ok((a, b - a + 1)),
        _ => Err(none()),
    }
}

/// Samples in one sweep: `extra.sweep_sample_counts[sweep]` when a reader records per-sweep
/// lengths (sweeps of different length), else the trace's `sample_count`.
pub fn sweep_samples(t: &TraceInfo, sweep: u32) -> u64 {
    t.extra
        .get("sweep_sample_counts")
        .and_then(|v| v.get(sweep as usize))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(t.sample_count)
}

/// Samples `[first, first + count)` of sweep `sweep` of trace `trace`, channels `channels` only
/// (in that order), read page by page: readers cap one `read_trace` (per channel and per value
/// count), so a single call may return fewer samples than asked. Stops early only when the
/// reader returns no more samples.
pub fn read_channels(
    ds: &mut dyn Dataset,
    trace: u32,
    sweep: u32,
    first: u64,
    count: u64,
    channels: &[u32],
) -> Result<Vec<Vec<f64>>> {
    let mut out: Vec<Vec<f64>> = channels.iter().map(|_| Vec::new()).collect();
    let mut pos = first;
    let end = first.saturating_add(count);
    while pos < end {
        let tr = ds.read_trace(trace, sweep, pos, end - pos)?;
        let got = tr.channels.first().map_or(0, Vec::len) as u64;
        if got == 0 {
            break;
        }
        for (k, &c) in channels.iter().enumerate() {
            let col = tr
                .channels
                .get(c as usize)
                .ok_or_else(|| Error::Other(format!("reader returned no channel {c}")))?;
            out[k].extend_from_slice(col);
        }
        pos += got;
    }
    Ok(out)
}

#[cfg(test)]
#[allow(clippy::float_cmp, clippy::field_reassign_with_default)]
mod tests {
    use super::*;

    #[test]
    fn stats_are_stable() {
        let mut a = Acc::default();
        for (i, v) in [1e9 + 1.0, 1e9 + 2.0, 1e9 + 3.0, f64::NAN]
            .iter()
            .enumerate()
        {
            a.push(*v, i as u64);
        }
        let s = a.stats();
        assert_eq!(s.count, 4);
        assert_eq!(s.finite, 3);
        assert!((s.mean.unwrap() - (1e9 + 2.0)).abs() < 1e-6);
        assert!((s.std.unwrap() - (2.0f64 / 3.0).sqrt()).abs() < 1e-9);
        assert_eq!(s.argmin, Some(0));
        assert_eq!(s.argmax, Some(2));
    }

    use crate::model::{CheckReport, FileInfo, FormatDescriptor, LsEntry, SignalChannelInfo};
    use crate::provenance::{Confidence, ProvenanceMap};
    use crate::{Plane, PlaneIndex, Trace};
    use serde_json::json;

    /// One trace whose channels are the given columns (all sweeps identical).
    struct Fake(Vec<Vec<f64>>);
    impl Dataset for Fake {
        fn info(&self) -> Result<FileInfo> {
            Err(Error::Other("unused".into()))
        }
        fn vendor_metadata(&self) -> Result<serde_json::Value> {
            Ok(serde_json::Value::Null)
        }
        fn provenance(&self) -> ProvenanceMap {
            ProvenanceMap::new()
        }
        fn entries(&self) -> Result<Vec<LsEntry>> {
            Ok(vec![])
        }
        fn read_plane(&mut self, _: u32, _: PlaneIndex) -> Result<Plane> {
            Err(Error::Other("unused".into()))
        }
        fn check(&mut self) -> Result<CheckReport> {
            Ok(CheckReport::new("fake", "fake"))
        }
        fn read_trace(&mut self, trace: u32, sweep: u32, first: u64, n: u64) -> Result<Trace> {
            let (a, b) = (first as usize, first.saturating_add(n) as usize);
            Ok(Trace {
                trace,
                sweep,
                first_sample: first,
                channels: self
                    .0
                    .iter()
                    .map(|c| c[a.min(c.len())..b.min(c.len())].to_vec())
                    .collect(),
            })
        }
    }

    fn file(t: TraceInfo) -> FileInfo {
        FileInfo {
            path: "x".into(),
            size_bytes: 0,
            format: FormatDescriptor {
                id: "fake".into(),
                name: "Fake".into(),
                vendor: String::new(),
                extensions: vec![],
                family: "chromatography".into(),
                can_read: true,
                can_write: false,
                confidence: Confidence::Low,
                known_gaps: vec![],
            },
            format_version: None,
            images: vec![],
            tables: vec![],
            spectra: vec![],
            traces: vec![t],
            plane_count: 0,
            notes: vec![],
        }
    }

    fn channel(i: u32, name: &str, unit: &str) -> SignalChannelInfo {
        SignalChannelInfo {
            index: i,
            name: name.into(),
            unit: Some(unit.into()),
            dtype: "float64".into(),
            scale: 1.0,
            offset: 0.0,
            extra: std::collections::BTreeMap::new(),
        }
    }

    fn slice(ds: &mut Fake, info: &FileInfo, first: u64, count: Option<u64>) -> TraceSlice {
        let mut r = TraceRequest::default();
        r.first_sample = first;
        r.count = count;
        r.max_samples = 10;
        slice_trace(ds, info, &r).unwrap()
    }

    /// A chromatogram acquired from −2.38 s (ChemStation MWD): `start_s` and the extremes are
    /// on the retention-time clock, not counted from the first sample.
    #[test]
    fn chromatogram_times_include_the_trace_start() {
        let y: Vec<f64> = (0..100).map(|i| if i == 40 { 9.0 } else { 0.0 }).collect();
        let mut t = TraceInfo {
            sample_rate_hz: 2.5,
            sample_count: 100,
            sweep_count: 1,
            channels: vec![channel(0, "mwd1B", "mAU")],
            start_s: Some(-2.38),
            ..TraceInfo::default()
        };
        t.extra.insert(
            "axis".into(),
            json!({"quantity": "retention_time", "unit": "min", "first": -2.38 / 60.0, "step": 0.4 / 60.0}),
        );
        let info = file(t);
        let mut ds = Fake(vec![y]);
        let s = slice(&mut ds, &info, 0, None);
        assert_eq!(s.start_s, -2.38);
        assert_eq!(s.sweep_start_s, Some(-2.38));
        let st = &s.channels[0].stats;
        assert_eq!(st.argmax, Some(40));
        assert_eq!(st.argmax_time_s, Some(13.62));
        let rt = st.argmax_axis_value.unwrap();
        assert!((rt - (13.62 / 60.0)).abs() < 1e-12, "{rt}");
        // a window: argmax stays a sweep index, the times stay on the same clock
        let s = slice(&mut ds, &info, 30, Some(20));
        assert_eq!(s.start_s, 9.62);
        assert_eq!(s.channels[0].stats.argmax, Some(40));
        assert_eq!(s.channels[0].stats.argmax_time_s, Some(13.62));
    }

    /// Episodic sweeps are timed from their own start; `sweep_start_s` places them.
    #[test]
    fn multi_sweep_traces_are_sweep_relative() {
        let mut t = TraceInfo {
            sample_rate_hz: 10.0,
            sample_count: 10,
            sweep_count: 3,
            channels: vec![channel(0, "IN 0", "mV")],
            start_s: Some(0.0),
            ..TraceInfo::default()
        };
        t.extra
            .insert("sweep_starts_s".into(), json!([0.0, 5.0, 10.0]));
        let info = file(t);
        let mut ds = Fake(vec![(0..10).map(f64::from).collect()]);
        let mut r = TraceRequest::default();
        r.sweep = 2;
        r.first_sample = 5;
        let s = slice_trace(&mut ds, &info, &r).unwrap();
        assert_eq!(s.start_s, 0.5);
        assert_eq!(s.sweep_start_s, Some(10.0));
        assert_eq!(s.channels[0].stats.argmax_time_s, Some(0.9));
        assert_eq!(s.channels[0].stats.argmax_axis_value, None);
    }

    /// Irregular sampling: times come from the `time` channel (minutes converted to seconds).
    #[test]
    fn irregular_traces_use_the_time_channel() {
        let t = TraceInfo {
            sample_rate_hz: 0.0,
            sample_count: 4,
            sweep_count: 1,
            channels: vec![channel(0, "time", "min"), channel(1, "intensity", "counts")],
            start_s: Some(6.0),
            ..TraceInfo::default()
        };
        let info = file(t);
        let mut ds = Fake(vec![vec![0.1, 0.2, 0.35, 0.5], vec![1.0, 7.0, 3.0, 0.5]]);
        let s = slice(&mut ds, &info, 1, None);
        assert_eq!(s.start_s, 12.0);
        let st = &s.channels[1].stats;
        assert_eq!(st.argmax, Some(1));
        assert_eq!(st.argmax_time_s, Some(12.0));
        assert_eq!(st.argmin_time_s, Some(30.0));
    }

    /// A spectrum has no time base: `start_s` 0, extremes placed on the ppm axis only.
    #[test]
    fn spectra_place_extremes_on_the_axis() {
        let mut t = TraceInfo {
            sample_count: 5,
            sweep_count: 1,
            channels: vec![channel(0, "real", "")],
            ..TraceInfo::default()
        };
        t.extra.insert(
            "axis".into(),
            json!({"quantity": "chemical_shift", "unit": "ppm", "first": 10.0, "step": -0.5}),
        );
        let info = file(t);
        let mut ds = Fake(vec![vec![0.0, 1.0, 5.0, 2.0, -1.0]]);
        let s = slice(&mut ds, &info, 0, None);
        assert_eq!(s.start_s, 0.0);
        let st = &s.channels[0].stats;
        assert_eq!(st.argmax_time_s, None);
        assert_eq!(st.argmax_axis_value, Some(9.0));
        assert_eq!(st.argmin_axis_value, Some(8.0));
    }

    fn window(ds: &mut Fake, info: &FileInfo, a: f64, b: f64) -> Result<TraceSlice> {
        let mut r = TraceRequest::default();
        r.x_range = Some([a, b]);
        r.max_samples = 10;
        slice_trace(ds, info, &r)
    }

    /// `x_range` on a descending ppm axis, in either order; on a time base (seconds); through
    /// an irregular time channel; and outside the data.
    #[test]
    fn axis_windows() {
        let mut t = TraceInfo {
            sample_count: 5,
            sweep_count: 1,
            channels: vec![channel(0, "real", "")],
            ..TraceInfo::default()
        };
        t.extra.insert(
            "axis".into(),
            json!({"quantity": "chemical_shift", "unit": "ppm", "first": 10.0, "step": -0.5}),
        );
        let info = file(t);
        let mut ds = Fake(vec![vec![0.0, 1.0, 5.0, 2.0, -1.0]]);
        for (a, b) in [(8.1, 9.5), (9.5, 8.1)] {
            let s = window(&mut ds, &info, a, b).unwrap();
            assert_eq!((s.first_sample, s.sample_count), (1, 3));
            assert_eq!(s.channels[0].samples, vec![1.0, 5.0, 2.0]);
            assert_eq!(s.x_range, Some([8.1, 9.5]));
        }
        // ends exactly on samples are inside
        let s = window(&mut ds, &info, 8.0, 9.0).unwrap();
        assert_eq!((s.first_sample, s.sample_count), (2, 3));
        assert!(window(&mut ds, &info, 20.0, 30.0).is_err());
        assert!(window(&mut ds, &info, 1.0, 1.0).is_err());
        // a time base: seconds on the start_s clock
        let t = TraceInfo {
            sample_rate_hz: 10.0,
            sample_count: 100,
            sweep_count: 1,
            channels: vec![channel(0, "IN 0", "mV")],
            start_s: Some(1.0),
            ..TraceInfo::default()
        };
        let info = file(t);
        let mut ds = Fake(vec![(0..100).map(f64::from).collect()]);
        let s = window(&mut ds, &info, 2.0, 3.0).unwrap();
        assert_eq!((s.first_sample, s.sample_count), (10, 11));
        assert_eq!(s.start_s, 2.0);
        // an irregular time channel in minutes
        let t = TraceInfo {
            sample_count: 4,
            sweep_count: 1,
            channels: vec![channel(0, "time", "min"), channel(1, "intensity", "counts")],
            ..TraceInfo::default()
        };
        let info = file(t);
        let mut ds = Fake(vec![vec![0.1, 0.2, 0.35, 0.5], vec![1.0, 7.0, 3.0, 0.5]]);
        let s = window(&mut ds, &info, 10.0, 25.0).unwrap();
        assert_eq!((s.first_sample, s.sample_count), (1, 2));
    }

    #[test]
    fn time_units() {
        assert_eq!(seconds_per_unit("SECONDS"), Some(1.0));
        assert_eq!(seconds_per_unit("min"), Some(60.0));
        assert_eq!(seconds_per_unit("ms"), Some(1e-3));
        assert_eq!(seconds_per_unit("ppm"), None);
        assert_eq!(tidy_s(-2.38 + 802.0 / 2.5), 318.42);
    }
}
