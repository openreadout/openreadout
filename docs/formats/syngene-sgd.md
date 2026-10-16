# Syngene GeneSys `.sgd`

Syngene G:Box and PXi gel and blot imagers save their captures through GeneSys as `.sgd` files.
OpenReadout returns every capture as an image (each exposure of a series and the marker image)
with the imager, camera, light, filter, application, exposure, time and user.

Derived from two public files of two depositors (GeneSys 1.5.6.0 and 1.6.1.0) and the public
[MS-CFB] specification; olefile (BSD-2-Clause) was run as a black box for the stream listing.
Provenance: `docs/provenance/syngene-sgd.md`. Crate: `openreadout-gel`.

| format id | files | reads | confidence |
| --- | --- | --- | --- |
| `syngene-sgd` | `.sgd` written by Syngene GeneSys | one 8- or 16-bit image per capture, imager, camera, light, filter, application, exposure, time, user | low (evidence rubric, `docs/assurance.md`) |

## Layout

A compound file (OLE2, MS-CFB). Streams seen:

| stream | holds |
| --- | --- |
| `Detector/MultiImage/MultiData` | u16 5, u32 count, count × (u32 length + ASCII storage name), u32: the captures in order (`RawData`, `RawData0`, …) |
| `Detector/GeneSnap/<capture>/PixelData` | 26-byte header: u16 2, u32 width, u32 height, three u32 that are 1, u32 sample code (2: 8-bit, 4: 16-bit); then width × height little-endian samples, row by row from the top left |
| `…/Capture` | u16 15, 8 bytes, u8; then u32-length texts and numbers: software (`GeneSys Version: 1.6.1.0`), f64 exposure (s), u16, light, camera (`Synoptics 6.0MP`), a second camera text (`ART-694 #321`), imager (`G:Box mini 6`, `PXi 6`), u16, filter; later texts name the set-up (`ECL : No Light : No Filter`) and the application (`ECL`, `Visible Marker`) |
| `…/Description` | u16 5, u32 Unix time, u16, u64 .NET ticks (the same instant), u32-length user name, optionally a u32-length GUID |
| `…/Thumbnail` | a 12-byte header and a 100 × 80 BMP |
| `…/Annotations`, `Note`, `User`, `View`, `Rotation`, `SpotBlotAnalyser` | not decoded |

The bytes per sample come from the stream size, which must be exactly 26 + width × height × 1 or
× 2; a sample code that does not match is a warning.

## What the reader returns

- **Images**: one per capture, in `MultiData` order, then any other storage with a `PixelData`
  stream (natural order). Name = the storage name. `uint16`, or `uint8` for the 8-bit capture some
  files hold, one channel, one plane. A capture whose header or size does not fit is left out and
  reported by `check` (`unreadable_image`); a listed capture without pixels is `missing_image`.
- **Channel**: name = the application (`ECL`, `Visible Marker`), else the light; `exposure_ms`
  from the exposure (seconds × 1000; the unit is inferred, `docs/provenance/syngene-sgd.md`).
- **Instrument**: manufacturer Syngene, model = the imager text, software `GeneSys` and its
  version, detector = the camera.
- **`acquired_at`**: the Unix time of `Description`, as UTC.
- **Image `extra`**: `light`, `filter`, `camera_label`, `setup`, `application`, `user`,
  `exposure_s`, `storage` (the storage path).
- **`format_version`**: `GeneSys <version>`.
- **Experiment**: vendor Syngene, model, software and version; `acquisition.started_at` and
  `ended_at` (first and last capture), `operator` = the user; method parameters `exposure_time` (s,
  one capture) or `exposure_times` (several), `application`, `light`, `filter`; an application
  naming ECL or chemiluminescence makes the technique western blot (OBI:0000854).

`info` reads the small streams and the 26-byte pixel headers only; `read_plane` reads one
capture's samples through its sector chain.

## Validation

The corpus test compares the image count, width, height and pixel type with olefile's stream
listing and the `PixelData` headers. No independent reader decodes the samples; the planes are
checked only once, by the correlation of the Dataverse capture with the depositor's TIFF export
(r = −0.9999, `docs/provenance/syngene-sgd.md`).

## Vocabulary (public API of `openreadout-gel` for this format)

| identifier | meaning |
| --- | --- |
| `SyngeneReader` | reader of Syngene GeneSys `.sgd` files |
| `SYNGENE_FORMAT_ID` | `syngene-sgd` |
| `SgdDataset` | the dataset the reader returns |
| `Capture`, `software`, `exposure_s`, `light`, `camera`, `camera_label`, `imager`, `filter`, `setup`, `application`, `parse_capture` | the `Capture` stream |
| `parse_description`, `parse_multi_data`, `parse_pixel_header` | the `Description`, `MultiData` and `PixelData` header parsers |
