# High-content screening plates

The model shared by the `opera-harmony`, `imagexpress` and `cellvoyager` readers.

Validated on the corpus plates listed in the three format notes (Harmony V4 and V5 exports of an Operetta, a Phenix and a Sonata; ImageXpress plates with and without HTD; CellVoyager CV7000 and CV8000 measurements) against the plate indexes parsed with the Python standard library, tifffile on every plane file, and Bio-Formats 8.5.0 run as a black box (`oracle/hcs.py`). Crate `openreadout-hcs`. Per-format notes: [opera-harmony.md](opera-harmony.md), [imagexpress.md](imagexpress.md), [cellvoyager.md](cellvoyager.md). Provenance: `docs/provenance/{opera-harmony,imagexpress,cellvoyager}.md`. Every name below is ours.

A plate imager writes one TIFF per plane (well × field of view × channel × Z × time point) and an index file that says which plane each TIFF is. The three readers turn such a folder into **one dataset whose images are the fields of view**:

- images are ordered by well (row by row, then column) and, within a well, by field number: `images[i]` is one field;
- channels, Z planes and time points are the plate's: every field has the same `size_c`, `size_z`, `size_t`; `c` follows the vendor's channel number, `z` and `t` their recorded index;
- `info` reads the index, lists the plate folder once and opens at most one plane file's TIFF header (for the sample type and to confirm the image size); it never opens every plane file. A 384-well Phenix index (27,648 planes, 47 MB) takes about a third of a second.

## Shared vocabulary (crate `openreadout-hcs`, modules `model`, `dataset`, `planes`, `lib`)

| our name | meaning |
| --- | --- |
| `HcsPlate` | a parsed plate: plate facts, `channels`, `fields`, `size_z`, `size_t`, `size_x`, `size_y`, `pixel_type`, `pixel_source` (where the sample type came from), `pixel_size_um`, `z_step_um`, `time_increment_s`, `objective`, `instrument`, `operator`, `started_at`, `ended_at`, `method`, `description`, `notes`, `plate_extra`, `channel_extra`, `vendor`, `unindexed_files`, `sidecars`, `index_bytes`, `format_version`, `format_id`, `source`, `root` (the folder plane names are relative to), `id`, `name`, `plate_type`, `rows`, `columns`, `declared_wells` |
| `HcsField` | one field of view = one image: `row`, `column` (zero-based), `field` (the vendor's field number), `field_index` (zero-based position in the well), `position_um` (stage x, y: the field's offset from the well centre where the index records it), `acquired_at`, `planes` |
| `PlaneSlot` | where one (c, z, t) plane is: `File` (on disk), `Missing` (the index expects it, the file is not on disk), `NotRecorded` (the index lists the plane without a file: the instrument recorded no image), `NotAcquired` (no record at all: a channel imaged at fewer Z planes or time points than the others). `state` spells them `present`, `missing`, `not_recorded`, `not_acquired`; `file_name` gives the name when known |
| `PlaneFile` | a plane on disk: `name` (relative path), `page`, `acquired_at`, `position_um` (x, y, z), `exposure_ms` |
| `RawPlane` | one index record before sorting: `row`, `column`, `field`, `channel`, `z`, `t`, `slot`, `field_position_um` |
| `PlateBuilder` | sorts records into fields and slots (`new`, `push`, `len`, `is_empty`, `finish`) |
| `MAX_PLATE_PLANES`, `MAX_PLATE_ROWS`, `MAX_PLATE_COLUMNS` | limits against hostile indexes (20 million planes, 256 x 256 wells) |
| `planes_per_field`, `slot`, `plane_of`, `path_of` | slot index of (c, z, t) = `c + C·(z + Z·t)`; its inverse; absolute path of a plane file |
| `earlier`, `mark_missing`, `list_names`, `number`, `to_micrometres` | helpers: timestamp order, turn `File` slots whose file is not in the folder listing into `Missing`, one folder listing, numbers with decimal commas, unit conversion |
| `PageShape` (`width`, `height`, `pixel_type`, `samples_per_pixel`, `pages`), `page_shape`, `check_file`, `read_file_plane`, `open_first_page`, `blank_plane` | plane-file access through the TIFF crate |
| `HcsDataset` (`new`, `plate_model`), `cached`, `finish` | the `Dataset`; parsed plates are cached per index (size and modification times) so parallel readers and the MCP server parse a 47 MB index once; `finish` lists the folder and takes the sample type from one plane file |
| `HarmonyReader`, `ImageXpressReader`, `CellVoyagerReader`, `FAMILY` | the three `FormatReader`s (`microscopy` family) |

## `info` (normalized model)

Each image (field of view):

| field | value |
| --- | --- |
| `name` | `<well> field <n>`, e.g. `C05 field 3` |
| `size_x`, `size_y`, `pixel_type` | from the index, confirmed by one plane file's TIFF header (the header wins; a note says so when they differ) |
| `size_c`, `size_z`, `size_t`, `channels` | the plate's channels (name, excitation/emission, exposure, mode as each format records them) |
| `physical_size` | x, y from the index; z = the Z step when `size_z` > 1 |
| `objective`, `instrument`, `acquired_at` | the plate's; `acquired_at` is the field's earliest plane time, else the measurement start |
| `extra.plate` | plate id / barcode |
| `extra.well`, `extra.row`, `extra.column`, `extra.row_index`, `extra.column_index` | `C05`, `C`, `5`, `2`, `4` |
| `extra.field`, `extra.field_index` | the vendor's field number and the zero-based position in the well |
| `extra.position_x_um`, `extra.position_y_um` | the field's stage offset (Harmony, CellVoyager) |
| `extra.absent_planes` | `[[c, z, t], ...]` never acquired or not recorded: they **read as blank** and are **left out of `stats` and `stats --per well`** |
| `extra.planes_missing`, `extra.missing_planes` | count and list (left out when the whole image is missing) of planes whose files are not on disk: reading them fails with exit 5 |
| `extra.planes_not_recorded` | count of index entries without a file |

`info` → `plate` (core `PlateSummary`, also produced for OME-Zarr plates from their images' `row`/`column`): `id`, `name`, `plate_type`, `rows`, `columns`, `wells[]` (`well`, `row`, `column`, `row_index`, `column_index`, `images` = the image indices of the well's fields, `planes_missing`), `field_count`, `planes_expected`, `planes_absent`, `planes_missing`, `complete`, `extra` (format facts, `channels` = per-channel vendor details, `wells_complete`, `selected_wells_without_images`, `unindexed_files`).

`experiment`: `sample.id` and `sample.barcode` = the plate id (the well of each image is `images[].extra.well`), instrument vendor/model/software, `method.name` (CellVoyager setting file), acquisition start, end, operator and duration, and a note that each image is one field of view.

## Planes that cannot be read

| state | cause | read | `stats` | `check` |
| --- | --- | --- | --- | --- |
| `not_acquired` | the acquisition protocol skipped it (e.g. CellVoyager fluorescence channels at 1 Z plane while brightfield takes 3) | blank (zeros) | left out | info `planes_not_acquired` |
| `not_recorded` | the index lists the plane with an empty file name (Harmony `URL` empty: acquisition skipped or failed) | blank | left out | warning `planes_not_recorded` |
| `missing` | the index names the file, it is not on disk (a partial copy) | error, exit 5, message names the file | `stats`: error; `stats --per well`: skipped and counted | error `missing_plane_files` (per well) |

Bio-Formats reads `not_acquired` CellVoyager planes as a copy of the channel's acquired plane (the Z-stack of a 2D channel is its image three times) and `missing` planes as blank; we read the former as blank and the latter as an error, and keep both out of statistics, so a well mean is never diluted by planes that hold no data.

## `check` (headers only; `check --headers-only` is the same)

| code | severity | meaning |
| --- | --- | --- |
| `missing_plane_files` | error | plane files named by the index are not on disk; one summary finding plus one per well (first 24 wells) with example names |
| `planes_not_recorded` | warning | index entries without a file, listed per well and field |
| `planes_not_acquired` | info | (c, z, t) combinations the protocol did not acquire |
| `unindexed_plane_files` | info | plane-like files in the folder the index does not name |
| `plane_file_unreadable`, `plane_file_mismatch`, `plane_file_truncated` | error | a present plane file is not a readable TIFF, has another size or sample type than the plate, or its strips run past the end of the file |
| `index_note` | info | the reader's notes (duplicates, unknown record types, ...) |

## Exports and statistics

- `export --format ome-zarr` of a plate writes an **OME-NGFF 0.5 HCS plate**: root `plate` metadata (every row and column of the plate, the imaged wells, `field_count`, `name` = plate id), one group per row, one `well` group per imaged well listing its fields `0`, `1`, … (in field order), and a `multiscales` image per field. `--well C05` restricts it to wells; `--skip-incomplete` leaves out fields whose selected planes are missing (listed as `images_skipped`); without it a partial copy is refused (exit 6) with that hint; `--no-plate` writes a `bioformats2raw.layout` collection instead. Validated with `ome-zarr-models` (NGFF 0.5 HCS) and `ome_zarr` (see *Validation*).
- `export --format ome-tiff --per-image` writes one OME-TIFF per field (`<well>_f<field>.ome.tiff`) into a directory, with the same `--well` / `--skip-incomplete`.
- `stats --per well` (MCP `openreadout_stats` with `per: "well"`, Python `File.well_stats`) returns tidy rows keyed by well: `plate, well, row, column, [image, field,] c, channel, fields, planes, planes_missing, count, mean, std, min, max, p1, median, p99, zero_fraction, saturated_fraction` (`--per field` for one row per field, `--well` to restrict, `--csv` for CSV). Statistics merge exactly across fields (the same accumulators as `stats`).

## Validation

- **Readers** (`cargo test -p openreadout-corpus-tests --features corpus`, `CORPUS_ONLY=hcs-`): 13 corpus entries (9 plates, among them two Columbus exports with multi-page TIFF and `.flex` files; 4 plates also opened as folders) against `oracle/hcs.py`: the plate layout (wells imaged, fields, the image indices of every well, rows, columns, plate id), channel names, image size, sample type and pixel size equal to the index parsed with the Python standard library; every present plane bit-exact against tifffile (xxh3-128 of the samples; 100 distinct planes, 146 comparisons with the folder entries) and against Bio-Formats 8.5.0's reading of the same series (90/90 distinct planes equal; the ImageXpress folder without HTD has no Bio-Formats reading); fields without files report exit 5 on read; never-acquired and not-recorded planes read as blank and are listed in `extra.absent_planes`. Bio-Formats lists one more idr0034 series than the index records (a grid position without any record) and repeats a 2D channel's plane at every Z of a CellVoyager Z stack (we read blank there); its CellVoyager channel order follows the acquisition actions and is mapped by channel number. Per-format numbers: [opera-harmony.md](opera-harmony.md), [imagexpress.md](imagexpress.md), [cellvoyager.md](cellvoyager.md).
- **Synthetic plates** (`crates/openreadout-hcs/tests/synthetic.rs`): all three readers on plates with known pixels, partial copies, empty file names, a channel at fewer Z planes (statistics not diluted), per-well statistics merged across fields, and 300 damaged indexes (truncated, byte-flipped, spliced; clean errors, no panic). Fuzz targets `whole_hcs_harmony`, `whole_hcs_cellvoyager`, `whole_hcs_imagexpress`.
- **OME-NGFF plate export** (`oracle/omezarr_plate_validate.py`): exports of the idr0034 and CellVoyager plates (`--skip-incomplete`, and `--well A01`) validate as NGFF 0.5 HCS with `ome-zarr-models` 1.8.1 (plate, wells, fields), `ome_zarr` 0.19.2 reads them as a plate, and every exported plane equals tifffile's plane of the source file (40 and 36 planes; absent planes are zeros). `crates/openreadout-omezarr/tests/plate.rs` round-trips a synthetic plate through the OME-Zarr reader.
- **Per-well statistics**: the Hoechst mean of CellVoyager well A01 (fields 1–2) from `stats --per well` equals the same computation with tifffile (34.156535).
