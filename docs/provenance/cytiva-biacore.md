# Cytiva Biacore `.blr` provenance

## 2026-09-26 — before the reader was written (Richard Zimring with Claude as assistant)

**Corpus files used** (all in `corpus/manifest.toml` with their licences):

- allotropy (Benchling, MIT, commit 77748ee8c29cda07994abe880c776f6f4ef467e6), test data of its
  `cytiva_biacore_t200_control` parser: `biacore-allotropy-ed-fig1a-b2`, `biacore-allotropy-ed-fig1a-b3`
  and `biacore-allotropy-fig2c-prongs` (kinetics/affinity wizard runs, 10 Hz),
  `biacore-allotropy-ed-fig6a-immob` and `biacore-allotropy-fig5a-hd-immob` (immobilization wizard
  runs, 1 Hz), `biacore-allotropy-fig4b-her3-immob` (a manual run, 1 Hz). Biacore T200 Control
  Software 2.0.1, one instrument.
- Zenodo 5011513 (CC-BY-4.0): `biacore-zenodo5011513-strep1`, `-strep2`, `-strep3` (kinetics wizard
  runs, Biacore T200 Control Software 2.0.2, 14 to 79 cycles).
- Two depositors only: no file is held out (the held-out rule needs three independent sources).

**Prior art consulted:** allotropy's `cytiva_biacore_t200_control_decoder.py` (MIT,
<https://github.com/Benchling-Open-Source/allotropy/tree/77748ee8c29cda07994abe880c776f6f4ef467e6/src/allotropy/parsers/cytiva_biacore_t200_control>),
read as documentation of the container: the file is a compound file; `Environment` and `Chip`
are `key=value` text with OLE-automation dates; per-cycle storages `_Cycle N/_Window N/_Curve N`
hold `Labels`, `XYData` and `Segment N` streams; allotropy skips 3 floats of `XYData` and 11 of
`Segment`. Its header skips were not copied: the header fields below were derived from the files
and checked against the vendor's own report points.

### Inferred from the files (scratch comparisons with olefile/numpy, not committed)

- A compound file (MS-CFB). `\x03BIA compability info` is text `FileType=Result File`,
  `FileTypeVersion=12` in all nine files; `\x03BIA application info` `BIACOR~1.EX 0.0`;
  `\x03BIA filehandler info` `BDATA.DLL 1.0`.
- `Environment`, `Chip`, `FileTag`, `AppData/Bioconf`: `key=value` lines. `Environment` names the
  application (`Biacore T200 Control Software`), its version, the run type (`Kinetics/Affinity`,
  `Immobilization`, `Manual Run`), the processing unit (`BiacoreT200`), the instrument id, the user,
  `Timestamp` and `EndTime` as OLE-automation dates (days since 1899-12-30, local time: 43082.6857 is
  2017-12-13 16:27:26; the runs last 1.9-12.6 h, consistent with their cycle counts and lengths).
  `Chip`: chip type (`CM3`, `CM5`), chip id, IFC, number of flow cells, ligand and immobilized level
  per flow cell. `FileTag`: the concentration unit.
- `RPoint Table`: tab-separated text with a header line (`Cycle`, `Fc`, `Aprog`, `DiodeRow`, `Time`,
  `Window`, `AbsResp`, `SD`, `Slope`, `LRSD`, `Quality`, `Baseline`, `RelResp`, `Id`, `Min`, `Max`,
  then the run's keyword columns). One row per report point per curve; `Fc` is a flow cell (`3`) or
  a subtraction (`4-3`).
- `_Cycle N/_Window N/_Curve N/Labels`: lines: title (`Sensorgram Fc=3`, `Subtracted Fc=4-3`),
  x name (`Time`), x unit (`s`), y name (`Response`, `Resp. Diff.`), y unit (`RU`).
  `Keywords`: `key=value` lines (cycle type, assay step, sample, concentration, molecular weight,
  temperature, buffer, flow cell, detector row). `RPoints`: the curve's report points (time, window,
  a flag, name). `Properties` of the window: its caption (`Startup`, `Sample`, the ligand).
- `Segment 1` (raw flow-cell curves): two u32 (1, 1); four f64 (step and start time, both 0.1 at
  10 Hz and 1.0 at 1 Hz in every file, so which is which is not determined; 0.0; 1.0); a u32 count
  n; n f32 responses. Stream size = 44 + 4n in all 288 segments. Every curve has one segment.
- `XYData` (reference-subtracted curves): two u32 (1, 1), a u32 count n, n f32 times then n f32
  responses. Size = 12 + 8n in all 118. The times are the segment grid (0.1, 0.2, … within float32
  rounding) of the raw curves of the same window, which have the same n.
- **Check against the vendor's report points:** in the six 10 Hz files, every `RPoint Table`
  `AbsResp` (1,224 rows over raw and subtracted curves) equals the mean of the curve's samples in
  the closed window [Time − Window/2, Time + Window/2] to 5e-11 RU with time = start + i·step. This
  fixes the time base (start 0.1 s, step 0.1 s) and the values of both curve kinds. In the three 1
  Hz files the report points disagree by up to 4 RU (the software averages data not stored at 1
  Hz); there the response lies within the report point's `Min`-`Max` neighbourhood.
- A subtracted curve is not simply the difference of the two stored raw curves: the median
  difference is 0 but it deviates by up to 1,400 RU at injection edges (the software shifts the
  flow cells in time, `TimeCorrection` streams); the stored subtracted curve is returned as stored.
- `_Cycle N/EventLog`: a count line, then `F<ms>;<code>;<arguments>` lines. The `d` argument of
  code 10 at 0 ms is an OLE date between the run's `Timestamp` and `EndTime`, increasing by cycle
  (the cycle's start). The other codes are not interpreted.
- `Quality`, `TimeCorrection`, `UniqueId`, `APoints`, `Attributes`, `AppData/Dip`,
  `AppData/ApplicationTemplate` (wizard XML; its `DataCollectionRate` is 10 in the 10 Hz files) and
  `AppData/ApplicationMethod` are listed but not interpreted.

## 2026-09-26: Biacore T200 evaluation files `.bme` (`cytiva-biacore-bme`)

**Corpus files used (development):** allotropy's evaluation test data (Benchling, MIT, commit
77748ee8; the `.bme` files are Git LFS objects, fetched from GitHub's media endpoint):
`biacore_evaluation_module_example.bme` (Kinetic Screen with 1:1 binding fits and Affinity Screen
with steady-state fits), `…example2.bme` (Concentration Analysis, no fits), `…example3.bme` and
`…example4.bme` (Kinetics/Affinity items with kinetics and steady-state fits); and the SGC
Toronto USP5 series on Zenodo (Mann, Harding, Schapira; CC-BY-4.0): 2548710 `USP5 SPR_plate
1.bme` (Affinity Screen, 30 steady-state fits), 3518127 `Experiment 1.bme`, 3543630 `SPR
results.bme`. Held out: the Klasse/Moore lab datasets on Harvard Dataverse (doi:10.7910/DVN/
GTNOTI, CC0), one `.bme`, not opened.
**Prior art consulted:** allotropy's `cytiva_biacore_t200_evaluation` decoder (MIT, same commit),
read as documentation: evaluation items are XML streams `Evaluation/EvaluationItemN` whose fits are
either `modelFits/modelFits/modelFit` (a `model` with `Chi2` and `Parameters`, a `curveSet`, a
`key`) or `Fits/FitN` beside the item's own `CurveSet`; `Parameters` is `id:name|value|error`
entries separated by `;`, the id itself possibly holding a second `:` (`0:1-32:ka`). allotropy is
also run as a black box for the sensorgrams (as for `.blr`).
**Inferred from the files:** the compatibility stream says `FileType=T200 Evaluation File`; the
run's result file is copied under `_DataManager N/` (one per result file the evaluation uses:
its `Environment`, `Chip`, `FileTag`, `AppData/…`, `_Cycle N/_Window N/_Curve N` storages, laid
out as in a `.blr`), and the evaluation software writes its own `Environment` at the root. The
item XML is ISO-8859-1 (`µM`). Each fit's `ModelName` is an attribute of `model` or `FitN`, its
expression in `Model`, the units of its parameters in `ReportParameters` (`KD (M)|KD`). Each
curve of a fit's curve set carries its file index, cycle, sample, ligand, concentration (and
molar concentration) and the report-point `Response` it was fitted to, and whether it was
`Included`.
