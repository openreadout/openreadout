# SerialEM metadata files (`mdoc`)

SerialEM writes an `.mdoc` text file next to each image stack, montage or movie it saves (`stack.mrc.mdoc`, `movie.tif.mdoc`): file-wide `key = value` lines, `[T = ...]` title lines, then one `[Kind = N]` section per image (`ZValue` for the sections of a stack or tilt series, `FrameSet` for a movie, `MontSection` for montage overviews). Derived from the SerialEM documentation of the format (https://bio3d.colorado.edu/SerialEM/hlp/html/about_formats.htm, public) and mdocfile (BSD-3, teamtomo) read as documentation and run as the oracle. Provenance: `docs/provenance/mdoc.md`. Crate: `openreadout-em` (`src/mdoc/`).

## Two ways in

- **On its own**: `info` shows one table, `sections`, a row per section, with the columns below (only those some row fills), `tables[0].extra.mdoc` (a summary) and `tables[0].extra.files` (the image file, each section's sub-frame file, gain references and defect files, and whether each is next to the mdoc or in a `frames`/`Frames` folder there). `openreadout table` reads the rows.
- **Next to an MRC stack** (`<name>.mdoc`, as SerialEM names it): the MRC reader adds the summary to `images[0].extra.mdoc` and the section values to the frame records of the matching section (`[ZValue = N]` → section N), so `plot` with `show: frames` draws tilt angle, dose or defocus per section.

## Section records

| mdoc key | ours |
| --- | --- |
| `TiltAngle` | `tilt_angle_deg` |
| `ExposureDose` (e/Å²) | `exposure_dose_e_per_a2` |
| `ExposureTime` | `exposure_time_s` |
| `Defocus`, `TargetDefocus` (µm) | `defocus_um`, `target_defocus_um` |
| `Magnification`, `PixelSpacing` (Å) | `magnification`, `pixel_spacing_a` |
| `StagePosition`, `StageZ` | `stage_x_um`, `stage_y_um`, `stage_z_um` |
| `PieceCoordinates` | `piece_x`, `piece_y` |
| `ImageShift`, `RotationAngle`, `Intensity`, `SpotSize`, `Binning`, `CountsPerElectron`, `DoseRate`, `PriorRecordDose` | `image_shift`, `rotation_angle_deg`, `intensity`, `spot_size`, `binning`, `counts_per_electron`, `dose_rate`, `prior_record_dose_e_per_a2` |
| `MinMaxMean` | `min`, `max`, `mean` |
| `NumSubFrames`, `FrameDosesAndNumber` | `subframes`, `frame_dose_e_per_a2` |
| `SubFramePath`, `GainReference`, `DefectFile` | `subframe_file`, `gain_reference`, `defect_file` (file names: SerialEM writes Windows paths) |
| `DateTime` (`11-Oct-19  14:25:01`) | `acquired_at` (ISO 8601, no zone) |

When every section has a time, they are put in acquisition order (`acquisition_order`) and the dose is accumulated in that order: `prior_dose_e_per_a2` (dose before the section) and `cumulative_dose_e_per_a2` (including it). A dose-symmetric tilt series shows its order this way. The summary has `sections`, `section_kind`, `tilt_range_deg`, `total_dose_e_per_a2`, `tilt_axis_deg` (from the `Tilt axis angle = ` title), `pixel_spacing_a`, `voltage_kv` and `image_file`. The experiment names SerialEM, the microscope from the `Digitized on …` title, voltage, pixel spacing, tilt axis and total dose.

## `check`

| code | severity | meaning |
| --- | --- | --- |
| `repeated_section` | warning | a `[Kind = N]` section appears twice |
| `files_not_found` | info | named files are not next to the mdoc |

## Vocabulary

| identifier | meaning |
| --- | --- |
| `MdocReader`, `MdocDataset`, `FORMAT_ID`, `MAX_SECTIONS`, `open`, `mdoc` | reader, opened file, format id `mdoc`, section limit, the parsed file |
| `Mdoc`, `parse`, `globals`, `titles`, `sections`, `global`, `tilt_axis_deg`, `acquisition_order`, `records`, `summary` | the parsed file and what is derived from it |
| `MdocSection`, `kind`, `index`, `fields`, `get` | one section |
| `file_name`, `iso_datetime`, `sidecar_of`, `read` | helpers: a Windows path's file name, SerialEM dates, the mdoc next to an image file |
