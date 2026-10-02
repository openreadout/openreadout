# Gamry Framework `.DTA`

Gamry Framework writes one `.DTA` text file per potentiostat experiment: cyclic and linear sweep voltammetry, chronoamperometry, chronopotentiometry, impedance (EIS) and open-circuit potential. OpenReadout returns every table in the file as a trace and the header parameters as experiment facts. Derived from public files written by an Interface 5000 and from gamry-parser (MIT), read as documentation and used as a reference reader. Provenance: `docs/provenance/gamry-dta.md`. Crate: `openreadout-echem`.

| format id | files | reads | confidence |
| --- | --- | --- | --- |
| `gamry-dta` | `.DTA` beginning `EXPLAIN` / `TAG` (Gamry Framework) | every table (curves, impedance, open-circuit record) and the header parameters | evidence rubric (`docs/assurance.md`) |

(`.dta` is also Stata's extension and Bruker BES3T's data file: the first two lines decide.)

## Layout

Text, tab-separated. Header lines `KEY  TYPE  value…` (`LABEL`, `QUANT`, `IQUANT`, `POTEN`,
`SELECTOR`, `TOGGLE`, `TWOPARAM`, `PSTAT`; `NOTES` is followed by its count of note lines).
Tables: `KEY  TABLE  [points]`, a row of column names, a row of units, tab-indented rows.
`CURVE`, `CURVE1` … `CURVEn` (the cycles of a CV) share one trace, one sweep each; `ZCURVE` is the
impedance table, `OCVCURVE` the open-circuit record before the run; other tables keep their name.
Numbers in the PC's locale (a decimal comma is accepted).

| column | our channel | unit |
| --- | --- | --- |
| `Pt` | `point` | — |
| `T`, `Time` | `time` | s |
| `Vf` | `potential` | V (vs. reference) |
| `Im` | `current` | A |
| `Vu` | `potential_uncompensated` | V |
| `Sig` | `signal` | V |
| `Ach` | `aux_channel` | V |
| `IERange` | `current_range` | — |
| `Cycle` | `cycle` | — |
| `Freq` | `frequency` | Hz |
| `Zreal`, `Zimag` | `z_real`, `z_imag` | Ω |
| `Zsig` | `z_signal` | V |
| `Zmod`, `Zphz` | `z_modulus`, `z_phase` | Ω, ° |
| `Idc`, `Vdc` | `current_dc`, `potential_dc` | A, V |
| `Vm` | `potential_measured` | V |
| `Temp` | `temperature` | °C |
| `Q` | `charge` | C |

Other columns keep their name in lower case with the file's unit. Text columns (`Over`, the
overload flags) are left out (info finding `text_columns`). `extra.label` holds Gamry's name and
`extra.file_unit` its unit text.

## What the reader returns

One trace per table group, `time` as channel 0 (irregular abscissa) when the table has one.
Experiment: vendor Gamry Instruments, potentiostat, Gamry Framework; `TAG` as the `experiment`
parameter and technique (CV, LSV/POLDYN, CHRONOA, CHRONOP, EISPOT, EISGALV, CORPOT mapped to
CHMO terms); `TITLE` as method name; `PSTAT` as the instrument serial; notes as comment;
`DATE`/`TIME` as start (month/day/year, local time); `AREA` (cm²) and `SCANRATE` (mV/s). Vendor
tree `gamry`: every header line.

## Validation

`tests/series_oracle/mod.rs`: gamry-parser 0.4.6 (MIT, run as a black box) returns the same values
for every numeric column of every curve (and the OCV curve), exactly. One further depositor is
held out.

## Vocabulary (public API of `openreadout-echem`, Gamry)

| identifier | meaning |
| --- | --- |
| `GamryReader` | reader of Gamry `.DTA` files |
| `GAMRY_FORMAT_ID` | `gamry-dta` |
