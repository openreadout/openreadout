# Provenance log — OME-Zarr / OME-NGFF (`ome-zarr`)

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

## 2026-09-22 — reader (Richard Zimring with Claude as assistant)

OME-Zarr is an open standard; no vendor format is involved and nothing was reverse engineered.

**Specifications read:**
- OME-NGFF 0.1–0.5, https://ngff.openmicroscopy.org/ (open specification; `multiscales`, `axes`, `coordinateTransformations`, `omero`, `labels`/`image-label`, `plate`, `well`, the `bioformats2raw.layout` transitional convention, RFC-9 zipped OME-Zarr).
- Zarr v2 and v3 storage specifications, https://zarr-specs.readthedocs.io/ (open specification; `.zarray`/`.zattrs`/`.zgroup`, `zarr.json`, chunk key encodings, `sharding_indexed`, `crc32c`).
- PKWARE `APPNOTE.TXT` (public ZIP specification) for the zip index (end-of-central-directory, ZIP64 records, central directory, local headers).
- c-blosc `README_CHUNK_FORMAT.rst` (https://github.com/Blosc/c-blosc, BSD-3-Clause) for the Blosc 1 header, `bstarts` and split streams; c-blosc `blosc/blosc.c` (`blosc_d`, `split_block`: when a block is split into one stream per byte of the type, and that a split whose compressed size equals its decoded size is stored raw) and `blosc/blosclz.c` (`blosclz_decompress`) read as BSD-3 prior art for `openreadout-codecs/src/blosc.rs`.
- HDF5 LZ4 filter layout (`hdf5_lz4_decode`, used by the Imaris reader): see `docs/provenance/ims.md`.

**Libraries used (not read as prior art for the format):** `zarrs` 0.23 (MIT OR Apache-2.0) for arrays, codec pipelines and sharding; its runtime codec registry (`register_codec_v2/v3`) was read in its source to plug in our decoders. `lz4_flex` 0.12 (MIT) for LZ4 blocks; `ruzstd` (MIT) for zstd; `flate2` (MIT OR Apache-2.0) for deflate.

**C-free decision:** `zarrs`' own `blosc` and `zstd` codecs link `blosc-src` and `zstd-sys` (C). The workspace keeps them off; `openreadout-zarr/src/codecs.rs` registers decode-only replacements. Validated bit for bit: Blosc lz4 + byte shuffle (Fractal corpus stores, fixtures), Blosc blosclz + shuffle, Blosc zstd inside shards, plain zstd (v2 numcodecs and v3 default), zlib, gzip.

**Oracle:** zarr-python 3.4 (MIT) decodes every array in `oracle/gen.py` (`ome_zarr_`); the NGFF metadata is interpreted in `gen.py` from the specification, independently of the Rust code (plate wells in `plate.wells` order, fields in `well.images` order, bioformats2raw series from `OME` → `series`). ome-zarr-py 0.19 (BSD-2-Clause) writes the synthetic fixtures' metadata.

**Corpus files used:**
- `zenodo13982701-organoid-mip`, `zenodo13982701-organoid` (Zenodo 13982701, CC-BY-4.0, Nicole Repina / FMI, Fractal): NGFF 0.4 HCS plate, Blosc lz4, zipped `.zarr` folder.
- `zenodo13305156-cardio-mip`, `zenodo13305156-cardio` (Zenodo 13305156, CC-BY-4.0, Fractal).
- Synthetic, generated 2026-09-22 by `oracle/make_zarr_fixtures.py` (zarr-python 3.4.0, ome-zarr-py 0.19.2, numcodecs 0.17.0) and committed as zip stores in `crates/openreadout-zarr/tests/fixtures/` with zarr-python oracles in `tests/fixtures/oracle/`: `ngff04-v2-tczyx-blosc-lz4`, `ngff04-v2-zyx-float-codecs`, `ngff05-v3-yx-zstd-stored`, `ngff05-v3-czyx-sharded`, `ngff04-v2-plate`, `ngff04-bf2raw-collection`. Not from a microscope; the OME-XML of the collection is hand-written.

**Inferred (ours, not in the specification):** image order and names for plates (`<well>/<field>`) and collections; axes without the standard names mapped by `type`; physical sizes only when an axis has a unit; `translation` reported, not applied; label images listed, not exposed.

## 2026-09-23 — growing stores (`write_state`)

**Scope:** a new, read-only judgement; reading is unchanged. `Dataset::write_state` reports a directory store as unfinished when the stored level-0 chunk files of an image form a strict prefix of the chunk grid in C order; a plane is complete when every chunk it touches is stored. Holes before a stored chunk mean a sparse array (finished). Zip stores are never reported.

**Files used:** synthetic stores written by our OME-Zarr exporter from a synthetic TIFF (`crates/openreadout-live/tests/replay.rs`).

**Inferred:** that acquisition software writes chunks in C order of the grid (`t` outermost) is an assumption, documented in `book/src/guides/lab-shares.md`.

**Prior art consulted:** none.

## 2026-09-23 — Region reads (Richard Zimring with Claude as assistant)

**Scope.** `Dataset::read_region`: the x/y ranges of the retrieved array subset are the rectangle's instead of the whole level, so zarrs decodes only the chunks it overlaps. `resolution_levels` in `info` lists each level's shape and chunk shape (x/y) from the array metadata. No new interpretation of the store.

**Oracle:** zarr-python 3 (MIT) array slicing.

## 2026-09-24 — Zip containers read by the shared `openreadout_core::zip`

**Scope:** the crate-private zip reader is replaced by one shared module in `openreadout-core` (`zip.rs`), used by qPCR (RDML, `.eds`, `.pcrd`), OpenLab CDS (`.dx`, `.rx`) and zipped OME-Zarr stores. No new interpretation of any format. The shared reader is the union of the three copies: central directory with ZIP64 records, case-insensitive and exact name lookup, stored and deflated members checked against their CRC-32, partial reads of stored members, a deterministic writer. Hardening over the copies: every offset and length is checked against the file size before it is read (a range past the end is exit 4, not an I/O error); a declared member count the central directory cannot hold is corrupt; inflation is bounded by the member's declared size (`miniz_oxide::inflate::decompress_to_vec_with_limit`, so a zip bomb stops there instead of pre-reserving or growing past it); the archive is read through one byte source with positional reads instead of a file opened per member. Fuzz target `core_zip`; unit tests flip and truncate every byte of a small archive.

**Prior art consulted:** PKWARE APPNOTE.TXT (public specification), as before. No new corpus files.

## 2026-09-25 — OME-XML of bioformats2raw stores read with the OME-TIFF reader's helpers

**Scope:** `OME/METADATA.ome.xml` is mapped through the helpers the OME-TIFF reader now shares (`docs/provenance/tiff.md`, same date): acquisition modes with `ContrastMethod`, detection bands from emission filters, `Plane/@ExposureTime` → `exposure_ms`, and our exporter's `openreadout.dev/normalized` annotations (software, exact acquisition time). Found by exporting `bsst749-4ii-bodipy-ctl` and `aics-s-3-t-1-c-3-z-5` with `--to ome-zarr` and running `check --against` (CZI exposures were lost). **Prior art consulted:** the OME 2016-06 XSD.

## 2026-09-25 — assurance profile (`src/assurance.rs`)

**Corpus files:** every development input of this format on disk (`corpus/assurance/evidence.json` lists them); no held-out file.
**Prior art consulted:** none.
**What was done.** No parsing logic changed. The reader declares an assurance profile (docs/assurance.md) that fingerprints each file from this reader's own normalized output: the NGFF and Zarr versions, the store kind, dtype, axes, plate layout and the codec pipeline of every array (`ArrayMeta.codecs`, through `Dataset::assurance_observations`). The table of validated feature values and the confidence level are generated from the development corpus by `cargo xtask assurance-audit`.

## 2026-09-26 — Blosc bit shuffle and Snappy; bool, 64-bit, half-float and complex arrays; label images; composed translations

**Prior art consulted** (read as documentation): c-blosc `blosc/shuffle.c` (BSD-3, `blosc_internal_bitunshuffle`: a block of a multiple of 8 elements is bit-unshuffled whole, the bytes after the last whole element are copied, other blocks are stored unshuffled) and `blosc/blosc.c` (`blosc_d`: bit shuffle applies when the block is at least one element long); the bitshuffle library (github.com/kiyo-masui/bitshuffle, MIT; `bitshuffle_core.c`, `bshuf_trans_bit_elem_scal` and its inverse): the transform is a byte transpose, a bit transpose of every 8 bytes and a transpose of the bit rows, so that bit `k` of byte `j` of element `i` lands at bit `i % 8` of byte `i / 8` of bit row `j * 8 + k`; the Snappy format description (github.com/google/snappy `format_description.txt`, BSD-3): varint length, literal and copy elements with 1/2/4-byte offsets.

**Test data:** numcodecs' Blosc fixtures (github.com/zarr-developers/numcodecs commit `1f83681`, `fixture/blosc/`, MIT; vendored in `crates/openreadout-codecs/tests/fixtures/numcodecs-blosc/`): lz4, blosclz and snappy with bit shuffle, lz4 with byte shuffle and 256-byte blocks; every chunk decodes to its stored array except the 1000-boolean array 03, which c-blosc 1.21 (numcodecs 0.17) does not reproduce either. numcodecs 0.17 is built without Snappy, so the Snappy fixtures are checked against the stored arrays alone. New synthetic fixture `ngff04-bf2raw-dtypes` (`oracle/make_zarr_fixtures.py`, zarr-python 3.4): int64, uint64, float16, bool, complex64 and complex128 series with Blosc bit shuffle, and composed dataset and multiscales translations. Corpus: `zenodo14641597-idr6001240` (Euro-BioImaging's zip of IDR image 6001240 with a label image), `zenodo14841309-scportrait-input` (a macOS zip: a `__MACOSX` folder beside the store folder, which previously made the store unreadable), `zenodo22727474-ri-c2bf7d19` (float32 time series) — Zenodo, CC-BY-4.0.

**Oracle:** zarr-python 3.4 decodes every array; `oracle/gen.py` now lists label images after the images (by the NGFF specification's `labels` list, duplicates once) and widens float16 / converts bool as the reader does.

**Inferred:** nothing beyond the specification and the codec documents. The composition order is the NGFF 0.4 specification's (https://ngff.openmicroscopy.org/0.4/, section "multiscales metadata"): a dataset lists exactly one scale and at most one translation after it, and the multiscales-level transformations "are applied after them".


## 2026-10-02 — OME-XML next to an image at the root

**Corpus files:** none new; the reader's fixtures (`crates/openreadout-zarr/tests/fixtures/`) and OpenReadout's own exports of `crates/openreadout-cli/tests/fixtures/mini.{nd2,lif,czi}`.
**Prior art consulted:** the OME-NGFF 0.4/0.5 specification's `bioformats2raw.layout` section (https://ngff.openmicroscopy.org/, CC-BY-4.0): the `OME` group holds `METADATA.ome.xml`.
**What was decided.** OpenReadout's OME-Zarr writer now also writes `OME/METADATA.ome.xml` (and an empty `OME` group) for a single image stored at the root, as it did for collections, so the objective, instrument, acquisition mode, exposures and fluorophores survive the export. When a root image's store has `OME/METADATA.ome.xml`, the reader applies OME `Image` 0 to it as it does for collection series. A document whose `Creator` is `openreadout …` is authoritative for channel colours: OpenReadout's writer gives every `omero` channel a display colour and states in the OME-XML whether the source recorded one, so a channel without an OME `Color` has none. Other writers' colours are read as before. The writer no longer invents a `name` ("Image N") for an unnamed source image (OME-NGFF makes it optional).

## 2026-10-07 — First public NGFF 0.5 stores; tile size of sharded arrays

**Why.** The October imaging bug hunt (`docs/benchmark/hunt-2026-10-imaging.md`) noted that NGFF 0.5 had no development file: only our synthetic fixtures and our own exports covered Zarr v3 with `ome` attributes.

**Corpus files used (new):** `idr0062A-6001240-labels-ngff05` and `idr0066-chicken-embryo-mip-ngff05`, the Image Data Resource's NGFF 0.5 sample stores (`zarr/v0.5/` on its EBI S3 endpoint, CC BY 4.0 by each store's `ro-crate-metadata.json`), copied file by file. The first is the same image as `zenodo14641597-idr6001240` (NGFF 0.4), written again by omero-zarr as 0.5 with sharded uint16 arrays (shards of 1 × 10 × 512 × 512, inner chunks of 1 × 1 × 256 × 256, Blosc zstd with byte shuffle, crc32c shard index) and an unsharded int8 label image (zstd). The second is a 6510 × 8978 uint8 mesoSPIM projection with 8 levels written by ome2024-ngff-challenge 1.0.2 (shards of 2048 × 2048, inner chunks of 256 × 256, Blosc zstd with bit shuffle).

**Prior art consulted:** the OME-NGFF 0.5 specification (https://ngff.openmicroscopy.org/0.5/, CC-BY-4.0): metadata under the `ome` key of a Zarr v3 group's attributes, `ome.version`; the Zarr v3 specification's `sharding_indexed` codec (https://zarr-specs.readthedocs.io/, CC-BY-4.0): the array's `chunk_grid` gives the shard shape, the codec's `chunk_shape` the inner chunks, and a reader decodes one inner chunk at a time. zarr-python 3.4 (MIT) is the oracle, as before.

**What was compared.** Every hashed plane of both stores, the label image, the axes, units and scales agree with zarr-python, and the planes of the 0.5 copy of image 6001240 hash the same as the 0.4 copy's. No change to reading was needed.

**Rule changed.** `resolution_levels` gave no tile size for a sharded array whose shard is larger than the image (the 0.5 copy of 6001240), and the shard size for the projection, although region reads decode inner chunks. The tile size of a sharded array is now its inner chunk shape (`ArrayMeta.inner_chunks`).

## 2026-10-07 — Large planes go through the streaming writer

**Corpus files:** `openslide-zeiss-5-cropped` (a 20563 × 20164 RGB plane) and `ome-qptiff-hande-compressed-scan1`. **Prior art consulted:** none. This changes only how OpenReadout writes a store, not how it reads one.
**What was changed.** The in-memory writer took planes up to 4 GiB. It holds the decoded plane, the deinterleaved samples, the compressed chunks and the read-back copy at once, so exporting the zeiss plane with `--pyramid none` peaked at 2.9 GB of RSS, and the QPTIFF plane at 4.6 GB. Planes larger than the streaming writer's 256 MiB block now go through the streaming writer: 0.90 GB and 1.0 GB. The stores of both files are byte-identical to the ones the in-memory writer made, with `--pyramid none` and, for the zeiss file, `--pyramid mean`. The memory-ceiling test in `openreadout-bench` exports one 512 MiB synthetic plane and checks that the peak heap stays within one block plus a small allowance.
