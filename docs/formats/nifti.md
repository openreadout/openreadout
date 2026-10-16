# NIfTI-1, NIfTI-2 and Analyze 7.5

NIfTI is the working format of neuroimaging (FSL, SPM, AFNI, FreeSurfer, ANTs, nibabel) and the usual target of DICOM conversion. OpenReadout reads single files (`.nii`, `.nii.gz`), header/image pairs (`.hdr` + `.img`, gzipped or not) and the older Analyze 7.5 pairs, and writes NIfTI-1 (`export --format nifti`). Derived from the NIfTI Data Format Working Group's public headers `nifti1.h` and `nifti2.h` (public domain); nibabel (MIT) is the reference reader. Provenance: `docs/provenance/nifti.md`.

## Header

`sizeof_hdr` is 348 (NIfTI-1, Analyze) or 540 (NIfTI-2), in either byte order; the byte order of every field follows it. NIfTI-1 has the magic `n+1` (single file) or `ni1` (pair) at byte 344, NIfTI-2 `n+2` or `ni2` at byte 4. A 348-byte header without magic beside a `.img` is Analyze 7.5.

| field | use |
| --- | --- |
| `dim[0..7]` | number of dimensions; x, y, z, t, then dimensions 5 to 7 folded into C |
| `datatype`, `bitpix` | sample type (table below); `check` compares them |
| `pixdim[1..4]`, `xyzt_units` | voxel size (m, mm or µm; mm when unset) and time step (s, ms, µs) |
| `vox_offset` | where the voxels start (single files) |
| `scl_slope`, `scl_inter` | applied when the slope is not 0 and the pair is not (1, 0): values come back float32 (float64 data stays float64) |
| `cal_min`, `cal_max` | the display range, reported as `extra.windows` and used by previews |
| `qform_code`, `quatern_*`, `qoffset_*`, `sform_code`, `srow_*` | the voxel-to-world affine: sform when set, else qform, else voxel sizes |
| `intent_*`, `slice_*`, `dim_info`, `toffset`, `descrip`, `aux_file` | reported |
| extensions | listed (code, name, size); comment, AFNI and XCEDE text in the vendor tree; DICOM extensions not decoded |

Analyze 7.5: SPM's scale factor in `funused1` is applied when positive and not 1; `patient_id` (byte 283) is withheld and flagged under `extra.personal_data`.

| datatype | returned |
| --- | --- |
| 2 uint8, 256 int8, 4 int16, 512 uint16, 8 int32, 768 uint32, 1024 int64, 1280 uint64, 16 float32, 64 float64 | the same type |
| 32 complex64, 1792 complex128 | `complex`, `double-complex` |
| 128 rgb24, 2304 rgba32 | uint8 with 3 or 4 samples per pixel |
| 1 binary, 1536 float128, 2048 complex256 | refused (exit 6) |

Planes are in file order: plane (z, t, c) starts at `vox_offset + (z + nz × (t + nt × c)) × plane bytes`. `.gz` files are decompressed once at open through `openreadout_core::gzip`.

`extra.orientation` gives the anatomical `plane` of the slices and the patient direction (LPS letters, as DICOM uses) that columns, rows and slices advance toward, derived from the affine (RAS) by negating x and y.

## Writing (`export --format nifti`)

One image becomes a NIfTI-1 single file, gzip-compressed when the name ends in `.gz` (the default `<stem>.nii.gz`). Values are written as returned (already rescaled; `scl_slope` 0), with `pixdim` in mm and seconds. The affine comes from the source's own (`extra.affine`) or, for DICOM, from the first slice position and the direction cosines (LPS turned into RAS); it is written as sform and qform with code 1 (scanner). Without an orientation both codes are 0. The file is read back with this reader and every plane compared before it is renamed into place.

## Vocabulary (every public identifier in `openreadout-medical/src/nifti` must appear here)

| identifier | meaning |
| --- | --- |
| `NiftiReader` | the reader |
| `FORMAT_ID` | the format id, `nifti` |
| `export_nifti`, `default_nifti_output` | write one image as NIfTI-1; the default output name |
| `NiftiExportOptions` (`image`, `overwrite`) | export options |
| `NiftiExportReport` (`input`, `output`, `format`, `image`, `dims`, `datatype`, `pixdim`, `affine_from`, `planes_written`, `bytes_written`, `verified`, `notes`) | what the export wrote |

### `info` extras

| key | meaning |
| --- | --- |
| `datatype`, `byte_order`, `dims` | the stored samples and `dim[1..]` |
| `rescale`, `stored_pixel_type` | `scl_slope`/`scl_inter` (or the SPM scale) applied |
| `windows` | `cal_min`/`cal_max` as a centre and width |
| `affine`, `affine_from`, `qform_code`, `sform_code` | the voxel-to-RAS affine (4 × 4 rows) and its source |
| `orientation` | `plane`, `columns_toward`, `rows_toward`, `slices_toward` |
| `intent`, `slice_timing`, `dim_info`, `toffset`, `aux_file`, `extensions` | as stored |
| `analyze_orient`, `personal_data` | Analyze 7.5 |
