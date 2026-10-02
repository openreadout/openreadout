# Provenance log — AIA/ANDI netCDF (chromatography and mass spectrometry)

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

## 2026-09-22 — initial implementation (Richard Zimring with Claude as assistant)

**Specification read (public):** Unidata, "The NetCDF Classic Format Specification" (netCDF-C documentation, https://docs.unidata.ucar.edu/netcdf-c/current/file_format_specifications.html; also Appendix "File Format Specifications" of the netCDF User's Guide), the BNF of the CDF-1/CDF-2/CDF-5 header, padding rules, record-variable interleaving and the single-record-variable exception. Used for everything in `netcdf.rs`.

**ANDI templates.** ASTM E1947 (chromatography) and ASTM E2077 (mass spectrometry) are sold by ASTM and were not obtained. netCDF files name their own dimensions, variables and attributes, so the mapping in `docs/formats/andi-chrom.md` was derived from the corpus files by listing their headers (with our reader and with scipy) and comparing exports of known instruments: which variables hold the signal (`ordinate_values`), the time base (`actual_sampling_interval`, `actual_delay_time`, `retention_unit`), the units (`detector_unit`), and for MS the scan table (`scan_acquisition_time`, `scan_index`, `point_count`, `total_intensity`) into the flat `mass_values`/`intensity_values` arrays.

**Corpus files used:** `cheminfo-agilent-hplc-cdf` and `cheminfo-agilent-gcms-cdf` (cheminfo/netcdf-gcms test data, MIT, commit ee7e2c0), `sciformats-andi-chrom-valid-cdf` (devrosch/sciformats test data, MIT, commit bf3d3ff: a synthetic file with every chromatography attribute filled), `mtbls1892-b20-pda-ch1-cdf` (Shimadzu LabSolutions export) and `mtbls390-wb-cc-bat-01-cdf` (Chrom-Card GC-FID export in minutes, time stamps written `20130507123000 + 0000`) from MetaboLights (EMBL-EBI Terms of Use), `zenodo7729413-sla-8-cdf` (Agilent GC-MS export, CC0-1.0).

**Inferred.** `retention_unit` `Minutes` scales the three time variables (MTBLS390: interval 0.001667 min = 0.1 s, 17,299 points = 28.83 minutes). Unused numeric values are written as −9999 (`a_d_sampling_rate`, `scan_duration`) and `time_values` holds the netCDF float fill value 9.97e36; neither is mapped. `total_intensity` is the per-scan TIC. The TIC is exposed as a trace only when the scan times are evenly spaced within 1 % (true for both GC-MS files), otherwise only through spectra.

**Prior art:** scipy's `scipy.io.netcdf_file` (BSD-3-Clause) is run as the oracle.

**Oracle results (2026-09-22):** all 6 ANDI corpus inputs pass: 6 traces bit-identical (xxh3 of the f64 values, first samples, point count, sample rate, first/last retention time), 3 peak tables bit-identical (column-major f64 xxh3), 400 spectra bit-identical (up to 200 evenly spaced scans of each of the 5,849- and 6,401-scan runs).

## 2026-09-24 — Scan headers without peaks (`spectra`, `Dataset::visit_scan_headers`)

**Scope:** ANDI/MS scans from the `scan_acquisition_time`, `point_count` and `total_intensity` variables and the file's polarity/experiment-type attributes; no mass or intensity values read.

**Validation:** `crates/openreadout-corpus-tests/tests/scans.rs` compares every oracle scan's header (scan number or native id, MS level, RT, polarity, filter, precursor m/z, charge, activation, isolation target, collision energy, 1/K0, position) with the corpus oracles (scipy netCDF: 2 files, 400 scans), and on up to 40 scans per file checks that the header equals the decoded spectrum's metadata.

**Prior art consulted:** none new.

## 2026-09-25 — assurance profile (`src/assurance.rs`)

**Corpus files:** every development input of this format on disk (`corpus/assurance/evidence.json` lists them); no held-out file.
**Prior art consulted:** none.
**What was done.** No parsing logic changed. The reader declares an assurance profile (docs/assurance.md) that fingerprints each file from this reader's own normalized output: the template revision and kind. The table of validated feature values and the confidence level are generated from the development corpus by `cargo xtask assurance-audit`.
