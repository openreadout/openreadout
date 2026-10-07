# Provenance log — Bruker timsTOF TDF / TSF (`.d` directories)

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

A timsTOF acquisition is a `.d` directory. TDF data (trapped ion mobility, `analysis.tdf` + `analysis.tdf_bin`) and TSF data (no mobility dimension, `analysis.tsf` + `analysis.tsf_bin`) keep their metadata in an SQLite database and the peak data in a binary file of per-frame compressed blobs.

## 2026-09-22 — initial derivation (Richard Zimring with Claude as assistant)

**Specifications consulted** (public):
- The SQLite database file format, https://www.sqlite.org/fileformat2.html (SQLite is public domain): 100-byte header, page size and reserved bytes, b-tree page layout (table interior/leaf pages, cell pointer arrays), record format (serial types, varints), overflow pages and the local-payload formula, `sqlite_schema` on page 1, `INTEGER PRIMARY KEY` as a rowid alias, and the write-ahead log (32-byte WAL header, 24-byte frame headers, salts, cumulative checksums, commit frames). Our read-only SQLite reader (`crates/openreadout-bruker-tims/src/sqlite.rs`) is written from this document alone.

**Prior art consulted** (permissively licensed; documentation and source read, no code copied):
- `timsrust` 0.6.6 (MannLabs), crate license Apache-2.0 (repository LICENSE file MIT), https://github.com/MannLabs/timsrust at commit `80e235a` — files read: `crates/timsrust-tdf/src/frame_reader/compression2.rs` (TDF frame blob: `u32` byte count and `u32` scan count at the frame's `TimsId` offset, then a zstd frame; the decompressed bytes are four byte-planes of `u32` values; value 0 is the scan count, values 1…n−1 are twice the peak count of each scan but the last, then (TOF-index delta, intensity) pairs where the TOF index restarts at each scan and is the running sum minus one), `compression1.rs` (read only to learn that compression type 1 is LZF-based; not implemented), `frame_reader/frame_info_reader.rs` and `file_readers/sql_reader/*.rs` (which columns of `Frames`, `Precursors`, `PasefFrameMsMsInfo`, `DiaFrameMsMsInfo`, `DiaFrameMsMsWindows`, `GlobalMetadata` are used; `MsMsType` 8 = DDA-PASEF, 9 = DIA-PASEF, 0 = MS1), `calibration.rs` and `metadata.rs` (the m/z and 1/K0 conversions timsrust uses: `sqrt(m/z)` linear in the TOF index between `MzAcqRangeLower` and `MzAcqRangeUpper` over `DigitizerNumSamples`, widened by 5 on each side when `AcquisitionSoftware` is `Bruker otofControl`; 1/K0 linear in the scan index from `OneOverK0AcqRangeUpper` at scan 0 to `OneOverK0AcqRangeLower` at the largest `NumScans` − 1), `spectrum_reader/dda.rs` and `raw_spectra.rs` (a DDA-PASEF MS2 spectrum is the sum, per TOF index, of the scans `ScanNumBegin`…`ScanNumEnd` (end exclusive) of every PASEF frame listed for the precursor), `spectrum_reader/dia.rs` (DIA windows likewise, per window group), `precursor_reader/dda.rs` (precursor m/z = `MonoisotopicMz`, charge, `ScanNumber` → 1/K0, `Intensity`, parent frame → retention time), and `crates/timsrust-tsf/src/{blobs,mz,spectrum}.rs` (TSF line-spectrum blobs: `u32` padded chunk length, `u32` compressed length, zstd; then `NumPeaks` `f64` TOF indices followed by `NumPeaks` `f32` intensities; only `HasLineSpectra = 1` datasets).

**What is therefore approximate:** m/z and 1/K0 use the acquisition-range approximations above, exactly as timsrust computes them. The `MzCalibration` and `TimsCalibration` tables are read and reported verbatim under `vendor`, but their models are not applied: no public, permissively licensed description of them exists, and deriving them from Bruker's library would break rule 2. This is a documented gap (`known_gaps`), and every m/z and 1/K0 value we report is bit-identical to timsrust's.

**Oracle:** `oracle/timsrust-oracle` (a separate Cargo project, not part of the workspace) runs timsrust 0.6 as a black box: per frame the scan offsets, TOF indices and intensities (xxh3), and our spectrum definitions recomputed from timsrust's frames and converters (see `docs/formats/bruker-tdf.md`). The SQLite tables are cross-checked against Python's `sqlite3` in `oracle/gen.py`.

**Corpus files used:**
- `timsrust-test-dda` and `timsrust-test-dia`: `tests/test.d` and `tests/dia_test.d` of timsrust at commit `80e235a` (synthetic datasets made by timsrust's own simulator notebooks; MIT / Apache-2.0).
- `pxd074950-b1-diapasef`: PRIDE PXD074950 `B1_S2-E1_1_3363.d.zip` (timsTOF HT, diaPASEF, 66 s, 613 frames; EMBL-EBI terms of use). Its acquisition was not closed properly (`ClosedProperly = 0`): 4 KiB `analysis.tdf` with a 3 MB `analysis.tdf-wal`, so the WAL must be read.
- `pxd075355-dda-pasef`: PRIDE PXD075355, a 30-minute DDA-PASEF run on a timsTOF Pro (EMBL-EBI terms of use).

**Observed and handled:** `NumScans` differs between frames of one run only in synthetic data; the real files use one value. Empty frames (`NumPeaks = 0`) have a blob with zero peaks. The WAL of `pxd074950-b1-diapasef` holds every table; its last commit frame defines the database size.

**Not supported (exit 6):** TDF compression type 1 (early timsTOF, LZF), TSF profile spectra (`HasProfileSpectra` without line spectra), SQLite text encodings other than UTF-8, `WITHOUT ROWID` tables.

## 2026-09-22 — robustness (fuzzing)

**Scope:** no change to what the reader infers from a file; bounds only. An empty frame blob
(byte count 8) with a scan count of 2^32−1 in its blob header made `decode_tdf_frame` allocate
a 32 GiB scan-offset table. Empty frames may now declare at most 2^20 scans (instruments write
about a thousand); more is `corrupt_file`.

**Corpus files used:** `timsrust-test-dda` (Apache-2.0), with the first blob header rewritten
by hand: `crates/openreadout-bruker-tims/tests/fixtures/malformed/bundle-whole_tims-empty-blob-huge-scan-count.bin`
(`analysis.tdf`, the fuzz harness separator, then the altered `analysis.tdf_bin`).

**Prior art consulted:** none.

The `tims_sqlite` fuzz target (3 min) then found that a record's payload length, a varint of
up to 2^64, sized a `Vec` before any overflow page was read (capacity overflow, multi-GB
allocations). A payload is now refused when it is longer than every page the database file
and its WAL physically hold, and an overflow chain may not visit more pages than exist.
Fixtures: `sqlite-fuzz-payload-capacity-overflow.tdf`, `sqlite-fuzz-payload-huge-alloc.tdf`
(fuzzer mutations of the `timsrust-test-dda` database). The SQLite file-format document
(public domain) already cited above was the only reference.

## 2026-09-23 — headers-only check

Corpus: timsrust-test-dda.d, timsrust-test-dia.d, the two MetaboLights `.d` runs. No new prior art and no new parsing: `check_headers` (used by `check --headers-only` and `openreadout index`) reports `ClosedProperly = 0` as `check` does and, instead of decoding every frame blob, checks that each frame's blob offset (as the reader already records it from the frame table) lies inside the binary file (`truncated` otherwise).

## 2026-09-24 — Scan headers without peaks (`spectra`, `Dataset::visit_scan_headers`)

**Scope:** `spectrum_at` split into its metadata (SQLite tables: frame RT and polarity, level, precursor, isolation, collision energy, 1/K0) and its decoding; the header of a whole frame adds `Frames.SummedIntensities` as TIC and `Frames.MaxIntensity` as base-peak intensity, and TSF `NumPeaks` as point count. PASEF precursors and DIA windows have no stored TIC (the decoded spectrum sums its peaks).

**Validation:** `crates/openreadout-corpus-tests/tests/scans.rs` compares every oracle scan's header (scan number or native id, MS level, RT, polarity, filter, precursor m/z, charge, activation, isolation target, collision energy, 1/K0, position) with the corpus oracles (timsrust + rusqlite: 2 files, 15 870 spectra), and on up to 40 scans per file checks that the header equals the decoded spectrum's metadata.

**Prior art consulted:** none new.

## 2026-09-25 — calibration models applied; compression type 1; TSF profiles; MALDI spots (Claude as assistant)

**Corpus files used** (all new, ProteoWizard vendor-reader test data, repository licence Apache-2.0, pinned to pwiz commit `699dd48953f8`; the `.mzML` files are ProteoWizard conversions made with Bruker's own library, i.e. vendor-calibrated values, used as black-box ground truth): `pwiz-bruker-hela-pasef` (+ `-mzml`), `pwiz-bruker-thyroglob-prm` (+ `-mzml`), `pwiz-bruker-diapasef` (+ `-mzml`), `pwiz-bruker-urine-tsf` (+ `-mzml`, `-centroid-mzml`), `pwiz-bruker-maldi-tsf` (+ `-mzml`, `-centroid-mzml`). Also `pxd075355-dda-pasef` and `pxd074950-b1-diapasef` (tables only, to see which model variants real files carry).

**Method.** For every reference spectrum (one per mobility scan, `frame=F scan=S`, or one per TSF frame) the scan's stored points were decoded from the binary file and paired with the reference points by equal intensity (exact integer counts): 2,073 (Hela, 6 frames), 2,509 (Thyroglob, 2 frames), 10,250 (diaPASEF, frame 5) TOF-index/m/z pairs and 55,589 (Urine TSF line spectra, 20 frames) and 338 (MALDI TSF) fractional-index/m/z pairs. Models were then fitted, and the stored coefficients identified in the fitted parameters:

- *m/z, `MzCalibration` model type 1.* `t = index · DigitizerTimebase + DigitizerDelay` (ns) is quadratic in `sqrt(m/z)`: a free fit gave the constant term = `C0` and the linear coefficient `2515.8` = `sqrt(10^12 / C1)` (to 1 part in 10^6). The residual of the Thyroglob file (non-zero `C4`) was exactly `K / sqrt(m/z)` with `K = C4 · sqrt(10^12/C1) / 2`, i.e. `C4` is an m/z offset inside the square root: `s = sqrt(m/z + C4)`, `t = C0 + β·s + C2·s² (+ C3·s³)`. What remained was a per-frame scale of `C1` of `1 + 10^-6 · dC1 · (T1_cal − T1_frame)` (`Frames.T1`; frames 0.037 °C below and 0.061 °C above the calibration temperature, dC1 = 31.1 and 60, both signs checked). With this model every pair agrees to the precision of the reference (≤ 0.065 ppm for the 32-bit exports, ≤ 0.011 ppm for the 64-bit diaPASEF export and ≈ 0 for the MALDI TSF). `C3` (cubic term) and `dC2` are 0 in every corpus file: a non-zero value is applied by the same formula but flagged unvalidated (`calibration_unvalidated`).
- *m/z, model type 2* (Urine TSF, 2021 timsTOF fleX; 23 columns `C0…C14`). `C3`, `C4` repeat `C0`, `C2`; with `s = sqrt(m/z)` the type-1 formula leaves residuals of up to 3.8 ppm inside `[C5, C6]` = [111.99, 929.83] and none outside except a fade near the edges. A free degree-6 fit of the residual in m/z against the uncorrected m/z reproduced `−C8…−C14` to 8 significant digits (`C7 = 7` coefficients): `m/z = m/z₁ − Σ C(8+k)·m/z₁^k`. Outside the range the correction is `P(bound) · exp(−d²)` with `d` the distance from the bound in m/z (checked at 20 points 0.01–1.01 below `C5` and 0.01–0.17 above `C6`). Result: max 0.00007 ppm over 55,589 points.
- *1/K0, `TimsCalibration` model type 2.* `1/K0 = 1 / (C6 + C7 / (C2 + (C3 − C2)/C1 · (scan − C0 − C4)))` with `scan` the zero-based scan index: max |Δ| 5·10^-13 over 2,497 reference scans of 3 files (two TDF 1.0/2.0, one TDF 3.x). This form was first read in `mzdata` (see below) and then checked; `C5`, `C8`, `C9` are not used by it.

**Prior art consulted** (documentation only, no code copied): `mzdata` (Joshua Klein), Apache-2.0, https://github.com/mobiusklein/mzdata `src/io/tdf/calibration.rs` (states it is adapted from `timsrust-calibration` (Apache-2.0) and `rustims` (MIT)): the 1/K0 form above, the m/z form `t = C0 + β·s + C2·s² + C3·s³`, `s = sqrt(m/z + C4)` (it treats model types 1 and 2 alike and ignores `C5…C14`, which the Urine file shows is 4 ppm off for type 2) and the temperature factor. Our m/z model was derived from the data before reading it; the two agree.

**Compression type 1 (LZF).** `timsrust` 0.6.6 `compression1.rs` (Apache-2.0; already cited above, re-read) for the blob layout: after the 8-byte header, `scans + 1` `u32` offsets counted from the blob start, one LZF stream per scan. Its reading of the decompressed `i32` values (a positive value is an intensity; a value `v ≤ 0` moves the TOF index by `−v − 1`) put peaks 1–11 indices early against the vendor m/z; fitting the Hela pairs shows a skip of `−k` advances `k` indices and the first index is 0 (residual then < 0.012 index everywhere). The blob's scan count (985, 2,742) exceeds `Frames.NumScans` (350, 274) in both type-1 files; `NumPeaks` and `SummedIntensities` equal the totals over all stored scans, so all are read (the reference conversions list only the first `NumScans` scans). LZF itself is the public liblzf scheme (decoder moved to `openreadout-codecs::lzf_decode`, shared with the Agilent reader).

**TSF profile spectra.** Urine and MALDI TSF blobs: the second header word is the byte length of the line-spectrum zstd frame; a second zstd frame follows to the end of the blob and decodes to `DigitizerNumSamples` `u32` intensities in four byte planes (as TDF frames). Sample `i` has TOF index `i`. Checked against the profile references: equal intensity sums, and the reference lists the non-zero samples, their zero neighbours and the first and last samples, which we reproduce.

**MALDI.** `MaldiFrameInfo` (spot name, region, raster X/Y index, stage position, laser power, shots) read into each spectrum's `extra` from the MALDI TSF file; `MaldiApplicationType` from `GlobalMetadata`.

**Not validated / not implemented:** prm-PASEF (`MsMsType` 10, `PrmFrameMsMsInfo`; no public file found yet — frames are exposed whole and a note says so); TimsCalibration model types other than 2 and MzCalibration types other than 1 and 2 (the acquisition-range approximation is used and flagged).

## 2026-09-25 — assurance profile (`src/assurance.rs`)

**Corpus files:** every development input of this format on disk (`corpus/assurance/evidence.json` lists them); no held-out file.
**Prior art consulted:** none.
**What was done.** No parsing logic changed. The reader declares an assurance profile (docs/assurance.md) that fingerprints each file from this reader's own normalized output: the schema, data kind, compression type, acquisition mode, and the MzCalibration/TimsCalibration state from `extra.mz_conversion` (not applied today: strict mode refuses timsTOF spectra until they are). The table of validated feature values and the confidence level are generated from the development corpus by `cargo xtask assurance-audit`.

## 2026-09-26 — a timsTOF HT run from a fifth depositor; assurance

**Corpus files:** `pxd080079-dda-pasef` (PRIDE PXD080079, CC0: `Hom_PC_AFA_1minDigestion_reg_red_0.005ug_try_Rep3_100SPD_DDA_S1-B12_1_7-16-2025_6416.d`, timsTOF HT, 2025). Candidates rejected after download: PXD078924's smallest archive is an interrupted upload (`analysis.tdf` is not an SQLite database; its `.status` file records only the first 5 MiB of `analysis.tdf_bin` as uploaded), PXD074784's holds a solariX `analysis.baf`, and PXD072230's holds `analysis.tdf` without `analysis.tdf_bin`.
**Oracle:** `oracle/timsrust-oracle` (timsrust 0.6.6, black box) on a copy of the directory. **Prior art consulted:** none new.
**What was done.** No parsing logic changed. The file reads without error; its 5,981 frames are bit-identical to timsrust's (scan offsets, TOF indices, intensities) and its 4,630 spectra match point for point (m/z and 1/K0 not compared: timsrust approximates them where the reader applies the calibration; `mz_agreement.rs` validates that). The assurance profile now reports MzCalibration and TimsCalibration as applied when the reader applies them, with the model type as a variant feature, and the m/z agreement test's per-file results count as independent confirmation of the ProteoWizard files' spectra. With ten confirmed files from five depositors the evidence rubric rates the reader high.

## 2026-09-26 — run polarities (second-opinion gap) (Richard Zimring with Claude as assistant)

**Corpus files:** the ten development `.d` inputs (`timsrust-test-dda`, `timsrust-test-dia`, `pxd074950-b1-diapasef`, `pxd075355-dda-pasef`, `pxd080079-dda-pasef`, `pwiz-bruker-hela-pasef`, `pwiz-bruker-thyroglob-prm`, `pwiz-bruker-diapasef`, `pwiz-bruker-urine-tsf`, `pwiz-bruker-maldi-tsf`); no held-out file. **Prior art consulted:** none new (the `Frames.Polarity` column was already read per frame, as documented in `docs/formats/bruker-tdf.md`).
**What was done.** The run's `extra.polarities` lists the distinct `Frames.Polarity` codes (`+` positive, `-` negative; the negative-mode urine TSF is the one `-` file), so the experiment's method names the polarity; the second opinion (Python sqlite3 on `Frames`) reported it on seven files where ours had none. The sample name (`GlobalMetadata.SampleName`) was already read; the urine file's name (`…_6minautoMSMS_default_0.3cycle`) was not taken as its sample id because the core's default-name rule matched `default` anywhere in a name. That rule now matches names that start with the word (`Default Patient ID`), not a method setting inside an acquired name (book/src/guides/metadata.md).

## 2026-10-06 — negative-ion runs: the mobility calibration on the voltage's magnitude (Richard Zimring with Claude as assistant)

Held-out draw D reported negative 1/K0 values on every MS2 precursor of a negative-mode DDA-PASEF run (finding D-M1). No development file was a negative-ion TDF run (the one negative development file, `pwiz-bruker-urine-tsf`, is TSF and has no mobility). No held-out file was opened.

**Corpus files used (new):** `mtbls13504-balf-neg` and `mtbls13504-balf-pos` (MetaboLights MTBLS13504, CC0-1.0: a negative-ion and a positive-ion DDA-PASEF run of one timsTOF Pro, timsTOF 3.1, TDF 3.7) and `mtbls12332-tft-neg` (MetaboLights MTBLS12332, EMBL-EBI terms: a negative-ion DDA-PASEF run of another laboratory's timsTOF Pro, otofControl 6.2, TDF 3.3). Every frame of the two negative runs has `Frames.Polarity` `-`.

**Prior art consulted:** timsrust (Apache-2.0 crate, MIT repository) at commit `80e235a`, `crates/timsrust-tdf/src/calibration.rs` and `metadata.rs`, again: it converts a scan to 1/K0 by a straight line from `OneOverK0AcqRangeUpper` at scan 0 to `OneOverK0AcqRangeLower` at the last scan and does not look at the polarity or at `TimsCalibration`. alpharaw (Apache-2.0, commit `46e5f4c`, `alpharaw/bruker/timstof.py`) does the same when Bruker's library is not installed. Neither describes the type-2 model.

**What was inferred from what:**
- `TimsCalibration` of the negative runs holds negative ramp voltages: `C2` −198.25 and `C3` −53.77 (MTBLS13504), −193.85 and −53.16 (MTBLS12332). The positive run of the same instrument holds `C2` 195.20 and `C3` 54.57; every positive development file holds positive values. With the model as documented, `1/K0 = 1 / (C6 + C7 / V)` and `V = C2 + (C3 − C2)/C1 · (scan − C0 − C4)`, a negative `V` gives a negative 1/K0 at every scan (−1.69 to −0.47 in MTBLS13504).
- With the magnitude of `V`, the negative run of MTBLS13504 runs from 1.48668 at scan 0 to 0.45103 at scan 1042. The positive run of the same instrument and method range runs from 1.48771 to 0.45100. The two curves differ by at most 0.0011 V·s/cm² over the scan range, about what the small difference in the stored voltages gives. MTBLS12332 runs from 1.48694 to 0.452431 with the magnitude. Its last-scan value equals its `OneOverK0AcqRangeLower` (0.452431) to six digits, as in the development file `pwiz-bruker-diapasef` (0.601567).
- timsrust's straight line gives positive values on these runs. Over every MS2 precursor, ours minus timsrust's runs from 0.0027 to 0.0371 (1st to 99th percentile) on the negative MTBLS13504 run and from 0.0023 to 0.0358 on MTBLS12332, against 0.0021 to 0.0359 on the positive run of the same instrument and 0.0026 to 0.0275 on `pxd080079-dda-pasef`: the straight line's usual distance from the model, and no negative value.

**Decided:** the type-2 model uses the magnitude of the voltage: `1/K0 = 1 / (C6 + C7 / |V|)`. Positive runs are unchanged. No independent reader confirms the calibrated values of a negative run (there is no vendor-library conversion of one in the corpus), so the assurance profile reports the polarity of a mobility run as a variant feature and the reader reports negative-run 1/K0 values as derived by this rule. `--strict` withholds them until a vendor-library conversion of a negative run confirms the rule.
