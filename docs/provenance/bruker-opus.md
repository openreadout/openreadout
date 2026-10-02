# Bruker OPUS provenance

## 2026-09-23 — before the reader was written (Richard Zimring with Claude as assistant)

**Corpus files used** (all in `corpus/manifest.toml` with their licences): the eight OPUS test
files of opusreader2 (`inst/extdata`, MIT, commit 77a1ee4), the example file of brukeropus
(`examples/file.0`, MIT, commit af5a508), the chrysene 16/08/2009 synchrotron FTIR folder of
Zenodo record 1309915 (CC-BY-4.0: OPUS `.0/.1/.2` files and the OPUS CSV export
`chrysene_003_00.CSV` of the same measurement), the 26 bitumen spectra of Zenodo record 10866787
(CC-BY-4.0, `OPUS Spectroscopy files.zip`) and Orange-Spectroscopy's `peach_juice.0` with its
OPUS data-point-table export `peach_juice.dpt` (GPL-3.0-or-later data files, commit fc7cc69).

**Permissive prior art consulted:**
- brukeropus 1.4.3 (Josh Duran, MIT; `brukeropus-1.4.3.dist-info/LICENSE` read first):
  `file/parse.py` (header at byte 4: float64 version, int32 directory start, maximum and used
  block counts; 12-byte directory records type/size-in-words/offset; the six-field split of the
  type word; parameter records of a 3-character name, a 16-bit type and a 16-bit size in 2-byte
  words; data blocks of float32; the series (3D) block header), `file/block.py` (which type
  fields mark data, data-status, parameter, directory and history blocks; pairing a data block
  with its data-status block by type), `file/data.py` (x axis `linspace(FXV, LXV, NPT)`, y =
  `CSF` × stored value, trimmed to `NPT`) and the short type-code tables of
  `file/constants.py` (`CODE_0`…`CODE_5`, `CODE_3_ABR`). Our
  parameter names and meanings come from the values in corpus files and from Bruker's public
  instrument brochures' vocabulary (resolution, scans, apodization, beamsplitter, source,
  detector).
- opusreader2 (spectral-cockpit, MIT) README only.

**Inferred from hex dumps before coding** (`scratchpad` dumps of `test_spectra.0` and `file.0`):
magic `0a 0a fe fe`; version 920622.0 in every file; directory at 24 with 40 slots; data-status
blocks carry `DPF NPT FXV LXV CSF MXY MNY DAT TIM DXU`; data blocks often hold a few more floats
than `NPT` (19280 bytes = 4820 floats for `NPT` 4819); bit 30 of the type word is set on some
blocks and not others in one file and carries no meaning we could find (masked off, as in
brukeropus); a block with type field 5 = 6 (`test_spectra.0` block 19) is neither data nor
parameters in brukeropus's tables and is listed, not decoded.

## 2026-09-23 — implementation and validation (Richard Zimring with Claude as assistant)

**Our findings, beyond brukeropus:** the later of two blocks of the same kind is the one OPUS
exported (`peach_juice.dpt` matches block 25, not block 18), so traces put it first (brukeropus
names the earlier one `r_2`); blocks whose type word is all zeros hold stale parameter copies;
`DAT` is `dd/mm/yyyy` or `yyyy/mm/dd` and `TIM` carries the offset (`(GMT+12)`), giving ISO 8601
with that offset; the instrument block's `SRT` is local wall-clock time counted as if UTC (it
equals `DAT`+`TIM` without the offset), so it is not used; `ZFF` is stored as text.

**Validation:** 16 fetched files: every trace bit for bit against brukeropus (65 traces), the OPUS
CSV export of `chrysene_003.1` and the data-point table of `peach_juice.0` point by point.
brukeropusreader (GPL-3.0, black box) agrees on single-absorbance files.

## 2026-09-25 — assurance profile (`src/assurance.rs`)

**Corpus files:** every development input of this format on disk (`corpus/assurance/evidence.json` lists them); no held-out file.
**Prior art consulted:** none.
**What was done.** No parsing logic changed. The reader declares an assurance profile (docs/assurance.md) that fingerprints each file from this reader's own normalized output: the OPUS version, data type, block types and undecoded data blocks. The table of validated feature values and the confidence level are generated from the development corpus by `cargo xtask assurance-audit`.

## 2026-09-26 — the OPUS release from the history block

**Corpus files:** the development OPUS files (opusreader2 extdata, brukeropus example, Orange, Zenodo 1309915 and 10866787). No held-out file.
**Prior art consulted:** none new.
**What was inferred from what.** Every file with a history block carries a line `Version X.Y Build: a, b, c yyyymmdd` (6.5, 7.2, 7.5, 7.8, 8.1, 8.2, 8.5, 8.5(SP1), 8.7 across the corpus) next to installation paths such as `OPUS_8.1.29`, which fixes the reading as the OPUS software release. It is reported as `software_version` (text, as written) and the assurance profile records writer `OPUS` and the writer version (major.minor) as descriptive features.
