# Memory model

How much memory each command needs, and why it does not grow with the size of the file.
Keep this page current when a reader changes how it reads.

## Rules

1. **No whole-file reads.** Readers open the file, `seek` and `read_exact` the bytes they need.
   There is no `fs::read`, `read_to_end` or memory map of an input file anywhere in the
   binary (only tests and `xtask` read whole files). Format detection reads the first 64 KiB.
2. **Metadata is bounded.** Every metadata block (CZI `ZISRAWMETADATA` XML, subblock and
   attachment directories, ND2 chunk map and LV/XML chunks, LIF XML header) is loaded whole,
   but its declared length is checked before allocating: it must fit in the rest of the file
   and under `MAX_METADATA_BYTES` (512 MiB), else the file is reported as corrupt
   (`openreadout_core::limits::checked_metadata_len`).
3. **Pixels are read one plane at a time.** `read_plane` returns a single (c, z, t) plane;
   `planes`, `export` and the MCP/Python front ends loop over planes and drop each before
   reading the next. A plane larger than the plane limit — 256x the file size, at least
   1 GiB and at most 4 GiB, or `OPENREADOUT_MAX_PLANE_BYTES=<bytes>` when set (still at most 4 GiB) — is
   refused with `unsupported_feature` instead of being allocated (`limits::plane_len`), so a
   few-KB file cannot make a reader allocate gigabytes.
4. **Every allocation driven by a header value is capped** against the bytes that could back
   it (`limits::checked_len`, `checked_count`), and decoders never produce more than the
   expected output (+1 byte to detect overlong streams): a decompression bomb costs at most
   one plane.
5. **Declared counts never drive a loop or a table on their own.** A count read from a
   header is also bounded by what the file could physically hold or by what any instrument
   writes, whichever applies (found by fuzzing each reader; `docs/provenance/<fmt>.md`):
   ND2 frames per image ≤ file length / 16; VSI pyramid levels ≤ 33 and missing planes
   counted from the tiles present, not by visiting each declared plane; HDF5 tree walks ≤
   file length / 16 objects, 32 group levels and 4096-byte paths (hard links can form
   cycles); SQLite (timsTOF) record payloads ≤ the pages the database and its WAL hold;
   timsTOF empty frames ≤ 2^20 scans; ABF sweeps ≤ samples per channel; SpikeGLX streams ≤
   65,536 channels; one JCAMP-DX ASDF table ≤ 2^24 ordinates; plate-reader values beyond 1024
   rows or columns are skipped and a worksheet spans ≤ 4 Mi cells (XLSX is streamed cell by
   cell); a JPEG frame is refused before decoding when it is larger than the tile, strip or
   subblock holding it. Sidecar and companion text files (OME-XML companions, XML
   containers, SpikeGLX `.meta`, Bruker `nuslist`) are size-checked before being read
   (`limits::read_metadata_file`).

## Per command

`P` = bytes of one decoded plane (width x height x samples x bytes per sample), `T` = bytes
of the largest decoded tile / subblock, `M` = size of the metadata blocks (XML, directories),
`N` = number of structural records (CZI subblocks, ND2 chunks, LIF memory blocks).

| command | peak memory | notes |
| --- | --- | --- |
| `info --view format` | 64 KiB | first 64 KiB only |
| `info`, `info --view structure` | O(M + N) | headers only; no pixel bytes are read. CZI keeps ~200 B per directory entry and 32 B per segment header; ND2 ~100 B per chunk-map entry; LIF ~80 B per memory block; PLX a few bytes per data block (`BlockList`) |
| `info --view full` | O(M + N) + vendor JSON | the vendor tree is the XML/LV metadata converted to JSON (typically 3-8x the XML size) |
| `check` | O(M + N) | reads one 256-byte subblock header (CZI) / 16-byte chunk header (ND2) per record; never decodes pixels; findings are O(problems) |
| `planes` | O(M + N) + P + T + compressed(T) | one plane at a time; CZI mosaics stitch tiles into the plane buffer, so one decoded tile and its compressed bytes coexist with the plane |
| `export --format ome-tiff` | O(M + N) + ~2-3 P | the plane, its compressed strip in the TIFF encoder, and during read-back verification one decoded IFD |
| `export --format ome-zarr` | O(M + N) + ~3 P | plane, de-interleaved samples, one downsampled level and the chunk buffer |
| `export --format mzml` | O(spectra) + 2 batches | spectra are read and compressed in batches of at most 256 spectra or 4 Mi points; one batch is compressed while the next is read. The index keeps ~100 B per spectrum |
| `export --format csv` | one read batch + 2 x threads segments | a read batch holds at most 64 Ki rows and 8 Mi values; rows are formatted and parsed back in segments of at most 4096 rows and 128 Ki values |
| MCP tools, Python `File` | same as the matching command | the Python `read_image` helpers allocate the requested N-d array on purpose |

Time for `info`/`info --view structure` is linear in `N` (one small read per CZI segment in the sequential
segment walk; ND2 and LIF read a map/XML and walk block headers). Nothing is linear in the
file size except the ND2 rescue scan, which only runs when the chunk map is unusable and
streams the file through a 1 MiB buffer.

## Measured

`/usr/bin/time -l` on macOS (arm64, 16 GB), release build, 2026-09-22, on a machine busy with
other builds (wall times are indicative only).

`zenodo10577621-Young-mouse.czi`: 3.7 GB, one scene of 190 309 x 69 378 px BGR24 stitched from
5 527 level-0 tiles (8 529 subblocks with the pyramid):

| command | max RSS | wall |
| --- | --- | --- |
| `info` | 7.5 MiB | 0.1 s warm (4.7 s cold) |
| `check` (8 529 subblock headers verified) | 8.8 MiB | 0.12 s |
| `planes --select c=0 --select z=0 --select t=0` (one plane) | 7.4 MiB | 0.05 s — refused, exit 6: the stitched plane is 39.6 GB, above the 4 GiB limit |

A plane that does fit, `aics-variable-scene-shape-first-scene-pyramid.czi` (412 MiB, one plane =
7 705 x 6 183 x uint16 = 95 MB stitched from zstd tiles):

| command | max RSS | ≈ |
| --- | --- | --- |
| `planes --select c=0` | 111 MiB | P + 16 MiB |
| `export --select c=0` (OME-TIFF, deflate, read-back verified) | 203 MiB | 2.1 P |

`ome-imagesc-110520-AMR1.lif` (2.8 GB, 27 images, 735 planes): `info` 28 MiB, `check` 27 MiB
(dominated by the multi-MB XML header and its parsed tree).

`mtbls11852-api4000-ku4.wiff` (14.5 MB with its `.wiff.scan`, 25 samples of MRM cycles with
about 1,170 precursors each, so 1.2 million spectra per sample). The Sciex reader keeps one
entry per index record (cycle) and works out a spectrum's transitions when it reads it, caching
the last cycle's values (2026-10-07):

| command | before | after |
| --- | --- | --- |
| `info` | 1.7 GiB, 2.9 s | 31 MiB, 1.6 s |
| `analyze chromatogram` (TIC, 1.2 M points) | 5.4 GiB, 16 s | 287 MiB, 1.9 s |
| `export --format mzml` (4.1 GB of mzML) | 1.8 GiB, 50 s | 196 MiB, 43 s |

## Known limits

- A CZI mosaic plane is materialized whole (stitched). Whole-slide scans whose level-0 plane
  exceeds available RAM need a tiled/region API (not yet available); `planes`/`export`
  refuse planes above the cap rather than exhausting memory.
- `N` is not bounded by a constant: a CZI with millions of subblocks costs a few hundred MB
  for `info`. Real files have 10^2-10^5 subblocks.
- XLS, XLSB and ODS plate-reader workbooks go through `calamine`'s `worksheet_range`, which
  allocates the dense bounding box of all cells before our 4 Mi-cell check sees it; only
  XLSX is streamed (SECURITY.md).
- JPEG XR tiles are decoded in memory (`openreadout-jpegxr`): two macroblock rows of 32-bit
  coefficients per channel plus the output tile; no temporary files.
