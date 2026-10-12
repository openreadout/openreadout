# Axona DacqUSB (`.set` + `.bin`, tetrode `.1` … `.32`)

Axona's DacqUSB records a trial as files that share a stem: the `.set` file with the recording
setup, the raw `.bin` file of every channel at 48 kHz, tetrode spike files `.1`, `.2` …, and
EEG (`.eeg`, `.egf`), position (`.pos`) and other files. OpenReadout reads the `.set`, the raw
`.bin` (one trace of the active tetrodes' channels) and the tetrode files (the `spikes` table).

Derived from Neo's `AxonaRawIO` (BSD-3, read as documentation and run as the reference reader) and
three public sessions. Provenance: `docs/provenance/axona-dacqusb.md`.

Crate: `openreadout-ephys`, format id `axona-dacqusb` (`AXONA_FORMAT_ID`), extension `set`, reader
`AxonaReader`, dataset `AxonaDataset` (`open`, `open_input`, `session`).

## Detection and input

A `.set` starting with `trial_date ` (`looks_like_axona`) is definite, as is a tetrode, `.eeg`,
`.egf` or `.pos` file with that header; a `.bin` whose first two 432-byte packets start with `ADU`
is likely. Any of them opens the session (`parse_session` → `AxonaSession`); without the `.set`
next to it, opening fails with exit 6.

## Layout

- **Headers** (`parse_header`): `key value` lines (CR LF), Latin-1, ending at `data_start` in
  the data files. The `.set` keys used: `rawRate` (`raw_rate_hz`), `ADC_fullscale_mv`,
  `gain_ch_<i>` (`raw_scale`), `collectMask_<t>` (1 = tetrode *t* recorded; `active_tetrodes`,
  tetrodes 1–16), `trial_date` + `trial_time` (`recorded_at`, local time), `sw_version`
  (`format_version`), `duration`. `set` keeps every entry in file order (`get`).
- **`.bin`** (`bin`, `bin_len`, `packets`): packets of `BIN_PACKET` = 432 bytes: `BIN_PACKET_HEADER`
  = 32 bytes (`ADU1`/`ADU2`, a uint32 packet counter, …), then `BIN_SAMPLES_PER_PACKET` = 3
  samples of `BIN_CHANNELS` = 64 int16 values, then 16 bytes. Channel *i* (0-based; tetrode *t*
  has 4*t*−4 … 4*t*−1) is value `bin_channel_slot(i)` of each sample: blocks of eight channels
  alternate between values 32–63 and 0–31 (0–7 → 32–39, 8–15 → 0–7, 16–23 → 40–47, …).
- **Tetrode files** (`tetrode_files`: `TetrodeFile` with `tetrode`, `path`, `header`,
  `data_offset`, `spikes` (`num_spikes`), `complete_spikes`, `timebase_hz` (`timebase`),
  `samples_per_spike`, `channels` (`num_chans`); `record_bytes`): after `data_start`, one record
  per spike of four groups (one per electrode) of a big-endian int32 time stamp and
  `samples_per_spike` int8 samples, then `\r\ndata_end\r\n`. Only 4 channels, 1-byte samples and
  4-byte time stamps are read (`tetrode_layout` otherwise). Tetrode files are looked for from
  `.1` up to `.MAX_TETRODES` (32) until one is missing.
- Other files of the session (`other_files`) are listed in the notes, not read.

## Mapping

- **Trace `raw`** (when there is a `.bin`, a `rawRate` and an active tetrode): one channel per
  electrode of the active tetrodes (`raw_channels`), named `<tetrode><a–d>` (`1a` … `4d`),
  unit µV, `scale` = 1000 × `ADC_fullscale_mv` / (`gain_ch_<i>` × 32768), `sample_count` =
  packets × 3, one sweep, `start_s` 0. Channel `extra`: `channel_number`, `tetrode`, `gain`.
  Trace `extra`: `recorded_at`, `application` (`DacqUSB`), `application_version`,
  `trial_duration_s` (the `.set` `duration`, which can differ from the `.bin` length),
  `active_tetrodes`. Reads are capped at `MAX_AXONA_READ` samples per channel.
- **Table `spikes`** (when there are tetrode files): `time_s` (time stamp / `timebase`),
  `timestamp`, `tetrode`, then `ch1_w0` … `ch4_w49` (raw int8 counts, NaN beyond a shorter
  spike). Rows are tetrode by tetrode, in file order. `extra`: `tetrodes` (number, spikes,
  `timebase_hz`), `recorded_at`.
- `.set` headers are capped at `MAX_HEADER_BYTES`.

## `check` finding codes

`truncated`, `no_data_start`, `tetrode_layout`, `bad_packets`, `no_raw_rate`, `bad_gain`
(errors); `partial_packet`, `packet_gaps` (warnings).

## Oracle mapping

`oracle/gen.py` `axona()`: Neo's `AxonaRawIO` on the `.set`. The raw trace is Neo's int16 chunk ×
Neo's gain; spikes are Neo's units (one per tetrode file) as sorted rows of time, raw time stamp,
tetrode and the four waveforms.

## Observed corpus values

| id | raw `.bin` | tetrode files | not read |
| --- | --- | --- | --- |
| `gin-axona-raw` | 192 packets (576 samples), tetrodes 1–4 | – | – |
| `gin-axona-multi-modal` | 19,200 packets (1.2 s), tetrodes 1–4 | `.1`–`.4`, 440 spikes | `.eeg`–`.eeg4`, `.egf`–`.egf4`, `.pos` |
| `gin-axona-unit-spikes` | – | `.1`–`.4`, 50,448 spikes | `.eeg`, `.pos` |

## Known gaps

- `.eeg`, `.egf` and `.pos` files are not read.
- Tetrode waveforms are not scaled to µV.

## Vocabulary (every public identifier in `crates/openreadout-ephys/src/axona/` must appear here)

| identifier | meaning |
| --- | --- |
| `AxonaReader`, `AXONA_FORMAT_ID`, `AxonaDataset`, `open`, `open_input`, `session` | entry points |
| `AxonaSession`, `parse_session`, `dir`, `stem`, `set`, `bin`, `bin_len`, `active_tetrodes`, `tetrode_files`, `other_files`, `findings`, `get`, `raw_rate_hz`, `packets`, `raw_channels`, `raw_scale`, `recorded_at` | the session |
| `TetrodeFile`, `tetrode`, `path`, `header`, `data_offset`, `spikes`, `complete_spikes`, `timebase_hz`, `samples_per_spike`, `channels`, `record_bytes` | a tetrode file |
| `BIN_PACKET`, `BIN_PACKET_HEADER`, `BIN_SAMPLES_PER_PACKET`, `BIN_CHANNELS`, `bin_channel_slot` | the raw `.bin` |
| `MAX_TETRODES`, `MAX_HEADER_BYTES`, `MAX_AXONA_READ`, `parse_header`, `looks_like_axona` | limits, headers, detection |
