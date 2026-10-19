# Provenance log — DeltaVision (`deltavision`)

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

## 2026-10-19 — reader (Richard Zimring with Claude as assistant)

**Prior art read (permissive):** `mrc` 0.4.0 by Sebastian Haase and Talley Lambert, BSD-3-Clause (https://github.com/tlambert03/mrc; licence in the package's `licenses/` folder), file `_new.py` (`DVFile`), read as documentation. From it: the 1024-byte header as a sequence of 32-bit integers, floats and 16-bit integers (sizes, pixel mode, start indices, sampling, pixel spacing dx/dy/dz, cell angles, axis order, minimum/maximum/mean, space group, extended-header length, the DeltaVision identifier −16224 at byte 96 and the byte order it gives, the number of integers and floats per section in the extended header, image type, lens id, number of time points, image sequence, tilt angles, number of wavelengths and five wavelengths, the origin, the 800-byte title); the pixel modes (0 uint8, 1 int16, 2 float32, 4 complex float32, 5 int16, 6 uint16, 7 int32); the three section orders (0 ZTW, 1 WZT, 2 ZWT); Z = sections ÷ (wavelengths × time points); data after the header and the extended header; per section of the extended header, the integers then the floats photosensor reading, time stamp, stage X, Y, Z, minimum, maximum and mean intensity, exposure time, neutral-density filter, excitation and emission wavelength, intensity scaling, energy conversion factor. Names in our code are ours.

**Black-box oracles:** `mrc` 0.4.0 produces the ground-truth planes; Bio-Formats 8.5.0 (GPL, `oracle/bftools`) is run as a black box for a second opinion on the geometry and pixel type.

**Corpus files used** (`corpus/manifest.toml`, format `deltavision`): OME sample images `DV/siRNAi-HeLa`, `DV/will/FRAP`, `DV/elena/P-TRE`, `DV/U2OS`, `DV/alexia/cajal-bodies` (each directory's `readme.txt` and `COPYING`: CC-BY-4.0); Zenodo 5887370 and 15919154 (CC-BY-4.0).

**Observed (hex dumps of the corpus files):**
- The identifier at byte 96 reads `a0 c0` in little-endian files and `c0 a0` in big-endian ones (`IN_02.r3d`, written by the acquisition computer, is big-endian; the deconvolved and projected files are little-endian).
- Header size checks: 1024 + extended-header length + width × height × sections × bytes per sample equals the file length in every complete file.
- Pixel modes 0 (a volume rendering), 1 (raw and projected), 2 (FRAP series) and 6 (deconvolved) occur; section orders 0, 1 and 2 occur (2: `IN_02.r3d`, the U2OS file and Zenodo 15919154; 1: the FRAP series and Zenodo 5887370; 0: the others).
- Extended header: 8 integers and 32 floats per section (160 bytes), the block padded to a multiple of 1024 bytes; the 14 named floats come first.
- Wavelengths 457, 528, 617, 685 nm; the FRAP series lists 528, −10 and −13 (photobleaching channels; the negative values are kept as written and not used as wavelengths).
- The title holds up to 10 lines of 80 characters: processing history (`IMGCORR`, `DECON3D`, `X:(1 512) Y:(1 512) Z:(1 40 1)`).

**Inferred (flagged in the format notes):** that the floats of the extended header are the same 14 in files with more floats per section (the rest are listed, not named); that the exposure time is in seconds and the time stamp in seconds from the start (compared with the `.log` files next to the OME samples); that a negative wavelength is not a wavelength.

**Validation:** `oracle/dv_oracle.py`.
