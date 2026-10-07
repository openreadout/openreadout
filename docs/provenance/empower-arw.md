# Provenance log — Waters Empower ASCII raw-data exports (`.arw`)

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

## 2026-09-26 — initial reader (Richard Zimring with Claude as assistant)

**Search for public Empower data.** Zenodo (file-type `arw`: one record, Sony camera raw files), Figshare, GitHub code search and the chromConverterExtraTests README table were searched on 2026-09-26. No Empower database backup or other native Empower data was found in any public repository; what is public are exports. Empower's native data lives in its database, which is not a per-run file (Waters' public product descriptions); the reader therefore reads the ASCII export, and Empower's AIA/netCDF exports are read by `andi-chrom`.

**Corpus files used:** `appia-empower-results1844` … `results1853` (6 exports; GitHub PlethoraChutney/Appia @a001b2c, `test-files/`, **MIT** (c) 2021 Richard Posert — the Appia section of the repository's `LICENSE`, read 2026-09-26; also listed as MIT in the chromConverterExtraTests README for `waters.arw`), and Appia's `processed-tests/Exp106_SEC_hplc-wide.csv` (same repository and licence) as the oracle. `waters_pda.arw` in chromConverterExtraTests has no licence recorded and was not used.

**Prior art.** Appia (MIT, https://github.com/PlethoraChutney/Appia) reads these exports; its processed output was used as ground truth (run output only).

**Inferred from the files:** the first row holds the exported field names in double quotes separated by tabs, the second their values; every further row a time and a value separated by a tab; rows end with CR. The field set is what the Empower export method selected (`SampleName`, `Channel`, `Sample Set Name`, `Instrument Method Name` here). Times are minutes (0 to 55 min, 0.008333 min apart, printed to 7 significant digits); the value unit is not stated. Every value of the six exports equals Appia's processed values for the same sample and channel exactly; Appia's times are the exported ones in single precision (differences below 2·10⁻⁶ min).

## 2026-10-06 — exports without the two header rows (Richard Zimring with Claude as assistant)

Held-out draw D reported an `.arw` export without header rows, with CR line endings, that was not detected (exit 3; finding D-G4). No held-out file was opened.

**Search for a development file.** figshare (`:extension: arw`), Zenodo (the file-type index and full-text search) and GitHub code search, 2026-10-06. Every licensed `.arw` found has a header: the two-row layout already read (Appia; Zenodo 20720664, CC-BY-4.0, from tissue studies, not added) or a vertical key/value header (polychem-mk/GPCreader, MIT; ArchercatNEO/HPLC, GPL-2.0). Repositories with one-field headers (camsunlab/BO-RFB, EmeryBosten/Peak_Detection) state no licence. No public headerless export was found.

**Corpus files used:** the development exports `appia-empower-results1844` … `results1853` (Appia, MIT). **Prior art consulted:** none.

**What was inferred, and from what:** the rows after the header of every development export are `time<TAB>value`, unquoted decimal numbers, with the export's line ending. An export method that selects no header fields leaves those rows alone. Nothing in such a file names Empower, so only the `.arw` extension together with rows of two numbers identifies it.

**Decided:** a file with the `.arw` extension whose first lines are all two tab-separated numbers is read as an Empower export without a header: the same rows, no fields, the channel named `value`. Without the extension the file is not claimed (any two-column text would match). No development file of this layout exists, so the reader reports the layout `headerless export` as a variant feature, which no development file validates: the trace is `unvalidated` and `--strict` refuses it. The unit tests build the layout from a development export with its two header rows removed.
