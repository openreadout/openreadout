# Provenance log — Hamamatsu VMS (`.vms` + JPEG images)

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

## 2026-10-12 — initial reader (Richard Zimring with Claude as assistant)

**Corpus file:** `openslide-vms-cmu-1` — `Hamamatsu-vms/CMU-1.zip` of the OpenSlide test data (CC0-1.0 in its `index.json`), a NanoZoomer brightfield slide from 2010: one `.vms` index, four JPEG images on a 2 × 2 grid, a map JPEG, a macro JPEG and an optimisation file.

**Prior art consulted:** OpenSlide's public format page https://openslide.org/formats/hamamatsu/ (CC-BY-SA-4.0 text): the `[Virtual Microscope Specimen]` keys (`NoLayers`, `NoJpegColumns`, `NoJpegRows`, `ImageFile`, `ImageFile(x,y)`, `MapFile`, `OptimisationFile`, `MacroImage`, `SourceLens`, `PhysicalWidth/Height` in nanometres, the slide-centre offsets), that restart intervals make the JPEGs tiled (one MCU row high), that the optimisation file lists MCU-row offsets in 40-byte entries and misses the last row, that the map is a low-resolution copy of the slide, and that `[Uncompressed Virtual Microscope Specimen]` (VMU) files hold NGR images. No OpenSlide source was read.

**Oracle:** OpenSlide 4.0.1 (openslide-python 1.4.6, LGPL, run as a black box): level sizes, `openslide.mpp-x/y`, `read_region` pixels.

**What came from where:**
- From the file: the `.vms` key list above with `Reference`, `AuthCode` and `LayerSpacing` (meaning unknown, kept as metadata); `ImageFile(x,y)` with x the column (the (1,0) image is 40960 × 61440 and sits right of the 61440 × 61440 `ImageFile`); every JPEG is baseline, 8-bit, three components without subsampling, a JFIF APP0 and a comment naming the NanoZoomer, `DRI` 512 for the slide images (intervals 4096 × 8 pixels) and 64 for the map (512 × 8); the optimisation file's entries hold one little-endian 64-bit offset followed by 32 zero bytes; entry k of an image is the byte where MCU row k starts (the first equals the end of the SOS segment, the others follow a restart marker whose number is `(15·k − 1) mod 8`); the images' entries follow each other in raster order of the grid, 19 071 = 7680 + 7680 + 1856 + 1856 − 1 entries.
- From OpenSlide as a black box: its seven levels (102400 × 76288 down to 1600 × 1192): the slide images decoded at 1, 1/2 and 1/4 size, then the map (12800 × 9536, an eighth of level 0) at 1, 1/2, 1/4 and 1/8 size; micrometres per pixel = `PhysicalWidth / 1000 / width`.
- Not read, because no public file shows them: VMU files (NGR images) and slides with more than one focal layer. The reader refuses both (exit 6).
