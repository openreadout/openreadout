# Tables, traces and spectra: the columns of `openreadout export --format parquet` as data frames.

.columnar <- function(h, kind, index, sweep = NULL, first_row = 0, last_row = NULL,
                      centroid = FALSE, max_rows = NULL) {
  res <- .ic(rs_columnar(.ptr(h), kind, as.integer(index), if (is.null(sweep)) NULL else as.integer(sweep),
                         as.double(first_row), if (is.null(last_row)) NULL else as.double(last_row),
                         isTRUE(centroid), if (is.null(max_rows)) NULL else as.double(max_rows)))
  meta <- .from_json(res$meta)
  df <- .frame(res$columns)
  cols <- meta$columns
  pick <- function(k) {
    v <- vapply(cols, function(cl) if (is.null(cl[[k]])) NA_character_ else as.character(cl[[k]]), "")
    names(v) <- vapply(cols, function(cl) cl$name, "")
    v
  }
  attr(df, "units") <- pick("unit")
  attr(df, "labels") <- pick("label")
  attr(df, "dtypes") <- pick("dtype")
  df
}

.rows <- function(first, n_max, what) {
  if (!is.numeric(first) || length(first) != 1 || is.na(first) || first < 1 || first != round(first)) {
    .openreadout_abort(sprintf("%s must be a single whole number >= 1", what), call = sys.call(-2))
  }
  if (!is.numeric(n_max) || length(n_max) != 1 || is.na(n_max) || n_max < 0) {
    .openreadout_abort("n_max must be a single number >= 0 (Inf for all)", call = sys.call(-2))
  }
  first0 <- first - 1
  last0 <- if (is.infinite(n_max)) NULL else first0 + n_max - 1
  list(first = first0, last = last0)
}

#' Read a table as a data frame
#'
#' Reads a table of the file: the events of an FCS data set (one column per parameter, `$PnN`
#' names, raw values: neither compensated nor scaled), a plate-reader export (one row per well
#' and read, `well` as a factor), a chromatography peak table, or an event/spike table
#' (Neuralynx, Blackrock, Plexon, NWB). Columns keep their values exactly (8/16-bit integers as
#' integer, 32-bit floats and wider as double).
#'
#' @param x A path or an [openreadout_open()] file.
#' @param table Which table (1 = the first; `openreadout_info(x)$tables`).
#' @param first_row First row to read (1 = the first).
#' @param n_max Most rows to read (`Inf`: to the end).
#' @return A data frame (a tibble when the tibble package is installed; set
#'   `options(openreadout.tibble = FALSE)` for a plain data frame) with attributes `units`,
#'   `labels` (FCS `$PnS`) and `dtypes` (the stored types), named by column.
#' @examples
#' \dontrun{
#' ev <- openreadout_table("tube1.fcs")
#' summary(ev$`FSC-A`)
#' attr(ev, "labels")
#' plate <- openreadout_table("plate.xlsx")
#' }
#' @export
openreadout_table <- function(x, table = 1, first_row = 1, n_max = Inf) {
  i <- .index0(table, "table")
  r <- .rows(first_row, n_max, "first_row")
  .with_file(x, function(h) .columnar(h, "table", i, first_row = r$first, last_row = r$last))
}

#' Read a trace (signal) as a data frame
#'
#' Reads a trace: electrophysiology sweeps (ABF, Neuralynx, Blackrock, SpikeGLX, Intan, NWB),
#' chromatograms and detector signals (ChemStation, ANDI, Waters, Shimadzu, stored TIC/SRM of
#' mzML), NMR FIDs and spectra (Bruker, Varian, JEOL), JCAMP-DX, FT-IR and Raman spectra. One row
#' per sample: a `sweep` column (1 = the first sweep, as the `sweep` argument), the abscissa (`time_s`, or the
#' spectrum's axis such as `chemical_shift_ppm` or `wavenumber_cm-1`) and one double column per
#' channel in physical units (mV, pA, mAU, ...).
#'
#' @param x A path or an [openreadout_open()] file.
#' @param trace Which trace (1 = the first; `openreadout_info(x)$traces`).
#' @param sweep One sweep (1 = the first), or `NULL` for every sweep.
#' @param first_sample First sample of each sweep to read (1 = the first).
#' @param n_max Most samples per sweep (`Inf`: to the end).
#' @return A data frame with attributes `units` and `labels` named by column.
#' @examples
#' \dontrun{
#' tr <- openreadout_trace("cell3.abf", sweep = 3)
#' plot(tr$time_s, tr[[3]], type = "l", ylab = attr(tr, "units")[[3]])
#' uv <- openreadout_trace("run.D", trace = 2)
#' }
#' @export
openreadout_trace <- function(x, trace = 1, sweep = NULL, first_sample = 1, n_max = Inf) {
  i <- .index0(trace, "trace")
  s <- if (is.null(sweep)) NULL else .index0(sweep, "sweep")
  r <- .rows(first_sample, n_max, "first_sample")
  .with_file(x, function(h) {
    df <- .columnar(h, "trace", i, sweep = s, first_row = r$first, last_row = r$last)
    if ("sweep" %in% names(df)) {
      # 1-based, like the `sweep` argument (the file's sweep 0 is sweep 1 here)
      df[["sweep"]] <- as.integer(df[["sweep"]]) + 1L
    }
    df
  })
}

#' Mass spectra
#'
#' `openreadout spectra`: the scan headers of a mass-spectrometry run (Thermo `.raw`, mzML,
#' mzXML, imzML, Bruker timsTOF `.d`, Agilent MassHunter `.d`, Waters `.raw`, Sciex `.wiff`)
#' without decoding any peaks, or, with `scan` or `index`, one spectrum's m/z and intensity
#' arrays.
#'
#' @param x A path or an [openreadout_open()] file.
#' @param scan The instrument's scan number (as in the vendor software and mzML ids).
#' @param index Position of the spectrum in the run (1 = the first).
#' @param run Which spectra run (1 = the first; `openreadout_info(x)$spectra`).
#' @param centroid With `scan` or `index`: return the instrument's centroid list where a scan
#'   stores both profile and centroids.
#' @param ms_level,polarity,rt_range,precursor_mz,ppm,charge,activation,scan_filter Scan
#'   headers only of MS level `ms_level`, polarity `"positive"` or `"negative"`, retention
#'   times within `rt_range` (`c(start, end)`, minutes), precursors within `ppm` of
#'   `precursor_mz`, precursor charge `charge`, activation `"HCD"`, `"CID"`, ..., and filter
#'   strings containing `scan_filter`.
#' @param offset,limit Skip the first `offset` matching scans; list at most `limit` (default
#'   all). Every match is counted.
#' @return Without `scan` and `index`: a data frame with one row per scan (`index`,
#'   `scan_number`, `ms_level`, `rt_s`, `polarity`, `precursor_mz`, `precursor_charge`,
#'   `isolation_window_mz`, `activation`, `collision_energy`, `scan_filter`, ...) and an
#'   attribute `scans` with the counts (`matched`, `ms_level_counts`, `rt_range_s`, ...).
#'   With one of them: a data frame with columns `mz` and `intensity`, and an attribute
#'   `spectrum`, a list with `index`, `scan_number`, `ms_level`, `rt_s` (retention time in
#'   seconds; `NA` when the file states none for the scan), `polarity`, `centroided`,
#'   `precursor_mz`, `precursor_charge`, `scan_filter`, `total_ion_current`, ...
#' @examples
#' \dontrun{
#' ms2 <- openreadout_spectra("sample.raw", ms_level = 2)
#' table(ms2$precursor_charge)
#' sp <- openreadout_spectra("sample.raw", scan = 1200)
#' attr(sp, "spectrum")$ms_level
#' plot(sp$mz, sp$intensity, type = "h")
#' }
#' @export
openreadout_spectra <- function(x, scan = NULL, index = NULL, run = 1, centroid = FALSE,
                                ms_level = NULL, polarity = NULL, rt_range = NULL,
                                precursor_mz = NULL, ppm = NULL, charge = NULL,
                                activation = NULL, scan_filter = NULL, offset = 0,
                                limit = NULL) {
  if (!is.null(scan) && !is.null(index)) {
    .openreadout_abort("give at most one of scan (the instrument's scan number) and index (1 = the first spectrum)")
  }
  run0 <- .index0(run, "run")
  if (is.null(scan) && is.null(index)) {
    filt <- list(
      ms_level = ms_level, polarity = if (!is.null(polarity)) tolower(polarity),
      rt_min_s = if (!is.null(rt_range)) as.numeric(rt_range[1]) * 60,
      rt_max_s = if (!is.null(rt_range)) as.numeric(rt_range[2]) * 60,
      precursor_mz = precursor_mz, precursor_tol_ppm = ppm, charge = charge,
      activation = activation, filter_contains = scan_filter
    )
    offset <- .count(offset, "offset", allow_null = FALSE)
    limit <- .count(limit, "limit")
    return(.with_file(x, function(h) {
      out <- .from_json(.ic(rs_scans(.ptr(h), run0, .to_json(filt), as.double(offset),
                                     if (is.null(limit)) -1 else as.double(limit))))
      df <- .records(out$scans)
      out$scans <- NULL
      attr(df, "scans") <- out
      df
    }))
  }
  number <- if (is.null(scan)) .index0(index, "index") else .count(scan, "scan", allow_null = FALSE)
  .with_file(x, function(h) {
    res <- .ic(rs_spectrum(.ptr(h), run0, as.double(number), !is.null(scan), isTRUE(centroid)))
    df <- .frame(list(mz = res$mz, intensity = res$intensity))
    meta <- .from_json(res$meta)
    # JSON null (no retention time stated for the scan) reads as NULL: keep the key, as NA
    if (is.null(meta$rt_s)) meta["rt_s"] <- list(NA_real_)
    attr(df, "spectrum") <- meta
    df
  })
}
