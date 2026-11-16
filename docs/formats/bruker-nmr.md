# Bruker TopSpin NMR

Bruker NMR spectrometers (TopSpin and the older XWIN-NMR) store each experiment as a directory. OpenReadout returns the time-domain data (FID or ser), the processed spectra of any dimension (1D to 8D, every real and imaginary part present), the acquisition and processing parameters of every dimension, and the non-uniform sampling list and the experiment's list files (`vdlist`, `vplist`, `fq1list`, …) as tables.

Derived from nmrglue's documentation and source (BSD-3-Clause, read as prior art and used as a reference reader) and public corpus experiment directories (XWIN-NMR 3.1/3.5, TOPSPIN 1.3, TopSpin 3.5 to 4.3.0). Provenance: `docs/provenance/bruker-nmr.md`. Format id `bruker-nmr`, family `nmr`.

A data set is a **directory**, not a file. OpenReadout opens the experiment directory `<name>/<expno>/`; a path to a file inside it (`fid`, `ser`, `acqus`, `acqu2s`, `pulseprogram`, `nuslist`) or to a processing directory (`pdata`, `pdata/<procno>`, or its `procs`/`1r`/`2rr`) resolves to that experiment (`resolve_experiment_dir`). The `<name>/` directory above several experiments is detected (`find_experiments`) but not opened: the error lists the experiment numbers (exit 2). A directory whose `acqus` names XMASS in its first lines is a Bruker flex MALDI-TOF spectrum, not an NMR experiment: it is left to the flex reader (`docs/formats/bruker-flex.md`).

```
<name>/<expno>/
    acqus  acqu2s  acqu3s …     acquisition parameters (status copies; `acqu`, `acqu2`: set-up copies)
    fid | ser                   time-domain samples (1D | nD)
    pulseprogram  nuslist  …    other files (listed by `info --view structure`; nuslist is also a table)
    vdlist vplist fq1list …     list files (older sets; tables)
    lists/<kind>/<name>         list files TopSpin 4 copies into the experiment (tables)
    pdata/<procno>/
        procs  proc2s …         processing parameters
        1r 1i | 2rr 2ri 2ir 2ii | 3rrr … 3iii | 4rrrr … 4iiii | …   processed spectrum (real/imaginary parts)
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

**Row order.** Row *j* of `ser` holds the coordinates of the indirect dimensions with `acqu2s` changing fastest (as nmrglue reshapes `ser`): in 3D, row *j* + `TD`(acqu2s) × *k* is increment *j* of dimension 2 and increment *k* of dimension 3, where `TD` counts the stored values of each dimension (for States and echo-antiecho encodings two rows per increment). `extra.sweep_dimensions` lists the dimensions fastest first (`[2, 3]`). The XWIN-NMR set `nmrglue-bruker-3d` has `PARMODE` 1 and no `acqu3s`: its 14848 rows are reported as one indirect dimension, which is all the parameter files say.

**List files** (`ExperimentList`, `parse_experiment_list`). An experiment that steps a delay, a power, a loop counter or a frequency names the list in `acqus` (`VDLIST`, `VPLIST`, `VALIST`, `VCLIST`, `VTLIST`, `FQ1LIST` … `FQ8LIST`). The reader takes the list beside `acqus` (`vdlist`, `vclist`, `vplist`, `valist`, `vtlist`, `fq1list` … `fq8list`, `difflist`; `desperate-rbno3-mqmas-20` keeps its `fq1list` there), else the copy `lists/<folder>/<name>` that `nmrtools-19f-r1rho-ts4` holds (folders `va` and `vp` are seen in the corpus; `vd`, `vc`, `vt`, `f1` for frequency lists and `diff` follow the same pattern and are not confirmed by a file). One value per line; a first line that is not a number is the header (`dB` in a power list, `O 114.530000` in a frequency list, kept as written); time lists (`vdlist`, `vplist`) take a unit letter (`n`, `u`, `m`, `s`) and are returned in seconds. Each list is a table (after the `nuslist`), one `value` column, with `extra.kind` = `experiment_list`, `list`, `file`, `parameter` and `header`; `check` reports lines that are not values (`list_unreadable`).

**Scaling.** nmrglue returns `fid`/`ser` unscaled. By analogy with its documented rule for processed data (value = stored × 2^`NC_proc`) OpenReadout multiplies time-domain values by 2^`NC` (`NC` is −1…−7 for int32 data and 0 for float64 data in the corpus). This is **inferred**; the factor is the channel `scale`, so the stored integers are `value / scale` exactly.

**Digital filter.** `group_delay` gives the group delay in points: `GRPDLY` when positive, else the `DSP_GROUP_DELAY` table (firmware `DSPFVS` 10–13, keyed by `DECIM`, as tabulated by nmrglue), else none. It is reported (`group_delay_points`, `group_delay_source`), never removed from the data.

## Processed data (`ProcLayout`)

| parameter (procs / proc2s) | use |
| --- | --- |
| `SI` | points per dimension; the direct dimension is the trace's `sample_count`, the product of the indirect ones its `sweep_count` (`rows`); the product of all of them must fit the file size (checked, else a corrupt-file error) |
| `XDIM` | submatrix edge of 2D and higher files (ignored for 1D; 0 or ≥ `SI` means untiled); `SI` must be a multiple of it, else the spectrum is a corrupt-file error naming both values |
| `DTYPP`, `BYTORDP` | sample type and byte order, as for acquisition data |
| `NC_proc` | values are stored × 2^−`NC_proc`; read values = stored × 2^`NC_proc` (nmrglue `scale_pdata`) |
| `OFFSET`, `SW_p`, `SF` | ppm axis: point *i* is at `OFFSET` − *i*·`SW_p`/(`SF`·`SI`) (nmrglue `unit_conversion`) |
| `AXNUC` | nucleus of the axis |

File names (`processed_dims`, `processed_components`): `<N>` and N letters `r`/`i`, the first letter for the direct dimension, N from 1 to 8 (`MAX_PROCESSED_DIMS`). The spectrum's dimension is the lowest N whose all-real file is present. Channels are the component files present, in the order of every `r`/`i` combination with the last letter changing fastest: 1D `real` (`1r`), `imag` (`1i`); 2D `real` (`2rr`), `ri`, `ir`, `ii`; 3D `real` (`3rrr`), `rri`, `rir`, `rii`, `irr`, `iri`, `iir`, `iii`; 4D `real` (`4rrrr`), `rrri`, … `iiii`. A set may hold only some of them (`nmrtools-19f-r1rho-ts4`: `3rrr` and `3iii`). Sweeps are the direct-dimension rows: sweep *j* holds the indirect coordinates with `proc2s` fastest, then `proc3s`, … (3D: sweep *j* + `SI`(proc2s) × *k* is row *j* of plane *k*, as nmrglue's `reshape`; `extra.sweep_dimensions` = `[2, 3]`). Files are stored as `XDIM`-sized submatrices: the submatrix index runs over the dimensions slowest-first, each submatrix row-major (`row_ranges`; nmrglue `reorder_submatrix`), the same loop for any N. 4D and higher have no public example; a synthetic 4D fixture written by nmrglue `write_pdata` (`oracle/make_nmr_fixtures.py`, `tests/fixtures/bruker_4d_pdata`) pins the order.

## Traces

| trace | `name` | channels | `sample_rate_hz` | `sweep_count` |
| --- | --- | --- | --- | --- |
| time domain (`extra.kind` = `time_domain`) | `fid` / `ser` | `real`, `imag` (complex) or `real` | `SW_h` (complex) or 2×`SW_h` (real) | rows |
| each `pdata/<procno>` with `1r`, `2rr`, `3rrr`, `4rrrr`, … (`processed_spectrum`) | `pdata/<procno>` | `real` [, `imag`] (1D); `real`, `ri`, `ir`, `ii` (2D); `real`, `rri` … `iii` (3D); `real`, `rrri` … `iiii` (4D) — those present | 0 (frequency domain) | the product of the indirect `SI` |

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
| `parameter_mode_code`, `acquisition_dimensions` | `PARMODE` | the code as written; dimensions = `PARMODE` + 1 (inferred: 1 with `acqu2s`, 2 with `acqu3s` in the TopSpin sets) |
| `indirect_dimensions[]` | `acqu2s` … | `{dimension, parameter_file, nucleus (NUC1), time_domain_size (TD), full_time_domain_size (NusTD, NUS sets only), spectral_width_hz (SW_h), spectral_width_ppm (SW), spectrometer_frequency_mhz (SFO1), base_frequency_mhz (BF1), carrier_offset_hz (O1), encoding (FnMODE: undefined, QF, QSEQ, TPPI, States, States-TPPI, Echo-Antiecho as named by nmrglue), encoding_code (FnMODE), stored_processing}`; `stored_processing` is the dimension's `procNs` of the first `pdata/<procno>` that has one (the fields of a processed dimension below, plus `procno`) |
| `sweep_dimensions` | – | `ser` only: the indirect dimensions in the order their coordinates change along the rows, fastest first |
| `non_uniform_sampling` | `FnTYPE` = 2 | `{nuslist_entries, amount_percent (NusAMOUNT), schedule (NUSLIST), full_time_domain_size (acqu2s NusTD), full_time_domain_sizes[] and acquired_time_domain_sizes[] (NusTD and TD of each acquNs), table (0, the nuslist table)}` |
| `experiment_lists[]` | list files | `{kind, file, parameter, entries, unit, header, table}`: one per list file found (see List files) |

### `extra` of a processed trace

`kind` (`processed_spectrum`), `procno`, `axis` (`{quantity: chemical_shift, unit: ppm, first, step, last, size, spectral_width_hz, spectrometer_frequency_mhz}`), `nucleus` (`AXNUC`), `spectral_width_hz` (`SW_p`), `spectrometer_frequency_mhz` (`SF`), `transform_size` (`FTSIZE`), `phase0_deg` (`PHC0`), `phase1_deg` (`PHC1`), `line_broadening_hz` (`LB`), `window_function_code` (`WDW`), `normalization_exponent` (`NC_proc`), `sample_type`, `byte_order`, `files`, `processing` (the direct dimension's processing, below); for 2D and higher also `submatrix` (`XDIM` slowest dimension first), `shape` (`SI` slowest first), `sweep_dimensions` (`[2, 3, …]`, fastest first), `indirect_axes` (the ppm axis of each indirect dimension from `proc2s`, `proc3s`, …, with its `nucleus`) and `indirect_dimensions[]`; for 2D also `sweep_axis` (the F1 axis from `proc2s`).

Each processed dimension (`processing`, `indirect_dimensions[]`, `stored_processing`; our name ← parameter of its `procs`/`procNs`): `dimension` (1 direct, 2 for `proc2s`, …), `parameter_file`, `nucleus` (AXNUC), `size` (SI), `submatrix` (XDIM), `spectral_width_hz` (SW_p), `spectrometer_frequency_mhz` (SF), `offset_ppm` (OFFSET), `window_function_code` (WDW), `line_broadening_hz` (LB), `gaussian_broadening` (GB), `sine_bell_shift` (SSB), `phase0_deg` (PHC0), `phase1_deg` (PHC1), `transform_mode_code` (FT_mod), `quadrature_mode_code` (MC2), `quadrature_mode` (MC2 named as FnMODE − 1: QF, QSEQ, TPPI, States, States-TPPI, Echo-Antiecho; inferred from the 84 corpus sets that have both), `effective_time_domain_size` (TDeff), `transform_size` (FTSIZE), `linear_prediction_mode_code` (ME_mod), `linear_prediction_coefficients` (NCOEF), `linear_prediction_size` (LPBIN), `reversed` (REVERSE `yes`), `strip_start` (STSR), `strip_size` (STSI); `indirect_dimensions[]` entries also carry `axis` (the ppm axis with `nucleus`).

The vendor tree (`info --view full`) adds `lists`: each list file's `file`, `parameter`, `header`, `unit` and `values`.

## Experiment facts (`Dataset::experiment`)

Facts the normalized model cannot carry, laid over the derived experiment ([`experiment-model.md`](../../book/src/guides/metadata.md)); origin `inferred`:

| experiment field | from |
| --- | --- |
| `sample.id` | the first of `USERA1`…`USERA5` in `acqus` that is not the `<user>`/`<>` placeholder (`source_field` `acqus ##$USERAn`), else the first non-empty line of `pdata/<procno>/title` when it reads like a label (at most 4 words, 64 characters, no `=`) |
| `sample.name` | that title line otherwise (`Bruker standard tube for water suppression`) |
| `acquisition.operator` | the `##OWNER=` header of `acqus` (the login that acquired) |

The `title` file is read only when it is at most 64 KiB; the first `pdata` directory with one wins.

## `check` finding codes

`truncated`, `missing_data`, `unsupported_sample_type`, `bad_parameters`, `missing_parameter` (TD), `missing_parameters` (pdata without procs), `bad_processed_data` (errors); `parameter_syntax`, `parameter_unterminated`, `acqus_missing`, `missing_dimension`, `extra_rows`, `nuslist_missing`, `nuslist_mismatch`, `nuslist_unreadable`, `list_unreadable`, `size_mismatch`, `missing_parameter` (SW_h) (warnings); `layout`, `fid_padding`, `group_delay`, `non_utf8` (info).

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
| `nmrtools-19f-r1rho-ts4` | TopSpin 4.2.0 | float64 ser 8192 × 18 × 11, pdata/1 `3rrr`, `3iii` (SI 1216 × 32 × 16) | 2 / 0 | | pseudo-3D R1ρ, `lists/vp` and `lists/va` (MIT) |
| `nmrtools-15n-trosy-ts3` | TopSpin 3.6.2 | ser, pdata/1 `2rr` … `2ii` | 0 / 0 | | FnMODE 6, `MC2` 5, linear prediction in F1 (MIT) |
| `zakary-rbnb2o5f-93nb-3qmas-255`, `-87rb-3qmas-60` | TopSpin 4.1.4 | ser, pdata/1 `2rr` … `2ii` | 0 / 0 | | FnMODE 4 (States), `MC2` 3, solid-state 3QMAS (MIT) |
| `desperate-rbno3-mqmas-20`, `-30` | TopSpin 4.1.1 | ser, pdata/1 `2rr`, `2ii`, `fq1list` | 0 / 0 | | `-30` NUS (TD 77 of NusTD 129), `-20` its fully sampled twin (MIT) |
| `bmrb27037-chsqc-850` | TopSpin 2.1 | ser, pdata/1 `2rr` … `2ii` | 0 / 0 | | 13C HSQC (BMRB 27037, CC0) |

No public data set with processed 4D or higher files (`4rrrr` …) was found (nmrXiv, BMRB, Zenodo, figshare, GitHub test data); nor a 2D set acquired with FnMODE 2 (QSEQ) and processed by TopSpin. Most public 3D deposits keep `proc3s` but not `3rrr`.

## Vocabulary (every public identifier in `crates/openreadout-nmr/src/bruker_*.rs` must appear here)

| identifier | meaning |
| --- | --- |
| `BrukerReader`, `BrukerDataset`, `BRUKER_FORMAT_ID`, `open`, `experiment` | reader entry points: format reader, opened experiment (core `Dataset`), the id `bruker-nmr`; the parsed experiment |
| `JCAMP_FORMAT_ID`, `JcampReader` | the sibling JCAMP-DX reader in the same crate (`docs/formats/jcamp-dx.md`) |
| `decode_text` | UTF-8, else Latin-1, text decoding of parameter files |
| `ParamFile`, `name`, `title`, `header`, `comments`, `params`, `issues`, `ended`, `latin1`, `size` | a parsed parameter file and its parts |
| `get`, `float`, `int`, `text`, `software`, `to_json`, `parse_param_file` | parameter lookup, `##TITLE=` software/version, JSON form, parser |
| `ParamValue` { `Int`, `Float`, `Text`, `List` }, `as_f64`, `as_i64`, `as_str` | one parameter value |
| `Experiment`, `dir`, `acqus`, `acqu_n`, `raw_file`, `nuslist_len`, `nuslist`, `nuslist_issue`, `parse_nuslist`, `lists`, `processing`, `files`, `total_size`, `files_truncated`, `load` | an experiment directory: parameter files, `fid`/`ser`, `nuslist` line count, list files, pdata, file listing |
| `ExperimentList`, `kind`, `parameter`, `file`, `header`, `values`, `unit`, `issues`, `parse_experiment_list` | one list file (`vdlist`, `lists/vp/<name>`, `fq1list`, …) and its parser |
| `Processing`, `procno`, `procs`, `data_files` | one `pdata/<procno>` directory |
| `SampleType` { `Int32`, `Float64` }, `from_code`, `width`, `dtype` | `DTYPA`/`DTYPP` |
| `ByteOrder` { `Little`, `Big` } | `BYTORDA`/`BYTORDP` |
| `decode_samples` | stored bytes → f64 |
| `RawLayout`, `file_name`, `file_len`, `sample_type`, `byte_order`, `complex`, `td`, `row_stride`, `rows`, `rows_in_file`, `unpadded_rows`, `notes`, `samples_per_row`, `row_bytes`, `from_experiment` | `fid`/`ser` layout |
| `ProcLayout`, `dims`, `si`, `xdim`, `rows`, `components`, `from_processing`, `is_data_name`, `values`, `file_bytes`, `row_ranges` | processed-file layout, submatrix addressing in any dimension |
| `MAX_PROCESSED_DIMS`, `processed_dims`, `processed_components` | processed file names: the largest dimension (8), the dimension of a name, the component files of a dimension |
| `DSP_GROUP_DELAY`, `group_delay` | digital-filter group delay table and lookup |
| `resolve_experiment_dir`, `find_experiments` | path → experiment directory; experiments inside a data-set directory |
