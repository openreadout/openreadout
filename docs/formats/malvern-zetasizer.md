# Malvern Zetasizer `.dts`

The Malvern Zetasizer software (Nano, Pro and Ultra) saves dynamic light scattering and zeta potential measurements as `.dts` files. OpenReadout returns one table row per size or zeta record: sample, time, temperature and the results the software computed (Z-average, PDI, peaks, zeta potential, mobility, conductivity). No prior art exists for this binary format. It was derived from public files of several depositors, one of whom also published the Zetasizer's own table of zeta results. Provenance: `docs/provenance/malvern-zetasizer.md`. Crate: `openreadout-biophys`.

| format id | files | reads | confidence |
| --- | --- | --- | --- |
| `malvern-zetasizer-dts` | `.dts` measurement files of the Zetasizer software (Nano, Pro, Ultra; versions 7.02-8.01) | size and zeta records: header, sample, results | evidence rubric (`docs/assurance.md`) |

Not read: ZS Xplorer `.zmes` files; the size distributions, correlation functions, zeta
distributions and phase plots stored in each record; molecular-weight and other record kinds
(listed only).

## Layout

A compound file ([MS-CFB]):

- `Header`: `u16` 1, a 16-byte identifier (`c0 fe e4 a2 b2 26 d6 11 99 1b 00 90 27 9b 77 0c`),
  `u16` 1, `u32` the next record number.
- `Deleted`: the deleted records.
- `REC<n>`: one stream per record, `n` its record number.

A record is a sequence of fields without tags. Strings are `u32` byte length, `0x01`, UTF-16LE
ending in NUL.

| offset | field |
| --- | --- |
| 0 | `u16` record schema (10 in software 7.02; 13 in 7.12-8.01) |
| 2 | `u16` kind: 1 size, 2 zeta |
| 12 | `u32` record number |
| 16 | string: software version (`7.13`, `8.00.4813`) |
| then | `f64` measurement date: OLE Automation days, local time |
| then | string: instrument serial (`MAL…`) |
| then | `u32`, `f32`, `f32` measured temperature (°C) |

Further on: the operator, the SOP path, the dispersant (one or more strings with their
properties), the material and a 46-byte block that begins `01 00 00 00 01 00` and ends
`02 00 00 00 01 00 00 02 00 00 00 01 00 00`, then the **sample name**. The reader takes the
string after the first material block of that shape.

**Size results** are located by their structure: `f32` Z-average (d.nm), `f32` PdI, `u32` n and
n `f32` (a fit of the correlation function), then for the intensity, number and volume
distributions in turn: `u32` k + k peak means (d.nm), `u32` k + k areas (%), `u32` k + k widths
(d.nm); a distribution's areas sum to 100.

**Zeta results**: `u32` 5 + 5 `f32` zeta peak areas (%, largest first), `u32` 5 + 5 zeta peak
means (mV), `f64` conductivity (mS/cm), then ten `f32`: applied voltage (V), zeta potential
(mV), zeta deviation (mV), mobility (µm·cm/V·s), mobility deviation, five not identified; then
`u32` 5 + 5 zeta peak widths and the mobility peak areas, means and widths (`u32` 5 + 5 each).

A result block is returned only when exactly one block of that structure is in the record; a
record with none (an aborted measurement) or several is listed without results and with a
finding.

## What the reader returns

Table `records`, one row per record in record-number order:

| column | unit | |
| --- | --- | --- |
| `record` | — | record number |
| `kind` | — | `size`, `zeta` or `other` |
| `sample_name`, `measured_at` | — | ISO local time |
| `temperature` | °C | measured |
| `z_average`, `pdi` | nm, — | size records |
| `peak1_mean` … `peak3_width` | nm, %, nm | intensity peaks (mean, area, width) |
| `zeta_potential`, `zeta_deviation` | mV | zeta records |
| `mobility`, `mobility_deviation` | µm·cm/(V·s) | |
| `conductivity` | mS/cm | |
| `voltage` | V | applied voltage (inferred) |
| `zeta_peak1_mean` … `zeta_peak3_width` | mV, %, mV | zeta peaks |

Table `peaks`: every stored peak (`record`, `distribution` intensity/number/volume/zeta/mobility,
`peak`, `mean`, `area`, `width`, `unit`).

Experiment: vendor Malvern Panalytical, model Zetasizer, serial and software version (first
record), start and end (earliest and latest record), the sample name when every record has the
same one, technique dynamic light scattering (all records size) or zeta-potential measurement
(all zeta).

The export's number mean, volume mean and diffusion coefficient are computed by the software from
the distributions and the Z-average; they are not stored and not returned.

## Validation

`tests/series_oracle/mod.rs` against the depositor's Zetasizer results table (`oracle/
zetasizer_oracle.py`): 36 zeta records (software 7.12) — record, sample name, date (within the
export's second), temperature, zeta potential, mobility and conductivity — agree within the
export's rounding, 252 cells. The other files (7.02, 7.13, 8.00, 8.01) are read for structure:
every record's header walks and every size record with results has exactly one block of the size
structure.

**Size results are withheld.** The size-result layout above was mapped on one depositor's size
records whose export matched it (Z-average, PdI, peaks), but that record is the source of a
held-out file of another format and cannot be development evidence; no other public export of
size records was found. Until one is in the corpus, `z_average`, `pdi` and the intensity peaks
are NaN and a finding (`size_results_withheld`) says so; the records, samples, dates and
temperatures of size records are returned.

## Vocabulary (public API of `openreadout-biophys`, Zetasizer)

| identifier | meaning |
| --- | --- |
| `ZetasizerReader` | reader of Zetasizer `.dts` files |
| `ZETASIZER_FORMAT_ID` | `malvern-zetasizer-dts` |
| `FORMAT_ID` | `malvern-zetasizer-dts` (module constant) |
| `Peaks` | one distribution's peaks: `mean`, `area`, `width` |
| `Size` | size results: `z_average`, `pdi`, `peaks` |
| `Zeta` | zeta results: `zeta`, `zeta_deviation`, `mobility`, `mobility_deviation`, `conductivity`, `voltage`, `zeta_peaks`, `mobility_peaks` |
| `Record` | one record: `stream`, `schema`, `kind`, `number`, `version`, `measured_at`, `serial`, `temperature`, `sample`, `size`, `zeta`, `matches` |
