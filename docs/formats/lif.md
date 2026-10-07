# Leica LIF family

Leica LAS X and LAS AF save microscope acquisitions as `.lif` project files; LAS X also writes the related LOF, XLIF, XLEF, XLCF and XLLF files (table below). OpenReadout returns every image in the project with its dimensions, pixel data and acquisition metadata. Derived from public files by hex dump and cross-checked against the public documentation and source of `liffile` (BSD-3-Clause). LOF, XLIF, XLEF, XLCF and XLLF follow liffile's documented structure and are tested on synthetic files (`crates/openreadout-lif/tests/fixtures/`, written by `make_fixtures.py`, cross-read by liffile). XLLF folder lists and XLIF files with TIFF frames are also checked on real LAS X 3.7 exports from two depositors (`figshare23522880-lasx-*`, `figshare30597152-lasx-*`). No licensed LOF, XLEF or XLCF file from LAS X is in the test corpus. Provenance: `docs/provenance/lif.md`.

LIF ("Leica Image File") is the container written by Leica LAS X / LAS AF. A file is one UTF-16 XML document describing a tree of experiments and images, followed by a sequence of raw memory blocks holding pixel data. Nothing is compressed. The same XML model is used by the rest of the family:

| extension | kind (`ContainerKind` / `XmlKind`) | what it is | how we read it |
| --- | --- | --- | --- |
| `.lif` | `Lif` | XML + named memory blocks | directly |
| `.lifext` | `Lifext` | sidecar next to `<stem>.lif`: more images (8-bit pyramid levels, histograms) keyed by the parent image's memory block id | directly; from the `.lif` it is listed (`info --view structure`), checked, and referenced in `extra.lifext_images` |
| `.lof` | `Lof` | one image: payload first, XML last | directly |
| `.xlif` | `Xlif` | XML text, one image; its memory block is stored as frames in other files (`.lof`, or TIFF/JPEG/PNG/BMP) | `.lof` and single-page TIFF frames (validated), JPEG/PNG (same rule, unvalidated); not BMP or multi-page OME/Aivia TIFF |
| `.xlef` | `Xlef` | XML text, experiment root; references `.xlif`/`.xlcf`/`.lof`/`.lif` files relative to itself | follows references |
| `.xlcf` | `Xlcf` | XML text, collection (folder) of references | follows references; image names get the collection's name as prefix |
| `.xllf` | `Xllf` | XML text, folder view | **inferred**: handled like `.xlcf` |

## Byte layout (all integers little-endian)

### File header block (offset 0; LIF, LIFEXT and LOF)

| offset | size | field (our name) | value / meaning |
| --- | --- | --- | --- |
| 0 | u32 | `block_marker` | always `0x70` |
| 4 | u32 | `block_len` | bytes that follow this field in the block header; equals `2 * xml_len + 5` |
| 8 | u8 | `text_marker` | always `0x2A` |
| 9 | u32 | `xml_len` | number of UTF-16 code units in the text |
| 13 | 2·`xml_len` | `xml_utf16` | LIF: the XML (`<LMSDataContainerHeader ...>`); LIFEXT: `<LMSDataContainerEnhancedHeader ...>`; LOF: the literal text `LMS_Object_File` (`LOF_TEXT`) |

### Memory blocks (LIF, LIFEXT: repeat until end of file, starting right after the XML)

| offset | size | field | value / meaning |
| --- | --- | --- | --- |
| 0 | u32 | `block_marker` | `0x70` |
| 4 | u32 | `block_len` | `2 * id_len + 14` (container version 2) or `2 * id_len + 10` (version 1) |
| 8 | u8 | `text_marker` | `0x2A` |
| 9 | u64 (v2) / u32 (v1) | `data_len` | size of the payload that follows the header; may be 0 |
| 17 (v2) / 13 (v1) | u8 | `text_marker` | `0x2A` |
| 18 / 14 | u32 | `id_len` | UTF-16 code units in the block id |
| 22 / 18 | 2·`id_len` | `block_id_utf16` | e.g. `MemBlock_86`; matches `Memory/@MemoryBlockID` in the XML |
| header end | `data_len` | `payload` | raw samples, or attachment bytes |

The container version is `LMSDataContainerHeader/@Version` in the XML. Every LIF corpus file so far is version 2. A version-1 branch (u32 `data_len`) is kept for older LAS AF files and marked `inferred` until a version-1 corpus file confirms it. A LIFEXT header says `Version="1"` but its blocks use the 64-bit layout (`ome-imagesc-110520-AMR1-lifext`, first block `MemBlock_17`, 55 050 240 B).

Truncation check: walk blocks; the last block's `header + data_len` must end exactly at the file size.

### LOF (single object)

| offset | size | field | value / meaning |
| --- | --- | --- | --- |
| 0 | 43 | header block | text `LMS_Object_File` |
| 43 | u8, u32, u8, u32 | `lof_versions` | `0x2A`, first version, `0x2A`, second version |
| 53 | u8, u64 | `data_len` | `0x2A`, payload length |
| 62 | `data_len` | payload | the image's memory block (no id; the XML's `Memory/@MemoryBlockID` names it) |
| 62 + `data_len` | 13 + 2·n | XML block | `0x70 u32 0x2A u32` + UTF-16 XML. Older files carry a bare `<Data><Image>…` fragment; we wrap it in an element named after the file stem (liffile documents the same) |

A truncated LOF loses its XML (it comes last), so it cannot be opened; the error is `corrupt_file`.

### XML containers (XLIF, XLEF, XLCF, XLLF)

Plain XML files, UTF-8 (with or without BOM) or UTF-16 (BOM, or `<\0?\0` / `\0<\0?`). Root `LMSDataContainerHeader`, one `Element`:

```
Element @Name
├── Data/Experiment | Data/Collection | Data/Image    (kind: XLEF | XLCF | XLIF)
├── Children/Reference @File @UUID                    (XLEF/XLCF/XLLF: other files, relative, URL-quoted, may use "\")
└── Memory @Size @MemoryBlockID                       (XLIF)
    └── Frame|Block|… @File @Offset @Size @UUID       (`FrameRef`: bytes [Offset, Offset+Size) of the block live in File)
```

References are resolved relative to the referencing file (`resolve_reference`, with a case-insensitive fallback per path component), recursion is depth-limited (16) and cycle-safe. A missing reference is a `check` error (`missing_reference`), never a failure to open. XLIF frames stored in `.lof` files become storage segments (each frame = that LOF's payload). A frame stored in an image file (`FrameKind`: `Tiff`, `Jpeg`, `Png`) is its *decoded* image: the decoded samples (little-endian) are bytes `Offset … Offset + Size` of the memory block (`FrameDecode`, decoded once on first read); an RGB image three times `Size` stands for a grey frame (its first sample); any other size is corrupt. LAS X keeps its memory order (blue at `BytesInc` 0, green 1, red 2 — `ChannelTag` 3, 2, 1) in the XLIF of an RGB image while the frame file is an ordinary RGB image, so a decoded RGB frame is placed by channel tag (`remap`): the channel tagged red receives the image's red samples. Validated on the LAS X 3.7 exports named above (TIFF frames `..\\name.tif`, URL-quoted, one folder above the `LeicaMetadata/` folder that holds the `.xlif`/`.xllf`, matched case-insensitively). BMP and multi-page OME/Aivia TIFF frames make plane reads exit 6 (`unsupported_frames` in `check`). Image names: an XLEF's children keep their own names; a collection prefixes its `Element/@Name` (liffile uses the same paths).

### Observed values (corpus)

| file | `block_len` | `xml_len` | blocks |
| --- | --- | --- | --- |
| `ome-michael-PR2729-frameOrderCombinedScanTypes.lif` | 30725 | 15360 | 2 (`MemBlock_72` 0 B, `MemBlock_86` 196608 B) |
| `aics-s-1-t-4-c-2-z-1.lif` | 51121 | 25558 | 2 (`MemBlock_296` 0 B, `MemBlock_312` 6031936 B) |
| `bsst749-2a-ishi-hf-fshr-dmso.lif` | 246137 | 123066 | 6 |
| `ome-imagesc-110520-AMR1.lif` | 5663079 | 2831537 | first two: `MemBlock_12` 0 B, `MemBlock_13` 10485760 B |
| `ome-imagesc-110520-AMR1.lifext` | 166751 | 83373 | first: `MemBlock_17` 55050240 B |
| `zenodo14976703-Convalaria-LambdaScan.lif` | 129017 | 64506 | 4 (`MemBlock_532` 15204352 B) |

## XML model (the subset we normalize)

```
LMSDataContainerHeader @Version          (LIFEXT: LMSDataContainerEnhancedHeader)
├── ChildrenOf @MemoryBlockID            (LIFEXT only: the parent image's block in the .lif; its Elements follow)
└── Element @Name @UniqueID
    ├── Data
    │   ├── Experiment @Path            (project-level; no pixels)
    │   │   └── TimeStamp @HighInteger @LowInteger   (Windows FILETIME halves)
    │   ├── Image                        (an image node)
    │   │   ├── ImageDescription
    │   │   │   ├── Channels/ChannelDescription @DataType @ChannelTag @Resolution @BytesInc @LUTName @Min @Max
    │   │   │   └── Dimensions/DimensionDescription @DimID @NumberOfElements @Origin @Length @Unit @BytesInc
    │   │   ├── TimeStampList @NumberOfTimeStamps  (text: hex FILETIMEs, one per frame)
    │   │   └── Attachment @Name ...     (HardwareSetting, ChannelAttachment, TileScanInfo, ImagePyramid, ...)
    │   └── SingleMoleculeDetection @IsImage="true"   (FALCON FLIM/TCSPC element; see below)
    ├── Memory @Size @MemoryBlockID      (which memory block holds this node's payload)
    └── Children/Element ...             (recursion; image name path = names joined with "/")
```

Images are listed in document order (pre-order), FLIM elements included, exactly as liffile lists them. For LIFEXT, every `ChildrenOf` group is walked (liffile 2026.7.14 follows only the first; `oracle/gen.py` walks all groups with liffile's own iterator) and paths start with the group's memory block id, e.g. `MemBlock_16/R 1_pmd_0`.

### Dimension ids (`DimID`) and how we expose them

| id | our axis (`Axis`) | exposed as |
| --- | --- | --- |
| 1 | `x` | X (`BytesInc` is the stride between neighbouring pixels) |
| 2 | `y` | Y (row stride) |
| 3 | `z` | Z |
| 4 | `t` | T (`Length` is in seconds) |
| 5 | `lambda` | **channels**: `size_c = channels × lambda steps`, channel-major (`c = channel * n_lambda + lambda_index`); each channel is named `<dye or LUT name> <wavelength> nm` and carries `emission_nm`, the λ coordinate, which is the **start** of the detection window (`zenodo14976703-Convalaria-LambdaScan`: 420, 430, … nm for windows 420–440, 430–450, …); `extra.lambda` has count and range. Wavelength = `Origin + i * Length / (n - 1)` (metres). The window itself comes from the hardware setting's `LambdaDefinition/LambdaEmission` (`LambdaDetectionBegin`, `LambdaDetectionStepSize`, `LambdaDetectionBandWidth`, `LambdaDetectionStepCount`) when its count and first window match the λ dimension: `emission_band_start_nm`/`emission_band_end_nm`/`emission_band_center_nm` and `emission_range_nm` (window i = [begin + i·step, begin + i·step + bandwidth]); without it the band is not reported. liffile keeps λ as its own array axis; bioio-lif 1.5.0 drops it (first plane only) |
| 6 | `rotation` | **separate images**, one per rotation index; name suffix `[rotation i]`, `extra.split` |
| 7 | `xt_slices` | **folded into T**: `t = t0 + n_T * (xt + n_XT * ts)` (T innermost, then XT slices, then T slices); `extra.t_folded` |
| 8 | `t_slices` | folded into T, outermost (see 7) |
| 9, 11, other | `excitation_lambda`, `loop`, `dimN` | separate images, like rotation (first XML axis innermost when several) |
| 10 | `mosaic` | **stitched**: one image per tile scan (see Tile scans) |

Axes 6–9 and 11 are validated only on the synthetic `synthetic-dims.lif` (cross-read by liffile); no licensed corpus file has them.

Physical step for a spatial axis = `Length / (NumberOfElements - 1)` in metres (sign may be negative for Z; we report the absolute value in µm). A step of 0, or of 1 cm or more, is reported as unknown: LAS X writes pixel indices labelled `m` (`Length = N - 1`) for FLIM result images, and `Length = 0` for some Z stacks. For `t`, the same formula gives seconds between frames.

### Channels

- `Resolution` = bits per sample (8, 12, 16, 32, 64). Storage width is `ceil(Resolution / 8)` bytes. When all integer channels of an image share it, it is reported as `images[].extra.bits_significant` (12 for 12-bit data in `uint16`), which sets the saturation level of `stats` and `watch --qc`.
- `DataType` 0 = integer, 1 = float. So (32, 1) → `float`, (16, 0) → `uint16`, (8, 0) → `uint8`, and **(16, 1) → IEEE half float**, which we widen exactly to `float` (32-bit) on read (NaNs are quieted, as hardware and NumPy do) and flag with `extra.stored_pixel_type = "float16"`. Seen in FLIM result images (`Fast Flim`, phasor maps).
- `BytesInc` = byte offset of this channel's first sample relative to the pixel origin. Channels are plane-interleaved when `BytesInc` equals the plane size, and pixel-interleaved (RGB) when it is 0, 1, 2 × sample width. Channels are ordered by `BytesInc` (storage order), as liffile orders them.
- `ChannelTag`: 0 = gray, 1 = red, 2 = green, 3 = blue (RGB cameras).
- `LUTName` is the display colour; we expose it as `color` when it maps to a known name.
- Dye: `ChannelProperty` with `Key` `DyeName` (LAS X 4, STELLARIS), or else the band of the channel's detector in the hardware setting: the sequential steps (`LDM_Block_Sequential_List/ATLConfocalSettingDefinition`, or the main setting without steps) in order, each step's active detectors (`DetectorList/Detector[@IsActive="1"]`) in order, each taking `Spectro/MultiBand[@Channel]/@DyeName` of the same step. The hardware mapping is used only when every step has its own bands, the active detectors match the channels one for one and no dye repeats. `Leica/` is dropped; `None` and empty mean no dye. A channel with a dye is named after it (`DAPI`, `ALEXA 488`) and the dye is its `fluorophore`; without one it is named after `LUTName`. The colour stays in `color`. Checked against liffile and Bio-Formats in `docs/provenance/lif.md` (2026-10-07).

### Sample address

For channel `c` (detector channel `c / L`, lambda `c % L`), plane `(z, t, split)`, tile `m` and pixel `(x, y)`:

```
block_offset + channel[c / L].BytesInc + (c % L) * inc(lambda) + z * inc(z) + fold(t) + split_offset + m * inc(mosaic) + y * inc(y) + x * inc(x)
```

where `inc(axis)` is that dimension's `BytesInc`, a missing axis contributes 0, `fold(t)` decomposes the folded T index over T / XT slices / T slices and `split_offset` does the same over the split axes. The block may be one byte range in one file (LIF, LIFEXT, LOF) or several (`Storage` segments: one per XLIF frame).

### Tile scans (DimID 10)

`Attachment[@Name="TileScanInfo"]` holds `@FlipX @FlipY @SwapXY` (`TileScan`) and one `Tile @FieldX @FieldY @PosX @PosY @PosZ` per tile (`TilePosition`, stage positions in metres), in the same order as the mosaic axis.

We stitch every tile scan into one plane on read (`mosaic.stitched_on_read = true`). Placement (`layout`, `PlacementMethod::StagePosition`):

1. negate `PosX` if `FlipX`, negate `PosY` if `FlipY`; then swap the two if `SwapXY`;
2. subtract the minimum over all tiles; divide by the X / Y pixel step in metres; round half up → `x_px`, `y_px`;
3. canvas = max offset + tile size; tiles are pasted **in file order, later tiles overwrite earlier ones** where they overlap. No registration, no blending.

This reproduces LAS X's own merge **bit for bit** for `aics-tiled` (165 tiles, `FlipX=FlipY=SwapXY=1`, 1-pixel overlaps) against `aics-merged-tiles` (7666 × 5622, all four channels). LAS X `…_Merged` images in `zenodo6606445-Project007` and `ome-imagesc-110520-AMR1` are registered by LAS X and do not equal a position-based placement (Project007 `Cell 1`: 972 × 972 by positions, 981 × 975 merged). When only one flip flag is set together with `SwapXY`, which image axis it reverses is inferred (flip before swap), not corroborated. bioio-lif 1.5.0 places tiles on the `FieldX/FieldY` grid with a one-pixel overlap instead (black-box observation).

Fallbacks, in order: when the stage positions are missing, non-finite, all equal while the field indices differ, not in metres, or give a canvas more than 64× the tiles' area (or over 2³⁴ pixels), tiles go on the `FieldX/FieldY` grid (same flip/swap rule, abutting; `FieldGrid`); with no usable tile metadata at all (or a tile count that differs from the mosaic axis), side by side in one row (`Row`). LIFEXT pyramid levels have a mosaic axis but no `TileScanInfo`; when the `.lif` is next to the `.lifext`, they inherit the parent image's tile positions (scaled by their own pixel step).

Per-tile access: `info` → `images[].extra.tiles` (`index`, `x_px`, `y_px`, `field_x`, `field_y`, `position_um`) and `extra.mosaic_placement`; `info --view structure` → one `tile` entry per tile with its byte offset (c 0, z 0, t 0), stride and `fully_visible` (no later tile covers it).

### FALCON FLIM / TCSPC

`Data/SingleMoleculeDetection[@IsImage="true"]` elements (`FlimInfo`): `Dataset/RawData/Format` (e.g. `LMSCOMPRESSED`), `Dimensions/Dimension/{DimensionIdentifier,Size}` (`X`, `Y`, `Z`, `C`, `M`, `WlEm`, `WlEx`, `S`, `T`, `L`, `V`), `VoxelSizeX/Y/Z` (m), `LaserPulseFrequency` (Hz), `ClockPeriod` (s), `PixelTime` (s). Histogram bins per laser period = floor(1 / frequency / clock period) (liffile documents the same; 528 in `zenodo13752242-FLIM250523`). We list these images with their X/Y/Z/C/T geometry, `pixel_type` `uint16` and `extra.flim`; reading a plane exits 6 (`unsupported_feature`) with a hint to read LAS X's derived child images (`Intensity`, `Fast Flim`, phasor maps), which are ordinary images and readable. liffile does not decode the raw data either.

### Acquisition metadata

- Start time: first entry of `TimeStampList`: its text holds hex FILETIMEs (LAS X); when it has none, its `TimeStamp` children hold the halves (`HighInteger << 32 | LowInteger`, LAS AF). Checked against liffile on every development LIF: 593 images agree, none differ (2026-09-26).
- Objective: first descendant of the image node carrying `@ObjectiveName` (LAS X hardware settings): `ObjectiveName` (trimmed: LAS X pads it with a trailing blank), `Magnification`, `NumericalAperture`, `Immersion` (`DRY` → `Air`, `OIL` → `Oil`, the shared spelling of `book/src/guides/metadata.md`), plus `@Software` on the `HardwareSetting` attachment and `SystemTypeName`/`MicroscopeModel`.
- LAS AF files (`bsst749-*`, `zenodo3382102-*`) have `Attachment[@Name="HardwareSettingList"]` instead: the objective is the `Variant` of `FilterSettingRecord[@Attribute="Objective"]` (magnification = the number before `x`, `63.0x1.40` → 63), its NA `FilterSettingRecord[@Attribute="NumericalAperture"]`, the system name `ScannerSettingRecord[@Identifier="SystemType"]` (`TCS SP5`), used as the model when there is no `MicroscopeModel`.
- Acquisition mode (`channels[].acquisition_mode`, every channel of the image): `Laser Scanning Confocal` when the hardware setting holds an `ATLConfocalSettingDefinition` or a `ScannerSettingRecord[@Identifier="dblPinhole"]`, `Widefield` when it holds an `ATLCameraSettingDefinition` (`AF 6000LX`); absent otherwise. No corpus LIF records the user (`UserManagementUserName="UserManagementFeatureInactive"` is not a name).
- Detection bands: `Attachment[@Name="ChannelAttachment"]/Band/Quantity` — first two values are the band start/end in metres.
- Tile positions: `Attachment[@Name="TileScanInfo"]/Tile @FieldX @FieldY @PosX @PosY @PosZ` (metres).

## Vocabulary (every public identifier in `openreadout-lif` must appear here)

| identifier | meaning |
| --- | --- |
| `LifReader`, `LifFile`, `LifError` | reader entry points |
| `ContainerHeader`, `block_marker`, `block_len`, `text_marker`, `xml_len`, `xml_utf16`, `xml_offset`, `container_version`, `lof_versions` | file header (XML offset; the two LOF version numbers) |
| `ContainerKind` { `Lif`, `Lifext`, `Lof` }, `kind`, `LOF_TEXT` | which binary member of the family a file is |
| `MemoryBlock`, `block_id`, `data_offset`, `data_len`, `header_offset` | memory block table |
| `ImageNode`, `name`, `path`, `unique_id`, `memory_block_id`, `memory_size`, `channels`, `dimensions`, `timestamps`, `hardware`, `tiles`, `tile_scan`, `flim`, `parent_block_id`, `frames` | parsed image (LIFEXT parent block; XLIF frames) |
| `ChannelDesc`, `data_type`, `channel_tag`, `resolution_bits`, `bytes_inc`, `lut_name`, `min`, `max`, `band_nm`, `dye_name` | channel (`lut_name` is the display colour, reported as `color`; `dye_name` the recorded dye, reported as the channel's `name` and `fluorophore`) |
| `DimensionDesc`, `dim_id`, `axis`, `count`, `origin`, `length`, `unit`, `bytes_inc`, `coordinate` | dimension (`coordinate(i)` = origin + i · length / (count − 1)) |
| `Axis` { `X`, `Y`, `Z`, `T`, `Lambda`, `Rotation`, `XtSlices`, `TSlices`, `Mosaic`, `Other` } | dimension ids |
| `HardwareInfo`, `objective_name`, `magnification`, `numerical_aperture`, `immersion`, `software`, `system_type_name`, `microscope_model`, `acquisition_mode`, `lambda_windows` | acquisition settings |
| `LambdaWindows`, `begin_nm`, `step_nm`, `bandwidth_nm`, `step_count` | λ-scan detection windows (`LambdaEmission`) |
| `TileScan`, `flip_x`, `flip_y`, `swap_xy`, `TilePosition`, `field_x`, `field_y`, `pos_x`, `pos_y`, `pos_z` | tile scan orientation flags and positions |
| `MosaicLayout`, `layout`, `width`, `height`, `tile_width`, `tile_height`, `method`, `placements`, `overlapped_by_later`, `TilePlacement`, `index`, `x_px`, `y_px`, `PlacementMethod` { `StagePosition`, `FieldGrid`, `Row` } | stitched-plane geometry and per-tile offsets |
| `FlimInfo`, `raw_format`, `raw_dims`, `voxel_size_m`, `laser_pulse_frequency_hz`, `clock_period_s`, `pixel_time_s`, `histogram_bins`, `size` | FALCON FLIM/TCSPC element |
| `XmlContainer`, `XmlKind` { `Xlif`, `Xlef`, `Xlcf`, `Xllf` }, `references`, `xml`, `from_xml`, `FrameRef`, `file`, `offset`, `uuid`, `memory_frames`, `decode_xml_text`, `normalize_reference`, `resolve_reference`, `looks_like_xml_container` | XML containers, their references and XLIF frames |
| `FrameKind` { `Tiff`, `Jpeg`, `Png` }, `from_ext`, `name`, `FrameDecode`, `kind`, `size`, `remap`, `index_of_frame`, `frame_kind` | XLIF frames stored as image files, how they are decoded, and their slots in the file table |
| `sample_address`, `plane_layout`, `PlaneLayout`, `contiguous`, `x_inc`, `y_inc`, `base_offset` | addressing |
| `Storage`, `Segment`, `segments`, `single`, `file_offset`, `virt_offset`, `contiguous_len`, `locate`, `FileTable`, `paths`, `index_of` | where a memory block's bytes live (one or more byte ranges in one or more files) |
| `FORMAT_ID`, `MAGIC`, `TEXT_MARKER` | constants |
| `LifDataset`, `open`, `path`, `file_len`, `first_block_offset`, `truncated_at`, `bad_block_at` | opened-file state and where the block walk stopped |
| `looks_like_lif`, `looks_like_lifext`, `looks_like_lof`, `parse_images`, `from_dim_id` | signature tests, XML walk, dimension-id mapping |
| `looks_like_lif`, `parse_images`, `from_dim_id` | signature test, XML walk, dimension-id mapping |
| `fuzz_xml_model`, `fuzz_sniff` | byte-slice entry points for the cargo-fuzz targets (`fuzzing` feature only; not a stable API) |

### Performance / robustness merge (2026-09-23)

Stitched canvases retain the tile-relative allocation guard with a hard 4 GiB ceiling. XML containers retain the stricter 256 MiB capped read.
