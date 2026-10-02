# Provenance log — generic HDF5 (`hdf5`) and NWB 2.x (`nwb`)

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

## 2026-09-22 — readers (Richard Zimring with Claude as assistant)

Both are open formats; nothing was reverse engineered.

**Specifications read:** the NWB 2 core schema documentation, https://nwb-schema.readthedocs.io/ (open standard; `NWBFile` root with `nwb_version`, `session_description`, `identifier`, `session_start_time`, `timestamps_reference_time`, `file_create_date`, `general/…`, `acquisition/`; `TimeSeries` with `data` (`unit`, `conversion`, `offset`, `resolution`), `starting_time` (`rate`) or `timestamps`). HDF5 itself is parsed by `hdf5-pure` 0.47 (MIT OR Apache-2.0).

**Oracle:** h5py 3.16 (BSD-3-Clause) in `oracle/gen.py` (`nwb`): TimeSeries under `acquisition/` in breadth-first sorted order, `data × conversion + offset` per column, uniform-timestamp test within 1e-6 relative, irregular timestamps as a leading time channel.

**Corpus files used** (DANDI Archive, https://dandiarchive.org; licence as recorded by DANDI for the dandiset): `dandi000126-sub-1` (000126 "NWB API Test Data", CC0-1.0), `dandi000027-sub-rat123` (000027, CC-BY-4.0), `dandi000006-anm372907-20170613` (000006, CC-BY-4.0, Economo & Svoboda), `dandi000059-ms10-170314` (000059, CC-BY-4.0, Petersen & Buzsáki). Human-subject dandisets were not used. Synthetic files `synthetic-timeseries.nwb` and `synthetic-generic.h5` were written with h5py (`crates/openreadout-hdf5/tests/fixtures/make_fixtures.py`).

**Observed:** NWB versions 2.0b, 2.0.2, 2.2.5, 2.3.0; `session_start_time` as ISO-8601 text with an offset; `file_create_date` as a 1-element string array; TimeSeries nested inside `BehavioralEvents` containers under `acquisition/`; behaviour series under `processing/behavior`.

**Inferred (ours):** which objects become traces (plain `TimeSeries` under `acquisition/` only); naming of 2-D columns `<series>[i]`; uniform timestamps treated as a regular sample rate.

## 2026-09-22 — robustness (fuzzing)

**Scope:** traversal bounds only; what is listed from a well-formed file is unchanged. The `whole_nwb` and `whole_hdf5` fuzz targets (3 min each; seeds cut from `dandi000027-sub-RAT123.nwb`, CC-BY-4.0) found group graphs made cyclic by hard links (a group linking to itself): the breadth-first walk followed the cycle, building ever longer paths (out of memory) and re-reading object headers for up to 200,000 visits of an 18 KB file (53 s). The walk now opens each child from its parent's handle, stops at 32 group levels and 4096-byte paths, and visits at most as many objects as the file could hold (file length / 16, at least 1024); stopping early is reported as `listing_capped`. Fixtures: `crates/openreadout-hdf5/tests/fixtures/malformed/file-fuzz-group-cycle-*`. **Prior art consulted:** none.

## 2026-09-23 — NWB writer (`export --to nwb`) (Richard Zimring with Claude as assistant)

**Specifications read:** the NWB 2 core schema documentation (https://nwb-schema.readthedocs.io/, open standard): the required `NWBFile` datasets and groups (`identifier`, `session_description`, `session_start_time` and `timestamps_reference_time` as ISO-8601 text, `file_create_date` as a text array, `acquisition`, `analysis`, `processing`, `stimulus/presentation`, `stimulus/templates`, `general`), the `neurodata_type`/`namespace`/`object_id` attributes, `TimeSeries` (`data` with `unit`, `conversion`, `offset`, `resolution`, `continuity`; `starting_time` with `rate` and `unit`; `description`, `comments`), `general/{experimenter, experiment_description, session_id, notes, source_script}`.

**Library read:** `hdf5-pure` 0.47 (MIT OR Apache-2.0) documentation and `chunked_write.rs`: it encodes chunk dimensions above 65,535 in 4 bytes, where the HDF5 library computes the minimal width (3 bytes for 65,536–16,777,215) and refuses the dataset ("stored chunk dimension encoding length does not match value calculated from chunk dimensions", seen with h5py 3.16); the writer keeps every chunk dimension below 65,536.

**Validators:** pynwb, hdmf and nwbinspector (all BSD-3), run on the written files.

**Validation** (`oracle/nwb_validate.py`: pynwb 4.2 read, samples compared with `openreadout trace`, nwbinspector 0.7): `pyabf-171116sh-0011`, `pyabf-05210017-vc-abf1`, `pyabf-2020-07-29-0062`, `pyabf-14o16001-vc-pair-step`, `pyabf-18425108`, `nlx-cheetah-v5-7-4-csc1`, `nlx-cheetah-v5-5-1-tet3a`, `brk-2-1-l101210-001.ns2`, `brk-pause-correct.ns2`, `intan-rhd-test-1`, `intan-rhs-test-1`, `sglx-digitalchanneltest-g0-t0.nidq`, `sglx-5-19-2022-ci1-g0-t0.imec0.lf` (rows 0–9999). Every file read and matched; nwbinspector's CRITICAL `check_subject_exists` (no subject in instrument files) is expected, and `check_data_orientation` fires only for the SpikeGLX LF test file, which holds fewer samples (26) than channels (384).

**Inferred (ours):** one TimeSeries per trace, sweep and unit (NWB `data` has one unit); float64 physical values rather than raw integers with per-channel `conversion` (channels sharing a series may have different scales); a session start without a zone written as `+00:00` and reported.

## 2026-09-23 — NWB depth: extracellular series, tables, processing modules (Richard Zimring with Claude as assistant)

**Specifications read:** NWB 2 core schema documentation (https://nwb-schema.readthedocs.io/en/stable/format.html, open standard; schema BSD-3, https://github.com/NeurodataWithoutBorders/nwb-schema) and HDMF common (https://hdmf-common-schema.readthedocs.io, BSD-3): `ElectricalSeries` (`data` [time × channel], `electrodes` as a `DynamicTableRegion` of row indices into `/general/extracellular_ephys/electrodes`, optional `channel_conversion` multiplied after `conversion`, then `offset`); `SpatialSeries` (a `TimeSeries` with an optional `reference_frame`); `SpikeEventSeries` (`data` [event × channel × sample] or [event × sample], `timestamps` required); `ProcessingModule` (a container; its series keep their own encoding); `DynamicTable` (`colnames` attribute, `id` `ElementIdentifiers`, `VectorData` columns, a ragged column `x` indexed by `x_index` whose values are the exclusive end offsets of each row), `Units` (`spike_times` + `spike_times_index`, optional `electrodes` region, `waveform_mean`, …), `TimeIntervals` (`start_time`, `stop_time`, …).

**Oracles:** pynwb and h5py (BSD-3), run as black boxes.

**Corpus files used** (DANDI, non-human dandisets; licences from each dandiset's metadata): `dandi000006-anm372907-20170613` (CC-BY-4.0: 64-row electrodes table with string columns, one unit, trials), `dandi000059-ms10-170314` (CC-BY-4.0: processing-module `TimeSeries` and a 3-column `SpatialSeries`, 47 electrodes with a boolean column), and new: `dandi000067-ee-044` (CC-BY-4.0, rat: a 104-channel int16 `ElectricalSeries` in `acquisition/` and an LFP `ElectricalSeries` in `processing/ecephys/LFP`, conversion 1e-6 V), `dandi000221-hi198-060619` (CC-BY-4.0, mouse: a `SpikeEventSeries` in `analysis/`, a units table with a numeric extra column, a one-row electrodes table with a float column), `dandi000034-mouse412804-155542` (CC-BY-4.0, mouse: 258 units, 1,190,752 spike times). Their structure was listed with h5py before any code was written. No human-subject dandiset was accessed (the DANDI search excluded dandisets whose species list names humans).

**Our normalization (Inferred):** traces are every `TimeSeries`, `ElectricalSeries` and `SpatialSeries` under `acquisition/` and `processing/` (breadth-first, names sorted), values = raw × conversion × channel conversion + offset; an `ElectricalSeries` channel carries its electrode row, id and the electrode's scalar table values. Every `DynamicTable` (electrodes, units, trials and other interval tables, any other table) is a table: `id` first, then the `colnames` order; numeric and boolean columns as numbers, text columns as category codes (`extra.categories`, first-appearance order), ragged columns as a `<name>_count` column, 2-D numeric columns as `<name>[k]` (up to 64), object references left out (`extra.skipped_columns`). A units table's `spike_times` also become a table of their own (`unit_row`, `unit_id`, `spike_time`). A `SpikeEventSeries` anywhere is a table: `time_s`, then its waveform samples × conversion + offset.

**Found while doing this:** `hdf5-pure` returns variable-length ASCII attributes (what h5py 3 writes for NWB 2.4+ files such as `dandi000221-hi198-060619`) as a separate attribute variant that `attr_text` did not handle, so such files were refused as "not an NWBFile"; `attr_text` now reads them.

**Validation:** h5py (`nwb()` in `oracle/gen.py`, rewritten to follow the rules above) is the bit-exact oracle for every trace channel and every table column of the seven corpus NWB files and the synthetic fixture. pynwb 4.2.0 (BSD-3, run only) cross-checks the same files through its own object model (`oracle/nwb_depth_validate.py`: shapes, rates, electrode rows, first samples with conversion and channel conversion, every table column value by value with text decoded, per-unit spike counts and spike times, SpikeEventSeries values): 20 objects in 6 files, no difference.

## 2026-09-23 — byte-source merge integration

Adapted existing reader and session discovery to `Input`/`Fs`/`SourceFile`, preserving
parsing layouts and scaling. Evidence: existing manifest corpus ids for this format
and committed oracle JSON; source equivalence is checked by `tests/sources.rs`.
Prior art consulted: this repository (MIT OR Apache-2.0) only; no external sources or new
format-layout inferences. No corpus files changed or added.

## 2026-09-25 — assurance profile (`src/assurance.rs`)

**Corpus files:** every development input of this format on disk (`corpus/assurance/evidence.json` lists them); no held-out file.
**Prior art consulted:** none.
**What was done.** No parsing logic changed. The reader declares an assurance profile (docs/assurance.md) that fingerprints each file from this reader's own normalized output: the generic dialect only. The table of validated feature values and the confidence level are generated from the development corpus by `cargo xtask assurance-audit`.

## 2026-09-26 — NWB intracellular (patch-clamp) series as traces

**Corpus files:** dandi000117-sub-20210511003-0019-icephys (NWB 2.3.0, IZeroClampSeries), dandi000035-sub-mouse-zudob-sample-27-icephys (NWB 2.1.0 patch-seq: CurrentClampSeries, IZeroClampSeries, CurrentClampStimulusSeries under stimulus/presentation), dandi000968-sub-na-icephys (NWB 2.6.0 with the intracellular recording tables), dandi000043-sub-m19-01-001-ses-20190228t222559-icephys (NWB 2.2.4 written by MIES). No held-out file (DANDI 000292/000293/000297 were not opened).
**Prior art consulted:** the NWB core schema (nwb-schema, BSD-3, https://github.com/NeurodataWithoutBorders/nwb-schema, `core/nwb.icephys.yaml`) for the PatchClampSeries family, `sweep_number`, `stimulus_description` and the clamp-setting datasets; pynwb 4.2.0 (BSD-3) run as a second opinion (`get_data_in_units()`).
**What was inferred from what.** Patch-clamp series are TimeSeries (the schema), so the existing TimeSeries reading applies unchanged; they also live under `stimulus/presentation/` (the schema and the patch-seq and MIES files), so that path is accepted for them only. MIES writes `conversion` as a float32 0.001; the h5py float64 product differs from the writer's intent by 5e-8 relative while pynwb (float32 arithmetic) and our shortest-decimal reading agree, so float32 scalars are taken at their shortest decimal, in the reader and in the oracle alike. Oracles: h5py values for all 163 series; pynwb agrees on every series (to float32 precision).

## 2026-09-26 — NWB session facts in the normalized experiment (second opinions)

**Corpus files:** the seven development NWB inputs (`dandi000006-anm372907-20170613`, `dandi000027-sub-rat123`, `dandi000034-mouse412804-155542`, `dandi000059-ms10-170314`, `dandi000067-ee-044`, `dandi000126-sub-1`, `dandi000221-hi198-060619`). **Prior art consulted:** the NWB schema documentation (nwb-schema, BSD-3, public) for the `NWBFile` fields; pynwb (BSD-3) run as the second reader.
**What was found.** The second-opinion harness (`oracle/second_fields/ephys.py`, pynwb) reported gaps on all seven: the session start (with its zone), experimenter, session description and subject id that pynwb reads were only in `notes` and in each trace's `extra.session_start_time`, not in `experiment`. No value was wrong.
**Changed (mapping only; no parsing logic).** The NWB dataset now gives `experiment.acquisition.started_at` = `session_start_time` (as written, zone included), `acquisition.operator` = `general/experimenter`, `acquisition.comment` = `session_description` and `sample.id` = `general/subject/subject_id`, each with provenance `spec`.
