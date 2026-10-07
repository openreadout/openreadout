# Blackrock NSx and NEV

Blackrock Microsystems recording systems write continuous signals as NSx files (`.ns1`–`.ns6`) and spikes and events as NEV files. OpenReadout reads one file at a time: an **NSx** file (`NsxDataset`) is one trace whose sweeps are its data packets (a new packet follows each pause); a **NEV** file (`NevDataset`) is one table with a row per data packet. A directory of these files opens as one recording.

Derived from Blackrock's public specification LB-0023 Rev 7.00 ("NEV and NSx file formats, FileSpec 3.0") and LB-0110, checked against public corpus files (NSx 2.1, 2.3, 3.0 and 3.0 PTP; NEV 2.1, 2.3, 3.0). Spec 2.1 NSx details come from Neo (BSD-3), read as documentation and used as a reference reader. See `docs/provenance/blackrock.md`.

Crate: `openreadout-blackrock`. All integers are little-endian; char arrays end at the first NUL (bytes after it are ignored).

## Detection

Bytes 0–7 (`SIGNATURES`): `NEURALSG` (NSx 2.1), `NEURALCD` (NSx 2.2/2.3), `BRSMPGRP` (NSx 3.0), `NEURALEV` (NEV ≤ 2.3), `BREVENTS` (NEV 3.0) → definite. An extension in `EXTENSIONS` without a signature → extension only.

## NSx

`NsxSpec`: `V21` (`NEURALSG`), `V22` (`NEURALCD`, covers 2.2 and 2.3), `V30` (`BRSMPGRP`); `name()`; `packet_header_len()` (0, 9, 13).

### Basic header, spec 2.2+ (`BASIC_HEADER_LEN` = 314)

| offset | type | our name |
| --- | --- | --- |
| 0 | char[8] | signature |
| 8, 9 | u8, u8 | `version` (major, minor) → `format_version` |
| 10 | u32 | bytes in headers (`header_len`; = 314 + 66 × channels, else `header_length_mismatch`) |
| 14 | char[16] | `label` (trace `name`, e.g. `30 kS/s`) |
| 30 | char[256] | `comment` |
| 286 | u32 | `period`: sample interval in 1/30000 s (`PERIOD_CLOCK_HZ`); rate = 30000 / period |
| 290 | u32 | `time_resolution`: timestamp ticks per second (30000, or 10⁹ for PTP, `PTP_RESOLUTION`) |
| 294 | u16[8] | Windows SYSTEMTIME, UTC (`systemtime`) → `recorded_at` |
| 310 | u32 | channel count (≤ `MAX_CHANNELS`) |

### Channel headers (`CC`, `CC_LEN` = 66 each, → `NsxChannel`)

| offset | type | our name |
| --- | --- | --- |
| 0 | char[2] | `CC` (else `bad_channel_header`) |
| 2 | u16 | `electrode_id` |
| 4 | char[16] | `label` (channel `name`; `elec<id>` when blank) |
| 20, 21 | u8, u8 | `connector`, `pin` |
| 22, 24 | i16, i16 | `min_digital`, `max_digital` |
| 26, 28 | i16, i16 | `min_analog`, `max_analog` |
| 30 | char[16] | `units` (`uV`, `mV`, …) |
| 46, 50, 54 | u32, u32, u16 | `highpass`: corner (mHz), order, type (0 none, 1 Butterworth) |
| 56, 60, 64 | u32, u32, u16 | `lowpass` |

Scaling (`cc_scaling`): `scale = (max_analog − min_analog) / (max_digital − min_digital)`, `offset = min_analog − min_digital × scale`, value in `units`. Equal digital limits → raw counts and `bad_scaling`.

### Data packets (`NsxPacket`: `offset`, `data_offset`, `timestamp`, `points`)

`0x01`, timestamp (u32 in 2.2/2.3, u64 in 3.0), u32 number of points, then `points` × channels int16, point-major (all channels of a point together). Packets follow each other to the end of the file. A header byte other than 0x01 → `bad_packet`; a packet that declares more points than the file holds → `truncated` (the whole points present stay readable). Each packet is a sweep (`NsxSweep`: `first_packet`, `packet_count`, `sample_count`, `timestamp`); `extra.sweep_starts_s` = timestamp / resolution, `extra.sweep_start_timestamps` the raw ticks; `check` reports `segments` and, when a packet starts before the previous one ended, `clock_reset`.

**PTP (spec 3.0, resolution 10⁹).** When the first packet holds exactly one point, every packet does: the file is a sequence of `13 + 2 × channels`-byte packets (`ptp_stride`, `ptp_packet_count`). Sweeps are gap-free runs: a gap is a timestamp step that departs from the period (period / 30000 s in ns) by more than half a sample. `check` reads every timestamp, and so does `info` when the packets take at most 64 MiB (`PTP_FULL_SCAN_BYTES`). In a larger file `info` probes 64 evenly spaced packets (`PTP_PROBES`): when each sits on the expected time it takes the file as one sweep and reports that as an assumption (`ptp_sweeps_assumed`, trace `extra.ptp_timestamps_read` = `probed`; otherwise `all`), and when one does not it reads every timestamp. The first and last timestamps alone can agree across gaps that cancel out.

### Spec 2.1 (`NEURALSG`, `SPEC21_HEADER_LEN` = 32)

| offset | type | use |
| --- | --- | --- |
| 8 | char[16] | `label` |
| 24 | u32 | `period` |
| 28 | u32 | channel count |
| 32 | u32 × channels | electrode ids |

Samples follow directly (no packet header, no timestamp): one sweep of `(size − header) / (2 × channels)` points; bytes of a partial point → `partial_sample`. The file carries no analog range. When a `.nev` with the same name sits next to it, each channel's `NEUEVWAV` digitization factor (nV per count) gives `scale` = factor / 1000 µV (`spec21_factor` replaces 21516 with 152592.547 nV, the overflow Neo documents for old Cerebus systems) and `scaling_source` names the NEV; otherwise values are raw counts (`no_scaling`). Channel names are `chan<id>` below 129 and `ainp<id − 128>` from 129 (`spec21_label`, Neo's convention).

`NsxFile`: `spec`, `version`, `label`, `comment`, `period`, `time_resolution`, `recorded_at`, `header_len`, `channels`, `packets`, `sweeps`, `ptp_stride`, `ptp_packet_count`, `ptp_sweeps_assumed`, `scaling_source`, `file_len`, `findings`; `sample_rate_hz()`, `frame_len()`, `sample_offset()`, `max_sweep_len()`; built by `parse_nsx`.

## NEV (`NevFile`, `parse_nev`)

### Basic header (`NEV_BASIC_LEN` = 336) and extended headers (`NEV_EXT_LEN` = 32)

| offset | type | our name |
| --- | --- | --- |
| 0 | char[8] | `signature` |
| 8, 9 | u8, u8 | `version` (≥ 3 → 64-bit timestamps, `wide_timestamps()`) |
| 10 | u16 | `flags` (bit 0: every waveform sample is 16-bit) |
| 12 | u32 | `header_len` (= 336 + 32 × extended headers) |
| 16 | u32 | `packet_len` (bytes per data packet) |
| 20 | u32 | `time_resolution` (ticks per second) |
| 24 | u32 | `sample_resolution` (waveform samples per second) |
| 28 | u16[8] | SYSTEMTIME, UTC → `recorded_at` |
| 44 | char[32] | `application` |
| 76 | char[256] | `comment` |
| 332 | u32 | number of extended headers |

Extended headers (`ExtHeader`: `id`, `offset`): `NEUEVWAV` → `WaveformHeader` (8 u16 `electrode_id`, 10 u8 `connector`, 11 u8 `pin`, 12 u16 `digitization_nv`, 14 u16 `energy_threshold`, 16 i16 `high_threshold_uv`, 18 i16 `low_threshold_uv`, 20 u8 `sorted_units`, 21 u8 `sample_bytes` (0/1 → 1), 22 u16 `spike_width` from spec 2.3); `NEUEVLBL` → `labels` (8 u16 id, 10 char[16]); `DIGLABEL` → `digital_labels` (8 char[16] label, 24 u8 mode: 0 serial, 1 parallel). Other ids are listed by `info --view structure` only.

### Data packets (`NevPacket`, `nev_packet`)

Timestamp (u32, or u64 from spec 3.0), u16 packet id, then (`payload_offset()` = 6 or 10):

| packet id | `kind` column | payload |
| --- | --- | --- |
| 0 | 0 (`KIND_DIGITAL`) | u8 insertion reason (`code`), u8 reserved, u16 digital input (`digital`) |
| 1 – 32767 (`MAX_ELECTRODE_ID`) | 1 (`KIND_SPIKE`) | u8 unit class (`code`: 0 unsorted, 255 noise), u8 reserved, waveform from `waveform_offset()` (8 or 12): `waveform_samples()` samples of `sample_bytes()` bytes, signed |
| 0xFFFF (`COMMENT_PACKET`) | 2 (`KIND_COMMENT`) | u8 character set (0 ANSI, 1 UTF-16), u8 flag, u32 data, text (in `info --view full`, not in the table) |
| anything else | 3 (`KIND_OTHER`) | not decoded (video sync, tracking, button, configuration, log, recording events; 0x8001 in the 3.0 corpus file is undocumented) |

Table columns: `time_s` (timestamp / resolution), `packet_id`, `kind`, `code`, `digital`, `w0`… (waveform in µV = raw × `uv_per_count()` = digitization factor / 1000; NaN outside spikes or past a shorter waveform). Rows = `packet_count()` = (file size − header) / packet size; a partial trailing packet → `partial_packet`. Table `extra`: `signature`, `file_version`, `timestamp_resolution_hz`, `waveform_rate_hz`, `packet_size`, `recorded_at`, `application`, `comment`, `electrodes[]`, `digital_labels[]`, `kinds`, `code`. `info --view full` adds up to `MAX_COMMENTS` comments with times and a count per packet id.

## Recording directories (sessions)

A directory of `.ns1`–`.ns6` and `.nev` files (plus `SESSION_COMPANIONS`: `.ccf` configuration, `.txt`, `.log`, `.xml`, `.json`, `.md`) is one recording (`session_files`, `open_session`): each NSx file is a trace with its own rate and clock (sampling groups are never merged), each NEV a table, in file-name order. A spec 2.1 NSx file takes its scaling from the NEV of the same name, exactly as when opened alone. NEV rows are not assigned to NSx sweeps. Nothing is resampled or realigned; every trace, channel and table names its file in `extra.source_file` / `extra.source_files`, `info --view structure` lists the member files and `check` checks each one (findings prefixed with the file). The composition itself is `openreadout_core::session` (`SessionDataset`).

## `check` finding codes

`truncated`, `bad_packet` (errors); `header_length_mismatch`, `bad_channel_header`, `bad_scaling`, `no_scaling`, `partial_scaling`, `partial_sample`, `partial_packet`, `clock_reset`, `timestamps_decrease`, `no_data`, `no_packets` (warnings); `segments`, `no_units`, `unknown_packets` (info). A file whose basic header is cut off fails to open as corrupt (exit 4).

## Observed corpus values

| file | spec | channels | rate | sweeps | notes |
| --- | --- | --- | --- | --- | --- |
| `brk-2-1-l101210-001.ns2` | 2.1 | 6 (analog inputs 137–143) | 1 kHz | 1 | 3641 samples; factor 21516 in the NEV |
| `brk-test2-test.ns5` | 2.1 | 2 | 30 kHz | 1 | 6 samples, no NEV |
| `brk-pause-correct.ns2` | 2.3 | 16 (mV) | 1 kHz | 2 | packets at ticks 0 and 930261 |
| `brk-reset.ns2` | 2.3 | 16 (mV) | 1 kHz | 2 | second packet starts at tick 96 (clock reset) |
| `brk-filespec2-3001.ns5` | 2.3 | 10 (uV) | 30 kHz | 1 | 900300 samples |
| `brk-file-spec-3-0.ns6` | 3.0 | 8 | 30 kHz | 1 | first packet at tick 26810899 |
| `brk-ptp-20231027-125608-001.ns2` | 3.0 PTP | 65 | 1 kHz | 1 | 2149 one-sample packets, ±240 ns jitter |

## Vocabulary (every public identifier in `openreadout-blackrock` must appear here)

| identifier | meaning |
| --- | --- |
| `session_files`, `open_session`, `SESSION_COMPANIONS` | recording directories (sessions) |
| `BlackrockReader`, `NsxDataset`, `NevDataset`, `FORMAT_ID`, `EXTENSIONS`, `SIGNATURES`, `open`, `nsx`, `nev`, `MAX_TABLE_READ`, `MAX_COMMENTS` | entry points and limits |
| `NsxSpec` { `V21`, `V22`, `V30` }, `name`, `packet_header_len` | NSx spec generations |
| `NsxFile`, `spec`, `version`, `label`, `comment`, `period`, `time_resolution`, `recorded_at`, `header_len`, `channels`, `packets`, `sweeps`, `ptp_stride`, `ptp_packet_count`, `ptp_sweeps_assumed`, `scaling_source`, `file_len`, `findings`, `sample_rate_hz`, `frame_len`, `sample_offset`, `max_sweep_len`, `parse_nsx` | NSx file |
| `NsxChannel`, `index`, `electrode_id`, `connector`, `pin`, `min_digital`, `max_digital`, `min_analog`, `max_analog`, `units`, `highpass`, `lowpass`, `scale`, `offset` | NSx channels |
| `NsxPacket`, `data_offset`, `timestamp`, `points`, `NsxSweep`, `first_packet`, `packet_count`, `sample_count` | NSx packets and sweeps |
| `BASIC_HEADER_LEN`, `CC_LEN`, `SPEC21_HEADER_LEN`, `PERIOD_CLOCK_HZ`, `PTP_RESOLUTION`, `PTP_FULL_SCAN_BYTES`, `PTP_PROBES`, `MAX_CHANNELS`, `cc_scaling`, `spec21_label`, `spec21_factor`, `systemtime` | NSx constants and helpers |
| `NevFile`, `signature`, `flags`, `packet_len`, `sample_resolution`, `application`, `ext_headers`, `waveforms`, `labels`, `digital_labels`, `wide_timestamps`, `payload_offset`, `waveform_offset`, `sample_bytes`, `waveform_samples`, `max_waveform_samples`, `uv_per_count`, `parse_nev`, `NEV_BASIC_LEN`, `NEV_EXT_LEN` | NEV file |
| `ExtHeader`, `id`, `WaveformHeader`, `digitization_nv`, `energy_threshold`, `high_threshold_uv`, `low_threshold_uv`, `sorted_units`, `spike_width` | NEV extended headers |
| `NevPacket`, `packet_id`, `code`, `digital`, `waveform`, `text`, `nev_packet`, `COMMENT_PACKET`, `MAX_ELECTRODE_ID`, `KIND_DIGITAL`, `KIND_SPIKE`, `KIND_COMMENT`, `KIND_OTHER` | NEV packets and table kinds |

### Byte-source integration

The existing layouts are read through `Input`/`Fs`/`SourceFile`, including directory
sessions and companions where applicable. Path, memory and callback namespaces use
the same parser and scaling. See [byte sources](../../book/src/guides/wasm.md).
