# Andor SIF

Andor Solis, the acquisition software of Andor (Oxford Instruments) cameras and spectrographs (iDus, Newton, iXon, iStar, Zyla on Shamrock and Kymera spectrographs and on microscopes), saves to `.sif`. Raman, photoluminescence, emission and absorption spectra, kinetic series and camera images are all SIF files.

Derived from hex dumps of corpus files and the documentation of `sif_parser` (BSD-3-Clause, Keisuke Fujii). `sif_parser` and `sifreader` (MIT) are run as black-box oracles (see *Validation*). No Andor specification or SDK was used, and every name below is ours. See `docs/provenance/andor-sif.md`. Format id `andor-sif`, crate `openreadout-sif`.

## What OpenReadout returns

A SIF file holds a **signal** block and, when they were recorded, **reference**, **background**, **live** and **source** blocks, in that order. Each block becomes:

- a **trace** when its frames are single rows (full vertical binning or a single track: the usual spectrum). Each frame is one sweep. When the file holds a calibration other than `0 1 0 0`, channel 0 is the calibrated axis (`Wavelength` in nm, `Raman shift` in cm⁻¹; `extra.axis` is irregular and points at channel 0) and channel 1 the values; otherwise the only channel is the values and `extra.axis` counts pixels from 1;
- an **image** otherwise: X = columns, Y = rows (several sub-images are stacked in Y, narrower ones padded with zeros on the right), T = frames; samples are float32 as stored, rows in file order.

The trace or image name is the block's name (`signal`, `background`, …). The names of the blocks after the signal are inferred from their position (they follow the data types of Andor Solis); `info` says so in a note.

`extra` of each trace and image: `block`, `sif_version`, `detector` (the detector model, e.g. `DU420_OE`), `detector_size` [columns, rows], `detector_temperature_c` (absent when the file stores −999), `exposure_s`, `accumulations`, `accumulation_cycle_s`, `kinetic_cycle_s` (the time between frames), `readout_time_s`, `frame_axis`, `data_type` (`Counts`), `image_axis`, `sub_images` (per sub-image: `left`, `top`, `right`, `bottom` in detector pixels, `x_binning`, `y_binning`, `width`, `height`), `calibration` {`coefficients` (ascending powers of the 1-based column number), `variable`, `unit`}, `frame_times_s` (when any is not 0), `original_name` (where Solis saved the file). Traces add `acquired_at` and, for kinetic series, `sweep_starts_s`; images set `acquired_at` (the acquisition start, Unix seconds, UTC), `time_increment_s` (the kinetic cycle time of a multi-frame block) and the channel's `exposure_ms`. `format_version` is the SIF version of the signal block.

## Layout (`layout.rs`)

All text is ASCII; values are separated by single spaces and lines end with `\n`.

| part | content |
| --- | --- |
| signature (`SIF_MAGIC`) | `Andor Technology Multi-Channel File\n` — detection (definite) |
| line 2 | `65538 1` |
| block | below, the signal |
| flags | four lines `0` or `1`, one per further block (reference, background, live, source); a `1` is followed directly by that block |
| XML trailer | newer versions: `<?xml …?><Signals><Signal>…` with settings (`PreAmpGainText`, `SpectrographSerial`, …): vendor metadata |

### One block (`Block`)

| item | content |
| --- | --- |
| first line | the SIF version (65555 … 65567), three values, the acquisition start (Unix seconds), the detector temperature (°C), 10 binary bytes, a value, then exposure time, accumulation cycle time, accumulated cycle time and number of accumulations (s, s, s, count), 2 binary bytes, kinetic cycle time and pixel readout time (s), then values kept as written (`first_line_values`) |
| detector line | the detector model |
| size line | ` columns rows L`, then the `L` bytes of the original file name |
| user text | `65538 U`, then `U` bytes: comments, or per-frame calibrations `Calibration data for frame N: c0,c1,c2,c3` |
| shutter line | a version, binary bytes and two numbers |
| settings lines | version-dependent: spectrograph, intensifier and calibration-version lines (`settings_lines`, kept as text); the line after the last line that starts with a 655xx value followed by control bytes holds the calibration coefficients |
| axis names | three length-prefixed strings: frame axis, data type, image axis (`10\nWavelength`, `6\nCounts`, `12\nPixel number`) |
| image structure | `655xx left top right bottom frames sub-images total-pixels frame-pixels` |
| sub-images | one line each: `65538 left top right bottom y-binning x-binning …` (width = (1 + right − left) ÷ x-binning, height = (1 + top − bottom) ÷ y-binning) |
| time stamps | one line per frame, µs |
| before the data | nothing, a `0` line, or a `1` line and one number per frame |
| data | frames × frame pixels float32, little-endian, each frame's sub-images in order |

The end of the first line is found by the shape of the next two lines (a printable detector line, then a size line of three numbers starting with a space), and the axis names by their shape (three length-prefixed printable strings followed by the image-structure line), so the version-dependent lines in between are never counted. The data start is the position after which the data ends at the end of the file or at a flag line.

## `check`

| code | severity | meaning |
| --- | --- | --- |
| `truncated` | error | a block's data runs past the end of the file |
| `structure` | error | a block after the signal cannot be read, or a flag line is missing |
| `frame_times` | warning | frame time stamps that go backwards |
| `trailing_bytes` | warning | bytes after the last block that are not XML |

A header that does not parse (a value missing, sizes that do not multiply) is a corrupt-file error (exit 4); reading a frame past the end of the file too.

## Validation (2026-10-19)

21 corpus files from 7 sources (`sif_parser` and `sifreader` test data, Zenodo 18293266, 15118586 and 4904806, figshare 30795137 and 28054061): SIF versions 65555, 65559, 65564 and 65567; detectors DH334T, DH734, DU401, DU420, DU491, DU888, DU897, DU920P, DU934P, DU940P, DV420 and a Zyla; calibrated and uncalibrated spectra, kinetic series of spectra and images, binned images, files with background and live blocks. Every trace (axis and values) and every plane agrees with `sif_parser`, and with `sifreader` for the three files `sif_parser` does not read (version 65564 with a cropped sub-image). `sif_parser` cannot parse the live block of `nd_lum_image_no_glue.sif` (its per-frame calibration text is followed by binary bytes); that block is compared by presence only.

## Vocabulary (every public identifier in `openreadout-sif/src` must appear here)

| identifier | meaning |
| --- | --- |
| `SifReader`, `SifDataset`, `FORMAT_ID`, `open`, `open_input`, `layout`, `dataset` | reader, opened file, format id `andor-sif`, modules |
| `SIF_MAGIC`, `looks_like_sif`, `parse` | signature, detection, parser |
| `SifLayout`, `file_len`, `blocks`, `xml`, `trailing_bytes`, `problems` | the parsed file |
| `Block`, `role`, `start`, `version`, `acquired_unix`, `temperature_c`, `exposure_s`, `accumulation_cycle_s`, `accumulated_cycle_s`, `accumulations`, `kinetic_cycle_s`, `readout_s`, `line3_rest`, `detector`, `detector_size`, `original_name`, `user_text`, `calibration`, `frame_calibrations`, `settings_lines`, `frame_axis`, `data_type`, `image_axis`, `area`, `frames`, `frame_pixels`, `sub_images`, `frame_times_us`, `data_offset` | one block (above) |
| `width`, `height`, `frame_bytes`, `data_end`, `name`, `uncalibrated`, `x_value` | block helpers |
| `SubImage`, `left`, `top`, `right`, `bottom`, `y_binning`, `x_binning` | one sub-image |
| `BLOCK_NAMES`, `MAX_HEADER`, `MAX_XML`, `MAX_FRAMES`, `MAX_SUBIMAGES` | block names and limits |
| `Exposed` { `Trace`, `Image` }, `axis_unit`, `MAX_TRACE_READ`, `MAX_PLANE_BYTES` | what a block becomes, axis units, read limits |
