# m/z and intensity agreement with vendor-library conversions

The corpus test `crates/openreadout-corpus-tests/tests/mz_agreement.rs` compares every spectrum
OpenReadout reads from a vendor file with a conversion of the same file made with the vendor's own
library: ProteoWizard's reference conversions of its vendor-reader test data (pwiz commit
699dd48953f8; data and mzML only, Apache-2.0) and depositors' msconvert conversions. For each
reference point it finds our nearest m/z in the matching spectrum and records the m/z difference in
ppm and the relative intensity difference; a reference point with no counterpart within
`mz_ppm_max` is unmatched. Spectra are matched by native id, scan number, position or, for timsTOF,
by frame and mobility scan (`mz_match`). The test fails when any file exceeds its limits in
`corpus/manifest.toml` (`mz_ppm_max`, `mz_intensity_rel_max`, `mz_unmatched_max`,
`mz_unmatched_spectra_max`). With `MZ_RESULTS=<file>` it also writes per-file results that
`cargo xtask assurance-audit refresh` counts as independent confirmation of the spectra.

```bash
OPENREADOUT_CORPUS_DIR=corpus/files cargo test -p openreadout-corpus-tests --features corpus --profile corpus --test mz_agreement -- --nocapture
```

## Results (2026-09-26)

Every compared reference point has a counterpart, and no file exceeds its limits.

| vendor | files | spectra | points | max \|ppm\| | intensities |
| --- | --- | --- | --- | --- | --- |
| Bruker timsTOF (TDF compression types 1 and 2, TSF line and profile, MALDI) | 5 | 2,512 | 422,509 | 0.060 | equal (diaPASEF within 1.6·10⁻⁴ relative: the export lowers some counts by one) |
| Waters MassLynx (12-, 8- and 6-byte values, centroid and continuum) | 9 + 2 depositor pairs | 815 | 2,846,123 | 0.060 | equal |
| Thermo RAW (ion-trap and Orbitrap profiles, ETD, HCD, MSX, SPS, in-source CID) | 7 | 100 | 304,131 | 0.059 | equal |
| Agilent MassHunter (quadrupole profiles, TOF centroids, 6560 ion mobility) | 8 | 9,511 | 2,038,349 | 0.060 | equal |

The largest differences, about 0.06 ppm, are the rounding of the references' 32-bit m/z arrays
(half a unit in the last place of an f32 is 0.06 ppm); 64-bit and 12-byte-centroid references
agree to 0.0000 ppm. The Bruker values are OpenReadout's application of each frame's MzCalibration
and TimsCalibration models (1/K0 within 5·10⁻¹³ V·s/cm² of the reference).

Per file:

| input | reference | spectra (not comparable) | points | unmatched | median |ppm| | p99 |ppm| | max |ppm| | median |ΔI|/I | max |ΔI|/I | max |Δ1/K0| (n) |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| pwiz-bruker-hela-pasef | pwiz-bruker-hela-pasef-mzml | 1793 (57) | 682 | 0 | 0.0206 | 0.0506 | 0.0566 | 0.00e0 | 0.00e0 | 5.0e-13 (1850) |
| pwiz-bruker-thyroglob-prm | pwiz-bruker-thyroglob-prm-mzml | 548 (0) | 2509 | 0 | 0.0208 | 0.0498 | 0.0574 | 0.00e0 | 0.00e0 | 5.0e-13 (548) |
| pwiz-bruker-diapasef | pwiz-bruker-diapasef-mzml | 99 (0) | 46495 | 0 | 0.0001 | 0.0001 | 0.0001 | 0.00e0 | 1.57e-4 | 5.0e-13 (99) |
| pwiz-bruker-urine-tsf | pwiz-bruker-urine-tsf-mzml | 50 (0) | 199432 | 0 | 0.0209 | 0.0533 | 0.0594 | 0.00e0 | 0.00e0 | - |
| pwiz-bruker-urine-tsf | pwiz-bruker-urine-tsf-centroid-mzml | 20 (0) | 55589 | 0 | 0.0000 | 0.0000 | 0.0000 | 0.00e0 | 0.00e0 | - |
| pwiz-bruker-maldi-tsf | pwiz-bruker-maldi-tsf-mzml | 1 (0) | 117464 | 0 | 0.0205 | 0.0526 | 0.0596 | 0.00e0 | 0.00e0 | - |
| pwiz-bruker-maldi-tsf | pwiz-bruker-maldi-tsf-centroid-mzml | 1 (0) | 338 | 0 | 0.0001 | 0.0001 | 0.0001 | 0.00e0 | 0.00e0 | - |
| pwiz-waters-091204-nfdm-008 | pwiz-waters-091204-nfdm-008-mzml | 41 (0) | 36298 | 0 | 0.0000 | 0.0000 | 0.0000 | 0.00e0 | 0.00e0 | - |
| pwiz-waters-160109-mix1-calcurve-070 | pwiz-waters-160109-mix1-calcurve-070-mzml | 15 (0) | 4105 | 0 | 0.0000 | 0.0000 | 0.0000 | 0.00e0 | 0.00e0 | - |
| pwiz-waters-atehlstlsek-lm-684-3469 | pwiz-waters-atehlstlsek-lm-684-3469-mzml | 8 (0) | 56519 | 0 | 0.0000 | 0.0000 | 0.0000 | 0.00e0 | 0.00e0 | - |
| pwiz-waters-atehlstlsek-lm-785-8426 | pwiz-waters-atehlstlsek-lm-785-8426-mzml | 8 (0) | 56519 | 0 | 0.0000 | 0.0000 | 0.0000 | 0.00e0 | 0.00e0 | - |
| pwiz-waters-atehlstlsek-profile | pwiz-waters-atehlstlsek-profile-mzml | 8 (0) | 296097 | 0 | 0.0203 | 0.0522 | 0.0594 | 0.00e0 | 0.00e0 | - |
| pwiz-waters-dda-isolationwindow | pwiz-waters-dda-isolationwindow-mzml | 2 (0) | 0 | 0 | NaN | NaN | NaN | NaN | NaN | - |
| pwiz-waters-minimal-dda | pwiz-waters-minimal-dda-mzml | 9 (0) | 9 | 0 | 0.0000 | 0.0000 | 0.0000 | 0.00e0 | 0.00e0 | - |
| pwiz-waters-mse-short | pwiz-waters-mse-short-mzml | 3 (0) | 248295 | 0 | 0.0203 | 0.0520 | 0.0595 | 0.00e0 | 0.00e0 | - |
| pwiz-waters-qc-lcms2-2-23-268-1-1 | pwiz-waters-qc-lcms2-2-23-268-1-1-mzml | 2 (0) | 49 | 0 | 0.0216 | 0.0447 | 0.0447 | 0.00e0 | 0.00e0 | - |
| pwiz-thermo-ltqvelos | pwiz-thermo-ltqvelos-mzml | 85 (0) | 292989 | 0 | 0.0191 | 0.0518 | 0.0591 | 0.00e0 | 0.00e0 | - |
| pwiz-thermo-bsa-ft-etd | pwiz-thermo-bsa-ft-etd-mzml | 1 (0) | 200 | 0 | 0.0228 | 0.0483 | 0.0538 | 0.00e0 | 0.00e0 | - |
| pwiz-thermo-bsa-ft-etd | pwiz-thermo-bsa-ft-etd-centroid-mzml | 1 (0) | 28 | 0 | 0.0000 | 0.0000 | 0.0000 | 0.00e0 | 0.00e0 | - |
| pwiz-thermo-bsa-ft-hcd | pwiz-thermo-bsa-ft-hcd-mzml | 1 (0) | 509 | 0 | 0.0218 | 0.0524 | 0.0554 | 0.00e0 | 0.00e0 | - |
| pwiz-thermo-bsa-ft-hcd | pwiz-thermo-bsa-ft-hcd-centroid-mzml | 1 (0) | 70 | 0 | 0.0000 | 0.0000 | 0.0000 | 0.00e0 | 0.00e0 | - |
| pwiz-thermo-ft-hcd-msx | pwiz-thermo-ft-hcd-msx-mzml | 1 (0) | 1504 | 0 | 0.0000 | 0.0000 | 0.0000 | 0.00e0 | 0.00e0 | - |
| pwiz-thermo-ft-hcd-msx | pwiz-thermo-ft-hcd-msx-centroid-mzml | 1 (0) | 1504 | 0 | 0.0000 | 0.0000 | 0.0000 | 0.00e0 | 0.00e0 | - |
| pwiz-thermo-it-hcd-sps | pwiz-thermo-it-hcd-sps-mzml | 1 (0) | 3088 | 0 | 0.0000 | 0.0000 | 0.0000 | 0.00e0 | 0.00e0 | - |
| pwiz-thermo-isolation-offset | pwiz-thermo-isolation-offset-mzml | 1 (0) | 1920 | 0 | 0.0208 | 0.0537 | 0.0591 | 0.00e0 | 0.00e0 | - |
| pwiz-thermo-isolation-offset | pwiz-thermo-isolation-offset-centroid-mzml | 1 (0) | 241 | 0 | 0.0000 | 0.0000 | 0.0000 | 0.00e0 | 0.00e0 | - |
| pwiz-thermo-source-cid | pwiz-thermo-source-cid-mzml | 3 (0) | 1039 | 0 | 0.0000 | 0.0000 | 0.0000 | 0.00e0 | 0.00e0 | - |
| pwiz-thermo-source-cid | pwiz-thermo-source-cid-centroid-mzml | 3 (0) | 1039 | 0 | 0.0000 | 0.0000 | 0.0000 | 0.00e0 | 0.00e0 | - |
| pwiz-agilent-gfb-4scan-timesegs | pwiz-agilent-gfb-4scan-timesegs-mzml | 182 (0) | 222901 | 0 | 0.0000 | 0.0000 | 0.0000 | 0.00e0 | 0.00e0 | - |
| pwiz-agilent-apci-piscan | pwiz-agilent-apci-piscan-mzml | 170 (0) | 249414 | 0 | 0.0000 | 0.0000 | 0.0000 | 0.00e0 | 0.00e0 | - |
| pwiz-agilent-mmi-piscan | pwiz-agilent-mmi-piscan-mzml | 337 (0) | 618659 | 0 | 0.0000 | 0.0000 | 0.0000 | 0.00e0 | 0.00e0 | - |
| pwiz-agilent-thyrxox-ts-diff-scan | pwiz-agilent-thyrxox-ts-diff-scan-mzml | 233 (0) | 196885 | 0 | 0.0000 | 0.0000 | 0.0000 | 0.00e0 | 0.00e0 | - |
| pwiz-agilent-ims-ccs | pwiz-agilent-ims-ccs-mzml | 701 (0) | 169825 | 0 | 0.0257 | 0.0557 | 0.0590 | 0.00e0 | 0.00e0 | - |
| pwiz-agilent-ims-allions | pwiz-agilent-ims-allions-mzml | 1192 (0) | 207193 | 0 | 0.0216 | 0.0500 | 0.0594 | 0.00e0 | 0.00e0 | - |
| pwiz-agilent-ims-chrom | pwiz-agilent-ims-chrom-mzml | 6484 (0) | 349162 | 0 | 0.0205 | 0.0516 | 0.0595 | 0.00e0 | 0.00e0 | - |
| pwiz-agilent-tof-sulfas | pwiz-agilent-tof-sulfas-mzml | 212 (0) | 24310 | 0 | 0.0202 | 0.0529 | 0.0595 | 0.00e0 | 0.00e0 | - |
| mtbls7290-scfa240-001-neg-blank-raw | mtbls7290-scfa240-001-neg-blank-raw | 247 (0) | 132053 | 0 | 0.0000 | 0.0000 | 0.0000 | 0.00e0 | 0.00e0 | - |
| pxd059722-mth2-alicine-td-1-raw | pxd059722-mth2-alicine-td-1-raw | 472 (0) | 2016179 | 0 | 0.0000 | 0.0000 | 0.0000 | 0.00e0 | 0.00e0 | - |

*spectra (not comparable)*: reference spectra compared and, in parentheses, those with no
counterpart (the truncated sixth frame of the HeLa PASEF file). `pwiz-waters-dda-isolationwindow`
stores one zero-intensity value per function, so none of its points count.

## Not compared here

- Waters ion-mobility and SONAR functions (the drift-resolved `.cdt` data is not decoded) and
  MRM functions (tables).
- Sciex files: ProteoWizard's Sciex test data are MRM acquisitions (compared as SRM chromatograms
  in `tests/corpus/`) or `.wiff2` (refused); the TripleTOF depositor exports hold the vendor's peak
  picking, which is not reproduced (`peaks_not_compared`).
- Agilent and Thermo SRM/MRM files (compared as SRM chromatograms in `tests/corpus/`).
