# Provenance: Sartorius (ForteBio) Octet `.frd` files

## 2026-09-26: initial reader (`sartorius-octet-frd`; Richard Zimring with Claude as assistant)

**Corpus files used (development):**

- Harvard Dataverse doi:10.7910/DVN/TTOPMD (CC0-1.0, Doyle L., "Figure7"; licence read from the
  dataset API 2026-09-26): `210122_001.frd` … `210122_008.frd` (OctetRED96e, DataAcquisition
  11.1.2.24, SAX sensors, 64 steps each).
- GitHub osvalB/pykingenie (MIT, commit 3a48b165): `test_files/230309_001.frd` … `_008.frd`
  (OctetRED384, DataAcquisition 11.1.1.19, 115 steps each).

Two depositors: no file is held out. A third public set (GitHub Katona-lab/BayesLayerAnalyst,
OctetRED96, DataAcquisition 10.0.1.3) has no licence and is not used.

**Prior art consulted:** pykingenie (MIT, https://github.com/osvalB/pykingenie,
`src/pykingenie/octet.py`), read as documentation: it decodes `AssayXData`/`AssayYData` of every
`KineticsData/Step` as base64 of little-endian float32 values. It is also the oracle, run as a
black box (`pip install pykingenie`, `oracle/octet_oracle.py`).

**What we inferred (from the files):**

- A `.frd` file is UTF-8 XML, root `ExperimentResults`, one file per biosensor. `ExperimentInfo`
  holds `RTDVersion` (2.0 in both sets), `RunID`, `ExperimentType`/`ExperimentSubType`
  (`KINETICS`/`KBASIC`), `StartDateTime` (local, no zone), `MachineName`, `UserName`, `PlateName`,
  `SensorName` (the sensor's column position), `SensorPlate`, `SensorType`, `SensorRole`,
  `WritingSW` (`DataAcquisition.exe <version>`), `InstrumentType`, `InstrumentSerial`,
  `InstrumentFW`, `IntegrationTime`, `DSPType`, `DSPStitching`, `HasManifest`.
- `KineticsData/Step` per assay step: `CommonData` (`SampleLocation`, `SampleID`, `SampleGroup`,
  `SampleInfo`, `SampleRow`, `SamplePlate`, `WellType`, `Concentration` and
  `ConcentrationUnits`, `MolarConcentration` and `MolarConcUnits`, `MolecularWeight`,
  `Temperature`, `StartTime`, `AssayTime`), `FlowRate` (shake speed), `StepType`, `StepName`,
  `StepStatus`, `ActualTime`, `CycleTime`, `AssayXData` and `AssayYData` with a `Points`
  attribute equal to the decoded value count in every step of both sets. `-1` marks an unset
  concentration or molecular weight.
- X is the time in seconds since the first step started (continuous across steps: a step's
  first X follows the previous step's last); Y is the wavelength shift in nm.
