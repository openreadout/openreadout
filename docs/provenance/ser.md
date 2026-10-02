# Provenance log — TIA / ES Vision series files SER (+ EMI) (`ser`), and EMD detection (`emd`)

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

## 2026-09-22 — initial derivation (Richard Zimring with Claude as assistant)

**Documentation consulted:**
- Chris Boothroyd, "TIA (Emispec) file format", https://personal.ntu.edu.sg/cbb/info/TIAformat/index.html (public web page). It reproduces the "Series File Format" page once published on the Emispec web site (the original vendor of ES Vision; `http://www.emispec.com/Support/SeriesFileFormat.asp`, no longer online) with changes FEI supplied. Used for: the header (byte order `II` = 0x4949, series id 0x0197, series version 0x0210 with 4-byte offsets / 0x0220 with 8-byte offsets, data type id 0x4120 = 1-D elements / 0x4122 = 2-D elements, tag type id 0x4152 = time / 0x4142 = time and position, total and valid element counts, offset-array offset, dimension count), the dimension array (size, calibration offset/delta/element, description and units strings), the data and tag offset arrays, the 1-D and 2-D element headers (calibrations, data type 1–10, array sizes), and the tag layouts (including the reported 2 undocumented bytes after the tag type).

**Prior art consulted:** none in source form. `ncempy` (openNCEM, GPL-3.0-or-later per its repository) is run as a black-box oracle (`oracle/gen.py`).

**Corpus files used:** `zenodo17463176-Fig_b1_1` + `zenodo17463176-Fig_b1-emi`, `zenodo17463176-Fig_b2_1` + `zenodo17463176-Fig_b2-emi`, `zenodo13821437-Fig2c-part1` (Zenodo, CC-BY-4.0).

**Observed in the corpus:**
- Series versions 0x0210 (`Fig2c-part1`: offset array at 68, 4-byte offsets) and 0x0220 (`Fig_b1_1`: offset array at 72, 8-byte offsets); one dimension ("Number", size 1); single 2-D uint16 elements; time-only tags.
- Element calibration deltas of 5.97e-10, 7.58e-11 and 1.17e-8 for STEM/TEM images: metres per pixel (0.60 nm, 0.076 nm, 11.7 nm). The unit is not stored in the `.ser`; we treat a delta in (0, 1e-3) as metres (inferred) and report no physical size otherwise (diffraction patterns are calibrated in reciprocal units).
- The `.emi` file embeds one XML document `<ObjectInfo>…</ObjectInfo>` (plain bytes, located by searching for the tags) with `Uuid`, `ExperimentalConditions/MicroscopeConditions/AcceleratingVoltage` (volts), and `ExperimentalDescription/Root/Data` label/value/unit triples (Microscope, User, Gun type, High tension, Mode, Defocus, Magnification, Camera length, Stage X/Y/Z/A/B, apertures, …); a `TrueImageHeaderInfo` element carries an escaped XML document of numbered values whose meaning is not documented (kept verbatim). The rest of the `.emi` (display layout, a copy of the data) is not parsed.
- `.ser` files are named `<emi stem>_<n>.ser`.

**Behaviour learned from black-box oracles (2026-09-22):** our first plane hashes differed from `ncempy`'s on all three files. Comparing arrays showed `ncempy` returns the stored rows in reverse order. RosettaSciIO 0.14.0 (GPL-3.0; installed in a scratch environment, `rsciio.tia.file_reader`, run only) returns the same reversed rows. We therefore return 2-D element rows top to bottom (stored bottom row first); inferred from two independent black-box readers, since the format description does not say. After the change all three files match `ncempy` bit for bit.

**Time zone of tags:** `Fig_b1_1`'s first tag is 1626624515 = 2021-07-18T16:08:35Z; the `.emi` `AcquireDate` is `Sun Jul 18 18:08:35 2021` (local time at the Antwerp lab, CEST = UTC+2). Tag times are therefore Unix UTC seconds, and `AcquireDate` is local time (kept as text).

EMD is covered in `docs/provenance/emd.md`.

## 2026-09-25 — assurance profile (`src/assurance.rs`)

**Corpus files:** every development input of this format on disk (`corpus/assurance/evidence.json` lists them); no held-out file.
**Prior art consulted:** none.
**What was done.** No parsing logic changed. The reader declares an assurance profile (docs/assurance.md) that fingerprints each file from this reader's own normalized output: the series version, element type and kind, the .emi sidecar, and the assumed metre unit of calibration deltas. The table of validated feature values and the confidence level are generated from the development corpus by `cargo xtask assurance-audit`.

## 2026-09-26 — spectra: scan layout, energy axis, traces; complex elements

**Corpus files used:** RosettaSciIO's TIA test data (`github.com/hyperspy/rosettasciio` commit `bc254db`, `rsciio/tests/data/tia/`, GPL-3.0 data files): the spectrum images (`16x16-spectrum_image-5x5x1024`, `16x16-spectrum_image_5x5x4000-not_square`), line profiles (`16x16-line_profile_horizontal_10x1024`, `16x16-line_profile_diagonal_10x1024`), point spectra (`16x16-point_spectrum-1x1024`, `16x16-2_point-spectra-2x1024`), the combined spectrum-image/diffraction acquisition (`16x16-diffraction_imagel_5x5x256x256_EDS`), single spectra (`X - Au NP EELS_2.ser`, `no_AcquireDate`), 2-D series (`64x64x5_TEM_preview`, `16x16x5_STEM_BF_DF_preview`, `03_Scanning Preview`) and their `.emi` files; openNCEM's test data (`github.com/ercius/openNCEM` commit `749ec1a`, `ncempy/data/`, GPL-3.0 data files): `01_Si110_5images`, `16_STOimage`, `After HAADF 130mm`.

**Prior art consulted:** none in source form. RosettaSciIO 0.14.0 and ncempy (both GPL-3.0) were run as black boxes.

**Inferred (ours) and checked:**
- **Element order of a scan.** The header's dimension array lists the scan dimensions fastest first: in both 5 × 5 spectrum images the position tags (TagTypeID 0x4142) of elements 0–4 share one Y and step in X by dimension 0's `CalibrationDelta` (1.99e-9 m, 1.21e-10 m), and every fifth element steps Y by dimension 1's delta (negative: rows go down). Dimension units are stored (`meters`). RosettaSciIO returns the same arrays as (dimension 1, dimension 0, spectrum).
- **Spectrum calibration.** A 1-D element's calibration gives value(i) = offset + (i − element) × delta. The unit is not stored in the `.ser`; it is eV: the EELS files' `.emi` records "Filter selected dispersion 0.20 eV/Channel", equal to their delta 0.2, and the EDS spectrum `no_AcquireDate_1.ser` (offset 11.98, delta 9.975) has its peaks at 271, 521, 710, 940, 1748, 6366, 8022 and 8890 eV — the C K, O K, Fe L, Cu L, Si K, Fe Kα, Cu Kα and Cu Kβ lines (277, 525, 705, 930, 1740, 6404, 8048, 8905 eV).
- **What we return.** A 1-D series whose valid elements fill its scan dimensions becomes one image: X = dimension 0, Y = dimension 1 (1 for a line or a list of points), one channel per spectrum bin named by its energy (like DM and Velox spectrum images); the scan pixel size comes from the dimension deltas when their unit is `meters`. Every 1-D series is also a trace whose sweeps are the elements (axis: energy in eV). An incomplete scan, or a series without dimensions, keeps the previous layout (one row per element). 2-D series over a scan keep elements as T and gain `extra.frame_grid`.
- Complex elements (types 9, 10) are interleaved (real, imaginary) float32/float64 pairs, returned as `complex` / `double-complex` (no public file; synthetic unit test only).

## 2026-09-26 — scan steps of 1 m are not pixel sizes

**Corpus files used:** `rsciio-tia-16x16-2-point-spectra-2x1024-1` (and its `.emi`), `rsciio-tia-16x16-point-spectrum-1x1024-1`, the spectrum images and line profiles listed in the entry above (development tier). No held-out file.
**Prior art consulted:** none; ncempy (GPL-3.0) is only run by the oracle script as a black box, as before.

**Inferred (ours):** the two point spectra of `16x16-2_point-spectra-2x1024` fill a one-dimensional scan dimension described "Position", unit `meters`, delta exactly 1.0; the single point spectrum has the same 1.0 m delta. Every calibrated scan in the corpus (spectrum images, line profiles, the diffraction/EDS acquisition) steps 1.2e-10 to 4.3e-9 m. A delta of exactly 1 m is TIA's placeholder for positions that were never calibrated (point spectra are taken at positions picked by hand, not on a raster), and the image returned a 1 m (1e6 µm) pixel. Scan-dimension deltas now follow the rule element deltas already follow: taken as metres only below 1 mm; above, no pixel size, a note, and the delta stays in `extra.scan_dimensions`. The oracle script applies the same rule and the file's oracle was regenerated (physical size x null).

