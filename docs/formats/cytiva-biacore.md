# Cytiva Biacore `.blr`

The Biacore T200 Control Software saves surface plasmon resonance runs as `.blr` result files, and the T200 Evaluation Software saves `.bme` evaluation files. OpenReadout returns every stored sensorgram, the report points, cycles and event log, and from `.bme` files the fits of the evaluation items.

Derived from public result files of two depositors (Control Software 2.0.1 and 2.0.2), the control software's own report points inside each file, and allotropy's output (MIT, black box). Provenance: `docs/provenance/cytiva-biacore.md`. Crate: `openreadout-biophys`.

| format id | files | reads | confidence |
| --- | --- | --- | --- |
| `cytiva-biacore-bme` | `.bme` evaluation files of the Biacore T200 Evaluation Software 3.0 | the result file(s) it holds, read as a `.blr`, and every fit of its evaluation items (model, parameters with standard errors, Chi², the curves fitted) | evidence rubric |
| `cytiva-biacore-blr` | `.blr` result files of the Biacore T200 Control Software | every stored sensorgram (raw flow cells and reference-subtracted curves, per cycle), the report points, cycles and event log, chip, instrument, software and run times | medium (evidence rubric, `docs/assurance.md`) |

Not read: Biacore evaluation files other than `.bme` (`.bie`), Biacore Insight and 8K result files, older
BIAevaluation files, wizard templates (`.bwKin`, `.bwImmob`). Kinetic and
affinity fits (ka, kd, KD, Rmax) are the evaluation software's and are not in result files.

## Layout

A compound file (MS-CFB). Streams read:

| stream | content | our reading |
| --- | --- | --- |
| `\x03BIA compability info` | `FileType=Result File`, `FileTypeVersion=12` | detection: a compound file with this stream saying `Result File` is a Biacore result file |
| `Environment` | `key=value` text: application, version, run type, processing unit, instrument id, user, `Timestamp`, `EndTime` (OLE dates: days since 1899-12-30, local time) | instrument, software, method name, operator, start, end |
| `Chip`, `FileTag`, `AppData/Bioconf` | `key=value` text: chip type and id, IFC, flow cells, ligand and level per flow cell; concentration unit; instrument configuration | vendor tree; flow-cell count |
| `RPoint Table` | tab-separated text, a header line: `Cycle`, `Fc`, `Aprog`, `DiodeRow`, `Time`, `Window`, `AbsResp`, `SD`, `Slope`, `LRSD`, `Quality`, `Baseline`, `RelResp`, `Id`, `Min`, `Max`, then the run's keyword columns | table `report_points` as written |
| `_Cycle N/EventLog` | a count line, then `F<ms>;<code>;<arguments>` | table `event_log` (codes not interpreted); code 10's `d<OLE date>` is the cycle's start |
| `_Cycle N/_Window N/Properties` | `Caption=`, `Title=` | the window caption |
| `_Cycle N/_Window N/_Curve N/Labels` | lines: title (`Sensorgram Fc=3`, `Subtracted Fc=4-3`), x name, x unit (`s`), y name (`Response`, `Resp. Diff.`), y unit (`RU`) | curve name, flow cell, units |
| `…/_Curve N/\x03Keywords` | `key=value`: cycle type, assay step, sample, concentration, molecular weight, `Temp#`, buffer, `Fc`, `DiodeRow` | trace `extra.keywords`; the cycles table |
| `…/_Curve N/\x03RPoints` | a line, then `time\twindow\tflag\tname` per report point | trace `extra.report_points` |
| `…/_Curve N/Segment 1` | u32 1, u32 1, f64 step (s), f64 start (s), f64 0, f64 1, u32 n, n × f32 response | raw flow-cell sensorgram on a regular grid |
| `…/_Curve N/XYData` | u32 1, u32 1, u32 n, n × f32 time (s), n × f32 response | reference-subtracted sensorgram |

Every curve seen has exactly one of `Segment 1` or `XYData`; stream sizes are exactly 44 + 4n and
12 + 8n. The segment's two time fields are equal in every file (0.1 s at 10 Hz, 1 s at 1 Hz), so
which one is the step and which the start is not determined: a curve where they differ is refused
(`unvalidated_time_base`), as are further segments, headers other than (1, 1), a non-identity
0/1 pair, or an x axis not in seconds. `Quality`, `TimeCorrection`, `UniqueId`, `APoints`,
`Attributes`, `AppData/Dip`, `AppData/ApplicationTemplate` and `AppData/ApplicationMethod` are
listed by `info --view structure`, not interpreted.

## Evaluation files (`.bme`)

The compatibility stream says `FileType=T200 Evaluation File` (`FileTypeVersion=4`), and each
reader leaves the other kind to the other reader (detection by content). The evaluation software
copies each result file it uses under `_DataManager N/` (its `Environment`, `Chip`, `FileTag`,
`AppData/…`, `Evaluation` (the result file's name and size) and the `_Cycle N/_Window N/_Curve N`
storages, laid out as above), and writes its own `Environment` at the root (application
`Biacore T200 Evaluation Software`, version, user). `DataManager` lists the keyword names and the
cycle count. The run facts come from `_DataManager 1/Environment`; the evaluation environment is
in `vendor.biacore.evaluation_environment`. Traces carry `extra.file` (N); with more than one
result file their names start `file N`, and the cycles table has a `file` column.

`Evaluation/EvaluationItemN` are XML documents (ISO-8859-1) whose root has a `ClassName`
(`ReportPointTable`, `Plot`, `Sensorgram`, `AffinityScreen`, `KineticScreen`, `KineticsAffinity`,
`ConcentrationAnalysis`) and a `Name`; `EvaluationItemNBinary` holds the float32 curves the
evaluation processed (not read). Fits take one of two layouts:

- `modelFits/modelFits/modelFit` (screens): a `model` element (`ModelName` attribute), the fit's
  own `curveSet/CurveSet`, a `key` (`fileNumber`, `sampleName`, `temperature`, `ligandName`,
  `curveName`) and a `fitStatus`;
- `Fits/FitN` (`KineticsAffinity`): `FitN` has the `ModelName` attribute and shares the item's
  `CurveSet`.

The model element holds `Model` (the expression, `Conc*Rmax/(Conc+KD)+offset` for steady-state
affinity), `Chi2`, `Parameters` (`id:name|value|standard error` separated by `;`, where the id
may hold a second `:`, `0:1-32:ka`) and `ReportParameters` (`KD (M)|KD`: the unit of each
parameter). A `CurveSet` has `SampleName`, `LigandName`, `Temperature` and `SubsetN` with a
`CurveName` (`Fc=2-1`) and `CurveN`: `FileIndex`, `CycleNumber`, `SampleName`, `ConcUnit`,
`Injections/Injection` (`Concentration`, `MolarConcentration`, `Response`) and, in screens,
`Included`.

Tables after the event log: `evaluation_items` (item, class, name, fits), `fits` (fit, item,
item name, model, sample, ligand, curve(s), temperature, Chi², status, number of curves, then per
parameter name its first value and `<name>_se` with the unit `ReportParameters` gives: `KD` in M,
`ka` in 1/Ms, `kd` in 1/s, `Rmax` in RU, …), `fit_parameters` (every parameter with its scope,
value, standard error and unit) and `fit_points` (per fit and curve: curve name, file, cycle,
sample, molar concentration, the concentration as entered with its unit, the response the fit
used, included 1/0 or NaN when not stored). Values are the evaluation software's; nothing is
refitted.

## What the reader returns

- **One trace per stored curve**, in cycle / window / curve order, named `cycle <n> <title>`
  (`cycle 2 Sensorgram Fc=3`, `cycle 2 Subtracted Fc=4-3`): one channel `response` (RU, float32
  values as stored); `sample_rate_hz` and `start_s` give the time since the cycle's start
  (`extra.axis` = time in s). A subtracted curve whose float32 times are not a regular grid (none
  seen) carries them as channel 0 `time`. `extra`: `kind` = `sensorgram`, `cycle`, `window`,
  `curve`, `window_caption`, `title`, `response` (the y name), `flow_cell` (`3`, `4-3`),
  `reference_subtracted`, `keywords`, `report_points` (time, window, flag, name), `storage`.
- **Table `report_points`** (when the file has report points): the `RPoint Table` columns in order;
  numeric columns as numbers (`N/A` and blanks as NaN), the others (`Fc`, `Aprog`, `Quality`,
  `Baseline`, `Id`, text keywords) as categories. Units: `Time`, `Window` s; `AbsResp`, `RelResp`,
  `SD`, `Min`, `Max` RU; `Slope` RU/s.
- **Table `cycles`**: `cycle`, `window_caption`, `start` (s after the run's `Timestamp`, from the
  event log), `curves`, then the keywords of the cycle's first curve (sample, concentration,
  molecular weight, assay step, …).
- **Table `event_log`**: `cycle`, `time` (s), `code`, `arguments` — as recorded, not interpreted.
- **Experiment**: instrument vendor Cytiva (Biacore), model from the processing unit (`BiacoreT200`
  → `Biacore T200`), serial = instrument id, kind surface plasmon resonance instrument
  (OBI:0001136), software and version; method name = run type (`Kinetics/Affinity`,
  `Immobilization`, `Manual Run`), technique surface plasmon resonance spectroscopy (CHMO:0000624),
  parameters `temperature` (°C, when every curve's `Temp#` agrees), `flow_cells`, `cycles`;
  acquisition `started_at`/`ended_at` (local time without a zone), `operator`, `duration_s`.
- **Vendor tree** `biacore`: `application`, `compatibility`, `environment`, `chip`, `file_tag`,
  `instrument_configuration`.
- `check`: header and size checks per curve (left-out curves are findings), every curve read,
  non-finite values, compound-file problems.

## Validation

`cargo test -p openreadout-corpus-tests --features corpus` (`tests/biacore_oracle/mod.rs`,
`oracle/biacore_oracle.py`):

- In the six files stored at 10 Hz, every report point the control software wrote (1,224 over raw
  and subtracted curves) equals the mean of the reader's sensorgram over the closed window
  [Time − Window/2, Time + Window/2] to 1e-6 RU (observed 5e-11): this fixes the time base and the
  values of both curve kinds. In the three 1 Hz files the software averaged data the file does not
  keep; its report points lie within the window's spread of the stored samples.
- allotropy's sensorgram cubes (all curves of the first 12 cycles): the same curves per cycle and
  flow cell, lengths, and times and responses at 64 samples each.
- Instrument id, software and version, model, user, run type, cycle and flow-cell counts and chip
  id equal the file's `Environment` and `Chip` text and allotropy's.

Two depositors only, so no file is held out.

**Evaluation files.** Six `.bme` files of two depositors (allotropy's test data, MIT; the SGC
Toronto USP5 series, CC-BY-4.0): every curve of the first 12 cycles equals allotropy's evaluation
parser (black box, 84 curves per file), and every fit (174 fits, 1,968 cells: sample, curves,
model, Chi², number of curves, each parameter's value and standard error) equals the oracle's own
parse of the item XML (`oracle/biacore_oracle.py`); a refit of the steady-state model to the
stored concentrations and responses reproduces the stored KD within 1 % for 97 of 108
steady-state fits (the others are fits the software itself left unconstrained, KD far above the
highest concentration). One lab (Harvard Dataverse doi:10.7910/DVN/GTNOTI) is held out.

## Vocabulary (public API of `openreadout-biophys`, Biacore)

| identifier | meaning |
| --- | --- |
| `BiacoreReader` | reader of Cytiva Biacore `.blr` result files |
| `BiacoreEvaluationReader` | reader of Biacore T200 `.bme` evaluation files |
| `BIACORE_BME_FORMAT_ID`, `BME_FORMAT_ID` | `cytiva-biacore-bme` |
| `Kind` { `Result`, `Evaluation` }, `kind_of` | the file kind the compatibility stream names |
| `Evaluation`, `evaluation`, `evaluation_tables`, `first_param`, `texts`, `multi_file`, `result_prefixes`, `MAX_ITEM` | evaluation-file content and its tables |
| `EvalItem`, `index`, `class`, `name`, `fits` | an evaluation item |
| `Fit`, `item`, `item_name`, `model`, `expression`, `chi2`, `sample`, `ligand`, `temperature`, `curves`, `status`, `params`, `units`, `points` | a fit |
| `Param`, `scope`, `value`, `se` | a fit parameter |
| `FitPoint`, `subset`, `file`, `cycle`, `concentration`, `concentration_stated`, `unit`, `response`, `included` | a curve of a fit |
| `parse_item`, `parse_parameters`, `parse_units` | item XML parsing |
| `BIACORE_FORMAT_ID` | `cytiva-biacore-blr` |
| `BiacoreDataset` | the dataset the reader returns |
