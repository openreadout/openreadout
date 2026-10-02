# Provenance log — Gatan Digital Micrograph DM3 / DM4 (`dm`)

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

Gatan does not publish the DM3/DM4 file format. Everything here comes from permissively licensed readers, a public third-party format description, and the corpus files themselves.

## 2026-09-22 — initial derivation (Richard Zimring with Claude as assistant)

**Prior art consulted** (documentation and source read; no code copied):
- `pyDM3reader` / `dm3_lib` 2.0, Pierre-Ivan Raynal and contributors, **MIT** (LICENSE.txt verified; commit `b2b3d2f`), https://github.com/piraynal/pyDM3reader — `_dm3_lib.py`: header (version, root length, byte-order word, all big-endian), tag group = sorted byte, open byte, tag count; tag entry = kind byte (20 group, 21 data), big-endian u16 name length, name, DM4 u64 tag length; data tag = `%%%%`, info count, info array (4-byte in DM3, 8-byte in DM4), values in the file's byte order; encoded types 2 i16, 3 i32, 4 u16, 5 u32, 6 f32, 7 f64, 8 bool, 9 char, 10 octet, 11 i64, 12 u64, 15 struct, 18 string, 20 array; struct info = name length, field count, (name length, type) pairs; array info = element type (or nested struct spec) then count; short u16 arrays are UTF-16 strings; `ImageList` entries hold `ImageData` (`Data`, `DataType`, `Dimensions`, `PixelDepth`, `Calibrations`), `ImageTags` and `Name`; `Thumbnails.ImageIndex`; the image `DataType` codes (1 i16, 2 f32, 3 complex8, 5 packed complex, 6 u8, 7 i32, 8 RGB, 9 i8, 10 u16, 11 u32, 12 f64, 13 complex16, 14 binary, 23 RGBA, 35/36 64-bit integers).
- `dm4` 1.0.3, James Anderson, **MIT**, https://github.com/jamesra/dm4 — `dm4file.py`, `headers.py`: DM4 tag directory header (name, u64 byte length, sorted, closed, u64 tag count), the `%%%%` check, u64 info arrays, struct and array info layouts. Its comment notes that it reverses the byte-order flag; we follow the files instead (flag 1 = little-endian values, verified on the corpus).
- Chris Boothroyd, "Digital Micrograph file format", https://personal.ntu.edu.sg/cbb/info/dmformat/index.html (public web page; states it was obtained by examining files; read as documentation). Used for: DM3/DM4 header and tag layouts (consistent with the two readers above), the 8 zero bytes that end the file, `Thumbnails::ImageIndex` naming the thumbnail's `ImageList` entry, and image `DataType` 23 = RGBA used for thumbnails. The page says the root length equals file length − 16 (DM3) / − 24 (DM4); the corpus disagrees for DM3 (see below).

**Reference reader:** openNCEM `ncempy` (GPL-3.0), run as a black-box second opinion.

**Corpus files used:** `zenodo8190744-EELS-STO`, `zenodo8190744-bto-atomic`, `zenodo13821437-Figure-2e`, `zenodo14541027-SAED1-TEM-0003`, `zenodo8398370-NF-0001` (Zenodo, CC-BY-4.0). Tag trees were dumped with `dm3_lib`.

**Observed in the corpus:**
- Root length = file length − header (12 bytes DM3, 16 bytes DM4) − 8: file length − 20 in all three DM3 files, − 24 in both DM4 files; the last 8 bytes are zero. So the root length counts the root tag group only.
- Every file has `ImageList` entry 0 = a 384 × 384 `DataType` 23 thumbnail and entry 1 = the data, with `Thumbnails.0.ImageIndex = 0`.
- Calibrations: `Dimension.<i>.Scale`/`Origin`/`Units` with units `nm` (images) and `1/nm` (diffraction patterns); `Brightness` scale 1.
- `ImageTags.Microscope Info` (`Voltage` in volts, `Indicated Magnification`, `Actual Magnification`, `Operation Mode`, `Illumination Mode`, `Microscope`, `Name`, `STEM Camera Length`, `Cs(mm)`), `ImageTags.Acquisition.Device.Name` (camera), `ImageTags.Acquisition.Frame.Sequence.Exposure Time (ns)`, `ImageTags.DataBar.Acquisition Date`/`Time` (locale-dependent strings) and `Acquisition Time (OS)` (a Windows FILETIME as a double: 1.3309980037321494e17 = 2022-10-11), `Acquisition.Frame.Sequence.Acquisition Start Time (epoch)` in milliseconds.
- `zenodo8190744-EELS-STO` holds 1-D data (an EELS spectrum): `Dimensions` has one entry; `dm3_lib` cannot read it (it assumes two dimensions).

**Inferred (ours):** `Acquisition Time (OS)` is a FILETIME (checked against the `DataBar` date strings on the corpus); thumbnails are exposed as attachments, not images; 3-D data whose third calibration unit is a length is a Z stack, otherwise the third axis is T; `1/nm` calibrations are reciprocal-space and give no physical pixel size.

## 2026-09-25 — assurance profile (`src/assurance.rs`)

**Corpus files:** every development input of this format on disk (`corpus/assurance/evidence.json` lists them); no held-out file.
**Prior art consulted:** none.
**What was done.** No parsing logic changed. The reader declares an assurance profile (docs/assurance.md) that fingerprints each file from this reader's own normalized output: the DM version, data type, dimensionality and undecoded data types. The table of validated feature values and the confidence level are generated from the development corpus by `cargo xtask assurance-audit`.

## 2026-09-26 — differential cross-check with Bio-Formats

**Corpus files:** every development file of this format on disk up to 400 MB (`corpus/oracle/second/*.json`); no held-out file.
**Prior art consulted:** Bio-Formats 8.5.0 (GPL) `showinf`/`bfconvert`, run as black boxes by `oracle/second_opinion.py`.
**Observations and adjudications:** `corpus/oracle/second/adjudications.toml` (differences are conventions or decoder rounding of the second reader; files whose series Bio-Formats groups differently are not compared).
**Inferred.** Nothing new; no reader change.

## 2026-09-26 — complex, RGB, 64-bit and packed-complex data; spectral axes; spectra as traces

**Corpus files used:** RosettaSciIO's DigitalMicrograph test data (`github.com/hyperspy/rosettasciio` commit `bc254db`, `rsciio/tests/data/digitalmicrograph/`, GPL-3.0 data files; only the data files were downloaded): the `1D/`, `2D/`, `3D/` `test-<DataType>.dm3/.dm4` series (DataTypes 1, 2, 3, 6, 7, 9, 10, 11, 12, 13, 14, 23, and DM4-only 27/28 names), `test_fft_packed_complex8.dm4`, the CL spectra and spectrum images (`test-MonoCL_*`, `test-MonarcCL_*`), `EELS_SI.dm4`, `test-EELS_spectrum.dm3`, `test-EDS_spectrum.dm3`, `multi_signal.dm3`, `test_stackbuilder_imagestack.dm3`. Zenodo records 8403583 (lunar apatite EELS spectrum image and HAADF, CC-BY-4.0), 8045363 (fluctuation-microscopy diffraction stack, CC-BY-4.0), 10848915 (TEM image, CC-BY-4.0). The existing corpus files listed above.

**Prior art consulted** (read as documentation; no code copied):
- Gatan, "*.dm5 documentation", https://www.gatan.com/dm5-documentation (the vendor's public web page; read through the Internet Archive copy of 2026-03-06 because the live page sits behind a browser check). Used for: the Data Type list (1 SIGNED_INT16 … 13 COMPLEX (16-byte), 14 BINARY, 15–22 RGB/RGBA variants, 23 RGBA_UINT8_3, 24–26 RGBA 16-bit/float, **27 COMPLEX8_PACKED, 28 COMPLEX16_PACKED**); "DataType 23 … RGBA. So, 8 bits each for Red, Green, Blue, and Alpha. The alpha channel contains no useful data"; the DM5 layout (HDF5 groups `ImageList/[i]/ImageData` with `Data`, `DataType`, `PixelDepth`, `Dimensions`, `Calibrations/Dimension/[i]` Origin/Scale; `Thumbnails/[0]` ImageIndex; tags as attributes).
- `nionswift-io` (Nion Co., **Apache-2.0**, commit `d887686`), https://github.com/nion-software/nionswift-io — `nion/io/DM_IO/dm3_image_utils.py`, `DM5Utils.py`: the calibration convention (`offset = −Origin × Scale`, i.e. value = (index − Origin) × Scale), RGBA 23 read as bytes R, G, B with the fourth byte dropped, `Meta Data.Format` = `Spectrum` / `Spectrum image` marking the spectral axis (the first stored dimension for 2-D data, the last for 3-D data), `Meta Data.IsSequence`, 4-D data as a 2-D collection (the last two stored dimensions) of 2-D data (the first two), the DM5 group/attribute layout. Its reference files (`nionswift_plugin/DM_IO/test/resources/ref_*.dm3|dm4|dm5`) are corpus inputs.
- Chris Boothroyd's page (above) for DataType 8 ("unused, red, green, blue"; the byte order is not stated, so type 8 stays undecoded).

**Black-box oracles (run only):** RosettaSciIO 0.14.0 (GPL-3.0) in a scratch environment: returns complex64 for DataTypes 3 and 27-named files, complex128 for 13, bool for 14, a structured R, G, B, A array for 23, and spectral axes for the CL line-scan (dimension 0) and 3-D spectrum images (dimension 2). For `test_fft_packed_complex8.dm4` (DataType 27, Dimensions 5 × 5, 120 data bytes) it returns a 5 × 4 complex array, which fits neither the stored 3 × 5 half plane nor the 5 × 5 image (noted in `docs/benchmark/microscopy-metadata.md`).

**Inferred (ours) and checked:**
- Calibrated axis value = (index − Origin) × Scale: `zenodo8190744-EELS-STO` (Origin −1400, Scale 0.25 eV) then spans 350–862 eV and its steepest rise sits at 452.75 eV, the Ti L3 edge (456 eV); in Zenodo 8403583's low-loss spectrum image the summed spectrum peaks at 23.35 eV (the apatite plasmon), where Origin + index × Scale would give −18.6 eV.
- DataType 27/28 store only the non-redundant half of the Fourier transform of a real image: `test_fft_packed_complex8.dm4` holds 120 bytes = (5/2 + 1) × 5 complex64 values for Dimensions 5 × 5, and the summed-image value (50, the sum of a 0–4 ramp over five rows) sits in column 0 of the middle row. We expose the stored half plane as it is ((X/2 + 1) × Y complex values, `extra.packed_half_plane`); how DM mirrors it into the full plane (which half is stored, the sign convention) is not established by any file, so no reconstruction is attempted.
- DataType 3 / 13 are interleaved (real, imaginary) float32 / float64 pairs: the `test-3`/`test-13` files hold 1 + 0i, 2 + 0i, … in that order. On big-endian files each component is byte-swapped separately.
- DataType 23 (RGBA): bytes R, G, B, A (the vendor page, nionswift-io and RosettaSciIO agree); returned as three interleaved uint8 samples, alpha dropped (the vendor page says it carries no data). The spectrum-plot thumbnails then render in DM's plot colours (a yellow grid, an olive fill), not in their R/B-swapped complement.
- The spectral axis: `Meta Data.Format` "Spectrum image" → the last of three dimensions, or the first of two (a line scan: `test-MonoCL_spectrum-SI.dm4` has Dimensions 1336 × 67 with units nm and µm, its first axis spanning 811–936 nm around the recorded "Central wavelength (nm)" 869.98); "Spectrum" with more than one spectrum → the first dimension; files without the tag → a third dimension (or the first of two) whose unit is an energy (eV, keV). A spectral axis whose other axes are all 1 is a single spectrum: kept as an image one pixel high (as before) and also exposed as a trace. Otherwise the spectral axis becomes channels (C), one per bin, named by the bin's calibrated value, like Velox spectrum images; positions stay X (and Y). Previously a 3-D spectrum image's energy axis was T and a CL spectrum image's wavelength axis (unit nm) was taken for Z with a physical Z size: both were wrong.
- `Meta Data.IsSequence` makes the stacked axis T even when its unit is a length.
