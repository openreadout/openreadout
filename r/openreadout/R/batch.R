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
#'   medians) or `"info"` (one row of header metadata per file). To summarize a table written
#'   with `output`, use [openreadout_summarize()].
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
#' @examples
#' \dontrun{
#' res <- openreadout_batch("table", "runs/", sample_sheets = "samples.csv",
#'                 where = "parameter=FITC-A", by = "condition")
#' attr(res, "summary")
#' openreadout_batch("stats", "plate/", recursive = TRUE, sample_sheets = "layout.xlsx",
#'          by = "condition", replicate = "well", test = "welch", control = "DMSO")
#' }
#' @export
openreadout_batch <- function(measure, inputs, recursive = FALSE, sample_sheets = NULL, where = NULL,
                     by = NULL, values = NULL, replicate = NULL, test = NULL, control = NULL,
                     image = NULL, trace = NULL, sweep = NULL, table = NULL, channels = NULL,
                     select = NULL, per = NULL, parameters = NULL, compensate = NULL,
                     transform = NULL, workspace = NULL, gatingml = NULL, populations = NULL,
                     medians = NULL, output = NULL, overwrite = FALSE, ...) {
  measure <- match.arg(measure, c("stats", "trace", "table", "gate", "info"))
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
#' `line`, `polygon` and `objects` measure one plane instead (`stats --line`, `--polygon`,
#' `--objects`; by default channel 1, the middle z and the first time point, or what `select`
#' picks). `line` and `polygon` are in pixel coordinates as the rulers of
#' [openreadout_preview()] show them (starting at 0) at pyramid `level`:
#'
#' - `line = c(x0, y0, x1, y1)`: the intensity profile along the line per channel (min, max and
#'   where, mean, FWHM, and the profile `values`), `line_width` pixels wide;
#' - `polygon`: a two-column matrix of vertices; the area and the mean, sd, median, min, max
#'   and sum per channel inside;
#' - `objects = TRUE` counts nuclei in `region` (`c(x, y, width, height)`, `x` and `y` from 1
#'   as in [openreadout_read_image()]), inside `polygon` or in the whole plane: how many, their
#'   sizes, centroids and mean in every channel, and how many are positive per channel. A list
#'   sets options: `kind` (`"nuclei"`, `"cells"`, `"spots"`), `diameter_um`, `threshold`,
#'   `method`, `polarity`, `stain` (RGB), `split`, `min_area_um2`, `max_area_um2`, `max_objects`.
#'
#' Areas and lengths are also given in µm when the pixel size is known. The result is the
#' `stats --json` data as a list (`profile`, `roi`, `objects`); `objects$objects` is a data frame.
#'
#' @param x A path (for a plate, the plate folder or index file) or an [openreadout_open()]
#'   file.
#' @param image Only this image (1 = the first; default all).
#' @param select Plane selection (`"c=0"`, `"z=2-5"`: zero-based, as in the CLI).
#' @param per `"channel"` (default), `"image"`, `"plane"`, `"well"` or `"field"`.
#' @param wells With `per = "well"` or `"field"`: only these wells (`"C05"`).
#' @param line,line_width,polygon,objects Measurements on one plane (see Details).
#' @param level,region Pyramid level and rectangle of the measurements.
#' @return A data frame. Pixel statistics carry an attribute `stats` with the rest of the
#'   output; plate statistics an attribute `plate`.
#' @examples
#' \dontrun{
#' openreadout_stats("run42.czi")
#' openreadout_stats("plate/", per = "well", select = "c=0", wells = c("A01", "A02"))
#' openreadout_stats("nuclei.czi", objects = TRUE)$objects$count
#' openreadout_stats("run42.czi", line = c(10, 200, 400, 200), line_width = 3)$profile
#' }
#' @export
openreadout_stats <- function(x, image = NULL, select = character(),
                              per = c("channel", "image", "plane", "well", "field"),
                              wells = character(), line = NULL, line_width = NULL,
                              polygon = NULL, objects = NULL, level = 0, region = NULL) {
  if (!is.null(line) || !is.null(polygon) || isTRUE(objects) || is.list(objects)) {
    return(.measure(x, image, select, line, line_width, polygon, objects, level, region))
  }
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

#' Group statistics of a saved table
#'
#' Summarizes a table written by [openreadout_batch()] with `output` (or any CSV, TSV, JSON
#' Lines, JSON or Parquet table) by the columns `by`. This is `openreadout summarize TABLE` and
#' the `openreadout_summarize` MCP tool.
#'
#' @param table The table file.
#' @param by Group by these columns (n, mean, sd, sem, median, min, max, CV % per group).
#' @param values Value columns to summarize (default: every numeric column).
#' @param replicate Average rows within each replicate first (a column name).
#' @param test,control `"welch"` or `"mann-whitney"` against the `control` group (a value of the
#'   first `by` column).
#' @param where Row filters `"COLUMN=VALUE"` / `"COLUMN!=VALUE"`.
#' @return A data frame of group statistics with an attribute `info`.
#' @examples
#' \dontrun{
#' openreadout_summarize("rows.parquet", by = c("condition", "dose"), values = "mean")
#' }
#' @export
openreadout_summarize <- function(table, by, values = NULL, replicate = NULL, test = NULL,
                                  control = NULL, where = NULL) {
  if (length(table) != 1) .openreadout_abort("openreadout_summarize() takes one table file")
  req <- list(table = path.expand(table), by = I(as.character(by)),
              values = if (!is.null(values)) I(as.character(values)), replicate = replicate,
              test = test, control = control,
              where = if (!is.null(where)) I(as.character(where)))
  out <- .from_json_raw(.ic(rs_summarize(.to_json(req))))
  s <- out$summary
  df <- .typed_table(s$table$columns, s$table$rows)
  attr(df, "info") <- s[setdiff(names(s), "table")]
  df
}

.measure <- function(x, image, select, line, line_width, polygon, objects, level, region) {
  if (!is.null(line) && (!is.numeric(line) || length(line) != 4 || anyNA(line))) {
    .openreadout_abort("line must be c(x0, y0, x1, y1)", call = sys.call(-1))
  }
  if (!is.null(polygon)) {
    polygon <- as.matrix(polygon)
    if (!is.numeric(polygon) || ncol(polygon) != 2 || nrow(polygon) < 3 || anyNA(polygon)) {
      .openreadout_abort("polygon must be a two-column matrix of at least three vertices",
                         call = sys.call(-1))
    }
  }
  if (!is.null(region)) {
    if (!is.numeric(region) || length(region) != 4 || anyNA(region) || any(region < 1)) {
      .openreadout_abort("region must be c(x, y, width, height), x and y from 1", call = sys.call(-1))
    }
    region <- list(x = region[1] - 1, y = region[2] - 1, width = region[3], height = region[4])
  }
  req <- list(
    image = if (!is.null(image)) .index0(image, "image"),
    select = I(as.character(select)),
    level = .count(level, "level"),
    region = region,
    line = if (!is.null(line)) I(as.numeric(line)),
    line_width = line_width,
    polygon = polygon,
    objects = if (isTRUE(objects) || (is.list(objects) && length(objects) == 0)) {
      structure(list(), names = character())
    } else {
      objects
    }
  )
  out <- .with_file(x, function(h) .from_json(.ic(rs_measure(.ptr(h), .to_json(req)))))
  if (!is.null(out$objects$objects)) {
    out$objects$objects <- .records(out$objects$objects)
  }
  out
}
