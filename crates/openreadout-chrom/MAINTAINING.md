# Maintaining `openreadout-chrom`

Chromatography data, one family of modules per format: Agilent ChemStation `.D` directories and `.ch`/`.uv`/`.ms` files (`chemstation`, `chemstation_*.rs`), Agilent OpenLab CDS `.dx` injections with `.rx` result packages (`openlab-cds`, `openlab_*.rs`), AIA/ANDI netCDF (`andi-chrom`, `andi_dataset.rs` + `netcdf.rs`), Shimadzu LabSolutions `.lcd`/`.gcd` (`shimadzu`, `shimadzu_*.rs`), Thermo Scientific Chromeleon 7 archives `.cmbx` (`chromeleon`, `chromeleon*.rs`) and Waters Empower ASCII exports `.arw` (`empower-arw`, `empower_arw.rs`). This is the largest crate (≈ 17 600 lines). Project-wide process: [docs/maintaining.md](../../docs/maintaining.md). Notes and provenance: `docs/formats/<format id>.md`, `docs/provenance/<format id>.md`.

## Decode pipeline

Shared: `binary.rs` (Pascal strings, bounded whole-file reads, date and number formatting), `openreadout_core::bytes` (bounds-checked big- and little-endian fields), `openreadout_core::zip` (`.dx`/`.rx`/`.cmbx`), `openreadout_core::cfb` (Shimadzu compound files).

- **ChemStation** (`chemstation_header.rs`, `chemstation_decode.rs`, `chemstation_dataset.rs`): the version string at byte 0 (`version_of`) decides kind and body encoding — **file versions are the main branch**: 2 (`.ms`), 30, 81/181 (`.ch`, delta records or second differences), 130/131 (`.uv`), 179 (float64); others listed, not decoded. Headers at fixed offsets (strings, dates, scaling, wavelengths). Bodies decode to raw values with a note of how they ended (`BodyEnd`). A `.D` directory adds `acqmeth.txt` (`chemstation_method.rs`) and vendor peak reports (`chemstation_report.rs`: `Result.xml`, `Report.TXT`, `RESULTS.CSV`).
- **OpenLab CDS** (`openlab_xml.rs`, `openlab_dataset.rs`): the `.dx` zip holds `injection.acmd` (manifest), one version-179 signal part per detector channel (`<guid>.CH`) and instrument curves (`<guid>.IT`); the `.rx` package holds the vendor's results; the result-set folder's `.acaml` gives the sequence. Namespaces `acmd20`/`acaml21` are matched by local name.
- **ANDI** (`netcdf.rs`, `andi_dataset.rs`): a netCDF classic (CDF-1/CDF-2) reader written from Unidata's public specification; the chromatography template (`ordinate_values`, peak table) and the mass-spectrometry template (`scan_index`, `point_count`, `mass_values`, `intensity_values`).
- **Shimadzu** (`shimadzu_dataset.rs`, `shimadzu_signal.rs`, `shimadzu_channels.rs`, `shimadzu_peaks.rs`, `shimadzu_tlm.rs`): a compound file; **two layouts branch everywhere**: older `LC Raw Data` (64-byte channel status records, 128-byte module records) and newer `LSS Raw Data` (`2D Data Item` XML with `DT`/`DK`/`ATN`); 2-D `RC` signal streams and the 3-D PDA stream; LabSolutions' own peak tables; LC-MS records (`TLM Raw Data`: zlib records with MRM/SIM triples and profiles).
- **Chromeleon** (`chromeleon.rs`, `chromeleon_results.rs`, `chromeleon_dataset.rs`): a zip of `header.xml` (archived items), the sequence file (a protocol-buffers stream describing each signal: unit, range, point count, device, scale), one `.raw` member per signal (sections `SignHdr` and difference-coded `PtsLDiff` blocks; LL2 and DDCMP encodings), stored integration results and LZMA2 blobs (methods, audit trails; decoded by `openreadout-codecs`).
- **Empower** (`empower_arw.rs`): quoted field names, their values, then `time<TAB>value` rows.

## Invariants and checks

- ChemStation: version string known; header inside the file; bodies decoded to the end (record tags, escapes, truncation); point count against the header's retention-time range; `.uv`/`.ms` scan records walked and counted; vendor peak retention times on our signal's local maxima.
- OpenLab / Chromeleon: zip central directory and every member's CRC-32; manifests/headers parsed and every listed signal matched; signals decoded to their declared counts, scales and time ranges; result peaks inside the signal's time range.
- ANDI: netCDF header, every variable inside the file, template variables present, point counts and monotonic scan times.
- Shimadzu: compound-file structures (FAT/DIFAT, mini FAT, directory), every stream readable; signals decoded to their point counts; the PDA maximum equals the stored max plot; vendor peak areas against our integration; LC-MS records against the TIC.

## Debugging a new file

- `openreadout ls FILE` lists compound-file storages (Shimadzu), zip members (OpenLab, Chromeleon), netCDF variables (ANDI) or the `.D` directory's files; `dump --json` → `vendor` has the parsed headers and XML.
- Each module holds unit tests with minimal byte layouts of every variant seen (`chemstation_header.rs` `versions`, `shimadzu_channels.rs` `older_layout_labels`/`newer_layout_labels`/`chromatogram_items_with_dt_48`, `chromeleon.rs` `second_difference_pairs`, `openlab_dataset.rs` `malformed_containers_are_clean_errors`). There are no integration tests: corpus files pin the rest.
- Ground truth: the vendor's own ASCII/AIA exports as `oracle-export` (`oracle/shimadzu_export.py`, `oracle/chromeleon_oracle.py`, `oracle/empower_arw_oracle.py`, `oracle/openlab_cds.py`), rainbow-api for ChemStation (`uv run --group chrom`).

## Fragile spots

- **Shimadzu channel identification** (`shimadzu_channels.rs`): `DT` 52 vs 48 across LabSolutions versions has already caused a regression and a revert (git history, 2026-09-26); the export title (`ATN`) is the tie-breaker. Detector scaling factors come from the channel status or `CF` and were the cause of a held-out failure (docs/assurance.md).
- ChemStation versions outside the list are refused; `.ch` above 512 MiB is refused (read whole).
- Chromeleon: the protocol-buffers field numbers are inferred from archives; stored results and audit blobs depend on the LZMA2 decoder in `openreadout-codecs`.
- Several modules exceed 1 500 lines (`chromeleon_dataset.rs`, `openlab_dataset.rs`, `shimadzu_dataset.rs`, `chemstation_dataset.rs`); split by format already, but a new format should get its own module family rather than grow a shared one.

<!-- BEGIN GENERATED guide -->
## Facts (generated)

*Generated by `cargo xtask guides --write` from the sources, `corpus/manifest.toml`, `corpus/assurance/evidence.json` and `corpus/intake/`; do not edit. CI fails when it is stale.*

### Formats

| format id | notes and provenance | confidence | basis | development files: read / confirmed | depositors | held-out pass / fail |
| --- | --- | --- | --- | --- | --- | --- |
| `chemstation` | [format note](../../docs/formats/chemstation.md), [provenance log](../../docs/provenance/chemstation.md) | high | prior art | 28 / 28 | 9 | 3 / 0 |
| `openlab-cds` | [format note](../../docs/formats/openlab-cds.md), [provenance log](../../docs/provenance/openlab-cds.md) | medium | reverse engineered | 12 / 12 | 3 | - |
| `andi-chrom` | [format note](../../docs/formats/andi-chrom.md), [provenance log](../../docs/provenance/andi-chrom.md) | high | open spec | 25 / 25 | 5 | 1 / 0 |
| `empower-arw` | [format note](../../docs/formats/empower-arw.md), [provenance log](../../docs/provenance/empower-arw.md) | low | reverse engineered | 6 / 6 | 1 | - |
| `shimadzu` | [format note](../../docs/formats/shimadzu.md), [provenance log](../../docs/provenance/shimadzu.md) | medium | reverse engineered | 12 / 9 | 4 | 0 / 1 |
| `chromeleon` | [format note](../../docs/formats/chromeleon.md), [provenance log](../../docs/provenance/chromeleon.md) | medium | reverse engineered | 12 / 2 | 2 | - |

### Source map

| file | what it does (its module documentation) |
| --- | --- |
| [`src/andi_dataset.rs`](src/andi_dataset.rs) | `Dataset` for AIA/ANDI netCDF files: the chromatography template (one trace from `ordinate_values`, the peak table as a table) and the mass-spectrometry template (one spectra |
| [`src/assurance.rs`](src/assurance.rs) | Assurance profiles (`docs/assurance.md`) of the chromatography readers (Agilent ChemStation and OpenLab CDS, ANDI netCDF, Shimadzu LabSolutions): the variant features of a data… |
| [`src/binary.rs`](src/binary.rs) | Length-prefixed strings, bounded whole-file reads, and the date and number formatting shared by the chromatography readers |
| [`src/chemstation_dataset.rs`](src/chemstation_dataset.rs) | `Dataset` for Agilent ChemStation `.D` directories and single `.ch`/`.uv`/`.ms` files: one trace per `.ch` signal, one multi-channel trace (one channel per wavelength) per `.uv` |
| [`src/chemstation_decode.rs`](src/chemstation_decode.rs) | Decoders for ChemStation signal bodies |
| [`src/chemstation_header.rs`](src/chemstation_header.rs) | ChemStation signal-file headers: version, body encoding, strings and numeric fields at fixed offsets |
| [`src/chemstation_method.rs`](src/chemstation_method.rs) | The method report ChemStation writes into a `.D` directory as `acqmeth.txt` (`docs/formats/chemstation.md` § Acquisition method text): method path, GC oven program, |
| [`src/chemstation_report.rs`](src/chemstation_report.rs) | Peak reports ChemStation writes into a `.D` directory (`docs/formats/chemstation.md` § Vendor peak reports): `Result.xml` (LC/GC ChemStation's XML export: every integrated peak |
| [`src/chromeleon.rs`](src/chromeleon.rs) | Thermo Scientific Chromeleon 7 archives (`.cmbx`): the parsers |
| [`src/chromeleon_dataset.rs`](src/chromeleon_dataset.rs) | `Dataset` for Thermo Scientific Chromeleon 7 archives (`.cmbx`): one trace per archived 2D signal (every injection of the sequence, every channel), described by the sequence file… |
| [`src/chromeleon_results.rs`](src/chromeleon_results.rs) | Chromeleon 7 sequence files (`.cmd`) beyond the signal descriptions: the object graph (catalog records 18, data records 19), injection details, processing-method components, |
| [`src/empower_arw.rs`](src/empower_arw.rs) | Waters Empower ASCII raw-data exports (`.arw`): what Empower writes when a result's raw data is exported as ASCII — a row of quoted field names the export method chose… |
| [`src/lib.rs`](src/lib.rs) | Readers for chromatography data |
| [`src/netcdf.rs`](src/netcdf.rs) | A minimal reader for the netCDF classic format (CDF-1) and its 64-bit-offset variant (CDF-2): header (dimensions, attributes, variables) and variable data, fixed-size and record |
| [`src/openlab_dataset.rs`](src/openlab_dataset.rs) | `Dataset` for Agilent OpenLab CDS injections: a `.dx` container (zip) holding the injection manifest (`injection.acmd`), one ChemStation-style version-179 signal part per detector |
| [`src/openlab_xml.rs`](src/openlab_xml.rs) | OpenLab CDS XML parts: the injection manifest (`injection.acmd` inside a `.dx`), the injection results (`Base/InjectionACAML` inside a `.rx`) and the sequence file (`.acaml` in a |
| [`src/shimadzu_channels.rs`](src/shimadzu_channels.rs) | What a Shimadzu `Chromatogram ChN` stream is: the detector it comes from, the name the vendor's ASCII export gives it, and the factor from stored integers to the export's units |
| [`src/shimadzu_dataset.rs`](src/shimadzu_dataset.rs) | `Dataset` for Shimadzu LabSolutions `.lcd`/`.gcd` files: the compound-file structure (every storage and stream with its size), the text found in the `File Property` stream, and the |
| [`src/shimadzu_peaks.rs`](src/shimadzu_peaks.rs) | LabSolutions' own peak tables (`docs/formats/shimadzu.md` § Vendor peaks; provenance 2026-09-26) |
| [`src/shimadzu_signal.rs`](src/shimadzu_signal.rs) | Shimadzu LabSolutions signal streams: the 2-D `RC` layout (`Chromatogram ChN`, `Max Plot`) and the 3-D PDA stream (one such record per time point) |
| [`src/shimadzu_tlm.rs`](src/shimadzu_tlm.rs) | LC-MS data of LabSolutions `.lcd` files (`TLM Raw Data`; `docs/formats/shimadzu.md` § LC-MS) |

### Where variants branch

The assurance profile ([`src/assurance.rs`](src/assurance.rs)) observes these features (each value is looked up in the validated table below; a value never confirmed makes the file `unvalidated` for the feature's outputs) and reports these structures and assumptions:

- feature layout `if regular { "2D, evenly spaced times" } else { "2D, uneven times" }`
- feature dialect `format!("{e} line endings")`
- feature writer_version `format!("Chromeleon {v}")`
- feature codec `e`
- feature layout `format!("{} Hz {kind} in {unit}", t.sample_rate_hz)`
- feature layout `format!("irregular {kind} in {unit}")`
- feature format_version `format!("stored results {v}")`
- feature format_version `format!("signal {v}")`
- feature format_version `format!("MS {v}")`
- feature writer `a::writer_name_only(s)`
- feature record `"vendor results (.rx)"`
- feature format_version `v`
- feature format_version `"no template revision"`
- feature layout `"chromatography template"`
- feature layout `"mass-spectrometry template"`
- feature dialect `"netCDF without ANDI attributes"`
- feature layout `format!("{kind} in {storage}")`
- feature record `"vendor peak table"`
- feature acquisition `format!("LC-MS {name}")`
- feature instrument `m` (descriptive)
- feature acquisition `f.to_ascii_uppercase()` (descriptive)
- feature instrument `i` (descriptive)
- feature writer_version `format!("ChemStation {s}")` (descriptive)
- feature writer_version `format!("OpenLab CDS {v}")` (descriptive)
- feature record `format!("unit {u}")` (descriptive)
- feature instrument `d` (descriptive)
- feature acquisition `format!("LC-MS {name}")` (descriptive)
- undecoded "stored integration results"
- undecoded "embedded MS data"
- undecoded "signal units"
- undecoded "signal files of unknown versions"
- undecoded "truncated signal body"
- undecoded "result package"
- undecoded "signals without a part"
- undecoded "short signal parts"
- undecoded "unmapped netCDF variables"
- undecoded "scan variables"
- undecoded "LC-MS profile spectra (full and product-ion scans)"
- undecoded "raw-data storages"
- assumed "traces[].channels[].unit"
- calibration "stored integers to signal units"
- calibration "detector integers to signal units"

### Validated variants and the corpus files that pin them

| format | kind | value | outputs | confirmed files | read | example corpus files |
| --- | --- | --- | --- | --- | --- | --- |
| `andi-chrom` | field | `experiment.acquisition.started_at` | descriptive | 24 | 24 | `cheminfo-agilent-gcms-cdf`, `cheminfo-agilent-hplc-cdf`, `mtbls1892-b01-pda-ch1-cdf` |
| `andi-chrom` | field | `experiment.instrument.model` | descriptive | 2 | 2 | `cheminfo-agilent-gcms-cdf`, `zenodo7729413-sla-8-cdf` |
| `andi-chrom` | format_version | `1` | metadata, spectra, traces | 1 | 1 | `mtbls390-wb-cc-bat-01-cdf` |
| `andi-chrom` | format_version | `1.0` | metadata, spectra, traces | 2 | 2 | `cheminfo-agilent-hplc-cdf`, `sciformats-andi-chrom-valid-cdf` |
| `andi-chrom` | format_version | `1.0.1` | metadata, spectra, traces | 22 | 22 | `cheminfo-agilent-gcms-cdf`, `mtbls1892-b01-pda-ch1-cdf`, `mtbls1892-b01-pda-ch2-cdf` |
| `andi-chrom` | layout | `chromatography template` | traces | 4 | 23 | `cheminfo-agilent-hplc-cdf`, `mtbls1892-b20-pda-ch1-cdf`, `mtbls390-wb-cc-bat-01-cdf` |
| `andi-chrom` | layout | `mass-spectrometry template` | spectra, traces | 2 | 2 | `cheminfo-agilent-gcms-cdf`, `zenodo7729413-sla-8-cdf` |
| `andi-chrom` | record | `unit Arbitrary Intensity Units` | descriptive | 2 | 2 | `cheminfo-agilent-gcms-cdf`, `zenodo7729413-sla-8-cdf` |
| `andi-chrom` | record | `unit Volts` | descriptive | 20 | 20 | `mtbls1892-b01-pda-ch1-cdf`, `mtbls1892-b01-pda-ch2-cdf`, `mtbls1892-b07-pda-ch1-cdf` |
| `andi-chrom` | record | `unit au` | descriptive | 1 | 1 | `sciformats-andi-chrom-valid-cdf` |
| `andi-chrom` | record | `unit mAU` | descriptive | 1 | 1 | `cheminfo-agilent-hplc-cdf` |
| `andi-chrom` | record | `unit mVolts` | descriptive | 1 | 1 | `mtbls390-wb-cc-bat-01-cdf` |
| `chemstation` | acquisition | `GC / MS DATA FILE` | descriptive | 14 | 14 | `chromhandler-rau-r505-00-data-ms`, `chromhandler-rau-r505-01-d-data-ms`, `chromhandler-rau-r505-02-d-data-ms` |
| `chemstation` | acquisition | `GC DATA FILE` | descriptive | 9 | 9 | `chemplexity-011f0601-fid1a-ch`, `chromhandler-001f0101-d`, `chromhandler-001f0102-d` |
| `chemstation` | acquisition | `LC DATA FILE` | descriptive | 5 | 5 | `autolab-001-p1-a1-a1-d`, `chromhandler-ca10-100um-d`, `entab-carotenoid-extract-d` |
| `chemstation` | acquisition | `MSD SPECTRAL FILE` | descriptive | 2 | 2 | `autolab-001-p1-a1-a1-d`, `entab-carotenoid-extract-d` |
| `chemstation` | codec | `delta_records` | traces | 4 | 4 | `autolab-001-p1-a1-a1-d`, `chromhandler-ca10-100um-d`, `entab-chemstation-mwd-d` |
| `chemstation` | codec | `float64` | traces | 2 | 2 | `entab-test-179-fid-ch`, `gc2asm-three-channels-d` |
| `chemstation` | codec | `mass_spectrum_records` | spectra | 16 | 16 | `autolab-001-p1-a1-a1-d`, `chromhandler-rau-r505-00-data-ms`, `chromhandler-rau-r505-01-d-data-ms` |
| `chemstation` | codec | `second_difference` | traces | 7 | 7 | `chemplexity-011f0601-fid1a-ch`, `chromhandler-001f0101-d`, `chromhandler-001f0102-d` |
| `chemstation` | codec | `spectrum_records` | traces | 2 | 2 | `entab-carotenoid-extract-d`, `zhulong-001-1-sm-d` |
| `chemstation` | field | `experiment.acquisition.started_at` | descriptive | 28 | 28 | `autolab-001-p1-a1-a1-d`, `chemplexity-011f0601-fid1a-ch`, `chromhandler-001f0101-d` |
| `chemstation` | field | `experiment.instrument.model` | descriptive | 21 | 21 | `autolab-001-p1-a1-a1-d`, `chemplexity-011f0601-fid1a-ch`, `chromhandler-ca10-100um-d` |
| `chemstation` | format_version | `MS 2` | metadata, spectra | 16 | 16 | `autolab-001-p1-a1-a1-d`, `chromhandler-rau-r505-00-data-ms`, `chromhandler-rau-r505-01-d-data-ms` |
| `chemstation` | format_version | `signal 130` | metadata, traces | 2 | 2 | `autolab-001-p1-a1-a1-d`, `zhulong-001-1-sm-d` |
| `chemstation` | format_version | `signal 131` | metadata, traces | 2 | 2 | `entab-carotenoid-extract-d`, `zhulong-001-1-sm-d` |
| `chemstation` | format_version | `signal 179` | metadata, traces | 2 | 2 | `entab-test-179-fid-ch`, `gc2asm-three-channels-d` |
| `chemstation` | format_version | `signal 181` | metadata, traces | 5 | 5 | `chromhandler-001f0101-d`, `chromhandler-001f0102-d`, `chromhandler-001f0103-d` |
| `chemstation` | format_version | `signal 30` | metadata, traces | 2 | 2 | `chromhandler-ca10-100um-d`, `entab-chemstation-mwd-d` |
| `chemstation` | format_version | `signal 81` | metadata, traces | 2 | 2 | `chemplexity-011f0601-fid1a-ch`, `entab-test-fid-ch` |
| `chemstation` | instrument | `DAD1` | descriptive | 1 | 1 | `zhulong-001-1-sm-d` |
| `chemstation` | instrument | `G1315B` | descriptive | 2 | 2 | `chromhandler-ca10-100um-d`, `entab-carotenoid-extract-d` |
| `chemstation` | instrument | `G1365B` | descriptive | 1 | 1 | `entab-chemstation-mwd-d` |
| `chemstation` | instrument | `GCI` | descriptive | 9 | 9 | `autolab-001-p1-a1-a1-d`, `chromhandler-001f0101-d`, `chromhandler-001f0102-d` |
| `chemstation` | instrument | `HP G1530A` | descriptive | 2 | 2 | `chemplexity-011f0601-fid1a-ch`, `entab-test-fid-ch` |
| `chemstation` | writer_version | `ChemStation 1` | descriptive | 2 | 2 | `entab-test-179-fid-ch`, `gc2asm-three-channels-d` |
| `chemstation` | writer_version | `ChemStation 3` | descriptive | 4 | 4 | `chromhandler-001f0101-d`, `chromhandler-001f0102-d`, `chromhandler-001f0103-d` |
| `chemstation` | writer_version | `ChemStation 5` | descriptive | 1 | 1 | `gc2asm-v181-d` |
| `chemstation` | writer_version | `ChemStation 6` | descriptive | 1 | 1 | `zhulong-001-1-sm-d` |
| `chemstation` | writer_version | `ChemStation 7` | descriptive | 1 | 1 | `autolab-001-p1-a1-a1-d` |
| `chromeleon` | codec | `3DRawSpc` | traces | 0 | 2 |  |
| `chromeleon` | codec | `PtsDDCmp` | traces | 0 | 1 |  |
| `chromeleon` | codec | `PtsLDiff` | traces | 2 | 11 | `cmbx-figshare-milks-sugars`, `cmbx-lauterbach-invivo-cascade` |
| `chromeleon` | codec | `PtsLL2Df` | traces | 0 | 1 |  |
| `chromeleon` | format_version | `stored results 1` | tables | 1 | 7 | `cmbx-lauterbach-invivo-cascade` |
| `chromeleon` | format_version | `stored results 2` | tables | 0 | 3 |  |
| `chromeleon` | instrument | `Acquity.dll` | descriptive | 0 | 1 |  |
| `chromeleon` | instrument | `DAD3000.dll` | descriptive | 0 | 1 |  |
| `chromeleon` | instrument | `DC-6000` | descriptive | 1 | 2 | `cmbx-figshare-milks-sugars` |
| `chromeleon` | instrument | `ICS-6000 SP` | descriptive | 1 | 2 | `cmbx-figshare-milks-sugars` |
| `chromeleon` | instrument | `PumpLPG3X00RS.dll` | descriptive | 0 | 1 |  |
| `chromeleon` | instrument | `Thermo Scientific Trace GC` | descriptive | 1 | 7 | `cmbx-lauterbach-invivo-cascade` |
| `chromeleon` | instrument | `Thermo.MassSpectrometer` | descriptive | 0 | 1 |  |
| `chromeleon` | layout | `1 Hz signal in mL/min` | traces | 0 | 1 |  |
| `chromeleon` | layout | `10 Hz signal in bar` | traces | 0 | 1 |  |
| `chromeleon` | layout | `100 Hz signal in bar` | traces | 0 | 1 |  |
| `chromeleon` | layout | `16 Hz signal in psi` | traces | 1 | 2 | `cmbx-figshare-milks-sugars` |
| `chromeleon` | layout | `2 Hz signal in nC` | traces | 1 | 2 | `cmbx-figshare-milks-sugars` |
| `chromeleon` | layout | `20 Hz 3D field in mAU` | traces | 0 | 1 |  |
| `chromeleon` | layout | `20 Hz signal in mAU` | traces | 0 | 1 |  |
| `chromeleon` | layout | `25 Hz 3D field in mAU` | traces | 0 | 1 |  |
| `chromeleon` | layout | `25 Hz signal in mAU` | traces | 0 | 1 |  |
| `chromeleon` | layout | `50 Hz signal in mV` | traces | 1 | 7 | `cmbx-lauterbach-invivo-cascade` |
| `chromeleon` | layout | `irregular signal in counts` | traces | 0 | 1 |  |
| `chromeleon` | writer_version | `Chromeleon 7.2` | metadata, traces | 1 | 10 | `cmbx-lauterbach-invivo-cascade` |
| `chromeleon` | writer_version | `Chromeleon 7.3` | metadata, traces | 1 | 2 | `cmbx-figshare-milks-sugars` |
| `empower-arw` | dialect | `CR line endings` | traces | 6 | 6 | `appia-empower-results1844`, `appia-empower-results1845`, `appia-empower-results1848` |
| `empower-arw` | layout | `2D, evenly spaced times` | traces | 6 | 6 | `appia-empower-results1844`, `appia-empower-results1845`, `appia-empower-results1848` |
| `openlab-cds` | codec | `InstrumentTrace179` | traces | 6 | 6 | `allotropy-openlab-luxo-01`, `allotropy-openlab-luxo-03`, `allotropy-openlab-sirius-01` |
| `openlab-cds` | codec | `Signal179` | traces | 12 | 12 | `allotropy-openlab-luxo-01`, `allotropy-openlab-luxo-03`, `allotropy-openlab-sirius-01` |
| `openlab-cds` | codec | `Spectra131` | traces | 2 | 2 | `cct-openlab-meoh1`, `cct-openlab-sirslt-norbert` |
| `openlab-cds` | field | `experiment.acquisition.started_at` | descriptive | 12 | 12 | `allotropy-openlab-luxo-01`, `allotropy-openlab-luxo-03`, `allotropy-openlab-sirius-01` |
| `openlab-cds` | field | `experiment.instrument.model` | descriptive | 4 | 4 | `allotropy-openlab-luxo-01`, `allotropy-openlab-luxo-03`, `allotropy-openlab-sirius-01` |
| `openlab-cds` | format_version | `signal 131` | metadata, traces | 2 | 2 | `cct-openlab-meoh1`, `cct-openlab-sirslt-norbert` |
| `openlab-cds` | format_version | `signal 179` | metadata, traces | 10 | 10 | `allotropy-openlab-luxo-01`, `allotropy-openlab-luxo-03`, `allotropy-openlab-sirius-01` |
| `openlab-cds` | instrument | `G1364F` | descriptive | 1 | 1 | `cct-openlab-sirslt-norbert` |
| `openlab-cds` | instrument | `G7104C` | descriptive | 1 | 1 | `cct-openlab-sirslt-norbert` |
| `openlab-cds` | instrument | `G7111B` | descriptive | 3 | 3 | `allotropy-openlab-luxo-01`, `allotropy-openlab-luxo-03`, `allotropy-openlab-sirius-01` |
| `openlab-cds` | instrument | `G7115A` | descriptive | 1 | 1 | `cct-openlab-sirslt-norbert` |
| `openlab-cds` | instrument | `G7116A` | descriptive | 3 | 3 | `allotropy-openlab-luxo-01`, `allotropy-openlab-luxo-03`, `cct-openlab-sirslt-norbert` |

… 26 more values: the generated table in `src/assurance.rs` has all of them.

### Tests, fixtures, fuzz targets, snapshots

- integration tests: none (unit tests in `src/`)
- fuzz targets (`fuzz/fuzz_targets/`): `chrom_cfb`, `chrom_netcdf`, `whole_andi`, `whole_chemstation`, `whole_chromeleon`, `whole_empower_arw`, `whole_openlab`, `whole_shimadzu`
- corpus inputs by tier: full 3, heldout 10, smoke 61, standard 31
- golden snapshots: [`corpus/snapshots/chemstation.jsonl`](../../corpus/snapshots/chemstation.jsonl), [`corpus/snapshots/openlab-cds.jsonl`](../../corpus/snapshots/openlab-cds.jsonl), [`corpus/snapshots/andi-chrom.jsonl`](../../corpus/snapshots/andi-chrom.jsonl), [`corpus/snapshots/empower-arw.jsonl`](../../corpus/snapshots/empower-arw.jsonl), [`corpus/snapshots/shimadzu.jsonl`](../../corpus/snapshots/shimadzu.jsonl), [`corpus/snapshots/chromeleon.jsonl`](../../corpus/snapshots/chromeleon.jsonl)

### Open new-variant intakes

None.
<!-- END GENERATED guide -->
