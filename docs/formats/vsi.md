# Olympus/Evident cellSens VSI (+ ETS)

Olympus/Evident cellSens software and the VS120 and VS200 slide scanners write a `.vsi` file plus a folder of `.ets` pixel files. OpenReadout returns each stored image (slide label, overview, scans with their pyramid levels) with names, calibration and channels. Focus data is listed, not decoded. Confidence (computed by the evidence rubric, `docs/assurance.md`): **high**; see *What is confirmed* below.

There is **no public specification**. The layout was derived from public datasets of several software generations by hex dump and by comparison with the values Bio-Formats 8.5.0 reports when run as a black box. See `docs/provenance/vsi.md`.

A cellSens dataset (cellSens Dimension, VS120/VS200 slide scanners) is a `.vsi` file plus a directory `_<stem>_/` next to it:

```
Image.vsi                         TIFF preview + tagged record tree (all metadata)
_Image_/stack1/frame_t.ets        pixels of stack 1 (e.g. the slide label)
_Image_/stack10000/frame_t.ets    overview; blob_*.ets + blob_*.meta = focus data (not decoded)
_Image_/stack10002/frame_t_0.ets  a scan (tiles, pyramid)
```

Stack directories are named `stack<id>`, the id matching a stack record in the `.vsi`. Each stack with a `frame_t*.ets` is one exposed image (in record-tree order, then any stack directory the tree does not name). A standalone `.ets` opens too (no names or calibration; non-XY dimensions exposed as T).

## ETS files

### SIS header (64 bytes)

| offset | size | our name | value |
| --- | --- | --- | --- |
| 0 | 4 | `SIS_MAGIC` | `SIS\0` |
| 4 | u32 | — | 64 (header size) |
| 8 | u32 | — | 3 |
| 12 | u32 | `coord_count` | coordinates per tile: X, Y, one per non-XY dimension, the pyramid level (4 in RGB slides, 6 in the fluorescence files) |
| 16 | u64 | `ets_offset` | 64 |
| 24 | u32 | `ets_size` | 228 |
| 32 | u64 | `table_offset` | tile table |
| 40 | u32 | `tile_count` | tile table entries |
| 48 | u64, u32 | — | an offset inside the tile data and a small count (0 or 3); meaning unknown |

### ETS header (at `ets_offset`)

| offset | size | our name | value |
| --- | --- | --- | --- |
| 0 | 4 | `ETS_MAGIC` | `ETS\0` |
| 4 | u32 | `version` | `0x00030006` (cellSens 3.x, VS200), `0x00030005` (cellSens 1.18), `0x00030003` (dotSlide 2.5, Stream 1.7) (reported as `format_version`) |
| 8 | u32 | `sample_type` | 2 → `uint8`, 4 → `uint16` (others: exit 6) |
| 12 | u32 | `samples_per_pixel` | 1, or 3 (RGB, interleaved) |
| 16 | u32 | `color_space` | 1 (gray) / 4 (RGB); meaning otherwise unknown |
| 20 | u32 | `compression` | `EtsCompression`: 0 `Raw` (little-endian samples), 2 `Jpeg` (baseline JFIF stream), 3 `Jpeg2000` (J2K codestream `FF4F FF51`) |
| 24 | u32 | `quality` | 90, 100 |
| 28, 32, 36 | u32 | `tile_width`, `tile_height`, `tile_depth` | 512 × 512 × 1 on slides; the whole plane in the fluorescence files |
| 108 | u32 | `background` | `0x00EEEEEE` / `0x00FFFFFF` (RGB), `0xFFFF` / `0x0FFF` (16-bit): value of tiles that are not stored |
| 184 | u32 + k × u32 | `sizes` | k, then X, Y and the sizes of the non-XY dimensions (e.g. `[1645, 1682, 1, 11, 2]`); k = 0 in version `0x00030003`: the image size is then only in the `.vsi` (record 2053, below) |

### Tile table (`Tile`)

`tile_count` entries of `4 + 4·coord_count + 16` bytes (`entry_size`): u32 (= `coord_count`), `coords` (`column`, `row`, the non-XY indices `extra`, the pyramid `level`), u64 `offset`, u32 `len`, u32 (a permutation of 0..N−1; unknown meaning). Column and row are **signed** 32-bit numbers: the Stream "MIA" stitch stores columns −27…4 and rows −5…3.

### Tile placement

Tile (column, row) of level 0 starts at image pixel (column × `tile_width` + o<sub>x</sub>, row × `tile_height` + o<sub>y</sub>), where (o<sub>x</sub>, o<sub>y</sub>) = `tile_origin` is record 2410 of the `.vsi` (below): (0, 0) in most files, (−42, −1) in a cellSens 1.18 slide, (13306, 2464) in a Stream stitch. At level L the offset is divided by 2^L and rounded to the nearest pixel, halves toward zero (Bio-Formats truncates; the block-averaged level 0 favours the nearest pixel where the two differ). Checked against the depositors' own full-resolution exports of both files (`crates/openreadout-corpus-tests/tests/vsi_exports.rs`). A standalone ETS (no `.vsi`) is placed from its smallest column and row. `images[].extra.tile_origin` reports a non-zero offset; `images[].extra.size_source` says where the level-0 size came from (`ets_header`, `vsi_layout`, or `tile_grid` for a standalone ETS without a size list, which may include background padding).

Whole slides store only tiles that hold tissue: missing tiles are filled with `background`. Pixel values of stitched planes match Bio-Formats' output within the manifest tolerance (JPEG decoders differ by up to 4 levels) including the filled areas.

### Pyramid

Level 0 size = `sizes[0..2]`, or the `.vsi` rectangle 2053 when the ETS has no size list. Level L = level L−1 halved (rounding up), clipped to the extent of the tiles stored at level L (their right and bottom edges after placement) (slides omit empty margins; Bio-Formats reports the same sizes: e.g. 158745 × 84465 → 78848 × 40960). Level L is downsampled 2^L times on both axes (`resolution_levels[].downsample_x/y`), also where it is clipped. Level L is downsampled 2^L times on both axes (`resolution_levels[].downsample_x/y`), also where it is clipped: the size ratio is then larger than the scale. `images[].pyramid_levels`, `extra.pyramid` (size and tile count per level), `info --view structure` rows `pyramid-level`; `planes --level N` / `read_plane_level` stitch any level. Planes over 4 GiB (full-resolution slides) are refused with exit 6 and a hint to read a coarser level.

## The `.vsi` file

A little-endian TIFF (`II*\0`) whose single IFD is a small JPEG preview (`PreviewInfo`: `width`, `height`, `compression`, `strip_offset`, `strip_len`, `tables` (`JPEGTables`), `make`, `model` — the camera in fluorescence files). The preview is exposed as attachment `preview` (a standalone JPEG: tables + strip). Right after the 8-byte TIFF header (`TREE_OFFSET` = 8) starts the **record tree**.

### Record tree (`TagTree`)

A record set ("volume"): 24-byte header `18 00 'I' 'S'` (`VOLUME_MAGIC`), u32 version (0x192 or 1), u32 offset of the first record relative to the set (0 = empty), u32 0, u32 record count, u32 0. A record (`TagRecord`): u32 `type_word`, u32 `tag`, u32 offset of the next record relative to the set (0 = last), u32 size; then:

| type bit | our name | meaning |
| --- | --- | --- |
| `0x08000000` | `FLAG_INDEXED` | a u32 `index` follows the 16-byte header (stack id, dimension number, element number, plane number) |
| `0x80000000` | `FLAG_VOLUME` | a nested record set follows (`TagValue::Volume`) |
| `0x40000000` | `FLAG_INLINE` | no payload; the size word is the value (`TagValue::Inline`) |
| `0x2000` | `FLAG_ARRAY` | the value is an array |
| otherwise | | `size` payload bytes (`TagValue::Data`) |

Low 12 bits = value kind: 0x1 bytes/UTF-8 text, 0x2 bytes, 0x4 u16, 0x5 i32, 0x6 u32, 0x7 i64, 0xA f64, 0xC boolean, 0xD UTF-16 text, 0x103 4 × i32, 0x104/0x10C/0x117 2 × f64, 0x10E 256 × RGB; arrays: 0x2000/0x2002 UTF-16 text, 0x2003 i32, 0x2007 i32 (4-byte elements although scalar kind 7 is 8 bytes: 12 bytes hold 3 values), 0x2008 i32 box. Value containers (tags ≥ `0x10000000`) hold a unit string (0x10000000, e.g. `10^-6m^1` = µm, `10^-9m^1` = nm, `10^-3s^1` = ms; `unit_factor`), the value (0x10000002) and a type word (0x10000003); `quantity`. Walking the next-links visits exactly the declared count in every set of every corpus file; `check` reports broken links, bad signatures and count mismatches (`bad_record_tree`). `vendor.records` is the whole tree as JSON with numeric keys `"<tag>"` / `"<tag>[<index>]"` (repeated keys become arrays; unknown kinds as `#hex`).

### Tags we normalize (`read_meta` → `VsiMeta`, `StackMeta`)

| path | our name | → |
| --- | --- | --- |
| 2000 | `TAG_FILE` | per-file record set |
| 2000/2001[id] | `TAG_STACK` | a stack; `id` = `stack<id>` directory |
| …/2003 | `TAG_DIM_SIZES` | `dim_sizes`: sizes of the non-XY dimensions (`[5, 18, 2]`; empty for RGB slides) |
| …/2034 | `TAG_BACKGROUND` | `background` bytes |
| …/2005 | `TAG_SETTINGS` | settings record set |
| …/2005/2030 | `TAG_NAME` | image `name` (`Label`, `Overview`, `20x_BF_01`, `488/640_50um_1x`) |
| …/2005/2019 + 2020 | `TAG_PIXEL_SIZE`, `TAG_PIXEL_UNIT` | `pixel_size_um` → `physical_size.x/y` |
| …/2005/2015 | `TAG_TIME` | `acquired_unix` (i64 Unix seconds) → `acquired_at` (UTC) |
| …/2005/2018 | `TAG_STAGE` | `stage_position_um` → `extra.stage_position_um` |
| …/2005/2043/*/120114 | `TAG_DEVICES`, `TAG_OBJECTIVE_SET` | the device set holding 120060 (`TAG_OBJ_MAGNIFICATION`), 120061 (`TAG_OBJ_NA`), 120063 (`TAG_OBJ_NAME`), 120062 (`TAG_OBJ_WORKING_DISTANCE`) → `objective` |
| …/2007[k] | `TAG_DIMENSION` | non-XY dimension k (`DimMeta`), elements 2008[i] (`TAG_DIM_ENTRY`) |
| …/2008[0]/2023 | `TAG_DIM_KIND` | `DimKind`: 1 `Z`, 2 `T`, 4 `Channel`; anything else `Other` (exposed as T, `check` warning) |
| …/2008[0]/2012, 2013 | `TAG_Z_START`, `TAG_Z_STEP` | `z_start_um`, `z_step_um` (absolute value → `physical_size.z`) |
| …/2008[i]/2021 | `TAG_CHANNEL_NAME` | channel `name` (`ChannelMeta`) |
| …/2008[i]/2417 | `TAG_EMISSION` | `emission_nm` |
| …/2008[i]/2474 | `TAG_EXCITATION` | `excitation_nm` (**inferred**, not corroborated) |
| …/2002[k] | `TAG_PLANE` | per-plane record (k = plane number, first non-XY dimension fastest) |
| …/2002[k]/2006 | `TAG_PLANE_VALUES` | per-plane values: 2017 (`TAG_PLANE_TIME`, ms) → `plane_times_ms` → `time_increment_s` and `frames`; 2014 (`TAG_PLANE_Z`, µm) → `plane_z_um` → `frames[].z_um` |
| …/2002[first]/2018 | `TAG_PLANE_LAYOUT` | layout set, only in the first plane record |
| …/2018/2053 | `TAG_IMAGE_RECT` | i32 `[x, y, width, height]` → `image_rect`: the level-0 size when the ETS header has none; must equal the ETS size otherwise (`check`: `dimension_mismatch`) |
| …/2018/20025 | `TAG_IMAGE_BOX` | i32 origin and size per dimension (`[0, 0, 0, 0, 0, 2304, 2304, 200, 1, 3]`); not used (2053 and the ETS say the same) |
| …/2018/2410 | `TAG_TILE_ORIGIN` | one signed i32 per dimension (type `0x2007`: 4-byte elements) → `tile_origin` (x, y) (*Tile placement*) |
| 2000/2004/2109/34, /35 | `TAG_FILE_INFO`, `TAG_SOFTWARE`, `TAG_SOFTWARE_VERSION` | `instrument.software` (`OLYMPUS VS200 ASW`), `software_version` |
| …/2005/2419 | `TAG_SETTING_NAME` | illumination setting name (`BF`, `Label Scan`) → `setting_name`: the channel name of stacks without a channel dimension |
| …/2002[first]/2037 | `TAG_PLANE_LAYOUT_ALT` | a second layout set (2053 and 20025, no 2410); the rectangle is read from it when 2018 is absent |
| …/2043/*/120130 | `TAG_DEVICE_TYPE` | device type code: `DEVICE_CAMERA` (0), `DEVICE_FRAME` (40500, the microscope stand); also seen: 20000 nosepiece, 20001 camera adapter, 20002 turret/condenser, 20004 aperture stop, 20006 polarizer, 20007 magnification changer, 20009 port changer, 20050 stage insert |
| …/2043/*/120116, 120132, 120133 | `TAG_DEVICE_NAME`, `TAG_DEVICE_MODEL`, `TAG_DEVICE_MAKER` | device name, model, manufacturer → `camera` (camera name) → `instrument.detector`; `microscope` (frame model, else name) → `instrument.model` |
| camera …/120114/100002 | `TAG_CAM_EXPOSURE_US` | exposure in µs → `exposure_ms`; channel entries 2008[i] carry their own device set → per-channel `exposure_ms` → `images[].channels[].exposure_ms` |
| objective …/120114/120079 | `TAG_OBJ_REFRACTIVE_INDEX` | immersion refractive index → `images[].extra.objective_refractive_index` |

Non-XY dimensions map to C, Z, T by kind; plane numbers run first dimension fastest (`[T, Z, C]` = `[5, 18, 2]` in the time-lapse test file, where every one of the 180 planes matches Bio-Formats). RGB slides are `samples_per_pixel = 3`, `size_c = 1`.

### Stacks without pixels

Focus maps, focus points and sample masks appear as stack records without an ETS directory; `info` notes them, `info --view structure` lists every stack (`kind: stack`, `has_pixels`). `blob_*.ets` files and extra `frame_t*.ets` files in a stack directory are listed (`kind: file`) and not read.

## What is confirmed

| item | how |
| --- | --- |
| ETS header fields, tile table, raw/JPEG/JPEG 2000 tiles, dimension order of fluorescence stacks | every plane of 4 fluorescence datasets equals Bio-Formats (raw bit-exact; JPEG 2000 within 3) |
| Pyramid level sizes, tile placement, background fill | 30 pyramid-level planes of two VS200 slides within 4 of Bio-Formats (JPEG); 17 level planes of a dotSlide 2.5 slide and overview and a cellSens 1.18 slide within 4 |
| Image size from the `.vsi` (ETS `0x00030003`) | dotSlide 2.5 slide 31740 × 20970 and overview 7685 × 16421, equal to Bio-Formats; 2053 equals the ETS size list in all 18 stacks that have one |
| Signed tile indices, tile-grid offset 2410 | Stream 1.7 stitch: level 0 against the depositor's JPEG export (r = 0.971, best alignment at zero shift; Bio-Formats 8.5.0 misplaces this file); cellSens 1.18 slide: level 0 bit-identical to Bio-Formats and best-aligned with the depositor's PNG export |
| 8-bit grey JPEG tiles, time-lapse [T, Z, C] order | cellSens 3.2 time-lapse: 64 of 600 planes within 1 of Bio-Formats |
| Stack names, pixel sizes, Z step, channel names and emission, acquisition time, objective | equal to Bio-Formats' OME-XML for all corpus files |
| Dimension kinds 1, 2, 4 | the Z-stack, time-lapse and channel dimensions of the corpus |
| Not confirmed | other kind codes, other sample types/compressions, tag 2474 as excitation, 16-bit JPEG tiles |

## Integrity checks (`check`)

Record tree (`bad_record_tree`, `no_stacks`), missing `_<stem>_` directory (`missing_ets_directory`), stack without ETS (`missing_ets`), unreadable ETS (`bad_ets`), several ETS files (`unexpected_ets_files`), unnamed stack directory (`unnamed_stack`), tile table or tile data past the end of the file (`truncated`), tiles outside the image or pyramid (`tile_out_of_range`), planes without tiles (`missing_planes`), `.vsi`/ETS size disagreement (`dimension_mismatch`), unknown dimension kind (`unknown_dimension_kind`), unknown sample type (`unsupported_sample_type`), and one decoded tile per image (`bad_tile`, `unsupported_tile`). Any error → exit 4.

## Vocabulary (every public identifier in `openreadout-vsi` must appear here)

| identifier | meaning |
| --- | --- |
| `tile_bytes`, `decode_tile_bytes` | a tile's stored bytes, and their decoding apart from the file (tiles of a region are decoded in parallel) |
| `VsiReader`, `VsiDataset`, `FORMAT_ID`, `open` | reader and opened dataset |
| `SIS_MAGIC`, `ETS_MAGIC`, `looks_like_ets`, `EtsFile`, `path`, `file_len`, `header`, `tiles`, `table_problem`, `levels`, `read_tile_bytes`, `decode_tile` | ETS file |
| `EtsHeader`, `coord_count`, `ets_offset`, `ets_size`, `table_offset`, `tile_count`, `version`, `sample_type`, `samples_per_pixel`, `color_space`, `compression`, `quality`, `tile_width`, `tile_height`, `tile_depth`, `background`, `sizes`, `pixel_type`, `extra_dims`, `entry_size` | ETS headers |
| `EtsCompression` { `Raw`, `Jpeg`, `Jpeg2000`, `Other` }, `from_code`, `name` | tile compression |
| `Tile`, `coords`, `offset`, `len`, `column`, `row`, `level`, `extra` | tile table entry |
| `TREE_OFFSET`, `VOLUME_MAGIC`, `FLAG_INDEXED`, `FLAG_VOLUME`, `FLAG_INLINE`, `FLAG_ARRAY` | record-tree constants |
| `TagTree`, `data`, `root`, `problems`, `record_count`, `parse`, `bytes`, `text`, `number`, `int64`, `ints`, `floats`, `quantity`, `to_json`, `find` | the record tree and value accessors |
| `TagRecord`, `tag`, `index`, `type_word`, `value`, `kind`, `is_array`, `children` | one record |
| `TagValue` { `Volume`, `Inline`, `Data` } | a record's value |
| `TAG_FILE`, `TAG_STACK`, `TAG_DIM_SIZES`, `TAG_BACKGROUND`, `TAG_SETTINGS`, `TAG_NAME`, `TAG_PIXEL_SIZE`, `TAG_PIXEL_UNIT`, `TAG_TIME`, `TAG_STAGE`, `TAG_DEVICES`, `TAG_DIMENSION`, `TAG_DIM_ENTRY`, `TAG_DIM_KIND`, `TAG_Z_START`, `TAG_Z_STEP`, `TAG_CHANNEL_NAME`, `TAG_EMISSION`, `TAG_EXCITATION`, `TAG_PLANE`, `TAG_FILE_INFO`, `TAG_SOFTWARE`, `TAG_SOFTWARE_VERSION`, `TAG_OBJECTIVE_SET`, `TAG_OBJ_MAGNIFICATION`, `TAG_OBJ_NA`, `TAG_OBJ_WORKING_DISTANCE`, `TAG_OBJ_NAME`, `TAG_PLANE_LAYOUT`, `TAG_PLANE_LAYOUT_ALT`, `TAG_IMAGE_RECT`, `TAG_IMAGE_BOX`, `TAG_TILE_ORIGIN`, `TAG_PLANE_VALUES`, `TAG_PLANE_Z`, `TAG_PLANE_TIME`, `TAG_SETTING_NAME`, `TAG_OBJ_REFRACTIVE_INDEX`, `TAG_DEVICE_TYPE`, `TAG_DEVICE_NAME`, `TAG_DEVICE_MODEL`, `TAG_DEVICE_MAKER`, `TAG_CAM_EXPOSURE_US`, `DEVICE_CAMERA`, `DEVICE_FRAME` | tag numbers and device type codes we normalize (tables above) |
| `VsiMeta`, `software`, `software_version`, `stacks`, `read_meta`, `unit_factor` | normalized file metadata |
| `StackMeta`, `id`, `dim_sizes`, `dims`, `pixel_size_um`, `stage_position_um`, `acquired_unix`, `objective`, `plane_times_ms`, `plane_z_um`, `image_rect`, `tile_origin`, `camera`, `exposure_ms`, `microscope`, `setting_name` | one stack |
| `DimMeta`, `z_start_um`, `z_step_um`, `channels`, `DimKind` { `Z`, `T`, `Channel`, `Other`, `Unknown` } | a non-XY dimension |
| `ChannelMeta`, `emission_nm`, `excitation_nm`, `exposure_ms` | a channel |
| `ObjectiveMeta`, `magnification`, `numerical_aperture`, `working_distance_um`, `refractive_index` | the objective |
| `PreviewInfo`, `ifd_offset`, `width`, `height`, `strip_offset`, `strip_len`, `tables`, `make`, `model`, `looks_like_vsi`, `preview_info`, `preview_jpeg` | the `.vsi`'s TIFF preview |
