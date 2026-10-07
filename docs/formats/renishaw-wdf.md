# Renishaw WiRE `.wdf`

Renishaw WiRE software (inVia Raman microscopes) saves single spectra, line scans, depth series and StreamLine maps as `.wdf` files. OpenReadout returns the spectra as one trace, the stage positions as a table, maps as images and the white-light image, with the laser and acquisition settings. Pre-WiRE-3 files and processed-data blocks are not read.

Derived from renishawWiRE (MIT, read as prior art and run as a reference reader), hex dumps of public WiRE 4.1 and 4.4 files, and text exports WiRE made of some of them. The property sets were decoded from hex dumps alone. Provenance: `docs/provenance/renishaw-wdf.md`. Format id `renishaw-wdf`, family `spectroscopy`, crate `openreadout-spectro` (`WdfReader`, `WDF_FORMAT_ID`).

A `.wdf` file is a chain of blocks: a 4-character name, an int32 uid, an int64 size (including this 16-byte header), the body. **Detection:** the file starts with `WDF1`.

## Blocks

| block | body |
| --- | --- |
| `WDF1` (512 bytes) | 0x3C points per spectrum (u32), 0x40 capacity (u64), 0x48 spectra stored (u64), 0x50 accumulations, 0x54/0x58 y/x list lengths, 0x5C origin lists, 0x60 application name (24 bytes), 0x78 version (4 × u16), 0x80 scan type, 0x84 measurement type (u32), **0x88 / 0x90 start and end times** (Windows FILETIME: 100 ns ticks since 1601, UTC; our finding: the first spectrum's time in `ORGN` lies between them in every corpus file), 0x98 spectral unit (u32), 0x9C laser wavenumber (f32, cm⁻¹), 0xD0 user (32 bytes), 0xF0 title |
| `DATA` | capacity × points float32, spectrum after spectrum |
| `XLST` / `YLST` | type (u32), unit (u32), the list (float32) |
| `ORGN` | number of lists (u32), then per list: type (u32; bit 31 = primary axis), unit (u32), name (16 bytes), capacity × 8 bytes |
| `WMAP` | flags, origin (3 × f32, µm), step (3 × f32), size (3 × u32) |
| `WHTL` | JPEG with EXIF (the white-light image) |
| `TEXT` | the measurement description |
| `WXDA`, `WXDM`, `WXCS`, `WXIS`, `ZLDC`, `MAP ` | property sets (below) |

**Codes named, and the corpus evidence.** x list unit 1 = Raman shift in cm⁻¹ (values 100–3200 with the laser line at 0); spectral unit 6 = counts; origin-list types 3/4/5 = X/Y/Z (their stored names say so), unit 5 = µm, type 11 with unit 24 = time as FILETIME ticks, types 16/17 = checksum and flags (stored names). Measurement types 1 single, 2 series, 3 map and scan types 1 static, 6 StreamLine, 7 StreamLineHR agree with each file's `TEXT` description. Other codes are reported as numbers (`x_list_type_code`, `x_list_unit_code`, `spectral_unit_code`, `scan_type_code`) and give an `x` axis or no y unit.

## Property sets (`PSET`)

`PSET`, u32 size, then items: type byte, flag byte (0x80: an array — u32 count, then the elements; 0x40 on a string: a compressed blob, kept as its size), u16 key, the value. Types: `?` and `c` one byte, `s` int16, `w` uint16, `i` int32, `l` uint32, `r` float32, `q` and `d` float64, `t` FILETIME, `u` text (u32 length), `b` binary (u32 length), `p` a nested set (u32 length), `k` the *name* of a key (u32 length, text). Keys with bit 15 set are named by `k` items in the same set; other keys are WiRE's predefined keys, which the file does not name: they appear as `#<number>` in `info --view full`, and only those whose meaning the values show are normalized — `WXIS` › `Microscope` › `#1023` the objective (`x50`, `x100 L`) and `#1022` its magnification; `System Configuration` › `#1019` the laser name (`532 nm edge`); `Serial Numbers` › `#1021` the instrument serial number; `WXDM` › `#1001` the measurement name; `MAP ` › `#411` the analysis label.

**Map analyses (`MAP `).** After the property set: a u64 count and one float32 per spectrum — a result WiRE computed for each spectrum (`Intensity At Point 1315`, `Signal To Baseline from 1550.00 To 1620.00`, ratios). They are columns `wire_map: <label>` of the spectra table.

## Data model

- **One trace:** one sweep per stored spectrum (count ≤ capacity: an interrupted measurement is reported by `check`). WiRE's x list is **not evenly spaced** (a grating spectrometer's calibration: steps of 1.38–1.72 cm⁻¹ in one file), so the trace has two channels — `raman_shift` (cm⁻¹, from `XLST`) and `intensity` (counts) — and `extra.axis` is `{quantity: raman_shift, unit: 1/cm, irregular: true, channel: 0, first, last, size}`. The name is the title for a single spectrum (else `Raman spectrum`), `Raman map (N spectra)`, `Raman series (N spectra)` or `Raman spectra (N)`.
- **Table `spectra`:** one row per spectrum: `spectrum`, the stage coordinates (`x_um`, `y_um`, `z_um`: the origin lists of data type 3/4/5 whatever their name, e.g. LiveTrack's `Z data`, converted to µm from µm/mm/m/nm; `x`/`y`/`z` without a unit when the list's unit is not a length), `time_s` (seconds from the first spectrum, from the FILETIME list), other origin lists by name (lower case, spaces as `_`, `_um` added for lists in µm such as LiveTrack's `z_actual_um` and `z_difference_um`), and the map analyses.
- **Map image** (WMAP with more than one row and column): `size_x` × `size_y` = the WMAP size, one channel per spectral point (channel names are the band positions, `1314.17 1/cm`), float32, pixel size = the WMAP step. A spectrum's pixel is column round((x − x₀)/Δx), row round((y − y₀)/Δy) from its stage coordinates, which handles both WiRE's row order (StreamLineHR, flags 0) and column order (StreamLine, flags 2); pixels without a spectrum are NaN. Maps whose coordinates do not fall on the grid are placed in storage order (a note says so). Line scans and depth/time series are the trace plus the table, not images. `preview --select c=N` draws the map at band N; `stats` gives per-band statistics.
- **White-light image:** attachment 0 (`extract FILE 0` writes the JPEG) and the last image (`white-light image`, 8-bit RGB decoded from the JPEG, after the map image), with `field_of_view_um` and `origin_um` from its EXIF (tags 0xA20E/0xA20F and 0xFEA0) and its size in pixels.

### `traces[].extra` (our vocabulary)

| key | from |
| --- | --- |
| `axis`, `data_type` (`RAMAN SPECTRUM`), `y_quantity` | `XLST`, header |
| `laser_wavelength_nm`, `laser_wavenumber_cm1` | header 0x9C (nm = 10⁷ / cm⁻¹) |
| `accumulations` | header 0x50 |
| `exposure_time_s` | `WXDM` › `Exposure Time` (ms) |
| `grating`, `grating_grooves_per_mm` | `WXIS` › `System Configuration` › `Grating`; `WXCS` › `Gratings` › that grating › `Groove Density (lines/mm)` |
| `laser_name`, `objective`, `objective_magnification` | `WXIS` predefined keys (above) |
| `laser_power_percent` | `WXIS` › `ND Transmission %` |
| `slit_opening_um`, `focus_mode` | `WXIS` › `Slits` › `Opening`; `System Configuration` › `FocusMode` |
| `detector`, `detector_temperature_c`, `detector_serial` | `WXCS` › `CCD` › `CCD`, `Temperature`; `WXDA` › `CCD serial number` |
| `measurement_type`, `measurement_type_code`, `scan_type`, `scan_type_code` | header 0x84, 0x80 |
| `acquired_at`, `ended_at` | header 0x88, 0x90 (UTC) |
| `title`, `operator` | header 0xF0, 0xD0 |
| `x_list_type_code`, `x_list_unit_code`, `spectral_unit_code`, `capacity` | header, `XLST` |
| `spectra_table`, `map_image` | indices of the table and image of this trace |

`images[].extra`: `trace`, `band_axis` (`{quantity, unit, first, last, size, irregular}`), `y_quantity`, `y_unit`, `map_origin_um`, `map_step_um`, `spectra_placed`, `wmap_flags`.

**Experiment.** `sample.id` = the title, instrument (Renishaw, WiRE and its version, serial number), `method.name` = the measurement name, `method.parameters`: `laser_wavelength` (nm), `exposure_time` (s), `accumulations`, `laser_power` (%), `grating`, `grating_grooves_per_mm`, `objective`, `objective_magnification`, `laser_name`, `slit_opening` (µm), `detector`, `detector_temperature` (°C), `focus_mode`, `scan_type`, `measurement_type`; `acquisition.started_at`, `operator`, `comment` (the `TEXT` block).

## Validation

- **renishawWiRE 0.1.16** (MIT) on the 10 corpus files: every spectrum's values and the Raman-shift list bit for bit (up to 64 spectra per file hashed), the X/Y/Z columns bit for bit, the time column within 2 µs (renishawWiRE divides the ticks before subtracting), laser wavelength and accumulations, and the map images (5 bands of the 109 × 103 StreamLineHR map, 4 bands of the 45 × 49 StreamLine map laid out on renishawWiRE's map grid).
- **WiRE's text exports** of three spectra (Zenodo 8102788, CC0) agree point by point to their 6 decimals.
- **WiRE's own map analyses** (`Intensity At Point 1315` and `1365` for all 11,227 spectra of `mapping.wdf`) equal our spectra linearly interpolated at those Raman shifts to 6 × 10⁻⁸ (float32 rounding): an independent check of both the x list and the values.
- **White-light images** of the three files that have one: size and µm per pixel equal renishawWiRE's EXIF reading; pixels decoded by our JPEG decoder differ from Pillow's (libjpeg) by at most 3 levels, 95–98 % identical, plane means within 0.034 (the corpus test's lossy tolerance is 0.05).

## Known gaps

Property-set keys WiRE does not name in the file are shown by number; the laser power is the neutral-density transmission (%), not an absolute power. Pre-WiRE-3 files, processed-data blocks (cosmic-ray removal, baseline results stored as extra `DATA`-like blocks) and Z-stack maps are not decoded.

## Vocabulary (public API of `openreadout-spectro`)

| identifier | meaning |
| --- | --- |
| `WdfReader` | the reader (`FormatReader`): detection by the first bytes, `open`/`open_input` |
| `WDF_FORMAT_ID` | the format id, `renishaw-wdf` |
| `SpectroDataset` | an opened file (the `Dataset` the four spectroscopy readers share): traces, tables, map images and attachments read lazily |

Everything else — trace names, channel names, `extra` keys and their values — is listed in the tables above in our own words.
