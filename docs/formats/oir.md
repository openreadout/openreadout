# Olympus/Evident OIR

FluoView on Olympus/Evident FV3000 and FV4000 confocal systems saves each acquisition as an `.oir` file. OpenReadout returns its planes with their dimensions and the XML metadata. Derived from public files by hex dump, then cross-checked against the documentation and source of `oirfile` 2026.9.6 (BSD-3-Clause). Provenance: `docs/provenance/oir.md`.

OIR ("Olympus Image format Raw") is written by FluoView (FV3000, FV4000 confocal systems). One `.oir` file holds one acquisition: up to T (time) × λ (lambda, spectral) × Z × channels planes of one XY size, a reference image, a bitmap thumbnail and XML metadata. Large acquisitions continue in files named `<stem>_00001`, `<stem>_00002`, … (no extension) next to `<stem>.oir`; they have the same binary layout and hold further pixel and frame-property blocks, while the `.oir` holds the metadata. Nothing is compressed.

## Byte layout (all integers little-endian)

### File header (`OirHeader`, 96 bytes)

| offset | size | field (our name) | value / meaning |
| --- | --- | --- | --- |
| 0 | 16 | `MAGIC` | ASCII `OLYMPUSRAWFORMAT` |
| 16 | 4 × u32 | `header_words` | `12, 0, 1, 2` in every corpus file (meaning unknown) |
| 32 | u64 | `declared_size` | file size as written. Stale bytes may follow it (`zenodo13680725-stitch-a01-g001`: 237 bytes of an older, longer index) — a `check` warning; a declared size beyond the end of the file means truncation |
| 40 | u64 | `index_offset` | offset of the block index |
| 48 | u32 | `block_count` | number of entries in the index |
| 52 | 3 × u32 | — | `0, 1, 0` |
| 64 | u64 | `thumbnail_offset` | offset of the thumbnail block, or `0xFFFFFFFFFFFFFFFF` |
| 72 | 8 | `producer` | ASCII `FLUOVIEW` |
| 80 | u32, u32, u64 | — | `3, 2, 0xFFFFFFFFFFFFFFFF` |
| 96 | | `FIRST_BLOCK_OFFSET` | first block |

### Block index (at `index_offset`)

`u32 INDEX_MARKER` (`0xFFFFFFFF`), then `block_count` × u64 block offsets, in file order, up to `declared_size`. Blocks tile the file with no gaps from byte 96 to the index (checked in every corpus file). When the index is unusable (truncated file, bad marker) the reader recovers the blocks by walking the chain from byte 96 (`payload_len` gives each block's end); `check` then reports `bad_index`.

### Blocks (`Block`)

| offset | size | field | meaning |
| --- | --- | --- | --- |
| 0 | u32 | `payload_len` | bytes after this 8-byte header |
| 4 | u32 | `kind` | `BlockKind` below |
| 8 | `payload_len` | payload | |

| kind | `BlockKind` | payload |
| --- | --- | --- |
| 0 | `Documents` | several XML documents (below); two per file: a snapshot written after the first frame and the complete set written at the end — we use the **last** one |
| 1 | `FrameProperties` | one `lsmframe:frameProperties` document per frame (a frame = one t, λ, z position, all channels) |
| 2 | `Thumbnail` | ASCII `BMP ` then a Windows bitmap (e.g. 116×86, 24-bit): exposed as attachment `thumbnail` |
| 3 | `ChunkTag` | `u32 plane_offset, u32 chunk_len, u32 name_len, name` — names the pixel block that follows |
| 4 | `Pixels` | raw samples of one chunk of one plane |
| 5 | `Separator` | empty; written after each frame |

The payload of kinds 0 and 1 starts with 36 bytes of words we do not interpret, then each document as `u32 length` + ASCII XML (the four bytes before every `<?xml` equal the document's length). `lut:LUT` documents are additionally preceded by `u32 36` + the ASCII uuid of the channel they belong to (`documents`, `XmlDoc.channel`).

### Plane chunks (`ChunkTag`, `ChunkName`)

Every plane (one channel at one t, λ, z) is stored as one or more chunks, each a `ChunkTag` block followed immediately by a `Pixels` block of `chunk_len` bytes. `plane_offset` is the chunk's byte offset inside its plane (e.g. 47 chunks of 11 264 bytes, the last 6 144, for a 512×512×2 plane). Chunks of different channels may interleave (A0, B0, A1, B1, …), so chunks are placed by `plane_offset`, not by file order; `check` verifies that a plane's chunks cover it exactly.

Chunk names:

| form | example | meaning |
| --- | --- | --- |
| `[t###][l###][z###]_<a>_<b>_<uuid>_<n>` | `l002z001_0_1_93e4632f-…_17` | 1-based time (`t`), lambda (`l`), z (`z`) indices (absent axes are omitted), a pair of numbers that is `_0_1` in every corpus file, the channel uuid, the chunk number |
| `REF_<source>_<uuid>_<n>` | `REF_LSM0_d59928c9-…_1` | reference image of that channel (`source` e.g. `LSM0`) |

Distinct index values are mapped to consecutive positions (oirfile does the same); `check` warns (`sparse_axis`) when they are not 1..n. Other axis letters are counted (`unknown_axes`, `check` warning `unknown_axis`) and not exposed.

### Continuation files

For `<dir>/<stem>.oir` the reader opens `<dir>/<stem>_00001`, `_00002`, … until the first missing number. Each has its own header, blocks and index; their chunk tags add planes (in `zenodo13680725-stitch-a01-g001`: 60 of 106 planes in the `.oir`, 46 in `_00001`). A continuation file on its own is detected as OIR with a note to open the `.oir`. `check` compares the planes stored with the planes the acquisition declares (`imageInfo` axes, below) × channels and reports `missing_planes`, naming the continuation file that would come next when it is absent; `info` adds a note.

## XML documents

Element names are matched by local name. The documents of the last documents block, by root element:

| root | our key in `vendor` | what we normalize from it |
| --- | --- | --- |
| `fileinfo:fileInfomation` (sic) | `file_info` | `version` → `format_version` (e.g. `2.1.2.3`) |
| `lsmimage:imageProperties` | `image_properties` | `ImageProperties`: creation time, system name/version, microscope, channels, acquisition axes, pixel size, objective, laser lines |
| `annotation:annotationStore` | `annotations` | — |
| `overlay:contents` | `overlays` | — |
| `lut:LUT` (one per channel) | `luts` (with `@channel`) | `lut:name` → channel `color` when it is a plain colour name |
| `lsmimage:lsmChannel` (one per channel) | `channel_settings` | `ChannelSettings`: detector (`deviceName`), dye name and excitation/emission maxima |
| `base:imageDefinition` | `image_definitions` | reference-image geometry |
| `event:eventList` | `event_list` | — |
| `cameraimage:*` | `camera_images` | reference-image sample width (`elementChannel/depth`); no corpus file |
| `lsmframe:frameProperties` (frame blocks) | `frame_properties.first` + `count` | `FrameRecord`: plane geometry (`FrameGeometry`: `width`, `height`, `depth` bytes/sample, `bitCounts`, `colorType`) and `axisValue` positions |

`vendor` keeps every document as JSON with its own names; text values longer than 4096 characters (the 512 KiB hex tables in `lut:LUT`) are replaced by `<N characters omitted>`. Per-frame values are exposed by `info --view full` under `images[0].extra.frames` rather than repeating every frame document.

### Normalized model

| field | from |
| --- | --- |
| images | image 0: the planes of the main chunks; image 1: the reference image (`REF_` chunks) when present, `extra.kind = "reference"`. A file without chunks (e.g. the overview map `zenodo13680725-map-a01`, whose frame properties declare a 45858×30169 canvas) has no images and a note |
| `size_x`, `size_y` | first frame's `imageDefinition` `width`/`height`; `size_y` grows to the stored rows when a plane holds more bytes (line scans; no corpus file) |
| `pixel_type` | `depth` 1 → `uint8`, 2 → `uint16`, 4 → `float` (4 from oirfile only; no corpus file) |
| `size_z`, `size_t` | distinct `z`, `t` indices in chunk names |
| `size_c` | channels with pixels × lambda steps, **channel-major**: `c = channel * n_lambda + lambda_index` (the same rule as the LIF reader); `extra.lambda` has the wavelengths |
| channel order | `channel` elements with an `id` in `imageProperties`, first occurrence per id, stably sorted by `@order` (oirfile does the same); channels with pixels but no description follow, sorted by uuid |
| channel `name` | `channel/name` (`CH1`, …); lambda channels get `"<name> <nm> nm"` |
| `fluorophore` | `lsmChannel/dyeName` |
| `excitation_nm` | the channel's `laserDataId`s → `imagingMainLaser[@id]` with `@enable="true"` → `wavelength`, when exactly one wavelength results |
| `emission_nm` | lambda channels: the lambda wavelength; otherwise the dye's `emissionWavelength` |
| `emission_range_nm` | `startWavelength`/`endWavelength` under the channel (detection band), non-lambda channels |
| `color` | the channel's LUT name when it is a plain colour |
| `physical_size.x/y` | first `length/x`, `length/y` in `imageProperties`, unit from the sibling `pixelUnit/x` (`MICRO_METER`, `NANO_METER`, …) |
| `physical_size.z` | `ZSTACK` axis `step` (absolute), else the spacing of the frames' `ZSTACK` positions |
| `time_increment_s` | difference of the `TIMELAPSE` positions (ms) of frame 0 and frame `n_lambda × n_z`, else the `TIMELAPSE` axis `step` (s) |
| acquisition axes | `imageProperties/imageInfo/axis` (`axis`, `startPosition`, `endPosition`, `step`, `maxSize`); the first-frame snapshot has none, and then the acquisition-settings `axis` elements with `@enable="true"` (and not `@paramEnable="false"`) |
| `objective` | `objectiveLens`: `name`, `magnification`, `naValue`, `immersion` (`DRY`, `OIL`, …) |
| `instrument` | manufacturer `Olympus/Evident`; model = `systemName` (`FV4000`); software = header `producer` (`FLUOVIEW`); version = `systemVersion`. `extra.microscope` = microscope stand name (`IX83P2ZF`) |
| `acquired_at` | `imageProperties` `creationDateTime` (ISO-8601 with offset, as written) |
| extras | `significant_bits`, `color_type`, `channel_ids` (uuids), `channel_detectors`, `continuation_files` |
| frames | one record per frame-properties block: `index`, `name`, `t`, `lambda`, `z` (0-based positions), `time_s`, `z_um`, `lambda_nm`, `created` |

## Integrity checks (`check`)

Header (signature, recorded size vs actual: `truncated` / `trailing_bytes`, index usable: `bad_index`, block count: `block_count_mismatch`, thumbnail offset: `bad_thumbnail_offset`); block chain contiguous from byte 96 to the index (`gap`, `overlapping_blocks`, `unknown_block`, `truncated`, `bad_block`); each chunk tag followed by a pixel block of the declared length (`orphan_chunk_tag`, `chunk_length_mismatch`, `unparsed_chunk_name`); each plane covered exactly (`incomplete_plane`, `overlapping_chunks`, `oversized_plane`); planes declared vs stored (`missing_planes`, naming the missing continuation file); frame count (`frame_count_mismatch`); XML (`bad_xml`, `missing_metadata`, `unknown_channel`); continuation files that do not open (`bad_continuation`). Any error → exit 4.

## Vocabulary (every public identifier in `openreadout-oir` must appear here)

| identifier | meaning |
| --- | --- |
| `OirReader`, `OirDataset`, `OirFile`, `FORMAT_ID`, `open` | reader, opened dataset, one opened container file (main or continuation) |
| `OirHeader`, `header_words`, `declared_size`, `index_offset`, `block_count`, `thumbnail_offset`, `producer` | file header fields |
| `MAGIC`, `FIRST_BLOCK_OFFSET`, `INDEX_MARKER`, `looks_like_oir` | signature, first block offset, index marker, signature test |
| `Block`, `offset`, `kind`, `payload_len`, `payload_offset`, `end`, `payload`, `pixels_after` | a block, where its payload is, the pixel block after a chunk tag |
| `BlockKind` { `Documents`, `FrameProperties`, `Thumbnail`, `ChunkTag`, `Pixels`, `Separator`, `Unknown` }, `from_code`, `code`, `name` | block kinds |
| `ChunkTag`, `plane_offset`, `chunk_len`, `chunk_tags` | a chunk tag (where the next pixel block goes in its plane) |
| `ChunkName`, `parse`, `t`, `lambda`, `z`, `unknown_axes`, `channel`, `reference`, `chunk` | parts of a chunk name |
| `XmlDoc`, `root`, `text`, `documents` | a length-prefixed XML document and the extractor |
| `path`, `file_len`, `header`, `blocks`, `index_problem`, `truncated_at`, `bad_block_at` | opened-file state and what went wrong while indexing |
| `FrameGeometry`, `width`, `height`, `depth`, `bit_count`, `color_type` | plane geometry |
| `FrameRecord`, `created`, `geometry`, `positions` | one frame's properties |
| `AxisDesc`, `axis`, `start`, `step`, `max_size` | an acquisition axis |
| `ChannelDesc`, `id`, `order`, `band_start_nm`, `band_end_nm`, `laser_ids` | a channel in the image properties |
| `LaserLine`, `wavelength_nm`, `enabled` | a laser line |
| `ObjectiveDesc`, `magnification`, `numerical_aperture`, `immersion`, `working_distance_mm` | the objective |
| `ImageProperties`, `system_name`, `system_version`, `microscope`, `channels`, `axes`, `pixel_length_x`, `pixel_length_y`, `pixel_unit`, `objective`, `lasers` | what we take from `imageProperties` |
| `ChannelSettings`, `detector`, `dye_name`, `dye_excitation_nm`, `dye_emission_nm` | what we take from `lsmChannel` |
| `image_properties`, `channel_settings`, `frame_record`, `geometry`, `lut_name`, `lut_color`, `unit_to_um` | XML readers: image properties, channel settings, frame properties, plane geometry, LUT name, LUT colour, pixel-unit factor to µm |
| `read_at`, `read_into` | internal windowed file reads (block headers; pixel chunks) |
