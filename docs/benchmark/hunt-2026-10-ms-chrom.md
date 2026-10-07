# Bug hunt on new mass-spectrometry and chromatography files (2026-10-06)

This workstream ran the readers on public files from records the corpus did not use, compared
them with the depositors' own conversions where they exist, and fixed what it found. No held-out
file was opened, and no file comes from a record a held-out draw reserves.

## Files surveyed

138 inputs from 45 source records, each run through `info`, `check`, the scan list, the first
spectrum (or trace), a chromatogram (or peaks) and an export, with time and peak memory recorded.

| source | licence | what |
| --- | --- | --- |
| MetaboLights (27 studies) | CC0 or EMBL-EBI terms | Agilent 5975/5977 GC/MSD, a 7890A GC/MSD ChemStation `.D`, 7010C GC-QQQ, 7200 GC/Q-TOF, 6224 TOF, 6540, 6400-series MRM; Thermo Q Exactive GC, Q Exactive Focus, Q Exactive Plus PRM, Elite, Orbitrap XL, Discovery DESI, SESI, TSQ Vantage, Exactive Plus GC, Exploris 480; Waters Xevo G2-XS, Synapt G2, Xevo TQ-S; Sciex API 3200/4000, QTRAP 6500, TripleTOF 5600; Bruker maXis (BAF) |
| PRIDE (5 projects) | CC0 or EBI terms | timsTOF SCP, 6545 (MOBILion), LTQ 2008, Orbitrap Fusion, TSQ Quantum Ultra |
| Zenodo 3604404 | CC-BY-4.0 | LTQ Velos |
| github.com/evanyeyeye/rainbow test data | LGPL-3.0 | MassHunter directories, ChemStation `.D` (versions 30 to 181, DAD, ADC, FID, MSD), OpenLab `.dx`, Waters UV/MS `.raw` |
| github.com/mzmine/mzmine test data | MIT | timsTOF LC-PASEF and MALDI spots, an Astral `.raw`, hand-edited mzML/mzXML |
| github.com/OpenMS/OpenMS test data | BSD-3-Clause | mzML, mzXML and mzMLb variants (indexes, compression, FAIMS, SWATH, chromatogram-only) |
| github.com/pymzml/pymzML test data | MIT | mzML with custom ids, numpress chromatograms |
| GitHub GC/LC repositories (lukeyf, pyGecko, saberger, cheminfo, DavidGoldLab: MIT; chromConverter, Oscillocat: GPL-3.0; adenylpred: AGPL-3.0) | as listed | ChemStation `.D` with CSV exports, MassHunter GC/MS `.D`, OpenLab `.dx`, ANDI |

No reader panicked or hung. 19 development inputs joined the corpus: rainbow's four MassHunter
directories with a rainbow-api oracle, and 15 vendor files paired with their depositors'
conversions.

## Failures found, and what changed

| finding | severity | before | after | evidence |
| --- | --- | --- | --- | --- |
| Agilent GC/MSD (5975, 5977) directories keep each scan's points in `MSPeak.bin` (16 bytes per point) and have no `MSProfile.bin` | S1: wrong values with no warning (the files were `unvalidated`, so `--strict` refused them) | every spectrum empty, `check` exit 4 | read | MTBLS12630: 12,257 of 12,257 spectra equal the depositor's ProteoWizard mzML; MTBLS1980: 5,808 of 5,808 |
| 7010C GC triple-quadrupole full scans store f32 abundances in the 8-byte point lists that MRM files fill with i32 counts | S2 | abundances of 10¹⁰ where the TIC is 2,036 | the block's MaxY decides the reading | MTBLS13904: on every scan the abundances sum to the record's TIC and peak at its base-peak m/z; the ANDI export is too coarse for an exact comparison |
| A scan whose profile sits in an `MSProfile.bin` the directory does not hold | S2 | empty spectrum | error; `info` lists `missing_files`, the assurance block reports it undecoded | unit behaviour, reached through the GC/MS investigation |
| `MSScan.xsd` written with `type="mstns:..."` | S3 | refused as corrupt | read | rainbow's `amber.D`, `cyan.D` (rainbow-api agrees) |
| Q-TOF profiles in the ion-mobility run-length encoding, uncompressed or with `UncompressedByteCount` holding the dense size | S3 | refused (LZF error or size mismatch) | read (the merged PR #30 found the same encoding on a 6546 file) | rainbow's `amber.D` (500 scans), `cyan.D`, `magenta.D`: counts equal rainbow-api's; m/z equal the stored base peaks |
| mzML text arrays (`null-terminated ASCII string`, MS:1001479) | S3 | spectrum failed as corrupt; `export` and `check` exit 4 | listed in `other_arrays`, not decoded | OpenMS `MzMLFile_6_*`; unit test |

Fuzz: `whole_masshunter` and `whole_mzml` were run for 10 minutes each after the changes (see the
pull request for the result).

## Read correctly, now pinned by the corpus

Thermo Q Exactive GC (555 scans), Q Exactive Focus (2,427), Q Exactive Plus PRM (4,338), Orbitrap
Elite with a detector channel (2,446), LTQ Orbitrap XL with analog controllers (1,697), Orbitrap
Discovery DESI (34), SESI breath on an Orbitrap XL (98) and TSQ Vantage SRM (194 chromatograms);
Agilent 6224 TOF (1,676), 6540 Q-TOF (1,689), 6545 with MOBILion (958) and 6400-series MRM
(487 chromatograms). All agree exactly with their depositors' conversions.

## Left open, and why

- **MALDI PASEF frames without precursors (timsTOF fleX, mzmine's `tims_spot.d`).** The reader
  returns no spectra and says so ("11 PASEF selections name no known precursor"); the assurance
  block lists them as left out. Defining a spectrum per selection needs an independent reference.
- **Sciex multi-sample `.wiff` memory.** `analyze chromatogram` on a 14.5 MB API 4000 file
  (MTBLS11852) peaked at 5.6 GB of memory and took 29 s; `export --to mzml` took 64 s at 1.7 GB.
- **7200 GC/Q-TOF (MTBLS1561).** Its 16-byte time-of-flight centroids are refused: the run has no
  calibration the reader applies. Its ANDI export is a nominal-mass reduction, not an oracle.
- **Exports too coarse to compare.** ProteoWizard 3.0.24002 rounds the MRM chromatograms of a
  MassHunter 10.1 run to whole numbers (MTBLS11480), and the 7010C's ANDI export keeps abundances
  at reduced precision. The corpus harness has no tolerance for either, so the first file was not
  added and the second carries `oracle_skip`.
- **mzML byte-shuffled and dictionary zstd (MS:1003781, MS:1003782).** Refused with a hint, as the
  known gaps say. OpenMS's `MzMLFile_zstd.mzML` is a test file for them.
- **Chromatogram-only files.** `export --to mzml` refuses mzML and Waters MRM files that hold
  chromatograms and no spectra ("the run has no spectra"). The command surface is being renamed in
  another pull request, so this was left alone.
- **Orbitrap Astral reporting its model as `MP01`** (mzmine's `astral.raw`): read, and
  `unvalidated` because the model maps to no known generation. No conversion exists to confirm it.
- **Not downloaded.** MassIVE returned HTTP 429 for the TSQ Altis SRM, Q Exactive GC, Velos Pro
  ETD, IQ-X and LTQ XL candidates (MSV000090848, 093924, 095828, 093028, 086254). The TSQ Altis
  run is the closest public match to held-out finding D-H1.
- **Evidence refresh.** `cargo xtask assurance-audit refresh` needs a full corpus run, which did
  not fit this workstream; the new files count once the next refresh runs.
- **Hand-edited test files.** `check` exits 4 on mzmine's and pymzML's cut mzML/mzXML files and on
  OpenMS files with stale counts or indexes. The files are damaged, so these findings are correct.
