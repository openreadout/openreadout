# Fuzzing

[cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz) (libFuzzer + AddressSanitizer) targets for every
reader, sub-parser and codec. Not part of the main workspace (it needs a nightly toolchain; the root
`Cargo.toml` excludes `fuzz/`) and never published (`publish = false`).

```bash
rustup toolchain install nightly
cargo install cargo-fuzz
cd fuzz
cargo +nightly fuzz list
cargo +nightly fuzz run -O -a whole_czi work/whole_czi corpus/whole_czi   # one target until the first crash
./run.sh 180 whole_tiff whole_plate   # named targets 3 min each, one after another, collecting all crashes
./run.sh 60                           # every target, 1 min each
python3 triage.py whole_tiff          # replay fuzz/artifacts/<target>/* and group them by cause
```

`-a` keeps debug assertions (integer-overflow checks) on, so overflows are reported as crashes.
Under the fuzzer the plane limit is 64 MiB (`OPENREADOUT_MAX_PLANE_BYTES`, set in `src/lib.rs`), so a
tiny input may not make a reader allocate gigabytes. `run.sh` runs one target at a time with one worker
and a 2 GB RSS cap: fuzzing next to builds on a 16 GB machine has exhausted memory before.

After fixing a crash, copy the input to `crates/<crate>/tests/fixtures/malformed/` as `file-<what>.<ext>`
(a single file) or `bundle-<target>-<what>.bin` (a multi-file input), move `artifacts/<target>` to the
gitignored `found/<target>-rN`, and log the fix in `docs/provenance/<fmt>.md`.

## Targets

Whole data sets run every read-only `Dataset` operation the CLI and MCP server expose:
`open → info → vendor_metadata → entries → check → read_plane(0, c0 z0 t0)` and its statistics
accumulator, then the first rows of table 0, the first samples of trace 0, spectrum 0, attachment 0 and
frame records (`src/lib.rs`, `exercise`). Multi-file inputs are split at `BUNDLE_SEP` and written to the
listed names in a scratch directory (`whole_bundle`).

| target | what it feeds |
| --- | --- |
| `whole_czi`, `whole_nd2`, `whole_lif`, `whole_oir`, `whole_zvi`, `whole_dcimg`, `whole_tiff`, `whole_oib` | microscopy files |
| `whole_oif` | Olympus FluoView OIF: `.oif`, then one plane's `.pty` and `.tif` in its `.oif.files` folder |
| `whole_hcs_harmony`, `whole_hcs_cellvoyager`, `whole_hcs_imagexpress` | high-content screening plates: index file(s) plus plane TIFFs, as a bundle |
| `whole_vsi` | `.vsi`, then `_s_/stack1/frame_t_0.ets` |
| `whole_mirax` | 3DHISTECH MIRAX: `s.mrxs`, then `s/Slidedat.ini`, `s/Index.dat`, `s/Data0000.dat` |
| `core_zip` | the shared zip reader (`openreadout_core::zip`) alone: every member decoded and CRC-checked, stored ranges read (seed: a whole RDML file) |
| `whole_zarr_zip`; `whole_zarr` | OME-Zarr zip store; directory store (`.zgroup`, `.zattrs`, `0/.zarray`, one chunk, v3 `zarr.json` files). No-ops on macOS: `zarrs` registers codecs with `inventory` initializers that Apple's linker rejects under sanitizer coverage; they run in CI (Linux) |
| `whole_ims`, `whole_nwb`, `whole_hdf5` | Imaris, NWB 2.x and generic HDF5 |
| `whole_mrc`, `whole_dm`, `whole_emd`; `whole_ser` | electron microscopy; TIA `.ser` + `.emi` |
| `whole_thermo`, `whole_mzml`, `whole_mzxml`; `whole_mzml_gz`, `whole_mzmlb`; `whole_imzml`; `whole_tims` | mass spectrometry; gzip-compressed mzML and mzMLb; imzML + `.ibd`; timsTOF `.d` (`analysis.tdf` SQLite + `analysis.tdf_bin`) |
| `whole_chemstation`, `whole_waters`; `whole_andi`, `whole_shimadzu`; `whole_sciex` | ChemStation `.D` and MassLynx `.raw` directories; ANDI netCDF, Shimadzu `.lcd`; Sciex `.wiff` + `.wiff.scan` |
| `whole_masshunter` | Agilent MassHunter `.d`: `AcqData/` scan-index schema, scan index, centroid and profile data, calibrations, time segments, contents, devices, sample info and one LC signal pair (seeds: two MetaboLights directories, cut) |
| `whole_openlab` | Agilent OpenLab CDS injection: `.dx` + `.rx` + sequence `.acaml` in one folder (seed: allotropy's MIT-licensed test result set) |
| `whole_chromeleon` | Thermo Scientific Chromeleon 7 archive `.cmbx` (a small synthetic archive: header.xml, a protocol-buffers sequence file, one PtsLDiff signal) |
| `whole_empower_arw` | Waters Empower ASCII export `.arw` (a small synthetic export) |
| `whole_fcs`; `whole_plate`, `whole_plate_xlsx`; `assay_input` | flow cytometry; plate-reader text/CSV and XLSX exports; plate-analysis layouts, well lists and long plate CSVs run through every `analyze assay` analysis |
| `batch_sheet` | sample sheets and plate layouts (CSV, TSV, text, XLSX) read by `--sample-sheet`. No-op on macOS (the batch crate links `zarrs`, see `whole_zarr`); runs on Linux |
| `whole_opus`, `whole_omnic`, `whole_wdf`, `whole_pesp`, `whole_spc`, `whole_witec`, `whole_agilent_fpa`, `whole_fsm` | vibrational spectroscopy: Bruker OPUS, Thermo OMNIC `.spa`/`.spg`, Renishaw WiRE `.wdf`, PerkinElmer `.sp`, Galactic `.spc`, WITec `.wip`/`.wid`, Agilent FT-IR imaging, PerkinElmer `.fsm` |
| `whole_biacore` | Cytiva Biacore `.blr` compound file: text streams, curve headers, segments and XY data |
| `whole_jws` | JASCO `.jws`: compound-file streams and records, the flat `L~S ` header and data |
| `whole_cary` | Agilent (Varian) Cary `.dsw`/`.bsw`/`.bsk`: the store chain, spectrum and baseline stores and their texts |
| `whole_seahorse` | Agilent Seahorse `.asyr`: gzip-compressed assay XML (plate map, readings, spans, protocol) |
| `whole_bme` | Cytiva Biacore T200 evaluation `.bme`: data-manager storages, curve headers, evaluation-item XML and fits |
| `whole_zetasizer` | Malvern Zetasizer `.dts`: compound file, Header identifier, record header walks, sample-name rule, size and zeta result structures |
| `whole_octet` | Sartorius Octet `.frd`: result XML, base64 float32 step arrays, step table |
| `whole_gpr` | GenePix Results `.gpr` (ATF): header records, titles, feature rows |
| `whole_itc` | MicroCal `.itc` text: header, injection lines, block markers, data rows |
| `whole_bes3t` | Bruker BES3T `.DSC` + `.DTA` (+ `.YGF`) bundle: descriptor layers, formats, axis files |
| `whole_esp` | Bruker ESP/WinEPR `.par` + `.spc` bundle: parameter keys, point counts, byte orders |
| `whole_xrdml` | PANalytical XRDML: scans, positions, intensities/counts, attenuation |
| `whole_bruker_raw` | Bruker `.raw` RAW1.01/RAW4.00: range headers, records |
| `whole_brml` | Bruker `.brml`: zip, data views, Datum rows |
| `whole_ras` | Rigaku `.ras`: header and data blocks |
| `whole_rasx` | Rigaku `.rasx`: zip, profiles, conditions |
| `whole_mpr` | BioLogic `.mpr`: modules, column table, records |
| `whole_mpt` | BioLogic `.mpt`: header block, labels, rows |
| `whole_gamry` | Gamry `.DTA`: EXPLAIN header and tables |
| `whole_nda` | Neware `.nda`: version 29 data section, BTS 9.0/9.1 records |
| `whole_ndax` | Neware `.ndax`: `.ndc` pages, run information, auxiliary files |
| `whole_arbin` | Arbin `.res`: the Jet 4 catalog, table definitions, data pages, rows (null masks, variable offsets, overflow pointers, long values, compressed text) |
| `whole_ngb` | NETZSCH `.ngb-*`: stream directories, record grammar, channel runs |
| `whole_ta001` | TA Instruments `.001`: UTF-16 header, signal count, records, end record |
| `whole_trios` | TA Instruments TRIOS `.tri`: header strings, step objects, typed properties, signal records with flag arrays, parallel-plate moduli |
| `whole_scn` | Bio-Rad Image Lab `.scn`: MIME multipart parts (nested, with and without lengths), XML headers, 16-bit image data |
| `whole_unicorn_res`, `whole_unicorn_zip` | Cytiva ÄKTA / UNICORN results: `.res` block directory and descriptors; UNICORN 6/7 export zips with nested padded zips and MS-NRBF records |
| `whole_rdml`, `whole_eds`, `whole_rex`, `whole_pcrd`, `whole_ixo` | qPCR: RDML (zip or XML), Applied Biosystems `.eds` (zip; seeds in the SDS, 7500 and JSON layouts, written by hand), Rotor-Gene `.rex`, Bio-Rad `.pcrd` (detection and refusal only), LightCycler 480 `.ixo` (a small synthetic object stream with a zlib acquisition store) |
| `whole_abf`, `whole_atf`, `whole_neuralynx`, `whole_blackrock`, `whole_intan`, `whole_plexon`, `whole_pl2`, `whole_heka`, `whole_spike2` (`.smr` and `.smrx`), `whole_winwcp`, `whole_openephys`; `whole_spikeglx` | electrophysiology (Plexon: magic-only committed seeds; the corpus files are share-alike and seed `local-seeds/` only); SpikeGLX `.meta` + `.bin` |
| `whole_bruker`, `whole_jcamp`, `whole_varian`, `whole_jeol`, `whole_spinsolve` | Bruker NMR experiment directory (`acqus`, `fid`, `pdata/1/procs`, `pdata/1/1r`, `acqu2s`, `ser`); JCAMP-DX; VnmrJ directory (`procpar`, `fid`, `text`); JEOL `.jdf`; Spinsolve experiment directory (`acqu.par`, `data.1d`, `proc.par`, `processing.script`, `spectrum.1d`) |
| `czi_segment_walk`, `czi_subblock_header`, `czi_metadata_xml` | CZI file header and segment walk; subblock, directory and attachment entries; `ImageDocument` XML |
| `nd2_chunk_map`, `nd2_lv`, `nd2_variant_xml` | ND2 chunk map (and rescue scan); LV decoder; variant XML decoder, then the normalizers |
| `lif_container`, `lif_xml_model` | LIF header, UTF-16 XML, memory blocks; XML → image nodes |
| `chrom_netcdf` | netCDF-3 (classic and 64-bit offset) header, every variable's shape and first values |
| `chrom_cfb` | compound-file (MS-CFB) header, FAT, mini FAT, directory, every stream |
| `signal_analysis` | NMR processing (group delay, apodization, FFT, phasing, baseline), NMR peak picking and integration, the JEOL digital-filter parser, action-potential detection and passive fits, band-pass filtering and extracellular spike detection on arbitrary samples and parameters |
| `quant_peaks` | compound lists (CSV/TSV/JSON) and peak detection/integration (every baseline mode, manual integration) on arbitrary f32 chromatograms |
| `tims_sqlite` | the read-only SQLite reader: header, b-trees, schema, every table |
| `waters_drift` | Waters drift index (`_funcNNN.ind`) and `.cdt` scan data (LZRW3 sections); first 4 bytes: length of the index part |
| `tims_frame` | TDF frame blobs (zstd, four byte planes) and TSF line spectra; first 4 bytes: scan count or peak count, byte 4: which |
| `jcamp_asdf` | JCAMP-DX ASDF (SQZ/DIF/DUP) tables, grouped decoding and the line lexer |
| `mzml_binary` | base64, zlib/zstd and MS-Numpress arrays; the first byte picks value type, compression and byte order |
| `codec_zstd0`, `codec_zstd1`, `codec_hilo`, `codec_jpegxr`, `codec_lzw`, `codec_zlib`, `codec_jpeg`, `codec_jpeg2000`, `codec_packbits`, `codec_lzma2`, `codec_webp`, `codec_jpegxl` | each `openreadout-codecs` entry point (first 3 bytes: expected decoded length or, for JPEG, the frame-size bound; WebP and JPEG XL decode with a 64 MiB limit; `codec_jpeg` also drives the `jpeg_markers` scanner) |
| `ome_xml` | OME-XML builder from a JSON `FileInfo` (asserts the output is well-formed 7-bit XML) |
| `selection` | `--select` parser |

## Seeds

`corpus/<target>/` holds the committed seeds (< 3 MB in total), regenerated by `seeds.py`
(`OPENREADOUT_CORPUS_DIR=corpus/files oracle/.venv/bin/python fuzz/seeds.py --info-json DIR`):

- synthetic minimal files written by `seeds.py` itself (8x8 CZI uncompressed/zstd0/LZW, 8x8 ND2 plain and
  zlib, 8x6x2 LIF v1/v2, an 8x8 OME-Zarr v2 array, JCAMP-DX tables, timsTOF frame blobs, mzML arrays) and
  codec streams of small generated images (imagecodecs and pynumpress, both BSD-3-Clause, used as encoders
  only);
- structures and heads cut from corpus files listed in `corpus/manifest.toml` with permissive licenses
  (MIT, BSD, Apache-2.0, CC0, CC-BY-4.0, public domain, EMBL-EBI terms); the list, with each license, is
  `COMMITTED_CUTS` / `COMMITTED_BUNDLES` in `seeds.py` and the CZI/ND2/LIF lists at its top;
- share-alike (CC-BY-SA-4.0) corpus files (Neuralynx, Blackrock, Intan, SpikeGLX) are never committed:
  `seeds.py` writes them to the gitignored `local-seeds/`, which `run.sh` adds when present. Fixtures for
  crashes found from those seeds are written from scratch instead;
- `ome_xml`: `info --json` output (paths reduced to file names) for the synthetic files and three corpus files.

## Regression fixtures

Every input that crashed a target, and the hand-made files from `craft.py` that exercise each allocation
guard, is kept in `crates/<crate>/tests/fixtures/malformed/`. `crates/openreadout-cli/tests/fuzz_regressions.rs`
replays every `file-*` fixture through every registered reader and every `bundle-<target>-*` fixture as the
target's directory; sub-parser fixtures (`segment-*`, `lv-*`, `sqlite-*`, `asdf-*`, codec prefixes) are
replayed by `tests/fuzz_regressions.rs` in their crate. `known-upstream/` holds reproducers for bugs in
dependencies that are not fixed here (see [Known issues in dependencies](#known-issues-in-dependencies)); the calamine one is not replayed because it is a
debug-build panic inside the dependency. The `jpegxr-*` reproducers of the former `jpegxr-pure-rs`
dependency (one here, three in `crates/openreadout-codecs/tests/fixtures/malformed/`) are replayed by
`crates/openreadout-jpegxr/tests/bitexact.rs` and must give clean errors.

## Campaigns

| date | target | time | executions | crashes |
|---|---|---|---|---|
| 2026-09-24 | `codec_jpegxr` (new JPEG XR decoder, 30 seeds) | 3 h | 7.8 million | 0 (coverage 4,700 edges) |
| 2026-09-24 | `whole_czi` (JPEG XR subblocks through the CZI reader) | 1 h | 1.4 million | 1, not in JPEG XR: an attachment offset overflow in the CZI reader (fixed; `crates/openreadout-czi/tests/fixtures/malformed/file-fuzz-attachment-position-overflow.czi`) |
| 2026-09-24 | `codec_jpegxl` (jxl-oxide, 6 GDAL seeds) | 15 min | 3.0 million | 0 |
| 2026-09-24 | `codec_webp` (image-webp, 4 GDAL seeds) | 15 + 15 min | 3.5 + 1.5 million | 1 in the first run: an integer overflow in image-webp's Huffman setup (panics only with overflow checks; SECURITY.md); none after building image-webp without them |
| 2026-09-24 | `codec_lerc` (lerc-rs, 12 GDAL seeds) | 15 + 10 min | 12,000 + 1.8 million | 52, then 16 after our header validation: panics inside lerc-rs; LERC left out of default builds and the target removed (SECURITY.md) |
| 2026-09-24 | campaign 3: the 38 targets added or substantially changed since campaign 2 (`fuzz/run.sh 90 …`, listed in the table below) | 90 s each | 10.0 million in all | 8 causes in 5 targets, all fixed with regression fixtures: `whole_wdf` (2), `whole_mzmlb` (2, one found on the rerun), `whole_waters` (2, one a division by zero that also crashed release builds), `codec_jpeg2000` (1), `assay_input` (1); the five reran clean for 90 s. Lowest coverage: `whole_sciex` 222 edges and `whole_oib` 804 (seeds need rework) |
| 2026-09-24 | `whole_masshunter` (new, two MetaboLights directory seeds) | 90 s | 19,031 | 0 (coverage 5,550 edges) |
| 2026-09-24 | second round: `core_zip`, `quant_bands` (new) and 13 targets whose readers had changed (qPCR, OpenLab CDS, Gen5, Shimadzu, WiRE, Harmony, CZI/VSI/NDPI pyramids, ND2, auto baseline) | 90 s each | 3.8 million in all | 1 cause: `whole_vsi` plane-size check overflow, fixed with a regression fixture; clean on rerun |

### Last recorded run per target (to 2026-09-24)

One libFuzzer worker (`-fork=1`, AddressSanitizer, debug assertions on) for every run. Campaign 2 (2026-09-23) ran every target that existed then for 3 minutes; campaign 3 (2026-09-24) ran every target added or substantially changed since then for 90 seconds, and a second round the same day reran the targets whose readers had changed (the shared zip reader, spectral bands, the auto baseline, qPCR, OpenLab CDS, Gen5, Shimadzu, WiRE, Harmony, the CZI/VSI/NDPI pyramids, ND2) for another 90 seconds each. The table shows the last run of each target that existed on 2026-09-24; targets added later have no long run recorded yet. In CI, the `fuzz-smoke` job runs on Linux. Pull requests fuzz the targets their changes could affect (`affected.py`) for 15 s each, and the weekly run fuzzes all targets for 60 s each.

| area | targets | last recorded run | notes |
| --- | --- | --- | --- |
| CZI | `whole_czi`, `czi_segment_walk`, `czi_subblock_header`, `czi_metadata_xml` | `whole_czi` 1 h (1.4 million executions, 2026-09-24; one crash in the attachment directory, fixed), and 90 s after the pyramid-level change (77,081 executions, no crash); the sub-parsers 3 min (campaign 2, 2026-09-23) |  |
| ND2, LIF | `whole_nd2`, `nd2_chunk_map`, `nd2_lv`, `nd2_variant_xml`, `whole_lif`, `lif_container`, `lif_xml_model` | 3 min (campaign 2, 2026-09-23); `whole_nd2` again for 90 s after the multichannel speed-up (163,149 executions, no crash) |  |
| Other light microscopy | `whole_oir`, `whole_zvi`, `whole_ims` | 3 min (campaign 2, 2026-09-23) |  |
| OIB, OIF, DCIMG, VSI | `whole_oib`, `whole_oif`, `whole_dcimg`, `whole_vsi` | 90 s each (campaign 3, 2026-09-24; 891,332 executions): `whole_vsi`: 1 cause found and fixed (plane-size check overflow); clean on rerun | `whole_oib` and `whole_oif` are new; `whole_vsi` rerun after the pyramid-factor change |
| TIFF family | `whole_tiff` | 90 s each (campaign 3, 2026-09-24; 203,212 executions): no crash | rerun after the JPEG 2000, WebP, JPEG XL, old-style JPEG and MetaMorph additions |
| OME-Zarr | `whole_zarr`, `whole_zarr_zip` | **no run recorded**: no-ops on macOS (`zarrs` does not link under sanitizer coverage with Apple's linker); they run only in CI's Linux `fuzz-smoke` job, which has not run while GitHub Actions is disabled |  |
| Screening plates | `whole_hcs_harmony`, `whole_hcs_cellvoyager`, `whole_hcs_imagexpress` | 90 s each (campaign 3, 2026-09-24; 151,731 executions): no crash |  |
| Electron microscopy | `whole_mrc`, `whole_dm`, `whole_emd`, `whole_ser` | 3 min (campaign 2, 2026-09-23) |  |
| HDF5, NWB | `whole_hdf5`, `whole_nwb` | 90 s each (campaign 3, 2026-09-24; 53,183 executions): no crash | `whole_hdf5` 3 min (campaign 2, 2026-09-23); `whole_nwb` rerun after the NWB depth work |
| Flow cytometry | `whole_fcs` | 3 min (campaign 2, 2026-09-23) | gating and transforms are reached through `whole_fcs` only as far as tables go; no dedicated FlowJo/Gating-ML target yet |
| Electrophysiology | `whole_abf`, `whole_neuralynx`, `whole_blackrock`, `whole_intan`, `whole_spikeglx` | 3 min (campaign 2, 2026-09-23) | share-alike seeds in `local-seeds/` only |
| ATF, Plexon | `whole_atf`, `whole_plexon`, `whole_pl2` | 90 s each (campaign 3, 2026-09-24; 596,601 executions): no crash | new; Plexon seeds: magic-only headers committed, one PLX and one PL2 corpus head locally |
| NMR | `whole_bruker`, `whole_varian`, `whole_jeol` | 90 s each (campaign 3, 2026-09-24; 110,634 executions): no crash | `whole_bruker` rerun after the 2D/3D processed-data and nuslist work |
| JCAMP-DX | `whole_jcamp`, `jcamp_asdf` | 3 min (campaign 2, 2026-09-23) |  |
| FT-IR / Raman | `whole_opus`, `whole_omnic`, `whole_wdf`, `whole_pesp` | 90 s each (campaign 3, 2026-09-24; 364,459 executions): `whole_wdf`: 2 causes found and fixed (origin-list stride overflow; spectra table sized from the header, 34 GB); clean on rerun |  |
| Mass spectrometry | `whole_mzml`, `whole_mzxml`, `mzml_binary`, `whole_imzml`, `whole_tims`, `tims_frame`, `tims_sqlite` | 3 min (campaign 2, 2026-09-23) |  |
| Thermo, Waters, Sciex, mzML.gz, mzMLb | `whole_thermo`, `whole_waters`, `whole_sciex`, `whole_mzml_gz`, `whole_mzmlb` | 90 s each (campaign 3, 2026-09-24; 544,603 executions): `whole_waters`: 2 causes found and fixed (division by zero, a release-build crash; scan-end overflow in `check`); clean on rerun; `whole_mzmlb`: 2 causes found and fixed (unfiltered and chunked storage past the end of the file made hdf5-pure allocate 2-3 GB); clean on rerun | `whole_thermo` and `whole_waters` rerun after the detector-trace and full-scan work |
| Agilent MassHunter | `whole_masshunter` | 90 s each (campaign 3, 2026-09-24; 19,031 executions): no crash | new; the reader had no target |
| Chromatography | `whole_chemstation`, `whole_andi`, `chrom_cfb` | 3 min (campaign 2, 2026-09-23) |  |
| netCDF, Shimadzu, OpenLab CDS | `chrom_netcdf`, `whole_shimadzu`, `whole_openlab` | 90 s each (campaign 3, 2026-09-24; 1,154,603 executions): no crash | `chrom_netcdf` did not build since the byte-source change (fixed today); `whole_shimadzu` rerun after the trace decoder |
| Plate readers | `whole_plate`, `whole_plate_xlsx` | 90 s each (campaign 3, 2026-09-24; 53,350 executions): no crash | `whole_plate_xlsx` 3 min (campaign 2, 2026-09-23) |
| qPCR | `whole_rdml`, `whole_eds`, `whole_rex`, `whole_pcrd` | 90 s each (campaign 3, 2026-09-24; 2,383,733 executions): no crash |  |
| Analysis | `quant_peaks`, `quant_bands`, `signal_analysis`, `assay_input` | 90 s each (campaign 3, 2026-09-24; 563,048 executions): `assay_input`: 1 cause found and fixed (plate-map header overflow); clean on rerun | `quant_bands` (spectral bands and regions) is new; `quant_peaks` rerun after the auto baseline |
| Zip containers | `core_zip` | 90 s each (campaign 3, 2026-09-24; 488,866 executions): no crash | new: the shared zip reader of qPCR, OpenLab CDS and zipped OME-Zarr |
| Batch sample sheets | `batch_sheet` | **no run recorded**: a no-op on macOS (links `zarrs` through the batch crate); Linux CI only |  |
| Codecs | `codec_zstd0`, `codec_zstd1`, `codec_hilo`, `codec_lzw`, `codec_zlib`, `codec_jpeg`, `codec_packbits`, `codec_jpeg2000` | 90 s each (campaign 3, 2026-09-24; 3,428,866 executions): `codec_jpeg2000`: 1 cause found and fixed (SIZ size-check overflow); clean on rerun | `codec_zstd1`, `codec_hilo`, `codec_zlib`, `codec_packbits` 3 min (campaign 2, 2026-09-23); `codec_jpeg2000` (now `rust-j2k`), `codec_jpeg`, `codec_lzw` and `codec_zstd0` rerun after their decoders changed |
| JPEG XR | `codec_jpegxr` | 3 h (7.8 million executions, 2026-09-24): 0 crashes | the in-house decoder |
| JPEG XL, WebP | `codec_jpegxl`, `codec_webp` | 15 min (3.0 million executions) and 30 min (5.0 million executions), 2026-09-24: no crash in jxl-oxide; one upstream overflow in image-webp | see SECURITY.md § Known issues in dependencies |
| OME-XML, `--select` | `ome_xml`, `selection` | 3 min (campaign 2, 2026-09-23) |  |

**Gaps in the fuzz coverage, to close next:** dedicated FlowJo workspace and Gating-ML targets (gating is reached only through `analyze gate`/`table`, which the whole-file targets do not call); `whole_sciex` and `whole_oib` reach little code from their committed seeds (coverage 222 and 804 edges: the seeds are cut before the structures the readers need), so their seeds need rework; `quant_peaks` and `signal_analysis` run about 35 executions per second, so 90 s is a smoke test only; runs of hours rather than minutes for every reader added since 2026-09-23; and a Linux run of the three macOS no-op targets.

## Known issues in dependencies

- `calamine` 0.36 (XLSX/XLS/XLSB/ODS for plate-reader workbooks), found by our fuzzer:
  - `Range` (what `worksheet_range` returns) is dense: two cells at A1 and XFD1048576 make it allocate the whole 1,048,576 x 16,384 grid. The plate reader streams XLSX cells instead and refuses worksheets spanning more than 4 Mi cells; XLS, XLSB and ODS still go through `worksheet_range`, so a hostile file in those formats can make it allocate before our check sees the size;
  - its cell-reference parser multiplies without overflow checks (`get_row_and_optional_column`): wrapping in release builds (a wrong cell position, then our bounds), a panic in debug builds, which the plate reader turns into a corrupt-file error. Reproducer: `fuzz/known-upstream/calamine-0.36.1-cell-reference-overflow.xlsx`.
- `image-webp` 0.2.4 (TIFF WebP chunks), found by our fuzzer: it adds lossless Huffman code-length counts in `u16` without overflow checks, so a hostile stream panics in builds with overflow checks (debug builds, the fuzzer). Release builds wrap and then reject the code as invalid. The workspace (and the fuzz workspace) build image-webp without overflow checks, so every build returns a clean error. Reproducer: `fuzz/known-upstream/image-webp-0.2.4-huffman-overflow.webp`, replayed by a test in `openreadout-codecs`.
- `lerc-rs` 0.5 (TIFF LERC chunks), found by our fuzzer in 12,000 executions: it trusts the LERC2 header (a zero micro-block size divides by zero, negative dimensions loop for hours; we now validate every header first), and past the header it still indexes its validity mask out of bounds and overflows a size multiplication (`decode.rs:363`). Those are panics in release builds too, so LERC decoding is not in default builds (the `lerc` feature of `openreadout-codecs`/`openreadout-tiff` turns it on for trusted files); LERC pages are exit 6. Reproducers: `fuzz/known-upstream/lerc-rs-0.5.0-*.lerc`.
- `hdf5-pure` 0.47 (Imaris, NWB, Velox EMD, mzMLb): reading rows of a dataset allocates the dataset's declared contiguous storage, or a stored chunk's declared size, before checking it against the file, so a 43 KB mzMLb asked for 2-3 GB (found by `whole_mzmlb` on 2026-09-24). The mzMLb reader now refuses datasets whose storage or chunks lie past the end of the file before reading; the other HDF5 readers bound what they read by the plane limit, but a hostile HDF5 file may still make `hdf5-pure` allocate up to its declared sizes in paths we have not guarded.
- `jpeg-decoder` 0.3: it allocates its per-component planes from the frame header before its own output limit applies (a 342-byte stream asked for 4 GB). Every JPEG decode now reads the header first and refuses frames larger than the tile, strip or subblock that holds them (`jpeg_decode_limited`).
