# Plexon PLX and PL2

Plexon recording systems write `.plx` and the newer `.pl2` files. OpenReadout returns continuous channels as traces, spike waveforms as a table `spikes` and events as a table `events`, for both containers.

PLX support is derived from Neo's `PlexonRawIO` (BSD-3, read as documentation) and checked against public files with Neo as a reference reader. PL2 support is derived from hex dumps of public files only: Plexon's PL2 reader is a Windows DLL that the clean-room rules keep us from opening, and Neo's PL2 path calls it. PL2 is therefore validated for internal consistency only. See `docs/provenance/plexon.md`.

Crate: `openreadout-plexon`, format id `plexon` (`FORMAT_ID`), extensions `plx`, `pl2` (`EXTENSIONS`), reader `PlexonReader`. Both containers map the same way:

- **Continuous (analog) channels → traces.** Channels on an identical sample grid (same rate, same start timestamp and length of every gap-free run) share one trace, in header order. A run of blocks is gap-free while each block starts within half a sample period of where the previous one ended (`runs` → `Run`: `first_block`, `block_count`, `samples`, `timestamp`); each run is a sweep. Channels whose header exists but that have no data are not listed. Trace `name` = the alphabetic prefix shared by the channel names (`WB`, `FP`, `AI`) when there is one; `start_s` = first timestamp ÷ clock; `extra.sweep_start_timestamps` (clock ticks), `sweep_starts_s`, `sweep_sample_counts` (when sweeps differ), `timestamp_clock_hz`; the recording date (`recorded_at`) and acquisition application (`application`; PL2 also `application_version`) go on the first trace, or on the first table of a file without traces, where the experiment model finds them.
- **Spike waveforms → table `spikes`** (one row per waveform), **events → table `events`** (one row per event); a table exists when the file has headers of that kind or data for it. Rows are in file order.

Timestamps count the file's clock (`clock_hz`, 40 kHz in the corpus) from the recording start.

## Detection

PLX: `PLEX` at byte 0 (`PLX_MAGIC`). PL2: byte 0 = 0xFE and `PLEXON` at byte 10 (`PL2_MAGIC`, `PL2_MAGIC_AT`, `looks_like_pl2`). Either → definite; a `.plx`/`.pl2` name without them → extension only.

## PLX (`PlxDataset`, `PlxTrace`, `parse_plx` → `PlxFile`)

All integers little-endian. `PlxFile`: `header`, `spike_channels`, `event_channels`, `continuous_channels`, `data_start`, `file_len`, `index`.

### File header (`FILE_HEADER_LEN` = 7504, `parse_file_header` → `PlxHeader`)

| offset | type | our name |
| --- | --- | --- |
| 0 | char[4] | `PLEX` |
| 4 | i32 | `version` (100–106 known) |
| 8 | char[128] | `comment` |
| 136 | i32 | `clock_hz` (timestamp ticks per second) |
| 140, 144, 148 | i32 × 3 | `spike_channel_count`, `event_channel_count`, `continuous_channel_count` (each ≤ `MAX_CHANNEL_HEADERS`) |
| 152, 156 | i32, i32 | `waveform_points`, `pre_threshold_points` |
| 160–180 | i32 × 6 | year, month, day, hour, minute, second → `recorded_at` |
| 188 | i32 | `waveform_rate_hz` |
| 192 | f64 | `last_timestamp` |
| 200, 201 | u8, u8 | `electrodes_per_channel`, `data_electrodes_per_channel` (version ≥ 103) |
| 202, 203 | u8, u8 | `spike_bits`, `continuous_bits` (≥ 103) |
| 204, 206 | u16, u16 | `spike_max_mv`, `continuous_max_mv` (≥ 103) |
| 208 | u16 | `spike_preamp_gain` (≥ 105) |
| 210, 228 | char[18] × 2 | `acquired_with`, `processed_with` (≥ 106) |
| 256–7503 | i32 arrays | per-channel counts (not used) |

### Channel headers

Spike channels (`SPIKE_HEADER_LEN` = 1020 each, `SpikeChannel`): 0 `name` (32), 32 `signal_name` (32), 64 `channel`, 80 `gain`, 84 `filter`, 88 `threshold`, 96 `unit_count`, 848 `comment` (128, ≥ 105). Event channels (`EVENT_HEADER_LEN` = 296, `EventChannel`): 0 `name`, 32 `channel`, 36 `comment`. Continuous channels (`CONTINUOUS_HEADER_LEN` = 296, `ContinuousChannel`): 0 `name`, 32 `channel`, 36 `rate_hz`, 40 `gain`, 44 `enabled`, 48 `preamp_gain`, 56 `comment`. They follow the file header in that order; data blocks start after them (`data_start`).

### Data blocks (`BLOCK_HEADER_LEN` = 16, `block_header`, `walk_blocks` → `PlxIndex`)

| offset | type | meaning |
| --- | --- | --- |
| 0 | u16 | block type: `BLOCK_SPIKE` (1), `BLOCK_EVENT` (4), `BLOCK_CONTINUOUS` (5); anything else → `bad_block`, walk stops |
| 2 | u16 | timestamp bits 32–39 |
| 4 | u32 | timestamp bits 0–31 |
| 8 | u16 | channel number |
| 10 | u16 | unit (spikes) or event value (events: a strobed word, or 0) |
| 12, 14 | u16, u16 | waveforms, words per waveform; then waveforms × words int16 samples |

`PlxIndex`: `continuous` (per channel number, `SampleBlock`: `offset` of the first sample, `timestamp`, `samples`), `spikes` (block offsets), `max_waveform`, `events` (`EventRecord`: `timestamp`, `channel`, `value`), `block_counts`, `data_end`, `findings`. A block that runs past the end of the file → `truncated`, walk stops (earlier blocks stay readable).

### Scaling (Neo's formulas; values in mV)

Continuous (`continuous_scale`): version ≤ 101: 5000 / (2048 × gain × 1000); 102: 5000 / (2048 × gain × preamp gain); ≥ 103: `continuous_max_mv` / (0.5 × 2^`spike_bits` × gain × preamp gain) (Neo uses the spike bit depth here; the corpus files have equal depths). Spike waveforms (`spike_scale`): ≤ 102: 3000 / (2048 × gain × 1000); 103–104: `spike_max_mv` / (0.5 × 2^bits × gain × 1000); ≥ 105: `spike_max_mv` / (0.5 × 2^bits × gain × `spike_preamp_gain`).

### Tables

`spikes`: `time_s`, `timestamp`, `channel`, `unit` (0 = unsorted), `w0`… (mV; NaN past a shorter waveform); `extra.channels[]` (`channel_number`, `name`, `signal_name`, `gain`, `mv_per_count`, `threshold`, `filter`, `unit_count`), `waveform_points`, `pre_threshold_points`, `waveform_rate_hz`, `timestamp_clock_hz`. `events`: `time_s`, `timestamp`, `channel`, `value`; `extra.channels[]`. Reads are capped at `MAX_TABLE_READ` rows.

## PL2 (`Pl2Dataset`, `Pl2Trace`, `parse_pl2` → `Pl2File`)

`Pl2File`: `header`, `spike_channels`, `analog_channels`, `digital_channels`, `file_len`, `index`.

**Records.** After the file header everything is a record: byte 0 record type, byte 1 source (device) number, u16 at 2 a length in 16-bit words; the record takes `record_len(words)` = 16 (`RECORD_HEADER_LEN`) + 2 × words rounded up to 16 bytes. Spike and event records compute their length from their counts (the word field is 16 bits wide).

### File header (`PL2_FILE_HEADER_LEN` = 0x480, `parse_pl2_header` → `Pl2Header`)

| offset | type | our name |
| --- | --- | --- |
| 0 | u8 | 0xFE |
| 10 | char[6] | `PLEXON` |
| 0x20 | u64 | `headers_end` |
| 0x28 | u64 | `data_start` |
| 0x30 | u64 | `footer_start` (end of the data records) |
| 0x38 | u64 | `index_start` (footer per-channel index; not read) |
| 0x40 | u64 | `start_count` |
| 0x48 | u64 | `duration_ticks` (also in the end record) |
| 0xE0 | char[256] | `comment` |
| 0x1E0 | char[64] | `application` (`OmniPlex`) |
| 0x220 | char[16] | `application_version` |
| 0x230 | u32 × 9 | second, minute, hour, day, month (0-based), year − 1900, weekday, day of year, DST → `recorded_at` |
| 0x258 | f64 | `clock_hz` |
| 0x260, 0x264, 0x26C, 0x274 | u32 | `channel_counts`: total, spike, analog, digital channel headers (each ≤ `MAX_PL2_CHANNELS`) |

### Channel headers (from 0x480: spike, then analog, then digital; → `Pl2Channel`)

| offset | type | our name |
| --- | --- | --- |
| 0 | u8 | `REC_SPIKE_HEADER` 0xD5 (2592 bytes), `REC_ANALOG_HEADER` 0xD4 (512), `REC_DIGITAL_HEADER` 0xD6 (368) |
| 1 | u8 | `source` |
| 16 | char[64] | `name` |
| 0x54 | u32 | `channel` (numbered within the source) |
| 0x58, 0x5C | u32, u32 | `enabled`, `recording` |
| 0x60 | char[16] | `units` (`Volts` → `V`; spike and analog) |
| 0x70, 0x78 | f64, f64 | `rate_hz`, `units_per_count` (spike and analog) |
| 0x80, 0x84, 0x88 | u32, i32, u32 | `waveform_samples`, `threshold`, `pre_threshold` (spike) |
| 0xA0 | u64 × `UNIT_SLOTS` | `unit_counts`: spikes per unit (spike; trailing zeros dropped) |
| 0xD8 | char[64] | `device` (analog) |

### Data records (`walk_records` → `Pl2Index`)

| type | fields after byte 4 | payload |
| --- | --- | --- |
| `REC_ANALOG` 0x42 | u16 channel, u16 sample count (also at byte 2), u64 timestamp of the first sample | int16 samples (a full record holds 65535 samples) |
| `REC_SPIKES` 0x31 | u16 channel, u16 samples per waveform, u32 spike count | u64 timestamps, u16 units, then the waveforms spike by spike (int16) |
| `REC_EVENTS` 0x5A | u16 channel, u16 event count | u64 timestamps, u16 values |
| `REC_END` 0x59 | — | u64 recording length at byte 24 (`end_duration`), read only from OmniPlex's 48-byte record (word count `END_RECORD_WORDS` = 10); offline-written files end the data with a longer 0x59 record that does not hold it |

Other record types are counted in `record_counts` and skipped. `Pl2Index`: `analog` (per (source, channel), `SampleBlock`s), `spikes` (`SpikeRun`: `offset`, `source`, `channel`, `waveform_samples`, `count`, `first_row`), `spike_rows`, `max_waveform`, `events` (`EventRun`: `offset`, `source`, `channel`, `count`, `first_row`), `event_rows`, `record_counts`, `end_duration`, `walk_end`, `findings`. The footer (a copy of the device settings and a per-channel index) is not read.

Values: analog and spike samples × `units_per_count` in the header's unit (V). Tables: `spikes` `time_s`, `timestamp`, `source`, `channel`, `unit`, `w0`…; `events` `time_s`, `timestamp`, `source`, `channel`, `value` (u16 as stored: the demo's strobed words carry bit 15).

## `check` finding codes

PLX: `truncated`, `bad_block`, `bad_clock`, `bad_rate` (errors); `unknown_version`, `timestamps_decrease`, `unlisted_channel`, `unlisted_spike_channel` (warnings); `segments` (info). PL2: `truncated`, `bad_clock` (errors); `no_end_record`, `footer_mismatch`, `duration_mismatch`, `spike_count_mismatch`, `unlisted_channel`, `timestamps_decrease` (warnings); `segments` (info). A file cut inside its headers fails to open as corrupt (exit 4); a cut data block/record is `truncated` (exit 4 from `check`).

## Observed corpus values

| file | container | traces | spikes | events |
| --- | --- | --- | --- | --- |
| `plexon-file-plexon-1` | PLX 101 | — | 24717 (4 channels, 40-sample waveforms) | — |
| `plexon-file-plexon-2` | PLX 101 | — (64 empty continuous headers) | 125820 | 2 |
| `plexon-file-plexon-3` | PLX 106 | `V1` 1 kHz, 600806 samples | 8351 | — |
| `plexon-4chdemoplx` (held) | PLX 106 | WB ×4 40 kHz, FP ×4 1 kHz, AI 1 kHz | 9819 | 1281 |
| `plexon-nc16fpspkevt-1m` | PL2 (OmniPlex 1.22.0) | FP01/05/09/13 1 kHz, 60055 samples | 16245 | 214 |
| `plexon-4chdemopl2` (held) | PL2 (OmniPlex 1.10.29) | WB ×4 40 kHz (1178799), FP ×4, AI | 12313 | 1279 |

## Vocabulary (every public identifier in `openreadout-plexon` must appear here)

| identifier | meaning |
| --- | --- |
| `PlexonReader`, `FORMAT_ID`, `EXTENSIONS`, `open`, `MAX_TABLE_READ` | entry points and limits |
| `PlxDataset`, `PlxTrace`, `plx`, `channels`, `rate_hz`, `runs` | PLX dataset and its traces |
| `PLX_MAGIC`, `FILE_HEADER_LEN`, `SPIKE_HEADER_LEN`, `EVENT_HEADER_LEN`, `CONTINUOUS_HEADER_LEN`, `BLOCK_HEADER_LEN`, `BLOCK_SPIKE`, `BLOCK_EVENT`, `BLOCK_CONTINUOUS`, `MAX_CHANNEL_HEADERS` | PLX constants |
| `PlxHeader`, `version`, `comment`, `clock_hz`, `spike_channel_count`, `event_channel_count`, `continuous_channel_count`, `waveform_points`, `pre_threshold_points`, `recorded_at`, `waveform_rate_hz`, `last_timestamp`, `electrodes_per_channel`, `data_electrodes_per_channel`, `spike_bits`, `continuous_bits`, `spike_max_mv`, `continuous_max_mv`, `spike_preamp_gain`, `acquired_with`, `processed_with`, `continuous_scale`, `spike_scale`, `parse_file_header` | PLX file header |
| `SpikeChannel`, `EventChannel`, `ContinuousChannel`, `name`, `signal_name`, `channel`, `gain`, `filter`, `threshold`, `unit_count`, `enabled`, `preamp_gain` | PLX channel headers |
| `PlxFile`, `header`, `spike_channels`, `event_channels`, `continuous_channels`, `data_start`, `file_len`, `index`, `parse_plx` | PLX file |
| `PlxIndex`, `continuous`, `spikes`, `max_waveform`, `events`, `block_counts`, `data_end`, `findings`, `walk_blocks`, `block_header` | PLX block walk |
| `SampleBlock`, `offset`, `timestamp`, `samples`, `EventRecord`, `value`, `Run`, `first_block`, `block_count` | blocks, events and gap-free runs |
| `Pl2Dataset`, `Pl2Trace`, `pl2` | PL2 dataset and its traces |
| `PL2_MAGIC`, `PL2_MAGIC_AT`, `PL2_FILE_HEADER_LEN`, `RECORD_HEADER_LEN`, `REC_SPIKE_HEADER`, `REC_ANALOG_HEADER`, `REC_DIGITAL_HEADER`, `REC_ANALOG`, `REC_SPIKES`, `REC_EVENTS`, `REC_END`, `END_RECORD_WORDS`, `UNIT_SLOTS`, `MAX_PL2_CHANNELS`, `record_len`, `looks_like_pl2` | PL2 constants and helpers |
| `Pl2Header`, `headers_end`, `footer_start`, `index_start`, `start_count`, `duration_ticks`, `application`, `application_version`, `channel_counts`, `parse_pl2_header` | PL2 file header |
| `Pl2Channel`, `source`, `recording`, `units`, `units_per_count`, `waveform_samples`, `pre_threshold`, `unit_counts`, `device` | PL2 channel headers |
| `Pl2File`, `analog_channels`, `digital_channels`, `parse_pl2` | PL2 file |
| `Pl2Index`, `analog`, `spike_rows`, `event_rows`, `record_counts`, `end_duration`, `walk_end`, `walk_records`, `SpikeRun`, `EventRun`, `count`, `first_row` | PL2 record walk |

### Byte-source integration

The existing layouts are read through `Input`/`Fs`/`SourceFile`, including directory
sessions and companions where applicable. Path, memory and callback namespaces use
the same parser and scaling. See [byte sources](../../book/src/guides/wasm.md).

The offline-writer rules above (footer offset 0, the 10-word end record) were inferred from a depositor's offline-written PL2 files and their paired PLX. That record is kept for testing on files from sources not used during development, so no development file exercises these rules (see `docs/provenance/plexon.md`).
