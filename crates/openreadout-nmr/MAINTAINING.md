# Maintaining `openreadout-nmr`

NMR and JCAMP-DX: Bruker TopSpin / XWIN-NMR experiment directories (`bruker-nmr`), Varian/Agilent VnmrJ `.fid` directories (`varian-nmr`), JEOL Delta `.jdf` (`jeol-jdf`), Magritek Spinsolve experiment directories (`magritek-spinsolve`), and JCAMP-DX (`jcamp-dx`, read and written). Project-wide process: [docs/maintaining.md](../../docs/maintaining.md). Notes: `docs/formats/{bruker-nmr,varian-nmr,jeol-jdf,magritek-spinsolve,jcamp-dx}.md`; provenance logs of the same names. JCAMP-DX is an open IUPAC standard; Bruker parameter syntax follows nmrglue's documentation (BSD-3).

## Decode pipeline

Shared: `text.rs` (ASCII in principle, UTF-8 or Latin-1 in practice). NMR processing of FIDs (group delay, apodization, FT, phasing) lives in `openreadout-signal`, not here.

- **Bruker** (`bruker_layout.rs`, `bruker_params.rs`, `bruker_dataset.rs`): resolve the experiment directory from any file in it (`resolve_experiment_dir`); parameter files `acqus`, `acquNs`, `procs`, `procNs` (`##$NAME= value` records, `(0..n)` arrays, `<strings>`; `parse_param_file`). The raw layout (`RawLayout`: `fid` or `ser`, `DTYPA` int32/float64, `BYTORDA`, rows on 1024-byte boundaries, `nuslist`) and processed layouts (`ProcLayout`: 1D/2D/3D components tiled by `SI` and `XDIM`). **TopSpin versions branch** on `DTYPA`, `DSPFVS`/`DECIM`/`GRPDLY` (group delay sources) and the software version in `acqus` (`software`). Values: time domain scaled by 2^NC (inferred, see fragile spots). Experiment facts (`bruker_experiment.rs`): USERA1–5, the `pdata` title, `##OWNER=`.
- **Varian** (`varian_layout.rs`, `varian_procpar.rs`, `varian_dataset.rs`): the `fid` file header (32 bytes, big-endian) and blocks (28-byte block headers, traces of interleaved real/imaginary int16/int32/float32 values; status bits give the element type), and `procpar` (three lines per parameter).
- **JEOL** (`jeol_header.rs`, `jeol_dataset.rs`): a 1360-byte big-endian header (identifier `JEOL.NMR`, byte order of the rest, dimensions, data type and layout, axis types, sizes, valid ranges, units, section offsets), 64-byte parameter records, data sections (submatrices), and the context section after the data (the experiment text; only its `sample_id` line is read, for ids longer than the 16-byte text parameters).
- **Spinsolve** (`spinsolve.rs`): the experiment directory of any file in it (`resolve_spinsolve_dir_in`; folders of experiments are listed), `acqu.par`/`proc.par` (`name = value`, `ParFile`), `Phase()` of `processing.script`, and every Prospa data file (32-byte header `PROS DATA V1.1`, data type 501 complex / 503 x + real / 504 x + complex, rows back to back). The FID (`data.1d`, `fid.1d`, `data.2d` with `nrPnts` points) comes first; an x block becomes `extra.axis` only when its spacing matches the dwell time or the spectral width. **Software versions branch** on the data type of `data.1d` (1.41: 504 with a time block; 2.0x: 501).
- **JCAMP-DX** (`jcamp_parse.rs`, `jcamp_asdf.rs`, `jcamp_dataset.rs`): labeled data records in nested blocks; tables `(X++(Y..Y))` in ASDF (AFFN, PAC, SQZ, DIF, DUP with Y and X check-points) or AFFN groups `(XY..XY)`; NTUPLES pages (NMR 5.01 complex data), PEAK TABLE, XYPOINTS. **JCAMP-DX versions and data classes branch here** (`data_type`, `data_class`, 4.24 vs 5.01 NTUPLES).
- **Writer** (`jcamp_write.rs`): `export --format jcamp` writes XYDATA in DIFDUP with Y checks (or AFFN), or NTUPLES pages; factors chosen to round-trip exactly when possible; verified by read-back.

## Invariants and checks

- Bruker: parameter files parse; required parameters present with known values; `fid`/`ser` length against TD and the row count; processed files against SI and XDIM tiling; the group delay determinable.
- Varian: header counts and sizes consistent with status bits; fid length against the declared blocks; block numbers sequential; procpar parses and agrees with the header (`np`, `arraydim`).
- Spinsolve: parameter lines parse; Prospa headers (magic, known type, sizes without overflow); file lengths against the declared rows; the FID's rows against `nrSteps`.
- JEOL: header identifier, dimensions, data type and layout, valid ranges; parameter records against the section length; data sections fit the file.
- JCAMP-DX: every `##TITLE=` has an `##END=`, labels well formed, required labels per data block; every table decodes with its check-points; point counts against `##NPOINTS=`/`##VAR_DIM=`; `##FIRSTY=` against the first ordinate.

## Debugging a new file

- `openreadout info DIR --view structure` lists the parameter and data files (Bruker, Varian) or the JCAMP-DX blocks; `info --view full --json` → `vendor` has every parameter as stored.
- `tests/synthetic.rs` (Bruker, JCAMP-DX), `tests/varian_jeol.rs` and `tests/jcamp_export.rs` (writer round trips); `jcamp_asdf.rs` has the spec's own examples as unit tests (`spec_examples`, `table_vi_difdup_and_checkpoint`).
- Spinsolve: `oracle/spinsolve.py` (nmrglue for parameters and single-row 504 files, the documented layout cross-checked against depositor CSVs); the software's own processed spectra in `*.pt1` plot files check FID processing (`tests/nmr_processing.rs`).
- Oracles: nmrglue (Bruker, Varian, JEOL, JCAMP-DX), with the `jcamp` package as a second opinion on JCAMP-DX (`oracle/gen.py`, `oracle/jcamp_validate.py` for exports).

## Fragile spots

- **2^NC scaling** of Bruker time-domain values is by analogy with nmrglue's NC_proc rule (inferred); the digital-filter group delay is reported and removed only when an FID is processed.
- `DTYPA`/`DTYPP` other than int32 and float64, 4D+ processed data and NUS reconstruction are not supported (NUS rows are returned as acquired, `nuslist` as a table).
- The JEOL context section's text syntax (`name => value;`) is inferred from 20 corpus files; the parameter stays the sample id when the context holds no `sample_id` line that begins with it, and a 16-character parameter is then flagged as possibly cut (`sample_id_truncated`, an assumed value).
- Spinsolve: the unit of an x block is not stored (named only when the spacing says so); data types other than 501/503/504, multi-row files with an x block, a non-zero first-order phase and `filter = "yes"` apodization have no public example; plot files (`.pt1`/`.pt2`) are not decoded.
- JCAMP-DX files in the wild bend the standard (labels, line endings, `##END=` placement): the parser collects problems instead of failing, and `check` reports them.

<!-- BEGIN GENERATED guide -->
## Facts (generated)

*Generated by `cargo xtask guides --write` from the sources, `corpus/manifest.toml`, `corpus/assurance/evidence.json` and `corpus/intake/`; do not edit. CI fails when it is stale.*

### Formats

| format id | notes and provenance | confidence | basis | development files: read / confirmed | depositors | held-out pass / fail |
| --- | --- | --- | --- | --- | --- | --- |
| `bruker-nmr` | [format note](../../docs/formats/bruker-nmr.md), [provenance log](../../docs/provenance/bruker-nmr.md) | high | prior art | 50 / 50 | 25 | 4 / 0 |
| `jcamp-dx` | [format note](../../docs/formats/jcamp-dx.md), [provenance log](../../docs/provenance/jcamp-dx.md) | high | open spec | 22 / 21 | 4 | 1 / 0 |
| `varian-nmr` | [format note](../../docs/formats/varian-nmr.md), [provenance log](../../docs/provenance/varian-nmr.md) | high | prior art | 12 / 12 | 6 | 1 / 0 |
| `jeol-jdf` | [format note](../../docs/formats/jeol-jdf.md), [provenance log](../../docs/provenance/jeol-jdf.md) | high | prior art | 14 / 14 | 8 | 2 / 0 |
| `magritek-spinsolve` | [format note](../../docs/formats/magritek-spinsolve.md), [provenance log](../../docs/provenance/magritek-spinsolve.md) | medium | prior art | 26 / 26 | 2 | - |

### Source map

| file | what it does (its module documentation) |
| --- | --- |
| [`src/assurance.rs`](src/assurance.rs) | Assurance profiles (`docs/assurance.md`) of the NMR and JCAMP-DX readers (Bruker TopSpin, JCAMP-DX, Agilent/Varian VnmrJ, JEOL Delta): the variant features of a data set and the |
| [`src/bruker_dataset.rs`](src/bruker_dataset.rs) | `Dataset` for a Bruker experiment directory: one trace for `fid`/`ser`, one per processed `pdata/<procno>` spectrum |
| [`src/bruker_experiment.rs`](src/bruker_experiment.rs) | Experiment facts of a Bruker experiment directory that the normalized model cannot carry (`Dataset::experiment`): the user sample fields `USERA1`…`USERA5` of `acqus`, the free-text |
| [`src/bruker_layout.rs`](src/bruker_layout.rs) | Bruker experiment directory layout: which files exist, how `fid`/`ser` and processed files are laid out on disk |
| [`src/bruker_params.rs`](src/bruker_params.rs) | Bruker parameter files (`acqus`, `acqu2s`, `procs`, `proc2s`, …): JCAMP-DX-style text with `##$NAME= value` records |
| [`src/jcamp_asdf.rs`](src/jcamp_asdf.rs) | Tabular data decoding: ASDF (AFFN, PAC, SQZ, DIF, DUP; JCAMP-DX 4.24 §5) for `(X++(Y..Y))` tables, and AFFN groups for `(XY..XY)` tables |
| [`src/jcamp_dataset.rs`](src/jcamp_dataset.rs) | `Dataset` for JCAMP-DX: one trace per data table (XYDATA, NTUPLES, PEAK TABLE, XYPOINTS) |
| [`src/jcamp_parse.rs`](src/jcamp_parse.rs) | JCAMP-DX structure: labeled data records (LDRs) grouped into (possibly nested) blocks |
| [`src/jcamp_write.rs`](src/jcamp_write.rs) | JCAMP-DX writer (`export --format jcamp`): one trace (an NMR FID or spectrum, a JCAMP-DX spectrum, a chromatogram, ...) as `##XYDATA=(X++(Y..Y))` when it has one channel and one |
| [`src/jeol_dataset.rs`](src/jeol_dataset.rs) | `Dataset` for a JEOL Delta `.jdf` file: one trace (an FID or a processed spectrum) whose sweeps are the rows of a 2D data set |
| [`src/jeol_header.rs`](src/jeol_header.rs) | JEOL Delta `.jdf` file header and parameter section |
| [`src/lib.rs`](src/lib.rs) | Readers for NMR data |
| [`src/spinsolve.rs`](src/spinsolve.rs) | `Dataset` for a Magritek Spinsolve experiment directory (`<yymmdd-hhmmss> <protocol> (<suffix>)/` with `acqu.par`, optional `proc.par` and `processing.script`, and Prospa data |
| [`src/text.rs`](src/text.rs) | Text decoding shared by both readers: parameter files and JCAMP-DX files are ASCII in principle, UTF-8 or Latin-1 in practice |
| [`src/varian_dataset.rs`](src/varian_dataset.rs) | `Dataset` for a Varian/Agilent VnmrJ data directory (`<name>.fid/` with `fid`, `procpar`, `text`, `log`): one time-domain trace whose sweeps are the traces of `fid` in disk order |
| [`src/varian_layout.rs`](src/varian_layout.rs) | Varian/Agilent VnmrJ data directory layout: the `fid` file header and block headers, sample decoding and path resolution |
| [`src/varian_procpar.rs`](src/varian_procpar.rs) | The `procpar` parameter text of a Varian/Agilent VnmrJ data directory |

### Where variants branch

The assurance profile ([`src/assurance.rs`](src/assurance.rs)) observes these features (each value is looked up in the validated table below; a value never confirmed makes the file `unvalidated` for the feature's outputs) and reports these structures and assumptions:

- feature record `k`
- feature sample_layout `s`
- feature acquisition `format!("{}D", dims.len() + 1)`
- feature acquisition `format!("encoding {e}")`
- feature acquisition `"non-uniform sampling"`
- feature writer `name`
- feature layout `f`
- feature sample_layout `b`
- feature acquisition `format!("AQ_mod {m}")`
- feature format_version `v`
- feature acquisition `d.to_ascii_uppercase()`
- feature layout `k`
- feature layout `"no data table"`
- feature format_version `"no procpar"`
- feature format_version `jdf`
- feature sample_layout `format!("axis {ax}")`
- feature sample_layout `format!("data type {code}")`
- feature layout `ext`
- feature writer_version `format!("{name} {v}")` (descriptive)
- feature record `format!("group delay from {g}")` (descriptive)
- feature instrument `i` (descriptive)
- feature writer_version `format!("Delta {d}")` (descriptive)
- feature writer_version `format!("Spinsolve {v}")` (descriptive)
- undecoded "unreadable data file"
- undecoded "rows missing from the data file"
- undecoded "unsupported data layout"
- undecoded "data layout"
- assumed "traces[].sweep_count"
- assumed "traces[].sweeps order"
- assumed "traces[].extra.sample_id"
- calibration "digital-filter group delay"
- calibration "block scale factors and drift correction"

### Validated variants and the corpus files that pin them

| format | kind | value | outputs | confirmed files | read | example corpus files |
| --- | --- | --- | --- | --- | --- | --- |
| `bruker-nmr` | acquisition | `2D` | traces | 5 | 5 | `nmrglue-bruker-2d`, `nmrglue-bruker-3d`, `nmrxiv-s501-6` |
| `bruker-nmr` | acquisition | `AQ_mod 1` | traces | 4 | 4 | `nmrxiv-s1132-2`, `nmrxiv-s1247-15`, `nmrxiv-s596-1` |
| `bruker-nmr` | acquisition | `AQ_mod 3` | traces | 46 | 46 | `nmrglue-bruker-1d`, `nmrglue-bruker-2d`, `nmrglue-bruker-3d` |
| `bruker-nmr` | acquisition | `encoding Echo-Antiecho` | traces | 1 | 1 | `nmrxiv-s837-21` |
| `bruker-nmr` | acquisition | `encoding States-TPPI` | traces | 2 | 2 | `nmrxiv-s501-6`, `nmrxiv-s837-24` |
| `bruker-nmr` | acquisition | `encoding undefined` | traces | 2 | 2 | `nmrglue-bruker-2d`, `nmrglue-bruker-3d` |
| `bruker-nmr` | acquisition | `non-uniform sampling` | traces | 1 | 1 | `nmrxiv-s837-21` |
| `bruker-nmr` | field | `experiment.acquisition.started_at` | descriptive | 50 | 50 | `nmrglue-bruker-1d`, `nmrglue-bruker-2d`, `nmrglue-bruker-3d` |
| `bruker-nmr` | field | `experiment.instrument.model` | descriptive | 13 | 13 | `nmrxiv-s1439-1h-nmr`, `nmrxiv-s1462-13c-nmr`, `nmrxiv-s1462-1h-nmr` |
| `bruker-nmr` | layout | `fid` | traces | 45 | 45 | `nmrglue-bruker-1d`, `nmrxiv-s1082-80`, `nmrxiv-s1082-81` |
| `bruker-nmr` | layout | `ser` | traces | 5 | 5 | `nmrglue-bruker-2d`, `nmrglue-bruker-3d`, `nmrxiv-s501-6` |
| `bruker-nmr` | record | `group delay from DSPFVS/DECIM table` | descriptive | 6 | 6 | `nmrglue-bruker-1d`, `nmrglue-bruker-2d`, `nmrglue-bruker-3d` |
| `bruker-nmr` | record | `group delay from GRPDLY` | descriptive | 44 | 44 | `nmrxiv-s1082-80`, `nmrxiv-s1082-81`, `nmrxiv-s1132-1` |
| `bruker-nmr` | record | `processed_spectrum` | traces | 44 | 44 | `nmrxiv-s1082-80`, `nmrxiv-s1082-81`, `nmrxiv-s1132-1` |
| `bruker-nmr` | record | `time_domain` | traces | 50 | 50 | `nmrglue-bruker-1d`, `nmrglue-bruker-2d`, `nmrglue-bruker-3d` |
| `bruker-nmr` | sample_layout | `big-endian` | traces | 6 | 6 | `nmrglue-bruker-1d`, `nmrglue-bruker-2d`, `nmrglue-bruker-3d` |
| `bruker-nmr` | sample_layout | `float64` | traces | 8 | 8 | `nmrxiv-s275-1`, `nmrxiv-s275-13`, `nmrxiv-s702-1` |
| `bruker-nmr` | sample_layout | `int32` | traces | 50 | 50 | `nmrglue-bruker-1d`, `nmrglue-bruker-2d`, `nmrglue-bruker-3d` |
| `bruker-nmr` | sample_layout | `little-endian` | traces | 47 | 47 | `nmrxiv-s1082-80`, `nmrxiv-s1082-81`, `nmrxiv-s1132-1` |
| `bruker-nmr` | writer | `TopSpin` | metadata, traces | 47 | 47 | `nmrxiv-s1082-80`, `nmrxiv-s1082-81`, `nmrxiv-s1132-1` |
| `bruker-nmr` | writer | `XWIN-NMR` | metadata, traces | 3 | 3 | `nmrglue-bruker-1d`, `nmrglue-bruker-2d`, `nmrglue-bruker-3d` |
| `bruker-nmr` | writer_version | `TopSpin 1` | descriptive | 2 | 2 | `nmrxiv-s596-1`, `nmrxiv-s597-1` |
| `bruker-nmr` | writer_version | `TopSpin 2` | descriptive | 9 | 9 | `nmrxiv-s1132-2`, `nmrxiv-s1250-1`, `nmrxiv-s1250-2` |
| `bruker-nmr` | writer_version | `TopSpin 3` | descriptive | 27 | 27 | `nmrxiv-s1082-80`, `nmrxiv-s1082-81`, `nmrxiv-s1132-1` |
| `bruker-nmr` | writer_version | `TopSpin 4` | descriptive | 9 | 9 | `nmrxiv-s275-1`, `nmrxiv-s275-13`, `nmrxiv-s702-1` |
| `bruker-nmr` | writer_version | `XWIN-NMR 3` | descriptive | 3 | 3 | `nmrglue-bruker-1d`, `nmrglue-bruker-2d`, `nmrglue-bruker-3d` |
| `jcamp-dx` | acquisition | `CONTINUOUS MASS SPECTRUM` | traces | 0 | 1 |  |
| `jcamp-dx` | acquisition | `INFRARED SPECTRUM` | traces | 5 | 5 | `jcamp-isas-pe1800`, `jcamp-isas-specfile`, `jcamp-lancashire-compound` |
| `jcamp-dx` | acquisition | `MASS SPECTRUM` | traces | 2 | 3 | `jcamp-isas-ms1`, `jcamp-lancashire-pktab1` |
| `jcamp-dx` | acquisition | `ND NMR SPECTRUM` | traces | 0 | 1 |  |
| `jcamp-dx` | acquisition | `NMR FID` | traces | 1 | 1 | `jcamp-isas-testfid` |
| `jcamp-dx` | acquisition | `NMR SPECTRUM` | traces | 8 | 9 | `jcamp-isas-brukaffn`, `jcamp-isas-brukdif`, `jcamp-isas-brukntup` |
| `jcamp-dx` | acquisition | `UV-VISIBLE SPECTRUM` | traces | 1 | 1 | `jcamp-lancashire-dupinc1` |
| `jcamp-dx` | acquisition | `UV/VIS SPECTRUM` | traces | 1 | 1 | `jcamp-lancashire-blckpac1` |
| `jcamp-dx` | field | `experiment.acquisition.started_at` | descriptive | 3 | 3 | `nmrxiv-s200-hsqc`, `nmrxiv-s200-qhnmr`, `zenodo5374178-menthol-jdx` |
| `jcamp-dx` | field | `experiment.instrument.model` | descriptive | 17 | 18 | `jcamp-isas-brukaffn`, `jcamp-isas-brukdif`, `jcamp-isas-brukntup` |
| `jcamp-dx` | format_version | `4.24` | metadata, traces | 8 | 8 | `jcamp-isas-pe1800`, `jcamp-isas-specfile`, `jcamp-lancashire-blckpac1` |
| `jcamp-dx` | format_version | `5` | metadata, traces | 1 | 1 | `jcamp-lancashire-pktab1` |
| `jcamp-dx` | format_version | `5.0` | metadata, traces | 6 | 6 | `jcamp-isas-brukaffn`, `jcamp-isas-brukdif`, `jcamp-isas-brukntup` |
| `jcamp-dx` | format_version | `5.00` | metadata, traces | 3 | 4 | `jcamp-isas-ms1`, `jcamp-isas-ms2`, `jcamp-isas-testfid` |
| `jcamp-dx` | format_version | `6.0` | metadata, traces | 3 | 3 | `nmrxiv-s200-hsqc`, `nmrxiv-s200-qhnmr`, `zenodo5374178-menthol-jdx` |
| `jcamp-dx` | layout | `ntuples` | traces | 2 | 4 | `jcamp-isas-brukntup`, `jcamp-isas-testfid` |
| `jcamp-dx` | layout | `peak_table` | traces | 2 | 3 | `jcamp-isas-ms1`, `jcamp-lancashire-pktab1` |
| `jcamp-dx` | layout | `xydata` | traces | 14 | 15 | `jcamp-isas-brukaffn`, `jcamp-isas-brukdif`, `jcamp-isas-brukpac` |
| `jeol-jdf` | acquisition | `2D` | traces | 3 | 3 | `nmrxiv-s200-cosy-jdf`, `nmrxiv-s200-hmbc-jdf`, `nmrxiv-s200-hsqc-jdf` |
| `jeol-jdf` | acquisition | `encoding complex` | traces | 2 | 2 | `nmrxiv-s200-hmbc-jdf`, `nmrxiv-s200-hsqc-jdf` |
| `jeol-jdf` | acquisition | `encoding real_complex` | traces | 1 | 1 | `nmrxiv-s200-cosy-jdf` |
| `jeol-jdf` | field | `experiment.acquisition.started_at` | descriptive | 14 | 14 | `nmrxiv-s1243-esinica`, `nmrxiv-s200-13c-jdf`, `nmrxiv-s200-cosy-jdf` |
| `jeol-jdf` | field | `experiment.instrument.model` | descriptive | 13 | 13 | `nmrxiv-s1243-esinica`, `nmrxiv-s200-13c-jdf`, `nmrxiv-s200-cosy-jdf` |
| `jeol-jdf` | format_version | `JDF 1.1` | metadata, traces | 2 | 2 | `zenodo5223412-13c-2a-jdf`, `zenodo5223412-1h-3b-jdf` |
| `jeol-jdf` | format_version | `JDF 1.2` | metadata, traces | 12 | 12 | `nmrxiv-s1243-esinica`, `nmrxiv-s200-13c-jdf`, `nmrxiv-s200-cosy-jdf` |
| `jeol-jdf` | instrument | `JNM-ECX400` | descriptive | 2 | 2 | `zenodo5223412-13c-2a-jdf`, `zenodo5223412-1h-3b-jdf` |
| `jeol-jdf` | instrument | `JNM-ECZ400S/L1` | descriptive | 6 | 6 | `nmrxiv-s200-13c-jdf`, `nmrxiv-s200-cosy-jdf`, `nmrxiv-s200-hmbc-jdf` |
| `jeol-jdf` | instrument | `JNM-ECZ500R/M1` | descriptive | 3 | 3 | `nmrxiv-s1243-esinica`, `nmrxiv-s908-aihe0`, `nmrxiv-s908-zgig30` |
| `jeol-jdf` | instrument | `NM-70020R4S1` | descriptive | 1 | 1 | `zenodo15045202-gk-ia-mbba-proton-ft-jdf` |
| `jeol-jdf` | instrument | `NM-70050G4` | descriptive | 1 | 1 | `zenodo15473381-o-1-1-200-scans-proton-jdf` |
| `jeol-jdf` | layout | `one_d` | traces | 11 | 11 | `nmrxiv-s1243-esinica`, `nmrxiv-s200-13c-jdf`, `nmrxiv-s200-qhnmr-jdf` |
| `jeol-jdf` | layout | `two_d` | traces | 3 | 3 | `nmrxiv-s200-cosy-jdf`, `nmrxiv-s200-hmbc-jdf`, `nmrxiv-s200-hsqc-jdf` |
| `jeol-jdf` | record | `processed_spectrum` | traces | 3 | 3 | `nmrxiv-s908-aihe0`, `nmrxiv-s908-zgig30`, `zenodo15045202-gk-ia-mbba-proton-ft-jdf` |
| `jeol-jdf` | record | `time_domain` | traces | 11 | 11 | `nmrxiv-s1243-esinica`, `nmrxiv-s200-13c-jdf`, `nmrxiv-s200-cosy-jdf` |
| `jeol-jdf` | sample_layout | `axis complex` | traces | 13 | 13 | `nmrxiv-s1243-esinica`, `nmrxiv-s200-13c-jdf`, `nmrxiv-s200-hmbc-jdf` |
| `jeol-jdf` | sample_layout | `axis real_complex` | traces | 1 | 1 | `nmrxiv-s200-cosy-jdf` |
| `jeol-jdf` | sample_layout | `float64` | traces | 14 | 14 | `nmrxiv-s1243-esinica`, `nmrxiv-s200-13c-jdf`, `nmrxiv-s200-cosy-jdf` |
| `jeol-jdf` | writer_version | `Delta 4` | descriptive | 2 | 2 | `zenodo5223412-13c-2a-jdf`, `zenodo5223412-1h-3b-jdf` |
| `jeol-jdf` | writer_version | `Delta 5` | descriptive | 6 | 6 | `nmrxiv-s1243-esinica`, `nmrxiv-s200-13c-jdf`, `nmrxiv-s200-cosy-jdf` |
| `jeol-jdf` | writer_version | `Delta 6` | descriptive | 6 | 6 | `nmrxiv-s908-aihe0`, `nmrxiv-s908-zgig30`, `zenodo10621204-compound3-1h-jdf` |
| `magritek-spinsolve` | field | `experiment.acquisition.started_at` | descriptive | 26 | 26 | `spinsolve-zenodo15131439-20dec-s1-241220-102617`, `spinsolve-zenodo15131439-20dec-s1-241220-104521`, `spinsolve-zenodo15131439-20dec-s2-241220-112656` |
| `magritek-spinsolve` | field | `experiment.instrument.model` | descriptive | 24 | 24 | `spinsolve-zenodo15131439-20dec-s1-241220-102617`, `spinsolve-zenodo15131439-20dec-s1-241220-104521`, `spinsolve-zenodo15131439-20dec-s2-241220-112656` |
| `magritek-spinsolve` | instrument | `C43` | descriptive | 10 | 10 | `spinsolve-zenodo15131439-20dec-s1-241220-102617`, `spinsolve-zenodo15131439-20dec-s1-241220-104521`, `spinsolve-zenodo15131439-20dec-s2-241220-112656` |
| `magritek-spinsolve` | instrument | `C60Ultra` | descriptive | 10 | 10 | `spinsolve-zenodo20597567-250606-151948-slic`, `spinsolve-zenodo20597567-250610-125455-t1`, `spinsolve-zenodo20597567-250701-174641-slic` |
| `magritek-spinsolve` | instrument | `C80Ultra` | descriptive | 3 | 3 | `spinsolveproc-proton`, `spinsolveproc-t1`, `spinsolveproc-t2` |
| `magritek-spinsolve` | instrument | `P60Grad` | descriptive | 1 | 1 | `spinsolveproc-pgste` |
| `magritek-spinsolve` | layout | `1d` | traces | 14 | 14 | `spinsolve-zenodo15131439-20dec-s1-241220-102617`, `spinsolve-zenodo15131439-20dec-s1-241220-104521`, `spinsolve-zenodo15131439-20dec-s2-241220-112656` |
| `magritek-spinsolve` | layout | `2d` | traces | 13 | 13 | `spinsolve-zenodo20597567-250606-151948-slic`, `spinsolve-zenodo20597567-250610-125455-t1`, `spinsolve-zenodo20597567-250701-174641-slic` |
| `magritek-spinsolve` | record | `spectrum` | traces | 1 | 1 | `spinsolveproc-pgste` |
| `magritek-spinsolve` | record | `time_domain` | traces | 24 | 24 | `spinsolve-zenodo15131439-20dec-s1-241220-102617`, `spinsolve-zenodo15131439-20dec-s1-241220-104521`, `spinsolve-zenodo15131439-20dec-s2-241220-112656` |
| `magritek-spinsolve` | sample_layout | `data type 501` | traces | 24 | 24 | `spinsolve-zenodo15131439-20dec-s1-241220-102617`, `spinsolve-zenodo15131439-20dec-s1-241220-104521`, `spinsolve-zenodo15131439-20dec-s2-241220-112656` |
| `magritek-spinsolve` | sample_layout | `data type 503` | traces | 1 | 1 | `spinsolveproc-pgste` |
| `magritek-spinsolve` | sample_layout | `data type 504` | traces | 3 | 3 | `spinsolveproc-pgste`, `spinsolveproc-proton`, `spinsolveproc-t2bulk` |
| `magritek-spinsolve` | writer_version | `Spinsolve 1.41` | descriptive | 3 | 3 | `spinsolveproc-proton`, `spinsolveproc-t1`, `spinsolveproc-t2` |

… 17 more values: the generated table in `src/assurance.rs` has all of them.

### Tests, fixtures, fuzz targets, snapshots

- integration tests: [`tests/fuzz_regressions.rs`](tests/fuzz_regressions.rs), [`tests/jcamp_export.rs`](tests/jcamp_export.rs), [`tests/spinsolve.rs`](tests/spinsolve.rs), [`tests/synthetic.rs`](tests/synthetic.rs), [`tests/varian_jeol.rs`](tests/varian_jeol.rs)
- committed fixtures: 10 files in [`tests/fixtures/`](tests/fixtures) (malformed ones are replayed through every reader by `openreadout`'s `tests/fuzz_regressions.rs`; all are snapshotted by its `tests/golden.rs`)
- fuzz targets (`fuzz/fuzz_targets/`): `jcamp_asdf`, `whole_bruker`, `whole_jcamp`, `whole_jeol`, `whole_spinsolve`, `whole_varian`
- corpus inputs by tier: heldout 21, hold 11, smoke 31, standard 82
- golden snapshots: [`corpus/snapshots/bruker-nmr.jsonl`](../../corpus/snapshots/bruker-nmr.jsonl), [`corpus/snapshots/jcamp-dx.jsonl`](../../corpus/snapshots/jcamp-dx.jsonl), [`corpus/snapshots/varian-nmr.jsonl`](../../corpus/snapshots/varian-nmr.jsonl), [`corpus/snapshots/jeol-jdf.jsonl`](../../corpus/snapshots/jeol-jdf.jsonl), [`corpus/snapshots/magritek-spinsolve.jsonl`](../../corpus/snapshots/magritek-spinsolve.jsonl)

### Open new-variant intakes

None.
<!-- END GENERATED guide -->
