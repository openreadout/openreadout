# Open Ephys recordings

The Open Ephys GUI records extracellular electrophysiology in two layouts: the binary format (a `structure.oebin` beside `continuous.dat` and NumPy event files) and the legacy "Open Ephys format" (`.continuous`, `.events` and `.spikes` files). OpenReadout returns each continuous stream as a trace whose recordings are sweeps, scaled by each channel's `bit_volts`. TTL and text events become a table `events`; legacy spike files become tables of waveforms. Binary event streams and spike groups of the binary format are listed, not decoded.

Derived from the Open Ephys GUI documentation and Neo's `OpenEphysBinaryRawIO` and `OpenEphysRawIO` (BSD-3, read as documentation), and checked on public recordings from GUI 0.4.4 to 1.0.1 with Neo as a reference reader. See `docs/provenance/open-ephys.md`.

Crate: `openreadout-ephys`, format id `open-ephys` (`OPEN_EPHYS_FORMAT_ID`), reader
`OpenEphysReader`, dataset `OpenEphysDataset` (`open`, `open_input`). Input: a recording directory
(a `Record Node`, experiment or recording folder, or the folder of legacy files), its
`structure.oebin`, or a `.continuous` file (its directory is read).

**Detection of a directory:** only a level of the recording itself is claimed — every entry must be a `Record Node …` (any spelling), `experiment…`, `recording…`, `continuous`, `events` or `spikes` folder or a recording file (`.continuous`, `.events`, `.spikes`, `.oebin`, `.openephys`, `.xml`, `.npy`, notes `.txt`/`.md`/`.json`/`.ok`), and a `structure.oebin` must lie within four levels or a `.continuous` file within one. A share that holds recordings among other files is not one recording (`index` walks into it).

## Binary format (`binary::discover` → `OeRecording`)

`structure.oebin` files are searched up to `MAX_OEBIN_DEPTH` levels below the input (at most
`MAX_RECORDINGS`; each at most `MAX_OEBIN_LEN` bytes). Each recording (`node` = the `Record…`
directory name or "", `experiment`, `recording`, `dir`, `gui_version`, `streams`, `events`,
`spike_groups`) lists continuous streams (`OeStream`: `folder`, `sample_rate`, `channels`,
`data_path`, `samples`, `first_sample`, `first_timestamp_s`, `processor`, `stream_name`) with
channels (`OeChannel`: `name`, `bit_volts`, `units`) and event streams (`OeEventStream`:
`folder`, `channel_name`, `kind`, `sample_rate`, `dir`).

- **Traces** (`binary::layout` → `OeTrace`: `recordings`, `stream`): one stream folder of one
  node and experiment with the same channels; its recordings are sweeps. `continuous.dat` holds
  interleaved little-endian int16 (`samples` = size / (2 × channels)); values = raw ×
  `bit_volts` in the channel's `units`. When `units` is empty: µV for `CH`/`AP`/`LFP` channels,
  V for `ADC`/`AI` channels (the Open Ephys docs), otherwise no unit; `extra.unit_assumed` marks
  it. Trace `extra`: `record_node`, `experiment`, `recordings`, `stream_folder`, `stream_name`,
  `source_processor`, `sweep_starts_s` (first sample number / rate, as Neo's `t_start`),
  `first_sample_numbers`, `first_timestamps_s` (the synchronized `timestamps.npy` of GUI 0.6+ or
  `synchronized_timestamps.npy` of 0.5), `sweep_sample_counts`; on trace 0 `application`,
  `application_version`. First sample numbers come from `sample_numbers.npy` (0.6+) or the
  integer `timestamps.npy` (before 0.6).
- **Table `events`** (`binary::events` → `EventRows`: `stream`, `sample_number`, `time_s`,
  `timestamp_s`, `state`, `line`, `full_word`, `text`, `texts`, `stream_list`, `undecoded`; at
  most `MAX_EVENT_ROWS`): TTL streams (`states.npy`, or `channel_states.npy` + `channels.npy`
  before 0.6; `full_words.npy`) and text streams (`text.npy`), in recording and stream order;
  `time_s` = sample number / the stream's rate; `extra.streams[]` describes each `stream` index,
  `extra.texts` the text codes. Binary event streams (`data_array.npy`, tracking) and spike
  groups are listed (`undecoded_events` finding, notes), not decoded.
- A stream folder listed in `structure.oebin` but missing → `missing_stream` (warning), skipped.

`.npy` files (`npy::read_header` → `NpyHeader`: `dtype`, `shape`, `data_offset`, `len`,
`is_empty`; `read_values`, `read_strings`): NumPy's documented header; element types `NpyType`
`U8`, `I16`, `U16`, `I32`, `I64`, `U64`, `F32`, `F64`, `Bytes(n)` (`width`, `is_integer`); other
types and Fortran order are refused. Headers ≤ `MAX_NPY_HEADER`, arrays ≤ `MAX_NPY_BYTES`.

## Open Ephys format (legacy; `legacy::discover` → `LegacyFiles`)

`.continuous`, `.events` (except `messages*.events`) and `.spikes` files in the directory and its
`Record…` subdirectories (at most `MAX_LEGACY_FILES` continuous files). Every file starts with a
1024-byte text header (`LEGACY_HEADER_LEN`, `parse_header`, `looks_like_legacy`) of
`header.<key> = <value>;` statements.

- **Continuous files** (`legacy::channel` → `LegacyChannel`: `path`, `name`, `source`,
  `segment`, `sample_rate`, `bit_volts`, `version`, `date_created`, `runs`, `refused`): records of
  `RECORD_LEN` = 2070 bytes: i64 timestamp (sample number), u16 sample count, u16 recording
  number, `RECORD_SAMPLES` = 1024 big-endian int16 samples, the marker `RECORD_MARKER`
  (0 1 2 … 8 255). A record continues a gap-free run (`Run`: `first_record`, `records`,
  `timestamp`, `recording`) when it has the same recording number and starts exactly 1024 samples
  after the previous one. A file is refused (`refused`, `unreadable_channel` finding; its samples
  are not read) when its body is not whole records, a marker is missing, a record holds other
  than 1024 samples, timestamps go backwards, or there are more than two runs and they are more than half the records
  (irregular clock). At most `MAX_RECORDS` records per file.
- File names: `<source>_<channel>[_<segment>].continuous` (`segment` ≥ 2 for later experiments).
- **Traces** (`legacy::layout` → `LegacyTrace`: `node`, `channels`): readable channels of one
  node, source and segment with the same sample rate and runs; runs are sweeps. Channel order
  (`channel_order`): `CH`, then `AUX`, then `ADC`, by number. Values = raw × `bitVolts`, in µV for
  `CH` channels and V otherwise. Trace `extra`: `record_node`, `source`, `segment`,
  `sweep_starts_s`, `sweep_recordings`, `sweep_sample_counts`; on trace 0 `application`,
  `format_version`, `date_created`; channel `extra.file`.
- **Table `events`**: every `.events` file's 16-byte records (`EVENT_RECORD_LEN`): `file`,
  `timestamp`, `time_s` (/ the header's `sampleRate`), `sample_position`, `event_type`,
  `processor_id`, `event_id`, `channel`, `recording`.
- **Tables `spikes <electrode>`** (`legacy::spikes` → `LegacySpikes`: `path`, `electrode`,
  `sample_rate`, `channels`, `samples`, `record_len`, `records`): records of u8 type, i64
  timestamp, i64 software timestamp, u16 source, u16 channel count, u16 samples, u16 sorted id,
  u16 electrode id, u16 channel, 3 colour bytes, 2 f32 projections, u16 rate, then channels ×
  samples u16 samples, channels × f32 gains, channels × u16 thresholds, u16 recording. Columns
  `timestamp`, `time_s`, `sorted_id`, `electrode_id`, `channel`, `recording`, `w<c>_<k>` =
  (sample − 32768) / gain × 1000 µV (each channel with its own gain).

Reads are capped at `MAX_OE_READ` samples per channel and `MAX_OE_TABLE_READ` rows.

## `check` finding codes

`missing_stream`, `partial_sample`, `unreadable_channel`, `partial_record` (warnings);
`bad_stream` (error); `undecoded_events` (info).

## Vocabulary (every public identifier in the Open Ephys modules of `openreadout-ephys`)

| identifier | meaning |
| --- | --- |
| `OpenEphysReader`, `OPEN_EPHYS_FORMAT_ID`, `OpenEphysDataset`, `open`, `open_input`, `MAX_OE_READ`, `MAX_OE_TABLE_READ` | entry points |
| `OeRecording`, `node`, `experiment`, `recording`, `dir`, `gui_version`, `streams`, `events`, `spike_groups`, `discover`, `MAX_OEBIN_DEPTH`, `MAX_OEBIN_LEN`, `MAX_RECORDINGS` | binary recordings |
| `OeStream`, `folder`, `sample_rate`, `channels`, `data_path`, `samples`, `first_sample`, `first_timestamp_s`, `processor`, `stream_name` | continuous streams |
| `OeChannel`, `name`, `bit_volts`, `units` | channels |
| `OeEventStream`, `channel_name`, `kind` | event streams |
| `OeTrace`, `recordings`, `stream`, `layout` | traces |
| `EventRows`, `sample_number`, `time_s`, `timestamp_s`, `state`, `line`, `full_word`, `text`, `texts`, `stream_list`, `undecoded`, `MAX_EVENT_ROWS` | binary events |
| `NpyHeader`, `NpyType`, `U8`, `I16`, `U16`, `I32`, `I64`, `U64`, `F32`, `F64`, `Bytes`, `dtype`, `shape`, `data_offset`, `len`, `is_empty`, `width`, `is_integer`, `read_header`, `read_values`, `read_strings`, `MAX_NPY_HEADER`, `MAX_NPY_BYTES` | `.npy` files |
| `LegacyFiles`, `LegacyChannel`, `path`, `source`, `segment`, `version`, `date_created`, `runs`, `refused`, `channel`, `channel_order`, `parse_header`, `looks_like_legacy`, `LEGACY_HEADER_LEN`, `RECORD_LEN`, `RECORD_SAMPLES`, `RECORD_MARKER`, `MAX_RECORDS`, `MAX_LEGACY_FILES` | legacy continuous files |
| `Run`, `first_record`, `records`, `timestamp` | gap-free runs |
| `LegacyTrace` | legacy traces |
| `LegacySpikes`, `electrode`, `record_len`, `spikes`, `EVENT_RECORD_LEN` | legacy spikes and events |
