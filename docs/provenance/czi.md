# Provenance log — Zeiss CZI

Rules: `docs/legal/clean-room-policy.md`.

## 2026-09-22 — initial derivation (Richard Zimring with Claude as assistant)

**Corpus files used:** `zenodo10577621-Channel-ZStack-LineScan-Bidirectional-Averaging` (67 KB, CC-BY-4.0), `aics-s-1-t-1-c-1-z-1`, `aics-RGB-8bit` (BSD-3), `zenodo7015307-S-2-2x2-CH-1`, `zenodo7015307-T-3-Z-5-CH-2` (CC-BY-4.0), `openslide-zeiss-5-slidepreview-{zstd0,zstd1-hilo,jxr}` (CC0).

**Prior art consulted (documentation only):**
- `czifile` README, Christoph Gohlke, BSD-3-Clause, https://github.com/cgohlke/czifile — segment ids, the ten dimension letters, the pixel-type and compression-id tables, the note that the ZEISS specification is confidential.
- ZEISS `libCZI` documentation site (concept pages, not source), LGPL-3.0, https://zeiss.github.io/libczi/ — the terms "subblock", "pyramid", "resolution protocol" and the statement that JPEG-XR subblocks may disagree with the declared pixel type.
- Bio-Formats format page (documentation page only) https://bio-formats.readthedocs.io/en/stable/formats/zeiss-czi.html.

**Method:** wrote a 60-line Python walker that follows `allocated_size` from segment to segment, verified for every file that the `ZISRAWFILE` header's three positions land exactly on `ZISRAWMETADATA`, `ZISRAWDIRECTORY` and `ZISRAWATTDIR` segments; that each subblock's `used_size == 256 + metadata_size + data_size + attachment_size` (or `16 + entry_len` when larger); that directory entries are byte-identical to the entry inside the subblock they point to; that `data_size` for uncompressed subblocks equals `stored_x * stored_y * bytes_per_pixel`. Read `zstd1`'s three header bytes off the OpenSlide file (`03 01 01`) and confirmed `28 B5 2F FD` (the zstd frame magic, public RFC 8878) follows; confirmed `zstd0` starts directly with the frame magic and `jpeg_xr` with `49 49 BC 01` (the JPEG XR codestream signature, ITU-T T.832). Attachment entry layout derived from the byte pattern `A1 … file_position … JPG … Thumbnail`.

**Inferred, awaiting corroboration:** exact meaning of the reserved 5 bytes after `pyramid_type`; `zstd1` header fields other than `hilo` (bit 0 of byte 2); the `chunked` compression id range.

## 2026-09-22 — completing the reader: JPEG, resolution protocol, attachments, multi-file, scenes, pyramid reads (Richard Zimring with Claude as assistant)

Logged before the parsing code was written; findings appended as they were confirmed.

**Corpus files used:** `openslide-zeiss-5-{jxr,flat,cropped,slidepreview-jxr,slidepreview-zstd0}` (CC0), `zenodo7015307-T-3-CH-2`, `zenodo7015307-W96-B2-B4-S-2-T-1-Z-1-C-1-Tile-5x9`, `zenodo10577621-LineScan-T80-Z25`, `zenodo10577621-Intestine-3color-RAC` (CC-BY-4.0), `aics-s-1-t-1-c-1-z-1`, `aics-s-3-t-1-c-3-z-5` (BSD-3), plus the new `synthetic-*` fixtures described below.

**Prior art consulted:**
- `czifile` 2026.8.16 source and docstrings (BSD-3-Clause, Christoph Gohlke, https://github.com/cgohlke/czifile), read as documentation: the `CZTIMS` and `CZEVL` content schemas (`read_time_stamps`, `read_event_list`, `CziEventListEntry`), the event type names, the attachment content-type list (`CZI`, `JPG`, `CZTIMS`, `CZEVL`, `CZLUT`, `CZFOC`, `CZEXP`, `CZHWS`, `Zip-Comp`, ...), the pixel-type conversion table used when compositing mixed pixel types (`CONVERT_PIXELTYPE`), the pyramid-level grouping (`normalize_pyramid_scale`, per-level stored-space origin `round(start / factor)`), and that compression id 1 is decoded as a plain JPEG stream without a B/R channel swap.
- ZEISS `libCZI` documentation pages (LGPL-3.0 project; pages only, no source): https://zeiss.github.io/libczi/pages/resolution_protocol.html — the directory entry is authoritative when it disagrees with the subblock header or with the decoded payload; a decoded JPEG XR bitmap whose size differs is cropped or zero-padded at the top-left origin; a differing pixel type is converted to the declared type (or zero-filled); these conditions indicate a damaged file and should be reported.
- `imagecodecs` documentation (BSD-3) for the JPEG/JPEG XR encoders used to build fixtures.

**Black-box oracle runs (pylibCZIrw 5.x, LGPL, run only):** writing supports `uncompressed:`, `zstd0:`, `zstd1:` only; `jpg:`/`jpgxr:` are rejected ("An unsupported compression mode was specified"). Reading a compression-id-1 subblock fails ("The method or operation is not implemented"), so czifile is the only JPEG oracle. On a JPEG XR stream carrying 48-bit RGB under a `bgr24` directory entry, pylibCZIrw returns each sample shifted right by 8 bits; czifile returns the low byte (a NumPy cast, not a conversion). On an 8-bit gray stream under `gray16`, both zero-extend. On a 24-bit RGB stream under `gray8`, both fail.

**Derived from hex dumps (our own Python walker, `oracle/make_czi_fixtures.py::read_container`):**
- `CZTIMS` (TimeStamps) payload: u32 `size`, u32 `count`, then `count` little-endian f64 values. The `size` field is inconsistent across writers (12 for one value in `openslide-zeiss-5-jxr`, 28 for three in `zenodo7015307-T-3-CH-2`, 648 = whole payload for 80 in `LineScan-T80-Z25`), so only `count` is trusted. Values are seconds; in `zenodo7015307-T-3-CH-2` their differences (0.338 s, 0.339 s) equal the differences of the per-subblock `AcquisitionTime` tags of consecutive T, so they are per-T acquisition times on a relative clock.
- `CZEVL` (EventList) payload: u32 `size`, u32 `count`, then entries of i32 `entry_size`, f64 `time` (s), i32 `event_type`, i32 `description_size`, `description_size` bytes of NUL-terminated text; `entry_size` = 20 + `description_size` (e.g. `28 00 00 00 … 00 00 00 00 14 00 00 00 "IncubationRecording\0"` in `zenodo7015307-T-3-CH-2`). Empty lists are `08 00 00 00 00 00 00 00`.
- Subblock metadata `<METADATA><Tags>` carries `AcquisitionTime` (ISO-8601 UTC), `StageXPosition`/`StageYPosition`/`FocusPosition` (µm, zero-padded signed decimals) and sometimes a `DetectorState` with `ExposureTime` (ns).
- `Information/Image/Dimensions/S/Scenes/Scene` carries `CenterPosition` ("x,y" µm), `ContourSize`, and in well-plate files a `Shape` element with `Name`, `Id`, `RowIndex`, `ColumnIndex` (`zenodo7015307-W96-…`: scene `B2` has `RowIndex 2`, `ColumnIndex 2`).
- `DisplaySetting/Channels/Channel[@Id]` carries `Color`, `DyeName`, `DyeMaxEmission`, `DyeMaxExcitation`, `IlluminationType`; matched to image channels by `Id`, not position.
- The OpenSlide `Zeiss-5-*` previews declare `Bgr24` in XML and `Bgr48` (id 4) in the directory; their JPEG XR stream is 48-bit and czifile returns uint16 RGB. The directory wins (already implemented).

**Synthetic fixtures:** no public CZI with JPEG (id 1) subblocks or a multi-file document was found (search of Zenodo, the BioImage Archive, the OpenSlide and OME sample-image indexes). `oracle/make_czi_fixtures.py` makes them: base files written by pylibCZIrw from NumPy arrays; JPEG and JPEG XR variants produced by re-encoding each subblock's pixels with imagecodecs and re-writing the container with our own writer (layout from `docs/formats/czi.md`); multi-file documents by moving T>0 subblocks into `<stem> (1).czi`, `<stem> (2).czi`.

**Inferred, awaiting corroboration:** the multi-file layout (following parts are named `<stem> (<k>).czi`, carry `file_part = k` and the master's `file_guid` as `primary_file_guid`, and hold only subblock/attachment segments; the directory and XML live in the master). Confidence `inferred`; no real multi-file corpus exists yet.

**Search for public JPEG / multi-file CZIs (2026-09-22, by header range reads, no whole-file downloads):** ~890 CZI files from Zenodo (all 307 records the API returns for `file_type=czi`), figshare (300 articles), the BioImage Archive (S-BIAD1343, 1285, 1407, 2492), IDR idr0164, the OME `Zeiss-CZI` sample directory, the OpenSlide Zeiss test data and the aicsimageio test resources. Compression ids seen: 0, 2, 4, 5, 6; none used JPEG (id 1). No file had `file_part > 0`. Zenodo 19047136 ("CZI file examples", CC-BY-4.0) holds `Image_1_…964.czi` plus `…964(1).czi` … `(3).czi`: independent Lightsheet tile files (each `file_part` 0 with its own primary GUID), which is why part discovery also tries the `<stem>(k).czi` spelling but only ever runs when an entry names `file_part > 0`. One figshare file (article 25335139, CC-BY-4.0, 259 KB) uses LZW (id 2); it is a candidate to validate the LZW path and is not yet in the manifest.

**Valid-pixel masks (found while validating pyramid levels):** the first pyramid-level comparison against czifile failed on tiled scans because czifile leaves masked-out pixels at the fill value. Layout taken from the libCZI documentation page https://zeiss.github.io/libczi/pages/valid_pixel_mask_concept.html (chunk container: GUID `{CBE3EA67-5BFC-492B-A16A-ECE378031448}`, i32 size, then u32 width, height, representation 0, stride, MSB-first bits, 1 = valid) and czifile's `CziSubBlockSegmentData.mask` (BSD-3), which also accepts the GUID after 16 leading bytes. Honouring the mask made every pyramid-level plane in the corpus match czifile.

**Pixel type 12 (`gray32`):** czifile maps it to a signed 32-bit integer (`<i4`); we now return `int32` (was `uint32`). No corpus file uses it.

**Compression id 7:** czifile names it "chunked (experimental)" with a link to the libCZI documentation page on chunked compression; we now report it as `chunked(7)` (unsupported, exit 6) instead of `unknown(7)`.

**Validation results (this entry):** corpus harness: 59 CZI files pass (5 documented skips), 588 full-resolution planes bit-exact, 220 pyramid-level planes on 18 pyramidal files bit-exact, 14 planes of 3 lossy-JPEG fixtures within tolerance (max |Δ| 1 for gray8, 3 for 4:2:0 RGB, 2 for 4:4:4 RGB against libjpeg-turbo), lossless 16-bit JPEG bit-exact, and the resolution-protocol fixtures bit-exact against pylibCZIrw. `oracle/validate_czi_extras.py` against czifile on 66 CZI files: 163/163 attachment payloads byte-identical via `export --attachment`, time stamps equal on 43/43 files, event lists equal on 43/43, 504/504 per-plane acquisition times equal to the subblock `AcquisitionTime` czifile reads.

## 2026-09-22 — performance: fewer copies on the decode path (Richard Zimring with Claude as assistant)

**Corpus files used:** all CZI files (corpus harness), the `openslide-zeiss-5-slidepreview-*` files for timing. **Prior art consulted:** none.

Performance only; no parsing logic or interpretation changed. The resolution protocol hands the decoder's buffer on when the bitmap already matches its directory entry (it was copied), B,G,R to R,G,B swaps happen in place, zstd frames of known size are decoded straight into a buffer of that size, and the HiLo unshuffle writes into a preallocated buffer. Criterion: `decode/czi-zstd0` 118 to 245 MiB/s, `decode/czi-zstd1-hilo` 184 to 264 MiB/s, `codec/hilo-unshuffle` 0.9 to 31 GiB/s. The corpus harness is bit-exact as before.

The tiles (subblocks) of one plane are now decoded in batches on the `--threads` pool (at most one decoded tile per thread and 256 MiB of them) and pasted in the same order as before, so planes are identical for any thread count (`check --planes --json` equal to the previous build on all 68 CZI corpus files). Whole-slide scenes with few, huge planes previously used at most one thread per plane: export of `openslide-zeiss-5-jxr` (two 367 MiB mosaic planes of 27 JPEG XR tiles) at 8 threads went from 2.5 s to 1.2 s.
## 2026-09-22 — robustness hardening

**Scope:** no change to what the reader infers from a file; only bounds checks, checked
arithmetic, allocation caps (`openreadout_core::limits`) and error paths, so malformed input
yields `corrupt_file` / `unsupported_feature` instead of a panic, overflow or out-of-memory abort.
Directory/attachment-directory/metadata sizes are capped against the file length and 512 MiB; a directory shorter than its 128-byte header is treated as unusable (scan fallback) instead of indexed; mosaic bounds, Z/C/T extents and tile offsets use saturating/`i64` arithmetic; tiles are clipped to the plane; JPEG XR tiles are checked against the directory geometry before decoding; `#AARRGGBB` colours must be ASCII.

**Corpus files used:** the smoke tier (malformed-file matrix: truncations, byte flips) and the
synthetic minimal files written by `fuzz/seeds.py`; fuzz findings are in
`crates/openreadout-czi/tests/fixtures/malformed/`.

**Prior art consulted:** none (no format knowledge was needed; the limits follow from our own layout notes above).

## 2026-09-22 — robustness (fuzzing): bounded JPEG frames

**Scope:** a bound only. The `codec_jpeg` fuzz target found that `jpeg-decoder` sizes its per-component planes from the frame header before applying its output cap (a 342-byte stream made it allocate 4 GB), and that a tiny stream declaring about 60000 x 15000 pixels takes tens of seconds to decode. JPEG subblocks (compression 1) are now decoded with `openreadout_codecs::jpeg_decode_limited`: a frame whose decoded size exceeds four times the subblock size the directory declares (at least 1 MiB) is refused before decoding. Decoded pixels are unchanged. **Prior art consulted:** none.

## 2026-09-23 — growing files (`write_state`)

**Scope:** a new, read-only judgement on top of the existing sequential segment walk; reading is unchanged. `Dataset::write_state` reports a CZI whose file header has subblock-directory position 0 (not finalized) as unfinished when the segment chain is intact up to a last segment that may run past the end; a header position that points past the end is a finished file cut short and is not reported. Complete planes are the level-0 subblocks found by the walk, in file order.

**Corpus files used:** `zenodo7015307-T-2-Z-5-CH-1.czi`, `zenodo7015307-T-3-Z-5-CH-2.czi` (replayed in `crates/openreadout-corpus-tests/tests/live.rs`).

**Inferred:** nothing new about the format. The replay's write order (header positions 0 until the end, directory and metadata segments reserved and filled last) is an assumption, documented as such in `book/src/guides/lab-shares.md`; the corpus files show the directory and metadata segments before the subblocks, which is consistent with reserved space but does not prove it.

**Prior art consulted:** none.

## 2026-09-23 — performance / robustness merge

Reconciled the existing performance branch with main; no new format layout was inferred.
Inputs: existing committed synthetic and malformed regression fixtures and the shared public
corpus (ids/licences unchanged in `corpus/manifest.toml`). Prior art: repository code and notes;
weezl 0.2.1 public buffer API (https://docs.rs/weezl/0.2.1/weezl/decode/struct.IntoStream.html,
MIT OR Apache-2.0) for a reusable bounded LZW staging buffer. Retained file-relative allocation
limits, checked geometry, bounded decoder output and capped sidecar reads alongside parallel
decoding and copy avoidance. The common plane-size guard also enforces the 4 GiB ceiling.

## 2026-09-23 — user name and microscope system

**Scope:** `images[].extra.experimenter` (new) and the `images[].instrument.model` fallback, for the experiment model's operator and instrument model.

**Corpus files used:** every CZI input, metadata XML read through `info --view full --json` and cross-checked with a few lines of plain Python that read the XML at the file header's metadata position (`evals/facts.py`). 50 of 68 files have `Information/Document/UserName` (`M1SRH` on the 27 Celldiscoverer 7 files of `zenodo7015307-*`, `zeiss`/`darkmaster` on the OpenSlide Axioscan files, `m1psc`, `LSM User`, `ruiany`, `sara.carlson`, …); the 6 LSM files of `zenodo10577621-*` and `aics-NoSceneNames` also have `Information/User/DisplayName`, always equal to `Document/UserName`. The pylibCZIrw-written `synthetic-*` files have neither. Those LSM files (`LineScan-*`, `Channel-ZStack-*`, `*PALM*`, `aics-NoSceneNames`) name no `Microscope/@Name` but have `Information/Instrument/Microscopes/Microscope/System` (`LSM 710, AxioObserver`, `LSM 780, Axio Examiner`, `Andor1, AxioObserver`).

**Inferred:** `Document/UserName` (else `User/DisplayName`) is the account the document was acquired under: `images[].extra.experimenter` = `{"user_name": …}`, the shape the OME-TIFF writer and the experiment model already read (`Experimenter/@UserName`, `acquisition.operator`). `Microscope/System` is the model when `Microscope/@Name` is absent. Generic accounts (`zeiss`, `LSM User`) are kept as recorded.

**Prior art consulted:** none.

## 2026-09-23 — Region reads (Richard Zimring with Claude as assistant)

**Scope.** `Dataset::read_region`: a rectangle of a scene's plane at level 0 or a pyramid level, composited from only the subblocks whose stored extent overlaps it (same placement, paste order and valid-pixel masks as a whole-plane read). Makes rectangles of stitched scenes above 4 GiB readable (e.g. the 190 309 × 69 378 RGB scene of `zenodo10577621-Young-mouse`). Decoded subblocks are kept in a 64 MiB cache between region reads. No new interpretation of the file: placement is the one already used for planes (level 0: `X`/`Y` start times the upsampling factor minus the scene origin; levels: origin divided by the snapped scale factor, round half to even).

**Oracle:** czifile (BSD-3-Clause) planes and levels, cropped in NumPy (`oracle/gen_regions.py`).
## 2026-09-23 — JPEG XR decoder replaced (compression 4)

**Scope:** decoding of JPEG XR subblocks moves from the `jpegxr-pure-rs` crate (a machine translation of jxrlib with raw pointers, which the fuzzer showed panicking and writing past its buffers) to `crates/openreadout-jpegxr`, a `forbid(unsafe_code)` port of the decoder path of Microsoft's jxrlib. The CZI container logic is unchanged; the resolution protocol (`conform`) still applies. JPEG XR features the new decoder does not implement (alpha planes, CMYK, packed 5/6/10-bit formats) are exit 6 instead of exit 4.

**Prior art consulted:** jxrlib, the JPEG XR reference library ("JPEG XR Device Porting Kit v1.0", Copyright Microsoft Corp., BSD 2-clause "New BSD License"), as bundled in the `jpegxr` 0.3.1 crate (https://crates.io/crates/jpegxr, BSD-3-Clause wrapper around the BSD jxrlib sources): `image/decode/{strdec,segdec,strInvTransform,strPredQuantDec,decode}.c`, `image/sys/{strcodec,adapthuff,image,strPredQuant,strTransform}.c`, `jxrgluelib/JXRGlue{,Jxr}.c`. The port is a derivative of that code and keeps its licence (`crates/openreadout-jpegxr/LICENSE`, `NOTICE`). ITU-T T.832 (public) for terminology. No ZEISS material; JPEG XR is an open ITU/ISO standard, not a vendor format.

**Corpus files used:** every JPEG XR subblock of `openslide-zeiss-5-{cropped,flat,jxr,slidepreview-jxr}`, `synthetic-gray16-jxr`, `synthetic-jxr-mismatch-*`, `zenodo10577621-{Intestine-3color-RAC,Kidney-RAC-3color,Young-mouse}` (Axioscan and older ZEN encoders: codec sub-versions 0 and 1, frequency order, overlap 0 and 1, tiles, Gray16, Bgr24 and Bgr48).

**Inferred / validated:** nothing new about CZI. Decoded samples are compared byte for byte with jxrlib (a C build of the same sources, and imagecodecs 2026.8.16, which wraps jxrlib, run as black boxes) on 1,154 corpus subblocks and on 1,800 codestreams jxrlib encoded from synthetic images over every encoder option; all identical. One jxrlib behaviour had to be reproduced exactly: with hard tile boundaries its inverse transform compares macroblock columns with the horizontal-slice table and, at the pseudo-column past the right edge, writes into the memory that follows the row buffer — which is the other row buffer. The port lays its buffers out as jxrlib does so those pixels match.

## 2026-09-24 — Pyramid levels: grouping by level factor, level grid anchored at the scene origin

**Problem.** `zenodo10577621-Young-mouse` (one 190 309 × 69 378 RGB scene) reported 11 levels whose sizes stopped decreasing after level 6 (924 × 905, 1024 × 540, 462 × 456, 743 × 270). czifile 2026.8.16 (BSD-3-Clause, read as documentation: `CziScenes._make_image`, `normalize_pyramid_scale`) reports the same 11 levels: it groups pyramid subblocks by their exact per-entry `size / stored_size` ratios, snapping only when one axis is an exact integer. The coarsest subblocks of that scan are single tiles whose ratios are 64.062 × 64.035, 128.222 × 128, 128.158 × 128.069 and 256.444 × 256.043 (ZEN builds each level from the previous one, so a 1024-pixel tile at 1/128 covers 131 234 level-0 pixels, not 131 072); each became a level of its own.

**Corpus files used:** `zenodo10577621-Young-mouse`, `zenodo10577621-Kidney-RAC-3color`, `zenodo10577621-Intestine-3color-RAC`, `aics-variable-scene-shape-first-scene-pyramid`, `aics-OverViewScan`, `openslide-zeiss-5-cropped` (subblock directories listed with czifile; per-level sizes and pixels compared with pylibCZIrw).

**Black-box observations (pylibCZIrw 6.1.0, LGPL, run only).** `read(roi=scene rectangle, zoom=1/f)` returns `floor(w / f) × floor(h / f)` pixels for every tested f (2 … 256, and non-powers 3, 5, 7, 50, 77, 100). On the lossless `Kidney-RAC-3color` scene (origin −67 457, 22 459, level tiles starting one pixel left of it), its 1/2, 1/4 and 1/8 outputs equal czifile's level arrays exactly once shifted: a stored pixel of a subblock starting at level-0 coordinate `start` lands at output column `floor((start − x0) / f)`, `x0` the scene's level-0 origin (checked on both axes, offsets −2.5, −1.25, −0.625, 0.5, 0.75, 0.875).

**Inferred.** (1) A pyramid level is one downsampling factor per axis for the whole scene; a subblock's own ratio is that factor distorted by rounding of its stored size (`stored = size / f` ± 2 pixels), so a subblock belongs to the level whose factor lies in `[size / (stored + 2), size / (stored − 2)]`. Levels are formed from the largest subblocks first; a level's factor is the power of two inside that interval when there is one, else the nearest integer inside it, else the ratio. Smaller subblocks join the first level whose factor lies inside their own interval (tiny edge tiles have wide intervals); one that fits none starts a level of its own. (2) Level geometry follows the scene, not the bounding box of the level's subblocks: level k of a scene whose level-0 rectangle is `x0, y0, w, h` is `floor(w / f) × floor(h / f)` pixels, and a subblock is placed at `floor((start − x0) / f)`, clipped to the level — the geometry pylibCZIrw produces at `zoom = 1/f`, which keeps `level coordinate × f + x0` a level-0 coordinate. czifile's level arrays are the bounding boxes of the level's subblocks (`Kidney` level 1: 5067 × 2860 at level origin −33 729; ours and libCZI's: 5064 × 2859 at −33 728.5) and are not used for level geometry any more.

**Oracle.** `oracle/gen_regions.py`: level factors are the distinct powers of two nearest to the subblock ratios czifile lists; level-k pixels come from pylibCZIrw `read(zoom=1/f)` over the scene rectangle (black box).

## 2026-09-25 — Channel acquisition mode as one readable label

**Scope:** normalization of fields already parsed. `Channel/AcquisitionMode` and `Channel/ContrastMethod` hold OME enumeration tokens in every corpus CZI that has them (`WideField` 39 files, `SpinningDiskConfocal` 3, `TIRF` 2, `LaserScanningConfocalMicroscopy` 1; `Fluorescence` 37, `Brightfield` 11, `Phase` 1, `Other` 2, counted by `grep` over the corpus files' metadata XML). `acquisition_mode` was the mode token, else the contrast token; it is now the shared label of both (`openreadout_core::acquisition_mode::from_ome`: `WideField` + `Fluorescence` → `Widefield Fluorescence`, `BrightField`/`Brightfield` → `Brightfield`), the spelling ND2 and LIF already use, and it survives an OME-TIFF round trip. Values that are not OME tokens pass through unchanged.

**Corpus files used:** the CZI corpus inputs (smoke and standard tiers). **Prior art consulted:** the OME 2016-06 XSD (`oracle/schema/ome.xsd`) for the enumerations.

## 2026-09-25 — Fill in pyramid tiles of slide scans (Richard Zimring with Claude as assistant)

**Problem.** `openreadout preview` of `zenodo10577621-Intestine-3color-RAC` (the default reads pyramid level 2) drew rectangles outside the scanned tiles white and the tissue near black: auto contrast resolved to 0–65535. `zenodo10577621-Kidney-RAC-3color` (level 3) was uniformly dark (white point 13 891 for data up to ~3 200).

**Corpus files used:** `zenodo10577621-Intestine-3color-RAC`, `zenodo10577621-Kidney-RAC-3color` (checked also, no fill found: `zenodo10577621-Young-mouse`, `aics-OverViewScan`, `aics-variable-scene-shape-first-scene-pyramid`, `aics-S-2-4x2-T-2-Z-3-CH-2`, `openslide-zeiss-5-{cropped,jxr,slidepreview-zstd1-hilo}`, `zenodo7015307-{S-2-3x3-T-3-CH-2,S-2-2x2-CH-1,W96-B2-B4-S-2-T-1-Z-1-C-1-Tile-5x9,S-3-1Pos-2Mosaic-T-2-Z-3-CH-2,S-1-3x3-T-3-Z-4-CH-2}`).

**Prior art consulted:** czifile 2026.8.16 (BSD-3-Clause), run to list subblocks (`directory_entries`, `read_segment_data().data()`, `.mask()`) and to composite levels through `oracle/czi_levels.py`. pylibCZIrw 6.1.0 (LGPL) run as a black box: `read(roi=total rectangle, plane={"C": 0}, zoom=z)` for z = 1, 1/2, 1/4, 1/8, pixel statistics only. (A wrong keyword argument made it raise; the traceback printed one line of its Python wrapper. Nothing was taken from it.)

**Observations.** Level 0 (ours, czifile, pylibCZIrw zoom 1): 30 tiles of 1388 × 1040 per channel, maximum 4095 (`ComponentBitCount` 12), no sample at 65535, 36.6% of the bounding box uncovered and 0. Pyramid subblocks (`pyramid_type` 2, no valid-pixel masks) cover rectangles larger than the acquired tiles and store 65535 there: level 1 subblocks up to 51% of their samples, the single level-3 subblock 36%. Along the boundary of the scanned area the pyramid holds values between 4096 and 65534 (the fill averaged with data by ZEN's downsampling; in `Kidney-RAC-3color`, whose tiles cover the whole box, only these, 0.2–0.6% of the samples of levels 2–4). pylibCZIrw at zoom 1/2, 1/4, 1/8 returns 8.8%, 13.0%, 36.0% samples at 65535 — the same as czifile and our reader (the level oracle hashes in `corpus/oracle/zenodo10577621-Intestine-3color-RAC.json` still match).

**Inferred.** Nothing new about the format and no reader change: uncovered canvas stays 0 (as czifile and pylibCZIrw), pyramid subblocks are returned as stored. 65535 above a 12-bit recorded depth is fill, not a measurement; the preview renderer now screens it (`crates/openreadout-preview/src/image.rs`, `screen_samples`): samples above the recorded depth are drawn as background when at least half of them are the type's maximum, otherwise, if they are at most 1% of the samples, only left out of the auto contrast; without a recorded depth, the type's maximum is left out of the auto contrast when it would set the white point.

## 2026-09-25 — assurance profile (`src/assurance.rs`)

**Corpus files:** every development input of this format on disk (`corpus/assurance/evidence.json` lists them); no held-out file.
**Prior art consulted:** none.
**What was done.** No parsing logic changed. The reader declares an assurance profile (docs/assurance.md) that fingerprints each file from this reader's own normalized output: the writer family from `images[].instrument.software`, codecs from `extra.compression`, `extra.stored_pixel_type`, mosaic/pyramid/multi-scene/multi-file layout, and the notes for extra dimensions, missing parts and structure problems. The table of validated feature values and the confidence level are generated from the development corpus by `cargo xtask assurance-audit`.

## 2026-09-26 — differential cross-check with a second reader; super-resolved subblocks flagged

**Corpus files:** every development CZI on disk (`corpus/oracle/second/*.json`); no held-out file.
**Prior art consulted:** pylibCZIrw 6.1 (LGPL; ZEISS libCZI), run as a black box by `oracle/second_opinion.py` (`read(plane=, scene=, roi=scene rectangle)`). czifile (BSD-3) and Bio-Formats 8.5 `showinf` (GPL, black box) for the two PALM files.
**Observations.** OpenReadout agrees with pylibCZIrw on every compared plane except four files (`corpus/oracle/second/adjudications.toml`): pylibCZIrw mis-decodes the zstd1-HiLo 48-bit preview (our decode equals the zstd0 copy of the same image, and czifile); a script artefact on per-scene T; and the two PALM renderings `zenodo10577621-PALM-OnlineVerrechnet` / `-Palm-mitDrift`, whose channel-0 subblock stores more pixels than its logical extent (1206 for 512, 2560 for 256). czifile and pylibCZIrw resample it to the logical grid; OpenReadout (like Bio-Formats) exposes the stored grid but keeps the logical grid's pixel size and puts the other channel, stored at the logical size, in the top-left corner of the larger canvas.
**Inferred / done.** No parsing change. The assurance profile now reports level-0 subblocks with `stored_size > size` as an undecoded structure affecting pixels and metadata (`Dataset::assurance_observations`), so these files are `unvalidated` and `--strict` refuses them. Reader fix (scale the pixel size by `size / stored_size` per channel or expose each resolution as its own image) left to the reader owners.

## 2026-09-25 — Super-resolved (PALM) renderings: one image per stored-to-logical ratio, pixel size of the rendering

**Why.** A second opinion (`corpus/oracle/second/adjudications.toml`) found the two ELYRA PALM files mis-registered: channel 0 stores a 1206 × 1206 rendering of a 512 × 512 logical extent (2560 × 2560 of 256 × 256 in `-Palm-mitDrift`), channel 1 the 512 × 512 widefield image; we exposed one 1206 × 1206 image with the widefield channel in its top-left corner and the logical grid's pixel size (0.1587 µm) for both. `zenodo10577621-Image-5-PALM-verrechnet` (the rendering alone) had the same pixel-size error.

**Corpus files used:** `zenodo10577621-PALM-OnlineVerrechnet`, `-Palm-mitDrift`, `-Image-5-PALM-verrechnet` (Zenodo 10577621, CC-BY-4.0). **Prior art consulted:** czifile 2026.x (BSD-3-Clause, https://github.com/cgohlke/czifile) source read as documentation: it maps stored-size subblocks with the float ratio `stored_shape / shape` per axis (> 1 for PALM renderings, < 1 for Airyscan fast-scan) and reads them at the stored size only when every selected subblock shares one ratio (`asarray(storedsize=True)` with a dimension selection). Bio-Formats 8.5.0 and pylibCZIrw run as black boxes (the second-opinion oracles).

**Rule implemented:** full-resolution subblocks of a scene are grouped by their stored-to-logical ratio on X (to 1e-6); a scene with more than one ratio becomes one image per ratio (the channels of that group), named `<scene> (rendered at <ratio>x)` for ratios other than 1. Subblocks are placed at `round(start × ratio)` and the image's pixel size is the logical pixel size divided by the ratio (0.1587 × 512 / 1206 = 0.06739 µm; 0.1 × 256 / 2560 = 0.01 µm). A scene whose subblocks all share a ratio other than 1 (the rendering alone) keeps one image with the divided pixel size. Checked: each group's planes equal czifile's `asarray(storedsize=True, C=c)` (`oracle/gen.py`), the widefield channel equals every reader's 512 × 512 plane.

## 2026-09-26 — CZI from 20 depositors; ZEN 3.10/3.13 writers; extra dimensions (H/I/R/V/B) as one image per index

**Why.** The CZI evidence came from four depositors (59 passes), so the rubric could not rate the reader above medium, and ZEN 3.10-generation Axioscan scans (the writer that stamps itself plain `ZEN`, version 3.10) were absent from the development corpus. The second-opinion review asked for the ZEN 3.10 pyramid geometry to be reproduced on a new development file. Separately, subblocks along H, I, R, V or B beyond index 0 were not addressable.

**Corpus files used (all new, Zenodo, CC-BY-4.0):** 20 CZIs from 20 records — `zenodo14895059-Figure-6C-D`, `zenodo13144501-airyscan-processed-100x-004`, `zenodo17880403-Snap-8347`, `zenodo10666482-N2-gonad-1`, `zenodo17252016-Figure-6G-CTRL`, `zenodo14139319-SourceDataF2D-withPI3P`, `zenodo10044967-fig2D-GFP-10B-rot-1`, `zenodo17482295-Figure-08-E-coli`, `zenodo11260215-LS-foto-1`, `zenodo16966021-HILO-SIM-GFP-ER-timelapse`, `zenodo17098115-Snap-212a`, `zenodo10708864-ztacktimeposition16bit`, `zenodo19688024-Claurdan-DMSO-control`, `zenodo10659514-B30-FAM-probe-test`, and the ZEN 3.10.103 Axioscan scans `zenodo17723322-axioscan-3scenes-BF` (3 scenes, Bgr24 JPEG XR, scene origins at negative X), `zenodo17948627-axioscan-2scenes` (2 scenes, 3 channels, Gray16 JPEG XR), `zenodo17725097-axioscan-FL-multichannel`; `zenodo12509122-PSR-LXR-KO-WT-1054` (brightfield slide scan); `zenodo22090339-6h-timelapse-06-MIP` (ZEN 3.13, zstd1); `zenodo16419509-Rizzollo-Example1-SIM-raw` (ELYRA 7 SIM raw, ZEN black 16: H = 13, C = 3, T = 14). No held-out file was opened; the held-out Axioscan file's record is not a source of any of these.

**Prior art consulted:** czifile 2026.8.16 (BSD-3-Clause, https://github.com/cgohlke/czifile), run as the oracle (`oracle/gen.py`, `oracle/czi_levels.py`) and read as documentation for how it names the extra dimensions in `asxarray` (one axis per dimension letter). Remote directory survey: czifile over HTTP range requests (subblock directory and metadata only) on 10 public Zenodo CZIs to find files whose subblocks vary along H/I/R/V/B.

**Observations.** Every new file passes against czifile at level 0 and at every pyramid level (the three ZEN 3.10 scans: 15, 24 and 5 pyramid-level planes): the pyramid-level geometry of ZEN 3.10 Axioscan scans (factor-2 levels whose subblock ratios round to powers of two, e.g. 7.98 and 15.98; per-scene bounding boxes with negative origins) needs no change, so the held-out failure was not reproduced on these files. In the SIM raw file each subblock carries one H coordinate (0–12) next to C and T.

**Rule implemented:** a scene's full-resolution subblocks are grouped by their coordinates on the dimensions other than X, Y, Z, C, T, S and M (and, as before, by stored-to-logical ratio); when those coordinates vary within a scene, each combination is its own image, named `<scene> <D>=<k>` (dimension letters in alphabetical order, the last varying fastest), with `images[].extra.dimension_index` giving the coordinates; its pyramid subblocks are those with the same coordinates. A plane reads only subblocks at the image's coordinates (before: coordinate 0 only). Validated against czifile's `asxarray(scene=s)` indexed on those axes.

**Pyramid-level size mismatch reproduced on a development file (and resolved).** `zenodo12509122-PSR-LXR-KO-WT-1054` (ZEN 3.7.97 slide scan) builds its pyramid by 3 (subblock ratios 3.0, 9.0, 27.0, 81.0). The corpus test failed on its level sizes: ours 12064 × 5818, 4021 × 1939, 1340 × 646, 446 × 215; the oracle 9048 × 4364, 4524 × 2182, … because `oracle/czi_levels.py` snapped every ratio to a power of two (3 → 4, 9 → 8). pylibCZIrw (LGPL, black box) `read(roi=scene rectangle, zoom=1/3, 1/9, 1/27, 1/81)` returns exactly our sizes, so the reader was right and the oracle wrong: `czi_levels.level_groups` now takes base 3 when the finest pyramid ratio rounds to a multiple of 3 (else 2) and snaps each ratio to a power of that base; with it the file passes at all four levels. No reader change. Whether this is the geometry behind the held-out ZEN failure was not checked (held-out files are not opened for development); regenerating that oracle with the corrected `czi_levels.py` is a measurement for the held-out owner.

## 2026-09-26 — Channels named by `DisplaySetting` when `Information/Image` lists none; exposure checked

**Why.** The per-field comparison with Bio-Formats (`oracle/metadata_compare.py`; docs/benchmark/microscopy-metadata.md) found channel names missing in `zenodo10577621-Palm-mitDrift`, `-PALM-OnlineVerrechnet`, `-winnt` and `aics-RGB-8bit`, and exposure times that differ in 11 files.

**Corpus files used:** those four; `zenodo7015307-*` (6 files), `aics-OverViewScan`, `openslide-zeiss-5-slidepreview-*`, `ome-idr0011-Plate1-Blue-A-02-Scene-{1,2}-*` for the exposures. **Prior art consulted:** czifile (BSD-3-Clause) to list the metadata XML and each subblock's metadata segment.

**Observations.** The four files have no `Information/Image/Dimensions/Channels`; their only channel list is `DisplaySetting/Channels/Channel` with `Name` (`HR 1`, `SWF 1`; `1`; `C1`) and `StartC` in two of them. Exposure: our value is the channel's `ExposureTime` (ns). In `zenodo7015307-*` and `aics-OverViewScan` every subblock's own metadata segment repeats it (`ExposureTime` 20000000 and 5000000 ns); Bio-Formats reports 150 ms, the `ExposureTime` of the `ShadingReference/ShadingReferenceSource` element (the shading reference image's exposure). In `ome-idr0011-*` the two channels' elements hold 40 and 50 ms (ours); Bio-Formats reports 320 and 15 ms, values found in no `ExposureTime` element of the document. No exposure change.

**Rule implemented:** when `Information/Image/Dimensions/Channels` lists no channel, the channels are those of `DisplaySetting/Channels` in `StartC` order (document order when absent), each with the display entry's name, colour, dye and wavelengths. When it lists fewer channels than the display does (`zenodo10577621-PALM-OnlineVerrechnet`: only `HR 1`, the rendering; the widefield `SWF 1` appears only under DisplaySetting) and the ids match, the unmatched display channels follow the listed ones in document order.

**Also (same entry):** `instrument.detector` was never filled; Bio-Formats reports it for 50 development files. Each channel names its detector in `DetectorSettings/Detector/@Id`; `Information/Instrument/Detectors/Detector` with that `Id` gives `Manufacturer/Model` (`Axiocam705c`), else `@Name`, else `Type` (`PMT`). An image's detector is the distinct labels of its channels, in channel order, joined by ` + `.

**H-split images keep the scene's channels (same day).** Two ZEN 3.8/3.9 files from new depositors (`zenodo17482295-Figure-08-P-aeruginosa-H4`, `zenodo17432573-MC1-Nt3EmCherry-7412-H4`, CC-BY-4.0; found by the remote directory survey of 136 public Zenodo CZIs) vary along H (4 values) but store channels 1 and 3 (`RhodB#-T1`, `EGFP#-T2`) at H = 0 only. czifile's `asxarray` gives H × C = 4 × 4 with zeros where no subblock exists; our first split gave the H = 1…3 images the span of their own channels (3). Now every image of a split scene has the scene's Z/C/T extents; channels with no subblock at the image's coordinates are listed in `images[].extra.absent_channels` and read as 0 (before: an error for a missing plane). Both files pass against czifile, all 16 planes.

## 2026-10-06 — 12-bit DCT JPEG subblocks

**Why.** `check --planes` over the development corpus refused one CZI, `synthetic-gray16-jpeg12` (gray16 subblocks re-encoded as 12-bit extended-sequential JPEG under compression id 1): `jpeg-decoder` stops at 8-bit DCT samples.

**Corpus files used:** `synthetic-gray16-jpeg12`, `synthetic-gray16-jpeg-lossless` and `synthetic-gray8-jpeg` (all written by `oracle/make_czi_fixtures.py`). **Prior art consulted:** ITU-T Recommendation T.81 (the JPEG standard, freely published by the ITU: Annex A for the DCT and level shift, Annex B for the marker syntax, Annex F for sequential Huffman decoding) for the decoder; ITU-T T.871 (JFIF) for the YCbCr to RGB conversion; czifile 2026.8.16 (BSD-3-Clause) as the oracle, which decodes these subblocks with libjpeg-turbo through imagecodecs (BSD-3-Clause), and imagecodecs' `jpeg8_encode(bitspersample=12)` to write test streams (`oracle/make_jpeg12_fixtures.py`).

**Inferred.** A 12-bit subblock is an ordinary JPEG stream whose frame header (`SOF1`) declares precision 12; its quantization tables may hold 8- or 16-bit entries. Nothing in the directory entry or subblock header tells it apart from an 8-bit or lossless JPEG subblock: the pixel type is gray16 in both the lossless and the 12-bit case, so the frame header is the only place the process can be read.

**Rule implemented:** `openreadout-codecs` routes streams whose frame header declares 12-bit samples to its own sequential decoder (one or three components without chroma subsampling, restart intervals; progressive, arithmetic-coded and subsampled 12-bit streams are refused). The decoded samples (0–4095) fill the gray16 subblock as stored. The assurance profile reads the frame header of the first JPEG subblock of each pixel type and reports `jpeg 12-bit` or `jpeg lossless` as their own codec values, so a 12-bit or lossless JPEG file is not validated by 8-bit JPEG files.

## 2026-10-06 — Chunked compression (id 7); camera and system raw ids named

**Why.** Compression id 7 exited 6. libCZI's public documentation now describes it (https://zeiss.github.io/libczi/pages/chunked_compression.html, read as a documentation page only), and czifile 2026.8.16 decodes it through imagecodecs, so newer ZEN versions may write it.

**Corpus files used:** `synthetic-gray16-chunked-zstd-hilo`, `synthetic-gray8-chunked-lz4`, `synthetic-bgr48-chunked-zstd`, new fixtures made by `oracle/make_czi_fixtures.py --chunked-only`: the pylibCZIrw-written bases `synthetic-gray16-s2c2t2-uncompressed`, `synthetic-gray8-c2z2t3-uncompressed` and `synthetic-bgr48-uncompressed` with every subblock re-encoded by imagecodecs' `chunked_encode` (BSD-3-Clause). No public CZI with id 7 was found: the remote directory survey of this date (HTTP range reads of the subblock directory of public Zenodo CZIs; see the survey entry below) found ids 0, 4, 5 and 6 only.

**Prior art consulted:** the libCZI documentation page above (header entries 0–4, varints of seven-bit groups, zstd or LZ4, HiLo); czifile 2026.8.16 (BSD-3-Clause, read as documentation: it maps id 7 to imagecodecs' `chunked_decode` with the sample size, and ids 100–999 and 1000 and up to camera and system raw); imagecodecs 2026.8.16 run as a black box to write streams. pylibCZIrw 6.1 (LGPL, black box) does not read id 7 ("not implemented"), so czifile is the only oracle.

**Read on the documentation page:** the decompressed-size entry takes the forms `[C]` (every chunk decompresses to C bytes) and `[C, L]` (every chunk but the last to C, the last to L); the writer emits only these two. HiLo is reversed by "a decoder [that] first decompresses the chunks and then reverses this preprocessing step".

**Inferred from imagecodecs' streams (hex dumps):** the same size forms (6000 bytes in chunks of 4096 give `[4096, 1904]`; 6000 in chunks of 2000 give `[2000]`); LZ4 chunks are raw blocks. imagecodecs applies HiLo to each chunk on its own (a 2000-byte chunk of 16-bit samples holds 1000 low bytes, then 1000 high bytes), which differs from the reading of the page above (one split over the whole subblock) whenever there is more than one chunk.

**Rule implemented:** `openreadout-codecs::chunked_decode`: header entries 1–4 as documented; one decompressed size per chunk, `[C]` or `[C, L]` (other forms exit 6); each chunk must decode to its size, and the joined chunks must fill the subblock exactly; unknown entry ids, codecs or preprocessing values exit 6. HiLo is undone only when the subblock is one chunk, where both readings agree; HiLo over several chunks exits 6 until a file written by ZEN or libCZI shows which reading is meant. The three fixtures (the HiLo one with one chunk per subblock, the LZ4 one in the `[5000, 2288]` form) decode to the same planes as czifile and as their uncompressed bases. Compression ids 100–999 are now named `camera_raw(N)` and 1000 and up `system_raw(N)` (both still exit 6: czifile reads them as uncompressed samples, but no public file shows what a camera stores there).
