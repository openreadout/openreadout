# Sciex `.wiff` + `.wiff.scan`

Sciex Analyst writes an acquisition as a `.wiff` file (method, samples, scan index) and a `.wiff.scan` file (the scan data). OpenReadout returns MRM data and TOF data-dependent acquisitions as spectra, plus TIC, BPC and LC device traces. Q1/Q3 scans, enhanced product-ion, MRM³, SWATH and precursor/neutral-loss scans are not decoded (refused). Confidence: low.

Derived from public MetaboLights files (QTRAP 6500 and 6500+ MRM, TripleTOF 6600 DDA, an older ten-sample MRM file) by hex dump and comparison with the depositors' own conversions and with ProteoWizard conversions made with the vendor library. The container is an OLE2 compound file read with `openreadout-core::cfb`, written from Microsoft's public [MS-CFB] specification. No Sciex library, document or converter source was used. Details: `docs/provenance/sciex-wiff.md`. Format id: `sciex-wiff`; crate `openreadout-sciex`.

**`.wiff2` is not read.** SCIEX OS `.wiff2` files are encrypted; the reader recognises the extension (detection `definite`, with a note) and `open` returns exit 6 with a hint to export mzML from the vendor software. No attempt is made to read their content (`docs/legal/clean-room-policy.md`, rule 4).

A legacy acquisition is two files with one stem: `<name>.wiff` (method, sample information, scan index) and `<name>.wiff.scan` (the scan data). The reader accepts either path (`wiff_path`); each file finds the other by name, case-insensitively. Without the `.wiff.scan`, `info`, `info --view structure` and `vendor` work (metadata and the index), spectra do not — unless the file uses the **single-file layout** of older Analyst QS files (QSTAR): each sample's scans are then in a `SampleSubtree/SampleN/Scan` stream inside the `.wiff`, a 32-byte stream preamble followed by the same scan data as a `.wiff.scan` holds after its 44-byte header; index offsets count from the end of that preamble (`STREAM_PREAMBLE`). Such a file needs no companion (note `single-file layout`; assurance layout feature `single file (Scan stream)`). Detection: extension `.wiff` plus the compound-file signature (`definite`), `.wiff` alone (`extension only`), `.wiff.scan` (`likely`, opens the `.wiff` beside it), `.wiff2` (`definite`, refused).

All integers and floats little-endian. Every stream below starts with a 32-byte preamble (`STREAM_PREAMBLE`: u32 0, u32 version 4 or 5, 24 zero bytes).

## Streams of the `.wiff`

| stream | layout | our names | status |
| --- | --- | --- | --- |
| `SampleSubtree/SampleN/Idx` | after the preamble, 54-byte records (`INDEX_RECORD`), one per scan: u32 offset and u32 byte length of the scan's data in `.wiff.scan`, counted from byte 0x2C of that file (`SCAN_FILE_HEADER`); f64 time in ms; u16 (6); f64 total ion current; f64 base-peak intensity; f64 (0); f64 base-peak position (a TDC bin in TOF data); u32 (0) | `IndexRecord`, `offset`, `byte_len`, `time_ms`, `tic`, `base_peak_intensity`, `base_peak_x`, `parse_index`, `index_trailing` | validated: times and TIC equal the exports' |
| `MethodSubtree/Method1/DeviceMethod0/Period0/ExperimentK/ExperimentHeader` | u16 at 0x56 = polarity (1 negative, 0 positive); u16 at 0x7A = scan type (4 MRM, 8 TOF MS, 9 TOF product ion); u32 at 0xB0 = number of mass ranges | `ExperimentHeader`, `scan_type`, `polarity`, `range_count`, `parse_experiment_header`, `SCAN_TYPE_MRM`, `SCAN_TYPE_TOF_MS`, `SCAN_TYPE_TOF_PRODUCT` | inferred (polarity by comparing a negative- and a positive-mode file of one study) |
| `…/ExperimentK/MassRangeEx/MassRangeEx` (older files: `…/MassRange/MassRange`, u32 1 instead of 2 and the dwell time in ms in place of the expected retention time, kept as parameter `dwell_ms`) | u32 2, u32 parameters per range, then per transition / mass range: f32 first mass (MRM Q1), u32, f32 second mass (MRM Q3), f32 expected retention time in minutes (scheduled MRM), u32, a u16 byte length + UTF-16 compound name, then per parameter a u16 byte length + UTF-16 key (`DP`, `EP`, `CE`, `CXP`, `CES`, `IRD`, …), f32 value, f32 value, u32 | `MassRange`, `first_mz`, `second_mz`, `expected_rt_min`, `name`, `parameters`, `parameter`, `parse_mass_ranges` | Q1/Q3: validated (the exports' targets); the rest inferred |
| `SampleSubtree/SampleN/SampleDAM/sMRMPro_adw1/sMRMPro_adw_Times` | (u32 start ms, u32 end ms) per transition: the scheduled window | `parse_windows`, `windows` | validated (see below) |
| `SampleSubtree/SampleN/TDCInfo` | f64 at 0x20: the TDC bin width in ns (0.025) | `tdc_width_ns` | validated via the calibration |
| `SampleSubtree/SampleN/TOFCalibrationData` | default f64 a, f64 t₀ at 0x20; u32 0; u32 number of index records; from 0x38 (`TOF_CAL_FIRST`) one 20-byte record (`TOF_CAL_RECORD`) per index record: f64 a, f64 t₀, u32 | `tof_calibration`, `tof_default_calibration`, `tof_mz` | validated: base-peak m/z equal the exports' to 1e-12 |
| `SampleSubtree/SampleN/DDERealTimeData` | one 32-byte slot (`PRECURSOR_SLOT`) per cycle and product-ion experiment, from the preamble on: f64 precursor m/z, f64, f64 (an integer; kept as `precursor_slot_value`), f64 | `precursor_slot` | validated: every precursor equals the export's |
| `SampleSubtree/SampleN/Log` | UTF-16 text; `key: value` fields separated by commas: `Component ID` (the model), `Serial Number`, `Manufacturer`, `Model`, `Firmware Version`, … | `log`, `log_fields`, `utf16_runs` | read |
| `SampleSubtree/SampleN/SampleDABE/DATA` | after the preamble (its last u32 is 4 or 6 here) a u32 100, then u16-byte-length UTF-16 strings: sample name, sample id, comment, data file, acquisition method, … | `strings`, `sample_strings` | read |
| `SampleSubtree/SampleTable` | u32 Unix time at 0x3E: the acquisition start (sample 1) | `started_at` | validated against the export's `startTimeStamp` |
| `SampleSubtree/SampleN/Devices/Device_K/Channel`, `DevData` | LC devices recorded with the sample: `Channel` after the preamble u32 1, u16 channel count, then per channel u32 1, u16-length UTF-16 name, u16-length UTF-16 unit (`(psi)`), f64 samples per second, u16 channel index, f32, f32, i32 (`DeviceChannel`, `rate_hz`, `unit`, `parse_device_channels`); `DevData` after the preamble one f64 per channel per sample (`parse_device_data`) | validated: every value of six channels equals the export's |
| `FileRec_Str` | UTF-16 text naming the software (`Analyst 1.7.2`, `Analyst TF 1.8.1`) | | read |
| `MethodSubtree/Method1/AcqMethodFileInfoStm` | UTF-16 path of the acquisition method (`.dam`) | | read |
| everything else | listed by `info --view structure` with its size; `vendor` lists every stream | | – |

## `.wiff.scan`

A 0x2C-byte header, then each scan's data where the index points.

- **MRM** (scan type 4; `expand_zero_runs`): f32 values in which a negative value −(n + 0.01) stands for n zeros. A cycle expands to 2 × (number of transitions) values whose second half holds the transitions in method order (QTRAP 6500/6500+; the first half is zero in every cycle of the corpus files and is not interpreted), or (older Analyst files) to exactly one value per transition. Any other width is an error.
- **TOF** (scan types 8 and 9; `decode_tdc`): a byte stream of the time-to-digital histogram in steps of `tdc_step` TDC bins (u32 at 0x38 of the experiment's `ExperimentHeaderEx`: 4 in the TOF experiments of the corpus). `FF FF FF FF` + u32 sets the TDC bin; a byte 0x00–0x7B is a count (then the next step); 0x7C, 0x7D, 0x7E are followed by a 1-, 2- or 4-byte count; 0x80–0xFB skip (byte − 0x80) empty steps; 0xFC, 0xFD, 0xFE are followed by a 1-, 2- or 4-byte number of steps to skip; 0xFF bytes running to the end of a scan end it; 0x7F is not known and is an error. Validated on every scan of the corpus TOF files: the counts sum exactly to the index TIC, and the index's base-peak position is the decoded profile's first maximum. m/z of TDC bin b: (a·(width·b − t₀))² with the scan's (a, t₀) and the TDC width.

## Mapping

- One run per sample (`Sample`, `number`; `spectra[k].index` = sample − 1). Index record *i* belongs to cycle ⌊i / E⌋ + 1 and experiment (i mod E) + 1 of the E experiments of period 0 (`Experiment`, `number`, `header`, `ranges`); records of length 0 (an experiment that recorded nothing in that cycle) give no spectrum.
- **MRM:** one spectrum per cycle and precursor (Q1): the transitions scheduled at the cycle's time (window start ≤ t ≤ end; without windows, all), m/z = Q3, sorted; `precursor_mz` = Q1; `collision_energy` = the transitions' `CE` when they share one (else per transition in `extra.collision_energies`); `activation` `CID`; `ms_level` 2; `centroided`; `native_id` `sample=S period=1 cycle=C experiment=E transition=K` (K the group's first transition); `extra`: `cycle`, `experiment`, `scan_type`, `index_record`, `transitions`, `compounds`. **Scheduling exception** (inferred from one file, 17 of 165 windows): a scan at a window's start or end time whose whole cycle is zero (TIC 0) — written while the instrument switches transitions — is left out of the transitions starting or ending there (not the first scan of the run).
- **TOF:** one spectrum per non-empty record: the histogram as a profile (non-empty steps plus the empty step on each side of a run), `centroided` false, `ms_level` 1 (TOF MS) or 2 (product ion); `total_ion_current` = index TIC; `base_peak_mz`/`base_peak_intensity` from the index (for a scan that stores no position: the first maximum of the profile); product ions: `precursor_mz` from `DDERealTimeData`, `collision_energy` from the experiment's `CE` parameter, `activation` `HCD` (beam-type CID) — `CID` when the writer is Analyst QS (the QSTAR exports' label; no field states it, inferred from the exports); `native_id` `sample=S period=1 cycle=C experiment=E` as in the exports; `extra.calibration` (a, t₀, bin width).
- `scan_number` = position + 1 (MRM groups share an index record).
- `traces[]`: per sample `TIC` (index TIC) and `BPC`; after them one trace per LC device of each sample (`LC device K: Column Pressure, Pump A Flowrate, …`: one channel per device channel with its unit, `sample_rate_hz` from the channel records, time from 0; `extra.sample`, `extra.device`) (index base-peak intensity; for MRM, which stores none, the largest transition value of the cycle), one point per non-empty record, irregular sampling.
- Run `extra`: `sample_name`, `sample_id`, `acquired_at` (`SampleTable` + 0x3E: the acquisition computer's local clock as seconds since 1970; written with the UTC offset it has from the compound file's earliest storage creation time, a UTC FILETIME, rounded to a quarter hour, else with no zone), `instrument_serial`, `method`, `polarities`, `experiments` (scan type, polarity, mass-range count per experiment), `cycles`, `stored_spectra` (`SRM` or `profile`), `method_summary`. `instrument`: manufacturer `SCIEX`, model = `Component ID`, software and version from `FileRec_Str`.

## `check` finding codes

`truncated` (index ends inside a record; a scan past the end of `.wiff.scan`), `container` (compound-file problems), `undecodable` (errors); `missing_file` (no `.wiff.scan`), `missing_method`, `time_not_monotonic`, `window_count_mismatch`, `tic_mismatch` (warnings).

## Vocabulary (every public identifier in `crates/openreadout-sciex` must appear here)

| identifier | meaning |
| --- | --- |
| `SciexWiffReader`, `SciexDataset`, `FORMAT_ID`, `open`, `open_input`, `wiff_path` | reader, opened file, the id `sciex-wiff`, open (a local path, or an input whose namespace holds the `.wiff` and `.wiff.scan`), the `.wiff` a path names |
| `STREAM_PREAMBLE`, `INDEX_RECORD`, `SCAN_FILE_HEADER`, `TOF_CAL_RECORD`, `TOF_CAL_FIRST`, `PRECURSOR_SLOT` | sizes and offsets above |
| `SCAN_TYPE_MRM`, `SCAN_TYPE_TOF_MS`, `SCAN_TYPE_TOF_PRODUCT` | scan type codes 4, 8, 9 |
| `le_u16`, `le_u32`, `le_f32`, `le_f64` | bounds-checked little-endian readers |
| `IndexRecord`, `offset`, `byte_len`, `time_ms`, `tic`, `base_peak_intensity`, `base_peak_x`, `parse_index`, `index_trailing` | the scan index |
| `expand_zero_runs`, `decode_tdc` | MRM and TOF scan decoders |
| `MassRange`, `first_mz`, `second_mz`, `expected_rt_min`, `name`, `parameters`, `parameter`, `parse_mass_ranges` | transitions / mass ranges |
| `ExperimentHeader`, `scan_type`, `polarity`, `range_count`, `parse_experiment_header` | experiment header fields |
| `Experiment`, `number`, `header`, `ranges`, `tdc_step` | one experiment of the method; TDC bins per stored TOF step |
| `Sample`, `index`, `windows`, `log`, `strings`, `started_at` | one sample (run): scan index, scheduled windows, log text, sample strings, acquisition start |
| `parse_windows`, `tof_calibration`, `tof_default_calibration`, `tof_mz`, `precursor_slot`, `tdc_step` | scheduling windows, TOF calibration, precursor slots |
| `utf16_runs`, `sample_strings`, `log_fields` | text helpers |
| `DeviceChannel`, `name`, `unit`, `rate_hz`, `parse_device_channels`, `parse_device_data`, `devices` | LC device channels of a sample and their values |
