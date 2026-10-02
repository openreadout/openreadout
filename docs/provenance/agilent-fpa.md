# Provenance log — Agilent FT-IR imaging (focal-plane-array) files

Rules: `docs/legal/clean-room-policy.md`. Each entry: date, who, corpus files, prior art (URL + license), what was inferred.

## 2026-09-26 — first reader (Richard Zimring with Claude as assistant)

**Prior art consulted** (permissively licensed; documentation and source read, no code copied):
- agilent-format 0.4.7 (python-agilent-file-formats, Canadian Light Source), MIT,
  https://github.com/stuart-cls/python-agilent-file-formats (`agilent_format/agilent.py`): the
  file set (`.dat`/`.seq` single tile with a `.bsp` header; `.dmt` header with `_XXXX_YYYY.dmd`
  and `.drd` tiles for mosaics), the 255 float32 values before the data, the band-sequential
  data order (all pixels of point 0, then point 1, …), the wavenumber axis
  `spacing × (first index + i)` and the interferogram axis in the same form, the tile numbering
  (left to right, top to bottom) and the statement that the tiles themselves are in Cartesian
  order (bottom row first), the settings it reads (time stamp, pixel aggregation, resolution,
  under-sampling ratio, effective laser wavenumber, symmetry, FPA and visible pixel sizes).
  agilent-format itself reads the header at fixed file offsets; the reader here parses the
  header as a compound file instead (below).
- agilent-format is also run as the oracle (`oracle/agilent_fpa_oracle.py`), a black box.

**Corpus files used:** `agilentformat-4-noimage-agg256` (`.dat`, `.seq`, `.bsp`),
`agilentformat-background-agg256`, `agilentformat-background-agg1024`, `agilentformat-5-mosaic-agg1024`
(`.dms`, `.dmt`, two `.dmd` and two `.drd` tiles; MIT, the agilent-format test data),
`zenodo4057208-cancer-a`, `zenodo4057208-cancer-b` (`.dat`, `.seq`, `.bsp`; CC-BY-4.0).

**What was inferred from the files** (hex dumps, before the parser was written):
- Every data file (`.dat`, `.seq`, `.dms`, `.dmd`, `.drd`) starts with the bytes `00 72 47 00`;
  the u32 at byte 9 is the point count, the u16s at bytes 24 and 26 the width and height in
  pixels, byte 129 a NUL-terminated version text (`3.4.0.0` in all seven files) and the u32 at
  byte 170 the pixel aggregation (16 in `agg256`, 32 in `agg1024`, 1 in the Zenodo files); the
  file size is exactly 1020 + 4 × points × width × height. A mosaic's `.dms` holds the whole
  mosaic (its width and height are the tiles'); in the corpus mosaic its first four rows are
  tile `_0000_0001` and the next four tile `_0000_0000`, so with tiles numbered top to bottom the
  stored rows run bottom to top, as agilent-format states for the tiles.
- The header files (`.bsp`, `.dmt`) are compound files (`D0 CF 11 E0`) with the storages
  `Spectra` (a stream named by an id and `IndexTable`), `PeakList`, `StingrayViewParms` and the
  streams `Version`, `2.4`. The spectrum stream is a serialized object: length-prefixed texts
  (u32 length, bytes; settings repeat the length: u32 4, u32 n, u32 n, bytes), a `Data` record
  (`1.00`, then u32 1, u32 4, u32 8, the spacing as f64, u32 4, the first index as i32, u32 4,
  the point count as i32, the byte count and the values as f64), a `Parms` record with the x and
  y labels (`Wavenumber`; `Absorbance` for sample images, `Response` for background images),
  typed properties (`PropType` records: code 0 = one f64 after `1.00`, 1, 1, 8; code 7 = a nested
  `Data` record, used for `Interferogram`) and string settings (key, then u32 4, 1, 4, 2, 4, 4,
  4, the length, 4, the length twice, the text).
- The `Data` record's values are the spectrum of one pixel, named by the stream's `SpectName`
  (`Row = 1 Col = 4`): in all five single-tile files they equal, as f64, that pixel's float32
  values in the data file at stored row and column (0-based), which confirms the band order
  and the row/column addressing.
- The interferogram axis (`.seq`, `.drd`): spacing 1.266e-4 cm = 2 / effective laser wavenumber,
  first index negative (−68, −260): optical path difference in cm, zero at the centre burst.
- The effective pixel size at the sample is the FPA pixel size times the aggregation (inferred:
  the aggregated images are 128 / 16 = 8 and 128 / 32 = 4 pixels wide).
