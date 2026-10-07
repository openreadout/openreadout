# Provenance log — Agilent MassHunter `.d`

Rules: `docs/legal/clean-room-policy.md`. Notes for this format (in force for every entry below):

- Everything below comes from the corpus files themselves. The depositor-made `.mzML` conversions in the corpus are read with `pyteomics` (Apache-2.0).
- The `MSScan.xsd`, `*.xml` and `.m` method files inside each `.d` are part of the data directory the depositor published under the study's licence; reading them is reading the file.

## 2026-09-23 — initial derivation (Richard Zimring with Claude as assistant)

### Corpus files used

All from MetaboLights (EMBL-EBI), `https://ftp.ebi.ac.uk/pub/databases/metabolights/studies/public/<MTBLS>/`, each study's `i_Investigation.txt` declaring `Comment[License] EMBL-EBI Terms of Use` (no restrictions on use or redistribution; checked 2026-09-23). Candidates came from `docs/provenance/ms-vendor-survey.md`. Each `.d` was deposited as a zip; each is paired with the depositor's mzML conversion (ProteoWizard, per the files' `softwareList`).

| id | instrument (from `Devices.xml`) | software (`Contents.xml`) | content | pairing |
| --- | --- | --- | --- | --- |
| `mtbls449-13047CHQ_0001_A1` | G6410A triple quadrupole | (not recorded), acquired 2014 | dynamic MRM, 116 transitions, negative, 28,277 scans | mzML (pwiz 3.0.10577): TIC + 116 SRM chromatograms |
| `mtbls243-03_D24062013T1259_1399CBU_01QC_A3` | G6410A triple quadrupole | (not recorded), acquired 2013 | dynamic MRM, 113 transitions, negative, 27,674 scans | zipped mzML: TIC + 113 SRM chromatograms (no polarity terms) |
| `mtbls874-BDV10076M3` | G6540B Q-TOF + DAD, binary pump, autosampler, column oven | B.05.01 (2019) | auto MS/MS, centroid only, negative, 1,810 scans | mzML, 32-bit m/z |
| `mtbls1334-STD_neg_MSMS_1min0205` | 6500-series Q-TOF | B.08.00 (2018) | direct infusion, targeted MS/MS with a collision-energy ramp; profile + centroid; no `MSMassCal.bin` | mzML, 64-bit m/z (centroids) |
| `mtbls7386-DS017_KO2_3_C18MSpos_IO29_20250129` | G6550B Q-TOF + two binary pumps, iso pump, DAD, autosampler, column oven | 10.1 (2025) | MS1 centroid, positive, 388 scans | mzML, 32-bit m/z |

Dropped: MTBLS6740 (listed in the survey as "auto MS/MS" Agilent) — its `.d` is a Bruker BAF directory (`analysis.baf`, `microTOFQImpacTemAcquisition.method`), not MassHunter; the survey entry is corrected below. MTBLS599 (mzXML pairs) was not used for time.

### Method and inferences

1. **Scan index.** Hex dumps of `MSScan.bin` (all five files) showed a 0x44-byte header, an i32 at 0x58 equal to the offset where recognisable records start (ScanID 1, 2, 3 … at a fixed stride in the triple-quadrupole files), and the stride (196 / 220 / 284 bytes). Laying the active (uncommented) elements of each file's own `MSScan.xsd` end to end with the XML Schema type sizes reproduced the triple-quadrupole stride exactly; for the Q-TOF files it gave 4 bytes too many until `ChromScaleFactor` was taken as a 4-byte float (its decoded values, 1.0 and small positive numbers, confirm it). The i32 at 0x4C is the number of `SpectrumParamValues` blocks (1 in peak-only files, 2 in MTBLS1334), matching the strides. Field meanings were checked value by value against the mzML: `ScanTime` = scan start time (≤ 2e-11 min), `ScanID` = the `scanId=` native id, `TIC` = total ion current and the TIC chromatogram (exact), `IonPolarity` 1 in the negative files and 0 in the positive one, `MzOfInterest` = MS/MS precursor and MRM Q1, `CollisionEnergy` = collision energy, `ChargeState` = charge state, `DDScanID` = `spectrumRef`, `MinX`/`MaxX` = scan window.
2. **Peak blocks.** In the triple-quadrupole files each block is 12 bytes: an f64 equal to the transition's product m/z and an f32 abundance equal to the SRM chromatogram point (so the layout is x-array then y-array; confirmed on multi-point Q-TOF blocks). In the Q-TOF files the f64s were ~2×10⁴–1.2×10⁵ — flight times — and needed calibration.
3. **Calibration.** `DefaultMassCal.xml` (file content) lists a "Traditional" step with two values and a "Polynomial" step with eight values and `ValueUseFlags`; `MSMassCal.bin` records at each scan's `MassCalOffset` hold an i32 10 and ten doubles equal to those two steps' values (the first two drifting slightly scan to scan). `m = (a(t − t0))²` alone left errors of up to 1.7e-5 relative. Fitting the residual showed the correction is a function of flight time whose shape matches Σ cₖ tᵖ with the powers read from the set bits of `ValueUseFlags` (214 → 1, 2, 4, 6, 7; the coefficient magnitudes fall off accordingly), subtracted from m, with t clamped to the step's first two values (they convert to 113 and 2834 m/z, the usual reference-ion range). Of ten candidate forms tried (added/subtracted/multiplicative, in m, √m or t, clamped or not) only this one reached the exports' precision: bit-exact on MTBLS1334's float64 export (567/567 spectra, all peaks) once the correction is summed before being subtracted, and equal after f32 rounding on 1810/1810 MTBLS874 spectra and all but 66 of 1,327,310 MTBLS7386 peaks (one f32 ulp each; no evaluation order we tried changes those 66).
4. **Profiles.** MTBLS1334's `MSProfile.bin` blocks have `UncompressedByteCount` = 16 + 4 × `PointCount` and a byte stream whose control bytes follow the LZF scheme (literal runs below 32, back-references above; the scheme is public-domain / BSD liblzf, implemented here from its description). Decompressed: two f64 (21352.0, 0.5 — a flight time and the `SamplingPeriod`) then i32 counts. Internal checks on every scan: counts sum to `TIC` exactly, the maximum equals `BasePeakValue`, and the calibrated m/z of the maximum bin equals `BasePeakMZ` (≤ 2e-11 relative). The depositor's mzML holds only the centroids, so profiles have no external comparison.
5. **Device signals.** `DAD1.cd`, `BinPump1.cd`, `TCC1.cd`, `HiP-ALS1.cd`, `IsoPump1.cd` (MTBLS874, MTBLS7386, MTBLS449): after the 0x44 header, (1, DeviceID, count) then per signal two u8-counted strings, i32 kind, i64 offset, i32 count, 40 constant bytes, a u8-counted unit and 16 bytes; every descriptor parses to its exact length and every `.cg` block (f64 start, f64 interval in minutes, count f64 values) ends inside its file with plausible values (DAD mAU around 0, pump pressure 240–530 bar, column temperature 33 °C).
6. **Metadata.** `Contents.xml`, `Devices.xml`, `sample_info.xml`, `MSTS.xml`, `AcqMethod.xml` are plain XML; names as they appear there.

### Converter behaviour recorded (not reproduced)

- ProteoWizard prints each MRM product target 0.004 below the recorded product m/z (Q3=311.196 for 311.2); the harness matches SRM traces with a 0.005 m/z tolerance (`srm_product_tolerance`), and separates scheduled transitions with the same Q1/Q3 by the `start=`/`end=` times in the chromatogram id and by collision energy.
- In the auto-MS/MS export (MTBLS874) every MS/MS precursor is the mean `MzOfInterest` of all MS/MS scans that share the precursor (checked: 160.84366/160.84390 → 160.84378); we report each scan's own value; the harness allows 1e-5 relative there (`precursor_tolerance`).
- The MTBLS243 export carries no polarity on its chromatograms; the harness then ignores polarity.

### Results (corpus harness, 2026-09-23)

| id | compared | result |
| --- | --- | --- |
| `mtbls449-13047CHQ_0001_A1` | 117 chromatograms rebuilt from 28,277 scans | 117/117 exact (points, f32 intensities, times) |
| `mtbls243-03_D24062013T1259_1399CBU_01QC_A3` | 114 chromatograms from 27,674 scans | 114/114 exact |
| `mtbls874-BDV10076M3` | 1,810 spectra | 1,810 bit-exact at export precision; metadata 0 mismatches |
| `mtbls1334-STD_neg_MSMS_1min0205` | 567 spectra | 567 bit-exact (float64); metadata 0 mismatches |
| `mtbls7386-DS017_KO2_3_C18MSpos_IO29_20250129` | 388 spectra | 327 bit-exact, 61 within tolerance (66 peaks one f32 ulp apart); metadata 0 mismatches |

## 2026-09-24 — Scan headers without peaks (`spectra`, `Dataset::visit_scan_headers`)

**Scope:** the scan record alone (`MSScan.bin`): level, RT, polarity, precursor, charge, collision energy, stored TIC and base peak; the primary view's block (profile when stored, else peaks) gives `centroided`, the scan window and `point_count`.

**Validation:** `crates/openreadout-corpus-tests/tests/scans.rs` compares every oracle scan's header (scan number or native id, MS level, RT, polarity, filter, precursor m/z, charge, activation, isolation target, collision energy, 1/K0, position) with the corpus oracles (pyteomics on the depositor mzML: 3 files, 2 765 scans), and on up to 40 scans per file checks that the header equals the decoded spectrum's metadata.

**Prior art consulted:** none new.

## 2026-09-26 — Older triple-quadrupole layouts, neutral-loss scans, ion-mobility (6560) data

**Corpus files:** the Agilent reader test data of ProteoWizard (pwiz commit 699dd48953f8, `pwiz/data/vendor_readers/Agilent/Reader_Agilent_Test.data.tar.bz2`, repository licence Apache-2.0): `GFb_4Scan_TimeSegs_1530_100ng`, `MRM Neg C5`, `RS080806_APCI_PIscan_CE35`, `RS080906_MMI_PIscan_CE35_250ms`, `RS080806_NL_448.2_001`, `Thyrxox 5 TS Diff Scan B`, `reserpine-MS2sim-010` (G6410A triple quadrupole, 2006–2008, MSScan layout 5), `ImsSynthCCS`, `ImsSynthAllIons`, `ImsSynth_Chrom` (6560 ion-mobility Q-TOF, B.06.00), and `TOFsulfasMS4GHzDualMode+DADSpectra+UVSignal272-NoProfile` (6200 TOF). Each `.d` comes with ProteoWizard's own mzML conversion (made with the vendor library), used as the black-box reference; ids `pwiz-agilent-*` in `corpus/manifest.toml`. Only the data and the mzML were used — no ProteoWizard or vendor source, header or documentation.

**Prior art consulted:** none (no permissive reader of these layouts is known to us).

**Inferences:**

1. *Quadrupole profiles.* Layout-5 scan records of full, product-ion and neutral-loss scans point (format 2) into `MSProfile.bin`, with `ByteCount` = 8 + 4 × `PointCount`. The first 8 bytes are two f32 (150.0 and 0.1000061 in GFb), then i32 counts; the counts sum to `TIC` and match the export's intensities exactly. With the stored step the m/z drifted from the export by up to 0.09 at 1600; the export's values are f32(150 + k × 0.1) exactly. The record's `SamplingPeriod` (0.10000000149, an f32 0.1 widened) is the set step: `first + k × period`, with the period taken as the shortest decimal its f32 prints, rounded to f32, is bit-identical on 922 spectra of four files. A period farther than 0.1 % from the block's step is not trusted (the block's step is used).
2. *8-byte point lists.* `MRM Neg C5` (format 3) holds one 8-byte point per scan: an f32 m/z equal to the method's Q3 (229.0 in `192_1.xml`) and an i32 count equal to the export's SRM chromatogram point. `Thyrxox`'s MRM time segments hold format-1 blocks of 32 bytes for 4 points in `MSPeak.bin` (offsets far below `MSProfile.bin`'s blocks); reading them as 4 f32 then 4 i32 makes every one of 571 scans self-consistent (sum = `TIC`, extremes = `MinY`/`MaxY`, first/last m/z = `MinX`/`MaxX`, maximum at `BasePeakMZ` in 570, the other a tie), while interleaved pairs do not — so the one-point case is planar too. The export holds only the first 233 Thyrxox scans, so these have no external reference.
3. *Collision energy sign.* Negative-ion records store `CollisionEnergy` −20 where the method (`192_1.xml`, `<collisionEnergy>20`) and the export say 20: the energy is reported as its magnitude.
4. *Neutral-loss scans.* `ScanType` 2048 records (`RS080806_NL_448.2`) hold `MzOfInterest` 161.1 with a 200–900 scan: the loss, not a precursor; reported as `extra.neutral_loss_mz`, `precursor_mz` left empty. Precursor-ion scans were not found (the "PIscan" files are product-ion scans, `ScanType` 512, whose precursor the export confirms).
5. *Ion-mobility index.* The IM-MS `MSScan.xsd` has no `SpectrumParamValues`; the header words at 0x48/0x4C/0x58 read as in layout 6 (6, 2, 296) and laying out the schema's elements gives 106-byte records that tile the file exactly (110 bytes with the extra `MsProfNzPointCount` of `ImsSynth_Chrom`). `IMSFrame.bin` tiles likewise from `IMSFrame.xsd` (130 bytes after offset 76, the i32 at 0x48). Records with `DriftBin` 0 carry the frame's `TIC` and a base-peak m/z: the frame sums. The export lists exactly the other records, in file order (701, 1192, 6484 spectra), each at its frame's `FrameScanTime` and at drift time (`DriftBin` − 1) × `FrameDtPeriod` (exact on all 8,377).
6. *Ion-mobility profile encoding.* Hex dumps of small blocks next to the export's points (converted back to bins with the default calibration, whose polynomial step is all zeros) showed: after two f64 (18465.0 = `MinMsBin` × 0.5, and 0.5 = `FrameMsXPeriod`), a u32 0x90NNNNNN whose low 24 bits equal `MsProfPointCount`, and an i32 equal to minus the first bin; single bytes 0x01–0x7f equal to counts; bytes such as 0x83/0x8b/0x93/0xe7/0xeb at gaps of 31/29/27/6/5 bins, i.e. `!(v >> 2)` with low bits 11; a 1-byte 0xfe before 16-bit counts above 127 and 16-bit skips; 0xfd before a 32-bit skip; and a 4-byte −1 right after the start. Taking the low two bits of every negative value as the width of the values that follow (11 → 1, 10 → 2, 01 → 4, 00 → 8 bytes; the stream starts at 4) decodes all 701 `ImsSynthCCS` drift spectra identically to the export; the same rule then reproduced the 1,192 `ImsSynthAllIons` and 6,484 `ImsSynth_Chrom` spectra without change (independent files: other frame methods, all-ions fragmentation, an LC time axis, single-count spectra stored in 4-byte mode), and every one of the 16,272 records' counts sums to its `TIC` with its maximum at `BaseMsBin`. Any other header byte, a count past the grid or a cut value is refused.
7. *Converter behaviour recorded (not reproduced).* The export numbers IM-MS spectra (frame − 1) × 354 + drift bin − 1 where the file's `ScanID` counts records; all-ions (FragClass 2) spectra get the scan window's centre as a precursor with a half-width isolation window, "collision-induced dissociation" and energy 30 (the last point of the method's ramp `db=0 e=20, db=210 e=20, db=280 e=30`); we report no precursor or energy for them (the ramp is in `info --view full`). The export labels triple-quadrupole product-ion scans "beam-type collision-induced dissociation" (our `HCD`) and MRM transitions CID. It prints each MRM Q3 0.004 below the stored value (228.996 for 229), as in the MetaboLights files.
8. *Profile block without `MSProfile.bin`.* The TOF "NoProfile" directory's records list a format-1 block although no `MSProfile.bin` exists; the reader then uses the peak list (the export has only centroids).

**Results (corpus harness, `mz_agreement.rs`, 2026-09-26):** every pwiz Agilent spectrum export matches: GFb 182, APCI 170, MMI 337, Thyrxox 233 (profiles, bit-identical after f32 rounding), TOF sulfas 212 (centroids), IM-MS 701 + 1,192 + 6,484 (≤ 0.1 ppm, counts exact); SRM chromatograms of `MRM Neg C5` 4/4 and the neutral-loss and reserpine TIC traces rebuilt exactly; scan headers equal the spectra on all 10 files.

## 2026-09-25 — assurance profile (`src/assurance.rs`)

**Corpus files:** every development input of this format on disk (`corpus/assurance/evidence.json` lists them); no held-out file.
**Prior art consulted:** none.
**What was done.** No parsing logic changed. The reader declares an assurance profile (docs/assurance.md) that fingerprints each file from this reader's own normalized output: the MSScan layout, instrument model, MS device, stored spectra, scan types and the time-of-flight calibration (applied). The table of validated feature values and the confidence level are generated from the development corpus by `cargo xtask assurance-audit`.

## 2026-09-26 — assurance profile: instrument series (Richard Zimring with Claude as assistant)

**Corpus files:** every development input of this format (`corpus/assurance/evidence.json`); no held-out file. **Prior art consulted:** none.
**What was done.** No parsing logic changed. The structural instrument feature is the instrument series from the model number (`generation 6200 TOF`, `generation 6400 triple quadrupole`, `generation 6500 Q-TOF`, `generation 6560 ion-mobility Q-TOF`; `instrument_generation`) instead of the exact model, which stays descriptive: the stored spectra follow the series, not the model within it.

## 2026-10-06 — product m/z of dynamic-MRM transitions against the acquisition method (Richard Zimring with Claude as assistant)

Held-out draw D reported product m/z printed to one decimal (254.3 where the depositor's mzML has 254.296) on a 6495 dynamic-MRM run (finding D-L1). No held-out file was opened.

**Corpus file used (new):** `mtbls4722-1` (MetaboLights MTBLS4722, EMBL-EBI terms: a 6470A dynamic-MRM run of 424 transitions, negative ESI, MassHunter 8.0) with the depositor's ProteoWizard 3.0.22110 mzML. **Prior art consulted:** none.

**What was inferred from what:**
- The acquisition method in the `.d` (`AcqData/Neg6470-2021-11.m/192_1.xml`) lists every transition with its precursor and product m/z as the analyst typed them, with 0 to 3 decimals (`241.01`, `142.07`, `115.1`, `137`). Our product m/z (`scan_window_mz`, the stored f64 of each MRM point list's m/z range) equals the method's value for every distinct (precursor, product) pair, including every product written with two or three decimals. The value MassHunter stores is the method's value; a one-decimal product is one the method states to one decimal.
- The mzML prints every product 0.004 below that value (`Q3=179.196` for 179.2, `Q3=241.006` for 241.01), as the earlier MetaboLights files showed. Precursors are not shifted.
- Fourteen transitions repeat a precursor, product and collision energy with an overlapping window; ProteoWizard writes them as separate chromatograms whose points alternate. Their scans carry different `scan_method` values (281 and 284 for 130 > 45), which tell them apart.

**Decided:** no reader change. The corpus harness now tries each `scan_method` group separately when a chromatogram's points come from more than one, and matches products within 0.005 (`srm_product_tolerance`, as for the other MetaboLights MRM files). With that, 425 of the run's 427 chromatograms are rebuilt exactly from our scans.

## 2026-10-06 — Q-TOF profiles in the ion-mobility encoding (MassHunter Acquisition 10.1) (Richard Zimring with Claude as assistant)

**Corpus files:** `mtbls12637-processblank2-pos-77-d` (MetaboLights MTBLS12637, CC0; 6546 LC/Q-TOF, acquisition software "6200 series TOF/6500 series Q-TOF 10.1 (48.0)", 2023) with the depositor's mzML (`mtbls12637-processblank2-pos-77-d-mzml`; vendor-centroided, so used only for peak positions). No held-out file. **Prior art consulted:** none new.
**Why.** `check` refused the file: "lzf: back-reference before the start of the output" on the first and last scan; `info` listed 1,194 spectra.
**What was inferred from what.** Each format-1 block of `MSProfile.bin` starts with the two f64 of a profile (26864.0 ns, 0.1 ns) uncompressed, then a u32 0x900C33A0 whose low 24 bits are the record's `PointCount` (799,648) and an i32 −1228, then signed integers of changing width: the layout of the ion-mobility profiles of 2026-09-25 (entry 6 above), not LZF. The record's `UncompressedByteCount` is still 16 + 4 × `PointCount`, which is why the reader took the block for LZF. Decoding every block with `decode_ims_profile` gives, on all 1,194 scans, counts that sum exactly to `TIC` with their maximum equal to `BasePeakValue`. Before the rule was recognised, reading the same stream as 16-bit values gave the same sums but put the base peak 8,191 bins early after every 0x8001 value (a skip of 8,191 that also switches to 4-byte values), which is how the existing rule was confirmed against the file's own base-peak m/z.
**Rule.** A format-1 block whose u32 at byte 16 has the top byte 0x90 and the record's `PointCount` in its low 24 bits is decoded as an ion-mobility profile; any other block as before (LZF when `ByteCount` differs from `UncompressedByteCount`).
