# Maintaining `openreadout-medical`

Formats: `dicom`, `nifti`, `bruker-paravision`, one module directory each. Notes: `docs/formats/{dicom,nifti,bruker-paravision}.md`; provenance: `docs/provenance/` under the same names. **Clean room**: DICOM comes from the public standard (PS3.3, 3.5, 3.6, 3.10, 3.15); NIfTI from the public `nifti1.h`/`nifti2.h` documentation; ParaVision from public parameter files and brukerapi (MIT). BrkRaw is GPL and was only run as a black box. Start with [docs/maintaining.md](../../docs/maintaining.md) for the project-wide process.

## Decode pipeline

- **DICOM** (`dicom/`): `parse.rs` reads the preamble and file meta group, then the data set (implicit or explicit VR, little or big endian, deflated data sets inflated first). Sequences of undefined length nest, and an explicit `UN` for a tag the dictionary knows is read with the dictionary's VR. `info` stops at the pixel data (`read_head`). `file.rs` holds one parsed file, `pixels.rs` the frame table (native frames, or encapsulated fragments with the basic and extended offset tables) and `decode_frame`, which **branches on the transfer syntax** (`dictionary.rs`: native, deflated frames, RLE, JPEG baseline/extended/lossless, JPEG-LS, JPEG 2000 and HTJ2K through `openreadout-codecs`). `values.rs` applies the modality rescale or LUT and palettes. `series.rs` groups files into images (series UID, size, photometric interpretation, orientation) and **picks the arrangement**: volume, volume series, cine, stack or segmentation. `wsi.rs` assembles VL whole-slide instances into a pyramid. `phi.rs` withholds personal data, `vendor.rs` writes `info --view full`, and `dataset.rs` ties it together for files, folders and DICOMDIRs.
- **NIfTI** (`nifti/`): `header.rs` reads NIfTI-1, NIfTI-2 and Analyze 7.5 headers (**the magic and `sizeof_hdr` branch here**, in either byte order). `dataset.rs` reads `.nii`, `.nii.gz` and `.hdr`/`.img` pairs, applies `scl_slope`/`scl_inter`, and works out the orientation from the sform or qform. `write.rs` is the NIfTI exporter (`export_nifti`).
- **ParaVision** (`paravision/`): `params.rs` parses the JCAMP-style parameter files (`visu_pars`, `reco`, `method`, `acqp`). `mod.rs` finds reconstructions from a `2dseq`, a `pdata/N` folder, an experiment or a study. `dataset.rs` maps the frame groups (`VisuFGOrderDesc`, first group fastest) to z, t and c, applies `VisuCoreDataSlope`/`VisuCoreDataOffs`, and turns 1-D spectroscopy into traces.

## Invariants and checks

- DICOM: the `DICM` magic (or a guessed raw data set), element lengths inside the file or item, sequence nesting depth and element count limits, a known transfer syntax, frame sizes that match rows × columns × samples × bits, fragments inside the file. `check` decodes the first and last frame of every image and reports `undecodable_frame`, `missing_slices`, `missing_file` (named by a DICOMDIR), `truncated` and `trailing_bytes`. Unknown compressed syntaxes are exit 6.
- NIfTI: `sizeof_hdr` 348 or 540, dimensions positive and their product inside the limit, `vox_offset` past the header and data inside the file, a supported `datatype` (exit 6 for binary, float128 and complex256).
- ParaVision: `visu_pars` parses, the core size times the frame count times the word size equals the `2dseq` length, and the frame groups multiply out to the frame count.
- Malformed input is an error with a hint, never a panic: `tests/synthetic.rs` and the fuzz targets `whole_dicom`, `whole_nifti` and `whole_paravision` cover this.

## Debugging a new file

- `openreadout info FILE --json` shows the arrangement, rescale, windows, orientation and `personal_data`. `info --view full --json` lists every DICOM element by keyword (personal data withheld), the NIfTI header fields, or the ParaVision parameters.
- `cargo run -p openreadout-medical --example probe -- FILE` prints what the readers make of a file, and `--example dump_plane` writes one plane's values.
- Oracles: pydicom with pylibjpeg and pyjpegls (DICOM), nibabel (NIfTI and Analyze), brukerapi (ParaVision), all in `oracle/medical.py`.
- `tests/synthetic.rs` builds small DICOM, NIfTI, Analyze and ParaVision files. Copy one of its builders to pin a new structure.

## Fragile spots

- DICOM grouping: files that share a series UID but differ in size or orientation become separate images, and multi-frame or unpositioned files stay on their own. Slices are sorted by position along the slice normal, then by instance number.
- JPEG colour: a JFIF or Adobe marker decides the colour transform first, then the photometric interpretation. Lossless JPEG never transforms. Files that break this rule decode with the wrong colours.
- Pixel spacing falls back to Imager Pixel Spacing and then to the first ultrasound region calibrated in centimetres. `extra.pixel_spacing_from` says which one was used.
- The rescale output type is int16, uint16 or int32 when slope and intercept keep values integral, otherwise float32.
- NIfTI orientation letters are converted from RAS to the LPS letters DICOM uses. Dimensions 6 and 7 are folded into T.
- ParaVision: frame groups other than slices, cycles and movies become channels. `FG_COMPLEX` gives a real and an imaginary channel. Spectral axes get no physical size.

<!-- BEGIN GENERATED guide -->
## Facts (generated)

*Generated by `cargo xtask guides --write` from the sources, `corpus/manifest.toml`, `corpus/assurance/evidence.json` and `corpus/intake/`; do not edit. CI fails when it is stale.*

### Formats

| format id | notes and provenance | confidence | basis | development files: read / confirmed | depositors | held-out pass / fail |
| --- | --- | --- | --- | --- | --- | --- |
| `dicom` | [format note](../../docs/formats/dicom.md), [provenance log](../../docs/provenance/dicom.md) | medium | open spec | 67 / 67 | 5 | - |
| `nifti` | [format note](../../docs/formats/nifti.md), [provenance log](../../docs/provenance/nifti.md) | medium | open spec | 8 / 8 | 2 | - |
| `bruker-paravision` | [format note](../../docs/formats/bruker-paravision.md), [provenance log](../../docs/provenance/bruker-paravision.md) | low | prior art | 5 / 5 | 1 | - |

### Source map

| file | what it does (its module documentation) |
| --- | --- |
| [`src/assurance.rs`](src/assurance.rs) | Assurance profiles (`docs/assurance.md`) of the medical-imaging readers (DICOM, NIfTI / Analyze, Bruker ParaVision): the variant features of a file and the feature values the |
| [`src/dicom/dataset.rs`](src/dicom/dataset.rs) | `Dataset` for DICOM files and folders: images, metadata, plane and region reads, checks |
| [`src/dicom/dictionary.rs`](src/dicom/dictionary.rs) | Look-ups in the data dictionary (`dictionary_table.rs`, generated from PS3.6) and the UID registry (PS3.6 Annex A) for the transfer syntaxes and storage classes this reader names |
| [`src/dicom/dictionary_table.rs`](src/dicom/dictionary_table.rs) | The DICOM data dictionary: tag, keyword and value representation of every element registered in DICOM PS3.6 2026d - Data Dictionary (NEMA, tables 6-1, 7-1 and 8-1) |
| [`src/dicom/element.rs`](src/dicom/element.rs) | Reading values out of parsed elements (PS3.5 §6.2: value representations) |
| [`src/dicom/file.rs`](src/dicom/file.rs) | One DICOM file: its meta group, transfer syntax, data set (up to the pixel data) and where the pixel data is |
| [`src/dicom/mod.rs`](src/dicom/mod.rs) | DICOM reader (NEMA PS3, an open standard): single files, series and study folders, DICOMDIR media, classic and enhanced multi-frame images, and whole-slide microscopy pyramids |
| [`src/dicom/parse.rs`](src/dicom/parse.rs) | The DICOM byte stream (PS3.5 §7, PS3.10 §7): the file preamble and meta group, data elements in implicit or explicit VR and either byte order, sequences and items of defined |
| [`src/dicom/phi.rs`](src/dicom/phi.rs) | Patient-identifying elements (PS3.15 Annex E, Basic Application Level Confidentiality Profile) |
| [`src/dicom/pixels.rs`](src/dicom/pixels.rs) | Pixel data: the Image Pixel module (PS3.3 C.7.6.3), native frames (PS3.5 §8.1), the frames of encapsulated pixel data (PS3.5 A.4), the decoders of each transfer syntax, and colour |
| [`src/dicom/series.rs`](src/dicom/series.rs) | From files and frames to images: per-frame geometry and value transforms (classic headers and multi-frame functional groups, PS3.3 C.7.6.2 and C.7.6.16), grouping frames into |
| [`src/dicom/tags.rs`](src/dicom/tags.rs) | Tags the reader interprets (PS3.6 keywords in the constant names, converted to Rust case) |
| [`src/dicom/values.rs`](src/dicom/values.rs) | Value transforms applied to decoded frames: the Modality LUT (PS3.3 C.11.1: Rescale Slope and Intercept, or a LUT) that turns stored values into output units such as Hounsfield |
| [`src/dicom/vendor.rs`](src/dicom/vendor.rs) | The data set as JSON (`info --view full` → `vendor`): elements by keyword (private and unknown ones as `(gggg,eeee)`), text and numbers as values, binary values summarized, |
| [`src/dicom/wsi.rs`](src/dicom/wsi.rs) | DICOM whole-slide microscopy (VL Whole Slide Microscopy Image IOD, PS3.3 A.32.8 and C.8.12.4): each pyramid level is one or more instances of tiled frames placed in a total |
| [`src/lib.rs`](src/lib.rs) | Clean-room readers for medical and preclinical imaging, registered as separate formats: |
| [`src/nifti/dataset.rs`](src/nifti/dataset.rs) | `Dataset` for NIfTI and Analyze files |
| [`src/nifti/header.rs`](src/nifti/header.rs) | The NIfTI-1 (348 bytes), NIfTI-2 (540 bytes) and Analyze 7.5 (348 bytes) headers, as the NIfTI Data Format Working Group's public `nifti1.h` and `nifti2.h` lay them out, and the |
| [`src/nifti/mod.rs`](src/nifti/mod.rs) | NIfTI-1, NIfTI-2 and Analyze 7.5 reader (`.nii`, `.nii.gz`, `.hdr`/`.img` pairs) |
| [`src/nifti/write.rs`](src/nifti/write.rs) | NIfTI-1 writer for `export --format nifti`: one image as a single `.nii` (or `.nii.gz`) file, with its voxel sizes and, when the source records its orientation, a scanner-space |
| [`src/paravision/dataset.rs`](src/paravision/dataset.rs) | `Dataset` for ParaVision experiments and studies: one image (or spectrum) per reconstruction |
| [`src/paravision/mod.rs`](src/paravision/mod.rs) | Bruker ParaVision reader: reconstructed MRI data (`pdata/<procno>/2dseq`) described by `visu_pars`, with the acquisition's `method` and `acqp`, in an experiment folder |
| [`src/paravision/params.rs`](src/paravision/params.rs) | ParaVision parameter files (`visu_pars`, `method`, `acqp`, `reco`, `subject`): JCAMP-DX style records `##$Name=value`, with array sizes in brackets (`( 2 )` then the values on |
| [`src/util.rs`](src/util.rs) | Helpers shared by the readers: number formatting and small geometry |

### Where variants branch

The assurance profile ([`src/assurance.rs`](src/assurance.rs)) observes these features (each value is looked up in the validated table below; a value never confirmed makes the file `unvalidated` for the feature's outputs) and reports these structures and assumptions:

- feature codec `ts`
- feature record `c`
- feature layout `l`
- feature sample_layout `p`
- feature sample_layout `im.pixel_type.ome_name()`
- feature format_version `v`
- feature sample_layout `d`
- feature layout `d`
- feature codec `"gzip"`
- feature layout `g`
- feature acquisition `m` (descriptive)
- undecoded "segmented palette"
- assumed "images[].physical_size.z"

### Validated variants and the corpus files that pin them

| format | kind | value | outputs | confirmed files | read | example corpus files |
| --- | --- | --- | --- | --- | --- | --- |
| `bruker-paravision` | acquisition | `Bruker:B1Map` | descriptive | 2 | 2 | `brukerapi-pv7-lego-phantom-e6`, `brukerapi-pv7-lego-phantom-study` |
| `bruker-paravision` | acquisition | `Bruker:CSI` | descriptive | 1 | 1 | `brukerapi-pv7-lego-phantom-study` |
| `bruker-paravision` | acquisition | `Bruker:DESS` | descriptive | 1 | 1 | `brukerapi-pv7-lego-phantom-study` |
| `bruker-paravision` | acquisition | `Bruker:DtiEpi` | descriptive | 2 | 2 | `brukerapi-pv7-lego-phantom-e10`, `brukerapi-pv7-lego-phantom-study` |
| `bruker-paravision` | acquisition | `Bruker:DtiSpiral` | descriptive | 1 | 1 | `brukerapi-pv7-lego-phantom-study` |
| `bruker-paravision` | acquisition | `Bruker:DtiStandard` | descriptive | 1 | 1 | `brukerapi-pv7-lego-phantom-study` |
| `bruker-paravision` | acquisition | `Bruker:EPI` | descriptive | 1 | 1 | `brukerapi-pv7-lego-phantom-study` |
| `bruker-paravision` | acquisition | `Bruker:EPSI` | descriptive | 1 | 1 | `brukerapi-pv7-lego-phantom-study` |
| `bruker-paravision` | acquisition | `Bruker:FAIR_EPI` | descriptive | 1 | 1 | `brukerapi-pv7-lego-phantom-study` |
| `bruker-paravision` | acquisition | `Bruker:FAIR_RARE` | descriptive | 1 | 1 | `brukerapi-pv7-lego-phantom-study` |
| `bruker-paravision` | acquisition | `Bruker:FISP` | descriptive | 1 | 1 | `brukerapi-pv7-lego-phantom-study` |
| `bruker-paravision` | acquisition | `Bruker:FLASH` | descriptive | 1 | 1 | `brukerapi-pv7-lego-phantom-study` |
| `bruker-paravision` | acquisition | `Bruker:FLOWMAP` | descriptive | 2 | 2 | `brukerapi-pv7-lego-phantom-e20`, `brukerapi-pv7-lego-phantom-study` |
| `bruker-paravision` | acquisition | `Bruker:FcFLASH` | descriptive | 1 | 1 | `brukerapi-pv7-lego-phantom-study` |
| `bruker-paravision` | acquisition | `Bruker:FieldMap` | descriptive | 1 | 1 | `brukerapi-pv7-lego-phantom-study` |
| `bruker-paravision` | acquisition | `Bruker:IgFLASH` | descriptive | 1 | 1 | `brukerapi-pv7-lego-phantom-study` |
| `bruker-paravision` | acquisition | `Bruker:MDEFT` | descriptive | 1 | 1 | `brukerapi-pv7-lego-phantom-study` |
| `bruker-paravision` | acquisition | `Bruker:MGE` | descriptive | 1 | 1 | `brukerapi-pv7-lego-phantom-study` |
| `bruker-paravision` | acquisition | `Bruker:MSME` | descriptive | 1 | 1 | `brukerapi-pv7-lego-phantom-study` |
| `bruker-paravision` | acquisition | `Bruker:RARE` | descriptive | 1 | 1 | `brukerapi-pv7-lego-phantom-study` |
| `bruker-paravision` | acquisition | `Bruker:RAREVTR` | descriptive | 1 | 1 | `brukerapi-pv7-lego-phantom-study` |
| `bruker-paravision` | acquisition | `Bruker:RAREst` | descriptive | 1 | 1 | `brukerapi-pv7-lego-phantom-study` |
| `bruker-paravision` | acquisition | `Bruker:SPIRAL` | descriptive | 1 | 1 | `brukerapi-pv7-lego-phantom-study` |
| `bruker-paravision` | acquisition | `Bruker:SegFLASH` | descriptive | 1 | 1 | `brukerapi-pv7-lego-phantom-study` |
| `bruker-paravision` | acquisition | `Bruker:T1_EPI` | descriptive | 1 | 1 | `brukerapi-pv7-lego-phantom-study` |
| `bruker-paravision` | acquisition | `Bruker:T2S_EPI` | descriptive | 1 | 1 | `brukerapi-pv7-lego-phantom-study` |
| `bruker-paravision` | acquisition | `Bruker:T2_EPI` | descriptive | 1 | 1 | `brukerapi-pv7-lego-phantom-study` |
| `bruker-paravision` | acquisition | `Bruker:UTE` | descriptive | 1 | 1 | `brukerapi-pv7-lego-phantom-study` |
| `bruker-paravision` | acquisition | `Bruker:UTE3D` | descriptive | 1 | 1 | `brukerapi-pv7-lego-phantom-study` |
| `bruker-paravision` | acquisition | `Bruker:ZTE` | descriptive | 1 | 1 | `brukerapi-pv7-lego-phantom-study` |
| `bruker-paravision` | field | `experiment.instrument.model` | descriptive | 4 | 4 | `brukerapi-pv7-lego-phantom-e10`, `brukerapi-pv7-lego-phantom-e20`, `brukerapi-pv7-lego-phantom-e6` |
| `bruker-paravision` | format_version | `ParaVision 7.0.0` | metadata, pixels | 5 | 5 | `brukerapi-pv7-lego-phantom-e10`, `brukerapi-pv7-lego-phantom-e20`, `brukerapi-pv7-lego-phantom-e35` |
| `bruker-paravision` | layout | `FG_CARDIAC_MOVIE` | pixels | 1 | 1 | `brukerapi-pv7-lego-phantom-study` |
| `bruker-paravision` | layout | `FG_COMPLEX` | pixels | 1 | 1 | `brukerapi-pv7-lego-phantom-study` |
| `bruker-paravision` | layout | `FG_CYCLE` | pixels | 1 | 1 | `brukerapi-pv7-lego-phantom-study` |
| `bruker-paravision` | layout | `FG_DIFFUSION` | pixels | 2 | 2 | `brukerapi-pv7-lego-phantom-e10`, `brukerapi-pv7-lego-phantom-study` |
| `bruker-paravision` | layout | `FG_ECHO` | pixels | 2 | 2 | `brukerapi-pv7-lego-phantom-e6`, `brukerapi-pv7-lego-phantom-study` |
| `bruker-paravision` | layout | `FG_FLOW` | pixels | 2 | 2 | `brukerapi-pv7-lego-phantom-e20`, `brukerapi-pv7-lego-phantom-study` |
| `bruker-paravision` | layout | `FG_IRMODE` | pixels | 1 | 1 | `brukerapi-pv7-lego-phantom-study` |
| `bruker-paravision` | layout | `FG_MOVIE` | pixels | 1 | 1 | `brukerapi-pv7-lego-phantom-study` |
| `bruker-paravision` | layout | `FG_SLICE` | pixels | 2 | 2 | `brukerapi-pv7-lego-phantom-e6`, `brukerapi-pv7-lego-phantom-study` |
| `bruker-paravision` | layout | `frames` | pixels | 1 | 1 | `brukerapi-pv7-lego-phantom-study` |
| `bruker-paravision` | sample_layout | `_16BIT_SGN_INT` | pixels | 3 | 3 | `brukerapi-pv7-lego-phantom-e10`, `brukerapi-pv7-lego-phantom-e20`, `brukerapi-pv7-lego-phantom-study` |
| `bruker-paravision` | sample_layout | `_32BIT_SGN_INT` | pixels | 2 | 2 | `brukerapi-pv7-lego-phantom-e6`, `brukerapi-pv7-lego-phantom-study` |
| `dicom` | acquisition | `CR` | descriptive | 2 | 2 | `pydicom-data-rg1-j2ki`, `pydicom-dicomdirtests` |
| `dicom` | acquisition | `CT` | descriptive | 7 | 7 | `idc-cmb-crc-ct-head`, `idc-stageii-colorectal-ct-venous`, `pydicom-ct-small` |
| `dicom` | acquisition | `DX` | descriptive | 1 | 1 | `idc-rider-pilot-dx` |
| `dicom` | acquisition | `MR` | descriptive | 18 | 18 | `idc-upenn-gbm-mr-t1-post`, `pydicom-data-emri-small`, `pydicom-data-emri-small-big-endian` |
| `dicom` | acquisition | `NM` | descriptive | 4 | 4 | `idc-varepop-nm`, `pydicom-data-jpeg-ll`, `pydicom-jpeg2000` |
| `dicom` | acquisition | `OT` | descriptive | 9 | 9 | `pydicom-data-mlut-18`, `pydicom-data-parametric-map-double-float`, `pydicom-data-parametric-map-float` |
| `dicom` | acquisition | `PT` | descriptive | 1 | 1 | `idc-rider-phantom-pet` |
| `dicom` | acquisition | `RF` | descriptive | 1 | 1 | `idc-varepop-rf` |
| `dicom` | acquisition | `RTDOSE` | descriptive | 2 | 2 | `pydicom-rtdose`, `pydicom-rtdose-rle` |
| `dicom` | acquisition | `SEG` | descriptive | 5 | 5 | `idc-upenn-gbm-seg`, `pydicom-data-liver`, `pydicom-data-liver-deflate` |
| `dicom` | acquisition | `SM` | descriptive | 2 | 2 | `openslide-dicom-cmu-1-jp2k-33005-v2-slide`, `openslide-dicom-leica-4-slide` |
| `dicom` | acquisition | `US` | descriptive | 10 | 10 | `idc-remind-us`, `pydicom-data-color3d-jpeg-baseline`, `pydicom-data-gdcm-us-aloka-16` |
| `dicom` | codec | `Deflated Explicit VR Little Endian` | pixels | 1 | 1 | `pydicom-image-dfl` |
| `dicom` | codec | `Deflated Image Frame Compression` | pixels | 1 | 1 | `pydicom-data-liver-deflate` |
| `dicom` | codec | `Explicit VR Big Endian` | pixels | 3 | 3 | `pydicom-data-emri-small-big-endian`, `pydicom-mr-small-bigendian`, `pydicom-sc-rgb-small-odd-big-endian` |
| `dicom` | codec | `Explicit VR Little Endian` | pixels | 28 | 28 | `idc-cmb-crc-ct-head`, `idc-remind-us`, `idc-rider-phantom-pet` |
| `dicom` | codec | `High-Throughput JPEG 2000 Image Compression` | pixels | 1 | 1 | `pydicom-data-htj2k-08-rgb` |
| `dicom` | codec | `High-Throughput JPEG 2000 Image Compression (Lossless Only)` | pixels | 1 | 1 | `pydicom-data-htj2klossless-08-rgb` |
| `dicom` | codec | `Implicit VR Little Endian` | pixels | 5 | 5 | `idc-upenn-gbm-mr-t1-post`, `pydicom-data-gdcm-us-aloka-16`, `pydicom-mr-small-implicit` |
| `dicom` | codec | `JPEG 2000 Image Compression` | pixels | 5 | 5 | `openslide-dicom-cmu-1-jp2k-33005-v2-slide`, `pydicom-data-mr2-j2ki`, `pydicom-data-rg1-j2ki` |
| `dicom` | codec | `JPEG 2000 Image Compression (Lossless Only)` | pixels | 4 | 4 | `pydicom-data-693-j2kr`, `pydicom-data-emri-small-jpeg-2k-lossless`, `pydicom-data-mr2-j2kr` |
| `dicom` | codec | `JPEG Baseline (Process 1)` | pixels | 4 | 4 | `openslide-dicom-leica-4-slide`, `pydicom-data-color3d-jpeg-baseline`, `pydicom-examples-ybr-color` |
| `dicom` | codec | `JPEG Extended (Process 2 & 4)` | pixels | 1 | 1 | `pydicom-jpgextended` |
| `dicom` | codec | `JPEG Lossless, Non-Hierarchical, First-Order Prediction…` | pixels | 3 | 3 | `pydicom-data-bad-sequence`, `pydicom-data-jpeg-ll`, `pydicom-data-jpglosslessp14sv1-1s-1f-8b` |
| `dicom` | codec | `JPEG-LS Lossless Image Compression` | pixels | 2 | 2 | `pydicom-data-emri-small-jpeg-ls-lossless`, `pydicom-mr-small-jpeg-ls-lossless` |
| `dicom` | codec | `JPEG-LS Lossy (Near-Lossless) Image Compression` | pixels | 2 | 2 | `pydicom-jpeglsnearlossless-08`, `pydicom-jpeglsnearlossless-16` |
| `dicom` | codec | `RLE Lossless` | pixels | 6 | 6 | `pydicom-data-emri-small-rle`, `pydicom-data-liver-rle`, `pydicom-data-obxxxx1a-rle-2frame` |
| `dicom` | instrument | `ADC_5146` | descriptive | 1 | 1 | `pydicom-dicomdirtests` |
| `dicom` | instrument | `Aperio converted by com.pixelmed.convert.TIFFToDicom` | descriptive | 1 | 1 | `openslide-dicom-cmu-1-jp2k-33005-v2-slide` |
| `dicom` | instrument | `Aquilion PRIME` | descriptive | 1 | 1 | `idc-cmb-crc-ct-head` |
| `dicom` | instrument | `Avanto` | descriptive | 2 | 2 | `pydicom-data-mr-siemens-dicom-withoverlays`, `pydicom-examples-overlay` |
| `dicom` | instrument | `CX50` | descriptive | 3 | 3 | `pydicom-data-obxxxx1a`, `pydicom-data-obxxxx1a-rle-2frame`, `pydicom-examples-palette` |
| `dicom` | instrument | `Cassette Holder Type 9840 500 70201` | descriptive | 1 | 1 | `pydicom-data-rg1-j2ki` |
| `dicom` | instrument | `Discovery STE` | descriptive | 1 | 1 | `idc-rider-phantom-pet` |
| `dicom` | instrument | `EPIQ 7C` | descriptive | 1 | 1 | `pydicom-data-jpglosslessp14sv1-1s-1f-8b` |
| `dicom` | instrument | `Encore2` | descriptive | 1 | 1 | `idc-varepop-nm` |

… 63 more values: the generated table in `src/assurance.rs` has all of them.

### Tests, fixtures, fuzz targets, snapshots

- integration tests: [`tests/synthetic.rs`](tests/synthetic.rs)
- fuzz targets (`fuzz/fuzz_targets/`): `whole_dicom`, `whole_nifti`, `whole_paravision`
- corpus inputs by tier: smoke 62, standard 18
- golden snapshots: [`corpus/snapshots/dicom.jsonl`](../../corpus/snapshots/dicom.jsonl), [`corpus/snapshots/nifti.jsonl`](../../corpus/snapshots/nifti.jsonl), [`corpus/snapshots/bruker-paravision.jsonl`](../../corpus/snapshots/bruker-paravision.jsonl)

### Open new-variant intakes

None.
<!-- END GENERATED guide -->
