# Agilent Seahorse XF `.asyr` provenance

## 2026-09-26 — before the reader was written (Richard Zimring with Claude as assistant)

**Corpus files used** (all in `corpus/manifest.toml`, CC-BY-4.0 Zenodo records, licences checked on
the record pages 2026-09-26):

- Zenodo 10435506 (Li, Zihan): `seahorse-zenodo10435506-mst-1`, `-mst-2` — XFe24 Cell Mito Stress
  Tests (24 wells, 4 × 6), 168-171 plate ticks, 12 measurements.
- Zenodo 15310495 (Grootaert, Montebugnoli, Van Camp, De Mol): `seahorse-zenodo15310495-spheroid-1`,
  `-spheroid-2`, `-plate-1` … `-plate-4` — XFe96 spheroid assays (96 wells, 8 × 12), 228-350 ticks,
  19-25 measurements, four ports with substrate-oxidation inhibitors.
- One further depositor's files are held out (`docs/benchmark/heldout.md`) and were not opened.

**Prior art consulted:** none. No open reader of `.asyr` files was found (searches: GitHub
repositories and code, Zenodo, figshare; seahtrue and dweindl/seahorse read Wave's Excel exports).

### Inferred from the files (scratch dumps with gzip + ElementTree, not committed)

- The file is gzip-compressed XML (`1F 8B`), root `XfeAssay` (XML namespace-free, .NET
  `DataContractSerializer`-style `nil` attributes). Top-level children, in order: `Instrument`
  (a GUID), `AppId`, `TemplateId`, `Ksv`, `AssayDataSet`, `SessionData` (an embedded UTF-16 XML
  document of the analysis session: normalization label and factor, no rates), `Cartridge`,
  `Plate`, `Groups`, `Protocol`, `AssayStrategies`, `FileName`, `ProjectName`, `LastRunBy`,
  `LastRunOn`, `Name`, `AtmosphericPressure`, `DefaultBufferCapacity`, `DefaultBufferFactor`,
  `Notes`, `AnalyticalNotes`, `WellVolume`, `CreatedBy`, `CreatedOn`, `VersionStamp`, `SWVersion`,
  `PlatedOn`, `InstrumentSerialNumber`, …
- `Plate`: `RowCount` × `ColumnCount` (4 × 6, 8 × 12), `Type`, barcode, serial, lot; `Wells/Well`
  in row-major order with `RowIndex`, `ColumnIndex`, `Flag`, `BufferCapacity`/`BufferFactor`
  (nil when unset) and `ParentGroup`: `GroupName`, `IsBackground`, `GroupColor`, and the group's
  conditions (`InjectionCondition/Injections` per port: `ReagentName`, `PortConcentration`,
  `PortConcentrationUnit`, `PortLocation`, `Volume`; `BioMaterialCondition`: cell line, seeding
  density, passage; `RunningMediaCondition`; `PretreatmentCondition`). Wells of no group have no
  `GroupName` (7 in one file).
- `Cartridge`: type, serial, lot, barcode, `Ports/PortInfo` (location A-D and volume µL).
- `AssayDataSet/PlateTickDataSets/PlateTickDataSet`: one per plate reading ("tick"): `TimeStamp`
  (ISO-8601 duration from the run start, `PT38M4.658S`), `TrayTemperature`,
  `EnvironmentalTemperature`, and per analyte (`O2`, `pH`) an `AnalyteDataSet` with one value per
  well (plate order) for `LedOnEmissionValues`, `LedOffEmissionValues`, `LedOnReferenceValues`,
  `LedOffReferenceValues`, `LedUseValues` (integers) and `CorrectedEmissionValues` (doubles),
  plus `WellTemperature`, `IsValid`, `CalibratedWellCount`. Every tick has exactly one value per
  well for every array in all 8 files.
- `AssayDataSet/RateSpans/RateSpan`: one per measurement, `StartTickIndex` … `EndTickIndex`
  (contiguous, 12 or 14 ticks; together they cover every tick) and `ValideRate`. The number of
  spans equals the number of `Measure` commands in `CommandHistory` in every file (12, 19, 22,
  25).
- `AssayDataSet/CommandHistory/CommandHistory`: the executed protocol: `InstructionName`
  (`Baseline`, `Oligomycin`, …), `CommandName` (`Mix`, `Wait`, `Measure`, `Inject`, `Calibrate`,
  …), `CommandIndex`, `StartTime`/`EndTime` (ISO-8601 with offset), `CompletionStatus`.
- `AssayDataSet/O2DataModifiers`, `pHDataModifiers`, `EnvironmentDataModifiers` and
  `AnalyteCalibrationsByAnalyteName`: the calibration and rate-calculation settings (Ksv, F0,
  AKOS/level method, rate technique `LineFit`, calibration emission values per well). Oxygen
  consumption and extracellular acidification rates are **not stored**: Wave computes them from
  the emissions with these settings.
- Well order: every file lists `Plate/Wells` row by row, and the per-well arrays have exactly one
  value per listed well. That array index i belongs to the i-th listed well is supported by the
  data: during the first three measurements the O2 emission of the four background wells (no
  cells; A1, B4, C3, D6 on the XFe24 and the corners on the XFe96) changes 3-43 % as much as that
  of the other wells, on average, in all six files (in both XFe24 files the four background
  wells are exactly the four flattest).

## 2026-09-26: rates investigated, not implemented

Read: Gerencser et al. 2009 (open access, PMC2727168) for the Stern-Volmer conversion with a
background-well gain correction and the compartment model (chamber, walls, probe, atmosphere).
Tried on Harvard Dataverse doi:10.7910/DVN/D5TSFG (CC0; `HL60 DHL60 XF Cell Mito Stress Test
.asyr` with `HL60 DHL60 mitostress assay.pzfx`, Wave's per-well OCR for wells C1-C3 and C10-C12):
[O2] = CO + (FO/Ksv)(1/F − 1/F_background) and a least-squares slope per rate span give OCR
proportional to Wave's for the three baseline measurements (factor about 23 pmol/min per
mmHg/min, the chamber volume and solubility) but 30-40 % off after oligomycin and FCCP. Without
Wave's AKOS details the rates would not match the vendor's; the reader keeps returning emissions
only (see the format note).

## 2026-09-26 — rates reproduced (Richard Zimring with Claude as assistant)

**Corpus files used (new, development):**

- Harvard Dataverse doi:10.7910/DVN/D5TSFG (CC0-1.0, licence read from the dataset API
  2026-09-26; depositor Pulikkottil/Fan lab): `seahorse-dataverse-d5tsfg-hl60`, `-mnf`, `-hnf`
  (three XFe96 Cell Mito Stress Tests, Wave 2.6.1, 362-380 readings, 15 measurements each) and
  their Wave exports to GraphPad Prism: `HL60 DHL60 mitostress assay.pzfx` (Prism XML: per-well
  OCR, ECAR and PER of 6 wells) and `MNF …`/`HNF … mitostress assay.pzfx` (binary Prism files:
  per-well OCR of 8 and 87 wells). The Prism files are oracle exports; how their cells are laid
  out was read from the files themselves (`oracle/seahorse_oracle.py`).
- seahtrue test data (github.com/vcjdeboer/seahtrue, Artistic-2.0, `inst/extdata/
  20191219_SciRep_PBMCs_donor_A.xlsx`): a Wave 2.6.1 Excel export (sheets `Raw`: per reading and
  well the corrected O2 and pH emissions with Wave's O2 (mmHg) and pH levels; `Rate`: OCR, ECAR,
  PER; `Assay Configuration`: Ksv, calculated FO, TAC/TAW/TW/TC/TP, pseudo volume, pH gain
  equation, ECAR technique and edge offset, plate volume, kVol, buffer factors; `Calibration`:
  per-well calibration emissions). No `.asyr` accompanies it; it is a second depositor for the
  level and rate formulas (`seahorse-seahtrue-pbmc-export`). Only the workbook was used.

**Prior art consulted:** Gerencser A.A. et al., "Quantitative microplate-based respirometry with
correction for oxygen diffusion", Anal. Chem. 2009, 81, 6868-6878 (open access, ACS AuthorChoice;
PMC2727168), its equations 1-15 and its Supporting Information (`ac900881z_si_001.pdf`: the
Savitzky-Golay kernels, the quadratic-interpolation integral of eq. 13, the exponential fill of
eq. 19 with a 30 s time constant; `ac900881z_si_002.xls`: the authors' worked spreadsheet of the
correction, used to check our kernels and integral term by term, 1e-13 agreement). This is the
published method.

**Inferred (each from the files named, compared with Wave's own numbers):**

- The `.asyr` fields the method needs are stored: `O2DataModifiers/{O2CalculationMethod=AKOS,
  FO, CO, COb, Ksv}` (FO = 12500·(1 + Ksv·CO): 12500 is `AnalyteCalibration[O2]/
  TargetEmissionValue`), `O2DataModifiers/Plate/{TauAC, TauAW, TauW, TauC, TauP, ChamberVolume}`
  (the paper's τ and apparent chamber volume), `SGA1`/`SGB1` (first-derivative kernel for O2 and
  for time), `SGA2` (second-derivative kernel), `Smoothing` (7-point quadratic smoothing), all 26
  copies identical to the paper's kernels in every file.
- O2 level (mmHg) = CO + (FO/Ksv)·(1/F − 1/F_T), F the corrected emission, F_T the arithmetic
  mean of the background wells' corrected emission at the same reading (the paper's eq. 5).
  seahtrue export: Wave's `O2 (mmHg)` equals this to 1e-12 for all 13,824 values once F_T is the
  mean over the wells Wave treats as background (its `Rate` sheet gives them 0: A01, H01, H12;
  well A12, labelled Background in `Raw`, has rates and is not one).
- OCR per well and measurement = the mean of the paper's corrected OCR(t) (eq. 13 and 14, with
  W(0) = the first reading's level, the kernels above, the quadratic-interpolation integral,
  and the gaps between measurements filled by eq. 19 at the measurement's mean reading interval)
  over the readings of the measurement without the first and last three, times
  ChamberVolume·COb/CO·60·1000 (pmol/min). Wave subtracts nothing further: the background
  normalisation of the level is its background correction. Against Wave's OCR: the three D5TSFG
  files (1,515 well-measurements incl. 62 empty wells per plate in HNF) agree to 0.17 pmol/min
  and 0.6 % (|OCR| > 5 pmol/min; median 0.13 %); the seahtrue export (1,008 values; its time
  stamps are rounded to whole seconds) to 0.29 pmol/min. The remaining ~0.1 % is a proportional
  difference, same for all wells of a measurement (numerics of the history integral, not
  identified); it is the stated tolerance, not hidden.
- pH level = pH_cal + (F − F_cal)/(1000·(C3·F_cal + C4)), F_cal the well's calibration emission
  (`AnalyteCalibration[pH]/CalibrationEmissionValues`, rows of the plate), C3/C4 the pH
  `GainEquation` (C1 = C2 = 0 in every file seen), pH_cal `DefaultCalibrationPH`. seahtrue export:
  equals Wave's pH to 1e-14 for all 13,824 values.
- ECAR (mpH/min) = −1000 × (least-squares slope of the pH level against time in minutes over the
  measurement's readings from the fourth on (`pHDataModifiers/RateModifier/Offset` = 3,
  technique `LineFit`) − the mean slope of the background wells). D5TSFG HL60: all 90 values
  within 5e-6 mpH/min of Wave's; seahtrue: 0.16 mpH/min (whole-second time stamps).
- PER (pmol H⁺/min) = (ECAR + background ECAR) × buffer factor × plate volume
  (`O2DataModifiers/PlateVolume`, 2.28 µL) × kVol: in the seahtrue export exactly, with kVol 1.6
  as its `Assay Configuration` prints for this XFe96 plate. kVol is not in `.asyr` files, so it
  is an assumed value (1.6, XFe96 only) and PER is withheld by `--strict`.
- Rate time (min) = time of the measurement's middle reading (index (n−1)/2) minus the first
  reading's, as both exports print it.
- Not validated, so refused: XFe24 plates (the three 24-well development files have no export),
  O2 methods other than AKOS, Ksv leak or temperature correction, a pH gain equation with C1 or C2,
  pH temperature gain correction, a flagged background well, readings the instrument marks
  invalid.
