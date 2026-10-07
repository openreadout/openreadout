# Thermo Scientific Chromeleon 7 archives (`.cmbx`)

Chromeleon 7 writes a `.cmbx` archive when a sequence (or other items of a data vault) is exported. The archive is a zip holding the sequence, its injections with their raw signals, 3D fields and audit trails, and the methods the sequence uses. OpenReadout returns one trace per signal and per 3D field, Chromeleon's stored results, and the sequence and injection metadata.

Format id `chromeleon`. Derived from the corpus files and checked against Chromeleon's own
exports and results; how each fact was established is in `docs/provenance/chromeleon.md`. Code:
`crates/openreadout-chrom/src/chromeleon.rs` (container, signals, 3D fields),
`chromeleon_results.rs` (sequence-file objects, stored results, compressed blobs) and
`chromeleon_dataset.rs` (data set).

## Container

A zip with deflated members:

- `header.xml` (last member, UTF-8 with BOM): root `ChromeleonHeader` with `GeneratorVersion`
  (the Chromeleon build that wrote the archive, e.g. `7.2.9.11323`, `7.2.10.23925`,
  `7.3.1.6535`), `ContainerVersion` (`2.0`), `DateCreated` (in the exporting computer's
  language). Nested `ChromeleonElement` items: `Name`, `ItemType`, `Url`, and for items with
  data the member (`Filename` for the sequence, `RawDataFilename` otherwise), its `Size` and a
  `RawDataFileId` (`YYYY\MM\DD\hhmmssmmm.raw`, the local time the data was started; a second
  folder of a day is `DD_2`). Item types seen: `Dionex.Chromeleon.Data.Sequence` →
  `.Injection` (with `InjectionType`: `Unknown`, `Blank`, `Standard`) → `.Signal`,
  `.SpectralField` (3D data, a `.sfd` member), `.AuditTrail`, `.MSRawItem`, `.CustomRawItem`;
  and, under the sequence, `.InstrumentMethod`, `.ProcessingMethod`, `.ReportDefinition`,
  `.DataPresentationLayout`. A `.Signal` without a member is a channel Chromeleon derives on
  demand (an extracted-ion chromatogram of MS data): it has no stored points.
- `<sequence>.seq_<n>.cmd` (first member): the sequence file, a protocol-buffers stream.
- `<n>_<id>.raw` / `.sfd`: one member per signal, 3D field, audit trail or MS item.

Detection: a zip whose first member ends in `.seq_<n>.cmd` (and the `.cmbx` extension).

## Signal members (2D data)

Sections, each an 8-byte tag and a little-endian u64 length counting the whole section:

- `SignHdr\0`: `01 02 07`, 2 bytes, a Windows FILETIME (the signal's acquisition start, UTC),
  a small kind byte (2, 3 or 5 seen), a length byte and the signal name, a 16-byte GUID, a short
  tail (not decoded).
- Point sections of one of three encodings (never mixed in one member):
  - `PtsLDiff`, repeated (about one minute each): f64 start time (minutes), then integers in a
    sign-and-magnitude variable-length code (first byte: continuation 0x80, sign 0x40, six low
    bits; each further byte seven bits): rate denominator and numerator (points per minute =
    numerator / denominator: 1 and 3000, 1 and 120, 10000 and 600000, 1000 and 600000 seen),
    scale numerator and denominator; the block's first value (absolute), the differences to
    each following value; a final 0. Value = running sum × numerator / denominator; point *i*
    at start + *i* / points-per-minute.
  - `PtsLL2Df`, repeated: f64 time ticks per minute (6·10¹⁰: nanoseconds), f64 value units
    (10⁸: value = integer / 10⁸), then (time, value) integer pairs in the same code: the
    block's first pair absolute, the second the first differences, every later pair the second
    differences; a final 0. A block may hold a single pair.
  - `PtsDDCmp`, repeated: integers 1, 0, the decompressed and the compressed length; an LZMA2
    dictionary byte and a raw LZMA2 stream that decompresses to (time in minutes, value) pairs
    of little-endian f64. Used for PDA-extracted channels (regular) and MS extracted-ion
    chromatograms (at the MS scan times: irregular).

Points whose times are not evenly spaced (the sequence file's `StepMin` and `StepMax` differ)
are a trace with sample rate 0 and two channels: `time` (minutes) and the value.

Refused (exit 6 on read, `signal_not_decoded` in `check`): another section tag, encodings mixed
in one member, blocks that change rate, scale or units, gaps between `PtsLDiff` blocks, a
`PtsDDCmp` block not starting 1, 0. Corrupt (exit 4): truncated sections or integers, a block
without its terminator, sums beyond 64 bits, non-increasing times, lengths that do not match.

## 3D fields (`.sfd` members)

`3DRawSpc`, then u32 1, u32 0, u32 the spectrum count, u32 8; then one record per spectrum:
f64 time (minutes), u32 length, and sections as above: `RawSpHdr` (not decoded; it repeats the
time) and one `PtsLDiff` spectrum whose f64 is the first wavelength (nm) and whose four header
integers are the wavelength step numerator and denominator (step = numerator / denominator:
10/10 = 1 nm, 12/10 = 1.2 nm) and the value scale numerator and denominator (1000 /
16777215 mAU), then the first value, differences and a final 0. A field is one trace with one
channel per wavelength (`<nm> nm`, `extra.wavelength_nm`) and one sample per spectrum. The
sequence file describes each field (time, wavelength and absorbance ranges). Spectra on another
grid or scale than the first, other sections and another header are refused.

## Sequence file

A protocol-buffers stream (keys and lengths as varints; wire types 0, 1, 2, 5; field numbers are
ours to name).

**Objects.** Top-level field 18 records are a catalog, one per object: 3 → 3 the type
(`Sequence`, `Injection`, `Signal`, `Chromatogram`, `AuditTrail`, `ProcessingMethod`,
`InstrumentMethod`, `ReportDefinition`, `DataPresentationLayout`), 5 the object's id, 6 its
parent's id (an id is a message of two fixed64 fields whose 16 bytes are a GUID), 8 its number
in the data vault (the `995` of `…/995.smp` in `header.xml` URLs), 16 (chromatograms)
Chromeleon's stored results. Top-level field 19 records hold the objects' data: 25 the id, 28
the name, 27 a version string, and one type-specific field.

**Signal descriptions.** A field-19 record whose first field (5) holds field 6 = a signal's
`RawDataFileId` describes the signal in its field 3: 3 = `<CmData>` XML with `StepMin`/`StepMax`
(minutes), 4 = number of points, 5 = time axis (1 first, 2 last, 3 unit, 4 quantity), 6 =
signal axis (1 minimum, 2 maximum, 3 unit `mV`, `nC`, `psi`, `bar`, `mAU`, `counts`, `mL/min`;
4 quantity), 7 = a label (`WVL:254 nm`), 11 = device (`GC`, `Pump_1`, `UV`, `PDA`,
`MSDevice`), 12 = module or driver, 15 = the scale factor. A 3D field is described in field 4
instead: 5 time axis, 6 wavelength axis (nm), 7 absorbance axis (minimum, maximum, `mAU`), 11
device, 13 driver.

**Injections** (field 19 of the injection's record): 2 calibration level, 3 → 3 injection type,
4 → 3 status (`Finished`, `Interrupted`), 5 position, 6 volume (µL), 7 → 1 inject time (ISO 8601
with offset), 9, 10, 11 three factors (dilution factor and weight among them: reported only
when all three are equal), 17 a GUID string; fields 40 link the processing method (40.1 = 0)
and the instrument method (40.1 = 1) by name.

**Processing methods** (field 14 settings, 36 a gzip-compressed designer layout, 41 field
collections, 43 → 2 the components): a component has 12 name, 10 id, 5 → 2 → 9 expected
retention time, 5 → 2 → 14 a second time value, 5 → 2 → 6 amount unit, 22 →
`Components.ConcentrationLevelCollection` the amount per calibration level. The processing
method's catalog record lists the components in field 17 (1 = the component's id, 4 = the key
stored peaks use).

**Instrument methods** (field 11 → 3): a `CpXm` blob of `<CmData><Method>` XML: the
instrument's symbol table, then a script of `StageNode`s (`InstrumentSetup`, `Inject`,
`StartRun`, `Run`, `StopRun`, `PostRun`) of `TimeStepNode`s (time in minutes; −∞ for a stage's
initial step) of `PropertyStepNode`s (a `SymbolPath` and a `Value` with its unit, e.g.
`Pump_1.%C.Value` = `92.5 [%]`) and `CommandStepNode`s.

## Compressed blobs

A 4-byte magic `D?AC` (`DIAC`, `DdAC`) or `Cp??` (`CpXm` instrument methods, `CpAu` audit-trail
members), a u16 header length (8, 16 or 20), a u16 version; headers of 16 bytes or more then
hold the compressed and the decompressed length. After the header: an LZMA2 dictionary-size
byte and a raw LZMA2 stream (decoded by `openreadout_codecs::lzma2_decode`). Every blob is
UTF-8 `<CmData>` XML. An audit trail appended to over time holds several `<CmData>` documents
one after the other.

## Chromeleon's stored results (`ChmData.` blobs)

A chromatogram's catalog field 16: `ChmData.`, `01 vv 00 00` (vv = 1 in 7.2, 2 in 7.3),
`01 00 00 00`, `67 00 00 00`, `01 00 00 00`, three length-prefixed 32-byte values, then u32 tag
+ u32 length sections: tag 2 the peaks (3 and 4 not decoded). Tag 2: 8 zero bytes; a count
(7-bit variable length) of skim lines, each a type byte (0: a straight line; others refused)
and two (time, value, slope) triples of f64; a count of peaks; per peak start and end (time,
value on the signal), a u32 code, the apex (time, value), each flank (leading, trailing) as
eight optional values, each after a presence byte 0/1: (always absent), the point at 10 % of the
height, at 50 %, at 5 %, (always absent), a tangent point and its slope, (always absent); then
retention time, area, height (f64); a presence byte and, when 1, an optional 16-byte component
key and an optional (expected retention time, second value) pair; a skim byte (the 1-based skim
line a rider peak rides on, 0 for none); after the peaks a count of baseline segments, each a
count of (time, value) points. A value never seen before where "always absent" stands is refused
(the chromatogram's results are listed as not decoded).

A peak's baseline is its skim line, else the baseline segment containing its limits. Its area
is the trapezoidal integral of (signal − baseline) between its limits, minus the areas of the
rider peaks inside it; its height is signal − baseline at the apex; both are stored as
magnitudes. Chromeleon stores results only when it has computed and saved them: injections at
the end of a sequence may have none.

## Mapping to the data model

- One trace per stored `Signal` item and one per `SpectralField` in document order, named
  `<injection> / <signal>`; signals have one channel (the signal name; `unit` from the
  description; `dtype` `int64` with `scale`, or `float64` for `PtsDDCmp`), irregular ones a
  `time` channel first; fields one channel per wavelength. A processing method may hold the
  signals of its calibration standards (`FixedInjectionList/FixedInjection`): they are traces
  too, named `<FixedInjectionName> / <signal>`, with `extra.fixed_injection` = true.
- `sample_count`, `sample_rate_hz`, `start_s` and `extra.axis` come from the description (a
  field's from its description and its member's stated spectrum count); members are decoded
  only when read (a field's first 256 KiB at open, for its grid).
- Trace `extra`: `signal`, `injection`, `injection_type`, `injection_index`, `fixed_injection`,
  `processing_method`, `instrument_method`, `sequence`, `member`, `file_id`, `acquired_local`,
  `device`, `module`, `quantity`, `label`, `wavelength_nm`, `stored_min`, `stored_max`,
  `encoding` (`PtsLDiff`, `PtsLL2Df`, `PtsDDCmp`, `3DRawSpc`), `x_start_min`, `x_end_min`,
  `time_channel`, `spectral_field`, `wavelength_range_nm`, `wavelength_step_nm`,
  `not_decoded`; from the injection details `injection_position`, `injection_volume_ul`,
  `inject_time`, `injection_status`, `calibration_level`, `dilution_factor`, `weight`; and
  `vendor_results` (`stored`, `none`, `not decoded: …`) with `vendor_peaks` (their count).
- `tables[0]` `vendor_peaks` (when Chromeleon saved results): one row per stored peak —
  `trace`, `peak`, `rt_min`, `start_min`, `end_min`, `baseline_start`, `baseline_end`, `area`,
  `height`, `area_pct` (of the chromatogram's stored areas), `component` (categories),
  `expected_rt_min`, `skim`, `code`, `width_50_min`, `width_10_min`, `width_5_min` (between the
  stored 50/10/5 % points), `asymmetry_10` (Ph. Eur., b/a at 10 %), `tailing_5` (USP), `plates_ep`
  (5.54 (tR/W0.5)²), `resolution_ep` (to the next peak, 1.18 Δt/(W0.5,1 + W0.5,2)), `apex_value`.
  `extra`: `source` `vendor`, `results`, `traces`, `results_versions`, `units`.
- The next table `injections`: `injection`, `name`, `injection_type`, `status`, `position`,
  `volume_ul`, `inject_time` (Unix seconds; `extra.inject_times` as stored), `level`,
  `processing_method`, `instrument_method`, `dilution_factor`, `weight`, `vault_number`.
- Attachments (`extract`): each audit trail (`<injection> / audit trail`), each
  instrument method and each processing method's settings, decompressed XML.
- `vendor_metadata` (`info --view full`): `injections`, `components`, `instrument_methods` (name and script
  `steps`: `stage`, `time_min`, `kind`, `symbol`, `value`), `chromatograms` (`results`, `peaks`,
  `version`), the header items and signal descriptions.
- `format_version`: `GeneratorVersion`. MS items: a note naming the embedded Thermo `.raw`
  member and how to extract it.

## `check` finding codes

`bad_member` (a member fails its CRC-32 or cannot be inflated), `missing_member`, `bad_signal`
(corrupt signal), `signal_mismatch` (point count, first or last time, or scale differ from the
description), `bad_field`, `field_mismatch` (a 3D field's spectrum count, time or wavelength
range, minimum or maximum differ from its description), `bad_results` (a corrupt stored-results
blob), `bad_blob` (an audit trail or method that does not decompress) — errors;
`signal_not_decoded`, `field_not_decoded`, `results_not_decoded`, `signal_name_mismatch`,
`stored_range_mismatch`, `start_time_mismatch`, `vendor_peak_mismatch` (a stored peak our
integration on the decoded signal does not reproduce within 1e-6), `bad_audit_trail` —
warnings; `signal_header_layout`, `member_not_verified`, `derived_signal` — info.

## Validation

- Chromeleon ASCII exports (Figshare 28560326, HPAEC-PAD, Chromeleon 7.3.1): 6 injections,
  55,446 points, every time and value equal to the printed precision; their position, volume
  and inject time equal the injection details.
- Chromeleon PDF report (Zenodo 18089202, GC-FID, Chromeleon 7.2.10): 57 injections; the
  plotted chromatograms agree with our values; all 15 peaks Chromeleon stored for the
  injections the report shows equal the report's retention time, area and height to the printed
  3 decimals (the report's other 4 peaks belong to 4 injections Chromeleon saved no results for).
- Chromeleon's stored results recomputed from our decoded signals: every stored peak of 5
  archives (3 of them with a vendor export) reproduced within 1e-6 relative (see the corpus test
  `chromeleon`), which checks the decoded signal values under every peak of every chromatogram
  Chromeleon integrated.
- 3D fields: every field of 2 archives decodes whole and equals its description (spectrum count,
  time range, wavelength range, minimum and maximum absorbance); Chromeleon's PDA-extracted
  channels (EXT350/450/600NM, stored separately) equal the 3D field at the nearest wavelength
  bit for bit (6 channels, 288,006 points); the DAD's own UV channels (recorded separately by
  the detector with a 4 nm bandwidth) agree with the field averaged over the same band (slope
  0.993–1.000, r ≥ 0.99997, 5 channels of one injection).
- Every signal of 10 archives (1,600+ signals, 5 depositors, Chromeleon 7.2.9, 7.2.10 and
  7.3.1; 3 members damaged as deposited): point count, times, scale, minimum and maximum equal
  to the sequence file's description.

Run: `cargo test -p openreadout-corpus-tests --features corpus --profile corpus --test corpus`
(`chromeleon_oracle`) and `--test chromeleon`.

## Known gaps

- Stored-results sections 3 and 4, the code word's bits and the flank tangent's exact meaning
  are not decoded; amounts are not stored with the peaks (Chromeleon computes them from the
  calibration when it reports) and are not computed.
- Report definitions, data presentation layouts and custom raw items (`ChromatogramCacheData`)
  are listed, not decoded.
- Mass-spectrometry data (MSRawItem) is an embedded Thermo .raw file: extract it and open it as
  thermo-raw. Derived MS channels without stored points are not traces.
- Chromeleon 6 backups (`.cmb`) are another format.

## Vocabulary (every public identifier in `chromeleon*.rs` must appear here)

| identifier | meaning |
| --- | --- |
| `ChromeleonReader`, `ChromeleonDataset`, `CHROMELEON_ID`, `open` | format reader, opened archive, the id `chromeleon`, open by path |
| `first_member_is_sequence` | detection from the first zip member's name |
| `MAX_SIGNAL_BYTES`, `MAX_SEQUENCE_BYTES`, `MAX_HEADER_BYTES`, `MAX_SIGNAL_POINTS`, `MAX_BLOB_BYTES` | largest signal member (256 MiB), sequence file (256 MiB), `header.xml` (64 MiB) read; most points per signal (and spectra per field); largest decompressed blob |
| `SIGNAL_HEADER_TAG`, `POINTS_TAG`, `POINTS_VERSION`, `LL2_TAG`, `DDCMP_TAG` | `SignHdr\0`, `PtsLDiff`, the rate denominator of the first files (1), `PtsLL2Df`, `PtsDDCmp` |
| `FIELD_MAGIC`, `SPECTRUM_HEADER_TAG` | `3DRawSpc`, `RawSpHdr` |
| `SIGNAL_ITEM`, `INJECTION_ITEM`, `SEQUENCE_ITEM`, `MS_RAW_ITEM`, `PROCESSING_METHOD_ITEM`, `AUDIT_TRAIL_ITEM`, `SPECTRAL_FIELD_ITEM` | the `ItemType`s read |
| `ArchiveHeader`, `generator_version`, `container_version`, `date_created`, `items`, `of_type`, `ancestor`, `parse_header` | `header.xml`: its attributes and items; items of a type; nearest enclosing item of a type; the parser |
| `ArchiveItem`, `id`, `name`, `item_type`, `url`, `member`, `size`, `file_id`, `injection_type`, `parent`, `fixed_injection` | one item; `fixed_injection` names the calibration standard (`FixedInjection`) a processing method holds the signal for |
| `SignalHeader`, `start_filetime` | the `SignHdr` section: name and acquisition start (FILETIME, UTC) |
| `DecodedSignal`, `header`, `encoding`, `start_min`, `points_per_min`, `scale_num`, `scale_den`, `raw`, `float_values`, `times`, `blocks`, `scale`, `values`, `step_min`, `time`, `len`, `is_empty` | a decoded signal: header, encoding, first time, rate (0 when irregular), scale, stored integers or floats, explicit times, section count; scale factor, scaled values, step, time of a point, point count |
| `SignalError` { `Corrupt`, `Unsupported` }, `decode_signal`, `signed_varint` | why a signal was not decoded; the decoder; one block integer |
| `FieldIndex`, `stated`, `records`, `index_field` | a 3D field member's stated spectrum count and each spectrum's time and byte range; the walker |
| `SpectrumGrid`, `x0`, `step_num`, `step_den`, `points`, `step`, `decode_spectrum` | a spectrum's first wavelength, wavelength step, scale and point count; one record's decoder |
| `PbValue` { `Varint`, `Fixed64`, `Bytes`, `Fixed32` }, `pb_fields` | a protocol-buffers field value; a message's fields |
| `SignalDescription`, `points`, `time`, `signal`, `device`, `module`, `step_range`, `label`, `is_regular`, `parse_sequence_signals` | the sequence file's description of a signal (file id, points, axes, device, module, scale, stored step range, label) and its parser |
| `FieldDescription`, `wavelength`, `value`, `parse_sequence_fields` | the description of a 3D field (time, wavelength and absorbance axes) and its parser |
| `SignalAxis`, `min`, `max`, `unit`, `quantity` | an axis of a description |
| `ObjectId`, `SequenceObject`, `kind`, `number` | an object's 16-byte id; a catalog record joined with its data record (type, id, parent, vault number, name) |
| `SequenceContents`, `objects`, `injections`, `chromatograms`, `components`, `instrument_methods`, `processing_methods`, `notes`, `parse_sequence_contents` | everything read from a sequence file beyond signal descriptions, and its parser |
| `InjectionDetails`, `status`, `position`, `volume_ul`, `inject_time`, `level`, `factors`, `processing_method`, `instrument_method` | an injection's details (the three factors of fields 9–11) |
| `Component`, `method`, `key`, `retention_min`, `second_value`, `amount_unit`, `levels` | a processing-method component and its calibration levels |
| `InstrumentMethod`, `blob`, `MethodStep`, `stage`, `time_min`, `symbol`, `method_steps` | an instrument method's compressed script; one step of it; the script parser |
| `AuditMessage`, `message`, `property`, `audit_messages` | one audit-trail message (time, level, category, message, device, property, value, unit); the parser |
| `ChromatogramRecord`, `signal_file_id`, `injection`, `results` | a chromatogram with the file id of its signal and its stored results |
| `StoredResults`, `version`, `skims`, `peaks`, `baselines`, `baseline_line`, `parse_stored_results` | a chromatogram's stored results; the straight baseline under a peak; the parser |
| `StoredPeak`, `start`, `end`, `code`, `apex`, `leading`, `trailing`, `area`, `height`, `component_key`, `component_values`, `skim` | one stored peak |
| `PeakFlank`, `at_10`, `at_50`, `at_5`, `tangent_point`, `tangent_slope` | a peak side's stored points |
| `SkimLine`, `start_slope`, `end_slope`, `Point` | a skim line; a (time, value) pair |
| `integrate_stored_peak` | our integration of a stored peak on the decoded signal |
| `is_blob`, `blob_magic`, `blob_stated_size`, `unpack_blob` | the compressed blobs |
