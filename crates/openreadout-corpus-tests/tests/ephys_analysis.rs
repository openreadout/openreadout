//! Electrophysiology analysis against eFEL, pyABF and SpikeInterface (`oracle/signal/ephys.py`
//! → `corpus/oracle/ephys-analysis/*.json`):
//!
//! - current clamp: per sweep, spike counts (whole sweep and inside the stimulus), the stimulus
//!   window, peak times and voltages, onset voltage, half-width, AHP, first-spike latency,
//!   adaptation index, baseline and steady-state voltage, sag and time constant against eFEL; per
//!   cell, the rheobase, input resistance, time constant and sag derived from eFEL's numbers;
//! - current clamp in NWB: per `CurrentClampSeries`, spike counts, peak times and voltages and
//!   onset voltages against eFEL on the samples h5py reads (no stimulus window: NWB has no epoch
//!   table);
//! - voltage clamp: holding current, access and membrane resistance and capacitance against
//!   pyABF's memtest;
//! - extracellular: per-channel spike counts and times against SpikeInterface's band-pass filter
//!   and `detect_peaks`.
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --test ephys_analysis --
//! --nocapture` (`OPENREADOUT_CORPUS_DIR` overrides `corpus/files`).
#![cfg(feature = "corpus")]
#![allow(
    clippy::many_single_char_names,
    clippy::case_sensitive_file_extension_comparisons
)]

use std::path::{Path, PathBuf};

use openreadout_core::{Dataset, FormatReader};
use openreadout_signal::ephys::{self, CellRequest, SpikesRequest};
use serde::Deserialize;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn files_dir() -> PathBuf {
    std::env::var_os("OPENREADOUT_CORPUS_DIR")
        .map_or_else(|| root().join("corpus/files"), PathBuf::from)
}

fn oracle_files(suffix: &str) -> Vec<PathBuf> {
    let dir = root().join("corpus/oracle/ephys-analysis");
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut v: Vec<PathBuf> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            let n = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            n.ends_with(suffix)
                && (suffix != ".json"
                    || (!n.ends_with(".memtest.json")
                        && !n.ends_with(".spikes.json")
                        && !n.ends_with(".nwb-cc.json")))
        })
        .collect();
    v.sort();
    v
}

type Vals = Option<Vec<Option<f64>>>;

#[derive(Deserialize)]
struct EfelSweep {
    sweep: u32,
    stim_start_ms: f64,
    stim_end_ms: f64,
    current_pa: f64,
    spike_count: Vals,
    spike_count_stimint: Vals,
    peak_time: Vals,
    peak_voltage: Vals,
    #[serde(rename = "AP_begin_voltage")]
    ap_begin_voltage: Vals,
    #[serde(rename = "AP_duration_half_width")]
    half_width: Vals,
    #[serde(rename = "AHP_depth_abs")]
    ahp: Vals,
    time_to_first_spike: Vals,
    adaptation_index2: Vals,
    voltage_base: Vals,
    steady_state_voltage_stimend: Vals,
    sag_ratio1: Vals,
    time_constant: Vals,
}
#[derive(Deserialize)]
struct EfelCell {
    rheobase_pa: Option<f64>,
    input_resistance_mohm: Option<f64>,
    tau_ms: Option<f64>,
    sag_ratio: Option<f64>,
}
#[derive(Deserialize)]
struct EfelOracle {
    id: String,
    sweeps: Vec<EfelSweep>,
    cell: EfelCell,
}

fn first(v: &Vals) -> Option<f64> {
    v.as_ref().and_then(|x| x.first().copied().flatten())
}
fn all(v: &Vals) -> Vec<f64> {
    v.as_ref()
        .map(|x| x.iter().filter_map(|y| *y).collect())
        .unwrap_or_default()
}

fn open(p: &Path) -> Box<dyn Dataset> {
    let n = p.to_string_lossy().to_ascii_lowercase();
    if n.ends_with(".abf") {
        openreadout_abf::AbfReader.open(p).unwrap()
    } else if n.ends_with(".ns5") || n.ends_with(".ns6") {
        openreadout_blackrock::BlackrockReader.open(p).unwrap()
    } else if n.ends_with(".nwb") {
        openreadout_hdf5::NwbReader.open(p).unwrap()
    } else if n.ends_with(".rhd") || n.ends_with(".rhs") {
        openreadout_intan::IntanReader.open(p).unwrap()
    } else {
        openreadout_neuralynx::NeuralynxReader.open(p).unwrap()
    }
}

fn med(mut v: Vec<f64>) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

#[test]
fn current_clamp_matches_efel() {
    let dir = files_dir();
    let mut failures = Vec::new();
    for p in oracle_files(".json") {
        let o: EfelOracle = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        let path = dir.join(format!("{}.abf", o.id));
        if !path.exists() {
            continue;
        }
        let mut ds = open(&path);
        let info = ds.info().unwrap();
        let r = ephys::analyze_cell(ds.as_mut(), &info, &CellRequest::default()).unwrap();
        let fs = r.sample_rate_hz;
        let sample_ms = 1000.0 / fs;
        let (mut n_spk, mut d_peak_t, mut d_peak_v) = (0usize, 0.0_f64, 0.0_f64);
        let (mut same_peak, mut near_peak) = (0usize, 0usize);
        let (mut d_thr, mut rel_hw, mut d_ahp) = (Vec::new(), Vec::new(), Vec::new());
        let (mut count_ok, mut stim_ok) = (0usize, 0usize);
        let mut d_lat = 0.0_f64;
        let (mut d_base, mut d_ss, mut d_sag, mut rel_tau, mut d_ai) =
            (0.0_f64, 0.0_f64, 0.0_f64, Vec::new(), 0.0_f64);
        for es in &o.sweeps {
            let row = &r.sweeps[es.sweep as usize];
            let ours: Vec<&ephys::Spike> =
                r.spikes.iter().filter(|s| s.sweep == es.sweep).collect();
            let theirs_n = first(&es.spike_count).unwrap_or(0.0) as usize;
            if row.spike_count == theirs_n {
                count_ok += 1;
            } else {
                failures.push(format!(
                    "{} sweep {}: {} spikes vs eFEL {theirs_n}",
                    o.id, es.sweep, row.spike_count
                ));
            }
            let st = row.stimulus_start_s.unwrap_or(f64::NAN) * 1000.0;
            let en = row.stimulus_end_s.unwrap_or(f64::NAN) * 1000.0;
            if (st - es.stim_start_ms).abs() <= sample_ms
                && (en - es.stim_end_ms).abs() <= sample_ms
                && (row.stimulus_pa.unwrap_or(f64::NAN) - es.current_pa).abs() < 1e-6
            {
                stim_ok += 1;
            } else {
                failures.push(format!(
                    "{} sweep {}: stimulus {st}..{en} ms {:?} pA vs eFEL {}..{} ms {} pA",
                    o.id,
                    es.sweep,
                    row.stimulus_pa,
                    es.stim_start_ms,
                    es.stim_end_ms,
                    es.current_pa
                ));
            }
            let their_in = first(&es.spike_count_stimint).unwrap_or(0.0) as usize;
            if row.spike_count_stimulus != Some(their_in) {
                failures.push(format!(
                    "{} sweep {}: {:?} spikes in the stimulus vs eFEL {their_in}",
                    o.id, es.sweep, row.spike_count_stimulus
                ));
            }
            let pt = all(&es.peak_time);
            let pv = all(&es.peak_voltage);
            let th = all(&es.ap_begin_voltage);
            let hw = all(&es.half_width);
            let ahp = all(&es.ahp);
            if ours.len() == pt.len() {
                for (k, s) in ours.iter().enumerate() {
                    n_spk += 1;
                    let dt = (s.peak_time_s * 1000.0 - pt[k]).abs();
                    d_peak_t = d_peak_t.max(dt);
                    if dt < 1e-6 {
                        same_peak += 1;
                    }
                    if dt <= sample_ms + 1e-6 {
                        near_peak += 1;
                    }
                    if let Some(v) = pv.get(k) {
                        d_peak_v = d_peak_v.max((s.peak_mv - v).abs());
                        // ours is the largest sample of the spike; eFEL's never exceeds it
                        if *v > s.peak_mv + 1e-6 {
                            failures.push(format!(
                                "{} sweep {} spike {k}: eFEL peak {v} above ours {}",
                                o.id, es.sweep, s.peak_mv
                            ));
                        }
                    }
                    if let Some(v) = th.get(k) {
                        d_thr.push((s.threshold_mv - v).abs());
                    }
                    if let (Some(a), Some(b)) = (s.half_width_ms, hw.get(k)) {
                        rel_hw.push((a - b).abs() / b);
                    }
                    if let (Some(a), Some(b)) = (s.ahp_mv, ahp.get(k)) {
                        d_ahp.push((a - b).abs());
                    }
                }
            }
            // eFEL's latency is the first spike of the whole sweep minus the stimulus onset; compare
            // when that first spike is inside the stimulus
            let first_in = ours
                .first()
                .is_some_and(|s| s.peak_time_s * 1000.0 >= es.stim_start_ms);
            if let (true, Some(a), Some(b)) = (
                first_in,
                row.first_spike_latency_ms,
                first(&es.time_to_first_spike),
            ) {
                d_lat = d_lat.max((a - b).abs());
            }
            if let (Some(a), Some(b)) = (row.adaptation_index, first(&es.adaptation_index2)) {
                d_ai = d_ai.max((a - b).abs());
            }
            if let Some(ps) = &row.passive {
                if let (Some(a), Some(b)) = (ps.baseline_mv, first(&es.voltage_base)) {
                    d_base = d_base.max((a - b).abs());
                }
                if let (Some(a), Some(b)) =
                    (ps.steady_state_mv, first(&es.steady_state_voltage_stimend))
                {
                    d_ss = d_ss.max((a - b).abs());
                }
                if let (Some(a), Some(b)) = (ps.sag_ratio, first(&es.sag_ratio1)) {
                    d_sag = d_sag.max((a - b).abs());
                }
                if let (Some(a), Some(b)) = (ps.tau_ms, first(&es.time_constant)) {
                    rel_tau.push((a - b).abs() / b);
                }
            }
        }
        let c = &r.cell;
        println!(
            "{}: {count_ok}/{} sweep spike counts equal, {stim_ok} stimulus windows equal; {n_spk} spikes: {same_peak} same peak sample, {near_peak} within one sample, max |d peak time| {d_peak_t:.4} ms, max |d peak V| {d_peak_v:.4} mV, median |d onset V| {:.3} mV (max {:.3}), median rel half-width diff {:.4}, median |d AHP| {:.3} mV; max |d latency| {d_lat:.4} ms; max |d adaptation| {d_ai:.4}; max |d baseline| {d_base:.4} mV, |d steady| {d_ss:.4} mV, |d sag| {d_sag:.4}; median rel tau diff {:.3}",
            o.id,
            o.sweeps.len(),
            med(d_thr.clone()),
            d_thr.iter().copied().fold(0.0, f64::max),
            med(rel_hw.clone()),
            med(d_ahp.clone()),
            med(rel_tau.clone()),
        );
        println!(
            "    cell: rheobase {:?} (eFEL {:?}) pA, Rin {:?} (eFEL {:?}) MΩ, tau {:?} (eFEL {:?}) ms, sag {:?} (eFEL {:?})",
            c.rheobase_pa,
            o.cell.rheobase_pa,
            c.input_resistance_mohm,
            o.cell.input_resistance_mohm,
            c.tau_ms,
            o.cell.tau_ms,
            c.sag_ratio,
            o.cell.sag_ratio
        );
        if c.rheobase_pa != o.cell.rheobase_pa {
            failures.push(format!(
                "{}: rheobase {:?} vs {:?}",
                o.id, c.rheobase_pa, o.cell.rheobase_pa
            ));
        }
        let rel = |a: Option<f64>, b: Option<f64>| match (a, b) {
            (Some(a), Some(b)) => (a - b).abs() / b.abs(),
            (None, None) => 0.0,
            _ => f64::INFINITY,
        };
        if rel(c.input_resistance_mohm, o.cell.input_resistance_mohm) > 0.01 {
            failures.push(format!(
                "{}: input resistance {:?} vs {:?}",
                o.id, c.input_resistance_mohm, o.cell.input_resistance_mohm
            ));
        }
        if (near_peak as f64) < 0.95 * n_spk as f64 {
            failures.push(format!(
                "{}: only {near_peak}/{n_spk} peaks within one sample of eFEL's",
                o.id
            ));
        }
        if d_base > 0.15 || d_ss > 0.05 || d_lat > sample_ms + 1e-6 {
            failures.push(format!(
                "{}: baseline/steady/latency off: {d_base} {d_ss} {d_lat}",
                o.id
            ));
        }
        if d_sag > 0.01 {
            failures.push(format!("{}: sag ratio off by {d_sag}", o.id));
        }
        if med(d_thr) > 1.5 || med(rel_hw) > 0.05 {
            failures.push(format!("{}: onset/half-width disagree", o.id));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[derive(Deserialize)]
struct NwbSeries {
    name: String,
    samples: u64,
    spike_count: Vals,
    peak_time: Vals,
    peak_voltage: Vals,
    #[serde(rename = "AP_begin_voltage")]
    ap_begin_voltage: Vals,
}
#[derive(Deserialize)]
struct NwbOracle {
    id: String,
    series: Vec<NwbSeries>,
}

#[test]
fn nwb_current_clamp_matches_efel() {
    let dir = files_dir();
    let mut failures = Vec::new();
    for p in oracle_files(".nwb-cc.json") {
        let o: NwbOracle = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        let path = dir.join(format!("{}.nwb", o.id));
        if !path.exists() {
            continue;
        }
        let mut ds = open(&path);
        let info = ds.info().unwrap();
        let (mut n_series, mut n_spk, mut near_peak) = (0usize, 0usize, 0usize);
        let (mut d_peak_v, mut d_thr) = (0.0_f64, Vec::new());
        for es in &o.series {
            let Some(t) = info
                .traces
                .iter()
                .find(|t| t.name.as_deref() == Some(es.name.as_str()))
            else {
                failures.push(format!("{}: no trace named {}", o.id, es.name));
                continue;
            };
            if t.sample_count != es.samples {
                failures.push(format!(
                    "{} {}: {} samples vs h5py {}",
                    o.id, es.name, t.sample_count, es.samples
                ));
            }
            let req = CellRequest {
                trace: t.index,
                ..CellRequest::default()
            };
            let r = match ephys::analyze_cell(ds.as_mut(), &info, &req) {
                Ok(r) => r,
                Err(e) => {
                    failures.push(format!("{} {}: {e}", o.id, es.name));
                    continue;
                }
            };
            n_series += 1;
            if r.clamp_mode != "current_clamp" {
                failures.push(format!("{} {}: clamp mode {}", o.id, es.name, r.clamp_mode));
            }
            let sample_ms = 1000.0 / r.sample_rate_hz;
            let theirs_n = first(&es.spike_count).unwrap_or(0.0) as usize;
            if r.spike_count_total != theirs_n {
                failures.push(format!(
                    "{} {}: {} spikes vs eFEL {theirs_n}",
                    o.id, es.name, r.spike_count_total
                ));
                continue;
            }
            let (pt, pv, th) = (
                all(&es.peak_time),
                all(&es.peak_voltage),
                all(&es.ap_begin_voltage),
            );
            for (k, s) in r.spikes.iter().enumerate() {
                n_spk += 1;
                if pt
                    .get(k)
                    .is_some_and(|t| (s.peak_time_s * 1000.0 - t).abs() <= sample_ms + 1e-6)
                {
                    near_peak += 1;
                }
                if let Some(v) = pv.get(k) {
                    d_peak_v = d_peak_v.max((s.peak_mv - v).abs());
                }
                if let Some(v) = th.get(k) {
                    d_thr.push((s.threshold_mv - v).abs());
                }
            }
        }
        println!(
            "{}: {n_series}/{} series analysed; {n_spk} spikes, {near_peak} peaks within one sample of eFEL's, max |d peak V| {d_peak_v:.4} mV, median |d onset V| {:.3} mV",
            o.id,
            o.series.len(),
            med(d_thr.clone()),
        );
        if n_series != o.series.len() || near_peak != n_spk || d_peak_v > 0.5 {
            failures.push(format!("{}: peaks disagree with eFEL", o.id));
        }
        if med(d_thr) > 1.5 {
            failures.push(format!("{}: onset voltages disagree with eFEL", o.id));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[derive(Deserialize)]
struct Memtest {
    id: String,
    #[serde(rename = "Ih_pa")]
    ih: Vec<Option<f64>>,
    #[serde(rename = "Ra_mohm")]
    ra: Vec<Option<f64>>,
    #[serde(rename = "Rm_mohm")]
    rm: Vec<Option<f64>>,
    #[serde(rename = "Cm_step_pf")]
    cm: Vec<Option<f64>>,
}

#[test]
fn voltage_clamp_matches_pyabf_memtest() {
    let dir = files_dir();
    let mut failures = Vec::new();
    for p in oracle_files(".memtest.json") {
        let o: Memtest = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        let path = dir.join(format!("{}.abf", o.id));
        if !path.exists() {
            continue;
        }
        let mut ds = open(&path);
        let info = ds.info().unwrap();
        let r = ephys::analyze_cell(ds.as_mut(), &info, &CellRequest::default()).unwrap();
        let theirs = |v: &[Option<f64>]| med(v.iter().filter_map(|x| *x).collect());
        let c = &r.cell;
        let total = med(r
            .sweeps
            .iter()
            .filter_map(|w| w.test_pulse.as_ref().and_then(|t| t.total_resistance_mohm))
            .collect());
        let (ih, ra, rm, cm) = (theirs(&o.ih), theirs(&o.ra), theirs(&o.rm), theirs(&o.cm));
        println!(
            "{}: {} — holding {:?} pA (pyABF {ih:.2}), Ra {:?} MΩ (pyABF {ra:.2}), total resistance {total:.1} MΩ (pyABF \"Rm\" = ΔV/ΔI steady, {rm:.1}), Rm {:?} MΩ, Cm {:?} pF (pyABF τ/Ra {cm:.2})",
            o.id,
            r.clamp_mode,
            c.holding_current_pa,
            c.access_resistance_mohm,
            c.membrane_resistance_mohm,
            c.membrane_capacitance_pf
        );
        let rel = |a: Option<f64>, b: f64| a.map_or(f64::INFINITY, |a| (a - b).abs() / b.abs());
        if r.clamp_mode != "voltage_clamp" {
            failures.push(format!("{}: clamp mode {}", o.id, r.clamp_mode));
        }
        if c.holding_current_pa
            .is_none_or(|h| (h - ih).abs() > 0.02 * ih.abs().max(10.0))
        {
            failures.push(format!("{}: holding current", o.id));
        }
        // same definition (ΔV over the steady-state current change): tight
        if rel(Some(total), rm) > 0.1 {
            failures.push(format!("{}: total resistance {total} vs {rm}", o.id));
        }
        // Ra: extrapolated transient amplitude; pyABF measures it above the steady state, we
        // above the baseline (ΔV/(Ra+Rm) apart)
        if rel(c.access_resistance_mohm, ra) > 0.15 {
            failures.push(format!("{}: access resistance", o.id));
        }
        // Cm: pyABF uses τ/Ra (τ from matching the area of the normalised decay), which leaves out
        // Rm and the charge the membrane current takes; we integrate the transient's charge. The
        // analytic check is the RC-circuit unit test; here only a factor-of-2 sanity bound.
        if c.membrane_capacitance_pf
            .is_none_or(|x| !(x / cm > 0.5 && x / cm < 2.0))
        {
            failures.push(format!("{}: capacitance", o.id));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[derive(Deserialize)]
struct SpikeChannel {
    channel: u32,
    count: usize,
    samples: Vec<u64>,
    noise: f64,
}
#[derive(Deserialize)]
struct SpikeOracle {
    id: String,
    file: String,
    samples: u64,
    channels: Vec<SpikeChannel>,
}

#[test]
fn extracellular_matches_spikeinterface() {
    let dir = files_dir();
    let mut failures = Vec::new();
    for p in oracle_files(".spikes.json") {
        let o: SpikeOracle = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        let path = dir.join(&o.file);
        if !path.exists() {
            continue;
        }
        let mut ds = open(&path);
        let info = ds.info().unwrap();
        // the broadband trace: highest rate
        let t = info
            .traces
            .iter()
            .max_by(|a, b| a.sample_rate_hz.total_cmp(&b.sample_rate_hz))
            .unwrap()
            .index;
        let req = SpikesRequest {
            trace: t,
            sweeps: Some(vec![0]),
            max_times: usize::MAX,
            ..SpikesRequest::default()
        };
        let r = ephys::analyze_extracellular(ds.as_mut(), &info, &req).unwrap();
        for oc in &o.channels {
            let Some(ours) = r.channels.iter().find(|c| c.channel == oc.channel) else {
                failures.push(format!("{}: no channel {}", o.id, oc.channel));
                continue;
            };
            let our_samples: Vec<u64> = ours.times.iter().map(|t| t.sample).collect();
            let matched = oc
                .samples
                .iter()
                .filter(|&&s| our_samples.iter().any(|&q| q.abs_diff(s) <= 3))
                .count();
            let recall = if oc.samples.is_empty() {
                1.0
            } else {
                matched as f64 / oc.samples.len() as f64
            };
            println!(
                "{} ch {}: {} spikes (SpikeInterface {}), {matched}/{} of theirs within 0.1 ms; noise {:.4} vs {:.4}; {} samples",
                o.id,
                oc.channel,
                ours.spike_count,
                oc.count,
                oc.samples.len(),
                ours.noise,
                oc.noise,
                o.samples
            );
            let diff = ours.spike_count.abs_diff(oc.count) as f64;
            if oc.count >= 20 && (diff > 0.03 * oc.count as f64 || recall < 0.95) {
                failures.push(format!(
                    "{} ch {}: {} vs {} spikes, recall {recall:.3}",
                    o.id, oc.channel, ours.spike_count, oc.count
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
