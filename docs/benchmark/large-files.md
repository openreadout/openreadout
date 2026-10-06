# Large files

Time and peak memory of the everyday operations on the largest files in the development corpus,
before and after the 2026-10 performance pass (branch `perf/large-files`). Index over many files
is benchmarked separately in [`docs/index.md`](../index.md).

## Conditions

| | |
| --- | --- |
| Machine | Apple M1 Pro (6 performance and 2 efficiency cores), 16 GB RAM, internal SSD |
| OS, compiler | macOS 26.2, rustc 1.99.0 |
| Build | `cargo build --profile profiling -p openreadout` (the release profile with line tables) |
| Before | commit `572e5922` (main) |
| After | commit `ac5fd9c2` |
| Load | 3 to 4 other agents were building and testing on the same machine. The 1-minute load average ranged from 2.6 to 11 during the runs. |

Each case runs once to warm the page cache, then three more times. The table gives the median wall
time and the largest peak RSS (`/usr/bin/time -l`). Wall times of the cases this pass did not
change differ by up to 30% between the two columns. That is the load, not the code. Treat
differences under 1.5x as noise.

```bash
cargo build --profile profiling -p openreadout
python3 bench/large_files.py run target/profiling/openreadout --out after.json
python3 bench/large_files.py report before.json after.json
```

The cases are listed in `bench/large_files.py`. The raw results are in
`bench/results/large_files_before.json` and `bench/results/large_files_after.json`.

## Results

"refused" is an export the command declines up front (exit 2 with a hint): NWB export holds the
whole selection in memory and refuses more than 64 Mi values.

| file | size | operation | before | after |
| --- | --- | --- | --- | --- |
| czi-youngmouse | 3.7 GB | `info` | 0.02 s; 17 MiB | 0.02 s; 16 MiB |
| czi-youngmouse | 3.7 GB | `preview` | 0.06 s; 35 MiB | 0.06 s; 36 MiB |
| czi-youngmouse | 3.7 GB | `stats --level 4` | 0.54 s; 220 MiB | 0.53 s; 218 MiB |
| czi-kidney | 395 MB | `info` | 0.01 s; 14 MiB | 0.01 s; 14 MiB |
| czi-kidney | 395 MB | `preview` | 0.04 s; 26 MiB | 0.04 s; 24 MiB |
| czi-kidney | 395 MB | `stats` | 1.14 s; 614 MiB | 1.00 s; 618 MiB |
| czi-kidney | 395 MB | `export ome-tiff` | 4.22 s; 491 MiB | 4.18 s; 496 MiB |
| czi-kidney | 395 MB | `export ome-zarr` | 4.59 s; 465 MiB | 4.08 s; 468 MiB |
| lif-amr1 | 3.0 GB | `info` | 0.03 s; 34 MiB | 0.03 s; 33 MiB |
| lif-amr1 | 3.0 GB | `preview` | 0.04 s; 39 MiB | 0.04 s; 38 MiB |
| lif-amr1 | 3.0 GB | `stats --image 0` | 0.05 s; 102 MiB | 0.05 s; 102 MiB |
| lif-amr1 | 3.0 GB | `stats (27 images)` | 0.73 s; 574 MiB | 0.79 s; 564 MiB |
| lif-amr1 | 3.0 GB | `export ome-tiff --image 0` | 0.11 s; 108 MiB | 0.11 s; 109 MiB |
| lif-amr1 | 3.0 GB | `export ome-zarr (27 images)` | 20 s; 857 MiB | 17 s; 845 MiB |
| nd2-sla2 | 605 MB | `info` | 0.01 s; 13 MiB | 0.01 s; 13 MiB |
| nd2-sla2 | 605 MB | `preview` | 0.02 s; 26 MiB | 0.02 s; 26 MiB |
| nd2-sla2 | 605 MB | `stats` | 0.16 s; 124 MiB | 0.15 s; 125 MiB |
| nd2-sla2 | 605 MB | `export ome-tiff` | 2.43 s; 161 MiB | 2.26 s; 146 MiB |
| nd2-sla2 | 605 MB | `export ome-zarr` | 5.40 s; 153 MiB | 3.76 s; 144 MiB |
| oir-stitch | 1.1 GB | `info` | 0.03 s; 20 MiB | 0.03 s; 18 MiB |
| oir-stitch | 1.1 GB | `preview` | 0.10 s; 92 MiB | 0.10 s; 93 MiB |
| oir-stitch | 1.1 GB | `stats` | 0.57 s; 198 MiB | 0.44 s; 198 MiB |
| oir-stitch | 1.1 GB | `export ome-tiff` | 7.03 s; 307 MiB | 5.91 s; 309 MiB |
| mrxs-bmp | 1.8 GB | `info` | 0.01 s; 13 MiB | 0.01 s; 13 MiB |
| mrxs-bmp | 1.8 GB | `preview` | 0.04 s; 46 MiB | 0.04 s; 46 MiB |
| mrxs-bmp | 1.8 GB | `stats --level 5` | 0.20 s; 112 MiB | 0.21 s; 112 MiB |
| vsi-vs200 | 1.5 GB | `info` | 0.01 s; 16 MiB | 0.01 s; 16 MiB |
| vsi-vs200 | 1.5 GB | `preview` | 0.05 s; 54 MiB | 0.05 s; 51 MiB |
| svs-cmu1 | 177 MB | `info` | 0.01 s; 14 MiB | 0.01 s; 14 MiB |
| svs-cmu1 | 177 MB | `preview` | 0.13 s; 142 MiB | 0.12 s; 141 MiB |
| svs-cmu1 | 177 MB | `stats --level 2` | 0.07 s; 59 MiB | 0.06 s; 58 MiB |
| qptiff | 422 MB | `info` | 0.01 s; 13 MiB | 0.01 s; 13 MiB |
| qptiff | 422 MB | `preview` | 0.10 s; 90 MiB | 0.09 s; 89 MiB |
| raw-eclipse | 868 MB | `info` | 0.05 s; 74 MiB | 0.05 s; 74 MiB |
| raw-eclipse | 868 MB | `export mzml` | 62 s; 89 MiB | 14 s; 191 MiB |
| raw-eclipse | 868 MB | `export parquet` | 25 s; 324 MiB | 19 s; 354 MiB |
| mzml-ascend | 1.2 GB | `info` | 0.96 s; 22 MiB | 0.38 s; 25 MiB |
| mzml-ascend | 1.2 GB | `export parquet` | 28 s; 222 MiB | 17 s; 258 MiB |
| tims-8334 | 140 MB | `info` | 0.07 s; 66 MiB | 0.07 s; 65 MiB |
| tims-8334 | 140 MB | `export mzml` | 7.82 s; 68 MiB | 4.70 s; 95 MiB |
| plx-bisver | 1.3 GB | `info` | 0.84 s; 1131 MiB | 0.85 s; 189 MiB |
| plx-bisver | 1.3 GB | `export csv` | 23 s; 1217 MiB | 4.91 s; 320 MiB |
| plx-bisver | 1.3 GB | `export parquet` | 4.74 s; 1286 MiB | 3.77 s; 427 MiB |
| plx-bisver | 1.3 GB | `export nwb` | refused; 1996 MiB | refused; 189 MiB |
| sglx-pt03 | 1.1 GB | `info` | 0.01 s; 14 MiB | 0.01 s; 13 MiB |
| sglx-pt03 | 1.1 GB | `export csv` | 97 s; 434 MiB | 18 s; 182 MiB |
| sglx-pt03 | 1.1 GB | `export parquet` | 20 s; 909 MiB | 14 s; 923 MiB |
| wid-crm2 | 403 MB | `info` | 0.01 s; 18 MiB | 0.01 s; 18 MiB |
| fcs-facsdiscover | 18 MB | `info` | 0.02 s; 22 MiB | 0.02 s; 23 MiB |
| fcs-facsdiscover | 18 MB | `export parquet` | 0.20 s; 172 MiB | 0.18 s; 181 MiB |

`info` stays headers-only on every file here, multi-file datasets included (MRXS, VSI, timsTOF
`.d`). PLX is the exception by format: it has no index, so `info` walks every data block header
(0.85 s for 1.3 GB). Previews of whole slides read the smallest pyramid level that covers the
preview size (36 to 51 MiB for the 1.5 to 3.7 GB slides). `stats` on a whole slide's full
resolution is refused above the plane limit, so the slide rows use `--level`.

## What changed

| change | effect |
| --- | --- |
| PLX block index stored as small differences (`BlockList`, `openreadout-plexon`) | `info` on a 1.3 GB recording with 31 million 10-sample blocks: 1131 MiB to 189 MiB. A trace sweep is read in one pass over its stretch of the file instead of one read per block |
| mzML writer compresses spectra on all threads, verifies in one pass, uses the CPU's SHA-1 instructions | Thermo RAW (868 MB) to mzML: 62 s to 14 s; timsTOF to mzML: 7.8 s to 4.7 s. Memory is bounded by two batches (at most 256 spectra or 4 Mi points each) |
| CSV export formats and parses rows on all threads, in segments sized by the column count | SpikeGLX (1.1 GB, 385 channels) to CSV: 97 s to 18 s and 434 MiB to 182 MiB; PLX spike table to CSV: 23 s to 4.9 s |
| mzML reader parses spectrum headers on all threads; base64 decoded through a lookup table | `info` on a 1.2 GB mzML: 0.96 s to 0.38 s; mzML to Parquet: 28 s to 17 s |
| Parquet/Arrow read-back hashes each column in one call, columns in parallel | part of the mzML and RAW to Parquet gains |
| NWB export plan borrows the trace description instead of copying it per sweep | a refused PLX export used 2 GB before refusing; now 189 MiB |
| MCP `openreadout_export` reports progress for Parquet and Arrow exports (rows or samples against the total) | a client can keep waiting on a long table or trace export instead of timing out |

Every output is byte-identical to the one written before the change. This was checked with `cmp` on
the mzML of the RAW file, the CSV of the SpikeGLX, PLX and FCS files, and the Parquet of the mzML
and RAW files, and with the JSON of `info --view full`, `check` and `spectra` on the PLX and mzML
files.

## Regression guards

These run in CI with no corpus:

- `crates/openreadout-bench/tests/info_reads.rs` opens synthetic OME-TIFF (98 MB), mzML
  (88 MB) and SpikeGLX (64 MB) files through a byte source that counts every byte read, and
  checks that `info` stays within a fixed budget: 1 MiB, 16 KiB per spectrum, and 256 KiB. Today it
  reads 94 KiB, 2.2 MB and 66 KiB.
- `crates/openreadout-bench/tests/memory_ceiling.rs` also exports mzML runs of 1,000 and 4,000
  synthetic spectra and checks that peak heap does not grow with the spectrum count (22 MiB for
  both, against a 64 MiB ceiling).
- `openreadout-plexon` `block_list_stays_small` checks that the block index costs at most 6 bytes
  per block.
- `crates/openreadout-cli/tests/mcp_progress.rs` checks that a Parquet export over MCP sends
  progress notifications that end at the total.

## Not changed, and why

- **Thermo RAW scan decoding** is the largest remaining cost of RAW to mzML and Parquet
  (`ThermoDataset::read_scan`, single-threaded). Another workstream is changing the Thermo reader.
  Reading scans on several handles in parallel would need changes there.
- **Parquet export of wide traces** peaks at 923 MiB for the 385-channel SpikeGLX file. The
  Parquet writer buffers a row group of 385 columns, and a read batch holds 64 Ki rows. Smaller
  batches would change the Arrow IPC file layout, and smaller row groups would change the Parquet
  file, so outputs would no longer be byte-identical.
- **NWB export** builds the whole selection in memory and refuses more than 64 Mi values. Streaming
  it needs chunked HDF5 datasets in the writer, a larger change.
- **Whole-file image stats and exports** (CZI, LIF) keep the existing decode windows (at most
  `threads` planes and 1 GiB per window, two windows alive). They hold 0.5 to 0.9 GiB on the
  multi-plane files here, as documented in [the memory model](../architecture-memory.md).
- **MCP `openreadout_stats`** runs without progress notifications. Whole-file stats took under
  1 s on every file here, and the MCP tool surface is being changed on another branch.
