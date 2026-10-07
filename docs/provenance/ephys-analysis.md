# Provenance log — electrophysiology analysis

## 2026-09-24 — action potentials, passive properties, test pulse, extracellular detection

**Corpus files used:** `pyabf-171116sh-0018`, `pyabf-2019-07-24-0055-fsi`, `pyabf-190619b-0003`
(current-clamp step protocols), `pyabf-18808025-memtest`, `pyabf-171116sh-0011` (membrane tests),
`pyabf-17o05024-vc-steps` (all MIT, pyABF sample data at commit 3ad9cd6),
`brk-filespec2-3001.ns5`, `intan-test-tetrode-163225.rhd`, `nlx-cheetah-v5-5-1-tet3a.ncs`
(licences in `corpus/manifest.toml`).

**Prior art consulted:**
- pyABF (MIT, https://github.com/swharden/pyABF), read as documentation: `waveform.py`
  (`getEpochWaveformsBySweep`: the holding period of `sweepPointCount/64` points before the first
  epoch, `level + sweep·levelDelta`, `duration + sweep·durationDelta`), `tools/memtestMath.py`
  (its step-based Ra/Rm/Cm, used as an oracle).
- eFEL (LGPL-3.0): only its public documentation page
  (https://efel.readthedocs.io/en/latest/eFeatures.html) was read for feature definitions
  (−20 mV spike threshold, `DerivativeThreshold` 10 V/s for 5 points, `voltage_base` and
  `steady_state_voltage_stimend` windows of 10 %, `sag_ratio1/2`).
  It is run as a black box by `oracle/signal/ephys.py`.
- SpikeInterface (MIT): documentation of `bandpass_filter` and `detect_peaks` (`by_channel`,
  `detect_threshold`, `exclude_sweep_ms`); run as the extracellular oracle.
- Quiroga, Nadasdy, Ben-Shaul, *Unsupervised spike detection and sorting with wavelets and
  superparamagnetic clustering*, Neural Comput. 16 (2004) 1661–1687 (median/0.6745 noise).
- Butterworth band-pass design: textbook bilinear transform (Oppenheim & Schafer).

**What was inferred from what:** the stimulus epoch rule (first step epoch whose level changes
between sweeps) from the corpus protocols (`0113 steps dual -100 to 300 step 25`: epoch 1 carries
the −100 + 25·sweep pA step; epoch 4 repeats it later in the sweep); clamp mode from the units of
the recorded channel and the enabled command output.

## 2026-10-02 — implausible channel units

**Corpus files:** `pyabf-2018-12-09-pclamp11-0001` (pyABF test data, MIT): its recorded channel `IN 0` is labelled `A` while its samples run from −5.4 to 0.2, which in amperes would be a holding current of −3.5 × 10¹² pA. **Prior art consulted:** none.
**What was decided.** The features keep taking the file's unit literally (nothing is rescaled), but when the largest sample of the analysed sweeps exceeds what a cell produces in that unit (10 V for a membrane voltage, 1 µA for a clamp current) the report carries a note that the unit label is probably wrong and what the samples would be in mV or pA.

## 2026-10-06 — NWB unit names

**Corpus files:** `dandi001544-icephys-cc328` and `dandi001544-icephys-cc356` (DANDI 001544, CC0-1.0), `dandi001746-icephys-mfa180730` (DANDI 001746) and `dandi001475-patchseq-rmbe91m1` (DANDI 001475): NWB 2.x files whose `CurrentClampSeries` and `VoltageClampSeries` give their unit as `volts` and `amperes`. **Prior art consulted:** the NWB 2 schema (open standard, https://nwb-schema.readthedocs.io), which fixes the unit of `CurrentClampSeries` to `volts` and of `VoltageClampSeries` to `amperes`.
**What was decided.** `to_mv` and `to_pa` read the spelled-out SI names (`volts`, `millivolts`, `microvolts`, `amperes`, `picoamperes`, `nanoamperes`, and their singular forms, in any case) as well as the symbols, so `analyze ephys-features` finds the membrane voltage or clamp current of an NWB intracellular series instead of refusing it with "a trace with no voltage or current channel". Symbols stay case-sensitive (`mV` is not `MV`). Spike counts and peak times on the DANDI 001544 sweeps were compared with eFEL run as a black box on the samples read with h5py (`oracle/signal/ephys.py`, `NWB_CURRENT_CLAMP`).
