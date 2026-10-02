# Neuralynx (NCS, NEV, NSE, NST, NTT)

Neuralynx Cheetah systems save a recording as a directory of per-channel files: continuous signals, events, spike waveforms and video tracking. OpenReadout reads one file at a time: continuous files become traces, the others tables. Derived from Neuralynx's public record-format documentation ("Neuralynx Data File Formats", Rev 1.1, 2013, and the web page of the same name) and checked on public files from Cheetah 4.0.2 to 6.4.1 and the BML tools; Neo (BSD-3) was read as documentation and is the reference reader. Provenance: `docs/provenance/neuralynx.md`.

Crate: `openreadout-neuralynx` (one crate per electrophysiology format, like the microscopy readers; ABF, Blackrock, SpikeGLX and Intan have their own crates). The file kinds:

| extension | `FileKind` | record (`record_len`) | exposed as |
| --- | --- | --- | --- |
| `.ncs` | `Continuous` | 1044 bytes (`NCS_RECORD_LEN`) | one trace, one channel; gap-free segments are sweeps |
| `.nev` | `Events` | 184 bytes (`NEV_RECORD_LEN`) | one table, one row per event |
| `.nse` / `.nst` / `.ntt` | `Spikes { electrodes: 1 / 2 / 4 }` | 48 + 64 × electrodes (112 / 176 / 304) | one table, one row per spike waveform |
| `.nvt` | `Video` | 1828 bytes (`NVT_RECORD_LEN`) | one table, one row per video frame |

The kind comes from `-RecordSize`, else the extension, else `-FileType` (`FileKind::from_record_len`, `from_extension`; `-FileType Video` → `Video`). Raw (`.nrd`) files are not read.

## Text header (`TextHeader`, `parse_header`)

Every file starts with a 16384-byte text header (`HEADER_LEN`), NUL-padded. The first line is usually `######## Neuralynx Data File Header` (`HEADER_MAGIC`, `has_magic`; missing in some Pegasus exports). Text is Latin-1 or UTF-8 (`decode_text`); lines end in CRLF or LF. Lines of the form `-Key value` become `entries` (`HeaderEntry { key, value }`; indentation and a trailing `:` on the key are ignored; values may be quoted or hold several space-separated numbers); lines starting with `##` become `comments`. Key lookups (`get`, `text`, `number`, `numbers`, `flag`) compare the ASCII letters of keys case-insensitively, so `DspFilterDelay_µs` matches however the µ was written.

| key | our field | use |
| --- | --- | --- |
| `SamplingFrequency` | `sample_rate_hz` | NCS rate; spike `waveform_rate_hz` |
| `ADBitVolts` (one per electrode) | `scale` | µV per count = ADBitVolts × 10⁶ |
| `AcqEntName` (else `NLX_Base_Class_Name`) | channel / table `name` | `ch0` when absent |
| `ADChannel` | `ad_channel` | A/D channel(s); differs from the record's channel number |
| `InputRange` | `input_range_uv` | ±µV |
| `InputInverted` | `input_inverted` | reported only, see Scaling |
| `ADMaxValue` | `ad_max_value` | |
| `DSPLowCutFilterEnabled`, `DspLowCutFrequency`, `DspLowCutNumTaps`, `DspLowCutFilterType`, the `HighCut` four, `DspDelayCompensation`, `DspFilterDelay_µs` | `dsp { low_cut_enabled, low_cut_hz, low_cut_taps, low_cut_type, high_cut_enabled, high_cut_hz, high_cut_taps, high_cut_type, delay_compensation, filter_delay_us }` | channel `extra` |
| `ReferenceChannel` | `reference` | |
| `ApplicationName` / `CheetahRev` | `application`, `application_version` (`application()`) | |
| `AcquisitionSystem`, `HardwareSubSystemType`, `FileVersion`, `FileUUID`, `SessionUUID`, `NLX_Base_Class_Type` | `acquisition_system`, `hardware_subsystem`, `file_version` (also `format_version`), `file_uuid`, `session_uuid`, `base_class` | |
| `TimeCreated` / `TimeClosed`, or `## Time Opened …` / `## Time Closed …` / `## Date Opened …` | `opened_at`, `closed_at` (`opened_at()`, `closed_at()`) | ISO-8601, local time (no zone in the file) |
| `OriginalFileName`, or `## File Name …` | `original_file_name` (`original_file_name()`) | |
| `WaveformLength`, `AlignmentPt`, `Feature …` | spike `samples_per_waveform`, `alignment_sample`, `features` | |

## NCS records (`RecordHead`, `record_head`)

| offset | type | our name |
| --- | --- | --- |
| 0 | u64 | `timestamp_us` of the first sample, µs |
| 8 | u32 | `channel_number` (not the A/D channel) |
| 12 | u32 | `rate_hz` (the header rate truncated to an integer) |
| 16 | u32 | `valid`: samples of the record that hold data (≤ 512, `NCS_SAMPLES`) |
| 20 | i16[512] | samples |

Samples after `valid` hold stale data and are never returned.

### Segments (sweeps; `Segment`, `segments_from`, `ContinuousIndex`)

`dt` = median of Δtimestamp / 512 over consecutive full records (`sample_interval_us`; fallback 1e6 / header rate). A new segment starts where a record's timestamp departs from `previous timestamp + previous valid × dt` by more than `dt / 2`. Each `Segment` has `first_record`, `record_count`, `sample_count` (sum of valid samples) and `first_timestamp_us`. `info` uses a fast path (`index_continuous` with `full = false`): when the first record is full and the last record's timestamp equals the first's plus `(records − 1) × 512` samples at the header rate (± half a sample), the file is one segment and only those two records are read (`scanned` false); otherwise every record header is read. `check` always reads every record (`valid_in` gives the valid count of a record).

Trace `extra`: `file_kind`, `record_count`, `record_channel_number`, `first_timestamp_us`, `timestamp_rate_hz` (1e6 / `dt`), `sweep_sample_counts`, `sweep_starts_s` (relative to the first record), `sweep_start_timestamps_us`, `segmented_by`, plus the header fields above. The Cheetah 4.0.2 corpus file says 27789 Hz while its timestamps advance 35 µs per sample (28571.4 Hz, a 1 MHz clock); we report the header rate and `timestamp_rate_hz`, and `check` warns `rate_mismatch`.

### Scaling

value (µV) = raw × ADBitVolts × 10⁶, no sign change. Neuralynx documents that Cheetah inverts the input itself when `-InputInverted True`, and its worked example applies no inversion to such a file; Neo negates the gain for these files. `extra.input_inverted` reports the flag.

## Event records (`EventRecord`, `event_record`)

| offset | type | our name / column |
| --- | --- | --- |
| 0 | i16 | reserved |
| 2 | i16 | `packet_id` |
| 4 | i16 | `data_size` (0 or 2 in the corpus) |
| 6 | u64 | `timestamp_us` |
| 14 | i16 | `event_id` |
| 16 | i16 | `ttl` |
| 18 | i16 | CRC (not checked) |
| 20 | i16 × 2 | reserved |
| 24 | i32[8] | `extra` (not in the table) |
| 56 | char[128] | `label` (NUL-terminated) |

Table columns: `timestamp_us`, `event_id`, `ttl`, `packet_id`, `label` (index into table `extra.labels`; `extra.label_counts` counts each string; at most `MAX_LABELS` distinct labels are listed).

## Spike records (`SpikeRecord`, `spike_record`)

| offset | type | our name |
| --- | --- | --- |
| 0 | u64 | `timestamp_us` |
| 8 | u32 | `entity` (acquisition entity) |
| 12 | u32 | `cell` (classified unit, 0 = unsorted) |
| 16 | i32[8] | `features` (`SPIKE_FEATURES`; signed in the corpus, the PDF says unsigned) |
| 48 | i16[32 × electrodes] | samples, point-major ([point, electrode]); `SPIKE_SAMPLES` = 32, `SPIKE_PREFIX_LEN` = 48 |

Table columns: `timestamp_us`, `entity`, `cell`, `feature_0`…`feature_7`, then `w<e>_<i>` = sample `i` of electrode `e` in µV (ADBitVolts of that electrode × 10⁶). Table `extra`: `electrodes`, `samples_per_waveform`, `alignment_sample`, `waveform_rate_hz`, `uv_per_count`, `features`, `channel` (the channel fields above), `record_size`.

## Video-tracker records (`VideoRecord`, `video_record`)

From the vendor's "Video Tracker Record" layout (NeuralynxDataFileFormats.pdf, Rev 1.1): u16 record start (`NVT_RECORD_START`, always 0x800; `start`), u16 originating system (`system_id`), u16 record size (`data_size`), u64 timestamp µs, u32[400] colour-transition points (`NVT_POINTS`), i16 unused, i32 extracted x, i32 extracted y, i32 head angle (degrees clockwise from +Y; 0 when angle tracking is off; invalid before Cheetah 5), i32[50] targets sorted by size (`NVT_TARGETS`; 0 = no target).

Table columns: `timestamp_us`, `x`, `y` (pixels; 0, 0 when nothing was tracked — left as recorded, not blanked), `angle` (°), `target_count` (non-zero target entries). The points and the target bitfields (12-bit x and y plus colour flags) are not decoded. Table `extra`: `frame_rate_hz` (`-SamplingFrequency`), `video_format`, `resolution`, `entity` (`-AcqEntName`), `record_size`. `check` counts records that do not start with 0x800 (`bad_video_record`) besides truncation and backward timestamps.

## Recording directories (sessions)

A directory is opened as one recording (`open_session`) when it holds Neuralynx data files and nothing else but companions (`session_files`; `SESSION_COMPANIONS`: `.txt` logs such as `CheetahLogFile.txt`, `.log`, `.cfg`, `.xml`, `.ini`, `.json`, `.md`, and the `.nrd` files this reader does not decode; `.nvt` files are members since 2026-09-26). Continuous files that hold only the text header are left out. Members are the files in name order. `.ncs` channels on an identical sample grid — the same `sample_rate_hz`, sweep count, sweep lengths and `sweep_start_timestamps_us` — join **one multi-channel trace** (named `continuous_<rate>Hz`), in file-name order; channels with another rate or other segment boundaries form further traces. Each `.nev`, `.nse`, `.nst`, `.ntt`, `.nvt` file is a table. Nothing is resampled or realigned; every trace, channel and table names its file in `extra.source_file` / `extra.source_files`, `info --view structure` lists the member files and `check` checks each one (findings prefixed with the file). The composition itself is `openreadout_core::session` (`SessionDataset`).

Neo (the oracle) groups `.ncs` files into streams by rate, input range and filter settings and refuses a directory whose streams have different segment structures; on the corpus directories its channels and ours coincide.

## `check` finding codes

`truncated`, `bad_record_size`, `bad_valid_count` (errors); `timestamps_decrease`, `bad_video_record`, `mixed_channels`, `rate_mismatch`, `no_records`, `no_scaling` (warnings); `segments`, `no_header_line`, `fast_index_differs` (info). A file shorter than the 16 KiB header fails to open as corrupt (exit 4).

## Detection

`looks_like_neuralynx`: the head starts with `HEADER_MAGIC` → definite. A Neuralynx extension (`EXTENSIONS`) plus `-RecordSize`, `-FileType`, `NLX_Base_Class_Type` or `-ADBitVolts` in the first 4 KiB → likely; the extension alone → extension-only.

## Observed corpus values

| file | writer | rate (header / timestamps) | records | segments | notes |
| --- | --- | --- | --- | --- | --- |
| `nlx-bml-csc1-trunc.ncs` | BML tools | 24000 | 9 | 1 | LF lines, tab-indented keys, no AcqEntName |
| `nlx-bml-unfilledsplit.ncs` | BML CutCsc | 29411 | 3 | 2 | record 1 has 308 valid samples, then a 2^32 µs jump |
| `nlx-cheetah-v4-0-2-csc14-trunc.ncs` | Cheetah 4.0.2 | 27789 / 28571.4 | 10 | 1 | 1 MHz clock (35 µs), ADMaxValue 2047 |
| `nlx-cheetah-v5-4-0-csc5-trunc.ncs` | Cheetah 5.4.0 | 1017.375 | 7 | 1 | `-FileType: CSC` |
| `nlx-cheetah-v5-5-1-tet3a.ncs` | Cheetah 5.5.1 | 32000 | 3332 | 2 | |
| `nlx-cheetah-v5-7-4-csc1.ncs` | Cheetah 5.7.4 | 32000 | 2727 | 4 | partial records of 31/287/287/255 samples |
| `nlx-cheetah-v6-3-2-csc1-reduced.ncs` | Cheetah 6.3.2 | 32000 | | 3 | incomplete blocks |
| `nlx-cheetah-v5-6-3-tt1.ntt` | Cheetah 5.6.3 | 32000 | 5699 | – | tetrode, 4 ADBitVolts |

## Vocabulary (every public identifier in `openreadout-neuralynx` must appear here)

| identifier | meaning |
| --- | --- |
| `session_files`, `open_session`, `SESSION_COMPANIONS` | recording directories (sessions) |
| `NeuralynxReader`, `NeuralynxDataset`, `FORMAT_ID`, `EXTENSIONS`, `open`, `kind`, `text_header`, `looks_like_neuralynx`, `MAX_LABELS`, `MAX_TABLE_READ` | entry points and limits |
| `TextHeader`, `entries`, `comments`, `has_magic`, `text_len`, `HeaderEntry`, `key`, `value`, `HEADER_LEN`, `HEADER_MAGIC`, `parse_header`, `decode_text`, `get`, `text`, `number`, `numbers`, `flag`, `application`, `opened_at`, `closed_at`, `original_file_name` | text header |
| `FileKind` { `Continuous`, `Events`, `Spikes`, `Video`, `electrodes` }, `record_len`, `name`, `from_extension`, `from_record_len` | file kinds |
| `VideoRecord`, `start`, `system_id`, `x`, `y`, `angle`, `target_count`, `video_record`, `NVT_RECORD_LEN`, `NVT_RECORD_START`, `NVT_POINTS`, `NVT_TARGETS` | video-tracker records |
| `RecordHead`, `timestamp_us`, `channel_number`, `rate_hz`, `valid`, `record_head`, `NCS_RECORD_LEN`, `NCS_SAMPLES` | continuous records |
| `Segment`, `first_record`, `record_count`, `sample_count`, `first_timestamp_us`, `segments_from` | segments |
| `ContinuousIndex`, `sample_interval_us`, `scanned`, `first`, `last`, `findings`, `valid_in`, `index_continuous` | continuous index |
| `EventRecord`, `packet_id`, `data_size`, `event_id`, `ttl`, `extra`, `label`, `event_record`, `NEV_RECORD_LEN` | events |
| `SpikeRecord`, `entity`, `cell`, `features`, `samples`, `spike_record`, `SPIKE_SAMPLES`, `SPIKE_FEATURES`, `SPIKE_PREFIX_LEN` | spikes |

### Byte-source integration

The existing layouts are read through `Input`/`Fs`/`SourceFile`, including directory
sessions and companions where applicable. Path, memory and callback namespaces use
the same parser and scaling. See [byte sources](../../book/src/guides/wasm.md).
