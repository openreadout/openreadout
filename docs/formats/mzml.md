# mzML and mzXML

mzML and mzXML are open formats for mass spectrometry data, written by converters such as ProteoWizard msConvert. OpenReadout returns every spectrum with its peak list and scan metadata and every chromatogram as a trace; it also reads imzML imaging data, gzip-compressed files and mzMLb. Derived from the HUPO-PSI mzML 1.1 specification and schemas, the PSI-MS controlled vocabulary (`psi-ms.obo` 4.2.2, CC BY 4.0), the ISB mzXML 2.x/3.x schemas and the MS-Numpress paper, with pyteomics (Apache-2.0) as the reference reader. Provenance: `docs/provenance/mzml.md`.

mzML (format id `mzml`) is XML whose spectra and chromatograms carry controlled-vocabulary terms (`cvParam accession="MS:…"`) and base64 binary arrays. mzXML (format id `mzxml`) is the older ISB format with attributes instead of terms and interleaved m/z-intensity pairs. Files run to many gigabytes, so the reader never loads a file whole.

## Locating spectra

1. **Offset index.** An `indexedmzML` file ends with `<indexListOffset>N</indexListOffset>`; byte `N` starts `<indexList>`, whose `<index name="spectrum">`/`<index name="chromatogram">` hold `<offset idRef="ID">byte</offset>` entries. mzXML: `<indexOffset>N</indexOffset>` → `<index name="scan">` with `<offset id="num">`. The last 64 KiB of the file are searched for the trailer.
2. **Validation at open.** Sixteen entries spread over the list (all of them when fewer) must start with `<spectrum … id="ID">` (`<scan … num="N">`). Any miss, an unreadable index or an offset past the end of the file makes the reader fall back to step 3 and adds a note; `check` then reports `index_mismatch`.
3. **Scan.** One streaming pass (quick-xml) records the byte offset of every `<spectrum>`/`<chromatogram>` start tag (mzXML: every `<scan>`, nested or not). A file that ends before `</mzML>` (`</mzXML>`) is **truncated**: its complete spectra stay readable, `info` says so in `notes`, `check` reports `truncated` (exit 4).
4. **Header.** Elements before the data (`cvList`, `fileDescription`, `referenceableParamGroupList`, `softwareList`, `instrumentConfigurationList`, `dataProcessingList`, …; mzXML `parentFile`, `msInstrument`, `dataProcessing`) are read into element trees for `info` and `info --view full --vendor`.
5. **Metadata-only reads.** `info` reads each spectrum only up to `<binaryDataArrayList>` (mzXML: up to `<peaks>`), with a 4 KiB buffer.

## mzML spectrum → `Spectrum`

`referenceableParamGroupRef` elements are expanded in place before any term is looked up.

| mzML | our field |
| --- | --- |
| position in the file (0-based) | `index` |
| `scan=N` in `spectrum/@id` (first match), else `index + 1` | `scan_number` |
| `spectrum/@id` | `native_id` |
| MS:1000511 *ms level* (MS:1000579 *MS1 spectrum* → 1) | `ms_level` |
| MS:1000127 *centroid spectrum* / MS:1000128 *profile spectrum* | `centroided` |
| MS:1000130 *positive scan* / MS:1000129 *negative scan* (spectrum, else first scan) | `polarity` |
| MS:1000285 *total ion current* | `total_ion_current` |
| MS:1000504 / MS:1000505 *base peak m/z / intensity* | `base_peak_mz`, `base_peak_intensity` |
| MS:1000528 / MS:1000527 *lowest / highest observed m/z* | `extra.observed_mz_range` |
| first `scan`: MS:1000016 *scan start time* × unit (UO:0000010 s, UO:0000031 min, UO:0000028 ms; no unit = minutes) | `rt_s` (null and `extra.retention_time_missing` when absent: never 0) |
| MS:1000512 *filter string* | `scan_filter` |
| MS:1000927 *ion injection time* | `extra.ion_injection_time_ms` |
| MS:1002815 *inverse reduced ion mobility* (scan, else selected ion) | `inverse_reduced_mobility` |
| `scanWindow` MS:1000501 / MS:1000500 | `scan_window_mz` |
| `scan/@instrumentConfigurationRef` | `extra.instrument_configuration` |
| more than one `scan` | `extra.scan_count` |
| last `precursor`: `isolationWindow` MS:1000827 target ± MS:1000828/MS:1000829 offsets | `isolation_window_mz`, `extra.isolation_target_mz` |
| `selectedIon` MS:1000744 *selected ion m/z*, MS:1000041 *charge state*, MS:1000042 *peak intensity* | `precursor_mz`, `precursor_charge`, `precursor_intensity` |
| `activation`: dissociation terms (children of MS:1000044) | `activation` (table below) |
| MS:1000045 *collision energy* (else MS:1000138 *normalized collision energy*) | `collision_energy` |
| `precursor/@spectrumRef` | `extra.precursor_spectrum_ref` |
| `binaryDataArray` MS:1000514 *m/z array* / MS:1000515 *intensity array* | `mz` (f64), `intensity` (f32) |
| any other array | named in `extra.other_arrays` |

**Activation names:** MS:1000133, MS:1000433, MS:1002472 → `CID`; MS:1000422, MS:1002481 → `HCD`; MS:1000598 `ETD`; MS:1000250 `ECD`; MS:1000262 `IRMPD`; MS:1000435 `PD`; MS:1003246 `UVPD`; MS:1000599 `PQD`; MS:1000136 `SID`; MS:1003247 `NETD`; MS:1003294 `EAD`; MS:1000282 `SORI`; MS:1000242 `BIRD`; MS:1001880 `ISCID`; MS:1002000 `LIFT`; MS:1000134 `PLASMA-DESORPTION`; MS:1000135 `PSD`; ETD with MS:1002678 (supplemental beam-type) → `ETHCD`, with MS:1002679 (supplemental CID) → `ETCID`; several → joined with `+`.

## Binary arrays

`binary` text is base64 (RFC 4648; whitespace ignored). Then, by the compression term:

| accession | name | handling |
| --- | --- | --- |
| MS:1000576 | no compression | as stored |
| MS:1000574 | zlib compression | inflate |
| MS:1003088 | truncation and zlib compression | inflate (truncation was lossy at write time) |
| MS:1003780 | zstd compression | zstd frame |
| MS:1002312 / 1002313 / 1002314 | MS-Numpress linear / positive integer / short logged float | numpress decode (below) |
| MS:1002746 / 1002747 / 1002748 | … followed by zlib | inflate, then numpress |
| MS:1003783 / 1003784 / 1003785 | … followed by zstd | zstd, then numpress |
| MS:1003089, MS:1003090, MS:1003781, MS:1003782, MS:1003826 | prediction/byte-shuffle/dictionary/grid encodings | recognised, not decoded: exit 6 |

Value types: MS:1000521 32-bit float, MS:1000523 64-bit float, MS:1000519 32-bit integer, MS:1000522 64-bit integer; mzML is little-endian. Numpress arrays decode to 64-bit floats whatever type term they carry. The decoded length must equal `binaryDataArray/@arrayLength` or else `spectrum/@defaultArrayLength` (mismatch = corrupt, exit 4); `check` also compares the base64 character count with `@encodedLength` (warning `encoded_length_mismatch`).

**MS-Numpress** (Teleman et al. 2014): *linear* — 8-byte big-endian IEEE-754 fixed point `fp`, two 4-byte little-endian unsigned first values, then half-byte-packed residuals `r` with `x[i] = 2·x[i−1] − x[i−2] + r`, value = `x/fp`; *positive integer* — half-byte-packed unsigned integers; *short logged float* — `fp`, then `u16` little-endian `y`, value = `exp(y/fp) − 1`. A packed integer is a head nibble `n` (0–8: `n` leading zero nibbles; 9–15: `n−8` leading `0xF` nibbles) followed by the remaining nibbles, least significant first; a final lone zero nibble is padding.

## Chromatograms → traces

Each `chromatogram` is a trace with one sweep: `name` = `@id`, `sample_count` = `@defaultArrayLength`, `sample_rate_hz` = 0 (irregular sampling, `extra.irregular_sampling`), channels `time` (seconds, converted from the time array's unit; no unit = minutes), `intensity` (unit = the array's `unitName`), then any other arrays by name (e.g. `ms level`). `extra.chromatogram_type` / `_accession` name the type term (MS:1000235 TIC, MS:1000628 BPC, …); SRM chromatograms add `extra.precursor_mz`/`product_mz`. `export --format csv` writes them.

## mzXML scan → `Spectrum`

| mzXML | our field |
| --- | --- |
| `scan/@num` | `scan_number`, `native_id` (`scan=N`) |
| `@msLevel` | `ms_level` |
| `@polarity` `+`/`-` | `polarity` |
| `@centroided` (absent: `dataProcessing/@centroided`) | `centroided` |
| `@retentionTime` (`xs:duration`, `PT…S`, `PT…M…S`, `P…DT…H`) | `rt_s` |
| `@filterLine` | `scan_filter` |
| `@totIonCurrent`, `@basePeakMz`, `@basePeakIntensity` | `total_ion_current`, `base_peak_mz`, `base_peak_intensity` |
| `@startMz`/`@endMz` | `scan_window_mz` |
| `@lowMz`/`@highMz` | `extra.observed_mz_range` |
| `@collisionEnergy` | `collision_energy` |
| `@scanType`, `@msInstrumentID`, `@ionisationEnergy`, `@cidGasPressure` | `extra.scan_type`, `instrument_id`, `ionisation_energy`, `cid_gas_pressure` |
| last `precursorMz` text, `@precursorCharge`, `@precursorIntensity`, `@activationMethod` (upper-cased), `@windowWideness` (m/z ± w/2), `@precursorScanNum` | `precursor_mz`, `precursor_charge`, `precursor_intensity`, `activation`, `isolation_window_mz`, `extra.precursor_scan` |
| `peaks` (`@precision` 32/64, `@byteOrder` network = big-endian, `@compressionType` none/zlib, `@contentType`/`@pairOrder` m/z-int) | `mz`, `intensity` |

A nested `scan` (MS2 inside its MS1 in 2.x files) ends the parent's own content. The pair count must equal `@peaksCount`.

## Run summary (`info` → `spectra[0]`)

`scan_count`, `ms_levels`, `rt_range_s` from every spectrum's metadata; `instrument.model` from the default instrument configuration's model term (mzXML: `msModel`), `instrument.software`/`software_version` from its `softwareRef`, `instrument.detector` from the detector component terms (mzXML `msManufacturer`, `msDetector`). A generic parent term written with the specific name in `value` (MS:1000031 instrument model, MS:1000531 software, MS:1000008 ionization type, MS:1000443 mass analyzer type, MS:1000026 detector type) reads as that value; a value-less `instrument model` as the user param `instrument model name` when there is one. `instrument_configurations[].components[]` list `terms` and, parallel to them, `accessions` (null for user params and value-carrying parent terms); `software[]` entries carry the term's `accession`. `extra` keys (mzML): `run_id`, `acquired_at` (`run/@startTimeStamp`; a note is added when it has no zone), `declared_spectrum_count`, `ms_level_counts`, `polarities`, `centroid_spectra`, `profile_spectra`, `total_points`, `chromatogram_count`, `indexed`, `located_by`, `file_content`, `source_files`, `software`, `instrument_configurations`, `instrument_serial`, `data_processing`; mzXML adds `declared_scan_count`, `start_time_s`, `end_time_s`, `ionisation`, `mass_analyzer`.

## Experiment facts (`Dataset::experiment`)

`sample.id` of the experiment ([experiment model](../../book/src/guides/metadata.md)) comes from the header's `sampleList/sample[0]`: `@name`, else `@id`, with the XML-id escapes `_xHHHH_` decoded (`_x0032_0090101_x0020_-_x0020_Sample_x0020_1` → `20090101 - Sample 1`); `source_field` is `sampleList/sample[0]/@name` (or `@id`), origin `spec`. A note says so when the list names several samples. Everything else in the experiment is derived from `info` (instrument model, polarities, MS levels, ion source and analyzer terms from `instrument_configurations`).

## imzML (mass spectrometry imaging)

Format id `imzml`, extension `.imzML`: an mzML document (usually not indexed) whose arrays live in a binary file with the same stem and extension `.ibd`. Everything above applies, plus the imzML controlled vocabulary (`imagingMS.obo` 1.1.0, https://github.com/imzML/imzML):

| term | use |
| --- | --- |
| IMS:1000101 *external data*, IMS:1000102 *external offset*, IMS:1000103 *external array length*, IMS:1000104 *external encoded length* | the array's bytes are `.ibd[offset .. offset + encoded length]`, holding *array length* values of the declared type and compression; `defaultArrayLength` is 0 |
| IMS:1000030 *continuous* / IMS:1000031 *processed* (fileContent) | `extra.imaging.storage` (continuous: every pixel points at one shared m/z array) |
| IMS:1000080 *universally unique identifier* | `extra.imaging.uuid`; `check` compares it with the first 16 bytes of the `.ibd` (`uuid_mismatch`) |
| IMS:1000091 *ibd SHA-1* | `check` hashes the `.ibd` (`checksum_mismatch`); IMS:1000090 *ibd MD5* is not verified |
| IMS:1000042/1000043 *max count of pixels x/y*, IMS:1000044/1000045 *max dimension x/y*, IMS:1000046/1000047 *pixel size x/y* (scanSettings) | `extra.imaging.pixels_x`, `pixels_y`, `dimension_um`, `pixel_size_um` |
| other IMS:10004xx value-less terms in scanSettings (scan pattern, direction, type) | `extra.imaging.scan_pattern` (names) |
| IMS:1000050/1000051/1000052 *position x/y/z* (scan) | spectrum `extra.position` `[x, y(, z)]` |

Each pixel is one spectrum, in file order. **Encoding:** quick-xml reads UTF-8 only; a document that declares a single-byte encoding (ISO-8859-1, windows-1252 — the imzML example files) is read with every byte above 0x7F replaced by `?`, which keeps byte offsets exact but loses non-ASCII characters in free text (contact names and addresses).

## Containers: gzip (`.mzML.gz`, `.mzXML.gz`) and mzMLb

Public repositories (PRIDE, MetaboLights) often serve mzML and mzXML gzip-compressed, and mzMLb puts mzML into HDF5. Both are read by the same mzML/mzXML reader through a byte view of the document inside; `info`, `spectra`, `trace`, `export` and `check` work as on the plain file.

**gzip** (RFC 1952; format id stays `mzml`/`mzxml`). Detected from the `1F 8B 08` signature plus the decompressed head's root element (`.mzML.gz`/`.mzXML.gz` names alone give an extension-only match). Opening decompresses the whole file once (`openreadout_core::gzip::GzipSource`), verifying every member's CRC-32 and length, and keeps **restart points**: at DEFLATE block boundaries about every MiB of output, the decoder's leftover bits and the last 32 KiB of output (at most 1,024 points, so at most 32 MiB; the spacing doubles on longer files). A read restarts at the nearest point, or continues one of four live decoders when that is cheaper, so reading front to back costs one decompression and a random spectrum costs at most one spacing of decompression. No temporary file is written. The trade-off: `info` on an indexed file decompresses the file twice (the pass at open, then the per-spectrum metadata pass in file order) where the plain file needs neither; measured on a 67 MB `.mzML.gz` (122 MB of mzML, 8,371 spectra): 0.75 s CPU and 27 MB peak memory, against 0.19 s for the plain file. Concatenated members (block gzip) are one stream; zero padding after the last member is ignored, other trailing bytes are a warning. A truncated or damaged file opens with its clean prefix: the mzML reader then reports the document as truncated, and `check` adds `gzip_truncated` / `gzip_corrupt` (errors) or `gzip_trailing_data` (warning). Byte offsets in the output refer to the decompressed document.

| gzip fact | where |
| --- | --- |
| members, compressed and decompressed sizes, restart points, `FNAME` of the first member, the problem when there is one | `spectra[].extra.compression` (and each trace's `extra.compression`, `info --view full` → `vendor.compression`): `container` = `gzip`, `members`, `compressed_bytes`, `decompressed_bytes`, `restart_points`, `original_name`, `problem` |
| file size | `size_bytes` is the compressed size |

**mzMLb** (format id `mzmlb`, extension `.mzMLb`; Bhamber et al., J. Proteome Res. 2021). An HDF5 file (read with `hdf5-pure`) holding:

| HDF5 object | meaning |
| --- | --- |
| dataset `mzML` (int8/uint8), attribute `version` (`mzMLb 1.0`) | the mzML document; every `<binary>` is empty |
| `mzML_spectrumIndex`, `mzML_chromatogramIndex` (int64) | byte offset of each `<spectrum>`/`<chromatogram>` in `mzML`, plus one final entry (the end) |
| `mzML_spectrumIndex_idRef`, `mzML_chromatogramIndex_idRef` (bytes) | the element ids, NUL-separated |
| `spectrum_MS_<accession>_<type>`, `chromatogram_MS_<accession>_<type>` (1-D numeric) | array values, many spectra concatenated |

A `binaryDataArray` points into them with MS:1002841 *external HDF5 dataset* (name), MS:1002842 *external offset* (first element) and MS:1002843 *external array length* (elements). Values are read as the dataset's stored type (8- to 64-bit integers, 32/64-bit floats) and returned as `mz` f64 / `intensity` f32 as for mzML. Compression is the dataset's HDF5 filter pipeline: deflate, shuffle and Fletcher-32 through `hdf5-pure`; Blosc (filter 32001; blosclz, LZ4, zlib, zstd inside) and LZ4 (32004) chunk by chunk with `openreadout-codecs`. The `zlib compression` term writers put on such arrays describes the filter, so general-purpose compression terms are not applied again. MS-Numpress arrays are byte datasets whose *external array length* counts bytes; they are decoded as in mzML and their value count checked against `defaultArrayLength`. The index datasets replace the `indexList` trailer (spot-checked at open, all verified by `check`); without them the XML is scanned. `info` adds `spectra[].extra.container`: `container` = `hdf5`, `mzmlb_version`, `xml_bytes`, `array_datasets`, `xml_filters`, `index`.

## `check`

`gzip_truncated`, `gzip_corrupt`, `gzip_trailing_data` (containers, see above), `truncated`, `index_mismatch` (unreadable index, entries not at their element, entry count ≠ elements in the file), `checksum_mismatch` (SHA-1 of the file up to `<fileChecksum>`), `checksum_placeholder` (warning: `<fileChecksum>` holds no SHA-1 digest — OpenMS writes `0`, other writers 40 zeros — so nothing can be verified from it), `count_mismatch` (`spectrumList/@count`, `chromatogramList/@count`, mzXML `msRun/@scanCount`), `bad_spectrum`/`bad_array`/`bad_scan`/`bad_chromatogram` (every array decoded), `encoded_length_mismatch` (warning), `unsupported_encoding` (warning), `no_index` (info).

## Vocabulary

| identifier | meaning |
| --- | --- |
| `MzmlReader`, `ImzmlReader`, `MzxmlReader` | the three `FormatReader`s |
| `MzmlDataset`, `MzxmlDataset` | an open file; `open(path)` (`open_imzml(path)` for imzML with its `.ibd`), `spectrum(i)` decodes spectrum `i`, `visit_headers(first, visit)` walks the spectrum headers from `first` on without decoding arrays (`scans`) |
| `MZML_FORMAT_ID`, `IMZML_FORMAT_ID`, `MZXML_FORMAT_ID` | `mzml`, `imzml`, `mzxml` |
| `ArrayEncoding` | how one array is stored: `value_type`, `compression`, `big_endian` (mzXML network order) |
| `ValueType` (`Float32`, `Float64`, `Int32`, `Int64`), `width()` | stored numeric type and its byte width |
| `Compression` (`NoCompression`, `Zlib`, `Zstd`, `NumpressLinear`, `NumpressPic`, `NumpressSlof`, `Unsupported`) | compression term |
| `Outer` (`Plain`, `Zlib`, `Zstd`) | compressor applied after MS-Numpress |
| `ArrayError` (`Base64`, `Decompress`, `Layout`, `Unsupported`) | why an array did not decode |
| `NumpressError` | why a numpress payload did not decode |
| `decode_base64`, `decode_values`, `decode_array` | base64 → bytes (+ character count), bytes → values, both |
| `decode_linear`, `decode_pic`, `decode_slof` | MS-Numpress decoders |
| `parse_duration` | `xs:duration` → seconds |
| `MzmlbReader`, `MZMLB_FORMAT_ID` | the mzMLb `FormatReader`; `mzmlb` |
| `ContainerDataset` | an mzML/mzXML dataset read through a container (gzip, mzMLb): forwards every call and adds the container's path, size, format, facts, notes and `check` findings |
