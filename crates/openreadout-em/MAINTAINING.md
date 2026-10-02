# Maintaining `openreadout-em`

Electron-microscopy readers, one module directory per format: MRC / CCP4 / MAP (`mrc`, `mrc/`), Gatan Digital Micrograph DM3/DM4/DM5 (`dm`, `dm/`; DM5 is HDF5, `dm/dm5.rs`), TIA / ES Vision series `.ser` with `.emi` (`ser`, `ser/`), Velox and NCEM/Berkeley EMD (`emd`, `emd/`; Berkeley in `emd/berkeley.rs`). Project-wide process: [docs/maintaining.md](../../docs/maintaining.md). Notes: `docs/formats/{em,mrc,dm,ser,emd}.md`; provenance: `docs/provenance/{mrc,dm,ser,emd}.md`. **Clean room**: FEI1/FEI2 extended-header decoding comes from mrcfile (BSD-3) and corpus files.

## Decode pipeline

Shared helpers: `util.rs` (bounded reads, endian-aware numbers, half floats, OLE and Unix timestamps).

- **MRC** (`mrc/header.rs`, `mrc/ext.rs`, `mrc/dataset.rs`): the 1024-byte MRC2014 header (open CCP-EM specification): MODE, NX/NY/NZ, MX/MY/MZ, CELLA, MAPC/MAPR/MAPS, the machine stamp (byte order, with detection of wrong stamps `wrong_stamp_is_detected_and_corrected`), NVERSION, labels; then NSYMBT bytes of extended header: **extended-header kinds branch here** (`ExtKind`: FEI1/FEI2 blocks, SerialEM `SERI`, Agard integer/real records, CCP4 symmetry text); then NZ sections. Layout (`Layout`: image, image stack, volume, volume stack) from ISPG and MZ. 4-bit packed mode 101 is unpacked (`unpack_4bit`).
- **DM** (`dm/tags.rs`, `dm/dataset.rs`): a big-endian header (**version 3 or 4 branches**: DM4 has 64-bit lengths and per-tag lengths), a tree of tag groups and data tags whose values use the header's byte order; images from `ImageList` (`ImageData`: `Data`, `DataType`, `Dimensions`, `Calibrations`; `ImageTags`: microscope and acquisition). Large arrays are referenced, not copied.
- **SER** (`ser/file.rs`, `ser/dataset.rs`, `ser/emi.rs`): header (byte order `II`, series id 0x0197; **versions 0x0210 and 0x0220 branch** on 4- vs 8-byte offsets), dimension array, offset arrays, elements (1-D spectra or 2-D images) with tags; the `.emi` sidecar's `<ObjectInfo>` XML only.
- **EMD** (`emd/dataset.rs`, `emd/spectra.rs`): HDF5 through `hdf5-pure`; `Data/Image/<id>` images with their JSON `Metadata`, EDS spectra (`Data/Spectrum`), spectrum images assembled from uint16 X-ray event streams (`decode_stream`: 65535 ends a pixel), STEM-EELS spectrum images. NCEM/Berkeley EMD 0.2 is recognised and refused.

## Invariants and checks

- MRC: MAP identifier and machine stamp; MODE in MRC2014; sizes non-negative; MAPC/MAPR/MAPS a permutation; NZ divisible by MZ for stacks; NLABL consistent; NVERSION 20140/20141; extended header consistent; file size = 1024 + NSYMBT + data (truncation and trailing bytes).
- DM: header version, byte-order word, root length; the tag directory parses to the end; every `ImageList` entry's data array lies inside the file and matches Dimensions × DataType; `Thumbnails.ImageIndex` names an image.
- SER: header ids and version; valid ≤ total elements = product of dimensions; every element and tag inside the file with one shared geometry; `.emi` well-formed.
- EMD: HDF5 parses; every image has a 2-D/3-D `Data` of a supported type and a JSON `Metadata`; last rows readable; event streams decode and their histogram equals a stored detector spectrum; EELS scan lines share one energy calibration.

## Debugging a new file

- `openreadout ls FILE` lists the MRC header/extended header/sections, the DM tag tree, the SER elements or the EMD HDF5 tree; `dump --json` → `vendor` has the header fields and tag trees by their stored names.
- `tests/synthetic_mrc.rs` and `tests/synthetic_emd.rs` build files; DM and SER have unit tests in their modules (`parses_scalars_strings_structs_and_arrays`, `parses_header_elements_and_tags`).
- Oracles: mrcfile (MRC; Bio-Formats second opinion, which flips rows and treats stacks as Z — adjudicated), dm3_lib (DM), ncempy (SER) and h5py (EMD), all in `oracle/gen.py`.

## Fragile spots

- MRC: MAPC/MAPR/MAPS are reported, not applied; FEI presence bitmasks are reported raw; complex modes 3/4 are returned as stored (NX complex values per row; no public complex file: synthetic tests only); gzip files are decompressed through `openreadout_core::gzip`, `.mrcz` and bzip2 are not read.
- DM: the axis rule (which dimension is spectral, Z or T) keys on `Meta Data.Format`/`Signal`, `IsSequence` and calibration units (`docs/formats/dm.md`); a new acquisition type usually lands there. Packed half-plane transforms (DataType 27/28) are returned as stored, not mirrored. DM5 chunked datasets are read whole (up to 2 GiB).
- SER: pixel-size units are not stored; deltas below 1 mm are taken as metres (scan dimensions in `meters` too: TIA writes 1 m on uncalibrated positions); 1-D elements filling the scan become X × Y with energy channels (dimension 0 fastest), other 1-D series stay rows plus traces.
- EMD: the first plane of an EDS spectrum image decodes every event into memory (capped at 400 million events). Berkeley EMD: which array dimension is X is inferred (the last, as ncempy's files name it); a length step of 1 mm or more per pixel is not a pixel size (ncempy's TIA diffraction conversions label reciprocal metres `m`); HDF5 attributes arrive in a hash map and are sorted before they reach the output; Velox complex FFTs are compound (real, imaginary) float32 half planes.

<!-- BEGIN GENERATED guide -->
## Facts (generated)

*Generated by `cargo xtask guides --write` from the sources, `corpus/manifest.toml`, `corpus/assurance/evidence.json` and `corpus/intake/`; do not edit. CI fails when it is stale.*

### Formats

| format id | notes and provenance | confidence | basis | development files: read / confirmed | depositors | held-out pass / fail |
| --- | --- | --- | --- | --- | --- | --- |
| `mrc` | [format note](../../docs/formats/mrc.md), [provenance log](../../docs/provenance/mrc.md) | high | open spec | 13 / 13 | 7 | 2 / 0 |
| `dm` | [format note](../../docs/formats/dm.md), [provenance log](../../docs/provenance/dm.md) | high | prior art | 89 / 89 | 15 | 2 / 0 |
| `ser` | [format note](../../docs/formats/ser.md), [provenance log](../../docs/provenance/ser.md) | medium | prior art | 21 / 21 | 4 | 1 / 0 |
| `emd` | [format note](../../docs/formats/emd.md), [provenance log](../../docs/provenance/emd.md) | high | prior art | 32 / 32 | 9 | 1 / 0 |

### Source map

| file | what it does (its module documentation) |
| --- | --- |
| [`src/assurance.rs`](src/assurance.rs) | Assurance profiles (`docs/assurance.md`) of the electron-microscopy readers (MRC, Gatan DM, TIA SER, Velox EMD): the variant features of a file and the feature values the developme… |
| [`src/dm/dataset.rs`](src/dm/dataset.rs) | `Dataset` for DM3/DM4 files: images from `ImageList`, thumbnails as attachments |
| [`src/dm/dm5.rs`](src/dm/dm5.rs) | DM5: the same tag tree as DM3/DM4, stored in HDF5 (`docs/formats/dm.md`, "DM5") |
| [`src/dm/mod.rs`](src/dm/mod.rs) | Gatan Digital Micrograph DM3 / DM4 / DM5 reader |
| [`src/dm/tags.rs`](src/dm/tags.rs) | The DM3/DM4 tag tree: header, tag groups and data tags (see `docs/formats/dm.md`) |
| [`src/emd/berkeley.rs`](src/emd/berkeley.rs) | NCEM/Berkeley EMD (the open "Electron Microscopy Dataset" HDF5 convention, versions 0.2 and 1.0): data groups marked by an `emd_group_type` attribute (1 in 0.2, `"array"` in 1.0) h… |
| [`src/emd/dataset.rs`](src/emd/dataset.rs) | `Dataset` for Velox EMD files (HDF5 through `hdf5-pure`) |
| [`src/emd/mod.rs`](src/emd/mod.rs) | EMD (HDF5-based) reader: Thermo Fisher Velox EMD images through the pure-Rust `hdf5-pure` crate |
| [`src/emd/spectra.rs`](src/emd/spectra.rs) | Velox EDS data: detector spectra (`Data/Spectrum/<id>`) and the X-ray event streams (`Data/SpectrumStream/<id>`) from which spectrum images are assembled |
| [`src/lib.rs`](src/lib.rs) | Clean-room readers for electron-microscopy formats, registered as separate formats: |
| [`src/mrc/dataset.rs`](src/mrc/dataset.rs) | `Dataset` for MRC/CCP4 files: normalized metadata, listing, plane reads, integrity checks |
| [`src/mrc/ext.rs`](src/mrc/ext.rs) | Extended headers: FEI1/FEI2 metadata blocks (layout from mrcfile's BSD-licensed `dtypes.py`), SerialEM `SERI` section records and Agard-style integer/real records (IMOD documentati… |
| [`src/mrc/header.rs`](src/mrc/header.rs) | The 1024-byte MRC2014 main header (CCP-EM specification; see `docs/formats/mrc.md`) |
| [`src/mrc/mod.rs`](src/mrc/mod.rs) | MRC / CCP4 / MAP reader (MRC2014, CCP-EM specification) |
| [`src/ser/dataset.rs`](src/ser/dataset.rs) | `Dataset` for TIA series files: one image per `.ser` (elements as T, or as rows for 1-D elements), metadata from the `.emi` sidecar when present |
| [`src/ser/emi.rs`](src/ser/emi.rs) | The `.emi` sidecar: only its embedded `<ObjectInfo>` XML document is read |
| [`src/ser/file.rs`](src/ser/file.rs) | TIA / ES Vision series file (`.ser`) structure: header, dimension array, offset arrays, element headers and tags (see `docs/formats/ser.md`) |
| [`src/ser/mod.rs`](src/ser/mod.rs) | TIA / ES Vision series files (`.ser`) and their `.emi` sidecars |
| [`src/util.rs`](src/util.rs) | Crate-private helpers shared by the EM readers: bounded file reads, byte-order names, half-float widening and timestamp conversion |

### Where variants branch

The assurance profile ([`src/assurance.rs`](src/assurance.rs)) observes these features (each value is looked up in the validated table below; a value never confirmed makes the file `unvalidated` for the feature's outputs) and reports these structures and assumptions:

- feature format_version `version`
- feature sample_layout `im.pixel_type.ome_name()`
- feature sample_layout `format!("mode {m}")`
- feature layout `l`
- feature record `format!("extended header {t}")`
- feature layout `"permuted axes"`
- feature codec `"gzip"`
- feature sample_layout `"unsigned mode 0 (IMOD)"`
- feature format_version `v`
- feature sample_layout `d`
- feature layout `format!("{}-D", dims.len())`
- feature layout `format!("{k} elements")`
- feature layout `"without .emi"`
- feature layout `"with .emi"`
- feature layout `"multi-frame"`
- feature layout `"eds spectrum image"`
- feature codec `"event stream uint16"`
- feature record `"eds spectra"`
- feature layout `"eels spectrum image"`
- feature acquisition `m` (descriptive)
- feature record `format!("detector {d}")` (descriptive)
- undecoded "truncated data"
- undecoded "unsupported DataType"
- undecoded "complex elements"
- undecoded "Velox data other than images and EDS spectra"
- assumed "images[].physical_size"
- assumed "images[].size_t"

### Validated variants and the corpus files that pin them

| format | kind | value | outputs | confirmed files | read | example corpus files |
| --- | --- | --- | --- | --- | --- | --- |
| `dm` | acquisition | `DIFFRACTION` | descriptive | 3 | 3 | `rsciio-dm-2d-test-diffraction-pattern-dm3`, `zenodo13821437-Figure-2e`, `zenodo8045363-fem-TbCoSiN-stack` |
| `dm` | acquisition | `GIF SCANNING` | descriptive | 2 | 2 | `rsciio-dm-3d-eels-si-dm4`, `zenodo7401985-ADF-Image` |
| `dm` | acquisition | `IMAGING` | descriptive | 3 | 3 | `ncem-dm-08-carbon-dm3`, `zenodo11471263-wte2-HRTEM`, `zenodo8398370-NF-0001` |
| `dm` | acquisition | `SCANNING` | descriptive | 12 | 12 | `rsciio-dm-1d-test-eds-spectrum-dm3`, `rsciio-dm-1d-test-eels-spectrum-dm3`, `rsciio-dm-1d-test-monarccl-spectrum-ccd-dm4` |
| `dm` | acquisition | `STEM` | descriptive | 2 | 2 | `zenodo13913066-CS-Co-EELS-SI`, `zenodo13913066-Core-DF` |
| `dm` | field | `experiment.acquisition.started_at` | descriptive | 10 | 10 | `rsciio-dm-2d-multi-signal-dm3`, `rsciio-dm-2d-test-diffraction-pattern-dm3`, `rsciio-dm-2d-test-monarccl-spectrum-si-dm4` |
| `dm` | field | `experiment.instrument.model` | descriptive | 20 | 20 | `ncem-dm-08-carbon-dm3`, `rsciio-dm-1d-test-eds-spectrum-dm3`, `rsciio-dm-1d-test-eels-spectrum-dm3` |
| `dm` | format_version | `DM3` | metadata, pixels | 40 | 40 | `ncem-dm-08-carbon-dm3`, `ncem-dm-dmtest-3d-int16-64-65-66-dm3`, `ncem-dm-dmtest-float32-nonsquare-diffpixelsize-dm3` |
| `dm` | format_version | `DM4` | metadata, pixels | 41 | 41 | `ncem-dm-dmtest-3d-int16-64-65-66-dm4`, `ncem-dm-dmtest-float32-nonsquare-diffpixelsize-dm4`, `nion-dm-ref-f-0-1-dm4` |
| `dm` | format_version | `DM5` | metadata, pixels | 8 | 8 | `nion-dm-ref-f-0-1-dm5`, `nion-dm-ref-f-0-2-dm5`, `nion-dm-ref-f-1-1-dm5` |
| `dm` | instrument | `FEI Tecnai` | descriptive | 3 | 3 | `rsciio-dm-2d-test-diffraction-pattern-dm3`, `zenodo13913066-CS-Co-EELS-SI`, `zenodo13913066-Core-DF` |
| `dm` | instrument | `FEI Tecnai Remote` | descriptive | 6 | 6 | `rsciio-dm-1d-test-eds-spectrum-dm3`, `rsciio-dm-1d-test-eels-spectrum-dm3`, `rsciio-dm-2d-multi-signal-dm3` |
| `dm` | instrument | `FEI Tecnai Remote TCPIP` | descriptive | 2 | 2 | `zenodo20131420-SI-nanoFe`, `zenodo7401985-ADF-Image` |
| `dm` | instrument | `JEOL COM` | descriptive | 3 | 3 | `zenodo11471263-wte2-HRTEM`, `zenodo2580185-Gatan-STEM-Image`, `zenodo8398370-NF-0001` |
| `dm` | instrument | `NCEM TEAM 0.5` | descriptive | 1 | 1 | `ncem-dm-08-carbon-dm3` |
| `dm` | instrument | `TitanX` | descriptive | 2 | 2 | `zenodo13821437-Figure-2e`, `zenodo8045363-fem-TbCoSiN-stack` |
| `dm` | instrument | `Unknown` | descriptive | 2 | 2 | `zenodo8190744-EELS-STO`, `zenodo8403583-apatite-lowloss-SI` |
| `dm` | instrument | `Zeiss SEM COM` | descriptive | 3 | 3 | `rsciio-dm-1d-test-monarccl-spectrum-ccd-dm4`, `rsciio-dm-2d-test-monarccl-spectrum-si-dm4`, `rsciio-dm-2d-test-monocl-spectrum-si-dm4` |
| `dm` | layout | `1-D` | pixels | 13 | 13 | `nion-dm-ref-f-0-1-dm3`, `nion-dm-ref-f-0-1-dm4`, `nion-dm-ref-f-0-1-dm5` |
| `dm` | layout | `2-D` | pixels | 49 | 49 | `ncem-dm-08-carbon-dm3`, `ncem-dm-dmtest-float32-nonsquare-diffpixelsize-dm3`, `ncem-dm-dmtest-float32-nonsquare-diffpixelsize-dm4` |
| `dm` | layout | `3-D` | pixels | 29 | 29 | `ncem-dm-dmtest-3d-int16-64-65-66-dm3`, `ncem-dm-dmtest-3d-int16-64-65-66-dm4`, `nion-dm-ref-f-1-1-dm3` |
| `dm` | sample_layout | `binary` | pixels | 2 | 2 | `rsciio-dm-2d-test-14-dm3`, `rsciio-dm-2d-test-14-dm4` |
| `dm` | sample_layout | `complex128` | pixels | 4 | 4 | `rsciio-dm-2d-test-13-dm3`, `rsciio-dm-2d-test-13-dm4`, `rsciio-dm-2d-test-28-dm4` |
| `dm` | sample_layout | `complex64` | pixels | 4 | 4 | `rsciio-dm-2d-test-27-dm4`, `rsciio-dm-2d-test-3-dm3`, `rsciio-dm-2d-test-3-dm4` |
| `dm` | sample_layout | `float32` | pixels | 49 | 49 | `ncem-dm-08-carbon-dm3`, `ncem-dm-dmtest-float32-nonsquare-diffpixelsize-dm3`, `ncem-dm-dmtest-float32-nonsquare-diffpixelsize-dm4` |
| `dm` | sample_layout | `float64` | pixels | 2 | 2 | `rsciio-dm-2d-test-12-dm3`, `rsciio-dm-2d-test-12-dm4` |
| `dm` | sample_layout | `int16` | pixels | 4 | 4 | `ncem-dm-dmtest-3d-int16-64-65-66-dm3`, `ncem-dm-dmtest-3d-int16-64-65-66-dm4`, `rsciio-dm-2d-test-1-dm3` |
| `dm` | sample_layout | `int32` | pixels | 5 | 5 | `rsciio-dm-2d-test-7-dm3`, `rsciio-dm-2d-test-7-dm4`, `rsciio-dm-2d-test-diffraction-pattern-dm3` |
| `dm` | sample_layout | `int8` | pixels | 2 | 2 | `rsciio-dm-2d-test-9-dm3`, `rsciio-dm-2d-test-9-dm4` |
| `dm` | sample_layout | `packed-complex64` | pixels | 1 | 1 | `rsciio-dm-2d-test-fft-packed-complex8-dm4` |
| `dm` | sample_layout | `rgba` | pixels | 4 | 4 | `rsciio-dm-1d-test-23-dm3`, `rsciio-dm-2d-test-23-dm3`, `rsciio-dm-2d-test-23-dm4` |
| `dm` | sample_layout | `uint16` | pixels | 6 | 6 | `rsciio-dm-2d-multi-signal-dm3`, `rsciio-dm-2d-test-10-dm3`, `rsciio-dm-2d-test-10-dm4` |
| `dm` | sample_layout | `uint32` | pixels | 6 | 6 | `rsciio-dm-1d-test-eds-spectrum-dm3`, `rsciio-dm-2d-test-11-dm3`, `rsciio-dm-2d-test-11-dm4` |
| `dm` | sample_layout | `uint8` | pixels | 2 | 2 | `rsciio-dm-2d-test-6-dm3`, `rsciio-dm-2d-test-6-dm4` |
| `dm` | writer | `Gatan DigitalMicrograph` | descriptive | 89 | 89 | `ncem-dm-08-carbon-dm3`, `ncem-dm-dmtest-3d-int16-64-65-66-dm3`, `ncem-dm-dmtest-3d-int16-64-65-66-dm4` |
| `dm` | writer_version | `Gatan DigitalMicrograph 2.31` | descriptive | 6 | 6 | `rsciio-dm-1d-test-eds-spectrum-dm3`, `rsciio-dm-1d-test-eels-spectrum-dm3`, `rsciio-dm-2d-test-stem-image-dm3` |
| `dm` | writer_version | `Gatan DigitalMicrograph 2.32` | descriptive | 4 | 4 | `rsciio-dm-2d-multi-signal-dm3`, `rsciio-dm-3d-eels-si-dm4`, `zenodo8403583-apatite-HAADF` |
| `dm` | writer_version | `Gatan DigitalMicrograph 3.0` | descriptive | 2 | 2 | `ncem-dm-dmtest-3d-int16-64-65-66-dm3`, `ncem-dm-dmtest-3d-int16-64-65-66-dm4` |
| `dm` | writer_version | `Gatan DigitalMicrograph 3.20` | descriptive | 2 | 2 | `ncem-dm-dmtest-float32-nonsquare-diffpixelsize-dm3`, `ncem-dm-dmtest-float32-nonsquare-diffpixelsize-dm4` |
| `dm` | writer_version | `Gatan DigitalMicrograph 3.21` | descriptive | 1 | 1 | `zenodo2580185-Gatan-STEM-Image` |
| `dm` | writer_version | `Gatan DigitalMicrograph 3.43` | descriptive | 1 | 1 | `zenodo8398370-NF-0001` |
| `dm` | writer_version | `Gatan DigitalMicrograph 3.44` | descriptive | 1 | 1 | `zenodo7401985-ADF-Image` |
| `dm` | writer_version | `Gatan DigitalMicrograph 3.50` | descriptive | 1 | 1 | `rsciio-dm-2d-test-monarccl-spectrum-si-dm4` |
| `dm` | writer_version | `Gatan DigitalMicrograph 3.51` | descriptive | 1 | 1 | `rsciio-dm-2d-test-fft-packed-complex8-dm4` |
| `dm` | writer_version | `Gatan DigitalMicrograph 3.53` | descriptive | 2 | 2 | `zenodo18016957-hBN-spectra-overlaid`, `zenodo20131420-SI-nanoFe` |
| `dm` | writer_version | `Gatan DigitalMicrograph 3.60` | descriptive | 1 | 1 | `zenodo14541027-SAED1-TEM-0003` |
| `emd` | codec | `event stream uint16` | pixels | 1 | 1 | `zenodo20131712-0017-SI-Nano-1.4um` |
| `emd` | field | `experiment.acquisition.started_at` | descriptive | 17 | 17 | `ncem-emd-camera-ceta-diffraction-micro`, `ncem-emd-camera-ceta-imaging-micro`, `ncem-emd-stem-haadf-df4-df2-bf-diffraction-micro` |
| `emd` | field | `experiment.instrument.model` | descriptive | 17 | 17 | `ncem-emd-camera-ceta-diffraction-micro`, `ncem-emd-camera-ceta-imaging-micro`, `ncem-emd-stem-haadf-df4-df2-bf-diffraction-micro` |
| `emd` | format_version | `Berkeley EMD` | metadata, pixels | 4 | 4 | `rsciio-emd-si100-1x1x3-zstart5.43`, `rsciio-emd-si100-2x1x1-3d`, `rsciio-emd-si100-3d` |
| `emd` | format_version | `Berkeley EMD 0.2` | metadata, pixels | 5 | 5 | `ncem-emd-acquisition-18`, `ncem-emd-pt-saed-d910mm-single`, `ncem-emd-type1-stringdims` |
| `emd` | format_version | `Berkeley EMD null.null` | metadata, pixels | 4 | 4 | `rsciio-emd-example-image`, `rsciio-emd-example-metadata`, `rsciio-emd-example-signal` |
| `emd` | format_version | `Velox 10` | metadata, pixels | 5 | 5 | `zenodo20040988-0050-STEM-15.4nm`, `zenodo20131420-0028-Camera-Micro`, `zenodo20131420-0030-STEM-Nano-2.79um` |
| `emd` | format_version | `Velox 11` | metadata, pixels | 4 | 4 | `zenodo18267156-STEM-EELS-1-DyScO3`, `zenodo19965490-graphene-2`, `zenodo21895889-0041-STEM-HAADF-BF` |
| `emd` | format_version | `Velox 7` | metadata, pixels | 2 | 2 | `zenodo18685575-1238-42kx-LAADF`, `zenodo18685575-1243-164kx-LAADF` |
| `emd` | format_version | `Velox 8` | metadata, pixels | 5 | 5 | `ncem-emd-camera-ceta-diffraction-micro`, `ncem-emd-camera-ceta-imaging-micro`, `ncem-emd-stem-haadf-df4-df2-bf-diffraction-micro` |
| `emd` | format_version | `Velox 9` | metadata, pixels | 3 | 3 | `rsciio-emd-fei-example-dpc-titles`, `rsciio-emd-fftcomplexeven`, `rsciio-emd-fftcomplexodd` |
| `emd` | instrument | `Spectra` | descriptive | 9 | 9 | `rsciio-emd-fftcomplexeven`, `rsciio-emd-fftcomplexodd`, `zenodo18267156-STEM-EELS-1-DyScO3` |
| `emd` | instrument | `Titan` | descriptive | 8 | 8 | `ncem-emd-camera-ceta-diffraction-micro`, `ncem-emd-camera-ceta-imaging-micro`, `ncem-emd-stem-haadf-df4-df2-bf-diffraction-micro` |
| `emd` | layout | `eds spectrum image` | pixels | 1 | 1 | `zenodo20131712-0017-SI-Nano-1.4um` |
| `emd` | layout | `eels spectrum image` | metadata, pixels | 1 | 1 | `zenodo18267156-STEM-EELS-1-DyScO3` |
| `emd` | layout | `multi-frame` | pixels | 2 | 2 | `rsciio-emd-fei-example-tem-stack`, `zenodo20131712-0017-SI-Nano-1.4um` |
| `emd` | record | `detector BF` | descriptive | 1 | 1 | `ncem-emd-stem-haadf-df4-df2-bf-diffraction-micro` |
| `emd` | record | `detector BF-S` | descriptive | 1 | 1 | `zenodo21895889-0041-STEM-HAADF-BF` |
| `emd` | record | `detector BM-Ceta` | descriptive | 5 | 5 | `ncem-emd-camera-ceta-diffraction-micro`, `ncem-emd-camera-ceta-imaging-micro`, `rsciio-emd-fei-example-complex-fft` |
| `emd` | record | `detector DF-S` | descriptive | 1 | 1 | `zenodo18267156-STEM-EELS-1-DyScO3` |
| `emd` | record | `detector DF2` | descriptive | 1 | 1 | `ncem-emd-stem-haadf-df4-df2-bf-diffraction-micro` |
| `emd` | record | `detector DF4` | descriptive | 2 | 2 | `ncem-emd-stem-haadf-df4-df2-bf-diffraction-micro`, `rsciio-emd-fei-example-dpc-titles` |
| `emd` | record | `detector EELS Strip Detector` | descriptive | 1 | 1 | `zenodo18267156-STEM-EELS-1-DyScO3` |
| `emd` | record | `detector HAADF` | descriptive | 10 | 10 | `ncem-emd-stem-haadf-df4-df2-bf-diffraction-micro`, `rsciio-emd-fftcomplexeven`, `rsciio-emd-fftcomplexodd` |
| `emd` | record | `detector SuperXG2` | descriptive | 1 | 1 | `zenodo20131712-0017-SI-Nano-1.4um` |
| `emd` | record | `detector SuperXG23 + SuperXG21 + SuperXG24 + SuperXG22` | descriptive | 1 | 1 | `zenodo20131712-0017-SI-Nano-1.4um` |
| `emd` | record | `eds spectra` | traces | 3 | 3 | `zenodo20131712-0002-Spectrum-Micro`, `zenodo20131712-0017-SI-Nano-1.4um`, `zenodo21895889-0043-Spectrum-EDS` |
| `emd` | sample_layout | `complex` | pixels | 4 | 4 | `rsciio-emd-fei-example-complex-fft`, `rsciio-emd-fei-example-dpc-titles`, `rsciio-emd-fftcomplexeven` |
| `emd` | sample_layout | `double` | pixels | 1 | 1 | `rsciio-emd-example-axis-len-1` |
| `emd` | sample_layout | `float` | pixels | 7 | 7 | `rsciio-emd-fei-example-dpc-titles`, `rsciio-emd-si100-1x1x3-zstart5.43`, `rsciio-emd-si100-2x1x1-3d` |
| `emd` | sample_layout | `int16` | pixels | 4 | 4 | `ncem-emd-camera-ceta-diffraction-micro`, `ncem-emd-camera-ceta-imaging-micro`, `rsciio-emd-fei-example-tem-stack` |
| `emd` | sample_layout | `int32` | pixels | 5 | 5 | `ncem-emd-pt-saed-d910mm-single`, `rsciio-emd-example-image`, `rsciio-emd-example-metadata` |
| `emd` | sample_layout | `int64` | pixels | 2 | 2 | `ncem-emd-type1-stringdims`, `rsciio-emd-example-bytes-string-metadata` |
| `emd` | sample_layout | `uint16` | pixels | 13 | 13 | `ncem-emd-acquisition-18`, `ncem-emd-stem-haadf-df4-df2-bf-diffraction-micro`, `rsciio-emd-fei-example-dpc-titles` |

… 50 more values: the generated table in `src/assurance.rs` has all of them.

### Tests, fixtures, fuzz targets, snapshots

- integration tests: [`tests/synthetic_emd.rs`](tests/synthetic_emd.rs), [`tests/synthetic_mrc.rs`](tests/synthetic_mrc.rs)
- committed fixtures: 1 files in [`tests/fixtures/`](tests/fixtures) (malformed ones are replayed through every reader by `openreadout`'s `tests/fuzz_regressions.rs`; all are snapshotted by its `tests/golden.rs`)
- fuzz targets (`fuzz/fuzz_targets/`): `whole_dm`, `whole_emd`, `whole_mrc`, `whole_ser`
- corpus inputs by tier: heldout 15, smoke 134, standard 21
- golden snapshots: [`corpus/snapshots/mrc.jsonl`](../../corpus/snapshots/mrc.jsonl), [`corpus/snapshots/dm.jsonl`](../../corpus/snapshots/dm.jsonl), [`corpus/snapshots/ser.jsonl`](../../corpus/snapshots/ser.jsonl), [`corpus/snapshots/emd.jsonl`](../../corpus/snapshots/emd.jsonl)

### Open new-variant intakes

None.
<!-- END GENERATED guide -->
