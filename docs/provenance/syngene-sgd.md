# Syngene GeneSys `.sgd` provenance

## 2026-10-15 — before the reader was written (Richard Zimring with Claude as assistant)

**Corpus files used** (licences checked 2026-10-15): `sgd-zenodo3975844-pxi6` (Zenodo 3975844,
CC0-1.0: a Syngene PXi 6 capture, GeneSys 1.5.6.0, six images) and `sgd-dataverse-tw68ad-gbox`
(Harvard Dataverse doi:10.7910/DVN/TW68AD file 11642900, CC0-1.0: a G:Box mini 6 capture, GeneSys
1.6.1.0, one image). The same Dataverse dataset holds the depositor's TIFF export of that capture
(file 11642343, 8-bit RGB), used as a check and not added to the corpus.

**Prior art consulted:** the public [MS-CFB] specification for the container (read with
`openreadout_core::cfb`). olefile 0.47 (BSD-2-Clause), run as a black box, lists the storages and
streams and their sizes. No Syngene documentation or software, no other reader of the format.

### Inferred from the files (olefile listings and hex dumps, not committed)

- The file is a compound file. `Detector/MultiImage/MultiData` lists the image storages: u16 5,
  u32 count, then count × (u32 length, ASCII name), then a u32. The names are `RawData`,
  `RawData0`, `RawData1`, … ; each is a storage under `Detector/GeneSnap/`.
- Each image storage holds `PixelData`, `Capture`, `Description`, `Thumbnail` (a 100 × 80 BMP),
  `Annotations` (starts `GeneSys Annotation File`), `Note`, `User`, `View`, `Rotation` and, in the
  newer file, `SpotBlotAnalyser`.
- `PixelData` is a 26-byte header — u16 2, u32 width, u32 height, three u32 that are 1 in every
  stream, and a u32 code (2 in the one 8-bit stream, 4 in the 16-bit ones) — followed by width ×
  height samples, row by row from the top, little-endian. Every stream is exactly 26 + width ×
  height × bytes per sample long (916 × 733: 671454 and 1342882 bytes; 2748 × 2199: 12085730).
  The Dataverse capture's 16-bit samples correlate with the grey level of the depositor's TIFF
  export of the same capture at Pearson r = −0.9999 (the export is shown inverted), and lose that
  correlation when the rows or columns are flipped, which confirms width, height, row order and
  byte order.
- `Capture` starts u16 15, 8 bytes, u8 1, then texts written as u32 length + ASCII: the software
  (`GeneSys Version: 1.5.6.0`), a little-endian f64 exposure, a u16, then the light (`No Light`,
  `Upper White Light`), the camera (`Synoptics 6.0MP`), a second camera text (`ART-694 #31`), the
  imager (`PXi 6`, `G:Box mini 6`), a u16, and the filter (`No Filter`). Later texts name the
  set-up (`ECL : No Light : No Filter`) and the application (`ECL`, `Visible Marker`). The 8-bit
  `RawData` of the PXi file has empty software, camera and imager texts. The exposure is in
  seconds: in the PXi file the four chemiluminescence images record 0.5, 1.5, 4 and 8 and their
  peak signal above the background grows in the same proportion (1474, 3584, 9229, 18582 counts);
  the unit itself is an inference.
- `Description` holds u16 5, u32 Unix time, u16, u64 .NET ticks of the same instant, then u32
  length + ASCII user name, then (newer file) u32 length + a GUID. The times match the dates in the
  file names (2019-09-15, 2022-09-19).

### What is and is not checked independently

The image count, width and height come from olefile's stream listing and the `PixelData`
headers; the Dataverse TIFF export confirms the geometry and, through the correlation above, the
pixel layout. No independent reader decodes the samples, so the corpus test compares geometry and
pixel type only, not the planes. The meaning of the `Capture` fields beyond the texts and the
exposure is not known and they are not reported.

## 2026-10-27 — pixel count overflow (Richard Zimring with Claude as assistant)

The `whole_sgd` fuzz target found that doubling the pixel count of a `PixelData` header with width and height near 2^32 overflowed `u64`. The doubling is checked now, and such an image is reported as unreadable. Fixture: `crates/openreadout-gel/tests/fixtures/malformed/file-fuzz-pixel-count-overflow.sgd`. Prior art: none.
