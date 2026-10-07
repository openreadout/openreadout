# Provenance log — mzML and mzXML

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

mzML is an open standard of the HUPO Proteomics Standards Initiative (PSI); mzXML is the older open format of the Institute for Systems Biology (ISB, Seattle Proteome Center). Both are published XML schemas with free specifications, so the structure comes from the standards and most normalized fields carry `Source::Spec`.

## 2026-09-22 — initial derivation (Richard Zimring with Claude as assistant)

**Specifications consulted** (public):
- HUPO-PSI mzML 1.1.0 specification document and XML schemas `mzML1.1.0.xsd` / `mzML1.1.2_idx.xsd` (https://www.psidev.info/mzML, https://github.com/HUPO-PSI/mzML; the PSI publishes its standards for free implementation). Used for: element structure (`mzML`, `cvList`, `fileDescription`, `referenceableParamGroupList`, `softwareList`, `instrumentConfigurationList`, `dataProcessingList`, `run`, `spectrumList`, `spectrum`, `scanList`/`scan`/`scanWindowList`, `precursorList`/`precursor`/`isolationWindow`/`selectedIonList`/`activation`, `binaryDataArrayList`/`binaryDataArray`/`binary`, `chromatogramList`/`chromatogram`), the `count` and `defaultArrayLength` attributes, `referenceableParamGroupRef`, and the `indexedmzML` wrapper (`indexList`/`index name="spectrum|chromatogram"`/`offset idRef=…`, `indexListOffset`, `fileChecksum`, SHA-1 of the file up to and including `<fileChecksum>`). Byte offsets in the index point at the `<` of the element (checked against every corpus file).
- PSI-MS controlled vocabulary `psi-ms.obo` data-version 4.2.2 (https://github.com/HUPO-PSI/psi-ms-CV, CC BY 4.0). Every accession the reader interprets was looked up there by id and name: binary data types (MS:1000519/1000521/1000522/1000523), compression types (MS:1000574 zlib, MS:1000576 none, MS:1002312–1002314 and MS:1002746–1002748 MS-Numpress, MS:1003780 zstd, MS:1003088 truncation + zlib; MS:1003089/1003090/1003781/1003782 and the other children of MS:1000572 are recognised and reported as unsupported), array types (MS:1000514 m/z, MS:1000515 intensity, MS:1000595 time, MS:1003006/1003008 inverse reduced ion mobility, MS:1000786 non-standard), spectrum terms (MS:1000511 ms level, MS:1000127/1000128 centroid/profile, MS:1000129/1000130 polarity, MS:1000285 TIC, MS:1000504/1000505 base peak, MS:1000527/1000528 highest/lowest observed m/z), scan terms (MS:1000016 scan start time with UO:0000010 second / UO:0000031 minute, MS:1000512 filter string, MS:1000501/1000500 scan window, MS:1002815 inverse reduced ion mobility), precursor terms (MS:1000744 selected ion m/z, MS:1000041 charge state, MS:1000042 peak intensity, MS:1000827/1000828/1000829 isolation window target/offsets, MS:1000045 collision energy, the dissociation methods under MS:1000044), chromatogram types (MS:1000235 TIC, MS:1000628 base peak), and instrument component terms (source/analyzer/detector are taken by element, the term names are copied verbatim).
- MS-Numpress: Teleman J. et al., "Numerical compression schemes for proteomics mass spectrometry data", Mol. Cell. Proteomics 13(6):1537–1542 (2014), open access; reference implementation `MSNumpress.cpp` (https://github.com/ms-numpress/ms-numpress, dual Apache-2.0 / BSD-3-Clause, read as documentation). Used for: the linear-prediction layout (8-byte big-endian fixed point, two 4-byte little-endian first values, then half-byte-packed residuals against `2·x[i-1] − x[i-2]`), the positive-integer layout (half-byte packed), the short-logged-float layout (8-byte fixed point, then `u16` little-endian values of `ln(x+1)·fp`), and the half-byte integer code (first nibble = count of leading zero nibbles, 9–15 = leading `0xF` nibbles for negative values). Our decoder is written from that description with bounds checks; no code copied.
- mzXML 3.2 schema `mzXML_idx_3.2.xsd` and the ISB/SPC mzXML documentation (http://tools.proteomecenter.org/wiki/index.php?title=Formats:mzXML; also mirrored with the TPP). Used for: `msRun`, `parentFile`, `msInstrument` (`msManufacturer`/`msModel`/`msIonisation`/`msMassAnalyzer`/`msDetector` `category`/`value`), `dataProcessing`, `scan` attributes (`num`, `msLevel`, `peaksCount`, `polarity`, `scanType`, `filterLine`, `retentionTime` as `xs:duration`, `lowMz`/`highMz`, `startMz`/`endMz`, `basePeakMz`/`basePeakIntensity`, `totIonCurrent`, `centroided`, `collisionEnergy`), `precursorMz` (`precursorScanNum`, `precursorIntensity`, `precursorCharge`, `activationMethod`, `windowWideness`), `peaks` (`precision` 32/64, `byteOrder` network, `pairOrder`/`contentType` `m/z-int`, `compressionType` none/zlib, `compressedLen`), nested scans (MS2 inside MS1 in 2.x writers) and the `index name="scan"` / `indexOffset` trailer. mzXML 2.x files (`test.mzXML`) differ only in the namespace.

**Prior art consulted** (permissively licensed; documentation and source read, no code copied):
- `pyteomics` 4.x, Apache-2.0, https://github.com/levitsky/pyteomics — `mzml.py`/`mzxml.py`/`auxiliary`: how `referenceableParamGroupRef` is expanded into the element, that mzXML retention times are `PT…S` durations, and that ProteoWizard writes `msInstrumentID` as text. Also run as the black-box oracle (`oracle/gen.py`).

**Corpus files used:** the depositor mzML/mzXML exports already in `corpus/manifest.toml` as `role = "oracle-export"` of the Thermo RAW entries (`mtbls404-Blanc04`, `mtbls404-Blanc04_b2`, `mtbls404-QC1_001`, `mtbls404-HU_neg_017`, `mtbls20-caffeine-pos`, `mtbls20-hydroxymethoxycinnamic-neg`, `mtbls805-msms-869`, `mtbls805-imaging-732`, `mtbls755-hilic-dpoly`, `mtbls797-dotsha05`: MetaboLights, EMBL-EBI terms; `pxd000001-tmt-erwinia-01` mzXML: PRIDE, EMBL-EBI terms), plus the small test files of pyteomics (`pyteomics-test`, `pyteomics-tiny-pwiz`, `pyteomics-test-mzxml`; Apache-2.0) and mzdata (`mzdata-small`, `mzdata-three-test-scans`, `mzdata-diapasef`; Apache-2.0), and a numpress-encoded conversion made locally (see below).

**Observed in the corpus and handled:**
- `defaultArrayLength` is the point count; `encodedLength` equals the base64 character count in every corpus file (`check` compares them).
- Empty arrays are written as `<binary/>` (`pyteomics-tiny-pwiz`).
- `scan start time` is in minutes (`unitName="minute"`) in every Thermo-derived file and in seconds in the timsTOF conversion (`mzdata-diapasef`); both are converted to seconds.
- Native ids seen: `controllerType=0 controllerNumber=1 scan=N` (Thermo conversions), `scan=N` (`pyteomics-tiny-pwiz`), `merged=0 frame=1 scanStart=1 scanEnd=927` (timsTOF conversion), `Scan=N` (imzML examples). The scan number is the first `scan=N` when present, else the zero-based index + 1 (the rule the pyteomics-based oracle uses).
- `referenceableParamGroupRef` inside spectra and binary arrays (`pyteomics-tiny-pwiz`: `CommonMS1SpectrumParams`, `CommonMS2SpectrumParams`), resolved before interpretation.
- A cut-down file whose `indexListOffset` points past its end (`pyteomics-test`, `pyteomics-test-mzxml`): the reader falls back to scanning; `check` reports `index_mismatch`.
- Index offsets are byte positions of `<spectrum`; an edited file whose offsets no longer land on `<spectrum id="…">` (`synthetic-mzml-stale-index`) falls back to a linear scan.
- mzXML `peaks` are big-endian ("network") interleaved (m/z, intensity) pairs; `dataProcessing centroided="1"` with no per-scan attribute (`pyteomics-test-mzxml`).
- Non-standard arrays in chromatograms (`ms level`, `mzdata-small`, `mzdata-diapasef`) and mean inverse reduced ion mobility arrays in spectra (`mzdata-diapasef`).
- SRM runs exported as chromatograms only (`mtbls1822-tsq-74`, 110 chromatograms, no spectra).

**Inferred, not yet corroborated:** zstd (MS:1003780, no corpus file uses it; decoding is plain zstd frames by the CV definition) and `truncation and zlib compression` (MS:1003088: the mantissa truncation is lossy at write time, so the stored values are ordinary floats after zlib; no corpus file).

## 2026-09-22 — imzML and single-byte encodings (Richard Zimring with Claude as assistant)

**Specification consulted:** the imzML controlled vocabulary `imagingMS.obo` data-version 1.1.0 (https://github.com/imzML/imzML, published by the imzML consortium for free implementation) for IMS:1000030/1000031 (continuous/processed), IMS:1000042–1000047 (pixel counts, dimensions, pixel size), IMS:1000050–1000052 (position x/y/z), IMS:1000080 (UUID), IMS:1000090/1000091 (ibd MD5/SHA-1) and IMS:1000101–1000104 (external data, offset, array length, encoded length); the imzML description at https://ms-imaging.org/imzml/ (the `.ibd` starts with the 16-byte UUID; arrays are addressed by the external terms).

**Prior art:** `pyimzML` (Apache-2.0, https://github.com/alexandrovteam/pyimzML) — run as the black-box oracle (`oracle/gen.py imzml_`).

**Corpus files:** `imzml-example-continuous`, `imzml-example-processed` (+ `.ibd` companions): the imzML example datasets as shipped in pyimzML's test data (Apache-2.0), 3×3 pixels each; all 18 pixel spectra and their coordinates match pyimzML bit-exact.

**Observed and handled:** the example files declare `encoding="ISO-8859-1"` and contain Latin-1 bytes in contact fields; quick-xml 0.42 only reads UTF-8, so documents declaring a single-byte encoding are read with bytes above 0x7F replaced by `?` (offsets unchanged). Spectrum ids are `Scan=N` (capital S): no `scan=` match, so scan numbers are index + 1. `ms level` is written as `0` next to *MS1 spectrum*; the reader takes the MS1 term.

## 2026-09-22 — provenance keys follow the file

Corpus: imzml-example-continuous, imzml-example-processed, mzdata-small. No new prior art. The metadata conformance walk showed that the imzML examples fill neither `spectra[0].rt_range_s` (no scan start times in imaging runs) nor `traces[]` (no chromatogramList). The provenance map now claims `rt_range_s` only when a spectrum carries a scan start time (or, before the spectra have been summarized, when the file is not an imaging run), and the trace keys only when the file has chromatograms. No parsing changed.

## 2026-09-22 — experiment facts from `sampleList`

Corpus: pyteomics-tiny-pwiz (`<sample id="_x0032_0090101_x0020_-_x0020_Sample_x0020_1" name="Sample 1">`), mzdata-three-test-scans (`name="sample_1"`), imzml-example-continuous/-processed (`name="Sample1"`), mzdata-small and the synthetic fixtures (`name=""`, `id="_x0031_"`), the depositor exports of the Thermo entries (no `sampleList`). Spec: mzML 1.1.0 `SampleType` (`@id`, `@name`), https://www.psidev.info/mzML (open standard). Inferred: `_xHHHH_` in ids is the XML-name escape ProteoWizard writes for characters an `xs:ID` may not start with or contain; decoded for the sample id. `crates/openreadout-mzml/src/experiment.rs`; no parsing changed (the header nodes were already kept).

## 2026-09-23 — headers-only check; member files

Corpus: every mzML, mzXML and imzML file (`cargo test -p openreadout-corpus-tests --features corpus --test index`). No new prior art and no new parsing: `check_headers` (used by `check --headers-only` and `openreadout index`) runs the same checks as `check` minus the ones that read beyond the header: the indexList offset verification, the whole-file scan for `<spectrum>`/`<scan>` elements, the SHA-1 checksum (mzML `fileChecksum`, imzML `.ibd` SHA-1) and the decoding of every binary array. Completeness (`</mzML>`), the index problem found at open, the declared counts and the `.ibd` UUID are still checked. `member_files` lists the imzML `.ibd`.

## 2026-09-24 — empty text is omitted (Richard Zimring with Claude as assistant)

**Scope:** normalization only (book/src/guides/metadata.md § General rules, "Empty text is missing"; held-out finding L2). No change to how files are walked or decoded. **Corpus files used:** `pyteomics-test` (its `<mzML version="">` was the one empty string `info` reported across the development corpus). mzML `version`, mzXML `xmlns` version and the mzXML `msInstrument` values (`msManufacturer`, `msModel`, `msDetector`, software name and version) that are empty or blank are now omitted instead of reported as `""`. No prior art consulted.

## 2026-09-23 — gzip containers (`.mzML.gz`, `.mzXML.gz`) and mzMLb (Richard Zimring with Claude as assistant)

**Specifications consulted** (public):
- RFC 1952 (gzip file format, IETF) and RFC 1951 (DEFLATE): member header (`1F 8B`, method 8, `FLG` bits `FTEXT`/`FHCRC`/`FEXTRA`/`FNAME`/`FCOMMENT`, reserved bits zero, `MTIME`, `XFL`, `OS`, `XLEN`), trailer (CRC-32 and the length modulo 2³² of the member's data, little-endian), concatenated members forming one stream, and DEFLATE's 32 KiB back-reference window (why a restart point needs the last 32 KiB of output).
- mzMLb: Bhamber R. S. et al., "mzMLb: A Future-Proof Raw Mass Spectrometry Data Format Based on Standards-Compliant mzML and Optimized for Speed and Storage Requirements", J. Proteome Res. 20(1):172–183 (2021), open access (PMC7871438). Used for: the XML stored as the byte dataset `mzML` with the string attribute `version` = `mzMLb 1.0`; arrays in numeric datasets named `spectrum_MS_<accession>_<type>` / `chromatogram_MS_<accession>_<type>`, several spectra's arrays concatenated in one dataset and addressed by offset; the index datasets `mzML_spectrumIndex` / `mzML_chromatogramIndex` (int64 byte offsets into `mzML`); compression done by HDF5 filters (deflate, shuffle, optionally Blosc); MS-Numpress output stored as a generic byte array.
- PSI-MS CV `psi-ms.obo` 4.2.2 (CC BY 4.0): MS:1002841 *external HDF5 dataset* ("the HDF5 dataset location containing the binary data ... there is no data in the `<binary>` section"), MS:1002842 *external offset*, MS:1002843 *external array length*; MS:1003088–1003090 (truncation / prediction + zlib, from the mzMLb work) stay recognised-not-decoded.
- HDF5 filter ids 1 (deflate), 2 (shuffle), 3 (Fletcher-32) from the HDF5 documentation; 32001 is the id the Blosc project registered with The HDF Group (https://github.com/HDFGroup/hdf5_plugins/blob/master/docs/RegisteredFilterPlugins.md).

**Prior art consulted** (permissive; read as documentation, no code copied):
- pyteomics `mzmlb.py` (Apache-2.0, https://github.com/levitsky/pyteomics): that the index has one entry more than there are elements (the last is the end of the document) and that `mzML_<kind>Index_idRef` holds the ids separated by NUL bytes; that the dataset name may also be given as *external dataset*; its linear/delta prediction inverses (not implemented here: no independent file uses them). Also run as the mzMLb oracle.
- psims `mzmlb/writer.py` (Apache-2.0): how an independent writer lays out datasets and compression (1 MiB chunks, `compression` file attribute) — run to make the synthetic fixtures.
- miniz_oxide (MIT/Zlib/Apache-2.0) API documentation for resuming inflate at a block boundary (`TINFL_FLAG_STOP_ON_BLOCK_BOUNDARY`, `block_boundary_state`); zlib's `examples/zran.c` idea (restart points holding the window) as described in its comments (zlib licence).

**Corpus files used:** `mzdata-small-gz`, `mzdata-timstof-gz`, `mzdata-small-mzmlb` (mzdata test data, Apache-2.0: small.mzMLb has uncompressed chunked datasets, 64-bit m/z, 32-bit intensity, `external array length` in elements, idRef dataset of NUL-separated ids, 49 offsets for 48 spectra); synthetic fixtures written by psims (`synthetic-mzmlb-zlib`, `-blosc`, `-numpress`) and Python's `gzip` module (`synthetic-mzml-gz-members`, `synthetic-mzxml-gz`, `synthetic-mzml-gz-truncated`), see `oracle/make_mzml_container_fixtures.py`.

**Observed and handled:** psims labels arrays `zlib compression` although the values are plain in the dataset (the deflate is the HDF5 filter), so for mzMLb the general-purpose compression terms are taken to describe the filter; its MS-Numpress arrays are byte datasets whose *external array length* counts bytes (the value count is `defaultArrayLength`); pyteomics returns such arrays undecoded (bytes), so the oracle decodes them with pynumpress. mzdata's writer leaves `encodedLength="0"`; the `encoded_length_mismatch` check does not apply to external arrays. mzdata's writer also emits, for the chromatogram's empty `ms level` array, a `binaryDataArray` with `arrayLength="0"` and an empty *external HDF5 dataset* name (pyteomics/h5py cannot open it); such placeholder arrays are left out. The `mzML` dataset is `int8` in both writers.

## 2026-09-24 — Scan headers without peaks (`spectra`, `Dataset::visit_scan_headers`)

**Scope:** the `<spectrum>` element up to its binary arrays (as `info`'s summaries), interpreted like a decoded spectrum; `defaultArrayLength` is `point_count`. mzXML: the `<scan>` element up to its peaks, `peaksCount`. imzML and mzMLb (arrays outside the element) and `.mzML.gz`/`.mzXML.gz` through the same code.

**Validation:** `crates/openreadout-corpus-tests/tests/scans.rs` compares every oracle scan's header (scan number or native id, MS level, RT, polarity, filter, precursor m/z, charge, activation, isolation target, collision energy, 1/K0, position) with the corpus oracles (pyteomics/pyimzML on the same file: 34 mzML, 4 mzXML, 4 mzMLb, 2 imzML files), and on up to 40 scans per file checks that the header equals the decoded spectrum's metadata.

**Prior art consulted:** none new.

## 2026-09-24 — mzMLb: fuzz finding, a bound only

**Scope:** an allocation bound; nothing the reader infers changed. The `whole_mzmlb` fuzz target (90 s, first run; seed `mzdata-small-mzmlb`, Apache-2.0) found a 43 KB file whose `mzML` dataset declares 2.86 GB of unfiltered storage: `hdf5-pure` allocates a dataset's whole contiguous storage when rows of it are read, before it finds the file too short, so the reader asked for 2.86 GB (the fuzzer's malloc limit turned that into a crash; outside it the read failed afterwards with a clean error). A dataset without filters stores its elements as they are, so `Rows::open` now refuses one whose declared size (elements × element size) exceeds the file, as `corrupt` with the sizes. The rerun found the same allocation through the layout message instead (a 2 GiB contiguous run declared for a small dataset), so `Rows::open` also refuses a contiguous run longer than the file, chunks whose decoded size exceeds the plane limit (`openreadout_core::limits::plane_limit`: 256 × the file size, at least 1 GiB), and a stored chunk that ends past the end of the file (the case of the second fixture: a 52-byte chunk at offset 620,756,992 of a 43 KB file made `hdf5-pure` allocate 2 GiB). A filtered dataset's total decoded size is not bounded: it can legitimately exceed the file. Regression fixtures: `crates/openreadout-mzml/tests/fixtures/malformed/file-fuzz-mzmlb-contiguous-size.mzMLb` and `file-fuzz-mzmlb-layout-size.mzMLb`, replayed through every reader by `crates/openreadout-cli/tests/fuzz_regressions.rs`.

## 2026-09-25 — assurance profile (`src/assurance.rs`)

**Corpus files:** every development input of this format on disk (`corpus/assurance/evidence.json` lists them); no held-out file.
**Prior art consulted:** none.
**What was done.** No parsing logic changed. The reader declares an assurance profile (docs/assurance.md) that fingerprints each file from this reader's own normalized output: the format version, writers, file content, chromatogram types, mzMLb container version, imzML imaging and gzip containers (mzML, imzML, mzXML, mzMLb). The table of validated feature values and the confidence level are generated from the development corpus by `cargo xtask assurance-audit`.

## 2026-09-26 — placeholder file checksums (held-out finding C-L2, reproduced on a development file) (Richard Zimring with Claude as assistant)

**Corpus files:** `openms-tutorial-gaussfilter` (OpenMS code examples, BSD-3-Clause): an indexed mzML whose `<fileChecksum>` holds `0`. A second candidate, mzmine's test file `sciex_no_mz_range_cv.mzML` (MIT; `fileChecksum` of 40 zeros), was not added: its index offsets point into the middle of elements (a hand-edited test file), so `check` rightly reports `index_mismatch`.
**Prior art consulted:** the mzML 1.1 index schema (`mzML1.1.2_idx.xsd`: `fileChecksum` is the SHA-1 of the file up to that element), as before. OpenMS writes `0` when it does not compute the digest (seen in its published example files, run as data only).
**What was inferred:** a SHA-1 digest is 40 hexadecimal digits. A `fileChecksum` that is not (`0`), or that is all zeros, is a placeholder the writer left, not a digest: nothing can be verified from it and it says nothing about damage. `check` now reports it as the warning `checksum_placeholder` instead of the error `checksum_mismatch`; real digests are verified as before.

## 2026-09-26 — an absent retention time stays absent (finding MS-1) (Richard Zimring with Claude as assistant)

**Corpus files:** `pyteomics-tiny-pwiz` (pyteomics test data, Apache-2.0): spectrum `scan=21` has a `<scan>` without `scan start time` (MS:1000016); every other development mzML/mzXML/mzMLb/imzML input (unchanged values). **Prior art consulted:** the mzML 1.1 specification (MS:1000016 is optional on `<scan>`), mzXML 3.2 schema (`retentionTime` is optional), as before; pyopenms (BSD-3) run as the second opinion reports −1 for the spectrum ("unset").
**What was decided.** A spectrum whose file states no retention time was reported at 0.0 s, a value the file does not hold. The core model's `Spectrum.rt_s` and `ScanHeader.rt_s` are now optional: null in JSON when the file states none (`retention_time_missing` in `extra` is kept). Consumers: chromatograms and XICs from spectra leave such scans out and say how many (`notes`); `spectra --rt-range` does not match them; the mzML writer omits `scan start time`; Arrow/Parquet and CSV columns hold null/empty; the run's `rt_range_s` ignores them (as before). No parsing logic of the file changed.

## 2026-10-02 — generic parent terms carrying a value; accessions of components and software (Richard Zimring with Claude as assistant)

**Corpus files:** `pyteomics-tiny-pwiz` (pyteomics test data, Apache-2.0), and its export by `export --to mzml`. **Prior art consulted:** the PSI-MS ontology (`psi-ms.obo`, CC-BY-4.0) for the parent terms MS:1000031 instrument model, MS:1000531 software, MS:1000008 ionization type, MS:1000443 mass analyzer type, MS:1000026 detector type.
**What was inferred.** Some writers (OpenReadout's own mzML writer among them) state a component, software or model for which they have no specific term as its generic parent term with the specific name in `value` (`<cvParam accession="MS:1000531" name="software" value="Xcalibur"/>`), or the parent `instrument model` term with a user param `instrument model name`. Reading the parent term's name gave "software", "detector type" and "instrument model" back. The summary (`instrument.model`, `software`, `detector`, `extra.instrument_configurations[].components[].terms`, `extra.software[].name`) now takes the value of these five parent terms when it is not empty, and the `instrument model name` user param for a value-less `instrument model`. New keys (additions): `extra.instrument_configurations[].components[].accessions` (parallel to `terms`; empty for user params and value-carrying parent terms) and `extra.software[].accession`, so a writer can copy the exact terms. Specific terms read as before.

## 2026-10-06 — imzML: eleven more files from six depositors (no parsing change) (Richard Zimring with Claude as assistant)

**Corpus files:** `zenodo1560646-mouse-kidney-cut` (Zenodo 1560646, MIT), `zenodo2628280-nglycan-control` (Zenodo 2628280, MIT), `zenodo17374882-spheroid-section01` and `-section46` (members of `zenodo17374882-spheroid`, Zenodo 17374882, CC-BY-4.0), `metaspace-untreated-3-434` (metaspace2020/metaspace test data, Apache-2.0), `kineticmsi-hd-rep6` and `kineticmsi-wt-rep1` (MSeidelFed/KineticMSI example data, BSD-2-Clause), `i2nca-pp`, `i2nca-cp`, `i2nca-pc` and `i2nca-cc` (cKNUSPeR/i2nca test data, GPL-3.0), each with its `.ibd`.

**Prior art consulted:** pyimzML 1.5.5 (Apache-2.0, https://github.com/alexandrovteam/pyimzML), run through `oracle/gen.py` as before. No source was read.

**How they were found:** the Zenodo search API (`q=imzml`, all 44 hits) and GitHub code search (`extension:imzML`, 91 hits), keeping licensed records whose `.ibd` is under 200 MB and that hold no patient material.

**What the files add:** writers not seen before (Cardinal 1.12.1, SCiLS Lab 7.02 exporting Bruker solariX data, the pyimzML writer, and a writer that leaves out `softwareList`), centroid spectra in both storage modes, 64-bit m/z with 32-bit intensity, an `ibd MD5` instead of an SHA-1, and instrument scan-pattern terms.

**What was compared:** pyimzML reads every pixel of ten files. The release binary agreed with it on every pixel compared (30 sampled pixels per file, all pixels of the four-pixel i2nca files): the m/z and intensity arrays bit for bit and the pixel positions. The corpus test compares every pixel.

**Found:** `i2nca-cc` declares continuous storage, but its eight spectra have m/z arrays of 1,990 to 1,999 values and intensity arrays of other lengths. pyimzML fails on it, so it has no oracle. `openreadout spectrum --index 1` refuses it (exit 4, "m/z array has 1999 values, intensity array 1997"), while `openreadout check` reports the file OK (exit 0). It is proposed as a malformed-file case, not as validation.

**Inferred:** nothing new. In the Zenodo 2628280 record the imzML and its binary file `control.ibd` have different stems (the imzML's is misspelled). The corpus stores both under one stem so that a reader finds the `.ibd`, as the specification requires.

## 2026-10-06 — `check` compares the m/z and intensity array lengths (Richard Zimring with Claude as assistant)

**Corpus files used:** `i2nca-cc` (GitHub cKNUSPeR/i2nca @517c91e, GPL-3.0), now a `corrupt` entry. **Prior art consulted:** none.

**What was found, and decided:** reading a spectrum already refused one whose m/z and intensity arrays differ in length (exit 4), but `check` decoded each array on its own and reported the file OK. `check` now applies the same rule to every spectrum and reports `bad_array`, so the two agree. Nothing about the layout was inferred.

## 2026-10-06 — imzML: the MS level a file states only in `fileContent` (Richard Zimring with Claude as assistant)

**Corpus files used:** `metaspace-untreated-3-434` (metaspace2020/metaspace test data, Apache-2.0), and for comparison `imzml-example-continuous`, `i2nca-pp`, `kineticmsi-hd-rep6`, `zenodo17374882-spheroid-section01`. **Prior art consulted:** the imzML 1.1 specification (open, imzml.org) for the meaning of `fileContent`.

**What was found:** `metaspace-untreated-3-434` names `MS1 spectrum` only in `fileDescription/fileContent`, not on its spectra, and the reader reported MS level 0 (not stated) for its 4,850 spectra. The files that state the level on each spectrum or in a referenceable group were read as 1. Files that name no spectrum kind at all (`kineticmsi-*`, the spheroid sections) stay 0. The oracle had hard-coded MS level 1 for every imzML pixel, because pyimzML does not report it. It now takes the level from the file's own cvParams with the reader's rule (a non-zero `ms level`, else 1 when `MS1 spectrum` is named, else 0), and the oracles were regenerated.

**Decided:** in an imzML file, a spectrum that states no MS level takes level 1 when `fileContent` names `MS1 spectrum` and no other spectrum kind (centroid and profile describe the representation and are not counted). mzML files are unchanged: their `fileContent` lists every kind in the run.
