# Revvity/PerkinElmer Harmony exports (Opera Phenix, Operetta)

Harmony, the software of the Opera Phenix and Operetta high-content screening systems, exports a measurement as an XML index plus one TIFF per plane. OpenReadout reads the index and returns the plate with one image per field of view, each with the plate's channels, Z planes and time points. Columbus exports and standalone Opera `.flex` files are read too. The shared plate model is described in [hcs.md](hcs.md).

Derived from three public Harmony indexes (V4 from an Operetta, V5 from a Phenix and a Sonata). The results were checked against the same indexes parsed with the Python standard library, against tifffile on every plane file and against Bio-Formats 8.5.0 run as a black box. No Revvity/PerkinElmer document, schema or software was used; every name below is ours. Format id `opera-harmony`, crate `openreadout-hcs` (module `harmony`). Provenance: `docs/provenance/opera-harmony.md`.

A Harmony export is a measurement folder (`<plate>__<date>-Measurement<n>/`) whose `Images/` folder holds `Index.idx.xml` (Harmony 6 and 7: `Index.xml`) and one TIFF per plane; some exports put the index and the TIFFs directly in the plate folder. Open the measurement folder, the `Images` folder or the index (`Index.idx.xml`; `Index.xml` and `Index.ref.xml` with the same root element are accepted).

**Columbus exports** use the same index structure in the namespace `http://www.perkinelmer.com/Columbus` (`ImageIndex.ColumbusIDX.xml`, next to `MeasurementIndex.ColumbusIDX.xml`): the reader handles them too, with three differences: `URL` has an attribute `BufferNo`, the zero-based **page** of a multi-page TIFF or Opera `.flex` file that holds several planes (`PlaneFile.page`); `ChannelColor` is a 32-bit ARGB number (`channels[].color` `#RRGGBB`); `Well id` is an internal number, not `RRCC`. `format_version` and `instrument.software` are `Columbus`.

**Standalone Opera `.flex` files** (module `flex`): an Opera writes one multi-page TIFF per well, `<row><col><...>.flex` (`001002000.flex` = row 1, column 2), whose first IFD carries tag 65200 (`TAG_FLEX_XML`): an XML document `Root` with `Arrays/Array` (one per page, in page order; `@Name` such as `Exp1Cam1`, used as the channel name) and `FLEX` (`@version` → `format_version` `FLEX 1.8.1.0`; `@OperaDevice` → `instrument.model`) holding the light sources (`LightSource@ID`, `Wavelength` nm), light-source combinations, objectives (`Magnification`, `NumAperture`, `Immersion` = the immersion's refractive index → `plate.extra.objective_immersion_refractive_index`), sublayouts (field offsets), stacks (Z offsets), the plate (`PlateName` → plate type, `XSize` rows, `YSize` columns, `Barcode` → plate id, `StartTime`) and the `Well` (`WellCoordinate@Row/@Col`, 1-based) with one `Images/Image@BufferNo` per page: `BufferNo` = zero-based page, `ExposureNo` = channel, `Stack` = Z plane, `Sublayout` = field, `CameraExposureTime` (s), `PositionX/Y/Z` (m), `DateTime`, `ImageResolutionX/Y` (m per pixel), `LightSourceCombinationRef` (a single light source's wavelength → the channel's `excitation_nm`). Open a `.flex` file (that one well) or the measurement folder of `.flex` files (the plate, `flex_files`). Validated on an OPERA5013 (FLEX 1.8.1.0, idr0001; 64 planes equal to tifffile and to Bio-Formats) and a later Opera's files written with a Columbus export (FLEX 1.8.1.1). Not validated: time series (`Kinetic`), compressed pages.

## Detection

An `.xml` file whose first 4 KB hold the root element `EvaluationInputData` and the namespace segment `PEHH` or `perkinelmer.com/Columbus` (definite); a folder with such an index directly inside or in `Images/`; a TIFF with the `.flex` extension (`is_flex_name`), or a folder of them without a Harmony/Columbus index.

## The index (XML, streamed)

| element (path) | our field | notes |
| --- | --- | --- |
| root namespace `…/PEHH/HarmonyV<n>` (Harmony 7: a GUID path `43B2A954-…/HarmonyV7`) | `format_version` (`HarmonyV5`), `instrument.software` `Harmony`, `software_version` `5` | manufacturer `PerkinElmer` when the namespace names it |
| `User` | `operator` | |
| `InstrumentType` | `instrument.model` (`Operetta`, `Phenix`, `Sonata`) | |
| `Plates/Plate/PlateID` | plate `id`, `sample.id`, `sample.barcode` | |
| `Plate/Name` | plate `name` when it differs from the id | |
| `Plate/MeasurementID` | `plate.extra.measurement_id` | |
| `Plate/MeasurementStartTime` | `started_at` | |
| `Plate/PlateTypeName` | `plate_type` | |
| `Plate/PlateRows`, `PlateColumns` | `rows`, `columns` | the standard plate that holds every record when absent or too small |
| `Plate/Well id="RRCC"` | `declared_wells` | selected wells; those without records are listed in `plate.extra.selected_wells_without_images` |
| `Maps/Map/Entry ChannelID/FlatfieldProfile` | `info --view full` only | flat-field profile text per channel; not applied |
| `Maps/Map/Entry ChannelID/{ChannelName, ImageResolutionX/Y, ImageSizeX/Y, Main*Wavelength, Objective*, ExposureTime, …}` | the channel's description, for every `Image` of that ChannelID that does not carry the field itself | Harmony 6/7 `Index.xml` keeps these only here (in whichever `Map` holds them: the 2nd or 3rd in the corpus); an `Image`'s own value wins; a note says when they were used |
| `Images/Image` | one plane record | fields below |
| `Image/URL` (Columbus: `@BufferNo` = page) | `PlaneFile.name`, `PlaneFile.page` | empty → `not_recorded`; absolute paths and URLs keep their last component |
| `Image/Row`, `Col` | well (1-based) | |
| `Image/FieldID` | `field` | |
| `Image/PlaneID` | Z (sorted) | |
| `Image/TimepointID` | T (sorted) | |
| `Image/ChannelID` | C (sorted) | `plate.extra.channels[].channel_id` |
| `Image/ChannelName` | `channels[].name` | from the channel's first record |
| `Image/ChannelType` | `channels[].acquisition_mode` | `Fluorescence`, `Brightfield` |
| `Image/AcquisitionType`, `IlluminationType`, `ImageType`, `BinningX`, `CameraType`, `MaxIntensity` | `plate.extra.channels[]`: `acquisition_type`, `illumination_type`, `image_type`, `binning`, `camera`, `max_intensity` | `CameraType` is also `instrument.detector` |
| `Image/MainExcitationWavelength`, `MainEmissionWavelength` | `excitation_nm`, `emission_nm` | 0 (brightfield emission) → absent |
| `Image/ExposureTime` (`Unit="s"`) | `exposure_ms` | per channel and per plane (`frames`) |
| `Image/ImageResolutionX/Y` (`Unit="m"`) | `physical_size.x/y` (µm) | |
| `Image/ImageSizeX/Y` | `size_x`, `size_y` | confirmed by a plane file header |
| `Image/PositionX/Y` (`Unit="m"`) | `extra.position_x_um/y_um`, `frames[].stage_x_um/y_um` | offset from the well centre (inferred: repeats across wells) |
| `Image/PositionZ` | `frames[].stage_z_um`; Z step = difference of the first two planes | |
| `Image/AbsTime` | `frames[].acquired_at`; field `acquired_at` = earliest; `ended_at` = latest | |
| `Image/MeasurementTimeOffset` | `time_increment_s` (difference of the first two time points) | |
| `Image/ObjectiveMagnification`, `ObjectiveNA` | `objective.nominal_magnification`, `lens_na` | |
| `Image/FlimID` | not a dimension | a note when several values occur |
| `Image/State`, `OrientationMatrix`, `AbsPositionZ` | `info --view full` only | |

Plane files are named `r<RR>c<CC>f<FF>p<PP>-ch<C>sk<T>fk1fl1.tiff`; files of that shape in the folder that the index does not name are reported (`unindexed_plane_files`).

## Vocabulary (module `harmony`)

| our name | meaning |
| --- | --- |
| `HARMONY_FORMAT_ID` | `opera-harmony` |
| `HARMONY_INDEX_NAMES` | `Index.idx.xml`, `Index.xml`, `Index.ref.xml`, `ImageIndex.ColumbusIDX.xml` (search order in a folder) |
| `looks_like_harmony` | detection on the first bytes |
| `find_index` | the index of a measurement folder (itself or its `Images/`) |
| `parse` | stream the index into an `HcsPlate` |
| `is_plane_name` | the `r..c..f..p..-ch..` file-name shape |
| `TAG_FLEX_XML`, `is_flex_name`, `flex_files`, `flex_xml` (module `flex`) | tag 65200; a `.flex` file name; the `.flex` files of a folder; the XML of a file |

## Validation (2026-09-24)

| corpus id | plate | ours vs oracle |
| --- | --- | --- |
| `hcs-harmony-zenodo7841360-index` | V5 Sonata, 1 well, 1 field, 1 channel | 1 plane bit-exact vs tifffile; Bio-Formats 1/1 |
| `hcs-harmony-idr0034-index` | V4 Operetta, 72 wells, 647 fields x 4 channels; partial copy | 26 planes bit-exact; 2,543 missing files reported (exit 5); 19 not-recorded planes blank; Bio-Formats 26/26 planes equal (it lists 648 series: one grid position has no record in the index) |
| `hcs-columbus-zenodo6327496-tif-index` | Columbus, 6-well plate, 2 wells x 1 field x 3 Z x 2 channels, 6-page LZW TIFF per well | 12 planes bit-exact vs tifffile (page = `BufferNo`); Bio-Formats 12/12 |
| `hcs-columbus-zenodo6327496-flex-index` | the same plate with Opera `.flex` files | 12 planes bit-exact vs tifffile; Bio-Formats 12/12; OME-Zarr plate export valid NGFF 0.5 (ome-zarr-models), 12/12 planes equal |
| `hcs-harmony-jump-br00117035-index` | V5 Phenix, 384 wells x 9 fields x 8 channels (47 MB index); partial copy | 16 planes bit-exact; 27,632 missing reported; Bio-Formats 16/16, 3,456 series mapped |

Channel names, plate geometry, pixel sizes and the well/field of every image agree with the stdlib parse and Bio-Formats' OME Plate.
