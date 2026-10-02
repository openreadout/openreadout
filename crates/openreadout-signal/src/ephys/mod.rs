//! Electrophysiology analysis.
//!
//! - [`analyze_cell`] (`openreadout analyze ephys-features`): patch-clamp recordings. Current
//!   clamp: action potentials and their features per sweep ([`ap`]), firing rate, latency, ISIs and
//!   adaptation during the stimulus, passive properties from hyperpolarising steps ([`passive`]),
//!   and per cell the rheobase, f–I curve and slope, input resistance, time constant, capacitance
//!   and sag. Voltage clamp: holding current and test-pulse metrics (access and membrane
//!   resistance, capacitance). The stimulus of each sweep comes from the protocol's epoch table
//!   ([`protocol`], ABF).
//! - [`analyze_extracellular`] (`openreadout analyze spikes`): band-pass filtering and threshold-
//!   crossing detection per channel ([`extracellular`]).
//!
//! Notes and vocabulary: `docs/formats/ephys-analysis.md`.

pub mod ap;
pub mod extracellular;
pub mod filter;
pub mod passive;
pub mod protocol;

use serde::{Deserialize, Serialize};

use openreadout_core::model::{FileInfo, TraceInfo};
use openreadout_core::{Dataset, Error, Result};

pub use ap::{ApSettings, Spike};
pub use extracellular::{DetectSettings, PeakSign};
pub use passive::{StepResponse, TestPulse};
pub use protocol::{Protocol, SweepEpoch};

/// Factor from `unit` to millivolts, if it is a voltage.
pub fn to_mv(unit: Option<&str>) -> Option<f64> {
    match unit?.trim() {
        "mV" => Some(1.0),
        "V" => Some(1000.0),
        "uV" | "µV" | "μV" => Some(1e-3),
        _ => None,
    }
}

/// Factor from `unit` to picoamperes, if it is a current.
pub fn to_pa(unit: Option<&str>) -> Option<f64> {
    match unit?.trim() {
        "pA" => Some(1.0),
        "nA" => Some(1e3),
        "uA" | "µA" | "μA" => Some(1e6),
        "mA" => Some(1e9),
        "A" => Some(1e12),
        "fA" => Some(1e-3),
        _ => None,
    }
}

/// A note when samples are far outside what a cell can produce in the channel's stated unit
/// (beyond 10 V for a membrane voltage, 1 µA for a clamp current): the file's unit label is
/// then probably wrong. `peak` is the largest |sample| in mV or pA, `scale` the factor that
/// converted the stated unit to them.
fn implausible_unit(
    name: Option<&str>,
    unit: Option<&str>,
    voltage: bool,
    peak: f64,
    scale: f64,
) -> Option<String> {
    let (limit, base, what, typical) = if voltage {
        (1e4, "mV", "membrane voltages", "mV")
    } else {
        (1e6, "pA", "clamp currents", "pA to nA")
    };
    if !(peak > limit) || scale <= 0.0 {
        return None;
    }
    let stated = peak / scale;
    Some(format!(
        "channel {:?} is labelled {:?} but reaches {} {} ({peak:.3e} {base}); {what} are {typical}, so the unit label is probably wrong (as numbers in {base} the samples would reach {} {base}). The features below take the label literally; check the recording's units before using them",
        name.unwrap_or("-"),
        unit.unwrap_or("-"),
        crate::ephys::fmt_g(stated),
        unit.unwrap_or("-"),
        crate::ephys::fmt_g(stated),
    ))
}

/// A number with 4 significant digits, without exponent noise for everyday magnitudes.
fn fmt_g(v: f64) -> String {
    if v != 0.0 && (v.abs() >= 1e6 || v.abs() < 1e-3) {
        format!("{v:.3e}")
    } else {
        let s = format!("{v:.4}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// What [`analyze_cell`] should do.
#[derive(Debug, Clone)]
pub struct CellRequest {
    /// Trace (default 0).
    pub trace: u32,
    /// Channel (default: the first voltage channel, else the first current channel).
    pub channel: Option<u32>,
    /// Sweeps (default all).
    pub sweeps: Option<Vec<u32>>,
    /// Spike detection settings.
    pub ap: ApSettings,
    /// Keep at most this many spikes in the per-spike table (counts are always complete).
    pub max_spikes: usize,
}

impl Default for CellRequest {
    fn default() -> Self {
        Self {
            trace: 0,
            channel: None,
            sweeps: None,
            ap: ApSettings::default(),
            max_spikes: 10_000,
        }
    }
}

/// The channel analysed.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ChannelRef {
    /// Channel index.
    pub index: u32,
    /// Name.
    pub name: String,
    /// Unit as recorded.
    pub unit: Option<String>,
}

/// The stimulus found in the protocol.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct StimulusInfo {
    /// Command output name.
    pub output: String,
    /// Command unit.
    pub unit: Option<String>,
    /// Holding level, in `unit`.
    pub holding: f64,
    /// Epoch index used as the stimulus.
    pub epoch: Option<u32>,
}

/// One sweep.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SweepFeatures {
    /// Sweep index.
    pub sweep: u32,
    /// Stimulus start, s from the sweep start.
    pub stimulus_start_s: Option<f64>,
    /// Stimulus end, s.
    pub stimulus_end_s: Option<f64>,
    /// Injected current relative to holding, pA (current clamp).
    pub stimulus_pa: Option<f64>,
    /// Command step relative to holding, mV (voltage clamp).
    pub stimulus_mv: Option<f64>,
    /// Spikes in the whole sweep.
    pub spike_count: usize,
    /// Spikes whose peak falls inside the stimulus.
    pub spike_count_stimulus: Option<usize>,
    /// Spikes in the stimulus / stimulus duration, Hz.
    pub firing_rate_hz: Option<f64>,
    /// First spike peak after stimulus onset, ms.
    pub first_spike_latency_ms: Option<f64>,
    /// Mean inter-spike interval inside the stimulus, ms.
    pub isi_mean_ms: Option<f64>,
    /// ISI coefficient of variation inside the stimulus.
    pub isi_cv: Option<f64>,
    /// ISI adaptation index inside the stimulus.
    pub adaptation_index: Option<f64>,
    /// Mean spike half-width, ms.
    pub half_width_mean_ms: Option<f64>,
    /// Mean spike amplitude (peak − onset), mV.
    pub amplitude_mean_mv: Option<f64>,
    /// Passive response (current clamp, spike-free sweeps; baseline always).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passive: Option<StepResponse>,
    /// Test-pulse metrics (voltage clamp).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_pulse: Option<TestPulse>,
}

/// One point of the f–I curve.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct FiPoint {
    /// Sweep.
    pub sweep: u32,
    /// Injected current relative to holding, pA.
    pub current_pa: f64,
    /// Firing rate during the stimulus, Hz.
    pub rate_hz: f64,
    /// Spikes during the stimulus.
    pub spike_count: usize,
}

/// Per-cell summary.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CellFeatures {
    /// Median baseline (pre-stimulus) voltage over sweeps, mV.
    pub resting_mv: Option<f64>,
    /// Smallest depolarising step that evokes at least one spike during the stimulus, pA.
    pub rheobase_pa: Option<f64>,
    /// The sweep of that step.
    pub rheobase_sweep: Option<u32>,
    /// f–I curve over depolarising steps.
    pub fi_curve: Vec<FiPoint>,
    /// Least-squares slope of rate vs current over steps that fire, Hz/pA.
    pub fi_slope_hz_per_pa: Option<f64>,
    /// Highest firing rate, Hz.
    pub max_firing_rate_hz: Option<f64>,
    /// Slope of steady-state deflection vs current over spike-free hyperpolarising steps, MΩ.
    pub input_resistance_mohm: Option<f64>,
    /// Median time constant over hyperpolarising steps, ms.
    pub tau_ms: Option<f64>,
    /// τ / R_in, pF.
    pub capacitance_pf: Option<f64>,
    /// Sag ratio of the most hyperpolarising step.
    pub sag_ratio: Option<f64>,
    /// Median holding current, pA (voltage clamp).
    pub holding_current_pa: Option<f64>,
    /// Median access resistance, MΩ (voltage clamp).
    pub access_resistance_mohm: Option<f64>,
    /// Median membrane resistance, MΩ (voltage clamp).
    pub membrane_resistance_mohm: Option<f64>,
    /// Median membrane capacitance from the test pulse, pF (voltage clamp).
    pub membrane_capacitance_pf: Option<f64>,
}

/// Output of `analyze ephys-features`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CellReport {
    /// Input path.
    pub path: String,
    /// Format id.
    pub format: String,
    /// Trace analysed.
    pub trace: u32,
    /// Channel analysed.
    pub channel: ChannelRef,
    /// `current_clamp`, `voltage_clamp` or `unknown` (from the channel and command units).
    pub clamp_mode: String,
    /// Sampling rate, Hz.
    pub sample_rate_hz: f64,
    /// Spike detection settings.
    pub settings: ApSettings,
    /// Stimulus from the protocol, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stimulus: Option<StimulusInfo>,
    /// Per-cell features.
    pub cell: CellFeatures,
    /// One row per sweep.
    pub sweeps: Vec<SweepFeatures>,
    /// Total spikes detected.
    pub spike_count_total: usize,
    /// One row per spike (at most `max_spikes`).
    pub spikes: Vec<Spike>,
    /// True when `spikes` was cut at `max_spikes`.
    pub spikes_truncated: bool,
    /// Remarks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

fn median(mut v: Vec<f64>) -> Option<f64> {
    v.retain(|x| x.is_finite());
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    let n = v.len();
    Some(if n % 2 == 1 {
        v[n / 2]
    } else {
        f64::midpoint(v[n / 2 - 1], v[n / 2])
    })
}

fn slope(points: &[(f64, f64)]) -> Option<f64> {
    if points.len() < 2 {
        return None;
    }
    let n = points.len() as f64;
    let mx = points.iter().map(|p| p.0).sum::<f64>() / n;
    let my = points.iter().map(|p| p.1).sum::<f64>() / n;
    let sxx: f64 = points.iter().map(|p| (p.0 - mx).powi(2)).sum();
    let sxy: f64 = points.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum();
    (sxx > 0.0).then(|| sxy / sxx)
}

fn trace_of(info: &FileInfo, index: u32) -> Result<&TraceInfo> {
    info.traces.iter().find(|t| t.index == index).ok_or_else(|| {
        if info.traces.is_empty() {
            Error::unsupported(
                "ephys-analysis",
                format!("electrophysiology analysis of a {} file", info.format.name),
                "This file holds no sampled traces; `analyze ephys-features` and `analyze spikes` need an electrophysiology recording (ABF, ATF, NWB, Neuralynx, Blackrock, SpikeGLX, Intan, Plexon).",
            )
        } else {
            Error::Usage(format!(
                "trace {index} out of range (file has {} traces)",
                info.traces.len()
            ))
        }
    })
}

fn read_channel(
    ds: &mut dyn Dataset,
    t: &TraceInfo,
    sweep: u32,
    channel: u32,
    scale: f64,
) -> Result<Vec<f64>> {
    let n = openreadout_core::trace::sweep_samples(t, sweep);
    let col = openreadout_core::trace::read_channels(ds, t.index, sweep, 0, n, &[channel])?
        .pop()
        .unwrap_or_default();
    Ok(col.into_iter().map(|v| v * scale).collect())
}

/// Patch-clamp features of one trace (module docs).
pub fn analyze_cell(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    req: &CellRequest,
) -> Result<CellReport> {
    let t = trace_of(info, req.trace)?.clone();
    let fs = t.sample_rate_hz;
    if !(fs > 0.0) {
        return Err(Error::unsupported(
            "ephys-analysis",
            "a trace without a sampling rate (a spectrum or table)",
            "Pick a time-series trace (`openreadout info` → traces[].sample_rate_hz > 0).",
        ));
    }
    let chan = match req.channel {
        Some(c) => {
            if c as usize >= t.channels.len() {
                return Err(Error::Usage(format!(
                    "channel {c} out of range (trace {} has {} channels)",
                    t.index,
                    t.channels.len()
                )));
            }
            c
        }
        None => t
            .channels
            .iter()
            .position(|c| to_mv(c.unit.as_deref()).is_some())
            .or_else(|| {
                t.channels
                    .iter()
                    .position(|c| to_pa(c.unit.as_deref()).is_some())
            })
            .ok_or_else(|| {
                Error::unsupported(
                    "ephys-analysis",
                    "a trace with no voltage or current channel",
                    "Name the channel with `--channel N`; its unit must be a voltage (mV, V) or a current (pA, nA).",
                )
            })? as u32,
    };
    let ch = &t.channels[chan as usize];
    let unit = ch.unit.as_deref();
    let protocol = Protocol::from_trace(&t);
    let cmd_pa = protocol.as_ref().and_then(|p| to_pa(p.unit.as_deref()));
    let cmd_mv = protocol.as_ref().and_then(|p| to_mv(p.unit.as_deref()));
    let (mode, scale) = if let Some(k) = to_mv(unit) {
        (
            if cmd_mv.is_some() {
                "unknown"
            } else {
                "current_clamp"
            },
            k,
        )
    } else if let Some(k) = to_pa(unit) {
        (
            if cmd_pa.is_some() {
                "unknown"
            } else {
                "voltage_clamp"
            },
            k,
        )
    } else {
        return Err(Error::unsupported(
            "ephys-analysis",
            format!("channel {chan} in unit {:?}", ch.unit),
            "`analyze ephys-features` analyses a membrane voltage (mV) or a clamp current (pA) channel; pick one with `--channel`.",
        ));
    };
    let mut notes = Vec::new();
    if mode == "unknown" {
        notes.push("the recorded channel and the command output have the same kind of unit; clamp mode unknown, analysed as recorded".into());
    }
    let voltage = to_mv(unit).is_some();
    if protocol.is_none() {
        notes
            .push("no stimulus protocol (epoch table) found: sweep and spike features only".into());
    } else if protocol.as_ref().and_then(|p| p.stimulus(0)).is_none() {
        notes.push(
            "the protocol has no step epoch that changes the command: no stimulus window".into(),
        );
    }
    let sweeps: Vec<u32> = match &req.sweeps {
        Some(s) => {
            for &k in s {
                if k >= t.sweep_count {
                    return Err(Error::Usage(format!(
                        "sweep {k} out of range (trace {} has {} sweeps)",
                        t.index, t.sweep_count
                    )));
                }
            }
            s.clone()
        }
        None => (0..t.sweep_count).collect(),
    };
    let mut rows = Vec::new();
    let mut spikes = Vec::new();
    let mut total = 0usize;
    // largest |sample| in mV or pA, for the unit plausibility check
    let mut peak = 0f64;
    for &sw in &sweeps {
        let y = read_channel(ds, &t, sw, chan, scale)?;
        peak = y
            .iter()
            .filter(|v| v.is_finite())
            .fold(peak, |m, v| m.max(v.abs()));
        let stim = protocol.as_ref().and_then(|p| p.stimulus(sw));
        let (s0, s1) = stim
            .as_ref()
            .map_or((0, 0), |e| (e.start_sample as usize, e.end_sample as usize));
        let holding = protocol.as_ref().map_or(0.0, |p| p.holding);
        let delta = stim.as_ref().map(|e| e.level - holding);
        let mut row = SweepFeatures {
            sweep: sw,
            stimulus_start_s: stim.as_ref().map(|_| s0 as f64 / fs),
            stimulus_end_s: stim.as_ref().map(|_| s1 as f64 / fs),
            ..SweepFeatures::default()
        };
        if voltage {
            row.stimulus_pa = delta.and_then(|d| cmd_pa.map(|k| d * k));
            let sp = ap::detect(&y, fs, 0, y.len(), sw, &req.ap);
            row.spike_count = sp.len();
            if !sp.is_empty() {
                row.half_width_mean_ms =
                    median_mean(sp.iter().filter_map(|s| s.half_width_ms).collect());
                row.amplitude_mean_mv = median_mean(sp.iter().map(|s| s.amplitude_mv).collect());
            }
            if stim.is_some() {
                let inside: Vec<&Spike> = sp
                    .iter()
                    .filter(|s| (s.peak_sample as usize) >= s0 && (s.peak_sample as usize) < s1)
                    .collect();
                let dur = (s1 - s0) as f64 / fs;
                row.spike_count_stimulus = Some(inside.len());
                row.firing_rate_hz = (dur > 0.0).then(|| inside.len() as f64 / dur);
                row.first_spike_latency_ms = inside
                    .first()
                    .map(|s| (s.peak_sample as f64 - s0 as f64) / fs * 1000.0);
                let isis: Vec<f64> = inside
                    .windows(2)
                    .map(|w| (w[1].peak_sample as f64 - w[0].peak_sample as f64) / fs * 1000.0)
                    .collect();
                if !isis.is_empty() {
                    let m = isis.iter().sum::<f64>() / isis.len() as f64;
                    row.isi_mean_ms = Some(m);
                    if isis.len() >= 2 && m > 0.0 {
                        let var = isis.iter().map(|x| (x - m).powi(2)).sum::<f64>()
                            / (isis.len() - 1) as f64;
                        row.isi_cv = Some(var.sqrt() / m);
                    }
                    row.adaptation_index = ap::adaptation_index(&isis);
                }
                if let Some(d) = row.stimulus_pa {
                    let mut pr = passive::current_step(&y, fs, s0, s1, d);
                    if !inside.is_empty() {
                        // spikes contaminate the passive measures
                        pr.tau_ms = None;
                        pr.sag_ratio = None;
                        pr.steady_fraction = None;
                        pr.input_resistance_mohm = None;
                        pr.extreme_mv = None;
                    }
                    row.passive = Some(pr);
                }
            } else {
                let base_end = (y.len() / 64).max(1).min(y.len());
                row.passive = Some(StepResponse {
                    baseline_mv: mean(&y[..base_end]),
                    ..StepResponse::default()
                });
            }
            total += sp.len();
            for s in sp {
                if spikes.len() < req.max_spikes {
                    spikes.push(s);
                }
            }
        } else {
            row.stimulus_mv = delta.and_then(|d| cmd_mv.map(|k| d * k));
            match (stim.as_ref(), row.stimulus_mv) {
                (Some(_), Some(dv)) if dv != 0.0 => {
                    row.test_pulse = Some(passive::test_pulse(&y, fs, s0, s1, dv));
                }
                _ => {
                    let base_end = (y.len() / 64).max(1).min(y.len());
                    row.test_pulse = Some(TestPulse {
                        holding_current_pa: mean(&y[..base_end]),
                        ..TestPulse::default()
                    });
                }
            }
        }
        rows.push(row);
    }
    if let Some(n) = implausible_unit(Some(ch.name.as_str()), unit, voltage, peak, scale) {
        notes.push(n);
    }
    let cell = summarize(&rows, voltage);
    Ok(CellReport {
        path: info.path.clone(),
        format: info.format.id.clone(),
        trace: t.index,
        channel: ChannelRef {
            index: chan,
            name: ch.name.clone(),
            unit: ch.unit.clone(),
        },
        clamp_mode: mode.into(),
        sample_rate_hz: fs,
        settings: req.ap,
        stimulus: protocol.as_ref().map(|p| StimulusInfo {
            output: p.output.clone(),
            unit: p.unit.clone(),
            holding: p.holding,
            epoch: p.stimulus(0).map(|e| e.index),
        }),
        cell,
        sweeps: rows,
        spike_count_total: total,
        spikes_truncated: total > spikes.len(),
        spikes,
        notes,
    })
}

fn mean(v: &[f64]) -> Option<f64> {
    let f: Vec<f64> = v.iter().copied().filter(|x| x.is_finite()).collect();
    (!f.is_empty()).then(|| f.iter().sum::<f64>() / f.len() as f64)
}

fn median_mean(v: Vec<f64>) -> Option<f64> {
    mean(&v)
}

fn summarize(rows: &[SweepFeatures], voltage: bool) -> CellFeatures {
    let mut c = CellFeatures::default();
    if !voltage {
        let tp = |f: fn(&TestPulse) -> Option<f64>| {
            median(
                rows.iter()
                    .filter_map(|r| r.test_pulse.as_ref().and_then(f))
                    .collect(),
            )
        };
        c.holding_current_pa = tp(|t| t.holding_current_pa);
        c.access_resistance_mohm = tp(|t| t.access_resistance_mohm);
        c.membrane_resistance_mohm = tp(|t| t.membrane_resistance_mohm);
        c.membrane_capacitance_pf = tp(|t| t.capacitance_pf);
        return c;
    }
    c.resting_mv = median(
        rows.iter()
            .filter_map(|r| r.passive.as_ref().and_then(|p| p.baseline_mv))
            .collect(),
    );
    // f–I
    let mut fi: Vec<FiPoint> = rows
        .iter()
        .filter_map(|r| {
            let i = r.stimulus_pa?;
            (i > 0.0).then(|| FiPoint {
                sweep: r.sweep,
                current_pa: i,
                rate_hz: r.firing_rate_hz.unwrap_or(0.0),
                spike_count: r.spike_count_stimulus.unwrap_or(0),
            })
        })
        .collect();
    fi.sort_by(|a, b| a.current_pa.total_cmp(&b.current_pa));
    if let Some(p) = fi.iter().find(|p| p.spike_count > 0) {
        c.rheobase_pa = Some(p.current_pa);
        c.rheobase_sweep = Some(p.sweep);
    }
    let firing: Vec<(f64, f64)> = fi
        .iter()
        .filter(|p| p.spike_count > 0)
        .map(|p| (p.current_pa, p.rate_hz))
        .collect();
    c.fi_slope_hz_per_pa = slope(&firing);
    c.max_firing_rate_hz = fi.iter().map(|p| p.rate_hz).reduce(f64::max);
    c.fi_curve = fi;
    // passive, from spike-free hyperpolarising steps
    let hyper: Vec<(&SweepFeatures, &StepResponse, f64)> = rows
        .iter()
        .filter_map(|r| {
            let i = r.stimulus_pa?;
            let p = r.passive.as_ref()?;
            (i < 0.0 && r.spike_count_stimulus == Some(0)).then_some((r, p, i))
        })
        .collect();
    let dv: Vec<(f64, f64)> = hyper
        .iter()
        .filter_map(|(_, p, i)| Some((*i, p.steady_state_mv? - p.baseline_mv?)))
        .collect();
    c.input_resistance_mohm = if dv.len() >= 2 {
        slope(&dv).map(|s| s * 1000.0)
    } else {
        dv.first().map(|(i, d)| d / i * 1000.0)
    };
    c.tau_ms = median(hyper.iter().filter_map(|(_, p, _)| p.tau_ms).collect());
    if let (Some(t), Some(r)) = (c.tau_ms, c.input_resistance_mohm)
        && r > 0.0
    {
        c.capacitance_pf = Some(t / r * 1000.0);
    }
    c.sag_ratio = hyper
        .iter()
        .min_by(|a, b| a.2.total_cmp(&b.2))
        .and_then(|(_, p, _)| p.sag_ratio);
    c
}

/// What [`analyze_extracellular`] should do.
#[derive(Debug, Clone, Default)]
pub struct SpikesRequest {
    /// Trace (default 0).
    pub trace: u32,
    /// Channels (default all).
    pub channels: Vec<u32>,
    /// Sweeps/segments (default all).
    pub sweeps: Option<Vec<u32>>,
    /// Filter and threshold.
    pub settings: DetectSettings,
    /// Only the first `max_seconds` of each sweep (default: all).
    pub max_seconds: Option<f64>,
    /// Keep at most this many spike times per channel.
    pub max_times: usize,
}

/// Detection results of one channel.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ChannelSpikes {
    /// Channel index.
    pub channel: u32,
    /// Channel name.
    pub name: String,
    /// Unit of `noise` and `threshold`.
    pub unit: Option<String>,
    /// Noise (MAD / 0.6745 of the filtered signal; the first sweep analysed).
    pub noise: f64,
    /// Detection threshold (`threshold × noise`).
    pub threshold: f64,
    /// Spikes detected, all sweeps.
    pub spike_count: usize,
    /// Spikes per second of analysed signal.
    pub rate_hz: f64,
    /// Seconds analysed.
    pub duration_s: f64,
    /// Spike times, s from each sweep's start, with the sweep (at most `max_times`).
    pub times: Vec<SpikeTime>,
    /// True when `times` was cut.
    pub times_truncated: bool,
}

/// One spike time.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SpikeTime {
    /// Sweep/segment.
    pub sweep: u32,
    /// Sample within the sweep.
    pub sample: u64,
    /// Seconds from the sweep start.
    pub time_s: f64,
    /// Filtered amplitude at the peak (in the channel unit).
    pub amplitude: f64,
}

/// Output of `analyze spikes`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SpikesReport {
    /// Input path.
    pub path: String,
    /// Format id.
    pub format: String,
    /// Trace analysed.
    pub trace: u32,
    /// Sampling rate, Hz.
    pub sample_rate_hz: f64,
    /// Settings used (the high edge after clamping to 0.45 × the sampling rate).
    pub settings: DetectSettings,
    /// Sweeps analysed.
    pub sweeps: Vec<u32>,
    /// One entry per channel.
    pub channels: Vec<ChannelSpikes>,
    /// Spikes on all channels.
    pub spike_count_total: usize,
    /// Remarks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// Extracellular spike detection (module docs, [`extracellular`]).
pub fn analyze_extracellular(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    req: &SpikesRequest,
) -> Result<SpikesReport> {
    let t = trace_of(info, req.trace)?.clone();
    let fs = t.sample_rate_hz;
    let mut settings = req.settings;
    settings.high_hz = settings.high_hz.min(0.45 * fs);
    if !(fs > 0.0)
        || filter::bandpass_sos(settings.order, settings.low_hz, settings.high_hz, fs).is_none()
    {
        return Err(Error::unsupported(
            "ephys-analysis",
            format!(
                "a {}–{} Hz band-pass at {fs} Hz sampling",
                settings.low_hz, settings.high_hz
            ),
            "Spike detection needs a broadband signal sampled well above the pass band (typically >= 20 kHz); use `--band LOW:HIGH` within the Nyquist range, or pick the wideband trace (`openreadout info`).",
        ));
    }
    let channels: Vec<u32> = if req.channels.is_empty() {
        (0..t.channels.len() as u32).collect()
    } else {
        for &c in &req.channels {
            if c as usize >= t.channels.len() {
                return Err(Error::Usage(format!(
                    "channel {c} out of range (trace {} has {} channels)",
                    t.index,
                    t.channels.len()
                )));
            }
        }
        req.channels.clone()
    };
    let sweeps: Vec<u32> = match &req.sweeps {
        Some(s) => {
            if let Some(&bad) = s.iter().find(|&&k| k >= t.sweep_count) {
                return Err(Error::Usage(format!(
                    "sweep {bad} out of range (trace {} has {} sweeps)",
                    t.index, t.sweep_count
                )));
            }
            s.clone()
        }
        None => (0..t.sweep_count).collect(),
    };
    let mut out: Vec<ChannelSpikes> = channels
        .iter()
        .map(|&c| ChannelSpikes {
            channel: c,
            name: t.channels[c as usize].name.clone(),
            unit: t.channels[c as usize].unit.clone(),
            ..ChannelSpikes::default()
        })
        .collect();
    let mut notes = Vec::new();
    for &sw in &sweeps {
        let mut n = openreadout_core::trace::sweep_samples(&t, sw);
        if let Some(m) = req.max_seconds {
            let cap = (m * fs).max(0.0) as u64;
            if cap < n {
                notes.push(format!(
                    "sweep {sw}: first {m} s analysed of {:.3} s",
                    n as f64 / fs
                ));
                n = cap;
            }
        }
        // paged: readers cap one read (a 385-channel probe returns ~174 k samples at a time)
        let cols = openreadout_core::trace::read_channels(ds, t.index, sw, 0, n, &channels)?;
        for (k, x) in cols.iter().enumerate() {
            let Some(y) = extracellular::bandpass(x, fs, &settings) else {
                continue;
            };
            let noise = extracellular::mad_noise(&y);
            let o = &mut out[k];
            if o.noise == 0.0 {
                o.noise = noise;
                o.threshold = noise * settings.threshold;
            }
            let peaks = extracellular::detect_peaks(&y, fs, noise, &settings);
            o.spike_count += peaks.len();
            o.duration_s += x.len() as f64 / fs;
            for p in peaks {
                if o.times.len() < req.max_times {
                    o.times.push(SpikeTime {
                        sweep: sw,
                        sample: p as u64,
                        time_s: p as f64 / fs,
                        amplitude: y[p],
                    });
                } else {
                    o.times_truncated = true;
                }
            }
        }
    }
    for o in &mut out {
        o.rate_hz = if o.duration_s > 0.0 {
            o.spike_count as f64 / o.duration_s
        } else {
            0.0
        };
    }
    Ok(SpikesReport {
        path: info.path.clone(),
        format: info.format.id.clone(),
        trace: t.index,
        sample_rate_hz: fs,
        settings,
        sweeps,
        spike_count_total: out.iter().map(|c| c.spike_count).sum(),
        channels: out,
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units() {
        assert_eq!(to_mv(Some("V")), Some(1000.0));
        assert_eq!(to_pa(Some("nA")), Some(1000.0));
        assert_eq!(to_mv(Some("pA")), None);
        assert_eq!(to_pa(None), None);
    }

    #[test]
    fn implausible_units_are_flagged() {
        // a channel labelled A whose samples are pA-sized numbers (−5.4 "A")
        let n = implausible_unit(Some("IN 0"), Some("A"), false, 5.4e12, 1e12).unwrap();
        assert!(n.contains("probably wrong"), "{n}");
        assert!(n.contains("5.4 pA"), "{n}");
        // ordinary recordings pass
        assert!(implausible_unit(None, Some("pA"), false, 2500.0, 1.0).is_none());
        assert!(implausible_unit(None, Some("V"), true, 70.0, 1000.0).is_none());
        // a V-labelled channel holding mV numbers (−65 "V" = −65000 mV)
        assert!(implausible_unit(None, Some("V"), true, 65_000.0, 1000.0).is_some());
    }

    #[test]
    fn slopes_and_medians() {
        assert_eq!(slope(&[(0.0, 1.0), (1.0, 3.0), (2.0, 5.0)]), Some(2.0));
        assert_eq!(slope(&[(1.0, 1.0)]), None);
        assert_eq!(median(vec![3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(vec![f64::NAN]), None);
    }
}
