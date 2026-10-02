# TA Instruments data files (Universal Analysis, `.001`)

TA Instruments' Q-series control software (Thermal Advantage) writes numbered data files (`.001`, `.002`, …) that Universal Analysis opens. OpenReadout returns every stored signal as one trace against time, with the instrument, operator, sample and method steps.

Derived from public DSC Q20 files of two depositors and their Universal Analysis exports. Provenance: `docs/provenance/ta-universal-analysis.md`. Crate: `openreadout-thermal`.

| format id | files | reads | confidence |
| --- | --- | --- | --- |
| `ta-universal-analysis` | `.001`, `.002`, … written by TA Instruments' Q-series control software (Thermal Advantage), read by Universal Analysis | every stored signal, instrument, serial, operator, sample, size, date, method steps | evidence rubric (`docs/assurance.md`) |

Not read: TRIOS `.tri` files (a different container).

## Layout

UTF-16LE with a byte-order mark. A header of `key value` lines (CR LF), beginning `CLOSED` (a
finished run) or `OPEN`: `VERSION`, `Instrument` (model and firmware, `DSC Q20 V24.11 Build
124`), `Module`, `Operator`, `File`, `InstSerial`, `Sample`, `Size` (value and unit), `Method`,
`Comment`, `Xcomment`, calibration lines (`Kcell`, `Calib`, `TempCal`, …), `Nsig`, `Sig1` …
`SigN` (`Time (min)`, `Temperature (°C)`, `Heat Flow (mW)`, …), `Date`, `Time`, `OrgMethod`
(one per method step). A form feed (U+000C) ends it.

Then one byte, the signal count (equal to `Nsig`; a file where it differs is refused), and records
of `Nsig` float32 values (little-endian) in `Sig` order, until the end-of-data record (first value
−100.0). A file without that record is read to its last whole record with a warning
(`no_end_record`); a run without records is read as empty (`no_records`).

## What the reader returns

- **One trace** `run`: channel 0 `time` (s; the file stores minutes), then every signal in file
  order, named from its label (`temperature`, `heat_flow`, `weight`, `rev_heat_flow`,
  `sample_purge_flow`, …: lower case, words joined by `_`) with the unit in its label; the label
  itself in `extra.label`. Values as stored (float32).
- **Experiment**: vendor TA Instruments, model and firmware from `Instrument`, serial, operator,
  sample name, `sample_mass` (from `Size` in mg), comment, the start (`Date` and `Time`, the
  instrument's local time), `method_steps`, duration; technique DSC, thermogravimetry or thermal
  analysis from the model (`DSC …`, `TGA …`, `SDT …`).
- **Vendor tree** `ta`: every header line.

## Validation

`tests/series_oracle/mod.rs`: the depositors' Universal Analysis exports of six runs: every exported
signal (matched by label) at 256 rows equals ours to the export's printed precision (rows matched
by time; the modulated run's export is resampled, compared by interpolation within 0.3 % of the
column's range), and the sample, operator, size and start in the export header.

## Vocabulary (public API of `openreadout-thermal`, TA Instruments)

| identifier | meaning |
| --- | --- |
| `TaReader` | reader of TA Instruments data files (`.001`, …) |
| `TA_FORMAT_ID` | `ta-universal-analysis` |
