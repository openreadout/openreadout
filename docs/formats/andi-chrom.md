# AIA/ANDI netCDF (`.cdf`)

AIA/ANDI netCDF is the vendor-neutral export that many chromatography and GC/MS data systems offer. OpenReadout returns a chromatography file as one trace plus its peak table, and a mass-spectrometry file as one spectra run plus a TIC trace.

The container is netCDF classic, a public Unidata specification. The ANDI templates (ASTM E1947 chromatography, ASTM E2077 mass spectrometry) are paywalled standards, so every variable and attribute name below was read from the corpus files themselves (netCDF is self-describing) and cross-checked with scipy's `netcdf_file` (BSD-3-Clause), also used as a reference reader. See `docs/provenance/andi-chrom.md`. Format id: `andi-chrom`; crate `openreadout-chrom` (`netcdf.rs`, `andi_dataset.rs`).

## netCDF classic container (`netcdf.rs`)

From the Unidata "NetCDF Classic Format Specification" (https://docs.unidata.ucar.edu/netcdf-c/current/file_format_specifications.html). All numbers are big-endian.

- Magic `CDF` + version byte: 1 (classic, 32-bit offsets) or 2 (64-bit offsets). 5 (CDF-5, 64-bit data) is recognised (`HeaderError::Cdf5`) and refused (exit 6).
- `numrecs` (u32; `0xFFFFFFFF` = streaming, read as 0), then three lists — dimensions (tag `0x0A`), global attributes (tag `0x0C`), variables (tag `0x0B`) — each either `ABSENT` (two zero words) or tag + count + elements.
- Names: u32 length + bytes padded to 4. Attribute values: type, count, values padded to 4. Types (`NcType`): 1 byte, 2 char, 3 short, 4 int, 5 float, 6 double.
- Variable: name, dimension ids, attributes, type, `vsize`, `begin` (u32 in CDF-1, u64 in CDF-2).
- A dimension of length 0 is the record (unlimited) dimension; its length is `numrecs`. Variables whose first dimension is the record dimension are **record variables**: one slab per record, interleaved with the other record variables. The record size is the sum of each record variable's slab padded to 4 bytes, except that a file with a single record variable is not padded. ANDI/MS files commonly make `point_number` the record dimension, so `mass_values` and `intensity_values` interleave.
- `read_f64` reads any element range of a variable (strided for record variables, in 4 MiB chunks); `read_strings` turns a char variable into strings (innermost dimension = string length, NUL-terminated).

## Template detection (`AndiTemplate`)

`MassSpectrometry` when the global attribute `ms_template_revision` or the variable `mass_values` exists; `Chromatography` for `aia_template_revision` or `ordinate_values`; otherwise `Unknown` (listed, nothing mapped; `check` warns `not_andi`). `sniff` looks for these names in the first 64 KiB after the `CDF\x01`/`CDF\x02` magic; a netCDF file named `.cdf` without them is `likely`.

## Chromatography template → one trace, one table

| ANDI name (from the files) | where | our field |
| --- | --- | --- |
| `ordinate_values` [`point_number`] | variable (float) | the trace's single channel (`scale_factor`/`add_offset` attributes applied when present) |
| `actual_sampling_interval` | scalar variable (or attribute) | `sample_rate_hz = 1 / interval` |
| `actual_delay_time` | scalar variable | `start_s` |
| `actual_run_time_length` | scalar variable | `extra.run_time_s`; `check` compares it with the point count (`run_length_mismatch`, info) |
| `retention_unit` | global attribute | `Seconds`/`seconds` or `Minutes` (then the three times above are minutes: seen in `mtbls390-wb-cc-bat-01-cdf`) |
| `detector_unit`, `detector_name` | global attributes | channel `unit`; trace and channel `name` |
| `detector_maximum_value`, `detector_minimum_value` | scalar variables | `extra.detector_maximum` / `detector_minimum` |
| `uniform_sampling_flag`, `autosampler_position` | attributes of `ordinate_values` | `extra.uniform_sampling` (only when `N`), `extra.autosampler_position` |
| `peak_*`, `baseline_*`, `retention_index`, `migration_time`, `mass_on_column`, `manually_reintegrated_peaks` [`peak_number`] | variables | table `peaks`: one numeric column per 1-D variable along `peak_number` (dtype as stored); char variables (`peak_name`, `peak_start_detection_code`, …) go to the table's `extra` as string lists |

`extra` also carries (ANDI global attribute → ours): `sample_name`, `sample_id`, `sample_type`, `sample_id_comments` → `sample_comments`, `operator_name` → `operator`, `dataset_origin` → `origin`, `dataset_owner` → `owner`, `experiment_title` → `title`, `separation_experiment_type` → `separation`, `company_method_name` → `method`, `detection_method_name` → `detection_method`, `detector_name` → `detector`, `source_file_reference` → `source_file`, `aia_template_revision`/`ms_template_revision` → `template_revision`, `dataset_completeness` → `completeness`, `sample_injection_volume` → `injection_volume`, `sample_amount`; time stamps `injection_date_time_stamp`/`experiment_date_time_stamp` → `acquired_at`, `dataset_date_time_stamp` → `dataset_date`, `netcdf_file_date_time_stamp` → `file_date` (`YYYYMMDDhhmmss±hhmm`, spaces ignored, all-zero stamps dropped; `andi_timestamp`). The time axis is `extra.axis` (`retention_time`, minutes).

## Mass-spectrometry template → one spectra run (+ TIC trace)

| ANDI name | our use |
| --- | --- |
| `scan_acquisition_time` [`scan_number`] (seconds) | spectrum `rt_s`; `rt_range_s` |
| `scan_index`, `point_count` [`scan_number`] | the scan's slice of `mass_values`/`intensity_values` |
| `mass_values`, `intensity_values` [`point_number`] | `mz` (f64) and `intensity` (f32), `scale_factor`/`add_offset` applied |
| `total_intensity` [`scan_number`] | `total_ion_current`; the `TIC` trace when the scan times are evenly spaced within 1 % |
| `test_ionization_polarity` (`Positive Polarity`) | `polarity` |
| `experiment_type` (`Centroided Mass Spectrum`) | `centroided` |
| `instrument_mfr`, `instrument_model`/`instrument_name`, `instrument_sw_version` [`instrument_number`, string] | `instrument` |
| `test_ms_inlet`, `test_ionization_mode`, `test_detector_type`, `test_scan_function` | `extra.inlet`, `ionization`, `detector`, `scan_function` |

MS level is always 1 (the template has no MS level). Unused values such as `-9999` (`a_d_sampling_rate`, `scan_duration`, `time_values` = 9.97e36 fill) are left in `info --view full` only.

## `check` finding codes

`truncated` (a variable's data runs past the end of the file), `missing_variable`, `bad_scan_index` (errors); `not_andi`, `point_count_mismatch`, `time_not_monotonic` (warnings); `run_length_mismatch`, `scan_index_gaps` (info).

## Vocabulary (every public identifier in `andi_*.rs` and `netcdf.rs` must appear here)

| identifier | meaning |
| --- | --- |
| `AndiReader`, `AndiDataset`, `ANDI_ID`, `looks_like_andi` | format reader, opened file, the id `andi-chrom`, signature test |
| `open`, `netcdf`, `template` | open a file; its parsed netCDF header; its template |
| `AndiTemplate` { `Chromatography`, `MassSpectrometry`, `Unknown` }, `name`, `andi_template` | template and its JSON name; classification of a header |
| `andi_timestamp` | ANDI time stamp to ISO-8601 |
| `NetCdf`, `version`, `numrecs`, `dims`, `attributes`, `variables`, `record_size`, `header_len` | parsed netCDF header |
| `parse`, `read_header`, `var`, `attr`, `shape`, `element_count`, `data_end`, `read_f64`, `read_strings` | header parsing (bytes / file); lookups; variable geometry; data reads |
| `HeaderError` { `Short`, `NotNetCdf`, `Cdf5`, `Bad` } | why a header did not parse |
| `Dimension`, `len`, `unlimited` | a dimension (record dimension: `unlimited`, length = records) |
| `Attribute`, `nc_type`, `value` | an attribute |
| `AttrValue` { `Text`, `Numbers` }, `as_text`, `as_f64`, `to_json` | attribute value and conversions |
| `Variable`, `dim_ids`, `vsize`, `begin`, `is_record` | a variable and where its data lives |
| `NcType` { `Byte`, `Char`, `Short`, `Int`, `Float`, `Double` }, `size`, `dtype` | element types, bytes per element, NumPy-style dtype name |
