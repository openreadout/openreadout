# Philips iSyntax (`.isyntax`)

Philips IntelliSite (UFS) slide scanners save whole slides as iSyntax: one file holding an XML header, a seek table and the code blocks of a wavelet pyramid in YCoCg colour. OpenReadout returns the slide as one RGB image pyramid, with the label and macro photos and the ICC profiles as attachments. The decoder is a port of libisyntax (BSD-2-Clause), checked against libisyntax run as a tool. Provenance: `docs/provenance/philips-isyntax.md`. Reader: crate `openreadout-wsi`, module `isyntax`, format id `philips-isyntax`.

## File layout

| part | content |
| --- | --- |
| XML header | UTF-8 XML up to the first byte 0x04 (`Header`: `length`) |
| seek table | an 8-byte tag (group 0x301D, element 0x2015, u32 size), then 40-byte entries (`SEEK_ENTRY`) |
| code blocks | the compressed blocks, at the offsets the seek table gives |

Detection (`looks_like`): the file starts with `<?xml` and names `DPUfsImport` in its first 512 bytes; the extension `.isyntax` alone is a weak match.

## Header

A tree of `DataObject` elements (attribute `ObjectType`) holding `Attribute` elements (`Name`, `Group`, `Element`, `PMSVR` value type, text value); an attribute whose type is `IDataObjectArray` holds an `Array` of data objects. `parse` reads it with roxmltree into `Header` { `length`, `attributes`, `images`, `tree` }. `attribute` returns a file-level value, `image` the first scanned image of a kind, `data_model` the interface version as (major, minor). `string_array` splits quoted array values (`"a" "b"`), `barcode` decodes `PIM_DP_UFS_BARCODE` (base64; blank barcodes are left out).

File level (`DPUfsImport`), as read: `PIM_DP_UFS_INTERFACE_VERSION` (`5.0`; 100 and above is the second data model), `DICOM_MANUFACTURER`, `DICOM_MANUFACTURERS_MODEL_NAME`, `DICOM_DEVICE_SERIAL_NUMBER`, `DICOM_SOFTWARE_VERSIONS`, `DICOM_DATE_OF_LAST_CALIBRATION`, `DICOM_TIME_OF_LAST_CALIBRATION`, `DICOM_ACQUISITION_DATETIME` (`YYYYMMDDhhmmss.ffffff`, no zone: `dicom_datetime`), `PIM_DP_UFS_BARCODE`, `PIIM_DP_SCANNER_CALIBRATION_STATUS`, rack, slot and operator attributes (operator left out of `info`), and `PIM_DP_SCANNED_IMAGES`: the scanned images.

Each `DPScannedImage` (`ScannedImage` { `kind`, `attributes`, `image_data`, `icc_profile`, `block_table`, `templates`, `dimensions`, `ranges`, `number_of_blocks`, `has_cluster_table` }): `PIM_DP_IMAGE_TYPE` (`WSI`, `LABELIMAGE`, `MACROIMAGE`); the label and macro hold a base64 JPEG in `PIM_DP_IMAGE_DATA`; `DP_COLOR_MANAGEMENT` holds a base64 ICC profile (`DICOM_ICCPROFILE`). The slide (`WSI`) also has:

- `UFS_IMAGE_BLOCK_HEADER_TABLE`: base64 of a u32 byte count and 48-byte entries (`BLOCK_ENTRY`), one per code block: a sequence item tag (0xFFFE, 0xE000, size 40), a coordinate tag and five u32 (x, y, colour component, scale, coefficient: 0 for LL, 1 for the three high-pass bands), a template tag and a u32 template index. Entries with offsets (80 bytes, sequence size 72) are refused (exit 6).
- `UFS_IMAGE_BLOCK_HEADER_TEMPLATES`: one `UFSImageBlockHeaderTemplate` per template (`BlockTemplate` { `ranges`, `compression` }): five `UFS_IMAGE_DIMENSION_RANGE`s (`DimRange` { `start`, `step`, `end` }, `count` = (end + step − start) / step): block width and height (128 × 128 in the corpus file), colour, scale and coefficient; `UFS_IMAGE_BLOCK_COMPRESSION_METHOD` (16).
- `UFS_IMAGE_GENERAL_HEADERS` → `UFSImageGeneralHeader`: `UFS_IMAGE_NUMBER_OF_BLOCKS` (an upper bound; the table holds fewer), `UFS_IMAGE_DIMENSIONS` (`Dimension` { `name`, `unit`, `scale_factor` }: x and y in `MicroMeter` with the size of a pixel, component `Y Co Cg`, scale, wavelet coefficient `LL LH HL HH`) and `UFS_IMAGE_DIMENSION_RANGES` (x and y: the padded image's coordinates; scale: the number of scales).
- Other attributes kept as metadata: `DICOM_DERIVATION_DESCRIPTION`, `DICOM_LOSSY_IMAGE_COMPRESSION`, `_RATIO`, `_METHOD`, `UFS_IMAGE_PIXEL_TRANSFORMATION_METHOD`, post-processing (`DP_IMAGE_POST_PROCESSING`: sharpness, CLAHE) and wavelet quantiser settings, which are not applied.

`info --view full` → `vendor.header` holds the whole tree, base64 values replaced by their length.

## Pyramid

`Pyramid` { `block_w`, `block_h`, `levels`, `offset`, `padded`, `padding`, `size`, `grid`, `mpp`, `blocks`, `compression` } (`from_header`):

- levels = the scale range's count; padding = 3·2^levels − 3 (`PER_LEVEL_PADDING`); size = padded size − 2 · padding; level s is `size >> s` (`level_size`);
- tiles are 2 · block size pixels (`tile_size`); the level-0 tile grid is ceil(padded / (block << levels)) << (levels − 1) per axis (`grid`), level s has `grid >> s` (`tiles`, `tile_count`);
- a code block (`CodeBlock` { `x`, `y`, `color`, `scale`, `ll`, `template`, `tile_x`, `tile_y`, `block_id` }) lies on tile ((x − offset − o) / (tile << scale)) of its scale, o = 3·2^scale − 2 for high-pass blocks and that + 2^scale for LL blocks; LL blocks exist only at the top scale;
- its seek-table entry (`block_id`) counts the tiles of the scales below it (including its own scale for an LL block), then row · columns + column, then colour · (all tiles of all scales + the top scale's tiles again for LL);
- a tile exists (`exists`, `existing_tiles`) when it has a colour-0 code block; `block` finds a code block by scale, kind, colour and tile.

`parse_seek_table`: entry `block_id` is a sequence item, then the offset (u64 at byte 16) and size (u64 at byte 32) of the block; each must lie inside the data.

## Code blocks

`decode_block` (compression method 16): blocks of 8 bytes or less are all zero. Otherwise: u32 length of the bit-plane data, the zero-run symbol (u8) and the zero-run counter width in bits (u8); a Huffman tree written as a depth-first walk (0 descends, 1 is a leaf followed by its 8-bit symbol; after a leaf the walk returns to the last left branch); then the message, least significant bit first: symbols, where the zero-run symbol followed by a non-zero counter stands for that many zero bytes and repeated zero-run symbols widen the counter. The data is 16 bit planes per coefficient band (3 bands for high-pass blocks, 1 for LL), each block_w · block_h / 8 bytes; when it is shorter, its last 2 or 6 bytes are masks of the planes stored. Plane order: band by band, mask bit 0 (the sign) first, then magnitude bits from the lowest. Samples come in 4 × 4 areas, raster order of areas; signed magnitude becomes two's complement. `CodecError` carries the reason a block is refused.

## Reconstruction

Tile (s, x, y) (`Decoder`, `tiles`; `TileKey`, `Coeffs`): each colour component is a buffer of four quadrants (LL | HL over LH | HH), each block size + 8 coefficients wide and high (`PAD` = 4 on each side), filled with the tile's own blocks and 4-coefficient margins from its eight neighbours (`Neighbour` { `ll`, `h` }, `tile_component`). A neighbour that does not exist leaves its margin 255 in the LL quadrant of Y and 0 elsewhere; one whose parent does not exist gives LL 255 (Y) or 0. LL blocks come from the top scale's code blocks or from the parent tile's transform (`child_ll`: the four block-sized quarters from coefficient 7, `FIRST_VALID`). `idwt` runs the 5/3 inverse lifting (first sample odd, 16-bit intermediates) over every row, then every column. Pixels (`to_rgb`): the 2·block window from coefficient 7, Y taken as an absolute value, then tmp = Y − (Cg >> 1), G = tmp + Cg, B = tmp − (Co >> 1), R = B + Co, clamped to 0..255. Tiles the file does not store are white.

Work is planned scale by scale: the requested tiles, their neighbours, the parents of those neighbours, up to the top scale; then decoded from the top down, in parallel within a scale. Coefficient blocks and RGB tiles are cached (384 MiB each).

## Regions and levels

Level s of the dataset is scale s. A level's pixel (0, 0) is pixel `padding >> s` of its tile grid (`level_origin`), the convention libisyntax uses; regions decode only the tiles they overlap, four tile rows at a time.

## Integrity checks (`check`)

`empty_scale` (warning), `bad_seek_table` (a code block's entry missing, not a sequence item, or its data outside the file), `bad_block` (a top-scale tile that does not decode; full check only).

## Vocabulary (every public identifier of the `isyntax` module must appear here)

| identifier | meaning |
| --- | --- |
| `IsyntaxReader`, `IsyntaxDataset`, `FORMAT_ID`, `open`, `looks_like` | reader, detection and opened dataset |
| `Header` { `length`, `attributes`, `images`, `tree` }, `parse`, `attribute`, `image`, `data_model`, `string_array`, `barcode`, `dicom_datetime` | the XML header |
| `ScannedImage` { `kind`, `attributes`, `image_data`, `icc_profile`, `block_table`, `templates`, `dimensions`, `ranges`, `number_of_blocks`, `has_cluster_table` } | one scanned image |
| `BlockTemplate` { `ranges`, `compression` }, `DimRange` { `start`, `step`, `end` }, `count`, `Dimension` { `name`, `unit`, `scale_factor` } | templates and dimensions |
| `Pyramid` { `block_w`, `block_h`, `levels`, `offset`, `padded`, `padding`, `size`, `grid`, `mpp`, `blocks`, `compression` }, `from_header`, `tiles`, `tile_count`, `level_size`, `level_origin`, `tile_size`, `block`, `exists`, `existing_tiles`, `PER_LEVEL_PADDING`, `BLOCK_ENTRY`, `SEEK_ENTRY`, `parse_seek_table` | geometry and code-block index |
| `CodeBlock` { `x`, `y`, `color`, `scale`, `ll`, `template`, `tile_x`, `tile_y`, `block_id` } | one code block |
| `decode_block`, `CodecError` | code-block codec |
| `idwt`, `tile_component`, `child_ll`, `to_rgb`, `Neighbour` { `ll`, `h` }, `PAD`, `FIRST_VALID` | reconstruction |
| `Decoder`, `new`, `tiles`, `TileKey`, `Coeffs` | tile decoding with caches |
| `isyntax`, `codec`, `dataset`, `header`, `pyramid`, `render`, `wavelet` | modules |

`info` → `images[0].extra.isyntax`: `data_model`, `compression_method`, `derivation_description`, `lossy_compression`, `lossy_compression_ratio`, `lossy_compression_method`, `pixel_transformation_method`, `declared_blocks`, `code_blocks`, `block_size`, `scales`, `padded_size`, `padding`, `image_offset`, `tiles_per_scale`, `software_versions`, `scanner_serial`, `calibration_dates`, `calibration_times`, `calibration_status`, `barcode`; `images[0].extra.codec`. Attachments: `label`, `macro` (JPEG, with `image_type`, `width`, `height`), `icc profile (wsi)`, `icc profile (macroimage)`.

## Oracle mapping (corpus harness)

`oracle/gen.py` (`isyntax_`) and `oracle/gen_regions.py` (`isyntax_targets`): libisyntax at commit 92471dbf6c70 through pyisyntax 0.1.7 — level sizes, micrometres per pixel, whole levels up to 48 MB and regions of levels 0, 1, 3, 5 and 7 (standard rectangles plus windows on tissue), compared bit for bit.

## Known gaps

- Only data model 5 with compression method 16 (one public file); data model 100 and later (cluster headers, `has_cluster_table`) and method 19 are refused with exit 6.
- The pixel origin follows libisyntax; the scanner's own convention is not documented.
- Post-processing (sharpening, CLAHE) and the ICC profile are not applied.
