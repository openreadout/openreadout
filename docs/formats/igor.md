# Igor Pro binary waves and packed experiments

WaveMetrics Igor Pro stores data as *waves*: arrays of up to four dimensions with a name, units, a scaling per dimension and an optional note. A binary wave file (`.ibw`, older Macintosh files `.bwav`) holds one wave. A packed experiment (`.pxp`, templates `.pxt`) holds a whole Igor session: waves in data folders, variables, procedures and the command history. Patch-clamp labs (NeuroMatic, Patcher's Pal), atomic force microscopes (Asylum Research), synchrotron beamlines and many spectroscopy groups save their data this way.

Derived from WaveMetrics' public technical notes TN003 (Igor binary file format) and PTN003 (writing packed files), and from hex dumps of corpus files. `igor2` (LGPL) is run only as a black-box oracle (see *Validation*). Every name below is ours. See `docs/provenance/igor.md`. Format id `igor`, crate `openreadout-igor`.

## What OpenReadout returns

Every numeric wave becomes a trace or an image:

- a wave of **one dimension** is a trace with one channel (the wave's name); a wave of **two dimensions whose columns carry labels** (a force curve's `Raw`, `Defl`, `ZSnsr`) is a trace with one channel per column. Complex waves give a real and an imaginary channel. When the row units are a time unit (`s`, `ms`, …) the row step gives `sample_rate_hz` and the row offset `start_s`; otherwise `extra.axis` holds the row scaling (`first`, `step`, `last`, `unit`) and the rate is 0;
- any other wave of **two to four dimensions** is an image: rows are X (Igor shows rows along the horizontal axis), columns Y, layers channels when they carry labels (AFM images: `HeightRetrace`, `AmplitudeRetrace`, …) and Z planes otherwise, chunks time points. Length units (`m`, `mm`, `µm`, `nm`, `pm`, `Å`) give the physical pixel size; a time unit on the chunks gives `time_increment_s`.

Traces and images are numbered in file order. The trace or image name is the wave name in a binary wave and `root:folder:…:name` in a packed experiment. Values are returned as stored (no scaling: Igor waves hold physical values). Text waves, waves without points and waves saved without their data (governed by a formula) are not exposed; text waves' elements are in the vendor metadata.

`extra` of every trace and image: `wave` (the full name), `wave_version`, `wave_type` (`float32`, `float64`, `int8`, … or `complex float32`), `dimensions`, `byte_order`, `wave_created` and `wave_modified` (local time of the writing computer, no zone), `full_scale` [bottom, top] when the file marks it valid, `formula`, `note_bytes`, and in packed experiments `application` (`Igor Pro`) and `application_version` (from the recreation procedures, e.g. `8.040`). Images add `data_units`, `axes` (per dimension: `label`, `unit`, `first`, `step`, `size`) and `layers_as` (`channels` or `z`). `format_version` is `ibw N` (N = the binary-wave version) or `pxp`.

The wave creation time is not the acquisition time (Asylum Research files carry the same creation time in every file), so `acquired_at` is not set.

## Byte order

A binary wave starts with its version (1, 2, 3 or 5) as a 16-bit integer: a first byte of 0 means big-endian (68k and PowerPC Macintosh), anything else little-endian. A packed experiment's byte order is the one under which its record headers chain (`packed_endian`).

## Binary wave (`wave.rs`)

### Binary header

| version | bytes (`bin_header_len`) | fields (offset) |
| --- | --- | --- |
| 1 | 8 | version (0), wave-section size (2), checksum (6) — from TN003's sizes, not seen |
| 2 | 16 | version (0), wave-section size (2), note size (6), unused (10), checksum (14) |
| 3 | 20 | version (0), wave-section size (2), note size (6), formula size (10), unused (14), checksum (18) — from TN003's sizes, not seen |
| 5 | 64 (`BIN_HEADER5`) | version (0), checksum (2), wave-section size (4), formula size (8), note size (12), extended data-units size (16), extended dimension-units sizes ×4 (20), dimension-label sizes ×4 (36), string-indices size (52), two unused fields (56, 60) |

The **checksum**: the first 134 / 142 / 146 / 384 bytes (`checksum_len`; versions 1 / 2 / 3 / 5) summed as 16-bit integers in the file's byte order give 0 (`checksum_zero`). It is the detection signature.

### Wave header, versions 1–3 (110 bytes, `WAVE_HEADER2`, from the end of the binary header)

| offset | field |
| --- | --- |
| 0 | wave type (16-bit) |
| 6 | name, 20 bytes, NUL-terminated |
| 34 | data units, 4 bytes |
| 38 | X units, 4 bytes |
| 42 | point count (32-bit) |
| 48, 56 | X step and offset (doubles): x = offset + step × point |
| 68, 70, 78 | full-scale flag (16-bit), top and bottom (doubles) |
| 96 | creation time (32-bit, seconds since 1904-01-01; not in version 1) |
| 102 | modification time |

The data follows (110 bytes after the wave header's start), then 16 bytes of padding (`PADDING2`), then the note (version 2) or the formula and the note (version 3). Data bytes = wave-section size − 126.

### Wave header, version 5 (320 bytes, `WAVE_HEADER5`, from offset 64)

| offset | field |
| --- | --- |
| 4, 8 | creation and modification times (seconds since 1904-01-01) |
| 12 | point count (all dimensions) |
| 16 | wave type (16-bit) |
| 26 | wave-header version (1) |
| 28 | name, 32 bytes, NUL-terminated |
| 68 | elements per dimension ×4 (rows, columns, layers, chunks; 0 = unused) |
| 84 | step per dimension ×4 (doubles) |
| 116 | offset per dimension ×4 (doubles) |
| 148 | data units, 4 bytes |
| 152 | units per dimension, 4 × 4 bytes |
| 172, 176, 184 | full-scale flag, top, bottom |

The data starts at 384; data bytes = wave-section size − 320. Then, each present when its size is not 0: the formula, the note, the extended data units, the extended units of dimensions 0–3 (they replace the 4-byte fields when units are longer than 3 bytes), the dimension labels of dimensions 0–3 (32-byte NUL-terminated fields: one for the dimension, then one per element), the string indices (text waves: the cumulative end offset of each element, 32-bit). Text is UTF-8 when it decodes as such and Latin-1 otherwise (`igor_text`); notes use carriage returns between lines.

Data is stored rows first, then columns, layers and chunks; complex values as real then imaginary.

### Wave types (`WaveKind`, `NumberType`)

| value | meaning | seen in the corpus |
| --- | --- | --- |
| 0 | text | yes |
| 2 | 32-bit float | yes |
| 4 | 64-bit float | yes |
| 8 | 8-bit integer | yes |
| 16 | 16-bit integer | inferred |
| 32 | 32-bit integer | inferred |
| 128 | 64-bit integer | inferred |
| +64 | unsigned (integers) | inferred |
| +1 | complex | inferred |

Other bit patterns are refused (`wave_type`).

### Data after the wave

Asylum Research AFM images append a byte preview and its description after the wave's last section. The bytes are counted (`trailing_bytes`, a note) and not read.

## Packed experiment (`packed.rs`)

A chain of records to the end of the file. Each record header is 8 bytes (`RECORD_HEADER`): kind (16-bit; the high bit marks a record superseded by a later one, `SUPERSEDED`, which is skipped), version (16-bit) and the length of the data (32-bit).

| kind | `RecordKind` | data |
| --- | --- | --- |
| 1 | `Variables` | variables of the current data folder (below) |
| 2 | `History` | command history (text) |
| 3 | `Wave` | one binary wave (version 2, 3 or 5) |
| 4 | `Recreation` | recreation procedures (text); the first line names the platform and Igor version (`// Platform=WindowsNT, IGORVersion=8.040, …`) |
| 5 | `Procedures` | the procedure window (text) |
| 7 | `GetHistory` | no data |
| 8 | `PackedFile` | a packed notebook or procedure file (not decoded) |
| 9 | `FolderStart` | a data folder's name, 32 bytes, NUL-terminated |
| 10 | `FolderEnd` | no data: back to the parent folder |
| other | `Other` | window, package and platform settings: skipped |

Detection (`looks_like_packed`): the record headers in the first 64 KiB chain without a gap, and the chain holds a wave, variables, folder or text record. With a `.pxp`/`.pxt` extension that is definite, without one likely.

### Variables record (`parse_variables`)

A header (version 1: version, system-variable count, user numeric count, user string count, 16-bit each; version 2 adds dependent numeric and dependent string counts), the system variables K0, K1, … as 32-bit floats (kept for the root folder only), the user numeric variables (56 bytes each, `NUMBER_VARIABLE`: a 32-byte name, a 16-bit field, the numeric type, the real and imaginary parts as doubles, 4 unused bytes), the user string variables (a 32-byte name, the length — 16-bit in version 1, 32-bit in version 2 — and the bytes). Dependent variables are not decoded (reported in `check`). Variables are vendor metadata.

## Vendor metadata (`info --view full`)

`container`, `byte_order`, `waves` (per wave: `path`, `name`, `version`, `type`, `type_name`, `points`, `dimensions`, `steps`, `offsets`, `data_units`, `dimension_units`, `created`, `modified`, `data_offset`, `data_bytes`, `checksum_ok`, `formula`, `note` (lines joined with newlines), `note_fields` (the note's `Key: value` lines when most lines have that shape: Asylum Research settings such as `ScanSize` or `SpringConstant`), `dimension_labels`, and for text waves `values`, at most 1000), and for packed experiments `variables` (per record: `folder`, `version`, `variables` with `name` and `value`/`imaginary`/`text`, `system`, `undecoded`), `igor_version`, `platform` and `texts` (history, recreation procedures and procedures: `record`, `bytes`, `text` up to 1 MiB). `info --view structure` lists every wave (offset, size, type, dimensions, what it became) and every other record.

## `check`

| code | severity | meaning |
| --- | --- | --- |
| `checksum` | warning | the headers do not sum to zero |
| `wave_type` | error | the wave type names no known type |
| `bad_points`, `bad_size`, `dims`, `data_size` | error | point count, section sizes and dimension sizes disagree with each other or with the data |
| `truncated` | error | the data, a section or a record runs past the end of the file or record |
| `long_text`, `labels` | warning | a note or formula longer than 4 MiB (cut), dimension labels not a multiple of 32 bytes |
| `bad_record` | error | a record with a negative length |
| `folders` | warning | data folders that end without a start or never end |
| `bad_wave` | error | a wave record that is not a binary wave |
| `variables` | warning | a variables record not fully decoded |
| `superseded`, `formula_without_data`, `trailing_bytes` | info | superseded records skipped; a wave saved without its data; bytes after a binary wave |
| `too_many_records`, `too_many_waves` | warning | more than 4 million records or 1 million waves |

A plane or trace whose data lies outside the file is a corrupt-file error (exit 4).

## Validation (2026-10-19)

Corpus files from 33 Zenodo records and NeuralEnsemble's test data (CC-BY-4.0, CC0-1.0, CC-BY-SA-4.0): binary waves of versions 2 (big- and little-endian) and 5 (1-D spectra and time series, 2-D maps, AFM images with three and four labelled layers, a force curve with labelled columns, a 3-D stack), packed experiments written by Igor Pro 6.2, 8.0 and 9.0 on Windows and Macintosh (up to 662 waves, nested data folders, text waves, an 8-bit integer wave, 2-D images). Ground truth: `igor2` 0.5.14 run as a black box (`oracle/igor_oracle.py`); traces compare names, units, sample counts, the first values and the xxh3 of every value, images their geometry, pixel type, physical size and the hash of every plane. `igor2` fails on one packed experiment (Zenodo 10473302 `Fig3.pxp`), which is therefore not in the corpus.

## Vocabulary (every public identifier in `openreadout-igor/src` must appear here)

| identifier | meaning |
| --- | --- |
| `IgorReader`, `IgorDataset`, `FORMAT_ID`, `open`, `open_input`, `igor` | reader, opened file (and its parsed structure), format id `igor` |
| `looks_like_wave`, `looks_like_packed`, `wave_endian`, `packed_endian` | detection and byte order |
| `wave`, `packed`, `dataset`, `parse_wave`, `parse_file`, `walk`, `parse_variables` | modules and parsers |
| `Wave`, `version`, `endian`, `start`, `end`, `wave_type`, `kind`, `name`, `points`, `dims`, `steps`, `offsets`, `data_units`, `dim_units`, `full_scale`, `created`, `modified`, `data_offset`, `data_len`, `formula`, `note`, `labels`, `string_indices`, `checksum_ok`, `problems` | one parsed wave (above) |
| `rank`, `readable`, `element_labels`, `dimension_label`, `data_omitted`, `read_elements` | wave helpers: dimensions in use, data readable, labels, saved without data, read stored values as little-endian |
| `WaveKind` { `Text`, `Numeric` }, `number`, `unsigned`, `complex`, `from_type`, `element_bytes`, `pixel_type`, `dtype`, `label` | wave types |
| `NumberType` { `Float32`, `Float64`, `Int8`, `Int16`, `Int32`, `Int64` }, `bytes` | sample types |
| `values_f64`, `text_values`, `igor_text`, `igor_time`, `checksum_zero`, `checksum_len`, `bin_header_len` | values, text, times (seconds since 1904), checksum |
| `BIN_HEADER5`, `WAVE_HEADER5`, `WAVE_HEADER2`, `PADDING2`, `MAX_TEXT`, `MAX_LABEL_BYTES`, `LABEL_FIELD` | sizes and limits |
| `Problem`, `code`, `detail`, `offset`, `fatal` | structure problems found while parsing |
| `Packed`, `records`, `Record`, `data_offset`, `superseded`, `folder`, `len` | the record chain |
| `RecordKind` { `Variables`, `History`, `Wave`, `Recreation`, `Procedures`, `GetHistory`, `PackedFile`, `FolderStart`, `FolderEnd`, `Other` }, `from_code` | record kinds |
| `RECORD_HEADER`, `MAX_RECORDS`, `MAX_RECORD_TEXT`, `MAX_VARIABLES_BYTES`, `SUPERSEDED`, `NUMBER_VARIABLE`, `NAME_FIELD` | record sizes and limits |
| `Variables`, `system`, `user`, `undecoded`, `Variable` { `Number`, `Text` }, `real`, `imaginary`, `number_type`, `value` | variables |
| `IgorFile`, `container`, `file_len`, `waves`, `variables`, `texts`, `igor_version`, `platform`, `trailing_bytes` | the parsed file |
| `Container` { `BinaryWave`, `Packed` } | binary wave or packed experiment |
| `WaveEntry`, `path`, `record`, `role`, `layers` | one wave and what it becomes |
| `Role` { `Trace`, `Image`, `Text`, `Empty`, `Unreadable` } | what a wave becomes |
| `LayerAxis` { `Channel`, `Z` } | how an image's layers are exposed |
| `note_fields` | `Key: value` lines of a note |
| `MAX_TRACE_READ`, `MAX_PLANE_BYTES`, `MAX_TEXT_ELEMENTS`, `MAX_WAVES`, `MAX_TRACE_COLUMNS` | read limits |
