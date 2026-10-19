# MaxWell Biosystems MaxOne / MaxTwo recordings (`.raw.h5`)

MaxLab Live, the software of MaxWell's MaxOne and MaxTwo high-density CMOS MEA systems, records
into HDF5 files named `*.raw.h5`. Files from 2019 on usually compress their samples with
MaxWell's own HDF5 filter (id 401, `mxw-data`). No open decoder of that filter exists: MaxWell
distributes it only as a compiled plugin, so those files are refused (exit 6) with a hint to
export the recording uncompressed. Uncompressed files of both layouts are read.

Derived from Neo's MaxwellRawIO (BSD-3), read as documentation, and the corpus files
(`docs/provenance/maxwell-h5.md`).

Crate: `openreadout-ephys`, format id `maxwell-h5` (`MAXWELL_FORMAT_ID`), extension `h5`, reader
`MaxwellReader`, dataset `MaxwellDataset` (`open`, `open_input`, `maxwell`). Detection: an HDF5
file whose first 64 KiB hold `Recorded with MaxLab`, or `mxw_version` and `wellplate` →
definite; a `.h5` file with `/version` and either `/sig` and `/mapping` (layout 20160704) or
`/data_store` and `/wells` → definite.

## Layout (`parse_maxwell` → `MaxwellFile`)

- `/version` names the layout (`version`), `/mxw_version` the MaxLab version (`mxw_version`),
  `/wellplate/version` the plate (`plate`).
- **Layout 20160704**: `/sig`, a uint16 matrix of amplifier channels (1028 rows on MaxOne) ×
  frames; `/mapping`, one record per routed channel (channel, electrode, x and y in µm); the
  samples of channel c are row c. `/settings/lsb` is volts per level (else 3.3 / (1024 × gain)).
  The file stores no frame rate: `MAXWELL_LEGACY_RATE_HZ` (20 kHz) is assumed. `/time` holds the
  start as local time.
- **Later layouts** (`20190530`): `/data_store/dataNNNN` per well and recording, with
  `groups/routed/raw` (routed rows × frames), `groups/routed/channels` (the channel of each row),
  `settings/mapping`, `settings/lsb`, `settings/sampling`, `well_id`, `recording_id` and
  `start_time` (Unix ms).
- `MaxwellRecording` (`name`, `matrix`, `channels`, `sample_rate_hz`, `rate_assumed`, `lsb_v`,
  `started_at`) is one trace, `MaxwellChannel` (`channel`, `electrode`, `x_um`, `y_um`, `row`)
  one channel. A matrix with `MAXWELL_COMPRESSION_FILTER` among its filters is refused.

## Mapping

- One trace per recording (`sig`, or `well<NNN> rec<NNNN>`), one sweep, channels in channel
  order named `ch<channel>`, unit µV, value = stored level × lsb × 10⁶ (the level of 0 V is not
  subtracted). Channel `extra`: `channel`, `electrode`, `x_um`, `y_um`.
- Trace `extra`: `layout_version`, `mxw_version`, `plate`, `recorded_at`, `sample_rate_assumed`,
  `lsb_v`.
- Reads are capped at `MAX_MAXWELL_READ` frames and `MAX_MAXWELL_READ_BYTES` per call.

## Oracle mapping

`oracle/gen.py` `maxwell()`: h5py reads the matrix rows; Neo's MaxwellRawIO must decode the same
matrix. Neo labels the rows of layout 20160704 with the mapping in its own order, so only the
matrix is compared with it.

## Vocabulary (every public identifier in `crates/openreadout-ephys/src/maxwell/` must appear here)

| identifier | meaning |
| --- | --- |
| `MaxwellReader`, `MAXWELL_FORMAT_ID`, `MaxwellDataset`, `open`, `open_input`, `maxwell` | entry points |
| `MaxwellFile`, `parse_maxwell`, `file_len`, `version`, `mxw_version`, `plate`, `recordings`, `findings` | the file |
| `MaxwellRecording`, `name`, `matrix`, `channels`, `sample_rate_hz`, `rate_assumed`, `lsb_v`, `started_at` | a recording |
| `MaxwellChannel`, `channel`, `electrode`, `x_um`, `y_um`, `row` | a channel |
| `MAXWELL_LEGACY_RATE_HZ`, `MAXWELL_COMPRESSION_FILTER`, `MAX_MAXWELL_READ`, `MAX_MAXWELL_READ_BYTES` | constants and limits |
