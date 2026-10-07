# Electrophysiology analysis

`openreadout analyze ephys-features` measures patch-clamp recordings (action potentials, rheobase, input resistance and membrane-test values), and `openreadout analyze spikes` detects spikes in extracellular recordings. The MCP tools `openreadout_ephys_features` and `openreadout_spikes` run the same analyses. Both work on top of the electrophysiology readers (ABF, ATF, NWB, Neuralynx, Blackrock, SpikeGLX, Intan, Plexon). This page describes how each value is computed and lists the analysis vocabulary.

Code: `crates/openreadout-signal/src/ephys/`. Provenance:
[`docs/provenance/ephys-analysis.md`](../provenance/ephys-analysis.md).

## `analyze ephys-features` (patch clamp)

**Channel and mode.** Default channel: the first whose unit is a voltage (mV, V, µV), else the
first current channel (pA, nA, µA, A). Voltage channel + current command = `current_clamp`;
current channel + voltage command = `voltage_clamp`; same kind for both = `unknown` (analysed as
recorded).

**Stimulus.** From the ABF epoch table (`info` → `traces[].extra.outputs`): the first output with
its waveform enabled and epochs. Sweep timing: the first `floor(samples/64)` samples hold, then the
epochs follow, epoch `e` lasting `duration + sweep·duration_step` samples at
`level + sweep·level_step` (pyABF's documented convention). The stimulus epoch is the first step
epoch whose level changes between sweeps, else the first step that differs from holding.
Recordings without an epoch table (gap-free, other formats) get spike and sweep features only.

**Spikes** (per sweep, whole sweep): an upward crossing of `--peak-threshold-mv` (−20 mV) starts a
spike, the next downward crossing ends it, the peak is the maximum in between (eFEL's
`Spikecount` definition). Onset (threshold): first sample after the previous spike's AHP minimum
from which dV/dt ≥ `--dvdt-threshold` (10 V/s) for 5 samples. Per spike: `peak_time_s`, `peak_mv`,
`threshold_time_s`, `threshold_mv`, `amplitude_mv` (peak − onset), `half_width_ms` (at the voltage
half way between onset and peak), `rise_time_ms` (onset → peak), `decay_time_ms` (peak → back to
the onset voltage), `ahp_mv` (minimum to the next spike's crossing or the sweep end),
`ahp_depth_mv` (onset − AHP), `upstroke_v_per_s`, `downstroke_v_per_s`, `isi_ms`.

**Per sweep:** `spike_count`, `spike_count_stimulus` (peaks inside the stimulus),
`firing_rate_hz` (= in-stimulus count / stimulus duration), `first_spike_latency_ms` (first peak
− stimulus onset), `isi_mean_ms`, `isi_cv`, `adaptation_index` = mean of
(ISIₙ₊₁ − ISIₙ)/(ISIₙ₊₁ + ISIₙ) over the in-stimulus ISIs, `half_width_mean_ms`,
`amplitude_mean_mv`; `passive` (`baseline_mv` = mean of the last 10 % before the stimulus,
`steady_state_mv` = mean of the last 10 % of the stimulus, `extreme_mv`,
`input_resistance_mohm` = (steady − baseline)/ΔI, `tau_ms` (single exponential fitted from the
onset to the voltage extreme), `sag_ratio` = (steady − min)/(baseline − min) (eFEL `sag_ratio1`),
`steady_fraction` = (baseline − steady)/(baseline − min) (eFEL `sag_ratio2`)); spikes inside the
stimulus void the passive measures of that sweep.

**Per cell:** `rheobase_pa` (smallest positive step with ≥ 1 in-stimulus spike) and
`rheobase_sweep`; `fi_curve`; `fi_slope_hz_per_pa` (least squares over firing steps);
`max_firing_rate_hz`; `input_resistance_mohm` (slope of steady − baseline against the step over
spike-free negative steps); `tau_ms` (median); `capacitance_pf` = τ/R_in; `sag_ratio` (the most
negative step); `resting_mv` (median baseline).

**Voltage clamp** (`test_pulse`, per sweep): `holding_current_pa` (baseline current),
`peak_current_pa` (extreme in the first 2 ms of the step − baseline), `steady_current_pa`,
`total_resistance_mohm` = ΔV/steady, `tau_ms` (single exponential decaying to the measured
steady state, least squares, fitted from where the transient has fallen to 90 % of its height to
where it first reaches the steady state, at most 50 ms or half the step), `access_resistance_mohm`
= ΔV/I0 with I0 the fit extrapolated back to the transient's apex, `membrane_resistance_mohm` =
total − access, `capacitance_pf` = Q/ΔV·((Ra + Rm)/Rm)² with Q the charge of the transient above
the steady state from the step onset to its first return to the steady state (exact for one
compartment behind a series resistance); per cell the medians.

`--csv sweeps|spikes|fi` prints one tidy table instead of the JSON report.

## `analyze spikes` (extracellular)

Per channel and sweep: Butterworth band-pass (`--band-hz 300:6000`, `--order 5`; the high edge is
clamped to 0.45 × the sampling rate), zero phase (forward-backward with odd-reflection padding and
steady-state initial conditions); noise = median(|x|)/0.6745 of the filtered sweep; a spike is a
sample beyond `--threshold` (5) × noise in the `--sign` direction (`neg`) that is the extreme within
`--exclude-ms` (0.1 ms) on both sides. Output: `spike_count`, `rate_hz`, `noise`, `threshold`,
`duration_s` and up to `--max-times` spike times per channel.

## Validation (automated in `crates/openreadout-corpus-tests/tests/ephys_analysis.rs`)

Reference readers: `oracle/signal/ephys.py` → `corpus/oracle/ephys-analysis/*.json` (eFEL 5.7 as a black box,
pyABF memtest, Neo + SpikeInterface 0.105). Results:

| data | against | result |
| --- | --- | --- |
| `pyabf-171116sh-0018`, `-2019-07-24-0055-fsi`, `-190619b-0003` (current-clamp steps; 44 sweeps, 1182 spikes) | eFEL | spike counts per sweep and inside the stimulus identical in 44/44 sweeps; stimulus windows and step currents identical; rheobase identical (50, 25, 120 pA); input resistance within 0.03 % (104.16 vs 104.14, 176.82 vs 176.77, 224.01 vs 223.99 MΩ); sag ratio within 0.0002; baseline within 0.1 mV, steady state within 0.03 mV; 1160/1182 peaks at the same or the neighbouring sample (differences on flat-topped peaks); median onset voltage difference 0.5–1.0 mV (different onset rules); median half-width difference 3 %; time constant median difference 10–17 % (different fit windows) |
| `pyabf-18808025-memtest`, `pyabf-171116sh-0011` (voltage-clamp membrane tests) | pyABF memtest | holding current within 0.2 pA; ΔV/ΔI steady-state resistance within 4 % (1922.6 vs 2002.2, 95.9 vs 97.1 MΩ); access resistance within 8 % (28.7 vs 26.8, 18.3 vs 16.9 MΩ); capacitance 24.0 vs 18.8 and 237.6 vs 150.8 pF — pyABF uses τ/Ra, which ignores Rm and the membrane current; ours integrates the transient's charge (exact on a synthetic RC cell, unit-tested) |
| `brk-filespec2-3001.ns5` (10 ch), `intan-test-tetrode-163225.rhd` (4 ch), `nlx-cheetah-v5-5-1-tet3a.ncs` | SpikeInterface bandpass + detect_peaks | spike counts identical on all 15 channels (3,772 spikes), every SpikeInterface spike matched within 0.1 ms, noise equal to 4 digits |

**Performance** (release build): `analyze ephys-features` on `pyabf-2019-07-24-0055-fsi`
(17 sweeps × 60,000 samples, 948 spikes) 0.02 s, 12 MB; `analyze spikes` on `brk-filespec2-3001.ns5`
(10 channels × 900,300 samples at 30 kHz) 0.47 s, 168 MB; on `intan-rhd-test-1.rhd` (192
channels × 30,000 samples) 0.30 s, 91 MB; on `figshare30728969-ns6-50mw002.ns6` (65 channels ×
1.8 million samples at 30 kHz, 234 MB) 21 s, 1.0 GB. `analyze spikes` reads the channels in groups
of at most 512 MiB of samples (`SPIKES_READ_BYTES`) and filters one channel at a time, and readers
are asked for at most 8 Mi values per call (`openreadout_core::trace::READ_VALUES`), so memory no
longer grows with the channel count; `--max-seconds` bounds the length analysed.

## Known gaps

- Stimulus windows come from ABF epoch tables only; NWB icephys stimulus series and ATF files
  give spike features without a stimulus.
- Ramps, trains and user-list protocols are not used for rheobase/f–I.
- Spike detection is a voltage-crossing rule; spikes that do not reach −20 mV (depolarisation
  block, dendritic recordings) need `--peak-threshold-mv`.
- Extracellular detection reads each sweep whole (`--max-seconds` limits it); no whitening,
  common-average referencing or sorting.

## Vocabulary (public identifiers of `crates/openreadout-signal/src/ephys/*.rs`)

| identifier | meaning |
| --- | --- |
| `to_mv`, `to_pa` | unit factors to mV and pA |
| `Protocol`, `output`, `unit`, `holding`, `from_trace`, `sweep_epochs`, `stimulus` | the command output's epoch table |
| `SweepEpoch`, `index`, `kind`, `start_sample`, `end_sample`, `level` | one epoch placed in a sweep |
| `ApSettings`, `peak_threshold_mv`, `dvdt_threshold_v_per_s`, `dvdt_window_samples` | spike detection settings |
| `Spike`, `sweep`, `peak_sample`, `peak_time_s`, `peak_mv`, `threshold_time_s`, `threshold_mv`, `threshold_from_dvdt`, `amplitude_mv`, `half_width_ms`, `rise_time_ms`, `decay_time_ms`, `ahp_mv`, `ahp_depth_mv`, `upstroke_v_per_s`, `downstroke_v_per_s`, `isi_ms` | one action potential |
| `detect`, `adaptation_index` | spike detection, ISI adaptation |
| `StepResponse`, `baseline_mv`, `steady_state_mv`, `extreme_mv`, `input_resistance_mohm`, `tau_ms`, `sag_ratio`, `steady_fraction`, `current_step` | passive response to a current step |
| `TestPulse`, `delta_mv`, `holding_current_pa`, `peak_current_pa`, `steady_current_pa`, `access_resistance_mohm`, `membrane_resistance_mohm`, `total_resistance_mohm`, `capacitance_pf`, `test_pulse` | voltage-clamp test pulse |
| `base_and_steady`, `fit_exponential`, `fit_decay` | measurement windows and the exponential fits (free asymptote; decay to zero) |
| `CellRequest`, `trace`, `channel`, `sweeps`, `ap`, `max_spikes` | `analyze ephys-features` request |
| `CellReport`, `path`, `format`, `clamp_mode`, `sample_rate_hz`, `settings`, `cell`, `spike_count_total`, `spikes`, `spikes_truncated`, `notes` | `analyze ephys-features` output |
| `ChannelRef`, `name` | the channel analysed |
| `StimulusInfo`, `epoch` | the stimulus found |
| `SweepFeatures`, `stimulus_start_s`, `stimulus_end_s`, `stimulus_pa`, `stimulus_mv`, `spike_count`, `spike_count_stimulus`, `firing_rate_hz`, `first_spike_latency_ms`, `isi_mean_ms`, `isi_cv`, `half_width_mean_ms`, `amplitude_mean_mv`, `passive` | one sweep |
| `FiPoint`, `current_pa`, `rate_hz` | f–I curve point |
| `CellFeatures`, `resting_mv`, `rheobase_pa`, `rheobase_sweep`, `fi_curve`, `fi_slope_hz_per_pa`, `max_firing_rate_hz`, `membrane_capacitance_pf` | per-cell summary |
| `analyze_cell`, `analyze_extracellular` | entry points |
| `SpikesRequest`, `channels`, `max_seconds`, `max_times` | `analyze spikes` request |
| `SpikesReport`, `ChannelSpikes`, `noise`, `threshold`, `duration_s`, `times`, `times_truncated`, `SpikeTime`, `sample`, `time_s`, `amplitude` | `analyze spikes` output |
| `SPIKES_READ_BYTES` | most bytes of samples `analyze spikes` reads at once |
| `DetectSettings`, `low_hz`, `high_hz`, `order`, `sign`, `exclude_ms`, `PeakSign` { `Neg`, `Pos`, `Both` } | extracellular detection settings |
| `mad_noise`, `bandpass`, `detect_peaks` | extracellular detection steps |
| `Section`, `bandpass_sos`, `filtfilt` | Butterworth band-pass and zero-phase filtering |
| `EphysQuery`, `SpikesQuery`, `NmrQuery`, `band_hz`, `baseline_mode`, `DEFAULT_MAX_SPIKES` | JSON queries shared by the MCP tools and batch tables (`api.rs`): the tool arguments by name, `request()` builds the request; `DEFAULT_MAX_SPIKES` is the per-spike rows returned by default (25) |
