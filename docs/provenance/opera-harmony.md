# Provenance log — Revvity/PerkinElmer Harmony exports (`opera-harmony`)

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

## 2026-09-23 — reader (Richard Zimring with Claude as assistant)

**Prior art read (permissive):** none for this format. The index is a self-describing XML document: every element name used by the reader was read from the corpus files themselves. The TIFF plane files are read with our own TIFF crate (`docs/formats/tiff.md`, `docs/provenance/tiff.md`).

**Bio-Formats as a black box:** Bio-Formats 8.5.0 `showinf -nopix -omexml` and `bfconvert` (oracle/bftools) were run on each corpus index. Observed: one series per (well, field) — 648 for idr0034 (72 wells x 9 fields), including fields whose files are absent ("Could not find valid TIFF file for series N"), which it reads as blank planes; channels = the index's distinct `ChannelID`s; width/height from the plane files.

**Corpus files used** (`corpus/manifest.toml`, ids `hcs-harmony-*`):
- `hcs-harmony-zenodo7841360-*` — CC-BY-4.0 (OME sample images `PerkinElmer-Operetta/zenodo-7841360`, `readme.txt` and `COPYING` checked; Zenodo record 7841360 by Alex Herbert). Namespace `…/HarmonyV5`, `InstrumentType` Sonata, one well (row 3, column 7), one field, one channel, 1080 x 1080 uint16.
- `hcs-harmony-idr0034-*` — CC-BY-4.0 (OME sample images `PerkinElmer-Operetta/idr0034`, `readme.txt` checked; IDR study idr0034-kilpinen-hipsci, study file licence CC BY 4.0). Namespace `…/HarmonyV4`, Operetta, 96-well plate, 72 wells x 9 fields x 4 channels, 1360 x 1024. Partial copy: wells r01c01 (fields 1–2) and r03c07 (every file the server holds).
- `hcs-harmony-jump-br00117035-*` — CC0-1.0 (Cell Painting Gallery, `cpg0016-jump/source_4`, AWS Open Data registry licence checked). Namespace `…/HarmonyV5`, Phenix, 384-well plate, 384 wells x 9 fields x 8 channels (27,648 planes; the index is 47 MB). Partial copy: well r01c01 fields 1–2.

**Observed (the XML text of the three indexes):**
- Root `EvaluationInputData` (attribute `Version="1"`) in a namespace whose last path segment names the software generation (`HarmonyV4`, `HarmonyV5`); a UTF-8 byte-order mark precedes the declaration.
- Children in order: `User`, `InstrumentType`, `Plates/Plate` (`PlateID`, `MeasurementID`, `MeasurementStartTime`, `Name`, `PlateTypeName`, `PlateRows`, `PlateColumns`, then one empty `Well id="RRCC"` per selected well), `Wells/Well` (`id`, `Row`, `Col`, then one `Image id=…` reference per plane), `Maps/Map/Entry ChannelID=…` (a `FlatfieldProfile` text in a brace syntax, and in the Phenix index a second map with base64 `SkewcropParameters`), and `Images/Image Version="1"`, one per plane.
- Each `Image`: `id` (`RRCCK1F<field>P<plane>R<channel>`), `State` (`Ok` in every corpus plane), `URL` (the TIFF file name relative to the index; **empty in 19 planes of idr0034 well r03c07**), `Row`, `Col` (1-based), `FieldID`, `PlaneID` (1-based), `TimepointID` (0-based), `ChannelID` (1-based), `FlimID`, `ChannelName`, `ImageType`, `AcquisitionType`, `IlluminationType`, `ChannelType`, `ImageResolutionX/Y` (attribute `Unit="m"`), `ImageSizeX/Y`, `BinningX/Y`, `MaxIntensity`, `CameraType`, `PositionX/Y/Z`, `AbsPositionZ` (m), `MeasurementTimeOffset` (s), `AbsTime` (ISO 8601 with offset), `MainExcitationWavelength`, `MainEmissionWavelength` (nm; 0 for brightfield emission), `ObjectiveMagnification`, `ObjectiveNA`, `ExposureTime` (s), `OrientationMatrix`.
- File names follow `r<RR>c<CC>f<FF>p<PP>-ch<C>sk<T+1>fk1fl1.tiff`; every corpus file named in an index is a single-page, uncompressed, little-endian uint16 TIFF whose size equals `ImageSizeX/Y`.
- idr0034: 2,588 planes listed; 19 with an empty `URL` (well r03c07, fields 6–9 and part of 5); 2 named files (`r03c07f05p01-ch3/ch4`) are absent from the upstream copy; 4 files on the server (`r07c02f07p01-ch1…4`) are not named by the index.

**Inferred (flagged in the format notes):** that `PositionX/Y` are the field's offset from the well centre (they repeat across wells); that an empty `URL` means the instrument recorded the plane without an image (acquisition skipped or failed); that `TimepointID` is 0-based and `PlaneID`/`FieldID`/`ChannelID` 1-based (every corpus index); the software generation from the namespace; the Z step as the difference of `PositionZ` between consecutive planes of a field.

## 2026-09-24 — Columbus exports (Richard Zimring with Claude as assistant)

**Corpus files used:** `hcs-columbus-zenodo6327496-*` — CC-BY-4.0 (OME sample images `PerkinElmer-Columbus/zenodo-6327496`, `readme.txt` and `COPYING` checked; Zenodo record 6327496 by Jens Wendt). Two exports of one 6-well plate (2 wells x 1 field x 3 Z planes x 2 channels): `tif/` with `ImageIndex.ColumbusIDX.xml` and one 6-page TIFF per well, and `flex/` with the same index naming `.flex` files.

**Observed:** `ImageIndex.ColumbusIDX.xml` has the same root element (`EvaluationInputData`, `Version="1"`) and the same `Plates`/`Wells`/`Images` structure as a Harmony `Index.idx.xml`, in the namespace `http://www.perkinelmer.com/Columbus`, on one line; `Well id` is six digits; `TimepointID` counts from 1; `ChannelColor` is an unsigned 32-bit ARGB number; `URL` carries an attribute `BufferNo` (0 to 5) and several planes name the same file: every file has six pages and `BufferNo` is the page. `MeasurementIndex.ColumbusIDX.xml` (root `ColumbusMeasurementIndex`) names the screen, plate, measurement and the index files.

**Inferred (flagged in the format notes):** `BufferNo` = zero-based TIFF page of the plane (the page count equals the number of planes naming the file, and the pages' pixels match Bio-Formats' planes); `ChannelColor` as ARGB (the brightfield channel is 4287795858 = 0xFF929292, grey).

## 2026-09-24 — Harmony 6/7 `Index.xml`: channel descriptions in `Maps` (N-M2 of the second generalization report)

**Problem:** on a Harmony V6 export (`Images/Index.xml`), the plate layout and pixels were exact, but channel names and pixel size were missing.

**Corpus files used** (new; none from the held-out Cell Painting Gallery datasets cpg0002/cpg0036 or BioImage Archive S-BIAD2152):
- `hcs-harmony6-cpg0048-41005680-index` + parts: Cell Painting Gallery cpg0048-bsc, plate 41005680. HarmonyV6 namespace, `InstrumentType` Sonata (Operetta CLS), 384 wells × 15 fields × 6 channels. `Index.xml` lies next to the TIFFs, with no `Images/` folder. Partial copy: well A01 field 1, all channels.
- `hcs-harmony7-cpg0047-br00143056-index` + parts: Cell Painting Gallery cpg0047-amish, plate BR00143056. HarmonyV7 in a namespace that is a GUID path (`43B2A954-…/HarmonyV7`), Phenix, 216 wells × 9 fields × 6 channels. Partial copy: well C03 field 1, all channels.

Both are CC0 1.0 (registry.opendata.aws/cellpainting-gallery and the cellpainting-gallery README, checked 2026-09-24).

**Prior art consulted:** none. Bio-Formats 8.5.0 was run as a black box only (`showinf -nopix -omexml`), as a second opinion on channel names and physical pixel size.

**Observed:**
- In both files, `Image` records carry only `id`, `State`, `URL`, `Row`, `Col`, `FieldID`, `PlaneID`, `TimepointID`, `SequenceID`, `GroupID`, `ChannelID`, `FlimID`, positions and times.
- The channel description sits once per channel in `Maps/Map/Entry[@ChannelID]`. That covers `ChannelName`, `ImageType`, `AcquisitionType`, `IlluminationType`, `ChannelType`, `ImageResolutionX/Y` (`Unit="m"`), `ImageSizeX/Y`, `BinningX/Y`, `MaxIntensity`, `CameraType`, `MainExcitationWavelength`/`MainEmissionWavelength` (nm), `ObjectiveMagnification`, `ObjectiveNA`, `ExposureTime` (s), `ExcitationPower`, `OrientationMatrix` and `CropArea`. These are the same element names older indexes put in every `Image`.
- The `Map` that holds them is not at a fixed position. In cpg0048 it is the second (the first holds `FlatfieldProfile`); in cpg0047 it is the third (after `FlatfieldProfile` and `SkewcropParameters`), and the flat-field entries are out of ChannelID order.

**Inferred:**
- A `Map/Entry` field describes every `Image` of that ChannelID that does not carry the field itself; an `Image`'s own value wins. The Map is found by content (an `Entry` holding such fields), never by index.
- A namespace ending in `/HarmonyV<n>` identifies a Harmony index even when it is not a PerkinElmer URL.

## 2026-09-25 — assurance profile (`src/assurance.rs`)

**Corpus files:** every development input of this format on disk (`corpus/assurance/evidence.json` lists them); no held-out file.
**Prior art consulted:** none.
**What was done.** No parsing logic changed. The reader declares an assurance profile (docs/assurance.md) that fingerprints each file from this reader's own normalized output: the index generation, the writer, missing and unindexed plane files, and the flat-field profiles (reported as available, not applied). The table of validated feature values and the confidence level are generated from the development corpus by `cargo xtask assurance-audit`.

## 2026-09-25 — Standalone Opera `.flex` files and measurement folders

**Corpus files used:** `hcs-flex-idr0001-*` (OME sample images `Flex/idr0001`, CC-BY-4.0, Graml et al., IDR idr0001: two wells of plate X_110331_S13, FLEX 1.8.1.0 from an OPERA5013) and the two `.flex` files of `hcs-columbus-zenodo6327496-flex-*` (Zenodo 6327496, CC-BY-4.0, Jens Wendt; a different Opera generation), opened on their own.

**Prior art consulted:** none. Bio-Formats 8.5.0 `showinf`/`bfconvert` run as a black box (GPL); tifffile (BSD-3) run as a tool to list the TIFF tags (it names tag 65200 `FlexXML`).

**Observed (tifffile tag listing, the XML text itself):** a `.flex` file is a little-endian multi-page TIFF (uncompressed 16-bit pages here) whose first IFD carries tag 65200, an XML document `<Root>` with `Arrays/Array` (one per page, in page order: `Name` e.g. `Exp1Cam1`, `Width`, `Height`, `BitsPerPixel`) and `FLEX` (`@version`, `@OperaDevice`) holding `LightSources/LightSource` (`@ID`, `Wavelength` nm), `Cameras/Camera` (`PixelSizeX/Y`, the sensor pixel), `Objectives/Objective` (`@ID`, `Magnification`, `NumAperture`, `Immersion` — a refractive index: 1.00 air, 1.33 water), `Sublayouts/Sublayout/Field@No` (`OffsetX/Y` m), `Stacks/Stack/Plane@No` (`OffsetZ` m), `LightSourceCombinations` (`@ID` → `LightSourceRef@ID` with `Power` W), `Plate` (`PlateName`, `XSize` = rows 8, `YSize` = columns 12 of a 96-well plate, `Barcode`, `StartTime`), and `Well` (`WellCoordinate@Row`/`@Col`, 1-based) with `Images/Image@BufferNo` per page: `ObjectiveRef`, `LightSourceCombinationRef`, `ExposureNo`, `CameraExposureTime` (s), `CameraBinningX/Y`, `Stack` (Z plane number), `Sublayout` (field number), `PositionX/Y/Z` (m), `DateTime`, `ImageResolutionX/Y` (m per pixel), `ImageWidth`/`Height`/`BitPerPixel`.

**Inferred:** `BufferNo` = the zero-based page of the file (the `Arrays` list has one entry per page in the same order, and every page's size equals its `Image`'s width × height); one channel per `ExposureNo` (Bio-Formats reports 2 channels, named after the page's `Array@Name`, which we use too); Z = `Stack`, field = `Sublayout`; the channel's excitation is the wavelength of the single light source its `LightSourceCombinationRef` lists. A measurement folder (`Meas_NN(date)`) of `.flex` files is one plate; one `.flex` file opened on its own is that one well.
