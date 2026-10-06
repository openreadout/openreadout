# Shimadzu LabSolutions `.lcd` / `.gcd`

Shimadzu LabSolutions writes HPLC data as `.lcd` and GC data as `.gcd` files. OpenReadout returns the chromatograms, the status traces (pressures, temperatures), the PDA field, LC-MS spectra (MRM and SIM; profile spectra are refused) and LabSolutions' own peak tables. Two layouts exist: an older one (File Property version 3.00) and a newer one (5.01).

The container is Microsoft's compound file format, a public open specification; the LabSolutions streams inside were decoded from hex dumps of public Zenodo files (CC-BY-4.0). olefile (BSD-2-Clause) is the reference reader for the container walk. Chromatograms are checked against chromConverter (GPL-3.0, run as a black box) and LabSolutions ASCII exports, and peak tables against the vendor's own peak areas. See `docs/provenance/shimadzu.md`. Format id: `shimadzu`; crate `openreadout-chrom` (`shimadzu_dataset.rs`; the compound-file reader is `openreadout-core/src/cfb.rs`, shared with the ZVI reader). Confidence (computed by the evidence rubric, `docs/assurance.md`): medium.

## Compound file (`openreadout-core/src/cfb.rs`)

From "[MS-CFB]: Compound File Binary File Format" (Microsoft Open Specifications, https://learn.microsoft.com/openspecs/windows_protocols/ms-cfb/): signature `D0 CF 11 E0 A1 B1 1A E1` (`CFB_MAGIC`); header with major version (3: 512-byte sectors, 4: 4,096-byte sectors), sector shift at 0x1E, mini-sector shift at 0x20 (64-byte mini sectors), FAT sector count, first directory sector, mini-stream cutoff (4,096), first mini-FAT sector, first DIFAT sector and count, and the first 109 DIFAT entries. Sector `n` starts at `(n + 1) × sector size`. Chains end at `0xFFFFFFFE`; `0xFFFFFFFF` is free. The directory is an array of 128-byte entries (UTF-16 name, type 1 storage / 2 stream / 5 root, left/right sibling and child ids forming a red-black tree, start sector, size, modification FILETIME). Streams shorter than the cutoff live in the mini stream (the root entry's chain) and are addressed through the mini FAT. Loops and out-of-range sectors are corrupt (exit 4). The walk reproduces olefile's stream count on all three corpus files (308, 254, 194 streams).

## LabSolutions content (from the corpus files)

- Storages named `<kind> Raw Data`: `LC Raw Data`, `PDA 3D Raw Data`, `MS Raw Data`, `TTFL Raw Data` (version 3.00); `LSS Raw Data`, `Mass Raw Data`, `PDA 3D Raw Data`, `TLM Raw Data`, `TLM8080 Raw Data` (version 5.01). Also `LC Instrument Parameters`, `LC Data Processing`, `LSS Configuration`, `Audit Trail`, `Chromatogram Parameters`, …
- `LC Raw Data/Chromatogram Ch1` … `Ch6` hold the detector chromatograms of the older layout (only `Ch1` non-empty in the corpus: 7,472 bytes), in the signal layout below (`field_04` = interval in ms, `field_08` = point count, as `info --view structure` and `info --view full` report them).
- `File Property`: version 3.00 files start with a u32 and the text `3.00`, then user names and Windows FILETIMEs; version 5.01 files hold XML records whose text values are written `@StoX@` + hex (`@StoX@352E3031` = `5.01`, `@StoX@53797374656D...` = `System Administrator`). We report every printable run (`text_runs`), every FILETIME between 1990 and 2100, and the XML fields (`property_fields`); `szVersion` (else the text at 0x04) is `format_version`, `szGeneratedBy` is noted in `info`.

## Signal streams (`shimadzu_signal.rs`)

A 2-D signal (`Chromatogram ChN`, `PDA 3D Raw Data/Max Plot`) is one **record**; the 3-D PDA stream (`PDA 3D Raw Data/3D Raw Data`) is one record per time point.

| bytes | our name | meaning |
| --- | --- | --- |
| 0 | — | `RC\0\0` |
| 4 | `interval_ms` | sampling interval, ms (2-D; 500 and 640 in the corpus = the PDA method's `SmplRt`); 1 in 3-D records |
| 8 | `count` | number of values (points; wavelengths in 3-D records) |
| 12 | `length` | bytes of the record (header included) |
| 16 | — | 8 bytes, 0 |
| 24 | blocks | until `count` values: u16 `n`, `n` bytes of values, u16 `n` again; ≤ 256 values per block (`BLOCK_VALUES`) |

Values: the top three bits of a value's first byte are the number of bytes that follow (0–3 seen); a first byte `0x80`–`0x9F` (top bits `100`) is instead a one-byte prefix before the value (an analog-board channel writes `0x82` before every value; decoded this way its 4,200 values equal the vendor's ASCII export, `lcd-streamfind-adc-uv`; meaning of the prefix unknown); the other 5 + 8·k bits are a two's-complement integer, most significant byte first. A block's first value is absolute, the others are differences from the previous value. `Wavelength Table`: u32 count, then count u32 wavelengths in 1/100 nm (189.69 … 800.33 nm, 491 diodes). `GUMM_Information/ShimadzuPDA.1/PDA.1.METHOD`: UTF-16 XML whose `<UPD ID="…"><Val>` pairs are kept in `info --view full` → `vendor.pda_method` (`SmplRt` 640, `StWav`/`EdWav`, `EdTm`, `AN$Wave#n`, …).

**Units.** PDA values are stored in µAU (inferred: chromConverter reports the max plot in mAU as value / 1000, and the values fit absorbance) and returned in mAU. `Chromatogram ChN` values of the older layout are scaled to mV with the channel's `Chromatogram Status` factor (see Channels; the "≈ 210 ×" of the peak-table heights noted earlier is 1 / 0.0047684); newer-layout chromatograms (and `.gcd`) are scaled by their `LSS Raw Data/Chromatogram Status` record in its unit (see below). The PDA's own `2D Data Item` states the stored unit, `<Unit>uAU</Unit>` with factor 1 (mAU 1000, AU 10⁶), and LabSolutions' peak areas on a PDA channel are 1000 × our mAU·s: both confirm µAU.

## Channels (who a `Chromatogram ChN` is; 2026-09-24)

- **Points.** LabSolutions' own chromatogram (and its ASCII export) starts at t = 0 with the first stored reading repeated, so it has one point more than the stream's `count`: stored reading k is at (k + 1) × interval. Each chromatogram trace has `count + 1` points on an axis from 0 in steps of the interval (`extra.stored_points` = `count`). This was checked point for point against the vendor's ASCII exports (`lcd-streamfind-adc-uv`, two chromatograms; the same holds for the status traces of two exports). chromConverter returns the stored points from t = 0.
- **Older layout** (`LC Raw Data`): channel N's 64-byte record in `LC Raw Data/Chromatogram Status` (at (N − 1) × 64) gives the module code, the factor (0.00476837158203125 = 10000 / 2²¹ for the SPD UV detectors in the corpus, 0.2 for an analog board), and the divisor 1000 (the export's `Intensity Multiplier` 0.001) and the wavelength (280 nm, 220 nm). Values are stored × factor × factor2 / divisor, in **mV** (the export's unit). The module with that code in `LSS Configuration/LC Configuration` gives `extra.detector_name` (`Detector A`, `AD1`) and `extra.detector_model` (`SPD-20AV`). The trace is named as the export names it: `Detector A-Ch1` for UV detectors (modules named `Detector …`, channels counted per module in stream order), `AD1` for others. `extra.export_section` is the export's section title.
- **Newer layout** (`LSS Raw Data`): the chromatogram `DII` (`DT` 52 from LabSolutions 5.1, 48 from a later writer; `DK` 0) with `CN` = N in `LSS Raw Data/2D Data Item` gives the name (its `ATN`, the export's own section title, else `DN`), `detector_name` (`DETN`) and `detector_model` (`DSN`). Its `CF` is not applied; the scale and unit come from the channel's `LSS Raw Data/Chromatogram Status` record (2026-09-26), as in the older layout.
- `extra.stream_label` keeps the stream-based name (`LC Ch1`); `extra.raw_scale` is the factor applied (values = stored × `raw_scale`).

## Mapping

- `traces[]`: one per `Chromatogram ChN` stream (named as above, else `<storage> ChN`, e.g. `LC Ch1`; one channel, dtype `int64`, unit mV when scaled), then, when the file has a PDA field, `PDA max plot` (one channel, mAU) and `PDA spectra` (one channel per wavelength, named `189.69 nm` …, `extra.wavelength_nm`, mAU; one sample = the UV spectrum at that time). All: `sample_rate_hz` = 1000 / interval, `start_s` 0, `extra.axis` {`retention_time`, `min`, 0, interval}, `extra.stream`, `extra.interval_ms`, `extra.detector` (`chromatogram`, `pda_max_plot`, `pda`), `extra.wavelength_range_nm` (PDA). chromConverter spreads the 5.01 file's 3,752 points over exactly 0–40 min; we use the stored interval (0.64 s, the method's `SmplRt`), which ends 0.64 s later.
- `check` decodes every signal (`bad_signal` when a block length, trailer or count disagrees) and compares the stored max plot with the PDA field's maximum over wavelengths at every time point, after subtracting the first spectrum (`pda_max_plot_mismatch`). LabSolutions computes the max plot that way: in a run whose first spectrum is not zero the max plot starts at 0.

## Status traces (2026-09-26)

`LC Raw Data/StatusLog ChN` (older) and `LSS Raw Data/StatusLog ChN` (newer) are `RC` records like the chromatograms; each is a trace with `extra.detector` `status`, the vendor's t = 0 point, and values scaled by its record in `… /StatusLog Status` (64 bytes per channel, the layout of `Chromatogram Status`: u16 module code at +4, f64 at +16, +24, +32, unit text at +40): value = stored × (+16) × (+24) / (+32) in that unit (`kgf/cm2`, `bar`, `°C`). Newer layout: named by `2D Data Item` (`DT` 48; the export's `LC Status Trace(...)` title, e.g. `Pump A Pressure`, `Oven Temp.`); older layout: named `<module model> <quantity> [n]` (`LC-20AD pressure 1`, `CTO-20AC temperature 2`) because the file stores no names. `extra`: `stream_label`, `export_section` (newer), `module_model`, `raw_scale`, `quantity` (`pressure`, `temperature`, `status`), `stored_points`. Status traces come after the chromatograms and the PDA traces. Logs without a status record are not exposed.

## Newer-layout chromatogram units and `.gcd` (2026-09-26)

A newer-layout chromatogram is scaled by its `LSS Raw Data/Chromatogram Status` record (same rule and unit text; `uV` → `µV`). A `2D Data Item` entry is a chromatogram when its title contains `Chromatogram` and not `Status Trace(` (LabSolutions 5.1 LC entries have `DT` 52; a `.gcd` writes `DT` 18, `[Chromatogram (Ch1)]`; the file-version-5.01 LCMS-8060NX files of MTBLS7425 write `DT` 48 and `DK` 0 for `[LC Chromatogram(Detector A-Ch1)]`, the `DT` of their status logs, which have `DK` 2), and a status log when its `DT` is 48 and its title contains `Status Trace(`; entries of neither kind name nothing (the trace keeps its stream-based name). A chromatogram stream of exactly 24 + 8 × count bytes holds f64 values after its header instead of integer blocks (the `.gcd`: 40 ms, 66,255 points; `extra.stored_as` `f64`, channel dtype `float64`); `ShimadzuDataset::signal_values` refuses those (the trace is read with `read_trace`).

## LC-MS data (`TLM Raw Data`, spectra run 0, 2026-09-26)

| stream | layout |
| --- | --- |
| `TLM Raw Data/Retention Time` | u32 ms per record |
| `TLM Raw Data/TIC Data` | u64 per record: the record's total ion current |
| `TLM Raw Data/Spectrum Index` | 24 bytes per record: u32 size, u32 1, u32 offset into `MS Raw Data`, u32 0, u32 record number (1-based), u32 next number |
| `TLM Raw Data/MS Raw Data` | per record: i32 −1, u32 inflated length, u32 packed length, a zlib stream |

An inflated record: u32 retention time (ms), u32 cycle start (ms), u32 event (1-based), u32 cycle, u32 record index, u16 acquisition code, u16, 12 bytes, u32 `polarity_code` (+36), u32 count (+40), then:

- code 15, MRM: count × (u32 Q1 × 100, u32 Q3 × 100, u32 intensity) → an MS2 spectrum, precursor Q1, points (Q3, intensity), `centroided`; one Q1 per record (two are an error);
- code 11, SIM: count × (m/z × 100, the same m/z × 100, intensity) → an MS1 spectrum of the selected ions;
- code 10, full scan, and code 14, product-ion scan: count, (precursor × 100 twice for product-ion scans), first and last m/z × 100, then count u32 intensities on a 0.1 m/z grid. **Refused** (exit 6): the vendor's own reader returns other values (full scans) or nothing that could check them (product-ion scans).

The spectra run (`name` `LC-MS`) lists every record: `scan_count`, `ms_levels`, `rt_range_s`, `extra.acquisitions` (records per kind: `mrm`, `sim`, `full scan`, `product ion scan`), `extra.events`, `extra.unreadable_records`. `spectra` gives each record's retention time, level, precursor (MRM, product-ion scan) and stored TIC, and `extra.acquisition`, `extra.event`. Spectra: `extra.acquisition`, `event`, `cycle`, `polarity_code`. Polarity is `unknown`: the u32 at +36 is 0 or 1 and differs between the two polarities of a two-polarity method, but nothing says which is which. `check` inflates every record, compares its retention time with the index and the sum of each MRM/SIM record's intensities with its stored TIC.

Validated (`tests/corpus/`, oracle `oracle/shimadzu_ms_gt.py`): 610 MRM and SIM spectra of three CC0 files equal to the ground-truth slices converted with ProteoWizard msconvert (vendor reader), point for point; the profile spectra of the four files are refused.

## Vendor peaks (`vendor_peaks`, 2026-09-26)

`tables[0]`, when the file holds LabSolutions peak tables, has one row per peak of every channel: `signal` (code into `extra.categories`: the trace name, or `PDA Ch<k> <wl> nm` for PDA channels), `peak`, `rt_min`, `start_min`, `end_min`, `area`, `height`, `baseline_start`, `baseline_end` (LabSolutions' µ-units: µV·s/µV, µAU·s/µAU — 1000 × a trace in mV or mAU), `capacity_factor`, `plates`, `plate_height`, `tailing`, `resolution` (older layout only; NaN otherwise), `flags` (0x10 on peaks the export does not mark `V`). `extra.source` is `vendor`, `extra.streams` names the stream of each signal's table.

- Older layout: `LC Data Processing/Peak Table-N` for chromatogram channel N: u32 count, u32 0, 280-byte records.
- Newer layout: `LSS Data Processing/PT-<DSID>` for a chromatogram's `2D Data Item` data set id (else `LSS Data Processing Original/PT-<DSID>`), and `PT-<PDA data set>.<k>` for PDA channels: `VER1`, u32 count, 12 bytes, records of (length − 20) / count bytes.
- Record: u32 flags (+0), u32 retention time ms (+4), f64 area (+8), f64 height (+24), f64 baseline at start (+40) and end (+48), u32 start and end ms (+56, +60); older layout also f64 k′ (+208), plates (+216), plate height (+224), tailing (+232), resolution (+240). A record whose retention time is outside its start–end window is an error (`a vendor peak table was not read` note, `vendor_peaks_unreadable` in `check`).
- `check` integrates each chromatogram and the PDA max plot between every peak's start and end above the vendor's baseline and reports `vendor_area_mismatch` when an area (for peaks above 1 % of the largest) differs by more than 10 %.

Tables and spectra: `vendor_peaks` only. `info` lists the raw-data storages and the chromatogram streams; `info --view structure` lists every storage and stream with its size (`kind` `storage`, `stream`, `chromatogram`); `info --view full` holds the property text, times and fields, the chromatogram stream headers and the full stream list; `check` validates the container and reads every stream to its declared size.

## Experiment facts (`Dataset::experiment`)

With no traces to derive from, the reader supplies the experiment model's facts from `File Property` itself:

| field | version 5.01 | version 3.00 |
| --- | --- | --- |
| `sample.id` | `SampleInfo/smpl_id`, else `smpl_name` | — |
| `sample.name` | `smpl_name` when it differs from the id | — |
| `sample.sequence_position` | `szVialNum` (else `vial_num`) | — |
| `acquisition.operator` | `operator_name`, else `szGeneratedBy` | the user name at 0x14 |
| `acquisition.started_at` | the FILETIME `dwHighGeneratedDateTime` × 2³² + `dwLowGeneratedDateTime` (low half written signed), UTC | the FILETIME at 0x34, UTC |
| `method.name` | stem of `SampleInfoFile/methodfile` (`….lcm`) | stem of the text run ending in `.lcm` |

The generated time is taken as the start in UTC: the 3.00 file's is 2023-06-06 07:29:29 UTC and its batch is `SingleRun120230606152927.lcb` (15:29 local, UTC+8). `inj_vol` (`@FtoX@41200000`, `@FtoX@1`) is not decoded. No instrument model is recorded in `File Property`.

## `check` finding codes

`bad_structure` (loops, out-of-range entries), `truncated` (a stream's chain or size runs past the file) (errors); `no_raw_data` (warning).

## Vocabulary (every public identifier in `shimadzu_*.rs` and `cfb.rs` must appear here)

| identifier | meaning |
| --- | --- |
| `ShimadzuReader`, `ShimadzuDataset`, `SHIMADZU_ID`, `open`, `channels` | format reader, opened file, the id `shimadzu`, open, chromatogram streams found |
| `ShimadzuChannel`, `stream`, `size`, `tag`, `field_04`, `field_08` | a `Chromatogram ChN` stream: path, bytes, first four bytes, u32 fields at 4 and 8 |
| `text_runs`, `property_fields` | printable text runs of a stream; `<name>value</name>` pairs with `@StoX@` hex decoding |
| `ShimadzuSignal`, `kind`, `stream`, `count`, `interval_ms`, `wavelengths`, `label`, `points`, `signals`, `signal_values` | a decodable signal (trace): stored count, its channel label, its trace points (stored + the vendor's t = 0 point for chromatograms), the list, its values |
| `ChannelStatus`, `channel`, `module_code`, `factor`, `factor2`, `divisor`, `wavelength_nm`, `scale`, `STATUS_RECORD`, `channel_statuses` | one 64-byte record of `LC Raw Data/Chromatogram Status` (u16 module code at +4, f64 factor at +16, f64 at +24, f64 divisor at +32, f32 wavelength at +58); stored → mV = factor × factor2 / divisor |
| `LcModule`, `code`, `serial`, `firmware`, `model`, `name`, `MODULE_RECORD`, `lc_modules` | one 128-byte module record of `LSS Configuration/LC Configuration` (u16 code at +0, serial at +8, firmware at +24, model at +56, user name at +72) |
| `DataItem`, `data_type`, `channel`, `name`, `detector`, `device`, `export_section`, `factor`, `DATA_ITEM_CHROMATOGRAM`, `data_items`, `hex_f64` | one `DII` of `LSS Raw Data/2D Data Item` (`DT`, `CN`, `DN`, `DETN`, `DSN`, `ATN`, `CF` hex f64) |
| `ChannelLabel`, `name`, `export_section`, `detector_name`, `detector_model`, `wavelength_nm`, `scale`, `unit`, `label_lc_channels`, `label_lss_channels` | who a chromatogram channel is (older layout: status record + module; newer layout: `2D Data Item`) |
| `ShimadzuSignalKind` { `Chromatogram`, `MaxPlot`, `Pda`, `Status` } | what a signal holds (`Status`: a status log) |
| `f64_values`, `vendor_peaks` | the signal stream holds f64 values (GC); the file's vendor peaks |
| `is_f64_record`, `decode_f64_record` | a signal stream of 24 + 8 × count bytes; its f64 values |
| `unit`, `display_unit`, `quantity` | a status record's unit text (+40); `uV` → `µV`, `C` → `°C`; pressure/temperature/status from a unit |
| `label_lc_status`, `label_lss_status`, `DATA_ITEM_STATUS`, `is_chromatogram_item`, `is_status_item`, `data_set` | names and scales of status logs (older/newer layout); `DT` 48; whether a `2D Data Item` entry is a chromatogram (its title contains `Chromatogram` and not `Status Trace(`, whatever its `DT`) or a status log (`DT` 48 and a `Status Trace(` title); its `DSID` |
| `TlmEntry`, `offset`, `size`, `TlmIndex`, `entries`, `rt_ms`, `tic`, `parse_index` | the LC-MS index streams (§ LC-MS data) |
| `TlmHeader`, `cycle_rt_ms`, `event`, `cycle`, `record`, `code`, `polarity_code`, `count`, `first_q1`, `precursor_mz`, `header`, `record_header`, `inflate_record` | an LC-MS record's header; reading it (the header alone, or the record inflated) |
| `TlmSpectrum`, `ms_level`, `centroided`, `scan_window_mz`, `points`, `decode`, `acquisition_name`, `MAX_TLM_RECORD`, `TLM_MRM`, `TLM_SIM`, `TLM_FULL_SCAN`, `TLM_PRODUCT_ION_SCAN` | a decoded record (MRM, SIM; profiles refused); acquisition codes; the largest record inflated |
| `VendorPeak`, `signal`, `number`, `flags`, `rt_min`, `start_min`, `end_min`, `area`, `height`, `baseline_start`, `baseline_end`, `capacity_factor`, `plates`, `plate_height`, `tailing`, `resolution` | one peak of a LabSolutions peak table (see Vendor peaks) |
| `peak_table_old`, `peak_table_new`, `multi_chromato_wavelengths`, `PEAK_RECORD_OLD`, `MAX_PEAKS` | parse an older/newer peak table; PDA channel wavelengths; older record size; most peaks accepted |
| `RcHeader`, `interval_ms`, `count`, `length`, `RC_HEADER_LEN`, `BLOCK_VALUES`, `parse_rc_header` | signal record header |
| `SignalError`, `decode_record`, `record_offsets`, `wavelength_table`, `method_values` | signal decoding; PDA wavelength table; PDA method values |
| `Cfb`, `version`, `sector_size`, `mini_sector_size`, `mini_cutoff`, `entries`, `problems`, `stream`, `read`, `CFB_MAGIC` | parsed compound file; find a stream; read a stream; signature |
| `CfbEntry`, `id`, `path`, `is_stream`, `start_sector`, `modified`, `created` | a directory entry: index, `/`-separated path, stream or storage, first sector, modification and creation FILETIMEs (UTC) |
