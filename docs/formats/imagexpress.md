# Molecular Devices ImageXpress / MetaXpress plates

Molecular Devices ImageXpress screening systems, run by MetaXpress (earlier MetaMorph), save a plate as a folder of TIFF images, usually with an `.HTD` plate description. OpenReadout reads the plate into the shared plate model ([hcs.md](hcs.md)): wells, sites, channels, time points and Z planes with their pixel data and calibration. Derived from public plates written by MetaMorph 6.2 and MetaXpress 6.6, checked against the HTD parsed as text, tifffile on every plane file, and Bio-Formats 8.5.0 run as a black box. Format id `imagexpress`, crate `openreadout-hcs` (module `imagexpress`). Provenance: `docs/provenance/imagexpress.md`. Every name below is ours.

A MetaXpress plate is a folder of TIFFs named `<plate>_<well>[_s<site>][_w<wave>][<GUID>].tif` (thumbnails add `_thumb`), usually with an `.HTD` plate description; time points and Z planes may sit in `TimePoint_<t>/` and `ZStep_<z>/` sub-folders. Open the `.HTD` or the folder. A folder without HTD is read from the file names alone: its wells, sites and wavelengths are what is on disk, so missing planes cannot be detected.

**Detection.** A folder holding an HTD (or whose `TimePoint_1/` holds one) is an ImageXpress plate (definite). A folder without HTD is claimed (likely) only on positive evidence: (1) the folder is not one another reader owns, by its name (`.oif.files`, `.oib.files`, `*.files`, `.d`/`.D`, `.raw`, `.fid`, `.zarr`, `.wiff.scan`) or by a marker file in it (a MetaMorph `.nd`, an Olympus `.oif`/`.oib`/`.oir`, Micro-Manager `metadata.txt`/`*_metadata.txt`/`DisplaySettings.json`, a CellVoyager `MeasurementData.mlf`, a Harmony `Index.*.xml`, `.companion.ome`, `.xdce`, `.vsi`); (2) at least two non-thumbnail plane files share one plate prefix and spell the well as MetaXpress does (row letters and a two-digit column: `A01`, not `C001`); (3) the first of them carries MetaMorph metadata (MetaSeries `<MetaData>` XML in its ImageDescription, or the STK `UIC1` tag). Olympus FluoView `s_C001.tif` channel files, for example, parse as prefix `s` and well `C1` but fail all three. The corpus test `detect_matches_manifest` (`crates/openreadout-corpus-tests/tests/detect.rs`) checks every folder of the development corpus.

## HTD (text: quoted key, comma-separated values; `"HTSInfoFile", Version 1.0` first, `"EndFile"` last)

| key | our use |
| --- | --- |
| `HTSInfoFile` | `format_version` (`HTSInfoFile 1.0`) |
| `Description` | plate `name` / `description` (bytes decoded as UTF-8, invalid bytes replaced) |
| `PlateType` | `plate.extra.plate_type_code` (a number; meaning not known) |
| `XWells`, `YWells` | `columns`, `rows` |
| `WellsSelection<row>` | `TRUE`/`FALSE` per column: the wells expected (`declared_wells`) |
| `Sites`, `XSites`, `YSites`, `SiteSelection<row>` | sites per well: the number of `TRUE` cells, numbered 1.. in reading order (inferred from files named `_s1`…`_s4` for 4 selected cells); `plate.extra.sites`, `site_grid` |
| `Waves`, `NWavelengths`, `WaveName<w>`, `WaveCollect<w>` | channels: wavelengths 1..N, named by `WaveName`, left out when `WaveCollect` is 0 |
| `TimePoints` | T |
| `ZSeries`, `ZSteps`, `ZProjection` | Z (`ZStep_<z>` folders); `plate.extra.z_projection` |
| `UniquePlateIdentifier` | `plate.extra.unique_plate_identifier` |

Every expected plane (selected wells x sites x collected waves x Z x T) whose file is not found is `missing`; files outside the expectation are included with a note.

## Plane files

Parsed right to left: extension, then a trailing GUID (36 characters, or `_[GUID]`), `_thumb`, `_w<n>`, `_s<n>`, `_<well>`; the rest is the plate prefix (the HTD's file stem; without HTD, the most common prefix). One plane file per wavelength is opened in `info` for its MetaMorph metadata (the TIFF crate decodes both conventions, `docs/formats/tiff.md` § MetaMorph):

| source (MetaSeries XML / STK tags) | our field |
| --- | --- |
| `_IllumSetting_` / `image-name` / STK name | channel name when the HTD has no `WaveName`; `plate.extra.channels[].illumination_setting` |
| `wavelength` / STK wavelength | `emission_nm` (the recorded wavelength; which side of the filter it describes is not documented: inferred) |
| `spatial-calibration-x/y` (µm, when calibration is on) / STK calibration in `um` | `physical_size` |
| `_MagSetting_`, `_MagNA_` | `objective.model` (`20X Plan Apo Lambda`), `nominal_magnification` (the leading number of the setting), `lens_na` |
| `Exposure: <n> ms` in the description | `exposure_ms` |
| `stage-position-x/y`, `z-position` / STK stage and absolute Z | `frames[].stage_*_um` (read per plane by `frames`) |
| `acquisition-time-local` | `frames[].acquired_at` (local time, no zone) |
| `ApplicationName`/`ApplicationVersion`, `Software Version:` line, TIFF `Software` | `instrument.software` |
| `Barcode:` line | plate `id` (else the HTD stem, else the file prefix) |
| `Plate Name:` line | plate `name` |
| `Instrument Serial Number` | `plate.extra.instrument_serial`, `experiment.instrument.serial` |

## Vocabulary (module `imagexpress`)

| our name | meaning |
| --- | --- |
| `IMAGEXPRESS_FORMAT_ID` | `imagexpress` |
| `looks_like_htd`, `find_htd` | detection; the HTD of a folder (or of its `TimePoint_1/`) |
| `plate_folder_without_htd` | detection of a plate folder without HTD (see § Detection); the number of plane files seen |
| `Htd` (`entries`, `get`, `values`, `uint`, `flag`), `parse_htd` | the parsed HTD: `(key, values)` in file order |
| `IxName` (`prefix`, `row`, `column`, `site`, `wave`, `thumb`), `parse_name` | a plane file name taken apart |
| `plane_meta` | per-plane time, stage position and exposure of one plane file (for `frames`) |
| `PlaneMeta` | what `plane_meta` returns: acquisition time, stage X/Y/Z (µm), exposure (ms) |
| `parse` | an HTD or a folder into an `HcsPlate` |

## Validation (2026-09-24)

| corpus id | plate | ours vs oracle |
| --- | --- | --- |
| `hcs-imagexpress-idr0081-htd` | 384 wells, no sites, 2 wavelengths (DAPI, FITC), 2048 x 2048, 1.72 µm; partial copy | 3 planes bit-exact vs tifffile; 765 missing reported; Bio-Formats 3/3 planes equal, 384 series mapped |
| `hcs-imagexpress-jump-a1170383-folder` | folder without HTD, well A01, 2 sites, 5 wavelengths (Cy5, Texas Red, Cy3, FITC, DAPI), 0.7032 µm | 10 planes bit-exact vs tifffile; channel names and pixel size equal to tifffile's MetaSeries parse; no Bio-Formats comparison (it needs the HTD) |
