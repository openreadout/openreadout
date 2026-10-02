# Provenance log — Waters Empower ASCII raw-data exports (`.arw`)

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

## 2026-09-26 — initial reader (Richard Zimring with Claude as assistant)

**Search for public Empower data.** Zenodo (file-type `arw`: one record, Sony camera raw files), Figshare, GitHub code search and the chromConverterExtraTests README table were searched on 2026-09-26. No Empower database backup or other native Empower data was found in any public repository; what is public are exports. Empower's native data lives in its database, which is not a per-run file (Waters' public product descriptions); the reader therefore reads the ASCII export, and Empower's AIA/netCDF exports are read by `andi-chrom`.

**Corpus files used:** `appia-empower-results1844` … `results1853` (6 exports; GitHub PlethoraChutney/Appia @a001b2c, `test-files/`, **MIT** (c) 2021 Richard Posert — the Appia section of the repository's `LICENSE`, read 2026-09-26; also listed as MIT in the chromConverterExtraTests README for `waters.arw`), and Appia's `processed-tests/Exp106_SEC_hplc-wide.csv` (same repository and licence) as the oracle. `waters_pda.arw` in chromConverterExtraTests has no licence recorded and was not used.

**Prior art.** Appia (MIT, https://github.com/PlethoraChutney/Appia) reads these exports; its processed output was used as ground truth (run output only).

**Inferred from the files:** the first row holds the exported field names in double quotes separated by tabs, the second their values; every further row a time and a value separated by a tab; rows end with CR. The field set is what the Empower export method selected (`SampleName`, `Channel`, `Sample Set Name`, `Instrument Method Name` here). Times are minutes (0 to 55 min, 0.008333 min apart, printed to 7 significant digits); the value unit is not stated. Every value of the six exports equals Appia's processed values for the same sample and channel exactly; Appia's times are the exported ones in single precision (differences below 2·10⁻⁶ min).
