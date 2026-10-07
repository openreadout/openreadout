# Zeiss CZI

ZEISS ZEN writes `.czi` files for widefield, confocal, light-sheet and slide-scanner images. OpenReadout returns one image per scene with its channels, Z planes, time points and tiles, the pyramid levels of slide scans, the metadata, and the attachments.

Derived from corpus files by hex dump and a Python segment walker, cross-checked against the public documentation and source of `czifile` (BSD-3-Clause) and the ZEISS `libCZI` documentation pages (concepts only: resolution protocol, valid-pixel mask). See `docs/provenance/czi.md`.

CZI ("Carl Zeiss Image") is a segmented container. Every segment starts with a 32-byte header; the file header points at the subblock directory, the XML metadata, and the attachment directory. Pixel data lives in subblocks, each a rectangular tile of one plane at one set of dimension coordinates, optionally compressed.

## Byte layout (all integers little-endian)

### Segment header (every segment, 32 bytes)

| offset | size | field | meaning |
| --- | --- | --- | --- |
| 0 | 16 | `segment_id` | ASCII, zero-padded: `ZISRAWFILE`, `ZISRAWDIRECTORY`, `ZISRAWSUBBLOCK`, `ZISRAWMETADATA`, `ZISRAWATTACH`, `ZISRAWATTDIR`, `DELETED` |
| 16 | u64 | `allocated_size` | bytes reserved for the payload; next segment starts at `offset + 32 + allocated_size` |
| 24 | u64 | `used_size` | bytes actually used (≤ allocated) |

`DELETED` segments are tombstones and are skipped. The file starts with a `ZISRAWFILE` segment whose allocated size is 512.

### File header payload (`ZISRAWFILE`)

| offset | size | field | observed |
| --- | --- | --- | --- |
| 0 | u32 | `version_major` | 1 |
| 4 | u32 | `version_minor` | 0 |
| 8 | 8 | reserved | |
| 16 | 16 | `primary_file_guid` | equal to `file_guid` in single-file documents |
| 32 | 16 | `file_guid` | |
| 48 | u32 | `file_part` | 0 for single-file documents and for the master of a multi-file document; k for its following part k (see *Multi-file documents*) |
| 52 | u64 | `directory_position` | byte offset of the `ZISRAWDIRECTORY` segment (0 if absent) |
| 60 | u64 | `metadata_position` | byte offset of the `ZISRAWMETADATA` segment |
| 68 | u32 | `update_pending` | non-zero means the writer did not finish |
| 72 | u64 | `attachment_directory_position` | byte offset of `ZISRAWATTDIR` (0 if none) |

### Metadata payload (`ZISRAWMETADATA`)

| offset | size | field |
| --- | --- | --- |
| 0 | u32 | `xml_size` |
| 4 | u32 | `attachment_size` (always 0 in the corpus) |
| 8 | 248 | reserved |
| 256 | `xml_size` | UTF-8 XML, root `ImageDocument` |

### Subblock directory payload (`ZISRAWDIRECTORY`)

| offset | size | field |
| --- | --- | --- |
| 0 | u32 | `entry_count` |
| 4 | 124 | reserved |
| 128 | var | `entry_count` × directory entry (below) |

### Directory entry (schema `DV`; also the first thing in a subblock header)

| offset | size | field | meaning |
| --- | --- | --- | --- |
| 0 | 2 | `schema` | ASCII `DV` |
| 2 | u32 | `pixel_type` | see table |
| 6 | u64 | `file_position` | byte offset of the `ZISRAWSUBBLOCK` segment |
| 14 | u32 | `file_part` | |
| 18 | u32 | `compression` | see table |
| 22 | u8 | `pyramid_type` | 0 none, 1 single, 2 multi (subblock belongs to a downsampled level) |
| 23 | 5 | reserved | |
| 28 | u32 | `dimension_count` | |
| 32 | 20 × n | `dimension_entry` | one per dimension |

Entry length = `32 + 20 * dimension_count`.

### Dimension entry (20 bytes)

| offset | size | field | meaning |
| --- | --- | --- | --- |
| 0 | 4 | `dimension` | ASCII, zero-padded: `X`, `Y`, `Z`, `C`, `T`, `R`, `S`, `I`, `H`, `V`, `B`, `M` |
| 4 | i32 | `start` | coordinate of the first element (pixels for X/Y; index otherwise) |
| 8 | u32 | `size` | extent in the full-resolution coordinate system |
| 12 | f32 | `start_coordinate` | physical start (unused by us) |
| 16 | u32 | `stored_size` | extent actually stored; `stored_size < size` means a pyramid level |

Level-0 (full resolution) subblocks have `stored_size == size` for X and Y and `pyramid_type == 0`.

### Subblock payload (`ZISRAWSUBBLOCK`)

| offset | size | field |
| --- | --- | --- |
| 0 | u32 | `metadata_size` |
| 4 | u32 | `attachment_size` |
| 8 | u64 | `data_size` |
| 16 | var | directory entry (as above) |
| `max(256, 16 + entry_len)` | `metadata_size` | UTF-8 XML `<METADATA>` (per-subblock tags such as `AcquisitionTime`, stage position) |
| + | `data_size` | pixel data, compressed per `compression` |
| + | `attachment_size` | optional attachment; may carry a valid-pixel mask (below) |

Invariant checked by `check`: `used_size == header_len + metadata_size + data_size + attachment_size`.

### Attachment directory (`ZISRAWATTDIR`) and attachments (`ZISRAWATTACH`)

`ZISRAWATTDIR`: u32 `entry_count`, reserved to 256, then 128-byte entries. `ZISRAWATTACH`: u32 `data_size`, 12 reserved, one 128-byte entry, padding to 256, then data.

Attachment entry (128 bytes): `schema` `A1` (2), reserved (10), u64 `file_position`, u32 `file_part`, 16-byte `content_guid`, 8-byte ASCII `content_file_type` (`JPG`, `CZTIMS`, `CZEVL`, `CZI`, `Zip-Comp`, …), 80-byte `name` (`Thumbnail`, `TimeStamps`, `EventList`, `Label`, `SlidePreview`, `Profile`, `InteractiveMeasurement`, …). The payload of an attachment starts `ATTACHMENT_DATA_OFFSET` = 32 + 256 bytes after its segment start; its length is the `ZISRAWATTACH` `data_size` (read at open, one small read per attachment, so `info --view structure` can show sizes).

Payloads we interpret or expose (corpus evidence in `docs/provenance/czi.md`):

| `content_file_type` | typical `name` | payload | what we do |
| --- | --- | --- | --- |
| `CZTIMS` | `TimeStamps` | u32 `size` (unreliable across writers), u32 `count`, `count` × f64 seconds | `parse_time_stamps` → `images[].extra.time_stamps_s` (per T, relative clock) and `planes[].extra.time_stamp_s` |
| `CZEVL` | `EventList` | u32 `size`, u32 `count`, then records: i32 `entry_size`, f64 `time_s`, i32 `event_code`, i32 text size, text (NUL-terminated) | `parse_event_list` → `images[].extra.events` (`kind`: `marker`, `interval_change`, `bleach_start`, `bleach_stop`, `trigger`) |
| `JPG` | `Thumbnail` | a JPEG file | listed; `extract` writes it (`.jpg`) |
| `CZI` | `Label`, `SlidePreview` | a complete embedded CZI file | listed; `extract` writes it (`.czi`), which every command then reads |
| `Zip-Comp` | `Profile` | gzip-compressed XML | listed; `extract` writes it (`.gz`) |
| anything else | | opaque | listed; `extract` writes it (`.bin`, `.xml` for `CZEXP`/`CZHWS`) |

`extension_for` maps content types to file extensions. Payloads larger than `MAX_INTERPRETED_ATTACHMENT` (64 MiB) are never interpreted, only extracted.

### Valid-pixel mask (subblock attachment)

From the libCZI documentation page *valid-pixel mask* (concept page, not source) and czifile's `mask()` (BSD-3): a subblock's attachment may be a chunk container, a sequence of (16-byte GUID, i32 size, payload). The chunk whose GUID is `MASK_CHUNK_GUID` (`{CBE3EA67-5BFC-492B-A16A-ECE378031448}`, stored `67 EA E3 CB FC 5B 2B 49 A1 6A EC E3 78 03 14 48`) holds:

| offset | size | field |
| --- | --- | --- |
| 0 | u32 | `mask_width` |
| 4 | u32 | `mask_height` |
| 8 | u32 | representation (must be 0) |
| 12 | u32 | `stride` (bytes per row) |
| 16 | `stride × mask_height` | `bits`, most significant bit = leftmost pixel; 1 = valid |

`parse_valid_mask` also accepts the variant czifile handles, where the GUID follows 16 leading bytes. When compositing a plane, only valid pixels of a masked subblock are pasted (`ValidMask::is_valid`); a mask that is all valid (`all_valid`) or malformed is ignored. In the corpus, masks occur on pyramid-level subblocks of tiled scans (`aics-*`, `openslide-zeiss-5-*`, `zenodo7015307-*` mosaics), where they stop a tile's padding from covering its neighbour.

## Pixel types (`pixel_type`)

| id | our name | samples | sample type | notes |
| --- | --- | --- | --- | --- |
| 0 | `gray8` | 1 | uint8 | |
| 1 | `gray16` | 1 | uint16 | `ComponentBitCount` in XML may say 12 or 14 |
| 2 | `gray32_float` | 1 | float | |
| 3 | `bgr24` | 3 | uint8 | stored B,G,R; we return R,G,B |
| 4 | `bgr48` | 3 | uint16 | stored B,G,R |
| 8 | `bgr96_float` | 3 | float | |
| 9 | `bgra32` | 4 | uint8 | alpha dropped on read |
| 10 | `gray64_complex_float` | 2 | float | unsupported |
| 11 | `bgr192_complex_float` | 6 | float | unsupported |
| 12 | `gray32` | 1 | int32 | signed (czifile maps it to `<i4`); no corpus sample |
| 13 | `gray64` | 1 | double | |

The directory entry's `pixel_type` is authoritative for the subblock; the XML `PixelType` can disagree (the OpenSlide previews say `Bgr24` in XML but store `Bgr48`).

## Compression (`compression`)

| id | our name | payload | status |
| --- | --- | --- | --- |
| 0 | `uncompressed` | raw samples, row-major, no padding | supported |
| 1 | `jpeg` | a complete JPEG stream (SOI … EOI); 3-sample streams decode to R,G,B (no B/R swap, as czifile) | supported: 8-bit baseline/progressive gray and YCbCr and lossless (SOF3) up to 16 bit via `jpeg-decoder`; 12-bit sequential DCT (SOF1) by our own decoder in `openreadout-codecs` (gray, or colour without chroma subsampling); 12-bit progressive or subsampled streams are exit 6 |
| 2 | `lzw` | LZW (TIFF flavour) | supported |
| 3 | `jpeg_lossless` | undocumented; no public sample | not supported (exit 6) |
| 4 | `jpeg_xr` | JPEG XR codestream, starts `49 49 BC 01` | supported via `openreadout-jpegxr` (our safe port of jxrlib's decoder; bit-exact with jxrlib/imagecodecs on every corpus subblock); alpha planes, CMYK and packed pixel formats are exit 6 |
| 5 | `zstd0` | plain zstd frame, starts `28 B5 2F FD` | supported |
| 6 | `zstd1` | small header then a zstd frame | supported |
| 7 | `chunked` | header entries (varint id, varint length, payload; id 0 ends them), then the compressed chunks back to back: see "Chunked compression" below | supported: zstd and LZ4 chunks; the HiLo split only when the subblock is one chunk |
| 100–999 | `camera_raw(N)` | raw data of a particular camera | not supported (exit 6) |
| ≥ 1000 | `system_raw(N)` | raw data of a particular system | not supported (exit 6) |

JPEG decoding differs from libjpeg-turbo (czifile's decoder) by rounding in the inverse DCT and chroma upsampling: on the synthetic fixtures the largest per-sample difference is 1 grey level for 8-bit gray and 3 for 4:2:0 colour. Lossless JPEG is bit-exact. The corpus harness therefore compares lossy JPEG fixtures against czifile's decoded planes with a tolerance (`pixel_tolerance` in `corpus/manifest.toml`).

### Chunked compression (id 7)

From libCZI's public documentation page on chunked compression, checked against streams written by imagecodecs (BSD-3-Clause; czifile 2026.8.16 decodes id 7 with it). Numbers are varints: little-endian groups of seven bits, the high bit set on every byte but the last. The header is a list of entries `id, length, payload`, ended by id 0:

| id | payload | |
| --- | --- | --- |
| 1 | compressed size of each chunk (varints) | required |
| 2 | codec, one byte: 0 zstd, 1 LZ4 (raw block) | zstd when absent |
| 3 | decompressed chunk sizes (varints): `[C]` every chunk C bytes; `[C, L]` every chunk but the last C bytes, the last L; or one size per chunk | required |
| 4 | preprocessing, one byte: 0 none, 1 HiLo | none when absent |

The chunks follow the header in order and are decompressed one by one. HiLo (all low bytes of the 16-bit samples, then all high bytes) is undone only when the subblock is a single chunk: the documentation says the decoder reverses it after decompressing the chunks, while imagecodecs (czifile's decoder) splits each chunk on its own, and the two readings differ when there are several chunks. HiLo over several chunks, other ids, other size lists, codecs and preprocessing values are refused (exit 6). The decoded chunks, joined, must fill the subblock's geometry exactly.

### Resolution protocol (decoded bitmap vs directory entry)

After decoding a JPEG or JPEG XR subblock, `conform` brings the bitmap to what the directory entry declares (libCZI documentation page *resolution protocol*: the directory entry is authoritative):

- **size**: crop or zero-pad at the top-left origin to `stored_size` X × Y;
- **pixel type**: convert samples to the declared type. 16 → 8 bit keeps the high byte (`>> 8`); 8 → 16 bit zero-extends; gray → RGB replicates; RGB → gray is `(R + G + B + 1) / 3` (integers) or the mean (floats); RGBA → RGB drops alpha; floats clip to the integer range. These are czifile's pixel-type conversions; pylibCZIrw (run as an oracle) agrees on 48 → 24 bit (`>> 8`) and 8 → 16 bit (zero-extend), and czifile itself, which casts instead of converting, returns the low byte for 48 → 24 bit;
- the result is a `Conformed` bitmap plus the list of `adjustments` made. `check` decodes one JPEG/JPEG XR subblock per (compression, pixel type) and reports a non-empty list as a `resolution_protocol` warning.

The OpenSlide `Zeiss-5-*` slide previews are the corpus case of XML/directory disagreement: the XML says `Bgr24`, the directory says `Bgr48` and the JPEG XR stream is 48-bit, so we return uint16 RGB, as czifile does; `images[].extra.xml_pixel_type` records the XML's claim when it differs.

`zstd1` header (derived from `openslide-zeiss-5-slidepreview-zstd1-hilo`): byte 0 = `header_size` (observed 3), byte 1 = `chunk_kind` (observed 1), byte 2 = `flags` where bit 0 = `hilo` byte shuffle. After decompression with `hilo` set, the buffer holds all low bytes of the 16-bit samples first, then all high bytes; `unshuffle_hilo` re-interleaves them.

## XML model (`ImageDocument/Metadata`, the subset we normalize)

```
Information/Image/{SizeX,SizeY,SizeZ,SizeC,SizeT,SizeS,SizeM,SizeH,SizeB,PixelType,ComponentBitCount,AcquisitionDateAndTime}
Information/Image/Dimensions/Channels/Channel[@Id,@Name]/{ExcitationWavelength,EmissionWavelength,Color,Fluor,ExposureTime,AcquisitionMode,IlluminationType,ContrastMethod}
Information/Image/Dimensions/Channels/Channel/DetectionWavelength/Ranges      ("415-735" → emission_range_nm)
  (AcquisitionMode + ContrastMethod, OME enumeration tokens → one acquisition_mode label: WideField + Fluorescence → "Widefield Fluorescence")
Information/Image/Dimensions/S/Scenes/Scene[@Index,@Name]/{CenterPosition,ContourSize,Shape[@Name,@Id]/{RowIndex,ColumnIndex}}
Information/Image/Dimensions/T/StartTime ; Z/StartPosition
Information/Instrument/Objectives/Objective[@Name]/{LensNA,NominalMagnification,Immersion,WorkingDistance}
Information/Instrument/Microscopes/Microscope[@Name]/{Type,System}   (System = the model when @Name is absent: LSM files)
Information/Document/UserName ; Information/User/DisplayName          (the user account → extra.experimenter.user_name)
Information/Application/{Name,Version}
Scaling/Items/Distance[@Id=X|Y|Z]/Value          (metres per pixel)
DisplaySetting/Channels/Channel[@Id,@Name]/{Color,DyeName,DyeMaxEmission,DyeMaxExcitation,IlluminationType}   (matched to image channels by Id)
Experiment[@Version]/ExperimentBlocks/AcquisitionBlock/SubDimensionSetups/*Setup[@IsActivated]   (nested; TimeSeriesSetup/{Duration/Cycles,Interval/TimeSpan/{Value,DefaultUnitFormat}})
```

Per-subblock `<METADATA>` (read only by `info --view full`, one small read per plane):

```
METADATA/Tags/{AcquisitionTime, StageXPosition, StageYPosition, FocusPosition, DetectorState (escaped XML with ExposureTime ns)}
```

Colours are `#AARRGGBB`; we drop the alpha byte. Where the image channel lacks a colour, name, dye or wavelength, the `DisplaySetting` channel with the same `Id` fills it (by position when no ids match).

Normalized extras per image (`images[].extra`): `experimenter` (`{user_name}`: `Document/UserName`, else `User/DisplayName`), `scene` (`index`, `center_position_um`, `contour_size_um`, `well` {`name`, `id`, `row_index`, `column_index`}), `experiment` (`version`, `acquisition_blocks`, `active_setups`, `time_series_cycles`, `time_series_interval_s`), `time_stamps_s`, `events`, `pyramid`. Per-frame records in `info --view full` (`images[].extra.frames`, the core's `Dataset::frames`; one record per plane, ordered t, then z, then c; first 100 per image unless `--max-frames -1`): `frame` (running index), `c`, `z`, `t`, `acquired_at` (subblock `AcquisitionTime`), `time_ms` (milliseconds since the image's earliest record, from those times when every record has one, else from the time stamps), `stage_x_um`/`stage_y_um`/`stage_z_um` (subblock `StageXPosition`/`StageYPosition`/`FocusPosition`), `exposure_ms` (subblock `DetectorState`, else the channel's `ExposureTime`), `time_stamp_s` (the `TimeStamps` value for that T). Field names follow the ND2 frame records.

## Image geometry

- One exposed image per scene (`S` index); files without `S` have one image.
- Full-resolution planes are the union of level-0 subblocks of that scene. The image's X/Y size is the bounding box of those subblocks (`max(start + size) - min(start)`); mosaics (`M`) are stitched by placing each tile at `start - min_start`.
- `size_z/c/t` = `max(start) + 1` over level-0 subblocks of the scene (indices start at 0 per scene in the corpus; when a scene's minimum start is non-zero we subtract it).
- Pyramid levels (inferred from the corpus; geometry checked against pylibCZIrw run as a black box; see `docs/provenance/czi.md`): a level is one downsampling factor per axis for a whole scene. Subblocks with `pyramid_type != 0` belong to a level; ZEN builds each level from the one below, so a subblock's own ratio `size / stored_size` is the level's factor distorted by the rounding of its stored size (`128.158` for a 1/128 tile of `zenodo10577621-Young-mouse`). A subblock is consistent with every factor in `[size / (stored + 2), size / (stored - 2)]` per axis (`level_scale_range`, `STORED_SIZE_SLACK`). Levels are formed from the largest subblocks first: a subblock joins the level whose factors lie inside its interval (the nearest, when several do) or starts one whose factor is the power of two inside the interval, else the nearest integer, else the ratio (`snap_factor`). Levels are ordered finest first: level 0 is full resolution. Level k of a scene whose level-0 rectangle is `x0, y0, w, h` is `floor(w / f) × floor(h / f)` pixels, and a subblock starting at level-0 `start` is placed at `floor((start - x0) / f)` (`level_place`), stored pixels unscaled, clipped to the level. That is the grid pylibCZIrw returns at `zoom = 1/f` over the scene rectangle, so `level pixel × f + origin` is a level-0 coordinate; czifile instead sizes a level as the bounding box of its subblocks and keys levels by exact ratios. `pyramid_levels` = 1 + number of levels; `extra.pyramid` lists each level's size and factors. `read_plane_level` composites a level exactly like level 0 (tile order by M index, valid-pixel masks honoured). Where pylibCZIrw differs: it resamples a subblock whose ratio is not exactly f to its logical footprint, and where a subblock's grid position is fractional it leaves the subblock's first row or column to its neighbour or the background (one pixel along tile seams).
- Canvas no subblock covers (the gaps of a mosaic inside its bounding box, at level 0 and at every pyramid level) is 0 in every sample, as in czifile and pylibCZIrw (default background). Pyramid subblocks are returned as stored: in the Axioscan RAC scans (`zenodo10577621-Intestine-3color-RAC`, `-Kidney-RAC-3color`, 12-bit data in uint16) ZEN's pyramid tiles extend past the acquired tiles and hold 65535 there, and 65535 averaged with data along the scanned area's edge (values from 4096 up), with no valid-pixel mask. A level therefore shows 65535 where level 0 shows 0; czifile and pylibCZIrw (`read(zoom=1/f)`, black box) return the same values, so the reader does not rewrite them. Consumers that know the recorded depth (`images[].extra.component_bit_count`) can tell the fill from data: `preview` draws samples above it as background when most of them are 65535 (see `book/src/reference/commands/index.md`; checks in `docs/provenance/czi.md`).
- Super-resolved renderings: full-resolution subblocks can store more pixels than their logical extent (an ELYRA PALM rendering: 1206 × 1206 stored for 512 × 512 logical; 2560 for 256). A scene's full-resolution subblocks are grouped by their stored-to-logical ratio on X (`stored_ratio`); a scene with several ratios becomes one image per ratio (its channels only; the rendering's name gets ` (rendered at <ratio>x)`), subblocks are placed at `round(start × ratio)` and the pixel size is the logical one divided by the ratio (`images[].extra.rendering_scale`). Nothing is resampled: the widefield channel keeps its 512 grid, the rendering its own. Validated against czifile's `asarray(storedsize=True, C=c)` on the three PALM files of Zenodo 10577621.
- Dimensions `H`, `I`, `R`, `V`, `B`: a subblock's coordinates on the dimensions other than X, Y, Z, C, T, S and M, sorted by letter, are its `extra_key`. A scene's full-resolution subblocks are grouped by that key (and by stored-to-logical ratio); when the coordinates vary within the scene (`extra_varying`), each combination is its own image, in key order (letters alphabetical, the last varying fastest), named `<scene> <D>=<k>` (`Scene <s> <D>=<k>` for an unnamed scene of a multi-scene file, `<D>=<k>` alone for a single unnamed scene), with `images[].extra.dimension_index` (`{"H": 3}`) and its pyramid subblocks those with the same coordinates. A plane reads only the subblocks at the image's coordinates on every such dimension (`extra_index`; a dimension constant across the scene is matched at its constant value). `extra.other_dimensions` still lists each varying dimension's extent across the scene. Validated on an ELYRA 7 SIM raw acquisition (H = 13 phases × C 3 × T 14, `zenodo16419509-Rizzollo-Example1-SIM-raw`) against czifile's `asxarray` indexed on H. No public file with I, R, V or B varying was found (light-sheet Z.1 and Lightsheet 7 files surveyed carry them at extent 1).
- Pyramid factors other than 2: ZEN 3.7 slide scans can build pyramids by 3 (subblock ratios 3, 9, 27, 81; `zenodo12509122-PSR-LXR-KO-WT-1054`). `snap_factor` picks the integer inside each subblock's interval, so levels are 1/3, 1/9, 1/27, 1/81 with `floor(w / f)` sizes, equal to pylibCZIrw's `read(zoom=1/3^k)` over the scene rectangle (black box).

## Multi-file documents (inferred)

No public multi-file CZI exists in the corpus; this section is inferred from the header fields and tested only against synthetic fixtures (`synthetic-multifile*`, made by `oracle/make_czi_fixtures.py`). Confidence: `inferred`.

- The master (`file_part` 0) holds the XML, the subblock directory and the attachment directory. Directory and attachment entries whose `file_part` is k > 0 point into following part k.
- Part k is looked for next to the master as `<master stem> (k).<ext>` (`part_path`; e.g. `slide.czi` → `slide (1).czi`), then as `<master stem>(k).<ext>` (the spelling ZEN gives separately saved Lightsheet tiles, e.g. Zenodo 19047136 `Image_1_…964(1).czi`; those files are independent documents with `file_part` 0, so they never trigger part discovery). It must start with a `ZISRAWFILE` segment whose `file_part` is k and whose `primary_file_guid` equals the master's `primary_file_guid`; otherwise `check` reports `bad_part`.
- Parts referenced by any entry are probed at open (`FilePart`: `path`, `present`, `file_len`, `part_problem`); `info --view structure` lists them as `file-part` rows, `info` adds a note, `check` reports `missing_part` (error, exit 4) for each absent one, and reading a plane stored in a missing part is an `io` "not found" error (exit 5) naming the expected part file, while the other planes stay readable.
- Opening a following part directly is exit 6 with a hint naming the master (`master_path`).

## Vocabulary (every public identifier in `openreadout-czi` must appear here)

| identifier | meaning |
| --- | --- |
| `CziReader`, `CziFile`, `FORMAT_ID`, `FILE_MAGIC`, `SEGMENT_HEADER_LEN` | entry points and constants |
| `SegmentHeader`, `segment_id`, `allocated_size`, `used_size`, `SegmentId` { `File`, `Directory`, `SubBlock`, `Metadata`, `Attachment`, `AttachmentDirectory`, `Deleted`, `Unknown` } | segment chain |
| `FileHeader`, `version_major`, `version_minor`, `primary_file_guid`, `file_guid`, `file_part`, `directory_position`, `metadata_position`, `update_pending`, `attachment_directory_position` | file header |
| `DirectoryEntry`, `schema`, `pixel_type`, `file_position`, `compression`, `pyramid_type`, `dimension_count`, `dimensions` | subblock directory |
| `DimensionEntry`, `dimension`, `start`, `size`, `start_coordinate`, `stored_size` | per-dimension extent |
| `SubBlockHeader`, `metadata_size`, `attachment_size`, `data_size`, `header_len`, `entry` | subblock header |
| `AttachmentEntry`, `content_guid`, `content_file_type`, `name` | attachment directory |
| `PixelTypeId` { `Gray8`, `Gray16`, `Gray32Float`, `Bgr24`, `Bgr48`, `Bgr96Float`, `Bgra32`, `Gray64ComplexFloat`, `Bgr192ComplexFloat`, `Gray32`, `Gray64`, `Unknown` }, `sample_type`, `samples_per_pixel`, `bytes_per_pixel` | pixel types |
| `CompressionId` { `Uncompressed`, `Jpeg`, `Lzw`, `JpegLossless`, `JpegXr`, `Zstd0`, `Zstd1`, `Chunked`, `CameraRaw`, `SystemRaw`, `Unknown` } | compression ids |
| `detector_id`, `detectors` | `DetectorSettings/Detector/@Id` of a channel; `Information/Instrument/Detectors` id → label (`Manufacturer/Model`, else `@Name`, else `Type`), the image's `instrument.detector` (distinct labels of its channels joined by ` + `) |
| `Scene`, `scene_index`, `scene_name`, `level0`, `bounds`, `min_x`, `min_y`, `width`, `height`, `size_z`, `size_c`, `size_t`, `pyramid_levels`, `other_dims`, `extra_index`, `extra_varying`, `extra_key`, `ExtraKey`, `absent_channels` | per-scene geometry; `absent_channels`: channels with no subblock at the image's H/I/R/V/B coordinates (read as 0); `extra_index`/`extra_varying`: the image's H/I/R/V/B coordinates and which of them vary within the scene |
| `Bounds`, `ImageXml`, `ChannelXml`, `ObjectiveXml`, `ScalingXml`, `channel_name`, `excitation_nm`, `emission_nm`, `color_argb`, `fluor`, `exposure_ns`, `acquisition_mode`, `illumination_type`, `objective_name`, `lens_na`, `nominal_magnification`, `immersion`, `microscope_name`, `user_name`, `application_name`, `application_version`, `acquisition_time`, `distance_x`, `distance_y`, `distance_z` | normalized XML fields |
| `decode_subblock`, `unshuffle_hilo`, `zstd1_header`, `header_size`, `chunk_kind`, `hilo` | codec plumbing (in `openreadout-codecs`) |
| `paste_tile`, `bgr_to_rgb`, `drop_alpha` | plane assembly |
| `CziDataset`, `open`, `path`, `file_len`, `problems` | opened-file state and structural problems collected while walking |
| `parse`, `from_raw`, `encoded_len`, `is_level0`, `scale`, `covers`, `upsample_factor`, `stored_ratio`, `bytes_per_sample`, `pixel_scale` | directory-entry helpers: id parsing, level-0 test, downsampling factor, index coverage, stored/logical ratio |
| `parse_image_xml`, `argb_to_rgb`, `channels`, `scaling`, `scene_names`, `t_increment_s`, `component_bit_count`, `contrast_method` | XML normalization helpers and fields |
| `min_z`, `min_c`, `min_t`, `tile_count` | per-scene index origins and tile count |
| `Level`, `levels`, `scale_x`, `scale_y`, `level_scale_range`, `AxisScale`, `STORED_SIZE_SLACK`, `snap_factor`, `level_place` | pyramid levels: downsampling factors, grouping of subblocks and the level pixel grid |
| `FilePart`, `parts`, `present`, `part_problem`, `part_file`, `part_path`, `master_path` | multi-file documents: following parts and their discovery |
| `ATTACHMENT_DATA_OFFSET`, `MAX_INTERPRETED_ATTACHMENT`, `extension_for` | attachment payload offset, interpretation size cap, extension for `extract` |
| `EventRecord`, `time_s`, `event_code`, `event_kind`, `description`, `parse_event_list`, `parse_time_stamps` | `CZEVL` and `CZTIMS` attachment payloads |
| `ValidMask`, `MASK_CHUNK_GUID`, `mask_width`, `mask_height`, `stride`, `bits`, `is_valid`, `all_valid`, `parse_valid_mask` | valid-pixel mask in a subblock attachment |
| `Conformed`, `conform`, `adjustments` | resolution protocol: a decoded bitmap brought to the declared size and pixel type |
| `SceneXml`, `scenes`, `center_x_um`, `center_y_um`, `contour_width_um`, `contour_height_um`, `well_name`, `well_id`, `row_index`, `column_index` | `Scenes/Scene` XML: stage centre, contour, well |
| `ExperimentXml`, `experiment`, `experiment_version`, `block_count`, `active_setups`, `time_series_cycles`, `time_series_interval_s` | `Experiment` XML summary |
| `detection_range_nm`, `dye_name` | channel detection band and display-setting dye |
| `SubBlockTags`, `parse_subblock_tags`, `stage_x_um`, `stage_y_um`, `focus_um` | per-subblock `<METADATA><Tags>` |
| `fuzz_directory_entries`, `fuzz_subblock_header`, `fuzz_attachment_entries`, `fuzz_metadata_xml` | byte-slice entry points for the cargo-fuzz targets (`fuzzing` feature only; not a stable API) |

### Performance / robustness merge (2026-09-23)

Tile sizes are checked once before worker decoding using the common file-relative plane limit, capped at 4 GiB. Parallel tiles retain bounded JPEG decoding and checked stack offsets; decoded buffers remain in place for colour conversion.
