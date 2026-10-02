# Varian / Agilent VnmrJ NMR data directories

Varian and Agilent NMR spectrometers (VNMRS and INOVA consoles, VNMR and VnmrJ software) store an experiment as a `.fid` directory. OpenReadout returns the FID as a trace with one sweep per stored trace, and the acquisition parameters from `procpar`. Processed data in `datdir` is listed, not decoded.

Derived from nmrglue's documentation and source (BSD-3-Clause, read as prior art and run as a reference reader) and public data directories. Provenance: `docs/provenance/varian-nmr.md`. Format id `varian-nmr`, family `nmr`.

A data set is a **directory** `<name>.fid/`, not a file. OpenReadout opens the directory, or a path to its `fid`, `procpar`, `text` or `log` (`resolve_varian_dir`). A folder holding several `.fid` directories (a study as nmrXiv ships it) is detected (`find_varian_experiments`) but not opened: the error lists the directories (exit 2). A directory with `acqus` is a Bruker experiment, never a Varian one.

```
<name>.fid/
    fid        binary: 32-byte file header, then blocks (block headers + traces)
    procpar    parameter text (optional: without it only the layout is known)
    text       free-text title (sample description)
    log        acquisition log (listed, not interpreted)
    datdir/phasefile, datdir/data   processed data, when present (listed, not decoded)
```

## The `fid` file (`FidHeader`, `BlockHeader`)

Everything is **big-endian** (every corpus file, as nmrglue documents).

File header (`FILE_HEADER_BYTES` = 32), as nmrglue's `get_fileheader` unpacks it (six 32-bit integers, two 16-bit, one 32-bit):

| offset | our name | meaning |
| --- | --- | --- |
| 0 | `block_count` | blocks in the file |
| 4 | `traces_per_block` | traces in each block (1 in every corpus file) |
| 8 | `points` | values per trace, real and imaginary counted separately (`np`) |
| 12 | `element_bytes` | 2 or 4 |
| 16 | `trace_bytes` | `points` × `element_bytes` |
| 20 | `block_bytes` | `traces_per_block` × `trace_bytes` + `block_headers` × 28 |
| 24 | `version_code` | 16-bit software/file version code (0 in the corpus) |
| 26 | `status` | 16-bit status bits (below) |
| 28 | `block_headers` | block headers at the start of each block (1 in the corpus) |

`problems` lists every inconsistency (negative counts, sizes that do not add up, an element size the status bits contradict); a header with problems is not treated as a Varian file by `sniff` (`read_fid_header`), and a directory whose `fid` fails is not opened.

Status bits (`status_names`; meanings as nmrglue documents them, names ours): 0x1 `data`, 0x2 `spectrum`, 0x4 `int32`, 0x8 `float32`, 0x10 `complex`, 0x20 `hypercomplex`, 0x80 `acquisition_parameters`, 0x100 `secondary_fourier_transform`, 0x200 `transposed`, 0x800/0x1000/0x2000/0x4000 `np_dimension`/`nf_dimension`/`ni_dimension`/`ni2_dimension`.

Sample type (`VarianSampleType`, `sample_type`): `Float32` when 0x8 is set, else `Int32` when 0x4 is set, else `Int16` (nmrglue `find_dtype`). Values are converted to f64 exactly (`decode_varian`). Observed: 0xc9 (float32) in every VNMRS file, 0x49 (float32 without 0x80) on the VnmrJ 4.2 INOVA, 0x45 (int32) on the older INOVA. int16 (`dp='n'`) appears in no corpus file; it is covered by synthetic tests only.

Block header (`BLOCK_HEADER_BYTES` = 28; four 16-bit, one 32-bit, four float32): `scale`, `status`, `index` (1-based block number), `mode`, `completed_scans` (the scan count of that block), `left_phase`, `right_phase`, `level`, `tilt` (drift correction). With two block headers per block the second is a hypercomplex header (nmrglue `get_hyperheader`); it is skipped. Block `scale` is 0 in every corpus file and, as in nmrglue, never applied; `check` reports blocks with a non-zero scale (`block_scale`). The INOVA int32 file carries non-zero `level`/`tilt`; they are reported in `info --view full` (first block), not applied.

Traces: each trace holds `points` / 2 complex points, real and imaginary interleaved (nmrglue `uninterleave_data`). **Sweeps are the traces in disk order**: sweep *s* is trace *s* mod `traces_per_block` of block ⌊*s* / `traces_per_block`⌋ — nmrglue's `as_2d=True` order. Arrayed (`array`) and multidimensional (`ni`, `ni2`, phase arrays) data are **not reordered**; `extra.arrayed_parameters`, `array_size` and `indirect_dimensions` say how the sweeps are organised. In every corpus file `arraydim` equals the number of traces (`check`: `array_mismatch` otherwise) and the block numbers run 1, 2, 3… (`block_index`).

A file shorter than `declared_len` is truncated: `info` notes it, `read_trace` of a missing block is a corrupt-file error (exit 4) and `check` reports `truncated` at the first incomplete block.

## Parameters (`procpar`: `Procpar`, `ProcparParam`, `ProcparValues`)

Text (UTF-8, else Latin-1 → `latin1`); one parameter per three or more lines (nmrglue `get_parameter`):

1. `name subtype basic_type max min step group display_group protection active intptr` — we keep `name`, `subtype`, `basic_type` and `active` (0 = switched off).
2. value count, then the values: reals on the same line (`ProcparValues::Real`); strings in double quotes, the first on this line and each further one on its own line (`ProcparValues::Text`). A string left open continues on the next line; `\"` and `\\` are unescaped.
3. enumeration count, then the allowed values (`enumeration`).

Problems (short value lists, non-numeric reals, missing lines) go to `issues`; `check` reports them as `parameter_syntax`. Lookup: `get`, `real` (first real value), `text` (first string, trimmed, non-empty), `first_text`, `len`, `is_empty`; `to_json` gives `info --view full`'s vendor tree (`procpar`). `parse_procpar` parses bytes.

### `extra` of the trace (our name ← parameter)

| our name | from | notes |
| --- | --- | --- |
| `kind`, `file` | – | `time_domain`, `fid` |
| `axis` | `sw` | `{quantity: time, unit: s, first: 0, step: 1/sw, size}` |
| `nucleus`, `nucleus_name` | `tn` | `H1` → `1H`, `C13` → `13C` (inferred rewrite); the name as written |
| `spectrometer_frequency_mhz` | `sfrq` | |
| `decoupler_nucleus`, `decoupler_frequency_mhz` | `dn`, `dfrq` | |
| `spectral_width_hz` | `sw` | also the trace's `sample_rate_hz` (complex data) |
| `spectral_width_ppm` | `sw` / `sfrq` | computed |
| `time_domain_size` | `np` | values per trace (real + imaginary) |
| `scans`, `completed_scans`, `dummy_scans` | `nt`, `ct`, `ss` | |
| `receiver_gain` | `gain` | |
| `pulse_program` | `seqfil` | sequence name |
| `experiment` | `pslabel` | parameter-set label |
| `solvent` | `solvent` | as written (`cdcl3`, `none`) |
| `temperature_c` | `temp` | °C as written (0 when no temperature control is recorded) |
| `acquired_at`, `completed_at` | `time_run`, `time_complete` | `YYYYMMDDThhmmss` → ISO-8601 **without zone** (spectrometer local time, inferred) |
| `acquisition_date` | `date` | as written (`Jun  6 2007`) |
| `console`, `instrument` | `console` | `vnmrs`, `inova` |
| `system_name` | `systemname_` | |
| `probe` | `probe_` | |
| `operator` | `operator_` | |
| `sample_name`, `comment`, `title` | `samplename`, `comment`, the `text` file | |
| `software`, `software_version` | `parver` | `VnmrJ VERSION 4.2 REVISION A` → `VnmrJ`, `4.2 REVISION A`; `format_version` is `parver` as written (else `procpar version <parversion>`) |
| `arrayed_parameters`, `array_size` | `array`, `arraydim` | |
| `indirect_dimensions[]` | `ni`/`ni2`/`ni3`, `sw1`/`sw2`/`sw3`, `phase`/`phase2`/`phase3`, `dn`/`dn2`/`dn3` | `{dimension, increments, spectral_width_hz, phase_values, nucleus}` for each `niN` ≥ 1 |
| `block_count`, `traces_per_block`, `block_headers_per_block`, `status_code`, `status_flags`, `sample_type`, `byte_order` | file header | |
| `block_scale`, `block_completed_scans` | first block header | |

## Experiment facts (`Dataset::experiment`)

| experiment field | from |
| --- | --- |
| `sample.id` | `samplename` (unless empty or `none`) |
| `sample.name` | first line of `text` |
| `acquisition.operator` | `operator_` |

The derived model adds vendor (format), model (`console`), software and version (`parver`), nucleus, pulse program, spectrometer frequency, solvent, scans, start time from the trace `extra`.

## `check` finding codes

`bad_header`, `truncated`, `odd_points` (errors); `extra_bytes`, `block_index`, `missing_parameters` (no `procpar`), `parameter_syntax`, `np_mismatch`, `array_mismatch`, `missing_parameter` (`sw`) (warnings); `block_scale`, `non_utf8` (info).

## Observed corpus values

| id | console / software | `fid` | notes |
| --- | --- | --- | --- |
| `nmrglue-agilent-1d` | vnmrs, no `parver` (2007) | 1 × 3000 float32 | 13C CP, `nt` 512 |
| `nmrglue-agilent-2d` | vnmrs | 332 × 3000 float32 | `ni` 166, `array` phase |
| `nmrglue-agilent-2d-tppi` | vnmrs | 600 × 2800 float32 | TPPI, no phase array |
| `nmrglue-agilent-3d` | vnmrs | 11264 × 2500 float32 | `array` phase,phase2 |
| `nmrglue-agilent-4d` | – (no `procpar`) | 1536 × 2800 float32 | layout only |
| `nmrpy-test1-fid` | inova, `parversion` 5.1 | 24 × 31084 int32 | 31P, arrayed `nt`, non-zero `lvl`/`tlt` |
| `nmrpy-test2-fid` | inova, VnmrJ 4.2 | 1 × 32768 float32 | status 0x49 |
| `nmrxiv-s325-1h` | vnmrs, VnmrJ 4.0 | 1 × 32768 float32 | `samplename`, `text` |
| `nmrxiv-s501-carbon` | vnmrs, VnmrJ 4.2 | 1 × 65536 float32 | |
| `nmrxiv-s501-ghsqcad` | vnmrs, VnmrJ 4.2 | 200 × 3232 float32 | `ni` 100, `array` phase |

## Vocabulary (every public identifier in `crates/openreadout-nmr/src/varian_*.rs` must appear here)

| identifier | meaning |
| --- | --- |
| `VarianReader`, `VarianDataset`, `VARIAN_FORMAT_ID`, `open`, `procpar` | reader entry points: format reader, opened directory (core `Dataset`), the id `varian-nmr`; the parsed `procpar` |
| `FidHeader`, `block_count`, `traces_per_block`, `points`, `element_bytes`, `trace_bytes`, `block_bytes`, `version_code`, `status`, `block_headers`, `parse`, `sample_type`, `problems`, `status_names`, `declared_len` | the 32-byte file header |
| `FILE_HEADER_BYTES`, `BLOCK_HEADER_BYTES` | 32 and 28 |
| `BlockHeader`, `scale`, `index`, `mode`, `completed_scans`, `left_phase`, `right_phase`, `level`, `tilt` | one block header (`status` as above) |
| `VarianSampleType` { `Int16`, `Int32`, `Float32` }, `width`, `dtype`, `decode_varian` | stored sample type; big-endian decoding |
| `read_fid_header`, `resolve_varian_dir`, `find_varian_experiments` | detection and path resolution |
| `Procpar`, `params`, `issues`, `latin1`, `get`, `real`, `text`, `to_json`, `parse_procpar` | the parsed `procpar` |
| `ProcparParam`, `name`, `subtype`, `basic_type`, `values`, `enumeration`, `active`, `first_text`, `len`, `is_empty` | one parameter |
| `ProcparValues` { `Real`, `Text` } | its values |
