# Bench instruments without open readers — candidate survey

**Status:** survey of 2026-09-25 (Richard Zimring with Claude as assistant). Scope: native files of
common biochemistry/biotech bench instruments that no open-source reader handles (or only a
copyleft one does). For each candidate: public, licence-checked sample files (count of independent
depositors), existing readers and their licences, feasibility, and the decision. Searches used:
Zenodo's `file_type` facet (exact extension counts), Zenodo full-text search, figshare's search API,
GitHub repository and code search (`gh search repos|code`), and web search. Counts are as of the
survey date. This note is the starting point for later work; the per-format notes
(`docs/formats/<fmt>.md`) are the source of truth for what was implemented.

Rule applied throughout: a file enters the corpus only with a redistribution-compatible licence
(CC-BY/CC0 on the repository record, or the licence of the GitHub repository that holds it). A
repository with no licence is "all rights reserved" and its files are not used, however useful.

| candidate | extension(s) | public files found (depositors) | open readers (licence) | container | decision |
| --- | --- | --- | --- | --- | --- |
| Cytiva ÄKTA / UNICORN 3-5 results | `.res` | 2 dev files (PyCORN sample, GPL-2.0; unicoRn test file, GPL-3.0); 3 more in PyCornGUI (no licence: not used) | PyCORN (GPL-2.0, oracle only), unicoRn (GPL-3.0, R) | own binary directory of named blocks | **implement** (`openreadout-fplc`) |
| Cytiva ÄKTA / UNICORN 6-7 result export | `.zip` | 4 depositors: allotropy test data (MIT), univiz docs (GPL-3.0, 4 runs), fictional-spoon-fplc-2-ids (MIT, 4 runs), and a fourth MIT-licensed repository (held out, not named here) | allotropy `cytiva_unicorn` (MIT, readable as documentation, ASM output), PyCORN 0.19+ (GPL) | zip of zips; curves as .NET BinaryFormatter float arrays ([MS-NRBF], a public Microsoft specification) | **implement** (`openreadout-fplc`) |
| Bio-Rad Image Lab (ChemiDoc, Gel Doc, imported Typhoon/other scans) | `.scn` | ≥15 Zenodo depositors (western blots, gels) | Bio-Formats `BioRadSCNReader` (GPL, oracle only) | MIME multipart: XML headers + raw little-endian 16-bit image | **implement** (`openreadout-gel`) |
| Agilent Seahorse XF (Wave) assay results | `.asyr` | 3 Zenodo depositors (XFe24, XF96), no vendor Excel exports alongside | none open (seahtrue reads Wave's Excel export) | gzip-compressed XML (not encrypted) | **implemented** 2026-09-26 (`agilent-seahorse-asyr`, confidence low): sensor emissions, plate map, injections, protocol; OCR/ECAR are not stored and are not computed (no vendor export to validate a rate calculation) |
| MicroCal ITC (VP-ITC, ITC200) | `.itc` | 4 Zenodo depositors (82 files) | NITPIC (closed), pytc (reads NITPIC output), others read only exports | text: `$`/`#`/`%`/`?` header lines, `@n` injection markers, time/power/temperature rows | **implement** (`openreadout-biophys`) |
| Cytiva Biacore T200 control run | `.blr` | allotropy test data (MIT, 6 files, one study), Zenodo 5011513 (3 files) | allotropy `cytiva_biacore_t200_control` (MIT, readable as documentation) | OLE compound file | **implemented** 2026-09-26 (`cytiva-biacore-blr`): every sensorgram, report points, cycles; validated against the file's own report points and allotropy |
| Cytiva Biacore evaluation | `.bme` | Zenodo SGC USP5 series (one depositor, 5 records); allotropy test files are Git LFS pointers | allotropy (MIT) | binary | later |
| Malvern Zetasizer (Nano, ZS) | `.dts` | 9 Zenodo depositors | none permissive found (dts-extract, zetasizer_zs: no licence) | OLE compound file; `REC<n>` streams are field-order serialised records without names | later: needs many files plus vendor exports to map positions |
| Malvern ZS Xplorer | `.zmes` | none found | — | — | not implemented (no files) |
| Jasco Spectra Manager (CD, UV-Vis, IR, fluorescence) | `.jws` | 4 Zenodo depositors (FT-IR 4700 and others); JASCOFiles.jl (MIT), jws2txt (MIT) and jasco_jws_reader (GPL) test files on GitHub | jasco_jws_reader (GPL-3.0), JWSProcessor (GPL-2.0), raman-spectrum-toolkit (MIT) | OLE compound file (`DataInfo`, `Y-Data`, `X-Data`, `SampleInfo`, `MeasParam`) and a flat `L~S ` file | **implemented** 2026-09-26 in `openreadout-spectro` (`jasco-jws`): FT-IR, Raman, UV-Vis, CD, fluorescence; five depositors (JASCOFiles.jl and jws2txt test data, MIT; jasco_jws_reader samples, GPL; two Zenodo depositors), validated against Spectra Manager exports and jws2txt |
| Agilent Bioanalyzer 2100 Expert | `.xad` | none with a licence found (vendor demo data only as XML exports in bioanalyzeR, MIT) | bioanalyzeR (MIT) reads the XML **export**, not `.xad` | XML with a compressed base64 block | not implemented (no native files) |
| Agilent TapeStation | native `.D1000`-style result files | none found; XML/CSV exports in bioanalyzeR (MIT) and allotropy (MIT) | allotropy reads the XML export | — | not implemented (exports are already open text) |
| Sartorius Octet BLI | `.frd` | none found on Zenodo, figshare or GitHub | — | XML | not implemented (no files) |
| Thermo NanoDrop native | `.twbk`, `.sql` | none found; text/CSV exports only (allotropy test data) | allotropy reads exports | — | not implemented |
| Axon GenePix | `.gpr` | 2 Zenodo depositors; thousands on GEO | limma (GPL), Bioconductor | ATF text (Axon's public format) | later (plain text; low novelty) |
| MicroCal DSC | `.dsc` | 12 Zenodo records (not all MicroCal) | — | text | later, with ITC |
| Agilent Cary UV-Vis | `.bsw`, `.csw` | 1 depositor (pySpecData examples, CC0) | pySpecData (BSD-3) | OLE | later |
| Horiba LabSpec | `.l6s` | 2 depositors | none permissive | binary | later |
| Sartorius Incucyte | exports | text exports only | — | — | not a native-format gap |

Held-out plan: ÄKTA `.zip` has four independent depositors, so one of them is held out and never
inspected while developing; `.scn`, `.itc` and `.jws` have more than three depositors each and
hold one out likewise; Seahorse `.asyr` has exactly three depositors and holds one out; Biacore
`.blr` has two, so none is held out.

## 2026-09-26 retries and new families

Searches: Zenodo (API full-text and `filetype:` queries), figshare (API; many records answered 403
under rate limiting), Harvard Dataverse, Dryad, Mendeley Data (first 50 hits per query), OSF, GitHub
(repository and code search). Found and implemented: Bruker EPR, X-ray diffraction (XRDML, Bruker,
Rigaku), BioLogic EC-Lab, Gamry, Neware, NETZSCH Proteus and TA Instruments `.001` (see their format
notes). Still open:

| candidate | what was found | why not read |
| --- | --- | --- |
| Agilent Seahorse XF rates (OCR, ECAR) | Harvard Dataverse doi:10.7910/DVN/D5TSFG (CC0): three `.asyr` with Wave's per-well OCR in Prism files | the AKOS diffusion correction of Wave was not reproduced (a straight-line slope matches baseline rates, not post-injection ones); `docs/formats/agilent-seahorse.md` |
| Malvern Zetasizer | Zenodo 19044980 (`.dts` with the software's records export), 13860620 (14 `.dts`), Macquarie 29163440 (`.dts` with a size-distribution CSV) | `REC<n>` streams of an OLE file hold typed fields without names; Z-average and peaks are stored as float32, but PdI and zeta potential are not stored as the export prints them (derived at display); a reader would have to reproduce the Zetasizer analysis |
| TA Instruments TRIOS | `.tri` from 8 Zenodo depositors (DSC25/250/2500, TGA550, DMA850, Discovery HR-2/HR30), CSV/XLS exports for three | a header of 7-bit-length key/value strings, then serialized objects. Signals are records `21 0k <u32 len> <GUID>` holding a float32 array (SI units: time in s, heat flow in W, torque in N·m, phase in rad) or a u32 companion array; the GUIDs are global (the same Temperature GUID in DSC, TGA, DMA and rheometer files) and DSC/TGA headers name them in `proceduresignals` order. Not implemented because not every signal is such a record: in a rheometer time sweep the torque exists only as an all-zero u32 array and the exported values live elsewhere, and moduli and viscosities are computed by TRIOS, not stored |
| JASCO circular dichroism, flat `SPECMAN R2.0.0` files | none outside the held-out draw (figshare 24716316, 4704664); 265 JASCO files of 2 further depositors are all OLE or UV-Vis flat files | no development file of the layout; held-out files are not used to develop |
| JASCO measurement time (held-out FT-IR ATR file) | 46 public FT-IR files: SampleInfo record 1 and MeasParam tag 12 always agree | the disagreeing field could not be reproduced on a development file; a finding now flags files whose BaseInfo clock differs by more than an hour |
| Octet `.frd` | Harvard TTOPMD (CC0): 8 `.frd` without trace exports | no ground truth |
| Biacore `.bme` | Harvard GTNOTI/XHRSGM (CC0), SGC Toronto (Zenodo, 5 records, KD fits as PDF) | evaluation files; no sensorgram export to validate against |
| Cary `.BSW` | DataverseNL 10.34894/BQNQJX (CC-BY-4.0): two `.BSW` with CSV of the same measurement; Zenodo 21041480 and 15644941 (CC0, no CSV) | implemented (`agilent-cary`) from the corpus files and the depositors' CSV exports |
| NanoDrop `.twbk` | tbwk-opener (MIT, 2 files with expected counts in its tests) | one depositor |
| Typhoon scans in Image Lab `.scn` | Zenodo 6754439 (one FLA 9500 scan; its TIFF is an 8-bit RGB rendering; the record's Rotor-Gene file is held out, and nothing here was developed on it) | no linear export to validate square-root-encoded values |
| Bioanalyzer `.xad`, TapeStation | none with a licence | no files |
| Neware held-out `.ndax` (BTS 8.2) | SINTEF Zenodo 20802274 | held out (measurement only) |
| Arbin `.res`, Maccor | galvani/cellpy/beep test files (Arbin is an MS Access database) | not started |


## 2026-09-26 depth and new readers

Searches: Zenodo (API full text, `file_type` facet counts for `res`, `cex`, `ccs`, `dts`, `zmes`,
`mea`, `msr`, `mmes`, `twbk`, `bme`, `xad`, `gel`, `lxd`, `lxb`, `msd`, `vessel`, `xfd`), Harvard
Dataverse (dataset APIs), GitHub repository and code search (`gh search repos|code`: Arbin table
names, `zetasizer`, `twbk`, `landt cex`, Maccor, Novonix), figshare search for `.xfd`, allotropy
and cellpy/BEEP/navani/convpot test data trees.

| candidate | what was found | outcome |
| --- | --- | --- |
| Seahorse XF rates (OCR, ECAR, PER) | Harvard Dataverse D5TSFG (CC0, three `.asyr` with Wave's per-well rates as Prism files), seahtrue's PBMC Wave Excel export | implemented: the published compartment model (Gerencser et al. 2009), validated against Wave's own rates on 2,694 OCR values (≤ 0.3 pmol/min) and ECAR (≤ 0.16 mpH/min); withheld for other plates/settings (`docs/formats/agilent-seahorse.md`) |
| Seahorse legacy `.xfd` (XF24/XF96 software) | none on Zenodo (facet 0), figshare, GitHub | not implemented (no files) |
| TA Instruments TRIOS `.tri` | 7 Zenodo depositors (DSC25/2500, TGA550, DMA850, HR-2/HR30), exports for three | implemented (`ta-trios`), 30 rheometer exports matched incl. TRIOS's moduli; TRIOS 3/4 (2019) files refused; one depositor held out |
| Sartorius Octet `.frd` | Harvard TTOPMD (CC0) and pykingenie's test data (MIT); Katona-lab repository has no licence (not used) | implemented (`sartorius-octet-frd`), 6 files match pykingenie |
| Malvern Zetasizer `.dts` | 8 Zenodo depositors; exports of the same records for two (one of them the source of a held-out file of another format, so not usable) | implemented (`malvern-zetasizer-dts`): per-record zeta results, 252 cells of 36 records match; size results withheld (no usable export); distributions not decoded; one depositor held out |
| Malvern ZS Xplorer `.zmes` | none (Zenodo facet 0) | not implemented |
| Malvern Mastersizer `.mea` / `.mmes` / `.msr` | `.mea` from two records of one Sevilla group (their xlsx are rheometer data, not size exports); `.mmes` from one depositor (8183277) with no exports; one 275 MB `.msr` | not implemented (no ground truth) |
| Arbin MITS Pro `.res` (Jet 4 database) | cellpy (MIT, with golden output), navani, convpot (MIT), SINTEF Zenodo 21631502 (CC-BY), vince-wu/electrochem (MIT) | implemented (`arbin-res`) over a clean-room Jet reader; 7 files of four depositors match access_parser exactly; vince-wu held out |
| Arbin `.xlsx`/`.csv` exports, Maccor text exports (`.0NN`, `.txt`), Novonix `.csv`, Landt `.txt`/`.csv` | cellpy, BEEP (Apache-2.0), navani, SINTEF 21631502 | not implemented (text exports; scope frozen for the release) |
| Maccor raw binary | none: every public "Maccor" file (BEEP, cellpy, navani, SINTEF) is a text export | not implemented (no files) |
| Landt/LANHE `.cex`/`.ccs` | one `.ccs` (SINTEF 21631502) with no export of the same cell; no `.cex` on Zenodo or GitHub | not implemented (no ground truth) |
| Cytiva Biacore T200 evaluation `.bme` | allotropy's four test files (Git LFS objects, downloadable from GitHub's media endpoint, MIT), SGC Toronto (Zenodo, 5 records), Klasse/Moore lab (Harvard Dataverse GTNOTI, XHRSGM) | implemented (`cytiva-biacore-bme`): sensorgrams equal allotropy, 174 fits equal the item XML; the Klasse lab held out. Their `.ble` files (Biacore 3000/X100) are not read |
| Molecular Dynamics GEL (Typhoon, Storm, FLA) `.gel` | 8 Zenodo depositors | implemented in the TIFF reader: square-root samples returned as counts, bit-exact with tifffile's `mdgel` series on three depositors; one held out |
| Thermo NanoDrop `.twbk` | tbwk-opener (MIT, two files of one depositor); none on Zenodo | not implemented (one depositor; scope frozen) |
| Agilent Bioanalyzer `.xad`, TapeStation | none with a licence (Zenodo facet 0) | not implemented (no files) |
| Luminex xPONENT `.lxb`/`.lxd`, MSD | none on Zenodo (`msd` hits are ZooMS mass-spectrometry files) | not implemented (no files) |
| Sartorius Incucyte `.vessel` | none | not implemented (no files) |
