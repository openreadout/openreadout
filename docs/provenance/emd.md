# Provenance log — EMD (HDF5-based; Velox) (`emd`)

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

## 2026-09-22 — HDF5 reader evaluation and Velox layout (Richard Zimring with Claude as assistant)

**Pure-Rust HDF5 evaluation** (the question in the checklist: can EMD be read without a C library?). Candidates on crates.io, 2026-09-22:
- `hdf5` 0.8 / `hdf5-metno` 0.15 (MIT OR Apache-2.0): bindings to the C libhdf5 (`hdf5-metno-sys`); a static musl build needs the C library. Rejected (the binary is pure Rust).
- `hdf5-pure` 0.47.0 (MIT OR Apache-2.0, https://github.com/CramBL/hdf5-pure): pure Rust, no build script; dependencies `byteorder` and (default feature `deflate`) `flate2`. Reads v0–v3 superblocks, contiguous/chunked/compact layouts, variable-length strings, and offers a streaming (non-mmap) open and row-window reads (`read_raw_rows`). A 30-line test program read the Velox sample `zenodo20040988-0050-STEM-15.4nm` (groups, the 512 × 512 × 1 uint16 image, the 60000-byte metadata dataset, the variable-length `Version` string) in 6 ms. **Chosen.**
- `hdf5-reader` 0.9 (MIT OR Apache-2.0): pure Rust but pulls `libc`, `memmap2`, `parking_lot`, `ndarray`; `rust-hdf5` 0.5 (MIT): younger. Not needed.

The crate is used as a dependency, not read as prior art for EMD (it knows nothing about EMD).

**Documentation / prior art for the EMD layout:** none. The Velox layout below was read off the corpus files with `h5py` (BSD-3-Clause, used as an HDF5 browser) and our own test program.

**Corpus files used:** `zenodo20040988-0050-STEM-15.4nm`, `zenodo20131420-0030-STEM-Nano-2.79um`, `zenodo20131420-0028-Camera-Micro`, `zenodo19965490-graphene-2` (Zenodo, CC-BY-4.0).

**Observed (Velox 3.17, `Version` = `{"version": "10", "format": "Velox"}`):**
- Root groups `Application`, `Data`, `Features`, `Operations`, `Presentation`; root datasets `Experiment`, `Info`, `Version` (variable-length JSON strings) and `Thumbnail.jpg` (uint8 JPEG bytes).
- `Data/Image/<32-hex id>/Data`: shape (rows, columns, frames), uint16 (STEM detectors) or int16 (Ceta camera); contiguous or chunked by row bands, uncompressed.
- `Data/Image/<id>/Metadata`: shape (60000, frames) uint8, a NUL-padded JSON document per frame (column). Keys used: `BinaryResult.Detector`, `BinaryResult.PixelSize.width/height` with `PixelUnitX/Y` = `m`, `Instrument.Manufacturer`/`InstrumentModel`/`InstrumentClass`/`ControlSoftwareVersion`, `Acquisition.AcquisitionStartDatetime.DateTime` (Unix seconds as text), `Optics.AccelerationVoltage` (volts), `Optics.NominalMagnification`, `Optics.CameraLength` (metres, inferred from values 0.091–0.363).
- `Data/Image/<id>/FrameLookupTable`: uint32, one entry per frame (not interpreted).
- `Data/Text/<id>`: a variable-length string (not interpreted).

**Inferred (ours):** frames are the third axis and exposed as T; pixel sizes in metres are converted to µm; `Thumbnail.jpg` is an attachment.

## 2026-09-25 — assurance profile (`src/assurance.rs`)

**Corpus files:** every development input of this format on disk (`corpus/assurance/evidence.json` lists them); no held-out file.
**Prior art consulted:** none.
**What was done.** No parsing logic changed. The reader declares an assurance profile (docs/assurance.md) that fingerprints each file from this reader's own normalized output: the Velox version, pixel type, multi-frame images and undecoded non-image data. The table of validated feature values and the confidence level are generated from the development corpus by `cargo xtask assurance-audit`.

## 2026-09-25 — Velox spectra, EDS spectrum images from event streams, multi-frame and float images on real files

**Corpus files used** (all CC-BY-4.0, in `corpus/manifest.toml`): `zenodo20131712-0017-SI-Nano-1.4um` (Velox 10, Super-X G2 EDS spectrum image, 512 × 512 × 50 frames, with a 50-frame HAADF image and 8 float32 quantified maps; Austin Houston), `zenodo20131712-0002-Spectrum-Micro` (Velox 10 point spectrum), `zenodo21895889-0043-Spectrum-EDS` (Velox 11 / 3.14, 5 eV per channel) and `zenodo21895889-0041-STEM-HAADF-BF` (two images in one file) (the ADAM deposit: Anderson, Borch et al.). The earlier four image files for regression.

**Prior art consulted:** none. h5py (BSD-3) was run as a tool to list the HDF5 tree and read datasets for the inferences below.

**Observed (h5py listing, JSON metadata):**
- `Data/Spectrum/<id>/Data`: (4096, 1) uint32, one per Super-X detector segment (`BinaryResult.Detector` = `SuperXG21` … `SuperXG24`) plus, in point spectra, their sum (`SuperXG2`); `Metadata` is the (60000, 1) uint8 JSON column as for images. Each segment's entry in `Detectors` has `Dispersion` (10 or 5) and `OffsetEnergy` (−250), in eV.
- `Data/SpectrumStream/<id>`: `AcquisitionSettings` JSON (`bincount` 4096, `StreamEncoding` `uint16`, `RasterScanDefinition` `Width`/`Height` for spectrum images), `Data` (n, 1) uint16, and for spectrum images `FrameLocationTable` (frames, 1) uint64.
- `Data/SpectrumImage/<id>/Data`: a uint8 blob whose layout we do not know (not used), `SpectrumImageSettings` (`startFramePosition`/`endFramePosition`).

**Inferred, with the evidence:**
- **Energy axis:** channel k is at `OffsetEnergy + k × Dispersion` eV. Peaks of the summed spectra of the two 10 eV files fall at 0.27, 0.52, 1.74, 2.12, 2.30, 3.31, 3.59, 8.02–8.04, 9.70–9.71, 11.44 and 13.34 keV = C Kα 0.277, O Kα 0.525, Si Kα 1.740, Au Mα 2.123, Mo Lα 2.293 / S Kα 2.307 (MoS₂ sample), K Kα 3.313, K Kβ 3.590, Cu Kα 8.048, Au Lα 9.713, Au Lβ 11.44, Au Lγ 13.38 keV, and the zero-energy peak at channel 25 = 0 eV (channel 50 at 5 eV per channel).
- **Event stream:** 65535 ends a pixel; every other value is the channel of one detected X-ray in that pixel, pixels in raster order (x fastest), frame after frame. Evidence: each detector stream of the spectrum image holds exactly 512 × 512 × 50 = 13 107 200 markers, each `FrameLocationTable` segment exactly 262 144; no frame segment ends with an event, 3 of 200 start with one (a marker that ended pixels would give the opposite); the histogram of each stream's events equals, channel for channel, one of the stored detector spectra (4 of 4 streams in the spectrum image and in both point spectra — Velox's own sums); and the per-pixel count map, placed with events belonging to the pixel whose marker follows them, correlates with the summed 50-frame HAADF image at r = 0.961 (0.954 and 0.932 when placed one and two pixels earlier, as the other marker convention would).
- The spectrum image is exposed as one image of `bincount` channels (one per energy channel, counts summed over detectors and frames); an event stream's detector is the spectrum whose histogram it reproduces.

## 2026-09-26 — Velox STEM-EELS spectrum images (`Data/EelsSpectrumImage`)

**Corpus file used:** `zenodo18267156-STEM-EELS-1-DyScO3` (Zenodo 18267156, CC-BY-4.0, D. Jannis: "STEM-EELS on DyScO3 … acquired on an Iliad system", Velox EMD, Spectra instrument, control software 3.24). Found by opening the public Zenodo `.emd` files over HTTP range requests with h5py and listing their `Data` groups.

**Prior art consulted:** none for the layout. h5py (BSD-3-Clause) to read the raw arrays and JSON documents; tabulated EELS edge energies (Sc L3 402 eV, O K 532 eV, Dy M5 1295 eV, public reference values) to check the energy axis.

**Observations.** Each `Data/EelsSpectrumImage/<id>` holds `Data` (128, 2048, 128) uint16 chunked (128, 2048, 1), `Metadata` (60000, 1) uint8 (one NUL-padded JSON document, the Velox image schema), `AcquisitionMetadata` (1536, 128) uint8 (one JSON document per column, i.e. per scan line: `{"Type": "EelsAcquisitionMetadata", "Data": {"offset", "dispersion", "exposureTime", "intensityScale", "intensityOffset", "detectorFramesAveraged", "scanPixelIndex", "detectorSensorId"[]}}`, identical calibration in all 128 columns) and `Info` (JSON `{"encoding": "uint16", "bincount": "2048", "width": "128", "height": "128"}`). The file holds two such images (dual EELS: `offset` −100 and 300, `dispersion` 1, exposures 2 µs and 5 ms) and four 128 × 128 images from the same scan (three with `BinaryResult.Detector` "EELS Strip Detector", one DF-S).

**Inferred, with the evidence:**
- **Axis order (columns, channels, rows):** the per-pixel sum over axis 1 of the core-loss image, transposed, correlates with the simultaneously acquired DF-S image at r = 0.886 (untransposed: 0.146); the low-loss sum anti-correlates at r = −0.63 (untransposed −0.09), as expected for a zero-loss intensity that falls where the specimen scatters more. Axis 1 has `bincount` entries. Chunks of one axis-2 index hold one scan line.
- **Energy axis:** channel k is at `offset + k × dispersion` eV (from `AcquisitionMetadata`). The low-loss image's zero-loss peak has its centroid at channel 99.0 (−1.0 eV); the core-loss sum rises most steeply at 398, 529 and 1290 eV, the Sc L3, O K and Dy M5 edges of DyScO3 (tabulated 402, 532, 1295 eV).
- **Pixel size:** the spectrum image's own `BinaryResult` has no `PixelSize`; the images of the same raster (same width and height) whose `BinaryResult.Detector` equals the spectrum image's report 5.214e-11 m. The reader takes that value only when every such image agrees; otherwise none.
- Samples are exposed as stored (uint16 counts); `intensityScale`/`intensityOffset` are reported in `extra`, not applied.

**More sources (same day).** `zenodo18685575-1238-42kx-LAADF` and `-1243-164kx-LAADF` (Zenodo 18685575, CC-BY-4.0, N. Schnitzer et al.): 4096 × 4096 uint16 STEM LAADF images. The depositor also shares Velox's TIFF export of each; our plane equals the export sample for sample (both files, fetched to scratch for the check, not added to the corpus).

## 2026-09-26 — Berkeley EMD (0.2 and 1.0); Velox complex Fourier transforms

**Held-out finding addressed:** C-G2 (a Berkeley EMD stack refused with exit 6). Fixed on development files only; the held-out file was not opened, dumped or compared.

**Corpus files used:** RosettaSciIO's EMD test data (`github.com/hyperspy/rosettasciio` commit `bc254db`, `rsciio/tests/data/emd/`, GPL-3.0 data files): the HyperSpy-written `example_*.emd`, the Prismatic outputs `Si100_*.emd`, and the Velox files `FFTComplexEven.emd`, `FFTComplexOdd.emd`, `fei_example_complex_fft.emd`, `fei_example_dpc_titles.emd`, `fei_example_tem_stack.emd`; openNCEM's test data (`github.com/ercius/openNCEM` commit `749ec1a`, `ncempy/data/`, GPL-3.0 data files): `Acquisition_18.emd`, `Pt_SAED_D910mm_single.emd`, `emd_type1_stringDims.h5` and three Velox files.

**Documentation consulted:** the EMD specification page https://emdatasets.com/format/ (public, the open convention): root attributes `version_major`/`version_minor` (and, in 1.0, `emd_group_type = "file"`), data groups marked by `emd_group_type` (1 in 0.2, `"array"` in 1.0), a `data` dataset and one calibration vector per dimension named `dim1`…`dimN` (0.2) or `dim0`… (1.0) with `name` and `units` attributes, "the i'th vector's length should match the length of the i'th data array dimension". h5py (BSD-3-Clause) to list the files.

**Inferred, with the evidence:**
- **Which dimension is X.** The specification does not say. In both ncempy-written files the calibration vectors name the dimensions: `Acquisition_18.emd` has `dim1` "Y" and `dim2` "X" (1024 × 1024), `Pt_SAED_D910mm_single.emd` has `dim1` "Number", `dim2` "y", `dim3` "x" (1 × 2048 × 2048, "Converted SER file … to EMD using the openNCEM tools"). The last array dimension is therefore X and the one before Y, as NumPy displays an array; leading dimensions are T (frames), or Z for a 3-D array whose first calibration is a length.
- **Prismatic/py4DSTEM 0.x groups** (`4DSTEM_simulation/data/realslices/...`, `emd_group_type` 1) hold the array under another name (`realslice`, `datacube`): the one dataset of the group that is not a `dimK` vector. A 3-D array whose first two calibrations are lengths (`R_x`, `R_y` in `[n_m]`) and whose third is not (`bin_outer_angle` in `[mrad]`) is positions × positions × detector bins: the bins are channels, X is the second dimension and Y the first.
- **Calibration vectors** hold either one value per element or a (start, start + step) pair (HyperSpy writes `[0, 1]` for a 3-element axis; py4DSTEM 1.0 writes 2-element vectors): the step is v[1] − v[0] in both cases. Units lose their brackets (`[m]` → `m`, `[n_m]` → `n_m`, read as nm).
- **Velox complex Fourier transforms** are `Data/Image/<id>/Data` arrays of a compound of two float32 members named `realFloatHalfEven`/`imagFloatHalfEven` (or `…Odd`), (rows, columns, frames) like other Velox images; 70 rows × 36 columns in `FFTComplexEven.emd` is the non-redundant half (36 = 70/2 + 1). Returned as `complex` samples of the stored half plane; previously the image was listed as uint8 and reading it exited 6.

## 2026-09-26 — Berkeley EMD: stable attribute order; reciprocal-space steps labelled `m`

**Corpus files used:** `ncem-emd-pt-saed-d910mm-single`, `ncem-emd-acquisition-18`, `rsciio-emd-example-signal`, `rsciio-emd-example-metadata`, `rsciio-emd-example-image`, `rsciio-emd-si100-3d` (development tier). No held-out file.
**Prior art consulted:** none. h5py (BSD-3) lists the files.

**What was found and changed:**
- **Non-deterministic output.** `hdf5-pure` returns a group's attributes as a `HashMap`, whose iteration order is random per map. `find_arrays` and `group_attrs` inserted them in that order into order-preserving JSON maps (`extra.group_attributes`, `extra.microscope`, `info --view full`), so `info` of the same file differed between runs (5 distinct outputs in 5 runs of `ncem-emd-acquisition-18`). They are now sorted by name. No value changed; the corpus snapshot test now opens every input twice and fails when the outputs differ.
- **Steps of 1 mm or more are not pixel sizes.** `Pt_SAED_D910mm_single.emd` (converted by openNCEM from a TIA `.ser`) calibrates its x/y dimensions in `m` with a step of 8 894 687 per pixel, centred on zero (first −9.108e9 over 2048 pixels): as a length that is 8.9 km per pixel. It is a diffraction pattern (microscope `Mode` "TEM uP SA Zoom Diffraction", `Camera length [m]` 0.91, `Magnification` "0 X"), and the step is reciprocal metres: a 28 µm (Ceta, binning 2) detector pixel at 0.91 m and λ = 1.97 pm (300 kV) subtends 28e-6 / (1.97e-12 × 0.91) ≈ 1.6e7 m⁻¹ per pixel, the same order as 8.9e6. The TIA reader already refuses element deltas of 1 mm or more as metres for the same reason (docs/provenance/ser.md). A length step of 1 mm or more per pixel is no longer a physical size; a note names the dimensions and `extra.dims` keeps the calibration as stored. The h5py oracle (`oracle/gen.py`, ours) applies the same rule, and the file's oracle was regenerated (physical size x/y null instead of 8.9e12 µm).

