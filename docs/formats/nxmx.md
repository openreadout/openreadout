# NeXus NXmx diffraction data (`nxmx`)

Hybrid-pixel detectors (DECTRIS EIGER, PSI JUNGFRAU) save a rotation sweep as HDF5: a master file (`<prefix>_master.h5`, or a beamline's `<prefix>.nxs`) with the experiment in the NeXus NXmx application definition, and data files (`<prefix>_data_000001.h5` or `<prefix>_000001.h5`) with the images, which the master reaches through external links `/entry/data/data_NNNNNN`. Derived from the NeXus NXmx definition (https://manual.nexusformat.org/classes/applications/NXmx.html, public), the bitshuffle library's documentation (MIT) and h5py/hdf5plugin as the oracle. Provenance: `docs/provenance/nxmx.md`. Crate: `openreadout-hdf5` (`src/nxmx.rs`).

## Detection

An HDF5 file named `*_master.h5` is NXmx (definite). A `.nxs` file is NXmx when `/entry/definition` is `NXmx` (or `/entry` has `instrument/detector` and `data`). A data file (`*_NNNNNN.h5`) is claimed when its master is next to it, and read through the master.

## Images

`hdf5-pure` does not resolve external links, so the reader finds them in the master's bytes: an HDF5 link message holds the link name `data_NNNNNN`, a two-byte length, a flags byte, then the file name and the object path, each NUL-terminated (`external_data_links`). Each data file's dataset (`/data` or `/entry/data/data`, shape images × rows × columns) adds its images as time points, in link order; `image_nr_low`/`image_nr_high` on the dataset are checked against the position. A virtual dataset (`/entry/data/data` in DLS `.nxs` files) is not read; its source files are.

Chunks are decoded by the shared HDF5 plane reader (`h5util`), which now handles the bitshuffle filter 32008: the client data give the element size, the block size and the compression (0 none, 2 LZ4); an LZ4 chunk is a big-endian 8-byte decoded size and 4-byte block size, then per block a 4-byte compressed size and an LZ4 block of bit-shuffled elements; the elements left after the last whole block of 8 are stored as they are (`openreadout_codecs::bitshuffle_decode`).

A master without its data files opens with the image size from `detectorSpecific` (`x_pixels_in_detector`, `y_pixels_in_detector`, `nimages` × `ntrigger`, `bit_depth_image`); reading pixels is then refused (exit 6) with a hint.

## Geometry

| NXmx field | `extra.diffraction` |
| --- | --- |
| `/entry/instrument/beam/incident_wavelength` (or `/entry/sample/beam/…`, `detectorSpecific/wavelength`) | `wavelength_a` (from `units`) |
| `detector/detector_distance` | `distance_mm` (metres unless `units` says mm) |
| `detector/beam_center_x`, `beam_center_y` | `beam_center_px` |
| `detector/x_pixel_size`, `y_pixel_size` | `pixel_size_mm` |
| `detector/count_time` | `exposure_s` |
| `detector/saturation_value` (or `detectorSpecific/countrate_correction_count_cutoff`) | `overload`, when below the type's maximum |
| the type's maximum | `masked_values`: pixels in the detector's mask are written as 2^n − 1 |
| the goniometer axis whose values change (`/entry/sample/transformations/*`, `/entry/sample/goniometer/*`) | `scan_axis`, `oscillation_start_deg`, `oscillation_width_deg` (from `<axis>_increment_set`, `<axis>_range_average`, `<axis>_end` or the step) |
| `/entry/instrument/source/probe` | `radiation` (else by wavelength) |

`extra.nxmx` holds `detector_serial`, `sensor_material`, `sensor_thickness_mm`, `threshold_ev`, `frame_time_s`, `bit_depth`, `beamline` (`/entry/instrument/name` or its `short_name`), `source`, `eiger_firmware`, `transmission`, `flux`, `pixel_mask_applied`, `flatfield_applied`, `countrate_correction_applied`, `pixel_mask` (its shape) and `data_files` (link, file, object, images, filters). Frame records (`frames()`) give each image's angle (`<axis>_deg`), data file and index in it. `acquired_at` is `/entry/start_time`; the experiment's `ended_at` is `/entry/end_time`.

## `check`

| code | severity | meaning |
| --- | --- | --- |
| `missing_file` | error | a linked data file is not next to the master |
| `image_numbers` | warning | a data file's `image_nr_low`/`high` disagree with its position |
| `missing_geometry` | warning | no wavelength, distance, pixel size or beam centre |
| `bad_image` | error | the first or last image of a data file does not decode |

## Vocabulary

| identifier | meaning |
| --- | --- |
| `NxmxReader`, `NxmxDataset`, `NXMX_FORMAT_ID`, `open` | reader, opened master, format id `nxmx` |
| `external_data_links` | the `data_NNNNNN` external links found in the master's bytes |
| `FILTER_BITSHUFFLE` | the bitshuffle filter id 32008 (h5util) |
