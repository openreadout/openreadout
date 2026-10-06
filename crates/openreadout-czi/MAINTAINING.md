# Maintaining `openreadout-czi`

Zeiss CZI (`czi`): confocal, widefield, Airyscan, lattice light-sheet and whole-slide (Axioscan) files, multi-file documents included. Project-wide process: [docs/maintaining.md](../../docs/maintaining.md). Layout and vocabulary: [docs/formats/czi.md](../../docs/formats/czi.md); provenance: [docs/provenance/czi.md](../../docs/provenance/czi.md). **Clean room**: libCZI and pylibCZIrw are LGPL oracles (their documentation pages may be read, never their source); czifile (BSD-3) may be read as documentation.

## Decode pipeline

1. **Detect** (`lib.rs` `CziReader::sniff`): the `ZISRAWFILE` segment id at offset 0.
2. **Container** (`container.rs` `CziFile::open`): the segment chain (`SegmentHeader`, `SegmentId`: file header, metadata, subblock directory, attachment directory, subblocks, attachments, deleted), the file header (positions of the directory segments; multi-file part number and master GUID), the subblock directory (`DirectoryEntry` with `DimensionEntry`s: X, Y, C, Z, T, S, M, B, H, I, R, V, and the pyramid/stored sizes), the attachment directory. Multi-file documents (`part_path`, `probe_part`: `<name> (<k>).czi`) join their parts.
3. **Metadata** (`xml.rs` `parse_image_xml`): the `ImageDocument` XML (scaling, channels from the image list or the display setting, objectives, detectors, scenes and wells, the experiment blocks; `collect_active_setups`) and per-subblock `<METADATA>` tags (`parse_subblock_tags`). **Writer versions branch here**: ZEN generations differ in where channels, detectors and the experiment live (`display_setting_by_position_when_ids_differ`, `channels_from_display_setting_alone` are the cases found so far).
4. **Images** (`dataset.rs` `build_scenes`, `build_levels`): one image per scene (and per combination of varying H/I/R/V/B, and per stored-to-logical ratio for super-resolved renderings); pyramid levels from subblocks whose stored size is below their logical size (`stored_ratio`, `level_place`, `snap_factor`).
5. **Pixels** (`dataset.rs` `read_level`, `paste_tile`, `composite`): subblocks overlapping the plane are fetched (`fetch_entry`), decoded by compression (`CompressionId`: uncompressed, JPEG, JPEG XR via `openreadout-jpegxr`, zstd 0/1 with HiLo byte unshuffle via `openreadout-codecs`), conformed to the directory entry's geometry and pixel type (`convert.rs` `conform`: the resolution protocol), masked by valid-pixel masks (`mask.rs`), pasted in directory order. **Codecs and pixel types branch here.**
6. **Attachments** (`attach.rs`): `CZTIMS` time stamps and `CZEVL` event lists are interpreted; thumbnails, labels and slide previews are extractable.

## Invariants and checks

- `check`: segment ids known and allocated/used sizes consistent, chain ends inside the file; file-header positions land on the right segments; every directory entry agrees with its subblock header; every subblock's data fits its segment and uncompressed sizes match the geometry; every scene has a subblock for every (c, z, t) it declares; attachments fit the file and TimeStamps/EventList parse; every part of a multi-file document is present, says it is that part and names the master's GUID; one JPEG / JPEG XR subblock per (compression, pixel type) decodes and matches its entry.
- Refused (exit 6): JPEG-lossless (id 3), chunked (id 7) and camera raw (id ≥ 100) subblocks, 12-bit DCT JPEG, complex pixel types. Scenes above 4 GiB are read by region or level.

## Debugging a new file

- `openreadout check FILE --report` gives the fingerprint (writer family, codecs, pixel types, layout) and which stage fails; `openreadout info FILE --view structure` lists every segment, directory entry and attachment with offsets.
- `info --view full --json` → `vendor` is the `ImageDocument` XML as JSON; per-subblock tags are in `info --view full --all-frames`.
- Unit tests next to the code are the templates: `container.rs` (`directory_entry_parsing_rejects_garbage`), `convert.rs` (resolution protocol cases), `mask.rs`, `xml.rs` (the metadata variants), `dataset.rs` (level snapping). Synthetic CZIs from pylibCZIrw (`oracle/make_czi_fixtures.py`) cover multi-file documents, pixel types and pyramids.
- Oracles: czifile (primary), pylibCZIrw (second opinion, `oracle/second_opinion.py`); level geometry: `oracle/czi_levels.py`.

## Fragile spots

- **Pyramid level geometry**: levels whose factor is not a power of two, tiles snapped to level factors and single-tile coarse levels (`edge_tiles_snap_to_the_level_factor`, `coarse_single_tile_levels_join_their_power_of_two_level`) — the held-out benchmark once flagged a ZEN generation here (docs/assurance.md).
- **Super-resolved renderings** (PALM) and Airyscan-era multi-track files with channels absent at H > 0 have their own images and `extra.absent_channels`; subblocks stored below their logical size without a pyramid flag are not upsampled.
- JPEG subblocks decode with jpeg-decoder (a few grey levels from libjpeg-turbo on lossy streams); multi-file part discovery is inferred from synthetic fixtures only.
- `dataset.rs` is the largest file in the crate (≈ 2 300 lines): scene/level assembly and decoding are intertwined; prefer adding cases to the unit tests before refactoring.

<!-- BEGIN GENERATED guide -->
## Facts (generated)

*Generated by `cargo xtask guides --write` from the sources, `corpus/manifest.toml`, `corpus/assurance/evidence.json` and `corpus/intake/`; do not edit. CI fails when it is stale.*

### Formats

| format id | notes and provenance | confidence | basis | development files: read / confirmed | depositors | held-out pass / fail |
| --- | --- | --- | --- | --- | --- | --- |
| `czi` | [format note](../../docs/formats/czi.md), [provenance log](../../docs/provenance/czi.md) | medium | prior art | 90 / 84 | 25 | 3 / 1 |

### Source map

| file | what it does (its module documentation) |
| --- | --- |
| [`src/assurance.rs`](src/assurance.rs) | Assurance profile (`docs/assurance.md`): the variant features of a CZI file and the feature values the development corpus validates |
| [`src/attach.rs`](src/attach.rs) | Attachment payloads we interpret: `CZTIMS` (time stamps) and `CZEVL` (event list) |
| [`src/container.rs`](src/container.rs) | Segment chain, file header, subblock directory, attachments |
| [`src/convert.rs`](src/convert.rs) | The resolution protocol: bring a decoded bitmap to the geometry and pixel type the directory entry declares |
| [`src/dataset.rs`](src/dataset.rs) | `Dataset` implementation for CZI: scenes, pyramid levels, plane assembly, decoding, attachments, per-plane metadata, integrity checks |
| [`src/fuzzing.rs`](src/fuzzing.rs) | Byte-slice entry points into the container parsers, for the cargo-fuzz targets in `fuzz/` |
| [`src/lib.rs`](src/lib.rs) | Clean-room reader for Zeiss CZI files |
| [`src/mask.rs`](src/mask.rs) | Valid-pixel masks carried in a subblock's attachment |
| [`src/xml.rs`](src/xml.rs) | The `ImageDocument` metadata XML and per-subblock `<METADATA>`: the subset we normalize |

### Where variants branch

The assurance profile ([`src/assurance.rs`](src/assurance.rs)) observes these features (each value is looked up in the validated table below; a value never confirmed makes the file `unvalidated` for the feature's outputs) and reports these structures and assumptions:

- feature codec `codec`
- feature sample_layout `format!("stored {s}")`
- feature layout `"mosaic"`
- feature layout `"pyramid"`
- feature writer `&fam`
- feature layout `"multi_scene"`
- feature layout `format!("extra dimension {axis}")`
- feature layout `"multi_file"`
- feature layout `"super-resolved rendering"`
- feature writer_version `format!("{fam} {v}")` (descriptive)
- feature instrument `m` (descriptive)
- undecoded "missing file parts"
- undecoded "structure: "

### Validated variants and the corpus files that pin them

| format | kind | value | outputs | confirmed files | read | example corpus files |
| --- | --- | --- | --- | --- | --- | --- |
| `czi` | codec | `jpeg` | pixels | 4 | 5 | `synthetic-bgr24-jpeg`, `synthetic-bgr24-jpeg444`, `synthetic-gray16-jpeg-lossless` |
| `czi` | codec | `jpeg_xr` | pixels | 13 | 15 | `openslide-zeiss-5-cropped`, `openslide-zeiss-5-flat`, `openslide-zeiss-5-jxr` |
| `czi` | codec | `uncompressed` | pixels | 62 | 64 | `aics-NoSceneNames`, `aics-OverViewScan`, `aics-RGB-8bit` |
| `czi` | codec | `zstd0` | pixels | 1 | 1 | `openslide-zeiss-5-slidepreview-zstd0` |
| `czi` | codec | `zstd1` | pixels | 4 | 4 | `openslide-zeiss-5-slidepreview-zstd1-hilo`, `synthetic-gray16-s2c2t2-zstd1`, `zenodo17252016-Figure-6G-CTRL` |
| `czi` | format_version | `1.0` | metadata, pixels | 84 | 90 | `aics-NoSceneNames`, `aics-OverViewScan`, `aics-RGB-8bit` |
| `czi` | instrument | `Andor1, AxioObserver` | descriptive | 3 | 3 | `zenodo10577621-Image-5-PALM-verrechnet`, `zenodo16419509-Rizzollo-Example1-SIM-raw`, `zenodo16966021-HILO-SIM-GFP-ER-timelapse` |
| `czi` | instrument | `Axio Imager.Z1` | descriptive | 1 | 1 | `zenodo10666482-N2-gonad-1` |
| `czi` | instrument | `Axio Imager.Z2` | descriptive | 1 | 3 | `zenodo17880403-Snap-8347` |
| `czi` | instrument | `Axio Observer.Z1 / 7` | descriptive | 7 | 7 | `aics-s-1-t-1-c-1-z-1`, `aics-s-3-t-1-c-3-z-5`, `aics-variable-per-scene-dims` |
| `czi` | instrument | `Axio Scan.Z1` | descriptive | 6 | 7 | `zenodo10577621-Intestine-3color-RAC`, `zenodo10577621-Kidney-RAC-3color`, `zenodo12509122-PSR-LXR-KO-WT-1054` |
| `czi` | instrument | `Axio Zoom.V16` | descriptive | 2 | 2 | `zenodo14895059-Figure-6C-D`, `zenodo17098115-Snap-212a` |
| `czi` | instrument | `Axioscan 7` | descriptive | 6 | 6 | `openslide-zeiss-5-cropped`, `openslide-zeiss-5-flat`, `openslide-zeiss-5-jxr` |
| `czi` | instrument | `Celldiscoverer 7` | descriptive | 27 | 27 | `aics-OverViewScan`, `aics-S-2-4x2-T-2-Z-3-CH-2`, `aics-variable-scene-shape-first-scene-pyramid` |
| `czi` | instrument | `LSM 510, AxioObserver` | descriptive | 1 | 1 | `zenodo10577621-Channel-ZStack-LineScan-Bidirectional-Averaging` |
| `czi` | instrument | `LSM 700, AxioObserver` | descriptive | 1 | 1 | `zenodo14139319-SourceDataF2D-withPI3P` |
| `czi` | instrument | `LSM 710, Axio Examiner` | descriptive | 1 | 1 | `zenodo10708864-ztacktimeposition16bit` |
| `czi` | instrument | `LSM 710, AxioObserver` | descriptive | 3 | 3 | `zenodo10577621-LineScan-T3500`, `zenodo10577621-LineScan-T80-Z25`, `zenodo10577621-LineScan-Z200` |
| `czi` | instrument | `LSM 780, Axio Examiner` | descriptive | 1 | 1 | `aics-NoSceneNames` |
| `czi` | instrument | `LSM 780, AxioObserver` | descriptive | 3 | 3 | `zenodo11260215-LS-foto-1`, `zenodo13144501-airyscan-processed-100x-004`, `zenodo17252016-Figure-6G-CTRL` |
| `czi` | instrument | `LSM 880, AxioObserver` | descriptive | 2 | 2 | `zenodo10044967-fig2D-GFP-10B-rot-1`, `zenodo19688024-Claurdan-DMSO-control` |
| `czi` | layout | `extra dimension H` | metadata, pixels | 3 | 3 | `zenodo16419509-Rizzollo-Example1-SIM-raw`, `zenodo17432573-MC1-Nt3EmCherry-7412-H4`, `zenodo17482295-Figure-08-P-aeruginosa-H4` |
| `czi` | layout | `mosaic` | pixels | 28 | 30 | `aics-OverViewScan`, `aics-S-2-4x2-T-2-Z-3-CH-2`, `aics-variable-scene-shape-first-scene-pyramid` |
| `czi` | layout | `multi_file` | pixels | 1 | 1 | `synthetic-multifile` |
| `czi` | layout | `multi_scene` | metadata | 33 | 34 | `aics-NoSceneNames`, `aics-S-2-4x2-T-2-Z-3-CH-2`, `aics-s-3-t-1-c-3-z-5` |
| `czi` | layout | `pyramid` | pixels | 22 | 23 | `aics-OverViewScan`, `aics-S-2-4x2-T-2-Z-3-CH-2`, `aics-variable-scene-shape-first-scene-pyramid` |
| `czi` | layout | `super-resolved rendering` | metadata, pixels | 3 | 3 | `zenodo10577621-Image-5-PALM-verrechnet`, `zenodo10577621-PALM-OnlineVerrechnet`, `zenodo10577621-Palm-mitDrift` |
| `czi` | sample_layout | `stored bgr24` | pixels | 11 | 12 | `aics-RGB-8bit`, `openslide-zeiss-5-cropped`, `openslide-zeiss-5-flat` |
| `czi` | sample_layout | `stored bgr48` | pixels | 4 | 4 | `openslide-zeiss-5-slidepreview-jxr`, `openslide-zeiss-5-slidepreview-zstd0`, `openslide-zeiss-5-slidepreview-zstd1-hilo` |
| `czi` | sample_layout | `stored gray16` | pixels | 54 | 57 | `aics-NoSceneNames`, `aics-OverViewScan`, `aics-S-2-4x2-T-2-Z-3-CH-2` |
| `czi` | sample_layout | `stored gray8` | pixels | 15 | 16 | `synthetic-gray8-c2z2t3-uncompressed`, `synthetic-gray8-jpeg`, `zenodo10577621-LineScan-T3500` |
| `czi` | sample_layout | `uint16` | pixels | 54 | 57 | `aics-NoSceneNames`, `aics-OverViewScan`, `aics-S-2-4x2-T-2-Z-3-CH-2` |
| `czi` | sample_layout | `uint16x3` | pixels | 4 | 4 | `openslide-zeiss-5-slidepreview-jxr`, `openslide-zeiss-5-slidepreview-zstd0`, `openslide-zeiss-5-slidepreview-zstd1-hilo` |
| `czi` | sample_layout | `uint8` | pixels | 15 | 16 | `synthetic-gray8-c2z2t3-uncompressed`, `synthetic-gray8-jpeg`, `zenodo10577621-LineScan-T3500` |
| `czi` | sample_layout | `uint8x3` | pixels | 11 | 12 | `aics-RGB-8bit`, `openslide-zeiss-5-cropped`, `openslide-zeiss-5-flat` |
| `czi` | writer | `ZEN` | metadata, pixels | 7 | 7 | `zenodo17432573-MC1-Nt3EmCherry-7412-H4`, `zenodo17482295-Figure-08-E-coli`, `zenodo17482295-Figure-08-P-aeruginosa-H4` |
| `czi` | writer | `ZEN black` | metadata, pixels | 17 | 17 | `aics-NoSceneNames`, `zenodo10044967-fig2D-GFP-10B-rot-1`, `zenodo10577621-Channel-ZStack-LineScan-Bidirectional-Averaging` |
| `czi` | writer | `ZEN blue` | metadata, pixels | 44 | 47 | `aics-OverViewScan`, `aics-S-2-4x2-T-2-Z-3-CH-2`, `aics-s-1-t-1-c-1-z-1` |
| `czi` | writer | `pylibCZIrw` | metadata, pixels | 14 | 16 | `synthetic-bgr24-jpeg`, `synthetic-bgr24-jpeg444`, `synthetic-bgr24-uncompressed` |
| `czi` | writer_version | `ZEN 3.10` | descriptive | 3 | 3 | `zenodo17723322-axioscan-3scenes-BF`, `zenodo17725097-axioscan-FL-multichannel`, `zenodo17948627-axioscan-2scenes` |
| `czi` | writer_version | `ZEN 3.13` | descriptive | 1 | 1 | `zenodo22090339-6h-timelapse-06-MIP` |
| `czi` | writer_version | `ZEN 3.8` | descriptive | 2 | 2 | `zenodo17482295-Figure-08-E-coli`, `zenodo17482295-Figure-08-P-aeruginosa-H4` |
| `czi` | writer_version | `ZEN 3.9` | descriptive | 1 | 1 | `zenodo17432573-MC1-Nt3EmCherry-7412-H4` |
| `czi` | writer_version | `ZEN black 11.0` | descriptive | 2 | 2 | `zenodo10577621-Image-5-PALM-verrechnet`, `zenodo10577621-PALM-OnlineVerrechnet` |
| `czi` | writer_version | `ZEN black 14.0` | descriptive | 7 | 7 | `aics-NoSceneNames`, `zenodo10044967-fig2D-GFP-10B-rot-1`, `zenodo11260215-LS-foto-1` |
| `czi` | writer_version | `ZEN black 16.0` | descriptive | 2 | 2 | `zenodo16419509-Rizzollo-Example1-SIM-raw`, `zenodo16966021-HILO-SIM-GFP-ER-timelapse` |
| `czi` | writer_version | `ZEN black 7.0` | descriptive | 6 | 6 | `zenodo10577621-Channel-ZStack-LineScan-Bidirectional-Averaging`, `zenodo10577621-LineScan-T3500`, `zenodo10577621-LineScan-T80-Z25` |
| `czi` | writer_version | `ZEN blue 1.1` | descriptive | 2 | 5 | `zenodo10577621-Intestine-3color-RAC`, `zenodo10577621-Kidney-RAC-3color` |
| `czi` | writer_version | `ZEN blue 2.3` | descriptive | 5 | 5 | `aics-OverViewScan`, `aics-s-1-t-1-c-1-z-1`, `aics-s-3-t-1-c-3-z-5` |
| `czi` | writer_version | `ZEN blue 2.6` | descriptive | 2 | 2 | `zenodo17098115-Snap-212a`, `zenodo17880403-Snap-8347` |
| `czi` | writer_version | `ZEN blue 3.0` | descriptive | 1 | 1 | `aics-variable-scene-shape-first-scene-pyramid` |
| `czi` | writer_version | `ZEN blue 3.3` | descriptive | 5 | 5 | `zenodo14895059-Figure-6C-D`, `zenodo7015307-S-1-3x3-T-3-Z-4-CH-2`, `zenodo7015307-S-2-3x3-T-1-Z-4-CH-2` |
| `czi` | writer_version | `ZEN blue 3.4` | descriptive | 3 | 3 | `aics-S-2-4x2-T-2-Z-3-CH-2`, `zenodo7015307-S-3-1Pos-2Mosaic-T-2-Z-3-CH-2`, `zenodo7015307-W96-B2-B4-S-2-T-1-Z-1-C-1-Tile-5x9` |
| `czi` | writer_version | `ZEN blue 3.5` | descriptive | 6 | 6 | `openslide-zeiss-5-cropped`, `openslide-zeiss-5-flat`, `openslide-zeiss-5-jxr` |
| `czi` | writer_version | `ZEN blue 3.6` | descriptive | 18 | 18 | `zenodo7015307-S-1-CH-2`, `zenodo7015307-S-2-2x2-CH-1`, `zenodo7015307-S-2-2x2-T-1-Z-4-CH-1` |
| `czi` | writer_version | `ZEN blue 3.7` | descriptive | 2 | 2 | `zenodo10659514-B30-FAM-probe-test`, `zenodo12509122-PSR-LXR-KO-WT-1054` |
| `czi` | writer_version | `pylibCZIrw 6.1` | descriptive | 14 | 16 | `synthetic-bgr24-jpeg`, `synthetic-bgr24-jpeg444`, `synthetic-bgr24-uncompressed` |

### Tests, fixtures, fuzz targets, snapshots

- integration tests: [`tests/fuzz_regressions.rs`](tests/fuzz_regressions.rs)
- committed fixtures: 10 files in [`tests/fixtures/`](tests/fixtures) (malformed ones are replayed through every reader by `openreadout`'s `tests/fuzz_regressions.rs`; all are snapshotted by its `tests/golden.rs`)
- fuzz targets (`fuzz/fuzz_targets/`): `czi_metadata_xml`, `czi_segment_walk`, `czi_subblock_header`, `whole_czi`
- corpus inputs by tier: full 4, heldout 13, smoke 46, standard 42
- golden snapshots: [`corpus/snapshots/czi.jsonl`](../../corpus/snapshots/czi.jsonl)

### Open new-variant intakes

None.
<!-- END GENERATED guide -->
