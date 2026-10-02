# JEOL Delta `.jdf`

JEOL Delta, the software of JEOL NMR spectrometers, saves each data set as a `.jdf` file. OpenReadout returns the stored FID or spectrum as a trace with the acquisition parameters, and can process FIDs into spectra ([NMR processing](nmr-processing.md)). Derived from nmrglue's JEOL reader (`nmrglue/fileio/jeol.py`, BSD-3-Clause), read as prior art and used as the reference reader, and from hex dumps of public `.jdf` files (Delta 5.3.1, 5.3.3 and 6.0; JNM-ECZ400S, ECZ500R and an unnamed 500 MHz console). Provenance: `docs/provenance/jeol-jdf.md`. Format id `jeol-jdf`, family `nmr`, extension `.jdf`.

A `.jdf` file is one data set: a fixed header, a parameter section, optional axis lists, the data section, a context section (the experiment text, read only for the sample id) and an annotation section (not decoded). Detection: the file starts with `JEOL.NMR`.

## Header (`JdfHeader`, 1360 bytes, always big-endian)

| offset | bytes | our name | meaning |
| --- | --- | --- | --- |
| 0 | 8 | – | `JEOL.NMR` |
| 8 | 1 | `little_endian` | 1: parameters and data are little-endian (every corpus file), 0: big-endian |
| 9, 10 | 1 + 2 | `version` | layout version (1.2 in the corpus) |
| 12 | 1 | `dimensions` | 1…8 |
| 14 | 1 | `data_type` (2 high bits), `data_format` (6 low bits) | 0 float64, 1 float32; 1 `one_d`, 2 `two_d`, 3…8 higher, 12…14 small-submatrix layouts (`format_name`) |
| 15 | 1 | `instrument` | console code (`instrument_name`: 25 ECA in the corpus; the name table is nmrglue's) |
| 24 | 8 | `axis_types` | per dimension (`JdfAxisType`): 1 `Real`, 2 `Tppi`, 3 `Complex`, 4 `RealComplex`, 5 `Envelope`, 0 `None` |
| 32 | 8 × 2 | `units` | per dimension (`JdfUnit`): high nibble of the first byte the SI prefix (`prefix`, signed: 1 milli, 2 micro, −1 kilo, −2 mega), low nibble the power (`power`), second byte the base unit (`base`: 13 hertz, 26 ppm, 28 second, 4 °C, 31 tesla, … as tabulated by nmrglue; `base_symbol`, `symbol`, `factor`) |
| 48 | 124 | `title` | |
| 172 | 4 | `axis_listed` | one nibble per dimension (high nibble first): 0 a linear range, 1 or 3 a point-by-point list (below) |
| 176 | 8 × 4 | `points` | stored points per dimension (2D: multiples of 32) |
| 208, 240 | 8 × 4 each | `valid_start`, `valid_stop` | the valid point range per dimension (only it is returned) |
| 272, 336 | 8 × 8 each | `axis_start`, `axis_stop` | axis value of stored point 0 and of the last stored point, in the dimension's unit |
| 400, 404 | 4 each | `created`, `revised` | date: first 16 bits = 7 bits years since 1990, 4 bits month, 5 bits day (our reading: nmrglue takes the day from the third byte, which disagrees with the acquisition dates of every corpus file); the other 16 bits are not decoded |
| 408 | 16 | `node_name` | computer name |
| 424, 552, 680 | 128 each | `site`, `author`, `comment` | spectrometer name, login, comment |
| 808 | 8 × 32 | `axis_titles` | `Proton`, `Carbon13`, `Silicon29` |
| 1064, 1128 | 8 × 8 each | `base_frequency`, `zero_point` | MHz; reported in `info --view full` |
| 1192 | 8 | `reversed` | |
| 1212, 1216 | 4 each | `param_start`, `param_length` | parameter section |
| 1220, 1252 | 8 × 4 each | `list_start`, `list_length` | axis lists |
| 1284 | 4 | `data_start` | data section |
| 1288 | 8 | `data_length` | bytes of data (two 32-bit halves, high first) |
| 1296, 1304 | 8, 4 | `context_start`, `context_length` | the context section: the experiment text the spectrometer ran (`header … end header;`, `acquisition … end acquisition;`); it follows the data section in every corpus file (nmrglue's names; its content inferred from the corpus) |
| 1320 | 8 | `total_size` | declared file size |

`JDF_HEADER_BYTES` = 1360. A file shorter than the header is corrupt (exit 4).

## Parameters (`JdfParam`, `parse_jdf_params`)

At `param_start`: record size (64 = `JDF_PARAM_BYTES`), first and last index, total size (four 32-bit integers in the file's byte order), then records first…last (nmrglue reads one record fewer: it counts to the last index exclusive). Each record: 4 bytes of class, a 16-bit power of ten (`scaler`), five 2-byte units (the first is `unit`), 16 bytes of value, a 32-bit value type, 28 bytes of name (`name`, blanks trimmed). Value types (`JdfValue`): 0 `Text` (16 characters: longer strings are cut in the file, e.g. `sample_id`), 1 `Integer`, 2 `Float`, 3 `Complex`, 4 `Infinity`, other `Unknown`. `number` = value × 10^`scaler`; `text` = trimmed string. Parameter names are looked up case-insensitively (`param`); the file mixes `X_FREQ` and `solvent`.

## Data (`two_d` submatrices, sections)

The data section holds 2^*c* **sections** of ∏`points` values each, where *c* is the number of `Complex` axes; a `RealComplex` pair gives 2 sections (nmrglue `nsections`). Decoded layouts:

| layout | axis types | sections | sweeps × channels |
| --- | --- | --- | --- |
| `one_d` | `Real` | 1 | 1 × `real` |
| `one_d` | `Complex` | 2 (real, imaginary) | 1 × `real`, `imag` |
| `two_d` | `Real`/`Real` | 1 | rows × `real` |
| `two_d` | `Complex`/`Real`, `RealComplex`/`RealComplex` | 2 | rows × `real`, `imag` |
| `two_d` | `Complex`/`Complex` | 4 | 2 × rows: sweep 2*k* = sections 0/1 of row *k*, sweep 2*k*+1 = sections 2/3 (the indirect real and imaginary parts) |

`one_d` sections are plain arrays. `two_d` sections are 32 × 32 submatrices, submatrix rows then columns, each submatrix row-major (nmrglue `reorder_submatrix` with edge 32). Other layouts (3D+, small submatrices) and axis types (`Tppi`, `Envelope`) are unsupported (exit 6). Only the valid range (`valid_start`…`valid_stop`) of each dimension is returned.

**Sign.** Values are returned **as stored**. nmrglue returns section 0 − i·section 1 (the complex conjugate), and for 2D complex data negates the odd rows; the oracle undoes both (exactly) before hashing.

**Axis.** Point *i* of a linear axis is at `axis_start` + *i* · (`axis_stop` − `axis_start`) / (`points` − 1) (the corpus FIDs: *n* − 1 points span `x_acq_time`; the processed 29Si spectra are centred on `X_OFFSET`). The trace's `extra.axis` covers the valid range: time in s for FIDs (`kind` `time_domain`), chemical shift in ppm for processed spectra (`kind` `processed_spectrum`).

**Listed axes (non-uniform sampling).** When `axis_listed` is set for a dimension, `list_length` bytes at `list_start` hold one **big-endian** float64 per stored point, in the dimension's unit (inferred from the corpus: the HSQC and HMBC lists are 0, 0.11696, 0.23392, 0.40936 … ms, multiples of the 0.05848 ms dwell — the sampled increments of a 25 % NUS schedule; the header's linear range is then meaningless). Each list is table `axis_list` (columns `point`, `value` in s/ppm/Hz; valid rows only) and the indirect axis in `extra` is `{listed: true, first, last, size, values_table}`. NUS reconstruction is not performed.

## Traces and `extra`

One trace, `name` `fid` (time domain) or `spectrum`; `sample_rate_hz` = `X_SWEEP` for FIDs.

| our name | from | notes |
| --- | --- | --- |
| `kind`, `axis` | header | see above |
| `nucleus`, `domain` | `X_DOMAIN` (else the axis title) | `Proton` → `1H`, `Carbon13` → `13C`, element name + mass number otherwise (`jeol_nucleus`, inferred) |
| `spectrometer_frequency_mhz` | `X_FREQ` | Hz in the file |
| `carrier_offset_ppm` | `X_OFFSET` | |
| `spectral_width_hz`, `spectral_width_ppm` | `X_SWEEP`, `X_SWEEP`/`X_FREQ` | |
| `time_domain_size` | `X_POINTS` | |
| `scans`, `total_scans` | `SCANS`, `TOTAL_SCANS` | |
| `pulse_program` | `experiment` | `single_pulse_dec`, `hsqcad_auto.jxp` |
| `solvent` | `solvent` | `CHLOROFORM-D` |
| `temperature_c`, `temperature_k` | `temp_get` | when its unit is °C (or K) |
| `field_strength_t` | `field_strength` | |
| `sample_id` | the context section's `sample_id => "…";` line when it begins with the `sample_id` parameter (or there is none), else the parameter | the parameter is a 16-byte text field and cuts longer ids (`20230816 Zheng R`); the context line holds the whole id (`20230816 Zheng Rui Qi MHSWJ-15.81`) |
| `sample_id_truncated` | – | `true` when only the parameter was found and it fills its 16 bytes: the id may be cut (reported in the assurance block's `assumed`) |
| `title`, `comment`, `operator`, `site` | header | `operator` is the header author |
| `instrument`, `instrument_serial` | `inst_model_number`, `inst_serial_number` | `JNM-ECZ400S/L1` |
| `console` | header instrument code | `ECA` |
| `software`, `software_version` | `version` | `Delta` (inferred: the file names no program; Delta is JEOL's spectrometer software) when `version` is present; `5.3.1 [Windows]`; `format_version` is `JDF <major>.<minor>, Delta <version>` |
| `sampling` | `sampling` | `Non Uniform` on the NUS 2D files |
| `acquired_at` | `ACTUAL_START_TIME` | seconds since 1990-01-01T00:00:00Z (inferred: agrees with the `sample.last shimmed` local times of three sites in three time zones after the zone offset) |
| `created_on`, `revised_on` | header dates | |
| `data_format`, `axis_types`, `stored_points`, `sample_type`, `byte_order` | header | |
| `indirect_dimensions[]` | `Y_DOMAIN`, `Y_POINTS`, `Y_SWEEP`, `Y_FREQ`, header | `{dimension, nucleus, domain, points, spectral_width_hz, spectrometer_frequency_mhz, encoding, axis}` |

## Experiment facts (`Dataset::experiment`)

`sample.id` ← `sample_id` (source `context sample_id` or `parameter sample_id`), `sample.name` ← header title when it differs from the id, `acquisition.operator` ← header author (origin `inferred`). The derived model adds vendor, model (`inst_model_number`), serial, nucleus, pulse program, frequency, solvent, temperature and start time from `extra`.

## `check` finding codes

`truncated`, `bad_header`, `unsupported_layout` (errors); `size_mismatch`, `extra_bytes`, `parameter_section`, `missing_parameters`, `missing_parameter` (`X_SWEEP`) (warnings). `axis_rate_mismatch` (info): a FID whose header axis range does not step by 1/`X_SWEEP`; its `extra.axis` then steps by 1/`X_SWEEP` from the header's first value (`step_from` `X_SWEEP`, as `X_ACQ_DURATION` = `X_POINTS` / `X_SWEEP` confirms) and the header's range is kept in `extra.header_axis`.

## Observed corpus values

| id | Delta | layout | notes |
| --- | --- | --- | --- |
| `nmrxiv-s200-qhnmr-jdf`, `-13c-jdf` | 5.3.1 | `one_d` complex, 65536 | JNM-ECZ400S |
| `nmrxiv-s200-cosy-jdf` | 5.3.1 | `two_d` real_complex/real_complex, 1280 × 256 | |
| `nmrxiv-s200-hsqc-jdf`, `-hmbc-jdf` | 5.3.1 | `two_d` complex/complex, 1024 × 32, 2048 × 64 | NUS, listed Y axis |
| `nmrxiv-s1243-esinica` | 5.3.3 | `one_d` complex, 35000 | 500 MHz |
| `nmrxiv-s908-zgig30`, `-aihe0` | 6.0 | `one_d` complex, 131072 stored, 104858 valid | processed 29Si spectra in ppm |

## Vocabulary (every public identifier in `crates/openreadout-nmr/src/jeol_*.rs` must appear here)

| identifier | meaning |
| --- | --- |
| `JeolReader`, `JeolDataset`, `JEOL_FORMAT_ID`, `open`, `param`, `header` | reader entry points: format reader, opened file (core `Dataset`), the id `jeol-jdf`; parameter lookup; the parsed header |
| `JdfHeader`, `JDF_HEADER_BYTES`, `parse`, `little_endian`, `version`, `dimensions`, `data_type`, `data_format`, `instrument`, `axis_types`, `units`, `title`, `axis_listed`, `list_start`, `list_length`, `points`, `valid_start`, `valid_stop`, `axis_start`, `axis_stop`, `created`, `revised`, `node_name`, `site`, `author`, `comment`, `axis_titles`, `base_frequency`, `zero_point`, `reversed`, `param_start`, `param_length`, `data_start`, `data_length`, `context_start`, `context_length`, `total_size`, `value_bytes`, `dtype`, `format_name`, `instrument_name` | the fixed header |
| `JdfAxisType` { `None`, `Real`, `Tppi`, `Complex`, `RealComplex`, `Envelope`, `Other` }, `name` | axis kind |
| `JdfUnit`, `prefix`, `power`, `base`, `base_symbol`, `factor`, `symbol` | a unit |
| `JdfParam`, `JDF_PARAM_BYTES`, `name`, `value`, `scaler`, `unit`, `number`, `text`, `parse_jdf_params` | one parameter record |
| `JdfValue` { `Text`, `Integer`, `Float`, `Complex`, `Infinity`, `Unknown` } | its value |
| `jeol_nucleus` | axis title → nucleus |
