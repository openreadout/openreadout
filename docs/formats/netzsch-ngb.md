# NETZSCH Proteus `.ngb-*`

NETZSCH Proteus saves thermal analysis runs (STA, DSC and dilatometry) as `.ngb-*` files. OpenReadout returns one trace per run with every recorded channel against time, plus instrument, operator, sample and date. Values are as stored: temperatures are uncalibrated and the DSC signal is in µV, because Proteus applies its calibrations only when it evaluates or exports. Derived from public files of two depositors (pyNGB's test files and a DSC 214 study with the depositors' Proteus exports), with pyNGB (MIT) read as documentation and used as a reference reader. Provenance: `docs/provenance/netzsch-ngb.md`. Crate: `openreadout-thermal`.

| format id | files | reads | confidence |
| --- | --- | --- | --- |
| `netzsch-ngb` | `.ngb-ss3` (STA sample run), `.ngb-bs3` (correction run), `.ngb-ds3` (sample + correction), `.ngb-sd7` (DSC), `.ngb-dla`/`.ngb-cla` (dilatometer) | every channel of every run, instrument, operator, sample, date | evidence rubric (`docs/assurance.md`) |

Not read: Proteus analysis files (`.ngb-taa`) and state files (`.ngb-od7`), refused; single-stream
files that are not zip containers (the container form every development file uses is the zip).

## Layout

A zip. `Streams/stream_N.table` (N = 1 metadata, 2 and 3 measured channels, others internal), each
a small database: `Netzsch TA file` at offset 2, `_db_format_1` at 28, a section directory at
0x50 (14-byte entries: `ff ff`, `u16` id, `u32` offset, `u32` size, two pad bytes; ended by a
non-`ff ff` entry or an all-zero one). Sections are contiguous and the last ends at the end of the
member (checked).

Every section is a run of records: `18 fc ff ff 03 80 01`, `u16` field id, the fixed
`00 00 01 00 00 00 0c 00 17 fc ff ff`, a type byte (0x02 `u16`, 0x03 `i32`, 0x04 `f32`, 0x05
`f64`, 0x10 `u8`, 0x14 8 bytes, 0x1A reference, 0x1F string, 0x48 16 bytes, 0x00 empty), then
`80 01` and a scalar or `a0 01`, a `u32` element count and the elements, ended by
`01 00 00 00 02 00 01 00 00` (or two variant endings). Strings are `ff fe ff <chars>` UTF-16 or a
`u32` byte length and UTF-8. A reference record whose payload ends `02 00 00 80 <type u16> 00 00`
opens a table (its field id is the table's category); the fields after it belong to it. Section
prologues, preamble records, `00 03 00` table trailers and a terminator-less final record are
known non-record forms; anything else in a data stream is refused as corrupt.

Data: in streams 2 and 3 a channel-header table (type 0x2B22; the low byte of its category is the
channel id) is followed by one value table per program segment (type 0x2B23) with one array
(field 0x0F40 `f64` or 0x0F3D `f32`); the arrays concatenate. A channel header that repeats starts
a new run (the embedded correction of a sample + correction file). All channels of a run have the
time channel's length (checked).

| channel id | our channel | unit |
| --- | --- | --- |
| 0x8C | `time` | s (stored in minutes) |
| 0x8D | `sample_temperature` | °C |
| 0x8E | `dsc_signal` | µV |
| 0x90 | `mass` | mg |
| 0x9C, 0x9D | `purge_flow_1`, `purge_flow_2` | mL/min |
| 0x9E | `protective_flow` | mL/min |
| 0x8F | `length_change` | µm |
| 0x4E, 0x4F | `force`, `force_setpoint` | N |
| 0x30 | `furnace_temperature` | °C |
| 0x31, 0x32 | `cooling_power`, `furnace_power` | — (not validated) |
| 0x33 | `h_foil_temperature` | °C |
| 0x34, 0x35 | `uc_module`, `environmental_pressure` | — |
| 0x36-0x38 | `acceleration_x`, `_y`, `_z` | — |
| others | `channel_<id>` | — (finding `channel_unknown`) |

Metadata (stream 1, the first table of a category holding the field): instrument name (0x1775 /
0x1059), project, lab, operator, comment, date performed (Unix seconds) (0x1772 / 0x083C, 0x0834,
0x0835, 0x083D, 0x083E), crucible, furnace, carrier (0x177E, 0x177A, 0x1779 / 0x0840), sample id,
name, material, mass (mg), length (mm) (0x7530 / 0x0898, 0x0840, 0x0962, 0x0C9E, 0x0C9F), the
instrument model (a table of type 0x2B17, field 0x0432) and the measurement kind (0x1770 /
0x103A: 1 correction, 2 sample, 3 sample + correction).

## What the reader returns

- **One trace per run** (`sample`; `correction` for a correction file or the second run of a
  sample + correction file): channel 0 `time` (s, irregular: `extra.axis`), then the channels in
  stream order (table above), values as stored: temperatures uncalibrated, the DSC signal in µV
  (Proteus converts it to mW or mW/mg with its sensitivity calibration and baseline correction
  only when it evaluates or exports).
- **Experiment**: vendor NETZSCH, software Proteus, model, operator, comment, sample name and id,
  `sample_mass` (mg), `sample_length` (mm), `measurement_type`, the date performed (UTC), duration;
  technique thermal analysis (TG and DSC), thermogravimetry, DSC or dilatometry by the channels.
- **Vendor tree**: the metadata above, the streams present, the run count, the extension and the
  instrument kind it implies.

## Validation

`tests/series_oracle/mod.rs`: pyNGB 0.6.0 (MIT, run as a black box) returns the same values for
every channel of every run of all 18 development files (256 rows per channel) and the same sample
name, operator, mass and date. The Proteus exports of the seven DSC files (ExpDat, resampled at
every kelvin, calibrated temperature) agree with our sample temperature within 0.6 K at every
exported time (the difference is the temperature calibration, below 0.5 K). A third depositor is
held out.

## Vocabulary (public API of `openreadout-thermal`, NETZSCH)

| identifier | meaning |
| --- | --- |
| `NgbReader` | reader of NETZSCH Proteus `.ngb-*` measurement files |
| `NGB_FORMAT_ID` | `netzsch-ngb` |
