# X-ray diffraction (PANalytical XRDML, Bruker RAW and BRML, Rigaku RAS and RASX)

OpenReadout reads the diffraction files of Malvern Panalytical (X'Pert PRO, Empyrean), Bruker (D8 series, DIFFRAC.SUITE) and Rigaku (SmartLab) instruments. It returns each scan as a trace of intensity against the scanned axis, with the tube, wavelengths, detector and counting time. Area-detector frames are refused.

Derived from public files of many depositors, the public XRDML schema (1.5 and 2.1), and geddes (MIT) and FAIRmat readers-xrd (Apache-2.0) read as documentation. geddes is run as a reference reader, and vendor exports of the same scans (Data Collector CSV, `.xy`, Bruker's UXD conversion, PDXL ASCII, M-DaC CSV) are compared too. Provenance: `docs/provenance/xrd.md`. Crate: `openreadout-xrd`.

| format id | files | reads | confidence |
| --- | --- | --- | --- |
| `panalytical-xrdml` | `.xrdml` (XML, public schema) | every scan of every measurement: positions, intensities or raw counts (attenuation applied per schema), counting times, wavelengths, tube, detector | evidence rubric (`docs/assurance.md`) |
| `bruker-raw` | `.raw` beginning `RAW1.01` or `RAW4.00` | every range: start, step, scanned drive, intensities, step time, tube, wavelengths, user, sample | evidence rubric |
| `bruker-brml` | `.brml` (zip of XML) | every data route of every raw-data member, columns as the data views name them | evidence rubric |
| `rigaku-ras` | `.ras` (text) | every scan with its header | evidence rubric |
| `rigaku-rasx` | `.rasx` (zip) | every `Data*/Profile*.txt` with its measurement conditions | evidence rubric |

Not read: area-detector frames (XRDML 2D, BRML 2D recordings, RASX images) — refused (exit 6);
Bruker `RAW ` and `RAW2` (versions 1 and 2) — refused; UXD, `.xy` and other text exports (they
are already open); Philips `.rd`/`.sd`.

## PANalytical XRDML

Schema (Malvern Panalytical, `http://www.xrdml.com/XRDMeasurement/<version>/`): `xrdMeasurements`
→ `xrdMeasurement` (`measurementType`, `sampleMode`) → `usedWavelength` (Å), `incidentBeamPath`
(`xRayTube`: `tension` kV, `current` mA, `anodeMaterial`), `diffractedBeamPath` (`detector`) →
`scan` (`scanAxis`, `mode`, `appendNumber`) → `header` (`startTimeStamp`, `endTimeStamp`,
`author/name`, `source/applicationSoftware@version`, `instrumentControlSoftware`,
`instrumentID`) and `dataPoints`: `positions` per axis (`commonPosition`,
`startPosition`/`endPosition` = evenly spaced over the points, or `listPositions`),
`commonCountingTime`/`countingTimes` (s), `commonBeamAttenuationFactor`/`beamAttenuationFactors`,
and the values: schema 1.x `intensities` (already multiplied by the attenuation factors) or
schema 2.x `counts` ("not yet corrected for beam attenuator or divergence factors"). Raw counts
with attenuation factors are multiplied by them (the raw counts are kept as channel `counts`,
and the calibration is reported as applied); raw counts with divergence corrections are refused.

The abscissa is the scan axis's positions (`2Theta` for `Gonio` and `2Theta-Omega` scans), else
the first varying axis; other listed axes become channels, fixed ones `extra.other_axes`.

## Bruker RAW

`RAW1.01` (DIFFRACplus, "version 3"): a 712-byte header (range count at 12; date `MM/DD/YY` at
0x10 and time at 0x1A; user 0x24, site 0x6C, sample 0x146; goniometer and stage codes 0x224/0x228;
radius (mm, float32) 0x234; anode 0x260; Kα1, Kα2, Kβ and the Kα2/Kα1 ratio as float64 at
0x270-0x288), then per range a header (u32 size ≥ 260, u32 points, float64 start θ at +8 and 2θ at
+16, step at +176, float32 step time at +192, u32 scan type at +196, u32 kV/mA at +224/+228,
float64 range wavelength at +240, u32 varying-parameter bits at +248, u32 record size at +252, u32
extra-record size at +256), the extra record and the records: float32 intensity, then a float64
measured 2θ when bit 0 is set (other bits are refused).

`RAW4.00` (DIFFRAC.SUITE, "version 4"): a 61-byte header (date `MM/DD/YYYY` at 0x0C, time 0x18),
then records of u32 kind and u32 length: text records (kind 10: a 24-byte name at +12, the value
from +36 — `USER`, `SAMPLEID`, `COMMENT`, `CREATOR`, `CREATOR_VERSION`, …), an instrument record
(kind 30: average Kα, Kα1, Kα2, Kβ, ratio as float64 at +0x48…+0x68, anode at +0x74) and ranges
(kind 0 or 160: a 160-byte header with the scan type text at +32, float64 start +72 and step +80,
u32 points +88, float32 kV and mA at +100/+104, float64 wavelength +112, u32 record size +136, u32
drive-record size +140; drive records (kind 50) with a flag at +8, a name at +12 and a position at
+56). The scanned axis is the flagged drive whose position is the range start, else 2θ for
coupled and detector scans. Records are one float32 intensity, or two (8 bytes: the second value
is returned as `record_value_2`, meaning not identified).

## Bruker BRML

`Experiment0/DataContainer.xml`: raw-data members (`RawDataReferenceList`), instrument
(`InstrumentDescription`: `DeviceTypeDesc`, `SerialNo`, `IcsVersion`), writer
(`CreatingVersion`), `MeasurementInfo` (`UserName`, `SampleName`, `Comment`). `RawData*.xml`:
`TimeStampStarted`/`Finished`, `DataRoutes/DataRoute` (`RouteFlag`) with `ScanInformation`
(`VisibleName`, `TimePerStep`, `ScanAxes`), `Datum` rows (comma-separated) and `DataViews`
(`RawDataView` with `Start`/`Length`: `MeasuredTime` s, `AbsorptionFactor`, the scan axes by
`FieldDefinitions`, the recorded counts); `FixedInformation/Instrument`: wavelengths, tube
material, voltage, current, goniometer radius. Counts are as stored; absorber factors are a
channel and are not applied.

## Rigaku RAS and RASX

RAS: `*RAS_DATA_START`; per scan `*RAS_HEADER_START` … `*KEY "value"` … `*RAS_HEADER_END`,
`*RAS_INT_START`, rows `x intensity attenuation`, `*RAS_INT_END`. RASX: `Data*/Profile*.txt`
(tab-separated `x intensity attenuation`) with `Data*/MesurementConditions*.xml` (the same keys as
elements and as a `RASHeader` of `*KEY`/value pairs). Keys used: `MEAS_SCAN_AXIS_X`,
`MEAS_SCAN_UNIT_X`/`Y`, `MEAS_SCAN_START_TIME`/`END_TIME` (`MM/DD/YYYY` or `MM/DD/YY`),
`MEAS_SCAN_MODE`, `MEAS_SCAN_SPEED` and unit, `MEAS_SCAN_STEP`, `MEAS_DATA_COUNT` (checked),
`HW_XG_TARGET_NAME`, `HW_XG_WAVE_LENGTH_ALPHA1`/`ALPHA2`/`BETA`, `MEAS_COND_XG_VOLTAGE`/`CURRENT`,
`FILE_OPERATOR`, `FILE_SAMPLE`, `FILE_COMMENT`, `FILE_SYSTEM_NAME`, `HW_COUNTER_SELECT_NAME`.
Axis names written in Shift-JIS by Japanese installations (`2θ/θ`) are recognised. Intensities
are as stored; attenuation factors other than 1 are flagged (warning `attenuation`) and not applied.

## What the readers return

- **One trace per scan** (range, data route, profile): channel `intensity` (counts, or the unit the
  file names, e.g. cps), the abscissa in `extra.axis` (`two_theta`, `omega`, `theta`, `phi`,
  `chi`, `x`, `y`, `z`, `time` …; unit `°`), or as channel 0 when positions are listed per point;
  further channels `counts`, `counting_time`, `attenuation_factor`, `absorption_factor`,
  `record_value_2`, other listed axes. `extra`: `kind` = `diffractogram`, `data_type`,
  `scan_axis`, `scan_mode`, `scan_type`, `status`, `append_number`, `measurement`, `range`,
  `route`, `member`, `scanned_drive`, `drives`, `detector`, `counting_time_s`,
  `time_per_step_s`, `step_time_s`, `start_theta`, `scan_type_code`, `tube_voltage_kv`,
  `tube_current_ma`, `range_wavelength_angstrom`, `wavelength_angstrom`, `attenuation_factor`,
  `other_axes`, `scan_speed`, `scan_speed_unit`, `scan_step`.
- **Experiment**: vendor, instrument kind X-ray diffractometer, model/serial where the file names
  them, software and version; technique X-ray diffraction (CHMO:0000156); parameters `anode`,
  `wavelength_kalpha1`, `wavelength_kalpha2`, `wavelength_kbeta` (Å), `kalpha2_kalpha1_ratio`,
  `tube_voltage` (kV), `tube_current` (mA), `counting_time`/`step_time` (s),
  `goniometer_radius` (mm), `detector`, `wavelength_intended`; operator, sample id/name, start and
  end.

## Validation

`tests/series_oracle/mod.rs` with `oracle/series_oracle.py`: vendor exports of the same scans
(positions and intensities at 256 rows: Data Collector CSV and `.xy` for XRDML, the UXD
conversion and a `.xy` export for RAW, M-DaC CSV and PDXL ASCII for RAS) and the header facts
they print (anode, wavelengths, voltage, current, step time, sample, user, date); geddes 1.0.0
(MIT, run as a black box) on every RAW, BRML, RAS and RASX file it opens (exactly); the RAW4 and
BRML files of the same two measurements agree. One further depositor per format is held out
(XRDML, RAW, BRML, RAS).

## Vocabulary (public API of `openreadout-xrd`)

| identifier | meaning |
| --- | --- |
| `XrdmlReader` | reader of PANalytical XRDML |
| `BrukerRawReader` | reader of Bruker DIFFRAC `.raw` |
| `BrmlReader` | reader of Bruker DIFFRAC.SUITE `.brml` |
| `RasReader` | reader of Rigaku `.ras` |
| `RasxReader` | reader of Rigaku `.rasx` |
| `XRDML_FORMAT_ID` | `panalytical-xrdml` |
| `BRUKER_RAW_FORMAT_ID` | `bruker-raw` |
| `BRML_FORMAT_ID` | `bruker-brml` |
| `RAS_FORMAT_ID` | `rigaku-ras` |
| `RASX_FORMAT_ID` | `rigaku-rasx` |
