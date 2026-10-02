# Renishaw WiRE `.wdf` provenance

## 2026-09-23 — before the reader was written (Richard Zimring with Claude as assistant)

**Corpus files used:** the six example files of py-wdf-reader's binary release
(`spectra_files.zip`: single spectrum, line scan, depth series, StreamLineHR map of 11,227
spectra, StreamLine map, undefined type; MIT, github.com/alchem0x2A/py-wdf-reader), the WiRE
files of Zenodo record 8102788 (CC0-1.0: single Raman spectra of Ediacaran/Cambrian artefacts,
each with the text file WiRE exported from it), and Orange-Spectroscopy's
`single-spectrum_undefined-type.wdf` (GPL-3.0-or-later data file, commit fc7cc69).

**Permissive prior art consulted:** renishawWiRE 0.1.16 (T. Tian, MIT; LICENSE read first),
`renishawWiRE/wdfReader.py` and `types.py`: the block chain (4-character name, int32 uid,
int64 size), the `WDF1` header offsets (points per spectrum at 0x3C, capacity, count,
accumulations, x/y list lengths, origin-list count, application name and version, scan and
measurement type, spectral unit and laser wavenumber at 0x98, user and title at 0xD0/0xF0),
`DATA` as float32 rows, `XLST`/`YLST` (type, unit, float32 values), the `ORGN` origin lists,
the `WMAP` grid fields and the `WHTL` JPEG. We use a numeric code from renishawWiRE's
enumerations only where corpus files confirm its meaning (e.g. the x-list unit code of a Raman
shift axis whose values run 100…3200 cm⁻¹, the time origin list whose values step by the
exposure).

**Reference reader:** RosettaSciIO's WDF reader (GPL-3.0), run as a black-box oracle.

**Property sets (`PSET` in `WXDA`, `WXDM`, `WXCS`, `WXIS`, `ZLDC`, `MAP `)** are decoded from
hex dumps of the corpus files alone (none of the permissive readers decodes them); findings are
recorded in the format notes.

## 2026-09-23 — implementation and validation (Richard Zimring with Claude as assistant)

**Findings from hex dumps (none of them in renishawWiRE):** the header's FILETIMEs at 0x88/0x90
(start and end; the first spectrum's `ORGN` time lies between them); the `PSET` item encoding
(type byte, flag byte with 0x80 = array and 0x40 = compressed text, key, value; `k` items name the
keys with bit 15 set; `p` nests); `q` items are float64 (e.g. `NDPercent` 5.0, `slitOpening` 10.0);
the `MAP ` blocks hold, after their property set, a u64 count and one float32 per spectrum — WiRE's
own map analyses; WMAP flags 0 (row order) and 2 (column order: StreamLine) are handled by placing
spectra from their stage coordinates; the predefined keys normalized were identified from their
values in the six WiRE 4.1/4.4 files (`x50` objective, `532 nm edge` laser, serial numbers).

**Validation:** renishawWiRE on all 10 files (values, Raman-shift lists, coordinates, laser,
accumulations, map images laid out on its grid); WiRE's text exports of 3 spectra; WiRE's
`Intensity At Point 1315/1365` analyses of 11,227 spectra equal our spectra linearly interpolated
to 6 × 10⁻⁸.

## 2026-09-24 — Stage-coordinate columns named by data type (`z_um` of LiveTrack maps; N-L2 of the second generalization report)

**Problem:** a WiRE map lacked the per-spectrum `z_um` column.

**Corpus file used** (new; not from a held-out record): `wdf-zenodo10694741-streamline-livetrack`, WiRE 5.4, "StreamLine image acquisition 3", a 91 × 91 map with 8,281 spectra, CC-BY-4.0 (Zenodo 10694741, Mitsutake, Rutledge, Bordallo, de Paula; licence from the record's API metadata, 2026-09-24).

**Observed** (the file's `ORGN` block, read with the Python standard library):
- The lists are X (type 3, unit 5, name `X`), Y (type 4, unit 5, `Y`), and **Z (type 5, unit 5, name `Z data`, not primary)**, followed by type 24 `Z actual` (unit 5), type 25 `Z difference` (unit 5), type 26 `LT Signal Used` (unit 0), time, flags and checksum.
- We named coordinate columns from the list *name* (`{name}_um`), so the Z list became `z data_um` and no `z_um` existed. renishawWiRE (MIT, read as documentation) picks the X/Y/Z lists by data type 3/4/5, not by name.
- Other search results (Zenodo 4018475 XZ depth slices, 10694741's StreamHR surfaces, 14245518 volumes) name their Z list `Z`, again with type 5 and unit 5.

**Change:** data types 3/4/5 are always `x_um`/`y_um`/`z_um`. Values are converted to µm from the unit code: 5 µm, 8 mm, 9 m, 3 nm. A list in another unit keeps its values in a column named after the axis (`x`/`y`/`z`) with no unit. Any other list in µm (types 24/25, `Z actual`, `Z difference`) gets `{name}_um` with unit µm, spaces becoming `_`.

**Validation:** renishawWiRE's `xpos`/`ypos`/`zpos` against our `x_um`/`y_um`/`z_um` (hashes in the oracle); the other 10 WDF files are unchanged.

## 2026-09-24 — fuzz findings, bounds only

**Scope:** bounds only; nothing the reader infers from the files changed. The `whole_wdf` fuzz target (90 s, first run; seeds cut from `wdf-zenodo8102788-ooid.wdf`, CC0-1.0) found two ways a malformed `.wdf` could crash the reader instead of returning an error:

- the stride of an `ORGN` origin list was `24 + capacity × 8` with the capacity (spectra) taken from the header: a capacity near 2^61 overflowed the addition (a panic with overflow checks). The stride and the list offsets now saturate, and a list past the end of the file is a short read as before;
- the `spectra` table's index column was built from the header's spectrum count (up to 2^32 − 1 values, a 34 GB allocation) whenever any origin list was present, even when no list held that many values. The table is now built only when at least one list has one value per spectrum, which bounds it by the bytes in the file.

Regression fixtures: `crates/openreadout-spectro/tests/fixtures/malformed/file-fuzz-wdf-orgn-stride-overflow.wdf` and `file-fuzz-wdf-spectrum-count-alloc.wdf`, replayed through every reader by `crates/openreadout-cli/tests/fuzz_regressions.rs`. No corpus file changes: the corpus test results are identical.

## 2026-09-25 — assurance profile (`src/assurance.rs`)

**Corpus files:** every development input of this format on disk (`corpus/assurance/evidence.json` lists them); no held-out file.
**Prior art consulted:** none.
**What was done.** No parsing logic changed. The reader declares an assurance profile (docs/assurance.md) that fingerprints each file from this reader's own normalized output: the WiRE major version, measurement and scan types, x-list type and white-light image layout. The table of validated feature values and the confidence level are generated from the development corpus by `cargo xtask assurance-audit`.
