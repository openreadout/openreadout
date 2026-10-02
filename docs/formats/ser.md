# TIA / ES Vision series files (SER + EMI)

TIA, the imaging and analysis software of FEI (now Thermo Fisher) electron microscopes, writes images, spectra and spectrum maps as `.ser` series files with an `.emi` metadata file. OpenReadout returns each series as an image, and series of spectra also as traces on an energy axis.

Derived from the series-file description once published by Emispec (the original ES Vision vendor), as reproduced on C. Boothroyd's public page, and from three public files; checked against `ncempy` run as a black box. See `docs/provenance/ser.md`.

TIA (Tecnai / Titan Imaging and Analysis) writes an acquisition as `<name>.emi` (display layout, a copy of the data, and an XML metadata document) plus one or more `<name>_<n>.ser` **series files** holding the data. Opening a `.ser` reads that series (and the `.emi` next to it for metadata); opening an `.emi` reads every `<name>_<n>.ser` beside it as one image each.

## Series file (all values little-endian)

| bytes | name in the description | our field (`SerHeader`) | content |
| --- | --- | --- | --- |
| 0–1 | ByteOrder | – | `II` (0x4949, `BYTE_ORDER_LE`) |
| 2–3 | SeriesID | – | 0x0197 (`SERIES_ID`) |
| 4–5 | SeriesVersion | `version` | 0x0210 (4-byte offsets) or 0x0220 (8-byte offsets); `offset_width` |
| 6–9 | DataTypeID | `data_type_id` | 0x4120 1-D elements (`ELEMENTS_1D`), 0x4122 2-D elements (`ELEMENTS_2D`) |
| 10–13 | TagTypeID | `tag_type_id` | 0x4152 time (`TAG_TIME`), 0x4142 time and position (`TAG_TIME_POSITION`) |
| 14–17 | TotalNumberElements | `total_elements` | product of the dimension sizes |
| 18–21 | ValidNumberElements | `valid_elements` | elements actually written (fewer if the acquisition stopped) |
| 22–25 / 22–29 | OffsetArrayOffset | `offset_array_offset` | start of the data offset array |
| 26–29 / 30–33 | NumberDimensions | `dimensions.len()` | scan dimensions |
| … | dimension array | `dimensions` (`SerDimension`: `size`, `calibration_offset`, `calibration_delta`, `calibration_element`, `description`, `units`) | 28 bytes + description + 4 + units each |
| offset array | data offsets, then tag offsets | `data_offsets`, `tag_offsets` | `total_elements` each, 4 or 8 bytes |

**Element headers** (`ElementHeader`: `offset`, `data_type`, `size_x`, `size_y`, `calibration_x`, `calibration_y`, `data_offset`; `data_len`): 2-D elements start with X and Y calibrations (`AxisCalibration`: `offset`, `delta`, `element` — f64, f64, i32), the data type (u16) and the array width and height (u32 each), 50 bytes in all; 1-D elements with one calibration, the data type and the length, 26 bytes.

| element data type | `SerDataType` | returned `pixel_type` |
| --- | --- | --- |
| 1 / 2 / 3 | `Uint8` / `Uint16` / `Uint32` | `uint8` / `uint16` / `uint32` |
| 4 / 5 / 6 | `Int8` / `Int16` / `Int32` | `int8` / `int16` / `int32` |
| 7 / 8 | `Float32` / `Float64` | `float` / `double` |
| 9 / 10 | `Complex64` / `Complex128` | `complex` / `double-complex` (interleaved real, imaginary) |
| other | `Other` | not decoded |

(`from_code`, `bytes`, `pixel_type`, `label`.)

**Tags** (`ElementTag`: `tag_type`, `time`, `position`): tag type (u16), two bytes the description calls undocumented (zero in the corpus), time as Unix seconds (u32, **UTC** — the corpus tag of `Fig_b1_1` is 16:08:35 while the `.emi`'s local-time `AcquireDate` is 18:08:35 CEST); position tags add X and Y as f64 at bytes 8 and 16.

## What we expose

- One image per series file; `extra.layout` says which of three layouts (`SerLayout`, `layout`):
  - `frames` (**2-D elements**): `size_x`/`size_y` from the first valid element, `size_t` = valid elements (the scan is flattened in element order; `extra.scan_dimensions` keeps it and `extra.frame_grid` lists its sizes when there are several dimensions).
  - `scan` (**1-D elements filling the scan**: one or two dimensions, every element valid, sizes multiplying to the element count): X = dimension 0 (the fastest: position tags step in X along it), Y = dimension 1 (1 for a line or a list of points), one channel per spectrum bin named by its energy (`"-19.8 eV"`). Scan pixel sizes come from the dimension deltas when their unit is `meters` and the delta is below 1 mm (TIA writes exactly 1 m on positions it never calibrated, e.g. point spectra; such a step stays in `extra.scan_dimensions` and is not a pixel size).
  - `rows` (**other 1-D series**: a single spectrum, an incomplete scan): one image with `size_x` = spectrum length and one row per element.
- **Spectra.** Bin i of a 1-D element is at offset + (i − element) × delta of its calibration, in eV (the `.ser` does not store the unit; inferred from an `.emi` dispersion of 0.20 eV/Channel equal to the delta and from EDS peaks at their X-ray line energies; `docs/provenance/ser.md`). `extra.spectral_axis` gives `quantity`, `unit`, `first`, `step`, `size`. Every series of 1-D elements is also a **trace** (`traces[]`) whose sweeps are its elements, one channel `counts`, `extra.axis` = energy axis.
- **Row order.** 2-D element data is stored bottom row first; rows are returned top to bottom. We learned this from the two readers we can run: `ncempy` and RosettaSciIO (both GPL, run as black boxes) return the stored rows reversed on all three corpus files.
- **Pixel size.** The `.ser` stores calibration deltas without units. A 2-D element delta in (0, 1e-3) is taken as metres and converted to µm (inferred: 0.6 nm, 0.076 nm and 11.7 nm pixels in the corpus, all plausible image scales); anything else (diffraction patterns calibrated in reciprocal units) gives no physical size. Raw calibrations are in `extra.element_calibration`.
- `acquired_at` = time tag of the first element. Per-element tags are `frames` records in the shared vocabulary (`book/src/guides/metadata.md`): `frame` and our `element` (the element index), `t` (2-D series), `acquired_at` (UTC), plus `time_unix_s` and, for position tags, `position_x`/`position_y` as stored (unit not documented) — `info --view full` embeds them.
- Elements whose size or type differ from the first are listed by `check` (`mixed_elements`) and make `read_plane` exit 6.

## The `.emi` sidecar (`EmiInfo`)

Only one part is read: the XML document `<ObjectInfo>…</ObjectInfo>`, found by searching the file (first its last 16 MiB, then the whole file up to 512 MiB). The sidecar of `<stem>_<n>.ser` is `<stem>.emi` in the same directory (`sidecar_of`); the series of an `.emi` are `<stem>_<n>.ser` sorted by `n` (`series_of`). Used: `ExperimentalConditions/MicroscopeConditions/AcceleratingVoltage` (volts) → `extra.voltage_kv`; `ExperimentalDescription/Root/Data` label/value/unit triples (`description`) → `extra.emi.experimental_description`, `Microscope` → `instrument.model`, `Magnification` → `extra.magnification`, `Mode` → `extra.mode`; `AcquireDate` (local time, text) → `extra.emi.acquire_date`. The whole document is `vendor.emi`. `field`, `summary`, `well_formed`, `read`, `xml`, `path`, `offset` are the accessors.

## `check` finding codes

| code | severity | meaning |
| --- | --- | --- |
| `truncated` | error | an element's header or data, or a tag, lies past the end of the file (exit 4) |
| `data_type_id`, `element_count` | error | unknown element kind; more valid than total elements |
| `series_version`, `tag_type_id` | warning | unknown series version or tag type |
| `incomplete_series` | warning | fewer valid than total elements |
| `dimension_product` | warning | dimension sizes do not multiply to the element count |
| `mixed_elements` | warning | an element differs from the first in size or type |
| `emi_unreadable`, `emi_xml` | warning | the opened `.emi` has no `<ObjectInfo>`; it is not well-formed |
| `no_emi` | info | no sidecar next to the `.ser` |

## Observed corpus values

| file | version | elements | element | delta (m) | `.emi` |
| --- | --- | --- | --- | --- | --- |
| `zenodo17463176-Fig_b1_1` | 0x0220 | 1 of 1 | 1024 × 1024 uint16 | 5.97e-10 | yes (Tecnai Osiris, 200 kV, STEM) |
| `zenodo17463176-Fig_b2_1` | 0x0220 | 1 of 1 | 2048 × 2048 uint16 | 7.58e-11 | yes |
| `zenodo13821437-Fig2c-part1` | 0x0210 | 1 of 1 | 1024 × 1024 uint16 | 1.17e-8 | no |

RosettaSciIO's and openNCEM's TIA test files add 1-D series (spectrum images 5 × 5, line profiles of 5 and 10 points, point spectra, single EELS and EDS spectra), position tags, 2-D series of 5 frames and a 5 × 5 scan of 256 × 256 diffraction patterns, in both series versions.

## Vocabulary (every public identifier in `openreadout-em/src/ser` must appear here)

| identifier | meaning |
| --- | --- |
| `SerReader`, `SerDataset`, `FORMAT_ID`, `open` | format reader, opened `.ser`/`.emi` (core `Dataset`), the id `ser`, open |
| `SerLayout` (`Frames`, `Scan`, `Rows`), `layout` | how a series is returned |
| `SerSeries`, `path`, `header`, `first` | one series file: its path, header and first valid element |
| `SerHeader`, `version`, `data_type_id`, `tag_type_id`, `total_elements`, `valid_elements`, `offset_array_offset`, `dimensions`, `data_offsets`, `tag_offsets`, `offset_width` | series header |
| `SerDimension`, `size`, `calibration_offset`, `calibration_delta`, `calibration_element`, `description`, `units` | one scan dimension |
| `ElementHeader`, `offset`, `data_type`, `size_x`, `size_y`, `calibration_x`, `calibration_y`, `data_offset`, `data_len` | one data element |
| `AxisCalibration`, `delta`, `element` | element axis calibration (with `offset`) |
| `SerDataType` (`Uint8`, `Uint16`, `Uint32`, `Int8`, `Int16`, `Int32`, `Float32`, `Float64`, `Complex64`, `Complex128`, `Other`), `from_code`, `bytes`, `pixel_type`, `label` | element data types |
| `ElementTag`, `tag_type`, `time`, `position` | per-element tag |
| `BYTE_ORDER_LE`, `SERIES_ID`, `ELEMENTS_1D`, `ELEMENTS_2D`, `TAG_TIME`, `TAG_TIME_POSITION`, `looks_like_ser` | signature values and detection |
| `EmiInfo`, `xml`, `read`, `field`, `summary`, `well_formed`, `sidecar_of`, `series_of` | the `.emi` sidecar |
