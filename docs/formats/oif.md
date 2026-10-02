# Olympus FluoView OIF / OIB

Olympus FluoView (FV1000 and FV1200 confocal systems) saves an acquisition as an OIB file (one compound file) or an OIF file (a settings file next to a folder of TIFF planes). OpenReadout returns the image with its channels, Z and time planes, pixel sizes, instrument and scan settings, and the thumbnail. Derived from hex dumps and settings-text listings of public data sets (FluoView 4.2), the documentation and source of `oiffile` (BSD-3-Clause), and black-box comparison with Bio-Formats 8.5.0; Bio-Formats and oiffile are the reference readers. No Olympus/Evident specification or SDK was used. Provenance: `docs/provenance/oif.md`. Format ids `oib` and `oif`, crate `openreadout-oif`. Every name below is ours.

## Containers (`store.rs`)

FluoView writes one acquisition as a set of files; two containers hold the same set:

- **OIF**: a main settings file `<name>.oif` plus a folder `<name>.oif.files` next to it holding everything else (plane TIFFs, `.pty` property files, `.lut` LUTs, `.roi` ROIs, a `s_Thumb.bmp` thumbnail with its `.pty`). Detection: the `.oif` extension and a settings-text start (`FF FE 5B 00`, UTF-16LE `[`, or a UTF-8 `[`) → definite.
- **OIB**: an OLE2 compound file (Microsoft's public **[MS-CFB]**, signature `D0 CF 11 E0 A1 B1 1A E1`, read with `openreadout_core::cfb`). Streams are named `StreamNNNNN`; the files of the data folder sit in a storage `Storage00001`. A root stream `OibInfo.txt` (settings text) maps names: section `[OibSaveInfo]` holds `MainFileName=StreamNNNNN` (the stream holding the `.oif` file), `StorageNNNNN=<name>.oif.files`, and one `StreamNNNNN=Storage00001/<file name>` line per member (`Stream00010=Spleenx20.oif`, `Stream00001=Storage00001/s_C001.tif`, ...), plus `Version`, `Compression` (`None` in every corpus file), `FileCount`, `FolderCount`, `ThumbFolderName`, `ThumbFileName`. Detection: the `.oib` extension and the compound-file signature → definite.

Both are opened as a `Store`: the parsed main settings (`main`, `main_name`), `OibInfo.txt` (`oib_info`, OIB only), and the `members` by lower-case file name, each a `Member` with its `name`, `size` and `source` (`MemberSource::Stream` for a compound-file stream, `MemberSource::File` for a file in the folder). `problems` lists what is missing (a stream `OibInfo.txt` names that does not exist, the `.oif.files` folder).

## Settings files (`settings.rs`)

Every metadata file (`.oif`, `.pty`, `OibInfo.txt`, `.roi`, `.lut`) is INI-style text: UTF-16LE with a byte-order mark in every corpus file (UTF-8 accepted when the file starts with `[`), `[Section]` lines and `Key=Value` lines, CR LF line ends. Values are unquoted numbers, `"double-quoted"` or `'single-quoted'` text. `.lut` files end with a binary `[ColorLUTData]` tail (after the UTF-16 line `[ColorLUTData]\r\n`), which is cut off and counted (`lut_bytes`).

`Settings::parse` → `sections` of `Section { name, entries }` in file order; `raw`, `text` (unquoted, empty → none), `number`, `section`, `to_json` (the whole file as `{section: {key: value}}` with integers, floats and text typed the way they are written, used for `vendor`), `unquote`, `MAX_SETTINGS_BYTES` (16 MiB bound).

**Decimal comma.** Corpus file `zenodo7080902` (the "does not open in ImageJ" test file) holds `WidthConvertValue=0,207.0` and `EndPosition=211,761.0`: numbers written with a decimal comma and a spurious `.0`. `number` reads `<digits>,<digits>[.0]` as `<digits>.<digits>` (0.207, 211.761); consistent with 1023 × 0.207 = 211.761 and with Bio-Formats' 0.207 µm (inferred).

## Main settings (`<name>.oif`)

Sections and keys we use (all others are kept in `vendor.main`):

| section | key | use |
| --- | --- | --- |
| `Axis Parameter Common` | `AxisOrder` (`XYCZ`, `XYCT`, `XYCL`, `XTC`, ...) | `extra.axis_order`; `dimension_order` (lambda counts as C; missing axes appended in C, Z, T order) |
| `Axis n Parameters Common` (n = 0..8) | `AxisCode` (`X`, `Y`, `C`, `Z`, `T`, `A`, `L`, `P`, `Q`), `MaxSize`, `StartPosition`, `EndPosition`, `Interval`, `PixUnit` | Z step = `Interval` of the `Z` axis in `PixUnit` (nm → µm) when size_z > 1; lambda bands from the `L` axis (below); the axis numbers of `T` and `Z` locate the per-plane positions in `.pty` files |
| `Channel n Parameters` | `CH Name`, `DyeName`, `ExcitationWavelength`, `EmissionWavelength`, `LightType` | channel n (the `C` number of the plane file names): `name`, `fluorophore` (`None` dropped), `excitation_nm`, `emission_nm` (0 dropped), `acquisition_mode` |
| `Acquisition Parameters Common` | `ImageCaputreDate` (sic), `ImageCaputreDate+MilliSec`, `ScanMode`, `Acquisition Device` | `acquired_at` (local time, no zone; a note says so), `extra.scan_mode`, instrument model fallback |
| `Reference Image Parameter` | `WidthConvertValue`, `HeightConvertValue`, `WidthUnit`, `HeightUnit`, `ValidBitCounts` | pixel size fallback when a plane has no `.pty`; `extra.bits_significant` fallback |
| `File Info` | `DataName` | image `name` |
| `Version Info` | `SystemName` (`FLUOVIEW FV1000`), `SystemVersion`, `FileVersion` | `instrument.model`, `instrument.software_version`, `format_version` |
| `ProfileSaveInfo` | `Version`, `PtyFileName*`, `LutFileName*`, `RoiFileName*`, `ReferenceFrameCount` | `format_version` fallback; `vendor` |

## Plane files and images (`dataset.rs`)

Each plane is one TIFF (`TIFF 6.0`; decoded by `openreadout-tiff`, from memory for OIB via `TiffFile::from_bytes`): 16-bit grey in every corpus file, uncompressed or LZW, one strip per few rows, one page. Its name carries the plane's indices: `s_` prefix, then axis letters each followed by a one-based number, e.g. `s_C001Z002T003.tif`, `s_C001L012.tif`, `s_C001T044.tif`; reference images of line scans carry a `-R###` suffix (`s_C001-R001.tif`). This convention is documented by oiffile (its `series` groups TIFF names by their letters and orders them by the numbers) and holds in every corpus file. `parse_plane_name` → `PlaneName { axes, reference }`; names without indices (`s_Thumb.tif` if present) are listed, not exposed.

Files are grouped into images by (reference mark, axis letters, values of unmapped letters): main acquisitions first, then reference images (`extra.kind = "reference"`, name `<DataName> [reference]`). Within an image the distinct numbers of each axis, sorted, become the ordinals; the letters map as `AXIS_CHANNEL` (C), `AXIS_Z` (Z), `AXIS_TIME` (T) and `AXIS_LAMBDA` (L, folded into C channel-major: c = channel ordinal × n_lambda + lambda ordinal, as for OIR); any other letter makes its own image (`extra.other_indices`; inferred, no corpus file). `REFERENCE_MARK` is `R`. Geometry and sample type come from the first plane's TIFF header; every plane is checked against it when read.

Lambda scans (`ScanMode="XYL"`): band i (1-based L number) starts at `StartPosition + (i − 1) × Interval` of the L axis and is `Resolution` nm wide (`Interval` when absent): `emission_range_nm = [lo, lo + width]`, `emission_nm` its centre; `extra.lambda_index` lists the L numbers. Corpus: 420–550 nm in 25 bands of 10 nm every 5 nm, matching the file name `420-550`.

Line scans (`ScanMode="XT"`, `AxisOrder="XTC"`): each plane TIFF is `X × lines` (228 × 10000); rows are successive lines in time (the plane's `.pty` gives `HeightUnit="ms"`), so no Y pixel size is reported; the reference images (`s_C00n-R00n.tif`, 256 × 256) are the overview frame the line was drawn on.

## Plane property files (`.pty`)

One per plane (`s_C001Z002.pty` next to `s_C001Z002.tif`), settings text:

| section | key | use |
| --- | --- | --- |
| `Image Parameters` | `WidthConvertValue`, `WidthUnit`, `HeightConvertValue`, `HeightUnit`, `ValidBitCounts`, `ImageGroup` (`Normal`, `Reference`) | pixel size x/y when the unit is a length (`um`), `extra.bits_significant` |
| `Acquisition Parameters Common` | `ObjectiveLens Name`, `Magnification`, `ObjectiveLens NAValue` (9999 = unset), `Time Per Line`, `Time Per Pixel` (µs), `PMTVoltage` | `objective` (name whitespace collapsed), `extra.line_time_us`, `extra.pixel_dwell_us`, frame `detector_voltage` |
| `Axis n Parameters` | `AbsPositionValue` of the T axis (ms since the start) and of the Z axis (nm) | frame `delta_t_s`, `stage_z_um`; `time_increment_s` = (last − first time) / (T − 1) from the first channel and z |

## Normalized model

`size_x/size_y`, `pixel_type`, `samples_per_pixel` from the TIFF; `size_c/z/t` from the plane names; `physical_size` from the first plane's `.pty` (else the main file); `instrument` = Olympus / FluoView / `SystemName` / `SystemVersion`; `extra`: `axis_order`, `scan_mode`, `bits_significant`, `compression` (TIFF compression of the first plane), `line_time_us`, `pixel_dwell_us`, `kind`, `lambda_index`, `other_indices`. `vendor` (`info --view full`): `main_file`, `main` (the main settings), `oib_info`, `first_plane_properties`. Frames (`info --view full`): `c`, `z`, `t`, `file`, `delta_t_s`, `stage_z_um`, `detector_voltage`. Attachment `#0`: the thumbnail BMP (`s_Thumb.bmp`). `info --view structure` lists planes, the main settings file and every other member (`properties`, `roi`, `lut`, `thumbnail`).

Differences from Bio-Formats (black-box observations): Bio-Formats reports the X pixel size as Y for line scans; its minimal-metadata mode (`-nometa`) reports no physical sizes.

## `check` finding codes

| code | severity | meaning |
| --- | --- | --- |
| `container` | error | a stream named in `OibInfo.txt` is missing, the compound file is damaged, or the `.oif.files` folder is missing |
| `planes` | error | no plane files, or (c, z, t) positions without exactly one plane file |
| `truncated` | error | a plane TIFF does not parse or decode completely (truncated stream or file) |
| `geometry` | error | a plane's size differs from the image's |
| `unsupported` | warning | a plane uses a TIFF feature the TIFF reader does not decode |
| `properties` | warning | a plane has no `.pty` file |

A truncated OIB whose directory or main settings are unreachable fails to open with exit 4.

## Vocabulary (every public identifier in `openreadout-oif/src` must appear here)

| identifier | meaning |
| --- | --- |
| `OibReader`, `OifReader`, `OIB_FORMAT_ID`, `OIF_FORMAT_ID`, `looks_like_settings` | the two readers, their ids `oib` and `oif`, settings-text detection |
| `FvDataset`, `open`, `store`, `images`, `files` | an opened data set, open, its container, its images, the files it is made of |
| `FvImage`, `info`, `reference`, `planes`, `duplicates` | one image: normalized info, reference-image flag, (c, z, t) → plane file name, repeated indices |
| `PlaneName`, `axes`, `parse_plane_name`, `AXIS_CHANNEL`, `AXIS_Z`, `AXIS_TIME`, `AXIS_LAMBDA`, `REFERENCE_MARK` | plane file names and their axis letters |
| `dimension_order`, `fluoview_date` | `AxisOrder` → `XYCZT`-style order; FluoView date → ISO-8601 local time |
| `Store`, `StoreKind` (`Oib`, `Oif`), `format_id`, `kind`, `path`, `main`, `main_name`, `oib_info`, `members`, `member`, `problems`, `read`, `settings`, `label` | the container (above) |
| `Member`, `MemberSource` (`Stream`, `File`), `name`, `size`, `source` | one file of the set |
| `Settings`, `Section`, `sections`, `entries`, `lut_bytes`, `parse`, `section`, `raw`, `text`, `number`, `to_json`, `unquote`, `MAX_SETTINGS_BYTES` | settings text (above) |
