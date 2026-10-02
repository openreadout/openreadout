# Provenance log — Bruker TopSpin NMR experiment directories

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

A Bruker TopSpin (and older XWIN-NMR) data set is a directory, not a file: `<expno>/` holds text parameter files in a JCAMP-DX-like syntax (`acqus`, `acqu2s`, …), the time-domain samples (`fid` for 1D, `ser` for nD) and processed data under `pdata/<procno>/` (`procs`, `proc2s`, `1r`/`1i`, `2rr`, …).

## 2026-09-22 — initial derivation (Richard Zimring with Claude as assistant)

**Prior art consulted** (permissively licensed; documentation and source read, no code copied):
- `nmrglue` 0.12, Jonathan J. Helmus and contributors, BSD-3-Clause, https://github.com/jjhelmus/nmrglue — `nmrglue/fileio/bruker.py` read as documentation: `read`/`read_binary`/`get_data` (`fid`/`ser` are int32 unless `DTYPA` = 2, then float64; big-endian when `BYTORDA` = 1; real/imaginary interleaved when `AQ_mod` is 1 or 3), `guess_shape` (its comment quotes the acquisition reference on `NBL`: each FID of a `ser` file starts on a 1024-byte boundary, so a row holds `TD` rounded up to 256 int32 or 128 float64 points), the NUS branch (`FnTYPE` = 2 with a `nuslist` file), `read_pdata`/`read_pdata_binary`/`reorder_submatrix` (processed files use `BYTORDP`/`DTYPP`; 2D processed data are stored as `XDIM`-sized submatrices), `scale_pdata` (processed values are divided by `2**-NC_proc`, i.e. multiplied by `2**NC_proc`), `add_axis_to_udic` together with `fileiobase.unit_conversion` (ppm of point *i* = `OFFSET` − *i*·`SW_p`/(`SF`·`SI`)), `rm_dig_filter` and its `bruker_dsp_table` (group delay in points from `GRPDLY` when > 0, else from the `DSPFVS`/`DECIM` table for firmware 10–13; nmrglue credits W. M. Westler and F. Abildgaard's public processing note for the table), `read_jcamp`/`parse_jcamp_line` (parameter-file syntax: `##$NAME= value`, `(0..n)` arrays continued on following lines, `<…>` strings that may span lines, `$$` comments, `##END=`), and `guess_topspin_version` (the version is the text after `Parameter file,` in `##TITLE=`).
- nmrglue is also run as a black-box oracle (`oracle/gen.py`, branch `bruker-nmr`).

Parameter meanings recorded below come from nmrglue's documentation and code comments and from the corpus files themselves.

**Corpus files used** (all in `corpus/manifest.toml`; bundles extracted by `cargo xtask corpus fetch`):
- nmrglue test data archive `test_data_v0.5-dev.zip` (release asset of the BSD-3-Clause nmrglue repository, v0.5): `nmrglue-bruker-1d` (XWIN-NMR 3.1, big-endian int32 (`BYTORDA` 1), `TD` 4096), `nmrglue-bruker-2d` (XWIN-NMR 3.5 `ser`, 2D HSQC, no `pdata`), `nmrglue-bruker-3d` (XWIN-NMR 3.5 `ser`, no `acqu3s`: `acqu2s` `TD` 14848 is the total row count).
- nmrXiv (https://nmrxiv.org, per-project licenses read from its public API `api/v1/list/projects`):
  - study S275 `nmrxiv-pt-succrose` (CC0-1.0, TopSpin 4.3.0): `DTYPA` = 2 float64 `fid`, `pdata/1` with `1r`/`1i` whose `NC_proc` ranges from −2 to 13 across experiments.
  - study S837 (CC0-1.0, TopSpin 3.7.0): 1D `fid` with `TD` 21424 (not a multiple of 256, file exactly `TD`×4 bytes: 1D files are **not** padded to 1024 bytes), 2D `ser` files with and without non-uniform sampling (`FnTYPE` = 2, `nuslist` of 16 increments, `acqu2s` `TD` = 32, `NusTD` = 128).
  - study S596 (CC-BY-4.0, TOPSPIN 1.3): big-endian `fid` (`BYTORDA` = 1), `AQ_mod` = 1, `DSPFVS` = 10 with `GRPDLY` = −1 (group delay from the table).
  - study S846 (CC-BY-4.0, TopSpin 3.5 pl 7): `TD` 131072, `pdata/1` with `XDIM` 8192 on 1D data (ignored for 1D, as in nmrglue).

**Method:** a Python walker (standard library only) printed, for each experiment directory, the `##TITLE=` line, `DTYPA`, `BYTORDA`, `TD` of every `acqu*s`, `NC`, `AQ_mod`, `DECIM`, `DSPFVS`, `GRPDLY`, `PARMODE`, the size of `fid`/`ser`, and for each `pdata/<n>`: `BYTORDP`, `DTYPP`, `NC_proc`, `SI`, `OFFSET`, `SW_p`, `SF`, `XDIM` and the processed files present. Observed:
- `fid`/`ser` sizes: `ser` rows are `TD` × 4 bytes rounded up to 1024 (S837 exp 24: `TD` 2048 × 128 rows = 1 MiB); 1D `fid` of S837 exp 18 is exactly `TD` × 4 = 85696 bytes. 1D files are therefore read as `TD` points with any trailing bytes ignored, and `ser` rows with a 1024-byte stride unless the file is exactly `TD` × bytes × rows long (then unpadded, reported as `unpadded_rows`).
- `DTYPA` 0 (int32) in XWIN-NMR 3.1/3.5, TOPSPIN 1.3, TopSpin 3.5, 3.7; `DTYPA` 2 (float64) with `NC` = 0 in TopSpin 4.3.0. `BYTORDA` 1 (big-endian) in the XWIN-NMR and TOPSPIN 1.3 sets, 0 in TopSpin 3.5 and later.
- `NC` (acquisition) is −1 … −7 for int32 data and 0 for float64 data. nmrglue returns `fid`/`ser` unscaled; by analogy with its documented `NC_proc` rule (value = stored × 2^`NC_proc`) we scale time-domain values by 2^`NC` and report the factor as the channel `scale`, so raw integers remain recoverable exactly. **Inferred**, not documented by prior art: flagged `inferred` in the provenance map.
- `GRPDLY` is −1 on older firmware (`DSPFVS` 10, 12) and a positive number (67.98…, 76) on `DSPFVS` 20/21.
- `##$DATE=` is a Unix timestamp (seconds); converted to ISO-8601 UTC. (Inferred: S837 exp 21 `DATE` 1740099624 = 2025-02-21T01:00:24Z, and the file's `$$ 2025-02-20 19:00:25.587 -0600` comment line is the same instant to within a second; S275 exp 1 agrees to within 4 s.)
- `acqu2s` of TopSpin 3.7 NUS experiments holds only a handful of parameters (no `DTYPA`), so byte order and sample type always come from `acqus`.
- TopSpin 4.3 `acqus` carries `scaledByNS` / `scaledByRG` = `no`; recorded, not interpreted.

**Inferred, not yet corroborated by a corpus file:** reading of `2rr` submatrices (no public corpus directory with 2D processed data was found: nmrXiv strips `2rr`; unit tests only), `DTYPP` = 2 processed data, `AQ_mod` 0/2 (real-only) sampling rate (2 × `SW_h`).

## 2026-09-22 — experiment facts (Richard Zimring with Claude as assistant)

Corpus: nmrxiv-s846-50 (`pdata/1/title` = `Q3,3'Br2`, `USERA1` = `<user>`), nmrxiv-s275-1 and -13 (two-line titles: a sample description, an acquisition note `13C{1H}, ns=16`), nmrxiv-s596-1 (`Ethylene Glycol in D2O+TMS`), nmrxiv-s837-18 (empty title), nmrglue-bruker-1d/2d/3d (no `pdata`). No new prior art: the `title` file and the `USERA1`…`USERA5` / `##OWNER=` records were read from the corpus directories themselves. Inferred: `USERAn` are free user fields that TopSpin fills with the `<user>` placeholder (every corpus file); `##OWNER=` is the login that wrote `acqus` (`nmrsu`, `dli`, `ns`); the first title line names the sample when it is short, and is a description or a processing note otherwise (the 4-word / no-`=` rule separates the corpus cases). `crates/openreadout-nmr/src/bruker_experiment.rs`; no parsing of existing fields changed.

## 2026-09-22 — processed components, 3D and NUS schedule

Before implementation: extend the existing corpus-derived XDIM addressing to three dimensions,
keeping direct dimension fastest and flattening indirect dimensions into sweeps. Prior art:
https://github.com/jjhelmus/nmrglue (BSD-3-Clause, LICENSE.txt verified), `bruker.read_pdata`
and `reorder_submatrix`. Existing corpus `nmrxiv-s837-21` provides a real `nuslist` (ASCII
integer acquisition indices); expose its rows without reconstruction or reordering. Processed
2ri/2ir/2ii and 3rrr coverage will include independently generated nmrglue fixtures; real-file
coverage, if unavailable, will be stated explicitly rather than claimed.

Implementation and validation (2026-09-23, Richard Zimring with Claude as assistant): the Codex
session's `wip:` commit (b80ee86) extended `ProcLayout` to 2D components and 3D; it was reviewed
and kept. Corpus: nmrXiv S501 (CC-BY-SA-4.0) experiment 6 is the first public directory with
`2ri`/`2ir`/`2ii` (TopSpin 3.5 pl 7, `XDIM` submatrices): all four components of every row match
nmrglue `read_pdata(all_components=True, scale_data=True)` bit for bit (1,024 rows × 4 channels).
No public 3D processed set was found; nmrglue `write_pdata` (run as a black box by
`oracle/make_nmr_fixtures.py`) wrote a small `3rrr`/`3rri` fixture with known values, which the
reader returns exactly (`crates/openreadout-nmr/tests/varian_jeol.rs`): the 3D submatrix order
is thus checked against nmrglue's writer, not only against our reading of `reorder_submatrix`.
`nuslist`: parsed leniently (a damaged list is reported by `check`, not fatal); table rows match
nmrglue `read_nuslist` (`nmrxiv-s837-21`). NUS reconstruction remains out of scope.
The oracle's Bruker parameter helper `_pick` had been shadowed by a later chromatography helper
of the same name (every Bruker oracle regenerated since then failed); renamed `_pick_keys`.

## 2026-09-25 — assurance profile (`src/assurance.rs`)

**Corpus files:** every development input of this format on disk (`corpus/assurance/evidence.json` lists them); no held-out file.
**Prior art consulted:** none.
**What was done.** No parsing logic changed. The reader declares an assurance profile (docs/assurance.md) that fingerprints each file from this reader's own normalized output: the writer (TopSpin / XWIN-NMR), data file, byte order, sample type, AQ_mod, dimensions, encodings, NUS, and the digital-filter group delay (available, removed only when processing). The table of validated feature values and the confidence level are generated from the development corpus by `cargo xtask assurance-audit`.
