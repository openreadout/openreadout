# JSON shapes (schema_version 1)

Authoritative: `openreadout self schema <info|info-format|info-full|info-explain|info-structure|check|planes|compare|export|extract|trace|scans|spectrum|formats|envelope>` prints the JSON Schema (MCP tools list theirs). This file is the human summary.

## `info` → `FileInfo`

```
path, size_bytes, format {id, name, vendor, extensions[], family, can_read, can_write, confidence, known_gaps[]},
format_version, plane_count, notes[],
images[]: {
  index, name, size_x, size_y, size_z, size_c, size_t, dimension_order ("XYCZT"),
  pixel_type (int8|int16|int32|uint8|uint16|uint32|float|double), samples_per_pixel,
  physical_size {x, y, z, unit: "µm"}, time_increment_s,
  channels[]: {index, name, fluorophore, excitation_nm, emission_nm, emission_range_nm [a,b], color "#RRGGBB", acquisition_mode, exposure_ms},
  objective {model, nominal_magnification, lens_na, immersion},
  instrument {manufacturer, model, software, software_version, detector},
  acquired_at (ISO-8601 when the file records it), mosaic {tile_count, tile_width, tile_height, stitched_on_read},
  pyramid_levels, plane_count, extra {format-specific, documented in docs/formats/<fmt>.md}
},
spectra[] (mass spectrometry only; images[] is then empty): {
  index, name, scan_count, ms_levels[], rt_range_s [first, last],
  instrument {manufacturer, model, software_version},
  extra {sample_name, sample_id, comment, vial, acquired_at, operator, instrument_serial, file_version,
         method_summary {instrument_method, segments, events_per_segment[], method_length_min},
         ms_level_counts {"1": n, "2": m}, polarities[], analyzers[] (FTMS|ITMS|...), ion_sources[] (ESI|NSI|MALDI|...),
         profile_scans, centroid_scans, ms1_scan_filters {filter: count}, mz_range [low, high], max_total_ion_current}
}
tables[] (FCS: one per dataset): {
  index, name ($FIL), row_count ($TOT),
  columns[]: {index, name ($PnN), label ($PnS), dtype (uint8|uint16|uint32|uint64|float32|float64), range [0, max],
              extra {bits, range_keyword, bit_mask, amplification {decades, offset}, gain, detector_voltage, excitation_wavelength_nm[], ...}},
  extra {fcs_version, datatype, byte_order, mode, instrument {model, serial_number}, software, acquisition_date,
         acquisition_start, acquisition_end, source, experimenter, operator, spillover {keyword, parameters[], matrix[][]},
         vendor_keywords {prefix: {keyword: value}}, supplemental_text, analysis_segment, ...}
}
traces[] (electrophysiology: ABF, Neuralynx NCS, Blackrock NSx, SpikeGLX, Intan, Plexon, NWB): {
  index, name (ABF protocol, NSx label, SpikeGLX stream "imec0.ap", Intan signal kind "amplifier"...),
  sample_rate_hz, sample_count (per sweep; the longest), sweep_count, start_s,
  channels[]: {index, name, unit (pA|mV|µV|V|A|...), dtype (int16|uint16|float32), scale, offset (value = raw × scale + offset), extra},
  extra {sweep_sample_counts[] (when sweeps differ), sweep_starts_s[], format-specific: ABF created_at, protocol, tags[], outputs[] {epochs[]};
         Neuralynx opened_at, timestamp_rate_hz; Blackrock recorded_at, ptp; SpikeGLX probe_type, first_sample; Intan notes, board_mode, ...}
}
tables[] also holds Neuralynx NEV events, NSE/NTT spikes (timestamp_us, cell, features, waveform w<e>_<i> µV), Blackrock NEV packets, Plexon spikes (time_s, timestamp, channel, unit, w<i>) and events, and NWB tables (electrodes, units, units/spike_times, trials, SpikeEventSeries; text columns are codes into columns[].extra.categories)
(time_s, packet_id, kind 0 digital/1 spike/2 comment/3 other, code, digital, waveform w<i> µV).
traces[] (Bruker NMR: fid/ser then one per pdata/<n>; JCAMP-DX: one per data table): {
  index, name, sample_rate_hz (0 for spectra), sample_count, sweep_count, start_s,
  channels[]: {index, name (real|imag|y|x|NTUPLES VAR_NAME), unit, dtype (stored), scale (applied), offset, extra},
  extra {kind (time_domain|processed_spectrum|xydata|ntuples|peak_table|xypoints),
         axis {quantity (time|chemical_shift|frequency|wavenumber|wavelength|mass_to_charge|x), unit, first, step, last, size},
         nucleus, spectrometer_frequency_mhz, spectral_width_hz, time_domain_size, scans, pulse_program, solvent,
         temperature_k, acquired_at, instrument, probe, group_delay_points, group_delay_source, indirect_dimensions[],
         non_uniform_sampling, normalization_exponent, sweep_axis, title, data_type, observe_frequency_mhz, ...}
}
```

## `spectrum --scan N` (one spectrum; schema `spectrum`) → `SpectrumOutput`

`path, format, run, view (primary|centroid), point_count, truncated, spectrum {index, scan_number, ms_level, rt_s, polarity (positive|negative|unknown), centroided, precursor_mz, precursor_charge, scan_filter, total_ion_current, mz[] (f64), intensity[] (f32)}`.

## `info --view full` → `Dump`

`file` (a `FileInfo`; per-frame records under `images[].extra.frames`, capped at 100 per image unless `--max-frames -1`, with `frame_records_total` and `frames_truncated`), `vendor` (JSON tree of the vendor metadata; CZI = the ImageDocument XML, ND2 = the LV/variant chunks by name (legacy files: the XML boxes by tag), LIF = the XML header), `provenance` (map of JSON path → `spec|vendor-impl|prior-art|inferred`).

CZI frame records are one per plane, ordered t, z, c: `{frame, c, z, t, acquired_at, time_ms, stage_x_um, stage_y_um, stage_z_um, exposure_ms, time_stamp_s}`. CZI `images[].extra` also carries `scene` (`center_position_um`, `contour_size_um`, `well` {`name`, `id`, `row_index`, `column_index`}), `experiment` (`active_setups`, `time_series_cycles`, `time_series_interval_s`), `time_stamps_s` (per T), `events` (`time_s`, `kind`, `description`), `pyramid` (per level: `size_x`, `size_y`, `downsample_x/_y`).

## `info --view explain` → `Explanation`

`summary` (string), `paragraphs[]` (strings, one topic each), `suggested_commands[]` (shell lines, paths quoted; a `# comment` says why), `caveats[]` (strings). Prose, not data: use `info` for values you compute with.

## `info --view structure` → `Listing`

`path, format, entries[]: {kind, name, offset, size, image, details}`. Kinds: `image`, `metadata`, `block` (LIF), `segment` (FCS: `HEADER`, `TEXT`, `STEXT`, `DATA`, `ANALYSIS`, `OTHERn`, `CRC`, with `details.data_set`), `subblock`/`pyramid-subblock`/`pyramid-level`/`attachment`/`file-part`/`file-header`/`subblock-directory`/`attachment-directory`/`deleted` (CZI; `attachment` rows have `details.index`, `details.content_type` for `extract`), `frame`/`metadata`/`custom-data`/`chunk` (ND2; legacy JPEG 2000 files: `frame`/`metadata`/`box`), `time-domain`/`processed-data`/`parameters`/`file` (Bruker: every file of the experiment directory, `name` relative to it), `block`/`table` (JCAMP-DX), `header`/`metadata`/`attachment`/`method`/`stream`/`index`/`scan` (Thermo RAW; one `scan` row per spectrum with `ms_level`, `rt_s`, `polarity`, `filter` in `details`).

## `check` → `CheckReport`

`path, format, ok, checks_performed[], findings[]: {severity: info|warning|error, code, message, offset}`. Codes seen: `truncated`, `bad_block_header`, `missing_block`, `missing_planes`, `size_mismatch`, `bad_offset`, `subblock_overflow`, `directory_mismatch`, `missing_pixels`, `bad_subblock`, `bad_chunk`, `loop_mismatch`, `structure`, `bad_box`, `bad_codestream`, `bad_metadata`, `unfinished_write`, `trailing_bytes`, `no_images`; CZI adds `missing_part`, `bad_part`, `resolution_protocol` (warning), `unsupported_subblock` (warning); FCS adds `data_length_mismatch`, `offset_discrepancy`, `data_end_off_by_one`, `missing_keyword`, `bad_keyword`, `duplicate_keyword`, `crc_mismatch`, `crc_not_computed` and others listed in `docs/formats/fcs.md`.

## `export` → `ExportReport`

`input, output, format ("ome-tiff" | "ome-zarr"), images_written, planes_written, bytes_written, verified, codec, ome_xml_bytes`. For `--format mzml` the report is `{input, output, format: "mzml", spectra_written, points_written, bytes_written, verified, view, sha1}`. For OME-Zarr `bytes_written` is the total size of the store directory, `codec` `deflate` means the Zarr `gzip` codec, and `ome_xml_bytes` is the size of `OME/METADATA.ome.xml` (0 when a single image was exported).

`export --format csv` → `TableExportReport`: `input, output, format ("csv"), table, first_row, rows_written, columns_written, header_lines, bytes_written, verified`; for a trace → `TraceExportReport`: `input, output, format ("csv"), trace, sweep, first_sample, samples_written, channels_written, bytes_written, verified`.

`export --format csv` of a trace → `TraceExportReport`: `input, output, format ("csv"), trace, sweep, first_sample, samples_written, channels_written, bytes_written, verified`.

## `trace` (CLI) / `openreadout_trace` (MCP) → `TraceSlice`

`path, format, trace, sweep, sweep_count, sample_rate_hz, sweep_sample_count, first_sample, sample_count (window), start_s, channels[]: {index, name, unit, stats {count, finite, min, max, mean, std, argmin, argmax}, samples[] (first max_samples of the window)}, truncated, axis` (the trace's `extra.axis` for spectra: sample i of the sweep is at `first + i*step`).

## `openreadout_table` (MCP) → `TableSlice`

`path, format, table, first_row, total_rows, columns[] ($PnN), labels[] ($PnS or null), rows[][] (row-major, raw values), truncated`.

## `trace` / `openreadout_trace` (MCP) → `TraceSlice`

`path, format, trace, sweep, sweep_count, sample_rate_hz, sweep_sample_count, first_sample, sample_count, start_s, channels[]: {index, name, unit, stats {count, finite, min, max, mean, std, argmin, argmax}, samples[]}, truncated`. Statistics cover the whole window; `samples` is capped (CLI `--max-samples`, default 1000; MCP `max_samples`, default 200, max 10000).

## `planes` → `PlanesOutput`

`path, format, planes[]: {image, level?, c, z, t, width, height, pixel_type, samples_per_pixel, xxh3}`; `xxh3` is xxh3-128 of the little-endian samples, 32 hex chars; `level` appears only for `--level N` with N > 0.

## `extract` → `ExtractOutput`

`path, format, attachment {index, name, content_type, extension, offset, size, extra}, output, bytes_written, xxh3, verified`.

## `info --view format` → `DetectOutput`

`path, format, name, confidence (definite|likely|extension-only), note`.
