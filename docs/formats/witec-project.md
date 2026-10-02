# WITec Project `.wip` / WITec Data `.wid`

WITec Control and WITec Project software (confocal Raman microscopes) save spectra, Raman maps, scalar images, video images and texts in `.wip` project and `.wid` data files. OpenReadout returns spectra as traces, maps and video images as images, and texts as attachments. Filters and mask definitions are listed, not decoded.

Derived from the published description of the tag format in wit_io (MIT-0, `README on WIT-tag format.txt`) and its Python port witio (MIT-0), read as documentation, and from public files of several depositors (WITec Control 1.60 to 7, format versions 5, 7 and 8). witio is the reference reader; the WITec software's own text export of one large data file confirms the calibration and the spectrum order independently. See `docs/provenance/witec-project.md`.

Crate: `openreadout-spectro`, format id `witec-project` (`WITEC_FORMAT_ID`), extensions `wip`
(project) and `wid` (data file), reader `WitecReader`; the dataset is the shared `SpectroDataset`.

## Detection

The first eight bytes: `WIT_PRCT` (project) or `WIT_DATA` (data file) for format versions 0–5,
`WIT_PR06` / `WIT_DA06` for versions 6 and later → definite. A `.wip`/`.wid` name without one of
them → extension only (open fails with exit 4).

## Container

After the magic, a tree of tags, little-endian. A tag is: name length (u32), name (Windows-1252),
type code (u32), start and end offsets of its data (u64, the end one past the last byte). The
data follows the header; the next tag starts at the end offset. Type codes: 0 a subtree (the
data is more tags), 2 float64, 3 float32, 4 int64, 5 int32, 6 dates (seven u16: year, month,
day, hour, minute, second, millisecond), 7 uint8, 8 booleans (one byte each), 9 strings (u32
length + bytes each). The reader refuses (exit 4) a tag whose data does not start after its
header or ends outside its parent, names longer than 4096 bytes, nesting deeper than 48 and
more than four million tags. Values up to 1 MiB are read while parsing, larger ones (the
spectra, images and bitmaps) on demand.

The root (`WITec Project` / `WITec Data`) holds `Version`, `SystemInformation` (application
version, system id, service and licence ids), a thumbnail, `Data` (pairs `DataClassName <n>` /
`Data <n>`, `NumberOfData`) and, in projects, `Viewer` (window layout; kept in the vendor
tree). Every data entry has a common record (`TData`: `ID`, `Caption`, history) and a record
named like its class. Entries refer to each other by `ID`.

## Mapping

| entry class | becomes |
| --- | --- |
| graph (spectra on an `SizeX × SizeY` grid, `SizeGraph` points each) | one trace, one sweep per spectrum; a table `<caption> positions` when the entry has a space transformation; an image with one channel per spectral point when both sizes exceed 1 (a Raman map) |
| image (a scalar map: band sums, peak positions, masks) | an image, one channel, the stored value type (masks as uint8 0/1) |
| bitmap (a video image) | an 8-bit RGB image; versions 0–5 also an attachment (the embedded BMP) |
| text (RTF) | an attachment (`text/rtf`); a measurement's `Information` text is parsed into its trace's `extra.information` |
| interpretations, transformations | the x axis, the positions and the pixel size of the entries that use them |
| filters, cursors, colour profiles, mask definitions | listed by `info --view structure` and kept in the vendor tree, not decoded (a note names them) |

**Spectrum order.** Sweep `k` is the spectrum at `x = k mod SizeX`, `y = k div SizeX` (along a
scan line first), the order of the WITec text export. The graph array stores each spectrum's
points contiguously; the spectra run along a column first (`y + SizeY·x`) unless the graph's
`DataFieldInverted` flag is set (then along a row first). Images follow the same rule with
`ImageDataIsInverted`. Value types (`DataType`): 1 int64, 2 int32, 3 int16, 4 int8, 5 uint32,
6 uint16, 7 uint8, 8 boolean, 9 float32, 10 float64.

**Incomplete scans.** Scan lines whose `LineValid` flag is false were not measured: their spectra
read as NaN (the map pixels too, and float image rows), `extra.incomplete_lines` lists them and a
note says how many.

**x axis.** The graph's `XTransformationID` names the calibration, its `XInterpretationID` the
display unit:

- spectral calibration type 1 (grating): λ(i) = (d/m)·(sin α + sin βᵢ) nm with α = asin(λc·m/(2d·cos(γ/2))) − γ/2,
  βᵢ = γ + α − δ − atan2(w·(nC − i) − f·sin δ, f·cos δ), for pixel i from 0; `nC` pixel centre,
  `λc` its wavelength, γ the included angle, δ the detector tilt (rad), m the order, d grooves/mm,
  w the pixel width and f the focal length (mm), all stored in the entry;
- type 0: second-order polynomial in the pixel index (nm); type 2: polynomial of the stored order
  between two pixel limits, constant outside them;
- linear (`value = step·(i − origin pixel) + origin value`) and lookup-table calibrations, in
  their interpretation's unit (time, position, frequency, …);
- the spectral interpretation's unit: 0 wavelength nm, 1 wavelength µm, 2 wavenumber 1/cm (10⁷/λ),
  3 Raman shift 1/cm (10⁷·(1/λexc − 1/λ)), 4/5 energy eV/meV (1239.84193/λ), 6/7 energy shift eV/meV.

Calibrated axes are irregular: the trace's channel 0 holds the x values and `extra.axis` says
`irregular`. A calibration of an unknown kind leaves x as the pixel index with a note (assurance:
`undecoded`).

**Orientation.** Rows run top to bottom: the space transformations of every map and video image
in the corpus map an increasing row index to a decreasing stage y (the version-5 BMP video
image, whose top row is known, has the same sign), so the first stored row (version 6+ bitmaps)
and the first scan line (maps) are the top.

**Positions.** A space transformation maps pixel (x, y, 0) to µm: `world + rotation·scale·(p − model)`
(3 × 3 matrices stored a column at a time). The positions table has `x_px`, `y_px`, `x_um`,
`y_um`, `z_um`; the map's pixel size is the length of the matrix's first two columns.

**Information text.** RTF, decoded to text (`\par` new line, `\tab` tab, `\'hh` Windows-1252),
then `Key:<tab>value` lines under `Section:` headings. A graph's text is the `Information` entry
with the same measurement caption (`Scan_000_Spec.Data 1_F (Sub BG)` → `Scan_000 Information`,
`Spectrum--003--Spec.Data 1` → `Spectrum--003--Information`), else the entry whose ID follows the
graph's.

### Trace `extra` (our vocabulary)

| key | from |
| --- | --- |
| `data_id` | the entry's ID |
| `size_x`, `size_y` | grid size |
| `value_type` | stored value type (`uint16`, `float32`, …) |
| `storage_order` | `column-first` or `row-first` (the inverted flag) |
| `x_calibration` | the calibration: `kind` (`grating`, `polynomial`, `pixel polynomial`, `linear`, `lookup table`) and its parameters (`center_pixel`, `center_wavelength_nm`, `included_angle_rad`, `detector_tilt_rad`, `diffraction_order`, `grooves_per_mm`, `pixel_width_mm`, `focal_length_mm`; `coefficients`, `first_pixel`, `last_pixel`; `origin_pixel`, `origin_value`, `step`; `entries`) |
| `x_calibration_caption`, `x_unit_code` | the calibration entry's caption, the interpretation's unit index |
| `excitation_wavelength_nm` | the spectral interpretation's excitation wavelength |
| `incomplete_lines` | scan lines not measured |
| `information`, `information_text` | the parsed information text (section → key → value) and its entry ID |
| `integration_time_s`, `accumulations`, `center_wavelength_nm`, `objective_magnification`, `detector_temperature_c` | information text numbers |
| `grating`, `objective`, `configuration`, `read_mode` | information text words |
| `acquired_at` | information text start date and time (local time, no zone) |

Image `extra`: `data_id`, `kind` (`image`, `video image`), `storage_order`, `stored_mean` and
`stored_deviation` (the image entry's own statistics), `value_unit`, `incomplete_lines`,
`origin_um` (position of pixel 0).

**Experiment.** `instrument`: vendor WITec, software and version from the application version
(`Control FIVE 5.1.12.68` → `WITec Control FIVE`, `5.1.12.68`), serial = the system id;
`acquisition.operator` (User Name), `acquisition.started_at` (local time); `sample.id` (Sample
Name, when filled); `method.parameters`: `laser_wavelength`, `integration_time`, `accumulations`,
`center_wavelength`, `objective_magnification`, `points_per_line`, `lines_per_image`,
`scan_width`, `scan_height`, `grating`, `objective`, `configuration` — from the first graph's
information text.

`format_version` is the root's `Version` (5, 7, 8 in the corpus; versions above 7 are read the
same way and noted).

## `check` finding codes

`graph_record_missing`, `graph_size`, `graph_value_type`, `graph_data_missing`,
`graph_data_size`, `image_record`, `image_data_size`, `bitmap_record`, `bitmap_data_size`,
`bitmap_size` (errors: the entry is left out), `data_count` (warning), `truncated`.

## Validation

- witio 0.2.0 (MIT-0) on 11 project files from six depositors: every graph's spectrum count and
  point count, whole spectra of up to six sweeps per graph (bit for bit as f64, including the
  blanked lines), the calibrated x values (relative 1e-9), every map, image and video image
  (bit for bit).
- The WITec software's text export of the version-5 data file `zenodo4944335-35d-crm2-scan1-crr`
  (300 × 210 × 1600 float32): 12 whole spectra, bit for bit, in the export's column order (which
  is the sweep order above), and their x in nm within the export's six significant digits.

## Known gaps

Linear and lookup-table x calibrations have no development file. Version-7/8 `Trace`
parameter records (Suite SIX and later) are kept in the vendor tree only; settings come from the
English information text. Filter and mask-definition records are not decoded.

## Vocabulary (public identifiers of the WITec module of `openreadout-spectro`)

| identifier | meaning |
| --- | --- |
| `WitecReader`, `WITEC_FORMAT_ID` | the reader and its format id |
