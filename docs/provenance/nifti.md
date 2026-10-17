# Provenance log — NIfTI and Analyze (`nifti`)

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

NIfTI is an open standard of the NIfTI Data Format Working Group (NIH); its reference headers are public domain. Most normalized fields carry `Source::Spec`.

## 2026-10-15 — initial derivation (Richard Zimring with Claude as assistant)

**Specification consulted** (public): `nifti1.h` and `nifti2.h` as published by the NIfTI DFWG (https://nifti.nimh.nih.gov/nifti-1, https://nifti.nimh.nih.gov/nifti-2; public domain): header layout and offsets, magic strings, datatype codes, `xyzt_units` codes, `scl_slope`/`scl_inter`, the extension flag and extension blocks (`esize` a multiple of 16, `ecode`), the qform quaternion (method 2), the sform (method 3), `pixdim[0]` as qfac, and the extension code list. The Analyze 7.5 header layout is the same 348 bytes with `data_history` fields (`patient_id` at byte 283, `orient` at 252) as `nifti1.h` documents in its comments.

**Prior art consulted:** nibabel (MIT), documentation only for the SPM Analyze scale factor in `funused1`; nibabel is the oracle (`oracle/gen.py`).

**Corpus files used:** nibabel test data (MIT: `anatomical.nii`, `functional.nii`, `example4d.nii.gz`, `example_nifti2.nii.gz`, `standard.nii.gz`, `reoriented_anat_moved.nii`), OpenNeuro ds000001 (CC0: a T1w volume and a BOLD run).

**Observed:** nibabel's `example4d.nii.gz` records TR 2000 with `xyzt_units` seconds: reported as the header says. `standard.nii.gz` has `xyzt_units` 0 (mm assumed, noted).

**Decisions:** dimensions 5 to 7 are folded into C; scaled data is returned as float32 computed in double precision (`stored × slope + intercept`), float64 data stays float64; orientation letters are derived from the affine's columns, RAS turned into LPS so they read like DICOM's.

## 2026-10-17 — voxel sizes without a unit (Richard Zimring with Claude as assistant)

**Specification consulted:** `nifti1.h` (public domain): `xyzt_units` code 0 is `NIFTI_UNITS_UNKNOWN`.

**Corpus files used:** nibabel `standard.nii.gz` (`xyzt_units` 0); a public micro-CT deposit (figshare 10.6084/m9.figshare.33265719, CC BY 4.0) whose skull scans have `pixdim` 1 and no spatial unit.

**Decision:** a NIfTI header with no spatial unit gives no physical size. The voxel sizes are named in a note instead of being taken as millimetres, which drew a 200 mm scale bar on a mouse skull. Analyze 7.5 sizes stay millimetres, that format's convention. The oracle (`oracle/medical.py`) reports no physical size for these files either.
