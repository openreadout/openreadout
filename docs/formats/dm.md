# Gatan Digital Micrograph DM3 / DM4 / DM5

Gatan DigitalMicrograph saves electron-microscopy images, spectra and spectrum images as DM3, DM4 and DM5 files. OpenReadout returns the images (including complex and RGBA data), single spectra as traces, the calibrated axes, and the microscope and acquisition metadata.

Derived from two MIT-licensed readers (pyDM3reader's `dm3_lib`, `dm4`), a public third-party format description (C. Boothroyd), Gatan's public DM5 documentation page, the Apache-2.0 reader in `nionswift-io`, and public corpus files; `dm3_lib` is used as a reference reader. Gatan does not publish the DM3/DM4 format; it documents DM5's HDF5 layout and the DataType list. See `docs/provenance/dm.md`.

A DM file is a short header followed by a **tree of tags**: *groups* (directories) and *data tags*. Images live in the group `ImageList`; each entry has `ImageData` (the pixels and their description), `ImageTags` (microscope and acquisition metadata) and `Name`. DM4 is DM3 with 8-byte structure words and a byte count on every tag.

## Header (`DmHeader`)

| bytes (DM3 / DM4) | our field | content |
| --- | --- | --- |
| 0–3 | `version` | 3 or 4, big-endian |
| 4–7 / 4–11 | `root_length` | bytes of the root tag group, big-endian: file length − `header_len` − 8 in 59 of the 89 development files, file length − `header_len` − 4 in the other 30 (rsciio's and Nion's test files, `zenodo13913066-*`, `ncem-dm-08-carbon-dm3`; DM3 and DM4 alike) |
| 8–11 / 12–15 | `little_endian` | 1 = tag values little-endian, 0 = big-endian |
| – | `header_len` | 12 (DM3) / 16 (DM4): where the root group starts |

The file ends with 8 zero bytes after the root group.

## Tags

Every structure word (counts, lengths, name lengths, info arrays) is **big-endian**; tag **values** use the header's byte order.

- **Group:** kind byte 20, name length (u16), name, [DM4: u64 byte length], sorted flag (u8), open flag (u8), entry count (u32 / u64), entries. Unnamed entries are common (`ImageList`, `Dimensions`, `Calibrations.Dimension`).
- **Data tag:** kind byte 21, name length, name, [DM4: u64 byte length], `%%%%`, info count, info words (i32 in DM3 / i64 in DM4), value.
- **Info words:** the first is the encoded type (`TagType`): 2 `I16`, 3 `I32`, 4 `U16`, 5 `U32`, 6 `F32`, 7 `F64`, 8 `Bool`, 9 `Char`, 10 `Octet`, 11 `I64`, 12 `U64`, 15 `Struct`, 18 `Text`, 20 `Array`; others → `Unknown`. A struct is followed by a name length, a field count and (name length, type) pairs; an array by its element type — a scalar type, or 15 and a struct spec — and then its element count (`ElementType`: `Scalar`, `Struct`). A DM3 string tag (18) carries its byte length.
- Short `U16` arrays hold UTF-16 text (units, names); they are decoded as strings when they are valid text.

We parse the whole tree at open, reading through a 1 MiB window and **skipping** arrays longer than `INLINE_ARRAY_LIMIT` (4096 elements) by offset, so pixel data is never read by `info`. DM4 tag lengths are checked against the parsed contents and used to resynchronise. Groups deeper than `MAX_DEPTH` (64) are refused. Parsed values: `TagGroup` (`entries` in file order; `get`, `group`, `path` with `.`-separated names, `value`, `children`, `to_json`), `Tag` (`Group`, `Data`), `TagValue` (`Json`, or `Array` with `element`, `count`, `offset`, `length` and `inline`, the decoded value of short arrays). Problems are `ParseIssue`s (`offset`, `message`).

## Images (`DmImage`)

For each `ImageList` entry (`list_index`): `ImageData.DataType` → `data_type` (`ImageDataType`), `ImageData.Dimensions` (unnamed scalars, fastest axis first) → `dimensions`, `ImageData.Calibrations.Dimension[i]` (`Scale`, `Origin`, `Units`) → `calibrations` (`Calibration`: `scale`, `origin`, `units`), the `ImageData.Data` array → `data_offset`, `data_length`, `Name` → `name`. `Thumbnails[*].ImageIndex` marks entries as `thumbnail`.

| DataType | `ImageDataType` | bytes | returned `pixel_type` |
| --- | --- | --- | --- |
| 1 | `Int16` | 2 | `int16` |
| 2 | `Float32` | 4 | `float` |
| 3 | `Complex8` | 8 | `complex` (real, imaginary float32) |
| 5 | `PackedComplex` | 4 | not decoded (exit 6): no public file shows the layout |
| 6 | `Uint8` | 1 | `uint8` |
| 7 | `Int32` | 4 | `int32` |
| 8 | `Rgb` | 4 | not decoded (exit 6): the byte order is not established |
| 9 | `Int8` | 1 | `int8` |
| 10 | `Uint16` | 2 | `uint16` |
| 11 | `Uint32` | 4 | `uint32` |
| 12 | `Float64` | 8 | `double` |
| 13 | `Complex16` | 16 | `double-complex` |
| 14 | `Binary` | 1 | `uint8`, values 0/1 |
| 23 | `Rgba` | 4 | `uint8` × 3 samples (R, G, B; the alpha byte is dropped) |
| 27 | `PackedComplex8` | 8 | `complex`: the stored half plane, X/2 + 1 columns |
| 28 | `PackedComplex16` | 16 | `double-complex`: the stored half plane |
| 35, 36 | `Int64`, `Uint64` | 8 | `int64`, `uint64` (no corpus file yet) |
| other (15–22, 24–26, ...) | `Other` | – | not decoded |

Complex elements are interleaved (real, imaginary) numbers; on big-endian files each number is byte-swapped separately (`component_bytes`). RGBA bytes are R, G, B, A in that order (Gatan's DM5 page: "8 bits each for Red, Green, Blue, and Alpha. The alpha channel contains no useful data"). A packed Fourier transform (27, 28) holds only the non-redundant half of the transform of a real image: (X/2 + 1) × Y complex values for `Dimensions` X × Y (`stored_dimensions`, `packed_half_plane`); it is returned as stored, with `extra.packed_half_plane` and `extra.full_size_x`, because no file establishes which half is kept.

`bytes_per_pixel`, `pixel_type`, `samples_per_pixel`, `packed_half_plane`, `label`, `from_code` are the helpers behind this table.

**Which entries are images.** Entries not listed in `Thumbnails` are images, in `ImageList` order (index 0 = the first data image). Thumbnails (in every corpus file entry 0, a 384-pixel RGBA preview) are **attachments** (`export --attachment #0` writes their raw pixels; `extra` gives width, height and data type). A file whose only entries are thumbnails exposes them as images.

**Axes** (`axes`, `AxisMap`). Calibrated values along a dimension are (index − `Origin`) × `Scale` (checked on three files: the Ti L3 edge of an STO spectrum at 452.75 eV, a zero-loss peak at −0.003 eV, an apatite plasmon at 23.4 eV). The **spectral** dimension, if any:

| `ImageTags.Meta Data.Format` | dimensions | spectral dimension |
| --- | --- | --- |
| `Spectrum image` | 3 (x, y, energy) | 2 |
| `Spectrum image` | 2 (a line scan, stored bins × positions) | 0 |
| `Spectrum` | 1 or 2 | 0 |
| absent | 3, third unit eV/keV/meV | 2 |
| absent | 1 or 2, first unit eV/keV/meV | 0 |

When every other dimension has size 1 the data is a **single spectrum** (`single_spectrum`): it is returned as an image one pixel high (X = the spectral dimension, as before) **and as a trace** (`traces[]`: one channel `intensity` in the brightness units, raw values; `extra.axis` = `{quantity, unit, first, step, size, dimension, last}`). Otherwise the spectral dimension is returned as **channels** (C), one per bin, named by the bin's calibrated value (`"532.25 eV"`), with `extra.spectral_axis`; the remaining dimensions are X and Y. Quantities: `energy loss` (Signal EELS), `energy` (eV units), `wavelength` (Signal CL or a length unit), else `spectral axis`.

Without a spectral dimension: X = `Dimensions[0]`, Y = `Dimensions[1]`; a third dimension is **Z** when its calibration unit is a length and `Meta Data.IsSequence` is not set, **T** otherwise; with four or more dimensions (4D-STEM: detector frames over a scan) the first two are the frame and the others are flattened into T, fastest first (`extra.frame_grid` gives their sizes). `extra.third_axis` says `z` or `t`. Planes are gathered with strides (`strides`), so a line scan's channel planes (every bin-th element) are read without loading the whole array into memory beyond a 64 MiB window per row.

**Physical size.** `Scale` × unit factor for length units (`length_to_um`): `nm` 1e-3, `µm`/`um` 1, `Å` 1e-4, `pm` 1e-6, `mm` 1e3, `m` 1e6. Reciprocal (`1/nm`), energy (`eV`) and empty units give no physical size; all calibrations stay in `extra.calibrations`.

## Metadata used from `ImageTags`

| tag path | our field |
| --- | --- |
| `Microscope Info.Microscope` (else `Microscope Info.Name`) | `instrument.model` |
| `Acquisition.Device.Name` | `instrument.detector` |
| `GMS Version.Created` | `instrument.software_version` (software `Gatan DigitalMicrograph`) |
| `Acquisition.Parameters.High Level.Exposure (s)`, else `Acquisition.Frame.Sequence.Exposure Time (ns)` | `channels[0].exposure_ms` |
| `DataBar.Acquisition Time (OS)` (a Windows FILETIME stored as a double), else `Acquisition.Frame.Sequence.Acquisition Start Time (epoch)` (ms) | `acquired_at` (UTC) |
| `Microscope Info.Voltage` (V) | `extra.voltage_kv` |
| `Meta Data.Format`, `Meta Data.Signal`, `Meta Data.IsSequence` | `extra.meta_format`, `extra.signal`, `extra.is_sequence` |
| `ImageData.Calibrations.Brightness.Units` | `extra.intensity_units`, the trace channel unit |
| `Microscope Info.*`, `Acquisition.Parameters.High Level.Exposure (s)` / `Binning`, `DataBar.Acquisition Date` / `Time` | `extra.microscope` (`voltage_v`, `indicated_magnification`, `actual_magnification`, `operation_mode`, `illumination_mode`, `imaging_mode`, `stem_camera_length`, `cs_mm`, `probe_current_na`, `exposure_s`, `binning`, `acquisition_date`, `acquisition_time`) |

`DataBar.Acquisition Date/Time` are locale-dependent strings (US or international order); they are copied, never parsed. `vendor` (`info --view full`) is the whole tag tree as JSON (groups of unnamed entries become arrays; large arrays are described by type, count, offset and size).

Other `extra` keys: `data_type`, `dimensions`, `calibrations`, `third_axis`, `image_list_index`.

## DM5

A `.dm5` file is HDF5 (the extension plus an HDF5 signature is a definite detection). Gatan documents the layout on its public DM5 page: every tag group is an HDF5 group, every data tag an attribute, and entries named `[k]` are the unnamed entries of a DM3/DM4 group (list items, `Dimensions`), in `k` order (`read_tree`; at most 64 levels and 50 000 groups are walked, so a malformed file whose groups link back into their own subtree ends with an issue instead of an exponential walk). An image's pixels are the dataset `ImageList/[i]/ImageData/Data` (`Dm5Data`: `path`, `shape` slowest first, `element_bytes`, `contiguous` byte range, `big_endian`): contiguous datasets are read from the file like DM3/DM4 arrays, chunked or compact ones are read whole on first use (up to 2 GiB; larger ones exit 6). GMS 3.53 stores an RGBA thumbnail as an HDF5 true-colour image of three bytes per pixel although `PixelDepth` says 4 (`stored_pixel_bytes`). Everything else — axes, calibrations, traces, thumbnails as attachments — is as for DM3/DM4; `format_version` is `DM5`.

## `info --view structure`

`metadata tag-directory` (byte range and completeness), one entry per `ImageList` entry (`image` or, for thumbnails, `attachment`) with the pixel array's offset and size.

## `check` finding codes

| code | severity | meaning |
| --- | --- | --- |
| `truncated` | error | root length larger than file length − header − 4, or a pixel array runs past the end (exit 4) |
| `tag_directory` | error | the tag tree cannot be parsed to the end |
| `missing_data` | error | an `ImageList` entry has no `Data` array |
| `dimension_mismatch` | error | `Dimensions` × bytes per pixel ≠ `Data` size |
| `root_length`, `tag_length` | warning | root length smaller than expected; a DM4 tag's byte count disagrees with its contents |
| `bad_thumbnail_index`, `no_images` | warning | `Thumbnails.ImageIndex` out of range; empty `ImageList` |
| `end_marker`, `unknown_data_type` | info | no 8 zero bytes after the directory; an undecoded `DataType` |

## Observed corpus values

| file | version | images (DataType, dimensions) | calibration units |
| --- | --- | --- | --- |
| `zenodo8190744-EELS-STO` | DM3 | float32 [2048] | eV |
| `zenodo8190744-bto-atomic` | DM3 | float32 [1024, 1024] | nm |
| `zenodo14541027-SAED1-TEM-0003` | DM3 | uint16 [2048, 2115, 1] | nm, nm, (none) |
| `zenodo13821437-Figure-2e` | DM4 | float32 [1024, 1024] | 1/nm (diffraction) |
| `zenodo8398370-NF-0001` | DM4 | int32 [2048, 2048] | µm |
| `zenodo20131420-SI-nanoFe` | DM4 | uint32 [512, 512] survey; float32 [21, 8] HAADF; float32 [21, 8, 2048] EELS spectrum image (energy → C) | nm; nm; nm, nm, eV |

## Vocabulary (every public identifier in `openreadout-em/src/dm` must appear here)

| identifier | meaning |
| --- | --- |
| `DmReader`, `DmDataset`, `FORMAT_ID`, `looks_like_dm` | format reader, opened file (core `Dataset`), the id `dm`, header check used by `sniff` |
| `open`, `tags`, `images` | open a file; its tag tree; its `ImageList` entries (thumbnails included) |
| `DmHeader`, `version`, `root_length`, `little_endian`, `header_len`, `parse` | file header |
| `TagGroup`, `entries`, `get`, `group`, `path`, `value`, `children`, `to_json` | a tag group and its accessors |
| `Tag` (`Group`, `Data`), `TagValue` (`Json`, `Array` with `element`, `count`, `offset`, `length`, `inline`) | tree nodes |
| `TagType` (`I16`, `I32`, `U16`, `U32`, `F32`, `F64`, `Bool`, `Char`, `Octet`, `I64`, `U64`, `Struct`, `Text`, `Array`, `Unknown`), `from_code`, `width` | encoded tag value types |
| `ElementType` (`Scalar`, `Struct`) | array element types |
| `INLINE_ARRAY_LIMIT`, `MAX_DEPTH` | parser limits |
| `ParseIssue`, `offset`, `message` | a structural problem found while parsing |
| `DmImage`, `list_index`, `name`, `data_type`, `dimensions`, `calibrations`, `data_offset`, `data_length`, `thumbnail`, `meta_format`, `meta_signal`, `is_sequence`, `intensity_units`, `stored_pixel_bytes`, `stored_dimensions`, `axes` | one `ImageList` entry |
| `AxisMap`, `x`, `y`, `spectral`, `single_spectrum`, `stack`, `stack_is_z` | where each stored dimension goes (Axes above) |
| `Dm5Data`, `path`, `shape`, `element_bytes`, `contiguous`, `big_endian` | a DM5 image's `Data` dataset |
| `Calibration`, `scale`, `origin`, `units` | one axis calibration |
| `ImageDataType` (`Int16`, `Float32`, `Complex8`, `PackedComplex`, `Uint8`, `Int32`, `Rgb`, `Int8`, `Uint16`, `Uint32`, `Float64`, `Complex16`, `Binary`, `Rgba`, `PackedComplex8`, `PackedComplex16`, `Int64`, `Uint64`, `Other`), `bytes_per_pixel`, `pixel_type`, `samples_per_pixel`, `packed_half_plane`, `label` | image data types |
| `length_to_um` | length-unit conversion |
