# Provenance log — Bruker BAF (`analysis.baf` in a `.d` directory)

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

## 2026-10-12 — initial reader (Richard Zimring with Claude as assistant)

**Corpus files** (each with the depositor's own mzML conversion, made with the vendor library by ProteoWizard or by CompassXport; none is used by a held-out entry):

| id | instrument, software | source | licence |
| --- | --- | --- | --- |
| `mtbls6740-baf-methanolneg-4` | timsTOF Pro in BAF mode, otofControl 6.2 (2021) | MetaboLights MTBLS6740 | EMBL-EBI terms of use |
| `mtbls4823-baf-microtofq` | micrOTOF-Q, otofControl 3.4 (2021) | MetaboLights MTBLS4823 | EMBL-EBI terms of use |
| `mtbls14117-baf-impacthd` | impact HD, otofControl 5.2 (2023) | MetaboLights MTBLS14117 | CC0-1.0 |
| `msv000080036-baf-maxis` | maXis, otofControl 3.4 (2016), profile export | MassIVE MSV000080036 | CC0-1.0 |
| `msv000088109-baf-solarix` | solariX FT-ICR (2018), profile export | MassIVE MSV000088109 | CC0-1.0 |
| `msv000094648-baf-impact2` | impact II, otofControl 4.1 (2023), no `analysis.sqlite` | MassIVE MSV000094648 | CC0-1.0 |
| `pwiz-bruker-csi-apexqe` | apex-Qe FT-ICR, ApexControl 3.0 (2008) | ProteoWizard vendor-reader test data (already in the corpus) | Apache-2.0 |

**Prior art:** none. A web search on 2026-10-12 found no open-source BAF reader; the vendor's library and its download page were not opened. No GPL/LGPL source was read; ProteoWizard's mzML outputs were used only as ground truth.

**Method:** hex dumps of the files above, differential comparison between them, and value searches for what the depositors' mzML files and the vendor-written `analysis.sqlite` caches state (retention times, isolation m/z, intensities, calibrant masses). General physics without a source: time of flight grows with √(m/z); the FT-ICR frequency falls with 1/(m/z).

**What was inferred from what:**

- Record framing (u32 length, u32 tag `0xBFA0xxxx`) and that the chain ends at the file end: hex dumps of all seven BAFs, walking the chain.
- Pointer record, spectrum summary table and trailer index: hex of the timsTOF and apex files; every offset checked against the walked chain.
- Acquisition header (UTC date with offset, the string table, variable definitions, reference calibrations): timsTOF hex against `analysis.sqlite` `Properties` and `Variables` (7 variables × 8521 spectra equal).
- Spectrum record (segment, id, block flags, retention time in ms, polarity byte, precursor list as f32, variable list, calibration): timsTOF hex against `analysis.sqlite`; retention time, MS level, polarity and isolation m/z against every mzML.
- Centroid block (element sizes from the gaps between array offsets; position as a fractional sample index, intensity): all files; intensities equal the mzML.
- Profile block, float32 coding: apex and solariX against their mzML. Packed coding (the file names it a "Variable VLI2 based compressor"): a prefix-coded stream of deltas found in the timsTOF stream (runs of 9 zero bits before 32-bit run lengths, then the class widths by scoring which ones parse every block to its declared sample count, the sign rule from tokens around known profile values) and validated on all 3 166 464 samples of the maXis file. It is compression, not a protection measure.
- TOF calibration (`t = delay + timebase · index`, a quadratic in √(m/z), a residual polynomial inside [lo, hi] fading as exp(−d²) outside): least-squares fits of index against mzML m/z, then matching the fitted constants to payload doubles; calibrant masses in the timsTOF `calibration.sqlite` reproduced to 4e-11; the fade from peaks just beyond `hi`. Kind 10 (maXis): a mass shift subtracted instead of the polynomial.
- Temperature drift: ratios of per-spectrum constants to the header reference against the two detector temperatures and the gradients (timsTOF, micrOTOF-Q, maXis).
- Recalibration in `Calibrator.ami`: the doubles of its payload equal the timsTOF `calibration.sqlite` rows; applied with the drift ratio it reproduces the mzML m/z of five files.
- FT-ICR calibration `m/z = c3 / (f + c2)`, `f = f_high · (1 − index / N)`: apex profile m/z against index, confirmed on solariX.

## 2026-10-12 — fuzzing

`whole_baf` (seed: the synthetic fixture from `oracle/make_baf_fixtures.py`) found a trailer-index offset near the top of the u64 range that overflowed an addition while opening. Offsets are now added with saturation, and the record chain checks lengths against the remaining bytes. The input is `crates/openreadout-bruker-tims/tests/fixtures/malformed/bundle-whole_baf-trailer-offset-overflow.bin`.

## 2026-10-18 — more depositors; zlib-compressed float32 profiles (Richard Zimring with Claude as assistant)

**Corpus files** (each with the depositor's vendor-library conversion, whose source-file SHA-1 equals the `analysis.baf`'s; none is used by a held-out entry):

| id | instrument, software | source | licence |
| --- | --- | --- | --- |
| `msv000095055-baf-microtof` | micrOTOF-Q, micrOTOFcontrol 3.4 | MassIVE MSV000095055, ProteoWizard 3.0.24066 mzML | CC0-1.0 |
| `msv000080102-baf-maxis-impact` | maXis impact, otofControl 3.4 | MassIVE MSV000080102, CompassXport 3.0.13 mzXML | CC0-1.0 |
| `msv000078982-baf-microqtof` | micrOTOF-Q, otofControl 3.4.14 | MassIVE MSV000078982, ProteoWizard 3.0.6948 with CompassXtract 3.1.7 mzML | CC0-1.0 |
| `msv000086640-baf-solarix-xr` | solariX XR 9.4 T, ftmsControl | MassIVE MSV000086640, CompassXport 3.0.6 profile mzML | CC0-1.0 |

**Prior art:** none. The mzML and mzXML files were read with pyteomics (Apache-2.0).

**What was inferred from what:**

- The first two files read unchanged: 1759 and 2208 spectra agree with their conversions.
- In `msv000078982-baf-microqtof`, 29 MS1 spectra have a profile and an empty centroid block. The CompassXtract export labels every spectrum centroided but holds these 29 profiles, without the first and last samples and a few other zero samples. Their non-zero samples equal ours in m/z (as float32) and intensity. The reader keeps reporting the empty centroid list for these spectra; the corpus harness compares them by their non-zero profile points (manifest `export_profile_for_empty_centroids`).
- In `msv000086640-baf-solarix-xr` the profile block's coding bytes are `02 01 09 00` (the packed coding has `02 01 06 00`). Its payload starts with `78 01`, a zlib header; inflated, it is 32 078 332 bytes, 8 019 583 little-endian float32 samples, and the first u32 of the header after the coding bytes is the compressed length. The 635 756 non-zero samples equal the export's non-zero intensities exactly, in order. So coding byte 2 = 9 with byte 1 = 1 is read as zlib-compressed float32 samples. The export holds 713 976 points: exactly our non-zero samples and the zero samples next to them, with the same m/z (to 5e-13) and intensities. The corpus harness reduces our profile the same way for this file (manifest `export_zero_runs_trimmed`).
