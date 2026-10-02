#' Export to an open format
#'
#' Writes the file (or part of it) to an open format. The output is written under a temporary
#' name, read back and verified, and only then renamed into place; the source file is never
#' modified, and an existing output is only replaced with `overwrite = TRUE`.
#'
#' | `to` | what | typical inputs |
#' | --- | --- | --- |
#' | `"ome-tiff"` | BigTIFF with OME-XML; pyramidal and tiled for pyramids | every image format |
#' | `"ome-zarr"` | OME-NGFF 0.5 (Zarr v3) with a pyramid; plates as HCS plates | every image format |
#' | `"mzml"` | indexed mzML 1.1 of every mass spectrum | Thermo `.raw`, timsTOF, mzXML, ... |
#' | `"parquet"`, `"arrow"` | a table, a trace or mass spectra with units in the metadata | FCS, plate reads, ABF, chromatograms, NMR, spectra |
#' | `"rdml"` | RDML 1.3 | qPCR (`.eds`, `.rex`, RDML) |
#'
#' @param x A path or an [openreadout_open()] file.
#' @param output The file (or `.ome.zarr` directory) to write.
#' @param to The target format; by default from the extension of `output` (`.ome.tif[f]`,
#'   `.ome.zarr`/`.zarr`, `.mzML`, `.parquet`, `.arrow`/`.feather`, `.rdml`).
#' @param image Only this image (1-based); default every image (OME-TIFF) or the first (OME-Zarr).
#' @param select Plane selection strings as in the CLI (`"c=0"`, `"z=2-5"`: zero-based).
#' @param compression `"deflate"` (default), `"lzw"` or `"none"` for images; `"snappy"` (Parquet
#'   default), `"lz4"` or `"none"` for Parquet/Arrow.
#' @param overwrite Replace an existing output.
#' @param level,region Export one pyramid level, or a rectangle `c(x, y, width, height)` with `x`
#'   and `y` counted from 1 (as in [openreadout_read_image()]).
#' @param pyramid `"auto"` (default: keep the source's pyramid, build one for large planes),
#'   `"source"`, `"mean"` or `"none"`.
#' @param levels Number of pyramid levels to write.
#' @param tile Tile (OME-TIFF) or chunk (OME-Zarr) size in pixels.
#' @param run Spectra run for `mzml` and `spectra = TRUE` (1-based).
#' @param centroid Write the instrument's centroids where scans store both.
#' @param table,trace Which table or trace to write as Parquet/Arrow (1-based).
#' @param sweep One sweep of the trace (1-based); default every sweep with a `sweep` column.
#' @param spectra Write the mass spectra (one row per point, plus a per-scan summary file).
#' @param rows `c(first, last)` rows or samples (1-based, inclusive).
#' @param embed_vendor Also store the vendor metadata tree in the output.
#' @return The export report (a list: output, verified, sizes, ...), invisibly.
#' @examples
#' \dontrun{
#' openreadout_export("run42.czi", "run42.ome.tiff")
#' openreadout_export("slide.svs", "slide.ome.zarr", region = c(1, 1, 4096, 4096))
#' openreadout_export("tube1.fcs", "tube1.parquet")
#' openreadout_export("sample.raw", "sample.mzML")
#' }
#' @export
openreadout_export <- function(x, output, to = NULL, image = NULL, select = NULL, compression = NULL,
                      overwrite = FALSE, level = 0, region = NULL, pyramid = NULL, levels = NULL,
                      tile = NULL, run = 1, centroid = FALSE, table = NULL, trace = NULL,
                      sweep = NULL, spectra = FALSE, rows = NULL, embed_vendor = FALSE) {
  if (!is.character(output) || length(output) != 1) .openreadout_abort("output must be a single path")
  output <- path.expand(output)
  to <- to %||% .export_target(output)
  to <- match.arg(to, c("ome-tiff", "ome-zarr", "mzml", "parquet", "arrow", "rdml"))
  if (!is.null(region)) {
    if (!is.numeric(region) || length(region) != 4 || any(region[1:2] < 1)) {
      .openreadout_abort("region must be c(x, y, width, height) with x and y from 1")
    }
    region <- I(as.integer(c(region[1] - 1, region[2] - 1, region[3], region[4])))
  }
  if (!is.null(rows)) {
    if (!is.numeric(rows) || length(rows) != 2 || rows[1] < 1 || rows[2] < rows[1]) {
      .openreadout_abort("rows must be c(first, last) with 1 <= first <= last")
    }
    rows <- I(c(rows[1] - 1, rows[2] - 1))
  }
  opts <- list(
    image = .idx_or_null(image, "image"),
    select = if (!is.null(select)) I(as.character(select)),
    compression = compression,
    overwrite = if (isTRUE(overwrite)) TRUE,
    embed_vendor = if (isTRUE(embed_vendor)) TRUE,
    level = as.integer(level),
    region = region,
    pyramid = pyramid,
    levels = levels,
    tile = tile,
    run = .index0(run, "run"),
    centroid = if (isTRUE(centroid)) TRUE,
    table = .idx_or_null(table, "table"),
    trace = .idx_or_null(trace, "trace"),
    sweep = .idx_or_null(sweep, "sweep"),
    spectra = if (isTRUE(spectra)) TRUE,
    rows = rows
  )
  report <- .with_file(x, function(h) .from_json(.ic(rs_export(.ptr(h), to, output, .to_json(opts)))))
  invisible(report)
}

.export_target <- function(output) {
  o <- tolower(output)
  if (grepl("\\.ome\\.tiff?$", o) || grepl("\\.tiff?$", o)) return("ome-tiff")
  if (grepl("\\.zarr/?$", o)) return("ome-zarr")
  if (grepl("\\.mzml$", o)) return("mzml")
  if (grepl("\\.parquet$", o)) return("parquet")
  if (grepl("\\.(arrow|feather|ipc)$", o)) return("arrow")
  if (grepl("\\.rdml$", o)) return("rdml")
  .openreadout_abort(sprintf(
    "cannot tell the export format from %s: pass to = \"ome-tiff\", \"ome-zarr\", \"mzml\", \"parquet\", \"arrow\" or \"rdml\"",
    basename(output)), call = sys.call(-1))
}

# openreadout_analyze() kind "qpcr".
.analyze_qpcr <- function(path, well = NULL, target = NULL, sample = NULL, run = NULL, compute_cq = FALSE,
                    threshold = NULL, baseline = NULL, ddcq = FALSE, reference_targets = NULL,
                    control_sample = NULL, standard_curve = FALSE, max_records = NULL,
                    undetermined_cq = NULL) {
  req <- list(well = well, target = target, sample = sample, run = run,
              compute_cq = if (isTRUE(compute_cq)) TRUE, threshold = threshold,
              baseline = if (!is.null(baseline)) I(as.integer(baseline)),
              ddcq = if (isTRUE(ddcq)) TRUE,
              reference_targets = if (!is.null(reference_targets)) I(as.character(reference_targets)),
              control_sample = control_sample, standard_curve = if (isTRUE(standard_curve)) TRUE,
              max_records = max_records, undetermined_cq = undetermined_cq)
  out <- .from_json(.ic(rs_qpcr(.path(path), .to_json(req))))
  df <- .records(out$records)
  out$records <- NULL
  attr(df, "report") <- out
  df
}
