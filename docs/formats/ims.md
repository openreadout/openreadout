# Imaris IMS (HDF5)

Imaris 5 `.ims` files are HDF5 files written by Imaris and by Andor Fusion. OpenReadout returns the image at every pyramid level, time point and channel (one plane per Z) with physical sizes, recording date and objective, plus the scene objects Imaris stored, such as filament tracings. Derived from OME sample files (Imaris file format 5.5.0) browsed with h5py and a synthetic file; h5py (with hdf5plugin for LZ4) is the reference reader, and Bio-Formats `showinf` a second opinion on geometry. Provenance: `docs/provenance/ims.md`. Format id `ims`, crate `openreadout-hdf5` (module `ims`).

Imaris 5 files are HDF5 (read through the pure-Rust `hdf5-pure`). Imaris 3 files, also called `.ims` (the `_IMS3` samples), are TIFF-based and not this format; without an HDF5 signature the `.ims` extension alone is only an extension-level match.

## Detection

HDF5 signature (`HDF5_SIGNATURE` at byte 0 or a user-block boundary, `looks_like_hdf5`) **and** either the `.ims` extension or the text `ImarisDataSet` (a root attribute value) within the first 64 KiB → definite.

## Layout (observed)

| path | content | our use |
| --- | --- | --- |
| root attributes `ImarisVersion`, `ImarisDataSet`, `DataSetDirectoryName`, `NumberOfDataSets` | text | `format_version` = `ImarisVersion` (`5.5.0` in every corpus file) |
| `DataSet/ResolutionLevel L/TimePoint T/Channel C/Data` | (z, y, x) volume, uint8/uint16/uint32/float32, padded to whole chunks | one plane per z; `L` = pyramid level, `T` = time, `C` = channel |
| `.../Channel C` attributes `ImageSizeX`, `ImageSizeY`, `ImageSizeZ` | the real extent at that level | planes are cropped to it; level 0 gives `size_x/y/z` |
| `.../Channel C/Histogram`, `Histogram1024` | uint64 histograms | not read |
| `DataSetInfo/Image` `X`, `Y`, `Z`, `ExtMin0..2`, `ExtMax0..2`, `Unit` | extent and physical bounding box | `physical_size` = (ExtMax − ExtMin) / size, in `Unit` (`um`, `nm`, …; empty = µm) |
| `DataSetInfo/Image` `RecordingDate` (`YYYY-MM-DD HH:MM:SS.mmm`), `Name`, `Description`, `LensPower`, `NumericalAperture`, `MicroscopeMode` | text | `acquired_at` (local time, no zone: a note says so), `name` (file part of `Name`), `extra.source_name`, `extra.description`, `objective.nominal_magnification`, `objective.lens_na`, `extra.microscope_mode` |
| `DataSetInfo/Channel C` `Name`, `Color` (`r g b`, 0–1), `LSMExcitationWavelength`, `LSMEmissionWavelength` (`561 nm`) | text | channel `name`, `color` (`#RRGGBB`), `excitation_nm`, `emission_nm` |
| `DataSetInfo/TimeInfo` `TimePoint1..N` | per-time-point date text | `time_increment_s` (mean interval), `extra.delta_t_s` |
| `DataSetInfo/Imaris` `Version` | writing software | `instrument.software_version` (`software` = `Imaris`) |
| `DataSetInfo/CustomData`, `Log`, `TimestampsPerFrame`, … | text | `vendor` (`info --view full`) only |
| `Scene8/Content/<Object>` | Imaris objects (filaments, spots, surfaces, cells) with their statistics | tables (below, § Scene objects) |
| `DataSetTimes`, `Thumbnail/Data`, `Scene` (legacy), `DataSetEvents` | times, thumbnail, legacy scene | listed by `info --view structure` / notes, not decoded |

Every attribute is stored as an **array of one-character strings** (`|S1`); `attr_text` joins them. Values may end with a NUL, which is dropped.

## Chunks and filters

`PlaneDataset` decodes one z plane itself: contiguous volumes are read row by row; chunked volumes read only the chunks whose z range holds the plane (chunk addresses from the B-tree through `hdf5-pure`'s chunk index), undo the filters in reverse pipeline order (skipping those masked off for the chunk) and copy the cropped rows. Filters decoded: deflate (1), shuffle (2), fletcher32 (3, checksum stripped), LZ4 (32004, `hdf5_lz4_decode`: big-endian decoded size and block size, then per block a big-endian compressed size, blocks stored raw when that equals the block size). Other filters exit 6. Observed: none (contiguous), gzip, shuffle + LZ4, LZ4 alone.

## Scene objects (`Scene8/Content`, module `ims_scene`)

Observed in Imaris 9.8 files of Zenodo 20758452 (filament tracings) with h5py; see
`docs/provenance/ims.md`. Each child group of `Scene8/Content` is one object
(`Filaments0`, `Points0`, `MegaSurfaces0`, …, attribute `Name` e.g. `Filaments 1`), visited in
natural name order. Compound datasets are read whole (`read_records` → `Records` { `fields`,
`text`, `rows` } of `FieldValue` { `Number`, `Text` }): integer and float fields as numbers,
fixed-length strings as text; datasets above 512 MiB exit 6.

**Statistics** (`StatisticsType`: `ID`, `ID_Category`, `ID_FactorList`, `Name`, `Unit`;
`StatisticsValue`: `ID_Time`, `ID_Object`, `ID_StatisticsType`, `Value`; `Category`: `ID`,
`CategoryName`; `Factor`: `ID_List`, `Name`, `Level`). Imaris keeps one statistic type per
combination of factor levels (37 `Dendrite Length` types for the `Depth` × `Level` pairs). One
table per category (`SceneTableKind::Statistics` { `category` }), named
`<object name>: <category> statistics`:

- rows: the distinct (`ID_Time`, `ID_Object`) pairs, sorted (object −1 / time −1: the overall
  values of the `Overall` category);
- columns (`StatColumn`): `ID` (`Id`), `Time` (`Time`), then the object-attribute factors
  (`Factor`: a factor with one level per object but several across objects, `Depth`, `Level`;
  numbers as `float64`, text levels as `uint32` codes with `extra.categories`), then the
  statistics (`Statistic`), sorted by name, factor levels, type id; a factor with several levels
  for one object and name splits a statistic into columns `Name [Factor=level, …]`; a factor
  with a single level for every type of a name (`Collection`) describes the name and is dropped.
  Column `unit` is the type's `Unit`. Values not recorded for a row are NaN.

**Record datasets** (`SceneTableKind::Records` { `path` }): every other 1-D compound dataset of
the object group and of its direct subgroups (e.g. `Points/Point`), sorted by path, as a table
`<object name>: <dataset>` of its numeric fields (`Vertex`: `PositionX/Y/Z`, `Radius`; `Edge`:
`VertexA`, `VertexB`; `Filament`, `DendriteSegment`, `Spine` with index ranges, …). Text fields
are left out and named in `extra.text_fields_omitted`; `StatisticsType`, `StatisticsValue`,
`StatisticsValueTimeOffset`, `Factor`, `FactorList`, `Category`, `CreationParameters` and the
`Label*` datasets are not record tables. Surface meshes (`SurfaceModel`, `BlockData`) are 2-D or
empty in the corpus and are not decoded.

Every table has `extra.object`, `extra.object_name`, `extra.kind` (`statistics`, `records`),
and statistics tables `extra.category`, record tables `extra.dataset` (`SceneTable` { `object`,
`object_name`, `kind`, `info`, `stat_columns` }; `scene_tables` lists them from headers and
the statistics' structure, `read_scene_table` reads the values).

**Validation.** h5py (`oracle/gen.py`, `_ims_scene_tables`) builds the same tables independently:
all 83 tables of the three files agree (column names, dtypes, every value). Against the
depositor's own exports of the Imaris statistics (`spines_dendrites_info.csv`,
`spines_mastersheet.csv`), 1 451 values (dendrite length; spine length, volume, area, mean
diameter) of the cells whose ids the exports keep agree to 4.8e-6 relative (the exports'
rounding); one cell's ids are renumbered in the exports (values equal in order).

## `check` finding codes

| code | severity | meaning |
| --- | --- | --- |
| `truncated` | error | the file is shorter than the HDF5 end-of-file address |
| `missing_data` | error | a level × time point × channel has no readable `Data` dataset |
| `extent` | error | a stored volume is smaller than the level's `ImageSize` |
| `plane_decode` | error | the last level-0 plane or the first plane of another level fails to decode |
| `unit` | warning | unknown length unit (no physical sizes) |

## Observed corpus values

| id | image | storage |
| --- | --- | --- |
| `ome-imaris-convallaria-3c-1t` | 1024 × 1024, 3 channels, uint16, 1.206 µm | contiguous |
| `ome-imaris-convallaria-3c-10t` | same, 10 time points (2.52 s) | contiguous |
| `ome-imaris-convallaria-3c-1t-2x2grid` | 1949 × 1949 (stored 2048), 3 levels | gzip |
| `ome-imaris-cropped-cdm3d-lz4` | 154 × 146 × 43 (stored 256 × 256 × 48), 2 channels, 2 levels | shuffle + LZ4 |
| `ome-imaris-cropped-retina-lz4` | 143 × 109 × 64, uint8 | LZ4 |
| `ome-imaris-retina-large` | 2048 × 1567 × 64 uint8, 4 levels (level 3 halves z) | gzip |

## Vocabulary (every public identifier in `openreadout-hdf5/src/ims.rs` and `ims_scene.rs` must appear here)

| identifier | meaning |
| --- | --- |
| `read_plane_region` | a rectangle of one z plane, reading only the chunks it overlaps (decoded chunks cached between reads) |
| `ImsReader`, `ImsDataset`, `IMS_FORMAT_ID`, `open` | format reader, opened file (core `Dataset`), the id `ims` |
| `ImsLevel`, `level`, `size_x`, `size_y`, `size_z` | one resolution level and its cropped extent |
| `version`, `levels`, `timepoints`, `channels`, `pixel_type` | root `ImarisVersion`, levels, time points, channels, sample type |
| `ims_color`, `ims_datetime` | `r g b` → `#RRGGBB`; date text → ISO-8601 local time |
| `PlaneDataset`, `path`, `shape`, `big_endian`, `read_plane`, `filter_names`, `chunk_shape`, `chunk_count` | a (z, y, x) dataset read one plane at a time (`h5util`) |
| `PlaneSource`, `h5`, `fs`, `path`, `format` | where a plane read finds its bytes: the parsed file, the namespace and path, the format id errors name (`h5util`) |
| `FILTER_DEFLATE`, `FILTER_SHUFFLE`, `FILTER_FLETCHER32`, `FILTER_LZ4` | HDF5 filter ids decoded (`h5util`) |
| `FieldValue` { `Number`, `Text` }, `Records` { `fields`, `text`, `rows` }, `read_records` | compound records (`ims_scene`) |
| `SceneTable` { `object`, `object_name`, `kind`, `info`, `stat_columns` }, `SceneTableKind` { `Statistics` { `category` }, `Records` { `path` } }, `StatColumn` { `Id`, `Time`, `Factor`, `Statistic` }, `scene_tables`, `read_scene_table` | scene-object tables (`ims_scene`) |
