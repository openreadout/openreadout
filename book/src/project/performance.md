# Performance

This page gives the time and memory OpenReadout needs, compared with Bio-Formats and bioio on the same files. The commands to reproduce every table are at the end. The raw results of the main comparison are in [`bench/results/compare.json`](https://github.com/openreadout/openreadout/blob/main/bench/results/compare.json).

Read the numbers with their conditions. They come from one laptop that was shared with other build jobs, so treat them as order-of-magnitude comparisons, not precise ratios. A † marks a measurement taken at a 1-minute load average above 3.

## Conditions

| | |
|---|---|
| Machine | Apple M1 Pro (8 cores: 6 performance, 2 efficiency), 16 GB RAM, internal SSD |
| OS | macOS 26.2 |
| OpenReadout | 0.1.0 at commit `ba314f2`, `cargo build --release`, rustc 1.98.1 |
| Bio-Formats | bftools 8.5.0 (`showinf`, `bfconvert`), OpenJDK 17.0.5, default `-Xmx512m` |
| Python | CPython 3.12.12; bioio 3.5.1 with bioio-czi 3.0.0, bioio-nd2 2.5.0, bioio-lif 1.5.0, bioio-ome-tiff 1.4.0; flowio 1.4.0; pyabf 2.3.8; pyteomics 5.0.1 |
| Load | 1-minute load average 2.0–3.0 before most measurements |

## Method

- **Same files, warm cache.** Each command runs once to warm the page cache, then 5 more times. The tables give the median. The files are from the [test corpus](validation.md#the-test-corpus).
- **One process per run.** Wall time is measured from the parent process. Peak resident memory (RSS) comes from `/usr/bin/time -l`.
- **Start-up is measured separately.** The Python readers also report the time spent inside the process after the imports ("in-process"), so interpreter start-up is visible but not counted twice.
- **Like-for-like metadata.** `openreadout info --json`; `showinf -nopix -no-upgrade`; for bioio, `BioImage(path)` followed by the dimensions, data type, channel names, pixel sizes and metadata of every scene.
- **Like-for-like export.** Uncompressed OME-TIFF from all three tools. OpenReadout's time includes reading the whole output back and comparing every plane, which the others do not do.

## Start-up (no file)

| Process | Time | Peak RSS |
|---|---|---|
| `openreadout --version` | 5.2 ms | 4 MiB |
| Bio-Formats `showinf -version` (JVM) | 221 ms | 64 MiB |
| `python -c pass` | 18 ms | 15 MiB |
| Python with `import bioio` and four plugins | 1.19 s | 174 MiB |

## Metadata (`info`)

Wall time per process and peak RSS.

| File | Size | OpenReadout `info` | Bio-Formats `showinf -nopix` | Python reader (in-process time) |
|---|---|---|---|---|
| CZI `aics-s-3-t-1-c-3-z-5` | 15 MB | 7.7 ms; 7 MiB | 587 ms; 135 MiB | bioio 1.52 s (243 ms); 206 MiB |
| CZI `zenodo10577621-Young-mouse` | 3.7 GB | 19 ms; 9 MiB | 991 ms; 322 MiB | bioio 8.22 s (6.90 s); 414 MiB |
| LIF `ome-imagesc-110520-AMR1` | 3.0 GB | 24 ms; 32 MiB | 945 ms; 288 MiB | bioio 2.47 s (1.26 s); 334 MiB |
| ND2 `ome-jonas-control002` | 48 MB | 6.4 ms; 5 MiB | 565 ms; 168 MiB | bioio 1.62 s (376 ms); 207 MiB |
| OME-TIFF `aics-s-3-t-1-c-3-z-5` | 9.7 MB | 11 ms; 14 MiB | 876 ms; 263 MiB | bioio 1.70 s (394 ms); 226 MiB |
| FCS `flowio-m0-wm278-s1-zero-gain` | 13 MB | 6.5 ms; 5 MiB | n/a | flowio 73 ms (0.3 ms); 33 MiB |
| ABF `pyabf-2020-07-29-0062` | 0.8 MB | 4.9 ms; 5 MiB | n/a | pyabf 78 ms (2.3 ms); 35 MiB |
| Thermo RAW `mtbls404-QC1_001` | 42 MB | 7.9 ms; 10 MiB | n/a | n/a |
| mzML `mtbls404-QC1_001` | 115 MB | 50 ms; 6 MiB | n/a | pyteomics 1.93 s (1.45 s); 132 MiB |

For a one-off question about a file, a native binary with nothing to start answers in milliseconds. JVM and Python tools spend most of their time starting. Inside a running Python process, the FCS and ABF readers are as fast as OpenReadout. `info` on mzML reads each spectrum header through the index, so its time grows with the number of spectra.

## Export to OME-TIFF

Wall time per process and peak RSS, with the default number of threads (8 here).

| File | OpenReadout (none) | OpenReadout (deflate) | `bfconvert` | bioio `save` (in-process) |
|---|---|---|---|---|
| CZI `aics-s-3-t-1-c-3-z-5` (15 MB) | 18 ms; 22 MiB | 67 ms; 24 MiB | 2.56 s; 381 MiB | 1.57 s (425 ms); 232 MiB |
| ND2 `ome-jonas-control002` (48 MB) | 32 ms; 13 MiB | 237 ms; 18 MiB | 3.76 s; 286 MiB | 2.43 s (1.03 s); 359 MiB † |
| LIF `zenodo14976703-Convalaria-LambdaScan` (15 MB) | 16 ms; 19 MiB † | 89 ms; 23 MiB | 1.35 s; 290 MiB | 1.54 s (350 ms); 221 MiB |
| OME-TIFF `aics-s-3-t-1-c-3-z-5` (9.7 MB) | 60 ms; 60 MiB | 115 ms; 62 MiB | 1.61 s; 306 MiB | 2.17 s (878 ms); 279 MiB † |

## Threads

`openreadout --threads N export FILE --compression …`: median wall time, speed-up over 1 thread, and peak RSS. Planes are decoded and compressed on N threads and written by one writer. The output is byte-identical for every N.

| File | 1 thread | 2 threads | 4 threads | 8 threads |
|---|---|---|---|---|
| ND2 `ome-karl-sample-image` 218 MB, none | 443 ms; 24 MiB | 229 ms (1.9x); 43 MiB | 157 ms (2.8x); 79 MiB | 137 ms (3.2x); 147 MiB |
| same, deflate | 4.66 s; 30 MiB | 2.46 s (1.9x); 48 MiB | 1.31 s (3.6x); 90 MiB | 1.06 s (4.4x); 157 MiB |
| CZI `openslide-zeiss-5-jxr` 69 MB, none | 4.90 s; 409 MiB | 2.60 s (1.9x); 832 MiB | 1.66 s (3.0x); 912 MiB | 1.36 s (3.6x); 1054 MiB |

Deflate is limited by compression and gains the most from threads. Uncompressed export is limited by memory copies, the write and the read-back. The 8-thread column also uses the slower efficiency cores.

## Memory

Export holds a bounded number of decoded planes, whatever the size of the file. Planes are decoded in windows of at most `threads` planes and at most 1 GiB, and at most two windows are alive at a time. Peak memory is therefore a few planes plus a few tens of MiB, not the file. No plane above 4 GiB is assembled in memory; larger images are read and exported block by block.

`crates/openreadout-bench/tests/memory_ceiling.rs` enforces this. It counts heap bytes while exporting synthetic images of 16 and 64 planes at 1 and 4 threads, and checks that the peak does not grow with the number of planes.

## Large images and whole slides

Release build, warm page cache, one run each, on the same laptop under heavy load (load average about 10).

Reading one 512 × 512 window at the centre of a level (`stats --level L --region X,Y,512,512`):

| File | Level 0 size | Level 0 window | Level 2 window |
| --- | --- | --- | --- |
| `zenodo10577621-Young-mouse.czi` (3.7 GB, JPEG XR) | 190,309 × 69,378 RGB | 0.08 s, 59 MB | 0.05 s, 28 MB |
| `openslide-aperio-CMU-1.svs` | 46,000 × 32,914 RGB | 0.01 s, 20 MB | 0.01 s, 20 MB |
| `openslide-hamamatsu-CMU-1.ndpi` | 51,200 × 38,144 RGB | 0.02 s, 23 MB | 0.01 s, 20 MB |
| `ome-qptiff-HandEcompressed_Scan1.qptiff` | 30,720 × 26,640 RGB | 0.01 s, 24 MB | 0.01 s, 18 MB |

Whole-slide exports; every block of every level is read back and compared before the rename:

| Export | Output | Time | Peak RSS |
| --- | --- | --- | --- |
| CMU-1.svs to OME-TIFF, source pyramid (3 levels) | 935 MB | 14.6 s | 1.04 GB |
| CMU-1.svs to OME-Zarr, source pyramid | 919 MB | 42.4 s | 1.20 GB |
| Young-mouse CZI, 20,000 × 20,000 region to OME-TIFF, 6 levels | 1.27 GB | 14.9 s | 1.15 GB |

## Screening plates

`info` on a plate reads the index and lists the folder once. `check` also reads the TIFF header of every plane file. Release build, median of 3 runs, machine under shared load.

| Plate | `info` | `check` |
| --- | --- | --- |
| JUMP Phenix index (47 MB, 27,648 planes) | 335 ms; 101 MiB | 280 ms; 46 MiB |
| CellVoyager CV8000 (2,304 fields) | 107 ms; 111 MiB | 50 ms; 34 MiB |
| CellVoyager CV7000 (2,817 fields) | 72 ms; 63 MiB | 37 ms; 22 MiB |

## Python arrays

`bench/python_arrays.py` runs each task in its own Python process, so the time includes importing the package and opening the file. Median of 3. `full` reads every plane of image 0 into one NumPy array; `region` reads the 512 × 512 window at the centre of full resolution. Both readers returned the same pixels in every row.

| File | Task | openreadout | bioio reader |
| --- | --- | --- | --- |
| `aics-s-3-t-1-c-3-z-5.czi` | full | 0.009 s, 38 MB | bioio-czi 0.968 s, 182 MB |
| `zenodo10577621-Kidney-RAC-3color.czi` | full | 1.26 s, 652 MB | bioio-czi 5.11 s, 1296 MB |
| `zenodo10577621-Young-mouse.czi` | region | 0.071 s, 78 MB | bioio-czi 8.42 s, 453 MB |
| `ome-karl-sample-image.nd2` | full | 0.424 s, 256 MB | bioio-nd2 1.34 s, 612 MB |
| `aics-s-3-t-1-c-3-z-5.ome.tiff` | full | 0.036 s, 46 MB | bioio-tifffile 1.04 s, 203 MB |

Much of each small-file time is start-up. For regions, the gap is mostly because bioio's plugins read whole planes, while OpenReadout reads only the tiles it needs.

## Codecs

Single-threaded decode throughput on the same 64 chunks of 512 × 512 pixels from a brightfield slide, compared with imagecodecs, which wraps the C libraries. One run each, on a loaded machine.

| Codec | OpenReadout (crate) | imagecodecs (library) | Ratio |
|---|---|---|---|
| WebP lossy (RGB) | 300 MB/s (`image-webp`) | 564 MB/s (libwebp) | 0.53 |
| JPEG XL lossy (RGB) | 53 MB/s (`jxl-oxide`) | 179 MB/s (libjxl) | 0.30 |
| JPEG 2000 lossy (RGB) | 38 MB/s (`rust-j2k`) | 138 MB/s (OpenJPEG) | 0.27 |
| JPEG 2000 lossless, 16-bit grey | 6.5 MB/s | 21 MB/s | 0.31 |
| JPEG XR lossy (RGB) | 185 MB/s (`openreadout-jpegxr`) | 280 MB/s (jxrlib) | 0.66 |
| JPEG XR lossless, 16-bit grey | 120 MB/s | 143 MB/s | 0.84 |

JPEG 2000 is the slowest codec. `rust-j2k` is used because it agrees with OpenJPEG to within 1 grey level.

## Reproduce

```bash
# micro-benchmarks (about 5 minutes; corpus files are used when present)
OPENREADOUT_CORPUS_DIR=$PWD/corpus/files cargo bench -p openreadout-bench
cargo nextest run -p openreadout-bench               # memory ceiling and synthetic-input tests

# comparison with Bio-Formats and bioio (needs the oracle environment and oracle/bftools)
cargo build --release -p openreadout
cargo xtask corpus fetch --tier smoke                # plus the standard and full files named in bench/compare.py
uv run --project oracle python bench/compare.py run --reps 5 --max-load 3
uv run --project oracle python bench/compare.py report

# Python arrays (build the Python extension first)
oracle/.venv/bin/python bench/python_arrays.py run --reps 3
oracle/.venv/bin/python bench/python_arrays.py report

# codecs and ND2 plane reads
cargo run --release -p openreadout-codecs --example codecbench -- <codec> <dir>
cargo run --release -p openreadout-nd2 --example nd2_throughput -- FILE.nd2

# profiling build: release code with symbols, for samply, cargo-flamegraph or macOS `sample`
cargo build --profile profiling -p openreadout
```

## Known limits

- All numbers come from one machine under shared load. A run on a quiet machine and on Linux x86-64 would make them firmer.
- `info` on mzML reads every spectrum header, so its time grows with the number of spectra. JCAMP-DX files are parsed whole.
