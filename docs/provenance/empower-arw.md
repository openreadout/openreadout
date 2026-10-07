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

## 2026-10-06 — two more depositors, and exports with one field per line (Richard Zimring with Claude as assistant)

**Corpus files used:** `gpcreader-empower-sample1` … `sample8`, `gpcreader-empower-calib1`, `-calib2` (GitHub polychem-mk/GPCreader @8bd9280, `data-raw/`, **MIT**, checked 2026-10-06), and `hplcrs-empower-46739`, `-46751`, `-46779`, `-46783`, `-46795`, `-46799`, `-46804` (GitHub ArchercatNEO/HPLC @ac89e1d, `test/samples/PC12/`, **GPL-2.0**, checked 2026-10-06). Ground truth for all of them: chromConverter 0.9.0 (GPL-3.0, CRAN), run as a black box with `read_chroms(format_in = "waters_arw")` through the new `oracle/empower_arw_chromconverter.py`. No held-out file was opened; the held-out record of draw D (figshare 30962618) was not used.

**Prior art consulted:** GPCreader's `R/readGPC.R` and `R/file_info.R` (MIT) were read to learn where its author looks for the data rows. None of its code was used. The HPLC-RS README (GPL-2.0) describes the same header layout for its users' exports; only the README was read.

**What the files show:** GPCreader `sample5` … `sample8` use the layout already read (a names row, a values row), with some values unquoted numbers (`Injection`, `Injection Volume`, `Run Time`, `Data Start`, `Data End`) and no `Channel` field; the reader read them unchanged and every value equals chromConverter's. The other GPCreader exports and all seven HPLC-RS exports write the header as one field per line, `"name"<TAB>value`, where the value is quoted text or an unquoted number (8 and 5 lines here), followed by the usual `time<TAB>value` rows. The reader refused them: exit 3 for GPCreader `sample1` (its first line, `"Sample Set Id"<TAB>2597`, is not all quoted cells) and exit 4 for HPLC-RS (the first two lines were taken as a names row and a values row). chromConverter returns the header lines of these files as data rows, so the oracle keeps the rows chromConverter returned as two numbers; their count equals the files' numeric lines (1,200 and 3,301).

**Inferred, and decided:** an export whose header lines (the lines before the first row of two numbers) are each exactly two cells with a quoted name first is read as one field per line, when there are more than two such lines or when the first line's value is not quoted. Two quoted lines stay the names-row-and-values-row layout, as before, because both layouts could produce them; every development file with two header lines is of that layout. A first line of one quoted name and an unquoted number is claimed only for a file named `.arw`. The layout is reported in `extra.one_field_per_line` and as the assurance layout `one field per line`.
The channel name is now `Channel` without its surrounding spaces: GPCreader's exports write `"SATIN-2 "`, and the corpus comparison, like the other readers, takes names without them.
