# Agilent Seahorse XF `.asyr`

Seahorse Wave saves each Seahorse XF assay as an `.asyr` result file. OpenReadout returns the per-well O2 and pH sensor emissions, the plate map, injections and run metadata, and on standard 96-well plates the O2 and pH levels and the OCR, ECAR and PER rates.

Derived from public assay files of several depositors, checked against Wave's own exported rates (Harvard Dataverse doi:10.7910/DVN/D5TSFG, CC0) and a Wave Excel export (seahtrue's test workbook, Artistic-2.0). Provenance: `docs/provenance/agilent-seahorse.md`. Crate: `openreadout-biophys`.

| format id | files | reads | confidence |
| --- | --- | --- | --- |
| `agilent-seahorse-asyr` | `.asyr` assay results written by Seahorse Wave | per-well O2 and pH sensor emissions at every plate reading, the plate map, measurements, injections, executed protocol, instrument serial, software version, operator and run times; on standard 96-well plates also the O2 and pH levels and the OCR, ECAR and PER per well and measurement | generated (`book/src/reference/evidence.md`) |

Not read: assay templates (`.asyt`); Wave's Excel exports (read them with seahtrue); rates of
plates whose compartment constants were never compared with Wave (24-well plates, spheroid
plates), of other O2 methods than AKOS, and of files with the Ksv leak/temperature or pH
temperature corrections on — those files return the emissions only, with a note saying why.

## Rates (OCR, ECAR, PER)

`.asyr` files do not store rates; Wave computes them from the emissions. The reader computes
them the same way, from the constants the file stores and the published method (Gerencser et
al., "Quantitative microplate-based respirometry with correction for oxygen diffusion", Anal.
Chem. 2009, 81, 6868, and its Supporting Information; Wave calls the method AKOS):

1. **O2 level** (mmHg, trace 3) per reading and well:
   `CO + (FO/Ksv)·(1/F − 1/F_T)`, `F` the corrected emission, `F_T` the mean corrected emission of
   the background wells at that reading (`O2DataModifiers/FO`, `Ksv`, `CO`). This background
   normalisation is Wave's background correction for OCR; nothing is subtracted later.
2. **OCR** (pmol/min) per well and measurement: the paper's corrected OCR(t) (eqs. 13-14) with
   `O2DataModifiers/Plate/{TauAC, TauAW, TauW, TauC, TauP}`, the stored Savitzky-Golay kernels
   (`SGA1`, `SGB1`, `SGA2`, `SGB2`, `Smoothing`: 7-point quadratic; the reader refuses any other),
   d²[O2]/dt² taken as `SGA2·M / (SGB2·t)²` exactly as the Supporting Information does (that is
   the local parabola's x² coefficient, half the curvature; Wave does the same — doubling it
   moves OCR 6-15 % away from Wave's), the wall oxygen integrated from the first reading (`W(0)` = its level) with the quadratic
   interpolation of the Supporting Information, and the time between measurements — when the
   probe is raised and nothing is read — filled with its eq. 19 (an exponential with a 30 s time
   constant from the last level of one measurement to the first of the next, at the measurement's
   mean reading interval). The rate is the mean of OCR(t) over the measurement's readings without
   the first and last three, times `ChamberVolume · COb / CO · 60 · 1000`.
3. **pH** (trace 4) per reading and well:
   `pH_cal + (F − F_cal) / (1000 · (C3 · F_cal + C4))`, `F_cal` the well's calibration emission
   (`AnalyteCalibrationsByAnalyteName[pH]/CalibrationEmissionValues`, one array per plate row),
   `C3`, `C4` the pH `GainEquation` (`C1` and `C2` are 0 in every file; others are refused),
   `pH_cal` `DefaultCalibrationPH`.
4. **ECAR** (mpH/min): −1000 × the least-squares slope of the pH against time (min) over the
   measurement's readings from the fourth on (`pHDataModifiers/RateModifier`: `LineFit`, offset 3),
   minus the background wells' mean.
5. **PER** (pmol H⁺/min): the uncorrected ECAR (step 4 without the background) × the well's
   `BufferFactor` × `O2DataModifiers/PlateVolume` × kVol, only for wells with a buffer factor.
   kVol is not stored; 1.6 is what a Wave export prints for the same 96-well plate, so it is an
   assumed value (`assurance.assumed`, withheld by `--strict`).
6. **Rate time** (min): the time of the measurement's middle reading (index (n−1)/2) after the
   first reading of the assay, as Wave prints it.

Background wells, and measurements the file marks not valid, have no rates (Wave prints 0).
Rates are computed only when every condition that was compared with Wave holds: 96 wells, AKOS,
no Ksv leak or temperature correction, the standard plate constants (τAC 746, τAW 0, τW 296,
τC 246, τP 60.9 s, chamber 9.15 µL, plate volume 2.28 µL), the published kernels, rate settings
`LineFit` with O2 offset 1 and pH offset 3, no pH temperature gain correction, a linear pH gain
equation, at least one background well and none flagged, every reading valid, at least seven
readings per measurement; PER also needs `PPRTechnique` `BcFix` with `PPROffset` 1.

**Agreement with Wave** (the tolerance the corpus test enforces, `tests/seahorse_oracle`):
OCR within max(0.2 pmol/min, 0.5 %), ECAR within 0.001 mpH/min.

| assay | values compared | OCR max \|Δ\| | ECAR max \|Δ\| |
| --- | --- | --- | --- |
| D5TSFG HL60 (`.asyr` + Prism XML) | 90 OCR, 90 ECAR | 0.17 pmol/min | < 5e-6 mpH/min |
| D5TSFG MNF (`.asyr` + binary Prism) | 120 OCR | 0.07 pmol/min | – |
| D5TSFG HNF (`.asyr` + binary Prism, 62 empty wells) | 1,380 OCR | 0.08 pmol/min | – |
| seahtrue PBMC (Wave Excel export, time stamps rounded to 1 s) | 1,104 OCR, ECAR, PER; 27,648 levels | 0.29 pmol/min | 0.16 mpH/min |

Levels equal Wave's to 1e-12 (O2) and 1e-14 (pH) in the Excel export. The remaining OCR
difference (about 0.1 % of the rate, the same for every well of a measurement) comes from the
numerics of the wall-oxygen history, which Wave does not document; it is well inside the
replicate spread of any assay but it is not zero, so the table says `within`, not `equal`.

## Layout

gzip-compressed UTF-8 XML, root `XfeAssay`.

| element | content | our reading |
| --- | --- | --- |
| `Plate` | `RowCount`, `ColumnCount`, type, barcode, `Wells/Well` (row by row: `RowIndex`, `ColumnIndex`, `Flag`, `BufferFactor`, `BufferCapacity`, `ParentGroup`) | table `wells` |
| `Well/ParentGroup` | `GroupName`, `IsBackground`, conditions; `InjectionCondition/Injections`: per port `ReagentName`, `PortConcentration`, `PortConcentrationUnit`, `PortLocation`, `Volume` | table `injections` (once per group) |
| `AssayDataSet/PlateTickDataSets/PlateTickDataSet` | one per plate reading: `TimeStamp` (ISO-8601 duration from the run start), `TrayTemperature`, `EnvironmentalTemperature`, per analyte (`O2`, `pH`) an `AnalyteDataSet` with one value per well for the LED on/off emission and reference arrays, `LedUseValues` and `CorrectedEmissionValues`, plus `WellTemperature`, `IsValid` | traces |
| `AssayDataSet/RateSpans/RateSpan` | `StartTickIndex`, `EndTickIndex`, `ValideRate` per measurement | table `measurements` |
| `AssayDataSet/CommandHistory/CommandHistory` | the executed protocol: `InstructionName`, `CommandName`, `StartTime`, `EndTime`, `CompletionStatus` | table `protocol`; run start and end |
| `O2DataModifiers`, `pHDataModifiers`, `EnvironmentDataModifiers`, `Cartridge`, `Groups`, `Protocol`, `AssayStrategies` | calibration, rate settings, cartridge, group and protocol definitions | vendor tree |
| `Name`, `InstrumentSerialNumber`, `SWVersion`, `VersionStamp`, `LastRunBy`, `WellVolume`, … | assay facts | experiment model, vendor tree |

Per-well values are matched to wells in the order of `Plate/Wells`, which lists them row by row in
every file; a plate listed otherwise is refused (exit 6). Readings whose arrays do not hold one
value per well are left out with an error finding.

## What the reader returns

- **Traces 0-1** (`O2 corrected emission`, `pH corrected emission`): one sweep; channel 0 `time`
  (s from the run start, irregular: `extra.axis` `{irregular: true, channel: 0}`), then one channel
  per well (`A1` … `H12`, no unit), the stored corrected emission at every plate reading.
  `extra.analyte`, `extra.kind` = `corrected_emission`, `extra.measurement_table` = 1.
- **Trace 2** (`temperatures`): time, `tray_temperature`, `environment_temperature` and each
  analyte's `…_well_temperature` (°C).
- **Traces 3-4** (`O2 level` in mmHg, `pH level`; only when the rates are computed): time, then
  one channel per well, steps 1 and 3 of *Rates*. `extra.kind` = `level`, `extra.computed` says
  how.
- **Table 0 `wells`**: `well`, `row`, `column` (1-based), `group`, `background` (0/1), `flagged`,
  `buffer_factor`, `buffer_capacity`.
- **Table 1 `measurements`**: `measurement` (1-based), `first_reading`, `last_reading` (indices
  into the traces), `start`, `end` (s), `valid`, `step` (the protocol instruction of the matching
  `Measure` command: `Baseline`, `Oligomycin`, …).
- **Table 2 `injections`**: `group`, `port`, `reagent`, `concentration`, `unit`, `volume` (µL).
- **Table 3 `protocol`**: `step`, `instruction`, `command`, `start` (s from the first command),
  `duration` (s), `status`.
- **Table 4 `rates`** (only when computed): one row per measurement and well, measurement-major
  like Wave's `Rate` sheet: `measurement`, `well`, `group`, `time` (min), `ocr` (pmol/min),
  `ecar` (mpH/min), `per` (pmol/min). `extra`: `method`, `agreement_with_wave`,
  `background_wells`, `kvol`, `plate_volume_ul`, `note`.
- **Experiment**: instrument vendor Agilent (Seahorse), serial, software version; method name
  (the assay name), `wells`, `measurements`, `well_volume` (µL); acquisition start and end (the
  protocol's first and last command, with the PC's UTC offset), operator (`LastRunBy`), duration.
- `notes`: whether the rates were computed, and if not, why.
- `check`: readings the instrument marks not valid, non-finite values, spans that do not tile the
  readings, spans against the protocol's `Measure` commands.

## Validation

`cargo test -p openreadout-corpus-tests --features corpus` (`tests/seahorse_oracle/mod.rs`,
`oracle/seahorse_oracle.py`):

- **Wave's own rates** (`independent: true`): for the three D5TSFG assays the oracle records
  Wave's Prism export (`--export`: Prism XML or binary Prism; the group layout Wave writes into
  the project notes, each data table's columns and replicates, the measurement times). Every
  exported column must match, within the tolerance above, the wells of one selected group (Wave
  lets users rename columns, so the group is found by matching, each group once); the background
  wells and measurement times must be Wave's. `tests/seahorse_rates.rs` runs the level and rate
  functions on the emissions of the seahtrue Wave Excel export (`--wave-xlsx`) and compares
  every level and rate.
- **Self-consistency** for the six files without an export (a second implementation in Python's
  standard library): wells, groups, background wells, injections, reading and measurement counts
  and 128 emission values per file agree; the number of measurement spans equals the number of
  `Measure` commands; and the physical check of the well order holds — during the first three
  measurements the O2 emission of the background wells (no cells) changes 3-43 % as much as that
  of the other wells (on average). That check does not hold in assays of weakly respiring cells
  with many empty wells (the D5TSFG files: 0.9-1.2), which is why the Wave export replaces it
  there.

## Vocabulary (public API of `openreadout-biophys`, Seahorse)

| identifier | meaning |
| --- | --- |
| `SeahorseReader` | reader of Agilent Seahorse XF `.asyr` files |
| `SEAHORSE_FORMAT_ID` | `agilent-seahorse-asyr` |
| `SeahorseDataset` | the dataset the reader returns |
| `OxygenModel` | an assay's oxygen constants (hidden from the docs; used by the corpus tests) |
| `f_zero` | `FO`, emission at zero oxygen |
| `ksv` | `Ksv`, Stern-Volmer constant (1/mmHg) |
| `ambient_mmhg` | `CO`, ambient oxygen (mmHg) |
| `ambient_mm` | `COb`, ambient oxygen (mM) |
| `tau_ac` | `TauAC`, atmosphere-to-chamber time constant (s) |
| `tau_aw` | `TauAW`, atmosphere-to-wall time constant (s) |
| `tau_w` | `TauW`, chamber-to-wall time constant (s) |
| `tau_c` | `TauC`, wall-to-chamber time constant (s) |
| `tau_p` | `TauP`, probe response time constant (s) |
| `chamber_ul` | `ChamberVolume`, apparent chamber volume (µL) |
| `oxygen_levels` | step 1 of *Rates* for one reading |
| `ph_level` | step 3 of *Rates* for one value |
| `oxygen_consumption` | step 2 of *Rates* for one well |
| `acidification` | step 4 of *Rates* before the background, for one well and measurement |
| `slope` | least-squares slope |
