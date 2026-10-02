# Provenance log — MRC / CCP4 / MAP (`mrc`)

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

MRC2014 is an open, published standard (Cheng et al. 2015, J. Struct. Biol. 192:146–150; specification page maintained by CCP-EM). Most normalized fields therefore carry `Source::Spec`.

## 2026-09-22 — initial derivation (Richard Zimring with Claude as assistant)

**Specification consulted** (public):
- CCP-EM, "MRC2014 — MRC/CCP4 2014 file format", https://www.ccpem.ac.uk/mrc_format/mrc2014.php (retrieved 2026-09-22). Used for: the 1024-byte main header (56 words + 10 × 80-byte labels), every word's meaning (NX/NY/NZ, MODE 0/1/2/3/4/6/12/101, NXSTART…, MX/MY/MZ, CELLA/CELLB, MAPC/MAPR/MAPS, DMIN/DMAX/DMEAN, ISPG, NSYMBT, EXTTYP at byte 105, NVERSION at byte 109, ORIGIN, `MAP` at byte 209, MACHST at 213, RMS, NLABL), note 3 (MZ = sections per volume, NZ/MZ volumes in a stack), note 5 (statistics marked undetermined by DMAX < DMIN, DMEAN < min(DMIN, DMAX), RMS < 0), note 6 (ISPG 0 = image or image stack, 1 = volume, 401–630 = volume stack), note 7 (NSYMBT = extended-header bytes), note 8 (EXTTYP codes CCP4, MRCO, SERI, AGAR, FEI1, FEI2, HDF5), note 9 (NVERSION = year × 10 + version: 20140, 20141), note 10 (ORIGIN), note 11 (machine stamp nibbles). The page's rendering of note 11 is cut off after "0x44 0x44 0x00 0"; the big-endian stamp 0x11 0x11 is taken from the IMOD documentation and mrcfile (below).
- IMOD "The MRC file format used by IMOD", https://bio3d.colorado.edu/imod/doc/mrc_format.txt (linked from the CCP-EM page; documentation of a GPL program, read as documentation). Used for: mode 101 packing (two 4-bit values per byte, lower coordinate in the low nibble, rows padded to `(nx + 1) / 2` bytes, same for both byte orders); mode 16 (3 × unsigned byte RGB, non-standard); the IMOD stamp 1146047817 at byte 153 and flag word at byte 157 (bit 1 = bytes are signed); the stamps 17 17 (big-endian) and 68 65 (little-endian) seen in older files; the SerialEM `SERI` extended header (`nint` = bytes per section at byte 129, `nreal` = bit flags at byte 131: tilt × 100, piece coordinates, stage × 25, magnification / 100, intensity × 25000, exposure dose as a two-short float); that NVERSION is 0 in files that use non-standard modes.

**Prior art consulted** (permissively licensed; documentation and source read, no code copied):
- `mrcfile` 1.5.4 (commit `a2a8c6b`), CCP-EM / Science and Technology Facilities Council, BSD-3-Clause, https://github.com/ccpem/mrcfile — `dtypes.py` (header field layout; FEI1/FEI2 extended-header block layout: 768-byte FEI1 block, FEI2 fields appended from byte 768; the block takes the header's byte order except the four bitmask words, which are always little-endian), `utils.py` (mode ↔ dtype table; machine stamps 0x44 0x44 / 0x44 0x41 → little-endian, 0x11 0x11 → big-endian; ISPG 401–630 = volume stack; data shape `(nz, ny, nx)` for stacks and volumes, `(nz / mz, mz, ny, nx)` for volume stacks), `mrcinterpreter.py` (permissive reading: `MAP` compared on its first three bytes; an invalid machine stamp falls back to little-endian; when the mode is invalid in the stamp's byte order but valid in the other, the other order is used), `mrcobject.py` (`voxel_size` = CELLA / (MX, MY, MZ), not permuted by MAPC/MAPR/MAPS; `validate`: positive dimensions, MAPC/MAPR/MAPS a permutation of 1,2,3, NZ divisible by MZ for volume stacks, NLABL equal to the number of non-blank labels, NVERSION 20140/20141, EXTTYP set when NSYMBT > 0, file size = 1024 + NSYMBT + data). `mrcfile` is also the oracle (`oracle/gen.py`).

**Corpus files used:** `mrcfile-emd-3197`, `mrcfile-emd-3001` (EMDB, CC0), `mrcfile-fei-extended`, `mrcfile-epu2.9-example` (mrcfile test data, BSD-3-Clause, contributed with permission for public distribution), `empiar10045-class2d-it025` (EMPIAR, CC0), `zenodo16462008-8RRH-molmap-30A`, `zenodo10526574-IPPK-TS08-rec`, `zenodo2578866-HAADF-aligned` (Zenodo, CC-BY-4.0). Header values were dumped with `mrcfile` (`permissive=True, header_only=True`).

**Observed in the corpus:**
- Machine stamps `44 41 00 00` (EMDB, RELION, ChimeraX, the HAADF tilt series) and `44 44 00 00` (EPU, IMOD/SerialEM).
- `EMD-3001`: MAPC/MAPR/MAPS = 3, 1, 2 (crystallographic section order), ISPG 4, NSYMBT 160 with EXTTYP zero bytes (CCP4 symmetry records), NXSTART/NYSTART/NZSTART 0/−21/−12, NVERSION 0.
- `zenodo10526574-IPPK-TS08-rec`: mode 0 with the IMOD stamp 1146047817 and flags 9 (bit 1: signed bytes; bit 8: RMS negative when not computed), ISPG 1, EXTTYP four spaces, nine labels (SerialEM, alignframes, NEWSTACK, CCDERASER, TILT, clip).
- `zenodo2578866-HAADF-aligned`: CELLA = (NX, NY, NZ) with MX/MY/MZ = NX/NY/NZ, i.e. 1 Å per pixel: a placeholder written when no calibration is known. Reported as 1 Å (it is what the header says) with a note.
- `empiar10045-class2d-it025`: CELLA = 0 (no calibration), ISPG 0, NZ = 20 (image stack).
- `zenodo16462008-8RRH-molmap-30A`: ChimeraX writes ISPG 0 for a volume.
- FEI files: EXTTYP `FEI1` with NSYMBT 786432 = 1024 × 768 (space reserved for 1024 frames, 1 used) and `FEI2` with NSYMBT 909312 (= 1024 × 888).

**Decisions (ours, documented in `docs/formats/mrc.md`):** pixels are returned in file order (columns = X, rows = Y, sections = Z or T) without applying MAPC/MAPR/MAPS; the physical size of each file axis is the cell sampling of the crystallographic axis it maps to. ISPG 0 with NZ > 1 is an image stack (T), following the spec and mrcfile, except for `.map`/`.rec` files, which are volumes by convention. Mode 12 is widened to float32, mode 101 unpacked to uint8, complex modes 3/4 exit 6.

## 2026-09-25 — assurance profile (`src/assurance.rs`)

**Corpus files:** every development input of this format on disk (`corpus/assurance/evidence.json` lists them); no held-out file.
**Prior art consulted:** none.
**What was done.** No parsing logic changed. The reader declares an assurance profile (docs/assurance.md) that fingerprints each file from this reader's own normalized output: the NVERSION, mode, layout, extended-header type, permuted axes and IMOD unsigned bytes; a 1 Å pixel with CELLA = grid size is reported as assumed. The table of validated feature values and the confidence level are generated from the development corpus by `cargo xtask assurance-audit`.

## 2026-09-26 — differential cross-check with Bio-Formats

**Corpus files:** every development file of this format on disk up to 400 MB (`corpus/oracle/second/*.json`); no held-out file.
**Prior art consulted:** Bio-Formats 8.5.0 (GPL) `showinf`/`bfconvert`, run as black boxes by `oracle/second_opinion.py`.
**Observations and adjudications:** `corpus/oracle/second/adjudications.toml` (differences are conventions or decoder rounding of the second reader; files whose series Bio-Formats groups differently are not compared).
**Inferred.** Nothing new; no reader change.

## 2026-09-26 — gzip-compressed maps and complex modes 3/4

**Corpus files:** `mrcfile-emd-3197-gz`, `mrcfile-emd-3001-gz` (ccpem/mrcfile test data at commit a2a8c6b, the gzip copies of the two EMDB maps already in the corpus), `emdb-emd-1026-gz`, `emdb-emd-1016-gz` (EMDB FTP, `emd_N.map.gz` as distributed). None is a held-out record.
**Prior art consulted:** the MRC2014 format description at ccpem.ac.uk/mrc_format/mrc2014.php (public; mode 3 = "complex 16-bit integers", mode 4 = "complex 32-bit reals"); RFC 1952 (gzip). mrcfile 1.5.4 (BSD-3) is the oracle, reading the gzip-decompressed file.
**What was inferred from what.** Complex samples are (real, imaginary) pairs of the mode's scalar type, NX pairs per row: from the MRC2014 table (the header's NX counts complex values). Mode 3's int16 parts are widened to float32, which is exact. A transform stored as a half plane (NX = N/2 + 1) is returned as stored, not expanded: the header does not say whether the file holds a half or a full transform. No public complex MRC file was found (searches: Zenodo "mrc fft", EMPIAR, mrcfile/IMOD test data), so modes 3/4 are covered by synthetic files only (`tests/synthetic_mrc.rs`, both byte orders). The gzip maps decompress to files identical in layout to the plain ones and match mrcfile on every compared plane.

