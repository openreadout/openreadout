# Agilent MassHunter `.d`

Agilent MassHunter acquisition software writes LC/MS runs (Q-TOF and triple quadrupole, including ion-mobility frames) as `.d` data directories. OpenReadout returns the run as mass spectra (profile and centroid, MS/MS precursors, MRM transitions), TIC and BPC traces and the device signals, and exports them to mzML.

Derived from public MetaboLights data directories by hex dump and comparison against the depositors' own mzML conversions. The record fields of the scan index come from the `MSScan.xsd` schema that the instrument software writes into every data directory (file content, not vendor documentation). No Agilent library (MassHunter Data Access Components or otherwise), header, document or converter source was used. Details: `docs/provenance/agilent-masshunter.md`.

**Confidence markers.** Every field below is one of:

- **validated** — decoded by `openreadout-agilent-ms` and compared value-for-value with the depositor's conversion on every corpus file that has it;
- **inferred** — consistent across every corpus file and with the structure around it, but no independent value to compare against (the evidence is given);
- **schema** — named by the file's own `MSScan.xsd`; stored and shown by `info --view full`, not otherwise interpreted.

All integers and floats little-endian.

## Directory map

```
<name>.d/
  AcqData/
    MSScan.bin          scan index: one fixed-size record per scan (validated)
    MSScan.xsd          the record's field list (schema, read to build the layout)
    MSPeak.bin          centroid / quadrupole point lists, uncompressed (validated)
    MSProfile.bin       profile spectra, LZF-compressed (inferred; see below)
    MSMassCal.bin       per-scan flight-time calibration (validated)
    DefaultMassCal.xml  default calibrations and the polynomial's power flags (validated)
    MSTS.xml            time segments with their scan counts (checked by `check`)
    Contents.xml        acquisition time, status, software version
    Devices.xml         modules: name, model number, serial number
    sample_info.xml     sample name, position, method path, operator, ...
    AcqMethod.xml       method report (the method name is read)
    <Module><n>.cd/.cg  device signals: descriptor + data (validated layout, see below)
    MSPeriodicActuals.bin, MSActualDefs.xml, DAD1.sd/.sp, MSTree2.bin, *.m/   listed, not decoded
    IMSFrame.bin        ion-mobility frame index (6560 IM-MS; validated, see below)
    IMSFrame.xsd        the frame record's field list (schema, read to build the layout)
    IMSFrameMeth.xml    frame methods: drift-bin width, flight-time grid, polarity, calibration id
```

The reader accepts the `.d` directory, its `AcqData` directory or any file inside `AcqData`; names are matched case-insensitively. A directory is claimed when `AcqData/MSScan.bin` exists (a Bruker `.d` holds `analysis.*`, a ChemStation `.D` top-level `.ch`/`.ms` files; the three never overlap in the corpus).

## Common header of the `.bin` files

Every `.bin` file starts with a 68-byte (0x44) header: a u16 file-type tag at byte 0 and zeros up to 0x44, except in `MSScan.bin` (below).

| tag | file |
| --- | --- |
| `0x0101` | `MSScan.bin` (`TAG_SCAN`) |
| `0x0102` | `MSProfile.bin` (`TAG_PROFILE`) |
| `0x0103` | `MSPeak.bin` (`TAG_PEAK`) |
| `0x0104` | `MSMassCal.bin` (`TAG_MASS_CAL`) |
| `0x0109` | `MSPeriodicActuals.bin` (not decoded) |
| `0x0116` | `IMSFrame.bin` (`TAG_IMS_FRAME`) |

`DATA_START` = 0x44 is where data begins in each.

## `MSScan.bin`

| offset | type | our name | meaning | status |
| --- | --- | --- | --- | --- |
| 0 | u16 | `tag` | 0x0101 | validated |
| 0x48 | i32 | `layout_version` | 5 in the triple-quadrupole files, 6 in all Q-TOF files | inferred |
| 0x4C | i32 | `block_count` | spectrum blocks per record: 1 (peaks only) or 2 (profile + peaks) | validated (record strides) |
| 0x58 | i32 | `first_record` | byte offset of record 0 (296 in layout 5, 228 in layout 6) | validated |
| 0x5C … | | | a per-file table (scan methods), not decoded | unknown |

`ScanFileHeader` holds these. Records follow back to back up to the end of the file; a file whose length is not `first_record + n × record length` has a cut-off last record (`check`: `truncated`, exit 4; `info` exposes the complete records with a note).

### Record layout (`ScanLayout`, `Field`, `FieldType`)

The record is the `ScanRecordType` sequence of the file's `MSScan.xsd`, **without** the elements inside XML comments, in schema order and without padding: `xs:int` 4 bytes (`Int32`), `xs:long` 8 (`Int64`), `xs:double` 8 (`Float64`), `xs:short` 2 (`Int16`), `xs:byte` 1 (`Int8`), `xs:float` 4 (`Float32`); nested complex types are inlined (our names `Parent.Child`). One exception: **`ChromScaleFactor` is declared `xs:double` but stored as a 4-byte float** — the record length of every layout-6 file (220 bytes with one block, 284 with two) only works out that way, and its values (1.0 and small positive floats) decode sensibly only as f32. The repeated `SpectrumParamValues` element is laid out separately (`block_fields`, `block_len`) and appears `block_count` times at the end of the record (`record_len`).

Layout 5 schema (triple quadrupole, 196-byte records): `ScanID`, `ScanMethodID`, `TimeSegmentID`, `ScanTime`, `MSLevel`, `ScanType`, `TIC`, `BasePeakMZ`, `BasePeakValue`, `CycleNumber`, `Status`, `IonMode`, `IonPolarity`, `Fragmentor`, `CollisionEnergy`, `MzOfInterest`, `SamplingPeriod`, `MeasuredMassRangeMin`, `MeasuredMassRangeMax`, `Threshold`, `IsFragmentorDynamic`, `IsCollisionEnergyDynamic`, `DataDependentScanParamType` (`DDScanID`, `DDScanParamMz`), then blocks of `SpectrumFormatID`, `SpectrumOffset`, `ByteCount`, `PointCount`, `MinY`, `MaxY`, `MinX`, `MaxX`.

Layout 6 schema (Q-TOF, 156-byte fixed part): `ScanID`, `ScanMethodID`, `TimeSegmentID`, `ScanTime`, `MSLevel`, `ScanType`, `TIC`, `BasePeakMZ`, `BasePeakValue`, `CalibrationID`, `CycleNumber`, `IonMode`, `IonPolarity`, `Fragmentor`, `CollisionEnergy`, `MzOfInterest`, `AbundanceLimit`, `SamplingPeriod`, `Threshold`, `ChargeState`, `ChromScaleFactor`, `MassCalOffset`, `ActualsOffset`, `NumOfActualsPerScan`, `DataDependentScanParamType`, then 64-byte blocks of `SpectrumFormatID`, `SpectrumOffset`, `ByteCount`, `PointCount`, `UncompressedByteCount`, `MinX`, `MaxX`, `MinY`, `MaxY`, `MeasuredNoise`.

**Ion-mobility layout** (`ims`; 6560 IM-MS data, acquisition software B.06): the `ScanRecordType` has no `SpectrumParamValues`; it holds `ScanID`, `FrameID`, `BaseAbund`, `BaseMsBin`, `DetectorGain`, `DriftBin`, `FirstNonzeroMsBin`, `LastNonzeroMsBin`, `TIC`, `TfsBasePeakAbund`, `TfsBasePeakMz`, `MsProfSpecFmtId`, `MsProfByteCount`, `MsProfOffset`, `MsProfPointCount`, `MsProfFullByteCount`, (`MsProfNzPointCount` in the newer file), `MsPeakSpecFmtId`, `MsPeakByteCount`, `MsPeakOffset`, `MsPeakPointCount`, `MsPeakMaxX`, `MsPeakMinX` (106 or 110 bytes). One record per frame and drift bin; `DriftBin` 0 is the frame's summed spectrum. The reader makes one block from the `MsProf*` fields and takes time, level and polarity from the frame (below).

When a directory has no `MSScan.xsd` these two lists are used by layout version (`FALLBACK_XSD_V5`, `FALLBACK_XSD_V6`); other layouts would need their schema.

### Normalized record (`ScanRecord`)

| our field | schema field | meaning | status |
| --- | --- | --- | --- |
| `record_offset` | | byte offset of the record | |
| `scan_id` | `ScanID` | scan number; the export's native id is `scanId=<ScanID>`; not contiguous in Q-TOF files | validated |
| `scan_time_min` | `ScanTime` | retention time, minutes | validated (1e-11 min) |
| `ms_level` | `MSLevel` | 1 or 2 (MRM scans are 2) | validated |
| `scan_type` | `ScanType` | 1 = full scan, 256 = MRM, 512 = product-ion (MS/MS) in the corpus | inferred |
| `tic` | `TIC` | total ion current; the export's TIC chromatogram and `total ion current` | validated |
| `base_peak_mz`, `base_peak_value` | `BasePeakMZ`, `BasePeakValue` | base peak | validated |
| `ion_polarity` | `IonPolarity` | 0 = positive, 1 = negative | validated |
| `ion_mode` | `IonMode` | 64 in every corpus file (electrospray sources) | inferred |
| `fragmentor` | `Fragmentor` | fragmentor voltage; stored negated in data-dependent MS/MS scans | inferred |
| `collision_energy` | `CollisionEnergy` | collision energy, V | validated |
| `mz_of_interest` | `MzOfInterest` | MS/MS precursor m/z; MRM Q1 m/z | validated (see precursor note) |
| `charge_state` | `ChargeState` | precursor charge (0 = unknown) | validated |
| `calibration_id` | `CalibrationID` | which `DefaultMassCal.xml` calibration the scan uses | validated |
| `mass_cal_offset` | `MassCalOffset` | offset of the scan's record in `MSMassCal.bin` (0 when the file is absent) | validated |
| `sampling_period` | `SamplingPeriod` | digitizer sampling period, ns (the profile bin width) | validated |
| `parent_scan_id` | `DataDependentScanParamType.DDScanID` | survey scan of a data-dependent MS/MS scan (equals `ScanID` otherwise); the export's `spectrumRef` | validated |
| `scan_method_id`, `time_segment_id`, `cycle_number` | `ScanMethodID`, `TimeSegmentID`, `CycleNumber` | acquisition bookkeeping | schema |
| `values` | all | every fixed-part field as stored, shown by `info --view full` | schema |
| `frame_id`, `drift_bin` | `FrameID`, `DriftBin` | ion-mobility frame and drift bin | validated |
| `drift_time_ms` | | `(DriftBin − 1) × FrameDtPeriod` of the frame's method; the export's `ion mobility drift time` | validated (8,377 spectra, exact) |
| `blocks` | `SpectrumParamValues` | spectrum blocks | |

`SpectrumBlock`: `format_id` (1 = profile in `MSProfile.bin`, `FORMAT_PROFILE`; 2 = centroids in `MSPeak.bin`, `FORMAT_PEAK`; 3 = quadrupole points in `MSPeak.bin`, `FORMAT_QUAD_PEAK`), `offset`, `byte_count`, `point_count`, `uncompressed_byte_count`, `min_x`/`max_x` (m/z range; the export's scan window), `min_y`/`max_y`.

## Spectrum data

**Peaks (`MSPeak.bin`, formats 2 and 3)** — validated. `point_count` little-endian f64 x values, then `point_count` f32 abundances (`byte_count` = 12 × points; `decode_peaks` → `PeakData`). Quadrupole points (format 3) and peaks in directories without a calibration store m/z directly; time-of-flight centroids store the **flight time** (ns), converted with the calibration below. The reader tells the two apart by comparing the first stored x with the block's `MinX` (an m/z).

**Triple-quadrupole point lists (layout 5, 8 bytes per point)** — validated. Format-3 blocks of MRM files, and format-1 blocks whose `byte_count` is 8 × `point_count` in files that have `MSPeak.bin` (the MRM time segments of `Thyrxox 5 TS Diff Scan B`, 4 transitions per scan), hold `point_count` f32 m/z then `point_count` i32 counts, in `MSPeak.bin`. Evidence: one-point MRM blocks against the export's SRM chromatograms (`MRM Neg C5`, 4/4 transitions exact); in the four-point blocks (571 scans) the counts sum to `TIC`, the extremes equal `MinY`/`MaxY`, the first and last m/z equal `MinX`/`MaxX`, and the largest count sits at `BasePeakMZ` (570/571; the other is a tie). They are MRM transition lists, reported as centroids with activation CID.

**Single-quadrupole GC/MS point lists (layout 2, format 1 in `MSPeak.bin`)** — validated. A GC/MSD (5975, 5977) data directory holds no `MSProfile.bin`, and every scan's format-1 block lies in `MSPeak.bin`: `16 × point_count` bytes, `point_count` f64 m/z then `point_count` f64 abundances. The blocks tile `MSPeak.bin` after its 68-byte header. The block's `MinX`/`MaxX` are a twentieth of the first and last m/z, so they do not give the scan window. Evidence: 12,257 scans of a 5977 run (MTBLS12630) and 5,808 of a 5975-era run (MTBLS1980) equal the depositors' ProteoWizard conversions point for point.

**7010C GC triple-quadrupole full scans (layout 5, format 1, 8 bytes per point)** — inferred. The same 8-byte lists as the MRM point lists above, but with f32 abundances where those hold i32 counts (`eight_byte_abundances`). The block's `MaxY` tells them apart: the f32 reading is taken only when its maximum equals `MaxY` and the i32 reading's does not. Evidence: on every scan of MTBLS13904's QC run the abundances sum to `TIC` and the most intense point is `BasePeakMZ`; the depositor's ANDI-MS export has the same point counts and m/z, with abundances stored at reduced precision.

**Time-of-flight centroids of 16 bytes per point (a 7200 GC/Q-TOF, layout 6, format 2)** — refused. f64 flight time and f64 abundance per point; `MinX` holds a flight time too. The run seen has no calibration the reader can apply, so these scans are refused (exit 6) rather than reported with flight times as m/z. When a calibration exists, the most intense point must calibrate to the record's `BasePeakMZ` or the scan is refused.

**Run-length profiles outside ion-mobility data (format 1)** — validated. Some Q-TOF profile blocks use the ion-mobility encoding below (the 0x90 word at byte 16 carries the block's `point_count`), stored uncompressed; `UncompressedByteCount` is then 0 or the dense size. `decode_profile` decodes them with `decode_ims_profile` and fills skipped bins with zeros. Evidence: rainbow-api (black box) reads the same counts on every scan of three files, and they sum to `TIC`.

**Quadrupole profiles (layout 5, format 2 in `MSProfile.bin`)** — validated. Triple-quadrupole full, product-ion and neutral-loss scans store `8 + 4n` bytes (`decode_quad_profile`): f32 first m/z, f32 step, then `n` i32 counts. The stored step is not the set one (0.1000061 where the method sets 0.1); the record's `SamplingPeriod` holds it as an f32 (0.1 → 0.10000000149), so bin *k* sits at `first + k × period` with the period read back as its shortest decimal (0.1), rounded to f32. With that, 922 spectra of four files are bit-identical to the export (GFb 182, APCI and MMI precursor-ion 170 and 337, Thyrxox 233); with the block's own step the m/z drift by up to 0.09 at the top of the range.

**Ion-mobility profiles (`MSProfile.bin` of IM-MS data)** — validated. `decode_ims_profile` → `ImsProfile`: f64 `first_x` (flight time of bin 0, ns: `MinMsBin` × `FrameMsXPeriod`), f64 `step_x` (0.5 ns), a u32 whose top byte is 0x90 (`IMS_PROFILE_FLAGS`, the only value seen; others are refused) and whose low 24 bits are `point_count` (= `MsProfPointCount`), an i32 equal to minus the first stored bin, then a stream of little-endian signed integers whose width starts at 4 bytes. A value ≥ 0 is the count of the current bin (then the next bin); a negative value `v` skips `!(v >> 2)` empty bins (0–31 in one byte) and sets the width of the following values from its low two bits: 3 → 1 byte, 2 → 2 bytes, 1 → 4 bytes, 0 → 8 bytes. So runs of small counts cost one byte each, and `0xfe` (no skip, 2-byte mode) precedes counts above 127. Evidence: on all 16,272 records of three files the decoded counts sum to `TIC`, their maximum equals `BaseAbund` at `BaseMsBin`, and the first bin equals `FirstNonzeroMsBin`; the 8,377 drift-bin spectra of the three exports are identical point for point (m/z within 0.1 ppm, which is the export's f32 rounding; counts exact).

**Profiles (`MSProfile.bin`, format 1)** — inferred. The block is LZF-compressed (`lzf_decompress`: control byte < 32 → that many plus one literal bytes; otherwise length = top 3 bits (+ next byte when 7) + 2, back-offset = ((low 5 bits) << 8) + next byte + 1) unless `byte_count` equals `uncompressed_byte_count`. Decompressed (`decode_profile` → `ProfileData`): f64 `first_x` (flight time of bin 0, ns), f64 `step_x` (bin width, ns; equals `SamplingPeriod`), then `point_count` i32 `counts`. Bin *i* sits at flight time `first_x + i·step_x`. Evidence: the counts of every decoded scan sum exactly to the record's `TIC`, their maximum equals `BasePeakValue`, and the calibrated m/z of the maximum bin equals `BasePeakMZ` to 2e-11 relative (MTBLS1334, 567 scans). The reader returns non-zero bins and the zero bins next to them.

## Ion-mobility frames (`FrameRecord`, `FrameMethod`)

`IMSFrame.bin` (`read_frames`): tag 0x0116, i32 at 0x48 = offset of the first record (76), then records laid out from `IMSFrame.xsd`'s `FrameRecordType` like the scan records (`frame_layout_from_xsd`, 130 bytes in the corpus). `FrameRecord`: `frame_id` (`FrameId`), `method_id` (`FrameMethodId`), `time_segment_id`, `cycle_number` (`CycleNumber`, −1 = none), `frag_class` (`FragClass`: 0 no fragmentation, 1 low energy, 2 high energy), `scan_time_min` (`FrameScanTime`: the export's scan start time of every drift spectrum in the frame, exact), `tic` (`FrameTic`), `base_abundance` (`FrameBaseAbund`), `mass_cal_offset` (`MassCalOffset`, 0 → default calibration), `ims_field`, `ims_pressure`, `ims_temperature` (`ImsField`, `ImsPressure`, `ImsTemperature`), `values`.

`IMSFrameMeth.xml` (`frame_methods` → `FrameMethod`): `id` (`FrameMethId`), `drift_period_ms` (`FrameDtPeriod`), `ms_period_ns` (`FrameMsXPeriod`), `min_ms_bin` (`MinMsBin`), `mass_cal_id` (`DefMassCalId`), `ion_polarity` (`IonPolarity`), `ionization_mode`, `frag_op_mode` (`FragOpMode`), `frag_energy` (`FragEnergy`), `frag_energy_ramp` (`FragEnergySegments` end points `db`, `e` as stored).

Spectra of IM-MS data are the drift-bin records (`DriftBin` > 0) in file order, as the exports list them; the frame sums are not listed (`spectra[0].extra.ion_mobility.frame_sum_spectra_not_listed`). MS level 2 for frames of `FragClass` 2 (all-ions high energy), else 1. The scan window is the calibrated grid: [`MinMsBin`, `MinMsBin` + `point_count` − 1] × `FrameMsXPeriod` (equals the export's). `native_id` stays `scanId=<ScanID>`; ProteoWizard numbers these spectra (frame − 1) × 354 + drift bin − 1 instead. All-ions MS2 spectra carry no precursor and no collision energy (the frame method holds a ramp over drift bins, reported in `info --view full` only; the export gives the ramp's last value, 30, and the scan window's centre as a precursor). TIC and BPC traces have one point per frame (`FrameTic`, `FrameBaseAbund`).

## Calibration (`Calibration`, `DefaultCalibration`)

Validated: bit-exact against a float64 export (MTBLS1334, 567 spectra), and against float32 exports equal after rounding ours to f32 on all but 66 of 1,327,310 peaks (one f32 ulp; MTBLS874 1810/1810 spectra identical, MTBLS7386 327/388).

```
m/z = (a · (t − t0))² − Σₖ cₖ · clamp(t, lo, hi)^pₖ
```

- `a`, `t0` — the "Traditional" step (two values).
- `range` = [`lo`, `hi`] and `terms` [(pₖ, cₖ)] — the "Polynomial" step: its first two values are the flight-time range of the calibrant ions (the correction is evaluated at the nearest end outside it), the rest the coefficients; the powers pₖ are the positions of the set bits of the step's `ValueUseFlags`, lowest first (flags 214 → powers 1, 2, 4, 6, 7; flags 232 → 3, 5, 6, 7). The correction is summed first and subtracted once (this order reproduces float64 exports bit for bit).
- **Per-scan values** (`MSMassCal.bin`, `mass_cal_values`): at `MassCalOffset`, an i32 count (10) then that many f64: `a, t0, lo, hi, c₁…c₆`. The power flags come from the scan's `CalibrationID` in `DefaultMassCal.xml`.
- **Defaults** (`DefaultMassCal.xml`, `default_calibrations`): each `DefaultCalibration` (`id` = `DefaultCalibrationID`) lists `Step`s with `CalibrationFormula` (`formulas`), `ValueUseFlags` (`flags`) and `Value`s (`values`, concatenated over steps). Used when there is no `MSMassCal.bin` (MTBLS1334).

## Device signals (`*.cd` + `*.cg`)

Validated layout (every corpus descriptor parses to its exact length; every block ends inside its data file; values are physically plausible), not compared with an export (the mzML files carry none).

`.cd` (`signal_directory` → `SignalDirectory`): the 0x44 header, then i32 (1), i32 `device_id` (the `DeviceID` in `Devices.xml`), i32 signal count; per signal (`SignalDef`): u8-counted `id` (`A`, `B`, …), u8-counted `description` (`Sig=254.0,4.0  Ref=off`, ` Pressure`), i32 `kind` (1 = detector signal, 2 = instrument reading), i64 `offset` into the `.cg`, i32 `count`, 40 bytes of constant words, u8-counted `unit` (`mAU`, `bar`, `°C`, `mL/min`, `%`), f64 `scale` (1.0), 8 bytes.

`.cg` block at `offset` (`signal_block`): f64 start time (min), f64 interval (min), then `count` f64 values. Exposed as traces after TIC and BPC: time (s) and value × scale.

## XML metadata (`Device`, `devices`, `sample_fields`, `tag_text`)

- `Contents.xml`: `AcquiredTime` → `extra.acquired_at`; `AcqSoftwareVersion` → `instrument.software_version`; `InstrumentName` → `extra.instrument_name`.
- `Devices.xml`: per `Device` — `id` (`DeviceID`), `name`, `model` (`ModelNumber`), `serial` (`SerialNumber`), `device_type` (`Type`), `driver_version`, `firmware_version`. The MS module (name containing TOF/Quad) gives `instrument.model` and `extra.instrument_serial`; `extra.ms_device` is its name.
- `sample_info.xml`: `Field` name/value pairs; `Sample Name` → `extra.sample_name` (and the run name), `Sample ID`, `Sample Position` → `extra.vial`, `Method` → `extra.method_summary.instrument_method`, `OperatorName` → `extra.operator`.
- `AcqMethod.xml`: `MethodName` → `extra.method`.
- `MSTS.xml`: `NumOfScans` per `TimeSegment`; `check` compares their sum with the index.

## Normalized output

- `spectra[0]`: one run; `scan_count`, `ms_levels`, `rt_range_s`, `instrument` (manufacturer Agilent Technologies, the MS module's model number, "MassHunter Data Acquisition" and its version), `extra` as above plus `polarities`, `scan_types`, `stored_spectra` (`centroid`, `profile`, `profile+centroid`), `method_summary.devices` (module names).
- Spectra: `scan_number` = `ScanID`, `native_id` `scanId=N`, `rt_s`, `polarity`, `centroided`, `precursor_mz`/`precursor_charge`/`collision_energy` for MS level ≥ 2 (the energy's magnitude: negative-ion scans store it negated, −20 where the method sets 20; neutral-loss scans, `ScanType` 2048, report no precursor: their `MzOfInterest` is the loss, `extra.neutral_loss_mz`), `activation` `HCD` (beam-type CID, MS:1000422, as the exports label Q-TOF and triple-quadrupole product-ion scans) and `CID` for MRM point lists, ion-mobility `extra.frame`, `extra.drift_bin`, `extra.drift_time_ms`, `scan_window_mz` = [`MinX`, `MaxX`], `total_ion_current`, `base_peak_*`; `extra`: `scan_type`, `fragmentor_v`, `ion_mode`, `scan_method`, `time_segment`, `cycle`, `parent_scan`. `SpectrumView::Primary` returns the profile when recorded, `Centroid` the peak list.
- MRM (triple quadrupole) scans are one-point spectra: Q1 = `precursor_mz`, Q3 = the point's m/z; the corpus harness rebuilds every SRM chromatogram of the export from them.
- Traces: `TIC`, `BPC` (all scans), then each device signal.
- `export --to mzml` writes `scanId=N` native ids with the Agilent MassHunter nativeID/file-format terms.

**Precursor m/z.** Each MS/MS scan records its own `MzOfInterest`. ProteoWizard's exports of the auto-MS/MS run (MTBLS874) report, for every scan, the mean over all MS/MS scans that share that precursor (up to 9.5e-6 relative from the scan's own value); we report the scan's value. In the targeted run (MTBLS1334) all values agree exactly.

## Vocabulary

| identifier | where | meaning |
| --- | --- | --- |
| `AgilentMsReader` | lib | the reader |
| `FORMAT_ID` | lib | `agilent-masshunter` |
| `AgilentDataset` | dataset | an open directory |
| `dataset_dir`, `acq_dir` | dataset | resolve the `.d` / `AcqData` directory from a path |
| `Device`, `devices`, `id`, `name`, `model`, `serial`, `device_type`, `driver_version`, `firmware_version` | dataset | `Devices.xml` |
| `sample_fields`, `tag_text` | dataset | `sample_info.xml` / simple XML text |
| `open`, `records`, `layout`, `calibration` | dataset | dataset methods |
| `DATA_START`, `TAG_SCAN`, `TAG_PROFILE`, `TAG_PEAK`, `TAG_MASS_CAL`, `TAG_IMS_FRAME` | layout | file header |
| `FORMAT_PROFILE`, `FORMAT_PEAK`, `FORMAT_QUAD_PEAK` | layout | spectrum block kinds |
| `u16_at`, `i32_at`, `i64_at`, `f32_at`, `f64_at` | layout | bounds-checked readers |
| `FieldType`, `Int8`, `Int16`, `Int32`, `Int64`, `Float32`, `Float64`, `size`, `read` | layout | stored field types |
| `Field`, `ty`, `offset` | layout | one record field |
| `ScanLayout`, `fields`, `fixed_len`, `block_fields`, `block_len`, `ims`, `record_len`, `field` | layout | record layout |
| `scan_layout_from_xsd`, `FALLBACK_XSD_V5`, `FALLBACK_XSD_V6` | layout | layout from the schema |
| `ScanFileHeader`, `tag`, `layout_version`, `block_count`, `first_record`, `scan_file_header` | layout | `MSScan.bin` header |
| `SpectrumBlock`, `format_id`, `byte_count`, `point_count`, `uncompressed_byte_count`, `min_x`, `max_x`, `min_y`, `max_y` | layout | spectrum block |
| `ScanRecord`, `record_offset`, `scan_id`, `scan_time_min`, `ms_level`, `scan_type`, `tic`, `base_peak_mz`, `base_peak_value`, `ion_polarity`, `ion_mode`, `fragmentor`, `collision_energy`, `mz_of_interest`, `charge_state`, `calibration_id`, `mass_cal_offset`, `sampling_period`, `parent_scan_id`, `scan_method_id`, `time_segment_id`, `cycle_number`, `frame_id`, `drift_bin`, `drift_time_ms`, `blocks`, `values`, `block`, `read_record` | layout | scan record |
| `lzf_decompress` | layout | LZF decoder |
| `PeakData`, `x`, `y`, `decode_peaks`, `eight_byte_abundances` | layout | peak block |
| `ProfileData`, `first_x`, `step_x`, `counts`, `decode_profile` | layout | profile block |
| `decode_quad_profile` | layout | quadrupole profile block |
| `ImsProfile`, `bins`, `decode_ims_profile`, `IMS_PROFILE_FLAGS` | layout | ion-mobility profile block |
| `FrameRecord`, `frame_id`, `method_id`, `frag_class`, `base_abundance`, `ims_field`, `ims_pressure`, `ims_temperature`, `frame_layout_from_xsd`, `read_frames` | layout | `IMSFrame.bin` |
| `FrameMethod`, `drift_period_ms`, `ms_period_ns`, `min_ms_bin`, `mass_cal_id`, `ionization_mode`, `frag_op_mode`, `frag_energy`, `frag_energy_ramp`, `frame_methods` | layout | `IMSFrameMeth.xml` |
| `Calibration`, `a`, `t0`, `range`, `terms`, `from_values`, `mz`, `mass_cal_values` | layout | flight time → m/z |
| `DefaultCalibration`, `flags`, `formulas`, `default_calibrations` | layout | `DefaultMassCal.xml` |
| `SignalDef`, `description`, `kind`, `count`, `unit`, `scale`, `SignalDirectory`, `device_id`, `signals`, `signal_directory`, `signal_block` | layout | device signals |

Schema field names quoted above (`ScanID`, `MzOfInterest`, …) are the file's own and appear only in `info --view full` output.
