# Bruker EPR

Bruker Xepr (ELEXSYS, EMX) writes BES3T `.DSC`/`.DTA` pairs, and the older WinEPR and ESP software writes `.par`/`.spc` pairs. OpenReadout returns cw spectra, 2D and 3D series and pulse data (DEER, ESEEM) as traces with their field, time or other axes and the acquisition parameters.

Derived from public data sets of many depositors, EasySpin's and DeerLab's readers (MIT) read as documentation, DeerLab's `deerload` run as an independent reader, and the depositors' ASCII exports. Provenance: `docs/provenance/bruker-epr.md`. Crate: `openreadout-epr`.

| format id | files | reads | confidence |
| --- | --- | --- | --- |
| `bruker-bes3t` | `.DSC` + `.DTA` (+ `.XGF`/`.YGF`/`.ZGF` axis files), Xepr | every point of 1D/2D/3D, real or complex, multi-value (harmonics) data sets; field, time or other axes; standard parameters | evidence rubric (`docs/assurance.md`) |
| `bruker-esp` | `.par` + `.spc`, WinEPR and ESP 300/380 | 1D and 2D spectra with the field axis and acquisition parameters | evidence rubric |

Open either member of a pair (`.DSC` or `.DTA`, `.par` or `.spc`); the other is found by its
stem (upper or lower case), and `member_files` lists both (and the axis files).

## BES3T

The descriptor is text: layers `#DESC`, `#SPL`, `#DSL` (device blocks `.DVC name, version`) and
`#MHL` (history; counted, not read). Lines are `KEY value`; a trailing `\` continues a line;
values may be quoted with `'` and lists are comma-separated.

| key | our reading |
| --- | --- |
| `BSEQ` | `BIG`/`LIT` byte order (missing: big-endian with a warning) |
| `IKKF` | one `REAL`/`CPLX` per data value of a point; several values (a multi-harmonic signal channel) become one trace each |
| `IRFMT` (`IIFMT`) | `C` int8, `S` int16, `I` int32, `F` float32, `D` float64; `A` (ASCII) and `0`/`N` (no data) are refused (exit 6) |
| `XPTS`/`YPTS`/`ZPTS` | points per dimension; the `.DTA` must hold values-per-point × XPTS × YPTS × ZPTS numbers (fewer: corrupt; more: a warning) |
| `xTYP` | `IDX` linear (`xMIN` + i·`xWID`/(n−1)); `IGD` values in `.xGF` (format `xFMT`); `NTUP` refused |
| `xNAM`, `xUNI`, `IRNAM`, `IRUNI`, `TITL` | axis labels and units, value labels and units, title |
| `#SPL` | `OPER`, `DATE`/`TIME` (month/day/year; used only when `DSRC` is `EXP`), `CMNT`, `SAMP`, `EXPT` (`CW`, `PLS`), `MWFQ` (Hz), `MWPW` (W), `B0MA` (T), `B0MF` (Hz), `RCAG` (dB), `RCTC` (s), `SPTP` (s), `A1CT`/`A1SW` (T), `AVGS`, `STMP` (K) |

Data layout: x fastest, then y, then z; within a point the values in `IKKF` order, a complex
value as real then imaginary. Multi-harmonic data sets (`IKKF` `REAL,REAL,…`) name each value by
the signal channel's enabled harmonics (`Enable1stHarm`, `Enable1stHarm90`, …, in that order) when
their count matches (`extra.harmonic`, `extra.harmonic_phase_deg`); this naming is inferred.

## ESP / WinEPR

The `.par` file is `KEY value` lines (CR, LF or CRLF). `DOS Format` marks WinEPR (little-endian
float32 `.spc`); otherwise ESP (big-endian int32). `JSS` bit 4 complex, bit 12 two-dimensional.
Points: `SSX`/`SSY` (2D), `ANZ`, `RES`, `REY`, `XPLS` (later ones win). Axis: `GST`+`GSI` (ENDOR,
or when `HCF`/`HSW`/`GST`/`GSI` are all present), `HCF` ± `HSW`/2, or `XXLB`/`XXWI` (2D EMX:
`XYLB`/`XYWI` for the second axis); `JEX` `Time-Sweep`: time `i × RCT/1000` s. The `.spc` must
hold the points the parameters describe.

## What the readers return

- **One trace per data value** (one for almost every file): one sweep per slice of the 2nd/3rd
  dimension; channel `intensity` (real) or `real` and `imaginary` (complex), values as stored
  (no normalization by scans, gain, power or conversion time); an axis file adds channel 0.
  `extra`: `axis` (`{quantity, unit, first, step, last, size, label}`; `irregular`/`channel` for
  an axis file), `kind` = `epr_spectrum`, `data_type` = `EPR SPECTRUM`, `experiment` (`cw`,
  `pls`, or ESP's `JEX`), `sweep_axis`/`sweep_axis_2`, `sweep_table`, `component`, `harmonic`,
  `harmonic_phase_deg`, `axis_from` (ESP: which keys gave the axis).
- **Table `sweeps`** (2D/3D): `sweep`, the y (and z) value of each sweep.
- **Experiment**: vendor Bruker, instrument kind ESR spectrometer, software Xepr/WinEPR/ESP;
  technique cw (CHMO:0000329), pulse (CHMO:0000330) or ESR (CHMO:0000328); parameters
  `microwave_frequency` (GHz), `microwave_power` (mW), `modulation_amplitude` (G),
  `modulation_frequency` (kHz), `receiver_gain` (dB; ESP: as written), `time_constant` (ms),
  `conversion_time` (ms), `center_field`, `sweep_width` (G), `scans`, `temperature` (K);
  operator, sample name, comment, start (local time), method name (the title).
- **Vendor tree**: `bes3t` (layer versions, descriptor, standard and device parameters) or
  `esp_parameters`.

Axis quantities: `magnetic_field` (G, mT, T), `time` (ns, µs, s), `frequency`, `angle`,
`attenuation`, `power`, `temperature`, `x`, `points`.

## Validation

`cargo test -p openreadout-corpus-tests --features corpus` (`tests/series_oracle/mod.rs`,
`oracle/series_oracle.py`): DeerLab's `deerload` (MIT, run as a black box) on every BES3T file with
a single value per point (256 samples of up to three sweeps per file, exactly; its abscissa where
`XMIN` is not negative — `deerload`'s key/value pattern drops a leading minus sign), Xepr's and
the depositors' ASCII exports (field and intensity), and WinEPR's `.asc` export and a depositor's
CSV of a 2D angular sweep (three sweeps) for ESP. One further depositor is held out.

## Vocabulary (public API of `openreadout-epr`)

| identifier | meaning |
| --- | --- |
| `Bes3tReader` | reader of Bruker BES3T data sets |
| `EspReader` | reader of Bruker ESP/WinEPR data sets |
| `BES3T_FORMAT_ID` | `bruker-bes3t` |
| `ESP_FORMAT_ID` | `bruker-esp` |
