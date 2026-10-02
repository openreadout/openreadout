# Agilent OpenLab CDS injections (`.dx`, `.rx`)

Agilent OpenLab CDS 2.x writes one `.dx` container per injection, with the data-analysis results in a `.rx` file of the same name. OpenReadout returns the detector signals of an injection and the integrated peaks of its results. Spectra directories, MS data, calibration and compound amounts are not read.

Derived from public files (Zenodo record 14316687, CC-BY-4.0, and allotropy's MIT-licensed test data) and allotropy's decoder (MIT, read as documentation). rainbow-api (LGPL), chromConverter (GPL) and allotropy are run as reference readers only. See `docs/provenance/openlab-cds.md`. Format id `openlab-cds`; crate `openreadout-chrom` (`openlab_*.rs`; zip containers through `openreadout_core::zip`). Confidence: medium.

OpenLab CDS 2.x keeps a sequence run as a **result set**: a folder `<name>.rslt` holding one injection container `<injection>.dx` per injection, the data-analysis results of each injection in a `.rx` package with the same stem, the sequence file `<name>.acaml`, and method files (`.amx` acquisition, `.pmx` processing, `.sqx` sequence template, `.scml`, `.mfx`). OpenReadout opens an injection: the `.dx` (or its `.rx`, which opens the `.dx` of the same name when present). A `.rslt` folder is refused with a hint listing its `.dx` files (`openreadout batch 'set.rslt/*.dx'` reads them all).

## Containers

`.dx` and `.rx` are zip archives (PKWARE APPNOTE; members stored or deflated) laid out as Open Packaging Conventions packages: `[Content_Types].xml` maps extensions to content types and `_rels/.rels` points at the manifest. `looks_like_openlab` recognises the first member (`injection.acmd`, `Base/InjectionACAML`, or a GUID-named part such as `b11988f5-7a62-4da7-903a-17df3a95267d.CH`): a definite detection with a `.dx`/`.rx` extension, a likely one without. A first `[Content_Types].xml` or `_rels/.rels` (any Open Packaging Conventions file, e.g. `.xlsx`) counts only with a `.dx`/`.rx` extension; any other zip with our extension is extension-only. JCAMP-DX files share the `.dx` extension but are text; they never start with `PK\x03\x04`.

| member | content type (from `[Content_Types].xml`) | what it holds |
| --- | --- | --- |
| `injection.acmd` | `Agilent.OpenLab.Rawdata/ACMD` | the injection manifest (XML) |
| `<trace id>.CH` | `Agilent.OpenLab.Rawdata/Signal179` | one detector signal |
| `<trace id>.IT` | `Agilent.OpenLab.Rawdata/InstrumentTrace179` | one instrument curve (pressure, flow, solvent ratio, temperature, lamp voltage, ...) |
| `<trace id>.UV` | `Agilent.OpenLab.Rawdata/Spectra131` | DAD spectra: a ChemStation-style version-131 `.uv` file (read with the ChemStation reader) |
| `<trace id>.UVD`, `_rels/<trace id>.UV.rels` | spectra directory and its relationship | listed, not decoded (an index of the `.UV` part) |
| other parts | other types | listed, not decoded |
| `Base/InjectionACAML` (in `.rx`) | `text/xml` | the injection's integration results |
| `Base/AuditTrail` (in `.rx`) | `application/octet-stream` | binary audit trail with readable user/computer/message strings; listed only |

Members are read whole into memory (limit 512 MiB each), checked against their CRC-32; encrypted members are refused (exit 6).

## Manifest (`injection.acmd`)

UTF-8 XML (with BOM), namespace `urn:schemas-agilent-com:acmd20`, one `InjectionInfo`. Elements are matched by local name.

| element | our name (`InjectionManifest`) | example |
| --- | --- | --- |
| `Version` | `version` | `2` |
| `Location` | `location` → `extra.vial` | `71`, `1`, `D1F-A8` (a string: plate positions occur) |
| `InjectionSource` | `injection_source` | `GC Injector`, `HipAls` |
| `InjectionVolume`, `InjectionVolumeUnits` | `injection_volume`, `injection_volume_unit` → `extra.injection_volume` (µL; nL and mL converted) | `1` `μL` (U+03BC), `5` `µL` (U+00B5) |
| `SequenceLine`, `Replicate` | `sequence_line`, `replicate` | `2`, `1` |
| `SampleName`, `RunOperator`, `Barcode` | `sample_name`, `operator`, `barcode` | `FKB-FA-035-A1-F`, `Admin` |
| `RunDateTime` | `run_started` → `extra.acquired_at` | `2023-02-23T15:05:20.4882635+01:00` (start of the run; the vendor CSV's "Injection date" is the end) |
| `AcquisitionMethod` | `acquisition_method` → `extra.method_path`, `extra.method` (file name) | `D:\Data\GC-FID\Results\FKB\FKB-FA-035-08.rslt\Polyarc_90_2_30_280_3.amx` |
| `Signals/Signal` | `signals[]` (`ManifestSignal`) | one per part |

`ManifestSignal`: `Encoding` → `encoding` (content type; `kind()` is its last component, `Signal179`), `TraceId` → `trace_id` (names the part), `DeviceName` → `device` (`FID`, `DAD`, `PMP`, `THM`, `WPS`, `FLD`), `DeviceNumber` → `device_number`, `ChannelName` → `channel` (`FID2B`, `DAD1D`, `PMP1A`), `Description` → `description` (`DAD1D,Sig=228,4  Ref=360,100`, `PMP1A,Pressure`), `TimeStart`/`TimeEnd` → `time_start`/`time_end` (ms for detector signals = the part header's first/last time; for instrument curves 0 and the value count), `Minimum`/`Maximum` → `minimum`/`maximum`, `Slope` → `slope` (= the part's scale factor), `NumberOfValues` → `declared_values`, `Units` → `units`, `IsIntegrable` → `integrable` (`true` for detector signals). A `Signal` without `TraceId` is skipped. Empty `GenericResults…` elements are ignored.

## Parts

Every decoded part is a ChemStation version-179 file (`docs/formats/chemstation.md`): `03 31 37 39` at byte 0, file type `OL DATA FILE`, the 0x1800 UTF-16 header (sample 0x35A, operator 0x758, date 0x957, method 0xA0E, units 0x104C, signal description 0x1075, scale f64 at 0x127C). The ChemStation header parser and body decoders are reused unchanged.

- **Detector signal (`Signal179`)** — body: little-endian f64 values (`Float64`; `SecondDifference` and `DeltaRecords` bodies are decoded the same way if a part carries another version). `value = raw × scale`. Time axis: the header's first and last time (f32 ms at 0x11A/0x11E) over `n − 1` intervals, as for ChemStation version 179. In the GC corpus the first time is 19.812 ms and the samples are 20 ms apart; the header's last time is 680,000 ms = 34,000 × 20 ms, so this rule gives 20.0000055 ms (0.19 ms, 3·10⁻⁶ min, of drift at the end of an 11 min run); both oracles use the same grid, and the vendor's peak limits fall on it to 10⁻¹² min.
- **DAD spectra (`Spectra131`)** — the part is a ChemStation version-131 `.uv` file (`docs/formats/chemstation.md`), opened in memory with the ChemStation reader: one trace, one channel per wavelength, one sample per spectrum. Its records carry tag 70: after the 22-byte record header (tag, length, time in ms, first/last/step wavelength × 20) little-endian f64 values, one per wavelength, instead of tag 67's 16-bit differences. `value = stored × scale × scale` (mAU): the header's scale factor (0.000476837158203125 = 1/2097.152, equal to the manifest's `ScaleFactor`) applies twice. rainbow-api's `parse_uv` returns stored × scale; the second factor is established against the DAD's own channels of the same injections (below). The manifest's `NumberOfRecords` is the spectrum count (`declared_records`).
- **Instrument curve (`InstrumentTrace179`)** — body: 16-byte samples (`PAIR_BYTES`), each a little-endian f64 time in ms and a little-endian f64 raw value; `value = raw × scale` (`1000 × 0.001` = 1.0 mL/min flow; `380 × 0.1` = 38 % solvent B). The header's time fields are not a time range for these parts. When the times are evenly spaced (all corpus curves: 60, 97.875, 100 and 1,000 ms apart) the trace is regular (`extra.axis` with `quantity` `time`); otherwise it gets a second channel `time` (minutes) and `sample_rate_hz` 0 (`extra.irregular_times`). A body cut inside a sample ends with `BodyEnd::Truncated`.

## Results (`.rx` → `Base/InjectionACAML`)

XML, namespace `urn:schemas-agilent-com:acaml21` (schema version 2.1.30.999). Read into `InjectionResults`: `Doc/DocInfo/CreationDate` → `processed_at`; `Content/Method/Name`, `OriginalVersion`, `QuantitationMethod` → `processing_method`, `processing_method_version`, `quantitation` (`ESTD`); per `Injections/Result`: `InjectionMeasData_ID/@id` → `measurement_id`, `DataAnalysisSoftware` (else `DataAnalysisApplication/AgilentApp` name and version) → `software`, `Integrator` → `integrator` (`TwelveTone`), `ProcessingStatus/TransformationChainState` → `processing_state` (`Passed`), and one `SignalResults` per `SignalResult` (`Signal_ID/@id` → `signal_id`, `Peak`s → `peaks`).

| `Peak` element | `VendorPeak` field | unit |
| --- | --- | --- |
| `@id` | `peak_id` | |
| `RetentionTime` | `rt_min` (a peak without a finite one is skipped) | min (`s`, `ms`, `h` converted) |
| `Type` | `peak_type` | `NormalPeak` |
| `Area` | `area`, `area_unit` | as written: `pA·s`, `mAU·s` |
| `AreaPercent`, `HeightPercent` | `area_percent`, `height_percent` | % |
| `Height` | `height`, `height_unit` | `pA`, `mAU` |
| `Symmetry` | `symmetry` | |
| `BeginTime`, `EndTime` | `start_min`, `end_min` | min |
| `BaselineCode` | `baseline_code` (trailing blanks removed) | `BB` |
| `BaselineStart`, `BaselineEnd` | `baseline_start`, `baseline_end` | signal units |
| `WidthBase` | `width_base_min` | min (the vendor CSV's `Width [min]`) |
| `BaselineModel` | `baseline_model` | `Linear` |
| `BaselineRetentionHeight` | `baseline_at_apex` | signal units |

`LevelStart`/`LevelEnd`, `PurityPassed`, `MSPurityPassed`, `BaselineParameters` and custom fields are kept in `info --view full` (`vendor.results`), not in the table. Each peak's `InjectionCompound` (linked by `Identification/Qualified/Peaks/Peak_ID/@id`) gives `compound` (`CompoundName`, empty names are none; every corpus file has empty names), `compound_type` (`Type`) and `expected_rt_min` (`ExpectedRetTime`, finite values only; the corpus writes `-INF`).

## Sequence file (`<result set>.acaml`)

Same namespace. Read into `SequenceFile`: `Resources/Instrument/Name` → `instrument_name` (`Luxo HPLC`, a configured name), `Technique` → `technique` (`LiquidChromatography`), `Module`s → `modules` (`InstrumentModule`: `@id` → `id`, `Name` → `name`, `Manufacturer` → `manufacturer`, `Type` → `kind` (`Pump`, `Sampler`, `ColumnCompartment`, `Detector`), `PartNo` → `part_number` (`G7117C`), `SerialNo` → `serial_number`, `FirmwareRevision` → `firmware`); per `Injections/MeasData` (`SequenceInjection`): `@id` → `measurement_id` (= the `.rx`'s `InjectionMeasData_ID`), `BinaryData/DataItem/Name` → `data_file` (the `.dx` name), `Signal`s → `signals` (`SequenceSignal`: `@id` → `id` (= the `.rx`'s `Signal_ID`), `Name` → `name`, `Description` → `description`, `Type` → `kind`, `TraceID` → `trace_id` (= the manifest's `TraceId`), `DetectorName` → `detector`, `InstrumentModule_ID/@id` → `module_id`); the first injection's `AcquisitionApplication/AgilentApp` (else `AcquisitionSoftware`) → `acquisition_software`. `injection(dx_name, measurement_id)` finds this injection by file name, else by measurement id. Every `.acaml` in the `.dx`'s folder is tried (up to 64 MiB each); the one listing the injection is used.

## Which signal a vendor peak belongs to (`SignalMapping`)

The `.rx` names signals by an id of the sequence file, not by the manifest's `TraceId`:

1. `SequenceFile` — the folder's `.acaml` lists the injection: `Signal_ID` → `SequenceSignal.trace_id` → our signal (`tables[0].extra.signal_mapping` = `sequence_file`).
2. `BaselineMatch` — no sequence file (the Polyarc data set): the vendor's `BaselineStart`/`BaselineEnd` of a peak are the raw signal at the samples nearest `BeginTime`/`EndTime` for baseline-to-baseline ends. For each result signal with peaks, the detector signal whose samples equal the most of these values (within 10⁻⁶ relative) is chosen; it must be a strict winner and different result signals must land on different signals (`baseline_match`). On all 54 GC injections this picks FID2B, the signal the vendor's CSV export names.
3. `SingleSignal` — one detector signal, one result signal with peaks.
4. `Unmapped` — otherwise; the table's `signal` column then holds the result's signal id and `trace` is NaN; `check` reports `results_unmapped`.

## Mapping to the data model

- `traces[]`: detector signals in manifest order, then instrument curves in manifest order, then spectra parts (their trace from the ChemStation reader under the manifest description, `extra.spectra` true, `extra.wavelength_range_nm`, `extra.record_encoding` `float64`) (so `analyze peaks FILE` integrates the first detector signal by default). Name = the manifest description (else the channel), one channel named after `ChannelName` (`FID2B`) with the manifest's unit (else the header's), `dtype` `float64`, `scale` from the header. `extra`: `file`, `part`, `trace_id`, `encoding`, `format_version`, `channel`, `signal`, `detector`, `device_number`, `integrable`, `declared_values`, `sample_name`, `operator`, `acquired_at`, `method`, `method_path`, `vial`, `injection_volume` (µL), `injection_source`, `sequence_line`, `replicate`, `barcode`, `separation` (`GC`/`LC` from the sequence file's technique, else `GC` when the injection source starts with `GC `), `software` (`OpenLab CDS`), `software_version` (sequence file), `instrument_name` (sequence file), `instrument` (the recording module's part number), `instrument_serial`, `module`, `module_firmware` (the module the sequence file links to the signal), `wavelength_nm`, `bandwidth_nm`, `reference_wavelength_nm`, `reference_bandwidth_nm` (from `Sig=`/`Ref=`), `x_start_min`, `x_end_min`, `axis`, `irregular_times`, `vendor_peak_count`. The experiment model reads sample, vial (sequence position), operator, start, method, injection volume, separation, instrument model and serial from `traces[0]`.
- `tables[0]` `vendor_peaks` (when a `.rx` was read), one row per vendor peak, grouped by trace, in retention-time order. **These values are calculated by the vendor software**, not by OpenReadout (`extra.source`). Columns: `signal` (code into `extra.categories`), `rt_min`, `start_min`, `end_min`, `area` (the vendor's unit, e.g. `pA·s` — `openreadout analyze peaks` reports signal × min unless `--area-seconds`), `height`, `area_percent`, `height_percent`, `width_base_min`, `symmetry`, `baseline_start`, `baseline_end`, `baseline_at_rt`, `baseline_code`, `peak_type`, `compound` (codes into `extra.categories`), `trace` (our trace index, NaN when unmapped). `extra`: `source`, `results_file`, `software`, `processing_method`, `processing_method_version`, `quantitation`, `integrator`, `processing_state`, `processed_at`, `signal_mapping`, `peaks_by_signal`.
- `info --view full` → `vendor`: `content_types`, `parts` (name, size, compressed size, method), `manifest` and `results` (the XML converted without renaming), `part_headers` (the ChemStation header of every decoded part), `results_file`, `sequence_file` (instrument, technique, modules, injection count).
- `info --view structure`: every member (`signal`, `instrument-curve`, `manifest`, `part`), `missing-part` entries for manifest signals without a part, the `.rx` (`results`) and the `.acaml` (`sequence`).

## `check` finding codes

`bad_member` (a member fails to inflate or its CRC-32), `truncated`, `bad_record`, `bad_results` (errors); the ChemStation reader's findings on each spectra part, prefixed `spectra <channel>:`; `missing_part`, `value_count_mismatch` (part values ≠ `NumberOfValues`), `record_count_mismatch` (spectra ≠ `NumberOfRecords`), `scale_mismatch` (header scale ≠ `Slope`), `no_samples`, `time_not_increasing`, `peak_outside_signal` (warnings); `not_decoded`, `results_unmapped` (info).

## Validation

Automated in `cargo test -p openreadout-corpus-tests --features corpus --test corpus` (traces, tables) and `--test openlab` (vendor results). Measured 2026-09-24:

| comparison | files | result |
| --- | --- | --- |
| detector signals against rainbow-api 1.5.2 (`.CH` parts extracted) and chromConverter 0.9.0 (whole `.dx`) — the two oracles agree exactly | 6 GC + 3 LC injections, 18 signals | bit-identical (xxh3 of every value), sample rates, first/last retention times, sample name, operator, method path |
| instrument curves against chromConverter | 3 LC injections, 29 curves | bit-identical values, times, units, names |
| DAD spectra parts against rainbow-api 1.5.2 `parse_uv` (× the scale factor once more) — 2026-09-26 | 2 LC injections (chromConverterExtraTests `MeOH1.dx`, `openlab.sirslt`), 4,050 + 1,013 spectra × 156 wavelengths | bit-identical (xxh3 of every wavelength channel), times, wavelengths |
| DAD spectra averaged over each DAD channel's band (`Sig=210,4`: 208–212 nm) against that channel, recorded separately by the detector — 2026-09-26 | 2 LC injections, 8 channels (200–360 nm) | slope 0.993–1.009 (1.023 at 200 nm, the edge of the range), r ≥ 0.99999: the unit of the spectra (a scale factor of 2,097 away from rainbow's reading) |
| detector signals and instrument curves of 3 more injections (MeOH1, a YADG GC injection, the `.sirslt` result) against rainbow-api and chromConverter — 2026-09-26 | 3 injections, 10 signals, 42 curves | bit-identical |
| our `vendor_peaks` table against allotropy 0.1.146's reading of the `.rx` | 54 GC + 12 LC injections, 482 peaks | 4,338 values equal bit for bit; every peak on the signal allotropy (via the sequence file) or the vendor CSV names |
| the table against the vendor's CSV export (`_1.csv`) | 54 GC injections, 196 peaks | 784 values within the CSV's printed precision (RT 3 decimals, others 2), baseline codes equal; sample name, operator, vial, method, injection volume and the integrated signal equal (324 fields) |
| our decoded signal integrated between the vendor's limits above the vendor's baseline (`integrate_range`), against the vendor's area | 196 peaks | median 0.0006 %, p95 0.010 %, max 0.25 % |
| height above the vendor's baseline (largest sample) against the vendor's height (an interpolated apex) | 196 peaks | median 0.42 %, max 1.4 % |
| `openreadout analyze peaks --baseline drop` (the default until 2026-09-24) against the vendor | 196 peaks | 196/196 found; retention time median 0.0001 min, max 0.004 min; area median 0.6 %, but p95 611 %: peaks on the solvent tail get the tail with a `drop` baseline |
| the same with `--baseline valley` | 196 peaks | area median 0.19 %, p95 3.8 %, max 12.7 %; area % among the vendor's peaks median 0.34, max 0.88 points |
| the same with the default `--baseline auto` | 196 peaks | 196/196 found; area median 0.49 %, p95 4.0 %, max 35 %; area % among the vendor's peaks median 0.09, p95 0.38, max 0.77 points |

OpenLab's integrator ends these GC peaks where they meet the solvent tail (baseline-to-baseline on the tail): the default `auto` baseline does the same (`baseline_reason: sloped_background`; `book/src/guides/quantitation.md` → The `auto` baseline), and `valley` also matches here; or read the vendor's own results from `tables[0]`. The LC result sets cannot validate integration: their publisher cut the detector signals to 4 values (`check` reports `value_count_mismatch`).

**Performance** (release build, Apple M-series, warm cache, measured 2026-09-24 with `/usr/bin/time -l`): `info` or `check` on a 560 KB GC `.dx` with its `.rx` under 5 ms and 13 MB peak memory; on an LC `.dx` with 16 parts, its `.rx` and a 391 KB sequence file 10 ms and 17 MB; `analyze peaks --baseline valley` on the GC injection 10 ms; `info` over all 54 GC injections 0.09 s. Parts are decoded at open (the largest corpus part is 278 KB); spectra parts, which can be tens of MB, are listed without being read.

## Known gaps

- Spectra parts with records of tag 67 (16-bit differences) inside a `.dx` are decoded as in ChemStation `.uv` files but no corpus `.dx` holds one; the `.UVD` spectra directory, MS data and any other content type are listed, not decoded.
- Compound amounts, calibration, custom fields (e.g. GPC results in a peak's `ComplexCustomFields`) and system-suitability values of the `.rx` are not read; every corpus `.rx` has unnamed compounds.
- The sequence file is used for signal mapping and instrument modules only; samples, sequence lines and methods of other injections are not reported. The `.amx`/`.pmx`/`.sqx` method files are not read.
- A result-set folder is not one data set.

## Vocabulary (every public identifier in `openlab_*.rs` must appear here)

| identifier | meaning |
| --- | --- |
| `OpenLabReader`, `OpenLabDataset`, `OPENLAB_ID`, `looks_like_openlab` | format reader, opened injection, the id `openlab-cds`, detection from the first zip member |
| `open`, `injections_in`, `signals`, `manifest`, `results`, `peak_rows`, `mapping`, `count` | open a `.dx`/`.rx`; the `.dx` files of a folder; decoded signals; the manifest; the results; vendor peaks with their signals; how they were mapped; values in a part |
| `MANIFEST_MEMBER`, `RESULTS_MEMBER`, `PAIR_BYTES`, `MAX_XML_BYTES` | `injection.acmd`; `Base/InjectionACAML`; 16-byte curve samples; largest XML part parsed (64 MiB) |
| `DxPart`, `name`, `size`, `compressed_size`, `method` | a container member: name, bytes, compressed bytes, zip method (0 stored, 8 deflate) |
| `DxSignal`, `manifest`, `part`, `part_size`, `header`, `body` | a manifest signal with its part, the part's size and ChemStation header, the decoded body |
| `PartBody` { `Channel`, `Pairs`, `Spectra`, `Missing`, `NotDecoded` }, `times_ms`, `raw`, `end` | detector values; curve samples (times in ms, raw values, how the body ended); a spectra part (index of its in-memory ChemStation data set); no part; not decoded |
| `decode_pairs` | decode curve samples |
| `PeakRow`, `result_signal`, `signal`, `trace`, `peak` | a vendor peak: its result signal, signal name, our trace index, the peak |
| `SignalMapping` { `SequenceFile`, `BaselineMatch`, `SingleSignal`, `Unmapped` } | how result signals were matched (section above) |
| `InjectionManifest`, `version`, `location`, `injection_source`, `injection_volume`, `injection_volume_unit`, `sequence_line`, `replicate`, `sample_name`, `operator`, `barcode`, `run_started`, `acquisition_method` | manifest fields (table above) |
| `ManifestSignal`, `encoding`, `trace_id`, `device`, `device_number`, `channel`, `description`, `time_start`, `time_end`, `minimum`, `maximum`, `slope`, `declared_values`, `declared_records`, `units`, `integrable`, `kind` | manifest signal fields (`declared_records`: `NumberOfRecords` of a spectra part) |
| `InjectionResults`, `measurement_id`, `processing_method`, `processing_method_version`, `quantitation`, `software`, `integrator`, `processing_state`, `processed_at` | results document fields |
| `SignalResults`, `signal_id`, `peaks` | peaks of one result signal |
| `VendorPeak`, `peak_id`, `rt_min`, `peak_type`, `area`, `area_unit`, `area_percent`, `height`, `height_unit`, `height_percent`, `symmetry`, `start_min`, `end_min`, `baseline_code`, `baseline_start`, `baseline_end`, `width_base_min`, `baseline_model`, `baseline_at_apex`, `compound`, `compound_type`, `expected_rt_min` | one vendor peak (table above) |
| `SequenceFile`, `instrument_name`, `technique`, `modules`, `acquisition_software`, `injections`, `injection` | sequence file fields; find an injection |
| `InstrumentModule`, `id`, `manufacturer`, `part_number`, `serial_number`, `firmware` | one instrument module |
| `SequenceInjection`, `data_file` | one injection of the sequence file |
| `SequenceSignal`, `detector`, `module_id` | one signal of a sequence injection |
| `parse_manifest`, `parse_results`, `parse_sequence` | XML parsers |
| `ZipIndex`, `ZipMember`, `is_zip`, `first_header`, `norm`, `text`, `read_named`, `read_text`, `file_len`, `members`, `local_offset`, `encrypted`, `crc32` | the shared zip reader (`openreadout_core::zip`) |
