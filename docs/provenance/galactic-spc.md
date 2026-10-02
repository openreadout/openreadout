# Provenance log — Galactic / Thermo GRAMS `.spc`

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

## 2026-09-26 — first reader (Richard Zimring with Claude as assistant)

**Prior art consulted** (permissively licensed; documentation and source read, no code copied):
- SpectroChemPy 1.0.0, CeCILL-B (a BSD-like permissive licence),
  `spectrochempy/core/readers/read_spc.py` (the copy installed in `oracle/.venv`): the 512-byte
  new-format header (flag byte, version byte 0x4B little-endian / 0x4C big-endian, experiment
  technique, exponent, point count, first and last x as float64, subfile count, x/y/z unit
  codes, packed collection date, resolution and source text, comment, axis-label text, log
  offset, modification flags, method, z increment, W-plane count and increment), the 256-byte
  old format (0x4D: float32 point count and x range, split date fields, word-swapped 32-bit
  values), the flag bits (16-bit values, multifile, random and ordered z, axis labels, per-subfile
  x arrays, an x array before the y values), the 32-byte subfile header (flags, exponent, index,
  z time and next time, noise, point count, co-added scans, W level), the fixed-point value rule
  (value = 2^exponent × integer / 2^32, or / 2^16 for 16-bit values; exponent −128 means IEEE
  float32 values), the subfile directory of per-subfile-x files, the log block (sizes and the
  offset of its text), and the x/y unit and technique code tables. SpectroChemPy quotes the
  layout comments of Galactic's public format description.
- spc-io 0.2.1 (h2020charisma), MIT, https://github.com/h2020charisma/spc-io
  (`spc_io/low_level/spc_raw.py`, `sub_file.py`, `headers/*.py`): the same layout as ctypes
  structures, the bit layout of the packed date (minute 6 bits, hour 5, day 5, month 4, year 12,
  least significant first) and of the modification flags.
- Both are also run as oracles (`oracle/spectro.py`, `spc()`): SpectroChemPy primary, spc-io as
  a recorded second opinion.

**Corpus files used** (all Zenodo, CC-BY-4.0): `zenodo7482510-p3-f-c-wb-aq-3`,
`zenodo7482510-water-2020-02-27` (Agilent MicroLab export, float), `zenodo20218285-001-backing-pristine-1`
(OMNIC export, 32-bit fixed point, exponent 0), `zenodo20328362-016-backing-det1` (Spectragryph,
float, nm axis), `zenodo16108826-sample-45-21`, `zenodo16108826-sample-5-13` (OMNIC, fixed point,
exponents 2 and 0), `zenodo22745748-pf1801` (Digilab, float, log block with instrument settings),
`zenodo15233137-box9-b3-n-s-mapping` (WiRE export, 2256 subfiles), `zenodo10391436-ters-map-cycle300`
(AIST-NT, common x array, 2200 subfiles on 44 W planes), `zenodo8161216-greenriver-organictype2`
(Spectragryph, transmission), `zenodo2248038-nujol1`, `zenodo14601517-bi183` (old 0x4D format).

**Observed** (a Python walker written from the above, `struct` only):
- Single-spectrum files store an exponent in the main header and a different one (0) in the
  subfile header (`zenodo16108826-sample-45-21`: main 2, subfile 0); the layout comments say the
  subfile exponent is ignored unless the file is a multifile. SpectroChemPy uses the main
  exponent; spc-io the subfile's.
- Every subfile of a multifile holds a full subfile header; the y values follow exactly
  (subfiles tile the file up to the log block, which starts at the header's log offset).
- The WiRE multifile numbers every subfile 0 and carries an impossible collection year (1385);
  the AIST-NT file numbers subfiles 0…2199 with increasing z times and 44 W planes.
- Log text is `key=value` (or `KEY = value`) lines ending in CR LF, then NUL.
- Old-format files hold one 2388-point spectrum whose 32-bit values have their two 16-bit words
  in most-significant-first order.

**Our choices (Inferred):**
- A file is one trace; subfiles are sweeps (spectra). The main exponent scales single-subfile
  files, each subfile's own exponent scales multifile subfiles; exponent −128 → float32.
- Per-subfile z and W values and scan counts form a table when the file is a multifile.
- Refused (exit 6): big-endian (0x4C) files, per-subfile x arrays (TXYXYS), old-format multifiles
  and 16-bit old-format values — none is in the corpus.
- `recorded_at` from the packed date when year, month and day are plausible.

## 2026-09-26 (later) — detection limited to `.spc` files

**Corpus files:** all development inputs (full corpus run). **Prior art:** none new. **What was found.** The first detection rule also claimed, without the `.spc` extension, any file whose byte 1 is 0x4B and whose next fields happened to parse: six plate-reader `.xlsx` exports (zips start `PK`, and `K` is 0x4B) were opened as SPC and refused. The SPC header has no signature, so only `.spc` files are claimed now, and the header check rejects zips, inconsistent XY-XY flags and large experiment codes.
