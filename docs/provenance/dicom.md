# Provenance log — DICOM (`dicom`)

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

DICOM is an open standard (NEMA PS3), published free of charge at https://dicom.nema.org. Almost every normalized field therefore carries `Source::Spec`.

## 2026-10-15 — initial derivation (Richard Zimring with Claude as assistant)

**Specification consulted** (public, NEMA, edition 2026d, https://dicom.nema.org/medical/dicom/current/):
- PS3.5 Data Structures and Encoding: §7 (data elements, explicit and implicit VR, the 4-byte length forms of OB/OD/OF/OL/OV/OW/SQ/SV/UC/UN/UR/UT/UV), §7.5 (sequences, items and delimiters, defined and undefined lengths), §8 (native pixel data, BitsAllocated/BitsStored/HighBit, PlanarConfiguration), §8.2 and Annex A.4 (encapsulated pixel data: Basic Offset Table item, one fragment or several per frame, Extended Offset Table), §10 and Annex A (transfer syntaxes: implicit VR little endian, explicit VR little/big endian, deflate, JPEG processes 1/2-4/14, JPEG-LS, JPEG 2000, RLE), Annex G (RLE: 64-byte header of segment offsets, PackBits-style segments, one per byte plane, most significant byte first).
- PS3.6 Data Dictionary, tables 6-1, 7-1, 8-1 (tag, keyword, VR), read by `scripts/gen_dicom_dictionary.py` into `dicom/dictionary.rs`; Annex A (transfer syntax and SOP class UIDs).
- PS3.10 Media Storage: §7 (the 128-byte preamble, `DICM`, the file meta group in explicit VR little endian), §8 (DICOMDIR and the referenced file ID as path components).
- PS3.3 Information Object Definitions: C.7.6.2 (Image Plane: ImagePositionPatient, ImageOrientationPatient, PixelSpacing, SliceThickness; the slice normal as the cross product of the row and column direction cosines), C.7.6.3 (Image Pixel), C.11.1 (Modality LUT: RescaleSlope/RescaleIntercept, RescaleType, HU for CT), C.11.2 (VOI LUT: WindowCenter/WindowWidth and the linear window function of C.11.2.1.2), C.7.6.16 (Multi-frame functional groups: Shared and PerFrame Functional Groups Sequences, PlanePositionSequence, PixelMeasuresSequence, PixelValueTransformationSequence, FrameContentSequence with InStackPositionNumber and TemporalPositionIndex), C.8.12.4 (VL Whole Slide Microscopy: TotalPixelMatrixColumns/Rows, DimensionOrganizationType TILED_FULL/TILED_SPARSE, ImageType VOLUME/LABEL/OVERVIEW/THUMBNAIL, PlanePositionSlideSequence with ColumnPositionInTotalImagePixelMatrix and RowPositionInTotalImagePixelMatrix), C.7.6.1 (General Image: BurnedInAnnotation).
- PS3.15 Annex E (Basic Application Level Confidentiality Profile): which attributes identify the patient. The list in `dicom/phi.rs` (names, IDs, birth date, address, telephone numbers, other patient IDs and names, every PN element) is taken from that table.

**Prior art consulted:** none for the parser. pydicom (MIT) and its test data were used only as an oracle and as corpus files (below); its source was not needed.

**Corpus files used:** pydicom test files (MIT, `pydicom-*` entries) and pydicom-data (MIT, `pydicom-data-*`); OpenSlide DICOM WSI test data (CC0-1.0, `openslide-dicom-*`); NCI Imaging Data Commons series under CC BY 3.0/4.0 (`idc-*`). Element trees were dumped with pydicom 3.0 (`dcmread`, `pixel_array`, `apply_modality_lut`).

**Decisions (ours, documented in `docs/formats/dicom.md`):**
- Planes are returned with the modality rescale applied (stored × slope + intercept), so CT values are Hounsfield units. The pixel type is the smallest signed or unsigned integer type that holds the rescaled range of BitsStored when slope and intercept are whole numbers, float32 otherwise. `extra.stored_pixel_type` keeps the stored type.
- A folder is grouped into images by SeriesInstanceUID, then by frame size and orientation; slices are sorted along the slice normal (ImagePositionPatient · normal), falling back to InstanceNumber. The Z step is the median distance between neighbouring positions.
- Patient-identifying elements are never printed: `info` lists them under `extra.personal_data` (tag, keyword and kind, no value) and the vendor tree replaces their values with `withheld`.

## 2026-10-18 — the writing software (Richard Zimring with Claude as assistant)

`images[].extra.implementation_version` is the Implementation Version Name (0002,0013) of the file meta group, which PS3.10 §7.1 defines as the name of the implementation that wrote the file. The assurance profile records its name without the version as the writer. Corpus files: the existing pydicom, pydicom-data, OpenSlide and IDC entries. Prior art: none.
