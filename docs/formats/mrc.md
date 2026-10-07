# MRC / CCP4 / MAP (MRC2014)

MRC is the working format of cryo-EM, electron tomography and 3D electron diffraction. OpenReadout returns the volume or image stack with its sample type, voxel size and extended header. Derived from the open MRC2014 specification published by CCP-EM (https://www.ccpem.ac.uk/mrc_format/mrc2014.php; Cheng et al., J. Struct. Biol. 192:146–150, 2015), the IMOD format documentation, and mrcfile (BSD-3-Clause), read as prior art and used as the reference reader. Provenance: `docs/provenance/mrc.md`.

One file is a 1024-byte **main header**, an optional **extended header** of NSYMBT bytes, and a **data block** of NZ sections of NX × NY samples. Extensions seen in the wild: `.mrc`, `.mrcs` (image stacks, RELION), `.map` / `.ccp4` (volumes, EMDB, crystallography), `.rec` (IMOD reconstructions), `.st` / `.ali` / `.preali` (IMOD tilt series). All of them are the same format.

## Main header (spec table; words are 4 bytes in the file's byte order)

| bytes | spec name | our field (`MrcHeader`) | use |
| --- | --- | --- | --- |
| 1–12 | NX, NY, NZ | `nx`, `ny`, `nz` | columns, rows, sections; `size_x = nx`, `size_y = ny` |
| 13–16 | MODE | `mode` (`Mode`) | sample type, see below |
| 17–28 | NXSTART, NYSTART, NZSTART | `nxstart`, `nystart`, `nzstart` | `extra.start_index` |
| 29–40 | MX, MY, MZ | `mx`, `my`, `mz` | grid sampling; MZ = sections per volume in a volume stack |
| 41–52 | CELLA | `cell_a` | cell size in Å; pixel size = CELLA / (MX, MY, MZ) |
| 53–64 | CELLB | `cell_b` | cell angles, `extra.cell_angles_deg` |
| 65–76 | MAPC, MAPR, MAPS | `mapc`, `mapr`, `maps` | cell axis of columns, rows, sections |
| 77–88 | DMIN, DMAX, DMEAN | `dmin`, `dmax`, `dmean` | `extra.density` |
| 89–92 | ISPG | `ispg` | 0 image stack, 1–230 volume, 401–630 volume stack |
| 93–96 | NSYMBT | `nsymbt` | extended-header bytes |
| 105–108 | EXTTYP | `exttyp` | `CCP4`, `MRCO`, `SERI`, `AGAR`, `FEI1`, `FEI2`, `HDF5` |
| 109–112 | NVERSION | `nversion` | 20140 / 20141; 0 in older files → `format_version` absent |
| 129–132 | (IMOD/SerialEM) | `nint`, `nreal` | bytes per section and item flags of `SERI` extended headers |
| 153–160 | (IMOD) | `imod_stamp`, `imod_flags` | stamp 1146047817; flag bit 1 = mode-0 bytes are signed |
| 197–208 | ORIGIN | `origin` | `extra.origin_angstrom` |
| 209–212 | MAP | `has_map_id` | `MAP` identifies the file (first three bytes compared) |
| 213–216 | MACHST | `machine_stamp`, `little_endian`, `stamp_problem` | `44 44`/`44 41` little-endian, `11 11` big-endian |
| 217–220 | RMS | `rms` | `extra.density.rms` |
| 221–224 | NLABL | `nlabl` | labels in use |
| 225–1024 | LABEL | `labels` | ten 80-character labels, `extra.labels` |

**Byte order.** The machine stamp decides; when the MODE word is invalid in that order but valid in the other, the other order is used and `stamp_problem` says so (files written with a wrong stamp exist; mrcfile does the same). An unknown stamp falls back to whichever order gives a plausible header (little-endian first).

## MODE (`Mode`) → returned samples

| MODE | `Mode` | returned `pixel_type` | note |
| --- | --- | --- | --- |
| 0 | `Int8` | `int8`, or `uint8` when `bytes_unsigned` | MRC2014 says signed; IMOD files with the stamp but without flag bit 1 are unsigned (`extra.bytes_signed`) |
| 1 | `Int16` | `int16` | |
| 2 | `Float32` | `float` | |
| 3 | `ComplexInt16` | `complex` | transforms: (real, imaginary) int16 pairs widened exactly to float32 pairs; NX complex values per row as stored (a half transform is not expanded) |
| 4 | `ComplexFloat32` | `complex` | transforms: (real, imaginary) float32 pairs as stored; NX complex values per row |
| 6 | `Uint16` | `uint16` | |
| 12 | `Float16` | `float` | IEEE half floats widened exactly to float32 |
| 16 | `Rgb8` | `uint8`, `samples_per_pixel = 3` | IMOD extension |
| 101 | `Packed4Bit` | `uint8` (0–15) | two values per byte, lower coordinate in the low nibble, rows padded to `(nx + 1) / 2` bytes |
| other | `Unknown` | — | `open` exits 6 |

`stored_bytes`, `section_bytes`, `in_spec`, `code`, `from_code`, `label` are the helpers behind this table.

## gzip-compressed files

EMDB distributes maps as `emd_NNNN.map.gz`. A gzip file whose first 1024 decompressed bytes are an MRC header (the `MAP ` stamp and a plausible header: *definite*; a plausible header in a file named `<name>.<mrc extension>.gz`: *likely*) is decompressed once at open through `openreadout_core::gzip` (restart points, CRC-32 and length checked) and then read like the plain file. `info` adds the note `gzip-compressed: <n> bytes decompress to <m>`; offsets in the output refer to the decompressed file. `.mrcz` and bzip2 files are not read.

## Sections → Z or T (`Layout`)

| ISPG | `Layout` | exposed as |
| --- | --- | --- |
| 401–630 with NZ divisible by MZ | `VolumeStack` | `size_z = MZ`, `size_t = NZ / MZ` (section `t * MZ + z`) |
| 0 | `ImageStack` | `size_t = NZ` (movie frames, particle stacks, tilt series) |
| 0 in a `.map`, `.rec` or `.ccp4` file | `Volume` | `size_z = NZ`, with a note (these files are volumes by convention) |
| anything else | `Volume` | `size_z = NZ` |

This follows the specification (note 6) and mrcfile's `is_image_stack` / `is_volume` / `is_volume_stack`. Programs that write ISPG 0 for volumes (ChimeraX `molmap`, many EM tools) therefore give T instead of Z for `.mrc` files; `extra.layout` says which rule applied.

## Physical sizes

`pixel_size_angstrom` (in `extra`) = CELLA / (MX, MY, MZ) for the cell axis each file axis maps to (MAPC/MAPR/MAPS; identity when they are not a permutation). `physical_size` is the same in µm (Å × 1e-4); `z` only for volumes. A zero cell or zero sampling gives no physical size (RELION stacks). A cell equal to the grid (1 Å per pixel) is reported as written, with a note, because many programs write it when no calibration is known.

**Axis order.** Planes are returned in file order: a plane is one section, NX columns by NY rows. MAPC/MAPR/MAPS are not applied (crystallographic maps such as EMD-3001, `3 1 2`, are not rotated); `extra.axis_order` names the cell axis (`X`, `Y`, `Z`) of columns, rows and sections, e.g. `ZXY`. `axis_map`, `cell_step_angstrom` and `file_axis_steps_angstrom` implement this.

## Extended header

Kept raw as attachment `#0` (`extract FILE #0`, content type = EXTTYP). Decoded as far as we know:

- **FEI1 / FEI2** (Thermo Fisher / FEI EPU, Tomography, Velox exports): one metadata block per section, all the same size; the first block's first word is the block size (`FEI1_BLOCK_LEN` = 768, `FEI2_BLOCK_LEN` = 888 for metadata version 2). Values use the header's byte order; the four bitmask words are always little-endian. Layout from mrcfile's `dtypes.py` (BSD-3-Clause); names are ours (table below). The bitmask words that say which fields are set are reported raw and not interpreted. Blocks are per-frame records (`frames`, `info --view full` → `images[0].extra.frames`) with `section`, the shared `t`/`z` indices, `stage_x_um`/`stage_y_um`/`stage_z_um` (from metres), `exposure_ms` (from the integration time in seconds) and `acquired_at`, followed by every field below; the first block is `extra.fei_metadata`. SerialEM and integer/real records carry `section`, `t` and `z` too.
- **SERI** (SerialEM): `nint` bytes per section; `nreal` flags select items stored in flag order: 1 tilt × 100 (short) → `tilt_angle_deg`; 2 piece coordinates (3 unsigned shorts) → `piece_coordinates`; 4 stage X/Y × 25 (shorts) → `stage_x_um`, `stage_y_um`; 8 magnification / 100 → `magnification`; 16 intensity × 25000 → `intensity`; 32 dose as a two-short float → `exposure_dose_e_per_a2`; 64–1024 reserved (skipped by size).
- **AGAR / MRCO** or SERI flags that do not add up: `nint` 32-bit integers then `nreal` floats per section → `integers`, `reals`.
- **CCP4** (and blank EXTTYP with ISPG > 0 and text content): symmetry operators as 80-character lines → `vendor.extended_header.symmetry`.
- **HDF5** and anything else: raw only.

### FEI metadata fields (our names; byte offset within a block)

| our name | offset | type | normalized use (units inferred from corpus values) |
| --- | --- | --- | --- |
| `metadata_size`, `metadata_version` | 0, 4 | int32 | block size, version (0 for FEI1, 2 for FEI2) |
| `bitmask_1` … `bitmask_4` | 8, 297, 490, 748 | uint32 LE | raw presence bits |
| `timestamp` | 12 | float64 | OLE Automation date (days since 1899-12-30) → `acquired_at` |
| `microscope_type`, `d_number` | 20, 36 | 16 chars | `instrument.model` |
| `application`, `application_version` | 52, 68 | 16 chars | `instrument.software`, `software_version` |
| `high_tension` | 84 | float64 | volts → `extra.high_tension_kv` |
| `dose` | 92 | float64 | as stored |
| `alpha_tilt`, `beta_tilt` | 100, 108 | float64 | degrees |
| `stage_x`, `stage_y`, `stage_z` | 116, 124, 132 | float64 | metres |
| `tilt_axis_angle`, `dual_axis_rotation` | 140, 148 | float64 | |
| `pixel_size_x`, `pixel_size_y` | 156, 164 | float64 | metres; `check` compares with CELLA/MX |
| `defocus`, `stem_defocus`, `applied_defocus` | 220, 228, 236 | float64 | metres |
| `instrument_mode`, `projection_mode` | 244, 248 | int32 | |
| `objective_lens_mode`, `high_magnification_mode` | 252, 268 | 16 chars | |
| `probe_mode` | 284 | int32 | |
| `eftem_on` | 288 | bool | |
| `magnification` | 289 | float64 | `extra.magnification` |
| `camera_length`, `spot_index`, `illuminated_area`, `intensity`, `convergence_angle` | 301, 309, 313, 321, 329 | f64, i32, f64, f64, f64 | |
| `illumination_mode` | 337 | 16 chars | |
| `wide_convergence_angle_range`, `slit_inserted` | 353, 354 | bool | |
| `slit_width`, `acceleration_voltage_offset`, `drift_tube_voltage`, `energy_shift` | 355, 363, 371, 379 | float64 | |
| `shift_offset_x`, `shift_offset_y`, `shift_x`, `shift_y` | 387, 395, 403, 411 | float64 | |
| `integration_time` | 419 | float64 | seconds → `channels[0].exposure_ms` |
| `binning_width`, `binning_height` | 427, 431 | int32 | |
| `camera_name` | 435 | 16 chars | `instrument.detector` (FEI1) |
| `readout_area_left`, `_top`, `_right`, `_bottom` | 451–463 | int32 | |
| `ceta_noise_reduction`, `ceta_frames_summed`, `direct_detector_electron_counting`, `direct_detector_align_frames` | 467, 468, 472, 473 | bool, i32, bool, bool | |
| `phase_plate` | 518 | bool | |
| `stem_detector_name`, `stem_gain`, `stem_offset` | 519, 535, 543 | 16 chars, f64, f64 | |
| `dwell_time`, `frame_time` | 571, 579 | float64 | |
| `scan_size_left`, `_top`, `_right`, `_bottom` | 587–599 | int32 | |
| `full_scan_fov_x`, `full_scan_fov_y` | 603, 611 | float64 | |
| `element`, `energy_interval_lower`, `energy_interval_higher`, `method` | 619, 635, 643, 651 | 16 chars, f64, f64, i32 | |
| `is_dose_fraction`, `fraction_number`, `start_frame`, `end_frame` | 655, 656, 660, 664 | bool, i32, i32, i32 | |
| `input_stack_filename` | 668 | 80 chars | |
| `alpha_tilt_min`, `alpha_tilt_max` | 752, 760 | float64 | |
| `scan_rotation`, `diffraction_pattern_rotation`, `image_rotation` | 768, 776, 784 | float64 | FEI2 |
| `scan_mode` | 792 | int32 | FEI2 |
| `acquisition_time_stamp` | 796 | int64 | FEI2; µs since 1970 → `acquired_at` when non-zero |
| `detector_commercial_name` | 804 | 16 chars | FEI2; `instrument.detector` when set |
| `start_tilt_angle`, `end_tilt_angle`, `tilt_per_image`, `tilt_speed` | 820–844 | float64 | FEI2 |
| `beam_center_x_pixel`, `beam_center_y_pixel` | 852, 856 | int32 | FEI2 |
| `cfeg_flash_timestamp` | 860 | int64 | FEI2 |
| `phase_plate_position_index`, `objective_aperture_name` | 868, 872 | int32, 16 chars | FEI2 |

The reserved camera and STEM parameters (bytes 474–489, 494–517, 551–570) and the 48 unused bytes at 172 are skipped.

## `extra` keys

`mode`, `mode_name`, `layout`, `byte_order`, `axis_order`, `pixel_size_angstrom`, `cell_angstrom`, `cell_angles_deg`, `grid_sampling`, `start_index`, `origin_angstrom`, `space_group`, `density` (`min`, `max`, `mean`, `rms`, `range_determined`, `mean_determined`, `rms_determined`), `labels`, `mrc_version`, `extended_header` (`type`, `bytes`, `decoded_as` = `none`/`fei1`/`fei2`/`serialem`/`integers-and-reals`/`symmetry`/`raw`, `records`), `imod_flags`, `bytes_signed`, `complex`, `fei_metadata`, `high_tension_kv`, `magnification`.

`vendor` (`info --view full`): `header` with the specification's names (`NX` … `LABEL`, plus `imod_stamp`/`imod_flags`) and `extended_header` (decoded records, up to 1000, or `symmetry`).

## `info --view structure`

`metadata header` (0, 1024), `metadata extended-header`, `image data` (the data block) and one `plane` entry per section (`section <n>`, with its z/t) for files with up to 4096 sections.

## `check` finding codes

| code | severity | meaning |
| --- | --- | --- |
| `truncated` | error | the file is shorter than 1024 + NSYMBT + data, or the extended header runs past the end (exit 4) |
| `bad_volume_stack` | error | ISPG 401–630 but NZ is not a multiple of MZ |
| `bad_dimensions` | error | data size overflows |
| `missing_map_id` | warning | no `MAP` at byte 209 |
| `machine_stamp`, `machine_stamp_mismatch` | warning | unknown stamp; stamp contradicts the header |
| `negative_field`, `bad_cell`, `bad_axis_map` | warning | MX/MY/MZ/ISPG/NLABL negative; CELLA negative; MAPC/MAPR/MAPS not a permutation |
| `label_count`, `label_gap` | warning | NLABL disagrees with the labels in use; an empty label before a used one |
| `unknown_version` | warning | NVERSION set but not 20140/20141 |
| `exttyp_unknown` | warning | EXTTYP not one of the registered codes |
| `fei_blocks`, `fei_block_size`, `fei_unreadable`, `pixel_size_mismatch` | warning | FEI extended header inconsistent with NZ, 768, itself, or CELLA/MX |
| `trailing_bytes` | warning | bytes after the data block |
| `nonstandard_mode`, `no_version`, `stats_undetermined`, `exttyp_unset` | info | IMOD mode 16; NVERSION 0; statistics flagged undetermined; extended header without EXTTYP |

## Observed corpus values

| file | mode | NX × NY × NZ | ISPG | MAPC/R/S | stamp | EXTTYP / NSYMBT | NVERSION |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `mrcfile-emd-3197` | 2 | 20 × 20 × 20 | 1 | 1 2 3 | 44 41 | – / 0 | 0 |
| `mrcfile-emd-3001` | 2 | 73 × 43 × 25 | 4 | 3 1 2 | 44 41 | (zeros) / 160 symmetry | 0 |
| `mrcfile-fei-extended` | 6 | 3710 × 3838 × 1 | 0 | 1 2 3 | 44 44 | FEI1 / 786432 | 20140 |
| `mrcfile-epu2.9-example` | 6 | 4096 × 4096 × 1 | 0 | 1 2 3 | 44 44 | FEI2 / 909312 | 20140 |
| `empiar10045-class2d-it025` | 2 | 200 × 200 × 20 | 0 | 1 2 3 | 44 41 | – / 0 | 0 |
| `zenodo16462008-8RRH-molmap-30A` | 2 | 39 × 39 × 28 | 0 | 1 2 3 | 44 41 | – / 0 | 0 |
| `zenodo10526574-IPPK-TS08-rec` | 0 (IMOD signed) | 274 × 477 × 67 | 1 | 1 2 3 | 44 44 | spaces / 0 | 0 |
| `zenodo2578866-HAADF-aligned` | 2 | 296 × 276 × 39 | 0 | 1 2 3 | 44 41 | – / 0 | 0 |

## Vocabulary (every public identifier in `openreadout-em/src/mrc` must appear here)

| identifier | meaning |
| --- | --- |
| `MrcReader`, `MrcDataset`, `FORMAT_ID`, `EXTENSIONS` | format reader, opened file (core `Dataset`), the id `mrc`, extensions MRC-family files use |
| `open`, `header`, `parse` | open a file; its decoded header; decode 1024 header bytes |
| `MrcHeader` and its fields `nx`, `ny`, `nz`, `mode`, `nxstart`, `nystart`, `nzstart`, `mx`, `my`, `mz`, `cell_a`, `cell_b`, `mapc`, `mapr`, `maps`, `dmin`, `dmax`, `dmean`, `ispg`, `nsymbt`, `exttyp`, `nversion`, `origin`, `has_map_id`, `machine_stamp`, `little_endian`, `stamp_problem`, `rms`, `nlabl`, `labels`, `imod_stamp`, `imod_flags`, `nint`, `nreal` | the main header, see the table above |
| `HEADER_LEN`, `IMOD_STAMP`, `FEI1_BLOCK_LEN`, `FEI2_BLOCK_LEN` | 1024; the IMOD stamp value; FEI block sizes |
| `Mode` (`Int8`, `Int16`, `Float32`, `ComplexInt16`, `ComplexFloat32`, `Uint16`, `Float16`, `Rgb8`, `Packed4Bit`, `Unknown`), `from_code`, `code`, `label`, `stored_bytes`, `in_spec` | MODE and its helpers |
| `Layout` (`ImageStack`, `Volume`, `VolumeStack`), `layout` | section organisation |
| `bytes_unsigned`, `section_bytes`, `data_offset`, `axis_map`, `cell_step_angstrom`, `file_axis_steps_angstrom`, `stats_undetermined` | header-derived geometry and flags |
| `looks_like_mrc`, `has_map_id` | detection helpers |
