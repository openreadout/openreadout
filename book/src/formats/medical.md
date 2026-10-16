# Medical imaging

OpenReadout reads CT, MR, PET, ultrasound, X-ray and nuclear medicine images from DICOM files and folders, NIfTI and Analyze volumes from neuroimaging, and Bruker ParaVision experiments from preclinical MRI. The crate is `openreadout-medical`. Format notes: [DICOM](dicom.md), [NIfTI and Analyze](nifti.md), [Bruker ParaVision](bruker-paravision.md).

## What you get

- **One image per series.** Point `info` at a DICOM file, a series or study folder, or a DICOMDIR. Slices are grouped by series and sorted along the slice normal into a volume (z), and repeated volumes or cine frames become t. Enhanced multi-frame files, segmentations (one channel per segment) and whole-slide microscopy (a pyramid, read tile by tile) are read too.
- **Real units.** Values come out rescaled: CT in Hounsfield units, PET and parametric maps in their stored units (`extra.value_unit`, `extra.rescale`). Voxel sizes are in `physical_size`, and `extra.orientation` says which way columns, rows and slices run (L/R, A/P, S/I) and which plane the slices are in.
- **Personal data stays out.** Patient names, IDs, birth dates and other identifying DICOM elements and ParaVision subject parameters are shown as `withheld`, in `info` and in `--view full`. `extra.personal_data` lists which elements hold them, and `index` flags these files in a lab share.
- **Compressed DICOM.** Implicit and explicit VR, big endian, deflate, RLE, JPEG (baseline, extended, lossless), JPEG-LS and JPEG 2000 (including HTJ2K), all decoded in pure Rust.

## Pictures

`preview` shows a slice the way a radiologist expects: the plane the scan was acquired in, patient left on the right of the picture, the file's display window (or the soft-tissue window for CT), and a scale bar in mm. `--contrast window:bone` (or `brain`, `lung`, `soft-tissue`, …) picks a preset, and `--view coronal`, `sagittal` or `orthogonal` cuts the volume along another plane.

```bash
openreadout preview ct-series/ --view orthogonal -o ct.jpg
```

<img src="../img/medical-ct-orthogonal.jpg" alt="Axial, coronal and sagittal cuts of an abdominal CT with lines where the planes cross and a 200 mm scale bar" width="1200" height="472">

*Contrast-enhanced abdominal CT, 41 slices of 5 mm (StageII-Colorectal-CT collection, The Cancer Imaging Archive, CC BY 4.0).*

A NIfTI volume is shown the same way. Voxels that are not square are stretched to their true shape.

```bash
openreadout preview sub-01_T1w.nii.gz --view sagittal -o t1.jpg
```

<img src="../img/medical-t1-sagittal.jpg" alt="Sagittal slice of a T1-weighted brain MRI with pixel rulers and a 50 mm scale bar" width="900" height="896">

*T1-weighted MRI, OpenNeuro ds000001 (CC0).*

## Numbers

`stats` reports values in the image's units, so a region of a CT gives Hounsfield units. A `plot` series with `show: profile` draws the values along a line:

```bash
openreadout plot '{"data": {"file": "ct-series/", "show": "profile", "line": [[0, 256], [512, 256]]}}' -o profile.png
```

<img src="../img/medical-ct-profile.png" alt="CT values along a line across the abdomen: air at -1000 HU, soft tissue near 100 HU" width="1200" height="742">

## Export

`export -o scan.nii.gz` writes NIfTI-1 with the voxel size and the scanner orientation (from the DICOM geometry or the source NIfTI header). OME-TIFF and OME-Zarr work as for any other image.

## Not read yet

MINC, NRRD and FreeSurfer MGH volumes are candidates for later. Raw k-space files (Siemens `.dat`, Philips `.raw`, GE P-files) and ECAT are out of scope.
