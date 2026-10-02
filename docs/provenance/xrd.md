# X-ray diffraction (PANalytical XRDML, Bruker RAW/BRML, Rigaku RAS/RASX) provenance

## 2026-09-26 — before the XRDML reader was written (Richard Zimring with Claude as assistant)

**Public specification consulted:** the XRDML XML schema published by Malvern Panalytical at its
schema location, `http://www.xrdml.com/XRDMeasurement/1.5/XRDMeasurement.xsd` (version 1.5 of
2010-03-05, authors X. Fransen and H. Rozendaal; fetched 2026-09-26). Its annotations define the
model we read: `xrdMeasurements` → `xrdMeasurement` (`measurementType`, `sampleMode`) →
`usedWavelength` (`kAlpha1`, `kAlpha2`, `kBeta` in Å, `ratioKAlpha2KAlpha1`) → `scan`
(`appendNumber`, `mode`, `scanAxis`, `status`) → `header` (`startTimeStamp`, `endTimeStamp`,
`author`, `source/applicationSoftware`, `instrumentControlSoftware`, `instrumentID`) and
`dataPoints`: per axis a `positions` element with `commonPosition` (same for all points),
`startPosition`/`endPosition` (constant step: step = (end − start)/(points − 1), the number of
points is the number of intensities) or `listPositions` (one value per point);
`beamAttenuationFactors` (per point, or `commonBeamAttenuationFactor`); `commonCountingTime` or
`countingTimes` (seconds); `intensities` (unit fixed to `counts`): "measured intensities multiplied
by the corresponding beam attenuation factor". Files written by newer Data Collector versions
declare namespace versions 1.x-2.x and use `counts` for the same list (seen in the corpus files
below; the 1.5 schema names it `intensities`).

**Corpus files used:** listed in the entries below as they are added.

**Reference reader:** xylib (LGPL), run as a black box.

**Schema 2.1 consulted as well** (`http://www.xrdml.com/XRDMeasurement/2.1/XRDMeasurement.xsd`,
fetched 2026-09-26): in 2.x files the list is `counts`, "counts as received by the detector, not
yet corrected for beam attenuator- or divergence factors", with optional
`commonBeamAttenuationFactor`/`beamAttenuationFactors` and
`commonDivergenceCorrection`/`divergenceCorrections`. We multiply raw counts by the attenuation
factor (the 1.5 schema defines `intensities` as counts multiplied by it) and keep the raw counts
as a channel; how a divergence correction combines is not defined, so such files are refused.

**Corpus files used (XRDML):** `xrd-zenodo15557974-car24014` (Empyrean, XRDML 2.1 `counts`, with
the Data Collector CSV export), `xrd-zenodo5484779-rutile-nb5` (X'Pert PRO, 1.5 `intensities`, CSV
export), `xrd-zenodo15498085-nn` and `xrd-zenodo20826400-kpc-low` (with `.xy` exports),
`xrd-yadg-210520step1` (1.6, `.xy` and CSV exports; yadg test data, GPL-3.0 data files),
`xrd-zenodo4635553-sand-818-28` and `xrd-zenodo14246011-0p1fe-unheated` (no exports). Inferred
from them: every scan's positions and intensity lists; the start/end positions of the scan axis
(`2Theta` for `Gonio` and `2Theta-Omega` scans) give exactly the exports' angles (to 1e-9°); the
exports' intensities equal the stored values (no attenuation factors in these files); the CSV
exports' anode, Kα1/Kα2 wavelengths, generator voltage and current and time per step equal the
file's `xRayTube`, `usedWavelength` and `commonCountingTime`. One further depositor is held out.

## 2026-09-26 — before the Bruker RAW reader was written

**Permissive prior art consulted (read as documentation):**

- geddes (Jiacheng Wang, MIT licence: `LICENSE` of github.com/jcwang587/geddes, GitHub licence
  API), commit c8a262d74ffdfdd809c3d0a324da7defb27d2563: `src/bruker.rs` and `docs/formats.md`.
  Taken from it: `RAW1.01` files (Bruker's "version 3") have a 712-byte file header with the range
  count at byte 12; each range starts with a u32 header size (≥ 260), u32 point count, float64
  start 2θ at +16, float64 step at +176, u32 scan type at +196, u32 bit mask of per-point varying
  parameters at +248, u32 data-record size at +252 (4 + 8 per varying bit), u32 size of an extra
  record at +256; data records follow the header and the extra record; the first float32 of a
  record is the intensity, and a measured 2θ follows when bit 0 is set. `RAW4.00` files have a
  61-byte header, then segments of u32 kind and u32 length; a range starts with kind 0 or 160: a
  160-byte header with the scan type text at +32 (24 bytes), float64 start at +72, float64 step
  at +80, u32 points at +88, u32 data-record size at +136, u32 size of the drive records that
  follow at +140; drive records (kind 50, ≥ 64 bytes) with a flag at +8, the drive name at +12
  (24 bytes) and its position at +56. geddes (1.0.0, installed from PyPI) is also run as an
  independent reader in the oracle.
- FAIRmat readers-xrd (Apache-2.0), commit 3f3eb8b307ed13a494ad44f3f3998f8c9120219b,
  `src/fairmat_readers_xrd/bruker_raw_parser.py`: `RAW4.00` text records named `USER`, `SITE`,
  `SAMPLEID`, `COMMENT`, `CREATOR`.

**Inferred from hex dumps of development files** (scratch scripts, not committed), compared with
Bruker's own UXD conversion of the same file (`_SAMPLE`, `_SITE`, `_USER`, `_DATEMEASURED`,
`_ANODE`, `_WL1`-`_WL3`, `_WLRATIO`, `_GONIOMETER_RADIUS`, `_STEPTIME`, `_STEPSIZE`, `_START`,
`_THETA`, `_KV`, `_MA`, detector `_HV`, `_GAIN`, `_LLD`, `_ULD`):
- `RAW1.01`: 0x10 date `MM/DD/YY`, 0x1A time; 0x24 user, 0x6C site, 0x146 sample id (NUL-padded
  text); 0x224 u32 goniometer code, 0x228 u32 stage code, 0x234 float32 goniometer radius (mm);
  0x260 anode text; float64 Kα1 at 0x270, Kα2 at 0x278, Kβ at 0x280, Kα2/Kα1 ratio at 0x288; 0x290
  wavelength unit text (`A`). Per range (offsets from its start): float64 start θ at +8, float32
  detector high voltage at +100, gain +104, lower and upper discriminator +108/+112, float32 step
  time (s) at +192, u32 kV at +224, u32 mA at +228, float64 range wavelength at +240.
- `RAW4.00`: 0x0C date `MM/DD/YYYY`, 0x18 time; text records (kind 10): u32 kind, u32 length, 4
  bytes, a 24-byte name, the value (UTF-8, `\n`-terminated) to the record end; an instrument
  record (kind 30) with the anode text and float64 average Kα, Kα1, Kα2, Kβ and Kα2/Kα1 ratio.

## 2026-09-26 — before the BRML, RAS and RASX readers were written

**Prior art consulted:** geddes (MIT, commit c8a262d) `docs/formats.md` and
`docs/reading-patterns.md` (BRML: the `Measured` data route; RAS/RASX: a third attenuator column
that geddes ignores) and `src/xml.rs` (BRML columns found through the data route's `DataViews`;
RASX `Data*/Profile*.txt`); FAIRmat readers-xrd (Apache-2.0) `readers.py` (the same members).

**Inferred from the files** (all self-describing text or XML; scratch scripts, not committed):
- BRML (`zip`): `Experiment0/DataContainer.xml` lists the raw-data members
  (`RawDataReferenceList/string`), the instrument (`InstrumentName`, `SerialNo`,
  `DeviceTypeDesc`, `IcsVersion`), the writer version (`CreatingVersion`) and the measurement
  (`MeasurementInfo@UserName`, `SampleName`, `Comment`). Each `RawData*.xml` holds
  `TimeStampStarted`/`TimeStampFinished` (ISO-8601 with offset), `DataRoutes/DataRoute`
  (`RouteFlag` `Measured` for acquired data) with `ScanInformation` (`ScanName`, `VisibleName`,
  `MeasurementPoints`, `TimePerStep`, `ScanAxes/ScanAxisInfo` with `AxisId`, `Unit`, `Start`,
  `Stop`, `Increment`), one `Datum` element per point (comma-separated numbers) and `DataViews`:
  `RawDataView` elements with `Start` and `Length` column positions, a `LogicName`
  (`MeasuredTime` s, `AbsorptionFactor`), a varying view whose `FieldDefinitions` name the axes
  (`TwoTheta`, `Theta`) in column order, and the recorded view (`Recording@LogicName`, `Unit
  @Base="Counts"`, `Size X×Y`). `FixedInformation/Instrument` holds `WaveLengthAlpha1`,
  `WaveLengthAlpha2`, `WaveLengthBeta`, `WaveLengthRatio` (Å), `TubeMaterial`, `Voltage` (kV),
  `Current` (mA) and the goniometer `Radius` (mm). Two development files (`Eptachori`, `TiO2 450
  thin film Si wafer`) come with the RAW4 file of the same measurement: their counts agree exactly and their positions
  to 5e-5° (the `Datum` rows print four decimals; RAW4 stores start and step).
- RAS (text, Latin-1): `*RAS_DATA_START`, then per scan `*RAS_HEADER_START` … `*KEY "value"` …
  `*RAS_HEADER_END`, `*RAS_INT_START`, rows of `x intensity attenuation`, `*RAS_INT_END`;
  `*RAS_DATA_END`. Keys used: `MEAS_SCAN_AXIS_X`, `MEAS_SCAN_UNIT_X`, `MEAS_SCAN_UNIT_Y`,
  `MEAS_SCAN_START`/`STOP`/`STEP`, `MEAS_SCAN_START_TIME` (`MM/DD/YYYY HH:MM:SS`),
  `MEAS_SCAN_SPEED` and its unit, `MEAS_DATA_COUNT`, `HW_XG_TARGET_NAME`,
  `HW_XG_WAVE_LENGTH_ALPHA1`/`ALPHA2`/`BETA`, `MEAS_COND_XG_VOLTAGE`/`CURRENT`,
  `FILE_OPERATOR`, `FILE_SAMPLE`, `FILE_COMMENT`, `FILE_SYSTEM_NAME`, `HW_COUNTER_SELECT_NAME`.
- RASX (`zip`): `Data*/Profile*.txt` (tab-separated `x intensity attenuation`, a UTF-8 BOM on
  the first line) and `Data*/MesurementConditions*.xml` (the same keys as RAS as XML elements).
- The attenuation column is 1 in every development file; values are returned as stored, and a
  file with other attenuation factors is reported (the meaning of the factor is not validated).
