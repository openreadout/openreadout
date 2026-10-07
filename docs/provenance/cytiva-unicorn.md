# Cytiva ÄKTA / UNICORN provenance

## 2026-09-25 — before the readers were written (Richard Zimring with Claude as assistant)

**Corpus files used** (all in `corpus/manifest.toml` with their licences, pinned to commits):

- `unicorn-res-pycorn-sample1`: `samples/sample1.res` of PyCORN (GPL-2.0, `pycorn/docs/LICENSE.txt`,
  commit d7747b6), an ÄKTAprime run (header text `UNICORN 3.10`, method dump "AKTAprime V2.01"),
  UV/conductivity/pH/pressure/temperature/concentration curves, 55 fractions, 19 logbook entries.
- `unicorn-res-unicornr-sample`: `tests/testthat/sample.res` of unicoRn (GPL-3.0, commit 67e48e5),
  an Ettan LC run (UNICORN 5-era strategy notes, header text `UNICORN 3.10`) with three UV
  wavelengths, conductivity, %conductivity, concentration, pressure, flow, two temperatures, one
  evaluated curve, fractions, an injection and method text blocks.
- UNICORN 7 result exports (`.zip`): `unicorn-zip-univiz-hitrap-1`, `-sec-1`, `-sec-2`, `-affinity-1`
  (univiz `docs/zip_files`, GPL-3.0, commit 80f5d03; ÄKTA pure 25, UNICORN 7.3.0.473),
  `unicorn-zip-fs-sample`, `unicorn-zip-fs-pd1-sec` (fictional-spoon-fplc-2-ids `data/akta`, MIT,
  commit b05b7d6; ÄKTA pure / pure 150L, UNICORN 7.3.0.473 and 7.1.0.378, one with a vendor peak
  table), and allotropy's two test files `unicorn-zip-allotropy-1` (a re-packed export whose
  members are named `Copy of …` and carry `.zip` suffixes; UNICORN 7.7, ÄKTA avant, a peak table
  and an evaluated curve) and `unicorn-zip-allotropy-single-uv` (a small synthetic export written
  by allotropy's authors) (MIT, commit 77748ee).
- One further depositor's UNICORN export is held out (`docs/benchmark/heldout.md`); it was not
  opened while writing the reader.

**Permissive prior art consulted:**

- allotropy 0.1.146 (Benchling, MIT; `allotropy-0.1.146.dist-info/licenses/LICENSE` read first):
  `parsers/cytiva_unicorn/reader/unicorn_zip_handler.py` (nested members are zip archives with
  trailing bytes after the end-of-central-directory record, cut at the last `PK\x05\x06` + 22; the
  `Xml` member of `SystemData`, `InstrumentConfigurationData` and `ColumnTypeData` holds XML with
  bytes before the first `<`), `structure/data_cube/{reader,converters,creator}.py` (a curve member
  holds `CoordinateData.Volumes` and `CoordinateData.Amplitudes` with `…DataType` members naming
  `System.Single[]`; volumes are the retention volume in ml). allotropy reads the floats from byte
  47 to 48 bytes before the end: that drops the first five and last twelve values; our decoding
  follows the public record layout below instead, and the corpus comparison allows for the shift.
- Microsoft [MS-NRBF] .NET Remoting Binary Format (a public Microsoft Open Specification,
  learn.microsoft.com/openspecs): SerializationHeaderRecord (record type 0, 17 bytes),
  ArrayOfPrimitiveType (record type 15: object id, length, primitive type; Single = 11),
  BinaryObjectString (record type 6: object id and a length-prefixed string with a 7-bit
  variable-length length), MessageEnd (record type 11).
- PKWARE APPNOTE (public) for zip and ZIP64 records (already implemented in `openreadout-core`).

**Copyleft readers, run as black boxes only:** PyCORN 0.19 (GPL-2.0; its PyPI
description and `docs/USAGE_pycorn_module.txt`, which are documentation, were read: `pc_res3`
returns `(volume, value)` pairs per block with the unit, and by default measures volumes from the
last injection "as UNICORN does"). Its `samples/sample1_2009Jun16no001_plot.jpg` (PyCORN's own
plot of sample1) was looked at as an oracle output: UV maximum ≈ 220 mAU near 172 ml, conductivity
≈ 15 mS/cm. The layout was derived from hex dumps.

### Inferred from hex dumps of the two `.res` files (scratch dumps, not committed)

- Header: `11 47 11 47` magic; u32 at 8 = directory offset (0x2b0 in both), u32 at 16 = file size;
  text at 0x18 `UNICORN 3.10` (the same in the ÄKTAprime file of 2009 and the Ettan LC file of
  2017: a file-layout version, not the software version). Two u32 Unix times at 0x68 and 0x6c; a
  u16 then a NUL-terminated user name at 0x74/0x76 (`prime`, `default`; the second file's logbook
  says "User default in control of the run"). 0x6c matches the run end in both files (method start
  in the logbook + the curves' duration, within 13 s, local time UTC+1/UTC+2); 0x68 matches the
  method start in the 2017 file and lies 34 min after it in the 2009 file, so it is reported as a
  file time, not as the start.
- Directory: 344-byte entries from the directory offset until an empty name. Bytes 0-5 a type word
  (`01 00 02 00 xx yy` for text/parameter blocks, `01 00 04 00 01 14` for curves, `01 00 04 00 03
  14` for an evaluated curve, `01 00 04 00 48 04` logbook, `… 44 04` fractions, `… 46 04`
  injections), bytes 6-301 the NUL-terminated name (curves: `<run name>:<run number>_<curve>`),
  then u32 data size, u32 allocated size (a multiple of 512), u32 data offset, u32 header size
  (240 for curves, 552 for event blocks, 0 for others).
- Curve and event blocks start with u16 6, u16 number of stored columns, u16 1, then 78-byte column
  descriptors: u16 78, u16 flags (0x8001 time, 0x8002 volume, 0x4001/0x4000 value), 40-byte name,
  16-byte unit, u16 storage code (0x0104 int32, 0x0308 float64, 0x044c 76-byte text), float64
  factor, float64 second value. A curve has three descriptors (time, volume, value) and stores
  (int32 volume, int32 value) pairs; value = stored × factor. The time column is not stored: its
  factor is the sampling interval in minutes (1/15 min for the ÄKTAprime; 0.2-2 s on the Ettan).
- Time of sample i = i × interval: on sample1 (second value 0) the logbook and fraction (time,
  volume) pairs agree with the curve volumes at that time to 0.1 sample on average. The Ettan
  file's second value is 14-51, always below one sampling interval in milliseconds (15 at 0.2 s,
  46 at 0.5 s, 51 at 1 s, 41 at 2 s); read as a start offset in milliseconds it moves times by at
  most 0.05 s. Its flow is too low (0.05-0.2 ml/min on a 0.01 ml volume grid) for the event
  comparison to decide; we apply it as milliseconds and record the assumption.
- Event blocks: 180-byte records (float64 time min, float64 volume ml, 76-byte text, 76-byte text,
  float64 value, int32 flags). Fraction records carry the tube label ("1D1", "Waste"); injection
  records the injection number.
- Text blocks: `CreationNotes`, `Methods`, `MethodStrategyNotes`, `ResultStrategyNotes`,
  `Techniques`, `METHODINFO` (method path) are plain text; `CONFIG`, `CALIB`, `ColumnsChoosen`,
  `Template`, `FracxyConfig`, `DOCUMENT`, `StrategyInformation` and the per-run block named by the
  run number are binary and are listed, not decoded.
- PyCORN (black box) agrees with these values except pH on sample1, which it reports 10× larger
  (146.4 where the descriptor factor 0.01 gives 14.64); we use the factor the file declares.

### Inferred from the UNICORN 7 exports (scratch scripts, not committed)

- The export is a zip: `Result.xml`, `Chrom.1.Xml` (curves, event curves, peak tables),
  `EvaluationLog.xml`, `Manifest.xml`, and nested zips (`SystemData`, `InstrumentConfigurationData`,
  `ColumnTypeData`, `MethodData`, `MethodDocumentationData`, `StrategyData`, …, and one per curve,
  `Chrom.1_<n>_True`). Nested zips use ZIP64 local headers and are padded with zeros after the
  end record (by up to ~150 kB). `NextFracData` can be an empty member.
- A nested `…Data` zip holds `XmlDataType` (`System.String`) and `Xml`: an [MS-NRBF] stream with
  one BinaryObjectString (the XML) and MessageEnd.
- A curve zip holds `CoordinateData.Volumes` and `CoordinateData.Amplitudes` (each an [MS-NRBF]
  ArrayOfPrimitiveType of Single) and their `…DataType` members (`System.Single[]`).
- `Chrom.1.Xml` gives per curve: `CurveDataType` (UV, Conduction, pH, Pressure, Temperature,
  Other), `Name`, `AmplitudeUnit`, `DistanceBetweenPoints` and `DistanceToStartPoint` (minutes),
  `MethodStartTime` with its UTC offset, `IsOriginalData`, `IsoChroneType` (Time for recorded
  curves, Volume for an evaluated one), `ColumnVolume`, `CurveUVInfo` (path length), and the file
  name of the points. Time of sample i = `DistanceToStartPoint` + i × `DistanceBetweenPoints`:
  checked against every Fraction/Injection/Logbook (time, volume) pair of six exports: the
  interpolated curve volume matches the event volume to within ±0.3 samples on average (offsets of
  one sample either way are 1-3 samples off).
- `Result.xml`: result name, system name (`Pure25#2363320`), batch id, folder, created by/at,
  run information (base64 text: used columns, instrument server UNICORN version, run start date),
  and the method's variable values (`ResultSearchCriteria` keyword pairs with units: flow rate,
  column type, fraction volume, UV wavelengths, …).
- `InstrumentConfigurationData`: instrument description (`AKTA pure 25`), firmware name and
  version. `ColumnTypeData`: column name, article number, bed height, void volume, hardware.
- Peak tables (`PeakTables/PeakTable`): the evaluated curve number, retention basis (`Volume`),
  counts and per-peak retention (start, maximum, end), height, area, widths, %-of-total areas,
  resolution, asymmetry, sigma and conductivities. Retentions are relative to the injection named
  by `ZeroAdjustedToInjectionNumber` (negative before it).
- `Manifest.xml` lists members with CRC codes that match `Result.xml` and `EvaluationLog.xml` but
  not the nested zips; they are not used for checking (the zip CRC-32s are).

## 2026-09-26 — peak tables measured above a baseline curve

**Why:** the held-out measurement of 2026-09-26 found UNICORN's peak-table heights 4-6 % below
the curve value at the peak on one export (`docs/benchmark/heldout.md`). Following the held-out
rule the question was settled on a new development file, never on the held-out one.

**Corpus file used:** `unicorn-zip-zenodo10810569-hplc` (Zenodo record 10810569,
Kutscherauer and Stránský, CC-BY-4.0; `HPLC_raw.zip`, a UNICORN 7.4.0.800 result export of an
ÄKTA run) whose peak table `UV@16,PEAK` names `<BaseLine><CurveNumber>15</CurveNumber>` — curve 15
is the evaluated curve `UV_CUT_TEMP@100,BASEM`, stored on a volume grid.

**Inferred (scratch scripts, not committed):** with the curve and its baseline curve read from
the export, and retentions shifted by the injection the table is zeroed at:
- `Height` = curve − baseline at `MaxPeakRetention` (366.8670 and 755.3947 mAU; the curve gives
  366.8154 and 755.3642, the baseline −0.0516 and −0.0304 there): equal to 5 significant digits;
- `Area` = ∫ (curve − baseline) d(volume) from `StartPeakRetention` to `EndPeakRetention` by the
  trapezoid rule on the samples inside and the interpolated ends (380.2787 vs 380.2792, 1003.3086
  vs 1003.3080); ∫ curve alone gives 378.96 and 1003.23;
- `StartPeakEndpointHeight`/`EndPeakEndpointHeight` are likewise curve − baseline at the limits;
- `Width` = `EndPeakRetention` − `StartPeakRetention` exactly;
- a peak table without a `BaseLine` element (the development export `unicorn-zip-fs-sample`)
  measures from zero: its heights equal the curve and its areas ∫ curve (to 4 digits).
The reader now reports the baseline curve of each peak table (`extra.baseline_curve_number`,
`extra.baseline_trace`) and the corpus test checks height, area and width against the curves
with these rules.

## 2026-10-06 — three .res files from a third repository (Richard Zimring with Claude as assistant)

Corpus ids: `unicorn-res-pycorngui-koko038`, `unicorn-res-pycorngui-koko135`,
`unicorn-res-pycorngui-koko029`.

Source: the sample files of PyCornGUI (https://github.com/kotarokelley/PyCornGUI, commit
98143df, GPL-2.0 as its README, `docs/LICENSE.txt` and `setup.py` state, checked 2026-10-06), a
GUI by Kotaro Kelley built on PyCORN. The runs are his own size-exclusion runs on an ÄKTApurifier
(UNICORN 5.01 build 318 in the logbook, "UNICORN 3.10" in the file header) from 2013, 2015 and
2016. The repository is not one the corpus used before.

Ground truth: PyCORN 0.19 (GPL-2.0), run as a black box through `oracle/fplc_oracle.py`, as for
the two earlier `.res` files.

Outcome with the release binary: for all 25 curves of the three files, the sample count, the 128
sampled values, the sum, the maximum and its position agree with PyCORN, and so do the volume
differences between sampled points. Nothing was inferred from these files.
