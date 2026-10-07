# HDF5 (generic) and NWB 2.x

HDF5 is a general container format. Neurodata Without Borders (NWB 2.x) is the open neurophysiology standard built on it. For an NWB file OpenReadout returns the time series as traces, the tables (units, trials, electrodes) and the session and subject metadata, and `export --format nwb` writes electrophysiology traces to NWB. Any other HDF5 file that no specific reader claims gets a structure listing (groups, datasets, attributes) but no images or traces. Derived from the open HDF5 file format as exposed by the pure-Rust `hdf5-pure` crate and the NWB 2 schema (open standard, https://nwb-schema.readthedocs.io), checked on public DANDI files and synthetic files with h5py as the reference reader. Provenance: `docs/provenance/hdf5.md`. Crate `openreadout-hdf5` (modules `generic`, `nwb`, shared `h5util`).

HDF5 is a container: EMD (`openreadout-em`), Imaris (`ims`, `docs/formats/ims.md`) and NWB claim their files definitely; any other file with the HDF5 signature falls to the generic reader (`Likely`, registered after them, before the plate-reader fallback).

## Generic HDF5 (`hdf5`)

- `info --view structure`: a breadth-first walk (`walk`, at most `MAX_NODES` objects, at most file length / 16 of them, 32 group levels and 4096-byte paths, because hard links can make the group graph cyclic): each group (kind `group`, attributes) and dataset (kind `dataset`, `shape`, `dtype`, attributes), children sorted by name. Attribute values become JSON (`attr_json`: numbers stay numbers, long arrays and texts are cut).
- `info`: no images or traces; notes give the object counts and the top-level names; `format_version` = superblock version.
- `check`: the tree parses; the file is not shorter than the superblock's end-of-file address (`truncated`); `listing_capped` when the walk stopped early.
- Reading planes exits 6.

## NWB 2.x (`nwb`)

Detection: HDF5 signature and the `.nwb` extension, or the text `NWBFile` (the root's `neurodata_type`) in the first 64 KiB. The root attribute `neurodata_type` must be `NWBFile` (NWB 1.x exits 6). `format_version` = root `nwb_version`.

| NWB | our field |
| --- | --- |
| `session_description`, `identifier`, `session_start_time`, `timestamps_reference_time`, `file_create_date` | `vendor.session`; description and start in `notes`; `session_start_time` in each trace's `extra` |
| `general/{experimenter, experiment_description, session_id, institution, lab, keywords, related_publications, protocol, notes, stimulus}`, `general/subject/{subject_id, species, sex, age, genotype, strain, description}` | `vendor.session` (lab, institution, experimenter, session id also in `notes`) |
| group under `acquisition/` or `processing/` (at any depth, so series inside processing modules and containers such as `LFP` or `BehavioralEvents`) whose `neurodata_type` is `TimeSeries`, `ElectricalSeries` or `SpatialSeries` (`TRACE_TYPES`) | one trace (`NwbSeries`, `neurodata_type`), in breadth-first order with names sorted; `extra.neurodata_type`, `extra.path` |
| `data` (rank 1: one channel; rank 2: one channel per column, named `<series>[i]`) with attributes `unit`, `conversion`, `offset` | channel `unit` (`n/a` dropped), `dtype`, `scale` = conversion (× channel conversion), `offset`; values returned as `data × conversion (× channel_conversion[i]) + offset` |
| `ElectricalSeries/electrodes` (a region: row indices into `/general/extracellular_ephys/electrodes`, `ELECTRODES_PATH`) | channel `extra.electrode_row` (`electrode_rows`) and `extra.electrode`: that row's `id` and scalar columns (text decoded) |
| `ElectricalSeries/channel_conversion` (one factor per channel) | `channel_conversion`, channel `extra.channel_conversion`, folded into `scale` |
| `starting_time` (value) with attribute `rate` | `sample_rate_hz` = rate, `start_s` = starting_time (`extra.time_base` = `starting_time + rate`) |
| `timestamps`, uniformly spaced (within 1e-6 relative) | `sample_rate_hz` = 1 / interval, `start_s` = first (`uniform timestamps`) |
| `timestamps`, irregular | `sample_rate_hz` = 0, a leading `time` channel (s) holds the timestamps, `extra.irregular_sampling` |
| group under `acquisition/`, `processing/` or `stimulus/presentation/` whose `neurodata_type` is `PatchClampSeries`, `CurrentClampSeries`, `IZeroClampSeries`, `CurrentClampStimulusSeries`, `VoltageClampSeries` or `VoltageClampStimulusSeries` (`ICEPHYS_TYPES`) | one trace, read as above (one sweep; the intracellular recording tables are read as tables, not used to group series into sweeps); attribute `sweep_number` → `extra.sweep_number`; `extra.icephys` (`icephys`) holds `sweep_number`, `stimulus_description` and each scalar clamp setting present (`ICEPHYS_SETTINGS`: `gain`, `bias_current`, `bridge_balance`, `capacitance_compensation`, `capacitance_fast`, `capacitance_slow`, `resistance_comp_bandwidth`, `resistance_comp_correction`, `resistance_comp_prediction`, `whole_cell_capacitance_comp`, `whole_cell_series_resistance_comp`) as `{value, unit}` (or a bare number without a `unit` attribute) |
| float32 `conversion`, `offset`, `rate` attributes and float32 clamp settings | taken at their shortest decimal (MIES stores `conversion` 0.001 as float32: the factor is 0.001, not 0.0010000000475); pynwb's float32 product agrees to float32 precision |
| every other `neurodata_type` group (ImageSeries, optical-physiology series, ProcessingModule and container groups, ElectrodeGroup, Device, …) and other series outside `acquisition/`/`processing/` | `info --view structure` kind `object` and an `info` note with counts; not decoded |

### Tables (`nwb_tables.rs`: `NwbTable` { `path`, `kind`, `info` }, `NwbTableKind`, `NwbColumn`)

In breadth-first group order (after the traces are chosen), each table's `name` is its HDF5 path without the leading `/`, `extra.path`, `extra.neurodata_type`, `extra.description`:

| NWB | table |
| --- | --- |
| any group with a `colnames` attribute and an `id` dataset (`table_rows`): `DynamicTable`, `Units`, `TimeIntervals` (trials, epochs), the electrodes table, … (`describe_dynamic`, `NwbTableKind::Dynamic`) | one row per `id`; columns `id`, then the `colnames` order: rank-1 numeric or boolean `VectorData` as numbers (`NwbColumn::Numeric`; booleans 0/1, dtype `uint8`); rank-2 numeric up to 64 wide as `<name>[k]` (`Matrix`, `k`, `width`); text as `uint32` codes into `extra.categories` in first-appearance order (the plate-reader `well` convention) (`Categorical`, `categories`; at most `MAX_CATEGORIES` distinct values over at most `MAX_TEXT_ROWS` rows; CSV export writes the text); a ragged column `x` with `x_index` as `x_count`, the values per row (`RaggedCount`, `index`); object references and other types are listed in `extra.skipped_columns` |
| a units table's `spike_times` + `spike_times_index` (`NwbTableKind::SpikeTimes`: `data`, `ends`, `ids`) | a second table `<units>/spike_times`, right after the units table: `unit_row`, `unit_id`, `spike_time` (s), one row per spike |
| `SpikeEventSeries` anywhere (`describe_spike_events`, `NwbTableKind::SpikeEvents`: `data`, `timestamps`, `width`, `conversion`, `offset`) | one row per event: `time_s`, then `value` (rank-1 data), `w<s>` (rank 2: event × sample) or `c<c>_w<s>` (rank 3: event × channel × sample), each × conversion + offset; `extra.electrode_rows`; at most `MAX_EXPANDED_COLUMNS` values per event |

A read returns at most `MAX_TABLE_ROWS` rows, or more for narrow tables within `MAX_TABLE_VALUES` values.

`check`: required session fields present (`session`, warning), timestamps as long as the data (`timestamps`, error), a time base (`time_base`, warning), the last sample readable (`data`, error), truncation (`truncated`), electrode rows inside the electrodes table and one per channel (`electrodes`), every table's last row readable (`table`, error), neurodata objects that could not be described (`unreadable`, error, the list `unreadable`).

## Writing NWB (`export --format nwb`; `export_nwb`, `nwb_write.rs`)

The traces of an electrophysiology file become plain `TimeSeries` under `/acquisition/` of an NWB 2.x file (`nwb_version` `2.7.0`), written with `hdf5-pure` (superblock v2/v3, latest-format object headers):

| NWB | from |
| --- | --- |
| root attributes `neurodata_type` = `NWBFile`, `namespace` = `core`, `nwb_version`, `object_id` (a UUID) | fixed; UUIDs are version-4 layout from an xxh3 hash of the source, time and process |
| `identifier` | a UUID (the report's `identifier`) |
| `session_description` | `<format name> recording <file name> (<n> trace(s)), exported by openreadout <version>` |
| `session_start_time`, `timestamps_reference_time` | the experiment model's acquisition start, else a trace's `extra.acquired_at`/`start_time`; a time without a zone is written with `+00:00` and the report says so; without any, the source file's modification time (noted) |
| `file_create_date` | the time of the export |
| `general/experimenter` | the experiment model's operator, else a trace's `extra.operator` |
| `general/experiment_description` | `Method: <name> — <technique>` from the experiment model, when known |
| `general/session_id` | the experiment model's sample id or name, when known |
| `general/notes` | the source file name and format, and the source's `info --json` (with `experiment`) |
| `general/source_script` (attribute `file_name`) | `openreadout <version>` |
| `analysis/`, `processing/`, `stimulus/presentation/`, `stimulus/templates/` | empty, as the schema requires |
| one `TimeSeries` group per trace, sweep and unit | channels sharing a unit share a series; name `<trace name>[_sweepS][_<unit>]` (`trace<T>` prefix when several traces are written) |
| `data` float64, shape `[samples]` or `[samples, channels]`, chunked (≤ 1 MiB and < 65,536 per dimension) with deflate | the channels' physical values (`read_trace`); attributes `unit`, `conversion` 1, `offset` 0, `resolution` −1, `continuity` `continuous` |
| `starting_time` (scalar float64) with `rate` and `unit` = `seconds` | `start_s` + first sample / rate, and `sample_rate_hz` |
| group attributes `description`, `comments` | the channel names and units in column order; JSON with source, trace, sweep, first sample and the channels' metadata |

Not written: `ElectricalSeries`/`PatchClampSeries` (they need electrodes/devices tables the sources do not describe completely), `general/subject` (instrument files record no subject; add one with pynwb before a DANDI upload), cached namespaces under `specifications/`. Traces without a sample rate, and NMR, chromatography and mass-spectrometry files, are refused (exit 6). At most 2^26 values (512 MiB of float64) per export; narrow larger recordings with `--trace`, `--sweep` and `--rows`.

Verification: the file is re-opened with `NwbDataset`: every series present with its samples, channels, rate (bit for bit) and unit, and an xxh3 digest of all values equal to the written one; then renamed into place. Third party: `oracle/nwb_validate.py` reads it with `pynwb.NWBHDF5IO`, compares samples with `openreadout trace`, and runs `nwbinspector` (no CRITICAL or PYNWB_VALIDATION messages other than the missing subject).

## Observed corpus values

| id | content |
| --- | --- |
| `dandi000126-sub-1` | NWB 2.3.0, one int32 TimeSeries (3 samples, uniform timestamps), Subject |
| `dandi000027-sub-rat123` | NWB 2.0b, no acquisition data (session fields only) |
| `dandi000006-anm372907-20170613` | NWB 2.0.2, two irregular lick-time TimeSeries inside BehavioralEvents; units (1 unit, 1,284 spike times, a ragged `electrodes` column), trials (139 rows, text columns), 64-row electrodes table |
| `dandi000059-ms10-170314` | NWB 2.2.5, three behaviour TimeSeries and a 3-column SpatialSeries under `processing/behavior`; trials; 47-row electrodes table with a boolean column |
| `dandi000067-ee-044` | NWB 2.2.5, a 104-channel int16 ElectricalSeries (20 kHz, conversion 1e-6) in `acquisition/` and a 1250 Hz LFP ElectricalSeries in `processing/ecephys/LFP`; 104-row electrodes table |
| `dandi000221-hi198-060619` | NWB 2.4.0 (variable-length ASCII attributes), a rank-1 SpikeEventSeries in `analysis/`, units with a ragged `trialsID`, trials (16 columns), a one-row electrodes table |
| `dandi000034-mouse412804-155542` | NWB 2.2.5, 258 units with 1,190,752 spike times |
| `synthetic-ecephys.nwb` (`crates/openreadout-hdf5/tests/fixtures`) | electrodes table (numeric, boolean, text, reference columns), ElectricalSeries with a region and channel conversion, LFP in a processing module, units (ragged spike times and electrodes, 2-D waveform mean, text), a 3-D SpikeEventSeries, trials |
| `synthetic-timeseries.nwb`, `synthetic-generic.h5` (`crates/openreadout-hdf5/tests/fixtures`) | starting_time + rate with 2-D int16 data, conversion and offset; uniform and irregular timestamps; a plain HDF5 tree |

## Vocabulary (every public identifier in `openreadout-hdf5/src/{nwb,nwb_tables,nwb_write,generic,h5util,lib}.rs` must appear here or in `ims.md`)

| identifier | meaning |
| --- | --- |
| `Hdf5Reader`, `Hdf5Dataset`, `HDF5_FORMAT_ID`, `MAX_NODES`, `nodes`, `truncated_listing` | generic reader, opened file, the id `hdf5`, walk cap, walked objects, whether the walk stopped early |
| `NwbReader`, `NwbDataset`, `NWB_FORMAT_ID`, `version`, `session`, `series`, `others`, `tables`, `electrodes`, `unreadable` | NWB reader, opened file, the id `nwb`, `nwb_version`, session fields, series read as traces, other objects, tables, electrodes-table rows as JSON, objects that could not be described |
| `NwbSeries`, `path`, `samples`, `columns`, `dtype`, `unit`, `conversion`, `offset`, `starting_time`, `rate`, `has_timestamps`, `uniform`, `description`, `neurodata_type`, `channel_conversion`, `electrode_rows`, `icephys` | one series read as a trace |
| `ICEPHYS_TYPES`, `ICEPHYS_SETTINGS` | patch-clamp series types read as traces; their scalar clamp settings |
| `TRACE_TYPES`, `ELECTRODES_PATH`, `read_f64_rows` | series types read as traces; the electrodes table path; numeric rows of a dataset as f64 |
| `NwbTable`, `kind`, `info`, `NwbTableKind` { `Dynamic`, `SpikeTimes`, `SpikeEvents` }, `ends`, `ids`, `timestamps`, `width`, `NwbColumn` { `Numeric`, `Matrix`, `Categorical`, `RaggedCount` }, `k`, `categories`, `index`, `data`, `read`, `describe_dynamic`, `describe_spike_events`, `table_rows`, `MAX_CATEGORIES`, `MAX_TEXT_ROWS`, `MAX_EXPANDED_COLUMNS`, `MAX_TABLE_ROWS`, `MAX_TABLE_VALUES` | NWB tables |
| `open` | open a file |
| `HDF5_SIGNATURE`, `looks_like_hdf5`, `open_h5` | detection and streaming open |
| `attr_text`, `attr_json`, `attrs_json`, `attrs_text` | attribute conversion (text joins arrays of one-character strings) |
| `pixel_type`, `dtype_name`, `big_endian`, `swap_samples` | sample types and byte order |
| `H5Node`, `walk`, `walk_limit`, `is_group`, `shape`, `attributes` | the tree walk, its node limit for a file (file length / 16, at least 1024) and its nodes |
| `export_nwb`, `default_nwb_output`, `NWB_VERSION` | write traces as NWB 2.x (verified by re-reading); default output `<stem>[.traceT][.sweepS].nwb`; the `nwb_version` written |
| `NwbExportOptions`, `rows`, `overwrite` | what to write: trace, sweep, samples `[first, last]` of each sweep; replace an existing file |
| `NwbExportReport`, `input`, `output`, `nwb_version`, `identifier`, `session_start_time`, `samples_written`, `bytes_written`, `verified`, `notes` | the export report |
| `NwbSeriesReport`, `name`, `trace`, `sweep`, `channels`, `channel_indices`, `rate_hz`, `starting_time_s` | one written TimeSeries (`samples`, `unit` as above) |

### Byte-source integration

The existing layouts are read through `Input`/`Fs`/`SourceFile`, including directory
sessions and companions where applicable. Path, memory and callback namespaces use
the same parser and scaling. See [byte sources](../../book/src/guides/wasm.md).
