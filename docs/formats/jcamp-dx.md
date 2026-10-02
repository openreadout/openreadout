# JCAMP-DX

JCAMP-DX is the open IUPAC text format for spectra, exported by many instruments and programs for IR, NMR, mass and other spectra. OpenReadout returns each data table as a trace with its x axis and the descriptive records, and `export --to jcamp` writes NMR and other 1-D spectra as JCAMP-DX. Derived from the public IUPAC JCAMP-DX protocols (IR 4.24, 1988; NMR, 1993; MS, 1994; 5.01, 1999) and checked on public test files (ISAS Dortmund test suite, R. J. Lancashire's public-domain files, MestReNova 14 exports); nmrglue's `jcampdx` (BSD-3) and the `jcamp` package (MIT) were read as prior art and serve as reference readers. Provenance: `docs/provenance/jcamp-dx.md`.

Format id `jcamp-dx`, family `spectroscopy`; extensions `jdx`, `dx`, `jcamp`, `jcm`. Section numbers (§) refer to the 4.24 protocol unless stated.

## Structure (`parse_jcamp`, `JcampFile`, `Block`, `Ldr`)

- Printable text in labeled data records (LDRs): `##LABEL= value`, continued on the following lines until the next `##` (§4.2). Labels are compared after upper-casing and dropping spaces, dashes, slashes and underscores (§4.4, `normalize_label`: `DATA TYPE`, `DATATYPE` and `Data_Type` are one label). `$$` starts a comment (removed from values). User-defined labels start with `$` (`##$AQ_mod=` in Bruker exports); data-type-specific labels start with `.` (`##.OBSERVE FREQUENCY=`).
- A **block** runs from `##TITLE=` to its `##END=`. Blocks nest: a `##DATA TYPE= LINK` block of a compound file holds child blocks (§6.1.4; `parent`, `depth`). Unbalanced `##END=` or a block without one are reported; a block without `##END=` is a `truncated` error. A bare `##END` (no `=`) closes the block too (`end_without_equals`, info).
- Lines may end in LF, CRLF or a lone CR (classic Mac, `jcamp-lancashire-mactab2`); blanks before `##` are accepted (`TESTNTUP.DX`). Text that is not UTF-8 is read as Latin-1 (`latin1`).
- Detection: the file starts (after blanks and a byte-order mark) with `##TITLE=` → definite; only the extension → extension-only.

## Data tables and how they become traces

One trace per data table, in file order (LINK blocks have none). Values are the table values multiplied by the declared factor; the factor is the channel `scale`.

| table | variable list | trace |
| --- | --- | --- |
| `##XYDATA=` (§6.4.1) | `(X++(Y..Y))`, `(X++(R..R))`, `(X++(I..I))` (NMR §4.1.1) | one channel (`y`, `real`, `imag`), factor `##YFACTOR=`; several `##XYDATA=` in one block become channels; X = `##FIRSTX=` + i·(`##LASTX=`−`##FIRSTX=`)/(`##NPOINTS=`−1) in `extra.axis` |
| `##NTUPLES=` (NMR §7, 5.01) | per page `##DATA TABLE= (X++(R..R)), XYDATA` | pages keyed `N=1`, `N=2` holding different symbols (R, I) become **channels**; pages keyed by another variable (`F1=17094.6`, a retention time) become **sweeps** (`extra.sweep_axis`); channel names are the `##VAR_NAME=` entries, factors `##FACTOR=`, units `##UNITS=`; the independent variable's `##FIRST=`/`##LAST=`/`##VAR_DIM=` give the axis |
| `##PEAK TABLE=`, `##XYPOINTS=` (§6.4.2–3) | `(XY..XY)`, `(XYW..XYW)`, `(XYM..XYM)` | channels `x`, `y` [, `w`/`m`] (the abscissa is a channel because it is not evenly spaced; `extra.axis.irregular` = true); factors `##XFACTOR=`/`##YFACTOR=`; non-numeric components (multiplicity letters, `<strings>`) are NaN |
| NTUPLES with `(XY..XY)` pages (MS spectral series) | | channels from the group symbols, one sweep per page; pages may differ in length (`sample_count` is the largest; `extra.sweep_sample_counts` lists each page's count when they differ) |
| `##PEAK ASSIGNMENTS=`, `##RADATA=`, JCAMP-CS structure blocks | | listed by `info --view structure`, no trace |

`sample_count`: `##NPOINTS=` (XYDATA, peak tables) or the independent variable's `##VAR_DIM=` (NTUPLES). `sample_rate_hz`: (n−1)/|LASTX−FIRSTX| when the X unit is time (`SECONDS`, `MS`, `MINUTES`), else 0. `start_s`: FIRSTX (or the NTUPLES `##FIRST=`) in seconds when the X unit is time and the abscissa increases, else unset (a falling time axis is placed by `extra.axis` only).

## ASDF (`decode_asdf`, `lex_asdf_line`; §5)

Each line: an abscissa (AFFN), then ordinates. Tokens (`YToken`):

| form | characters | meaning |
| --- | --- | --- |
| AFFN / PAC | `+`, `-`, digits, `.`, exponent `E±dd` | actual value (`Abs`); a sign also separates values (PAC) |
| SQZ | `@ A–I` = 0…9, `a–i` = −1…−9 | actual value, leading digit and sign in one character (`Abs`) |
| DIF | `% J–R` = 0…9, `j–r` = −1…−9 | difference from the previous ordinate (`Dif`) |
| DUP | `S–Z` = 1…8, `s` = 9 | the previous token occurs this many times in total (`Dup`; in DIFDUP the difference repeats) |
| `?` | | missing or off-scale ordinate (`Invalid`, NaN) |

`E`/`e` is an exponent only when followed by a sign and a digit; otherwise it is SQZ 5 / −5.

**Check-points (§5.8).** When a line ends in DIF form (including a DUP of a DIF), the next line's first ordinate repeats the last ordinate (Y-value check): it is compared and dropped, and a DUP right after it repeats that value (`TESTNTUP.DX` writes `h5T`). A mismatch is a `y_check` error. Each line's abscissa labels the ordinate at its index (the repeated one after a DIF line); abscissas more than half a step plus their own rounding (half a unit in their last written digit, times `XFACTOR`) from `FIRSTX` + i·step are an `x_sequence` warning (MestReNova writes whole Hz; Spectrafile-IR labels the first new point). The last line of a DIF table may hold only the check value.

## Descriptive records copied into trace `extra` (our name ← label)

| our name | label | notes |
| --- | --- | --- |
| `kind` | – | `xydata`, `ntuples`, `peak_table`, `xypoints` |
| `block` | – | block index (see `info --view structure`) |
| `title` | `TITLE` | also the trace `name` |
| `jcamp_version` | `JCAMP-DX` | |
| `data_type`, `data_class` | `DATA TYPE`, `DATA CLASS` | |
| `origin`, `owner` | `ORIGIN`, `OWNER` | |
| `spectrometer` | `SPECTROMETER/DATA SYSTEM` | |
| `instrument_parameters` | `INSTRUMENT PARAMETERS` | |
| `sample_description` | `SAMPLE DESCRIPTION` | |
| `compound_name`, `names`, `molecular_formula`, `cas_registry_number` | `CAS NAME`, `NAMES`, `MOLFORM`, `CAS REGISTRY NO` | |
| `date`, `time`, `long_date` | `DATE`, `TIME`, `LONG DATE` | verbatim |
| `nucleus` | `.OBSERVE NUCLEUS` | e.g. `^13C` |
| `observe_frequency_mhz` | `.OBSERVE FREQUENCY` | |
| `field_t` | `.FIELD` | |
| `scans` | `.AVERAGES` | |
| `solvent` | `.SOLVENT NAME` | |
| `pulse_sequence` | `.PULSE SEQUENCE` | |
| `acquisition_mode` | `.ACQUISITION MODE` | |
| `ms_spectrometer_type`, `ms_inlet`, `ms_ionization_mode` | `.SPECTROMETER TYPE`, `.INLET`, `.IONIZATION MODE` | MS protocol §5.2 |
| `resolution`, `min_y`, `max_y`, `first_y` | `RESOLUTION`, `MINY`, `MAXY`, `FIRSTY` | numbers |
| `axis` | `XUNITS`/`FIRSTX`/`LASTX`/`NPOINTS` or NTUPLES `UNITS`/`FIRST`/`LAST`/`VAR_DIM` | `{quantity, unit, first, last, step, size}`; quantity from the unit: `frequency` (HZ), `chemical_shift` (PPM), `time`, `wavenumber` (1/CM), `wavelength` (NANOMETERS, MICROMETERS), `mass_to_charge` (M/Z), else `x` |
| `sweep_axis` | NTUPLES `PAGE=` | `{variable, unit, values}` |
| `sweep_sample_counts` | – | points per page when NTUPLES pages differ in length |
| `ntuples`, `symbols`, `variables` | `NTUPLES`, `SYMBOL`, `VAR_NAME` | |
| `undecodable`, `problems`, `note` | – | why a table cannot be read |

Records of the enclosing LINK block apply to its children (a compound file's shared `##ORIGIN=`), except `TITLE`, `DATA TYPE` and `DATA CLASS`. Every record is in `info --view full`'s vendor tree (`blocks[].records`, tables summarized as their variable list and line count).

## `check` finding codes

`truncated`, `no_blocks`, `y_check`, `npoints_mismatch`, `bad_table`, `missing_label` (XYDATA without FIRSTX/LASTX/NPOINTS) (errors); `x_sequence`, `firsty_mismatch` (tolerance: one factor step), `missing_label`, `outside_block`, `extra_end`, `bad_label`, `undecodable`, `no_data` (warnings); `non_numeric`, `non_utf8`, `end_without_equals` (info).

## Observed corpus values

| id | writer | table | notes |
| --- | --- | --- | --- |
| `jcamp-isas-brukaffn`, `-bruksqz`, `-brukpac`, `-brukdif` | Bruker NMR JCAMP-DX V1.0 (1992) | XYDATA 16384, AFFN / SQZ / PAC / DIFDUP | AFFN, SQZ and PAC decode to identical values |
| `jcamp-isas-brukntup`, `-testntup` | Bruker, ISAS | NTUPLES R/I, 16384 | TESTNTUP: X column is an index × `##FACTOR=` |
| `jcamp-isas-testfid` | ISAS | NTUPLES FID (TIME, FID/REAL, FID/IMAG) | |
| `jcamp-isas-pe1800`, `-specfile` | Perkin Elmer 1800, Spectrafile-IR | IR XYDATA PAC, DIFDUP | SPECFILE: `S` (DUP 1) after every value; last line `31999@` fails its check (0 ≠ 26506): `check` exit 4 |
| `jcamp-isas-ms1`, `-ms2` | ISAS MS | PEAK TABLE; XYDATA with descending X in seconds | `##DATA CLASS= PEAKTABLE` |
| `jcamp-lancashire-compound`, `-blckpac1` | UWI Mona | LINK with 5 IR / UV-Vis XYDATA blocks | |
| `jcamp-lancashire-dupinc1`, `-pacdec1`, `-sqzdec1` | Perkin Elmer, T. Davies | UV-Vis DUP, IR PAC, NMR SQZ | |
| `jcamp-lancashire-pktab1`, `-mactab2` | UWI Mona | MS PEAK TABLE | mactab2: CR line ends and a trailing 0xFF byte |
| `nmrxiv-s200-qhnmr` | MestReNova 14.0.1 (JCAMP-DX 6.0) | LINK + XYDATA 104858 points | X written in whole Hz |
| `nmrxiv-s200-hsqc` | MestReNova 14.0.1 | nD NMR SPECTRUM NTUPLES, 1024 pages `F1=` × 820 points `(F2++(Y..Y))` | `##FIRST=` repeated in every page |

## Writing (`export --to jcamp`; `export_jcamp`, `jcamp_write.rs`)

One trace per file, JCAMP-DX 5.01 (the NMR protocol's layout, readable by the IR/MS protocol readers):

| trace | written as |
| --- | --- |
| one channel, one sweep, regular axis | `##DATA CLASS= XYDATA`, `##XYDATA= (X++(Y..Y))` with `XUNITS`/`YUNITS`, `XFACTOR`, `YFACTOR`, `FIRSTX`, `LASTX`, `DELTAX`, `NPOINTS`, `FIRSTY` |
| several channels or sweeps | `##DATA CLASS= NTUPLES`: `##NTUPLES=`, `VAR_NAME`, `SYMBOL` (`X`, then `R`/`I` for a `real`/`imag` pair, `Y` for one channel, `Y1`, `Y2`, … otherwise, and `N`), `VAR_TYPE`, `VAR_FORM`, `VAR_DIM`, `UNITS`, `FIRST`, `LAST`, `FACTOR`; one `##PAGE= N=k` + `##DATA TABLE= (X++(R..R)), XYDATA` per channel and sweep (one sweep: `N` numbers the channels, as NMR FIDs do; several sweeps: `N` numbers the sweeps and every sweep has one page per channel) |
| irregular abscissa (`extra.axis.irregular`: peak tables, point lists), two columns, one sweep | `##PEAK TABLE= (XY..XY)` (or `##XYPOINTS=`) with AFFN `x,y` pairs in their shortest exact decimal form, factors 1 |

Header: `TITLE` (trace name), `JCAMP-DX= 5.01`, `DATA TYPE` (the source's own for JCAMP-DX input; `NMR FID`/`NMR SPECTRUM` for Bruker time-domain/processed traces; `CHROMATOGRAM` for chromatography; `UNKNOWN` otherwise), `DATA CLASS`, `ORIGIN`, `OWNER` (source or experiment model; `(not recorded)` when unknown), `LONGDATE` (`YYYY/MM/DD HH:MM:SS` from the acquisition start), `SPECTROMETER/DATA SYSTEM`, `.OBSERVE FREQUENCY`, `.OBSERVE NUCLEUS` (`^1H`), `.SOLVENT NAME`, `.PULSE SEQUENCE`, and `##$OPENREADOUT SOURCE=` (format id, file name, trace).

Ordinates are integers × a per-channel factor (`YFACTOR` or the NTUPLES `FACTOR`). The factor is, in order: the channel's own `scale` (a JCAMP-DX input's `YFACTOR`) when every value is exactly an integer multiple of it; else the largest power of two that makes every value an integer (Bruker values are integers × 2^`NC`), provided the integers stay within 2^53; else (values spanning more than 53 bits, e.g. decimal fractions) a power of two that fits the largest magnitude in 32 bits, and the report says `exact: false` with `max_abs_error` ≤ factor / 2. Non-finite values are refused (exit 6).

DIFDUP (default): each line is the abscissa as an integer multiple of `XFACTOR` = |step| (the Bruker convention), the first ordinate in SQZ form and differences in DIF form, repeats of a difference as DUP counts of one character (≤ 9 per group, because the `jcamp` Python package reads single-character counts only); lines are at most 80 characters and each ends in DIF form, so the next line starts with the Y-value check (the last ordinate again); a final line repeats the last ordinate as the last check. `--compression none` writes AFFN instead (space-separated integers).

Verification: the file is re-read with `JcampDataset`: one trace of the written shape, every ordinate equal to `integer × factor` bit for bit, the axis within 1e-9 relative, and `check` without errors; only then is it renamed into place. Third-party readers: `oracle/jcamp_validate.py` (nmrglue for NMR data types, `jcamp` for XYDATA and peak tables; `jcamp` cannot read negative numbers in these tables, so files whose abscissa goes below zero, e.g. most ppm spectra, are read by nmrglue only).

## Vocabulary (every public identifier in `crates/openreadout-nmr/src/jcamp_*.rs` must appear here)

| identifier | meaning |
| --- | --- |
| `JcampReader`, `JcampDataset`, `JCAMP_FORMAT_ID`, `open` | reader entry points: format reader, opened file (core `Dataset`), the id `jcamp-dx` |
| `JcampFile`, `blocks`, `issues`, `latin1`, `len`, `parse_jcamp` | a parsed file: blocks, structural findings, Latin-1 fallback, size |
| `Block`, `index`, `parent`, `depth`, `ldrs`, `offset`, `end`, `closed` | a `##TITLE=` … `##END=` block |
| `get`, `all`, `text`, `number`, `title`, `data_type`, `data_class`, `is_link` | record lookup and common labels |
| `Ldr`, `label`, `key`, `value`, `line`, `head`, `body` | one labeled data record: label as written, normalized key, value, line number; first line and table body |
| `normalize_label`, `parse_affn` | label normalization (§4.4) and AFFN numbers (§4.5.3) |
| `YToken` { `Abs`, `Dif`, `Dup`, `Invalid` } | ASDF ordinate tokens |
| `AsdfLine`, `x`, `x_resolution`, `tokens`, `lex_asdf_line` | one lexed data line |
| `AsdfTable`, `y`, `x_checks`, `y_check_failures`, `lines`, `decode_asdf` | a decoded `(X++(Y..Y))` table and its check-points |
| `decode_groups` | `(XY..XY)` group tables |
| `export_jcamp`, `default_jcamp_output` | write one trace as JCAMP-DX (verified by re-reading); the default output name `<stem>[.traceT][.sweepS].jdx` |
| `JcampExportOptions`, `rows`, `encoding`, `overwrite` | what to write: trace, sweep, samples `[first, last]` of each sweep, DIFDUP or AFFN, replace an existing file |
| `JcampEncoding` { `Difdup`, `Affn` } | ordinate form |
| `JcampExportReport`, `input`, `output`, `jcamp_version`, `data_class`, `sweeps`, `pages`, `first_sample`, `samples_written`, `channels_written`, `factors`, `exact`, `max_abs_error`, `bytes_written`, `verified` | the export report (`format`, `data_type`, `trace` as elsewhere in this table) |
