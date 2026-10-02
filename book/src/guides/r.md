# R

The `openreadout` R package reads raw instrument files into R arrays and data frames. It compiles the same Rust readers as the command line into the package with [extendr](https://extendr.github.io/), so it does not call the `openreadout` program. Its only required R dependency is `jsonlite`; it returns tibbles when the tibble package is installed.

```r
library(openreadout)

info <- openreadout_info("run42.czi")                   # metadata: images, channels, pixel sizes, experiment
img  <- openreadout_read_image("run42.czi", image = 2)  # array [x, y, z, c, t]
ev   <- openreadout_table("tube1.fcs")                  # FCS events, one column per parameter
tr   <- openreadout_trace("cell3.abf", sweep = 3)       # time_s and one column per channel
sp   <- openreadout_spectra("sample.raw", scan = 1200)  # mz and intensity
xic  <- openreadout_analyze("sample.raw", "chromatogram", mz = 195.0877, ppm = 5)
pk   <- openreadout_analyze("run.D", "peaks", traces = 1)  # one row per integrated peak
openreadout_export("run42.czi", "run42.ome.tiff")       # verified by reading it back
```

## Install

The package builds its Rust library from source. You need R 4.2 or newer, the usual R build tools (Rtools on Windows, the Xcode command-line tools on macOS) and a Rust toolchain, version 1.91 or newer, from [rustup](https://rustup.rs) or `brew install rust`.

From a checkout of the repository:

```bash
git clone https://github.com/openreadout/openreadout && cd openreadout
R CMD INSTALL r/openreadout
```

The first build takes several minutes. Binary packages are not available yet.

To build a self-contained source package that installs offline, vendor the Rust dependencies first:

```bash
cargo xtask r-vendor
R CMD build r/openreadout
R -e 'install.packages("openreadout_0.1.0.tar.gz", repos = NULL)'
```

## Functions

Every function takes a path or a handle from `openreadout_open()`. A handle reads through one open file instead of reopening it; close it with `openreadout_close()`. `?openreadout_read_image` and the other help pages document each argument. The functions are named after the [commands](../reference/commands/index.md) of the command line.

Metadata and integrity:

- `openreadout_info(x, view)`: the `openreadout info --view VIEW --json` data as a list. `view = "summary"` (the default) is the normalized summary; `"full"` adds the vendor's metadata tree (`vendor`) and where each normalized field's meaning came from (`provenance`); `"structure"` lists the container's elements; `"explain"` describes the file in sentences (`ask` answers a question); `"format"` is only the detected format.
- `openreadout_formats()`: a data frame of supported formats.
- `openreadout_check(x)`: the integrity report.
- `openreadout_ome_xml(x)`: OME-XML.

Data:

- `openreadout_read_image(x, image, c, z, t, level, region, drop)`: an array `[x, y, z, c, t]`, or `[x, y, s, z, c, t]` for RGB.
- `openreadout_read_plane(x, image, c, z, t, level, region)`: one plane as a matrix `[x, y]`.
- `openreadout_levels(x, image)`: the pyramid levels of an image.
- `openreadout_table(x, table, first_row, n_max)`: FCS events, plate reads, and peak and event tables, as a data frame.
- `openreadout_trace(x, trace, sweep, first_sample, n_max)`: one row per sample, with the x axis and one column per channel in physical units.
- `openreadout_spectra(x, scan, index, run, centroid, ms_level, ...)`: the scan headers of a mass-spectrometry run, or with `scan` or `index` one mass spectrum.
- `openreadout_stats(x, image, select, per, wells)`: pixel statistics per channel, image or plane; `per = "well"` (or `"field"`) gives per-well statistics of a screening plate.

Analyses:

- `openreadout_analyze(x, kind, ...)`: `openreadout analyze KIND`. `"chromatogram"` and `"peaks"` return data frames of chromatograms and integrated peaks, or band areas on a spectrum (see [Chromatograms and peaks](quantitation.md)); `"qpcr"` returns one row per well and target with Cq and Tm, with ΔΔCq and standard curves in the `report` attribute; `"nmr-peaks"`, `"ephys-features"`, `"spikes"`, `"assay"` and `"gate"` return the command's JSON as a list. The options are those of the MCP tool `openreadout_analyze`, with 1-based indices.

Many files and export:

- `openreadout_batch(measure, inputs, sample_sheets, where, by, ...)`: one tidy table over many files, with a group summary. See [Many files](batch.md). `openreadout_batch("summarize", table, by, ...)` gives the group statistics of a table written earlier.
- `openreadout_link(paths)`: files of the same sample across instruments.
- `openreadout_export(x, output, to, ...)`: OME-TIFF, OME-Zarr, mzML, Parquet, Arrow or RDML, chosen from the file extension and verified by reading it back.

The argument names match the [Python package](python.md) and the [MCP tools](../reference/mcp.md).

## Indices start at 1

Indices that count from the start are 1-based, as for R vectors. `image = 1` is the first image and `c = 2` the second channel; `sweep`, `table`, `trace`, `run` and `index` work the same way. In `region = c(x, y, width, height)`, `x` and `y` are the column and row of the top-left pixel, counted from 1.

Some numbers keep the file's own numbering:

- the `index` fields in the metadata lists from `openreadout_info()` start at 0;
- pyramid `level` 0 is full resolution;
- the `image`, `channel` and `sweep` columns of `openreadout_batch()` start at 0, because it is the same table the command line writes;
- `select` strings use the command line's syntax, such as `"c=0"`.

## Images

```r
f <- openreadout_open("run42.czi")
img <- openreadout_read_image(f, image = 2)
dim(img)                        # x, y, z, c, t
dimnames(img)$c                 # channel names from the file
attr(img, "physical_size")      # µm per pixel, and the z step
mip <- apply(img[, , , 1, 1], c(1, 2), max)          # maximum projection of channel 1
openreadout_close(f)

thumb <- openreadout_read_plane("slide.svs", level = 3)
tile  <- openreadout_read_plane("slide.svs", region = c(20001, 15001, 1024, 1024))   # decodes only its tiles
```

The axis order `x, y, z, c, t` is the order the pixels are stored in. It is the reverse of the Python package's `T, C, Z, Y, X`, and the order EBImage and imager use, so `imager::as.cimg(img[, , , 1, 1])` works as is. Use `t(m)` for a row-by-column matrix.

8-bit, 16-bit and signed 32-bit samples become R integers. Unsigned 32-bit and floating-point samples become doubles. `drop = TRUE` drops dimensions of size 1 other than `x` and `y`.

`openreadout_read_image()` refuses to read more than `getOption("openreadout.max_bytes")` (default 4 GiB) into R memory. The error says how to read less: fewer planes, a coarser level, or a region.

## Tables

Table functions return a tibble when the tibble package is installed. Set `options(openreadout.tibble = FALSE)` for plain data frames. Units, labels and stored types are attributes, for example `attr(ev, "units")`. Values are raw: FCS events are neither compensated nor scaled.

## Errors

A failure is an R condition of class `openreadout_error` and one of these classes:

- `openreadout_file_not_found`
- `openreadout_io_error`
- `openreadout_unknown_format`
- `openreadout_usage_error`
- `openreadout_corrupt_file`
- `openreadout_unsupported_feature`

Each condition carries `code`, `exit_code` and `hint`, as in the command line's [JSON errors](../getting-started/reading-json.md#the-json-wrapper). The message ends with the hint.

```r
tryCatch(openreadout_read_image("broken.czi"),
         openreadout_corrupt_file = function(e) message("corrupt: ", e$hint))
```

A malformed file never crashes R: a reader error or a bug in the Rust code is raised as an R error.

## Working on the package

```bash
OPENREADOUT_R_PROFILE=dev R CMD INSTALL r/openreadout     # unoptimized, fast rebuilds
Rscript -e 'testthat::test_local("r/openreadout")'
```

Tests that compare with reference files need `OPENREADOUT_CORPUS_DIR`; without it they are skipped. The bindings are in [`crates/openreadout-r`](https://github.com/openreadout/openreadout/tree/main/crates/openreadout-r).
