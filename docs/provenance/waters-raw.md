# Provenance log — Waters MassLynx `.raw` directories

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

## 2026-09-22 — initial implementation (Richard Zimring with Claude as assistant)

**Corpus files used:** `mtbls15166-brain-b1-raw` (MetaboLights MTBLS15166, CC0-1.0: Xevo TQ-XS, six MRM functions, `_CHRO001.DAT` system pressure) and `mtbls3555-bv-ix-alpha-raw` (MetaboLights MTBLS3555, EMBL-EBI Terms of Use: one MRM function of five transitions). Both are zipped `.raw` directories extracted by `cargo xtask corpus fetch`.

**Public documentation consulted** (web pages of an LGPL-3.0 project): rainbow documentation, https://rainbow-api.readthedocs.io/en/latest/waters.html and its pages `waters/funcidx.html`, `waters/funcdat2.html`, `waters/funcdat6.html`, `waters/funcdat8.html`, `waters/chrodat.html`. Used for: the 22-byte `_FUNCnnn.IDX` record (DAT offset, 22-bit pair count, retention time f32 in minutes at +12), the `_CHROnnn.DAT` layout (0x80 header, f32 time/value pairs), the `_FUNCTNS.INF` 416-byte block with 32 f32 masses at 0xA0, and the existence of 2-, 6- and 8-byte DAT layouts and of `$$ Cal Function` calibration lines.

**Inferred from the files.**
- `_HEADER.TXT`: `$$ Key: value` lines, read directly. `_extern.inf`: `Instrument Parameters - Function N:` sections with `Polarity ES+/ES-`, read directly.
- The corpus DAT files hold 4 bytes per value (DAT size ÷ Σ counts = 4 for all seven functions), a layout rainbow's pages do not describe. rainbow (run as a black box) decodes them to intensities; the f32 at IDX +8 is, for every scan, exactly half the sum of rainbow's values. Splitting each u32 into a 22-bit mantissa and a 10-bit exponent (the same 22/10 split the IDX count word uses) and scaling by 2^(e − 21) reproduces the IDX value as the sum of a scan's values for all 2,561 scans (relative difference < 1e-6), so we read the IDX f32 as the stored TIC and use this scale; rainbow's values are exactly twice ours.
- The second f32 array at `_FUNCTNS.INF` +0x120 is read as the MRM products: it has as many values as the 0xA0 array (the transitions rainbow reports) and every value is smaller than the precursor at the same position (583.4 → 209.2, 296.6, 343.1, 402.2), as products are; no text file in the corpus lists the transitions, so this naming is unconfirmed; f32 at +10/+14 are the function's start and end times (0.75/3.65 min for function 1 of MTBLS15166, bracketing its scans).
- `_CHROMS.INF`: header (0x80 length, channel count, record length 0x55), then per channel 4 bytes, the NUL-terminated description and a `$CC$,1.000000,3,0,0,bar` string whose last field is the unit; read from the one corpus file.

**Oracle results (2026-09-22):** both corpus inputs pass: 7 function tables whose retention-time and transition columns (51 columns) are bit-identical to rainbow's output (transitions halved, see above; column names from `_FUNCTNS.INF`). The analog channel has no oracle (rainbow does not return it); `check` verifies its pairs.

## 2026-09-23 — full-scan spectra, calibration, MS/MS and lock-mass functions (Richard Zimring with Claude as assistant)

Notes for this entry (in force for everything below):

- Conversions to mzML appear only as the depositors' own `.mzML` files, read with `pyteomics` (Apache-2.0).
- rainbow's public documentation pages (listed in the first entry) were re-read for the 2-, 6- and 8-byte layouts. Its `funcidx` page states that the index of "HRMS" data is not documented; everything below about 12-byte values, 30-byte index records, the calibration and `.STS` files comes from the files themselves.
- Hex dumps and small Python scripts (numpy, Apache/BSD) over the corpus files were the instruments.

### Corpus files used

| id | source, licence | instrument (`_HEADER.TXT`) | content | pairing |
| --- | --- | --- | --- | --- |
| `mtbls7290-scfa240-001-neg-blank-raw` | MetaboLights MTBLS7290 `RAW_FILES/DIME__SCFA240_20220530_SSCFA_001_neg_Blank.raw.zip`, EMBL-EBI Terms of Use | `SYNAPT-G2#UCA057`, MassLynx 4.1 (`_extern.inf`) | one TOF MS function, ES−, 2,873 centroid scans, 22-byte index records | depositor mzML (ProteoWizard 3.0.21354), `DERIVED_FILES/…001_neg_Blank.mzML`: the first 247 scans (0.018–0.873 min) |
| `pxd059722-mth2-alicine-td-1-raw` | PRIDE PXD059722 `240429_MTH2_Alicine_TD_1_r0CENT.raw.zip`, CC0 (project licence) | `SYNAPT-XS#NotSet` | TOF MS (function 1), TOF MS/MS at a set mass (function 2), lock-spray reference (function 3), ES+, 472 scans, centroided after acquisition (`CENT`, `_history.inf`), 30-byte index records | depositor mzML `240429_MTH2_Alicine_TD_1_r0CENT.mzML` (ProteoWizard): all 472 scans |

Searched and not used: MetaboLights MTBLS694 (SYNAPT ToF; 888 MB per `.raw.zip`; the EBI FTP served about 40 kB/s on 2026-09-23, so the download of `PipelineTesting_RPOS_ToF10_B1SRD47.raw.zip` was stopped after its 122 MB mzML had reached 89 MB; its export also keeps only peaks above an absolute intensity of 100, which would need a threshold option in the harness), PRIDE PXD040188/PXD037021/PXD055555/PXD073126 (Xevo/SYNAPT pairs of 0.5–12 GB). No public ion-mobility (`.cdt`) acquisition with a depositor conversion under 1 GB was found, so drift time is not read.

### Method and inferences

1. **12-byte values.** Both DAT files hold exactly 12 bytes per value (DAT length ÷ Σ index counts). Word 0 decodes with the existing 22/10-bit packed scheme to the export's intensities (all 132,053 and 2,016,179 peaks equal). Word 1 increases with m/z within a scan; its top 5 bits change where the value crosses a power of two and bits 26..0 always have bit 26 set, i.e. a 5-bit exponent and a 27-bit mantissa: raw mass = mantissa × 2^(exponent − 27) (50.1199 for the first MTBLS7290 peak). Word 2 is not interpreted (it varies slowly within a scan; no export field matches it).
2. **Calibration.** MTBLS7290's raw masses differ from the export by ~1e-3 relative. `_HEADER.TXT` line `Cal Function 1: c0,…,c5,T1` — reading `T1` as "polynomial in √m": √m_cal = Σ cᵢ (√m_raw)^i, m_cal = (√m_cal)² — reproduces the export to 6e-8 relative, and rounding the result to single precision reproduces all 132,053 m/z values bit for bit (the export's m/z are exactly representable as f32). `T0` (the form rainbow's pages document for the 6- and 8-byte layouts) is m_cal = Σ cᵢ m_raw^i.
3. **Already-calibrated data.** In PXD059722 the stored masses equal the export's m/z exactly, although `_HEADER.TXT` carries a non-trivial `Cal Function n … T1`; applying it moves them by 1e-4. Its index records are 30 bytes and the flag field of every count word is 288 (0x120) instead of 32 (0x20) as in MTBLS7290 and both MRM corpus files. We read bit 0x100 of that field as "values stored calibrated" (and as the 30-byte record marker; see 4). Two files are the only evidence: low confidence, stated in `known_gaps`.
4. **30-byte index records.** `_FUNC001.IDX` of PXD059722 is 11,940 bytes for the 398 function-1 scans of the export (30 × 398); the first 22 bytes follow the known layout except that the u32 offset is 0, and bytes 22..30 are a u64 that equals the running sum of 12 × count, i.e. the DAT offset.
5. **Scan order.** The export lists the scans of all functions merged by retention time (reference-function scans between MS and MS/MS scans); sorting by (time, function) reproduces its order for all 472 spectra. Native ids are `function=F process=0 scan=S`, S counted per function from 1.
6. **Function kinds.** `_extern.inf` names each function (`Function Parameters - Function 2 - TOF MSMS FUNCTION`, `… - Function 3 - REFERENCE`); the export gives MS level 2 to the MS/MS function and 1 to the reference (lock-spray) function, which it includes as ordinary MS1 scans. `Polarity ES+`/`ES-` appears once for the whole acquisition in these files (per function only in the MRM files).
7. **`.STS` files (per-scan statistics).** Self-describing: u16 header length, u16 (1), u16 record length, u16 field count, then from byte 32 one 48-byte descriptor per field (u16 id, u16 type, u16 offset in the record, a NUL-terminated name, u16 size at +32); types 0 = 1 byte, 1 = i16, 2 = i32, 3 = f32. Records follow the header, one per scan. The field named `Set Mass` is the export's precursor m/z of every MS/MS scan (f32 1223.3878, printed 1223.39 by the export); `Collision Energy` holds the trap collision energy (35 eV, the ramp start in `_extern.inf`).
8. **Scan window.** `_FUNCTNS.INF` block f32 at 0xA0 and 0x120 hold the function's start and end mass for scanning functions (50/400, 700/1700, 300/2000), equal to the export's scan window; the same slots hold MRM precursors and products in MRM functions.

### Converter behaviour recorded (not reproduced)

- The MTBLS7290 export stops at scan 247 of 2,873 (0.873 of 10 min): a partial conversion; the harness compares those 247 scans.
- The export prints the MS/MS set mass rounded to two decimals (1223.39 for f32 1223.3878); the harness allows 1e-5 relative on precursors for that file (`precursor_tolerance`).

## 2026-09-23 — 2-byte MRM values validated (Richard Zimring with Claude as assistant)

**Corpus file:** `mtbls225-tqs-rln-20140623-056-raw` (MetaboLights MTBLS225, EMBL-EBI Terms of Use; Xevo TQ-S by name, one MRM function of 21 transitions, ES−, 788 scans, 2014), paired with the depositor's mzXML 2.1 `TQS-RLN-20140623-056.mzXML` (made through an mzML; the tool is not named).

**What the export holds.** 17,336 one-point "scans" with m/z 0 and precursor 0: 22 blocks of 788 points (the TIC, then one block per transition in `_FUNCTNS.INF` order), each block with the function's scan times written as seconds but equal to the index's retention times in minutes (as f32). pyteomics (Apache-2.0) reads it; `oracle/gen.py` (`waters_chromatogram_export`) rebuilds the function table from the export's values and names the columns from `_FUNCTNS.INF`.

**Inferred and checked.** `_FUNC001.DAT` is 33,096 bytes for 788 × 21 values: 2 bytes per value. Decoding each u16 as base (high 13 bits) × 4^(low 3 bits) — the layout rainbow's documentation describes for SIR data — gives every one of the 16,548 transition values of the export exactly; the TIC block equals the f32 TIC stored in `_FUNC001.IDX`. So the 2-byte layout also stores MRM transitions (the table's columns are named `precursor > product` from the function block, as for the 4-byte layout).

## 2026-09-23 — byte sources (no parsing change)

Main moved the chrom-crate Waters reader to byte sources while this crate was being written, and the same change is carried over here: directory listing, sidecars, indexes and `.DAT` ranges are read through the input's namespace. No format knowledge was added or changed. Checked with the synthetic three-function directory read from disk and from memory (identical `info`, spectra and `check`) and with the corpus byte-source harness.

## 2026-09-23 — header fields on MRM tables; model and serial split

- Corpus: `mtbls225-tqs-rln-20140623-056` (Xevo TQ-S MRM), `pxd059722-mth2-alicine-td-1` (SYNAPT XS).
- The `_HEADER.TXT` fields (acquired name, instrument, operator, vial, date) were only copied onto evenly spaced traces, so an MRM run whose scan times are uneven (no trace) carried none of them. Function tables now carry the same header fields.
- `Instrument` reads `MODEL#SERIAL` on these systems (`XEVO-TQS#WAA049`, `SYNAPT-XS#NotSet`), inferred from the two files: split at `#` into `instrument` and `instrument_serial`; `NotSet` is treated as no serial.
- The experiment model reads `traces[0].extra.*` header fields from `tables[0].extra.*` when a file has no traces (core `experiment.rs`), recording the path actually used.

## 2026-09-24 — Scan headers without peaks (`spectra`, `Dataset::visit_scan_headers`)

**Scope:** the function's index, `.STS` statistics and header alone: level, RT, polarity, `Set Mass` precursor, collision energy, scan window; the header's TIC is the index's stored TIC (`stored_tic`), which the decoded spectrum replaces by the sum of its points.

**Validation:** `crates/openreadout-corpus-tests/tests/scans.rs` compares every oracle scan's header (scan number or native id, MS level, RT, polarity, filter, precursor m/z, charge, activation, isolation target, collision energy, 1/K0, position) with the corpus oracles (pyteomics on the depositor mzML: 2 files, 719 scans), and on up to 40 scans per file checks that the header equals the decoded spectrum's metadata.

**Prior art consulted:** none new.

## 2026-09-24 — fuzz findings, bounds only

**Scope:** arithmetic guards; nothing the reader infers changed. The `whole_waters` fuzz target (90 s, first run since the full-scan reader; seeds cut from the corpus `.raw` directories) found two crashes: `infer_bytes_per_value` evaluated the quotient of a scan's byte spacing by its value count with `then_some`, which evaluates its argument even when the count is 0 (a division by zero, a panic in release builds too; now `then`); and `check` added a scan's offset to its value count × bytes per value without overflow checks (now saturating). Regression fixtures: `crates/openreadout-waters/tests/fixtures/malformed/bundle-whole_waters-*.bin`, replayed as directories by `crates/openreadout-cli/tests/fuzz_regressions.rs`.

## 2026-09-25 — ProteoWizard test data: 30-byte index records without flag 0x100, intensity flags, function types, MSe, PDA (Claude as assistant)

**Corpus files used** (new; ProteoWizard vendor-reader test data, repository licence Apache-2.0, pinned to pwiz commit `699dd48953f8`; each `.raw` comes with ProteoWizard conversions made with Waters' own library, used as black-box ground truth): `pwiz-waters-091204-nfdm-008` (Synapt MS, 2009, 10 functions), `pwiz-waters-160109-mix1-calcurve-070` (Xevo TQ-MS, 2009: 12 MRM + 12 product-ion-scan functions), `pwiz-waters-atehlstlsek-lm-684-3469`, `pwiz-waters-atehlstlsek-lm-785-8426`, `pwiz-waters-atehlstlsek-profile` (SYNAPT G2-Si, 2016: fast DDA, centroid and continuum), `pwiz-waters-mse-short`, `pwiz-waters-hdmse-short-nolm`, `pwiz-waters-hddda-short-nolm`, `pwiz-waters-hdmrm-short-nolm` (SYNAPT G2-Si, 2018: MSe, HDMSe, HDDDA, HD-MRM), `pwiz-waters-sonar-short` (Xevo G2-XS, 2018), `pwiz-waters-qc-lcms2-2-23-268-1-1` (ACQUITY SQD, 2023, with a PDA function and analog channels), and two synthetic directories written by the MassLynx SDK (`pwiz-waters-minimal-dda`, `pwiz-waters-dda-isolationwindow`).

**Inferred and checked** (hex dumps and scripts against the references; `crates/openreadout-corpus-tests/tests/mz_agreement.rs` compares every point):

1. **Intensity flags of the 12-byte layout.** Five points (lock-mass peaks of reference scans and a few analyte peaks) decoded 2^64–2^128 too high: their exponent field (the top 10 bits of the intensity word) had bit 6 (0x40) or bit 7 (0x80) set. With the exponent taken from the low 6 bits the reference intensities are matched exactly (811.0, 1006.0, 619.379, 178688.125, 1054.949). 0x40 marks the lock-mass peak of each lock-spray reference scan (684.3469 and 785.8426 exactly in the two lock-mass-corrected copies of the ATEHLSTLSEK acquisition; 637.17 ± 0.002 in the uncorrected 091204 file); 0x80's meaning is not known. The flagged lock-mass m/z are reported as `extra.lock_mass_peak_mz`, other flags counted (`extra.flagged_points`). The 4-byte MRM decoder is unchanged (no flags seen).
2. **30-byte index records without flag 0x100.** `_FUNC002.IDX` of the continuum ATEHLSTLSEK file is 150 bytes (5 × 30), flags 0x60: the 22-byte reading chosen from the flag gave garbage. The record length is now the one (22 or 30) whose records tile the file and whose DAT offsets step by count × bytes per value through the whole DAT; the flag decides only when both fit. Flag 0x100 still means "values stored calibrated": files without it (0x60) match the references only with the `Cal Function` applied, files with it (0x120) only without.
3. **Function types.** The low 5 bits of the `_FUNCTNS.INF` block code give the function type (0x12 TOF MS, 0x10 TOF MS/MS, 0x06 quadrupole product-ion scan, 0x09 MRM, 0x0C photodiode array, 0x00 quadrupole full scan); bit 0x8000 marks the lock-spray reference function. The `_extern.inf` type text is misleading (the fast-DDA MS/MS function is named `TOF SURVEY FUNCTION`), so MS level now comes from the block type: 0x10 and 0x06 are MS2. A product-ion scan's precursor is the f32 at block offset 0x18 (418.726 … 759.426, equal to the references); a TOF MS/MS scan's is the `.STS` `Set Mass` (descriptor id 77 — also the only, unnamed, field of SDK-written files).
4. **MSe.** A TOF MS function whose `_extern.inf` section (including its `[ACQUISITION]`/`[PARENT MS SURVEY]` subsections, which the parser used to drop) ramps a high collision energy (`Ramp High Energy from 14.0 to 40.0`) is the high-energy function: MS2, isolation window = the scan window, precursor its centre, collision energy the `.STS` value (14 eV), as the references record.
5. **Photodiode array.** Function type 0x0C with 6-byte values holds (wavelength nm, signed absorbance count): the rainbow 6-byte layout decodes the reference wavelengths (209.953 nm + 1 nm steps) and values (−249, −217, …) exactly. Such functions are no longer returned as mass spectra; they are a trace (time, then one channel per wavelength).
6. **Ion mobility / SONAR.** Functions with `_funcNNN.cdt` + `.ind` store drift-resolved data (200 bins of one pusher period, 69.25 µs = 1/`Pusher Frequency`); the `.DAT` spectrum is the drift-summed one (its TIC equals the sum over the reference's drift bins). The `.cdt` encoding was not decoded; spectra of such functions say so (`extra.drift_bins`).
7. **Not reproduced:** the references drop the first stored point of every scan of the two SDK-written synthetic directories (no acquired file shows this); the SQD reference labels the ES+ full-scan function (block type 0, like the ES− one) MS2 with a window-centre precursor.

**Prior art consulted:** none new (rainbow documentation pages cited above for the 6- and 8-byte layouts).

## 2026-09-25 — assurance profile (`src/assurance.rs`)

**Corpus files:** every development input of this format on disk (`corpus/assurance/evidence.json` lists them); no held-out file.
**Prior art consulted:** none.
**What was done.** No parsing logic changed. The reader declares an assurance profile (docs/assurance.md) that fingerprints each file from this reader's own normalized output: the header version, instrument, function kinds, value widths, stored spectra, the Cal Function (applied) and lock-mass correction (available, not applied). The table of validated feature values and the confidence level are generated from the development corpus by `cargo xtask assurance-audit`.

## 2026-09-26 — drift-resolved data (`_funcNNN.ind` / `.cdt`): partial layout, not decoded

**Corpus files:** `pwiz-waters-hdmrm-short-nolm` (with its per-drift-bin ProteoWizard export `HDMRM_Short_noLM.mzML`, vendor library, black box). **Prior art consulted:** none.
**What was found (no parsing logic changed).** `_func001.ind` is a 36-byte header (u32 at 4 = 200 drift bins) followed by one 4,012-byte block per scan: three u32, then five arrays of 200 u32 per drift bin. The fourth array is the number of non-zero points of each drift bin: on all 200 bins of scan 1 it equals the count of non-zero intensities in the export's drift-bin spectra (sum 3,138). The first array holds 0 or 8 per bin, the second and fifth zeros, the third the f32 3.21 in every bin; their meaning is unknown. The `.cdt` is a byte stream in which some flight-time bins appear as little-endian u16 (0x3644 where the export's peak lies near bin 13,898), but the stream mixes variable-length fields whose grammar we have not derived, and the export's m/z values could not be placed on an integral flight-time grid through the inverted `Cal Function` (residuals up to half a bin), so the positions cannot yet be checked point by point. The drift-resolved data remain undecoded; the drift-summed spectra in `_FUNCnnn.DAT` are what the reader returns.

## 2026-09-26 — assurance profile: instrument generation (Richard Zimring with Claude as assistant)

**Corpus files:** every development input of this format (`corpus/assurance/evidence.json`); no held-out file. **Prior art consulted:** none.
**What was done.** No parsing logic changed. The structural instrument feature is the instrument generation (`generation Synapt`, `generation Xevo TQ`, `generation Xevo QTOF`, `generation single quadrupole`; `instrument_generation`) instead of the exact model string, which stays descriptive: the function layouts follow the instrument family, and model strings vary within one family (`SYNAPT-G2`, `SYNAPTG2-Si`, `JAA143 Synapt MS`).

## 2026-09-26 — sample description in the experiment (second-opinion gap) (Richard Zimring with Claude as assistant)

**Corpus files:** `mtbls225-tqs-rln-20140623-056-raw` (an MRM-only Xevo TQ-S run: no spectra, its header fields are on the SRM table) and the other development `.raw` inputs; no held-out file. **Prior art consulted:** none new.
**What was done.** No parsing logic changed. `_HEADER.TXT`'s `Sample Description` was already read (`extra.description` on the spectra run or, in MRM-only runs, the SRM table); the second opinion looked for it only on the spectra run. The experiment now carries it as `sample.name` (a descriptive name beside the id, the acquired name), wherever the reader put it, and the second opinion compares that normalized field. The `acquisition software version` gaps on four ProteoWizard test files remain: those directories hold no `_extern.inf`, and ProteoWizard's `4.1` is its constant, not a value in the file.
