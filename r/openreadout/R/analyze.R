# openreadout_analyze(): `openreadout analyze KIND` (one MCP tool per kind). The kinds with
# R-shaped results (chromatogram, peaks, qpcr) are in chrom.R and export.R; the others pass
# their options through as JSON.

.analyze_kinds <- c("peaks", "chromatogram", "nmr-peaks", "ephys-features", "spikes", "qpcr",
                    "assay", "gate")

# Options that hold one index (1-based in R, 0-based for the native side) or a list of them.
.index_options <- c("trace", "sweep", "channel", "table")
.index_list_options <- c("sweeps", "channels")
# Options that are JSON arrays even with one element.
.array_options <- c("sweeps", "channels", "medians", "populations", "reference_targets",
                    "integrate")
# Options that are paths.
.path_options <- c("layout", "workspace", "gatingml")

.analyze_options <- function(opts) {
  opts <- opts[!vapply(opts, is.null, logical(1))]
  for (k in intersect(names(opts), .index_options)) {
    opts[[k]] <- .index0(opts[[k]], k)
  }
  for (k in intersect(names(opts), .index_list_options)) {
    opts[[k]] <- vapply(opts[[k]], .index0, 0L, what = k)
  }
  for (k in intersect(names(opts), .path_options)) {
    opts[[k]] <- path.expand(opts[[k]])
  }
  if (!is.null(opts$integrate)) {
    opts$integrate <- matrix(as.numeric(opts$integrate), ncol = 2)
  }
  for (k in intersect(names(opts), .array_options)) {
    if (!is.matrix(opts[[k]])) opts[[k]] <- I(opts[[k]])
  }
  .to_json(opts)
}

#' Analyses with documented methods
#'
#' `openreadout analyze KIND`: chromatograms and chromatographic peaks, NMR peaks and
#' integrals, patch-clamp features, extracellular spikes, qPCR, plate-reader assays and
#' flow-cytometry gating. The methods are described in the OpenReadout book
#' (<https://openreadout.github.io/openreadout/>), and the options are the arguments of that
#' kind's MCP tool (`openreadout_peaks`, `openreadout_nmr_peaks`, ...), with R's 1-based indices (`trace`, `sweep`,
#' `channel`, `table`, `sweeps`, `channels`, `traces`, `run`).
#'
#' @section `kind = "chromatogram"`:
#' Total-ion (`tic = TRUE`), base-peak (`bpc = TRUE`) and extracted-ion (`mz`, with `ppm`
#' (default 10) or `da`) chromatograms computed from the mass spectra in one pass, SRM/MRM
#' `transitions` (`"Q1>Q3"` strings such as `"279.2>179.2"`), or stored detector signals
#' (`traces`: UV/DAD, FID, TCD, stored TIC; `channel` picks one DAD wavelength). With no source
#' chosen: the first detector trace, else the TIC. Other options: `run`, `ms_level`, `polarity`
#' (`"positive"`, `"negative"`), `scan_filter`, `precursor` (product-ion XIC), `rt_range` and
#' `mz_range` (`c(start, end)` in minutes / m/z), `max_points` (default 20000), `precursor_tol`,
#' `transition_tol`, `profile`, `aggregate`.
#'
#' Returns a long data frame: `chromatogram` (label), `kind`, `rt_min`, `intensity`; attribute
#' `chromatograms` holds one row per chromatogram (label, kind, source, points, apex, integral,
#' intensity unit, decimated, ...), `notes` the notes.
#'
#' @section `kind = "peaks"`:
#' Finds and integrates peaks in the chromatograms chosen as for `"chromatogram"`: retention
#' time, start, end, height, area, area %, widths, tailing and asymmetry factors, plates,
#' resolution and signal-to-noise per peak. Method options: `smooth` (Savitzky–Golay window in
#' points), `min_snr` (default 3), `min_height`, `min_width` (minutes), `baseline` (`"auto"`,
#' `"drop"`, `"valley"`, `"tangent"`; `"linear"` or `"none"` under windows), `area_seconds`.
#' `rt` and `window` (minutes) pick the peak at an expected retention time (`pick`:
#' `"largest"` or `"nearest"`); `integrate` is a two-column matrix of manual windows (minutes);
#' `x_range` a two-column matrix of windows on a spectrum's own axis (IR/Raman cm^-1, UV-Vis
#' nm, NMR ppm); `compounds` a data frame of targets (`name` and `mz`, `q1`+`q3` or `trace`,
#' optionally `rt`, `window`, `ppm`, ...); `summary_only` leaves out the peak tables.
#'
#' Returns one row per peak (`chromatogram`, `area_unit`, `number`, `rt_min`, `start_min`,
#' `end_min`, `height`, `area`, `area_percent`, widths, `tailing_factor`, `asymmetry_factor`,
#' `plates`, `resolution`, `snr`, ...), or for a spectrum one row per band (`spectrum`, `x`,
#' `start`, `end`, `height`, `area`, ...). Attributes: `chromatograms`, `picked`, `manual`,
#' `compounds`, `regions` (data frames when asked for) and `notes`.
#'
#' @section `kind = "qpcr"`:
#' A qPCR run (RDML, Applied Biosystems `.eds`, Qiagen Rotor-Gene `.rex`; `x` is a path): one
#' record per well and target with sample, target, dye, task, quantity, the instrument's Cq,
#' replicate statistics and melting temperatures. Options: `well`, `target`, `sample`, `run`
#' (only these records); `compute_cq` (recompute Cq and compare; `cq_method` is "threshold",
#' "stored-threshold" or "second-derivative", with `threshold` and `baseline = c(first, last)`
#' cycles for the threshold methods); `ddcq` (relative quantities against
#' `reference_targets` and `control_sample`); `standard_curve` (slope, R², efficiency per
#' target); `max_records`; `undetermined_cq` (the Cq at which undetermined wells count).
#'
#' Returns a data frame of records with an attribute `report` (instrument, program,
#' `cq_comparison`, `relative_quantities`, `standard_curves`, notes).
#'
#' @section Other kinds:
#' `"nmr-peaks"` (NMR peak list and region integrals: `from` = `"auto"`, `"fid"` or
#' `"processed"`, `phase`, `lb`, `baseline`, `min_snr`, `range_ppm`, `integrate` (a two-column
#' matrix in ppm), `integral_reference`, ...), `"ephys-features"` (patch clamp: `trace`,
#' `channel`, `sweeps`, `peak_threshold_mv`, `dvdt_threshold`, `max_spikes`), `"spikes"`
#' (extracellular threshold crossings: `trace`, `channels`, `sweeps`, `band_hz`, `threshold`,
#' `sign`, ...), `"assay"` (plate-reader assays, `x` a path: `analysis` = `"wells"`,
#' `"curve"`, `"dose-response"`, `"kinetics"`, `"growth"` or `"qc"`, `layout` (a plate-map
#' CSV), `standards`, `blank_wells`, `model`, `weighting`, `normalize`, `plot`, ...) and
#' `"gate"` (flow cytometry, `x` the FCS file: `workspace` (FlowJo `.wsp`) or `gatingml`,
#' `sample`, `populations`, `medians`, `table`) return the CLI's JSON as a list. With
#' `kind = "assay"` and `plot = TRUE`, attribute `plot_png` holds the fitted curve as a PNG
#' (a raw vector).
#'
#' @param x A path or an [openreadout_open()] file (`"qpcr"`, `"assay"` and `"gate"` read the
#'   file by its path).
#' @param kind The analysis: `"peaks"`, `"chromatogram"`, `"nmr-peaks"`, `"ephys-features"`,
#'   `"spikes"`, `"qpcr"`, `"assay"` or `"gate"`.
#' @param ... The options of `kind` (see the sections).
#' @return Depends on `kind` (see the sections).
#' @examples
#' \dontrun{
#' xic <- openreadout_analyze("run.raw", "chromatogram", mz = c(195.0877, 138.0662), ppm = 5)
#' pk <- openreadout_analyze("run.D", "peaks", traces = 1)
#' pk[order(-pk$area), c("rt_min", "area", "area_percent")]
#' q <- openreadout_analyze("run.raw", "peaks", mz = 195.0877, rt = 5.3, window = 0.2)
#' attr(q, "picked")
#' rq <- openreadout_analyze("plate.eds", "qpcr", ddcq = TRUE)
#' nmr <- openreadout_analyze("nmr/1", "nmr-peaks", from = "fid")
#' ic50 <- openreadout_analyze("dose.csv", "assay", analysis = "dose-response",
#'                             layout = "map.csv")
#' }
#' @export
openreadout_analyze <- function(x, kind, ...) {
  if (missing(kind) || !is.character(kind) || length(kind) != 1 || !kind %in% .analyze_kinds) {
    .openreadout_abort(sprintf("kind must be one of %s", paste(.analyze_kinds, collapse = ", ")))
  }
  path_of <- function(x) if (inherits(x, "openreadout_file")) x$path else .path(x)
  switch(kind,
    chromatogram = .analyze_chromatogram(x, ...),
    peaks = .analyze_peaks(x, ...),
    qpcr = .analyze_qpcr(path_of(x), ...),
    assay = ,
    gate = {
      res <- .ic(rs_analyze(path_of(x), kind, .analyze_options(list(...))))
      out <- .from_json(res$json)
      if (!is.null(res$png)) attr(out, "plot_png") <- res$png
      out
    },
    .with_file(x, function(h) {
      .from_json(.ic(rs_analyze_dataset(.ptr(h), kind, .analyze_options(list(...)))))
    })
  )
}
