# EMD (Velox and Berkeley EMD)

EMD files are HDF5 files written by Thermo Fisher Velox and by open-source electron-microscopy tools. OpenReadout returns their images, spectra and spectrum images with the microscope metadata.

Derived from Velox corpus files inspected with h5py, which is also the reference reader. See `docs/provenance/emd.md`.

"EMD" names two HDF5 layouts with the same extension: Thermo Fisher **Velox** EMD (what Velox writes on Talos, Spectra, Titan and Krios microscopes) and the open NCEM/Berkeley **EMD** convention (0.2, written by ncempy, HyperSpy and Prismatic; 1.0, written by py4DSTEM's `emdfile`). Both are read; a file with neither Velox data nor EMD data groups exits 6 with a hint.

HDF5 is parsed by the pure-Rust `hdf5-pure` crate (MIT OR Apache-2.0) with a streaming (non-mmap) open, so the binary stays C-free.

## Detection

The `.emd` extension **and** the HDF5 signature (`HDF5_SIGNATURE`, `\x89HDF\r\n\x1a\n`) at byte 0 or a user-block boundary (512, 1024, …; `looks_like_hdf5`). Other HDF5 files are not claimed.

## Velox layout (observed)

| path | content | our use |
| --- | --- | --- |
| `Version` | variable-length string, JSON `{"version": "10", "format": "Velox"}` | `format_version` (`Velox 10`) |
| `Data/Image/<id>/Data` | (rows, columns, frames), uint16 (STEM) or int16 (camera) | one image per `<id>`: `size_y` = rows, `size_x` = columns, `size_t` = frames |
| `Data/Image/<id>/Metadata` | (60000, frames) uint8, one NUL-padded JSON document per frame | first frame's document → normalized fields below and `vendor` |
| `Data/Image/<id>/FrameLookupTable` | uint32 per frame | not interpreted |
| `Data/Spectrum/<id>/Data` | (channels, 1) uint32: an EDS spectrum of one detector segment (`SuperXG21` … `SuperXG24`) or their sum (`SuperXG2`) | one trace each (below) |
| `Data/SpectrumStream/<id>` | `AcquisitionSettings` JSON, `Data` (n, 1) uint16 X-ray event stream, `FrameLocationTable` (frames, 1) uint64 | spectrum images (below) |
| `Presentation/Displays/ImageDisplay/<id>` | JSON: `display.label` (URL-escaped) and `dataPath` `/Data/Image/<id>` | the image's `name` (`HAADF`, or the element of a quantified EDS map: `Au`, `C`, …) |
| `Data/EelsSpectrumImage/<id>` | `Data` (columns, channels, rows) uint16 STEM-EELS counts, chunked one scan line per chunk; `Metadata` (60000, 1) uint8 JSON (the image schema; `BinaryResult.Detector` `EELS Strip Detector`, no `PixelSize`); `AcquisitionMetadata` (bytes, rows) uint8, one JSON per scan line: `Data.offset` (eV of channel 0), `Data.dispersion` (eV per channel), `exposureTime` (s), `intensityScale`, `intensityOffset`; `Info` JSON (`bincount`, `width`, `height`) | one image per `<id>` after the EDS spectrum images: `size_x` = columns, `size_y` = rows, `size_c` = channels; plane c = `Data[:, c, :]` transposed to (rows, columns); `extra.energy_axis` (`first` = offset, `step` = dispersion, eV) when every scan line agrees, else `extra.energy_calibration_varies`; `extra.exposure_s`, `extra.intensity_scale`/`intensity_offset` (reported, not applied); pixel size from the images of the same raster and detector when they agree (the spectrum image records none). Axis order and energy axis validated on `zenodo18267156-STEM-EELS-1-DyScO3` (docs/provenance/emd.md, 2026-09-26) |
| `Data/SpectrumImage`, `Data/Line`, `Data/Text` | other Velox data (the `SpectrumImage` blob's layout is unknown) | listed in `info --view structure` and a note, not decoded |
| `Thumbnail.jpg` | JPEG bytes | attachment `#0` (`export --attachment #0` writes a `.jpg`) |

Frames are the innermost axis in storage, so reading frame `t` of a multi-frame image gathers every `frames`-th sample from windows of rows (`read_raw_rows`, about 64 MiB at a time); single-frame images are one read.

## Spectra and spectrum images (2026-09-25)

**Spectra** (`VeloxSpectrum`, `spectra`): each `Data/Spectrum/<id>` is a trace of `bins` samples, channel `counts`. Its energy axis (`traces[].extra.axis`, keV: `first`, `last`, `step`, `size`) is `OffsetEnergy + k × Dispersion` eV from the `Detectors` entry whose `DetectorName` is the spectrum's `BinaryResult.Detector`; a sum spectrum (no entry of its own) takes the calibration its segments share (`offset_ev`, `dispersion_ev`, `energy_kev`). Checked against X-ray lines: C Kα, O Kα, Si Kα, Au Mα/Lα/Lβ/Lγ, Mo Lα, K Kα/Kβ, Cu Kα and the zero peak all fall within half a channel of their energies (`docs/provenance/emd.md`).

**Event streams** (`VeloxStream`, `streams`): `AcquisitionSettings` gives `bincount`, `StreamEncoding` (`uint16`; other encodings are not read) and, for spectrum images, `RasterScanDefinition` `Width`/`Height`. The stream is uint16 values: `PIXEL_END` (65535) ends a pixel, any other value is the energy channel of one X-ray detected in that pixel; pixels follow in raster order (x fastest), frame after frame (`FrameLocationTable` holds each frame's first value). A stream must end exactly width × height × frames pixels and hold no channel ≥ `bincount` (`decode_stream`), else reading and `check` report it corrupt. Each stream's event histogram (`stream_histogram`) equals one stored detector spectrum in every corpus file (Velox's own sums); `check` warns when none matches (`stream_spectrum_mismatch`).

**Spectrum image**: the streams with one raster make one extra image after the Velox images, `EDS spectrum image`: `size_x` × `size_y` = the raster, `size_c` = `bincount` (one channel per energy channel, named `1.234 keV`; `extra.energy_axis`), uint32 counts summed over detector segments and frames (`extra.detectors_summed`, `extra.frames_summed`), pixel size from the spectra's `BinaryResult.PixelSize`. The first plane read decodes every stream into an `EventTable` (events grouped by channel: `offsets`, `pixels`, `per_stream`; at most `MAX_EVENTS`), kept for later planes. Validated: the per-pixel event map correlates with the file's HAADF image at the pixel placement above (r = 0.96).

## Metadata used

| JSON path | our field |
| --- | --- |
| `BinaryResult.Detector` | image `name` and channel name (unless a display labels the image), `instrument.detector` |
| `BinaryResult.PixelSize.width` / `height` with `PixelUnitX` / `PixelUnitY` (`m`) | `physical_size` (µm) |
| `Instrument.Manufacturer`, `Instrument.InstrumentModel` (else `InstrumentClass`), `Instrument.ControlSoftwareVersion` | `instrument` (software `Velox`) |
| `Acquisition.AcquisitionStartDatetime.DateTime` (Unix seconds as text) | `acquired_at` |
| `Optics.AccelerationVoltage` (V), `Optics.NominalMagnification`, `Optics.CameraLength` (m) | `extra.voltage_kv`, `extra.magnification`, `extra.camera_length_m` |

Other `extra` keys: `velox_id`, `unsupported_sample_type`. `vendor` (`info --view full`): `Version`, the first-frame metadata document of every image keyed by id, and the other data groups.

## `check` finding codes

| code | severity | meaning |
| --- | --- | --- |
| `truncated` | error | the file is shorter than the superblock's end-of-file address, or the last row of an image cannot be read (exit 4) |
| `metadata` | warning | an image's `Metadata` is missing or not JSON |
| `unsupported_sample_type` | info | the image's sample type is not decoded |
| `trailing_bytes` | info | bytes after the end-of-file address |

A file whose HDF5 structure cannot be parsed fails `open` with exit 4; so does a truncated file whose image group is lost.

## Observed corpus values

| file | image | chunks |
| --- | --- | --- |
| `zenodo20040988-0050-STEM-15.4nm` | 512 × 512 × 1 uint16 HAADF, 0.030 nm | one chunk |
| `zenodo20131420-0030-STEM-Nano-2.79um` | 1024 × 1024 × 1 uint16 HAADF, 2.73 nm | one chunk |
| `zenodo19965490-graphene-2` | 2048 × 2048 × 1 uint16 HAADF, 7.5 pm | 512-row bands |
| `zenodo20131420-0028-Camera-Micro` | 4096 × 4096 × 1 int16 BM-Ceta, 0.41 nm | 256-row bands |

| `zenodo20131712-0017-SI-Nano-1.4um` | 512 × 512 × 50 uint16 HAADF (multi-frame), 8 float32 quantified element maps, the EDS spectrum image (4 streams) and 4 spectra | one frame per chunk |
| `zenodo20131712-0002-Spectrum-Micro`, `zenodo21895889-0043-Spectrum-EDS` | point spectra: 4 segments and their sum, 10 and 5 eV per channel | — |
| `zenodo21895889-0041-STEM-HAADF-BF` | two 2048 × 2048 uint16 images (HAADF, BF-S), Velox 11 | 512-row bands |

Per-frame metadata columns are also covered by a synthetic file written with hdf5-pure (`crates/openreadout-em/tests/synthetic_emd.rs`).

## Berkeley EMD

Opened when the Velox reader finds no Velox data (`looks_like_berkeley`: the root, or a group below it, has `version_major` or `emd_group_type`). Every group whose `emd_group_type` attribute is 1 (EMD 0.2) or `"array"` (EMD 1.0) is a data group (`find_arrays`, depth ≤ 16, at most 20 000 groups visited; a data group's children are not searched): its array is the `data` dataset, else the one dataset that is not a `dimK` vector (older py4DSTEM/Prismatic files). `EmdArray`: `group`, `dataset`, `shape` (slowest first), `pixel_type` (integers, floats, and a two-float compound as `complex`/`double-complex`), `big_endian`, `dims` (`EmdDim`: `name`, `units` without brackets, `first`, `step` = v[1] − v[0] for full or start/step vectors, `length`), `attrs` (the group's scalar attributes). One image per data group, in path order (`arrays`):

- X = the last dimension, Y = the one before, leading dimensions flattened into T (fastest last); Z instead of T for a 3-D array whose first calibration is a length (`length_to_um`: m, mm, µm, nm, `n_m`, Å, pm); `extra.frame_grid` lists the leading sizes.
- A 3-D array of two lengths and a non-length (Prismatic's positions × positions × detector bins; `last_is_channels`): channels = the bins, X = dimension 1, Y = dimension 0.
- Physical sizes from the X/Y (/Z) steps in length units, when a step is below 1 mm per pixel: `ncem-emd-pt-saed-d910mm-single` (a TIA diffraction pattern converted by openNCEM) labels its axes `m` but steps 8.9e6 per pixel, which are reciprocal metres; such a step is not a pixel size, a note names the dimensions and the calibration stays in `extra.dims`. `attrs`, `microscope` and the other metadata groups are sorted by name (HDF5 attributes arrive unordered). `extra`: `emd_group`, `emd_dataset`, `shape`, `dims`, `group_attributes`, `microscope` (the root-level `microscope` group's attributes; its `Microscope`/`name` is `instrument.model`, `AcceleratingVoltage`/`voltage` in V is `voltage_kv`). `sample`, `user` and `comments` groups are in `info --view full`. `format_version` is `Berkeley EMD <major>.<minor>`.
- Planes are read one leading-dimension row at a time (`read_raw_rows`), so a stack or a 4D-STEM cube is never read whole.

## Velox complex images

A `Data/Image/<id>/Data` array of a compound of two float32 (float64) members is complex (`complex`, `double-complex`). Velox stores Fourier transforms this way as the non-redundant half plane (member names `realFloatHalfEven`/`realFloatHalfOdd`, `fft_half_plane`): returned as stored with `extra.packed_half_plane` and `extra.half_plane_member`, not mirrored into the full plane.

## Vocabulary (every public identifier in `openreadout-em/src/emd` must appear here)

| identifier | meaning |
| --- | --- |
| `EmdReader`, `EmdDataset`, `FORMAT_ID`, `open`, `images` | format reader, opened file (core `Dataset`), the id `emd`, open, the Velox images |
| `VeloxImage`, `id`, `rows`, `columns`, `frames`, `pixel_type`, `big_endian`, `metadata`, `display_label`, `fft_half_plane` | one `Data/Image/<id>` entry |
| `BerkeleyDataset`, `arrays`, `EmdArray`, `group`, `dataset`, `shape`, `dims`, `attrs`, `last_is_channels`, `EmdDim`, `name`, `units`, `first`, `step`, `length`, `length_to_um` | Berkeley EMD files and their data groups |
| `VeloxSpectrum`, `detector`, `bins`, `offset_ev`, `dispersion_ev`, `energy_kev`, `spectra` | one `Data/Spectrum/<id>` entry |
| `VeloxStream`, `raster`, `values`, `streams`, `PIXEL_END`, `MAX_EVENTS`, `decode_stream`, `stream_histogram` | one `Data/SpectrumStream/<id>` entry and its decoding |
| `EelsSpectrumImage`, `exposure_s`, `intensity_scale`, `intensity_offset`, `calibration_varies`, `pixel_um`, `eels_spectrum_images` | one `Data/EelsSpectrumImage/<id>` entry (STEM-EELS spectrum image) |
| `EventTable`, `offsets`, `pixels`, `per_stream`, `event_table` | decoded events of a spectrum image, grouped by channel |
| `HDF5_SIGNATURE`, `looks_like_hdf5` | HDF5 detection |
