# Hamamatsu DCIMG

Hamamatsu's acquisition software (HCImage and other DCAM-API applications) records camera streams as `.dcimg` files. OpenReadout returns the stream as one image whose frames are time points, with each frame's counter and time stamp.

Derived from hex dumps of corpus files (format versions 7 and 0x1000000; ORCA-Flash4.0 `C11440-22C` and ORCA-Fusion `C15440-20UP`; 16-bit) and the documentation of the `dcimg` Python package (MIT), with Bio-Formats 8.5.0 run as a black box (see *Validation*). No Hamamatsu specification, DCAM-API/SDK header or DLL was used; every name below is ours. See `docs/provenance/dcimg.md`. Format id `dcimg`, crate `openreadout-dcimg`.

A DCIMG file is one camera stream written by Hamamatsu's acquisition software (HCImage, DCAM-API based applications): frames of one sensor sub-array, each with the camera's frame counter and a time stamp. It becomes **one image whose frames are time points** (C = Z = 1).

## File header (all little-endian)

| offset | type | our name | meaning |
| --- | --- | --- | --- |
| 0 | 8 bytes | `DCIMG_MAGIC` | `DCIMG\0\0\0` — detection (definite) |
| 8 | u32 | `version` | 7 (`VERSION_PACKED`) or 0x01000000 (`VERSION_FRAMED`); anything else is refused (exit 6) |
| 0x20 | u32 | `session_count` | 1 in every corpus file; only the first session is read |
| 0x24 | u32 | (frames) | frame count; the session header's count wins when they differ (warning `frame_count`) |
| 0x28 | u32 | `header_bytes` | offset of the session header (0x78 in version 7, 0x70 in 0x1000000) |
| 0x30 | u64 | `declared_size` | file size (repeated at 0x40); a shorter file is truncated |

## Packed layout (version 7, `FrameLayout::Packed`)

Session header at `header_bytes` (offsets relative to it):

| offset | type | our name | meaning |
| --- | --- | --- | --- |
| 0 | u64 | `session_bytes` | size of the session (header, frames, footer and its index) |
| 0x20 | u32 | `frame_count` | frames |
| 0x24 | u32 | `bytes_per_pixel` | 2 (1 accepted, inferred) |
| 0x2c | u32 | `width` | pixels |
| 0x30 | u32 | `row_bytes` | ≥ width × bytes per pixel (rows are cut to the width) |
| 0x34 | u32 | `height` | pixels |
| 0x38 | u32 | `frame_bytes` | = `row_bytes` × `height` (checked) |
| 0x44 | u32 | (data offset) | frame 0 starts at `header_bytes` + this (`data_offset`) |
| 0x48 | u64 | (data size) | data offset + frames × frame bytes; the **footer** starts at `header_bytes` + this |

Frames follow each other without gaps (`frame_stride` = `frame_bytes`). Footer (`footer`, offsets relative to its start): u32 7, u64 at +8 = offset of a second structure, u32 at +0x28 = footer size. In the second structure: u64 +0x30 = offset of the frame counters (`counter_table`, u32 per frame), u64 +0x40 = offset of the time stamps (`stamp_table`, u32 seconds + u32 microseconds per frame), u64 +0x58 = offset of the stored pixels (`table_offset`, `count` samples per frame), u32 +0x64 = their byte offset inside a frame, u64 +0x68 = their size in bytes. A footer that is missing or does not start with 7 is reported (`truncated` / `bad_footer`); the frames stay readable.

## Framed layout (version 0x1000000, `FrameLayout::Framed`)

Session header at `header_bytes`:

| offset | type | our name | meaning |
| --- | --- | --- | --- |
| 0 | u64 | `session_bytes` | as above |
| 0x3c | u32 | `frame_count` | frames |
| 0x40 | u32 | `bytes_per_pixel` | 2 |
| 0x48 | u32 | `width` | pixels |
| 0x4c | u32 | `height` | pixels |
| 0x50 | u32 | `row_bytes` | bytes per row |
| 0x54 | u32 | `frame_bytes` | bytes per frame |
| 0x60 | u64 | (data offset) | frame 0 at `header_bytes` + this |
| 0x74 | u32 | `frame_stride` | frame bytes + trailer |
| 0x7c | u32 | `trailer_bytes` | 16 or 32 in the corpus; when stride and trailer disagree, a 32-byte trailer is assumed (warning `trailer`) |
| 0xf0 | 16-byte entries | (block table) | (u32 kind, u32 size, u64 offset from `header_bytes` + 0xa0) of contiguous blocks, up to the first block |

Each frame is followed by a trailer: u32 frame counter, u32 seconds, u32 microseconds, then (32-byte trailers) the stored pixels at +12. Blocks we read:

- the 248-byte block (`CameraText`): NUL-padded text at +0x00, +0x20 and +0x80 (`versions`: software/firmware version strings, meaning not documented, listed as found), +0x40 (`model`: `C11440-22C`, `C15440-20UP`), +0x90 (`serial`: the text after `S/N:`), and at +0xc8 four u16 `sub_array` = sensor x, width, y, height of the frame (binning = width ÷ frame width);
- a kind-4 block (56 bytes; only files with stored pixels have one): u32 at +12 = size of the stored pixels in bytes, u32 at +16 = their byte offset inside a frame.

## Stored pixels (`StoredPixels`)

The ORCA-Flash4.0 files overwrite the first four pixels of one row of every frame with `0, 65535, 0, 65535` and store their real values separately: in the footer table (version 7) or in each frame's trailer (0x1000000). `row` = in-frame offset ÷ `row_bytes`, `column` = the remainder ÷ bytes per pixel, `count` = size ÷ bytes per pixel. In both corpus files with them the row is 83 of 168, which is sensor row 1023 (the sensor's centre: sub-array y 940 + 83). **The reader puts each frame's own stored values back** before returning the plane (`extra.stored_pixels` with `applied: true`; a note says so). The ORCA-Fusion files have neither the junk pattern nor a kind-4 block, and nothing is changed.

## Normalized model

`size_x/size_y` = width/height, `size_t` = frames, `pixel_type` uint16 (uint8 for 1 byte per pixel), no physical size (the file records none); `name` = the file stem; `acquired_at` = the first frame's time stamp (UTC — the stamps are Unix seconds: `Cell09` frame 0 = 16:53:17Z, its lab note says 12:54 PM US Eastern daylight time), with microseconds; `time_increment_s` = (last − first stamp) ÷ (frames − 1); `instrument` = Hamamatsu, model and detector = the camera model (framed files). `extra`: `dcimg_version`, `frame_layout` (`packed`/`framed`), `camera_serial`, `camera_versions`, `sensor_sub_array` {x, width, y, height}, `binning`, `stored_pixels` {row, column, count, applied}, `frame_counter_range` [first, last]. Frames (`info --view full`, `Dataset::frames`): `t`, `frame` (the camera's counter), `delta_t_s` (since the first frame), `acquired_at` (UTC). `vendor`: `file_header`, `session`, `footer`, `camera`, `stored_pixels` in the vocabulary below. `info --view structure`: file header, session header, one entry per frame (the first 64, then one summary row), footer.

Row order: planes are returned in stored order (row 0 first), as `dcimg` returns them. Bio-Formats returns them bottom row first.

## `check`

| code | severity | meaning |
| --- | --- | --- |
| `truncated` | error | frames (or the last trailer, or the footer) run past the end of the file; the file is shorter than its header or session declares |
| `bad_footer` | error | the version-7 footer does not start with 7 or its offsets are implausible |
| `frame_counter_gap` | warning | the camera frame counter does not increase by one: frames were dropped |
| `trailing_bytes` | warning | the file is longer than its header declares |
| `frame_count`, `trailer`, `sessions`, `bad_table` | warning | header fields that disagree (see above) |

Reading a frame that lies past the end of the file is a corrupt-file error (exit 4); a file cut inside its headers does not open (exit 4).

## Validation (2026-09-23)

15 corpus files (Zenodo 14281237, 14268554, 14287640; CC-BY-4.0), 24 planes, all bit-exact: version 7 against `dcimg` (full-frame reads with each frame's stored values), version 0x1000000 against Bio-Formats (rows flipped back). Bio-Formats puts frame 0's stored values into every frame of the version-7 file (4 pixels differ on frames 1–9); `dcimg` reads the version-0x1000000 files without correction (its correction position is a fixed header offset that these files do not use), differing from us in exactly the 4 corrected pixels of the `Cell09` files and nowhere in the bead files. Frame counters and time stamps equal `dcimg`'s for all 24 frames.

## Vocabulary (every public identifier in `openreadout-dcimg/src` must appear here)

| identifier | meaning |
| --- | --- |
| `DcimgReader`, `DcimgDataset`, `FORMAT_ID`, `open`, `layout` | reader, opened file, format id `dcimg`, its parsed structure |
| `DCIMG_MAGIC`, `VERSION_PACKED`, `VERSION_FRAMED`, `looks_like_dcimg` | signature and the two known versions |
| `DcimgLayout`, `read`, `version`, `session_count`, `header_bytes`, `declared_size`, `file_len`, `session_bytes`, `frame_count`, `bytes_per_pixel`, `width`, `height`, `row_bytes`, `frame_bytes`, `data_offset`, `frame_stride`, `trailer_bytes`, `footer`, `counter_table`, `stamp_table`, `stored_pixels`, `camera`, `sub_array`, `problems` | parsed structure (above) |
| `FrameLayout` { `Packed`, `Framed` } | the two layouts |
| `frame_offset`, `frames_end`, `complete_frames`, `stamps`, `stored_values` | frame positions, frames inside the file, counters/time stamps, stored pixel values of a frame |
| `StoredPixels`, `row`, `column`, `count`, `table_offset` | the overwritten pixels |
| `CameraText`, `model`, `serial`, `versions` | camera text of framed files |
| `FrameStamp`, `counter`, `seconds`, `micros`, `unix_seconds`, `iso8601` | one frame's counter and time stamp |
| `Problem`, `code`, `detail`, `offset` | structure problems found while parsing |
