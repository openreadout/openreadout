# Cytiva ÄKTA / UNICORN results

ÄKTA protein-purification systems are run by Cytiva UNICORN. UNICORN 3–5 save `.res` result files; UNICORN 6 and 7 keep results in a database and export them as `.zip` files. OpenReadout reads both and returns the curves, the logbook, fraction and injection marks, and the method; from 6/7 exports also UNICORN's peak tables.

Derived from hex dumps and XML of public files (ÄKTAprime, Ettan LC, ÄKTA pure and ÄKTA avant), allotropy's `cytiva_unicorn` reader (MIT) read as prior art, and the public [MS-NRBF] specification; PyCORN (GPL-2.0) and allotropy are run as black boxes for ground truth. Provenance: `docs/provenance/cytiva-unicorn.md`. Crate: `openreadout-fplc`.

| format id | files | reads | confidence |
| --- | --- | --- | --- |
| `cytiva-unicorn-res` | `.res` (UNICORN 3-5 result files; ÄKTAprime, Ettan LC; header text `UNICORN 3.10`) | curves, logbook, fraction and injection marks, method text | low (evidence rubric, `docs/assurance.md`) |
| `cytiva-unicorn-zip` | `.zip` written by UNICORN 6/7 *Export result* | curves, logbook, fraction and injection marks, UNICORN's peak tables, system, instrument, column, method | medium (evidence rubric, `docs/assurance.md`) |

Not read: the UNICORN 6/7 database itself and its backups, `.result`/`.bak` files, method files,
report PDFs. A zip that is not a result export (no `Result.xml` and `Chrom.<n>.Xml`) is refused
with exit 6 and a hint.

## The model (both variants)

A UNICORN result is one chromatography run: **curves** recorded against time (UV absorbance at up
to three wavelengths, conductivity, pH, pressures, temperatures, the %B the pumps deliver, flows),
**event lists** (the logbook of method instructions, fraction marks, injection marks) and, in
UNICORN 6/7 exports, **peak tables** the software integrated.

### Traces: one per curve

- `name`: the curve name as the file gives it (`UV 1_280`, `Cond`, `% Cond`, `Conc B`,
  `System pressure`, `UV1_215nm`, `SAPA_215nm`). With several chromatograms in one export the names
  start with `Chrom.<n>: `.
- channel 0: the values, named by the curve **kind** (below), in the file's unit (`mAU`, `mS/cm`,
  `%`, `MPa`, `°C`, `ml/min`, `cm/h`, `CV/h`, `cm`; pH has none). Channel 1 `volume`: the retention
  volume of each sample in ml, counted from the method start, as stored.
- `sample_rate_hz` and `start_s`: the curve is sampled at a fixed interval from the method start;
  sample *i* is at `start_s + i / sample_rate_hz` seconds. `trace` statistics give both the time
  of the maximum (`argmax_time_s`) and its retention volume (`argmax_axis_value`, ml).
- `extra.axis` = `{quantity: retention_volume, unit: ml, irregular: true, channel: 1}`: the
  volume is the curve's abscissa (UNICORN plots against volume). `trace --x-range A:B` selects by
  volume. `analyze chromatogram` and `analyze peaks` work on the time base (minutes).

Trace `extra` (our vocabulary):

| key | meaning |
| --- | --- |
| `kind` | `uv`, `conductivity`, `conductivity_percent`, `concentration_b`, `ph`, `pressure`, `temperature`, `flow`, `other` (from the unit, the name and the UNICORN 7 curve type) |
| `wavelength_nm` | UV wavelength from the curve name (`UV 1_280` → 280; a channel named `…_0` is a switched-off monitor channel and has none) |
| `original` | `true` for curves the instrument recorded, `false` for curves the software evaluated afterwards (smoothed, baseline-corrected, cut) |
| `interval_min`, `start_min` | sampling interval and time of sample 0 after the method start, minutes |
| `injection_volume_ml` | the volume of the last injection mark: UNICORN draws retention volumes from it (subtract it to match UNICORN's axis) |
| `curve_type` | UNICORN 7's own curve type (`UV`, `Conduction`, `pH`, `Pressure`, `Temperature`, `Other`) |
| `curve_number` | UNICORN 7's curve number (peak tables refer to it) |
| `isochrone` | `time` (sampled in time) or `volume` (an evaluated curve on a volume grid: it has volumes but no times) |
| `volume_step_ml`, `volume_start_ml` | the volume grid of an evaluated curve |
| `column_volume_ml` | the column volume the export records with the curve |
| `uv_path_length`, `uv_nominal_path_length`, `uv_normalized_to_nominal_path` | UV flow-cell path length (cm) and whether values are normalized to the nominal path |
| `display_decimals` | decimals UNICORN displays |
| `method_start` | the method start with its UTC offset (UNICORN 7) |
| `member` | the export member that holds the points |
| `block`, `channel_label`, `time_axis_label`, `volume_resolution_ml`, `time_offset_ms` | `.res`: the block name, its column labels, the volume resolution (the stored integer's factor) and a sub-sample start offset (see below) |

### Tables

Event lists come first, one table each, then the peak tables.

- **`logbook`**, **`fractions`**, **`injections`** (and any other event list by its lower-case
  name): columns `time` (min), `volume` (ml) and the text as a category code: `text` (the logbook
  line), `fraction` (the tube or well label, `Waste`), `injection` (the injection number or text).
  Table `extra.kind` = `events`.
- **Vendor peak tables** (UNICORN 6/7, named as UNICORN names them, e.g. `UV 1_280@01,PEAK (1)`):
  one row per peak, UNICORN's integration as stored: `retention` (the maximum), `start`, `end`,
  `height`, `area`, `percent_of_total_area`, `percent_of_total_peak_area`, `width`,
  `width_at_half_height`, `resolution`, `asymmetry`, `sigma`, `start_endpoint_height`,
  `end_endpoint_height`, `average_conductivity`, and `name` when peaks are named. Retentions are in
  ml (or min when the table's basis is time) **from the injection the table is zeroed at**
  (`extra.zero_at_injection`); heights in the curve's unit, areas in unit·ml. Table `extra`:
  `kind` = `vendor_peaks`, `trace` (our curve), `retention_basis`, `curve_number`,
  `detected_peaks`, `total_peak_area`, `total_area_evaluated_peaks`, `ratio_peak_area_total_area`,
  `max_peaks`, `zero_at_injection`, `column_volume_ml`, `algorithm`, `technique`, `created`,
  `height_reference` (`baseline` or `zero`), `baseline_curve_number` and `baseline_trace` (the
  evaluated baseline curve the table is measured above, when it names one).
- **What UNICORN's peak values mean** (validated on the exports of `docs/provenance/cytiva-unicorn.md`):
  `height` = curve − baseline at `retention`; `area` = the trapezoid integral of curve − baseline
  over retention volume from `start` to `end` (ends interpolated); `start_endpoint_height` and
  `end_endpoint_height` = curve − baseline at the limits; `width` = `end` − `start`. The baseline
  is the table's `baseline_trace` (UNICORN's `…BASEM` evaluated curve on a volume grid); a table
  without one measures from zero, so its heights equal the curve. A drifting UV signal therefore
  gives heights below the curve maximum; subtract the baseline trace to reproduce them.

### Experiment facts

`instrument`: vendor `Cytiva (ÄKTA)`, software `UNICORN`, the model (`.res`: the system named in
the method dump or strategy notes, e.g. `AKTAprime`, `EttanLC`; UNICORN 7: the instrument
configuration, e.g. `AKTA pure 25`), the UNICORN version (UNICORN 7). `method`: name (the logbook's
`Method Run … Method: <name>`, the method file, or the method description), technique
(size-exclusion CHMO:0001013, affinity CHMO:0001006 or ion-exchange CHMO:0001014 when the method or
column says so, else liquid chromatography CHMO:0001004) and parameters: `column`,
`column_volume` (mL), `column_article_number`, `column_bed_height` (cm), `flow_rate` (mL/min),
`fraction_volume` (mL), `uv_wavelength_1…3` (nm), `technique` (UNICORN's own word), `run_duration`
(min), `firmware`. `acquisition`: operator (`.res`: the header user; UNICORN 7: the result's
creator), start (UNICORN 7: the method start with its offset; `.res`: derived, see below), end
(`.res`), duration. `sample.id` when the method has a `Sample_ID` variable with a value.

## `.res` (UNICORN 3-5)

Little-endian. Header: `11 47 11 47`; u32 at 8 the directory offset; u32 at 16 the file size (a
mismatch is reported as truncation); text at 0x18 (`UNICORN 3.10` in both files: a layout version,
not the software version, reported as `format_version`); u32 Unix times at 0x68 and 0x6c; the user
name at 0x76.

Directory: 344-byte entries until an empty name: bytes 0-5 a type word, 6-301 the NUL-terminated
name, then u32 data size, u32 allocated size, u32 data offset, u32 header size (240 curves, 552
event lists, 0 others). Curve names are `<run name>:<run number>_<curve>`; an evaluated curve's
name starts with `=` and its type word has `03` where recorded curves have `01`.

Curve and event blocks start with u16 6, u16 stored columns, u16 1, then 78-byte column
descriptors: u16 78, u16 flags (`0x8001` time, `0x8002` volume, `0x4001`/`0x4000` value), a
40-byte label, a 16-byte unit, u16 storage (`0x0104` int32, `0x0308` float64, `0x044c` 76-byte
text), float64 factor, float64 second value.

- **Curves**: three descriptors; (int32 volume, int32 value) pairs follow the header; physical =
  stored × factor (a factor of 1/k divides by k so 15328 × 0.001 is 15.328). The time column is not
  stored: its factor is the sampling interval in minutes and sample *i* is at *i* × interval. Its
  second value, when not 0, is read as a start offset in milliseconds (always below one sampling
  interval in the files seen; `extra.time_offset_ms`; an assumption, see provenance).
- **Event lists**: 180-byte records: float64 time (min), float64 volume (ml), two 76-byte texts
  (joined), float64 value, int32 flags.
- **Text blocks** (`CreationNotes`, `Methods`, `MethodStrategyNotes`, `ResultStrategyNotes`,
  `Techniques`, `METHODINFO`, `Method Signatures`) are kept in the vendor tree; binary blocks are
  listed by `info --view structure`.
- **Times**: `acquisition.ended_at` is the header time at 0x6c (UTC; it matched the run end in
  both files); `started_at` is that time minus the longest curve's duration (the logbook's `Method
  Run` text is local time without a zone and is kept in the vendor tree as `method_run`); the time
  at 0x68 is kept as `header_time_0x68` (it matched the start in one file and not in the other).

## UNICORN 6/7 result export (`.zip`)

A zip whose members are XML documents and nested zips (members may sit in a folder or carry
`Copy of ` prefixes and `.zip` suffixes when a user re-packed the export: names are matched on
their base name without them):

- `Result.xml`: result name, system name, creator and times, run information (base64 text:
  used columns, instrument server version, run start), and the method's variable values
  (`ResultSearchCriteria` keyword/value/unit triples; all in the vendor tree, the common ones as
  method parameters).
- `Chrom.<n>.Xml`: per curve its name, `CurveDataType`, `AmplitudeUnit`, `IsoChroneType`,
  `DistanceBetweenPoints` and `DistanceToStartPoint` (minutes for time curves, ml for volume-grid
  curves), `MethodStartTime` and its UTC offset, `IsOriginalData`, `ColumnVolume`, `CurveUVInfo`,
  and the member holding the points; event curves (`Fraction`, `Injection`, `Logbook`, …) with
  `EventTime` (min), `EventVolume` (ml), `EventText`; peak tables.
- Nested zips (`SystemData`, `InstrumentConfigurationData`, `ColumnTypeData`, `MethodData`,
  `MethodDocumentationData`, `StrategyData`, …) hold `XmlDataType` (`System.String`) and `Xml`: a
  .NET binary-serialized string ([MS-NRBF]: a 17-byte serialization header, a string record with a
  7-bit length prefix, a message end). A damaged or non-serialized metadata document is a warning,
  never fatal.
- **Curve points** (`Chrom.<n>_<curve>_True`): a nested zip (ZIP64 local headers, padded with zeros
  after the end record) with `CoordinateData.Volumes` and `CoordinateData.Amplitudes`, each an
  [MS-NRBF] array of 32-bit floats (record 15, primitive type 11) and a `…DataType` member
  (`System.Single[]`). The declared array length must fill the member exactly; a member cut short
  or holding bare floats is refused (the curve is left out, `check` reports it). An evaluated curve
  on a volume grid may store no volumes: they are `volume_start_ml + i × volume_step_ml`.
- Time of sample *i* = `DistanceToStartPoint + i × DistanceBetweenPoints` (validated against every
  event of six exports, see provenance). A curve with a zero or missing interval keeps its volumes
  and has no times (a warning).

`Manifest.xml` lists members with CRC codes that do not match the nested zips; they are not used.
Every member read is checked against its zip CRC-32.

## Validation

`cargo test -p openreadout-corpus-tests --features corpus` (`tests/fplc_oracle/mod.rs`): every
curve PyCORN returns matches ours at 128 sampled positions (exactly for UNICORN 7 floats; within
1e-9 for `.res`), allotropy's data cubes agree (it converts conductivity to S/m), fraction and
injection marks match, the 22 vendor peaks of the two exports with peak tables (one measured from
zero, one above a baseline curve) have their height (curve − baseline at the retention) and area
(∫ curve − baseline) within 0.5 % of UNICORN's and their width equal to end − start, and damaged
curve members are refused.
PyCORN and allotropy both read UNICORN 7 floats from byte 47 to 48 bytes before a member's end, so
they drop the first five and last eleven samples of every curve; the record layout accounts for
every byte, and the event (time, volume) pairs fit our sample indexing, not theirs. PyCORN applies
fixed scales to `.res` pH and pressure (÷10, ÷100) where the ÄKTAprime file declares 0.01 and
0.001; its method dump's 300 kPa pressure limit, which PyCORN's values would exceed for the whole
run, shows the declared factor is right.

## Vocabulary (public API of `openreadout-fplc`)

| identifier | meaning |
| --- | --- |
| `UnicornResReader` | reader of UNICORN 3-5 `.res` result files |
| `UnicornZipReader` | reader of UNICORN 6/7 result exports (`.zip`) |
| `UNICORN_RES_FORMAT_ID` | `cytiva-unicorn-res` |
| `UNICORN_ZIP_FORMAT_ID` | `cytiva-unicorn-zip` |
| `FplcDataset` | the dataset both readers return |

[MS-NRBF]: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-nrbf/
