# FIF: MEGIN / Elekta Neuromag MEG and EEG recordings (`.fif`)

FIF (Functional Image File format) is the file format of MEGIN (formerly Elekta Neuromag)
MEG systems: Vectorview, TRIUX and their MaxFilter output. MNE-Python and MNE-C write it too,
so EEG converted from other systems, cleaned or resampled recordings and MEG from CTF, KIT and
4D systems often arrive as `_raw.fif` files. OpenNeuro holds several dozen MEG data sets in it.

Derived from MNE-Python's FIF reader (BSD-3), read as documentation, and from the corpus files
(`docs/provenance/fif.md`). MNE-Python is also the reference reader the corpus compares with.

Crate: `openreadout-ephys`, format id `fif` (`FIF_FORMAT_ID`), extension `fif`, reader
`FifReader`, dataset `FifDataset` (`open`, `open_input`, `fif`). Detection (`looks_like_fif`):
the file starts with a file-id tag (kind 100, type 31, 20 bytes) → definite. Other `.fif` files
are extension-only matches.

## Layout (`parse_fif` → `FifFile`)

- **Tags.** Every tag has a 16-byte big-endian header (`FIF_TAG_HEADER`): kind, data type,
  size and the position of the next tag (0 = right after this one, −1 = none), then its data
  (`FifTag`: `kind`, `data_type`, `size`, `pos`, `data_pos`). The first tag is the file id
  (`version` as major × 65536 + minor, `file_id_secs`), the second the position of the tag
  directory. The reader takes the directory when it is there and valid (`from_directory`; an
  entry of −1s ends it), else it follows the tags from the start, also when `next` points
  backwards, and stops at a position it has seen (`tag_count` tags, at most `MAX_FIF_TAGS`).
- **Blocks.** Block-start (104) and block-end (105) tags, whose data is the block kind, nest the
  other tags into a tree (`FifBlock`: `kind`, `tags`, `children`, `find`, `tag`; the root has
  kind 999; at most `MAX_FIF_DEPTH` deep).
- **Measurement info** (block 101 inside the measurement block 100): channel count, sample rate
  (float32), one 96-byte channel-info tag per channel (`FifChannel`: `scan_number`,
  `logical_number`, `kind`, `range`, `cal`, `coil_type`, `loc`, `unit`, `unit_mul`, `name`),
  low-pass and high-pass corners, line frequency, the measurement date as seconds and
  microseconds (seconds 0 with microseconds 2³¹ − 1 means none), UTC offset, experimenter,
  description, project name. Blocks 113 (one per channel, in order) override channel fields;
  their names may be longer than the 16 bytes of the channel-info tag. Block 359 lists bad
  channels as a colon-separated name list (`name_list`, `\:` for a colon in a name). Creator
  tags (113) anywhere in the file are collected (`creators`), and the MNE command line
  (`command_line`). Small tags are read up to `MAX_FIF_TAG_READ` bytes; at most
  `MAX_FIF_CHANNELS` channels.
- **Samples.** The first raw-data block (102), else continuous-data block (112), else
  active-shielding raw-data block (119, MaxShield) of the measurement holds data-buffer tags
  (300) of samples × channels, sample-major, big-endian (`FifBuffer`: `part`, `data_pos`,
  `sample_type`, `samples`, `first`). `FifSampleType` is `Int16` (types 2 and 16), `Int32` (3),
  `Float32` (4) or `Float64` (5); complex buffers are refused. An optional first-sample tag
  (208) numbers the first sample; a data-skip tag (301) before the first buffer moves it on by
  that many buffers, one between buffers marks a gap of that many buffers. Runs of buffers
  without a gap are sweeps (`FifSweep`: `first_buffer`, `end_buffer`, `first`, `samples`).
  `FifDataBlock` names the block (`Raw`, `Continuous`, `ActiveShieldingRaw`).
- **Split files.** A file-reference block (118) with role 2 names the next part by file name
  or number (`name-1.fif`, …); the parts are read as one recording (`parts`, `part_lens`, at
  most `MAX_FIF_PARTS`).
- **Annotations** (block 3810): onsets (3568) and ends (3569) as float32 arrays, descriptions
  as a name list in a comment tag, and an optional origin (204) (`FifAnnotation`: `onset_s`,
  `duration_s`, `text`).
- `version_text` is `major.minor`; `recorded_at` the measurement date in ISO 8601 UTC with
  microseconds; `row_bytes` the bytes of one sample of every channel.

## Mapping

- **One trace** named after the data block (`raw`, `continuous`, `active shielding raw`).
  `sample_rate_hz` is the stored rate; channels in the stored order with their names (`ch<i>`
  for an empty one), unit from the unit code (`unit_name`: `T`, `T/m`, `V`, …, empty for
  unitless), dtype of the first buffer, and `scale` = range × cal in float64
  (`factor`), offset 0. Values are stored number × scale, in SI units. Channel `extra`: `kind`
  (`kind_name`: `meg`, `eeg`, `stimulus`, `eog`, `ecg`, `emg`, `misc`, `reference_meg`, …;
  `kind_code` when the code has no name), `coil_type`, `logical_number`, `scan_number`, `range`,
  `cal`, `unit_mul` (when not 0), `bad`, `position_m` (the first three location values).
- `start_s` is the first sample number / rate (time after the acquisition start). With gaps,
  each run of buffers is a sweep; `extra.sweep_starts_s` (after the first sample) and
  `extra.sweep_sample_counts` give them.
- Trace `extra`: `first_sample`, `data_buffers`, `data_block`, `recorded_at`, `lowpass_hz`,
  `highpass_hz`, `line_freq_hz`, `experimenter`, `description`, `project`, `bad_channels`,
  `creators`, `split_parts`.
- **Table** `annotations`: `onset_s` and `duration_s` (seconds after the first sample) and
  `text` (a code into `extra.categories`). Onsets count from the block's origin when it has one
  (shifted by its difference from the measurement date), else from the first sample when the
  measurement has a date, else from the acquisition start.
- Projections, gradient compensation and SSS information stored in the file are not applied.
  Files of epochs, evoked responses, forward or inverse solutions and covariances are refused
  (exit 6); `other_content` names what a measurement holds instead of raw data.
- Reads are capped at `MAX_FIF_READ` samples per channel and `MAX_FIF_READ_BYTES` per call.

## `check` finding codes

`truncated`, `bad_next` (errors); `directory_unusable`, `trailing_bytes`, `unbalanced_blocks`,
`buffer_size`, `split_part_missing`, `duplicate_channel_name`, `zero_calibration` (warnings);
`mixed_buffer_types` (info).

## Oracle mapping

`oracle/gen.py` `fif()`: MNE-Python `read_raw_fif` (with `allow_maxshield`) gives channel names,
unit codes, the sample rate and `get_data` values; runs of MNE's buffers between skips are the
sweeps. Annotations from `read_annotations`: count and description codes.

## Vocabulary (every public identifier in `crates/openreadout-ephys/src/fif/` must appear here)

| identifier | meaning |
| --- | --- |
| `FifReader`, `FIF_FORMAT_ID`, `FifDataset`, `open`, `open_input`, `fif`, `looks_like_fif` | entry points and detection |
| `FifFile`, `parse_fif`, `file_len`, `version`, `file_id_secs`, `root`, `from_directory`, `tag_count`, `channels`, `sfreq`, `lowpass_hz`, `highpass_hz`, `line_freq_hz`, `meas_date`, `utc_offset`, `experimenter`, `description`, `project`, `bad_channels`, `creators`, `command_line`, `data_block`, `other_content`, `buffers`, `sweeps`, `first_sample`, `parts`, `part_lens`, `annotations`, `findings`, `version_text`, `recorded_at`, `row_bytes` | the file |
| `FifTag`, `kind`, `data_type`, `size`, `pos`, `data_pos` | a tag |
| `FifBlock`, `tags`, `children`, `find`, `tag` | a block |
| `FifChannel`, `scan_number`, `logical_number`, `range`, `cal`, `coil_type`, `loc`, `unit`, `unit_mul`, `name`, `factor`, `kind_name`, `unit_name`, `name_list` | channels |
| `FifSampleType`, `Int16`, `Int32`, `Float32`, `Float64`, `of`, `bytes`, `dtype`, `FifBuffer`, `part`, `sample_type`, `samples`, `first`, `FifSweep`, `first_buffer`, `end_buffer` | samples |
| `FifDataBlock`, `Raw`, `Continuous`, `ActiveShieldingRaw` | data blocks |
| `FifAnnotation`, `onset_s`, `duration_s`, `text` | annotations |
| `FIF_TAG_HEADER`, `MAX_FIF_TAGS`, `MAX_FIF_DEPTH`, `MAX_FIF_CHANNELS`, `MAX_FIF_TAG_READ`, `MAX_FIF_PARTS`, `MAX_FIF_READ`, `MAX_FIF_READ_BYTES` | constants and limits |
