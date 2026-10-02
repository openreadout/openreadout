# Provenance log — Imaris IMS (`ims`)

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

## 2026-09-22 — reader (Richard Zimring with Claude as assistant)

Bio-Formats 8.5.0 `showinf` was run as a black box to compare image geometry (`oracle/gen.py`, field `bioformats` of each oracle): it agrees with our width, height, z, c, t and pixel type for every file and lists each resolution level as a series with the same sizes.

**Tools:** `hdf5-pure` 0.47 (MIT OR Apache-2.0) parses HDF5 (see `docs/provenance/emd.md` for its evaluation). h5py 3.16 (BSD-3-Clause) was used as an HDF5 browser to list groups, attributes, dataset shapes, chunk shapes and filter ids, and is the pixel oracle; hdf5plugin 7.1 (MIT-style licences of the bundled filters) lets h5py decode LZ4 chunks.

**LZ4 filter (id 32004) layout:** from the HDF Group's `hdf5_plugins` repository, `LZ4/src/H5Zlz4.c` (https://github.com/HDFGroup/hdf5_plugins, BSD-style HDF5 licence): big-endian u64 decoded size, big-endian u32 block size, then per block a big-endian u32 compressed size, a block being copied as is when its compressed size equals the block size. Implemented in `openreadout-codecs` (`hdf5_lz4_decode`) on top of `lz4_flex` (MIT) and validated bit for bit on the two LZ4 corpus files.

**Corpus files used** (OME sample images, https://downloads.openmicroscopy.org/images/Imaris-IMS/, each directory's COPYING: CC-BY-4.0): `ome-imaris-convallaria-3c-1t`, `-3c-10t`, `-3c-1t-2x2grid` (davemason: Dave Mason, Andor Dragonfly/Fusion), `ome-imaris-cropped-cdm3d-lz4`, `ome-imaris-cropped-retina-lz4` (bitplane/lz4, Zenodo 14197622), `ome-imaris-retina-large` (eliana). A synthetic file (`crates/openreadout-hdf5/tests/fixtures/synthetic-imaris.ims`) was written with h5py following the observed layout.

**Observed (h5py listings of all six files):**
- Root attributes `ImarisVersion` = `5.5.0`, `ImarisDataSet`, `DataSetDirectoryName` = `DataSet`, `DataSetInfoDirectoryName`, `ThumbnailDirectoryName`, `NumberOfDataSets` (uint32 1). Every text attribute is an array of `|S1` single characters.
- `DataSet/ResolutionLevel L/TimePoint T/Channel C/Data`: rank-3 (z, y, x) datasets, stored extent a multiple of the chunk shape (e.g. 1949 × 1949 stored as 2048 × 2048; 154 × 146 × 43 as 256 × 256 × 48); the channel group's `ImageSizeX/Y/Z` holds the real extent at each level (Bio-Formats reports the same cropped sizes). Levels halve x and y; the last level of `retina_large` also halves z.
- Filters seen: none (contiguous), `[1]` gzip, `[2, 32004]` shuffle + LZ4, `[32004]`.
- `DataSetInfo/Image`: `X`, `Y`, `Z`, `ExtMin0..2`, `ExtMax0..2`, `Unit` (empty or `um`), `RecordingDate`, `Name`, `LensPower`, `NumericalAperture`, `MicroscopeMode`. `(ExtMax − ExtMin) / size` gives 1.206 µm for the 10× Dragonfly files (1234.9 µm over 1024 px), matching Bio-Formats' physical size.
- `DataSetInfo/Channel C`: `Name`, `Color` (three 0–1 floats), `LSMExcitationWavelength` (`561 nm nm`), `LSMEmissionWavelength` (`500 nm`), `ColorRange`, `ColorOpacity`, `GammaCorrection`.
- `DataSetInfo/TimeInfo`: `TimePoint1..N` date texts; the 10-time-point file spans 22.708 s (2.523 s per step).

**Inferred (ours):** empty `Unit` = micrometres; `RecordingDate` is local time (no zone written); plane order XYZCT; the file part of `DataSetInfo/Image/Name` is the image name.

## 2026-09-23 — Region reads and chunk cache (Richard Zimring with Claude as assistant)

**Scope.** `Dataset::read_region`: a rectangle of a z plane at any resolution level, reading only the HDF5 chunks it overlaps (contiguous datasets: only the rows it covers). Decoded chunks, which span several z planes, are kept in a 64 MiB cache, so reading a z stack plane by plane decodes each chunk once instead of once per plane. No new interpretation of the file.

**Oracle:** h5py (BSD-3-Clause) with hdf5plugin (MIT) for the LZ4 filter, slicing the same `Data` dataset.

## 2026-09-25 — assurance profile (`src/assurance.rs`)

**Corpus files:** every development input of this format on disk (`corpus/assurance/evidence.json` lists them); no held-out file.
**Prior art consulted:** none.
**What was done.** No parsing logic changed. The reader declares an assurance profile (docs/assurance.md) that fingerprints each file from this reader's own normalized output: resolution levels, the microscope mode and the HDF5 filter ids of the level-0 data set (through `Dataset::assurance_observations`). The table of validated feature values and the confidence level are generated from the development corpus by `cargo xtask assurance-audit`.

## 2026-09-26 — Scene objects: statistics and records as tables (Richard Zimring with Claude as assistant)

**Corpus files used** (new; Zenodo 20758452, Thabault et al., "Age-dependent reorganization of behavioral and striatal function in Cntnap2 knockout mice [Neuronal Morphology]", licence `cc-by-4.0` on the record): `zenodo20758452-296-c2-ko-20w-x60`, `zenodo20758452-181-c2-wt-20w-x60`, `zenodo20758452-460-c3-ko-10w-x60` (Imaris 9.8 filament tracings of dendrites and spines, Scene8 objects with statistics) and the depositor's exported statistics `spines_dendrites_info.csv`, `spines_mastersheet.csv` (derived from Imaris' statistics exports). Also `imariswriter-minimal-ims` (the 27 KB `Minimal_IMS_File.ims` of `imaris/ImarisWriter`, Apache-2.0, commit b128e6e7). Not used: Zenodo 18473314 and 17975750 (held-out records).

**Prior art consulted:** none (the minimal file ImarisWriter's repository ships is used as data). **Tools:** h5py 3.16 (BSD-3-Clause) as an HDF5 browser and oracle.

**Observed (h5py listings):**
- `Scene8/Content` holds one group per object (`Filaments0`, `Points0`, `MegaSurfaces0`, …; attribute `Name`, e.g. `Filaments 1`); the older `Scene/Content` holds a different, legacy structure (`GraphTracks`, `Graphs`) and is listed only. `Scene8/Data` is a UTF-8 XML byte array whose root `bpImarisApplication Version = "Imaris x64 9.8.2"` names the Imaris that saved the scene.
- Statistics: `StatisticsType` (compound: `ID`, `ID_Category`, `ID_FactorList`, `Name` S256, `Unit` S256), `StatisticsValue` (`ID_Time`, `ID_Object`, `ID_StatisticsType`, `Value` f64), `Category` (`ID`, `CategoryName`, `Name`: `Dendrite`, `Filament`, `Overall`, `Point`, `Spine`), `Factor` (`ID_List`, `Name`, `Level`) grouped by `FactorList` id. A statistic name exists once per combination of factor levels (37 `Dendrite Length` types for `Depth`/`Level` pairs); object −1 / time −1 are the overall values.
- Checked by value: dendrite 510000000148 of `181_c2_wt_20w_x60.ims` has `Dendrite Length` 28.216794967651367 (type with `Depth`=1, `Level`=1); the depositor's CSV row for FilamentID 100000005 / ID 510000000148 has `dendrite_length` 28.216800689697266, `Depth` 1, `Level` 1.
- Record datasets: 1-D compound datasets of numbers and fixed-length strings (`Vertex`: `PositionX/Y/Z`, `Radius` f32; `Edge`: `VertexA`, `VertexB`; `Filament`, `DendriteSegment`, `Spine` with index ranges; `Points/Point`; `Time`, `TimeBegin`, `TrackSegment*`).

**Inferred (ours):** the pivot into one table per category: rows are (time, object) pairs; a factor with several levels for one object and name splits the statistic into columns (`Name [Factor=level]`), a factor with one level per object but several across objects is an object attribute (a column of its own: `Depth`, `Level`), a factor with one level for every type of a name describes the name (`Collection`) and is dropped. Record datasets become tables of their numeric fields; text fields are left out and named in the table's `extra`.
