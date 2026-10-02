# JEOL JDF provenance

## 2026-09-22 — initial implementation

Inputs: five `nmrxiv-s200-jeol-*` experiments, CC0-1.0, from the already recorded S200
bundle (https://nmrxiv.org/sample/S200, DOI 10.57992/nmrxiv.p33.s200). Hex dumps inspected
before implementation: `JEOL.NMR`, big-endian fixed header, byte 8 = 1 (little-endian
payload), dimensions at 12, axis kinds at 24, sizes at 176, start/stop indices at 208/240,
axis endpoints at 272/336. These are raw FIDs rather than the processed JCAMP exports.

Permissive prior art: https://github.com/jjhelmus/nmrglue at
`5e2f095705bb90c6dc2f6a916abdfda82bac8e04`, BSD-3-Clause (LICENSE.txt verified first),
`nmrglue/fileio/jeol.py`. Current upstream DOES contain a JEOL reader. This permissive
implementation was consulted.
Use it as oracle for data sections, 32×32 2D tiles and complex signs; parameter units and
axis values remain separately recorded. Supported layouts are limited to those whose
addressing and units can be validated; other layout codes return unsupported.

## 2026-09-23 — implementation and validation (Richard Zimring with Claude as assistant)

The entry above was written before the reader existed; this one records how it was written.

**Prior art consulted** (permissive): nmrglue 0.12 as installed in `oracle/.venv`, BSD-3-Clause
(`nmrglue-0.12.dist-info/licenses/LICENSE.txt` read first; `nmrglue/fileio/jeol.py` is part of
the same package under the same licence): `read_header` (field offsets and sizes of the fixed
header), `read_parameters` (64-byte records), `read_bin_data`/`nsections`/`split_sections`/
`reorganize_1d`/`reorganize_2d` (sections, complex conjugate, 2D row interleaving),
`submatrix_shape` and `ConversionTable` (axis type, data type, layout, unit base and instrument
code tables), `truncate_data` (valid ranges). Names in our code and docs are our own.

**Corpus files used:** nmrXiv S200 (5 files, CC0), S1243 (1 of 5, CC0), S908 (2 of 4, CC0),
all listed in `corpus/manifest.toml`; also inspected, not added: S217 (CC0, 4 more Delta 5.3.1
files) and S454 (CC-BY-SA-4.0, Delta 5.3.1 `real_complex` HMQC/HMBC). Licences from nmrXiv's
per-study schema API.

**Our own findings, differing from or going beyond nmrglue:**
- the parameter section holds records *first…last inclusive* (201 records for first 0, last
  200, and 201 × 64 + 16 = the section length); nmrglue reads one fewer;
- the header dates are a 16-bit field of 7 bits years since 1990, 4 bits month, 5 bits day: this
  gives 2019-07-27 for the S200 files, the day of `ACTUAL_START_TIME` and of `sample.last
  shimmed`, where nmrglue's reading (day from the third byte) gives other days;
- `ACTUAL_START_TIME` / `end_time` are seconds since 1990-01-01T00:00:00Z: converted, they sit
  5 h (S200, UIC Chicago, CDT), 8 h (S1243) and 5 h (S908, EST) after the local `sample.last
  shimmed` times, i.e. they are UTC (inferred from three sites);
- listed axes: header byte 172 holds one nibble per dimension (high nibble first); for the NUS
  HSQC/HMBC the Y nibble is 3 and `list_length`[1] = 8 × Y points at `list_start`[1]; the bytes
  are big-endian float64 (the data are little-endian), in the axis unit (ms): 0, 0.11696,
  0.23392, 0.40936 … = multiples of the 0.05848 ms dwell (`Y_SWEEP` 17,099.86 Hz), i.e. the
  sampled increments of a 128-point grid at 25 % (`nus_rate` 25, `Y_ORIG_POINTS` 128). nmrglue
  ignores the lists;
- unit bytes: the high nibble is a signed SI prefix (1 milli, 2 micro; `x_pulse` is 7.74 with
  prefix 2 = µs; the HSQC Y axis unit is ms), which nmrglue does not apply;
- string values occupy 16 bytes: `sample_id` is cut at 16 characters in every file;
- the processed 29Si spectra (S908) store 131,072 points with valid range 13,107…117,964 (the
  central 80 %); their axis ends are the ppm values of stored points 0 and n − 1, centred on
  `X_OFFSET`.

**Validation:** `oracle/gen.py` branch `jeol` (nmrglue `jeol.read`, conjugate and 2D row
signs undone exactly): 8 files, 906 sweep × channel blocks bit-exact (xxh3-128), sample and
sweep counts, sample rate (`x_sweep`), ppm axis ends of the processed spectra, and nucleus,
frequency, sweep width, scans, points, solvent, pulse program. The depositors' JCAMP-DX files in
S200/S1243 are processed MestReNova spectra of the same experiments, not the FIDs, so they cannot
check sample values; they were not used for validation.

## 2026-09-25 — assurance profile (`src/assurance.rs`)

**Corpus files:** every development input of this format on disk (`corpus/assurance/evidence.json` lists them); no held-out file.
**Prior art consulted:** none.
**What was done.** No parsing logic changed. The reader declares an assurance profile (docs/assurance.md) that fingerprints each file from this reader's own normalized output: the JDF version, data format and axis types. The table of validated feature values and the confidence level are generated from the development corpus by `cargo xtask assurance-audit`.

## 2026-09-26 — FID axis from the sampling rate when the header range disagrees

**Corpus files:** `zenodo10621204-compound3-1h-jdf` (the case), and the other thirteen development `.jdf` files (unchanged: their header range steps by 1/X_SWEEP). **Prior art:** none new. **What was inferred from what.** In this file the header's `axis_stop` (1.99480 s over 14,993 valid points) implies a step 0.047 % shorter than 1/X_SWEEP, while `X_ACQ_DURATION` (1.99587 s) = `X_POINTS` / `X_SWEEP` exactly; the trace's `sample_rate_hz` (X_SWEEP) and its axis therefore disagreed (the `trace_clock` corpus test). The acquisition parameters agree with each other, so the time axis now follows them; the header range is reported, not dropped. nmrglue's values (the oracle) are unaffected.

## 2026-09-26 — the full sample id from the context section (finding NMR-1) (Richard Zimring with Claude as assistant)

**Corpus files:** the fourteen development `.jdf` inputs (`nmrxiv-s1243-esinica`, `nmrxiv-s908-zgig30`, `nmrxiv-s908-aihe0`, `nmrxiv-s200-{qhnmr,13c,cosy,hsqc,hmbc}-jdf`, `zenodo15045202-gk-ia-mbba-proton-ft-jdf`, `zenodo10621204-compound3-1h-jdf`, `zenodo18548733-ecrmn-06-proton-raw-jdf`, `zenodo5223412-{1h-3b,13c-2a}-jdf`, `zenodo15473381-o-1-1-200-scans-proton-jdf`) and the other `.jdf` members of the S1243 and S908 development bundles (differential comparison only); no held-out file.
**Prior art consulted:** nmrglue `fileio/jeol.py` (BSD-3-Clause, read as documentation): the header words after the data section are named `context_start` (64-bit, offset 1296), `context_length` (32-bit, 1304), `annote_start` (1308) and `annote_length` (1316). nmrglue does not interpret the context section.
**What was inferred from what.** In all twenty files `context_start` equals `data_start + data_length` and the section is text: the experiment source the spectrometer ran (`header … end header;`, `acquisition … end acquisition;`), with a line `sample_id => "…";`. The `sample_id` parameter is a text parameter of 16 bytes: `20230816 Zheng R`, `LMV_AdOxiraneCH2`; the context line holds the whole id (`20230816 Zheng Rui Qi MHSWJ-15.81`, `LMV_AdOxiraneCH2OPhOH-meta`), which begins with the parameter in every file and equals the header title in every file (a second, independent place). Decided: `extra.sample_id` (and `experiment.sample.id`) is the context value when it begins with the parameter (the parameter otherwise); a parameter that fills all 16 bytes with no longer value found is reported as possibly cut (`extra.sample_id_truncated`, an `assumed` entry in the assurance block). The section is read only up to 1 MiB and its first `sample_id =>` line is taken; a missing or unreadable section, or a value that does not begin with the parameter, leaves the parameter.
