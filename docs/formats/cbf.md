# CBF / imgCIF diffraction images (`cbf`)

CBF (Crystallographic Binary File) is the imgCIF form of the IUCr's CIF: CIF text, then one or more MIME-style binary sections, each holding an image. PILATUS and EIGER detectors write "miniCBF" files: one image per file and the experiment as `# key value` lines in the CIF item `_array_data.header_contents` (header convention `PILATUS_1.2`). Beamline software writes "full" imgCIF too, with the geometry in CIF categories. Derived from the IUCr imgCIF specification, the CBFlib documentation pages and FabIO (MIT) read as documentation. Provenance: `docs/provenance/cbf.md`. Crate: `openreadout-diffraction` (`src/cbf.rs`).

## Layout

- First line `###CBF: VERSION 1.5` (the version is `format_version`), then a `data_<name>` block (the name is the image name).
- CIF items before the first binary section are tokenised (whitespace-separated values, quoted strings, `;` text fields, `loop_` tables) and kept in `info --view full` → `vendor.items` and `vendor.loops`.
- A binary section starts with `--CIF-BINARY-FORMAT-SECTION--`, then MIME header lines: `Content-Type: application/octet-stream; conversions="x-CBF_BYTE_OFFSET"`, `X-Binary-Size` (coded bytes), `X-Binary-Element-Type` (`"signed 32-bit integer"`, ...), `X-Binary-Element-Byte-Order`, `X-Binary-Number-of-Elements`, `X-Binary-Size-Fastest-Dimension`, `-Second-Dimension`, `-Third-Dimension`, `X-Binary-Size-Padding`, `Content-MD5` (base64 MD5 of the coded bytes). After a blank line come the start marker `0C 1A 04 D5` and the data. The section closes with `--CIF-BINARY-FORMAT-SECTION----`.
- **Byte-offset coding**: each value is the previous one plus a delta: one signed byte; the byte `0x80` (−128) escapes to a little-endian 2-byte delta, −32768 to a 4-byte delta, and the smallest 32-bit value to an 8-byte delta. The first value is relative to 0.
- `x-CBF_NONE` sections are stored samples in the declared byte order. Other conversions (packed, canonical) are refused (exit 6).

Every binary section is an image (`images[i]`), fastest dimension = `size_x`, second = `size_y`, third = `size_z`. `info` reads the text and the MIME headers only; `read_plane` reads and decodes one section. A `.cbf.gz` file is decompressed once at open.

## Geometry

The PILATUS header lines (`PilatusHeader`) fill the shared diffraction geometry `images[].extra.diffraction` (`openreadout_core::diffraction::Geometry`):

| header line | `extra.diffraction` | note |
| --- | --- | --- |
| `Wavelength 0.97950 A` | `wavelength_a` | |
| `Detector_distance 0.19018 m` | `distance_mm` | metres → mm |
| `Beam_xy (1235.34, 1279.08) pixels` | `beam_center_px` | (fast, slow), as dxtbx reads it |
| `Pixel_size 172e-6 m x 172e-6 m` | `pixel_size_mm` | |
| `Start_angle`, `Angle_increment` | `oscillation_start_deg`, `oscillation_width_deg` | |
| `Oscillation_axis X, CW` | `scan_axis` | first word, lower case |
| `Exposure_time` | `exposure_s` | |
| `Count_cutoff` | `overload` | values at or above it are overloaded |
| `Detector_2theta` | `two_theta_deg` | only when not 0 |
| `Detector: PILATUS 6M-F, S/N 60-0114-F` | `instrument.model`, `extra.cbf.detector_serial` | DECTRIS for PILATUS and EIGER |
| the time stamp line | `acquired_at` | ISO 8601; `2009/Mar/12 13:06:50` is converted |

`negative_is_masked` is true on PILATUS and EIGER images: −1 marks module gaps and −2 bad pixels. `radiation` is `electron` below 0.1 Å, else `xray`. Full imgCIF files give the wavelength from `_diffrn_radiation_wavelength.wavelength`; their axis categories are listed raw. `extra.cbf` holds `compression`, `header_convention`, `detector_serial`, `sensor`, `sensor_thickness_mm`, `threshold_ev`, `exposure_period_s`, `flux`, `transmission`, `goniometer_deg` and a time stamp that could not be read as ISO.

The experiment (`openreadout_core::diffraction::experiment`) is single-crystal X-ray diffraction (or electron diffraction), with the wavelength, energy, distance, beam centre, pixel size, rotation and exposure as method parameters.

## `check`

| code | severity | meaning |
| --- | --- | --- |
| `truncated` | error | a section's data runs past the end of the file, or the gzip stream ends early |
| `md5_mismatch` | error | `Content-MD5` differs from the MD5 of the coded bytes |
| `bad_data` | error | the byte-offset stream ends before the declared number of values, or a value leaves the element type's range |
| `unused_bytes` | warning | coded bytes follow the last value (beyond the padding) |
| `masked_pixels`, `overloads` | info | pixels without a measurement; pixels at or above `Count_cutoff` |

## Vocabulary (every public identifier in `openreadout-diffraction/src` must appear in this document or in `smv.md`)

| identifier | meaning |
| --- | --- |
| `CbfReader`, `CbfDataset`, `CBF_FORMAT_ID`, `FORMAT_ID`, `open`, `layout` | reader, opened file, format id `cbf`, its parsed structure |
| `BINARY_MARKER`, `START_MARKER` | the section line and the 4-byte start of the data |
| `CbfLayout`, `read`, `file_len`, `version`, `block`, `convention`, `header_lines`, `items`, `loops`, `sections`, `problems`, `item`, `pilatus` | the parsed file |
| `Section`, `marker_offset`, `data_offset`, `binary_size`, `mime`, `conversion`, `conversion_text`, `element`, `big_endian`, `dims`, `elements`, `md5`, `padding` | one binary section |
| `Conversion` { `ByteOffset`, `Uncompressed` } | how a section is coded |
| `decode`, `encode_byte_offset` | byte-offset decoding (and coding, for tests and fuzz seeds) |
| `PilatusHeader`, `parse`, `detector`, `serial`, `timestamp`, `pixel_size_mm`, `sensor`, `sensor_thickness_mm`, `exposure_time_s`, `exposure_period_s`, `count_cutoff`, `threshold_ev`, `wavelength_a`, `distance_mm`, `beam_xy_px`, `start_angle_deg`, `angle_increment_deg`, `two_theta_deg`, `oscillation_axis`, `goniometer`, `flux`, `transmission` | the miniCBF header values used |
| `common`, `Opened`, `file`, `len`, `gzip`, `read_exact`, `number`, `tidy`, `md5`, `base64`, `radiation_of`, `ELECTRON_MAX_WAVELENGTH_A` | shared helpers: plain or gzip input, bounded reads, numbers as written, MD5 and base64 for `Content-MD5`, X-ray or electron by wavelength |
