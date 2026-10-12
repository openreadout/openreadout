# Hamamatsu VMS (`.vms`)

Older Hamamatsu NanoZoomer scanners save a slide as VMS: a short settings file next to a grid of large JPEG images, a low-resolution map, a macro photo and an optimisation file. OpenReadout returns the slide as one RGB image pyramid, with the macro, the map and the optimisation file as attachments. Derived from OpenSlide's public format page, one public slide, and comparison with OpenSlide run as a black box. Provenance: `docs/provenance/hamamatsu-vms.md`. Reader: crate `openreadout-wsi`, module `vms`, format id `hamamatsu-vms`. (Hamamatsu NDPI files are TIFF-like and read by the TIFF reader: `docs/formats/tiff.md`.)

## Settings file

An INI file (read with the MIRAX settings parser: `Ini`) whose first line is `[Virtual Microscope Specimen]` (`VMS_SECTION`; `section_of` checks it). Keys used:

| key | meaning |
| --- | --- |
| `NoLayers` | focal layers; only 1 is read (others exit 6) |
| `NoJpegColumns`, `NoJpegRows` | the image grid (`grid`) |
| `ImageFile`, `ImageFile(x,y)` (`ImageFile(z,x,y)` with layers) | the image of column x, row y (`parse_cell`); `ImageFile` is (0,0) |
| `MapFile` | the map JPEG |
| `OptimisationFile` | MCU-row offsets |
| `MacroImage` | the macro photo (attachment `macro`) |
| `SourceLens` | objective magnification |
| `PhysicalWidth`, `PhysicalHeight` | slide size in nanometres: micrometres per pixel = value / 1000 / width |
| `XOffsetFromSlideCentre`, `YOffsetFromSlideCentre`, `PhysicalMacroWidth`, `PhysicalMacroHeight` | nanometres (in `extra.vms`) |
| `Reference` | image name |
| `AuthCode`, `LayerSpacing` | unknown; in `vendor.vms` only |

`[Uncompressed Virtual Microscope Specimen]` (`VMU_SECTION`: VMU files with NGR images) is detected and refused (exit 6): no public sample. `info --view full` → `vendor.vms` holds every key.

## Images

Every slide image is a baseline 8-bit JPEG with three components, no chroma subsampling and a restart interval (`DRI`) of 512 MCUs in the corpus file. Columns share a width and rows a height; image (x, y) starts at the sum of the widths of columns before x and the heights of rows before y (`VmsImage` { `path`, `len`, `layout`, `cell`, `origin` }). The JPEG header (`JpegLayout` { `header`, `sof_at`, `width`, `height`, `mcu_w`, `mcu_h`, `restart`, `scan_start` }, `parse`) is kept without its `DRI`: each restart interval (`interval_width` × MCU height pixels, `intervals_per_row` per MCU row, `rows` rows, `interval_size` at the right and bottom edges) decodes as a small JPEG of the header with the frame size set to the interval's, its entropy-coded bytes and an end marker (`frame`). `split_intervals` cuts an MCU row at its restart markers. Progressive, 12-bit, subsampled or interval-less slide images exit 6.

**MCU rows.** The optimisation file (`OPT_ENTRY` = 40 bytes per entry: a u64 offset, then zeros) lists where each MCU row starts, image after image in raster order of the grid, without the last row of the last image (`read_optimisation`, `opt_rows`). An offset is used when the first one equals the end of the SOS segment and they increase; the rows it lacks are found by scanning for restart markers (`row_index`: row r starts after marker r · intervals per row). Before decoding, a row must follow the restart marker its number requires, `(r · intervals − 1) mod 8`; otherwise the image is corrupt (exit 4).

**Map.** The map JPEG (`DRI` 64) is a reduction of the whole slide; it is a level when its size times a power of two (2 to 64) is the slide's size (`map_downsample`).

## Levels

`VmsSlide` { `path`, `dir`, `ini`, `grid`, `images`, `map`, `map_downsample`, `levels`, `size`, `opt_rows`, `problems` } (`open`, `settings`, `get`) lists the levels (`VmsLevel` { `source`, `shift`, `size`, `downsample` }; `LevelSource` { `Slide`, `Map` }) as OpenSlide does: the slide images at 1, 1/2, 1/4 … below the map's reduction, then the map at 1, 1/2, 1/4 and 1/8. Reduced sizes are decoded with a reduced inverse DCT (`jpeg_decode_reduced` in openreadout-codecs, jpeg-decoder's 4 × 4, 2 × 2 and 1 × 1 transforms), interval by interval; map levels up to 128 MB are decoded whole and kept. Only level 0 and the map at full size hold the stored samples; OpenSlide's reduced levels come from libjpeg's reduced transforms, which round differently (corpus file: up to 3 grey levels at 1/8, means within 0.05 at 1/2 and 1/4).

Region reads decode only the intervals a rectangle overlaps, in parallel, and keep them in a 256 MiB cache.

## Integrity checks (`check`)

`map_or_optimisation` (warning: the map is not a power-of-two reduction or is subsampled; optimisation offsets that do not fit), `missing_file` (warning: the macro, map or optimisation file is named but absent), `bad_rows` (MCU rows not found or not after their restart marker), `bad_interval` (the first or last interval of the first or last row of an image does not decode). `check --headers-only` reads the settings and the JPEG headers only.

## Vocabulary (every public identifier of the `vms` module must appear here)

| identifier | meaning |
| --- | --- |
| `VmsReader`, `VmsDataset`, `FORMAT_ID`, `VMS_SECTION`, `VMU_SECTION`, `section_of`, `open` | reader, detection and opened dataset |
| `VmsSlide` { `path`, `dir`, `ini`, `grid`, `images`, `map`, `map_downsample`, `levels`, `size`, `opt_rows`, `problems` }, `settings`, `get`, `row_index`, `read_optimisation`, `parse_cell`, `OPT_ENTRY` | the slide model |
| `VmsImage` { `path`, `len`, `layout`, `cell`, `origin` } | one JPEG file |
| `VmsLevel` { `source`, `shift`, `size`, `downsample` }, `LevelSource` { `Slide`, `Map` } | pyramid levels |
| `JpegLayout` { `header`, `sof_at`, `width`, `height`, `mcu_w`, `mcu_h`, `restart`, `scan_start` }, `parse`, `interval_width`, `intervals_per_row`, `rows`, `interval_size`, `frame`, `split_intervals` | restart-interval access to a JPEG |
| `vms`, `dataset`, `jpeg`, `slide` | modules |

`info` → `images[0].extra.vms`: `image_grid`, `image_files[]` (`file`, `cell`, `size`, `restart_interval_px`), `map_file`, `map_size`, `map_downsample`, `levels[]` (`source`, `dct_reduction`), `physical_width_nm`, `physical_height_nm`, `x_offset_from_slide_centre_nm`, `y_offset_from_slide_centre_nm`, `physical_macro_width_nm`, `physical_macro_height_nm`, `optimisation_file`; `images[0].extra.codec`.

## Oracle mapping (corpus harness)

`oracle/gen.py` (`vms`): OpenSlide's level sizes and mpp; whole levels up to 48 MB with their means (sidecars with `ORACLE_SIDECARS=1`; the manifest's `pixel_tolerance` 4 covers the 1/8 level). `oracle/gen_regions.py`: `openslide_targets` on every level, compared within the lossy tolerance.
