# Yokogawa CellVoyager (CV7000, CV8000, CQ1)

Yokogawa CellVoyager high-content screening systems save each plate measurement as a folder of TIFF images with XML description files. OpenReadout returns the plate, its wells and fields, and the images with their channels, Z planes and time points.

Derived from corpus measurements (a CV8000 Cell Painting plate with a brightfield Z stack, and a CV7000 plate of maximum projections), validated against the measurement files parsed with the Python standard library, tifffile on every plane file and Bio-Formats 8.5.0 (black box). No Yokogawa document, schema or software was used; every name below is ours. Format id `cellvoyager`, crate `openreadout-hcs` (module `cellvoyager`). Shared plate model: [hcs.md](hcs.md). Provenance: `docs/provenance/cellvoyager.md`.

A measurement folder holds `MeasurementData.mlf` (one record per image), `MeasurementDetail.mrf` (plate and channel geometry), the measurement setting (`.mes`, named by the `.mrf`), the plate files (`.wpi`, `.wpp`), correction files and the TIFFs `<plate>_<well>_T<tttt>F<fff>L<ll>A<aa>Z<zz>C<cc>.tif`. Open the folder, the `.mlf`, or the `.mrf`/`.wpi`/`.mes` next to it.

## Files read

| file | element / attribute | our field |
| --- | --- | --- |
| `.mrf` root `MeasurementDetail` | `Version` | `format_version` (`MeasurementDetail 1.0`) |
| | `OperatorName` | `operator` |
| | `Title` | plate `name` (when it differs from the id); `plate.extra.title` |
| | `Application` | `description`, `experiment.acquisition.comment` |
| | `BeginTime`, `EndTime` | `started_at`, `ended_at` |
| | `MeasurementSettingFileName` | the `.mes` read; `method` = its stem |
| | `RowCount`, `ColumnCount` | `rows`, `columns` |
| | `FieldCount`, `ZCount`, `TimePointCount`, `Status` | `plate.extra.field_count`, `z_count`, `time_point_count`, `status` |
| | `TargetSystem`, `ReleaseNumber` | `instrument.model` (`CV8000 AZ01`), `software_version` |
| `.mrf` `MeasurementSamplePlate` | `Name` | plate `id` (barcode), `sample.id` |
| | `WellPlateFileName`, `WellPlateProductFileName` | the `.wpi` and `.wpp` read |
| `.mrf` `MeasurementChannel` | `Ch` | channel order |
| | `HorizontalPixels`, `VerticalPixels` | `size_x`, `size_y` (first channel; a note when channels differ) |
| | `HorizontalPixelDimension`, `VerticalPixelDimension` | `physical_size` (µm) |
| | `InputBitDepth` | sample type (16 → uint16, confirmed by a plane file header) |
| | `CameraNumber`, `InputLevel`, `ShadingCorrectionSource`, `FilterWheelPosition`, `FilterPosition` | `plate.extra.channels[]`: `camera_number`, `input_level`, `shading_correction_source`, `filter_wheel_position`, `filter_position`; also `input_bit_depth` |
| `.mes` `ChannelList/Channel` | `Target` | channel name when every channel's target differs (`DNA`, `ER`, …); `plate.extra.channels[].target` |
| | `Acquisition` (`BP445/45`) | channel name otherwise; `emission_nm` = centre, `emission_range_nm` = centre ± width/2 (inferred from the name); `emission_filter` |
| | `LightSourceName` → `LightSourceList/LightSource WaveLength` | `excitation_nm` when the channel lists exactly one light source (a lamp's 0 → absent); `light_sources` |
| | `ExposureTime` (ms), `Kind`, `Color` (`#AARRGGBB`), `Fluorophore` | `exposure_ms`, `acquisition_mode`, `color` (`#RRGGBB`), `fluorophore` |
| | `Objective`, `Magnification` | `objective.model`, `nominal_magnification` (no NA is recorded) |
| | `Method`, `Binning`, `CameraType`, `PinholeDiameter` | `plate.extra.channels[]`: `method`, `binning`, `camera_type`, `pinhole_diameter` |
| `.wpp` `WellPlateProduct` | `Manufacturer` + `Name` | `plate_type` (`PerkinElmer CellCarrier-384-Ultra`) |
| | `ProductID`, `ColumnPitch`, `RowPitch`, `WellShape`, `BottomMaterial` | `plate.extra.plate_product_id`, `column_pitch_mm`, `row_pitch_mm`, `well_shape`, `bottom_material` |
| `.mlf` `MeasurementRecord` (streamed) | `Type` | only `IMG` records are planes; others are counted in a note |
| | `Row`, `Column` (1-based), `FieldIndex`, `ZIndex`, `TimePoint`, `Ch` | well, field, Z, T, C |
| | element text | the plane file |
| | `Time` | `frames[].acquired_at`, field `acquired_at` |
| | `X`, `Y`, `Z` (µm) | `extra.position_x_um/y_um`, `frames[].stage_*_um`; Z step = smallest Z difference between consecutive `ZIndex` of one field and channel |
| | `Action`, `ZImageProcessing` | `plate.extra.channels[].actions` (`2D`, `BF3D`, `3D`), `z_image_processing` (`Maximum`) |
| | `PartialTileIndex` | tiled fields are not stitched (note when several tiles occur) |

A channel acquired at fewer Z planes than the others (the CV8000 Cell Painting protocol: 5 fluorescence channels at one plane, brightfield at three) leaves its other planes `not_acquired`: they read as blank and are left out of statistics (Bio-Formats repeats the acquired plane there). Files with the plane-name shape that no record names (the CV7000 plate's `…C05.tif`) are reported as `unindexed_plane_files`.

## Vocabulary (module `cellvoyager`)

| our name | meaning |
| --- | --- |
| `CELLVOYAGER_FORMAT_ID` | `cellvoyager` |
| `MEASUREMENT_DATA`, `MEASUREMENT_DETAIL` | `MeasurementData.mlf`, `MeasurementDetail.mrf` |
| `looks_like_cellvoyager` | detection (the `yokogawa.co.jp/BTS` namespace and a measurement root element) |
| `find_index` | the `.mlf` of a folder |
| `parse` | the measurement into an `HcsPlate` |
| `is_plane_name` | the `_T..F..Z..C..` file-name shape |

## Validation (2026-09-24)

| corpus id | measurement | ours vs oracle |
| --- | --- | --- |
| `hcs-cellvoyager-jump-1053601756-mlf` | CV8000, 384 wells x 6 fields, 6 channels, Z 3 (brightfield only); partial copy | 16 planes bit-exact vs tifffile; 18,416 missing reported; 20 never-acquired planes blank; Bio-Formats 16/16 planes equal (it repeats the acquired plane at the 20 never-acquired positions) |
| `hcs-cellvoyager-idr0093-mlf` | CV7000, 313 wells / 2,817 fields x 4 channels, 2560 x 2160 big-endian; partial copy | 4 planes bit-exact; 11,264 missing reported; Bio-Formats 4/4 (its channel order follows the acquisition actions and is mapped by channel number) |
