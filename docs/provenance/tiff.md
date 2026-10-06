# Provenance log — TIFF family (plain TIFF, BigTIFF, OME-TIFF, ImageJ, Aperio SVS, Hamamatsu NDPI, Zeiss LSM, PerkinElmer QPTIFF, Micro-Manager)

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

## 2026-09-22 — initial derivation (Richard Zimring with Claude as assistant)

**Scope.** `crates/openreadout-tiff` reads the TIFF container and the metadata conventions that microscopy software layers on top of it. The container itself is an open, published standard; most of the conventions are documented publicly by their authors or by permissively licensed readers.

**Open specifications read (no restrictions):**
- TIFF Revision 6.0 (Adobe, 1992), https://www.itu.int/itudoc/itu-t/com16/tiff-fx/docs/tiff6.pdf — header, IFD layout, field types, baseline tags, strips/tiles, PackBits, LZW, predictor 2, PlanarConfiguration, JPEG (compression 7 via TIFF Technical Note 2).
- BigTIFF design, https://www.awaresystems.be/imaging/tiff/bigtiff.html (public) — magic 43, 8-byte offsets, 20-byte IFD entries, types 16–18.
- Adobe Photoshop TIFF Technical Note 3 (floating-point predictor 3), public PDF.
- OME-TIFF specification and OME-XML schema 2016-06, https://ome-model.readthedocs.io/en/stable/ome-tiff/specification.html and https://www.openmicroscopy.org/Schemas/OME/2016-06/ome.xsd (open standard) — TiffData `IFD`/`FirstC`/`FirstZ`/`FirstT`/`PlaneCount` defaults, `UUID`/`FileName` multi-file sets, `BinaryOnly`/`MetadataFile` companions, `DimensionOrder` rasterisation, `SubIFDs` pyramids, units.
- ImageJ hyperstack description keys (`images`, `channels`, `slices`, `frames`, `hyperstack`, `order`, `unit`, `spacing`, `finterval`, `mode`) are plain `key=value` lines that ImageJ (public domain software) writes into `ImageDescription`; read directly from the corpus files and cross-checked against tifffile's documentation of them (below).

**Permissively licensed prior art consulted as documentation** (no code copied; field names below are our own, see `docs/formats/tiff.md`):
- `tifffile` by Christoph Gohlke, BSD-3-Clause, https://github.com/cgohlke/tifffile (installed in `oracle/.venv`, version 2026.9.20). Read for: the CZ_LSMINFO record layout (magic numbers 0x0300494C / 0x0400494C, field order and types, offsets of the channel-colour, time-stamp and wavelength sub-records, ScanType → axis order), the LSM quirks (thumbnail IFDs interleaved with data IFDs; StripByteCounts holding uncompressed sizes in compressed files), the Hamamatsu NDPI private tag numbers 65420–65449 and the meaning of Magnification −1 (macro) / −2 (map), the Aperio description convention (`|`-separated `key = value` after a header line; `MPP`, `AppMag`), the PerkinElmer QPI marker (`Software` starts with `PerkinElmer-QPI`, per-page XML description), the Micro-Manager metadata tag 51123 (JSON), and how tifffile groups pages into series (so that our oracle mapping is well defined).
- OpenSlide format notes (the web pages are CC BY-SA 4.0 documentation): https://openslide.org/formats/aperio/ and https://openslide.org/formats/hamamatsu/ — confirms the SVS page roles (tiled baseline first, stripped thumbnail second, tiled pyramid levels, then label = NewSubfileType 1 and macro = NewSubfileType 9), `aperio.MPP`/`aperio.AppMag` as the resolution and objective power, compressions 33003/33005 being JPEG 2000, and that NDPI stores each level as one JPEG with restart markers.
- The `tiff` crate (MIT), https://github.com/image-rs/image-tiff — evaluated as a decoder (see "Decisions" below); used in our unit tests to write fixtures and to cross-check our chunk decoder.

**Corpus files used** (all in `corpus/manifest.toml`, format `tiff`; licences recorded there, each OME sample directory's `COPYING`/`readme.txt` checked to state CC-BY-4.0, each OpenSlide file's licence taken from the directory `index.yaml`, Zenodo licences from the record metadata):
- OME sample images, CC-BY-4.0: `ome-artificial-*` (bioformats-artificial: single/multi-channel, Z, T, C×Z, C×T, Z×T, C×Z×T; `.ome.btf`, `.ome.tf8`, `.ome.tf2` BigTIFF variants), `ome-tubhiswt-{2d,3d,4d}-*` (multi-file sets, unpacked from the published zips), `ome-companion-multifile-*` (BinaryOnly + `multifile.companion.ome`), `ome-binaryonly-multifile-*` (BinaryOnly pointing at an OME-TIFF), `ome-subresolutions-retina-large` (SubIFD pyramid), `mm-thomas-*` (Micro-Manager 1.4.22, Thomas Julou), `ome-qptiff-*` (PerkinElmer Vectra/inForm samples).
- OpenSlide test data, CC0-1.0: `openslide-aperio-cmu-1-small-region`, `openslide-aperio-cmu-1`, `openslide-aperio-cmu-1-jp2k-33005`, `openslide-hamamatsu-cmu-1`.
- Zenodo, CC-BY-4.0: `zenodo5781661-lsm-*` (Zeiss LSM, 16-bit, single plane and a 43-frame two-channel time series whose BitsPerSample lists three values for two samples), `zenodo14510432-lsm-10-01` (four channels as planar samples).
- Allen Institute aicsimageio test resources, BSD-3-Clause: `aics-tiff-*` (ImageJ hyperstack big-endian, ImageJ single plane, tiled deflate OME-TIFF, RGB OME-TIFF, plain LZW RGB, 10-channel tiled OME-TIFF, 6×65 OME-TIFF, Micro-Manager two-position set), plus the four `aics-*.ome.tiff` conversions already in the manifest (now TIFF inputs).

**What was inferred from what.**
- Container: header/IFD/field-type layout from the TIFF 6.0 and BigTIFF documents; confirmed on every corpus file by walking the chain (no file needed a workaround except the LSM byte counts below).
- OME-TIFF: all semantics from the OME-TIFF specification (TiffData defaults, DimensionOrder rasterisation, UUID/FileName, BinaryOnly). Differential check: `ome-companion-*` vs `ome-binaryonly-*` (the same five planes described by a companion XML vs by an OME-TIFF). Renamed single-file sets (Micro-Manager files whose OME-XML has no root UUID and names the original file) are recognised because every TiffData names one missing file — inferred from `mm-thomas-*` copied under another name.
- ImageJ: keys and page order from the corpus descriptions (`aics-tiff-s-1-t-10-c-3-z-1`: `channels=3 frames=10 hyperstack=true`, planes verified in c-fastest order against tifffile); unit scaling from `unit=micron` and XResolution.
- Zeiss LSM: record layout, sub-record layouts and ScanType→axes from tifffile's documentation (BSD-3); the byte-count repair for compressed files from tifffile's documentation (no compressed LSM in the corpus yet, so this branch is `inferred`, not corroborated); the BitsPerSample quirk from `zenodo5781661-lsm-time-1-43`.
- Aperio SVS: description convention and page roles from tifffile documentation and OpenSlide's format page; confirmed on `openslide-aperio-*` (MPP 0.499, AppMag 20, label/macro NewSubfileType 1/9).
- Hamamatsu NDPI: tag numbers 65420–65449 from tifffile's documentation; confirmed on `openslide-hamamatsu-cmu-1` (magnification 20, four pyramid pages and one macro page with magnification −1).
- PerkinElmer QPI: `Software` marker and per-page XML element names read directly from `ome-qptiff-*`; tifffile's documentation used only for the series grouping so the oracle mapping is well defined.
- JPEG: `jpeg-decoder` output (the decoder the CZI branch put behind the codecs `jpeg` feature; we added `jpeg_decode_tiff` for `JPEGTables` and the photometric colour choice) differs from libjpeg-turbo (used by tifffile via imagecodecs) by at most one count in about 0.4 % of samples on `openslide-aperio-cmu-1-small-region` (IDCT rounding); such files are marked `lossy = true` and compared by plane mean. For photometric RGB streams without an Adobe marker the decoder must be told the components are R, G, B (`ColorTransform::RGB`); its "no transform" setting returned different samples (checked on the same file). `zune-jpeg` was tried first and behaves the same (≤ 1 count), so the workspace keeps a single JPEG decoder.

**Decisions.**
- The `tiff` crate's decoder (0.11) was evaluated first. It decodes strips/tiles, BigTIFF, none/LZW/deflate/PackBits/JPEG (via `zune-jpeg`) and predictors, but for our purposes it (a) inverts WhiteIsZero samples, whereas the stored values are the ground truth we must report, (b) returns JPEG data in the stream's own colour space without YCbCr→RGB conversion, (c) only walks the main IFD chain (SubIFD pyramids are unreachable) and (d) refuses to open a file whose first IFD it cannot interpret. We therefore walk IFDs ourselves (needed anyway for `check`, private tags and SubIFDs) and decode chunks with `openreadout-codecs` (LZW, deflate, zstd, PackBits, and JPEG via the shared `jpeg` feature, `jpeg-decoder`, MIT/Apache-2.0). The `tiff` crate remains a dependency for tests: fixtures are written with its encoder and decoded by both implementations.
- Plane hashes are defined on the stored sample values (no photometric inversion, no palette expansion), little-endian, one plane per (c, z, t), RGB samples interleaved — the same definition the other readers use.

**Validation (2026-09-22).** `cargo test -p openreadout-corpus-tests --features corpus`: 48 TIFF-family corpus files pass (55 images, 1,099 plane hashes equal to tifffile's; one JPEG plane within the lossy tolerance), one documented skip (`openslide-aperio-cmu-1-jp2k-33005`, JPEG 2000). `check` exits 0 on every TIFF corpus file; truncated copies exit 4 (CLI tests). Exports of a CZI, an ND2 (RGB, two positions) and a LIF read back through this reader with identical geometry, channel names, physical sizes and plane hashes.

## 2026-09-22 — performance: OME-XML parsed once at open (Richard Zimring with Claude as assistant)

**Corpus files used:** all TIFF-family files (`info` and `info --view full` output compared byte for byte before and after on 139 files), `aics-s-3-t-1-c-3-z-5.ome.tiff` for profiling. **Prior art consulted:** none.

Performance only; no parsing logic or interpretation changed. A sampling profile of `info` on an OME-TIFF showed the OME-XML parsed three times: once to build the model, once more to convert it to JSON for the vendor tree (which `info` never shows), and a third time in `info` to read the schema name. The schema name is now recorded when the document is parsed, and the `ome` part of the vendor tree is converted when `vendor_metadata` (`info --view full`, export with `--embed-vendor`) asks for it, with the same key order. `info` on `aics-s-3-t-1-c-3-z-5.ome.tiff`: 19.4 ms to 8.9 ms per process.
## 2026-09-22 — robustness (fuzzing): bounded JPEG frames

**Scope:** a bound only. The `codec_jpeg` fuzz target found that `jpeg-decoder` sizes its per-component planes from the frame header before applying its output cap (a 342-byte stream made it allocate 4 GB), and that a tiny stream declaring about 60000 x 15000 pixels takes tens of seconds to decode. JPEG chunks are now decoded with `openreadout_codecs::jpeg_decode_tiff_limited`: a frame whose decoded size exceeds twice the chunk (full chunk height, all samples; at least 1 MiB) is refused before decoding. Decoded pixels are unchanged. **Prior art consulted:** none.

## 2026-09-23 — growing files (`write_state`)

**Scope:** a new, read-only judgement; reading is unchanged. `Dataset::write_state` reports a single-file TIFF with an intact IFD chain as unfinished when it is an `.ome.tif(f)` whose first page has no OME-XML yet, when its OME-XML declares planes the chain does not reach, or when the last page's strips run past the end. Pointers past the end (a next-IFD offset, an out-of-line description) mean a finished file cut short and give no status.

**Corpus files used:** `ome-artificial-time-series.ome.tiff`, `aics-s-3-t-1-c-3-z-5.ome.tiff` (replayed in `crates/openreadout-corpus-tests/tests/live.rs`).

**Inferred from the files:** in both, page 0's ImageDescription (the OME-XML) is stored after the last page's pixel data, and every IFD precedes its own data: the OME-XML was written last. The placeholder the writer keeps in page 0 until then is not visible in finished files; the replay uses an empty inline string (assumed).

**Prior art consulted:** none.

## 2026-09-23 — performance / robustness merge

Reconciled the existing performance branch with main; no new format layout was inferred.
Inputs: existing committed synthetic and malformed regression fixtures and the shared public
corpus (ids/licences unchanged in `corpus/manifest.toml`). Prior art: repository code and notes;
weezl 0.2.1 public buffer API (https://docs.rs/weezl/0.2.1/weezl/decode/struct.IntoStream.html,
MIT OR Apache-2.0) for a reusable bounded LZW staging buffer. Retained file-relative allocation
limits, checked geometry, bounded decoder output and capped sidecar reads alongside parallel
decoding and copy avoidance. The common plane-size guard also enforces the 4 GiB ceiling.

## 2026-09-23 — MetaMorph STK, MetaSeries TIFF and `.nd` series (Richard Zimring with Claude as assistant)

**Scope.** Three MetaMorph conventions in `openreadout-tiff`: STK files (a TIFF whose first page carries the UIC1–UIC4 private tags and whose planes follow the first plane back to back), MetaSeries TIFFs (a `<MetaData>` XML document in `ImageDescription`), and the `.nd` text file that ties many STK/TIFF files into one multi-stage, multi-wavelength time series.

**Permissively licensed prior art read as documentation:** `tifffile` 2026.9.20 (BSD-3-Clause, https://github.com/cgohlke/tifffile, installed in `oracle/.venv`): `read_uic1tag` / `read_uic2tag` / `read_uic3tag` / `read_uic4tag` / `read_uic_tag` and the `UIC_TAGS` id → type table (which UIC1 ids hold an offset to a rational, a string or a Julian date/time pair, which UIC4 ids hold per-plane arrays), `julian_datetime` (MetaMorph day numbers are the Julian day number minus one, milliseconds since midnight), `series_stk` (plane count = UIC2 count; Z when every UIC2 Z distance is non-zero, else T when the creation times differ; calibration units; planes stored contiguously after the first), and `metaseries_description_metadata` (the `<prop id type value>` elements of `<MetaData>`, `<PlaneInfo>`, `<SetInfo>`). Names in our code are ours.

**Bio-Formats as a black box:** Bio-Formats 8.5.0 `showinf -omexml` / `bfconvert` on every `.nd` data set and STK file: one series per stage position named `Stage<n> "<label>"`, wavelengths as channels named by `WaveName<n>`, `NTimePoints` time points (the Hurst data set holds a 25th file per wavelength that Bio-Formats ignores), `NZSteps` Z planes from the STK stacks, channel names, pixel size from the MetaSeries `spatial-calibration-x/y`, from the STK UIC1 calibration, or, when STK calibration is off, from the TIFF resolution tags (32.05 µm for 312 pixels/cm), Z step from `ZStepSize`, acquisition date from `StartTime1`. Stage X positions: Bio-Formats reports −22440.1 where tifffile reports 4272527.196 for the same UIC4 value `0xfea9975c / 1000`: the numerators are signed (two's complement), which our reader follows.

**Corpus data sets used** (licences per deposit): OME sample images `Metamorph/zenodo-13642395` (CC0-1.0, Zenodo 13642395: `.nd` + 78 MetaSeries TIFFs, 6 stages × 13 time points, MetaMorph 7.8.13); `Metamorph/ssbd-repos-000232` (CC-BY-4.0, SSBD:repository 232: `Dish1.nd` of `20211203_Vec_ISOto5HT_35mm`, 2 wavelengths × 31 time points, and `Dish2.nd` of `20211116_DRD2_ISOtoDA_4well`, 4 stages × 2 wavelengths × 31 time points, MetaSeries TIFFs with multi-line `Description` in the `.nd`); Figshare 7583960 "Photoconversion of Hoechst" (CC0-1.0, Verena Hurst and Susan Gasser: a `.nd` + 49 STK files written by VisiView 3.3.0, 2 wavelengths × 24 declared time points × 14 Z, one 8-byte STK and one undeclared 25th time point); Figshare 12981617 (CC-BY-4.0, Oliver Seitz: one STK, `Extended Data Figure 4a sdc405.stk`, VisiView 4.4.0, 11 Z planes, spatial calibration 0.13 µm on). Cultured cell lines and plates only; no patient material.

**Observed (hex dumps and tag listings):**
- STK: UIC1 (33628) LONG × n = n (id, value) pairs; UIC2 (33629) RATIONAL × planes but 6 u32 per plane (Z distance as a rational, creation day and milliseconds, modification day and milliseconds); UIC3 (33630) RATIONAL × planes (wavelength, 525 / 450 / 460); UIC4 (33631) LONG × planes, a sequence of u16 id + data: id 28 = per plane (x num, x den, y num, y den) with signed numerators, id 37 = per plane a u32 length + text (stage label: `Row0_Col0`), then ids 46 and 1. `ImageDescription` holds one NUL-separated text per plane (`Exposure: 40 ms\r\nBinning: 1…`). Page 0's strips cover plane 0 only; plane i starts at the first strip offset + i × plane bytes (14 × 512 × 512 × 2 + 8 = 7340040 ≤ the IFD at 7340050).
- MetaSeries: `Software` = `MetaSeries`, `ImageDescription` = `<MetaData><prop id="Description" …/>…<PlaneInfo>…<prop id="spatial-calibration-x" type="float" value="0.65"/>…<prop id="acquisition-time-local" type="time" value="20240816 09:49:42.978"/>…</PlaneInfo><SetInfo><prop id="number-of-planes" …/></SetInfo></MetaData>`.
- `.nd`: `"Key", value` lines (`"NDInfoFile", Version 2.0`, `"DoTimelapse", TRUE`, `"NTimePoints", 13`, `"DoStage", TRUE`, `"NStagePositions", 6`, `"Stage1", "B2"`, `"DoWave"`, `"NWavelengths"`, `"WaveName1", "FRET"`, `"WaveDoZ1"`, `"DoZSeries"`, `"NZSteps"`, `"ZStepSize", 0.50` or `0,50`, `"WaveInFileName"`, `"StartTime1"`, `"EndFile"`); a `"Description"` value continues on the following lines until the next quoted key. File names: `<nd stem>` + `_w<i><WaveName>` (wavelength, name only with `WaveInFileName` TRUE) + `_s<j>` (stage) + `_t<k>` (time point), each part present only when its `Do…` key is TRUE, extension `.TIF` or `.stk` (any case): `Dish2_w1FRET_s1_t1.TIF`, `test_timelapse_20240816_s1_t1.TIF`, `170227-hoechst only7_w2Laser4054BD4BP_t3.stk`.

**Inferred (flagged in `docs/formats/tiff.md`):** that a wavelength with `WaveDoZ` FALSE in a Z series has one plane repeated at every Z (no corpus file); that stage positions are in µm; that `.nd` and STK times are local time (no zone is recorded).

## 2026-09-23 — Nikon NIS-Elements TIFF exports and their file sequences (Richard Zimring with Claude as assistant)

**Scope.** TIFF files written by NIS-Elements' "export ND document to TIFF" (one file per position, channel, Z and time point) as a sub-format of the TIFF reader: a few private double-valued tags, and the grouping of the files of one export into one data set by the index tokens at the end of their names.

**Bio-Formats as a black box:** Bio-Formats 8.5.0 `showinf -omexml` on `single_plane_1-2000_2xy01c1.tif` with its siblings next to it: format "Nikon Elements TIFF", one series of one plane, no physical size, the siblings not grouped. Its planes are compared with ours file by file.

**Corpus files used** (licence per deposit): Zenodo 7677827 "Images of H3K9me3 Immunostain in IMR90 Cells" (CC-BY-4.0, Sarah Mangiameli; 98 files `single_plane_1-2000_2xy<NN>c<N>.tif`, 2048 × 2048 uint16; the six files of positions 1–3 are in the corpus). The record's description is the key to the names: "two-channel images at 49 unique xy positions … DRAQ5 DNA stain (c1) and H3K9me3 immunostain … (c2)", imaged on a Nikon Ti2-E with a 40× objective and an Andor Zyla sCMOS camera, "controlled by NIS-Elements AR". IMR90 is a cell line; no patient data. The sibling records Zenodo 7677835 and 7677868 (same authors, `h4k8acxy22c1.tif`, `h4k20me002xy13c2.tif`) were looked at for the naming only.

**Observed (tag listings, six files):**
- Tags 65325–65333 on page 0, no `Software`/`ImageDescription`: 65325 DOUBLE (7293.57, 15060.66, 21955.12 ms for positions 1, 2, 3 — the same for c1 and c2 of a position), 65326 DOUBLE 0.1625, 65327 and 65328 DOUBLE 1.0, 65329 DOUBLE × 4 (−2546.81, 5717.09, 2132.28, 7293.57 for position 1; X grows by 329.47 per position while Y and Z stay), 65330 and 65332 BYTE blocks: a directory (u32 count, then per entry a u32 name length, a UTF-16 name, a u32 size and a u32 offset) of named blocks (`CameraTemp1`, `Camera_ExposureTime1`, `PFS_OFFSET`, `PFS_STATUS`, `ROGlobalTiffV1_0`, `TextInfoTiffV1_0`, `X`, `Y`, `Z`, `Z1`; `CustomDataV2_0` XML), 65331 a block named `MetadataTiffV1_0` (its entry has 8 more bytes), with the same LV-encoded structures the ND2 reader decodes (`SLxPictureMetadata`, `SLxImageTextInfo`).
- Inferred: 65326 is the pixel size in µm — 0.1625 µm is 6.5 µm sCMOS pixels behind the 40× objective the record names, and the stage steps of 329.47 µm between neighbouring positions are just under one field of view (2048 × 0.1625 = 332.8 µm), as in a tile scan with a small overlap; 65329 is the stage X, Y, Z in µm and the time; 65325 the time in ms since the acquisition started. 65327/65328 are kept as found.
- File names: `<prefix>` + index tokens `xy<n>` (position; the description's "xy positions"), `c<n>` (channel; "c1", "c2" in the description), and, not seen in the corpus, `t<n>` and `z<n>` for time points and Z planes (inferred from the same pattern); tokens are read from the end of the name. The files of one export share the prefix and the token letters.

**Not decoded (listed as a gap):** the LV blocks of 65330/65331 (objective, text information, per-plane names) and the XML of 65332; the reader keeps their names only.

**Validation (2026-09-23, MetaMorph and NIS-Elements).** Corpus harness (`oracle/gen.py`: `metamorph_nd` — Bio-Formats planes, every one also found among tifffile's planes of the member files; `tiff` — tifffile's `stk` and `uniform` series; `nis_export` — tifffile per file, Bio-Formats per file): 4 `.nd` series (452 planes hashed, up to 64 per image), 2 STK files (25 planes), 1 MetaSeries TIFF (1 plane), 1 NIS-Elements export (6 files, 6 planes) — all bit-exact; image names (stage labels, 10 images) and channel names (wavelength names, 6 images) equal Bio-Formats' (22 values); pixel sizes equal Bio-Formats' (0.65, 0.740001, 32.05 µm) and tifffile's (0.13 µm, Z 0.5 µm); the 8-byte STK of Figshare 7583960 is rejected on open (exit 4). Every TIFF-family corpus file read before this change still matches. Synthetic tests: `crates/openreadout-tiff/tests/metamorph.rs` (a hand-built STK with all four UIC tags, a cut STK, a `.nd` series with a missing and a cut member, a file past `NTimePoints`) and `tests/nis.rs` (grouping by name tokens, a missing combination, a token-less export file).

## 2026-09-23 — Pyramid levels and region reads (Richard Zimring with Claude as assistant)

**Scope.** Reading reduced-resolution levels of every TIFF flavour that lists them (OME-TIFF `SubIFDs`, SVS, NDPI, QPTIFF, plain TIFFs with reduced-resolution pages) and reading a rectangle of any page by decoding only the tiles or strips it overlaps (`Dataset::read_region`). Until now levels were listed in `info` but not readable, and pages above 4 GiB were not readable at all.

**Prior art consulted:** the OME-TIFF specification, "Sub-resolutions" (https://ome-model.readthedocs.io/en/stable/ome-tiff/specification.html, CC-BY-4.0): every full-resolution plane IFD lists its own reduced levels in `SubIFDs` (tag 330), in decreasing size. tifffile (BSD-3-Clause) documentation of `TiffPageSeries.levels` (how SVS, NDPI and QPTIFF pyramids are grouped); tifffile is also the pixel oracle. TIFF 6.0 (Adobe, public) for tile and strip addressing: chunk `i` of a page is row `i / across`, column `i % across`, planar-configuration 2 pages repeat the chunk grid per sample.

**Inferred from the corpus files** (`ome-qptiff-HandEcompressed_Scan1.qptiff`, `openslide-aperio-CMU-1.svs`, `openslide-hamamatsu-CMU-1.ndpi`, `aics-s_1_t_1_c_4_z_3_pyramid.ome.tiff`, `ome-subresolutions-retina_large.ome.tiff`): in whole-slide flavours the pages of one level follow the order of the full-resolution pages (QPTIFF: one reduced page per channel, level by level; NDPI: one pyramid per focal plane). A level's page for a plane is therefore the k-th main-chain page with the level's size, counted from the level's first page, where k is the plane's position among the image's distinct full-resolution pages. OME-TIFF: the level of a plane is the `SubIFDs` entry of that plane's own page (the first plane's entries only give the sizes).

**NDPI full resolution.** Level 0 of an NDPI slide is one baseline JPEG (a single strip, 51 200 × 38 144 in `openslide-hamamatsu-CMU-1.ndpi`) with a restart interval. tifffile's documentation and source (BSD-3; read as documentation) describe NDPI private tag 65426 as the byte offsets of the restart intervals relative to the strip (and 65432 as their high 32 bits for files above 4 GiB), and read the page as a grid of intervals by giving each interval a copy of the JPEG header with the frame size set to one interval. Observed in the corpus file: 119 200 offsets for 25 × 4 768 intervals of 2 048 × 8 pixels (restart interval 256 MCUs of 8 × 8, YCbCr 1 × 1 sampling); the first offset (660) is the end of the start-of-scan segment; each interval's bytes end with its RST marker. We rebuild a small JPEG per interval (header with SOF height = MCU height, width = restart interval × MCU width, DRI removed; the interval's bytes without the trailing RST; EOI) and decode it with the existing JPEG decoder. Progressive frames are refused (exit 6). Validated against tifffile's own reading of the same windows (`corpus/oracle/regions/openslide-hamamatsu-cmu-1.json`).

**Update 2026-09-24 (merge with the JPEG 2000 / WebP / JPEG XL / LERC / old-style JPEG decoders).** Region reads decode their chunks through the same per-chunk decoder as whole pages, so every codec above works for `--region` and `--level`. Region ground truth added, from tifffile + imagecodecs (OpenJPEG, libwebp, libjxl; BSD-3-Clause, run as libraries): `openslide-aperio-jp2k-33003-1` (every level), `openslide-aperio-cmu-1-jp2k-33005` (levels 0, 1 and the last: windows of the 4.5 GB level 0, which is too large to read as a plane), `gdal-webp-rgbsmall-tiled`, `gdal-jxl-rgbsmall-tiled-separate` (planar samples, which OpenReadout exposes as channels, compared channel by channel), and one Harmony screening field (`hcs-harmony-idr0034-index`, image 0 = well A01 field 1, from the field's per-channel TIFFs: the generic crop path). All match exactly or within the lossy tolerance.

## 2026-09-24 — JPEG 2000 tiles (Aperio SVS 33003 / 33005, TIFF 34712)

**Scope.** Decoding of JPEG 2000 compressed chunks; parallel decoding of the chunks of one page. Container parsing is unchanged.

**Corpus files used:** `openslide-aperio-cmu-1-jp2k-33005` (CC0-1.0) and the new `openslide-aperio-jp2k-33003-1` (OpenSlide test data, "Free to use and distribute, with or without modification", `corpus/manifest.toml`). Marker dumps of their tiles (our own script): 33005 tiles are 240 × 240 raw codestreams with 3 components, COD MCT = 1, 9/7 wavelet, 5 levels, one layer, COM `Kakadu-v6.4.1`; 33003 tiles are 256 × 256 with components 1 and 2 at XRsiz = 2, MCT = 0, 9/7, COM `MIL`. Photometric is 2 (RGB) on both.

**Prior art consulted:** tifffile (BSD-3-Clause, https://github.com/cgohlke/tifffile) as documentation: compression names `APERIO_JP2000_YCBC` = 33003 ("Matrox libraries"), `APERIO_JP2000_RGB` = 33005 ("Kakadu libraries"), and that tifffile hands each chunk to imagecodecs' `jpeg2k_decode`. OpenJPEG (BSD-2-Clause, https://github.com/uclouvain/openjpeg) `src/bin/common/color.c` behaviour, as documented and as observed through imagecodecs: 3-component codestreams whose chroma is sub-sampled are treated as sYCC and converted with `sycc_to_rgb` (chroma replicated; the coefficients and truncation are in `docs/formats/tiff.md`). OpenSlide (LGPL-2.1) was run as a black box only (openslide-python 1.4.6, openslide-bin 4.0.1).

**Inferred / validated.** 33003's Y, Cb, Cr are converted to R, G, B (the Aperio convention the compression code names); with OpenJPEG's conversion our pixels match tifffile + imagecodecs within 2 grey levels, 0.06 % of samples differing; OpenSlide agrees with both within 1 at full resolution. Decoder choice (`docs/formats/tiff.md`, codecs `j2k.rs`): `hayro-jpeg2000` 0.4 and its fork `dicom-toolkit-jpeg2000` 0.5 (the decoder this project used for ND2/VSI) reconstruct quantized coefficients at the bin edge and differ from OpenJPEG by up to 43 grey levels (mean 0.97) on the coarsely quantized pyramid levels of the 33005 file; `rust-j2k` 0.3 (MIT OR Apache-2.0, no dependencies, no `unsafe`) is within 1 (mean 0.0003).

## 2026-09-24 — WebP, JPEG XL, LERC and old-style JPEG chunks

**Scope.** Decoding of chunks with compression 50001 (WebP), 50002/52546 (JPEG XL), 34887 (LERC) and 6 (old-style JPEG). Container parsing is unchanged; the tags LercParameters (50674), JPEGProc (512), JPEGInterchangeFormat (513), JPEGRestartInterval (515), JPEGQTables/JPEGDCTables/JPEGACTables (519–521), YCbCrCoefficients (529), YCbCrSubSampling (530) and ReferenceBlackWhite (532) are now read for decoding.

**Files used:** GDAL autotest data at commit 7617801 (https://github.com/OSGeo/gdal/tree/7617801fa6b2c9be05b7775b536647925c1bf66f/autotest/gcore/data; GDAL LICENSE.TXT: MIT): 22 GDAL-written samples committed as fixtures (`crates/openreadout-tiff/tests/fixtures/codecs/`), 3 of them also corpus entries (`gdal-webp-rgbsmall-tiled`, `gdal-jxl-rgbsmall-tiled-separate`, `gdal-lerc-float32-mask`), and `zackthecat.tif` (corpus `gdal-ojpeg-zackthecat`, not committed: its origin before GDAL is not recorded).

**Specifications and prior art consulted:** TIFF 6.0 (Adobe, public) sections 21 (YCbCr, ReferenceBlackWhite) and 22 (JPEG, the old-style tags); ITU-T T.81 (JPEG) for the DQT/DHT/DRI/SOF/SOS segment layouts; the TIFF compression code registry as tifffile (BSD-3-Clause) names it (`LERC` 34887, `WEBP` 50001, `JPEGXL` 50002, `JPEGXL_DNG` 52546) and tifffile's reading of `LercParameters` (second value: 0 none, 1 deflate, 2 zstd). The decoders are third-party crates, used through their public APIs: `image-webp` 0.2 (MIT OR Apache-2.0), `jxl-oxide` 0.12 (MIT OR Apache-2.0), `lerc-rs` 0.5 (Apache-2.0). libtiff 4.7 (through Pillow 12.3) was run as a black box.

**Inferred / validated.** Every GDAL-written sample (WebP lossy and lossless, JPEG XL, LERC in 8 sample types, with and without masks, deflate- and zstd-wrapped, strips, tiles, planar) decodes to exactly tifffile + imagecodecs' samples. Masked LERC pixels are 0 in tifffile's output (checked on both float samples: 128 masked pixels, all 0), so we write 0. For old-style JPEG, that ReferenceBlackWhite applies was inferred from libtiff's output: JFIF conversion is 9.5 grey levels off on average, the section 21 conversion 1.02.
## 2026-09-24 — JPEG colour: photometric RGB pages holding YCbCr streams (Richard Zimring with Claude as assistant)

**Trigger.** Finding H2 of `docs/benchmark/heldout-2026-09-24.md`: an OME-TIFF written by Bio-Formats 6.7 (JPEG, photometric RGB, 256 × 256 tiles) decodes to wrong colours with no warning. Per the held-out rules the file itself was not opened; the problem was reproduced on new fixtures.

**Corpus files used.** The eight `synthetic-tiff-jpeg-*` fixtures (written by `oracle/make_tiff_jpeg_fixtures.py` from a 1300 × 1000 crop of `openslide-aperio-cmu-1-small-region`, CC0-1.0), and for regression the existing JPEG TIFFs `openslide-aperio-cmu-1-small-region`, `openslide-aperio-cmu-1`, `openslide-hamamatsu-cmu-1` and `ome-qptiff-hande-compressed-scan1`.

**Tools run as black boxes.** Bio-Formats `bfconvert` 6.7.0 (https://downloads.openmicroscopy.org/bio-formats/6.7.0/artifacts/bftools.zip) and 8.5.0 (GPL; run) with `-compression JPEG [-tilex 256 -tiley 256 -pyramid-resolutions 3 -pyramid-scale 2]`, from a PNG (interleaved input) and from an uncompressed TIFF (Bio-Formats then writes one JPEG stream per sample plane).

**Prior art read (documentation, BSD-3):** tifffile 2026.9.20 `jpeg_decode_colorspace()` and the JPEG branch of `TiffWriter.write` (https://github.com/cgohlke/tifffile, BSD-3-Clause): for photometric RGB, libjpeg is told the input colour space is RGB only when the stream has no JFIF marker ("found in Aperio SVS"); with JFIF it keeps libjpeg's default (Y, Cb, Cr); photometric YCbCr keeps the default; separate planes decode to grey. `jpeg-decoder` 0.3.2 (MIT/Apache-2.0, our dependency) `determine_color_transform`, for the order of its own marker rules. The rule order for streams that must decide for themselves (JFIF → YCbCr; else Adobe transform 0 → RGB, 1 → YCbCr; else component ids 1,2,3 → YCbCr, `R`,`G`,`B` → RGB, others → YCbCr) is IJG libjpeg 6b's, which libjpeg-turbo (the library under tifffile/imagecodecs) keeps; read from the public article linked in `jpeg-decoder`'s source, https://entropymine.wordpress.com/2018/10/22/how-is-a-jpeg-images-color-type-determined/ (libjpeg 9a+ let component ids override JFIF; `jpeg-decoder` follows 9a, which is why we decide ourselves instead of using its default).

**Observed (marker listings with our scanner, a throw-away Python marker lister):**
- Bio-Formats 6.7.0, interleaved input: PhotometricInterpretation 2, PlanarConfiguration 1, every tile an `APP0 JFIF` stream with components ids 1, 2, 3 and sampling 2×2, 1×1, 1×1, i.e. Y, Cb, Cr with 4:2:0 subsampling. Bio-Formats 8.5.0 writes the identical streams (same strip sizes byte for byte) and tags the page PhotometricInterpretation 6.
- Bio-Formats 6.7.0 from a planar source: PhotometricInterpretation 2, PlanarConfiguration 2, one single-component JFIF stream per sample plane (grey: no colour transform needed). 8.5.0: PhotometricInterpretation 6 with PlanarConfiguration 2 — separate Y, Cb, Cr planes, which tifffile refuses ("chroma subsampling not supported").
- Aperio SVS (corpus): photometric 2, no JFIF, no Adobe segment, component ids 0, 1, 2, no subsampling: R, G, B as coded. QPTIFF: photometric 2, ids `R`,`G`,`B`. NDPI: photometric 6. tifffile's RGB writer with `outcolorspace="RGB"`: photometric 2, `Adobe` transform 0, ids `R`,`G`,`B`.
- Before the fix, `openreadout stats` on the Bio-Formats 6.7.0 fixture: mean 160.8 and median 129 over all samples, against tifffile's 221.2 (226.5 / 215.0 / 222.1 per sample): the Y, Cb, Cr values themselves (Cb and Cr sit near 128). The same pattern as the held-out report's numbers.

**Inferred / decided.** The colour transform is chosen per chunk from the page's photometric tag and the stream's markers: photometric YCbCr → the stream decides (libjpeg order); photometric RGB → as coded unless the stream has a JFIF segment, which declares Y, Cb, Cr (tifffile's rule). Refused with exit 6 instead of returning wrong samples: JPEG with photometric YCbCr stored as separate planes, three-component streams in pages whose photometric is not RGB or YCbCr, JPEG in palette/CMYK/Lab pages, streams with 2 or 4+ components. `check` reads the markers of each JPEG page's first tile and warns (`unsupported_samples`) on the refused combinations, and (`jpeg_colour_ambiguous`) on photometric RGB without JFIF but with an `Adobe` segment declaring YCbCr, which is decoded as RGB like tifffile does.

## 2026-09-24 — single-plane MetaMorph files with UIC1 only; LZW strips without EOI (Richard Zimring with Claude as assistant)

**Corpus files:** `hcs-imagexpress-idr0081-*` (CC-BY-4.0; MetaMorph 6.2.3 plane files of an ImageXpress plate) and `hcs-harmony-idr0034-r03c07f01p01-ch4sk1fk1fl1-tiff` (CC-BY-4.0; a Harmony plane file). **Prior art:** tifffile (BSD-3) as documentation and as the reader whose output these files are compared with; nothing else.

**Observed:** the ImageXpress plane files carry the UIC1 tag (33628, with calibration, name and wavelength entries) but no UIC2/UIC3/UIC4 tags; tifffile reads their `stk_metadata` from UIC1 alone (`SpatialCalibration` 1, `XCalibration` 1.72, `CalibrationUnits` `um`). One Harmony plane file is a single LZW strip that ends without the end-of-information code; tifffile (imagecodecs) decodes it to the full 1360 x 1024 plane.

**Changed:** `parse_stk` reads UIC1 when UIC2 is absent, as a single plane (`plane_count` 1); the TIFF reader still calls it only for files with both tags (`is_stk`), so its STK detection is unchanged; the HCS reader calls it for plane files with UIC1. The LZW decoder keeps the decoded bytes of a strip without EOI (libtiff warns "Strip not terminated with EOI code" and does the same); a strip that decodes short is still refused by the chunk length check.

**Update (same day): LERC off by default.** A 15-minute `codec_lerc` fuzz run found panics inside `lerc-rs` 0.5 past the header (validity-mask index out of bounds, a multiplication overflow); LERC decoding moved behind the `lerc` feature and the corpus entry `gdal-lerc-float32-mask` was dropped. The committed GDAL LERC fixtures still decode exactly with the feature on and are exit 6 without it.

## 2026-09-24 — NDPI restart intervals framed before their JPEG markers are read

**Problem.** Since the JPEG colour-transform change (b76400b), every chunk's marker segments are scanned (`codecs::jpeg_markers`) before decoding. The restart-interval chunks of an NDPI single-strip page carry no header (the reader frames each with the page's JPEG header, `ndpi::frame`), so the scan failed with "stream lacks an SOI marker" and every region or level read of `openslide-hamamatsu-CMU-1.ndpi` failed (exit 4). Found by the new corpus-wide pyramid invariant test (`crates/openreadout-corpus-tests/tests/pyramids.rs`) and the region oracles.

**Fix.** The framed stream (header + interval + EOI) is built first and both the marker scan and the decoder read it. No new interpretation of the format.

**Corpus files used:** `openslide-hamamatsu-CMU-1.ndpi` (region oracles from tifffile + zarr in `corpus/oracle/regions/`). **Prior art consulted:** none.

## 2026-09-24 — JPEG 2000 size check overflow

**Scope:** a bound only. The `codec_jpeg2000` fuzz target (90 s, first run since the switch to `rust-j2k`) found SIZ markers whose width × height × components × 2 overflowed `u64` in the pre-decode size check of `openreadout_codecs::jpeg2000_decode_limited` (a panic with overflow checks; a wrapped, too small size in release builds, which the decoder then met with its own limits). The product now saturates, so such a codestream is refused as implausible before decoding. Regression fixture: `crates/openreadout-codecs/tests/fixtures/malformed/jpeg2000-fuzz-siz-size-overflow.bin`.

## 2026-09-25 — OME-XML filters, contrast method and our normalized annotations read into the model

**Scope:** more of the OME-XML (open standard, OME 2016-06 schema, `oracle/schema/ome.xsd`) is mapped to the normalized model; plane addressing and decoding are unchanged. Found by exporting corpus files to OME-TIFF and running `check --against` on source and export: `zenodo14976703-Convalaria-LambdaScan` gave 149 metadata differences with identical pixels, and a sweep over 79 CZI/ND2/LIF corpus files (smoke and standard tiers, none held out) 917.

**What is read now.** `Instrument/Filter/TransmittanceRange@CutIn/@CutOut` (+ `*Unit`, default nm) and `Instrument/FilterSet/EmissionFilterRef`; `Channel/LightPath/EmissionFilterRef` (else the channel's `FilterSetRef`) gives the channel's detection band, the pass band shared by its emission filters. `Channel@ContrastMethod` joins `@AcquisitionMode` in one readable label (`openreadout_core::acquisition_mode::from_ome`; the enumerations are the schema's). The first `Detector` stays the instrument's detector; our exporter now writes `instrument.detector` first. `MapAnnotation`s of namespace `openreadout.dev/normalized` (our exporter's own, documented in `book/src/guides/metadata.md` § OME-XML export) linked from the `Image` or a `Channel` restore software, the exact acquisition time, a non-OME immersion and a non-OME mode label. The OME-Zarr reader shares these helpers for bioformats2raw `OME/METADATA.ome.xml`.

**Corpus files used:** `zenodo14976703-Convalaria-LambdaScan`, `bsst749-*` (six LAS AF LIFs), `aics-s-3-t-1-c-3-z-5`, `ome-aryeh-b16-14-12`, `aics-ND2-dims-*`, `zenodo7015307-*`, the synthetic CZIs, as sources of exports. **Prior art consulted:** the OME 2016-06 XSD only.

## 2026-09-25 — assurance profile (`src/assurance.rs`)

**Corpus files:** every development input of this format on disk (`corpus/assurance/evidence.json` lists them); no held-out file.
**Prior art consulted:** none.
**What was done.** No parsing logic changed. The reader declares an assurance profile (docs/assurance.md) that fingerprints each file from this reader's own normalized output: the container version and OME-XML schema of `format_version`, the sub-format note, the writer (`ome_creator`, `imagej_version`, `Software`), and per image the first page's codec (with the photometric interpretation folded in for JPEG-family codecs), planar configuration, predictor, bit depth and tiling, read from the page layouts through `Dataset::assurance_observations`. The table of validated feature values and the confidence level are generated from the development corpus by `cargo xtask assurance-audit`.

## 2026-09-26 — differential cross-check with Bio-Formats

**Corpus files:** every development file of this format on disk up to 400 MB (`corpus/oracle/second/*.json`); no held-out file.
**Prior art consulted:** Bio-Formats 8.5.0 (GPL) `showinf`/`bfconvert`, run as black boxes by `oracle/second_opinion.py`.
**Observations and adjudications:** `corpus/oracle/second/adjudications.toml` (differences are conventions or decoder rounding of the second reader; files whose series Bio-Formats groups differently are not compared).
**Inferred.** Nothing new; no reader change.

## 2026-09-26 — packed-bit, 64-bit integer, half/24-bit float and complex samples; short edge tiles

**Corpus files used:** GDAL's test data (`github.com/OSGeo/gdal` commit `7617801`, `autotest/gcore/data/`, MIT licence per GDAL's LICENSE.TXT; only the TIFF files were downloaded): `1bit_2bands.tif`, `oddsize1bit.tif`, `oddsize_1bit2b.tif`, `empty1bit.tif`, `test_with_mask_1bit.tif`, `test3_with_mask_1bit.tif`, `int10.tif`, `int12.tif`, `int24.tif`, `float16.tif`, `float24.tif`, `int64.tif`, `uint64.tif`, `cfloat32.tif`, `cfloat64.tif`, `cint16.tif`, `cint32.tif`, `complex_float32.tif`, `complex_int32.tif`, `cint_sar.tif`, `contig_tiled.tif` and the other files listed in the manifest (codec and layout variants, malformed files).

**Documentation consulted:** the TIFF 6.0 specification (open): BitsPerSample, SampleFormat (1 unsigned, 2 signed, 3 IEEE float, 4 undefined), FillOrder, sample packing "bit-packed ... most significant bit first ... rows begin on byte boundaries"; the SampleFormat values 5 (complex integer) and 6 (complex float) as tifffile (BSD-3) names them in its SAMPLEFORMAT enumeration and returns them; the layouts were confirmed on the files (float24, complex parts in the file's byte order).

**Oracle:** tifffile 2026.9.20 with imagecodecs (BSD-3). It returns 1-bit samples as bool (our uint8 0/1: the same bytes), 10/12-bit samples as uint16, float16 as float16 (the oracle widens it to float32, like our output), float24 as float32, complex integers as complex64/complex128 (widened, like ours). It does not decode 24-bit integers; there the check is GDAL's own test design: `int24.tif` and `int12.tif` hold the same 20 × 20 image (107, 123, 132, …), and our decode of `int24.tif` equals tifffile's decode of `int12.tif`.

**Inferred, with the evidence:**
- Packed samples are a most-significant-bit-first bit stream with rows starting on a byte, also for 24 bits in a little-endian file (`int24.tif` bytes `00 00 6b 00 00 7b` → 107, 123; read as little-endian 3-byte integers they would be 7012352, 8060928).
- 24-bit floats are stored in the file's byte order (`float24.tif`, little-endian: `00 ac 45` = 0x45AC00 = 107.0 with a 7-bit exponent biased by 63).
- Tiles of the last tile row may hold only the rows inside the image: `contig_tiled.tif` (35 × 37 RGB, one 64 × 64 PackBits tile) decodes to 37 × 64 × 3 = 7104 bytes; tifffile pads the tile and we now do too (previously exit 4).

All 20 sample-format files above equal tifffile's page 0 (int24: GDAL's int12 image).

## 2026-09-26 — Thermo Fisher EER movies and gain references

**Corpus files:** `empiar13509-falcon4i-eer` and `empiar13509-falcon4i-gain` (EMPIAR-13509, 2025), `empiar12080-falcon4-eer` (EMPIAR-12080, 2022 writer without frame metadata), `empiar11906-falcon4i-eer` (EMPIAR-11906, 2022). EMPIAR data are released without restriction (CC0). No held-out record.
**Prior art consulted (as documentation):** imagecodecs `imagecodecs/imcd.c`, `imcd_eer_decode` (github.com/cgohlke/imagecodecs, BSD-3-Clause): the bit order, the run/continuation/event rule, the end conditions and the compression-to-bit-size table; tifffile 2026.9.20 (BSD-3-Clause) `tifffile.py`: tags 65001/65002 as XML metadata blocks, 65007–65009 as the bit sizes of compression 65002, `eer_xml_metadata`.
**Inferred from the files:** pages are frames (T); a pixel holds at most one event per frame (from the coding rule: the run after an event starts on the next pixel); the per-frame `dose` item equals events / pixels to 6 decimals — measured on all 648 frames of `empiar13509-falcon4i-eer` before `check` was written to rely on it; `sensorPixelSize` is the pixel size at the specimen (7.06e-11 m against EMPIAR's calibrated 0.686 Å), not the physical sensor pitch; the gain reference's XML declaration (`encoding="utf - 8"`) is not well-formed, so the block is read from `<metadata` on.
**Oracle:** tifffile + imagecodecs (`eer_decode` at superres 0; bool frames compared as uint8 0/1) for the first 64 frames of each movie; the dose relation for every frame (vendor-written, independent of any decoder).

## 2026-09-26 — Leica SCN whole-slide files

**Corpus files:** `openslide-leica-fluorescence-1` (`Leica-Fluorescence-1.scn`) and `openslide-leica-1` (`Leica-1.scn`) from the OpenSlide test data (index.yaml: "distributable", "Free to use and distribute, with or without modification"). No held-out record.
**Before:** both files were read as plain TIFF: pages of equal size became Z planes of one image (the brightfield and fluorescence overviews as two Z planes, the three fluorescence channels as three Z planes), and every pyramid level was a separate image.
**Prior art consulted (as documentation):** tifffile 2026.9.20 (BSD-3-Clause): its `scn` series (one series per `image`, levels from `dimension@r`, channels from `@c`, focal planes from `@z`, pages from `@ifd`); OpenSlide's public "Leica format" web page (openslide.org/formats/leica/, a documentation page of an LGPL project): the namespace, `view` in nanometres, macro images covering the collection, base64 barcodes.
**Inferred from the files:** pixel size = `view` size / pixel count (equal to the resolution tags: 0.5 µm and 16.44 µm); `objective` is a magnification (20 for the region, 0.60833 for the overview camera); the channel `exposureTime` unit is unknown (105000, 146000) and is not converted; excitation and emission from the filter names (centre/width) are inferred and tagged so.
**Oracle:** tifffile `scn` series (every image; planes and 59 region/level windows via `aszarr`, JPEG within the lossy tolerance).

## 2026-09-26 — Ventana BIF whole-slide files

**Corpus file:** `openslide-ventana-1` (`Ventana-1.bif`, OpenSlide test data, CC0-1.0, PathAI; DP 200). No held-out record.
**Before:** read as plain TIFF: the label and probability images first, then every pyramid level as its own image, each named by its `level=N mag=M quality=Q` description.
**Prior art consulted (as documentation):** tifffile 2026.9.20 (BSD-3-Clause): `is_bif` (XMP tag, `ScanOutputManager`/`Ventana` software, `Label Image`/`Label_Image`/`Probability_Image` descriptions), its `bif` series (the `level=` pages as one pyramid, `ScanRes` as µm per pixel, `Magnification`, `Z-spacing`/`Z-layers` and the ImageDepth extension for volumes) and its statement that tiles "may overlap and require stitching based on the TileJointInfo elements" and are not stitched; OpenSlide's public Ventana format page (a documentation page of an LGPL project): detection by an `iScan` element as root or child of `Metadata`, properties from `iScan` attributes, `ScanRes` → microns per pixel.
**Inferred from the file:** the `EncodeInfo` element on the level-0 page's XMP (`SlideStitchInfo/ImageInfo` rows, columns and tile size, `TileJointInfo` with `OverlapX`/`OverlapY`/`Direction`), counted but not applied: no description of how joints position tiles was available, so overlapping joints are reported and the pixels marked partially decoded.
**Oracle:** tifffile's `bif` series (level 0 is 1.5 GB: 14 region/level windows via `aszarr`, JPEG within the lossy tolerance).

## 2026-09-26 — Philips TIFF (Richard Zimring with Claude as assistant)

**Corpus files:** `openslide-philips-1` (`Philips-1.tiff`, CAMELYON16 lymph node section, CC0-1.0 per `Philips-TIFF/index.yaml`), `openslide-philips-4` (`Philips-4.tiff`, CAMELYON17, sparse, CC0-1.0).
**Before:** read as `plain`: one image, the pyramid pages grouped away, no pixel size.
**Prior art consulted (documentation only):** OpenSlide's public format page https://openslide.org/formats/philips/ (CC-BY-SA-4.0 text): detection (Software starts with `Philips`, description XML root `DataObject` of `ObjectType` `DPUfsImport`), level sizes include tile padding, correct downsamples from the levels' pixel spacings, sparse tiles (offset and byte count 0) outside regions of interest rendered white once downsampled, label and macro as `Label`/`Macro` pages or base64 JPEGs in the XML, mpp = 1000 × `DICOM_PIXEL_SPACING` (column spacing for x). tifffile 2026.9.20 (BSD-3-Clause) run as a tool: `philips` flag and series (8 levels on `Philips-1`).
**Inferred from the files:** the XML tree (`DataObject` → `Attribute` with `Name`, `Group`, `Element`, `PMSVR` type; `IDataObjectArray` attributes hold `Array/DataObject` children: `DPScannedImage` per image type `WSI`/`LABELIMAGE`/`MACROIMAGE`, `PixelDataRepresentation` per level with `PIIM_PIXEL_DATA_REPRESENTATION_NUMBER`, `…_COLUMNS`/`…_ROWS` and its own `DICOM_PIXEL_SPACING`); quoted list values (`"0.000226891" "0.000226907"`, row then column, millimetres); `DICOM_ACQUISITION_DATETIME` as `YYYYMMDDhhmmss.ffffff`; `PIM_DP_UFS_BARCODE` base64. On `Philips-1` the WSI spacing (0.226907 µm) differs from representation 0's (0.227273, a rounded nominal value): the WSI value is the pixel size, as OpenSlide reports it.

## 2026-09-26 — Molecular Dynamics GEL files (Typhoon, Storm, FLA scanners)

**Corpus files:** Zenodo 17516010 (Gerson et al., CC-BY-4.0; Amersham Typhoon, phosphor),
15688057 (Marcand, CC-BY-4.0; Amersham Typhoon, fluorescence), 5786227 (Saridakis et al.,
CC-BY-4.0; a gel with an `MDLabName`), and two `.gel` files re-saved without the MD tags (Zenodo
15102749, 5773282) that must stay plain TIFF. Held out: Zenodo 4630771 (Mogi et al., CC0), not
opened.
**Before:** a `.gel` file was read as a plain TIFF: the stored 16-bit values, which the scanner
wrote as square roots of the counts, were returned as intensities with no note.
**Prior art consulted (as documentation):** tifffile 2026.9.20 (BSD-3-Clause): the MD tag names
(33445 `MDFileTag`, 33446 `MDScalePixel`, 33447 `MDColorTable`, 33448 `MDLabName`, 33449
`MDSampleInfo`, 33450 `MDPrepDate`, 33451 `MDPrepTime`, 33452 `MDFileUnits`) and its `mdgel`
series: `MDFileTag` 2 means square-root data, returned as float32 value² × `MDScalePixel`; 128 means
linear data, value × `MDScalePixel`.
**Inferred from the files:** every MD-tagged corpus file has `MDFileTag` 2, `MDScalePixel` 1/42948
and `MDFileUnits` `Counts`; `MDSampleInfo` is `key=value` lines (serial number, date and time,
laser, filter, PMT voltage, pixel size, scan mode, scanner software).
**Oracle:** tifffile's `mdgel` series (every pixel, float32).

## 2026-10-06 — 12-bit JPEG pages (compression 7, BitsPerSample 12)

**Why.** 12-bit JPEG pages exited 6 (`jpeg-decoder` stops at 8-bit DCT samples). Scientific cameras and some slide scanners write them.
**Prior art consulted:** ITU-T T.81 (sequential DCT, Huffman coding; public), ITU-T T.871 (JFIF colour conversion); tifffile 2026.9.20 and imagecodecs 2026.8.16 (BSD-3-Clause) as the oracle, run as black boxes: tifffile returns such pages as uint16.
**Rule implemented:** a page with compression 7, BitsPerSample 12 and unsigned samples is read as uint16; each chunk is decoded by the 12-bit decoder of `openreadout-codecs` (the same JPEG colour rules as 8-bit pages). Progressive, arithmetic-coded or chroma-subsampled 12-bit chunks exit 6.
