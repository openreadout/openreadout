# DeltaVision

DeltaVision widefield deconvolution microscopes and OMX super-resolution systems (Applied Precision, later GE Healthcare and Leica) acquire to `.r3d` files, and their softWoRx software writes deconvolved (`_D3D.dv`), projected (`_PRJ.dv`), volume-rendered and FRAP results as `.dv`. Both are the same format: an MRC-like header followed by the image sections.

Derived from hex dumps of corpus files and the documentation of the `mrc` Python package (BSD-3-Clause, Sebastian Haase and Talley Lambert). `mrc` and Bio-Formats (GPL) are run only as black-box oracles (see *Validation*). Every name below is ours. See `docs/provenance/deltavision.md`. Format id `deltavision`, crate `openreadout-dv`.

## What OpenReadout returns

One image: X × Y = the section size, C = the wavelengths, Z = sections ÷ (wavelengths × time points), T = the time points. Planes are returned in stored row order with the samples converted to little-endian. `dimension_order` gives the file's section order (`XYZTC`, `XYCZT` or `XYZCT`). The physical pixel size is dx, dy (and dz when there is more than one Z plane) in µm. Each channel's name is its wavelength from the header; `emission_nm` and `excitation_nm` come from the extended header of the channel's first section (the header wavelength when the extended header has none), `exposure_ms` from its exposure time. `time_increment_s` is the time-stamp step between the first two time points.

`extra`: `section_order` (`ZTW`, `WZT`, `ZWT`), `pixel_mode`, `byte_order`, `lens_id` (the objective's id number), `image_type`, `wavelengths_nm` (as written; FRAP series use negative values for their photobleaching channels), `titles` (the processing history softWoRx writes: `IMGCORR`, `DECON3D`, `X:(1 512) Y:(1 512) Z:(1 40 1)`), `written_by` (`softWoRx deconvolution` when a title line starts with `DECON3D`, `softWoRx image correction` when one starts with `IMGCORR`, `acquisition` otherwise), `extended_header` {`integers`, `floats` per section}.

## Header (`header.rs`, 1024 bytes)

The byte order comes from the identifier −16224 at byte 96 (`a0 c0` little-endian, `c0 a0` big-endian).

| offset | type | our name | meaning |
| --- | --- | --- | --- |
| 0, 4, 8 | i32 | `width`, `height`, `sections` | section size and count |
| 12 | i32 | `mode` | 0 uint8, 1 int16, 2 float32, 4 complex float32, 5 int16, 6 uint16, 7 int32; 3 (complex int16) and others are refused |
| 16–36 | i32 × 6 | `start`, `sampling` | start indices and sampling |
| 40, 44, 48 | f32 | `spacing_um` | pixel spacing x, y, z (µm) |
| 76, 80, 84 | f32 | `intensity` | minimum, maximum, mean (first wavelength) |
| 92 | i32 | `ext_len` | bytes of the extended header |
| 96 | i16 | (`DV_ID`) | −16224 |
| 128, 130 | i16 | `ext_ints`, `ext_floats` | integers and floats per section in the extended header |
| 136–156, 172, 176 | f32 | `other_ranges` | minimum and maximum of wavelengths 2–5 |
| 160 | i16 | `image_type` | 0 or 100 normal, 1 tilt series, 3 averaged (projections), … |
| 162 | i16 | `lens_id` | objective id |
| 180 | i16 | `time_points` | |
| 182 | i16 | `sequence` | section order: 0 Z, then T, then wavelength (`ZTW`); 1 wavelength, Z, T (`WZT`); 2 Z, wavelength, T (`ZWT`) |
| 184–192 | f32 × 3 | `tilt` | tilt angles |
| 196 | i16 | `wavelengths` | channels |
| 198–206 | i16 × 5 | `wavelength_nm` | one per channel |
| 208, 212, 216 | f32 | `origin_um` | z, x, y origin |
| 224 | 10 × 80 bytes | `titles` | text lines |

The sections follow at 1024 + `ext_len`, each `width × height` samples.

### Extended header (`section_info`)

One record per section, in file order: `ext_ints` 32-bit integers then `ext_floats` 32-bit floats (8 and 32 in every corpus file; the block is padded to a multiple of 1024 bytes). The first 14 floats (`SectionInfo`): photosensor reading, time stamp (s), stage x, y, z (µm), minimum, maximum and mean intensity, exposure time (s), neutral-density filter, excitation and emission wavelength (nm), intensity scaling, energy conversion factor. `info --view full` lists them for the first 4096 sections.

## `check`

| code | severity | meaning |
| --- | --- | --- |
| `truncated` | error | the file is shorter than header + extended header + sections |
| `trailing_bytes` | warning | the file is longer |
| `extended_header` | warning | the extended header is smaller than its records need |

A header without the identifier, with sizes ≤ 0, sections that are not a whole number of Z planes, or an unknown section order is refused at open (exit 4 or 6). A plane past the end of the file is a corrupt-file error (exit 4).

## Validation (2026-10-19)

11 corpus files from 8 sources (OME sample images `DV/siRNAi-HeLa`, `will/FRAP`, `elena/P-TRE`, `U2OS`, `alexia/cajal-bodies`; Zenodo 5887370, 15919154, 377035): raw `.r3d` (big-endian) and processed `.dv` files (deconvolved, projected, volume renderings, FRAP); pixel modes 0, 1, 2 and 6; section orders 0, 1 and 2; 1 to 4 wavelengths, 1 to 33 time points, up to 150 sections. Every compared plane is bit-exact with `mrc`'s `DVFile`; Bio-Formats 8.5.0 reports the same sizes and pixel type for every file.

## Vocabulary (every public identifier in `openreadout-dv/src` must appear here)

| identifier | meaning |
| --- | --- |
| `DvReader`, `DvDataset`, `FORMAT_ID`, `open`, `open_input`, `header`, `dataset` | reader, opened file (and its parsed header), format id `deltavision`, modules |
| `looks_like_dv`, `dv_endian`, `mode_type`, `parse` | detection, byte order, pixel mode, header parser |
| `DvHeader`, `endian`, `width`, `height`, `sections`, `mode`, `start`, `sampling`, `spacing_um`, `intensity`, `ext_len`, `ext_ints`, `ext_floats`, `image_type`, `lens_id`, `time_points`, `sequence`, `wavelengths`, `wavelength_nm`, `other_ranges`, `origin_um`, `tilt`, `titles` | the header (above) |
| `channel_count`, `time_count`, `z_count`, `section_of` | sizes and the section of a plane |
| `SectionOrder` { `Ztw`, `Wzt`, `Zwt` }, `from_code`, `name` | section orders |
| `SectionInfo`, `section_info`, `photosensor`, `time_s`, `stage_um`, `exposure_s`, `nd_filter`, `excitation_nm`, `emission_nm`, `intensity_scaling`, `energy_conversion`, `SECTION_FLOATS` | extended header |
| `HEADER_LEN`, `DV_ID`, `DV_ID_OFFSET`, `MAX_EXT`, `MAX_PLANE_BYTES` | sizes and limits |
| `written_by` | which program wrote the file, from the title lines |
