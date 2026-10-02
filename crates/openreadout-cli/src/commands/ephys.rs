//! `analyze ephys-features` (patch clamp) and `analyze spikes` (extracellular detection).

use std::path::PathBuf;

use openreadout_core::{Error, Registry, Result};
use openreadout_signal::ephys::{
    ApSettings, CellReport, CellRequest, DetectSettings, PeakSign, SpikesReport, SpikesRequest,
    analyze_cell, analyze_extracellular,
};

use crate::output::{emit, fail};

/// Parse `0,2,5-7` into sweep/channel numbers.
pub fn parse_list(s: &str, what: &str) -> Result<Vec<u32>> {
    let mut out = Vec::new();
    for part in s.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let err = || Error::Usage(format!("{what} {s:?}: expected numbers like 0,2,5-7"));
        if let Some((a, b)) = part.split_once('-') {
            let a: u32 = a.trim().parse().map_err(|_| err())?;
            let b: u32 = b.trim().parse().map_err(|_| err())?;
            if b < a || b - a > 1_000_000 {
                return Err(err());
            }
            out.extend(a..=b);
        } else {
            out.push(part.parse().map_err(|_| err())?);
        }
    }
    Ok(out)
}

/// Which tidy table `--csv` prints.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum CellTable {
    /// One row per sweep.
    Sweeps,
    /// One row per spike.
    Spikes,
    /// The f–I curve.
    Fi,
}

/// `ephys-features`.
#[derive(Debug, Clone, clap::Args)]
pub struct EphysArgs {
    /// A patch-clamp recording (ABF, ATF, NWB, …).
    pub file: PathBuf,
    /// Trace index (see `info`).
    #[arg(long, default_value_t = 0)]
    pub trace: u32,
    /// Channel index (default: the first voltage channel, else the first current channel).
    #[arg(long)]
    pub channel: Option<u32>,
    /// Sweeps to analyse, e.g. `0,3,5-9` (default all).
    #[arg(long, value_name = "LIST")]
    pub sweeps: Option<String>,
    /// Voltage a spike must cross upward, mV.
    #[arg(long, value_name = "MV", default_value_t = -20.0, allow_negative_numbers = true)]
    pub peak_threshold: f64,
    /// dV/dt defining the spike onset (threshold), V/s.
    #[arg(long, value_name = "V_PER_S", default_value_t = 10.0)]
    pub dvdt_threshold: f64,
    /// Keep at most this many rows in the per-spike table (counts stay complete; `spikes_truncated`
    /// says when rows were left out). Default 25 with --json, every spike with --csv.
    #[arg(long, value_name = "N")]
    pub max_spikes: Option<usize>,
    /// Print one tidy table as CSV instead: `sweeps`, `spikes` or `fi`.
    #[arg(long, value_enum, value_name = "TABLE", conflicts_with = "json")]
    pub csv: Option<CellTable>,
    #[arg(long)]
    pub json: bool,
}

fn cell_request(a: &EphysArgs) -> Result<CellRequest> {
    let dvdt_ok = a.dvdt_threshold.is_finite() && a.dvdt_threshold > 0.0;
    if !a.peak_threshold.is_finite() || !dvdt_ok {
        return Err(Error::Usage(
            "--peak-threshold must be finite and --dvdt-threshold positive".into(),
        ));
    }
    Ok(CellRequest {
        trace: a.trace,
        channel: a.channel,
        sweeps: a
            .sweeps
            .as_deref()
            .map(|s| parse_list(s, "--sweeps"))
            .transpose()?,
        ap: ApSettings {
            peak_threshold_mv: a.peak_threshold,
            dvdt_threshold_v_per_s: a.dvdt_threshold,
            ..ApSettings::default()
        },
        max_spikes: a.max_spikes.unwrap_or(if a.csv.is_some() {
            usize::MAX
        } else {
            openreadout_signal::api::DEFAULT_MAX_SPIKES
        }),
    })
}

/// Run `ephys-features` on one file.
pub fn ephys_features(reg: &Registry, a: &EphysArgs) -> Result<CellReport> {
    let req = cell_request(a)?;
    let (_, mut ds) = reg.open(&a.file)?;
    let info = ds.info()?;
    analyze_cell(ds.as_mut(), &info, &req)
}

fn opt(v: Option<f64>) -> String {
    v.map_or_else(String::new, |x| format!("{x}"))
}

fn csv_table(r: &CellReport, t: CellTable) -> String {
    let mut s = String::new();
    match t {
        CellTable::Sweeps => {
            s.push_str("sweep,stimulus_pa,stimulus_mv,stimulus_start_s,stimulus_end_s,spike_count,spike_count_stimulus,firing_rate_hz,first_spike_latency_ms,isi_mean_ms,isi_cv,adaptation_index,half_width_mean_ms,amplitude_mean_mv,baseline_mv,steady_state_mv,extreme_mv,input_resistance_mohm,tau_ms,sag_ratio,holding_current_pa,access_resistance_mohm,membrane_resistance_mohm,capacitance_pf\n");
            for w in &r.sweeps {
                let p = w.passive.clone().unwrap_or_default();
                let tp = w.test_pulse.clone().unwrap_or_default();
                s.push_str(&format!(
                    "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}\n",
                    w.sweep,
                    opt(w.stimulus_pa),
                    opt(w.stimulus_mv),
                    opt(w.stimulus_start_s),
                    opt(w.stimulus_end_s),
                    w.spike_count,
                    w.spike_count_stimulus
                        .map_or_else(String::new, |c| c.to_string()),
                    opt(w.firing_rate_hz),
                    opt(w.first_spike_latency_ms),
                    opt(w.isi_mean_ms),
                    opt(w.isi_cv),
                    opt(w.adaptation_index),
                    opt(w.half_width_mean_ms),
                    opt(w.amplitude_mean_mv),
                    opt(p.baseline_mv),
                    opt(p.steady_state_mv),
                    opt(p.extreme_mv),
                    opt(p.input_resistance_mohm),
                    opt(p.tau_ms),
                    opt(p.sag_ratio),
                    opt(tp.holding_current_pa),
                    opt(tp.access_resistance_mohm),
                    opt(tp.membrane_resistance_mohm),
                    opt(tp.capacitance_pf),
                ));
            }
        }
        CellTable::Spikes => {
            s.push_str("sweep,index,peak_time_s,peak_mv,threshold_time_s,threshold_mv,amplitude_mv,half_width_ms,rise_time_ms,decay_time_ms,ahp_mv,ahp_depth_mv,upstroke_v_per_s,downstroke_v_per_s,isi_ms\n");
            for p in &r.spikes {
                s.push_str(&format!(
                    "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}\n",
                    p.sweep,
                    p.index,
                    p.peak_time_s,
                    p.peak_mv,
                    p.threshold_time_s,
                    p.threshold_mv,
                    p.amplitude_mv,
                    opt(p.half_width_ms),
                    p.rise_time_ms,
                    opt(p.decay_time_ms),
                    opt(p.ahp_mv),
                    opt(p.ahp_depth_mv),
                    p.upstroke_v_per_s,
                    opt(p.downstroke_v_per_s),
                    opt(p.isi_ms),
                ));
            }
        }
        CellTable::Fi => {
            s.push_str("sweep,current_pa,rate_hz,spike_count\n");
            for p in &r.cell.fi_curve {
                s.push_str(&format!(
                    "{},{},{},{}\n",
                    p.sweep, p.current_pa, p.rate_hz, p.spike_count
                ));
            }
        }
    }
    s
}

fn render_cell(r: &CellReport) -> String {
    let c = &r.cell;
    let f = |v: Option<f64>, u: &str| v.map_or_else(|| "-".into(), |x| format!("{x:.4} {u}"));
    let mut s = format!(
        "{} ({}), channel {} [{}], {} sweeps, {} spikes",
        r.clamp_mode,
        r.format,
        r.channel.name,
        r.channel.unit.as_deref().unwrap_or("?"),
        r.sweeps.len(),
        r.spike_count_total
    );
    if r.clamp_mode == "voltage_clamp" {
        s.push_str(&format!(
            "\n  holding current {}\n  access resistance {}\n  membrane resistance {}\n  capacitance {}",
            f(c.holding_current_pa, "pA"),
            f(c.access_resistance_mohm, "MΩ"),
            f(c.membrane_resistance_mohm, "MΩ"),
            f(c.membrane_capacitance_pf, "pF"),
        ));
    } else {
        s.push_str(&format!(
            "\n  resting {}\n  rheobase {}{}\n  f–I slope {}\n  max rate {}\n  input resistance {}\n  tau {}\n  capacitance {}\n  sag ratio {}",
            f(c.resting_mv, "mV"),
            f(c.rheobase_pa, "pA"),
            c.rheobase_sweep.map_or_else(String::new, |w| format!(" (sweep {w})")),
            f(c.fi_slope_hz_per_pa, "Hz/pA"),
            f(c.max_firing_rate_hz, "Hz"),
            f(c.input_resistance_mohm, "MΩ"),
            f(c.tau_ms, "ms"),
            f(c.capacitance_pf, "pF"),
            f(c.sag_ratio, ""),
        ));
        s.push_str("\n  sweep  stim(pA)  spikes  in-stim  rate(Hz)");
        for w in &r.sweeps {
            s.push_str(&format!(
                "\n  {:>5}  {:>8}  {:>6}  {:>7}  {:>8}",
                w.sweep,
                w.stimulus_pa
                    .map_or_else(|| "-".into(), |x| format!("{x:.1}")),
                w.spike_count,
                w.spike_count_stimulus
                    .map_or_else(|| "-".into(), |x| x.to_string()),
                w.firing_rate_hz
                    .map_or_else(|| "-".into(), |x| format!("{x:.2}")),
            ));
        }
    }
    for n in &r.notes {
        s.push_str(&format!("\n  note: {n}"));
    }
    s
}

pub fn run_ephys_features(reg: &Registry, a: &EphysArgs) -> i32 {
    match ephys_features(reg, a) {
        Ok(r) => {
            if let Some(t) = a.csv {
                print!("{}", csv_table(&r, t));
                0
            } else {
                emit(a.json, &r, render_cell)
            }
        }
        Err(e) => fail(a.json, &e),
    }
}

/// `spikes`.
#[derive(Debug, Clone, clap::Args)]
pub struct SpikesArgs {
    /// An extracellular recording (Blackrock, Neuralynx, SpikeGLX, Intan, Plexon, NWB, …).
    pub file: PathBuf,
    /// Trace index (see `info`; pick the broadband stream).
    #[arg(long, default_value_t = 0)]
    pub trace: u32,
    /// Channels, e.g. `0-3,7` (default all).
    #[arg(long, value_name = "LIST")]
    pub channels: Option<String>,
    /// Sweeps/segments, e.g. `0` (default all).
    #[arg(long, value_name = "LIST")]
    pub sweeps: Option<String>,
    /// Band-pass `LOW:HIGH` in Hz (Butterworth, zero phase).
    #[arg(long, value_name = "LOW:HIGH", default_value = "300:6000")]
    pub band: String,
    /// Butterworth order.
    #[arg(long, default_value_t = 5)]
    pub order: usize,
    /// Threshold in noise units (noise = median(|x|)/0.6745 of the filtered signal).
    #[arg(long, value_name = "K", default_value_t = 5.0)]
    pub threshold: f64,
    /// Spike polarity: `neg`, `pos` or `both`.
    #[arg(long, value_enum, default_value = "neg")]
    pub sign: SignArg,
    /// A spike must be the extreme within this many ms on either side.
    #[arg(long, value_name = "MS", default_value_t = 0.1)]
    pub exclude_ms: f64,
    /// Analyse only the first N seconds of each sweep.
    #[arg(long, value_name = "S")]
    pub max_seconds: Option<f64>,
    /// Spike times listed per channel (counts stay complete).
    #[arg(long, value_name = "N", default_value_t = 1000)]
    pub max_times: usize,
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum SignArg {
    Neg,
    Pos,
    Both,
}

fn spikes_request(a: &SpikesArgs) -> Result<SpikesRequest> {
    let err = || Error::Usage(format!("--band {:?}: expected LOW:HIGH in Hz", a.band));
    let (lo, hi) = a.band.split_once(':').ok_or_else(err)?;
    let lo: f64 = lo.trim().parse().map_err(|_| err())?;
    let hi: f64 = hi.trim().parse().map_err(|_| err())?;
    if !(lo > 0.0 && hi > lo && hi.is_finite()) {
        return Err(err());
    }
    let threshold_ok = a.threshold.is_finite() && a.threshold > 0.0;
    let exclude_ok = a.exclude_ms.is_finite() && a.exclude_ms >= 0.0;
    if !threshold_ok || !exclude_ok {
        return Err(Error::Usage(
            "--threshold must be positive and --exclude-ms >= 0".into(),
        ));
    }
    if a.order == 0 || a.order > 10 {
        return Err(Error::Usage("--order must be 1 to 10".into()));
    }
    Ok(SpikesRequest {
        trace: a.trace,
        channels: a
            .channels
            .as_deref()
            .map(|s| parse_list(s, "--channels"))
            .transpose()?
            .unwrap_or_default(),
        sweeps: a
            .sweeps
            .as_deref()
            .map(|s| parse_list(s, "--sweeps"))
            .transpose()?,
        settings: DetectSettings {
            low_hz: lo,
            high_hz: hi,
            order: a.order,
            threshold: a.threshold,
            sign: match a.sign {
                SignArg::Neg => PeakSign::Neg,
                SignArg::Pos => PeakSign::Pos,
                SignArg::Both => PeakSign::Both,
            },
            exclude_ms: a.exclude_ms,
        },
        max_seconds: a.max_seconds.filter(|s| *s > 0.0),
        max_times: a.max_times,
    })
}

/// Run `spikes` on one file.
pub fn spikes(reg: &Registry, a: &SpikesArgs) -> Result<SpikesReport> {
    let req = spikes_request(a)?;
    let (_, mut ds) = reg.open(&a.file)?;
    let info = ds.info()?;
    analyze_extracellular(ds.as_mut(), &info, &req)
}

fn render_spikes(r: &SpikesReport) -> String {
    let mut s = format!(
        "{} spikes on {} channels ({}–{} Hz band-pass, {}× noise, {:?})",
        r.spike_count_total,
        r.channels.len(),
        r.settings.low_hz,
        r.settings.high_hz,
        r.settings.threshold,
        r.settings.sign
    );
    for c in &r.channels {
        s.push_str(&format!(
            "\n  {:>4} {:<12} {:>7} spikes  {:>8.3} Hz  noise {:.3} {}",
            c.channel,
            c.name,
            c.spike_count,
            c.rate_hz,
            c.noise,
            c.unit.as_deref().unwrap_or("")
        ));
    }
    for n in &r.notes {
        s.push_str(&format!("\n  note: {n}"));
    }
    s
}

pub fn run_spikes(reg: &Registry, a: &SpikesArgs) -> i32 {
    match spikes(reg, a) {
        Ok(r) => emit(a.json, &r, render_spikes),
        Err(e) => fail(a.json, &e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists() {
        assert_eq!(parse_list("0,2,5-7", "x").unwrap(), vec![0, 2, 5, 6, 7]);
        assert!(parse_list("3-1", "x").is_err());
        assert!(parse_list("a", "x").is_err());
    }
}
