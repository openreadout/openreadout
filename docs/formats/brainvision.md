# BrainVision Core Data Format (`.vhdr`, `.vmrk`, `.eeg`)

Brain Products' BrainVision Recorder and Analyzer store an EEG recording as three files: a text
header (`.vhdr`), a text marker file (`.vmrk`) and the samples (`.eeg`, or `.dat` in some
exports). pybv, MNE-Python, FieldTrip, EEGLAB and BciPy write the same layout, and BIDS accepts it
for EEG. OpenReadout reads the recording from any of the three files; the header names the other
two.

Derived from the BSD-licensed readers and writer that document the layout (Neo's
`BrainVisionRawIO`, MNE-Python's BrainVision reader, pybv) and from the corpus files
(`docs/provenance/brainvision.md`). MNE-Python and Neo are run as the reference readers.

Crate: `openreadout-ephys`, format id `brainvision` (`BRAINVISION_FORMAT_ID`), extensions `vhdr`
and `vmrk`, reader `BrainVisionReader`, dataset `BrainVisionDataset` (`open`, `open_input`,
`header`). Detection: a first line that identifies a BrainVision header (`looks_like_vhdr`) →
definite; a `.vmrk` whose first line identifies a marker file (`looks_like_vmrk`) → definite; an
`.eeg` or `.dat` file with a `.vhdr` of the same stem next to it → likely. Axona `.eeg` files are
recognised by their own content first.

## Layout (`parse_header` → `BvHeader`)

- **Header** (`.vhdr`), UTF-8 or Windows-1252 text (`decode_text`). The first line
  (`identification`) is `Brain Vision Data Exchange Header File Version 1.0`, `BrainVision Data
  Exchange Header File Version 2.0` or `BrainVision Core Data Format 1.0`
  (`identification_version` → `version`). The next line, a `;` comment, often names the writer
  (`writer_note`, `writer`: `BrainVision Recorder`, `BrainVision Analyzer` for "history path"
  exports, `pybv`, `MNE-Python`, `FieldTrip`, `EEGLAB`, `BciPy`). Then INI sections of `key=value`
  lines (`parse_ini` → `sections`, case-insensitive) up to `[Comment]`, whose free text
  (amplifier and filter settings) is kept as `comment`.
  - `[Common Infos]`: `DataFile` and `MarkerFile` (resolved next to the header: as named, then
    without regard to case, then under the header's own stem → `data_path`, `marker_path`),
    `DataFormat` (`BINARY` or `ASCII`), `DataOrientation` (`MULTIPLEXED`: frames of one sample per
    channel; `VECTORIZED`: all of channel 1, then channel 2 … → `vectorized`), `DataType` (only
    `TIMEDOMAIN` is read), `NumberOfChannels` (at most `MAX_BV_CHANNELS`), `SamplingInterval` in
    µs (`sampling_interval_us`; `rate_hz` = 1,000,000 / interval), `DataPoints` (`data_points`),
    `SegmentationType` (`segmentation`), `SegmentDataPoints` (`segment_points`), `Averaged`
    (`averaged`).
  - `[Binary Infos]`: `BinaryFormat` (`BvSampleFormat`: `Int16` `INT_16`, `UInt16` `UINT_16`,
    `Int32` `INT_32`, `Float32` `IEEE_FLOAT_32`; `bytes`, `name`, `dtype`) and
    `UseBigEndianOrder` (`big_endian`).
  - `[ASCII Infos]` (`Ascii` format): `DecimalSymbol` (`decimal_symbol`), `SkipLines`
    (`skip_lines`), `SkipColumns` (`skip_columns`). Only MULTIPLEXED ASCII data is read; every
    line is indexed at open, up to `MAX_BV_ASCII` bytes.
  - `[Channel Infos]`: `Ch<n>=<name>,<reference>,<resolution>,<unit>` (`BvChannel`: `name`,
    `reference`, `resolution`, 1 when empty, `unit`, `µV` when absent); `\1` stands for a comma.
    `[Coordinates]`: `Ch<n>=<radius>,<theta>,<phi>` (`coordinates`).
- **Markers** (`.vmrk`, `parse_markers`): `[Marker Infos]` `Mk<n>=<type>,<description>,<position>,
  <points>,<channel>[,<date>]` (`BvMarker`: `kind`, `description`, `position` counting from 1,
  `points`, `channel`, 0 for all channels, `date` `YYYYMMDDhhmmssuuuuuu` on `New Segment` markers),
  in `Mk` number order, at most `MAX_BV_MARKERS`. The header and marker files are read up to
  `MAX_BV_TEXT` bytes.
- **Values**: stored sample × the channel's `resolution`, in the channel's unit, for every format
  (pybv writes floats divided by the resolution too).
- `recorded_at` is the first `New Segment` marker's date, `yyyy-mm-ddThh:mm:ss.uuuuuu`.

## Mapping

- One trace `signals`: a channel per header channel (dtype of the stored format, `scale` the
  resolution, `offset` 0; channel `extra`: `reference`, `resolution`, `coordinates`). Samples per
  channel: whole frames in the data file, or `DataPoints` when it is smaller.
- **Sweeps**: one, except when `SegmentationType=MARKERBASED` and `SegmentDataPoints` divides the
  data evenly: then each segment is a sweep (`extra.sweep_starts_s`).
- Trace `extra`: `header_version`, `binary_format`, `orientation`, `sampling_interval_us`,
  `segmentation`, `averaged`, `recorded_at`, `application` (the writer) and `written_by` (the
  writer note as written).
- **Table** `markers` (when the marker file has any): `onset_s` and `duration_s` (seconds after
  the first sample: (position − 1) / rate and points / rate), `sample` (position − 1), `points`,
  `channel`, and `type` and `description` as codes into their columns' `extra.categories`.
- `format_version` is the header version. `member_files` lists the three files.
- Reads are capped at `MAX_BV_READ` samples per channel and `MAX_BV_READ_BYTES` bytes.

## `check` finding codes

`missing_channel`, `truncated`, `zero_resolution` (errors); `missing_marker_file`,
`marker_identification`, `bad_marker`, `partial_frame`, `extra_bytes`, `marker_past_end`
(warnings).

## Oracle mapping

`oracle/gen.py` `brainvision()`: Neo's `BrainVisionRawIO` gives the stored samples in their own
type, MNE-Python each channel's resolution and the markers (`_read_mrk`); values are the stored
samples × resolution in float64, checked against MNE's own values. Units are Neo's where it read
one, else MNE's.

## Vocabulary (every public identifier in `crates/openreadout-ephys/src/brainvision/` must appear here)

| identifier | meaning |
| --- | --- |
| `BrainVisionReader`, `BRAINVISION_FORMAT_ID`, `BrainVisionDataset`, `open`, `open_input`, `header` | entry points |
| `BvHeader`, `parse_header`, `parse_ini`, `decode_text`, `identification_version`, `version`, `identification`, `writer_note`, `writer`, `sections`, `comment`, `data_path`, `marker_path`, `format`, `vectorized`, `big_endian`, `sampling_interval_us`, `rate_hz`, `data_points`, `segmentation`, `segment_points`, `averaged`, `decimal_symbol`, `skip_lines`, `skip_columns`, `channels`, `markers`, `findings`, `recorded_at` | the header |
| `BvSampleFormat`, `Int16`, `UInt16`, `Int32`, `Float32`, `Ascii`, `bytes`, `name`, `dtype` | sample formats |
| `BvChannel`, `reference`, `resolution`, `unit`, `coordinates` | a channel |
| `BvMarker`, `parse_markers`, `kind`, `description`, `position`, `points`, `channel`, `date` | a marker |
| `MAX_BV_TEXT`, `MAX_BV_CHANNELS`, `MAX_BV_MARKERS`, `MAX_BV_READ`, `MAX_BV_READ_BYTES`, `MAX_BV_ASCII`, `looks_like_vhdr`, `looks_like_vmrk` | limits and detection |
