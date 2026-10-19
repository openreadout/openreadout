# DICOM

DICOM is how hospitals and many research scanners store CT, MR, PET, nuclear medicine, ultrasound, X-ray and, more and more, whole-slide microscopy. OpenReadout reads single files, series and study folders (with or without a DICOMDIR), classic and enhanced multi-frame images, segmentations and VL whole-slide images, and returns each series as one image with real voxel sizes, its orientation, and pixel values in output units (Hounsfield units for CT). Derived from the DICOM standard (NEMA PS3, free at https://dicom.nema.org); pydicom (MIT) is the reference reader. Provenance: `docs/provenance/dicom.md`.

## What a file is (PS3.10, PS3.5)

A Part 10 file is a 128-byte preamble, `DICM`, a file meta group (group 0002, always explicit VR little endian) that names the transfer syntax, then the data set. Files without the preamble (older media, some exports) start directly with a group 0002 or 0008 element; the reader recognises them by the first element and says so in `check`.

Data elements are a tag, a VR (explicit syntaxes) and a length. Sequences hold items of defined or undefined length; a UN element of undefined length holds an implicit-VR sequence (PS3.5 §6.2.2). Parsing stops at the top-level Pixel Data (7FE0,0010; also float 7FE0,0008 and double 7FE0,0009 pixel data): `info` reads headers only. Values above 256 KiB are left on disk and read when needed (LUT data, extended offset tables).

| transfer syntax | UID | pixel decoding |
| --- | --- | --- |
| Implicit / explicit VR little endian | 1.2.840.10008.1.2, .1.2.1 | native |
| Explicit VR big endian | 1.2.840.10008.1.2.2 | native, byte-swapped (8-bit OW samples swapped in pairs) |
| Deflated explicit VR little endian | 1.2.840.10008.1.2.1.99 | the data set is inflated at open |
| Encapsulated uncompressed | 1.2.840.10008.1.2.1.98 | native samples in fragments |
| JPEG baseline, extended, lossless (and retired 52-55) | 1.2.840.10008.1.2.4.50, .51, .57, .70 | `openreadout-codecs` JPEG (8, 12 and 16 bit) |
| JPEG-LS lossless and near-lossless | .4.80, .4.81 | `openreadout-codecs` JPEG-LS (one component) |
| JPEG 2000, HTJ2K, Part 2 | .4.90-.93, .4.201-.203 | `openreadout-codecs` JPEG 2000 |
| JPEG XL | .4.110-.112 | `openreadout-codecs` JPEG XL |
| RLE lossless | 1.2.840.10008.1.2.5 | PackBits segments per byte plane (Annex G) |
| Deflated image frame compression | 1.2.840.10008.1.2.8.1 | each frame inflated |
| MPEG-2, MPEG-4, HEVC | .4.100-.108 | not decoded (exit 6); headers read |

Encapsulated frames are found from the Extended Offset Table, one fragment per frame, the Basic Offset Table, or, failing those, where codestreams start (JPEG SOI, JPEG 2000 SOC/SIZ, JP2 and JPEG XL signatures).

JPEG colour: the stream's own JFIF or Adobe markers decide; without them, Photometric Interpretation RGB means the components are R, G, B as coded and the YBR interpretations are converted to RGB. Lossless JPEG has no colour transform. Native and RLE YBR_FULL and YBR_FULL_422 frames are converted with the full-range BT.601 equations of PS3.3 C.7.6.3.1.2.

## Pixel values

Samples are decoded at Bits Allocated, with the bits above Bits Stored cleared (unsigned) or sign-extended (signed), as pydicom does. Then:

- **Modality LUT** (PS3.3 C.11.1): Rescale Slope and Intercept, per frame in enhanced images (Pixel Value Transformation), or a Modality LUT Sequence, are **applied**. With whole-number slopes and intercepts the type is the smallest of int16, uint16 and int32 that holds every rescaled stored value (from Bits Stored), otherwise float32. CT values are Hounsfield units; `extra.value_unit` says the unit (`HU`, PET `Units` such as `BQML`, RT dose units). `extra.stored_pixel_type` keeps the stored type and `extra.rescale` the transform (`varies: true` when slices differ, as in PET).
- **Palette colour**: palettes, segmented ones included (C.7.9.2), are applied: 8-bit entries give uint8 R, G, B, 16-bit ones uint16.
- **MONOCHROME1**: values are returned as stored; `extra.display_inverted` tells previews to draw low values bright.
- 1-bit segmentation frames come back as uint8 0/1.

## Folders, series and images

A folder (or a DICOMDIR, or the DICOMDIR's folder) is read file by file, headers only. Every DICOM file found is read; a DICOMDIR's missing files are reported. Frames are grouped into images by Series Instance UID, frame size, sample layout and orientation; multi-frame files and projection images (no position) are images of their own.

Within an image (`extra.arrangement`):

| arrangement | when | Z and T |
| --- | --- | --- |
| `volume` | every frame has a different position along the slice normal | Z sorted by position · normal (normal = row × column direction) |
| `volume series` | positions repeat the same number of times (dynamic CT, fMRI, DCE) | Z by position, T by temporal position index, trigger time, acquisition time, instance number |
| `cine` | frames without positions in time order (Frame Time, Frame Increment Pointer to time, or US/XA/RF) | T |
| `segmentation` | SEG frames with Referenced Segment Number | C = segments (named by Segment Label), Z = the union of positions; absent frames read as zeros |
| `stack` | anything else | Z in instance and frame order |
| `slide pyramid` | VL Whole Slide Microscopy VOLUME instances | see below |

The Z step is the median distance between neighbouring positions (irregular spacing is noted); without positions, Spacing Between Slices, then Slice Thickness (an assumption the assurance records). The time step is Frame Time, else the median step of trigger or acquisition times.

`physical_size` comes from Pixel Spacing (row spacing is `y`, column spacing `x`), else Imager Pixel Spacing (at the detector; `extra.pixel_spacing_from`), else an ultrasound region calibrated in cm.

`extra.orientation`: the row and column direction cosines, the first slice's position, the anatomical `plane` the slice normal is closest to (`axial`, `coronal`, `sagittal`, `oblique`), and the patient direction (LPS letters L/R, A/P, S/I) that columns, rows and slices advance toward (`columns_toward`, `rows_toward`, `slices_toward`). Previews use them to show anatomical views; NIfTI export turns them into an affine.

## Whole-slide microscopy

VOLUME instances of one series form the pyramid, one level per Total Pixel Matrix size (a level may span several instances). Frames are tiles: TILED_FULL in row-major order, TILED_SPARSE placed by Plane Position (Slide) Sequence. Missing tiles read as zeros and are reported by `stored_areas` (previews draw them as background). LABEL, OVERVIEW and THUMBNAIL instances become images named `label`, `overview` and `thumbnail`. Only the first focal plane and optical path are placed. Opening one instance of a slide reads its siblings of the same series in the same folder, as slide viewers do.

## Personal data

Patient-identifying values are never printed. `extra.personal_data` lists which elements of the file hold them (`{field, kind}`, no value), the vendor tree shows `withheld` in their place, and `openreadout index` counts them as personal-data flags (rule `reader_declared`). Withheld: every PN element, and the identifying elements of the PS3.15 Annex E profile (patient ID, other IDs, birth date and time, addresses, telephone numbers, accession number, medical record locator, comments). Free text in private elements is withheld too. Burned In Annotation YES is flagged as `burned_in_text`; Patient Identity Removed YES sets `extra.deidentified`. Sex, age, weight and height stay under `extra.subject` (they are needed for SUV and dose).

## Vocabulary (every public identifier in `openreadout-medical/src/dicom` must appear here)

| identifier | meaning |
| --- | --- |
| `DicomReader` | the reader (core `FormatReader`) |
| `FORMAT_ID` | the format id, `dicom` |

### `info` extras (`images[].extra`)

| key | meaning |
| --- | --- |
| `modality`, `sop_class`, `series_description`, `series_number`, `study_description`, `protocol_name`, `body_part` | what was imaged, from the General Series and Study modules |
| `arrangement` | how frames became Z, T and C (table above) |
| `files` | files the image is read from |
| `transfer_syntax` | `{uid, name, lossy}` |
| `implementation_version` | Implementation Version Name from the file meta group: the software that wrote the file |
| `lossy_compression` | Lossy Image Compression is `01` |
| `photometric_interpretation`, `stored_pixel_type`, `bits_stored` | the stored samples |
| `rescale` | `{slope, intercept, applied}`, `{lut_entries, lut_first_value, lut_bits, applied}` or `{varies, transforms, applied}` |
| `value_unit` | unit of the returned values (`HU`, PET units, dose units, Rescale Type) |
| `windows` | the file's display windows `[{center, width, name}]`, in returned values |
| `display_inverted` | MONOCHROME1 |
| `orientation` | `row_direction`, `column_direction`, `first_position_mm`, `plane`, `columns_toward`, `rows_toward`, `slices_toward` |
| `pixel_spacing_from`, `z_step_from` | where the pixel size or Z step came from when not Pixel Spacing or slice positions |
| `acquisition` | `kvp`, `tube_current_ma`, `exposure_time_ms`, `convolution_kernel`, `repetition_time_ms`, `echo_time_ms`, `inversion_time_ms`, `flip_angle_deg`, `field_strength_t`, `scanning_sequence`, `slice_thickness_mm`, `radiopharmaceutical` |
| `subject` | `sex`, `age`, `weight_kg`, `height_m`, `species` |
| `personal_data`, `deidentified` | see Personal data |
| `dimension_organization`, `container_identifier`, `specimen`, `level_files` | whole-slide images: tiling, slide barcode, specimen identifier and description, files and tiles per level |

`frames` (`info --view full`) lists every plane: `z`, `t`, `c`, `file`, `frame`, `instance_number`, `position_mm`, `slice_location_mm`, `acquisition_time_s`, `trigger_time_ms`, `rescale_slope`, `rescale_intercept`.
