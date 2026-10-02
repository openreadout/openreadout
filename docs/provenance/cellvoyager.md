# Provenance log — Yokogawa CellVoyager measurements (`cellvoyager`)

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

## 2026-09-23 — reader (Richard Zimring with Claude as assistant)

**Prior art read (permissive):** none. The measurement files are self-describing XML: every element and attribute name used by the reader was read from the corpus files. The TIFF plane files are read with our own TIFF crate.

**Bio-Formats as a black box:** Bio-Formats 8.5.0 `showinf -nopix -omexml` and `bfconvert` (oracle/bftools) were run on each corpus `.wpi`. Observed: one series per (well, field) of the wells that have files (6 for the JUMP subset: the well's 6 fields, 2 of them present); SizeC = channels of the `.mrf`, SizeZ = the largest `ZIndex`; planes that were never acquired (channels 1–5 at Z 2 and 3 in the JUMP plate) read as blank; "Found data from CV8000 AZ01; this is not well-supported".

**Corpus files used** (`corpus/manifest.toml`, ids `hcs-cellvoyager-*`):
- `hcs-cellvoyager-jump-1053601756-*` — CC0-1.0 (Cell Painting Gallery, `cpg0016-jump/source_2`). CV8000 (`TargetSystem="CV8000 AZ01"`, release R2.03.02), 384 wells x 6 fields, channels 1–5 acquired at one Z (`Action="2D"`), channel 6 at three (`Action="BF3D"`), 996 x 996 uint16, 18,432 `IMG` records. Partial copy: well A01 fields 1–2.
- `hcs-cellvoyager-idr0093-*` — CC-BY-4.0 (OME sample images `CV7000/idr0093`, `readme.txt` checked; IDR study idr0093-mueller-perturbation, study file licence CC BY 4.0). CV7000 (`CV7000 FD`, R1.17.05), 4 channels in the `.mrf` and the records, maximum projections (`ZImageProcessing="Maximum"`), 2560 x 2160 big-endian uint16; a fifth channel's files (`…C05.tif`) are present upstream but not recorded. Partial copy: well B02 field 1.
- Read for comparison only: the OME sample `CV7000/cpg0016/Dest21053D1-15214` `.mrf`/`.mlf`/`.mes` (a CV8000 plate of the same gallery with the same 2D + BF3D layout).

**Observed (XML text):**
- `MeasurementDetail.mrf`: root `bts:MeasurementDetail` with `OperatorName`, `Title`, `Application`, `BeginTime`, `EndTime`, `MeasurementSettingFileName` (the `.mes`), `ColumnCount`, `RowCount`, `TimePointCount`, `FieldCount`, `ZCount`, `TargetSystem`, `ReleaseNumber`; `MeasurementSamplePlate` (`Name`, `WellPlateFileName`, `WellPlateProductFileName`); one `MeasurementChannel` per channel (`Ch`, `HorizontalPixelDimension`/`VerticalPixelDimension` in µm, `CameraNumber`, `InputBitDepth`, `InputLevel`, `HorizontalPixels`/`VerticalPixels`, `FilterWheelPosition`, `FilterPosition`, `ShadingCorrectionSource`, `ObjectiveMagnificationRatio`, `OriginalHorizontalPixels`/`OriginalVerticalPixels`).
- `MeasurementData.mlf`: root `bts:MeasurementData`, one `MeasurementRecord` per image with attributes `Type` (`IMG` in every corpus record), `Time`, `Column`, `Row` (1-based), `TimePoint`, `FieldIndex`, `ZIndex` (1-based), `TimelineIndex`, `ActionIndex`, `Action`, `X`, `Y`, `Z` (µm), `Ch`; CV7000 adds `PartialTileIndex`, `TileXIndex`, `TileYIndex`, `ZImageProcessing`, `ZTop`, `ZBottom`; the element text is the TIFF file name.
- `.mes`: `bts:MeasurementSetting` with a `Timelapse/Timeline` (wells, fixed field positions, `ActionAcquire`/`ActionAcquireBF3D` lists), `LightSourceList/LightSource` (`Name`, `Type`, `WaveLength`, `Power`) and `ChannelList/Channel` (`Ch`, `Target`, `Objective`, `Magnification`, `Method`, `Acquisition` = the emission filter as `BP<centre>/<width>`, `ExposureTime` ms, `Binning`, `Color` `#AARRGGBB`, `Kind`, `CameraType`, `Fluorophore`, `LightSourceName` children).
- `.wpi`: `bts:WellPlate` (`Name`, `ProductID`, `Columns`, `Rows`); `.wpp`: `bts:WellPlateProduct` (`Name`, `Manufacturer`, `Columns`, `Rows`, pitches and well geometry in mm).
- File names `<plate>_<well>_T<tttt>F<fff>L<ll>A<aa>Z<zz>C<cc>.tif`; TIFFs single-page uncompressed uint16 (little-endian in the CV8000 plate, big-endian in the CV7000 plate).

**Inferred (flagged in the format notes):** the emission band from `BP<centre>/<width>`; the excitation wavelength as the channel's laser when it lists exactly one; the channel name as the `.mes` `Target` when targets are distinct (else the filter name); that `X`/`Y` are the field's offset from the well centre in µm; the handling of channels acquired at fewer Z planes than others (planes never acquired: see the format notes).

## 2026-09-25 — assurance profile (`src/assurance.rs`)

**Corpus files:** every development input of this format on disk (`corpus/assurance/evidence.json` lists them); no held-out file.
**Prior art consulted:** none.
**What was done.** No parsing logic changed. The reader declares an assurance profile (docs/assurance.md) that fingerprints each file from this reader's own normalized output: the index generation, tiled fields and non-image records. The table of validated feature values and the confidence level are generated from the development corpus by `cargo xtask assurance-audit`.
