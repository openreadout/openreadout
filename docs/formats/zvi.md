# Zeiss AxioVision ZVI

Zeiss AxioVision 4.x saves microscope images (grey or colour, with Z stacks and channels) as `.zvi` files. OpenReadout returns the image with its planes, channels, exposure times and pixel size.

Derived from hex dumps of public files from many depositors (several microscope stands and cameras, 2003 to 2025) and comparison with Bio-Formats 8.5.0 run as a black box; olefile (BSD-2) is a second container reader for a file Bio-Formats cannot open. See `docs/provenance/zvi.md`. Format id `zvi`, crate `openreadout-zvi`. No ZEISS specification or SDK was used; every name below is ours.

## Container

A ZVI file is an OLE2 compound file (Microsoft's public **[MS-CFB]** specification; signature `D0 CF 11 E0 A1 B1 1A E1`). The shared reader `openreadout_core::cfb` (moved there from the Shimadzu reader) parses the header, FAT/DIFAT, mini FAT and directory tree and reads streams through their sector chains.

Detection: the `.zvi` extension and the compound-file signature → definite; the extension alone → extension-only.

Streams observed:

| stream | content | our use |
| --- | --- | --- |
| `Image/Contents` | typed values (below), then stored-object names of the sub-storages | not needed |
| `Image/Tags/Contents` | tag list of the whole image | scale, file name, image-level defaults |
| `Image/Item(n)/Contents` | one plane: typed values, the raw-image header, the samples | pixels |
| `Image/Item(n)/Tags/Contents` | tag list of that plane | indices, channel, exposure, times, optics |
| `Tags` | a 16-byte identifier, then a tag list (document level) | `vendor.document_tags` |
| `Thumbnail` | a 16-byte identifier, 10 more bytes, then a Windows BMP (`BM`, size at +2) | attachment `#0` (`.bmp`) |
| `Image/Layers/…`, `Image/Scaling/…`, `Image/DisplayItem`, `Image/RootFolder`, `SummaryInformation`, `DocumentSummaryInformation` | layers, shapes, property sets | listed by `info --view structure` |

## Typed values and tag lists (`tags.rs`)

A value is a little-endian u16 type code followed by its data. The codes observed are the OLE Automation `VARENUM` numbers of Microsoft's public **[MS-OAUT]** specification, so their data sizes follow that document: 0/1 empty; 2 i16; 3 i32; 4 f32; 5 f64; 7 date (f64 days since 1899-12-30); 8 text (u32 byte length + UTF-16LE, NUL-terminated); 11 bool (i16); 9/13 a 16-byte payload (all zeros in the corpus); 65 blob and 69 stored-object name (u32 length + bytes). Others from [MS-OAUT] (i8, u8, u16, u32, i64, u64, currency, ANSI text, 66–70) are accepted by size.

A **tag list** (`TagList`, `parse_tags`) is: value = version (i32 `0x20001000` in every file), value = count, then `count` entries of (value, i32 tag id, i32 attribute). Parsing stops cleanly at the first entry that is truncated or has an unknown type code (`unparsed`).

## Item `Contents` and the raw-image header

After the typed values, every plane stream holds a 28-byte header of seven little-endian u32: `IMAGE_MARKER` (`0x10002000`), width, height, depth (1), bytes per pixel, pixel format, valid bits; the samples follow immediately and end the stream (width × height × bytes per pixel bytes). `find_header` searches the first 64 KiB for the marker and accepts it only when the sample block fits in the stream. Observed: in every file the header starts at offset 296 and the samples at 324.

| bytes per pixel, format | samples | status |
| --- | --- | --- |
| 2, 4 | uint16 grey | validated (9 files) |
| 6, 8 | 3 × uint16 colour, stored blue, green, red (returned as red, green, blue) | validated (1 file) |
| 1, – | uint8 grey | inferred, no corpus file |
| 3, – | 3 × uint8 colour (BGR) | inferred, no corpus file |
| other | – | exit 6 (`extra.unsupported_sample_layout`) |

## Tag ids used (identified by value matching across files and with Bio-Formats' output)

| constant | id | meaning (ours) |
| --- | --- | --- |
| `TAG_WIDTH`, `TAG_HEIGHT`, `TAG_PIXEL_FORMAT` | 515, 516, 518 | image size and pixel format (same as the raw header) |
| `TAG_Z_INDEX`, `TAG_C_INDEX`, `TAG_T_INDEX` | 2819, 2820, 2821 | stored z, channel and time index of an item; values need not start at 0 or be contiguous (z 13–30, channels 2 and 4 are seen): distinct values sorted ascending become the z/c/t ordinals |
| `POSITION_TAGS` | 2822, 2823, 2827 | further per-item indices, 0 in every corpus file; items that differ in them become separate images (inferred) |
| `TAG_CHANNEL_NAME`, `TAG_CHANNEL_COLOR`, `TAG_EXPOSURE_MS` | 1284, 1282, 2564 | channel name, display colour (`0x00BBGGRR`), exposure in ms (50 ↔ Bio-Formats 0.05 s) |
| `TAG_EXCITATION_NM`, `TAG_EMISSION_NM` | 0x1000110, 0x1000111 | wavelengths (498/516 for eGFP) |
| `TAG_SCALE_X`/`TAG_UNIT_X`, `TAG_SCALE_Y`/`TAG_UNIT_Y`, `TAG_SCALE_Z`/`TAG_UNIT_Z` | 769/770, 772/773, 775/776 | pixel size and its unit code (`Image/Tags`); `UNIT_MICROMETRE` = 76; code 0 = uncalibrated (no physical size; Bio-Formats reports 1.0 µm) |
| `TAG_ACQUIRED` | 1025 | acquisition date (OLE date, local time) → `acquired_at` without a zone (a note says so) |
| `TAG_RELATIVE_TIME` | 300 | time of the item since the first, in days → frame `delta_t_s`; `time_increment_s` when T > 1 |
| `TAG_OBJECTIVE_NAME`, `TAG_OBJECTIVE_MAGNIFICATION`, `TAG_OBJECTIVE_NA` | 2049, 2076, 2077 | objective (`Plan Apochromat 100x/1.40 Oil …`, 100, 1.4) |
| `TAG_MICROSCOPE`, `TAG_CAMERA` | 2075, 1042 | `instrument.model` (`Axioskop 2`), `instrument.detector` (`AxioCamHR3`) |
| `TAG_FILE_NAME` | 1553 | name the image was saved under → image `name` |
| `TAG_CENTER_X`, `TAG_CENTER_Y` | 2073, 2074 | image centre in µm (half the calibrated width/height) → `extra.center_um` |

Every tag of the image and of each item is kept in `vendor` (`info --view full`), keyed by numeric id.

## Normalized model

One image per distinct `POSITION_TAGS` combination (one in every corpus file). `size_z/c/t` = number of distinct stored indices; `dimension_order` = the axis that changes first between consecutive items is fastest (`XYZCT` for z stacks stored channel by channel, `XYCZT` for channel-interleaved stacks). Channels in ascending stored channel index. `extra`: `bytes_per_pixel`, `pixel_format`, `bits_significant`, `stored_indices`, `center_um`, `position_indices`. Frames (`info --view full`): `c`, `z`, `t`, `frame` (item number), `delta_t_s`, `exposure_ms`.

Differences from Bio-Formats (black-box observations, `oracle/metadata_compare.py`, `docs/provenance/zvi.md`): Bio-Formats reports 1.0 µm for uncalibrated axes (we omit them); it reports an exposure of 0 for one channel in 3 files where tag 2564 holds 100, 5894 and 1001 ms (we report the stored values); its `AcquisitionDate` is tag 1025 of the **second** item in all 11 multi-plane files with dates (none for single-plane files), where we report the first item's (the acquisition start); it cannot open `figshare32769939-zvi-a1` at all. Colour snapshots keep the exposure only in `Image/Tags`: a single channel falls back to the image tags for its name, exposure and wavelengths.

## `check` finding codes

| code | severity | meaning |
| --- | --- | --- |
| `compound_file` | error | the compound-file directory or FAT has problems |
| `planes` | error | an item without a raw-image header, or (c, z, t) positions without exactly one item |
| `truncated` | error | an item's sample block is not fully reachable (truncated file) |
| `tags` | warning | tag entries that could not be parsed |

## Vocabulary (every public identifier in `openreadout-zvi/src` must appear here)

| identifier | meaning |
| --- | --- |
| `ZviReader`, `ZviDataset`, `FORMAT_ID`, `open`, `items`, `images` | format reader, opened file, the id `zvi`, open, stored planes, images |
| `ZviItem`, `number`, `width`, `height`, `bytes_per_pixel`, `pixel_format`, `valid_bits`, `data_offset`, `stream_size`, `z`, `c`, `t`, `position`, `tags` | one `Image/Item(n)` |
| `ZviImage`, `planes`, `layout`, `info` | one image: (c, z, t) → item, sample layout, normalized info |
| `SampleLayout`, `sample_layout`, `pixel_type`, `samples_per_pixel`, `bgr` | how samples are stored |
| `IMAGE_MARKER`, `IMAGE_HEADER_LEN` | the raw-image header |
| `TAG_WIDTH`, `TAG_HEIGHT`, `TAG_PIXEL_FORMAT`, `TAG_Z_INDEX`, `TAG_C_INDEX`, `TAG_T_INDEX`, `POSITION_TAGS`, `TAG_CHANNEL_NAME`, `TAG_CHANNEL_COLOR`, `TAG_EXPOSURE_MS`, `TAG_EXCITATION_NM`, `TAG_EMISSION_NM`, `TAG_SCALE_X`, `TAG_UNIT_X`, `TAG_SCALE_Y`, `TAG_UNIT_Y`, `TAG_SCALE_Z`, `TAG_UNIT_Z`, `UNIT_MICROMETRE`, `TAG_ACQUIRED`, `TAG_RELATIVE_TIME`, `TAG_OBJECTIVE_NAME`, `TAG_OBJECTIVE_MAGNIFICATION`, `TAG_OBJECTIVE_NA`, `TAG_MICROSCOPE`, `TAG_CAMERA`, `TAG_FILE_NAME`, `TAG_CENTER_X`, `TAG_CENTER_Y` | tag ids (table above) |
| `zvi_color`, `ole_date` | `0x00BBGGRR` → `#RRGGBB`; OLE date → ISO-8601 local time |
| `TagValue` (`Empty`, `Int`, `Float`, `Date`, `Bool`, `Text`, `Bytes`, `Opaque`), `read_value`, `as_f64`, `as_i64`, `as_text`, `to_json` | one typed value |
| `Tag`, `id`, `value`, `attribute`, `TagList`, `version`, `unparsed`, `get`, `f64`, `i64`, `text`, `parse_tags` | tag lists |
