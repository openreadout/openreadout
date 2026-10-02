# WinWCP `.wcp`

WinWCP (Strathclyde Electrophysiology Software) records whole-cell patch-clamp sweeps to `.wcp` files. OpenReadout returns one trace with a channel per signal and a sweep per record, with each record's time, status and type.

Derived from Neo's `WinWcpRawIO` (BSD-3, read as documentation and run as a reference reader) and two public files (file versions 8 and 9, WinWCP 5.3.7). Provenance: `docs/provenance/winwcp.md`.

Crate: `openreadout-ephys`, format id `winwcp` (`WINWCP_FORMAT_ID`), extension `wcp`, reader
`WinWcpReader`, dataset `WinWcpDataset` (`open`, `open_input`, `wcp`). Detection: the file
starts with `VER=` and the header holds `NC=` and `NBD=` lines (`looks_like_wcp`) → definite; a
`.wcp` name alone → extension only.

## Layout (`parse_wcp` → `WcpFile`)

- **Text header** (`WCP_HEADER_LEN` = 1024 bytes): `KEY=value` lines separated by CR LF,
  decimal commas (`DT=6,005E-005`), kept verbatim in `header`. Used: `VER` (`version`), `NC`
  channels, `NR` records, `NBA` and `NBD` analysis and data sectors per record
  (`analysis_sectors`, `data_sectors`; `WCP_SECTOR` = 512 bytes), `ADCMAX` (`adc_max`), and per
  channel *c* `YN`*c* name, `YU`*c* unit, `YG`*c* gain in V per unit, `YO`*c* position in the
  sample frame, `YZ`*c* zero level (`WcpChannel`: `name`, `unit`, `gain_v_per_unit`,
  `offset_in_frame`, `zero_level`); `RTIME` (version 9) / `CTIME`, `VERPROG`, `ID`.
- **Records**: record *k* starts at 1024 + *k* × (`NBA` + `NBD`) × 512 (`record_offset`,
  `record_bytes`). Its analysis header (`WcpRecord`): 8 characters of status (`ACCEPTED`), 4 of
  type (`TEST`), then float32 group number, time recorded (s since the first record), sampling
  interval (s) and one full-scale voltage per channel (`status`, `record_type`, `group`,
  `time_s`, `interval_s`, `vmax`). Its data: `NBD` × 512 bytes of int16 frames of `NC` samples
  (`samples` = `NBD` × 512 / 2 / `NC` per channel).
- **Values** (`gain`): raw × `VMax`(record, channel) / `ADCMAX` / `YG`, in the channel's unit
  (Neo's formula; each record's own `VMax`, constant in both public files). `YZ` is reported
  (`extra.zero_level_adc`), not subtracted, as Neo does.
- **Sample rate**: 1 / the median of the records' float32 intervals (`interval_s`; Neo's rule).
- **Channel columns**: `YO` gives each channel's position in a frame when the offsets are a
  permutation of 0…`NC`−1; otherwise header order, with a `channel_offsets` warning.
- `recorded_at`: `RTIME` (else `CTIME`), `dd/mm/yyyy hh:mm:ss` → local time without zone.

## Mapping

One trace `record` with one channel per signal (dtype `int16`, `scale` = record 0's gain), one
sweep per record (`MAX_WCP_RECORDS`), `start_s` 0; `extra`: `file_version`, `application`
(`WinWCP`) and `application_version` (`VERPROG`), `recorded_at`, `comment` (`ID`),
`sweep_starts_s` (each record's time recorded), `record_status`, `record_type`, `record_group`;
channel `extra`: `zero_level_adc`, `gain_v_per_unit`, `vmax_v`. Reads are capped at
`MAX_WCP_READ` samples per channel; at most `MAX_WCP_CHANNELS` channels.

## `check` finding codes

`truncated`, `bad_gain` (errors); `channel_offsets`, `extra_bytes`, `bad_interval` (warnings);
`rejected_records` (info).

## Observed corpus values

| id | version | channels | records × samples | rate |
| --- | --- | --- | --- | --- |
| `winwcp-file-winwcp-1` | 8 | Im (pA), Vm (mV) | 16 × 49,920 | 16.65 kHz |
| `winwcp-file-winwcp-2` | 9 (5.3.7, RTIME 2019-05-21) | Vm0 (mV), Icom0 (pA) | 20 × 39,680 | 20.08 kHz |

## Vocabulary (every public identifier in `crates/openreadout-ephys/src/winwcp/` must appear here)

| identifier | meaning |
| --- | --- |
| `WinWcpReader`, `WINWCP_FORMAT_ID`, `WinWcpDataset`, `open`, `open_input`, `wcp` | entry points |
| `WcpFile`, `parse_wcp`, `header`, `version`, `adc_max`, `channels`, `records`, `analysis_sectors`, `data_sectors`, `samples`, `file_len`, `findings`, `record_bytes`, `record_offset`, `gain`, `interval_s`, `recorded_at` | the file |
| `WcpChannel`, `name`, `unit`, `gain_v_per_unit`, `offset_in_frame`, `zero_level` | a channel |
| `WcpRecord`, `status`, `record_type`, `group`, `time_s`, `vmax` | a record's analysis header |
| `WCP_HEADER_LEN`, `WCP_SECTOR`, `MAX_WCP_CHANNELS`, `MAX_WCP_RECORDS`, `MAX_WCP_READ`, `looks_like_wcp` | constants, limits, detection |
