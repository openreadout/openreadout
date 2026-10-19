# MGF (Mascot generic format)

MGF is the plain-text peak-list format of Matrix Science's Mascot search engine, and the most common way to pass MS/MS spectra between tools: ProteoWizard msConvert, OpenMS, MZmine, GNPS, MassIVE-KB, PRIDE and many search engines write it. An MGF file holds processed peak lists (usually centroided), not raw scans. OpenReadout returns every spectrum with its precursor, retention time, charge and peaks. Derived from Matrix Science's public description of the format (<https://www.matrixscience.com/help/data_file_help.html>), with pyteomics (Apache-2.0) as the reference reader. Provenance: `docs/provenance/mgf.md`.

Format id `mgf`, extensions `.mgf` and `.mgf.gz`. It is read by the `openreadout-mzml` crate (`MgfReader`), next to mzML and mzXML, and shares their spectrum model.

## Layout

The file is text, one item per line. Blank lines may appear anywhere.

1. **File parameters.** Lines `KEY=value` before the first block apply to the whole file (Mascot search settings such as `COM`, `CLE`, `DB`, `MODS`, `ITOL`, `TOL`, `CHARGE`, `FORMAT`). Keys are case-insensitive; the reader upper-cases them. A file-level `CHARGE` is the charge of every spectrum that names none.
2. **Comments.** Lines starting with `#`, `;`, `!` or `/` outside blocks are comments (counted in `vendor.comment_lines`).
3. **Blocks.** `BEGIN IONS` ... `END IONS` holds one MS/MS spectrum: its own `KEY=value` lines first, then one peak per line, `m/z intensity [charge]`, separated by white space; anything after the third value is ignored (some writers annotate peaks, `72.04 101 # b1`).
4. **Peptide-mass-fingerprint lines.** A number outside a block is a peptide-mass-fingerprint query. These are counted (`extra.pmf_queries`), not read as spectra.

Detection: within the first 64 KiB, a `BEGIN IONS` line preceded only by blank lines, comments and `KEY=value` lines (definite). A `.mgf` name without that is an extension-only match. Gzip-compressed files are read through the same container as `.mzML.gz` (`docs/formats/mzml.md` § Containers).

MGF has no index. `open` reads the file once, line by line, and keeps the byte range and the parameters of each block; peaks are counted then and parsed only when a spectrum is read. A file that ends inside a block is **truncated**: the incomplete block is left out, `info` says so in `notes`, and `check` reports `truncated`.

## Block → `Spectrum`

| parameter | field |
| --- | --- |
| `PEPMASS=mz [intensity] [charge]` | `precursor_mz`, `precursor_intensity`; a charge printed here is that precursor's charge and is used before the block's `CHARGE`. Several `PEPMASS` lines (chimeric spectra): the first is `precursor_mz`, all are in `extra.precursors` |
| `CHARGE` (`2+`, `3-`, `2`, `2+ and 3+`, `2+,3+`) | `precursor_charge` when it names one charge and the first `PEPMASS` line names none; several are in `extra.charges` |
| `RTINSECONDS` (`95.3`, or a range `95-97`) | `rt_s` (the start of a range; the range is `extra.rt_range_s`). A value that is not a number is kept as `extra.rt_unparsed` |
| `SCANS` | `scan_number` when it is one whole number, else the position from 1; the text is `extra.scans` (`1278,1280-1284`, `F1:2478`) |
| `TITLE` | `extra.title`; a ProteoWizard title (`... NativeID:"controllerType=0 controllerNumber=1 scan=17"`) also gives `native_id` |
| `MSLEVEL` (library and feature exports) | `ms_level`; without it 2, as MGF holds MS/MS queries |
| `IONMODE`, `ION_MODE` (`positive`, `negative`) | `polarity`; without one `unknown` |
| every parameter of the block | `extra.params`, keys upper-cased, values as written |
| peak lines | `mz` and `intensity` in file order (intensities as 32-bit floats, like every spectrum); the optional third value is `extra.peak_charges` (null where a peak has none) |

`centroided` is true: Mascot accepts centroided peak lists only, and the format has no way to say otherwise.

## Run summary (`info` → `spectra[0]`)

| key | meaning |
| --- | --- |
| `scan_count`, `ms_levels`, `rt_range_s` | spectra, MS levels, retention-time range over the blocks that give one |
| `extra.ms_level_counts` | spectra per MS level |
| `extra.charge_states` | spectra per precursor charge (`none`, `several` when a block names more than one) |
| `extra.total_points` | peak lines in all blocks |
| `extra.spectra_with_rt` | blocks with a usable `RTINSECONDS` |
| `extra.chimeric_spectra` | blocks with more than one `PEPMASS` line |
| `extra.block_parameters` | how many blocks use each parameter key |
| `extra.file_parameters` | the file parameters |
| `extra.pmf_queries` | peptide-mass-fingerprint lines outside blocks |
| `extra.polarities` | polarities named by the blocks |
| `extra.dialect`, `extra.writer` | see below |
| `name` | the file name in the first block's `RAWFILE`, when there is one |

### Dialects

Writers use different optional parameters, which decides how titles, scan numbers, charges and polarity are read. `extra.dialect` names the one the parameters show (inferred from the corpus):

| dialect | recognised by |
| --- | --- |
| `spectral library (GNPS)` | the first block has `SPECTRUMID` or an `ORGANISM` starting with `GNPS` |
| `feature export (FEATURE_ID)` | the first block has `FEATURE_ID` (OpenMS GNPSExport, MZmine) |
| `search submission (file parameters)` | file parameters such as `SEARCH`, `DB`, `CLE`, `MODS` or `COM` |
| `annotated peptide spectra (SEQ)` | the first block has `SEQ` |
| `peak list` | none of these |

`extra.writer` names the software when the file says: `OpenMS` (a `COM` or `USERNAME` file parameter that names it), `GNPS` (a library record: `SPECTRUMID` `CCMSLIB…` or a `GNPS-` collection), `ProteoWizard` (titles with `NativeID:"…"`).

## `check`

`check --headers-only`: truncation, structure problems (a `BEGIN IONS` before the previous block's `END IONS`, an `END IONS` without a block, lines that are neither parameters nor peaks), blocks without a usable `PEPMASS` (`no_pepmass`). `check` also parses every peak line (`bad_peak`: a peak line without a finite m/z and intensity) and compares the count with the scan at open (`peak_count`).

## Vocabulary

| identifier | meaning |
| --- | --- |
| `MgfReader` | the MGF `FormatReader` |
| `MGF_FORMAT_ID` | `mgf` |
| `MgfDataset` | an open MGF file; `spectrum(i)` reads block `i` with its peaks |
| `spectrum` | read one spectrum with its peaks |
| `pmf_queries`, `comment_lines`, `file_parameters`, `blocks` | `vendor` keys: peptide-mass-fingerprint lines, comment lines, file parameters, block count |
| `title`, `scans`, `charges`, `precursors`, `params`, `peak_charges`, `rt_range_s`, `rt_unparsed` | spectrum `extra` keys (table above) |
| `ms_level_counts`, `charge_states`, `total_points`, `spectra_with_rt`, `chimeric_spectra`, `block_parameters`, `polarities`, `dialect`, `writer` | run `extra` keys (table above) |
| `truncated`, `structure`, `no_pepmass`, `bad_peak`, `peak_count` | `check` finding codes |
