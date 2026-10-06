# Spike features for a folder of recordings

Use this when you have a folder of patch-clamp recordings (ABF, ATF, NWB) and want one table of their features: spike counts, rheobase, f–I slope, input resistance for current clamp, or holding current and access resistance for voltage clamp.

## Run it

```text
$ openreadout batch ephys-features cells/ --set rows=cell --fields clamp_mode,spike_count_total,rheobase_pa,fi_slope_hz_per_pa,input_resistance_mohm
3 data sets (3 ok, 0 failed)
path                                 format  clamp_mode     spike_count_total  rheobase_pa  fi_slope_hz_per_pa  input_resistance_mohm
───────────────────────────────────  ──────  ─────────────  ─────────────────  ───────────  ──────────────────  ─────────────────────
cells/pyabf-171116sh-0018.abf        abf     current_clamp                117           50              0.0655               104.1564
cells/pyabf-190619b-0003.abf         abf     current_clamp                117          120             -0.0538               224.0087
cells/pyabf-2019-07-24-0055-fsi.abf  abf     current_clamp                948           25              0.3717               176.8225
```

`cells/` holds three public current-clamp recordings from the [pyABF](https://github.com/swharden/pyABF) sample data (MIT): `171116sh_0018.abf`, `190619b_0003.abf` and `2019-07-24 0055 fsi.abf`, a fast-spiking interneuron. Each has a series of current steps that drive the cell from silence to repetitive firing. The output on this page is real.

## What it tells you

- Each row is one recording (`rows=cell`). `--fields` keeps the table narrow; without it you also get `max_firing_rate_hz`, `tau_ms`, `sag_ratio` and `resting_mv`. Voltage-clamp files give `holding_current_pa`, `access_resistance_mohm` and `membrane_resistance_mohm` instead.
- `clamp_mode` is decided from the units: a voltage channel with a current command is current clamp, a current channel with a voltage command is voltage clamp.
- Rheobase is the smallest step that makes the cell fire, and the f–I slope is fitted over the steps that do. A negative slope means the rate falls at the larger steps: `190619b_0003` fires 98 spikes at 240 pA and 1 at 420 pA (`--csv fi` shows the curve), so look at it before you average slopes.
- Check the units before you trust a number. If samples are implausible for the channel's unit (a current channel labelled `A` that holds values of a few units, say), the report adds a note naming the likely unit. `openreadout info FILE` shows each channel's unit.
- A file that cannot be analyzed becomes a row with `error` and `error_code`, and the run carries on.

## Variations

### One row per sweep or per spike

`rows=sweep` (the default) gives one row per sweep, with the stimulus window and level, `spike_count`, `firing_rate_hz`, `first_spike_latency_ms`, the ISI measures and the passive or test-pulse values. `rows=spike` gives one row per action potential, with threshold, amplitude, half-width and AHP.

### One file

```bash
openreadout analyze ephys-features cell.abf
openreadout analyze ephys-features cell.abf --csv sweeps > sweeps.csv
openreadout analyze ephys-features cell.abf --csv fi > fi.csv
```

`--csv` prints one tidy table (`sweeps`, `spikes` or `fi`) instead of the report. A spike must cross −20 mV (`--peak-threshold-mv`), and its onset is where dV/dt reaches 10 V/s (`--dvdt-threshold`). `--sweeps 0,3,5-9` limits the sweeps. Stimulus steps, and so rheobase and the f–I curve, come from ABF epoch tables only.

### Grouped by genotype

```bash
openreadout batch ephys-features cells/ -r --set rows=cell \
    --sample-sheet genotypes.csv --by genotype -o cells.parquet
```

OpenReadout picks the join key itself (usually the file name) and reports it, and `--by` adds n, mean, SD, SEM and median per group. `--set peak_threshold_mv=-30` passes an option; the names are those of the MCP tool. Writing a CSV of the table above:

```text
$ openreadout batch ephys-features cells/ --set rows=cell -o cells.csv
...
wrote cells.csv (3 rows × 13 columns, csv, 699 bytes, verified=true)
```

### From an assistant

The MCP tool is `openreadout_ephys_features`, with arguments `trace`, `channel`, `sweeps`, `peak_threshold_mv`, `dvdt_threshold` and `max_spikes`. For a folder, `openreadout_batch` takes `measure: "ephys-features"` with the same options plus `rows`.

## More

- [Electrophysiology](../guides/ephys.md): how every feature is computed, and how it was validated against eFEL and pyABF.
- [Many files](../guides/batch.md): sample sheets, group summaries and output formats.
- [`analyze`](../reference/commands/analyze.md#ephys-features) and [`batch`](../reference/commands/batch.md) references.
- JSON: [`ephys-features`](../reference/json/ephys-features.md), [batch table](../reference/json/batch-table.md).
