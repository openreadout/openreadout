# Sartorius (ForteBio) Octet `.frd`

Octet biolayer-interferometry instruments write one `.frd` result file per biosensor. OpenReadout returns the sensor's wavelength shift against time through every assay step, the step table and the instrument facts. Kinetic fits are not in these files.

Derived from public files of two depositors (OctetRED96e and OctetRED384, DataAcquisition 11.1), compared value by value with pykingenie (MIT) run as a black box. Provenance: `docs/provenance/sartorius-octet.md`. Crate: `openreadout-biophys`.

| format id | files | reads | confidence |
| --- | --- | --- | --- |
| `sartorius-octet-frd` | `.frd` result files of Octet biolayer-interferometry instruments (one per biosensor) | the sensor's wavelength shift against time through every assay step, the step table (sample, well type, concentration, molar concentration, molecular weight, temperature, times, shake speed, status), instrument model and serial, acquisition software, operator, start time, sensor type and role | generated (`book/src/reference/evidence.md`) |

Not read: kinetic fits (made by the analysis software; not in `.frd` files); non-kinetics
experiments (no `KineticsData`: refused with exit 6); the experiment method (`.fmf`) and plate
definition files next to the `.frd` files.

## Layout

UTF-8 XML, root `ExperimentResults`.

| element | content | our reading |
| --- | --- | --- |
| `ExperimentInfo` | `RTDVersion`, `RunID`, `ExperimentType`/`ExperimentSubType`, `StartDateTime` (local), `MachineName`, `UserName`, `PlateName`, `SensorName`, `SensorPlate`, `SensorType`, `SensorRole`, `WritingSW`, `InstrumentType`, `InstrumentSerial`, `InstrumentFW`, `IntegrationTime`, `DSPType`, `DSPStitching`, `HasManifest` | experiment facts; vendor tree `octet`; `RTDVersion` → `format_version` |
| `KineticsData/Step` | `CommonData` (`SampleLocation`, `SampleID`, `SampleGroup`, `SampleInfo`, `SampleRow`, `SamplePlate`, `WellType`, `Concentration`, `ConcentrationUnits`, `MolarConcentration`, `MolarConcUnits`, `MolecularWeight`, `Temperature`, `StartTime`, `AssayTime`), `FlowRate`, `StepType`, `StepName`, `StepStatus`, `ActualTime`, `CycleTime` | table 0 `steps` (−1 → not set) |
| `Step/AssayXData`, `Step/AssayYData` | base64 of little-endian float32: time (s, continuous across steps) and wavelength shift (nm); `Points` = the value count | trace 0 |

## What the reader returns

- **Trace 0** (`sensor <SensorName>`): channels `time` (s; the irregular axis), `response` (nm),
  `step` (1-based step of each point): every step's values concatenated in file order.
- **Table 0 `steps`**: `step`, `name`, `type`, `status`, `sample_id`, `well_type`,
  `sample_row`, `sample_location`, `concentration` with `concentration_unit`,
  `molar_concentration` with `molar_concentration_unit`, `molecular_weight`, `temperature` (°C),
  `start` (s, the step's first time), `assay_time`, `actual_time`, `cycle_time` (s),
  `flow_rate` (rpm), `points`.
- **Experiment**: vendor Sartorius (ForteBio), model, serial, software version (`WritingSW`),
  operator, start time, method (`KINETICS KBASIC`), sensor, sensor type and role, plate name,
  duration.
- `check`: every step's arrays decode to their stated point count; a step that starts before
  the previous one ends is a warning.

## Validation

`cargo test -p openreadout-corpus-tests --features corpus` (`tests/series_oracle/mod.rs`,
`oracle/octet_oracle.py`, pykingenie run as a black box): 256 sampled response and time values,
every step's point count, concentration, molar concentration, molecular weight, temperature,
assay, actual and cycle time and shake speed, and the model, serial, operator and start time.

## Vocabulary (public API of `openreadout-biophys`, Octet)

| identifier | meaning |
| --- | --- |
| `OctetReader` | reader of Sartorius Octet `.frd` files |
| `OCTET_FORMAT_ID` | `sartorius-octet-frd` |
