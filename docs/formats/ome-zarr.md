# OME-Zarr (OME-NGFF)

OME-Zarr is the chunked image format of the open OME-NGFF specification. OpenReadout reads OME-Zarr stores (Zarr v2 and v3, NGFF 0.1–0.5) with every pyramid level, physical sizes and channel metadata, and `export --to ome-zarr` writes them. Everything here comes from the published specifications (https://ngff.openmicroscopy.org, https://zarr-specs.readthedocs.io), not from reverse engineering. It was checked on public Fractal stores and synthetic stores written by zarr-python and ome-zarr-py, with zarr-python (`oracle/gen.py`) as the reference reader. Provenance: `docs/provenance/ome-zarr.md`.

Format id `ome-zarr`, crate `openreadout-zarr`.

## Stores

| store | how it is opened |
| --- | --- |
| directory (`*.zarr`, `*.ome.zarr`) | the directory; also a path to its root `zarr.json` / `.zattrs` / `.zgroup` |
| zip, hierarchy at the archive root (NGFF RFC-9, what `make_zarr_fixtures.py` writes) | read in place through our own zip index (`ZipIndex`: end-of-central-directory, ZIP64, stored and deflated members) |
| zip holding one top-level `.zarr` folder (Zenodo uploads) | the folder becomes the store `prefix` |

Stored members are read by byte range (sharded arrays read only the shard index and the inner chunks they need); deflated members are inflated whole. Encrypted members and other zip methods exit 6.

## Detection

- a directory whose `zarr.json`, `.zattrs` or `.zgroup` mentions `multiscales`, `plate`, `well` or `bioformats2raw.layout` → definite; a Zarr directory without those keys → likely (opening it exits 6);
- a `.zip` starting with `PK\3\4` whose first 64 KiB name `zarr.json`, `.zgroup`, `.zattrs` or `.zarray` → definite (plate-reader `.xlsx` files never contain these names);
- a root metadata document itself (`zarr.json`, `.zattrs`, `.zgroup`) that mentions an NGFF key → definite.

## Group metadata

`GroupAttrs`: Zarr v3 groups keep NGFF metadata in `zarr.json` → `attributes.ome` (NGFF 0.5, with `version` there); Zarr v2 groups in `.zattrs` at the top level (NGFF ≤ 0.4, `version` inside `multiscales[0]`, `plate`, `well`). `version_of` reads either.

What becomes images (`info.images[]`, in this order):

| root metadata | images |
| --- | --- |
| `multiscales` | each `multiscales` entry of the root group (normally one) |
| `plate` | for each `plate.wells[]` (in order), each `well.images[]` field of that well group; name `<well path>/<field path>` (e.g. `C/02/0`); `extra.well`, `extra.field`, `extra.row`, `extra.column`, `extra.acquisition` |
| `bioformats2raw.layout` (value 3) | the groups listed in `OME` → `series`, else `0`, `1`, … while present; `extra.series`. `OME/METADATA.ome.xml` is parsed with the TIFF crate's OME-XML parser and series `N` takes image name, channel names/fluorophores/wavelengths/colours, acquisition modes, detection bands (emission filters) and exposures (first `Plane/@ExposureTime` of the channel), objective, instrument, acquisition date, our export's `openreadout.dev/normalized` annotations (the same helpers as the OME-TIFF reader, `docs/formats/tiff.md`) and (when the scale gives none) physical sizes and time increment from OME `Image` N |
| none of these | exit 6 (plain Zarr group); a bare array at the root exits 6 too |

`labels` groups (`<group>/labels` → `labels: [names]`, a name listed twice counts once) are listed in `info --view structure` (kind `label`) and in `extra.labels` of their image, and every label multiscales is **exposed as an image** after all other images (`add_label_images`): named `<group>/labels/<name>`, `extra.label` = true, `extra.label_of` = the index of the image it annotates (root labels of a single root image annotate image 0; root labels of a plate or collection have none), `extra.image_label` = the NGFF `image-label` block (colours, properties) as stored. Its samples are the stored label ids. `export` to OME-TIFF leaves label images out unless one is chosen with `--image`.

## Multiscales → the normalized image

| NGFF | our field |
| --- | --- |
| `axes` (0.4+: objects with `name`, `type`, `unit`; 0.3: names; 0.1/0.2: none = `t, c, z, y, x`) | `AxisRole` per array dimension: by name `x`/`y`/`z`/`c`/`t`, else by `type` (`channel`, `time`, the last `space` axes as x, y, z); other axes are read at index 0 and listed in `extra.unmapped_axes` |
| `datasets[i].path` | pyramid level `i` (`pyramid_levels`, `check --planes --level i`); level 0 gives `size_x/y/z/c/t` |
| `datasets[0].coordinateTransformations` `scale` × multiscales-level `scale`, with the axis `unit` | `physical_size` (µm; `length_um` converts UDUNITS names such as `nanometer`, `millimeter`) and `time_increment_s` (`time_s`); no unit → no physical size (the raw scale stays in `extra.scale`) |
| `translation` | `extra.translation` (not applied) |
| `name` | image `name` (plate fields are named by well and field) |
| `type` (downsampling method) | `extra.downsampling` |
| `omero.channels[c]` `label`, `color` (`RRGGBB`), `window` | channel `name`, `color` (`#RRGGBB`, `color`), `extra.channel_windows` |
| array `dtype` / `data_type` | `pixel_type`: `u1 u2 u4 u8 i1 i2 i4 i8 f4 f8 c8 c16` and their v3 names as stored; `f2`/`float16` widened exactly to `float`; `b1`/`bool` as `uint8` 0/1; others (strings, structured, datetime) → `extra.unsupported_dtype`, plane reads exit 6 |
| dataset and multiscales `translation` | composed with the scales (coordinate = multiscales scale × (dataset scale × index + dataset translation) + multiscales translation): `extra.translation` per axis, `extra.origin_um` (x, y, z of the first sample in µm, for axes with length units) |

`dimension_order` is `XY` followed by the other axes from fastest to slowest (TCZYX arrays → `XYZCT`). Planes are one `(c, z, t)` index across the full `y × x` extent, little-endian; an array that stores `x` before `y` is transposed to rows of `x`. `format_version` reads `<NGFF version> (NGFF, Zarr v<2|3>, <directory|zip> store)`.

## Chunk codecs

Arrays are read with the pure-Rust `zarrs` crate (features `filesystem`, `gzip`, `zlib`, `sharding`, `crc32c`, `transpose`). `zarrs` implements `blosc` and `zstd` with C libraries, so `codecs::register` installs decode-only replacements (`DecodeOnly`, `ChunkCompressor`) backed by `openreadout-codecs`:

| codec (v2 `compressor.id` / v3 `name`) | decoder |
| --- | --- |
| `blosc` (v2 and v3) | our Blosc 1 decoder: blosclz, lz4, zlib and zstd inside, byte shuffle; bit-shuffle and snappy are not decoded |
| `zstd` | `ruzstd` |
| `lz4` (numcodecs, v2) | 4-byte size + LZ4 block (`lz4_flex`) |
| `gzip`, `zlib`, `crc32c`, `sharding_indexed`, `bytes`, `transpose` | `zarrs` |

No codec needs a C toolchain. Writing with the replacements is refused (the OME-Zarr writer uses `gzip`).

## `check` finding codes

| code | severity | meaning |
| --- | --- | --- |
| `array_metadata` | error | an array listed in `datasets` cannot be opened, or its shape disagrees |
| `chunk_decode` | error | the first or last stored chunk of an array does not decode |
| `pyramid` | error / warning | levels of different rank / a level larger than the previous one |
| `omero_channels` | warning | `omero.channels` count differs from the `c` axis |
| `metadata` | warning | a well, image group or `OME/METADATA.ome.xml` listed but missing or unreadable |
| `missing_chunks` | info | chunks not stored (read as the fill value, which Zarr allows) |
| `unsupported_dtype` | info | the level-0 data type is not read |

Chunk presence is counted for up to 100 000 chunks per array (evenly sampled beyond).

## Export round trip

`export --to ome-zarr` (crate `openreadout-omezarr`, NGFF 0.5 / Zarr v3 / gzip) reads back through this reader hash-identical for every plane. Normalized metadata survives only in multi-image stores, which carry `OME/METADATA.ome.xml` (a CZI, LIF or ND2 comes back equal except `dimension_order`); a single image written at the root has only the NGFF `omero` block, so its channels keep names and colours but lose wavelengths, bands and modes, and objective, instrument and acquisition time are not stored (known gap) (`crates/openreadout-omezarr/tests/roundtrip.rs`); interleaved RGB images come back as three channels (the writer stores each sample as a channel), each equal to its deinterleaved sample plane.

## Observed corpus values

| id | layout | images |
| --- | --- | --- |
| `zenodo13982701-organoid-mip` | NGFF 0.4 plate, 1 well, 2 fields (acquisitions), zip with a top-level folder | 535 × 680 uint16, 4 levels, Blosc lz4 + shuffle, labels and AnnData tables |
| `zenodo13982701-organoid` | same, z stacks | 535 × 680 × 50 |
| `zenodo13305156-cardio-mip` / `-cardio` | NGFF 0.4 plate, 1 field | 5120 × 2160 (× 2), 4 levels |
| fixtures (`crates/openreadout-zarr/tests/fixtures`, synthetic) | NGFF 0.4 TCZYX + labels, 0.4 ZYX float with blosclz/zstd/zlib levels and `.` separators, 0.5 YX (zstd, stored zip), 0.5 CZYX sharded (blosc zstd inside shards, crc32c index) + gzip level, 0.4 plate, bioformats2raw collection with OME-XML | |

## Vocabulary (every public identifier in `openreadout-zarr/src` must appear here)

| identifier | meaning |
| --- | --- |
| `ZarrReader`, `ZarrDataset`, `FORMAT_ID`, `open`, `images`, `info` | format reader, opened store (core `Dataset`), the id `ome-zarr`, open, the images, an image's normalized info |
| `ZarrImage`, `multiscale`, `roles`, `levels`, `omero`, `labels` | one image: its multiscales entry, axis roles, level arrays, `omero` block, label names |
| `AxisRole`, `axis_roles` | t/c/z/y/x role of an array dimension (`Other` for the rest) |
| `ArrayMeta`, `array_meta`, `path`, `shape`, `chunks`, `dtype`, `pixel_type`, `codecs`, `format` | array metadata read from `.zarray` / `zarr.json` |
| `ZarrFormat`, `number` | Zarr version 2 or 3 |
| `GroupAttrs`, `group_attrs`, `ngff`, `raw` | a group's NGFF attributes and the whole attribute document |
| `Multiscale`, `multiscales`, `group`, `name`, `version`, `axes`, `implied_axes`, `method` | one `multiscales` entry |
| `Axis`, `kind`, `unit` | one axis (`type` is `kind`) |
| `Level`, `scale`, `translation` | one `datasets[]` entry with its combined scale |
| `length_um`, `time_s`, `color`, `version_of` | unit conversion, `#RRGGBB` colour, NGFF version lookup |
| `Store`, `StoreKind`, `storage`, `prefix`, `get`, `json`, `exists`, `size`, `zip_summary`, `join` | store access (directory or zip) |
| `ZipStorage` | `zarrs` storage over a zip archive |
| `ZipIndex`, `ZipMember`, `members`, `file_len`, `method`, `compressed_size`, `local_offset`, `encrypted`, `read`, `read_stored_range` | the zip central directory and member reads |
| `DecodeOnly`, `ChunkCompressor`, `compressor`, `register` | decode-only Blosc / zstd / LZ4 codecs registered with `zarrs` |
