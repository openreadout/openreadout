# Bruker ParaVision

ParaVision runs Bruker BioSpec and PharmaScan preclinical MRI scanners. A study folder holds one folder per experiment (`<expno>/`, with `acqp` and `method`), each with reconstructions in `pdata/<procno>/`: the image data `2dseq` and its description `visu_pars`. OpenReadout reads a study, an experiment, a `pdata/<procno>` folder or a `2dseq` file, one image per reconstruction (spectroscopy reconstructions are traces). Derived from brukerapi (MIT, Pilsen MR group), read as documentation, and the corpus; brukerapi is also the reference reader. Provenance: `docs/provenance/bruker-paravision.md`.

## Parameter files

JCAMP-DX-style records `##$Name=value`. An array declares its size in brackets (`( 2 )`, `( 5, 9 )`) and continues on the following lines; strings are in angle brackets (`<mm>`); structures in parentheses (`(2, <FG_ECHO>, <Map Type>, 0, 2)`); `@n*(v)` repeats a value; `$$` lines are comments. `parse` never fails: unreadable records are skipped.

## 2dseq

`VisuCoreFrameCount` frames of `VisuCoreSize` samples (x fastest; 2 or 3 core dimensions; 1 for spectroscopy) of `VisuCoreWordType` (`_8BIT_UNSGN_INT`, `_16BIT_SGN_INT`, `_32BIT_SGN_INT`, `_32BIT_FLOAT`) in `VisuCoreByteOrder`. Each frame is scaled by its `VisuCoreDataSlope` and `VisuCoreDataOffs`; the values come back float32 (stored × slope + offset), the way brukerapi scales them.

Frames are ordered by the frame groups of `VisuFGOrderDesc`, the first group varying fastest. Groups become axes:

| group | axis |
| --- | --- |
| `FG_SLICE` | Z (a 3-D core already fills Z: then C) |
| `FG_CYCLE`, `FG_MOVIE` | T |
| every other group (`FG_ECHO`, `FG_DIFFUSION`, `FG_FLOW`, `FG_COMPLEX`, `FG_ISA`, ...) | C, named by `VisuFGElemComment` |

When the groups do not multiply to the frame count, frames are listed as T and a note says so.

Pixel size: `VisuCoreExtent / VisuCoreSize` (mm). Z step of 2-D slices: `VisuCoreSlicePacksSliceDist`, else the distance between the first two `VisuCorePosition`s. `VisuCoreOrientation` is reported (`extra.orientation.matrix`, and the plane its third row is closest to); frames stored transposed (`VisuCoreTransposition`) are returned as stored and `check` says so.

Spectroscopy (`VisuCoreDimDesc` `spectroscopic`) is a trace; its ppm axis is centred on the method's `PVM_FrqWorkPpm` and spans `VisuCoreExtent`, high to low (inferred).

Subject names, IDs, birth dates and the referring user are withheld (`extra.personal_data`; `withheld` in the vendor tree).

## Vocabulary (every public identifier in `openreadout-medical/src/paravision` must appear here)

| identifier | meaning |
| --- | --- |
| `ParavisionReader` | the reader |
| `FORMAT_ID` | the format id, `bruker-paravision` |

### `info` extras

| key | meaning |
| --- | --- |
| `expno`, `procno` | experiment and processing numbers |
| `method`, `protocol` | `Method` (or `VisuAcqSequenceName`) and `VisuAcquisitionProtocol` |
| `word_type`, `frame_groups`, `frame_count` | the stored data |
| `rescale`, `stored_pixel_type` | slope and offset applied (`varies` when frames differ) |
| `value_unit` | `VisuCoreDataUnits` |
| `acquisition` | `repetition_time_ms`, `echo_time_ms`, `inversion_time_ms`, `flip_angle_deg`, `averages`, `echo_train_length`, `imaging_frequency_mhz`, `scan_time_s`, `nucleus`, `receive_coil`, `field_strength_t` (from the 1H frequency) |
| `subject` | `sex`, `type`, `weight_kg`, `position` |
| `orientation` | `plane`, `matrix` |
| `personal_data` | withheld parameters |
