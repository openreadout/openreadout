# SMV diffraction images (`smv`)

SMV `.img` files hold one diffraction image after a text header: `{`, then `KEY=value;` lines, `}`, padded with spaces to `HEADER_BYTES` (512 or more). ADSC CCDs introduced the layout; electron-diffraction software (Instamatic, MicroED converters for Ceta and TimePix cameras) writes it too. Derived from dxtbx (BSD-3) and FabIO (MIT) read as documentation. Provenance: `docs/provenance/smv.md`. Crate: `openreadout-diffraction` (`src/smv.rs`).

## Layout

| key | use |
| --- | --- |
| `HEADER_BYTES` | where the image starts |
| `DIM` | must be 2 |
| `SIZE1`, `SIZE2` | samples per row (`size_x`), rows (`size_y`) |
| `TYPE` | `unsigned_short` (uint16), `signed_short`, `unsigned_int`, `signed_int`, `float` |
| `BYTE_ORDER` | `little_endian` or `big_endian` |
| `COMPRESSION` | refused unless absent or `None` |

Detection: `{` within the first bytes and `HEADER_BYTES=` in the first 128. A `.img.gz` file is decompressed once at open. Every `KEY=value` pair is kept as written (`info --view full` → `vendor.header`).

## Geometry

| key | `extra.diffraction` |
| --- | --- |
| `WAVELENGTH` (Å) | `wavelength_a` |
| `DISTANCE` (mm) | `distance_mm` |
| `PIXEL_SIZE` (mm) | `pixel_size_mm` |
| `BEAM_CENTER_X`, `BEAM_CENTER_Y` | `beam_center_px`: ADSC files give millimetres with X along the slow axis and Y along the fast axis, so x = Y ÷ pixel size and y = X ÷ pixel size; files that name `BEAMLINE=TimePix_SU` give pixels (x, y) as stored (dxtbx's rule). `extra.smv.beam_center_convention` says which |
| `OSC_START` (or `PHI`), `OSC_RANGE` | `oscillation_start_deg`, `oscillation_width_deg` |
| `AXIS` | `scan_axis` |
| `TIME` | `exposure_s` |
| `TWOTHETA` | `two_theta_deg` (when not 0) |
| `CCD_IMAGE_SATURATION`, `SATURATED_VALUE` | `overload` (65535 for uint16 without either) |

`radiation` is `electron` below 0.1 Å. `extra.smv` holds `beamline`, `detector_serial`, `date`, `binning`, `gain`, `pedestal` (`IMAGE_PEDESTAL`, not subtracted: values are returned as stored) and `beam_center_convention`. Rigaku-style headers keep their keys raw and get no beam centre.

The beam-centre rules were checked on the corpus images: the direct beam of the TimePix file and the centre of the diffuse scatter of the Ceta-D file lie at the computed positions (the Ceta-D header's centre is a nominal value 50 px from the scatter centre).

## `check`

| code | severity | meaning |
| --- | --- | --- |
| `truncated` | error | the file is shorter than the header and SIZE1 × SIZE2 samples |
| `trailing_bytes` | warning | bytes follow the image |
| `bad_geometry` | warning | DISTANCE, WAVELENGTH or PIXEL_SIZE is not a positive number |
| `beam_center_outside` | warning | the beam centre lies outside the image (another unit convention?) |

## Vocabulary

| identifier | meaning |
| --- | --- |
| `SmvReader`, `SmvDataset`, `SMV_FORMAT_ID`, `FORMAT_ID`, `open`, `header` | reader, opened file, format id `smv`, its header |
| `looks_like_smv` | detection |
| `SmvHeader`, `parse`, `fields`, `header_bytes`, `size`, `pixel_type`, `big_endian`, `get`, `data_len`, `beam_center_in_pixels`, `geometry` | the parsed header |
