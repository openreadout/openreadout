# Maintaining `openreadout-tiff`

The TIFF family (`tiff`): TIFF 6.0 and BigTIFF, with OME-TIFF (single and multi-file, `BinaryOnly`, `*.companion.ome`), Zeiss LSM, MetaMorph STK / MetaSeries / `.nd` series, Aperio SVS, Hamamatsu NDPI, PerkinElmer QPTIFF, ImageJ hyperstacks, Micro-Manager, Nikon NIS-Elements exports, Leica SCN, Ventana BIF, Thermo Fisher EER movies and plain TIFF. Project-wide process: [docs/maintaining.md](../../docs/maintaining.md). Layout and vocabulary: [docs/formats/tiff.md](../../docs/formats/tiff.md); provenance: [docs/provenance/tiff.md](../../docs/provenance/tiff.md) (open specifications; tifffile, BSD-3, read as documentation; Bio-Formats as a black-box second opinion). Other crates depend on this one: `openreadout-oif` (plane TIFFs), `openreadout-hcs` (plate planes), `openreadout-zarr` (the OME-XML parser, a hidden item).

## Decode pipeline

1. **Detect** (`lib.rs`): the TIFF/BigTIFF header (`file_starts_like_tiff`), companion OME files (`is_companion_path`), MetaMorph `.nd` files (`looks_like_nd`).
2. **Container** (`container.rs` `TiffFile::open`): header (byte order, 42 or 43), the IFD chain (`walk_chain`, `read_ifd`), field values (`decode_value`). Never trusts an offset: bounds-checked, cycles detected, problems recorded for `check` instead of aborting.
3. **Flavour** (`dataset.rs` `Flavor`, `build_detected`; each `build_*` lives in its flavour's module, next to the parser): **the convention is the main branch**, detected from tags in priority order: OME-TIFF (`ome.rs`: OME-XML schemas 2008-02 … 2016-06; `try_ome`, `build_ome`, multi-file sets via `files.rs`), LSM (`lsm.rs`, tag 34412), STK (`metamorph.rs` UIC tags, `build_stk`), EER (`eer.rs`, compressions 65000–65002; `build_eer`), NIS-Elements exports (`nis.rs`, sibling files by name tokens), SVS (`flavors.rs` `parse_aperio`, `build_svs`), Leica SCN (`scn.rs`, the SCN XML's `dimension` elements name every page; `build_scn`, levels with explicit `Level.pages`), Philips TIFF (`philips.rs`), Ventana BIF (`bif.rs`, `iScan` XMP; `build_bif`), NDPI (`ndpi.rs`, `build_ndpi`), QPTIFF (`flavors.rs` `parse_qpi`, `build_qptiff`), MetaSeries XML (`metamorph.rs`), ImageJ (`imagej.rs`), Micro-Manager JSON (`flavors.rs` `apply_micromanager`), plain (`dataset.rs` `build_plain`); a MetaMorph `.nd` file opens its series (`nd.rs`). Each builds `Series` (images) with their plane maps (`PlaneSrc`: page and sample selection) and pyramid levels (SubIFDs or pages); plane and region reads are in `dataset/read.rs`, `ls` entries in `dataset/listing.rs`.
4. **Pixels** (`decode.rs` `read_page`, `read_region`): strips or tiles decoded into stored sample values (no photometric inversion, palette expansion or bit scaling); **codecs branch here** (`compression_name`): none, LZW, deflate, PackBits, zstd, JPEG (`jpeg_page_supported`, `jpeg_color`: the photometric tag and JFIF/Adobe markers decide the colour decoding), JPEG 2000 (`jpeg2000.rs`: Aperio 33003/33005, 34712), WebP, JPEG XL, LERC, old-style JPEG (`chunk_codecs.rs`), EER electron events (`eer.rs` `decode_counts`); packed 1–31-bit, half/24-bit float and complex-integer samples are widened in `decode_coded` (`SampleCoding`); predictors undone (`undo_horizontal`, `undo_float`). NDPI level 0 is read by JPEG restart intervals as tiles (`ndpi.rs`).

## Invariants and checks

- `check` (`check.rs`): header (byte order, version, first IFD); IFD chain inside the file, no loops, terminated; every out-of-line value inside the file; every page's dimensions, sample layout and strip/tile count match its geometry; every strip/tile inside the file; every (c, z, t) plane of every image maps to an existing page; every JPEG page's colour coding is one the decoder handles; OME `TiffData` references (files exist and are TIFFs, IFD indices exist, UUIDs agree).
- Refused (exit 6): packed, half/24-bit float and complex-integer samples inside JPEG/JPEG 2000/WebP/JPEG XL/LERC chunks; EER codings outside 2–16-bit runs; volumetric BIF; LERC in default builds; NDPI and LSM files above 4 GiB with wrapped offsets.

## Debugging a new file

- `openreadout report FILE` gives the flavour, writer, codec with its colour space and layout (the assurance fingerprint folds the photometric interpretation into colour-sensitive codecs: `jpeg (rgb)` ≠ `jpeg (ycbcr)`).
- `openreadout info FILE --view structure` lists every IFD with its tags and strips/tiles; `info --view full --json` → `vendor` has the tags, OME-XML and flavour metadata.
- Tests: `tests/fixtures.rs` (committed files against oracle JSON), `tests/other_codecs.rs` (`tests/fixtures/codecs/`: every compression with `.expected` samples), `tests/jpeg_colour.rs`, `tests/jpeg2000_pages.rs`, `tests/metamorph.rs`, `tests/nis.rs`, `tests/corpus_pages.rs`. `oracle/make_tiff_jpeg_fixtures.py` regenerates JPEG fixtures.
- Oracles: tifffile + imagecodecs (primary), Bio-Formats (second opinion).

## Fragile spots

- Flavour builders share `TiffDataset`'s state (`series`, `attachments`, `notes`, `provenance`) and the helpers in `dataset.rs` (`push_slide` for whole-slide images, `sub_ifd_levels`, `set_provenance`); a change there touches every flavour. Add a flavour's unit tests before touching shared assembly code.
- JPEG colour handling (YCbCr vs RGB, Adobe transform flags) and irreversible JPEG 2000 (within one grey level of OpenJPEG) are where values differ from other readers.
- OME Modulo annotations (FLIM/lambda sub-dimensions) are not expanded; Micro-Manager multi-file datasets depend on file naming.
- The OME-XML parser is used by `openreadout-zarr` as a hidden item: keep it source-compatible within a release series.

<!-- BEGIN GENERATED guide -->
## Facts (generated)

*Generated by `cargo xtask guides --write` from the sources, `corpus/manifest.toml`, `corpus/assurance/evidence.json` and `corpus/intake/`; do not edit. CI fails when it is stale.*

### Formats

| format id | notes and provenance | confidence | basis | development files: read / confirmed | depositors | held-out pass / fail |
| --- | --- | --- | --- | --- | --- | --- |
| `tiff` | [format note](../../docs/formats/tiff.md), [provenance log](../../docs/provenance/tiff.md) | high | open spec | 113 / 112 | 22 | 5 / 0 |

### Source map

| file | what it does (its module documentation) |
| --- | --- |
| [`src/assurance.rs`](src/assurance.rs) | Assurance profile (`docs/assurance.md`): the variant features of a TIFF-family file (sub-format, writer, codec with its colour space, sample arrangement, layout) and the feature… |
| [`src/bif.rs`](src/bif.rs) | Roche Ventana BIF whole-slide files (iScan HT, Coreo, DP 200/600): a BigTIFF whose pages are named by their `ImageDescription` (`level=N mag=M quality=Q` pyramid levels… |
| [`src/check.rs`](src/check.rs) | `check`: structural validation of the IFD chain, strip/tile extents and the OME plane map |
| [`src/chunk_codecs.rs`](src/chunk_codecs.rs) | Chunks coded with WebP (50001), JPEG XL (50002, 52546), LERC (34887) and old-style JPEG (6) |
| [`src/container.rs`](src/container.rs) | The TIFF container: header, IFD chain, field values |
| [`src/dataset.rs`](src/dataset.rs) | `Dataset` implementation: sub-format detection, image (series) assembly, plane reads, listing and integrity checks |
| [`src/dataset/growing.rs`](src/dataset/growing.rs) | Growing-file evidence for a TIFF that is still being written |
| [`src/dataset/listing.rs`](src/dataset/listing.rs) | `ls`: the structure of a TIFF data set (header, metadata blocks, pages, pyramid levels and member files) |
| [`src/dataset/read.rs`](src/dataset/read.rs) | Plane, pyramid-level and region reads: where a plane is stored and how its bytes are decoded |
| [`src/decode.rs`](src/decode.rs) | Strip/tile decoding into one plane |
| [`src/eer.rs`](src/eer.rs) | Thermo Fisher EER (Electron Event Representation) movies from Falcon 4/4i/C cameras: a BigTIFF with one page per detector frame, each page one strip of electron events coded as |
| [`src/files.rs`](src/files.rs) | The set of files an image may span: the opened file plus OME-TIFF siblings and companion metadata files |
| [`src/flavors.rs`](src/flavors.rs) | Small text conventions layered on TIFF by slide scanners and acquisition software: Aperio SVS descriptions, PerkinElmer QPI page XML, Micro-Manager JSON |
| [`src/imagej.rs`](src/imagej.rs) | ImageJ `ImageDescription` (`ImageJ=…` key=value lines) and the ImageJ binary metadata tags |
| [`src/jpeg2000.rs`](src/jpeg2000.rs) | JPEG 2000 tiles and strips (compression 33003, 33004, 33005 and 34712) |
| [`src/lib.rs`](src/lib.rs) | Clean-room reader for the TIFF family |
| [`src/lsm.rs`](src/lsm.rs) | Zeiss LSM: the info record in private tag 34412 and its sub-records |
| [`src/mdgel.rs`](src/mdgel.rs) | Molecular Dynamics GEL files (`.gel` of Typhoon, Storm and FLA scanners): TIFF pages carrying the MD tags 33445-33452 |
| [`src/metamorph.rs`](src/metamorph.rs) | MetaMorph conventions: STK files (UIC1–UIC4 private tags), MetaSeries TIFFs (`<MetaData>` XML in `ImageDescription`) and `.nd` series files |
| [`src/nd.rs`](src/nd.rs) | MetaMorph `.nd` series: the TIFF/STK files a `.nd` file names, opened as one data set (stage positions as images, wavelengths as channels) |
| [`src/ndpi.rs`](src/ndpi.rs) | Tiled access to the full-resolution page of a Hamamatsu NDPI file |
| [`src/nis.rs`](src/nis.rs) | Nikon NIS-Elements TIFF exports: the private double-valued tags on page 0 and the index tokens at the end of the file names that tie one export's files together |
| [`src/ome.rs`](src/ome.rs) | OME-XML (the open OME data model, schemas 2008-02 … 2016-06) → the subset we normalize |
| [`src/philips.rs`](src/philips.rs) | Philips TIFF whole-slide exports: page 0's `ImageDescription` is an XML `DataObject` tree (`ObjectType="DPUfsImport"`) of DICOM-named attributes; the reduced pages are the pyramid, |
| [`src/scn.rs`](src/scn.rs) | Leica SCN whole-slide files (SCN400, SCN400F, Aperio Versa exports of that layout): a BigTIFF whose first `ImageDescription` is an XML document (namespace… |
| [`src/tags.rs`](src/tags.rs) | Tag numbers used by the reader (TIFF 6.0, TIFF Technical Notes, and the private tags the microscopy conventions use) |

### Where variants branch

The assurance profile ([`src/assurance.rs`](src/assurance.rs)) observes these features (each value is looked up in the validated table below; a value never confirmed makes the file `unvalidated` for the feature's outputs) and reports these structures and assumptions:

- feature writer `&n`
- feature format_version `part`
- feature sample_layout `layout`
- feature layout `"pyramid"`
- feature layout `"multi_file"`
- feature dialect `flavor`
- feature sample_layout `format!("Molecular Dynamics GEL, {encoding} data")`
- feature codec `value`
- feature sample_layout `if l.planar == 2 { "planar samples" } else { "interleaved samples" }`
- feature codec `format!("predictor {}", l.predictor)`
- feature sample_layout `format!("{} bits per sample", l.bits_per_sample)`
- feature sample_layout `"half-float samples"`
- feature sample_layout `"complex-integer samples"`
- feature sample_layout `"complex-float samples"`
- feature sample_layout `"fill order 2"`
- feature layout `if l.tiled { "tiles" } else { "strips" }`
- feature writer_version `format!("{n} {v}")` (descriptive)
- feature instrument `m` (descriptive)
- undecoded "tile overlaps (Ventana BIF)"
- undecoded "sub-format metadata"

### Validated variants and the corpus files that pin them

| format | kind | value | outputs | confirmed files | read | example corpus files |
| --- | --- | --- | --- | --- | --- | --- |
| `tiff` | codec | `deflate` | pixels | 6 | 6 | `aics-tiff-actk`, `aics-tiff-s-1-t-1-c-1-z-1-ome-tiff-tiles`, `aics-tiff-s-1-t-1-c-10-z-1-ome-tiff-tiles` |
| `tiff` | codec | `eer 7+2+2` | pixels | 3 | 3 | `empiar11906-falcon4i-eer`, `empiar12080-falcon4-eer`, `empiar13509-falcon4i-eer` |
| `tiff` | codec | `jpeg (minisblack)` | pixels | 4 | 4 | `gdal-byte-jpg-tablesmodezero`, `gdal-byte-ovr-jpeg-tablesmode1`, `gdal-byte-ovr-jpeg-tablesmode3` |
| `tiff` | codec | `jpeg (rgb)` | pixels | 1 | 3 | `openslide-aperio-cmu-1-small-region` |
| `tiff` | codec | `jpeg (ycbcr)` | pixels | 3 | 7 | `openslide-leica-1`, `openslide-leica-fluorescence-1`, `zenodo14025917-ex3-5x` |
| `tiff` | codec | `jpeg-2000 (rgb)` | pixels | 1 | 2 | `openslide-aperio-jp2k-33003-1` |
| `tiff` | codec | `jpeg-xl (rgb)` | pixels | 1 | 1 | `gdal-jxl-rgbsmall-tiled-separate` |
| `tiff` | codec | `lzw` | pixels | 5 | 5 | `aics-s-1-t-1-c-1-z-1-ome-tiff`, `aics-s-3-t-1-c-3-z-5-ome-tiff`, `aics-tiff-4c-3z-pyramid` |
| `tiff` | codec | `none` | pixels | 71 | 71 | `aics-OverViewScan-ome-tiff`, `aics-tiff-image-stack-tpzc-50tp-2p-5z-3c-512k-1-mmstack-2-pos000-000`, `aics-tiff-image-stack-tpzc-50tp-2p-5z-3c-512k-1-mmstack-2-pos001-000` |
| `tiff` | codec | `old-jpeg (ycbcr)` | pixels | 0 | 1 |  |
| `tiff` | codec | `packbits` | pixels | 4 | 4 | `gdal-contig-strip`, `gdal-contig-tiled`, `gdal-separate-tiled` |
| `tiff` | codec | `predictor 2` | pixels | 2 | 2 | `aics-tiff-4c-3z-pyramid`, `gdal-bug4468` |
| `tiff` | codec | `webp (rgb)` | pixels | 2 | 2 | `gdal-tif-webp`, `gdal-webp-rgbsmall-tiled` |
| `tiff` | codec | `zstd` | pixels | 1 | 1 | `gdal-byte-zstd` |
| `tiff` | dialect | `aperio-svs` | metadata, pixels | 4 | 4 | `openslide-aperio-cmu-1`, `openslide-aperio-cmu-1-jp2k-33005`, `openslide-aperio-cmu-1-small-region` |
| `tiff` | dialect | `hamamatsu-ndpi` | metadata, pixels | 1 | 1 | `openslide-hamamatsu-cmu-1` |
| `tiff` | dialect | `imagej` | metadata, pixels | 4 | 4 | `aics-tiff-s-1-t-1-c-1-z-1`, `aics-tiff-s-1-t-10-c-3-z-1`, `gel-zenodo5773282-resaved-no-md-tags` |
| `tiff` | dialect | `leica-scn` | metadata, pixels | 2 | 2 | `openslide-leica-1`, `openslide-leica-fluorescence-1` |
| `tiff` | dialect | `metamorph-nd` | metadata, pixels | 4 | 4 | `metamorph-figshare7583960-nd`, `metamorph-ssbd232-drd2-4well-dish2-nd`, `metamorph-ssbd232-vec35-dish1-nd` |
| `tiff` | dialect | `metamorph-stk` | metadata, pixels | 2 | 2 | `metamorph-figshare12981617-ed4a-sdc405-stk`, `metamorph-figshare7583960-w1-t1-stk` |
| `tiff` | dialect | `metaseries` | metadata, pixels | 1 | 1 | `metamorph-zenodo13642395-test-timelapse-20240816-s1-t1` |
| `tiff` | dialect | `nis-elements` | metadata, pixels | 1 | 1 | `nis-zenodo7677827-xy01c1` |
| `tiff` | dialect | `ome-companion` | metadata, pixels | 1 | 1 | `ome-companion-multifile-companion` |
| `tiff` | dialect | `ome-tiff` | metadata, pixels | 36 | 36 | `aics-OverViewScan-ome-tiff`, `aics-s-1-t-1-c-1-z-1-ome-tiff`, `aics-s-3-t-1-c-3-z-5-ome-tiff` |
| `tiff` | dialect | `perkinelmer-qptiff` | metadata, pixels | 2 | 2 | `ome-qptiff-hande-compressed-scan1`, `ome-qptiff-hne-3-component` |
| `tiff` | dialect | `philips-tiff` | metadata, pixels | 2 | 2 | `openslide-philips-1`, `openslide-philips-4` |
| `tiff` | dialect | `plain` | metadata, pixels | 45 | 46 | `aics-tiff-4c-3z-pyramid`, `empiar13509-falcon4i-gain`, `gdal-1bit-2bands` |
| `tiff` | dialect | `thermo-eer` | metadata, pixels | 3 | 3 | `empiar11906-falcon4i-eer`, `empiar12080-falcon4-eer`, `empiar13509-falcon4i-eer` |
| `tiff` | dialect | `ventana-bif` | metadata, pixels | 1 | 1 | `openslide-ventana-1` |
| `tiff` | dialect | `zeiss-lsm` | metadata, pixels | 3 | 3 | `zenodo14510432-lsm-10-01`, `zenodo5781661-lsm-time-1-43`, `zenodo5781661-lsm-z0-7-st-1321` |
| `tiff` | field | `experiment.acquisition.started_at` | descriptive | 18 | 18 | `empiar11906-falcon4i-eer`, `empiar13509-falcon4i-eer`, `metamorph-figshare12981617-ed4a-sdc405-stk` |
| `tiff` | field | `experiment.instrument.model` | descriptive | 8 | 8 | `openslide-aperio-cmu-1`, `openslide-aperio-cmu-1-jp2k-33005`, `openslide-aperio-cmu-1-small-region` |
| `tiff` | format_version | `6.0` | metadata, pixels | 92 | 93 | `aics-OverViewScan-ome-tiff`, `aics-s-1-t-1-c-1-z-1-ome-tiff`, `aics-s-3-t-1-c-3-z-5-ome-tiff` |
| `tiff` | format_version | `6.0+BigTIFF` | metadata, pixels | 15 | 15 | `aics-tiff-actk`, `empiar11906-falcon4i-eer`, `empiar12080-falcon4-eer` |
| `tiff` | format_version | `MetaMorph ND 1.0` | metadata, pixels | 1 | 1 | `metamorph-figshare7583960-nd` |
| `tiff` | format_version | `MetaMorph ND 2.0` | metadata, pixels | 3 | 3 | `metamorph-ssbd232-drd2-4well-dish2-nd`, `metamorph-ssbd232-vec35-dish1-nd`, `metamorph-zenodo13642395-nd` |
| `tiff` | format_version | `OME-XML 2015-01` | metadata | 2 | 2 | `mm-thomas-test-stack`, `mm-thomas-test2-stack` |
| `tiff` | format_version | `OME-XML 2016-06` | metadata | 35 | 35 | `aics-OverViewScan-ome-tiff`, `aics-s-1-t-1-c-1-z-1-ome-tiff`, `aics-s-3-t-1-c-3-z-5-ome-tiff` |
| `tiff` | instrument | `Amersham TYPHOON` | descriptive | 1 | 1 | `gel-zenodo17516010-typhoon-phosphor` |
| `tiff` | instrument | `Amersham Typhoon` | descriptive | 1 | 1 | `gel-zenodo15688057-typhoon-fluorescence` |
| `tiff` | instrument | `Eclipse TE300` | descriptive | 6 | 6 | `ome-tubhiswt-2d-tubhiswt-c0`, `ome-tubhiswt-2d-tubhiswt-c1`, `ome-tubhiswt-3d-tubhiswt-c0` |
| `tiff` | instrument | `Leica SCN400` | descriptive | 1 | 1 | `openslide-leica-1` |
| `tiff` | instrument | `Leica SCN400F` | descriptive | 1 | 1 | `openslide-leica-fluorescence-1` |
| `tiff` | instrument | `NanoZoomer` | descriptive | 1 | 1 | `openslide-hamamatsu-cmu-1` |
| `tiff` | instrument | `ScanScope CPAPERIOCS` | descriptive | 3 | 3 | `openslide-aperio-cmu-1`, `openslide-aperio-cmu-1-jp2k-33005`, `openslide-aperio-cmu-1-small-region` |
| `tiff` | instrument | `ScanScope SS1283` | descriptive | 1 | 1 | `openslide-aperio-jp2k-33003-1` |
| `tiff` | instrument | `Typhoon FLA 9500` | descriptive | 1 | 1 | `gel-zenodo5786227-typhoon-fla9500` |
| `tiff` | instrument | `VENTANA DP 200` | descriptive | 1 | 1 | `openslide-ventana-1` |
| `tiff` | layout | `multi_file` | metadata, pixels | 17 | 17 | `aics-tiff-image-stack-tpzc-50tp-2p-5z-3c-512k-1-mmstack-2-pos000-000`, `aics-tiff-image-stack-tpzc-50tp-2p-5z-3c-512k-1-mmstack-2-pos001-000`, `metamorph-figshare7583960-nd` |
| `tiff` | layout | `pyramid` | pixels | 9 | 16 | `aics-OverViewScan-ome-tiff`, `aics-variable-scene-shape-first-scene-pyramid-ome-tiff`, `gdal-byte-ovr-jpeg-tablesmode1` |
| `tiff` | layout | `strips` | pixels | 88 | 89 | `aics-OverViewScan-ome-tiff`, `aics-s-1-t-1-c-1-z-1-ome-tiff`, `aics-s-3-t-1-c-3-z-5-ome-tiff` |
| `tiff` | layout | `tiles` | pixels | 15 | 22 | `aics-OverViewScan-ome-tiff`, `aics-tiff-s-1-t-1-c-1-z-1-ome-tiff-tiles`, `aics-tiff-s-1-t-1-c-10-z-1-ome-tiff-tiles` |
| `tiff` | sample_layout | `1 bits per sample` | pixels | 6 | 6 | `gdal-1bit-2bands`, `gdal-empty1bit`, `gdal-oddsize-1bit2b` |
| `tiff` | sample_layout | `10 bits per sample` | pixels | 1 | 1 | `gdal-int10` |
| `tiff` | sample_layout | `12 bits per sample` | pixels | 1 | 1 | `gdal-int12` |
| `tiff` | sample_layout | `128 bits per sample` | pixels | 1 | 1 | `gdal-cfloat64` |
| `tiff` | sample_layout | `24 bits per sample` | pixels | 1 | 1 | `gdal-float24` |
| `tiff` | sample_layout | `Molecular Dynamics GEL, square root data` | pixels | 3 | 3 | `gel-zenodo15688057-typhoon-fluorescence`, `gel-zenodo17516010-typhoon-phosphor`, `gel-zenodo5786227-typhoon-fla9500` |
| `tiff` | sample_layout | `complex` | pixels | 4 | 4 | `gdal-cfloat32`, `gdal-cint-sar`, `gdal-cint16` |
| `tiff` | sample_layout | `complex-float samples` | pixels | 3 | 3 | `gdal-cfloat32`, `gdal-cfloat64`, `gdal-complex-float32` |
| `tiff` | sample_layout | `complex-integer samples` | pixels | 4 | 4 | `gdal-cint-sar`, `gdal-cint16`, `gdal-cint32` |
| `tiff` | sample_layout | `double` | pixels | 2 | 2 | `aics-tiff-actk`, `gdal-dbl-min` |
| `tiff` | sample_layout | `double-complex` | pixels | 3 | 3 | `gdal-cfloat64`, `gdal-cint32`, `gdal-complex-int32` |
| `tiff` | sample_layout | `float` | pixels | 8 | 8 | `empiar13509-falcon4i-gain`, `gdal-float16`, `gdal-float24` |
| `tiff` | sample_layout | `half-float samples` | pixels | 1 | 1 | `gdal-float16` |
| `tiff` | sample_layout | `int64` | pixels | 1 | 1 | `gdal-int64` |
| `tiff` | sample_layout | `int8` | pixels | 11 | 11 | `ome-artificial-4d-series-tiff`, `ome-artificial-multi-channel-4d-series-btf`, `ome-artificial-multi-channel-4d-series-tiff` |
| `tiff` | sample_layout | `interleaved samples` | pixels | 16 | 24 | `aics-tiff-4c-3z-pyramid`, `aics-tiff-s-1-t-1-c-2-z-1-rgb`, `gdal-1bit-2bands` |
| `tiff` | sample_layout | `planar samples` | pixels | 6 | 6 | `gdal-jxl-rgbsmall-tiled-separate`, `gdal-md-dg`, `gdal-separate-tiled` |
| `tiff` | sample_layout | `uint16` | pixels | 28 | 28 | `aics-OverViewScan-ome-tiff`, `aics-s-1-t-1-c-1-z-1-ome-tiff`, `aics-s-3-t-1-c-3-z-5-ome-tiff` |
| `tiff` | sample_layout | `uint64` | pixels | 1 | 1 | `gdal-uint64` |
| `tiff` | sample_layout | `uint8` | pixels | 34 | 34 | `aics-OverViewScan-ome-tiff`, `aics-variable-scene-shape-first-scene-pyramid-ome-tiff`, `empiar11906-falcon4i-eer` |
| `tiff` | sample_layout | `uint8x2` | pixels | 2 | 2 | `gdal-1bit-2bands`, `gdal-oddsize-1bit2b` |
| `tiff` | sample_layout | `uint8x3` | pixels | 13 | 21 | `aics-tiff-4c-3z-pyramid`, `aics-tiff-s-1-t-1-c-2-z-1-rgb`, `gdal-cielab` |
| `tiff` | sample_layout | `uint8x4` | pixels | 1 | 1 | `gdal-bug4468` |
| `tiff` | writer | `Amersham TYPHOON Scanner Control Software` | metadata, pixels | 1 | 1 | `gel-zenodo17516010-typhoon-phosphor` |
| `tiff` | writer | `Amersham Typhoon Scanner Control Software` | metadata, pixels | 1 | 1 | `gel-zenodo15688057-typhoon-fluorescence` |
| `tiff` | writer | `Aperio Image Library` | metadata, pixels | 4 | 4 | `openslide-aperio-cmu-1`, `openslide-aperio-cmu-1-jp2k-33005`, `openslide-aperio-cmu-1-small-region` |
| `tiff` | writer | `Bio-Formats` | metadata, pixels | 28 | 28 | `aics-OverViewScan-ome-tiff`, `aics-s-1-t-1-c-1-z-1-ome-tiff`, `aics-s-3-t-1-c-3-z-5-ome-tiff` |
| `tiff` | writer | `ImageJ` | metadata, pixels | 4 | 4 | `aics-tiff-s-1-t-1-c-1-z-1`, `aics-tiff-s-1-t-10-c-3-z-1`, `gel-zenodo5773282-resaved-no-md-tags` |

… 27 more values: the generated table in `src/assurance.rs` has all of them.

### Tests, fixtures, fuzz targets, snapshots

- integration tests: [`tests/corpus_pages.rs`](tests/corpus_pages.rs), [`tests/eer.rs`](tests/eer.rs), [`tests/fixtures.rs`](tests/fixtures.rs), [`tests/jpeg2000_pages.rs`](tests/jpeg2000_pages.rs), [`tests/jpeg_colour.rs`](tests/jpeg_colour.rs), [`tests/metamorph.rs`](tests/metamorph.rs), [`tests/nis.rs`](tests/nis.rs), [`tests/other_codecs.rs`](tests/other_codecs.rs)
- committed fixtures: 61 files in [`tests/fixtures/`](tests/fixtures) (malformed ones are replayed through every reader by `openreadout`'s `tests/fuzz_regressions.rs`; all are snapshotted by its `tests/golden.rs`)
- fuzz targets (`fuzz/fuzz_targets/`): `whole_tiff`
- corpus inputs by tier: full 1, heldout 10, smoke 85, standard 35
- golden snapshots: [`corpus/snapshots/tiff.jsonl`](../../corpus/snapshots/tiff.jsonl)

### Open new-variant intakes

None.
<!-- END GENERATED guide -->
