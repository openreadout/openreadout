# Imaging deep pass, October 2026

The imaging bug hunt (`hunt-2026-10-imaging.md`) checked one plane and the first image of each
new file. This pass checks every plane of every image of the development image corpus, exports
every image to OME-TIFF and OME-Zarr, and reads each export back with OpenReadout and with an
independent reader. This page lists what we ran, what we found, what we fixed and what is left.

## What we ran

Every development input of the image formats (microscopy, whole-slide, EM, screening plates,
OME-Zarr) that was on disk: 677 files. Held-out files were left out (the `heldout` and `hold`
tiers), as were the two inputs over 3 GB (`zenodo10577621-Young-mouse`, 3.7 GB, and
`ome-imagesc-110520-AMR1`, 3.0 GB).

| format | files | format | files |
| --- | --- | --- | --- |
| TIFF family | 148 | MRC | 23 |
| CZI | 120 | SER / EMI | 22 |
| DM3 / DM4 | 114 | IMS | 16 |
| ND2 | 42 | OIR | 16 |
| EMD | 37 | ZVI | 16 |
| LIF | 32 | DCIMG | 15 |
| VSI | 29 | Opera / Operetta | 11 |
| OIB / OIF | 15 | OME-Zarr | 8 |
| MIRAX | 7 | CellVoyager, ImageXpress | 6 |

For each file the script:

1. ran `info`, then `planes` to decode and hash every plane of every image (xxh3-128), and
   compared the hashes with the corpus oracle where it has them;
2. ran `export --format ome-tiff` and `export --format ome-zarr` on the whole file;
3. read each export back with OpenReadout (`info`, `planes`) and with an independent reader:
   tifffile 2026.9.20 for OME-TIFF (OME-XML parsed with the Python standard library, pixels
   through `series.aszarr()`), and zarr-python 3.4.0 for OME-Zarr (with ome-zarr-py opening the
   store as a second check). Every plane hash, the sizes, dtype, physical pixel sizes, channel
   names and channel colours were compared with the source;
4. recorded wall time and peak RSS of every command (`/usr/bin/time -l`).

Two processes ran at a time on an 8-core, 16 GB machine that other jobs were also using, so the
wall times are rough. The first 330 files ran on the `main` build, the rest on a build with the
fixes below. Every file with a finding was then run again on the final build.

## Totals

- 1,728 images and 66,225 planes decoded. No crash, no hang, no panic. `planes` refused the
  level 0 of 16 slides above the 4 GiB plane limit (with a hint to read a region or a level) and
  the photon data of one FLIM LIF file, as documented.
- Oracle: 12,747 planes from 611 files agree with their oracle hash. 124 planes differ on files
  the manifest marks `lossy` (JPEG slides, decoded by another library), and 64 on
  `ome-jonas-nd2Test-Exception9-e3`, where the manifest records that the `nd2` package reads the
  file one byte off. No other plane differs.
- OME-TIFF: 614 files exported. OpenReadout and tifffile both read back all 65,719 planes of
  those files hash-identical to the source.
- OME-Zarr: 620 files exported. zarr-python reads back all 66,174 planes hash-identical, and
  OpenReadout all 61,931 it was asked to compare (RGB images were left out of that comparison,
  because OME-Zarr stores their samples as channels). Before the fixes, OpenReadout's read-back
  of 7 exports returned no planes (finding D1).
- Metadata: no difference in size, dtype, physical pixel size, channel name or colour between
  the source and either reader's view of either export, after the conventions listed under
  "Intended differences".

## Findings

| # | severity | what happened | files | status |
| --- | --- | --- | --- | --- |
| D1 | S2 | An OME-Zarr export whose last image is blank (an empty label image, an all-zero frame or channel) has no chunk files for it, because `zarrs` leaves out chunks that hold only the fill value. For five minutes after the export, the OME-Zarr reader took the store for an acquisition still being written, and `planes` returned nothing: it counted the planes of unfinished images only, so the finished images were dropped too. | `zenodo20559997-scmx-mip`, `gdal-empty1bit`, `rsciio-emd-si100-2x1x1-3d`, `rsciio-emd-example-axis-len-1`, `rsciio-emd-fftcomplexeven`, `zenodo18016957-MoS2-ref-EELS`, `rsciio-tia-16x16-spectrum-image-5x5x4000-not-square-1` | fixed: the writer stores zero chunks, and a growing store counts the planes of its finished images as complete (`docs/provenance/ome-zarr.md`; two tests in `crates/openreadout-omezarr/tests/roundtrip.rs`) |
| D2 | S3 | `planes` on a 31,740 × 20,970 RGB VSI slide (a 1.9 GB plane) peaked at 3.8 GB: the reader decoded every tile before pasting any, so it held the plane twice. | `figshare28409411-vsi-dotslide` | fixed: tiles are decoded and pasted 64 MiB at a time, 2.2 GB peak, same hash (`crates/openreadout-corpus-tests/tests/vsi_memory.rs`) |
| D3 | S4 | Exporting a file with no images (16 metadata-only VSI files, an OIR map, a CZI that holds only attachments) exited 2 with "selection matches no planes" and a hint about `--select`, which had not been given. | the `vsi-meta` files, `zenodo13680725-map-a01`, `zenodo7015307-W96-B2-B4-S-2-T-2-Z-4-C-3-Tile-5x9` | fixed: exit 6, the file's own note says why (for example that the `.ets` folder is missing), and the hint points to `info` (test in `roundtrip.rs`) |
| D4 | S4 | The exit-6 hints for pixel types the writers do not take sent each writer to the other: OME-TIFF said "OME-Zarr export takes the others" and OME-Zarr said "export to OME-TIFF", although both refuse 64-bit integer and complex images. | 9 `gdal-*` TIFFs, 10 DM and 7 EMD files | fixed: the hint says which writer takes the file, or that neither does and `planes --dump-dir` writes the raw samples |
| D5 | S4 | OME-Zarr export of a whole slide is much slower than OME-TIFF export of the same slide: 664 s against 84 s on `openslide-mirax2-2-4-bmp` and 816 s against 159 s on `openslide-mirax-cmu-1` (`main` build). OME-Zarr also peaked higher, 2.0 GB against 1.2 GB on `bia2666-vsi-24B0759-t6`. | the 16 slides above 4 GiB | open: compressing chunks on worker threads did not help on the slide we timed and doubled the peak, so we left the writer as it was (see "What is left") |

Severity follows the hunt report: S2 wrong or missing data in normal use, S3 memory or time far
beyond what the documented model says, S4 a wrong message.

## `stats` memory on `openslide-zeiss-5-flat.czi`

A reviewer saw 1.37 GB with the strip-reading `stats` change (pull request #40, not merged yet)
against 1.06 GB before it. Measured here with `/usr/bin/time -l`, 8 threads, three runs each:

| build | 1 thread | 4 threads | 8 threads | wall (8 threads) |
| --- | --- | --- | --- | --- |
| `main` | 418 MB | 927 MB | 1.06 GB | 1.9 s |
| #40 | 181 MB | 878 MB | 1.04 GB | 3.3 s |

Peak memory did not regress: it is the same or lower at every thread count. We could not
reproduce 1.37 GB. Time did regress, by 1.75 times. The two scenes are CZI mosaics of 2,056 ×
2,464 tiles placed at stage positions, so the strips, which follow a regular grid of that tile
height, cut through most tiles and each tile is decoded twice. Memory stays near 1 GB with
several threads because four strips of 84 MB are read at once, each with its own decoded tiles.
We did not change #40 from this branch. Reading CZI mosaics whole when the plane fits the decode
window, or cutting strips at the tile rows the subblock directory records, would remove the extra
decoding.

## Intended differences

These are conventions, not bugs. `book/src/reference/commands/export.md` now lists them.

- OME-Zarr has no samples axis. An RGB image becomes three channels named `<channel> (R)`,
  `(G)` and `(B)`, coloured red, green and blue.
- OME-Zarr channels whose source records no colour get a display colour (white for one channel).
  OpenReadout's own OME-XML in the store says the source had none, so OpenReadout reads back no
  colour. Other readers see the display colour.
- OME-TIFF leaves out the label images of an OME-Zarr source (`--image` still writes one).
- OME-TIFF takes 1-sample 8 to 32-bit integer, float and double images and 3-sample uint8,
  uint16 and float images. Three 2- and 4-sample `gdal-*` TIFFs go to OME-Zarr only.
- Neither writer takes 64-bit integer or complex images: 9 `gdal-*` TIFFs, 10 DM files
  (complex FFTs) and 7 EMD files (complex FFTs and int64 images).
- JPEG-compressed slides export OpenReadout's decoded values. They match the source as
  OpenReadout reads it, bit for bit, but may differ from another JPEG decoder by a few grey
  levels (the files the manifest marks `lossy`).
- 10 plates are partial copies (`hcs-*` files whose manifest notes say so). `planes` and
  OME-TIFF export stop at the first missing plane file with exit 5, and OME-Zarr export refuses
  them with a hint to pass `--skip-incomplete`.

## What is left

- **Level 0 of the largest slides was not compared.** 16 slides (6 MIRAX, 3 VSI, and 7 SVS,
  NDPI, Philips and PhenoCycler TIFFs) have a level 0 above the 4 GiB plane limit, so `planes` refuses it and this pass
  compared only their metadata and each export's own read-back verification. The next step is
  a sampled check: 16 regions of 2,048 × 2,048 per plane, hashed with `planes --region` on the
  source and on both exports and with tifffile or zarr-python on the exports.
- **OME-Zarr export of whole slides is slow (D5).** We timed one batching change and reverted
  it. The next thing to measure is where the time goes: gzip, one file per chunk on APFS, or
  the read-back of every chunk.
- **Partial plate copies** stop `planes` at the first missing plane file. A `planes` option to
  skip the missing fields, as export has, would let the rest be checked.
- The two inputs over 3 GB were not run.
