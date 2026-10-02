# Spike features for a folder of recordings

Use this when you have a folder of patch-clamp recordings (ABF, ATF, NWB) and want one table of their features: spike counts, rheobase, f–I slope, input resistance for current clamp, or holding current and access resistance for voltage clamp.

## Run it

```text
$ openreadout batch ephys-features cells/ --set rows=cell --fields clamp_mode,spike_count_total,holding_current_pa
3 data sets (2 ok, 1 failed)
path                                      format  clamp_mode     spike_count_total  holding_current_pa  error                                     error_code
────────────────────────────────────────  ──────  ─────────────  ─────────────────  ──────────────────  ────────────────────────────────────────  ───────────────────
cells/pyabf-2018-12-09-pclamp11-0001.abf  abf     voltage_clamp                  0           -3.482e12  -                                         -
cells/pyabf-model-vc-step.atf             atf     voltage_clamp                  0            -139.648  -                                         -
cells/pyabf-sine-sweep-magnitude-20.atf   atf     -                              -                   -  ephys-analysis: unsupported feature: a …  unsupported_feature
```

`cells/` holds the three public pyABF sample recordings committed at [`fuzz/corpus/whole_abf/`](../../../fuzz/corpus/whole_abf/) and [`fuzz/corpus/whole_atf/`](../../../fuzz/corpus/whole_atf/). Two are voltage-clamp recordings and one is a sine-sweep test file, so there are no spikes to report; on current-clamp recordings the same command fills the spike, rheobase and f–I columns. The output on this page is real.

## What it tells you

- Each row is one recording (`rows=cell`). `--fields` keeps the table narrow; without it you also get `access_resistance_mohm` and `membrane_resistance_mohm` here. Current-clamp files add `rheobase_pa`, `fi_slope_hz_per_pa`, `max_firing_rate_hz`, `input_resistance_mohm`, `tau_ms`, `sag_ratio` and `resting_mv`.
- `clamp_mode` is decided from the units: a voltage channel with a current command is current clamp, a current channel with a voltage command is voltage clamp.
- Check the units before you trust a number. The ABF file labels its input channel `A` (amperes) while its values are a few units, so the holding current comes out as −3.482 × 10¹² pA. `openreadout info FILE` shows each channel's unit.
- A file that cannot be analyzed becomes a row with `error` and `error_code`, and the run carries on. The sine-sweep ATF has no voltage or current unit, so it is `unsupported_feature`.

## Variations

### One row per sweep or per spike

`rows=sweep` (the default) gives one row per sweep, with the stimulus window and level, `spike_count`, `firing_rate_hz`, `first_spike_latency_ms`, the ISI measures and the passive or test-pulse values. `rows=spike` gives one row per action potential, with threshold, amplitude, half-width and AHP.

### One file

```bash
openreadout analyze ephys-features cell.abf
openreadout analyze ephys-features cell.abf --csv sweeps > sweeps.csv
openreadout analyze ephys-features cell.abf --csv fi > fi.csv
```

`--csv` prints one tidy table (`sweeps`, `spikes` or `fi`) instead of the report. A spike must cross −20 mV (`--peak-threshold`), and its onset is where dV/dt reaches 10 V/s (`--dvdt-threshold`). `--sweeps 0,3,5-9` limits the sweeps. Stimulus steps, and so rheobase and the f–I curve, come from ABF epoch tables only.

### Grouped by genotype

```bash
openreadout batch ephys-features cells/ -r --set rows=cell \
    --sample-sheet genotypes.csv --by genotype -o cells.parquet
```

OpenReadout picks the join key itself (usually the file name) and reports it, and `--by` adds n, mean, SD, SEM and median per group. `--set peak_threshold_mv=-30` passes an option; the names are those of the MCP tool. Writing a CSV of the table above:

```text
$ openreadout batch ephys-features cells/ --set rows=cell -o cells.csv
...
wrote cells.csv (3 rows × 9 columns, csv, 472 bytes, verified=true)
```

### From an assistant

The MCP tool is `openreadout_analyze` with `kind: "ephys-features"` and options `trace`, `channel`, `sweeps`, `peak_threshold_mv`, `dvdt_threshold` and `max_spikes`. For a folder, `openreadout_batch` takes `measure: "ephys-features"` with the same options plus `rows`.

## More

- [Electrophysiology](../guides/ephys.md): how every feature is computed, and how it was validated against eFEL and pyABF.
- [Many files](../guides/batch.md): sample sheets, group summaries and output formats.
- [`analyze`](../reference/commands/analyze.md#ephys-features) and [`batch`](../reference/commands/batch.md) references.
- JSON: [`ephys-features`](../reference/json/ephys-features.md), [batch table](../reference/json/batch-table.md).
