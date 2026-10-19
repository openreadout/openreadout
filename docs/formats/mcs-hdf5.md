# Multi Channel Systems HDF5 (`.h5`, MCS-HDF5 RawData protocol)

Multi Channel DataManager converts recordings of Multi Channel Systems MEA and in vivo systems
(MC_Rack `.mcd`, Multi Channel Experimenter and Multi Channel Analyzer recordings) into HDF5
files that follow the MCS-HDF5 "RawData" protocol. Most shared MCS recordings are in this form.

Derived from Multi Channel Systems' published definition of the protocol
(`docs/mcs_hdf5_protocol.rst` of the BSD-3-licensed McsPyDataTools repository) and the corpus
files (`docs/provenance/mcs-hdf5.md`). The older binary MC_DataTool export is a separate format
(`mcs-raw`).

Crate: `openreadout-ephys`, format id `mcs-hdf5` (`MCS_HDF5_FORMAT_ID`), extension `h5`, reader
`McsHdf5Reader`, dataset `McsHdf5Dataset` (`open`, `open_input`, `mcs`). Detection: an HDF5 file
whose first 64 KiB hold the root attribute name `McsHdf5ProtocolType` (`memfind`) → definite; a
`.h5` or `.hdf5` file whose root attribute `McsHdf5ProtocolType` (`protocol_type`) is `RawData` →
definite. Other protocol types (CMOS-MEA) are refused.

## Layout (`parse_mcs_hdf5` → `McsFile`)

- Root attributes: protocol type and version (`protocol_type`, `protocol_version`), and from
  protocol version 2 the generating application, its version and the McsDataTools version.
  `/Data` attributes: recording program and version, MEA name and layout, date (a text and
  .NET ticks of 100 ns since year 1), comment. All are kept as text (`attributes`).
- `/Data/Recording_<n>/AnalogStream/Stream_<n>` (`McsAnalogStream`: `recording`, `stream`,
  `label`, `data_subtype`, `channels`, `segments`, `matrix`, `digital`): `InfoChannel`, one
  compound record per channel (`McsChannel`: `id`, `row`, `group`, `label`, `raw_data_type`,
  `unit`, `exponent`, `ad_zero`, `tick_us`, `conversion_factor`, `adc_bits`, `high_pass`,
  `low_pass`; `step` = conversion factor × 10^exponent); `ChannelData`, a channels × samples
  matrix; `ChannelDataTimeStamps`, one row per segment: time stamp of its first sample (µs),
  first and last column (`McsSegment`: `timestamp_us`, `first`, `last`).
- `EventStream/Stream_<n>/EventEntity_<n>`: 2 × n (time stamps, durations, µs);
  `TimeStampStream/Stream_<n>/TimeStampEntity_<n>`: 1 × n time stamps (µs). They become
  `McsEventTable` (`name`, `entities`, `rows`) of `McsEvent` (`time_s`, `duration_s`, `entity`),
  at most `MAX_MCS_HDF5_EVENTS` rows.
- Segment streams (spike cutouts, averages) and frame streams (CMOS sensor frames) are listed in
  `not_read`.
- `version_text` is `RawData <version>`; `writer` the generating application (else the
  recording program) and version; `recorded_at` the `/Data` date in ticks as ISO 8601 without a
  zone (the recording computer's local time).

## Mapping

- **One trace per analog stream**, named by the stream label. Sample rate = 10⁶ / tick (every
  channel of a stream must share one tick). Channels in record order, each read from its own
  matrix row. Values are (stored − ADZero) × ConversionFactor × 10^Exponent in the record's
  unit (`scale` and `offset` report the same map); digital streams keep their stored numbers
  (unit empty). Each segment is a sweep: `start_s` is the first segment's time stamp,
  `extra.sweep_starts_s` the others' after it, `extra.sweep_sample_counts` their lengths.
- Channel `extra`: `channel_id`, `row`, `group_id`, `ad_zero`, `conversion_factor`, `exponent`,
  `adc_bits`, `high_pass_filter`, `low_pass_filter` (`type`, `cutoff_hz`, `order`).
- Trace `extra`: `recording`, `stream`, `data_subtype`; on trace 0 `recorded_at`, `writer`,
  `writer_version`, `recording_program`, `recording_program_version`, `mea_name`, `mea_layout`.
- **Tables** `events` and `timestamps`: `time_s`, `duration_s`, `entity` (a code into
  `extra.categories`, `<stream label> / <entity label>`), sorted by time.
- Reads are capped at `MAX_MCS_HDF5_READ` samples and `MAX_MCS_HDF5_READ_BYTES` per call.

## HDF5 helpers (`h5rec`, shared with `brw` and `maxwell-h5`)

Compound table datasets are decoded member by member (`records`, `Record`, `field`, `Field`:
`Int`, `Float`, `Text`, `Other`, with `int`, `float`, `text`); variable-length strings are
resolved in the file's global heap collections (`Heap`, `object`, `vl_size`; collections up to
`MAX_H5_HEAP_BYTES`, tables up to `MAX_H5_RECORDS` records). `Matrix` (`path`, `rows`, `cols`,
`pixel_type`, `filters`, `read`) reads column windows of a 2-D sample matrix chunk by chunk;
`MAX_H5_MATRIX_ROWS`, `MAX_H5_TABLE_BYTES` and `MAX_H5_EXPANSION` bound what is read; `decode_samples` and `dtype_of` turn its little-endian samples into f64 and name their type.

## `check` finding codes

`bad_segment` (error); `events_capped`, `shared_row`, `zero_conversion`,
`overlapping_segments` (warnings).

## Oracle mapping

`oracle/gen.py` `mcs_hdf5()`: h5py reads the matrices, information records and time stamps,
values by the definition's formula; McsPyDataTools (run as a black box) must agree to 1e-9.

## Vocabulary (every public identifier in `crates/openreadout-ephys/src/mcsh5/` and `src/h5rec.rs` must appear here)

| identifier | meaning |
| --- | --- |
| `McsHdf5Reader`, `MCS_HDF5_FORMAT_ID`, `McsHdf5Dataset`, `open`, `open_input`, `mcs`, `protocol_type`, `memfind` | entry points and detection |
| `McsFile`, `parse_mcs_hdf5`, `file_len`, `protocol_version`, `attributes`, `analog`, `tables`, `not_read`, `findings`, `version_text`, `writer`, `recorded_at` | the file |
| `McsAnalogStream`, `recording`, `stream`, `label`, `data_subtype`, `channels`, `segments`, `matrix`, `digital` | an analog stream |
| `McsChannel`, `id`, `row`, `group`, `raw_data_type`, `unit`, `exponent`, `ad_zero`, `tick_us`, `conversion_factor`, `adc_bits`, `high_pass`, `low_pass`, `step` | a channel record |
| `McsSegment`, `timestamp_us`, `first`, `last` | a segment |
| `McsEventTable`, `name`, `entities`, `rows`, `McsEvent`, `time_s`, `duration_s`, `entity` | events and time stamps |
| `MAX_MCS_HDF5_READ`, `MAX_MCS_HDF5_READ_BYTES`, `MAX_MCS_HDF5_EVENTS` | limits |
| `Field`, `Int`, `Float`, `Text`, `Other`, `int`, `float`, `text`, `Record`, `field`, `records`, `Heap`, `object`, `vl_size`, `Matrix`, `path`, `rows`, `cols`, `pixel_type`, `filters`, `read`, `decode_samples`, `dtype_of`, `MAX_H5_RECORDS`, `MAX_H5_HEAP_BYTES`, `MAX_H5_MATRIX_ROWS`, `MAX_H5_TABLE_BYTES`, `MAX_H5_EXPANSION`, `check_backed` | HDF5 helpers |
