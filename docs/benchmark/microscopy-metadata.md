# Microscopy: where OpenReadout, Bio-Formats and the Python readers disagree

Measured 2026-09-26 on every development input of six light-microscopy formats (no held-out
file). Two comparisons:

- **Pixels** against the primary oracle (czifile, nd2, liffile, oirfile, tifffile, olefile; all
  BSD/MIT) by the corpus test, and against a second reader (Bio-Formats 8.5.0, pylibCZIrw 6.1,
  readlif 0.6.6; GPL/LGPL, run as black boxes) by the second-opinion test.
  Every disagreement is adjudicated in `corpus/oracle/second/adjudications.toml` or in the
  oracle generator (`oracle/gen.py`, `VSI_BF_WRONG`).
- **Key metadata** field by field against Bio-Formats' OME-XML with `oracle/metadata_compare.py`
  (sizes, pixel type, physical sizes, channel names, excitation/emission, acquisition date,
  time increment, objective, detector, microscope, per-channel exposure).

Rerun: `OPENREADOUT_CORPUS_DIR=corpus/files oracle/.venv/bin/python oracle/metadata_compare.py
--format czi,nd2,lif,oir,vsi,zvi --json out.json`.

## Metadata agreement with Bio-Formats

Rows are (image, field) pairs. "Only OpenReadout" is mostly fields Bio-Formats does not report
(detection bands, stage positions, microscope models, exposures it leaves out); "only
Bio-Formats" is analysed below.

| format | files | images compared | agree | differ | only OpenReadout | only Bio-Formats |
| --- | --- | --- | --- | --- | --- | --- |
| CZI | 90 | 135 | 2131 | 35 | 129 | 0 |
| ND2 | 28 | 79 | 1301 | 17 | 421 | 11 |
| LIF | 22 | 541 | 3878 | 474 | 708 | 775 |
| OIR | 16 | 15 | 193 | 33 | 113 | 6 |
| VSI | 27 | 16 | 237 | 2 | 28 | 0 |
| ZVI | 16 | 15 | 233 | 13 | 30 | 12 |

LIF's large "differ" and "only Bio-Formats" counts come almost entirely from one file,
`zenodo13752242-FLIM250523` (473 images): Bio-Formats orders its FLIM-derived series differently,
so images matched by order and size are not the same images (their pixel sizes are shifted by one
series), and it reports 10 000–100 000 µm pixels for phasor plots and masks. The other 437
"differ" rows are histogram and phasor images whose sample type Bio-Formats reports as float32
(below).

## Where the other reader is wrong

Each case was decided on evidence from the file itself or a third reader, not by majority.

| format | files | field | OpenReadout | other reader | evidence |
| --- | --- | --- | --- | --- | --- |
| CZI | 3 PALM files (`zenodo10577621-*PALM*`) | pixels, pixel size | rendering at its stored size (1206 × 1206, 0.0674 µm) as its own image | pylibCZIrw and czifile's default read resample it to the 512 grid; Bio-Formats keeps 0.1587 µm | czifile `asarray(storedsize=True)` equals ours; 0.1587 × 512 / 1206 = 0.0674 |
| CZI | `openslide-zeiss-5-slidepreview-zstd1-hilo` | pixels | ours | pylibCZIrw zstd1-HiLo decode | the same preview stored zstd0 (lossless) and JPEG XR equals ours |
| CZI | 11 (`zenodo7015307-*`, `aics-OverViewScan`, …) | exposure | the channel's `ExposureTime` (20 ms, 5 ms), repeated in every subblock's metadata | 150 ms | 150 ms is the `ShadingReference` image's exposure |
| CZI | `ome-idr0011-*` (2) | exposure | 40 and 50 ms (the channels' elements) | 320 and 15 ms | neither value appears in any `ExposureTime` of the document (unresolved: not ours to explain) |
| ND2 | 4 colour-camera files | pixels | one RGB channel in R, G, B order | three channels in stored B, G, R order, or near-equal planar samples (legacy JPEG 2000) | equal to the nd2 library; DAPI plane lights blue |
| ND2 | `ome-karl-sample-image` | exposure | 100, 50, 200, 20, 40 ms (one sample setting per plane) | 100, 100, –, –, – | `sSampleSetting/a0…a4` hold the five values; the nd2 library reports them too |
| LIF | `zenodo13752242-FLIM250523` | pixels | half floats decoded (mean 1.361) | readlif returns raw bits as uint16 (mean 7446) | liffile equals ours |
| LIF | `ome-imagesc-110520-AMR1-lifext` (3 histogram images) | pixel type | uint32 | float32 | `DataType="0"`, `Resolution="32"`; liffile reads uint32 |
| LIF | `zenodo6606445-Project007` and 5 more | excitation | none | 448 nm on the DAPI channel | the 448 nm line has intensity 0 and its laser is `PowerState="Off"`; the active line is 405 nm (we report no excitation for LIF yet: a gap, not a disagreement) |
| OIR | 9 files | excitation / emission | laser line (405, 488, 561 nm) and dye maximum; detection band in `emission_range_nm` (430–470 nm) | 430 / 470 nm | Bio-Formats' "excitation" is the start of the detection band and its emission the band's end; no 430 nm laser exists on the instrument |
| VSI | `figshare27677802-vsi-stitch` | pixels | 90 tiles placed at signed grid positions; equal to the depositor's JPEG export (Pearson 0.99, zero shift best) | Bio-Formats drops every plane (negative tile indices) | `VSI_BF_WRONG` in `oracle/gen.py`; `crates/openreadout-corpus-tests/tests/vsi_exports.rs` |
| VSI | `figshare30384007-vsi-spleen` | pixels (level 4) | offset `o / 2^L`, rounded to nearest, halves toward zero | truncation, a one-pixel shift | depositor's PNG export |
| VSI | `ome-cellsens-metadatatest-01` | channel names | `Cy5`, `GFP Wild Type, non-UV excitation` | both named after the stack (`488/640_50um_1x`) | the channel entries' records hold the names |
| ZVI | 11 multi-plane files | acquisition date | first plane's time (item 0, tag 1025) | the second plane's time | in all 11 its value equals `Image/Item(1)` tag 1025 |
| ZVI | 3 files | exposure | 100, 5893.982, 1000.572 ms | 0 | the items' tag 2564 |
| ZVI | `a1.zvi` | everything | read; planes equal the olefile oracle | Bio-Formats throws (NullPointerException in its compound-file parser) | olefile opens it |
| MRC (EM, for completeness) | 7 files | pixels | sections in file order, as mrcfile | rows flipped | exact vertical flip |

## Where we were wrong or silent, and fixed (2026-09-26)

| format | files | field | before | after | evidence |
| --- | --- | --- | --- | --- | --- |
| LIF | 8 (45 images) | acquisition date | missing | first `TimeStamp` (`HighInteger`/`LowInteger` FILETIME halves) | liffile: all 593 dated images agree |
| ND2 | 3 (`zenodo21162526-*`, 113 channel rows) | exposure | first channel only | every channel (one shared sample setting) | `SampleSetting` holds a single entry; Bio-Formats agrees |
| ND2 | 5 (edge-described filters) | excitation / emission | rising band edge (450, 500 nm) | band centre (470, 525 nm) | a 500–550 nm filter; Bio-Formats agrees; the nd2 library reports the edge (oracle adjudicated) |
| CZI | 50 files | detector | missing | `Detectors/Detector` model via each channel's `DetectorSettings` | Bio-Formats agrees on all 50 |
| CZI | 4 files | channel names | missing | from `DisplaySetting` when the image list omits channels | Bio-Formats agrees |

## Electron microscopy, TIFF variants and OME-Zarr (2026-09-26)

Measured on development files only (no held-out record): Gatan DM 89 files (83 new), TIA SER 21
(18 new), EMD 32 (21 new), MRC 13 (5 new), TIFF 115 (45 new: GDAL's sample-format and codec
files, four EMPIAR EER movies, two Leica SCN slides, a Ventana BIF slide), OME-Zarr 7 (3 new). Primary oracles:
dm3_lib and a NumPy reading of the DM tag tree, ncempy, h5py, mrcfile, tifffile + imagecodecs,
zarr-python (all BSD/MIT or run as black boxes where GPL: ncempy, RosettaSciIO).

### Where the other reader is wrong or silent

| format | files | field | OpenReadout | other reader | evidence |
| --- | --- | --- | --- | --- | --- |
| DM | `test_fft_packed_complex8.dm4` (DataType 27, 5 × 5) | pixels | the stored half plane, 3 × 5 complex (`extra.packed_half_plane`) | RosettaSciIO returns a 5 × 4 complex array | 120 data bytes = (5/2 + 1) × 5 complex64 values; 5 × 4 fits neither the stored half nor the 5 × 5 image |
| TIFF | GDAL `int24.tif` | pixels | decoded (107, 123, 132, …) | tifffile does not decode 24-bit integers | GDAL stores the same 20 × 20 image as `int12.tif`; ours equals tifffile's decode of that file |
| TIFF (EER) | 4 EMPIAR movies | pixel type | uint8 counts | tifffile returns bool frames | the same bytes (a pixel holds at most one event per frame); counts are what a sum over frames needs |
| Blosc (OME-Zarr codecs) | numcodecs fixture array 03 (1000 booleans, bit shuffle) | pixels | not reproduced | numcodecs 0.17 (c-blosc 1.21) does not reproduce its own stored array either | left out of the fixture test; every other numcodecs Blosc fixture decodes exactly |

### Where we were wrong or silent, and fixed

| format | files | field | before | after | evidence |
| --- | --- | --- | --- | --- | --- |
| DM | CL spectrum images (`test-MonoCL_*`, `test-MonarcCL_*`) | axes | the wavelength axis (unit nm) taken for Z, with a physical Z size | channels named by wavelength (`extra.spectral_axis`); the axis spans 811–936 nm around the recorded central wavelength 869.98 nm | `Meta Data.Format` "Spectrum image"; RosettaSciIO (black box) makes the same axis spectral |
| DM | 3-D EELS spectrum images (`EELS_SI.dm4`, Zenodo 8403583) | axes | energy loss as T | channels named by energy loss (`"532.25 eV"`) | with `value = (index − Origin) × Scale` the zero-loss peak, plasmon and core-loss edges fall at their known energies |
| DM | DataTypes 3, 13, 23, 27, 28, 35, 36 and DM5 files | pixels | refused (exit 6) | complex (`complex`, `double-complex`), RGBA (alpha dropped), packed half planes as stored, 64-bit integers, DM5 (HDF5) | equal to the NumPy reading of the tag tree; RGBA order from Gatan's public DM5 page |
| DM | single spectra | shape | an image one pixel high only | also a trace (`traces[]`, calibrated axis) | — |
| EMD | Berkeley EMD 0.2/1.0 (ncempy, HyperSpy, Prismatic, py4DSTEM files) | everything | refused (exit 6; held-out finding C-G2) | images with their dimension vectors as calibrations | h5py; ncempy's own files name the last dimension "x" |
| EMD | Velox complex FFTs (`FFTComplexEven/Odd.emd`, `fei_example_complex_fft.emd`) | pixel type | listed as uint8, reading exited 6 | `complex` half planes (compound `realFloatHalf*`/`imagFloatHalf*`) | h5py |
| SER | TIA spectrum images, line profiles, point spectra | shape | spectra as rows of a T stack | X × Y scan with one channel per energy bin; other 1-D series also as traces; complex elements as `complex` | position tags step in X along dimension 0; RosettaSciIO (black box) returns the same arrays |
| TIFF | 20 GDAL files (1–31-bit, 64-bit integer, half and 24-bit float, complex integer and float) | pixels | refused (exit 6) | decoded | equal to tifffile's page 0 (int24: see above) |
| TIFF | GDAL `contig_tiled.tif` (short last-row tile) | pixels | corrupt (exit 4) | the tile is padded, as libtiff and tifffile do | tifffile |
| TIFF (EER) | 4 EMPIAR movies (Falcon 4, 4i; 2022–2025 writers) | everything | "1-bit samples with compression 65001" (exit 6) | frames as T, pixel size, frame time, detector, frame metadata; `check` compares events with the recorded dose | first 64 frames equal tifffile + imagecodecs; every frame's event count equals its recorded dose (events / pixels to 6 decimals) in the 1 467 frames that record one; the 2022 file's event total equals its `totalDose` |
| TIFF (Leica SCN) | `Leica-1.scn`, `Leica-Fluorescence-1.scn` | images | overviews merged into a Z stack, the three fluorescence channels as Z planes, every pyramid level its own image | one image per scanned image, channels, levels from the SCN XML, pixel size, objective, NA, filter bands | tifffile `scn` series: planes and 59 region/level windows within the JPEG tolerance |
| TIFF (Ventana BIF) | `Ventana-1.bif` | images | the label and probability images first, then every pyramid level as its own image | one pyramidal image (pixel size, magnification, scanner from `iScan`); label and probability images as attachments; the 105 overlapping tile joints the scanner records are reported (not stitched: tifffile does not stitch either) | tifffile `bif` series: 14 region/level windows within the JPEG tolerance |
| MRC | EMDB `.map.gz` files (4) | everything | not recognised | decompressed once at open and read | mrcfile on the decompressed files; complex modes 3/4 (no public file) on synthetic files |
| OME-Zarr | label images; bool, int64, uint64, float16, complex arrays; Blosc bit shuffle and Snappy; translations | pixels, geometry | labels listed only; those arrays refused (exit 6); those chunks corrupt (exit 4); translations reported, not applied | labels as images; arrays decoded; chunks decoded; scale and translation composed as NGFF 0.4 orders them | zarr-python on a synthetic bf2raw set and three Zenodo stores; numcodecs' Blosc fixtures |
| OME-Zarr | a macOS zip with a `__MACOSX` folder (`zenodo14841309-scportrait-input`) | everything | store not found | read | zarr-python |

## Whole-slide formats and Imaris scene objects (2026-09-26)

Development files only (no held-out record): OpenSlide test data (MIRAX 7 slides, Philips TIFF 2),
Zenodo 20758452 (Imaris filament tracings, 3) and ImarisWriter's minimal file. Oracles: OpenSlide
4.0.1 (LGPL, black box) for MIRAX and Philips regions and levels, tifffile for Philips geometry,
h5py for Imaris tables, the depositor's Imaris statistics exports as vendor-computed values.

### Where the other reader is wrong or silent

| format | files | field | OpenReadout | other reader | evidence |
| --- | --- | --- | --- | --- | --- |
| MIRAX | `openslide-mirax2-fluorescence-1`, `-2` | channels | one channel per filter (`Rhodamine`, `FITC`, `UV`), named and coloured from `Slidedat.ini` | OpenSlide returns the stored images as RGB: in `fluorescence-1` the Rhodamine filter shows in blue and the UV (nuclear) filter in red | the red component shows nuclei; `UV` has `STORING_CHANNEL_NUMBER` 2 (components are stored B, G, R) |
| MIRAX | all 7 | camera photos, levels | read; Bio-Formats and tifffile read no MIRAX | — | OpenSlide's regions (below) |
| Philips TIFF | `openslide-philips-1` | pixel size | 0.226907 µm (the WSI's `DICOM_PIXEL_SPACING`, as OpenSlide) | tifffile 0.227273 µm (level 0's representation spacing and the resolution tags) | the file was converted from an NDPI; 0.227273 is the rounded nominal spacing all levels share; the corpus oracle takes OpenSlide's value |
| Philips TIFF | `openslide-philips-4` | unstored tiles | white | tifffile zeros; OpenSlide transparent | the scanner's own downsampled levels render them white; equal to OpenSlide composited over white |
| Imaris | 3 filament tracings | scene objects | 27–29 tables per file (statistics per category, record datasets) | Bio-Formats and the Python readers read the volumes only | h5py rebuilds every table; 1 451 statistics equal the depositor's exports to 4.8e-6 |

### Where we were wrong or silent, and fixed

| format | files | field | before | after | evidence |
| --- | --- | --- | --- | --- | --- |
| MIRAX | 7 slides | everything | not recognised | one pyramidal image; level sizes equal OpenSlide's; 294 regions within 4 grey levels (JPEG decoding and OpenSlide's 8-bit compositing); level 0 of the PNG and BMP slides sample for sample | OpenSlide regions |
| Philips TIFF | 2 slides | geometry, pixel size, instrument, date | read as plain TIFF: one image, no pyramid, no pixel size | pyramid with OpenSlide's level sizes, pixel size, scanner, software, acquisition time, barcode, label/macro attachments | OpenSlide and tifffile level sizes; 51 regions within 3 grey levels of OpenSlide |
| Imaris | Scene8 files | statistics, filaments | listed, not decoded | tables | h5py; depositor's exports |

### Searched, not done in this round

- **Hamamatsu VMS/VMU**: the OpenSlide VMS slide `CMU-1` (CC0) was downloaded; the reader was not
  started before the scope freeze. No public VMU or Sakura SVSlide file exists (OpenSlide's
  listing and the Wayback Machine have none; Zenodo, figshare and OME downloads searched).
- **Philips iSyntax**: feasible clean-room: libisyntax (BSD-2-Clause; its header says it follows
  the documentation Philips published on openpathology.philips.com in 2020, now behind a
  registration portal) may be read as documentation, and Zenodo 5037046 (`testslide.isyntax`, MIT,
  495 MB, UFS interface 5.0 / data model v1) is a public file. Not implemented in this round; data
  model v2 files (clusters) have no small public sample.
- **Ventana BIF stitching**: the only public BIF files with `Direction="RIGHT"` joints are
  OpenSlide's `OS-1.bif`/`OS-2.bif` (3.6 / 2.5 GB); not fetched.
- **Hamamatsu DCIMG version 0x2000000**: every public DCIMG found (OME downloads, Zenodo 15150937,
  16875377, figshare 31265875, 7685597) is version 0x7 or 0x1000000; the one 0x2000000 file seen
  (attached to a GitHub issue of lens-biophotonics/dcimg) carries no licence.
- **High-content screening**: CellVoyager CQ1 (Zenodo 5727645, 4751185, CC-BY-4.0: `MeasurementResult.ome.xml` + projections), CV8000 (Zenodo 12794819, 12795729), Harmony `Index.xml` naming (Cell Painting Gallery cpg0037/cpg0045, CC0) are candidates; no licensed Harmony `.tiff.gz` set or Cellomics `.c01` set was found (OME's Operetta 59548/59549 have no licence).

## Remaining differences that are conventions

- Channel names: LIF channels are named after the dye LAS X records for them (`DAPI`) and
  otherwise after the LUT (`Green`); Bio-Formats keeps the `Leica/` prefix, reads only the
  detector bands and names some channels where we do not (`Leica/Kaede (Green)` in a file whose
  detectors we cannot match to its channels). ND2 RGB planes are one channel for us, three for
  Bio-Formats.
- Extra dimensions: CZI H-split files and SIM phases are one image per H for us, folded into T by
  Bio-Formats; LIF lambda scans are channels for us, T for Bio-Formats; per-scene T extents
  (`aics-variable-per-scene-dims`) are per scene for us, file-wide for Bio-Formats.
- Exposure rounding: ND2 3.0208 ms against Bio-Formats' 3.0 ms (the file stores 3.020750879…).
- ND2 `ome-jonas-control002` (legacy): Bio-Formats takes 488/520 nm and 400 ms from the free-text
  description's laser line ("Exposure: 400 ms" under the swept-field laser settings, while the
  camera's own exposure is "Triggered"); we report no value rather than parse that text.

## Not compared

Files Bio-Formats groups into different series (MetaMorph sets, some OME-TIFF collections) and
planes above the second-opinion size limit (`ORACLE_BF_MAX_BYTES`) are listed as not compared by
the second-opinion test.
