# Many files: batch tables, group summaries and sample links (https://openreadout.github.io/openreadout/guides/batch.html).

# A data frame from the {columns, rows} of a batch or summary table, typed from the column list.
.typed_table <- function(columns, rows) {
  names <- vapply(columns, function(c) c$name, "")
  cols <- lapply(seq_along(columns), function(j) {
    vals <- lapply(rows, function(r) r[[j]])
    vals[vapply(vals, function(v) is.null(v) || length(v) == 0, logical(1))] <- list(NA)
    v <- unlist(vals, use.names = FALSE)
    switch(columns[[j]]$type,
      float = as.numeric(v),
      integer = {
        v <- as.numeric(v)
        if (all(is.na(v) | abs(v) < .Machine$integer.max)) as.integer(v) else v
      },
      boolean = as.logical(v),
      as.character(v)
    )
  })
  names(cols) <- names
  df <- .frame(cols)
  attr(df, "columns") <- .records(lapply(columns, function(c) lapply(c, unlist)))
  df
}

.idx_or_null <- function(v, what) if (is.null(v)) NULL else .index0(v, what)

#' One tidy table over many files
#'
#' Runs one measurement over files, directories or glob patterns and returns one tidy table:
#' one row per file (and per image and channel, trace, sweep and channel, FCS parameter, well,
#' gate population, ...), joined to sample sheets or plate layouts, optionally summarized by
#' group. This is `openreadout stats|trace|table|analyze gate|info --tidy` and the
#' `openreadout_batch` MCP tool.
#'
#' @param measure `"stats"` (pixel statistics per image and channel), `"trace"` (per trace,
#'   sweep and channel), `"table"` (FCS: per parameter events, median, mean, sd, min, max; plate
#'   reads: one row per well), `"gate"` (FlowJo/Gating-ML populations: count, % parent, % total,
#'   medians) or `"info"` (one row of header metadata per file); or `"summarize"`: the group
#'   summary (`by`, `values`, `replicate`, `test`, `control`, `where`) of a table file written
#'   with `output` (or any CSV, TSV, JSON Lines, JSON or Parquet table), `inputs` being that
#'   file (`openreadout summarize TABLE`).
#' @param inputs Files, directories or glob patterns.
#' @param recursive Walk sub-directories.
#' @param sample_sheets Sample sheets (CSV, TSV, XLSX) or plate layouts to join. The key is chosen
#'   from the data (file name, path, sample id recorded in the file, well, barcode, ...) and
#'   reported in the `joins` attribute.
#' @param where Row filters `"COLUMN=VALUE"` / `"COLUMN!=VALUE"`, e.g. `"parameter=FITC-A"`.
#' @param by Summarize by these columns (n, mean, sd, sem, median, min, max, CV % per group), in
#'   the `summary` attribute.
#' @param values Value columns to summarize (default: the measure's main values).
#' @param replicate Average rows within each replicate first (a column name).
#' @param test,control `"welch"` or `"mann-whitney"` against the `control` group (a value of the
#'   first `by` column).
#' @param image,trace,sweep,table Restrict to one image, trace, sweep or FCS data set (1-based).
#' @param channels Trace channels (1-based).
#' @param select Plane selection for `stats` (`"c=0"`, `"z=2-5"`: zero-based, as in the CLI).
#' @param per `stats` rows per `"channel"` (default), `"image"` or `"plane"`.
#' @param parameters FCS parameters (`$PnN` or `$PnS`) for `table`.
#' @param compensate,transform FCS compensation (`"auto"`, `"fcs"`, `"gating"`) and transform
#'   (`"logicle"`, `"arcsinh-cofactor:5"`, ...).
#' @param workspace,gatingml FlowJo workspace (`.wsp`) or Gating-ML file for `gate`.
#' @param populations,medians `gate`: only these populations; parameters whose median per
#'   population is reported.
#' @param output Also write the full table (or the summary, with `by`) to a `.csv`, `.tsv`,
#'   `.jsonl`, `.json` or `.parquet` file (verified; `overwrite` to replace).
#' @param overwrite Replace an existing `output`.
#' @param ... Other fields of the `openreadout_batch` request (`formats`, `worksheet`, `keys`,
#'   `fields`, `from_index`, `query`, `sample`).
#' @return A data frame, one row per measured item, with attributes `summary` (a data frame, with
#'   `by`), `columns` (name, type, unit, role, description), `joins`, `inputs`, `warnings`.
#'   For `"summarize"`, a data frame of group statistics with an attribute `info`.
#' @examples
#' \dontrun{
#' res <- openreadout_batch("table", "runs/", sample_sheets = "samples.csv",
#'                 where = "parameter=FITC-A", by = "condition")
#' attr(res, "summary")
#' openreadout_batch("stats", "plate/", recursive = TRUE, sample_sheets = "layout.xlsx",
#'          by = "condition", replicate = "well", test = "welch", control = "DMSO")
#' openreadout_batch("summarize", "rows.parquet", by = c("condition", "dose"), values = "mean")
#' }
#' @export
openreadout_batch <- function(measure, inputs, recursive = FALSE, sample_sheets = NULL, where = NULL,
                     by = NULL, values = NULL, replicate = NULL, test = NULL, control = NULL,
                     image = NULL, trace = NULL, sweep = NULL, table = NULL, channels = NULL,
                     select = NULL, per = NULL, parameters = NULL, compensate = NULL,
                     transform = NULL, workspace = NULL, gatingml = NULL, populations = NULL,
                     medians = NULL, output = NULL, overwrite = FALSE, ...) {
  measure <- match.arg(measure, c("stats", "trace", "table", "gate", "info", "summarize"))
  if (measure == "summarize") {
    if (length(inputs) != 1) .openreadout_abort("measure \"summarize\" takes one table file")
    if (is.null(by)) .openreadout_abort("measure \"summarize\" needs by")
    req <- list(table = path.expand(inputs), by = I(as.character(by)),
                values = if (!is.null(values)) I(as.character(values)), replicate = replicate,
                test = test, control = control,
                where = if (!is.null(where)) I(as.character(where)))
    out <- .from_json_raw(.ic(rs_summarize(.to_json(req))))
    s <- out$summary
    df <- .typed_table(s$table$columns, s$table$rows)
    attr(df, "info") <- s[setdiff(names(s), "table")]
    return(df)
  }
  arr <- function(v) if (is.null(v)) NULL else I(as.character(v))
  req <- c(list(
    measure = measure,
    inputs = I(path.expand(as.character(inputs))),
    recursive = isTRUE(recursive),
    sample_sheets = if (!is.null(sample_sheets)) I(path.expand(as.character(sample_sheets))),
    where = arr(where), by = arr(by), values = arr(values),
    replicate = replicate, test = test, control = control,
    image = .idx_or_null(image, "image"), trace = .idx_or_null(trace, "trace"),
    sweep = .idx_or_null(sweep, "sweep"), table = .idx_or_null(table, "table"),
    channels = if (!is.null(channels)) I(vapply(channels, .index0, 0L, what = "channels")),
    select = arr(select), per = per, parameters = arr(parameters),
    compensate = compensate, transform = transform,
    workspace = if (!is.null(workspace)) path.expand(workspace),
    gatingml = if (!is.null(gatingml)) path.expand(gatingml),
    populations = arr(populations), medians = arr(medians),
    output = if (!is.null(output)) path.expand(output), overwrite = isTRUE(overwrite)
  ), list(...))
  out <- .from_json_raw(.ic(rs_batch(.to_json(req))))
  df <- .typed_table(out$columns, out$rows)
  if (!is.null(out$summary)) {
    s <- out$summary
    sm <- .typed_table(s$table$columns, s$table$rows)
    attr(sm, "info") <- s[setdiff(names(s), "table")]
    attr(df, "summary") <- sm
  }
  attr(df, "measure") <- out$measure
  attr(df, "grain") <- unlist(out$grain)
  attr(df, "joins") <- out$joins
  attr(df, "inputs") <- out$inputs
  attr(df, "warnings") <- unlist(out$warnings)
  attr(df, "output") <- out$output
  if (length(out$warnings)) {
    for (w in utils::head(unlist(out$warnings), 5)) message("openreadout: ", w)
  }
  df
}

#' Files of the same sample
#'
#' Finds files that measured the same sample across instruments and formats (sample ids, names,
#' barcodes, times recorded in the files), with the evidence and a confidence per link.
#'
#' @param paths Files or directories.
#' @param min_confidence `"low"`, `"medium"` (default) or `"high"`.
#' @param formats Only these format ids.
#' @return A list with `groups` (linked files and evidence) and `files`.
#' @export
openreadout_link <- function(paths, min_confidence = NULL, formats = NULL) {
  req <- list(paths = I(path.expand(as.character(paths))), min_confidence = min_confidence,
              formats = if (!is.null(formats)) I(as.character(formats)))
  .from_json(.ic(rs_link(.to_json(req))))
}

#' Pixel statistics
#'
#' `openreadout stats`: count, min, max, mean, sd, percentiles, zero and saturated fractions of
#' the pixels, one row per image and channel (`per = "channel"`, the default), per image or per
#' plane; or, for a high-content screening plate (Harmony, ImageXpress, CellVoyager, OME-Zarr
#' plates), one row per well and channel (`per = "well"`) or per well, field and channel
#' (`per = "field"`): mean, sd, min, max, p1, median, p99, planes and missing planes.
#'
#' The `image`, `c`, `z` and `t` columns start at 1, like the arguments of
#' [openreadout_read_image()].
#'
#' @param x A path (for a plate, the plate folder or index file) or an [openreadout_open()]
#'   file.
#' @param image Only this image (1 = the first; default all).
#' @param select Plane selection (`"c=0"`, `"z=2-5"`: zero-based, as in the CLI).
#' @param per `"channel"` (default), `"image"`, `"plane"`, `"well"` or `"field"`.
#' @param wells With `per = "well"` or `"field"`: only these wells (`"C05"`).
#' @return A data frame. Pixel statistics carry an attribute `stats` with the rest of the
#'   output; plate statistics an attribute `plate`.
#' @examples
#' \dontrun{
#' openreadout_stats("run42.czi")
#' openreadout_stats("plate/", per = "well", select = "c=0", wells = c("A01", "A02"))
#' }
#' @export
openreadout_stats <- function(x, image = NULL, select = character(),
                              per = c("channel", "image", "plane", "well", "field"),
                              wells = character()) {
  per <- match.arg(per)
  if (per %in% c("well", "field")) {
    return(.with_file(x, function(h) {
      out <- .from_json(.ic(rs_well_stats(.ptr(h), as.character(select), as.character(wells),
                                          per == "field")))
      df <- .records(out$rows)
      attr(df, "plate") <- out$plate
      df
    }))
  }
  if (length(wells)) {
    .openreadout_abort("wells goes with per = \"well\" or \"field\"")
  }
  image0 <- if (is.null(image)) -1L else .index0(image, "image")
  .with_file(x, function(h) {
    out <- .from_json(.ic(rs_stats(.ptr(h), image0, as.character(select), per == "plane")))
    key <- paste0(per, "s")
    rows <- lapply(out[[key]], function(r) {
      flat <- c(r[setdiff(names(r), "stats")], r$stats)
      for (k in intersect(names(flat), c("image", "c", "z", "t"))) flat[[k]] <- flat[[k]] + 1L
      flat
    })
    df <- .records(rows)
    out[c("planes", "channels", "images")] <- NULL
    attr(df, "stats") <- out
    df
  })
}
