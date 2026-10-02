# 3DHISTECH MIRAX (`.mrxs`)

3DHISTECH slide scanners save whole slides as MIRAX: a `.mrxs` file next to a directory of settings, index and data files. OpenReadout returns the slide as a tiled image pyramid with its channels, plus the other stored images and attachments. Derived from OpenSlide's public format page and Benjamin Gilbert's public "Introduction to MIRAX/MRXS" (documentation prose, no source code), public slides, and comparison with OpenSlide run as a black box. Provenance: `docs/provenance/mirax.md`. Reader: crate `openreadout-wsi`, module `mirax`, format id `mirax`.

A MIRAX slide is a `.mrxs` file (itself a JPEG preview of the slide) next to a directory with the
same name without the extension (`data_dir`), holding:

| file | content |
| --- | --- |
| `Slidedat.ini` | settings (`Ini`, `IniSection`): UTF-8 with a byte-order mark (UTF-16LE with a BOM is decoded too, `decode_text`), `[SECTION]` and `KEY = VALUE` lines |
| `Index.dat` (`[HIERARCHICAL] INDEXFILE`) | where every stored item lies (`IndexFile`) |
| `Data0000.dat` … (`[DATAFILE] FILE_COUNT`, `FILE_<n>`) | the stored images and records, each after a 5-character version, the slide id, the file number and 256 bytes of padding |

## Settings (`Slidedat.ini`)

`[GENERAL]`: `SLIDE_ID`, `SLIDE_NAME` (image name), `SLIDE_VERSION`, `CURRENT_SLIDE_VERSION`
(`format_version`: 1.9, 2, 2.2 in the corpus), `IMAGENUMBER_X/Y` (the image grid, `grid`),
`CameraImageDivisionsPerSide` (`divisions`: images per camera photo side, 4 in every corpus file),
`SLIDE_TYPE` (`SLIDE_TYPE_BRIGHTFIELD`, `SLIDE_TYPE_FLUORESCENCE`), `SLIDE_CREATIONDATETIME`
(`DD/MM/YYYY hh:mm:ss`, `creation_time` → `acquired_at`, no zone), `OBJECTIVE_MAGNIFICATION`,
`OBJECTIVE_NAME` (objective), `CAMERA_TYPE` (detector). `[NONHIERLAYER_0_SECTION]`:
`SCANNER_SOFTWARE_VERSION` (`1,12,25,1` → software version `1.12.25.1`), `SCANNER_HARDWARE_VERSION`,
`SCANNING_TIME_IN_SEC`, `SCANNED_FOV_COUNT` (in `extra.mirax`). The whole file is in `info --view full` →
`vendor.slidedat`.

`[HIERARCHICAL]` lists hierarchies `HIER_<i>_NAME` with `HIER_<i>_COUNT` values `HIER_<i>_VAL_<j>`,
each with a `…_SECTION`, and non-hierarchical layers `NONHIER_<i>_NAME` likewise.

- `Slide zoom level` (`ZOOM_HIERARCHY`): one value per pyramid level (`ZoomLevel`); its section has
  `MICROMETER_PER_PIXEL_X/Y` (`mpp`), `OVERLAP_X/Y` (`overlap`: nominal overlap of neighbouring
  camera photos, pixels of that level), `IMAGE_CONCAT_FACTOR` (`concat_factor`), `IMAGE_FORMAT`
  (`image_format`: `JPEG`, `PNG`, `BMP` or `BMP24`), `IMAGE_FILL_COLOR_BGR` (`fill_color`),
  `DIGITIZER_WIDTH/HEIGHT` (`image_size`: every stored image's size).
- `Slide filter level` (`FILTER_HIERARCHY`): one value per filter (`Filter`): `FILTER_NAME`
  (`name`), `COLOR_R/G/B` (`color`), `STORING_CHANNEL_NUMBER` (`storing_channel`),
  `EXPOSURE_TIME` (`exposure_raw`, unit not recorded), `DIGITALGAIN` (`digital_gain`).
  Brightfield slides have three `Default` filters.
- Other hierarchies (`Scan info layer`) have empty records in the corpus.

## Index file (`Index.dat`)

| offset | size | content |
| --- | --- | --- |
| 0 | 5 | version (`01.02`), `version` |
| 5 | len(`SLIDE_ID`) | the slide id again (checked) |
| +0 | 4 | `hier_root`: offset of the hierarchical root table |
| +4 | 4 | `nonhier_root`: offset of the non-hierarchical root table |

All integers are little-endian 32-bit. A root table has one 4-byte pointer per record: the
hierarchical records of `HIER_0` values in order, then `HIER_1`'s, and so on (the non-hierarchical
table likewise). A record pointer leads to a linked list of data pages (0: the record is empty, as
the version-2 files write for their filter records): each page is `count`, `next` (0 ends the list)
and `count` items; the first page holds no items. Hierarchical items (`HierItem`) are `image`
(`y · IMAGENUMBER_X + x` of the image's first grid cell), `offset`, `length`, `file`;
non-hierarchical items (`NonHierItem`) two leading `words` (0 in the corpus), then `offset`,
`length`, `file`. Lists that loop, point outside the file or hold more than `MAX_RECORD_ITEMS`
items are errors (`IndexProblem`: `at`, `what`).

## Pyramid levels and placement

Level k's images sit on grid indices that are multiples of its `step` = 2^(sum of
`IMAGE_CONCAT_FACTOR` of levels 0..k): 1, 2, 4, … in slides as scanned (factor 0 at level 0, 1
above), from 2 or 16 in slides saved at 1/2 or 1/16 resolution. An image covers `step × step` grid
cells; a camera photo is `divisions × divisions` cells.

**Camera positions** (`Camera`: `flag`, `x`, `y`; `PositionSource`): 9-byte records (flag byte, `i32`
x, `i32` y) in row-major camera order, `IMAGENUMBER_X/divisions × IMAGENUMBER_Y/divisions` of them,
in pixels of the file's level 0: the non-hierarchical record `VIMSLIDE_POSITION_BUFFER`/`default`
(`PositionBuffer`), or in version 2.2 and later `StitchingIntensityLayer`/`StitchingIntensityLevel`,
zlib-compressed (`StitchingIntensity`). In version 1.9 and later flag 1 marks cameras with stored
images; the others hold 0 or garbage. A camera is drawn (`valid_cameras`) when a level-0 image
covers it and, if any flag is 1, its flag is 1.

**Placement** (`LevelLayout`, `Subtile`: `image`, `src`, `dest`, `order`), level k, positions
divided by `step / step₀` (2^k):

- `step ≤ divisions`: an image lies inside one camera photo; it goes to that camera's position plus
  `((x mod d)/step · w, (y mod d)/step · h)`.
- `step > divisions`: an image holds `(step/d)²` camera photos ("subtiles"), each `w·d/step ×
  h·d/step` pixels (fractional at coarse levels), each placed at its own camera's position; cameras
  beyond the camera grid's last column are skipped, not wrapped into the next row.
- Without a position table (slides exported with overlaps removed): no development file shows
  how such slides are placed, so their pixels are refused (exit 6); `info` gives the image grid
  as the size.

**Composition** (`compose`), derived by comparison with OpenSlide regions
(`docs/provenance/mirax.md`): each placed rectangle is resampled bilinearly at its fractional
offset, pixels outside it transparent; a rectangle whose source starts at a fractional pixel is
first resampled into its own `ceil(w) × ceil(h)` surface; rectangles are painted in reverse raster
order of their first grid cell with a "saturate" rule (a source adds only the coverage the
destination still lacks), so where camera photos overlap the later one in raster order shows, and
two images meeting at a half pixel average there with full coverage. What stays uncovered, and the
uncovered part of edge pixels, is the level's fill colour (`fill_rgb`: `IMAGE_FILL_COLOR_BGR` read
as `0xBBGGRR`; grey in every corpus file, so the order is unverified and a non-grey value is noted).
At level 0 every offset is an integer and the stored samples are copied unchanged.

**Size.** Level 0 (`level0_size`): with positions, `cameras · (photo − o) + o` per axis, `photo =
image size · divisions / step₀`, `o = round(OVERLAP of level 0) / step₀`, truncated — OpenSlide's
level-0 size on all corpus files, kept so that coordinates agree with OpenSlide-based tools (in
saved slides `o` is not the physical overlap). Without positions: the grid of images. Level k
(`level_size`): level-0 size / 2^k, rounded down.

Region reads decode only the images whose subtiles touch the region (in parallel; a 256 MiB cache
keeps them for the next read) and compose in bands of 512 rows.

## Channels

Brightfield: one channel of interleaved 8-bit R, G, B. Fluorescence (`is_fluorescence`): each stored
image is a 3-component JPEG whose components hold up to three filters; filter k is decoded
component `2 − STORING_CHANNEL_NUMBER` (B, G, R storage: in `openslide-mirax2-fluorescence-1` the
`UV` filter, storing channel 2, is the red component and shows nuclei). One `uint8` channel per
filter, named by `FILTER_NAME`, colour from `COLOR_R/G/B`. More than three filters or two filters
sharing a component exit 6.

## Stored images

`IMAGE_FORMAT` `JPEG`, `PNG` or `BMP` (`codec`, `ImageCodec`: `Jpeg`, `Png`, `Bmp`; `sniff`,
`header_size`). All are decoded to 8-bit RGB (`decode_rgb`, `RgbImage`: `width`, `height`,
`data`): JPEG with `openreadout-codecs`, PNG with the `png` crate, BMP by `decode_bmp` (uncompressed
8-bit palette, 24- and 32-bit rows, bottom-up or top-down). A decoded image must have the level's
`image_size`, else it is corrupt (exit 4).

## Attachments

Non-hierarchical records (`NonHierRecord`: `layer`, `value`, `section`, `items`) of `Scan data layer`:
`ScanDataLayer_SlideBarcode` → `label`, `ScanDataLayer_SlideThumbnail` → `macro`,
`ScanDataLayer_SlidePreview` → `thumbnail` (the names OpenSlide gives them), then every other record
with data under `layer/value` (scan maps and stage position maps as PNG, `ScanDataLayer_XMLInfoHeader`,
`ProfileXMLHeader`, `ProfileXML` as XML — also in `info --view full` → `vendor.xml_records` — histogram and
stitching records as binary), and the `.mrxs` preview JPEG (`mrxs preview`).

## Integrity checks (`check`)

`bad_index` (a record whose page list is broken), `missing_file` (a data file listed or used but
absent), `slide_id_mismatch` (warning: a data file header that does not name the slide), `truncated`
(an image or record past the end of its data file), `unsupported_codec` (warning), `off_grid_images`
(warning: an image index not on its level's grid), `no_positions`, `unplaced_images` (warning:
images of cameras flagged empty), `bad_image` (an image header whose size is not the level's, or the
first image of a level that does not decode). `check --headers-only` skips the image headers.

## Vocabulary (every public identifier in `openreadout-wsi` must appear here)

| identifier | meaning |
| --- | --- |
| `MiraxReader`, `MiraxDataset`, `FORMAT_ID`, `open` | reader and opened dataset |
| `Ini` { `sections` }, `IniSection` { `name`, `entries` }, `get`, `f64`, `i64`, `section`, `to_json`, `parse`, `parse_f64`, `decode_text` | settings file |
| `IndexFile` { `version`, `hier_root`, `nonhier_root` }, `len`, `is_empty`, `hier_items`, `nonhier_items`, `MAX_RECORD_ITEMS` | index file |
| `HierItem` { `image`, `offset`, `length`, `file` }, `NonHierItem` { `words`, `offset`, `length`, `file` }, `IndexProblem` { `at`, `what` } | index items and problems |
| `MiraxSlide` { `path`, `dir`, `ini`, `index`, `data_files`, `grid`, `divisions`, `levels`, `filters`, `cameras`, `nonhier`, `problems` }, `record`, `read_item`, `camera_grid`, `is_fluorescence`, `codec`, `level0_size`, `level_size`, `general`, `data_dir`, `creation_time` | the slide model |
| `ZoomLevel` { `level`, `section`, `mpp`, `overlap`, `concat_factor`, `step`, `image_format`, `fill_color`, `image_size`, `images` }, `ZOOM_HIERARCHY` | pyramid level |
| `Filter` { `name`, `color`, `storing_channel`, `exposure_raw`, `digital_gain` }, `FILTER_HIERARCHY` | filter (channel) |
| `Camera` { `flag`, `x`, `y` }, `PositionSource` { `PositionBuffer`, `StitchingIntensity` }, `name` | camera positions |
| `NonHierRecord` { `layer`, `value`, `section`, `items` } | non-hierarchical record |
| `LevelLayout` { `subtiles`, `unplaced` }, `build`, `candidates`, `images_for`, `Subtile` { `image`, `src`, `dest`, `order` }, `valid_cameras`, `compose`, `fill_rgb` | placement and composition |
| `ImageCodec` { `Jpeg`, `Png`, `Bmp` }, `sniff`, `RgbImage` { `width`, `height`, `data` }, `decode_rgb`, `decode_bmp`, `header_size` | stored images |
| `mirax`, `image`, `dataset`, `index`, `ini`, `render`, `slide`, `assurance` | modules |

`info` → `images[0].extra.mirax`: `slide_id`, `slide_version`, `current_slide_version`,
`index_version`, `slide_type`, `image_grid`, `camera_divisions`, `camera_grid`, `camera_positions`,
`cameras_with_images`, `levels[]` (`level`, `grid_step`, `concat_factor`, `image_format`,
`image_size`, `images`, `overlap_px`, `micrometre_per_pixel`, `fill_color_bgr`), `filters[]`
(`name`, `storing_channel`, `exposure_time_raw`, `digital_gain`), `scanner_hardware_version`,
`scanner_software_version`, `scanning_time_s`, `scanned_fields`; `images[0].extra.codec`.

## Oracle mapping (corpus harness)

`oracle/gen.py` (`mirax`): OpenSlide's level-0 size, mpp and level sizes; whole levels up to
48 MB read with `read_region`, composited over `openslide.background-color`, hashed with their
means (lossy: compared by mean, or by sidecar within `pixel_tolerance`); a fluorescence slide's RGB
split into one plane per filter. `oracle/gen_regions.py` (`mirax_targets`): the standard
rectangles of chosen levels plus 512 × 512 windows on tissue.
