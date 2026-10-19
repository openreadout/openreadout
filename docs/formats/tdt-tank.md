# TDT tanks and blocks (`.tsq` + `.tev`)

Tucker-Davis Technologies systems (OpenEx, Synapse) record a session as a *block*: a directory of
files that share a stem. A *tank* is a directory of blocks. OpenReadout reads the `.tsq` index
and the `.tev` data file of each block: every stream store becomes a trace, snippets the `spikes`
table, and strobe (epoc) onsets the `epocs` table. A tank's blocks become sweeps.

Derived from Neo's `TdtRawIO` (BSD-3, read as documentation and run as the reference reader) and
six public blocks and tanks. Provenance: `docs/provenance/tdt-tank.md`.

Crate: `openreadout-ephys`, format id `tdt-tank` (`TDT_FORMAT_ID`), extensions `TDT_EXTENSIONS`
(`tsq`, `tev`, `tbk`, `tdx`, `tin`, `tnt`, `sev`), reader `TdtReader`, dataset `TdtDataset`
(`open`, `open_input`).

## Detection and input

- A directory with a `.tsq` and a `.tev` of the same stem is a block (definite).
- A directory `<tank>` whose visible entries are subdirectories and note files (`.ini`, `.txt`,
  `.md`), with at least one subdirectory `<block>` holding `<tank>_<block>.tsq` (the names TDT
  gives a tank's blocks), is a tank (definite). Opened, every subdirectory with a `.tsq` is a
  block.
- A `.tsq` whose record 1 is a start mark (`looks_like_tsq`) is definite; a `.tev`, `.Tbk`,
  `.Tdx`, `.tin`, `.tnt` or `.sev` next to a `.tsq` is definite (the block is read). Without a
  `.tsq` alongside, the extension alone matches and opening fails with exit 6.
- `find_blocks` and `block_in_dir` collect the files (`BlockFiles`: `dir`, `stem`, `tsq`, `tev`,
  `tbk`, `sev`, `members`). A directory with several `.tsq` files is refused (exit 6) unless a
  file of one block was named. `.sev` file names give a store and channel (`sev_name_parts`:
  `<stem>_<store>_Ch<n>.sev`).

## Layout

### `.tsq` index (`parse_tsq` → `TsqIndex`)

Records of `TSQ_RECORD_LEN` = 40 bytes, little-endian (at most `MAX_TSQ_RECORDS`):

| bytes | field |
| --- | --- |
| 0–3 | int32 size in 4-byte words, this record's 40 bytes included |
| 4–7 | int32 event type |
| 8–11 | four-character store name (`store_text`) |
| 12–13 | uint16 channel (strobe-off records: first two characters of the onset store) |
| 14–15 | uint16 sort code (strobe-off records: last two characters of the onset store) |
| 16–23 | float64 time stamp, seconds since 1970-01-01 UTC |
| 24–31 | int64 offset of the data in the `.tev`; strobes hold their float64 value here |
| 32–35 | int32 data format code (`SampleFormat`: 0 `Float32`, 1 `Int32`, 2 `Int16`, 3 `Int8`, 4 `Float64`; `from_code`, `width`, `name`, `decode`) |
| 36–39 | float32 sampling rate in Hz |

Record 0 has type 0 and the file length as its size (`index_header` warning otherwise). Event
types: `EVENT_MARK` 0x8801 (store bytes 1: start mark → `start_time`; 2: stop mark →
`stop_time`), `EVENT_STREAM` 0x8101 (compared after `EVENT_STREAM_MASK` 0xFF0F), `EVENT_SNIPPET`
0x8201, `EVENT_STROBE_ON` 0x101, `EVENT_STROBE_OFF` 0x102, `EVENT_SCALAR` 0x201 (counted, not
decoded). `record_count`, `file_len`, `type_counts`, `findings`, `undecoded_records` (scalar and
unknown records).

- **Stream stores** (`streams`: `StreamStore` with `name`, `format_code`, `size_words`,
  `sample_rate_hz`, `channels`, `unreadable`; `format`, `chunk_bytes` = size × 4 − 40,
  `chunk_samples`). Each record is one chunk of one channel (`StreamChannel`: `offsets`,
  `first_time`, `last_time`, `gaps`). A store whose format code or chunk size changes, or whose
  format code is unknown, is `unreadable`.
- **Snippets** (`snippets`: `Snippet` with `time`, `store` (index into `snippet_stores`),
  `channel`, `sort_code`, `offset`, `bytes` = size × 4 − 40, `format_code`). The waveform length
  comes from the size field; the `.Tbk` point count can be larger (provenance log).
- **Strobes** (`strobes`: `Strobe` with `time`, `store`, `onset_store`, `value`, `off`). Strobe
  records have no `.tev` data: the offset field holds the value as float64.

### `.tev` data

Chunks and snippet waveforms back to back at the offsets the index gives. Samples of a channel are
its chunks in index order. Reads go through a 4 MiB window. A chunk past the end of the `.tev`
ends the channel there (`truncated`).

### `.Tbk` store headers (`parse_store_headers`)

Text items `NAME=…;TYPE=…;VALUE=…;` (store name, channel count, point count, data format,
sampling rate…) between binary bytes. They are reported in `vendor_metadata`
(`store_headers`), not used for decoding (at most `MAX_TBK_BYTES`).

### The `.tin` (`synapse`)

Synapse writes a `.tin` beside the `.tsq`: a zip archive holding `Summary.txt`, the experiment
(`.synexpz`) and the rig (`.synrig`). `Summary.txt` ends with `Versions:`, one
`  <component>: <version>` line per software component. The reader keeps the `SynapseAPI` line
(`41743P`) and reports `format_version` `Synapse 41743P` (`vendor_metadata`:
`synapse_api_version`). It reads only `Summary.txt`, at most `MAX_SUMMARY_BYTES`. A block without
a `.tin`, or with one it cannot read, names no writer and is read the same way.

### Not read

`.sev` files (one per channel of a store that the newer Synapse versions write outside the
`.tev`), `.Tdx`, the rest of the `.tin` and `.tnt` (notes). A stream store with
`.sev` files is listed in `unread_stores` and the notes.

## Mapping (`TdtDataset`)

- Blocks are sorted by start mark (`blocks`: `TdtBlock` with `files`, `index`, `tev_len`,
  `store_headers`, `synapse`). Times count from the first block's start mark (`origin`; the first time stamp
  when there is no start mark).
- **Traces** (`traces`: `TdtTrace` with `store`, `channels`, `format`, `chunk_samples`,
  `sample_rate_hz`, `blocks`), one per readable stream store, sorted by store name. Channels are
  the store's channel numbers in order, named `<store> <n>` (`extra.channel_number`), `dtype` the
  data format, `scale` 1 and no unit: TDT files record no unit or gain, so values are as stored.
  Sweeps are blocks (`BlockStream`: `chunks` wholly in the `.tev`, `indexed_chunks`, `start_s`).
  `sample_count` is the longest sweep; `extra`: `store`, `data_format`, `samples_per_chunk`,
  `recorded_at` (the first block's start mark, UTC), `sweep_sample_counts` (when sweeps differ),
  `indexed_sample_counts` (when the `.tev` is cut short), `sweep_starts_s` (tanks). Channels of a
  store with different chunk counts give the shortest channel's samples (`uneven_channels`).
  A store whose layout differs between blocks is not read. Reads are capped at
  `MAX_TDT_READ_VALUES` values.
- **`spikes` table** (when there are snippets): `time_s`, `block`, `store` (index into
  `extra.stores`, `snippet_stores`), `channel`, `sort_code`, `w0` … (`snippet_width` =
  `extra.samples_per_snippet`, NaN beyond a shorter snippet or for one past the end of the
  `.tev`). Waveform values as stored.
- **`epocs` table** (when there are strobe onsets): `time_s`, `block`, `store` (index into
  `extra.stores`, `epoc_stores`), `value`, `duration_s` (`EpocRow`). The k-th strobe-off record
  naming a store closes its k-th onset when it falls between that onset and the next; otherwise
  `duration_s` is NaN. `extra.paired_offsets` counts onsets with a duration; strobe-off records
  left over are `unpaired_offsets`.

## `check` finding codes

`truncated`, `snippets_past_end` (errors); `index_header`, `partial_record`, `no_start_mark`,
`no_stop_mark`, `uneven_channels`, `gaps`, `unpaired_offsets` (warnings); `store_not_read`
(info).

## Oracle mapping

`oracle/gen.py` `tdt()`: Neo's `TdtRawIO` on a symlinked copy of each block's `.tsq`, `.tev` and
`.Tbk` (an empty `.Tdx` added, an empty `.tev` replaced by one byte, `.sev` files left out, all
explained in the function). Traces: Neo's streams with samples sorted by name, every sweep and
channel hashed; where the `.tev` is cut short, the oracle hashes the samples in whole chunks and
says so in `oracle_note`. Spikes: Neo's units as sorted rows (`time_s`, `block`, `channel`,
`sort_code`, waveform cut to the record's own length). Epocs: Neo's events as sorted rows
(`time_s`, `block`, `value`), from the `.tsq` with Neo's record layout where Neo's `.Tbk` parse
misses the stores. `store` and `duration_s` are not compared.

## Observed corpus values

| id | blocks | streams | snippets | strobe stores |
| --- | --- | --- | --- | --- |
| `gin-tdt-aep-05` | 2 (tank) | LFPs, Resp, pNeu (int16) | eNeu, 62 float32 samples | Tick, Stim, Odor, Conc |
| `gin-tdt-dataset-0-single-block` | 1 | EEG1, EEG2, Wav6 (float32; `.tev` cut short) | – | – |
| `gin-tdt-dataset-1-single-block` | 1 | ANIN, Wav1–Wav8 (float32; `.tev` cut short) | – | – |
| `gin-tdt-epocs-with-offsets-1` | 1 | – | – | s1s/, s4s/, sms/ (with offsets), Tick |
| `gin-tdt-epocs-with-offsets-2` | 1 | – | – | PC0/, Pu1e (with offsets) |
| `gin-tdt-epocs-with-payload` | 1 | – | – | PAB_ |

## Known gaps

- `.sev` files are not read; a block whose streams are only in `.sev` files has no traces.
- Scalar records (0x201) are counted, not decoded.
- Units: none recorded, none assumed.

## Vocabulary (every public identifier in `crates/openreadout-ephys/src/tdt/` must appear here)

| identifier | meaning |
| --- | --- |
| `TdtReader`, `TDT_FORMAT_ID`, `TDT_EXTENSIONS`, `TdtDataset`, `open`, `open_input`, `dataset`, `tsq` | entry points and modules |
| `BlockFiles`, `dir`, `stem`, `tsq`, `tev`, `tbk`, `sev`, `members`, `block_in_dir`, `find_blocks`, `sev_name_parts` | the files of a block |
| `TSQ_RECORD_LEN`, `MAX_TSQ_RECORDS`, `EVENT_STROBE_ON`, `EVENT_STROBE_OFF`, `EVENT_SCALAR`, `EVENT_STREAM`, `EVENT_SNIPPET`, `EVENT_MARK`, `EVENT_STREAM_MASK`, `looks_like_tsq`, `parse_tsq`, `store_text` | the index |
| `SampleFormat`, `Float32`, `Int32`, `Int16`, `Int8`, `Float64`, `from_code`, `width`, `name`, `decode` | data format codes |
| `TsqIndex`, `file_len`, `record_count`, `start_time`, `stop_time`, `streams`, `snippet_stores`, `snippets`, `strobes`, `type_counts`, `findings`, `undecoded_records` | a parsed index |
| `StreamStore`, `format_code`, `size_words`, `sample_rate_hz`, `channels`, `unreadable`, `format`, `chunk_bytes`, `chunk_samples` | a stream store |
| `StreamChannel`, `offsets`, `first_time`, `last_time`, `gaps` | a stream channel |
| `Snippet`, `time`, `store`, `channel`, `sort_code`, `offset`, `bytes` | a snippet record |
| `Strobe`, `onset_store`, `value`, `off` | a strobe record |
| `TdtBlock`, `files`, `index`, `tev_len`, `store_headers`, `parse_store_headers`, `MAX_TBK_BYTES` | an opened block |
| `synapse`, `synapse_version`, `summary_synapse_version`, `MAX_SUMMARY_BYTES`, `synapse_api_version` | the Synapse version from the `.tin` |
| `TdtTrace`, `blocks`, `BlockStream`, `chunks`, `indexed_chunks`, `start_s`, `MAX_TDT_READ_VALUES` | traces |
| `traces`, `unread_stores`, `origin`, `snippet_width`, `epoc_stores`, `epocs`, `unpaired_offsets` | the dataset |
| `EpocRow`, `time_s`, `block`, `duration_s` | an `epocs` row |
