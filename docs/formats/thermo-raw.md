# Thermo Fisher `.raw`

Thermo Fisher mass spectrometers (LTQ, Orbitrap, Exactive, Q Exactive, Exploris, Fusion, Eclipse, Ascend, ID-X, IQ-X, Astral, Stellar and TSQ series) write each acquisition to one `.raw` file. OpenReadout returns the mass spectra with their scan events and precursors, the instrument method and logs, and the traces of UV, PDA and analog detectors recorded in the same file.

Derived from public files by hex dump and comparison with the depositors' own mzML and mzXML conversions, and with an msconvert conversion for the detector data. Prior art read as documentation: the unfinnigan wiki, the OpenTFRaw format specification (CC-BY-SA) and the `thermorawfile` crate documentation (MIT/Apache). Details and URLs: `docs/provenance/thermo-raw.md`.

**Confidence markers.** Every field below is one of:

- **validated** — decoded by `openreadout-thermo` and compared value-for-value with the depositor's conversion on every corpus file (bit-exact arrays, exact metadata);
- **inferred** — consistent across every corpus file and with the structure around it, but no independent value to compare against (the evidence is given);
- **prior art** — taken from the notes above and not contradicted by the corpus, but not independently checked;
- **unknown** — kept verbatim under a neutral name (`*_words`, `*_value`, `texts`) and shown by `info --view full`.

**Supported versions:** 57, 61, 62, 63, 64, 66 (validated). 58–60 and 65 are attempted with the prior-art layouts and reported as unvalidated; anything else exits 6. Versions 57 (LTQ, Xcalibur 1.0, 2005), 61 (LTQ Orbitrap, Xcalibur 2.0) and 62 (LTQ FT Ultra, Xcalibur 2.2, 2008) were read with the prior-art layouts unchanged and match their depositors' conversions on every compared scan (see the provenance log, 2026-10-06). Every public file found from 2023–2026 (Orbitrap Astral, Astral Zoom, Ascend, Eclipse, IQ-X, ID-X, Exploris, TSQ Altis, TSQ 9610; Xcalibur and Tune up to 5.1) is version 66: new instruments changed what a version-66 file holds, not its version.

All integers little-endian. Strings are UTF-16LE: *fixed* fields are zero-padded to their byte width; *counted* strings are a u32 number of UTF-16 code units followed by the units (no terminator).

## File map

```
0            file header (1356 bytes)
1356         sequence row (variable)
             autosampler block (variable)
             file-info block (804 / 1840 / 1856 bytes + 6 counted strings)  -> scan data address, run header address
             [embedded instrument-method container, versions with method_file_present = 1]
scan data    one packet per scan (or peaks + window record), addressed through the scan index
run header   (7408 bytes before v64; 7576 from v64)                         -> all stream addresses
             instrument id
             instrument-log columns
instrument log   records
error log        u32 lead + entries
             segment/event table
             per-scan parameter columns
             (tune block, not decoded)
scan index   one row per scan
scan events  u32 lead + one event per scan
scan parameters  one fixed-size record per scan
```

Every corpus file satisfies: the run header's self-address equals the pointer that led to it; the instrument-log columns end exactly where the log records start; `records × (4 + record size)` ends exactly at the error log; the scan events end exactly at the scan-parameter address; every packet header's size equals the scan index's size. `openreadout check` verifies all of these.

## File header (offset 0, 1356 bytes)

| offset | size | our name | meaning | confidence |
| --- | --- | --- | --- | --- |
| 0 | 18 | `SIGNATURE` | bytes `01 A1` then `Finnigan` in UTF-16LE | validated (all files) |
| 18 | 2 | — | terminating NUL | inferred |
| 20 | 16 | `header_words` | four u32; observed 0, 0, 0, 0x80000 | unknown |
| 36 | 4 | `version` | file version (63, 64, 66 in the corpus) | validated |
| 40 | 112 | `created` | audit stamp (below): acquisition start | prior art + inferred (local clock; the depositors' mzML `startTimeStamp` repeats it with a `Z` suffix) |
| 152 | 112 | `modified` | audit stamp: last write | prior art |
| 264 | 4 | `header_value` | observed 0, 1 | unknown |
| 268 | 60 | — | zero | unknown |
| 328 | 1028 | `header_text` | fixed UTF-16 free text (always empty in the corpus) | prior art |

Audit stamp (112 bytes): u64 `filetime` (Windows FILETIME, local clock), fixed 50-byte `account`, fixed 50-byte `account_2` (observed `Xcalibur_System`, `ExactiveUser`, an instrument name), u32 `stamp_value`.

**Header checksum** (`header_checksum`, `CHECKSUM_OFFSET` = 148 = the `stamp_value` of `created`): Adler-32 with start value 0 (not the RFC's 1) over the first `min(file length, CHECKSUM_SPAN)` = 10 MiB of the file with these four bytes zeroed. Hint from the `thermorawfile` docs; validated: it matches on all thirteen corpus files, so `check` reports `checksum_mismatch` when anything in the first 10 MiB changed after writing.

## Sequence row (offset 1356)

| part | our name | confidence |
| --- | --- | --- |
| u32 | `injection_words[0]` | unknown (observed 0) |
| u32 | `row_number` | prior art |
| u32 | `injection_words[1]` | unknown |
| fixed 12 bytes | `vial_label` | prior art |
| 5 × f64 | `injection_volume`, `sample_weight`, `sample_volume`, `internal_standard_amount`, `dilution_factor` | prior art (values 0/1/10 in the corpus) |
| 13 counted strings | `texts[0..13]` | slots below |
| v ≥ 57: 3 counted strings + u32 `row_value` | `texts[13..16]` | prior art |
| v ≥ 60: 15 counted strings | `texts[16..31]` | prior art; always empty |

Named slots (accessors on `SequenceRow`): 1 `sample_name` (observed `QC1`, `HU17`, `blanc`, `884_Caffeine_POS` — the per-injection label; **inferred**), 2 `sample_id`, 3 `comment`, 4–8 `user_labels`, 9 `instrument_method` (path of the method file; validated by content), 10 `processing_method`, 11 `original_file_name`, 12 `original_path`, 13 `vial` (e.g. `D:37`).

## Autosampler block

Six u32 `numbers` then a counted `tray_description` (e.g. `1.8 ml Vial, 5 trays 40 vials each`). **Inferred:** `numbers[1]` is the linear vial index — `D:37` → 157, `D:35` → 155, `A:11` → 11, `C:17` → 97 with `numbers[2]` = 40 vials per tray (`vial_index`). Unused autosampler: `numbers[0..2]` = `u32::MAX`.

## File-info block

| offset in block | size | our name | confidence |
| --- | --- | --- | --- |
| 0 | 4 | `method_file_present` | prior art; 1 exactly when a Finnigan-signed container sits between this block and the scan data (validated by content) |
| 4 | 8 × u16 | `utc_time` (year, month, weekday, day, hour, minute, second, ms) | inferred: the audit stamp minus the lab's UTC offset (MTBLS404 Paris +2 h, PXD000001 Cambridge +1 h, MTBLS805 Bremen +2 h/+1 h by season, MTBLS797 +1 h, MTBLS755 Okinawa +9 h); identical to the millisecond except in MTBLS755, where the two differ by a further 90 s. `info` reports it as `acquired_at` (UTC) and the audit stamp with that offset as `acquired_at_local` |
| 20 | 4 | — | unknown |
| 24 | 4 | data address (before v64) | validated |
| 28 | 4 | `controller_count` | prior art, validated (1; 2 in the Exactive and TSQ files; 3 in the Fusion Lumos file) |
| 36 | 12 × `controller_count` | `controllers` (before v64): per row u32 `controller_type`, u32 `controller_index`, u32 `run_header_address` | inferred (six single-controller v63 files: (0, 0, run header); the v63 Accela PDA file: (3, 0, PDA), (4, 1, channels), (0, 2, MS)) |
| 44 | 4 | run header address (before v64) = the first row's address | validated |
| 56 | 4 | `run_header_address_2` (before v64) = the second row's address (prior art put it at 52, which does not fit the three-controller file) | inferred |
| 20 + 0x314 | 8 | `data_address` (v ≥ 64) | prior art, validated |
| 20 + 0x31C | 16 × `controller_count` | `controllers` (v ≥ 64): per row u32 `controller_type`, u32 `controller_index`, u64 `run_header_address` | inferred (see below) |

`ControllerRef` rows: `controller_type` 0 on the row whose run header holds the mass-spectrometer scans in every file; 2 on the extra rows of the Exactive and Fusion Lumos files (analog channels), 5 on the TSQ file's second row (its autosampler); a Vanquish DAD/CAD file adds types 3 (PDA) and 4 (UV/CAD channels), all read as traces (see "Detector controllers"). The first row's address is `run_header_address`, the second's `run_header_address_2` (the prior-art fields). The MS controller is not always first: in the Fusion Lumos file it is row 3 (`controller_index` 2), after two type-2 rows. `ms_run_header_address` takes the first row of type 0, else `run_header_address`; that is the run header whose self-address check then passes on every file. Before v64 the rows are 12 bytes from offset 36 (32-bit addresses); in the v63 Accela PDA file the MS controller is the third row, and taking the first address (the older reading) fails at scan event 10.

Binary length `file_info_binary_len`: 804 (v < 64), 1840 (v64), 1856 (v66). Then five counted `label_headings` (`Study`, `Client`, `Laboratory`, `Company`, `Phone`) and a counted `computer_name` (acquisition PC).

## Embedded instrument method

When `method_file_present` is 1, the bytes from the end of the file-info block to the scan-data address hold (inferred from the hex dump of every such file):

| part | our name |
| --- | --- |
| a complete 1356-byte file header (signature, version, audit stamps) | — |
| u32 | `container_size` |
| counted string | `source_path` (the acquisition PC's temporary copy of the method) |
| u32 n, then n × (counted display name, counted storage name) | `devices` (e.g. `LTQ Orbitrap Discovery MS` / `LTQ`, `Accela Pump` / `LegacyDualPump`) |
| `container_size` bytes | a Compound File Binary container ([MS-CFB], a public Microsoft Open Specification), starting `D0 CF 11 E0 A1 B1 1A E1` (`CFB_SIGNATURE`); `container_offset` |

The container has one storage per device with streams `Data` (binary settings, not decoded), `Header` (a Finnigan file header) and `Text` — the human-readable method the acquisition software prints (UTF-16LE). `MethodDocument` keeps the stream list and each device's text (`device_texts`); `info --view full` shows them under `vendor.instrument_method`, `info` lists the device names and the `MS Run Time (min):` line under `method_summary`. The container sizes and the scan-data address agree on every corpus file with a method; the Q Exactive MALDI files have none.

### Gradient table

`info` puts the LC pump program under `method_summary.gradient` (`device`, `solvents` {channel → name}, `steps[]` {`time_min`, `flow_ul_min`, `percent` {channel → %}}) when a device text holds one in a layout seen in the corpus (`gradient.rs`):

| device text | layout | corpus |
| --- | --- | --- |
| Accela pumps (`LegacyDualPump/Text`) | `Solvent A:` … `Solvent D:` lines; `Pump 1 gradient table:`, a `No. Time A% B% C% D% µl/min` header, one row per step | `mtbls404-*`, `mtbls20-*`, `mtbls797-dotsha05`, `mtbls1822-tsq-74` |
| EASY-nLC (`Proxeon_EASY-nLC/Text`) | `Gradient:`, a `Time [mm:ss] Duration [mm:ss] Flow [nl/min] Mixture [%B]` header, one row per step | `pxd000001-tmt-erwinia-01` |
| Vanquish (`SiiXcalibur/Text`) | `PumpModule.Pump.%A1_Equate: "…"` names chosen by `%A_Selector`/`%B_Selector`; `<t> [min]` blocks with `PumpModule.Pump.Flow.Nominal: 0.500 [ml/min]` and `PumpModule.Pump.%B.Value: 1.0 [%]` | `mtbls1820-lumos-uplc-31` |

Times are minutes (nLC `mm:ss` converted), flows µL/min (`nl/min` ÷ 1000, `ml/min` × 1000); the composition is what the text lists (the nLC and Vanquish list %B only). The instrument's `model` is `model_2` when that extends it word by word (`TSQ` → `TSQ Vantage Standard`) or when the two begin with the same word and `model` lacks a word of `model_2` (an instrument name with a site label: `Orbitrap Exploris Slot #10076` → `Orbitrap Exploris 240`), and `software` is `Xcalibur` whenever a software version is recorded (the name every depositor mzML gives that version).

## Run header

| offset | size | our name | confidence |
| --- | --- | --- | --- |
| 0 | 2 × u32 | `sample_words` | unknown (1, 0) |
| 8 | u32 | `first_scan` | validated (1 everywhere) |
| 12 | u32 | `last_scan` | validated (scan count = last − first + 1) |
| 16 | u32 | `instrument_log_count` | validated (log length arithmetic) |
| 20 | u32 | `error_log_count` | validated |
| 28–40 | 4 × u32 | scan index, scan data, instrument log, error log addresses (before v64; 0 from v64) | validated |
| 48 | 5 × f64 | `max_total_ion_current`, `low_mz`, `high_mz`, `start_time_min`, `end_time_min` | prior art; start/end equal the first/last index retention times |
| 88 | 56 | — | unknown (zero) |
| 144 | 88 + 40 + 320 | `sample_texts` | prior art (empty) |
| 592 | 6 × 520 | `device_files[0..6]` | prior art; temporary device files and the original path |
| 3712 | 2 × f64 | `run_values` | inferred: [0.5, method length in minutes] (19, 60, 45 match the method names) |
| 3728 | 7 × 520 | `device_files[6..13]` | prior art |
| 7368 | u32 ×2 | scan events, scan parameters addresses (before v64) | validated |
| 7376 | u32 | `scan_event_count` | validated (= scans) |
| 7380 | u32 | `scan_parameter_count` | validated |
| 7384 | u32 | `segment_count` | validated against the segment table |
| 7396 | u32 | own address (before v64) | validated |
| 7400 | 2 × u32 | `display_words` | **inferred:** `display_words[1]` = decimals of m/z in scan filters (`filter_mass_decimals`): 2 in the v63/v64 and Q Exactive Plus files whose filters read `[50.00-1000.00]`, 4 in the Q Exactive HF files whose filters read `[70.0000-1000.0000]`; `display_words[0]` = 2 everywhere (energies are always printed with 2) |
| 7408 (v ≥ 64) | 7 × u64 | scan index, scan data, instrument log, error log, (unknown), scan events, scan parameters addresses | prior art, validated |
| 7464 | 2 × u32, u64 | —, `own_address` | validated |
| 7480 | 24 × u32 | — | unknown (zero) |

## Instrument id (right after the run header)

Three u32 `id_words` (observed [1,0,0] and [1,5,0]), counted `model`, `model_2`, `serial_number`, `software_version`, then four counted `tags` (observed `rev. 1`, empty, `m/z`, `Relative Abundance`). Model, serial and software version validated against the mzML `instrument serial number` and `Xcalibur` software version. `instrument.model` (`full_model`) is `model_2` when it holds every word of `model` and more, after dropping a last word of `model` equal to the serial number: `TSQ` / `TSQ Vantage Standard`, `Exploris` / `Orbitrap Exploris 240`, `Orbitrap Exploris MB11229C` / `Orbitrap Exploris 120` (the Exploris files' exports name the instrument by `model_2`).

## Self-describing records (instrument log, per-scan parameters)

Columns: u32 count, then per column u32 `kind`, u32 `width`, counted `label`. Value sizes by kind (inferred from record-length arithmetic on every file; the kind meanings follow OpenTFRaw's table):

| kind | bytes | decoded as |
| --- | --- | --- |
| 0 | 0 | section heading, no value |
| 1 | 1 | i8 |
| 2, 3, 4 | 1 | boolean |
| 5 | 1 | u8 |
| 6 / 7 | 2 | i16 / u16 |
| 8 / 9 | 4 | i32 / u32 |
| 10 / 11 | 4 / 8 | f32 / f64 (`width` = display decimals) |
| 12 | `width` | 8-bit NUL-terminated text |
| 13 | 2 × `width` | UTF-16 text |

Labels are the instrument's own strings (`Charge State:`, `Monoisotopic M/Z:`, `Ion Injection Time (ms):`, `Conversion Parameter B:`, …); `info --view full` shows them untouched. Instrument-log records are an f32 retention time (minutes) followed by one record. Per-scan parameter records have no prefix and sit back to back at the scan-parameters address.

## Error log, segment/event table, parameter columns

At the error-log address: a u32 (`error_log_lead`, observed 0 or the entry count), then `error_log_count` entries of f32 retention time + counted message (`ErrorLogEntry`). Then the **segment/event table** (`MethodTable`): u32 segment count; per segment u32 event count and that many templates (`ScanTemplate`). Each template is a complete scan event of the file's version (`scan_event`, the layout of "Scan events" below); `template_words` holds its reaction and range counts (before v66 also its coefficient and tail-item counts and closing word) and `scan_range` its first range. Most files store events with no reaction and one range, or nothing at all, there: preamble + 36 bytes before v66 and preamble + 24 bytes in v66, which is how the table was first read. Some store full ones: the TSQ Altis Plus file the transition's precursor reaction and product window with the compound name as `tail_text` (`glycine`), the Exploris 120 file a DDA template's reactions and ranges with a label (`A4G4`), the Exactive file `mtbls797-dotsha05` (v64) reactions in its second segment. Reading those as fixed-size templates stopped mid-table and lost the per-scan parameter columns that follow (`mtbls13930-neg-sqc-5` had none before 2026-10-06 and has 71 now; `mtbls797-dotsha05` had none and has 43). Then the per-scan parameter columns. Validated: the table ends exactly where the parameter columns begin, and they decode, in every development file of versions 57–66. The counts equal `segment_count` and the method's scan events per cycle.

### Runs without per-scan events

The TSQ Altis Plus (`mtbls6991-tsq-altis-plus-sar11-40`), TSQ 9610 (`msv97728-tsq9610-gc-crbalf03`) and ISQ (`mtbls758-isq-growth-93`, version 64) files record 0 in the run header's `scan_event_count`, and their scan-event stream is the 4-byte lead alone. Each scan's event is then the template at the segment and event number of its index row (`MethodTable::template_event`); the position field of those rows holds no position (u32::MAX in the TSQ Altis Plus file, 0 in the TSQ 9610 and ISQ files). Validated: the TSQ Altis Plus export's 291 SRM chromatograms and TIC are rebuilt exactly from the scans read this way, the TSQ 9610 export's 7,372 spectra (filters included) equal ours, and so do the ISQ file's 6,864 scans against its ANDI-MS export (which stores intensities as integers and only the points inside each scan's mass range). `check` warns (`template_mismatch`) when a scan's index row records a range more than 0.05 m/z away from its template's: the TSQ rows equal their templates, the ISQ rows end 0.015 above the template's 900.0.

## Scan index (one row per scan)

| offset | size | our name | confidence |
| --- | --- | --- | --- |
| 0 | u32 | data offset (before v64) | validated |
| 4 | u32 | `scan_index` (zero-based position) | validated (no position in runs without per-scan events: u32::MAX or 0, see above) |
| 8 | u16 | `event_number` | inferred (position of the scan in the method cycle; 0xFFFF in MALDI files) |
| 10 | u16 | `segment_number` | inferred |
| 12 | u32 | `next_scan` | prior art |
| 16 | u32 | `kind_code` | 18–21 observed for packet scans (meaning unknown); 24 = `WINDOW_RECORD_KIND` (validated: every scan of the TSQ Vantage SRM file, see "Window-record scans") |
| 20 | u32 | `data_size` | validated: packet length in bytes; for kind 24 the window count + 1 |
| 24 | f64 | `rt_min` | validated (= mzML `scan start time`) |
| 32 | 5 × f64 | `total_ion_current`, `base_peak_intensity`, `base_peak_mz`, `low_mz`, `high_mz` | validated (TIC, base peak equal the mzML values) |
| 72 (v ≥ 64) | u64 | `data_offset` | validated |

Row length (`scan_index_entry_len`): 72 (v < 64), 80 (v64), 88 (v66; the last 8 bytes hold a cycle counter and a u32 we do not use). Measured as (events address − index address) / scans on every file. Packet address = scan-data address + `data_offset`.

## Scan events (one per scan)

A u32 lead (the scan count before v66, 0 in v66), then events back to back:

| part | before v66 | v66 |
| --- | --- | --- |
| `preamble` | 128 bytes (80 in v57–61, 120 in v62) | 136 bytes |
| u32 reaction count | then `reactions` | then `reactions` |
| u32 window count, `scan_ranges` | 2 × f64 each | 2 × f64 each |
| u32 coefficient count, `coefficients` | f64 each | f64 each |
| u32 item count, `tail_items` | 8 bytes each | 8 bytes each |
| `tail_words` | 1 × u32 | 1 × u32, then counted UTF-16 `tail_text` |

`scan_ranges` was first read as a constant u32 followed by one range; the TSQ Vantage SRM file, whose events carry 1–3 ranges (one per product ion, each the product m/z ± 0.001) after that word, shows the word is the range count (validated: the events then end exactly at the parameter address, and the ranges equal the window records' f32 bounds on all 10 400 scans). `scan_range` is the low end of the first window to the high end of the last.

The v66 event ends with a u32 and a **counted UTF-16 text** (`tail_text`): zero units in every file before 2024, so it was first read as a second constant word; the Orbitrap Exploris 120 file `mtbls13930-neg-sqc-5` writes a five-character label there on every event (`L1031`, meaning unknown), which is what made the old reading fail at event 2 (validated: the events then end exactly at the parameter address in all five Exploris files). `tail_words` keeps the u32 and the unit count.

In version 66 each reaction is 56 bytes: the 32 below plus three f64 `reaction_values` (observed the scan range and 0.0 for the full-range MS1 "reaction", or 0.0, 0.0, 0.5 for MS2). This was first read as three doubles after the reaction list; the Orbitrap Fusion Lumos file, whose MS1 events have no reaction and no such doubles while its MS2 events have both, shows they belong to the reaction. `tail_items`: the f64 values of the options the preamble turns on, in-source CID energy first (`source_cid_energy`, byte 8 = 0, printed `sid=10.00`: 10.0 and 20.0 in `pwiz-thermo-source-cid`, whose first scan has no item and no `sid=`), then the FAIMS compensation voltage (`faims_cv`, byte 122 = 0, printed `cv=-40.00`). Q Exactive and Exactive events carry one item of 0.0 with neither option on. Until 2026-10-06 any non-zero first item was read as the in-source CID energy, which would have printed the Astral file's FAIMS voltage as `sid=-40.00`. `info` lists the voltages under `faims_compensation_voltages` and the options under `scan_options`; each spectrum carries `extra.faims_cv`.

Validated on every file: the stream ends exactly at the scan-parameters address (this is what fixed the counted tail: the v64 Exactive file has one tail item, the v64 Velos file none). A **reaction** is f64 `precursor_mz`, f64 `isolation_width`, f64 `energy`, 2 × u32 `reaction_words` (32 bytes). `reaction_words[0]` is the fragmentation code (`Activation::from_reaction_code`): 1 = trap CID (`cid`, MTBLS20), 11 = beam-type CID (`hcd`, PXD000001, MTBLS755, MTBLS805), 9 = electron-transfer dissociation (`etd`, `ElectronTransfer`; `pwiz-thermo-bsa-ft-etd`) — validated against the exports' activation terms and filter strings on every MS2 scan. In v66 and in the v64 Exactive file even MS1 events carry one reaction describing the full-range isolation (centre 535, width 930 for a 70–1000 scan) — `precursor_reactions` returns every reaction of an MS^n (n ≥ 2) event and none of an MS1 event. An MS^n event with more reactions than stages is multiplexed (`is_multiplex`, filter `msx`): the four reactions of `pwiz-thermo-ft-hcd-msx`'s MS2 event are its four co-isolated precursors, printed in stored order. An MS3 event has one reaction per stage (`pwiz-thermo-it-hcd-sps`: 599.77@hcd, 677.31@cid); its other SPS notches are only in the scan parameter `SPS Masses`.

### Preamble bytes

Positions are the same in every version (the array only grew at the end):

| byte | constant | meaning | values | confidence |
| --- | --- | --- | --- | --- |
| 4 | `POLARITY_BYTE` | `Polarity` | 0 negative, 1 positive | validated |
| 5 | `DATA_KIND_BYTE` | profile / centroid (`is_profile`) | 1 `p`, 0 `c` | validated (filter `p`/`c`) |
| 6 | `MS_LEVEL_BYTE` | `ms_level` | 1, 2, … | validated |
| 7 | `SCAN_KIND_BYTE` | scan type (`scan_kind_token`) | 0 `Full`, 2 `SIM`, 3 `SRM`, … | validated for 0; others prior art |
| 8 | `SOURCE_CID_BYTE` | in-source CID on (`source_cid_energy`) | 0: the first tail item is the in-source CID energy (`sid=10.00`; `pwiz-thermo-source-cid`, `zenodo19222374-cannabis-neg-h1-1`); 1 or 2 in every other corpus event | validated (filters of the source-CID file) |
| 9 | `SCAN_RATE_BYTE` | scan rate (`scan_rate_token`, `scan_rate_token_for`) | in ion-trap scans 0 → ` t` (turbo) after the source (every scan of the two ion-trap files `pwiz-thermo-it-hcd-sps`, `pwiz-thermo-source-cid`); 1 and 2 print nothing on LTQ-generation traps (ion-trap MS1/MS2 of LTQ XL and Velos); on Tribrid traps (`ScanRates::Tribrid`: Orbitrap Fusion, Fusion Lumos, Eclipse, ID-X, IQ-X, Ascend) 1 → ` r` (rapid; the ion-trap MS2 scans of `pxd064311-lumos-hela-gluc` and `msv102435-iqx-leaf-qc-neg-03`) | validated on the files named |
| 10 | `DEPENDENT_BYTE` | dependent scan (`is_dependent`) | 1 → ` d` in the filter | validated |
| 11 | `IONIZATION_BYTE` | `Ionization` | 3 `ESI`, 5 `NSI`, 8 `MALDI`; others prior art | validated for 3, 5, 8 |
| 24 | — | unknown: 4 in the v63/v64 files, 1 in v66, for CID and HCD scans alike (prior art calls it the activation; the corpus contradicts that — see reactions) | | |
| 28, 30 | `BRACE_BYTE` | printed as `{b28,b30}` followed by two spaces after the analyzer when not 0xFF | 0xFF almost everywhere; 1,1 in the v64 Exactive file (`FTMS {1,1}  + p ESI Full lock ms …`) | inferred from one file |
| 40 | `ANALYZER_BYTE` | `Analyzer` | 4 `FTMS`, 0 `ITMS`, 7 `ASTMS` (the Orbitrap Astral's second analyzer, `Analyzer::Astral`); 6 in the TSQ files, whose filters have no analyzer token (`info` lists such a code as `code 6`); others prior art | validated for 0, 4, 6, 7 |
| 42 | `LOCK_BYTE` | 0 prints `lock` before `ms` | 0 in the v64 Exactive file, 2 elsewhere | inferred from one file |
| 120 | `SUPPLEMENTAL_ACTIVATION_BYTE` | 1 prints `sa` (supplemental activation) after `d` | 1 in all 7,475 ETD scans of `pxd032908-velos-etd-pep38` (its ThermoRawFileParser export writes `d sa Full ms2 …@etd…`); 0 in its MS1 scans and in the ETD scan of `pwiz-thermo-bsa-ft-etd` (no `sa`); absent before version 63 | inferred from two files |
| 122 | `FAIMS_BYTE` | FAIMS on (`faims_cv`) | 0: a tail item (after the in-source CID energy, when there is one) is the FAIMS compensation voltage, printed `cv=-40.00` after the ion source; 1 or 2 elsewhere | validated on every event of `msv99294-astral-nanopots-1cell-b9` (-40 V) and `msv99508-eclipse-faims-ptrc-f03` (-45, -60, -75 V) |

### Scan filter text (not stored; composed)

No corpus file contains a filter string in any encoding. `filter_text` composes

`{analyzer}[ {b28,b30} ] {+|-} {p|c} {source}[ sid={energy}][ cv={voltage}][ t|r][ d][ sa] {scan type}[ lock][ msx] ms[n] [{precursor}@{activation}{energy} …] [{low}-{high}]`

The order of `sid=` and `cv=`, and of `cv=` and `t`/`r`, is not established (no corpus file has both): `scan_options` names those cases and the assurance profile leaves them unvalidated.

with m/z printed to `filter_mass_decimals` places and energies to 2, an exact binary tie rounded away from zero (`fixed_decimals`: a scan range ending at 309.03125 prints `309.0313`; Rust's formatting rounds ties to even and gave `309.0312` on 7 scans of `mtbls14508`). It equals the mzML `filter string` on every scan of every corpus file that carries one (the PXD000001 mzXML has no filter lines).

**Inferred, no reference:** SRM/CRM scans (scan type 3/4) print each precursor without `@activation energy` and list every window: `- c ESI SRM ms2 258.026 [78.942-78.944, 96.939-96.941]`. The TSQ export holds chromatograms only, so no filter string exists to compare against. The TSQ reactions carry reaction code 12, which `Activation` leaves `Unknown`.

## Scan data packet

40-byte `PacketHeader`: u32 `header_value` (the number of acquisition segments: 1, or one per mass range of a multi-range SIM scan; `segments`, accepted up to `MAX_SEGMENTS` = 64), u32 `profile_words`, u32 `centroid_words`, u32 `layout_flags`, u32 `descriptor_words`, u32 `extra_words`, u32 `triplet_words`, u32 `annotation_words`, f32 `low_mz`, f32 `high_mz`. Sizes in 4-byte words. A packet of *k* > 1 segments continues with the other segments' ranges, 2(*k* − 1) f32 (`extra_range_words`; `Packet.segment_ranges` holds all *k*). `packet_len` = 40 + 4 × (range words + the six sizes) equals the index size on every scan (validated on every scan of all 19 files, and on the 14 three-range SIM scans of the LTQ Velos test file, whose packets are 16 bytes longer than the six sizes say). Then, in order:

1. **Profile** (`profile_words` > 0): f64 `first_value`, f64 `step`, u32 chunk count, u32 `bin_count`, then chunks: u32 `first_bin`, u32 bin count, [f32 `mz_correction` when `layout_flags` bit 7 is set], f32 × n `values`. Grid value of bin b = `first_value + step·b` — a frequency when `step` < 0.
2. **Centroids** (`centroid_words` > 0): per segment u32 n then n records of f32 m/z + f32 intensity (words = Σ(1 + 2n)) or f64 m/z + f32 intensity (words = Σ(1 + 3n)). Every corpus file uses the 8-byte form. The multi-range SIM centroids are validated by consistency (no export holds them: ProteoWizard's test conversion leaves SIM scans out): on all 14 scans their intensities sum to the index's TIC and the largest sits at its base-peak m/z. Profile data of multi-segment packets has not been seen and is refused (exit 6).
3. **Peak descriptors** (`descriptor_words` = n): per centroid u16 `peak_index`, u8 `flags`, u8 `descriptor_byte`.
4. `extra_words` (n + 1 f32) and `triplet_words` (m/z, value, value triples) — unknown, skipped.
5. **Peak annotations** (`annotation_words` > 0; Orbitrap Exploris generation, 0 in every older file): skipped. `annotation_words` was read as a constant `header_value_2` (0 on every scan of the fourteen files before the Exploris files); on the Exploris 120/240/480 files the index size exceeds the other five parts by exactly 4 × this word on every scan (9,459 + 7,098 + 7,176 + 4,110 scans), which is what `check` reported as `packet_size` before. Layout observed on every scan of those files (**inferred**, meaning unknown): u32 `0x000A000F`, then records of u32 tag (`0x00080000` in all), u32 word count and that many words: the first record holds a u32 0 and one u16 per centroid (0x1FFF for most peaks; other values carry high bits 0x4000/0x8000/0xC000 and small numbers shared by neighbouring peaks, which looks like isotope-cluster membership), the second a u32 1 and 16-byte entries (an f64 m/z and two u32). No reference conversion reports them.

**m/z of a profile bin** (`MzScale`). An m/z grid (`step` > 0, `Direct`: ion-trap profiles) numbers its stored values from 1: value j of a chunk lies at bin `first_bin + j + 1`, and a value past `bin_count` is not part of the scan — validated on every ion-trap profile scan of `pwiz-thermo-ltqvelos` (43 scans) and `pwiz-thermo-it-hcd-sps` (reading `first_bin + j` put every m/z one step, 0.02 m/z ≈ 45 ppm at m/z 450, too low and kept one point more). With coefficients `A, B, C` = event coefficients 2, 3, 4 (5 or 7 coefficients; Orbitrap), `mz = A + B/(f·f) + C/((f·f)·(f·f)) + mz_correction`. Bit-identical to the reference conversion only in exactly this evaluation order (validated on 12 000+ profile scans). Four coefficients → `A + B/f + C/f²` with coefficients 1, 2, 3 (prior art; validated at single precision on the 3,577 profile FT-ICR scans of the LTQ FT Ultra file `pxd000951-ltqft-he4`, whose export stores 32-bit m/z). `step` > 0 → the grid is m/z already (validated on the ion-trap profiles named above).

**Rendering the profile as the reference conversion does** (`render_profile`; validated bit-for-bit on every MTBLS404 and MTBLS755 profile scan — see below for the one exception):

- every stored chunk bin is emitted; in addition up to `PROFILE_PAD_BINS` = 4 zero-intensity bins before and after every chunk, and grid bins 1–4 and `bin_count−3`…`bin_count` (never bin 0 as padding);
- a padding bin takes the `mz_correction` of the chunk containing it, else of the first chunk starting after it, else of the last chunk;
- only when flagged peaks are excluded (`set_exclude_flagged_peaks(true)`, the older conversions): chunks whose m/z span contains a centroid whose descriptor has a `FLAG_EXCLUDED` bit set are emitted with zero intensity. The Exploris 240 profile export `mtbls12283-rumenwall-qc-id-01-neg` (ProteoWizard 3.0.22284) keeps them: all 1,232 of its profile scans hold flagged peaks and match with no blanking;
- m/z stays strictly increasing: a point whose m/z (padding with a very different correction) falls at or below its predecessor's is placed at predecessor + `MONOTONIC_NUDGE` (1·10⁻⁵).

**Centroid view:** the stored centroid list, every peak included — what current conversions with the vendor library write (validated on every scan of the six files whose exports were made by ProteoWizard 3.0.20286 or later: `mtbls14508`, `mtbls13401`, `mtbls13930`, `mtbls12283`, `mtbls14308`, `mtbls13880`). Conversions made by ProteoWizard up to 3.0.20239 (all 2.x builds, 3.0.7099 … 3.0.19280 and GNPS's 3.0.20239 of August 2020 in the corpus) left out the centroids flagged `FLAG_EXCLUDED` and blanked their profile chunks (validated on MTBLS20, MTBLS404, MTBLS805, MTBLS755, MTBLS797, PXD000001 MS2 and the 2,622 centroid scans of MTBLS773); the change came between 3.0.20239 and 3.0.20286 (October 2020); `ThermoDataset::set_exclude_flagged_peaks(true)` reproduces that, and the corpus harness uses it for those exports. This difference is held-out finding M2 (a Q Exactive HF PRM run whose 2024 export had one more point per scan than we returned): the Q Exactive HF-X file `mtbls14308-mwy251008a-mix01` (ProteoWizard 3.0.24002) shows it on 1,590 of 1,602 scans, all exact once the flagged peaks are kept. `FLAG_EXCLUDED` is the mask 0x30: bit 0x10 marks peaks at a few fixed m/z per run in the negative-mode urine runs (80.47, 240.4), bit 0x20 the 445.12 polysiloxane ion that PXD000001's method used as lock mass; both look like reference/background ions, but that meaning is **inferred**, not known. Other observed flag bits (0x40, 0x80) do not change the converters' output. `ThermoDataset::read_scan(i, view, include_flagged = true)` keeps them. ThermoRawFileParser does the same as ProteoWizard, one step later: its 1.3.4 export `pxd032908-velos-etd-pep38` leaves the flagged centroids out (285 of the first 300 scans have them; all 1,500 compared scans are exact once they are left out), its 1.4.2 export `pxd058413-chem-iz11-6-3` keeps them (404 of 407 scans have them; all exact as stored); the corpus harness leaves them out for ThermoRawFileParser up to 1.3. Every centroid list names its flagged peaks' positions in `extra.flagged_peaks` (array indices); `spectrum --exclude-flagged` (command line only; `Dataset::exclude_flagged_peaks`) leaves them out, and `extra.flagged_peaks_left_out` counts them.

**Known difference:** in `mtbls755-hilic-dpoly`, 256 of 1420 profile MS1 scans differ from the depositor's mzML by a constant relative factor between 6·10⁻¹³ and 3·10⁻¹¹ (≤ 3·10⁻⁸ m/z at m/z 1000). The converter evaluated those scans with coefficients `B, C` equal to those of an *earlier* scan of the same polarity (e.g. scan 121 with scan 106's); we use each scan's own stored coefficients. The choice rule is not recoverable from the files (tolerant equality of events and n-significant-digit keys were tried and do not explain it). The Orbitrap Ascend file `pxd059315-ascend-etd-wkl-1` shows the same on 1,479 of its 5,663 profile MS1 scans, with factors up to 5.9·10⁻¹¹.

## Window-record scans (`kind_code` 24)

Every scan of the TSQ Vantage SRM file (v64, 10 400 scans) is stored without a `PacketHeader`. The index `data_offset` points at a `WindowRecord`, and the scan's peaks sit immediately before it:

```
peaks        n × (f32 m/z, f32 intensity)             at scan data + windows[0].peak_offset
peak_flags   n × u8                                   (0, 4, 6 observed; meaning unknown)
WindowRecord u32 lead_word (0)
             windows × AcquisitionWindow (28 bytes): f32 low_mz, f32 high_mz,
                 f64 window_value (≈2.0e-4 on every window), u32 peak_offset (first window
                 only, 0 in the others), 2 × u32 window_words (0)
             16 bytes reserved (0)
             u32 peak_bytes (8 × n)
             u32 trailing_word (0)
```

`window_record_len` = 4 + 28 × windows + 24; windows = index `data_size` − 1 = the event's `scan_ranges` count. Validated on all 10 400 scans: peaks + flags end exactly at the record (`peak_offset + 9n = data_offset`), n equals the window count, every peak lies inside a window, the f32 window bounds equal the event's f64 ranges, the sum of intensities equals the index TIC and the largest peak equals the index base peak. The depositor's mzML holds this run as 110 chromatograms, the TIC and 109 SRM traces, with no spectra. `corpus-tests` rebuilds every trace from our spectra. 107 match bit for bit: point count, times, f32 intensities. The other 3 match on every non-zero point; ProteoWizard padded them with zero-intensity points taken from a neighbouring transition (115.007→71.103 and 115.05→71.143 differ by only 0.04 in both Q1 and Q3). The window records are reported as centroided MS2 spectra; the mzML export marks them `SRM spectrum` and writes one scan window per product window. Full-scan quadrupole scans (`Q1MS`, `Q3MS`) are not in the corpus, and whether they use this layout is unknown.

## Precursor and charge

`precursor_mz` = the per-scan parameter `Monoisotopic M/Z:` when it is > 0 **and lies less than `MONOISOTOPIC_MAX_SHIFT` = 3.0 m/z from the last reaction's `precursor_mz`** (the isolation target), otherwise that target (validated: equals the mzML `selected ion m/z` / mzXML `precursorMz` on every MS2 scan of every file). The threshold comes from the Exploris files: in `mtbls13401-neg-id-01` the export takes the monoisotopic value on 5,685 scans, with distances up to 2.99996, and the target on 194 scans whose monoisotopic value lies 3.00007 or more away (4 more m/z in `mtbls14508`: 1,771 / 15 scans, 2.99729 / 3.00448); the older files have no such scans. `precursor_charge` = `Charge State:` when non-zero (validated). Decoded spectra and scan headers alike carry the last reaction's isolation window (target ± width / 2), activation and collision energy; when an event has several reactions (MS^n, multiplexed) `extra.precursors` lists every precursor m/z in filter order (exports differ in which one they name first: ProteoWizard lists them in reverse), `extra.multiplexed` marks `msx` scans, `extra.sps_masses` holds an MS3 scan's `SPS Masses` parameter and `extra.source_cid_energy` the `sid=` value.

**No charge in ion-trap and triple-quadrupole files (held-out finding L1, not reproduced as a reader gap):** the LTQ XL file `pxd059878-amrutha-050713-1` records `Charge State: 0` on all 15,265 MS2 scans and its ProteoWizard 3.0.24094 export states no charge either; TSQ Vantage SRM scans (`mtbls1822-tsq-74`) have no `Charge State:` parameter at all. A converter that reports a charge for such scans (RawConverter reports 2) assigns it itself; we report none.

## Detector controllers (UV/DAD channels, PDA field, analog channels)

Rows of the controller table whose type is not 0 are the LC's detectors and sensors, each with its own run header (the MS layout: scan range, first/last time, maximum stored value, stream addresses at 7408) followed by instrument-id texts naming the channel and device (`UV_VIS_1` / `Thermo.Vanquish.DAD` / module / serial; `3DFIELD`; `CAD_1` / `Thermo.Vanquish.CAD`; `Pump_1_Pressure` / `Thermo.Vanquish.BinaryPump`; `CC_Temp` / `Thermo.Vanquish.TCC`; the Exactive's A/D card with tags `time`, `volts`). Controller types seen: 2 analog, 3 PDA, 4 channel, 5 autosampler (no scans). Confidence: the Accela PDA layout (file version 63) is **validated** against an independent conversion (GNPS/MassIVE's msconvert mzML of `mtbls773-001-blank-start`; evidence below); the Vanquish DAD/CAD and analog layouts (versions 64/66) are **inferred**, checked by internal consistency only (see the provenance log).

**Scan index row (72 bytes, file versions 64 and 66):**

| offset | type | our name | meaning |
| --- | --- | --- | --- |
| 4 | u32 | `scan_number` | 1-based |
| 8 | u32 | `record_kind` | 10 PDA (`KIND_PDA`), 12 channel (`KIND_CHANNEL`), 13 analog (`KIND_ANALOG`) |
| 16 | u32 | `values_per_scan` | analog values per sample (2 on the Exactive A/D card, else 1) |
| 32 | f64 | `rt_min` | sample time, minutes |
| 40, 48 | f64 | `low`, `high` | PDA wavelength range (nm); 0 otherwise |
| 56 | f64 | `stored_value` | the sample (channel, single analog value) or the spectrum total (PDA, in stored units) |
| 64 | u64 | `data_offset` | into the scan-data stream |

**Before version 64** (the v63 Accela PDA file) the row is 64 bytes: u32 `data_offset` at 0 (32-bit), then the same fields as above from 4 to 63; the f64 at 24 is `sample_rate_hz` (5.0 in the PDA rows = `Scan Rate (Hz): 5`, 10.0 in the channel rows = `Channel sample rate (Hz): 10`; 0 in v64/v66 files); the channel rows' `values_per_scan` is 3 and their `stored_value` and `rt_min` are not the samples' (0, and times that advance in 0.018 s steps with 1.74 s jumps: arrival stamps, not sample times). `detector_index_len`, `DETECTOR_INDEX_LEN_32`.

**Samples:** channel — (f64 value, f64 time in minutes) per sample in v64/v66 (`sample_has_time`); before v64 `values_per_scan` f64 per sample and no time (the Accela detector's channels A, B, C: 24 bytes per sample); which of the two is taken from the first two rows' offsets (16 or 24 bytes apart for 1 or 3 values); analog — `values_per_scan` f64 per sample; PDA — `points` i32 values, then an 80-byte trailer (`PdaTrailer`) at `data_offset`: u32 1, u32 start and end nm, f64 `start_nm`, `end_nm`, `step_nm`, two f64 0, f64 `per_unit` (1,000,000), u32 `points`, u32 0, u64 `values_offset` (where the values start; = `data_offset` − 4 × `points`). Before v64 the trailer is 72 bytes (`PDA_TRAILER_LEN_32`, `pda_trailer_len`): the same up to `points`, then a u32 `values_offset`; the f64 after `step_nm` holds 200.0 there (the start wavelength again) and 0 in v66.

**Traces** (`info` → `traces[]`, in controller order; controllers without samples are left out): name = the channel name (`<name> spectra` for the PDA). UV/DAD/CAD channels and the PDA sample at the detector's fixed data rate: `sample_rate_hz` = the row's stored rate when there is one (`extra.stored_sample_rate_hz`), else (samples − 1) / time span, `start_s` = the first sample's time, `extra.axis` = {`retention_time`, `min`, first, step}; `check` verifies every recorded time lies within half an interval of that grid (`detector_time_grid`). Channels: the value (a UV/CAD channel), or one per PDA wavelength named `200 nm`, … with `extra.wavelength_nm` (values = stored integer × `1000 / per_unit`, mAU): one sample of the PDA trace is the UV spectrum at that time, one channel the chromatogram at that wavelength. Analog channels can be logged on change (the Lumos column temperature: 258 irregular samples), so they are irregular: `sample_rate_hz` 0, `extra.irregular_sampling`, channel 0 `time` (s, recorded), then one channel per value (`<name> 1`, `<name> 2` when there are two). `extra`: `detector` (`channel`, `pda`, `analog`), `controller_type`, `controller_index`, `instrument_texts` (the four id texts), `device`, `stored_maximum`, `x_start_min`, `x_end_min`, `axis` (regular traces), `irregular_sampling` (analog), `unit_source`, and for the PDA `wavelength_range_nm` [start, end, step]. A single-wavelength channel gets `wavelength_nm` from the method text (`UV.UV_VIS_1.Wavelength: 254.0 [nm]`). A channel controller with several values per sample whose method text lists as many lettered channels (`A Channel wavelength (nm): 280`, `A Channel bandwidth (nm): 9`; `lettered_channels`, `LetteredChannel`) names them `Channel A`, `Channel B`, … in stored order with `extra.wavelength_nm` and `extra.bandwidth_nm` (checked: A, B, C correlate with the PDA field's 9-nm band means at 280, 365 and 520 nm with r = 0.99998, 0.9999995, 0.99992). Older devices put the maker in the first id text (`Thermo`, `Accela PDA Detector`): the device text names the trace then (`channel_name`). Channel sample times are on the grid `start_s` + k / rate (the index times of the Accela channel rows are not sample times). **Units** are not stored: DAD channels and the PDA are reported in mAU (`unit_source` `inferred`: the Vanquish channels equal the PDA field ÷ 1000 at their wavelength, with the PDA stored in µAU; the Accela channels are stored in the PDA's own units, slope 0.995–1.010 against its band means, so they are scaled by 1/1000 (`channel_scale`, channel `scale` 0.001)); pressure and temperature channels take the unit the method text gives its limits or set points (`Pressure.UpperLimit: 1250 [bar]`, `Temperature.Nominal: 40.00 [°C]`; `unit_source` `method text`); the Exactive A/D card's unit is its id text `volts`; the CAD has none.

**Evidence** (`crates/openreadout-corpus-tests/tests/thermo_detectors.rs`): UV_VIS_2/3/8 (200, 320, 400 nm) equal the PDA column at their wavelength on all 28,800 samples within 0.00103 mAU (one storage step); UV_VIS_1 (254 nm, off the 4-nm grid) follows the mean of 252 and 256 nm (r = 0.99991); every channel/analog sample equals its index value bit for bit and every PDA spectrum sums to its stored total within one step per point (`check`); sample rates equal the method's `Data_Collection_Rate` (20 Hz DAD, 2 Hz CAD); the column temperature stays within 0.1 °C of its 40 °C set point.

**Independent reference** (`accela_pda_matches_an_independent_conversion` in the same test file; oracle `corpus/oracle/mtbls773-001-blank-start.json` → `detector_export`, made by `oracle/thermo_detectors.py` with pyteomics from GNPS's msconvert mzML; we did not run the converter): all 10,501 PDA spectra × 401 wavelengths equal the export's stored integers (4,210,901 of 4,210,901 values) and its spectrum times (within 3·10⁻¹³ min); the export's `UV 1` chromatogram equals channel A on all 21,001 samples; its `PDA 1` chromatogram is each spectrum's total ÷ 401. The export times `UV 1` one channel interval (0.1 s) later than our grid (first sample at 0.1 s, last at 35.0017 min, past the run header's end time of 35.0 min). We keep the first sample at the run header's start: interpolating channel A to the PDA spectrum times and fitting it to the 280/365 nm band means leaves an RMS residual of 569/97.6 (stored units) on our grid and 1828/1941 with the export's +0.1 s shift.

`check` adds `detector_value_mismatch` (error), `detector_time_order` and `detector_time_grid` (warnings) and `detector_unreadable` (error); a controller that cannot be opened is a `detector` warning and is left out. The value comparison is skipped for value-only channels (their index holds 0).

## Vocabulary (every public identifier in `openreadout-thermo`)

| identifier | meaning |
| --- | --- |
| `ThermoRawReader`, `ThermoDataset`, `FORMAT_ID`, `SIGNATURE`, `SUPPORTED_VERSIONS`, `open` | entry points |
| `FILE_HEADER_LEN`, `CHECKSUM_OFFSET`, `CHECKSUM_SPAN`, `adler32`, `header_checksum` | file header size and integrity checksum |
| `FileHeader`, `version`, `header_words`, `created`, `modified`, `header_value`, `header_text`, `has_signature`, `parse_file_header` | file header |
| `AuditStamp`, `filetime`, `account`, `account_2`, `stamp_value`, `iso`, `filetime_iso` | audit stamps |
| `SequenceRow`, `injection`, `texts`, `row_value`, `sample_name`, `sample_id`, `comment`, `user_labels`, `instrument_method`, `processing_method`, `original_file_name`, `original_path`, `vial`, `parse_sequence_row`, `sequence` | sequence row |
| `InjectionRecord`, `injection_words`, `row_number`, `vial_label`, `injection_volume`, `sample_weight`, `sample_volume`, `internal_standard_amount`, `dilution_factor` | injection record |
| `AutosamplerInfo`, `numbers`, `tray_description`, `vial_index`, `parse_autosampler`, `autosampler` | autosampler block |
| `FileInfoBlock`, `method_file_present`, `utc_time`, `utc_time_iso`, `utc_unix_seconds`, `filetime_unix_seconds`, `data_address`, `run_header_address`, `run_header_address_2`, `controller_count`, `label_headings`, `computer_name`, `file_info_binary_len`, `parse_file_info`, `file_info` | file-info block |
| `ControllerRef`, `controllers`, `controller_type`, `controller_index`, `ms_run_header_address` | data-controller table |
| `Detector`, `controller`, `kind`, `id`, `scan_count`, `start_min`, `end_min`, `max_value`, `scan_index_address`, `scan_data_address`, `values_per_scan`, `sample_has_time`, `sample_rate_hz`, `version`, `pda`, `channel_name`, `channel_scale`, `step_min`, `detectors` | a detector controller (UV channel, PDA, analog) |
| `DetectorKind` { `Channel`, `Pda`, `Analog`, `NoData` }, `name` | what a detector records |
| `DetectorRow`, `scan_number`, `record_kind`, `values_per_scan`, `sample_rate_hz`, `rt_min`, `low`, `high`, `stored_value`, `data_offset`, `parse_detector_row`, `DETECTOR_INDEX_LEN`, `DETECTOR_INDEX_LEN_32`, `detector_index_len` | detector scan-index row |
| `KIND_PDA`, `KIND_CHANNEL`, `KIND_ANALOG` | record kinds 10, 12, 13 |
| `PdaTrailer`, `start_nm`, `end_nm`, `step_nm`, `per_unit`, `points`, `values_offset`, `parse_pda_trailer`, `PDA_TRAILER_LEN`, `PDA_TRAILER_LEN_32`, `pda_trailer_len` | PDA spectrum trailer |
| `open_detectors`, `read_rows`, `read_samples`, `unit_scale`, `wavelengths`, `trace_info`, `channel_wavelength` | reading detector controllers |
| `LetteredChannel`, `letter`, `wavelength_nm`, `bandwidth_nm`, `lettered_channels` | lettered channels of an older PDA detector's method text (`A Channel wavelength (nm): 280`) |
| `RunHeader`, `address`, `sample_words`, `first_scan`, `last_scan`, `instrument_log_count`, `error_log_count`, `max_total_ion_current`, `low_mz`, `high_mz`, `start_time_min`, `end_time_min`, `sample_texts`, `device_files`, `run_values`, `scan_event_count`, `scan_parameter_count`, `segment_count`, `display_words`, `filter_mass_decimals`, `streams`, `own_address`, `byte_len`, `scan_count`, `parse_run_header`, `run_header` | run header |
| `StreamAddresses`, `scan_index`, `scan_data`, `instrument_log`, `error_log`, `scan_events`, `scan_parameters` | stream addresses |
| `InstrumentId`, `id_words`, `model`, `model_2`, `serial_number`, `software_version`, `tags`, `parse_instrument_id`, `instrument` | instrument id |
| `GenericField`, `kind`, `width`, `label`, `GenericHeader`, `fields`, `record_len`, `decode`, `parse_generic_header`, `MAX_GENERIC_FIELDS`, `instrument_log_header`, `scan_parameter_header` | self-describing record columns |
| `ErrorLogEntry`, `rt_min`, `message`, `parse_error_entry`, `errors`, `error_log_lead` | error log |
| `MethodTable`, `templates`, `ScanTemplate`, `segment`, `event`, `template_words`, `scan_event`, `template_event`, `parse_method_table`, `method_table` | segment/event table |
| `ScanIndexEntry`, `data_offset`, `event_number`, `segment_number`, `next_scan`, `kind_code`, `data_size`, `total_ion_current`, `base_peak_intensity`, `base_peak_mz`, `scan_index_entry_len`, `parse_scan_index_entry`, `index` | scan index |
| `ScanEvent`, `preamble`, `reactions`, `scan_ranges`, `scan_range`, `coefficients`, `tail_items`, `tail_words`, `tail_text`, `preamble_len`, `parse_scan_event`, `events`, `precursor_reactions` | scan events |
| `Reaction`, `precursor_mz`, `isolation_width`, `energy`, `reaction_words`, `reaction_values` | MS^n stage |
| `POLARITY_BYTE`, `DATA_KIND_BYTE`, `MS_LEVEL_BYTE`, `SCAN_KIND_BYTE`, `SCAN_RATE_BYTE`, `DEPENDENT_BYTE`, `IONIZATION_BYTE`, `BRACE_BYTE`, `ANALYZER_BYTE`, `LOCK_BYTE`, `SUPPLEMENTAL_ACTIVATION_BYTE` | preamble byte positions |
| `is_multiplex`, `source_cid_energy`, `scan_rate_token`, `faims_cv`, `SOURCE_CID_BYTE`, `FAIMS_BYTE` | multiplexed events, in-source CID energy, scan-rate filter token, FAIMS compensation voltage, and the preamble bytes that turn the last two on |
| `polarity`, `is_profile`, `ms_level`, `scan_kind_code`, `is_dependent`, `ionization`, `activation`, `analyzer`, `filter_text`, `fixed_decimals`, `scan_kind_token`, `filter_token`, `from_code`, `from_reaction_code`, `word`, `sign`, `BRACE_BYTE`, `LOCK_BYTE` | preamble accessors and filter composition |
| `ScanRates`, `Ltq`, `Tribrid`, `for_model`, `scan_rate_token_for`, `filter_text_for`, `scan_rates` | how an instrument generation numbers its ion-trap scan rates, and the filter composed with it |
| `Polarity` { `Negative`, `Positive`, `Unknown` } | ion polarity |
| `Analyzer` { `IonTrap`, `TripleQuadrupole`, `SingleQuadrupole`, `TimeOfFlight`, `FourierTransform`, `Sector`, `Astral`, `Unknown` } | analyzer (filter tokens `ITMS`, `TQMS`, `SQMS`, `TOFMS`, `FTMS`, `Sector`, `ASTMS`) |
| `Ionization` { `ElectronImpact`, `ChemicalIonization`, `FastAtomBombardment`, `Electrospray`, `AtmosphericPressureChemical`, `Nanospray`, `Thermospray`, `FieldDesorption`, `Maldi`, `GlowDischarge`, `Unknown` } | ion source (filter tokens `EI`, `CI`, `FAB`, `ESI`, `APCI`, `NSI`, `TSP`, `FD`, `MALDI`, `GD`) |
| `Activation` { `BeamCollision`, `TrapCollision`, `ElectronTransfer`, `Unknown` } | fragmentation (filter tokens `hcd`, `cid`, `etd`) |
| `PacketHeader`, `header_value`, `profile_words`, `centroid_words`, `layout_flags`, `descriptor_words`, `extra_words`, `triplet_words`, `annotation_words`, `packet_len`, `segments`, `extra_range_words`, `MAX_SEGMENTS`, `chunks_have_correction`, `PACKET_HEADER_LEN`, `parse_packet_header` | packet header |
| `Packet`, `header`, `profile`, `centroids`, `descriptors`, `segment_ranges`, `parse_packet`, `packet`, `window_record`, `peak_flags` | decoded packet |
| `WindowRecord`, `lead_word`, `windows`, `reserved`, `peak_bytes`, `trailing_word`, `AcquisitionWindow`, `low_mz`, `high_mz`, `window_value`, `peak_offset`, `window_words`, `WINDOW_RECORD_KIND`, `WINDOW_LEN`, `WINDOW_PEAK_LEN`, `MAX_WINDOWS`, `window_record_len`, `parse_window_record`, `parse_window_peaks`, `scan_extent` | window-record (SRM) scans |
| `Profile`, `first_value`, `step`, `bin_count`, `chunks`, `ProfileChunk`, `first_bin`, `mz_correction`, `values` | profile |
| `Centroid`, `mz`, `intensity`, `PeakDescriptor`, `peak_index`, `flags`, `descriptor_byte`, `FLAG_EXCLUDED` | centroids and descriptors |
| `MzScale` { `Direct`, `InverseSquare`, `Inverse` }, `from_event`, `render_profile`, `PROFILE_PAD_BINS`, `MONOTONIC_NUDGE` | profile m/z conversion and rendering |
| `ScanSummary`, `scan_number`, `rt_s`, `scan_filter`, `data_len`, `scans`, `read_scan`, `scan_header`, `scan_parameters`, `set_exclude_flagged_peaks`, `MONOISOTOPIC_MAX_SHIFT` | per-scan summaries, headers (`scans`: index row, event and trailer; no packet read) and reads |
| `MethodDocument`, `container_offset`, `container_size`, `source_path`, `devices`, `streams`, `device_texts`, `method` | embedded instrument method |
| `Gradient`, `GradientStep`, `device`, `solvents`, `steps`, `time_min`, `flow_ul_min`, `percent`, `to_json`, `from_device_texts`, `full_model` | LC pump program from the method text; full model name |
| `CompoundFile`, `CfbEntry`, `CFB_SIGNATURE`, `path`, `entry_kind`, `stream_size`, `entries`, `parse`, `read` | compound-file container reader |
| `Cursor`, `new`, `offset`, `position`, `remaining`, `seek_to`, `take`, `skip`, `u8`, `u16`, `i16`, `u32`, `i32`, `u64`, `f32`, `f64`, `utf16_fixed`, `utf16_counted`, `utf16_z`, `read_at`, `MAX_TEXT_UNITS` | bounds-checked byte reading |
