# MicroCal ITC `.itc`

MicroCal isothermal titration calorimeters (VP-ITC, iTC200, and PEAQ-ITC through its `.itc` export) save each run as an `.itc` raw data file. OpenReadout returns the thermogram (differential power and cell temperature over time), the injection table and the run settings: concentrations, cell volume, temperature, stirring and reference power. Derived from public files of four depositors (VPViewer2000, ITC200 and MicroCal ITC software), Origin's raw and integrated-heat exports of some of them, and the depositors' notebooks and file names. Provenance: `docs/provenance/microcal-itc.md`. Crate: `openreadout-biophys`.

| format id | files | reads | confidence |
| --- | --- | --- | --- |
| `microcal-itc` | `.itc` raw data files of MicroCal VP-ITC, iTC200 and the MicroCal ITC software (also what PEAQ-ITC exports as `.itc`) | the thermogram, the injections, concentrations, cell volume, temperature, stirring, reference power, instrument and software | medium (evidence rubric, `docs/assurance.md`) |

Not read: PEAQ-ITC native `.apitc` files and analysis projects (`.apj`), Origin projects (`.opj`),
NITPIC/SEDPHAT files. Integrated injection heats are the analysis software's (its baseline and
integration) and are not stored in `.itc` files.

## Layout

Text (CRLF; Latin-1 is accepted). Line prefixes:

| prefix | lines | our reading |
| --- | --- | --- |
| `$` | `ITC`; the number of injections; `NOT` (kept verbatim); the target temperature (°C); the initial delay (s); the stirring speed (rpm); the reference power (µcal/s); a code (kept); `ADCGainCode: n`; three booleans (kept); then one line per injection: volume (µl), duration (s), spacing (s), filter period (s) | method |
| `#` | 0; syringe concentration (mM); cell concentration (mM); cell volume (ml); temperature (°C); two further numbers (kept) | concentrations, cell volume |
| `?` | starts the operator's comment, which runs until the first `%` line | comment |
| `%` | the instrument identifier (`VPITC…`, `ITC200_…`, `MICROCALITC_…`); the cell volume; calibration numbers (kept verbatim, not interpreted); last, the software and version (`VPViewer2000 Ver: 1.4.8`, `ITC200 Ver: 1.26.1`, `MicroCalITC Ver: 1.29.32 Run time:…`) | instrument, software |
| `@0` | starts the pre-injection baseline | |
| `@n,v[,d[, t]]` | starts injection n's data block (volume v µl, duration d s, and in newer files the time t s the injection started) | injection table |
| digits | a data row: time (s), differential power (µcal/s), cell temperature (°C), and in newer files 4 or 6 further columns | thermogram |

## What the reader returns

- **Trace 0 `thermogram`** (one sweep): channel 0 `differential_power` (µcal/s), channel 1
  `cell_temperature` (°C), then `column_4` … `column_9` (unnamed further columns, no unit) when the
  file has them. Sampling is uniform in every file seen (2 s, 1 s or 5 s): `sample_rate_hz` and
  `start_s` give the time (`extra.axis` = time in s); if a file's time column is uneven it becomes
  channel 0 `time` (s) instead and `check` says so. `extra.kind` = `thermogram`.
- **Table 0 `injections`**: `injection` (1-based), `volume` (µL), `duration` (s), `spacing` (s),
  `filter_period` (s), `start` (s: the block line's time, else the block's first sample time),
  `first_sample` (index into the trace).
- **Experiment**: instrument vendor Malvern Panalytical (MicroCal), model `VP-ITC`, `iTC200` or
  `MicroCal ITC` from the identifier, kind calorimeter (OBI:0000930), software and version; method
  technique isothermal titration calorimetry (CHMO:0000683) with `cell_concentration`,
  `syringe_concentration` (mM), `cell_volume` (mL), `temperature` (°C), `initial_delay` (s),
  `stirring_speed` (rpm), `reference_power` (µcal/s), `injections`; `acquisition.comment` (the `?`
  text) and `duration_s` (the last sample's time). The `Run time` text of the MicroCal ITC software
  line is kept in the vendor tree (local time without a zone, not an acquisition time).
- `check`: the declared number of injections against the listed injection lines and the data
  blocks (a run that stopped early is a warning), unparseable lines, non-finite values.

## Validation

`cargo test -p openreadout-corpus-tests --features corpus` (`tests/itc_oracle/mod.rs`): time and
differential power equal Origin's raw exports of three runs at 256 rows each (to the export's five
decimals, over 1,350-6,062 samples), the injection volumes and cell concentration equal Origin's
integrated-heat tables of four runs, and the concentrations match the depositors' file names and
notebooks for three; the other files are checked for internal consistency. One further depositor's
file is held out.

## Vocabulary (public API of `openreadout-biophys`, ITC)

| identifier | meaning |
| --- | --- |
| `ItcReader` | reader of MicroCal `.itc` files |
| `ITC_FORMAT_ID` | `microcal-itc` |
| `ItcDataset` | the dataset the reader returns |
