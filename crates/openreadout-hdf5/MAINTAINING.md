# Maintaining `openreadout-hdf5`

HDF5-based formats through the pure-Rust `hdf5-pure` crate: Imaris `.ims` volumes (`ims`, `ims.rs`), Neurodata Without Borders 2.x (`nwb`, `nwb.rs`, `nwb_tables.rs`; also the NWB writer `nwb_write.rs` behind `export --format nwb`), NeXus NXmx diffraction data (`nxmx`, `nxmx.rs`: EIGER master and data files) and a generic HDF5 fallback (`hdf5`, `generic.rs`). Project-wide process: [docs/maintaining.md](../../docs/maintaining.md). Notes: [docs/formats/ims.md](../../docs/formats/ims.md), [docs/formats/hdf5.md](../../docs/formats/hdf5.md), [docs/formats/nxmx.md](../../docs/formats/nxmx.md); provenance: [docs/provenance/nxmx.md](../../docs/provenance/nxmx.md), [docs/provenance/ims.md](../../docs/provenance/ims.md), [docs/provenance/hdf5.md](../../docs/provenance/hdf5.md).

## Decode pipeline

1. **Detect** (`h5util.rs` `looks_like_hdf5`; registry order matters): Imaris and NWB claim their files definitely (Imaris by the `ImarisDataSet` marker in its first bytes; NWB by `NWBFile` in the root attributes); `Hdf5Reader` is registered after EMD, Imaris and NWB and takes any other HDF5 file.
2. **HDF5 access** (`h5util.rs`): `open_h5` over a byte source (`H5Source`), attribute text and JSON, a bounded tree walk (`walk_limit`), and a plane reader for 3-D (z, y, x) datasets (`PlaneDataset`) that decodes chunks itself — so the LZ4 filter 32004 Imaris writes works and one z plane touches only its chunks (`chunk_shape`, `read_plane`, `read_plane_region`, `unfilter`, `unshuffle`). **Chunk filters branch here** (`filter_names`: deflate, shuffle, fletcher32, LZ4, bitshuffle 32008 with or without LZ4; others exit 6).
3. **Imaris** (`ims.rs`): `DataSet/ResolutionLevel N/TimePoint T/Channel C/Data` volumes (pyramid levels), `DataSetInfo` metadata (image size, extents in µm, channel colours and names, times; `ims_color`, `ims_datetime`, `unit_um`). **Imaris versions branch** on the file's version attribute and how `DataSetInfo` stores sizes.
4. **NWB** (`nwb.rs`, `nwb_tables.rs`): session fields; `TimeSeries`, `ElectricalSeries`, `SpatialSeries` under `acquisition/` and `processing/` as traces (time base from `starting_time` + `rate` or `timestamps`; irregular series get a `time` channel; electrode metadata per channel); every `DynamicTable`, units' spike times and `SpikeEventSeries` as numeric tables (text columns as category codes, ragged columns as counts).
5. **NXmx** (`nxmx.rs`): detection by name (`*_master.h5` definite; `.nxs` and data files after a look inside or at the siblings); the external links `/entry/data/data_NNNNNN` read from the master's bytes (`external_data_links`, because `hdf5-pure` does not resolve external links); each data file's 3-D dataset through `PlaneDataset`; the geometry into `extra.diffraction` (`openreadout_core::diffraction`). **Masters branch** between the DECTRIS file writer (`detectorSpecific`, `goniometer`) and beamline NXmx (`transformations`); a master alone opens with sizes from `detectorSpecific`.
6. **Generic** (`generic.rs`): structure only: groups, datasets (shape, type), attributes.

## Invariants and checks

- Imaris: HDF5 parses; `DataSet/ResolutionLevel/TimePoint/Channel` groups numbered without gaps; every level × time × channel has a `Data` dataset of the image's sample type at least as large as the level's size; the last plane of every level-0 volume and the first of every other level decode.
- NWB: root is an `NWBFile` with the required session fields; every series has rank-1/2 numeric data, a time base and a readable last sample; electrode rows inside the electrodes table; `channel_conversion` has one factor per channel; every table's last row reads; ragged indices are non-decreasing offsets inside their data.
- NXmx: an NXmx entry; every linked data file present, of one image size and type, numbered in order (`image_nr_low/high`); the first and last image of each decode; the geometry fields present.
- Generic: the superblock and object tree parse; the file is at least as long as the superblock's end-of-file address.

## Debugging a new file

- `openreadout info FILE --view structure` lists the HDF5 tree (groups, datasets with shape/type/filters, attributes) for all three readers — the first command for any HDF5 variant.
- `tests/fixtures.rs` reads the committed `.ims`/NWB/HDF5 fixtures against their oracle JSON; `tests/nwb_export.rs` round-trips the NWB writer (also checked by `oracle/nwb_validate.py` with pynwb).
- Oracles: h5py-based readers in `oracle/gen.py` (`ims`, `nwb`) and `oracle/diffraction.py` (`nxmx`, with hdf5plugin for bitshuffle).

## Fragile spots

- `hdf5-pure` is a young dependency: a new HDF5 feature (a message type, a newer superblock or B-tree version) shows as a parse error in every reader of this crate at once.
- Imaris: only the first data set is read when a file declares several; Scene/Scene8 objects are listed, not decoded; Imaris 3 files are TIFF.
- NXmx: the external-link scan reads link messages from raw bytes; a writer that stores the link value differently (a dense link heap that compresses, an absolute path) needs a look at `external_data_links`. Virtual datasets are not read. Per-module detector geometry is not applied.
- NWB: PatchClampSeries, ImageSeries and optical-physiology series are listed, not decoded; NWB 1.x and the Zarr backend are not read.

<!-- BEGIN GENERATED guide -->
## Facts (generated)

*Generated by `cargo xtask guides --write` from the sources, `corpus/manifest.toml`, `corpus/assurance/evidence.json` and `corpus/intake/`; do not edit. CI fails when it is stale.*

### Formats

| format id | notes and provenance | confidence | basis | development files: read / confirmed | depositors | held-out pass / fail |
| --- | --- | --- | --- | --- | --- | --- |
| `ims` | [format note](../../docs/formats/ims.md), [provenance log](../../docs/provenance/ims.md) | high | vendor docs | 26 / 26 | 21 | 1 / 0 |
| `nwb` | [format note](../../docs/formats/hdf5.md), [provenance log](../../docs/provenance/hdf5.md) | high | open spec | 22 / 21 | 19 | 1 / 0 |
| `nxmx` | [format note](../../docs/formats/nxmx.md), [provenance log](../../docs/provenance/nxmx.md) | medium | open spec | 3 / 3 | 2 | - |
| `hdf5` | [format note](../../docs/formats/hdf5.md), [provenance log](../../docs/provenance/hdf5.md) | medium | open spec | 4 / 4 | 4 | - |

### Source map

| file | what it does (its module documentation) |
| --- | --- |
| [`src/assurance.rs`](src/assurance.rs) | Assurance profiles (`docs/assurance.md`) of the HDF5-based readers (Imaris `.ims`, NWB and generic HDF5): the variant features of a file and the feature values the development… |
| [`src/generic.rs`](src/generic.rs) | Any other HDF5 file: `info --view structure` lists groups, datasets and attributes; `info` summarizes; nothing is decoded as images or traces |
| [`src/h5util.rs`](src/h5util.rs) | Shared HDF5 helpers on top of `hdf5-pure`: detection, attribute text, a tree walk, and a plane reader for 3-D (z, y, x) datasets that decodes chunks itself so that filters |
| [`src/ims.rs`](src/ims.rs) | Imaris `.ims` (HDF5): `DataSet/ResolutionLevel N/TimePoint T/Channel C/Data` volumes, `DataSetInfo` metadata |
| [`src/ims_scene.rs`](src/ims_scene.rs) | Imaris scene objects (`Scene8/Content/<Object>`): the statistics Imaris computed for filaments, spots, surfaces and cells, pivoted into one table per object category, and the |
| [`src/lib.rs`](src/lib.rs) | HDF5-based formats through the pure-Rust `hdf5-pure` crate: Imaris `.ims` volumes (pyramids, LZ4/deflate chunks), NWB 2.x (session fields, plain `TimeSeries` as traces), |
| [`src/nwb.rs`](src/nwb.rs) | Neurodata Without Borders (NWB 2.x, HDF5): session fields; `TimeSeries`, `ElectricalSeries` and `SpatialSeries` under `acquisition/` and `processing/`, and the intracellular |
| [`src/nwb_tables.rs`](src/nwb_tables.rs) | NWB tables: every `DynamicTable` (electrodes, units, trials, …), the spike times of a units table, and `SpikeEventSeries`, as numeric tables |
| [`src/nwb_write.rs`](src/nwb_write.rs) | NWB 2.x writer (`export --format nwb`): the traces of an electrophysiology file (ABF sweeps, Neuralynx, Blackrock, SpikeGLX, Intan, ...) as plain `TimeSeries` under… |
| [`src/nxmx.rs`](src/nxmx.rs) | NeXus NXmx diffraction data (DECTRIS EIGER and JUNGFRAU master files, beamline `.nxs` files): the experiment in `/entry` (beam, detector, goniometer) and the images in data |

### Where variants branch

The assurance profile ([`src/assurance.rs`](src/assurance.rs)) observes these features (each value is looked up in the validated table below; a value never confirmed makes the file `unvalidated` for the feature's outputs) and reports these structures and assumptions:

- feature codec `"hdf5 unfiltered"`
- feature codec `name`
- feature codec `format!("hdf5 {name}")`
- feature layout `format!("data at {obj}")`
- feature layout `"resolution_levels"`
- feature record `format!("scene statistics: {object}")`
- feature record `format!("scene records: {object}")`
- feature format_version `v`
- feature record `n`
- feature layout `format!("time base {b}")`
- feature dialect `"generic hdf5"`
- feature acquisition `m` (descriptive)
- feature format_version `v` (descriptive)
- undecoded "additional data sets"
- undecoded "Imaris groups (legacy Scene, events)"
- undecoded "length unit"
- undecoded "NWB objects"

### Validated variants and the corpus files that pin them

| format | kind | value | outputs | confirmed files | read | example corpus files |
| --- | --- | --- | --- | --- | --- | --- |
| `hdf5` | dialect | `generic hdf5` | metadata, tables, traces | 4 | 4 | `hdf5-zenodo14794967-picoharp-histogram-cl1-det2`, `hdf5-zenodo15463833-data-pmoke-mag`, `hdf5-zenodo4165174-wse2-cb` |
| `hdf5` | format_version | `superblock version 0` | descriptive | 3 | 3 | `hdf5-zenodo14794967-picoharp-histogram-cl1-det2`, `hdf5-zenodo15463833-data-pmoke-mag`, `hdf5-zenodo4165174-wse2-cb` |
| `hdf5` | format_version | `superblock version 2` | descriptive | 1 | 1 | `hdf5-zenodo7806036-figure7-nexus` |
| `ims` | acquisition | `LaserScanningConfocalMicroscopy` | descriptive | 3 | 3 | `figshare26891368-103-EP-405-45-561-7-5-Airyscan-Processin`, `figshare27011572-117-EP-405-4-561-6-Airyscan-Processing-C`, `zenodo17406102-tmp` |
| `ims` | acquisition | `SpinningDiskConfocal` | descriptive | 7 | 7 | `figshare19317977-Fig4c-static`, `figshare24966153-ICAM-2-S-Dev25-3-2021-09-30-Molly-iPSC-C`, `ome-imaris-convallaria-3c-10t` |
| `ims` | codec | `hdf5 deflate` | pixels | 18 | 18 | `figshare20081261-SuppFile22-Tail-Practice-Image-3`, `figshare20484396-Extended-Data-Fig-3b`, `figshare26891368-103-EP-405-45-561-7-5-Airyscan-Processin` |
| `ims` | codec | `hdf5 lz4` | pixels | 2 | 2 | `ome-imaris-cropped-cdm3d-lz4`, `ome-imaris-cropped-retina-lz4` |
| `ims` | codec | `hdf5 shuffle` | pixels | 1 | 1 | `ome-imaris-cropped-cdm3d-lz4` |
| `ims` | codec | `hdf5 unfiltered` | pixels | 6 | 6 | `figshare19317977-Fig4c-static`, `figshare24966153-ICAM-2-S-Dev25-3-2021-09-30-Molly-iPSC-C`, `ome-imaris-convallaria-3c-10t` |
| `ims` | field | `experiment.acquisition.started_at` | descriptive | 25 | 25 | `figshare19317977-Fig4c-static`, `figshare20081261-SuppFile22-Tail-Practice-Image-3`, `figshare20484396-Extended-Data-Fig-3b` |
| `ims` | format_version | `5.5.0` | metadata, pixels | 26 | 26 | `figshare19317977-Fig4c-static`, `figshare20081261-SuppFile22-Tail-Practice-Image-3`, `figshare20484396-Extended-Data-Fig-3b` |
| `ims` | layout | `resolution_levels` | pixels | 16 | 16 | `figshare20081261-SuppFile22-Tail-Practice-Image-3`, `figshare26891368-103-EP-405-45-561-7-5-Airyscan-Processin`, `figshare27011572-117-EP-405-4-561-6-Airyscan-Processing-C` |
| `ims` | record | `scene records: Cells` | tables | 1 | 1 | `zenodo4433202-ovule-732` |
| `ims` | record | `scene records: Filaments` | tables | 3 | 3 | `zenodo20758452-181-c2-wt-20w-x60`, `zenodo20758452-296-c2-ko-20w-x60`, `zenodo20758452-460-c3-ko-10w-x60` |
| `ims` | record | `scene records: ImageMasks` | tables | 1 | 1 | `zenodo4433202-ovule-732` |
| `ims` | record | `scene records: MegaSurfaces` | tables | 1 | 1 | `zenodo19630647-btbr-lab` |
| `ims` | record | `scene records: Surfaces` | tables | 1 | 1 | `zenodo4433202-ovule-732` |
| `ims` | record | `scene statistics: Cells` | tables | 1 | 1 | `zenodo4433202-ovule-732` |
| `ims` | record | `scene statistics: Filaments` | tables | 3 | 3 | `zenodo20758452-181-c2-wt-20w-x60`, `zenodo20758452-296-c2-ko-20w-x60`, `zenodo20758452-460-c3-ko-10w-x60` |
| `ims` | record | `scene statistics: MegaSurfaces` | tables | 1 | 1 | `zenodo19630647-btbr-lab` |
| `ims` | record | `scene statistics: Surfaces` | tables | 1 | 1 | `zenodo4433202-ovule-732` |
| `ims` | sample_layout | `uint16` | pixels | 18 | 18 | `figshare19317977-Fig4c-static`, `figshare20081261-SuppFile22-Tail-Practice-Image-3`, `figshare24966153-ICAM-2-S-Dev25-3-2021-09-30-Molly-iPSC-C` |
| `ims` | sample_layout | `uint8` | pixels | 8 | 8 | `figshare20484396-Extended-Data-Fig-3b`, `ome-imaris-cropped-retina-lz4`, `ome-imaris-retina-large` |
| `ims` | writer | `Imaris` | descriptive | 26 | 26 | `figshare19317977-Fig4c-static`, `figshare20081261-SuppFile22-Tail-Practice-Image-3`, `figshare20484396-Extended-Data-Fig-3b` |
| `ims` | writer_version | `Imaris 10.0` | descriptive | 2 | 2 | `zenodo10477219-MS1WHO-1`, `zenodo15001649-bfGetReaderTest` |
| `ims` | writer_version | `Imaris 10.1` | descriptive | 2 | 2 | `ome-imaris-cropped-retina-lz4`, `zenodo14235726-performanceTestImage` |
| `ims` | writer_version | `Imaris 10.2` | descriptive | 2 | 2 | `ome-imaris-cropped-cdm3d-lz4`, `zenodo19666865-exm-artifact` |
| `ims` | writer_version | `Imaris 5.5` | descriptive | 7 | 7 | `figshare19317977-Fig4c-static`, `figshare24966153-ICAM-2-S-Dev25-3-2021-09-30-Molly-iPSC-C`, `imariswriter-minimal-ims` |
| `ims` | writer_version | `Imaris 7.4` | descriptive | 1 | 1 | `figshare20484396-Extended-Data-Fig-3b` |
| `ims` | writer_version | `Imaris 9.0` | descriptive | 1 | 1 | `ome-imaris-retina-large` |
| `ims` | writer_version | `Imaris 9.5` | descriptive | 2 | 2 | `zenodo4433202-ovule-732`, `zenodo4449687-hela-h2b` |
| `ims` | writer_version | `Imaris 9.6` | descriptive | 1 | 1 | `figshare20081261-SuppFile22-Tail-Practice-Image-3` |
| `ims` | writer_version | `Imaris 9.7` | descriptive | 3 | 3 | `figshare26891368-103-EP-405-45-561-7-5-Airyscan-Processin`, `figshare27011572-117-EP-405-4-561-6-Airyscan-Processing-C`, `zenodo5895076-polymersome-spot` |
| `ims` | writer_version | `Imaris 9.8` | descriptive | 4 | 4 | `zenodo19630647-btbr-lab`, `zenodo20758452-181-c2-wt-20w-x60`, `zenodo20758452-296-c2-ko-20w-x60` |
| `ims` | writer_version | `Imaris 9.9` | descriptive | 1 | 1 | `zenodo17406102-tmp` |
| `nwb` | format_version | `2.0.2` | metadata, tables, traces | 1 | 1 | `dandi000006-anm372907-20170613` |
| `nwb` | format_version | `2.0b` | metadata, tables, traces | 1 | 1 | `dandi000027-sub-rat123` |
| `nwb` | format_version | `2.1.0` | metadata, tables, traces | 1 | 1 | `dandi000035-sub-mouse-zudob-sample-27-icephys` |
| `nwb` | format_version | `2.10.0` | metadata, tables, traces | 1 | 1 | `dandi001829-nwb-011226` |
| `nwb` | format_version | `2.2.2` | metadata, tables, traces | 1 | 1 | `figshare12780425-nwb-icephys-171017-kk13` |
| `nwb` | format_version | `2.2.4` | metadata, tables, traces | 1 | 1 | `dandi000043-sub-m19-01-001-ses-20190228t222559-icephys` |
| `nwb` | format_version | `2.2.5` | metadata, tables, traces | 3 | 3 | `dandi000034-mouse412804-155542`, `dandi000059-ms10-170314`, `dandi000067-ee-044` |
| `nwb` | format_version | `2.3.0` | metadata, tables, traces | 2 | 3 | `dandi000117-sub-20210511003-0019-icephys`, `dandi000126-sub-1` |
| `nwb` | format_version | `2.4.0` | metadata, tables, traces | 1 | 1 | `dandi000221-hi198-060619` |
| `nwb` | format_version | `2.5.0` | metadata, tables, traces | 1 | 1 | `dandi001746-icephys-mfa180730` |
| `nwb` | format_version | `2.6.0` | metadata, tables, traces | 2 | 2 | `dandi000968-sub-na-icephys`, `dandi000987-ecephys-a7802` |
| `nwb` | format_version | `2.7.0` | metadata, tables, traces | 3 | 3 | `dandi001358-ecephys-a19034-s062`, `dandi001544-icephys-cc328`, `dandi001544-icephys-cc356` |
| `nwb` | format_version | `2.8.0` | metadata, tables, traces | 1 | 1 | `dandi001475-patchseq-rmbe91m1` |
| `nwb` | format_version | `2.9.0` | metadata, tables, traces | 2 | 2 | `dandi001641-ecephys-le31-s1-03`, `dandi001641-ecephys-le31-s1-06` |
| `nwb` | layout | `time base starting_time + rate` | traces | 11 | 12 | `dandi000035-sub-mouse-zudob-sample-27-icephys`, `dandi000043-sub-m19-01-001-ses-20190228t222559-icephys`, `dandi000067-ee-044` |
| `nwb` | layout | `time base timestamps` | traces | 3 | 4 | `dandi000006-anm372907-20170613`, `dandi000059-ms10-170314`, `dandi000987-ecephys-a7802` |
| `nwb` | layout | `time base uniform timestamps` | traces | 1 | 1 | `dandi000126-sub-1` |
| `nwb` | record | `CurrentClampSeries` | traces | 7 | 8 | `dandi000035-sub-mouse-zudob-sample-27-icephys`, `dandi000043-sub-m19-01-001-ses-20190228t222559-icephys`, `dandi001544-icephys-cc328` |
| `nwb` | record | `CurrentClampStimulusSeries` | traces | 6 | 7 | `dandi000035-sub-mouse-zudob-sample-27-icephys`, `dandi000043-sub-m19-01-001-ses-20190228t222559-icephys`, `dandi001544-icephys-cc328` |
| `nwb` | record | `DynamicTable` | tables | 7 | 7 | `dandi000006-anm372907-20170613`, `dandi000059-ms10-170314`, `dandi000067-ee-044` |
| `nwb` | record | `ElectricalSeries` | traces | 2 | 2 | `dandi000067-ee-044`, `dandi001475-patchseq-rmbe91m1` |
| `nwb` | record | `ElectrodesTable` | tables | 2 | 2 | `dandi001641-ecephys-le31-s1-03`, `dandi001641-ecephys-le31-s1-06` |
| `nwb` | record | `IZeroClampSeries` | traces | 3 | 3 | `dandi000035-sub-mouse-zudob-sample-27-icephys`, `dandi000117-sub-20210511003-0019-icephys`, `dandi000968-sub-na-icephys` |
| `nwb` | record | `IntracellularElectrodesTable` | tables | 1 | 1 | `dandi000968-sub-na-icephys` |
| `nwb` | record | `IntracellularRecordingsTable` | tables | 1 | 1 | `dandi000968-sub-na-icephys` |
| `nwb` | record | `IntracellularResponsesTable` | tables | 1 | 1 | `dandi000968-sub-na-icephys` |
| `nwb` | record | `IntracellularStimuliTable` | tables | 1 | 1 | `dandi000968-sub-na-icephys` |
| `nwb` | record | `SequentialRecordingsTable` | tables | 1 | 1 | `dandi000968-sub-na-icephys` |
| `nwb` | record | `SimultaneousRecordingsTable` | tables | 1 | 1 | `dandi000968-sub-na-icephys` |
| `nwb` | record | `SpatialSeries` | traces | 2 | 2 | `dandi000059-ms10-170314`, `dandi000987-ecephys-a7802` |
| `nwb` | record | `SpikeEventSeries` | tables | 1 | 1 | `dandi000221-hi198-060619` |
| `nwb` | record | `SweepTable` | tables | 6 | 7 | `dandi000035-sub-mouse-zudob-sample-27-icephys`, `dandi000043-sub-m19-01-001-ses-20190228t222559-icephys`, `dandi000117-sub-20210511003-0019-icephys` |
| `nwb` | record | `TimeIntervals` | tables | 7 | 8 | `dandi000006-anm372907-20170613`, `dandi000059-ms10-170314`, `dandi000221-hi198-060619` |
| `nwb` | record | `TimeSeries` | traces | 4 | 5 | `dandi000006-anm372907-20170613`, `dandi000059-ms10-170314`, `dandi000126-sub-1` |
| `nwb` | record | `Units` | tables | 7 | 7 | `dandi000006-anm372907-20170613`, `dandi000034-mouse412804-155542`, `dandi000221-hi198-060619` |
| `nwb` | record | `VectorData` | tables | 7 | 7 | `dandi000006-anm372907-20170613`, `dandi000034-mouse412804-155542`, `dandi000221-hi198-060619` |
| `nwb` | record | `VoltageClampSeries` | traces | 0 | 1 |  |
| `nwb` | record | `VoltageClampStimulusSeries` | traces | 0 | 1 |  |
| `nxmx` | codec | `hdf5 bitshuffle` | pixels | 2 | 2 | `zenodo3383611-therm-6-1-master`, `zenodo3383611-therm-6-1-nxs` |
| `nxmx` | format_version | `NXmx` | metadata, pixels | 3 | 3 | `zenodo3383611-therm-6-1-master`, `zenodo3383611-therm-6-1-nxs`, `zenodo4327001-wtb2-1-master` |
| `nxmx` | instrument | `Eiger 16M` | descriptive | 3 | 3 | `zenodo3383611-therm-6-1-master`, `zenodo3383611-therm-6-1-nxs`, `zenodo4327001-wtb2-1-master` |
| `nxmx` | layout | `data at /data` | pixels | 2 | 2 | `zenodo3383611-therm-6-1-master`, `zenodo3383611-therm-6-1-nxs` |
| `nxmx` | sample_layout | `uint16` | pixels | 2 | 3 | `zenodo3383611-therm-6-1-master`, `zenodo3383611-therm-6-1-nxs` |

### Tests, fixtures, fuzz targets, snapshots

- integration tests: [`tests/fixtures.rs`](tests/fixtures.rs), [`tests/nwb_export.rs`](tests/nwb_export.rs)
- committed fixtures: 11 files in [`tests/fixtures/`](tests/fixtures) (malformed ones are replayed through every reader by `openreadout`'s `tests/fuzz_regressions.rs`; all are snapshotted by its `tests/golden.rs`)
- fuzz targets (`fuzz/fuzz_targets/`): `nxmx_links`, `whole_hdf5`, `whole_ims`, `whole_nwb`
- corpus inputs by tier: heldout 8, smoke 19, standard 36
- golden snapshots: [`corpus/snapshots/ims.jsonl`](../../corpus/snapshots/ims.jsonl), [`corpus/snapshots/nwb.jsonl`](../../corpus/snapshots/nwb.jsonl), [`corpus/snapshots/nxmx.jsonl`](../../corpus/snapshots/nxmx.jsonl), [`corpus/snapshots/hdf5.jsonl`](../../corpus/snapshots/hdf5.jsonl)

### Open new-variant intakes

None.
<!-- END GENERATED guide -->
