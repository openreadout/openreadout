# Agilent FT-IR imaging

Agilent FT-IR imaging systems with a focal-plane array detector (Resolutions Pro software) write single tiles and mosaics of spectra or interferograms, one per detector pixel. OpenReadout returns them as a trace with one sweep per pixel and as an image with one channel per spectral point, plus the acquisition settings. Tiles are not assembled into a mosaic when the mosaic file is missing.

Derived from agilent-format 0.4.7 (MIT, read as documentation and used as a reference reader) and hex dumps of corpus files from two depositors. See `docs/provenance/agilent-fpa.md`.

Crate: `openreadout-spectro`, format id `agilent-fpa` (`AGILENT_FPA_FORMAT_ID`), reader
`AgilentFpaReader`; the dataset is the shared `SpectroDataset`.

## Files

| extension | holds |
| --- | --- |
| `.dat` | one tile's spectra (or, beside a `.dmt`, a whole mosaic's) |
| `.seq` | one tile's interferograms |
| `.bsp` | the header of a tile: compound file with the axis and the settings |
| `.dms` | a mosaic's spectra as one image |
| `.dmt` | the header of a mosaic |
| `.dmd`, `.drd` | one mosaic tile's spectra / interferograms, named `<stem>_XXXX_YYYY` |

Opening a `.bsp` opens the `.dat` (else `.seq`) beside it; opening a `.dmt` opens the `.dms`
(exit 6 with a hint when there is none). **Detection:** a data-file extension and the data
header below → definite; a `.bsp`/`.dmt` compound file with the streams `Spectra/IndexTable`
and `Version` → definite.

## Data files

A 1020-byte header, then float32 values, band-sequential: every pixel's point 0 (a row at a
time), then every pixel's point 1, and so on. The file is exactly 1020 + 4 × points × width ×
height bytes (checked on open, exit 4 otherwise).

| offset | type | meaning |
| --- | --- | --- |
| 0 | 4 bytes | `00 72 47 00` |
| 9 | u32 | points per pixel |
| 24, 26 | u16, u16 | width, height (pixels) |
| 129 | text | version (`3.4.0.0`), the `format_version` |
| 170 | u32 | pixel aggregation (detector pixels combined per side) |

Rows are stored bottom to top (a mosaic's `.dms` stores its lower tile first while tiles are
numbered top to bottom): row 0 of our images and sweep order is the top row. Sweep `k` is the
pixel at column `k mod width`, row `k div width` from the top.

## Header (`.bsp`, `.dmt`)

A compound file whose stream under `Spectra` (named by an id, beside `IndexTable`) is a
serialized record list: texts are length-prefixed (u32 n, n bytes; settings: u32 4, u32 n, u32 n,
bytes). What the reader takes from it:

- the axis (`Data` record at the start: after `1.00`, u32 1, 4, 8, the spacing f64, u32 4, the
  first index i32, u32 4, the point count i32): x = spacing × (first index + i), wavenumbers in
  1/cm; for interferograms the `Interferogram` property's nested `Data` record: optical path
  difference in cm (spacing 2 / effective laser wavenumber, zero at the centre burst);
- the x and y labels after the `Parms` record's `XO` code: `Absorbance` → channel
  `absorbance`, `Response` → `single_beam`, `Transmittance`, `Reflectance`; others `intensity`;
- the pixel whose spectrum the header also stores (`Row = r Col = c`, stored row and column
  from 0), in `extra.header_spectrum_pixel`;
- text settings (key, u32 4, 1, 4, 2, 4, 4, 4, length, 4, length, length, text): the first
  non-empty value per key; numeric properties (`PropType` records of code 0: an f64 after
  `1.00`, 1, 1, 8), e.g. `FPA Pixel Size`, `Visible Pixel Size`.

A missing or unreadable header leaves the x axis as the point index (note; assurance
`undecoded`).

## Mapping

One trace (`absorbance spectra`, `single_beam spectra`, … or `interferograms`) with one sweep per
pixel and a regular `extra.axis` (`wavenumber` 1/cm or `optical_path_difference` cm); one image
(width × height, one float channel per point, `preview --channel N` draws band N). Pixel size =
the detector pixel size × the aggregation (µm; inferred).

Trace `extra` (our vocabulary): `x_spacing`, `x_first_index`, `x_label`, `y_label`, `size_x`,
`size_y`, `aggregation`, `header_spectrum_pixel` (`stored_row`, `column`), `mosaic_tile`
(`[x, y]` of a `.dmd`/`.drd`), `detector_pixel_um`, `resolution_cm1`, `scans`,
`background_scans`, `laser_wavenumber_cm1`, `undersampling_ratio`, `apodization`,
`optics_mode`, `objective`, `detector`, `conversion` (the processing's `To`, e.g.
`Absorbance`). Image `extra`: `row_order`.

**Experiment.** `instrument.vendor` Agilent, `model` (`MicroscopePresent`, e.g. `UMA 600`),
`software_version` (`Software Version`); `acquisition.operator` (`User Stamp`), `started_at`
(`Time Stamp`, local time: `Wednesday, July 29, 2020 09:26:49` → `2020-07-29T09:26:49`);
`sample.id` (`Sample File Name`); `method.parameters`: `resolution`, `scans`,
`background_scans`, `laser_wavenumber`, `integration_time`, `apodization`, `optics_mode`,
`objective`, `detector`, `beamsplitter`, `source`, `pixel_size`.

## Validation

agilent-format 0.4.7 (`MAT=True`) on 13 data files from 2 depositors: spectrum and point counts,
six whole spectra per file bit for bit, 24 sampled x values each (1e-9), the middle plane bit for
bit, resolution and laser wavenumber; the mosaic `.dms` against agilent-format's mosaic
assembled from the `.dmd` tiles (it never reads the `.dms`).

## Known gaps

Tiles are not assembled into a mosaic when the `.dms` is missing (open the tiles). Stage
positions are not in these files. The header's own stored spectrum is not returned.

## Vocabulary (public identifiers of the Agilent module of `openreadout-spectro`)

| identifier | meaning |
| --- | --- |
| `AgilentFpaReader`, `AGILENT_FPA_FORMAT_ID` | the reader and its format id |
