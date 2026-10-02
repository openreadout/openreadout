# Provenance log — JCAMP-DX

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

JCAMP-DX is an open IUPAC standard for exchanging spectra as printable ASCII. Its structure comes from the published protocols, so most normalized fields carry `Source::Spec`.

## 2026-09-22 — initial derivation (Richard Zimring with Claude as assistant)

**Specifications consulted** (public; the IUPAC CPEP Subcommittee on Electronic Data Standards publishes them at http://www.jcamp-dx.org/protocols.html, retrieved 2026-09-22):
- R. S. McDonald, P. A. Wilks Jr., "JCAMP-DX: A Standard Form for the Exchange of Infrared Spectra in Computer Readable Form", Appl. Spectrosc. 42(1) 151–162 (1988), `protocols/dxir01.pdf`. Used for: labeled data records (LDR, `##label=`, labels compared after upper-casing and removing spaces, dashes, slashes and underscores, §4.4), `$$` comments, user-defined labels starting with `$`, AFFN numbers (§4.5.3; `E` followed by a sign and digits is an exponent), the ASDF compression forms and their pseudo-digits (SQZ `@A–I`/`a–i`, DIF `%J–R`/`j–r`, DUP `S–Z s`, PAC separators; §5, Table VII), `?` as the invalid-data symbol (§5.7), X-sequence and Y-value check-points (§5.8: when a line ends in DIF form the next line repeats the last ordinate; the last line of a DIF block holds only the check value), `##XYDATA=(X++(Y..Y))` with X incremented by (`LASTX`−`FIRSTX`)/(`NPOINTS`−1) (§5.1.1, §6.4.1), `##XFACTOR=`/`##YFACTOR=` (§6.2.5), `##FIRSTX=`/`##LASTX=`/`##FIRSTY=`/`##NPOINTS=` (§6.2), `##XYPOINTS=` and `##PEAK TABLE=` `(XY..XY)` groups (§6.4.2–6.4.3), compound files with `##BLOCKS=` and LINK blocks, one `##END=` per `##TITLE=` (§6.1).
- A. N. Davies, P. Lampen, "JCAMP-DX for NMR", Appl. Spectrosc. 47(8) 1093–1099 (1993), `protocols/dxnmr01.pdf` ("The specifications are placed in the public domain"). Used for: `##DATA TYPE=` values (NMR FID, NMR SPECTRUM, NMR PEAK TABLE, NMR PEAK ASSIGNMENTS), `##DATA CLASS=` (XYDATA, XYPOINTS, PEAK TABLE, ASSIGNMENTS, NTUPLES), `(X++(R..R))`/`(X++(I..I))` variable lists, `##.OBSERVE FREQUENCY=` (MHz), `##.OBSERVE NUCLEUS=`, `##.ACQUISITION MODE=`, `##.DELAY=`, and the NTUPLES layout (`##NTUPLES=`, `##VAR_NAME=`, `##SYMBOL=`, `##VAR_TYPE=`, `##VAR_FORM=`, `##VAR_DIM=`, `##UNITS=`, `##FIRST=`, `##LAST=`, `##MIN=`, `##MAX=`, `##FACTOR=`, `##PAGE=`, `##DATA TABLE=`, `##END NTUPLES=`).
- P. Lampen et al., "JCAMP-DX for Mass Spectrometry", Appl. Spectrosc. 48(12) 1545–1552 (1994), `protocols/dxms01.pdf`. Used for: MASS SPECTRUM peak tables, CONTINUOUS MASS SPECTRUM XYDATA, NTUPLES spectral series with one `##PAGE=` per spectrum.
- "JCAMP-DX Extension 5.01" (IUPAC, 1999), `protocols/dx5-01-correctedv2.pdf`. Used for: `##LONG DATE=`, `##VAR_DIM=` with a PAGE variable, `##BLOCK_ID=`.

**Prior art consulted** (permissively licensed; documentation and source read, no code copied):
- `nmrglue` 0.12, BSD-3-Clause, https://github.com/jjhelmus/nmrglue — `nmrglue/fileio/jcampdx.py`: nested blocks as a stack, the DIF check-point skip (the first ordinate of a line is dropped when the previous line ended in DIF or in a DUP of a DIF), DUP repeating a difference in DIFDUP form, NTUPLES pages with `R..R`/`I..I` tables scaled by the per-variable `##FACTOR=`.
- `jcamp` 1.2, Nathan Hagen, MIT, https://github.com/nzhagen/jcamp — README and `jcamp.py`: compound files expose child blocks; `(XY..XY)` tables are parsed as pairs.
- Both are also run as black-box oracles (`oracle/gen.py`, branch `jcamp-dx`).

**Corpus files used** (in `corpus/manifest.toml`):
- The ISAS Dortmund JCAMP-DX test suite (`DX-DIR.TXT`: "JCAMP-DX Testfiles, (C) COPYRIGHT ISAS Dortmund 1992, 1994, 1995", the test disk announced in the NMR protocol §7), as redistributed in the MIT-licensed `nzhagen/jcamp` repository, commit `9a4af7d`: `jcamp-isas-brukaffn` (AFFN), `-bruksqz` (SQZ), `-brukdif` (DIFDUP), `-brukpac` (PAC), `-brukntup` (NTUPLES real/imaginary), `-testntup`, `-testfid` (NTUPLES FID), `-pe1800` (IR, PAC), `-isas-ms1` (MS peak table), `-isas-ms2` (continuous MS, descending X), `-isas-cdx` (compound file with a JCAMP-CS structure block and an NMR peak-assignment block).
- `jcamp-uvvis-toluene` (UV-Vis; `nzhagen/jcamp`, MIT).
- nmrXiv study S200 (project P33 CENAPTNMR, CC0-1.0): MestReNova 14.0 JCAMP-DX 6.0 exports: a 1D NMR SPECTRUM (`##XYDATA=(X++(Y..Y))` inside a LINK block) and 2D NTUPLES spectra (`##DATA TABLE=(F2++(Y..Y)), PROFILE` pages keyed by `F1=`).

**Observed in the corpus (and now handled):**
- Leading blanks before `##` (`TESTNTUP.DX`), CRLF line ends, `$$` comments after data values.
- `##JCAMPDX=` and `##JCAMP-DX=` spellings; `##DATA CLASS= PEAKTABLE` without the space (`ISAS_MS1.DX`) — both covered by label/value normalization.
- NTUPLES whose X column of each data line is an index scaled by the X `##FACTOR=` (`TESTNTUP.DX`: `16383` × 1.4672 = 24038.5 Hz).
- MestReNova 2D NTUPLES repeat `##FIRST=` inside every page and use `F2` instead of `X` as the independent symbol.
- ISAS_MS2 stores descending X (`FIRSTX` 13.998 > `LASTX` 6.999).

**Inferred, not in the protocols:** exposing a regularly spaced X axis in trace `extra.axis` and irregular abscissas (`XYPOINTS`, `PEAK TABLE`) as a channel named `x`; grouping NTUPLES pages keyed by `N` into channels and pages keyed by any other variable into sweeps.

## 2026-09-22 — robustness (fuzzing)

**Scope:** a bound only; decoding is unchanged. The `jcamp_asdf` fuzz target (3 min) found a 72-byte ASDF table whose DUP count asked for 2^30 ordinates (8 GiB). One table may now decode to at most 2^24 ordinates (128 MiB), far above any 1D spectrum; NTUPLES pages are decoded one by one. Fixture: `crates/openreadout-nmr/tests/fixtures/malformed/asdf-fuzz-dup-count-8gib.txt` (fuzzer output).

**Corpus files used:** seeds cut from `jcamp-lancashire-dupinc1.jdx` and `jcamp-lancashire-sqzdec1.jdx` (public domain). **Prior art consulted:** none.

## 2026-09-23 — JCAMP-DX writer (`export --to jcamp`) (Richard Zimring with Claude as assistant)

**Specifications consulted:** the same public protocols as above (McDonald & Wilks 1988 §5 for SQZ/DIF/DUP and the Y-value check-points, including the final check line; Davies & Lampen 1993 for the NMR `DATA TYPE`s, the NTUPLES header records and `R`/`I` pages; §6.4.2–6.4.3 for `(XY..XY)` tables).

**Prior art consulted** (permissive; read as documentation, no code copied): `nmrglue` 0.12 `fileio/jcampdx.py` (BSD-3-Clause) — its NTUPLES reader takes the real/imaginary factors from `##SYMBOL=` entries named exactly `R` and `I`, so complex data are written with those symbols; `jcamp` 1.3.2 `jcamp.py` (MIT) — its tokenizer accepts no minus sign in `(X++(Y..Y))` lines (outside exponent notation), no `$$` comment on a data line, and only one-character DUP counts, so the writer keeps data lines comment-free and DUP counts ≤ 9; negative abscissas stay correct per the protocol (that package cannot read them).

**Corpus files used for validation** (round trip through our reader, nmrglue and jcamp; `oracle/jcamp_validate.py`): `jcamp-isas-brukaffn`, `-bruksqz`, `-brukdif`, `-brukpac`, `-brukntup`, `-testntup`, `-testfid`, `-pe1800`, `-specfile`, `-ms1`, `-ms2`; `jcamp-lancashire-sqzdec1`, `-dupinc1`, `-pacdec1`, `-blckpac1`, `-compound`, `-mactab2`, `-pktab1`; nmrXiv S200 1D spectra; Bruker `nmrglue-test-data/bruker_1d`, `bruker_2d`, nmrXiv S275 `1` and `1/pdata/1`; ANDI chromatograms `cheminfo-agilent-hplc`, `mtbls390-WB_CC_BAT_01`. Every export was exact (bit-for-bit ordinates).

**Inferred (ours):** the factor choice (the channel's own scale, else a power of two, else 32-bit quantization), `XFACTOR` = |step| with integer X-check values (as the Bruker files in the corpus do), the page numbering for multi-sweep NTUPLES (one page per channel and sweep, `N` = sweep + 1), `##$OPENREADOUT SOURCE=`.

## 2026-09-23 — `traces[].start_s` for time abscissas (Richard Zimring with Claude as assistant)

**Scope:** `traces[].start_s` only; decoding is unchanged. A cross-reader audit of every corpus trace (does `start_s` equal the time axis' `first`, and does `step` equal 1/`sample_rate_hz`?) found JCAMP-DX the one reader that left `start_s` unset for a time abscissa: `jcamp-isas-testfid` (`##XUNITS= SECONDS`, FIRSTX 0) and `jcamp-isas-ms2` (`##XUNITS= SECONDS`, FIRSTX 13.998, LASTX 6.999: a magnet scan recorded against falling time).

**Inferred (ours):** when `##XUNITS=` is a time unit (`SECONDS`, `MS`, `MINUTES`; the same set that gives `sample_rate_hz`) and the abscissa increases, `start_s` = FIRSTX (or `##FIRST=` of the NTUPLES independent variable) converted to seconds. A falling time abscissa keeps `start_s` unset, because `start_s + i / sample_rate_hz` would run the wrong way; `extra.axis` (`first`, negative `step`) remains the way to place its samples.

**Corpus files used:** `jcamp-isas-testfid`, `jcamp-isas-ms2` (ISAS test files, public). **Prior art consulted:** none (JCAMP-DX 4.24/5.0 protocols as above).

## 2026-09-25 — assurance profile (`src/assurance.rs`)

**Corpus files:** every development input of this format on disk (`corpus/assurance/evidence.json` lists them); no held-out file.
**Prior art consulted:** none.
**What was done.** No parsing logic changed. The reader declares an assurance profile (docs/assurance.md) that fingerprints each file from this reader's own normalized output: the JCAMP-DX version, data type and table kind. The table of validated feature values and the confidence level are generated from the development corpus by `cargo xtask assurance-audit`.

## 2026-09-26 — `##END` without `=`

**Corpus files:** none new; a synthetic unit test (`end_without_equals_closes_the_block`). **Prior art:** none. **What was inferred from what.** A held-out measurement (draw C, finding C-L1) reported that `check` exits 4 on an intact file whose last line is `##END` with no `=`; the held-out file itself was not opened. JCAMP-DX labels are `##LABEL=`, but `##END` is unambiguous without the `=`, so it now closes the open block with an info finding (`end_without_equals`) instead of being skipped as a bad label (which left the block unclosed: "truncated file?").
