# Nikon ND2

Nikon NIS-Elements saves microscope acquisitions as `.nd2` files. OpenReadout returns the images with their channels, Z planes and time points, physical sizes, per-frame records, events and ROIs. Derived from public files by hex dump and a Python chunk walker, cross-checked against the public BSD-3 source of the `nd2` Python package (a pure-Python reader). Provenance: `docs/provenance/nd2.md`.

ND2 ("Nikon NIS-Elements image") comes in two generations. The modern one (NIS-Elements ≥ 3, `Ver2.x`/`Ver3.x`) is a flat sequence of named chunks with a chunk map at the end of the file. The legacy one (NIS-Elements 2.x) is a JPEG 2000 box sequence whose frames are JPEG 2000 codestreams and whose metadata are XML boxes (§ Legacy container). Both are read.

## Modern container (all integers little-endian)

### Chunk

| offset | size | field | value / meaning |
| --- | --- | --- | --- |
| 0 | u32 | `chunk_magic` | `0x0ABECEDA` (bytes `DA CE BE 0A`) |
| 4 | u32 | `name_len` | bytes of the name field (padded; observed 32 and 4128) |
| 8 | u64 | `data_len` | payload length |
| 16 | `name_len` | `name` | ASCII, ends with `!`, zero padded |
| 16 + `name_len` | `data_len` | `payload` | |

Chunks are 4096-byte aligned in the corpus (`chunk_alignment`), but the reader never assumes alignment. A frame chunk whose declared `data_len` runs past the end of the file is reported as corrupt by `read_plane`, even when the pixel bytes it needs happen to be present.

### File signature chunk (offset 0)

Name `ND2 FILE SIGNATURE CHUNK NAME01!`, payload 64 bytes starting with the ASCII version, e.g. `Ver3.0`.

### Chunk map

The last 8 bytes of the file hold `chunk_map_offset`, the offset of a chunk named `ND2 FILEMAP SIGNATURE NAME 0001!`. Its payload is a sequence of entries: `name` (ASCII up to and including `!`), u64 `chunk_offset` (offset of the chunk header), u64 `chunk_data_len`. The list ends with the entry `ND2 CHUNK MAP SIGNATURE 0000001!` (the 32 bytes before the final offset repeat that name).

When the map is missing or damaged (interrupted acquisition), `rescue_scan` walks the file for `chunk_magic` followed by a plausible name and rebuilds the map; `check` reports that this happened. When `uiSequenceCount` is 0 (acquisition never finalized) the frame count is the number of `ImageDataSeq|N!` chunks found.

### Chunk names we use

| name | payload | role |
| --- | --- | --- |
| `ImageAttributesLV!` | LV tree `SLxImageAttributes` | geometry: `uiWidth`, `uiHeight`, `uiWidthBytes` (row stride), `uiComp` (interleaved samples per pixel = channels), `uiBpcInMemory`, `uiBpcSignificant`, `uiSequenceCount` (frames), `eCompression`, `ePixelType`, `uiVirtualComponents` |
| `ImageMetadataLV!` | LV tree `SLxExperiment` | the acquisition loop tree (below); absent for single-frame files |
| `ImageMetadataSeqLV\|0!` | LV tree `SLxPictureMetadata` | first frame: `dTimeMSec`, `dTimeAbsolute` (Julian day of the experiment start), `dXPos`, `dYPos`, `dZPos`, `dCalibration` (µm/px), `wsObjectiveName`, `dObjectiveMag`, `dObjectiveNA`, `dRefractIndex1` (else `dRefractIndex`; immersion refractive index → `extra.refractive_index`, which OME-TIFF export writes as `ObjectiveSettings/@RefractiveIndex`), `sPicturePlanes` (channels, § Channels). Only frame 0 carries one in every corpus file |
| `ImageCalibrationLV\|0!` | LV tree `SLxCalibration` | `dCalibration` µm/px, `dAspect`, `sObjective` |
| `ImageTextInfoLV!` | LV tree `SLxImageTextInfo` | free text; `TextInfoItem_5` holds a `Dimensions: T(3) x XY(4) x λ(2) x Z(5)` line, `TextInfoItem_9` a local-time date string, `TextInfoItem_13` the optics, `TextInfoItem_14` the application version. When the frame metadata and the calibration chunk name no objective, `TextInfoItem_13` is the objective name and the description's `Numerical Aperture: 1.4` line its NA (`ome-jonas-control002`); the magnification is the number before `x` in the name (`20xC` → 20) |
| `ImageDataSeq\|N!` | f64 `frame_timestamp_ms` then `uiHeight` rows of `uiWidthBytes` bytes | frame N's pixels, channels interleaved per pixel |
| `CustomDataVar\|CustomDataV2_0!` | LV tree | the custom-data tag table (§ Per-frame records) |
| `CustomData\|<tag>!` | raw array, one value per frame | a custom-data column named by the tag table (`X`, `Y`, `Z`, `Z1`, `PFS_STATUS`, `Camera_ExposureTime1`, …) |
| `CustomData\|AcqTimesCache!` | f64 per frame | acquisition time of each frame, ms since the experiment start |
| `ImageEventsLV!`, `ImageEvents!`, `CustomData\|ExperimentEventsV1_0!` | LV / XML `RLxExperimentRecord` | experiment events (§ Events) |
| `CustomData\|RoiMetadata_v1!` | LV tree | ROIs (§ ROIs) |
| `CustomDataVar\|AppInfo_V1_0!` and other `CustomDataVar\|*` | LV | decoded into `info --view full`'s `vendor` tree |

### LV ("lite variant") encoding

A sequence of items: u8 `item_type`, u8 `name_units` (UTF-16 code units including the terminator), UTF-16LE `name`, then a value:

| type | value |
| --- | --- |
| 1 | u8 boolean |
| 2 | i32 |
| 3 | u32 |
| 4 | i64 |
| 5 | u64 |
| 6 | f64 |
| 7 | u64 (opaque) |
| 8 | UTF-16LE string, `00 00` terminated |
| 9 | u64 length + bytes. Either opaque bytes (validity masks such as `pItemValid`, matrices; exposed as numbers up to 4096 bytes) or a nested LV structure, recognized by a plausible item header (type 1–11 and a name of ≥ 1 character, or type 76) and decoded when that succeeds |
| 11 | nested level: u32 `child_count`, u64 `level_len`, then `child_count` items, then `child_count` × u64 offsets (skipped) |
| 76 (`L`) | compressed block: after a 10-byte header the rest of the buffer is a zlib stream of more items, spliced into the enclosing level |

Repeated names inside a level (e.g. `""` under `ppNextLevelEx`, `aN` planes) become arrays. Nesting is bounded (64 levels) so malformed input errors out instead of recursing without end.

### Experiment loop tree → frame index

`SLxExperiment` is a loop with `eType`, `uLoopPars` (the parameters; `uiCount` is the declared count), `pItemValid` (a per-item validity mask), `uiRepeatCount` and `ppNextLevelEx` children (one per nested loop). Loop types (numbering from the `nd2` package's public enumeration):

| `eType` | our name | frames it contributes | notes |
| --- | --- | --- | --- |
| 1 | `time_loop` | `uiCount` | `dPeriod` ms between frames |
| 2 | `xy_position_loop` | number of `Points` whose `pItemValid` entry is set | `Points` with `dPosX/Y/Z`, `dPosName`, `dPFSOffset`; `bRelativeXY` adds `dReferenceX/Y`; older trees store parallel lists `dPosX`, `dPosY`, `dPosZ`. `ome-karl-sample-image`: `uiCount` 15, one valid point, 21 frames written |
| 4 | `z_stack_loop` | `uiCount` | `dZStep` µm (when 0: `\|dZHigh − dZLow\| / (count − 1)`) |
| 6 | `spectral_loop` | none | the λ loop: carries `pPlanes` (the channels, count `pPlanes/uiCount`). Channels are interleaved inside each frame, so it never indexes frames and does not add a nesting level |
| 7 | `custom_loop` | `uiCount` | no corpus file; folded into T |
| 8 | `ne_time_loop` | sum of `uiCount` over periods in `pPeriod` whose `pPeriodValid` flag is set | `uLoopPars/uiCount` counts all periods including invalid ones. Periods keep their own `dPeriod`; `time_periods` in `extra` lists them |
| other | `loop_type_N` | `uiCount` | folded into T, with a note |

Rules for walking the tree (mirroring the `nd2` package, validated on every corpus file): a node without loop parameters, whose type is 0, or whose count is 0 ends its branch; at a nesting level that already holds a loop, a sibling of the same kind replaces it only when it counts more frames, and a sibling of another kind is ignored. `uiRepeatCount` > 1 appears only on a nested loop and equals the number of times its parent runs it (`ome-jonas-control002`: a Z loop with repeat 65 under a 65-step NE-time loop); it adds no axis and is reported in `extra.loops`. Legacy trees may list loops as siblings `LoopNo00`, `LoopNo01`, … outermost first.

Frame index = row-major flattening of the counted loops from outermost to innermost. For `T(3) × XY(4) × Z(5)`: `frame = t*20 + p*5 + z`. Several T-axis loops (NE time plus a custom loop) combine mixed-radix into one T index.

When the loops describe fewer frames than the file holds (`ome-aryeh-b16-14-12`: 50 described, 51 written; Bio-Formats also reports 50) the extra frames are not addressed. When they describe more (interrupted acquisition) the outermost loop is cut to the frames written (rounded up) and planes past the end report corrupt; both cases add `layout_note` to `notes` and a `loop_mismatch` warning to `check`.

### Pixel types

`ePixelType` 1 with `uiBpcInMemory` 8/16/32 → `uint8`/`uint16`/`uint32`; `ePixelType` 2 with 32 → `float` (inferred from prior art, no float corpus file yet). Planes whose `uiCompCount` is 3 are RGB (`samples_per_pixel` 3); `uiComp` is the sum of the planes' component counts (`zenodo8161776-VPA002`: four RGB planes, `uiComp` 12).

**RGB sample order.** A modern file stores the three samples of a colour-camera plane as B, G, R (validated: in `zenodo8161776-VPA002` the DAPI plane lights sample 0 and the Texas Red plane sample 2; the `nd2` package's pseudo-wavelengths 420/515/590 nm per component agree). Planes are returned R, G, B, so `export --to ome-tiff` writes true photometric RGB; `info` says so per RGB image with `extra.sample_order` (`RGB`) and `extra.stored_sample_order` (`BGR`). The `nd2` package returns stored order, so the corpus oracle reverses its `S` axis for modern RGB files. Legacy (JPEG 2000) planes keep the codestream order, which each `jp2h` `colr` box declares as sRGB (`stored_sample_order` `RGB`; **inferred**, no legacy file with a reference). Four-component planes are not handled as RGB.

`eCompression`: 2 = uncompressed; 0 = lossless, where the block after the timestamp is a zlib stream inflating to `uiHeight × uiWidthBytes` (confirmed on `ome-jonas-nd2Test-Exception-2`); 1 = lossy. No public lossy sample exists (checked: every modern corpus file, the four ND2 files of Zenodo records 21162526 and 8161776, and the ND2 of Zenodo 5277605 read over HTTP ranges, all `eCompression` 2; the `nd2` package's published sample metadata lists no lossy file and its reader has no lossy decoder). The codec is therefore unknown and lossy frames report `Unsupported` (exit 6) with metadata still readable.

### Version 2 metadata (XML variants)

Version 2.x files carry the same metadata as XML documents in chunks named without `LV` (`ImageAttributes!`, `ImageMetadata!`, `ImageMetadataSeq|N!`, `ImageCalibration|N!`, `ImageTextInfo!`): `<variant version="1.0"><no_name runtype="CLxListVariant"><uiWidth runtype="lx_uint32" value="696"/>…`. Scalar `runtype`s: `lx_uint32`, `lx_int32`, `lx_uint64`, `lx_int64`, `double`, `bool`, `CLxStringW`, `CLxByteArray` (base64). Lists are elements with children (`_00`, `_01`, … or named; repeated element names become arrays). `variant_decode` produces the same JSON shape as `lv_decode`; the wrapper (`SLx…` or `no_name`) is unwrapped by `unwrap_root`. Version 2 picture planes live under `sPicturePlanes/sPlane` and name channels by `sDescription` or `sOpticalConfigName`.

## Channels (`sPicturePlanes`)

`sPicturePlanes/sPlaneNew/aN` (or `sPlane/aN`), in index order, one per channel:

| field | our field | notes |
| --- | --- | --- |
| `sDescription` (else `sOpticalConfigName`) | `channels[].name` | |
| `uiCompCount` | samples per pixel | 3 = RGB |
| `uiColor` (else `pFluorescentProbe/m_uiColor`) | `channels[].color` | red in the lowest byte: `65280` → `#00FF00`, `255` → `#FF0000` (byte order checked against the `nd2` package's colours on every corpus file) |
| `uiModalityMask` (else `eModality` mapped to a mask) | `extra.channel_settings[].modality`, `channels[].acquisition_mode` | bits below |
| `pFluorescentProbe/m_sName` | `channels[].fluorophore` | when not empty |
| `pFilterPath/m_pFilter/*/m_sName` | `extra.channel_settings[].filters` | |
| probe then first filter `m_ExcitationSpectrum` / `m_EmissionSpectrum` | `channels[].excitation_nm` / `emission_nm` | a probe spectrum gives the wavelength of its point with the largest `dTValue` (first on ties); a filter spectrum given by a rising and a falling edge (`eType` 2 and 3) gives the centre of the band (2026-09-26: the first-on-ties rule picked the rising edge, e.g. 500 nm for a 500–550 nm filter), other filter spectra their peak; for excitation, a filter spectrum of several `eType` 4 points gives the point at the plane's index. Points carry `dWavelength` or, in older files, integral `uiWavelength` (the `nd2` package ignores the latter). RGB planes get none |
| emission filter points of `eType` 2 and 3 (rising and falling edge) | `channels[].emission_range_nm` | |
| `sSampleSetting/aK` (K = `uiSampleIndex`, or the plane index when 0; the only entry when there is one, shared by every plane, as on the Nikon A1plus files of Zenodo 21162526) | `extra.channel_settings[]` | `pCameraSetting/{CameraUserName, CameraUniqueName}` → `detector`, `dExposureTime` (else `PropertiesQuality/Exposure`) → `exposure_ms`, `PropertiesQuality/GainMultiplier` → `em_gain`, `FormatQuality/fmtDesc/dBinningX × dBinningY` → `binning`, `sSpecSettings` `key: value` lines → `camera_settings`; `pObjectiveSetting` fills the objective when frame 0 has none |
| older files: per-plane `sCameraSetting` | same | `sCameraName`, `dExposure`, `dGain` → `gain`, `dCamBinningX/Y`, `sSpecSettings` |

Modality mask bits (values from the `nd2` package's public enumeration; our names): `0x1` fluorescence, `0x2` brightfield, `0x10` phase_contrast, `0x20` dic, `0x40` rcm, `0x80` vcs, `0x100` camera, `0x200` laser_scanning_confocal, `0x400` spinning_disk_confocal, `0x800` swept_field_confocal_slit, `0x1000` swept_field_confocal_pinhole, `0x2000` dsd_confocal, `0x4000` sim, `0x8000` isim, `0x10000` multiphoton, `0x20000` tirf, `0x40000` live_sr, `0x100000` pmt, `0x200000` spectral, `0x400000` vaas_if, `0x800000` vaas_nf, `0x1000000` transmitted_light_detector, `0x2000000` non_descanned_detector, `0x4000000` virtual_filter, `0x8000000` gaasp, `0x10000000` remainder, `0x20000000` aux, `0x40000000` sora. A mask with neither of the two light bits reads as brightfield for RGB planes and fluorescence otherwise. `eModality` 0–12 map to fluorescence+camera, brightfield+camera, fluorescence+laser_scanning_confocal, fluorescence+spinning_disk_confocal, fluorescence+swept_field_confocal_slit, fluorescence+multiphoton+laser_scanning_confocal, brightfield+phase_contrast, brightfield+dic, fluorescence+spectral+laser_scanning_confocal, fluorescence+vaas_nf+laser_scanning_confocal, fluorescence+vaas_if+laser_scanning_confocal, fluorescence+vaas_nf+laser_scanning_confocal, dsd_confocal.

`acquisition_mode` is a readable summary: Brightfield / Phase Contrast / DIC, or Widefield, Spinning Disk Confocal, Laser Scanning Confocal, Swept Field Confocal, Multiphoton or TIRF Fluorescence.

## Time

`dTimeAbsolute` of frame 0 is the Julian day number (UTC) of the experiment start: `acquired_at` = ISO-8601 of `(jdn − 2440587.5) × 86 400 000` ms since the Unix epoch, rounded to the millisecond. Values outside 1900–2100 (uninitialized clocks store small numbers, e.g. `ome-jonas-control002` 397.7) give no `acquired_at`. `extra.acquired_at_source` is then `julian_day_utc`. Only when the Julian day is missing or implausible is the local-time text `TextInfoItem_9` normalized instead (`text_datetime_to_iso8601`, source `text_local_time`): it is ISO-8601 *without* an offset, because the text is the acquisition PC's wall-clock time in its own locale and the file records neither the zone nor the locale. Its date order varies with that locale (`9/28/2021  9:34:47 AM` is month-first, `06/03/2009  10:58:30 AM` in `aics-ND2-jonas-header-test2` is day-first — 6 March per its Julian day, `3-7-2026  13:32:25` is day-first), so a date whose day and month are both ≤ 12 and differ yields no `acquired_at` rather than a guess (`ome-jonas-control002`: `11/3/2009`). The text itself is always kept as `extra.acquired_at_text` (`ome-karl-sample-image`: text `06/06/2017 11:15:06`, JDN → `2017-06-06T09:15:06.980Z`, the site being UTC+2). A frame's absolute time is `jdn + time_ms / 86 400 000`.


When `acquired_at` comes from the text (`extra.acquired_at_source` = `text_local_time`), `info` adds a note that the value is local clock time with no recorded time zone (`book/src/guides/metadata.md` § Timestamps).

## Per-frame records

`info --view full` embeds one record per frame under `images[i].extra.frames` (the first 100 per image; `--all-frames` for all), with `frame_records_total` and `frames_truncated`. A record covers all channels of a frame (they are interleaved in one frame and share one timestamp).

| field | source |
| --- | --- |
| `frame` | sequence index (`ImageDataSeq\|N!`) |
| `t`, `z` | the frame's T and Z index within the image |
| `period` | index of the NE-time period the frame falls in (only with more than one period) |
| `time_ms` | `CustomData\|AcqTimesCache!` (legacy: `VIMD` `dTimeMSec`) |
| `acquired_at` | `time_ms` added to the start Julian day |
| `stage_x_um`, `stage_y_um` | custom-data columns `X`, `Y`; else the XY-loop point (legacy: `dXPos`, `dYPos`) |
| `stage_z_um` | column `Z`, else the lowest-numbered `Z<n>` column (a Z drive; `ome-karl-sample-image` has only `Z2`, "Ti ZDrive"); legacy `dZPos` |
| `pfs_status`, `pfs_offset` | columns `PFS_STATUS`, `PFS_OFFSET` |
| `exposure_ms`, `camera_temperature_c` | columns `Camera_ExposureTime1`, `CameraTemp1` |
| `exposure_ms_per_channel` | legacy: each plane's `sCameraSetting/dExposure` |
| `tags` | every other custom-data column by tag id (`Z1`, `Camera_ExposureTime2`, `1-AO0-Op`, …) |

The tag table `CustomTagDescription_v1.0/Tag<n>` has `ID` (the chunk suffix), `Type` (`value_kind`: 3 = f64, 2 = i32, 1 = UTF-16 strings in equal slots), `Size` (values), `Desc` and `Unit`.

## Events

`RLxExperimentRecord`: compact records `pEvents/*/{T time ms, T2 second clock, M meaning, D description, A data, I id, S stimulation}`, or (older and legacy) `pFirstEvent/no_name[]/{dTime, eMeaning, wsDescription, wsData}`. Normalized to `extra.events[]` = `{time_ms, meaning, meaning_code, description, data}` (at most 256 in `info`; `event_count` gives the total). Meaning codes 0–54 (from the `nd2` package's public enumeration) get our names: unspecified, autofocus, user_1_old … user_4_old, jobs, command, macro, pause, resume, cancel, ram_grab_zero_time, time_loop_next_phase, refocus, stimulation, external_stimulation, experiment_start, experiment_end, phase_start, phase_end, before_xy_move, after_xy_move, before_z_series, after_z_series, before_lambda_loop, after_lambda_loop, before_large_image, after_large_image, before_stimulation, after_stimulation, user_events, stream_data, user_1 … user_8, before_capture, after_capture, real_time_ttl_data, no_acquisition_start, no_acquisition_end, hardware_error, storm_event, incubation_info, incubation_error, interactive_experiment_end, experiment_pause, wid_replenishment_start, wid_replenishment_end, nstorm. Legacy `IEVE` boxes hold the older form (`aics-ND2-aryeh-but3-cont200-1`: command events such as `Wait(2);`).

## ROIs

`CustomData|RoiMetadata_v1!` → `RoiMetadata_v1` with keys carrying a lowercase type prefix that is dropped (`m_vect2PerMPoint_Size` → `2PerMPoint_Size`): global ROIs `Global_Size`, `Global_<i>`; per-position ROIs `2PerMPoint_Size`, `2PerMPoint_<p>/{Size, "<i>"}`. Each ROI: `Id`, `Info/{ShapeType, InterpType, Scope, Label, Color, …}`, `AnimParams_Size`, `AnimParams_<k>/{TimeMs, CenterX, CenterY, CenterZ, RotationZ, BoxShape/{SizeX, SizeY, SizeZ}, ExtrudedShape/{SizeZ, BasePoints_Size, BasePoints_<j>}}`. Normalized to `extra.rois[]` = `{id, label, shape, role, scope, position_index, color, keyframes[{time_ms, center, rotation_z, box_size, extrusion_z, base_points}]}` with shape names any, raster, point, rectangle, ellipse, polygon, bezier, line, polyline, circle, square, ring, spiral and roles any, standard, background, reference, stimulation. The layout was taken from the `nd2` package; `zenodo14231228-Sla2-WT-18-roi` (2026-09-26, found by a remote survey of 452 public Zenodo ND2 files, most of which hold empty ROI trees) has one global background rectangle whose id, shape, role, scope, colour, centre (0.2326, −0.6022) and box (0.1316 × 0.1123) equal the `nd2` package's. The geometry is reported as stored: those values are not pixels or micrometres of the 1000 × 1000 image (0.065 µm pixels), and their unit and origin are not established, so no conversion is made.

## Legacy container (NIS-Elements 2.x)

JPEG 2000 file format boxes (u32 big-endian length, four-character type, payload; length 1 = u64 extended length, 0 = to end of file). It starts with the JPEG 2000 signature box (`00 00 00 0C 6A 50 20 20 0D 0A 87 0A`), then `ftyp`, `jp2h` (whose `ihdr` gives height, width, components and bits per component − 1), then one `jp2c` box per picture plane per frame, frame-major, then `xml ` metadata boxes and a `uuid` box.

The last 40 bytes are the ASCII signature `LABORATORY IMAGING ND BOX MAP 00` (`BOX_MAP_SIGNATURE`) and a u64 little-endian distance from the end of the file to the box map: u32 big-endian entry count, then 16-byte entries `box_type` (4 bytes), `tag` (4 bytes), u64 little-endian `box_offset`. Frame codestreams are `jp2c`/`LUNK` (the signature, `ftyp` and `jp2h` boxes are also tagged `LUNK`). XML boxes by tag and root element:

| tag | root element | content |
| --- | --- | --- |
| `ARTT` | `AdvancedImageAttributes` | `SignificantBits`, `VirtualComponents` |
| `ACAL` | `Calibration` | attributes `Calibration_11` … (µm/px) |
| `VCAL` | `CalibrationSeq` (one per frame) | `dCalibration`, `dAspect`, `sObjective` |
| `VIMD` | `MetadataSeq` (one per frame) | the per-frame picture metadata of modern files (`dTimeMSec`, `dTimeAbsolute`, `dXPos`, `sPicturePlanes/sPlane/aN`, …) |
| `AIM1` | `Metadata_V1.2` | the experiment tree under `vMetadata` |
| `AIMD` | `Metadata` | the experiment tree directly, or as `LoopNo00`, `LoopNo01`, … |
| `TINF` | `TextInfo` | `TextInfoItem` elements with `Text` and `Index` attributes |
| `IEVE` | `Events` | events (older form) |
| `SROB` | `ReportObjects` | not used |

Variant-wrapped boxes (`<X><variant><no_name>…`) decode with the version-2 variant rules (`legacy_xml_decode`); attribute-style boxes decode to their attributes as strings. The frame count is the number of `VCAL` boxes; frame `f`, channel `c` is codestream `f × planes + c`, where a 3-component codestream holds an RGB plane. Codestreams are raw J2K (`FF 4F FF 51`); the SIZ marker's size is checked against `ihdr` before decoding, and decoding uses the pure-Rust `dicom-toolkit-jpeg2000` crate at native bit depth (bit-exact against `imagecodecs`/OpenJPEG on all five corpus files). When the box map is missing the reader walks the boxes from the start and tags XML boxes by root element; `check` reports the missing map as `truncated`.

## Vocabulary (every public identifier in `openreadout-nd2` must appear here)

| identifier | meaning |
| --- | --- |
| `Nd2Reader`, `Nd2File`, `FORMAT_ID`, `CHUNK_MAGIC`, `CHUNK_MAGIC_BYTES`, `FILE_SIGNATURE`, `CHUNK_MAP_SIGNATURE`, `FILE_MAP_NAME`, `LEGACY_SIGNATURE` | entry points and constants |
| `ChunkHeader`, `chunk_magic`, `name_len`, `data_len`, `name`, `payload_offset` | chunk header |
| `ChunkMapEntry`, `chunk_offset`, `chunk_data_len`, `chunk_map_offset`, `rescue_scan`, `rescued` | chunk map |
| `LvValue`, `lv_decode`, `item_type`, `name_units`, `child_count`, `level_len` | LV decoder |
| `Attributes`, `width`, `height`, `row_bytes`, `components`, `bits_in_memory`, `bits_significant`, `frame_count`, `compression`, `pixel_kind`, `virtual_components` | `SLxImageAttributes` |
| `LoopKind` { `TimeLoop`, `XyPositionLoop`, `ZStackLoop`, `SpectralLoop`, `CustomLoop`, `NeTimeLoop`, `Other` }, `Loop`, `kind`, `count`, `declared_count`, `level`, `invalid_items`, `repeat_count`, `z_step_um`, `period_ms`, `periods`, `positions`, `axis`, `name` | experiment tree |
| `Position`, `pos_x`, `pos_y`, `pos_z`, `pos_name`, `pfs_offset` | XY-loop points |
| `Period`, `start_ms`, `duration_ms` | valid NE-time periods |
| `FrameLayout`, `t_count`, `p_count`, `z_count`, `frame_index`, `frame_timestamp_ms`, `loop_indices`, `coords`, `frames`, `layout_note` | frame addressing |
| `PlaneDesc`, `description`, `component_count`, `sample_index`, `color_abgr`, `modality_mask`, `modality`, `fluorophore`, `filters`, `emission_nm`, `excitation_nm`, `emission_band_nm`, `camera` | channel planes |
| `CameraSetting`, `detector`, `exposure_ms`, `binning`, `gain`, `em_gain`, `settings_text` | per-channel detector settings |
| `MODALITY_BITS`, `modality_flags`, `acquisition_mode`, `color_hex` | channel normalization |
| `FrameMeta`, `time_ms`, `start_jdn`, `stage_x`, `stage_y`, `stage_z`, `calibration_um`, `objective_name`, `objective_mag`, `objective_na`, `refractive_index` | first-frame metadata |
| `jdn_to_iso8601`, `text_datetime_to_iso8601` | Julian day → ISO-8601 UTC; unambiguous local-time text → ISO-8601 without offset |
| `CustomTag`, `tag_id`, `value_kind`, `size`, `unit`, `custom_tags_from_lv`, `tag_values`, `f64_values`, `record_field`, `stage_z_tag`, `base_record`, `legacy_record_fields` | per-frame records |
| `events_from_lv`, `rois_from_lv` | events and ROIs |
| `frame_row`, `deinterleave`, `frame_payload` | plane assembly |
| `variant_decode`, `legacy_xml_decode`, `unwrap_root`, `base64_decode`, `runtype` | XML variants |
| `LegacyFile`, `LegacyBox`, `BOX_MAP_SIGNATURE`, `box_type`, `tag`, `box_offset`, `boxes`, `box_map_offset`, `tagged`, `codestreams`, `read_box`, `xml`, `image_header`, `codestream_size` | legacy container |
| `Nd2Dataset`, `open`, `path`, `file_len`, `problems`, `read_chunk` | opened-file state and chunk access |
| `from_lv`, `from_raw`, `from_loops`, `loops_from_lv`, `planes_from_lv`, `order` | constructors that normalize decoded LV/variant trees; `order` is the counted-loop nesting (axis, count) list |
| `emission_nm`, `excitation_nm` | plane wavelengths when the picture-plane record carries them |
| `fuzz_chunk_map`, `fuzz_normalize` | byte-slice entry points for the cargo-fuzz targets (`fuzzing` feature only; not a stable API) |

### Performance / robustness merge (2026-09-23)

Frame and output geometry remain file-relative and size-checked; contiguous full-channel planes reuse the frame buffer and other layouts copy by row/chunk. The common plane guard is capped at 4 GiB.
