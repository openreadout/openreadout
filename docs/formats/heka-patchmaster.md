# HEKA PatchMaster bundles

HEKA PatchMaster saves a patch-clamp session as a `.dat` bundle holding the acquisition tree and the sampled data. OpenReadout returns every series as a trace whose sweeps are PatchMaster's sweeps and whose channels are its trace records, with the amplifier and recording settings of each channel. Samples are leak-subtracted as PatchMaster stored them; the zero offset is reported, not subtracted. Derived from HEKA's public file-format documents (`DataFile_v9.txt`, `PulsedFile_v9.txt`, `FileFormat.txt`, `TimeFormat.txt` and their v1000/v2000 variants) and load-heka-python (MIT), read as documentation, and checked on public files from PatchMaster v2x60 to v2x90.3 with load-heka-python and pyHEKA as reference readers. Provenance: `docs/provenance/heka-patchmaster.md`.

Crate: `openreadout-ephys`, format id `heka-patchmaster` (`HEKA_FORMAT_ID`), extension `dat`,
reader `HekaReader`, dataset `HekaDataset` (`open`, `open_input`, `tree`, `heka_traces`).

## Mapping

- **Every series is a trace** (`HekaTrace`: `group`, `series`, `channels`, `sweeps`,
  `interval_s`, `part`). Its sweeps are the trace's sweeps; the trace records of a sweep are its
  channels, in record order. Channels whose sample interval or per-sweep point counts differ from
  the others form further traces of the same series (`part` 1, 2, …; name `<series> (part k)`;
  `extra.series_part`); a channel missing from some sweeps gets a trace holding only the sweeps it
  is in (`extra.sweep_indices`).
- Values: integer samples × the trace record's data scaler (`scaler`), float samples as stored,
  in the record's unit (`A`, `V`, as PatchMaster writes them). PatchMaster stores samples leak-
  subtracted but **not zero-subtracted**; the zero level is reported per channel
  (`extra.zero_offset`) and not subtracted. The scaler can change between sweeps (a gain switch):
  reads use each sweep's own scaler; `extra.scale_varies` flags it; `scale` is the first sweep's.
- Trace `extra`: `group_index`, `group_label`, `series_index`, `series_label`, `series_comment`,
  `method`, `recorded_at` (first sweep's time, the acquisition computer's wall clock, no zone),
  `sweep_starts_s` (sweep times relative to the root record's start time; `start_s` is the first),
  `sweep_sample_counts` (when sweeps differ), `temperature_c`, and on trace 0 `application`
  (`PatchMaster`), `application_version` (the bundle's version text) and `pulsed_tree_version`
  (the root record's version: 9 or 1000).
- Channel `extra`: `trace_record`, `adc_channel`, `recording_mode` (`inside-out`, `on-cell`,
  `outside-out`, `whole-cell`, `current-clamp`, `voltage-clamp`, `no-mode`), `zero_offset`,
  `time_offset_s`, `x_start_s`, `bandwidth_hz`, `linked_dac`, `leak`, `virtual` (a trace PatchMaster computed, such as a lock-in capacitance; its stored samples are read like any other), `imon`, `vmon`, `clipped`,
  `series_resistance_ohm`, `c_slow_f`, `seal_resistance_ohm`, `pipette_resistance_ohm`, `holding`.

## Detection

`DAT1` or `DAT2` at byte 0 followed by four zero bytes (`looks_like_bundle`) → definite. `DATA`
at byte 0 of a `.dat` → likely (a pre-bundle raw-data file; refused on open).

## Bundle header (`BUNDLE_HEADER_LEN` = 256, `parse_bundle` → `Bundle`)

| offset | type | our name |
| --- | --- | --- |
| 0 | char[8] | `signature` (`DAT2`; `DAT1`/`DATA` → refused, exit 6) |
| 8 | char[32] | `version` (`v2x90.2, 22-Nov-2016`) |
| 40 | f64 | `modified` (PatchMaster seconds) |
| 48 | i32 | `item_count` |
| 52 | u8 | `little_endian` (0 → refused) |
| 56 | i32 | 2000 in PatchMaster Next's v2000 layout → refused |
| 64 | 12 × `BundleItem` (`BUNDLE_ITEMS`) | i32 `start`, i32 `length`, char[8] `extension`; `slot`, `end`, `item` |

Item extensions seen: `.dat` (samples), `.pul` (pulsed tree), `.pgf`, `.amp`, `.sol`, `.mrk`,
`.mth`, `.onl`. Only `.pul` and `.dat` are read.

## Tree container (`parse_pulsed` → `PulsedTree`)

`eerT` (little-endian `Tree`; big-endian `Tree` → refused), i32 level count (`PULSED_LEVELS` = 5),
one i32 record size per level, then records top-down, each followed by an i32 child count.
Record sizes come from the file (`record_sizes`); a field past its record's stored size is
absent. Records per tree ≤ `MAX_TREE_RECORDS`, record size ≤ `MAX_RECORD_LEN`, tree ≤
`MAX_PUL_LEN` bytes. Stored sizes in the corpus: v2x60/v2x65 544/128/1408/160/408, v2x90.2
640/144/1408/288/512, v2x90.3 640/144/1728/352/512. `walked_len` is the tree length as walked.

| record | offset | type | our name |
| --- | --- | --- | --- |
| root | 0 | i32 | `root_version` |
| root | 8 | char[32] | `version_name` |
| root | 120 | char[400] | `root_text` |
| root | 520 | f64 | `start_time` |
| group (`GroupRecord`) | 4, 36 | char[32], char[80] | `label`, `text` |
| group | 116 | i32 | `experiment_number` |
| series (`SeriesRecord`) | 4, 36 | char[32], char[80] | `label`, `comment` |
| series | 116 | i32 | `series_count` |
| series | 136 | f64 | `time` |
| series | 312, 872 | char[32], char[80] | `method`, `username` |
| sweep (`SweepRecord`) | 4 | char[32] | `label` |
| sweep | 40, 44 | i32, i32 | `stim_count`, `sweep_count` |
| sweep | 48, 56 | f64, f64 | `time`, `timer` |
| sweep | 96 | f64 | `temperature` |
| trace (`TraceRecord`) | 4 | char[32] | `label` |
| trace | 40, 44 | i32, i32 | `data_offset` (absolute file offset), `points` |
| trace | 64 | u16 | `data_kind` (`kind_bit`: 0 little-endian, 1 leak, 2 virtual, 3 Imon, 4 Vmon, 5 clipped) |
| trace | 68, 70 | u8, u8 | `recording_mode` (`recording_mode_name`), `format_code` (`sample_type`) |
| trace | 72, 80, 88 | f64 × 3 | `scaler`, `time_offset_s`, `zero_offset` |
| trace | 96 | char[8] | `unit` |
| trace | 104, 112 | f64, f64 | `x_interval`, `x_start` |
| trace | 120 | char[8] | `x_unit` |
| trace | 136, 144 | f64, f64 | `y_offset`, `bandwidth_hz` |
| trace | 152, 168, 176, 192 | f64 | `pipette_ohm`, `seal_ohm`, `c_slow_f`, `rs_ohm` |
| trace | 216, 222 | i32, i16 | `linked_dac`, `adc_channel` |
| trace | 292, 296 | i32, i32 | `interleave_size`, `interleave_skip` (bytes; 0 = contiguous) |
| trace | 408 | f64 | `holding` (when the record is long enough) |

`SampleType` (`from_code`, `width`, `dtype`, `is_integer`): 0 `Int16`, 1 `Int32`, 2 `Float32`,
3 `Float64`. Interleaved traces hold `interleave_size` bytes of samples every `interleave_skip`
bytes from `data_offset`.

Times (`heka_time_to_unix`, `heka_time_iso`): HEKA's `TimeFormat.txt` rule (subtract 1580970496,
add 2^32 when negative, shift to 1601 and on to 1970).

## Refused (exit 6, with a hint) — never seen in a development file

`DAT1`/`DATA` files (trees in separate files), big-endian bundles or trees, the v2000 layout,
trees whose root/group/series/sweep/trace records are shorter than 528/120/144/64/128 bytes (fields past a shorter trace record, such as the interleave sizes of PatchMaster 2.1x files, are absent and taken as contiguous storage),
non-zero Y offsets, float traces with a scaler other than 1, integer traces
without a usable scaler, interleave blocks that do not divide into samples. Reads are capped at
`MAX_HEKA_READ` samples per channel.

## `check` finding codes

`truncated` (a bundle item or trace samples outside the file / the `.dat` item; error),
`tree_length_mismatch` (warning), `unsupported_trace` (warning), `bad_interval` (error),
`series_split` (info).

## Observed corpus values

| file | version | groups/series | channels | notes |
| --- | --- | --- | --- | --- |
| `zenodo7530512-2018-12-21c4` | v2x60 | 1 / 7 | Imon-1, Imon-2, Adc2 | whole-cell voltage clamp, 2 amplifiers |
| `zenodo4311847-feb0821c` | v2x65 | 1 / 6 | Vmon-2 / Adc1, Imon | current clamp, long single sweeps (2.5 M samples) |
| `zenodo3827171-w2019-07-08b` | v2x90.2 | 1 / 7 | Vmon-1, Imon-2 | non-zero zero levels (−70 mV) |
| `hekareader-180514s1c1r1` | v2x90.3 | 1 / 3 | Vmon-1 … Vmon-8 | EPC 10 quadro, per-channel time offsets |
| `zenodo4992914-04-11-12-hek-prestin` | 2.11 (2006) | 1 / 16 | Imon; CM (virtual, float32, 4 points per sweep) | lock-in capacitance series split into two parts per series; records 536/128/1120/160/280 bytes |

Contiguous, zero Y offset; int16 except the virtual lock-in traces (float32, scaler 1).

## Vocabulary (every public identifier in the HEKA modules of `openreadout-ephys` must appear here)

| identifier | meaning |
| --- | --- |
| `HekaReader`, `HEKA_FORMAT_ID`, `HekaDataset`, `open`, `open_input`, `tree`, `heka_traces` | entry points |
| `HekaTrace`, `group`, `series`, `channels`, `sweeps`, `interval_s`, `part` | one normalized trace |
| `Bundle`, `BundleItem`, `BUNDLE_HEADER_LEN`, `BUNDLE_ITEMS`, `looks_like_bundle`, `parse_bundle`, `item`, `end`, `slot`, `start`, `length`, `extension`, `signature`, `version`, `modified`, `item_count`, `little_endian`, `items` | bundle header |
| `PulsedTree`, `parse_pulsed`, `PULSED_LEVELS`, `MAX_TREE_RECORDS`, `MAX_RECORD_LEN`, `MAX_PUL_LEN`, `MAX_HEKA_READ`, `root_version`, `version_name`, `root_text`, `start_time`, `record_sizes`, `groups`, `walked_len` | pulsed tree |
| `GroupRecord`, `label`, `text`, `experiment_number` | group record |
| `SeriesRecord`, `comment`, `series_count`, `time`, `method`, `username` | series record |
| `SweepRecord`, `timer`, `stim_count`, `sweep_count`, `temperature`, `traces` | sweep record |
| `TraceRecord`, `data_offset`, `points`, `data_kind`, `kind_bit`, `recording_mode`, `recording_mode_name`, `format_code`, `sample_type`, `scaler`, `time_offset_s`, `zero_offset`, `unit`, `x_interval`, `x_start`, `x_unit`, `y_offset`, `bandwidth_hz`, `linked_dac`, `adc_channel`, `interleave_size`, `interleave_skip`, `rs_ohm`, `c_slow_f`, `seal_ohm`, `pipette_ohm`, `holding` | trace record |
| `SampleType`, `Int16`, `Int32`, `Float32`, `Float64`, `from_code`, `width`, `dtype`, `is_integer` | stored sample types |
| `heka_time_to_unix`, `heka_time_iso` | stored times |
