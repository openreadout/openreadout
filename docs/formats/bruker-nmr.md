# Bruker TopSpin NMR

Bruker NMR spectrometers (TopSpin and the older XWIN-NMR) store each experiment as a directory. OpenReadout returns the time-domain data (FID or ser), the processed spectra (1D, 2D and 3D, real and imaginary parts), the acquisition and processing parameters, and the non-uniform sampling list as a table.

Derived from nmrglue's documentation and source (BSD-3-Clause, read as prior art and used as a reference reader) and public corpus experiment directories (XWIN-NMR 3.1/3.5, TOPSPIN 1.3, TopSpin 3.5 to 4.3.0). Provenance: `docs/provenance/bruker-nmr.md`. Format id `bruker-nmr`, family `nmr`.

A data set is a **directory**, not a file. OpenReadout opens the experiment directory `<name>/<expno>/`; a path to a file inside it (`fid`, `ser`, `acqus`, `acqu2s`, `pulseprogram`, `nuslist`) or to a processing directory (`pdata`, `pdata/<procno>`, or its `procs`/`1r`/`2rr`) resolves to that experiment (`resolve_experiment_dir`). The `<name>/` directory above several experiments is detected (`find_experiments`) but not opened: the error lists the experiment numbers (exit 2).

```
<name>/<expno>/
    acqus  acqu2s  acqu3s …     acquisition parameters (status copies; `acqu`, `acqu2`: set-up copies)
    fid | ser                   time-domain samples (1D | nD)
    pulseprogram  nuslist  …    other files (listed by `info --view structure`; nuslist is also a table)
    pdata/<procno>/
        procs  proc2s …         processing parameters
        1r 1i | 2rr 2ri 2ir 2ii | 3rrr … 3iii   processed spectrum (real/imaginary parts)
```

## Parameter files (`ParamFile`, `parse_param_file`)

JCAMP-DX-like text (nmrglue `read_jcamp`):

- `##TITLE= Parameter file, TopSpin 4.3.0` — the writing software and version (`software`; also `XWIN-NMR\t\tVersion 3.1`, `TOPSPIN\t\tVersion 1.3`). It is the file's `format_version`.
- `##$NAME= value` — one parameter. Names are case-sensitive (`SW` ≠ `SW_h`) and kept exactly as written.
  - `(0..n)` starts an array of n+1 values continued on the following lines; items are numbers, bare words or `<strings>`.
  - `<…>` is a string; it may continue over several lines until `>` (a probe head string in the synthetic tests).
  - `yes`/`no` and other bare words are kept as text.
- `##LABEL=` without `$` (`##JCAMPDX=`, `##ORIGIN=`, `##OWNER=`) → `header`; `$$` lines → `comments` (the second is a timestamp with time zone, the third the file's original path).
- `##END=` ends the file (`ended`). Problems (short arrays, unterminated strings, stray lines) are collected in `issues`, never fatal; `check` reports them as `parameter_syntax` / `parameter_unterminated`.

Values are exposed verbatim in `info --view full`'s vendor tree (`acquisition.acqus.parameters`, `pdata.<procno>.procs.parameters`).

## Time-domain data (`RawLayout`)

| parameter | use |
| --- | --- |
| `DTYPA` | `SampleType`: 0 (or absent) `Int32`, 2 `Float64`; anything else → unsupported (exit 6) |
| `BYTORDA` | `ByteOrder`: 1 `Big`, otherwise `Little` |
| `AQ_mod` | 1 or 3: real/imaginary interleaved (`complex`, channels `real`,`imag`); 0 or 2: real only |
| `TD` (acqus) | values per row (real and imaginary counted separately) |
| `TD` (acqu2s, acqu3s, …) | rows of `ser`, multiplied (`rows`) |
| `NC` | normalization exponent: values are scaled by 2^`NC` (channel `scale`; see below) |

- `fid`: one row of `TD` values; bytes after them are ignored (TopSpin 3.7 writes exactly `TD`×4 bytes even when `TD` is not a multiple of 256; `check` reports extra bytes as `fid_padding`).
- `ser`: each row starts on a 1024-byte boundary (`row_stride` = `TD`×width rounded up to 1024; nmrglue quoting the acquisition reference on `NBL`). A file that is exactly `TD`×width×rows long is read unpadded (`unpadded_rows`).
- `rows_in_file` counts rows whose values are present (the last row's padding may be missing). Fewer rows than declared → `truncated` (error, exit 4; a stopped acquisition looks the same).
- No `acqu2s`: rows from the file size. More whole rows than declared, and a multiple of them (nmrglue's 3D test set has no `acqu3s`): rows from the file size, with a note.
- Non-uniform sampling (`FnTYPE` = 2): `ser` holds the acquired rows only (`acqu2s` `TD` = 2 × `nuslist` entries); they are returned as sweeps, not reconstructed onto the `NusTD` grid (NUS reconstruction is out of scope). The `nuslist` itself is table 0 (`nuslist`, `extra.kind` `sampling_schedule`): one row per acquired increment in file order, one `uint32` column per indirect dimension (`index_1`, `index_2`, …), parsed by `parse_nuslist` (whitespace-separated unsigned integers, the same count on every line, at most 7). A file that is not such a list (or is over 16 MiB) is no table; `check` reports it (`nuslist_unreadable`, from `nuslist_issue`).

**Scaling.** nmrglue returns `fid`/`ser` unscaled. By analogy with its documented rule for processed data (value = stored × 2^`NC_proc`) OpenReadout multiplies time-domain values by 2^`NC` (`NC` is −1…−7 for int32 data and 0 for float64 data in the corpus). This is **inferred**; the factor is the channel `scale`, so the stored integers are `value / scale` exactly.

**Digital filter.** `group_delay` gives the group delay in points: `GRPDLY` when positive, else the `DSP_GROUP_DELAY` table (firmware `DSPFVS` 10–13, keyed by `DECIM`, as tabulated by nmrglue), else none. It is reported (`group_delay_points`, `group_delay_source`), never removed from the data.

## Processed data (`ProcLayout`)

| parameter (procs / proc2s) | use |
| --- | --- |
| `SI` | points per dimension; the direct dimension is the trace's `sample_count`, the product of the indirect ones its `sweep_count` |
| `XDIM` | submatrix edge of 2D and 3D files (ignored for 1D; 0 or ≥ `SI` means untiled); `SI` must be a multiple of it |
| `DTYPP`, `BYTORDP` | sample type and byte order, as for acquisition data |
| `NC_proc` | values are stored × 2^−`NC_proc`; read values = stored × 2^`NC_proc` (nmrglue `scale_pdata`) |
| `OFFSET`, `SW_p`, `SF` | ppm axis: point *i* is at `OFFSET` − *i*·`SW_p`/(`SF`·`SI`) (nmrglue `unit_conversion`) |
| `AXNUC` | nucleus of the axis |

1D: channels `real` (`1r`) and `imag` (`1i`, when present). 2D: channels `real` (`2rr`), `ri`, `ir`, `ii` (`2ri`, `2ir`, `2ii`, each when present; the first letter is F2, the second F1, as in the file names), one sweep per F1 row. 3D: channels `real` (`3rrr`), `rri`, `rir`, `rii`, `irr`, `iri`, `iir`, `iii`; sweep *j* + `SI`(F2) × *k* is F2 row *j* of F1 plane *k* (F2 fastest, as nmrglue's `reshape`). Files are stored as `XDIM`-sized submatrices: submatrix index runs over the dimensions slowest-first (F1, F2, direct), each submatrix row-major (`row_ranges`; nmrglue `reorder_submatrix`). 4D+ processed data are not decoded (listed by `info --view structure`).

## Traces

| trace | `name` | channels | `sample_rate_hz` | `sweep_count` |
| --- | --- | --- | --- | --- |
| time domain (`extra.kind` = `time_domain`) | `fid` / `ser` | `real`, `imag` (complex) or `real` | `SW_h` (complex) or 2×`SW_h` (real) | rows |
| each `pdata/<procno>` with `1r`, `2rr` or `3rrr` (`processed_spectrum`) | `pdata/<procno>` | `real` [, `imag`] (1D); `real`, `ri`, `ir`, `ii` (2D); `real`, `rri` … `iii` (3D) — those present | 0 (frequency domain) | 1, `SI`(F1) or `SI`(F2) × `SI`(F1) |

### `extra` of the time-domain trace (our name ← Bruker parameter)

| our name | from | notes |
| --- | --- | --- |
| `kind` | – | `time_domain` |
| `file` | – | `fid` or `ser` |
| `axis` | `SW_h` | `{quantity: time, unit: s, first: 0, step: 1/rate, size}` |
| `nucleus` | `NUC1` | |
| `spectrometer_frequency_mhz` | `SFO1` | |
| `base_frequency_mhz` | `BF1` | |
| `carrier_offset_hz` | `O1` | |
| `spectral_width_hz` | `SW_h` | |
| `spectral_width_ppm` | `SW` | |
| `time_domain_size` | `TD` | |
| `scans` | `NS` | |
| `dummy_scans` | `DS` | |
| `receiver_gain` | `RG` | |
| `pulse_program` | `PULPROG` | |
| `experiment` | `EXP` | parameter-set name |
| `solvent` | `SOLVENT` | |
| `temperature_k` | `TE` | |
| `acquired_at` | `DATE` | Unix seconds → ISO-8601 UTC (inferred; matches the `$$` timestamp line) |
| `instrument` | `INSTRUM` | |
| `probe` | `PROBHD` | |
| `software`, `software_version` | `##TITLE=` | |
| `quadrature` | `AQ_mod` | `complex` or `real` |
| `acquisition_mode_code` | `AQ_mod` | the number as written |
| `normalization_exponent` | `NC` | |
| `sample_type`, `byte_order` | `DTYPA`, `BYTORDA` | |
| `row_stride_bytes` | – | `ser` only |
| `group_delay_points`, `group_delay_source` | `GRPDLY`, `DSPFVS`, `DECIM` | see Digital filter |
| `decimation` | `DECIM` | |
| `dsp_firmware` | `DSPFVS` | |
| `indirect_dimensions[]` | `acqu2s` … | `{dimension, parameter_file, nucleus (NUC1), time_domain_size (TD), spectral_width_hz (SW_h), spectral_width_ppm (SW), spectrometer_frequency_mhz (SFO1), encoding (FnMODE: undefined, QF, QSEQ, TPPI, States, States-TPPI, Echo-Antiecho as named by nmrglue)}` |
| `non_uniform_sampling` | `FnTYPE` = 2 | `{nuslist_entries, amount_percent (NusAMOUNT), full_time_domain_size (acqu2s NusTD)}` |

### `extra` of a processed trace

`kind` (`processed_spectrum`), `procno`, `axis` (`{quantity: chemical_shift, unit: ppm, first, step, last, size, spectral_width_hz, spectrometer_frequency_mhz}`), `nucleus` (`AXNUC`), `spectral_width_hz` (`SW_p`), `spectrometer_frequency_mhz` (`SF`), `transform_size` (`FTSIZE`), `phase0_deg` (`PHC0`), `phase1_deg` (`PHC1`), `line_broadening_hz` (`LB`), `window_function_code` (`WDW`), `normalization_exponent` (`NC_proc`), `sample_type`, `byte_order`, `files`, and for 2D/3D `submatrix` (`XDIM` slowest dimension first), `shape` (`SI` slowest first) and `indirect_axes` (the ppm axis of each indirect dimension from `proc2s`, `proc3s`); for 2D also `sweep_axis` (the F1 axis from `proc2s`).

## Experiment facts (`Dataset::experiment`)

Facts the normalized model cannot carry, laid over the derived experiment ([`experiment-model.md`](../../book/src/guides/metadata.md)); origin `inferred`:

| experiment field | from |
| --- | --- |
| `sample.id` | the first of `USERA1`…`USERA5` in `acqus` that is not the `<user>`/`<>` placeholder (`source_field` `acqus ##$USERAn`), else the first non-empty line of `pdata/<procno>/title` when it reads like a label (at most 4 words, 64 characters, no `=`) |
| `sample.name` | that title line otherwise (`Bruker standard tube for water suppression`) |
| `acquisition.operator` | the `##OWNER=` header of `acqus` (the login that acquired) |

The `title` file is read only when it is at most 64 KiB; the first `pdata` directory with one wins.

## `check` finding codes

`truncated`, `missing_data`, `unsupported_sample_type`, `bad_parameters`, `missing_parameter` (TD), `missing_parameters` (pdata without procs), `bad_processed_data` (errors); `parameter_syntax`, `parameter_unterminated`, `acqus_missing`, `missing_dimension`, `extra_rows`, `nuslist_missing`, `nuslist_mismatch`, `nuslist_unreadable`, `size_mismatch`, `missing_parameter` (SW_h) (warnings); `layout`, `fid_padding`, `group_delay`, `non_utf8` (info).

## Observed corpus values

| id | software (`##TITLE=`) | data | `DTYPA`/`BYTORDA` | `NC` | notes |
| --- | --- | --- | --- | --- | --- |
| `nmrglue-bruker-1d` | XWIN-NMR 3.1 | fid TD 4096 | 0 / 1 (big) | −2 | `DSPFVS` 12, `DECIM` 16 |
| `nmrglue-bruker-2d` | XWIN-NMR 3.5 | ser 1300 × 600 | 0 / 1 (big) | −2 | 2D HSQC, no pdata |
| `nmrglue-bruker-3d` | XWIN-NMR 3.5 | ser 1300 × 14848 | 0 / 1 (big) | −2 | acqu2s TD = total rows, no acqu3s |
| `nmrxiv-s596-1` | TOPSPIN 1.3 | fid TD 65536, pdata/1 | 0 / 1 (big) | −1 | `AQ_mod` 1, `DSPFVS` 10, `GRPDLY` −1 |
| `nmrxiv-s846-50` | TopSpin 3.5 pl 7 | fid TD 131072, pdata/1 (XDIM 8192) | 0 / 0 | −6 | |
| `nmrxiv-s837-18` | TopSpin 3.7.0 | fid TD 21424 (unpadded) | 0 / 0 | −5 | |
| `nmrxiv-s837-21` | TopSpin 3.7.0 | ser 2048 × 32, NUS | 0 / 0 | −6 | nuslist 16 entries, NusTD 128 |
| `nmrxiv-s837-24` | TopSpin 3.7.0 | ser 2048 × 128 | 0 / 0 | −7 | |
| `nmrxiv-s501-6` | TopSpin 3.5 pl 7 | ser + pdata/1 `2rr`, `2ri`, `2ir`, `2ii` | 0 / 0 | | first public 2D processed set (nmrXiv S501, CC-BY-SA-4.0) |
| `nmrxiv-s275-1`, `-13` | TopSpin 4.3.0 | fid TD 65536 float64, pdata/1 | 2 / 0 | 0 | `NC_proc` 13 and −2 |

## Vocabulary (every public identifier in `crates/openreadout-nmr/src/bruker_*.rs` must appear here)

| identifier | meaning |
| --- | --- |
| `BrukerReader`, `BrukerDataset`, `BRUKER_FORMAT_ID`, `open`, `experiment` | reader entry points: format reader, opened experiment (core `Dataset`), the id `bruker-nmr`; the parsed experiment |
| `JCAMP_FORMAT_ID`, `JcampReader` | the sibling JCAMP-DX reader in the same crate (`docs/formats/jcamp-dx.md`) |
| `decode_text` | UTF-8, else Latin-1, text decoding of parameter files |
| `ParamFile`, `name`, `title`, `header`, `comments`, `params`, `issues`, `ended`, `latin1`, `size` | a parsed parameter file and its parts |
| `get`, `float`, `int`, `text`, `software`, `to_json`, `parse_param_file` | parameter lookup, `##TITLE=` software/version, JSON form, parser |
| `ParamValue` { `Int`, `Float`, `Text`, `List` }, `as_f64`, `as_i64`, `as_str` | one parameter value |
| `Experiment`, `dir`, `acqus`, `acqu_n`, `raw_file`, `nuslist_len`, `nuslist`, `nuslist_issue`, `parse_nuslist`, `processing`, `files`, `total_size`, `files_truncated`, `load` | an experiment directory: parameter files, `fid`/`ser`, `nuslist` line count, pdata, file listing |
| `Processing`, `procno`, `procs`, `data_files` | one `pdata/<procno>` directory |
| `SampleType` { `Int32`, `Float64` }, `from_code`, `width`, `dtype` | `DTYPA`/`DTYPP` |
| `ByteOrder` { `Little`, `Big` } | `BYTORDA`/`BYTORDP` |
| `decode_samples` | stored bytes → f64 |
| `RawLayout`, `file_name`, `file_len`, `sample_type`, `byte_order`, `complex`, `td`, `row_stride`, `rows`, `rows_in_file`, `unpadded_rows`, `notes`, `samples_per_row`, `row_bytes`, `from_experiment` | `fid`/`ser` layout |
| `ProcLayout`, `dims`, `si`, `xdim`, `components`, `from_processing`, `is_data_name`, `values`, `row_ranges` | processed-file layout, 2D/3D submatrix addressing |
| `DSP_GROUP_DELAY`, `group_delay` | digital-filter group delay table and lookup |
| `resolve_experiment_dir`, `find_experiments` | path → experiment directory; experiments inside a data-set directory |
