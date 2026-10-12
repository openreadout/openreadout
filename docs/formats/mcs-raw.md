# Multi Channel Systems MC_DataTool binary export (`.raw`)

MC_DataTool, Multi Channel Systems' converter, exports MC_Rack recordings to binary `.raw` files.
With the header option the file starts with a text header that gives the sampling rate, the
electrode gain and the stream names. OpenReadout reads these files as one trace with a channel per
stream. Exports without the header, and MC_Rack `.mcd` recordings themselves, are not read.

Derived from Neo's `RawMCSRawIO` (BSD-3, read as documentation and run as the reference reader)
and one public file. Provenance: `docs/provenance/mcs-raw.md`.

Crate: `openreadout-ephys`, format id `mcs-raw` (`MCS_RAW_FORMAT_ID`), extension `raw`, reader
`McsRawReader`, dataset `McsRawDataset` (`open`, `open_input`, `header`, `file_len`, `frames`).
Detection: the file starts with `MC_DataTool binary conversion` (`MCS_RAW_MAGIC`,
`looks_like_mcs_raw`) → definite. Other `.raw` files (Thermo, Waters) are left to their readers.

## Layout (`parse_header` → `McsRawHeader`)

- **Header** (Windows-1252, CR LF lines, searched for `EOH` in the first `MAX_MCS_HEADER` bytes):
  `MC_DataTool binary conversion`, `Version <v>` (`version`), then `key = value` lines
  (`entries`): `MC_REC file` (`source_file`), `Sample rate` (`sample_rate_hz`), `ADC zero`
  (`adc_zero`), `El = <gain><unit>/AD` (`gain`, `unit`, e.g. `0.2083µV/AD`) and `Streams =
  El_21;El_31;…` (`streams`, at most `MAX_MCS_STREAMS`). The samples start after `EOH\r\n`
  (`data_offset`).
- **Samples**: frames of one uint16 per stream, little-endian, back to back; `frames` = whole
  frames in the file.
- **Values**: raw × `gain` + offset, offset = −`gain` × `adc_zero` (`scale` and `offset` of every
  channel), in the `El` unit.
- `unreadable`: a file is refused (exit 6) when it lacks a usable rate, ADC zero or gain, or has a
  stream not named `El_…` (only the electrode gain is in the corpus file's header).

## Mapping

One trace `Signals`, one channel per stream (dtype `uint16`), one sweep, `start_s` 0;
`format_version` the MC_DataTool version; `extra`: `application` (`MC_DataTool`),
`application_version`, `source_file`, `adc_zero`. Reads are capped at `MAX_MCS_READ_VALUES`.

## `check` finding codes

`partial_frame`, `duplicate_stream` (warnings).

## Oracle mapping

`oracle/gen.py` `mcs_raw()`: Neo's `RawMCSRawIO`, its uint16 chunk × gain + offset per channel.

## Observed corpus values

| id | version | streams | frames | rate | gain |
| --- | --- | --- | --- | --- | --- |
| `gin-rawmcs-raw-mcs-with-header-1` | 2.6.10 | 60 (`El_12` … `El_87`) | 100,000 | 20 kHz | 0.2083 µV |

## Vocabulary (every public identifier in `crates/openreadout-ephys/src/mcsraw/` must appear here)

| identifier | meaning |
| --- | --- |
| `McsRawReader`, `MCS_RAW_FORMAT_ID`, `McsRawDataset`, `open`, `open_input`, `header`, `file_len`, `frames` | entry points |
| `McsRawHeader`, `parse_header`, `entries`, `version`, `source_file`, `sample_rate_hz`, `adc_zero`, `gain`, `unit`, `streams`, `data_offset`, `unreadable` | the header |
| `MCS_RAW_MAGIC`, `MAX_MCS_HEADER`, `MAX_MCS_STREAMS`, `MAX_MCS_READ_VALUES`, `looks_like_mcs_raw` | detection and limits |
