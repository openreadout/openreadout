# CED Spike2 `.smr` and `.smrx`

CED's Spike2 software records waveforms, events and spike shapes in `.smr` files (32-bit) and `.smrx` files (64-bit). OpenReadout returns every waveform channel as a trace, with pauses as separate sweeps, and the event, marker and spike channels as tables.

The `.smr` layout comes from Neo's `Spike2RawIO` (BSD-3), read as documentation, and is checked against public corpus files with Neo as a reference reader. The `.smrx` layout was derived from public files alone (no documentation, library or reader of the format exists that the clean-room rules allow) and checked against the Spike2 software's own text and MATLAB exports of the same recordings (section *64-bit `.smrx`* below). See `docs/provenance/ced-spike2.md`.

Crate: `openreadout-ephys`, format id `ced-spike2` (`SPIKE2_FORMAT_ID`), extensions `smr`, `smrx`,
reader `Spike2Reader`, dataset `Spike2Dataset` (`open`, `open_input`, `smr`).

## Mapping

- **Every waveform channel (Adc int16, RealWave float32) is a trace** (`SmrTrace`: `channel`,
  `sweeps`) with one channel. A block that starts more than one sample interval after the previous
  block's last sample starts a new sweep (a pause). Trace `name` = channel title (`ch<N>` when
  empty); `start_s` = first block's first time; `extra`: `channel_number` (1-based, as Spike2
  numbers channels), `kind`, `physical_channel`, `comment`, `ideal_rate_hz`,
  `sample_interval_ticks`, `tick_s`, `sweep_starts_s` and `sweep_sample_counts` (with pauses); on
  trace 0 `recorded_at`, `application` (`Spike2`), `application_version` (the header's creator
  code, e.g. `S2071431`), `file_version`, `file_comments`. Channels without samples are not listed.
- Values: Adc samples × `scale` / 6553.6 + `offset` (`gain`), RealWave as stored, in the channel's
  unit.
- **Table `events`** (plain events, markers, text marks; one row per item, channel order, then
  file order): `time_s`, `tick`, `channel`, `code0`…`code3` (the four marker bytes; NaN for plain
  events), `text` (index into `extra.texts`, the distinct text-mark texts in order of first
  appearance; NaN otherwise). `extra.channels[]`: `channel_number`, `kind`, `title`, `comment`,
  `physical_channel`, `ideal_rate_hz`, `unit`, `items`, `blocks`, and `initially_low` for level
  events.
- **Table `spikes`** (AdcMark, RealMark): `time_s`, `tick`, `channel`, `unit` (first marker byte),
  `code1`…`code3`, `w0`… (the waveform, scaled like Adc samples or as stored; NaN past a shorter
  waveform). `extra.channels[]` adds `waveform_points`, `pre_trigger_points`, `interleave` (traces
  interleaved in each waveform, returned in stored order), `waveform_rate_hz`, `scale`, `offset`.

## Detection

`(C) CED 87` at byte 2 and a file version 1–9 at byte 0 (`looks_like_smr`, `SMR_COPYRIGHT`) →
definite; `S64` at byte 0 (`looks_like_smrx`) → definite, read by `parse_smrx`; a `.smr` / `.smrx`
name without either → extension only.

## File header (`SMR_HEADER_LEN` = 512, `parse_smr` → `SmrFile`)

| offset | type | our name |
| --- | --- | --- |
| 0 | i16 | `system_id` (file version) |
| 2 | char[10] | `(C) CED 87` |
| 12 | char[8] | `creator` |
| 20, 22 | i16, i16 | `us_per_time`, `time_per_adc` |
| 26 | i32 | first data block |
| 30 | i16 | channel headers (`channel_slots`, ≤ `MAX_SMR_CHANNELS`) |
| 40 | i32 | `max_time` (ticks) |
| 44 | f64 | `time_base_s` (from version 6; 1 µs before) |
| 52–57 | u8 × 6 | hundredths, second, minute, hour, day, month → `recorded_at` (Inferred; only when valid) |
| 58 | u16 | year |
| 107 | 5 × Pascal char[80] | `comments` (kept when printable) |

`tick_s` = `us_per_time` × `time_base_s`. From version 9, block offsets count 512-byte units.

## Channel headers (`SMR_CHANNEL_LEN` = 140 at 512 + 140 × index → `SmrChannel`)

| offset | type | our name |
| --- | --- | --- |
| 6, 10 | i32, i32 | first and last block (−1 when empty) |
| 14 | u16 | `header_blocks` |
| 16, 18 | u16, i16 | `extra_bytes`, `pre_trigger` |
| 26 | Pascal char[72] | `comment` |
| 102 | i32 | divider (sample interval in ticks, from version 6) |
| 106 | i16 | `physical` |
| 108 | Pascal char[10] | `title` |
| 118 | f32 | `ideal_rate` |
| 122 | u8 | `kind` (`ChannelKind`, `from_code`, `name`) |
| 124, 128 | f32, f32 | `scale`, `offset` (Adc, AdcMark); level channels: 124 = initial level (`initially_low`) |
| 132 | Pascal char[6] | `unit` |
| 138 | i16 | before version 6 the ADC divide (interval = divide × `time_per_adc` ticks); from 6 the AdcMark `interleave` |

`number` = index + 1. `interval_ticks` (waveforms and AdcMark). `ChannelKind`: 1 `Adc`, 2
`EventFall`, 3 `EventRise`, 4 `EventBoth`, 5 `Marker`, 6 `AdcMark`, 7 `RealMark`, 8 `TextMark`, 9
`RealWave` (`is_waveform`, `is_spikes`, `is_events`). Item sizes (`item_len`): 2 (Adc), 4
(RealWave, events), 8 (markers), 8 + `extra_bytes` (AdcMark, RealMark, TextMark);
`wave_points`, `item_count`.

## Blocks (`SMR_BLOCK_HEADER_LEN` = 20 → `SmrBlock`)

| offset | type | meaning |
| --- | --- | --- |
| 0 | i32 | previous block (−1 first) |
| 4 | i32 | next block (−1 last) |
| 8, 12 | i32, i32 | `start`, `end`: times of the first and last item (ticks) |
| 16 | i16 | channel (index + 1) |
| 18 | u16 | `items` |

`offset` = first item. Chains are followed from the first block to −1 (at most `MAX_SMR_BLOCKS`);
`segments` groups contiguous waveform blocks into sweeps. Text-mark texts (`item_text`, NUL-
terminated) are collected at open (`texts`, at most `MAX_TEXT_MARKS` items read). `findings` holds
chain problems. Reads are capped at `MAX_SMR_READ` samples and `MAX_SMR_TABLE_READ` rows.

## 64-bit `.smrx` (`file64.rs`, `parse_smrx` → the same `SmrFile`)

Inferred from the files (Source `Inferred`); the offsets below are all little-endian.

**Header stream** (`header_stream`, `HeaderStream`): the first 64 KiB (`SMRX_BLOCK_LEN`) of the
file, then each extra header block after its 16-byte block header. Offsets in the file header and
channel records count in this stream (a file with 399 channel slots has its string table 16 bytes
further on in the file than its stated offset).

| offset | type | meaning |
| --- | --- | --- |
| 0 | char[6] | `S64pl\0` |
| 6, 7 | u8, u8 | version bytes (`son64`: `1.1`, `0.1`; `format_version` `smrx 1.1`) |
| 16 | char[8] | creator (`S2091349`; empty in imported files) |
| 24–31 | u8 × 6, u16 | hundredths, second, minute, hour, day, month, year → `recorded_at` |
| 32 | f64 | seconds per tick (`tick_s`; 1 µs, 2 µs, 3.7 µs in the corpus) |
| 44 | u32 | channel table |
| 52 | u32 | string table |
| 56, 60 | u32, u32 | channel slots, bytes per channel record (272) |
| 64–83 | u32 × 5 | file comments (string indices, 0 = none) |
| 996 | u32 | extra header blocks |
| 1000, 1016 | u64, i64 | file length, largest time (`max_time`) |
| 1024 | u64 × n | offsets of the extra header blocks (`MAX_SMRX_HEADER_BLOCKS`) |

**String table** (`string_table`): u32 bytes, u32 count (`MAX_SMRX_STRINGS`), then per string a
u32 reference count and NUL-terminated Latin-1 text padded to 4 bytes; referred to 1-based.

**Channel record** (272 bytes; a record whose kind byte is 0 is a deleted channel and is skipped):

| offset | type | meaning |
| --- | --- | --- |
| 0x00 | u64 | root index block (0: no data) |
| 0x08 | i64 | last time |
| 0x10 | u64 | live data blocks (`header_blocks`) |
| 0x18 | u64 | blocks left by a deleted channel of this slot |
| 0x20 | u32 | item bytes: 2 Adc, 4 RealWave, 8 events, 16 markers, 16 + data for AdcMark/RealMark/TextMark (`extra_bytes` = item − 16) |
| 0x24, 0x26, 0x28 | u16 × 3 | marker points, traces (`interleave`), pre-trigger points |
| 0x2c | u16 | generation (increments when the slot is re-created) |
| 0x2e, 0x2f | u8, u8 | kind (`ChannelKind` codes as in `.smr`), the kind before deletion |
| 0x30 | i32 | physical port (−1: none) |
| 0x34, 0x38, 0x3c | u32 × 3 | title, unit, comment (string indices) |
| 0x40 | u64 | sample interval in ticks (`interval_ticks`) |
| 0x48, 0x50, 0x58 | f64 × 3 | ideal rate, scale, offset (values = int16 × scale / 6553.6 + offset, as in `.smr`) |

**Blocks.** Index blocks are 4 KiB (`SMRX_INDEX_LEN`), data blocks 64 KiB. Every block starts
with a 16-byte header (`SMRX_BLOCK_HEADER_LEN`): u64 whose high bits are the parent block's
offset and whose low 12 bits are the level (bits 8–11, `block_level`: 0 data, 1 index of data
blocks, 2 index of index blocks) and the position in the parent; u16 channel (0-based); u16
generation; u32 count. Index entries follow: (u64 first time, u64 block offset), at most 255
(`SMRX_INDEX_ENTRIES`). The tree is followed from the root, at most `MAX_SMRX_LEVELS` deep, until
the record's live data blocks are found; data blocks whose channel or generation differ from the
record's are left-overs of a deleted channel and skipped. Waveform data blocks hold `count` runs
of (u64 first time, u64 samples, the samples); each run is one `SmrBlock` (a pause between runs
or blocks splits sweeps as in `.smr`). Event blocks hold `count` i64 times; marker items are an
i64 time, the four marker bytes, 4 bytes, then the item's data (`time_len`, `head_len`: 8 and 16
here, 4 and 8 in `.smr`; `wide_times` marks `.smrx` channels). Event-table ticks are `int64`.

**Validated** (tests `tests/corpus/` → `spike2x_oracle`, oracle `oracle/spike2x_oracle.py` from the
vendor's exports, never from the `.smrx`): Adc channels (every int16 code of 9 channels equal to
the MATLAB export, incl. 10.5 million samples under a two-level index; 21 channels within the text
exports' 5 decimals), rising-event channels (every time), channel titles, units, comments, starts,
intervals, scales and offsets, deleted and re-created channel slots, a channel table spanning two
header blocks, empty marker and event channels. **Not validated** (no public file): RealWave,
falling and level events, markers with items, AdcMark/RealMark/TextMark, multi-run data blocks,
three-level indexes, the level of a level-event channel before its first edge (not reported).

## `check` finding codes

`truncated`, `bad_block_link`, `bad_interval`, `bad_item_size` (errors); `block_count_mismatch`,
`last_block_mismatch`, `overlapping_blocks`, `marker_size_mismatch` (warnings); `pauses` (info).

## Observed corpus values

| file | version | traces | events | spikes |
| --- | --- | --- | --- | --- |
| `spike2-file-spike2-1` | 4 | Respi 200 Hz | 5711 (level, falling, keyboard) | 247 (AdcMark) |
| `spike2-file-spike2-2` | 5 | — | 1 (text mark) | 646 (AdcMark, 100 points) |
| `spike2-file-spike2-3` | 3 | Song 25 kHz | — | — |
| `spike2-multi-sampling` | 5 | 7 at 1–10 kHz, 9 pauses each | 80 | 112 |
| `spike2-130322-1ly` | 5 | 2 at 20.8 kHz | text marks | — |
| `spike2-two-mice-bigfile-test000` | 9 (512-byte block units) | 6 at 10 kHz | 30 | — |
| `zenodo4985334-aa-mvc` | 3 | 14 (EMG 2 kHz, force 200 Hz) | — | — |
| `zenodo15783208-cpp600-c7e-da` | 7 | ECG, spikes 20 kHz, LFP | stimulus | — |
| `zenodo20750122-sub3-transpai` | 7 | LFP3 4.96 kHz | — | — |
| `zenodo4437568-fig1-rec-f-control` | 6 | 6 LFP 10 kHz | — | — |
| `zenodo10624872-21022013` | 6 | IL 1 kHz | 966 rising edges | — |
| `figshare26177569-sssort-singlea-smrx`, `-singleb-smrx`, `-doubleab-smrx` | smrx 1.1 | 7 Adc at 10 kHz | 2 rising-event channels | — |
| `figshare25112837-fig2-aud-smrx` | smrx 0.1 | 12 Adc at 5–33.3 kHz, 315 s | 15,764 frame triggers; empty keyboard markers | — |
| `spike2-m365-1sec-smrx` | smrx 1.1 | 16 Adc at 30 kHz | empty keyboard markers | — |
| `figshare26177569-sssort-singleb-asym03-smr` | 7 | channel 5 of `SSSort_singleB.smrx` | — | — |

## Vocabulary (every public identifier in the Spike2 modules of `openreadout-ephys` must appear here)

| identifier | meaning |
| --- | --- |
| `Spike2Reader`, `SPIKE2_FORMAT_ID`, `Spike2Dataset`, `open`, `open_input`, `smr` | entry points |
| `SmrTrace`, `channel`, `sweeps` | one waveform trace |
| `SmrFile`, `parse_smr`, `system_id`, `creator`, `us_per_time`, `time_per_adc`, `time_base_s`, `tick_s`, `max_time`, `recorded_at`, `comments`, `channels`, `channel_slots`, `file_len`, `findings`, `texts`, `son64` | the file |
| `parse_smrx`, `header_stream`, `HeaderStream`, `bytes`, `string_table`, `block_level`, `SMRX_BLOCK_LEN`, `SMRX_INDEX_LEN`, `SMRX_BLOCK_HEADER_LEN`, `SMRX_INDEX_ENTRIES`, `MAX_SMRX_HEADER_BLOCKS`, `MAX_SMRX_STRINGS`, `MAX_SMRX_LEVELS` | the 64-bit `.smrx` layout |
| `SmrChannel`, `number`, `kind`, `title`, `comment`, `physical`, `ideal_rate`, `extra_bytes`, `pre_trigger`, `scale`, `offset`, `unit`, `interleave`, `interval_ticks`, `initially_low`, `blocks`, `header_blocks`, `item_len`, `item_count`, `wave_points`, `gain`, `wide_times`, `time_len`, `head_len` | a channel |
| `SmrBlock`, `start`, `end`, `items` | a data block |
| `ChannelKind`, `Adc`, `EventFall`, `EventRise`, `EventBoth`, `Marker`, `AdcMark`, `RealMark`, `TextMark`, `RealWave`, `from_code`, `name`, `is_waveform`, `is_spikes`, `is_events` | channel kinds |
| `SMR_HEADER_LEN`, `SMR_CHANNEL_LEN`, `SMR_BLOCK_HEADER_LEN`, `SMR_COPYRIGHT`, `MAX_SMR_CHANNELS`, `MAX_SMR_BLOCKS`, `MAX_TEXT_MARKS`, `MAX_SMR_READ`, `MAX_SMR_TABLE_READ` | constants and limits |
| `looks_like_smr`, `looks_like_smrx`, `segments`, `item_text` | helpers |
