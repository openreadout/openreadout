# Oxford Nanopore raw signal (POD5 and FAST5)

OpenReadout reads the raw current that Oxford Nanopore sequencers (MinION, GridION, PromethION)
record for every read: POD5, the current format, and FAST5, the HDF5 format it replaces. Every
read's signal comes back in picoamperes, one sweep per read, with a table of the reads and the
run's metadata.

Derived from ONT's published POD5 format documents and the VBZ README (MPL-2.0 projects; their
source was not read), the StreamVByte description (Apache-2.0), the Arrow IPC and FlatBuffers
documentation, and black-box runs of `pod5` and h5py with `vbz_h5py_plugin`, which are also the
ground truth. Provenance: `docs/provenance/nanopore.md`. Crate: `openreadout-seq`
(`src/nanopore/`).

| format id | files | reads | confidence |
| --- | --- | --- | --- |
| `pod5` | `.pod5` | reads, run info, VBZ or uncompressed signal | evidence rubric (`docs/assurance.md`) |
| `fast5` | `.fast5` (HDF5) | multi-read (1.0, 2.x) and single-read files, deflate or VBZ signal | evidence rubric |

## POD5

The signature `8B 50 4F 44 0D 0A 1A 0A` starts and ends the file. Before the last signature: a
16-byte section marker, a little-endian i64 footer length, and before that the footer, a
FlatBuffer table with `file_identifier`, `software`, `pod5_version` and `contents`, a list of
embedded files (offset i64, length i64, format, content type: 0 reads table, 1 signal table, 2
read-id index, 3 other index). Each embedded file is an Arrow IPC file. The reader keeps its own
bounds-checked decoder of the IPC buffers for the column types POD5 uses (integers, floats,
booleans, timestamps, fixed and variable binary, strings, lists, maps, string dictionaries); the
FlatBuffer metadata goes through the verified accessors of `arrow-ipc`. Compressed IPC bodies
are refused, and so are files written by pod5 before 0.1 (pore, calibration and end reason as
dictionaries of structs; `pod5 update` rewrites them).

- **Reads table**: `read_id` (16-byte UUID), `signal` (list of signal-table rows), `channel`
  (and `channel_32bit`), `well`, `pore_type`, `calibration_offset`, `calibration_scale`,
  `read_number`, `start` (samples since the acquisition started), `median_before`,
  `num_minknow_events`, `num_samples`, `end_reason`, `end_reason_forced`, `run_info`
  (acquisition id), `open_pore_level`.
- **Run-info table**: `acquisition_id`, `acquisition_start_time`, `adc_min`, `adc_max`,
  `context_tags` and `tracking_id` (string maps), `experiment_name`, `flow_cell_id`,
  `flow_cell_product_code`, `protocol_name`, `protocol_run_id`, `protocol_start_time`,
  `sample_id`, `sample_rate`, `sequencing_kit`, `sequencer_position`,
  `sequencer_position_type`, `software`, `system_name`, `system_type`.
- **Signal table**: `read_id`, `signal` (VBZ `minknow.vbz` large binary, or a list of int16),
  `samples`. `open` reads only the IPC footer and each record batch's message header (its row
  count); a read's rows are decoded when its signal is asked for.

**VBZ (POD5)**: zstd around 16-bit variable-byte integers: one control bit per value (least
significant first, eight per control byte; 0 one byte, 1 two bytes little-endian), all control
bytes before the data; each value is the zig-zag encoded difference from the previous sample
(from 0 for the first).

## FAST5

HDF5 (read with `hdf5-pure`). Multi-read files: `read_<id>` groups with `Raw/Signal` (int16),
`Raw` attributes `read_id`, `read_number`, `start_time` (samples), `duration`, `median_before`,
`start_mux`, `end_reason` (2.x: a numeric code, kept as written), `channel_id` attributes
`channel_number`, `digitisation`, `offset`, `range`, `sampling_rate`, and `tracking_id` and
`context_tags` attribute sets; basecalled files add `Analyses/Basecall_*` (counted, not read).
Single-read files: `Raw/Reads/Read_<n>` with `UniqueGlobalKey/channel_id`, `tracking_id` and
`context_tags`. pA = (raw + offset) × range / digitisation.

**VBZ (FAST5, filter 32020, version 1)**: each chunk is a u32 with the decoded size in bytes,
then zstd of StreamVByte (two control bits per value for 1–4 bytes, four values per control
byte) of the zig-zag encoded differences. Filter parameters: version, integer size (2), zig-zag
flag, zstd level. Deflate chunks are decoded by `hdf5-pure`.

## What the readers return

- **Trace 0, raw signal**: one sweep per read in the order of table 0, channel `current` (pA,
  each read's own calibration), the sample rate of the run (4 or 5 kHz);
  `extra.sweep_sample_counts`, `signal_compression` (`vbz`, `gzip`, `none`), `layout` (FAST5).
- **Table 0, reads**: `read_id`, `channel`, `well`, `read_number`, `start` (s), `duration` (s),
  `samples`, `median_before` (pA), `open_pore_level` (pA), `end_reason`, `end_reason_forced`,
  `calibration_offset`, `calibration_scale`. `extra`: flow cell, flow-cell product, kit, sample,
  experiment, protocol, software, position, position type, system type, acquisition start and a
  `summary` (reads, samples, signal seconds, median, longest and N50 duration, channels).
- **Experiment**: vendor Oxford Nanopore Technologies, model (system type, else position type),
  serial (system name or position), software; DNA sequencer, DNA sequencing assay; parameters
  sample rate, flow-cell product, flow cell, kit; started at; sample id.
- **Preview**: the read's whole signal, a zoom where single samples show, and the durations of
  the file's reads (`preview --sweep N` for read N).
- **Export**: `export --format parquet --trace 0` writes every read's samples (a `sweep` column,
  time and current); without `--trace`, the reads table.

## Validation

`tests/corpus/seq_oracle/mod.rs` with `oracle/seq_oracle.py`: every read's metadata and signal
(sum, extremes, the first 32 samples in pA) against pod5 for POD5 files and against h5py for
FAST5 files; the reads of a FAST5 file also against pod5's reading of their POD5 conversion. Five
development files: a PromethION R10.4.1 POD5 (5 kHz), the same ten MinION reads as POD5 and as
FAST5 1.0 (gzip), a FAST5 2.0 with VBZ and a one-read FAST5.

## Vocabulary (public API of `openreadout-seq`: `src/nanopore/`)

| identifier | meaning |
| --- | --- |
| `acquisition_id` | Acquisition id |
| `adc_max` | See `adc_min` |
| `adc_min` | ADC range |
| `basecalled` | Reads whose group holds base calls (`Analyses/Basecall_*`) |
| `BatchBlock` | One record batch of a file: its location and row count |
| `batches` | Record batches in order |
| `BatchMeta` | A record batch message: rows, field nodes `(length, nulls)` and buffers `(offset, length)` relative to the body, plus (for a dictionary batch) the dictionary id |
| `BatchReader` | Decodes the columns of one batch from its body |
| `block_bytes` | The metadata and body bytes of a block of `file` |
| `body_len` | Body length |
| `buffers` | Buffers |
| `channel` | Channel (1-based) |
| `ColType` | The type of a column, as far as this reader decodes it |
| `Column` | A decoded column |
| `column` | Decode the next column of type `ty` (columns must be read in schema order) |
| `compression` | Signal compression of the first read (`vbz`, `gzip`, `none`) |
| `content` | 0 reads table, 1 signal table, 2 read-id index, 3 other index |
| `contents` | Embedded files |
| `context_tags` | Context tags |
| `dict_blocks` | Dictionary blocks |
| `dict_id` | Dictionary id, for a dictionary batch |
| `Embedded` | An embedded file listed in the footer |
| `end_reason` | Why the read ended (name in POD5, numeric code in FAST5) |
| `end_reason_forced` | The read was ended forcibly (mux change, unblock) |
| `experiment` | Experiment (protocol group) name |
| `extension` | `ARROW:extension:name` of the field, when it has one |
| `Fast5` | An open FAST5 file |
| `fast5_chunk` | Decode a FAST5 VBZ chunk (filter 32020) of 16-bit integers, with the filter's parameters (version, integer size, zig-zag flag, zstd level) |
| `Fast5Reader` | Reader of FAST5 files |
| `Fast5Signal` | The signal of an open FAST5 file |
| `Field` | A column of the schema |
| `field_index` | The index of field `name` |
| `fields` | Columns |
| `file_identifier` | `MINKNOW:file_identifier` |
| `FILTER_VBZ` | HDF5 filter id of VBZ |
| `flow_cell_id` | Flow-cell id |
| `flow_cell_product` | Flow-cell product code (`FLO-PRO114M`) |
| `Footer` | What the footer says |
| `footer` | What the footer says |
| `FORMAT_ID` | Format id of FAST5 files |
| `IpcFile` | An Arrow IPC file held in memory (or its footer only, for large tables read batch by batch) |
| `IpcResult` | An error message (the caller adds the format and the table) |
| `is_empty` | True when no row of a column was decoded |
| `length` | Length |
| `looks_like` | True when `head` starts with the POD5 signature |
| `MAX_SAMPLES` | Largest decoded signal accepted from one compressed row or chunk (samples) |
| `MAX_TABLE_BYTES` | Largest IPC file read into memory as a whole (reads and run-info tables) |
| `median_before` | Current level before the read (pA) |
| `meta_len` | Metadata length (with prefix and padding) |
| `metadata` | Schema-level custom metadata |
| `name` | Column name |
| `NanoporeDataset` | An open POD5 or FAST5 file |
| `new` | A reader over `body` described by `meta`; `dicts` resolves dictionary columns |
| `nodes` | Field nodes |
| `num_events` | MinKNOW events in the read |
| `num_samples` | Samples in the read |
| `offset` | Offset of the message in the IPC file |
| `open` | Open a FAST5 file |
| `open_pore_level` | Open-pore level (pA) |
| `parse_footer` | Parse the footer and schema of an IPC file whose bytes are `file` (or whose first 8 bytes and last `file.len()` bytes are given: only the tail is needed when `tail_offset` > 0, the position of `file[0]` in the IPC file) |
| `parse_message` | Parse the message metadata at the start of `meta` (the `meta_len` bytes of a block) |
| `Pod5` | An open POD5 file: footer, reads, runs and the signal table |
| `pod5_row` | Decode a POD5 VBZ signal row of `n` samples |
| `pod5_version` | POD5 specification version of the tables |
| `Pod5Reader` | Reader of POD5 files |
| `Pod5Signal` | The signal table: its location and record batches, read batch by batch |
| `pore_type` | Pore type |
| `position` | Sequencer position |
| `position_type` | Sequencer position type (`PromethION`, `MinION Mk1B`) |
| `protocol` | Protocol name |
| `protocol_run_id` | Protocol run id |
| `protocol_started_ms` | Protocol start, ms since 1970 |
| `read_all` | Read every batch (and dictionary) of a table held in memory as `file`; returns each column's values over all batches, in schema order |
| `read_id` | Read id (UUID) |
| `read_number` | Read number on its channel |
| `ReadMeta` | One read's metadata |
| `reads` | Reads |
| `rows` | Rows |
| `row_count` | Rows of a table: what its record batches declare, capped at its longest decoded column |
| `run` | Index of the read's run in the run list |
| `run_name` | The run's acquisition id as the read names it |
| `RunMeta` | One run (acquisition) |
| `runs` | The run (from the first read's `tracking_id` and `context_tags`) |
| `sample_id` | Sample id |
| `sample_rate` | Samples per second |
| `scale` | See `offset` |
| `sequencing_kit` | Sequencing kit |
| `signal` | Signal access |
| `signal_is_vbz` | True when the signal is VBZ-compressed (else stored as plain 16-bit lists) |
| `SignalSource` | Decodes one read's raw ADC samples |
| `SIGNATURE` | The POD5 signature at both ends of the file |
| `size` | File size |
| `software` | Writing software |
| `start_sample` | Samples since the acquisition started, at the read's start |
| `started_ms` | Acquisition start, ms since 1970 |
| `streamvbyte_decode` | Decode standard StreamVByte (`n` values) |
| `svb16_decode` | Decode the 16-bit variable-byte layer of a POD5 signal row (`n` samples) |
| `system_name` | System name |
| `system_type` | System type |
| `table_metadata` | Schema metadata of the reads table |
| `tracking_id` | Tracking id fields |
| `ty` | Its type |
| `unzstd` | zstd-decompress `z`, refusing output larger than `max` bytes |
| `uuid` | A 16-byte UUID as text |
| `version` | `file_version` of the root group |
| `well` | Well (mux), 1-based |
