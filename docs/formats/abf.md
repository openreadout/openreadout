# Axon ABF

ABF files are written by Clampex and AxoScope (pCLAMP) for patch-clamp recordings. OpenReadout exposes a file as **one trace**: `sweep_count` sweeps × input channels, every sample scaled to the channel's physical unit (`pA`, `mV`, …). Sweeps are the episodes of an episodic protocol; gap-free recordings have one sweep. ABF 1 (1.3–1.84) and ABF 2 (2.0–2.9) are read, gap-free and episodic, float32 and int16. Clampfit's ATF text exports are covered at the end of this page.

Derived from Scott Harden's public, MIT-licensed pyABF documentation and source (pyABF 2.3.8, commit `3ad9cd6`) and public corpus files; pyABF is also used as a reference reader. See `docs/provenance/abf.md`.

All numbers are little-endian. Offsets below are bytes from the start of the file (ABF 1) or of the section (ABF 2).

## Detection

Bytes 0–3: `ABF ` (ABF 1) or `ABF2` (ABF 2). Both are definite signatures. Older `CLPX`/`FTCX` pCLAMP files are not read.

## ABF 2

### Fixed header (512 bytes)

| offset | type | our name | use |
| --- | --- | --- | --- |
| 0 | char[4] | – | `ABF2` |
| 4 | u8[4] | `version` | stored build, bugfix, minor, major → `2.6.0.0` |
| 12 | u32 | `header_sweep_count` | sweeps recorded (0 in gap-free files) |
| 16 | u32 | – | date `YYYYMMDD` → `created_at` |
| 20 | u32 | – | milliseconds since midnight → `created_at` |
| 30 | u16 | `sample_format` | 0 → `Int16`, 1 → `Float32` |
| 40 | u8[16] | `guid` | GUID (first three fields little-endian) |
| 56 | u8[4] | `creator_version` | writer version, stored reversed |
| 60 | u32 | – | string index of the writer name → `creator` |
| 72 | u32 | – | string index of the protocol path → `protocol_path` |
| 76 | 18 × 16 bytes | `sections` | section map (below) |

### Section map (`SECTION_MAP_OFFSET` = 76, `SECTION_ENTRY_LEN` = 16)

Each entry is `u32 block`, `u32 entry_size`, `i64 entry_count`; the section starts at `block × 512` (`BLOCK_LEN`). The 18 entries, in order and in our names (`SECTION_NAMES`): `protocol`, `adc`, `dac`, `epoch`, `adc_per_dac`, `epoch_per_dac`, `user_list`, `stats_region`, `math`, `strings`, `data`, `tag`, `scope`, `delta`, `voice_tag`, `synch_array`, `annotation`, `stats`. A section's bytes are `entry_size × entry_count`, except `strings`, which is one block of `entry_size` bytes holding `entry_count` strings (inferred; see provenance). `info --view structure` lists every non-empty section with its offset and size.

### Sections we decode

**protocol** (one entry): 0 i16 operation mode (`AcquisitionMode`: 1 `EventVariableLength`, 2 `EventFixedLength`, 3 `GapFree`, 4 `HighSpeedOscilloscope`, 5 `Episodic`, other → `Other`); 2 f32 sample interval per channel in µs (`sample_interval_us`; rate = 1e6 / interval); 14 f32 synch time unit in µs (`synch_time_unit_us`); 62 f32 sweep start-to-start interval in s (`sweep_interval_s`); 110 f32 ADC range in V (`adc_range_v`); 118 i32 ADC resolution in counts (`adc_resolution`); 126 i16 experiment type (`experiment_kind_code`); 132 i32 string index of the file comment (`comment`); 182 i16 alternate DAC output state (`alternate_dac_output`, non-zero = on); 206 i16 digitizer type (`digitizer_code`).

**adc** (one entry per recorded channel, in data order → `InputChannel`): 0 i16 physical ADC number (`adc_number`); 2 i16 telegraph enabled (== 1); 4 i16 telegraph instrument (`instrument_code`); 6 f32 telegraph additional gain (`additional_gain`); 10 f32 telegraph filter (`filter_hz`); 14 f32 membrane capacitance (`membrane_capacitance`); 18 i16 telegraph clamp mode (`clamp_mode_code`); 28 f32 `programmable_gain`; 40 f32 `instrument_scale` (V at the ADC per user unit); 44 f32 `instrument_offset`; 48 f32 `signal_gain`; 52 f32 `signal_offset`; 56 f32 `lowpass_hz`; 60 f32 `highpass_hz`; 74 i32 string index of the channel `name`; 78 i32 string index of the `unit`.

**dac** (→ `OutputChannel`): 0 i16 DAC number (`index`); 12 f32 `holding_level`; 24 i32 string index of the name; 28 i32 string index of the unit; 40 i16 waveform enabled (`waveform_enabled`); 42 i16 waveform source (`waveform_source_code`: 1 epochs, 2 stimulus file); 44 i16 inter-sweep level (`inter_episode_last`: non-zero = hold the last epoch level between sweeps instead of the holding level); 60 i16 conditioning train enabled (`conditioning`); 118 i32 string index of the stimulus file path (`stimulus_file`).

**user_list**: per entry 2 i16 enabled, 4 i16 parameter to vary; any entry enabled or naming a parameter (> 0) sets `user_list_active`.

**epoch_per_dac** (→ `Epoch`, grouped by DAC): 0 i16 epoch number (`index`); 2 i16 DAC number; 4 i16 type (`kind_code`, `EpochKind`: 0 `Off`, 1 `Step`, 2 `Ramp`, 3 `PulseTrain`, 4 `TriangleTrain`, 5 `CosineTrain`, 7 `BiphasicTrain`, other `Other`); 6 f32 first level (`level`); 10 f32 per-sweep level increment (`level_step`); 14 i32 first duration in samples (`duration`); 18 i32 per-sweep duration increment (`duration_step`); 22 i32 `pulse_period`; 26 i32 `pulse_width`.

**epoch** (→ `DigitalEpoch`): 0 i16 epoch number (`epoch`); 2 i16 digital output bits (`pattern`, bit n = output n; shown as eight binary digits).

**strings** (→ `strings`, `parse_string_table`): `SSCH`, u32 1, u32 number of strings, two u32 we do not interpret, NUL padding, then NUL-terminated strings. String index 1 is the first string; 0 means none. Bytes are Latin-1 (0xB5 = `µ`).

**data**: `entry_count` samples of `entry_size` bytes (2 for `Int16`, 4 for `Float32`), channels interleaved sample by sample in `adc` order, sweeps back to back.

**synch_array** (→ `synch`): per sweep `i32 start, i32 length`; start in synch-time units (µs × `synch_time_unit_us`) or, when the unit is 0, in multiplexed samples; length in multiplexed samples (all channels).

**tag** (→ `Tag`): 0 i32 time (`raw_time`, synch-time units; `time_s`); 4 char[56] `comment`; 60 i16 type (`kind_code`: 0 time, 1 comment, 2 external, 3 voice).

## ABF 1

A fixed header: 2048 bytes (`ABF1_BASIC_LEN`) in files written before version 1.6, 6144 bytes (`ABF1_EXTENDED_LEN`) after. We read the fields past byte 2048 only when the data starts at or after byte 6144 (`header_len`), because in old files those offsets are sample data.

| offset | type | use |
| --- | --- | --- |
| 4 | f32 | version (`1.83` → `version`) |
| 8 | i16 | operation mode (as ABF 2) |
| 10 | i32 | samples in the data, all channels (`total_samples`) |
| 14 | i16 | samples ignored at the data start (× sample width added to `data_offset`) |
| 16 | i32 | sweeps (`header_sweep_count`) |
| 20 / 24 | i32 / i32 | date (`YYYYMMDD`, or `YYMMDD` with 80–99 → 19xx) / seconds since midnight → `created_at` |
| 40 | i32 | data block → `data_offset` |
| 44 / 48 | i32 / i32 | tag block / tag count (64-byte `Tag` records, as ABF 2) |
| 92 / 96 | i32 / i32 | synch-array block / entry count |
| 100 | i16 | sample format (0 int16, 1 float32) |
| 120 | i16 | channels (1–16) |
| 122 | f32 | sample interval in µs, **multiplexed**: per-channel interval = value × channels |
| 130 | f32 | synch time unit (µs) |
| 178 | f32 | sweep start-to-start interval (s) |
| 244 / 252 | f32 / i32 | ADC range (V) / resolution |
| 260 | i16 | experiment type |
| 294 | char[16] | `creator` |
| 310 | char[56] | comment (older field) |
| 366 | i16 | milliseconds added to the start time |
| 410 | i16[16] | physical ADC sampled at each data position: channel `i` uses per-ADC arrays at index `sequence[i]` |
| 442 / 602 | char[10][16] / char[8][16] | ADC names / units |
| 730 / 922 / 986 / 1050 / 1114 | f32[16] | `programmable_gain` / `instrument_scale` / `instrument_offset` / `signal_gain` / `signal_offset` |
| 1178 / 1242 | f32[16] | `lowpass_hz` / `highpass_hz` |
| 1306 / 1346 / 1394 | char[10][4] / char[8][4] / f32[4] | DAC names / units / `holding_level` |
| 1436 / 1588 | i16 / i16[10] | digital outputs enabled / per-epoch digital pattern |
| 1440, 1444–1583 | i16, 10-entry tables | active DAC and its epoch table (old files: type i16, levels f32, durations i16) |
| 2136 / 2216 | i32[2][10] | `pulse_period` / `pulse_width` (extended) |
| 2296 / 2300 | i16[2] | waveform enabled / source per DAC (extended) |
| 2308–2667 | [2][10] tables | epoch type, level, level step, duration, duration step (extended) |
| 2736 | char[256][2] | stimulus file per DAC (extended) |
| 4512–4799 | [16] arrays | telegraph enabled, instrument, additional gain, filter, capacitance, clamp mode (extended) |
| 4898 / 5154 | char[256] / char[128] | `protocol_path` / `comment` (extended) |
| 5282 | u8[16] | `guid` (extended) |
| 5798 | i16[4] | `creator_version` (extended) |

The sample rate is `1e6 / interval / channels`.

## Sweeps (`Sweep`)

Sweep count = header sweeps, except 1 for gap-free files and when the header says 0. When the synch array has one entry per sweep and the lengths differ (variable-length event acquisition, `pyabf-2020-06-16-0000`), each sweep's `sample_count` is its synch length ÷ channels and sweeps follow one another (`first_sample`); otherwise the data divides into equal sweeps (a remainder is reported as `sweep_length_mismatch`). `start_s` comes from the synch array when present, else `sweep × sweep_interval_s` (or × the sweep length). Trace `sample_count` is the longest sweep; `extra.sweep_sample_counts` lists them when they differ.

## Scaling (`channel_scaling`)

For int16 data, per channel, evaluated in this order in f64:

```
scale  = 1 / instrument_scale / signal_gain / programmable_gain [/ telegraph additional_gain if telegraph enabled] × adc_range_v / adc_resolution
offset = instrument_offset − signal_offset
value  = raw × scale + offset
```

Float32 data is already in physical units (`scale` 1, `offset` 0). `SignalChannelInfo.scale`/`offset` report the pair. pyABF computes the same chain but applies it in float32; see the provenance log for why the oracle recomputes in f64.

## Command waveforms (trace 1; `command_plan` → `CommandPlan` {`outputs`, `refused`}, `command_sweep`, `command_trace_info`)

The DAC command of each sweep is synthesized from the epoch table when the header fixes it completely: ABF 2, episodic stimulation, sweeps of one length, no active user list, no alternating DAC outputs; per DAC: waveform enabled with the epoch table as its source (not a stimulus file), no conditioning train, only off, step, ramp and pulse-train epochs (pulse period > 0), non-negative durations, and the epochs ending inside the sweep. Otherwise there is no trace 1 and `info` notes say why (`refused`); the epoch table stays in trace 0 `extra.outputs`.

A sweep of `n` samples: the first `n / 64` (integer division) samples are the pre-sweep level; then each epoch that is not off, in order, lasts `duration + duration_step × sweep` samples at `level + level_step × sweep`; the rest of the sweep is the post-sweep level. The pre-sweep level is the holding level, or with `inter_episode_last` the last epoch level of the previous sweep (the holding level for sweep 0); the post-sweep level is the holding level, or with `inter_episode_last` this sweep's last epoch level. A step holds its level; a ramp goes linearly from the level before it to its own level, both ends included (`i × (b − a)/(len − 1) + a`, the last sample exactly `b`); a pulse train holds the level before it and takes the epoch level for `pulse_width` samples at every multiple of `pulse_period` (whole periods only). This is the rule pyABF documents and ClampEx follows (the 1/64 pre-sweep holding is ClampEx's).

Trace 1: `name` `command`, same `sample_rate_hz`, `sample_count` and `sweep_count` as trace 0; one float64 channel per synthesized DAC (`name` = the DAC name or `DAC<n>`, `unit` = the DAC unit, channel `extra.dac`, `extra.holding_level`); trace `extra.synthesized` = true, `extra.source`, `extra.not_synthesized` (reasons for other DACs). Values are the command in the DAC's units; nothing is read from the file's data section. A sweep whose epochs overrun it is refused on read (exit 6).

## Normalized fields

`TraceInfo`: `name` = protocol name (file stem of `protocol_path`), `sample_rate_hz`, `sample_count` (longest sweep), `sweep_count`, `channels[]` (`name` or `ch<i>` when blank, `unit`, `dtype` `int16`/`float32`, `scale`, `offset`, `extra`: `adc_number`, `programmable_gain`, `instrument_scale`, `instrument_offset`, `signal_gain`, `signal_offset`, `lowpass_hz`, `highpass_hz`, `telegraph {enabled, instrument_code, additional_gain, filter_hz, membrane_capacitance, clamp_mode_code}`), `start_s` = 0. Trace `extra`: `abf_version`, `generation` (`abf1`/`abf2`), `acquisition_mode` (+`_code`), `sample_format`, `sample_interval_us`, `adc_range_v`, `adc_resolution`, `sweep_interval_s`, `sweep_sample_counts`, `sweep_starts_s`, `created_at`, `creator`, `creator_version`, `protocol`, `protocol_path`, `comment`, `guid`, `experiment_kind_code`, `digitizer_code`, `tags[] {time_s, comment, kind_code}`, `outputs[] {index, name, unit, holding_level, waveform_enabled, waveform_source_code, stimulus_file, epochs[] {index, kind, kind_code, level, level_step, duration, duration_step, pulse_period, pulse_width}}`, `digital_outputs[] {epoch, pattern}`.

## `check` finding codes

`truncated`, `section_out_of_bounds`, `bad_offset`, `bad_section`, `bad_sample_interval`, `bad_scaling` (ADC range/resolution) (errors); `sweep_length_mismatch`, `synch_length_mismatch`, `partial_sample_group`, `data_length_mismatch`, `no_samples`, `bad_scaling` (per channel), `bad_channel_map`, `tag_outside_recording` (warnings); `no_creation_time`, `synch_count_mismatch`, `unnamed_channel` (info). A header that cannot be parsed at all (no signature, no section map, no protocol/ADC/data section, impossible channel count) fails `open` with a corrupt-file error (exit 4).

## Observed corpus values

| file | version | mode | format | ch | sweeps | rate (Hz) | notes |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `pyabf-130618-1-12` | 1.30 | episodic | int16 | 1 | 3 | 50000 | 2048-byte header, date `180618` |
| `pyabf-invaliddate-abf1` | 1.30 | episodic | int16 | 1 | 50 | 20000 | date and time −1; blank channel name |
| `pyabf-sample-trace-0054` | 1.65 | gap-free | int16 | 1 | 1 | 50000 | header says 1081 sweeps (acquisition chunks) |
| `pyabf-file-axon-3` | 1.83 | episodic | int16 | 2 | 5 | 20000 | sampling sequence 5, 7 (physical ADCs) |
| `pyabf-multichannelabf1withtags` | 1.84 | episodic | int16 | 2 | 187 | 20000 | 2 tags |
| `pyabf-14o16001-vc-pair-step` | 2.0.0.0 | episodic | int16 | 2 | 13 | 10000 | synch unit 10 µs, sweeps 4 s apart |
| `pyabf-171117-hfmixfret` | 2.0.0.0 | episodic | int16 | 4 | 13 | 10000 | unit `µA` (0xB5), user list string |
| `pyabf-user-list-durations` | 2.0.0.0 | episodic | float32 | 2 | 3 | 2000 | ADC range 10.24 V |
| `pyabf-file-axon-7` | 2.6.0.0 | episodic | float32 | 1 | 12 | 403.2258 | interval 2480 µs |
| `pyabf-2020-06-16-0000` | 2.3.0.0 | event, variable | int16 | 1 | 3 | 10000 | sweeps of 3540, 70040, 16040 samples |
| `pyabf-18425108` | 2.9.0.0 | episodic (1 sweep) | int16 | 2 | 1 | 25000 | same recording as `pyabf-18425108-abf1` |

## ATF (Axon Text File; `AtfReader`, `AtfDataset`, format id `atf`, `ATF_FORMAT_ID`)

Derived from pyABF's `ATF` class (MIT, read as documentation) and four ATF 1.0 files from the pyABF repository. Clampfit exports a recording as text:

| line | content | our name |
| --- | --- | --- |
| 1 | `ATF` and a version (`1.0`) (`looks_like_atf`: definite) | `version` |
| 2 | number of header records, number of columns (≤ `MAX_HEADER_RECORDS`, `MAX_COLUMNS`) | |
| 3 … | one header record per line: `"Key=value"`, or `"Signals="` followed by one tab-separated quoted signal per data column | `header` (`acquisition_mode`, `comment`, `sweep_start_times_ms`, `signals_exported`, `sync_time_units` in trace `extra`) |
| next | column titles: `"Time (s)"`, then `"Trace #1 (pA)"`, … | `time_title`, `time_scale` (s, ms or µs), `AtfColumn` (`title`, `channel`, `sweep`, `unit` from the parentheses) |
| rest | one row per sample: time, then every data column (tab-separated decimals) | `rows` (byte offset per row), `times` |

One trace: channels are the signals in the order `Signals=` first names them (`channels`), and the n-th column naming a signal is its n-th sweep (`sweeps`); without `Signals=` every column is a channel of one sweep. Rate = 1 / (second time − first time); `start_s` = first time. Values are the decimal text parsed as float64 (dtype `float64`, scale 1). Files up to `MAX_ATF_LEN`. A table whose first column is not time (GenePix results files are also ATF) exits 6. `check`: `truncated` (a row with fewer values than columns, or a last row without its line end) (errors); `uneven_sweeps`, `irregular_time`, `no_samples` (warnings); `no_signals` (info).

| file | channels | sweeps | rows | rate (Hz) |
| --- | --- | --- | --- | --- |
| `pyabf-18702001-step-atf` | 2 (`IN 0` pA, `IN 1` A) | 3 | 20000 | 20000 |
| `pyabf-model-vc-ramp-atf` | 1 | 50 | 2400 | 20000 |
| `pyabf-model-vc-step-atf` | 1 | 20 | 10000 | 20000 |
| `pyabf-sine-sweep-magnitude-20-atf` | 1 (no unit) | 1 | 100000 | 10000 |

## Vocabulary (every public identifier in `openreadout-abf` must appear here)

| identifier | meaning |
| --- | --- |
| `AtfReader`, `AtfDataset`, `ATF_FORMAT_ID`, `MAX_ATF_LEN`, `MAX_HEADER_RECORDS`, `MAX_COLUMNS`, `looks_like_atf`, `AtfColumn`, `title`, `channel`, `sweep`, `unit`, `header`, `time_title`, `time_scale`, `columns`, `channels`, `sweeps`, `rows`, `times` | Axon Text File |
| `AbfReader`, `AbfDataset`, `FORMAT_ID`, `open`, `header`, `trace_info`, `MAX_READ_SAMPLES` | entry points; `header` returns the parsed `AbfFile` |
| `AbfFile`, `generation`, `version`, `file_len`, `header_len`, `mode`, `sample_format`, `data_offset`, `total_samples`, `sample_interval_us`, `sample_rate_hz`, `header_sweep_count`, `sweeps`, `channels`, `outputs`, `digital`, `adc_range_v`, `adc_resolution`, `synch_time_unit_us`, `sweep_interval_s`, `creator`, `creator_version`, `protocol_path`, `comment`, `created_at`, `guid`, `experiment_kind_code`, `digitizer_code`, `tags`, `synch`, `sections`, `strings`, `findings` | parsed header |
| `samples_per_channel`, `data_len`, `protocol_name`, `max_sweep_len`, `variable_length` | derived values |
| `Generation` { `Abf1`, `Abf2` }, `AcquisitionMode` { `EventVariableLength`, `EventFixedLength`, `GapFree`, `HighSpeedOscilloscope`, `Episodic`, `Other` }, `SampleFormat` { `Int16`, `Float32` }, `from_code`, `code`, `name`, `width`, `dtype` | enumerations |
| `SectionEntry`, `block`, `entry_size`, `entry_count`, `offset`, `byte_len`, `end`, `SECTION_NAMES`, `SECTION_MAP_OFFSET`, `SECTION_ENTRY_LEN`, `BLOCK_LEN`, `ABF1_BASIC_LEN`, `ABF1_EXTENDED_LEN`, `MAX_RECORDS`, `MAX_STRINGS_LEN` | layout |
| `InputChannel`, `index`, `adc_number`, `unit`, `programmable_gain`, `instrument_scale`, `instrument_offset`, `signal_gain`, `signal_offset`, `lowpass_hz`, `highpass_hz`, `telegraph`, `scale` | input channels |
| `Telegraph`, `enabled`, `instrument_code`, `additional_gain`, `filter_hz`, `membrane_capacitance`, `clamp_mode_code` | amplifier telegraphs |
| `OutputChannel`, `holding_level`, `waveform_enabled`, `waveform_source_code`, `stimulus_file`, `epochs`, `inter_episode_last`, `conditioning` | command outputs |
| `CommandPlan`, `outputs`, `refused`, `command_plan`, `command_sweep`, `command_trace_info`, `user_list_active`, `alternate_dac_output` | synthesized command waveforms |
| `Epoch`, `kind`, `kind_code`, `level`, `level_step`, `duration`, `duration_step`, `pulse_period`, `pulse_width`, `EpochKind` { `Off`, `Step`, `Ramp`, `PulseTrain`, `TriangleTrain`, `CosineTrain`, `BiphasicTrain`, `Other` } | epoch tables |
| `DigitalEpoch`, `epoch`, `pattern` | digital outputs |
| `Tag`, `raw_time`, `time_s` | tags |
| `Sweep`, `first_sample`, `sample_count`, `start_s` | sweep layout |
| `channel_scaling`, `usable_factor`, `iso_datetime`, `guid_text`, `parse_string_table` | helpers |
| `Block`, `origin`, `bytes`, `read_block`, `latin1_field`, `i16_at`, `u16_at`, `i32_at`, `u32_at`, `i64_at`, `f32_at`, `u8_at`, `bytes_at`, `text_at` | bounds-checked byte access |

### Byte-source integration

The existing layouts are read through `Input`/`Fs`/`SourceFile`, including directory
sessions and companions where applicable. Path, memory and callback namespaces use
the same parser and scaling. See [byte sources](../../book/src/guides/wasm.md).
