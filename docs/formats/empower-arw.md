# Waters Empower ASCII exports (`.arw`)

Waters Empower keeps raw data and results inside its own database, so data leaves Empower only through exports. The ASCII raw-data export, saved with the `.arw` extension, holds one chromatogram as text. OpenReadout returns it as one trace with the sample, method and injection fields the export includes. The AIA/ANDI netCDF export (`.cdf`) is read by the `andi-chrom` reader. Sony `.ARW` camera files share the extension but are TIFF and never start with a quote.

Format id `empower-arw`. Code: `crates/openreadout-chrom/src/empower_arw.rs`. How each fact was
established: `docs/provenance/empower-arw.md`.

## Layout

Text (ANSI/UTF-8), rows separated by CR, LF or CRLF, cells by tabs:

1. the names of the fields the export method includes, each in double quotes
   (`"SampleName"`, `"Channel"`, `"Sample Set Name"`, `"Instrument Method Name"`, …);
2. their values, in double quotes, as many as names;
3. one row per point: retention time (minutes) and the detector value, unquoted decimal
   numbers.

The value's unit is not in the export (it is the detector's: mV, AU, EU for fluorescence, …).
Times are printed to 7 significant digits, so an evenly sampled run shows steps that differ in
the last digit; points within 10⁻⁶ of the grid (relative to the time) are a regular trace.

Some exports write the header as one field per line instead, `"name"<TAB>value`, the value quoted
text or an unquoted number, followed by the same data rows (GPCreader's and HPLC-RS's test
exports). The reader takes the lines before the first row of two numbers as this layout when there
are more than two of them, or when the first value is unquoted, and each is a quoted name and one
value (`extra.one_field_per_line` true; assurance layout `one field per line`). Two quoted lines are
read as a names row and a values row, the layout every two-line development export has.

An export method that selects no fields writes the data rows alone. Such a file is read when it
has the `.arw` extension and its first lines are all two tab-separated numbers (`extra.headerless`
true, no fields, the channel named `value`). Nothing else in it names Empower, so without the
extension it is not claimed. No public export of this layout was found, so the assurance layout
`headerless export` is unvalidated and `--strict` refuses its trace.

Detection: a first line of tab-separated, double-quoted cells (with or without the `.arw`
extension); in a file named `.arw`, also a first line of a quoted name and a number, or rows of
two numbers. Rows of more than two columns (a multi-wavelength PDA export) are refused (exit 6);
a header whose value count differs from its name count, or a data row that is not two numbers,
is corrupt (exit 4).

## Mapping to the data model

- One trace named `<SampleName> / <Channel>` (or the channel alone), one channel named after
  `Channel` without surrounding spaces (else `value`), `dtype` `float64`, no unit. Evenly spaced times: `sample_rate_hz`,
  `start_s` and `extra.axis` (retention time, minutes); otherwise `sample_rate_hz` 0 and a first
  channel `time` (minutes).
- `extra`: `fields` (every exported name and value), `headerless` and `one_field_per_line` (only when true), and from them `sample_name`, `channel`,
  `sample_set`, `instrument_method`, `processing_method`, `vial`, `injection`,
  `injection_volume`, `acquired_by`, `acquired_at` when exported; `line_ending`, `x_start_min`,
  `x_end_min`, `time_channel`.
- `check`: `time_not_increasing` (warning), `irregular_times`, `no_channel_field` (info).

## Validation

- Six exports of Appia's test data (MIT, two fluorescence channels of three samples, 6,601
  points each): every value equal to Appia's own reading (`processed-tests/…hplc-wide.csv`, an
  independent reader) and the times within Appia's single-precision rounding (2·10⁻⁶ min).
- Ten GPC exports of GPCreader's test data (MIT; four with a names row and a values row, six with
  one field per line) and seven HPLC exports of HPLC-RS's test data (GPL-2.0, one field per line):
  every point equal to chromConverter's reading (GPL-3.0, run as a black box).

## Known gaps

- Multi-column (3D PDA) exports: no licensed public example; refused.
- Empower's native data (database) and its report exports are not read; AIA/netCDF exports are
  read by `andi-chrom`.

## Vocabulary (every public identifier in `empower_*.rs` must appear here)

| identifier | meaning |
| --- | --- |
| `EmpowerArwReader`, `EmpowerArwDataset`, `EMPOWER_ARW_ID`, `open`, `export` | reader, opened export, the id `empower-arw`, open by path, the parsed export |
| `MAX_ARW_BYTES` | largest export read (512 MiB) |
| `ArwExport`, `fields`, `times`, `values`, `line_ending`, `headerless`, `one_field_per_line`, `field`, `regular_step` | a parsed export: header names and values, points, line ending, whether it has no header rows, whether its header has one field per line; a named field's value; the step of an evenly spaced export |
| `ArwError` { `Corrupt`, `Unsupported` }, `parse_arw`, `looks_like_arw`, `looks_like_arw_field_lines`, `looks_like_headerless_arw` | why an export was not read; the parser; detection of an export with quoted header rows, with one field per line starting with a number, and without header rows |
