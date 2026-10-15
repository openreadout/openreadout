# Pictures: openreadout_preview() (`openreadout preview`) and openreadout_plot()
# (`openreadout plot`). Both return an `openreadout_picture`: the encoded bytes with what was
# drawn, which plot() draws on the current graphics device and which can be written to a file.

# Preview options that hold one index or a list of indices (1-based in R).
.preview_index_options <- c("image", "trace", "sweep", "run", "spectrum", "table")
.preview_index_list_options <- c("channels")
# Preview options that are JSON arrays even with one element.
.preview_array_options <- c("select", "columns", "channels")

.preview_options <- function(opts) {
  opts <- opts[!vapply(opts, is.null, logical(1))]
  for (k in intersect(names(opts), .preview_index_options)) {
    opts[[k]] <- .index0(opts[[k]], k)
  }
  for (k in intersect(names(opts), .preview_index_list_options)) {
    opts[[k]] <- vapply(opts[[k]], .index0, 0L, what = k)
  }
  if (!is.null(opts$region)) {
    r <- opts$region
    if (!is.numeric(r) || length(r) != 4 || anyNA(r) || any(r != round(r)) || any(r < 1)) {
      .openreadout_abort("region must be c(x, y, width, height): whole numbers, x and y from 1",
                         call = sys.call(-1))
    }
    opts$region <- list(x = r[1] - 1, y = r[2] - 1, width = r[3], height = r[4])
  }
  if (!is.null(opts$annotations)) {
    opts$annotations <- .annotations(opts$annotations)
  }
  for (k in intersect(names(opts), .preview_array_options)) {
    opts[[k]] <- I(opts[[k]])
  }
  opts
}

# Annotations: a list of lists, or a data frame with one row per annotation.
.annotations <- function(a) {
  if (is.data.frame(a)) {
    a <- lapply(seq_len(nrow(a)), function(i) {
      row <- as.list(a[i, , drop = FALSE])
      row[!vapply(row, function(v) length(v) == 1 && is.na(v), logical(1))]
    })
  }
  if (!is.list(a)) {
    .openreadout_abort("annotations must be a list of lists or a data frame", call = sys.call(-2))
  }
  lapply(a, function(x) {
    if (!is.null(x$points)) x$points <- matrix(as.numeric(x$points), ncol = 2)
    x
  })
}

# Write `data` to a temporary file beside `file`, read it back and compare, then rename it into
# place.
.write_verified <- function(data, file, overwrite, inputs = character()) {
  file <- path.expand(file)
  # never over a file the picture was drawn from, however its path is spelled
  inputs <- unlist(inputs)
  inputs <- path.expand(inputs[file.exists(inputs)])
  if (file.exists(file) && length(inputs) > 0 &&
      normalizePath(file) %in% normalizePath(inputs)) {
    .openreadout_abort(sprintf("%s is an input file; raw files are never modified", file),
                       call = sys.call(-1))
  }
  if (file.exists(file) && !isTRUE(overwrite)) {
    .openreadout_abort(sprintf("%s exists; pass overwrite = TRUE to replace it", file),
                       call = sys.call(-1))
  }
  tmp <- tempfile(".openreadout-", tmpdir = dirname(normalizePath(file, mustWork = FALSE)))
  on.exit(if (file.exists(tmp)) unlink(tmp), add = TRUE)
  writeBin(data, tmp)
  back <- readBin(tmp, "raw", n = length(data) + 1L)
  if (!identical(back, data)) {
    stop(structure(
      class = c("openreadout_io_error", "openreadout_error", "error", "condition"),
      list(message = sprintf("read-back of %s differs from what was written", file),
           call = sys.call(-1), code = "io", exit_code = 5L, hint = "Check the disk and try again.")
    ))
  }
  if (!file.rename(tmp, file)) {
    .openreadout_abort(sprintf("could not move the picture into place at %s", file), call = sys.call(-1))
  }
  invisible(file)
}

.picture <- function(res, format, kind) {
  structure(list(data = res$data, format = format, meta = .from_json(res$json)),
            class = c(paste0("openreadout_", kind), "openreadout_picture"))
}

#' A picture of a file
#'
#' `openreadout preview`, as the agent tool `openreadout_preview` draws it: an image plane with
#' rulers in file pixels and a scale bar, a trace or chromatogram with its largest peaks
#' labelled, a mass spectrum, a plate heat map or a grid of growth curves in plate positions,
#' or a flow-cytometry density plot. `plot()` draws the result on the current graphics device
#' (PNG and JPEG need the png or jpeg package); `file` writes it.
#'
#' Annotations are drawn on top in the picture's own coordinates: file pixels on images (the
#' ruler numbers, which start at 0), axis units on plots and wells on plates. Give them as a list
#' of lists or a data frame with columns `type` (point, box, polygon, line, arrow, text, vline,
#' hline, vband, hband or well), `x`, `y`, `x2`, `y2`, `width`, `height`, `label`, `color`,
#' `channel`, `well`; `points` (a two-column matrix) for polygons and lines. The result's
#' `meta$annotations` says where each one landed in picture pixels.
#'
#' @param x A path or an [openreadout_open()] file.
#' @param ... The options of `openreadout_preview`: `image`, `select` (zero-based plane
#'   selection such as `"c=1"` or `"c=0,1,z=2"`, as in the CLI), `mip` (`"z"`, `"t"`),
#'   `composite`, `level`, `region` (`c(x, y, width, height)`, `x` and `y` from 1 as in
#'   [openreadout_read_image()]), `max_size` (default 1024), `axes` (`"rulers"`, `"grid"`,
#'   `"scale-bar"`, `"none"`), `title`, `caption`, `contrast`, `lut`, `trace`, `sweep`, `channels`, `run`, `spectrum`, `scan`,
#'   `centroid`, `table`, `columns`, `marks` (`"auto"`, `"peaks"`, `"spikes"`, `"none"`),
#'   `annotations` and `dark`. Indices (`image`, `trace`, `sweep`, `channels`, `run`,
#'   `spectrum`, `table`) start at 1.
#' @param format `"png"` (default), `"jpeg"` or `"svg"`.
#' @param file Write the picture here (read back and compared before the rename).
#' @param overwrite Replace an existing `file`.
#' @return An `openreadout_preview` picture: a list with `data` (a raw vector), `format` and
#'   `meta` (the `preview --json` data: size, the image's pixel mapping, `panels`, `marks`,
#'   `annotations`, `notes`).
#' @examples
#' \dontrun{
#' p <- openreadout_preview("run42.czi", select = "c=1",
#'   annotations = list(list(type = "point", x = 812, y = 440, label = "nucleus")))
#' plot(p)
#' openreadout_preview("cell.abf", sweep = 11, marks = "spikes", file = "sweep.png")
#' }
#' @export
openreadout_preview <- function(x, ..., format = c("png", "jpeg", "svg"), file = NULL,
                                overwrite = FALSE) {
  format <- match.arg(format)
  opts <- .to_json(.preview_options(list(...)))
  pic <- .with_file(x, function(h) .picture(.ic(rs_preview(.ptr(h), opts, format)), format, "preview"))
  if (!is.null(file)) .write_verified(pic$data, file, overwrite, pic$meta$path)
  pic
}

#' A figure from lab files
#'
#' `openreadout plot`: each series names its data by `file` and a selector (`trace`, `sweep` and
#' `channel`; a chromatogram, `chromatogram = "tic"` or `mz` for an XIC, or the spectrum at
#' `rt`; plate `wells` or layout `groups`; FCS `x` and `y` parameters and `populations`; `show`
#' (`"fi"`, `"dose_response"`, `"standard_curve"`, `"melt"`, ...) for an analysis result; a
#' `table` file's columns) or gives inline `values`.
#' `panels`, `facet` (`"plate"` lays wells out in plate positions), `share`, `x`/`y` axis
#' options, `annotate` (bands, lines, labelled points, arrows) and `theme` (`"light"`,
#' `"dark"`) arrange it. The spec is the JSON of the `openreadout_plot` MCP tool; indices in it
#' start at 0, as there. Reference: <https://openreadout.github.io/openreadout/reference/commands/plot.html>.
#'
#' @param spec The spec: a list, JSON text or the path of a `.json` file; `NULL` to build it
#'   from `...` alone.
#' @param ... Top-level spec keys, added to `spec` (`data`, `title`, `panels`, `facet`, ...).
#' @param format `"png"` (default), `"svg"` or `"pdf"`.
#' @param scale PNG only: 2 (default) draws at twice the spec's width, sharp on high-density
#'   screens and in documents.
#' @param file Write the figure here (read back and compared before the rename).
#' @param overwrite Replace an existing `file`.
#' @return An `openreadout_figure` picture: a list with `data` (a raw vector), `format` and `meta`
#'   (the `plot --json` data: each panel's axes, series and callouts, `notes`, `warnings`).
#' @examples
#' \dontrun{
#' fig <- openreadout_plot(data = list(file = "run.mzML", chromatogram = "tic"), title = "Total ion current")
#' plot(fig)
#' openreadout_plot(data = list(file = "growth.pda", wells = "A1:H12"), facet = "plate",
#'                  format = "pdf", file = "plate.pdf")
#' }
#' @export
openreadout_plot <- function(spec = NULL, ..., format = c("png", "svg", "pdf"), scale = 2,
                             file = NULL, overwrite = FALSE) {
  format <- match.arg(format)
  if (is.character(spec) && length(spec) == 1 && !grepl("^\\s*\\{", spec)) {
    spec <- paste(readLines(path.expand(spec), warn = FALSE, encoding = "UTF-8"), collapse = "\n")
  }
  if (is.character(spec)) {
    spec <- jsonlite::fromJSON(spec, simplifyVector = FALSE)
  }
  if (!is.null(spec) && !is.list(spec)) {
    .openreadout_abort("spec must be a list, JSON text or a .json file")
  }
  body <- utils::modifyList(if (is.null(spec)) list() else spec, list(...))
  json <- if (length(body) == 0) "{}" else
    as.character(jsonlite::toJSON(body, auto_unbox = TRUE, null = "null", digits = NA))
  pic <- .picture(.ic(rs_plot(json, format, as.double(scale))), format, "figure")
  if (!is.null(file)) .write_verified(pic$data, file, overwrite, pic$meta$files)
  pic
}

#' @export
print.openreadout_picture <- function(x, ...) {
  m <- x$meta
  what <- if (inherits(x, "openreadout_figure")) {
    n <- length(m$panels)
    sprintf("figure%s, %d panel%s", if (!is.null(m$title)) paste0(" \"", m$title, "\"") else "",
            n, if (n == 1) "" else "s")
  } else {
    sprintf("%s preview of %s", m$kind, basename(m$path))
  }
  cat(sprintf("<openreadout %s: %d x %d px, %s, %d bytes>\n", what, m$width, m$height,
              x$format, length(x$data)))
  for (n in c(m$notes, m$warnings)) cat("  note:", n, "\n")
  invisible(x)
}

#' @export
plot.openreadout_picture <- function(x, ...) {
  img <- switch(x$format,
    png = {
      if (!requireNamespace("png", quietly = TRUE)) {
        .openreadout_abort("plot() of a PNG picture needs the png package (install.packages(\"png\"))")
      }
      png::readPNG(x$data)
    },
    jpeg = {
      if (!requireNamespace("jpeg", quietly = TRUE)) {
        .openreadout_abort("plot() of a JPEG picture needs the jpeg package (install.packages(\"jpeg\"))")
      }
      jpeg::readJPEG(x$data)
    },
    .openreadout_abort(sprintf("plot() draws PNG and JPEG pictures; write this %s with file =", x$format))
  )
  h <- dim(img)[1]
  w <- dim(img)[2]
  op <- graphics::par(mar = c(0, 0, 0, 0))
  on.exit(graphics::par(op))
  graphics::plot.new()
  graphics::plot.window(xlim = c(0, w), ylim = c(0, h), asp = 1, xaxs = "i", yaxs = "i")
  graphics::rasterImage(img, 0, 0, w, h, interpolate = TRUE)
  invisible(x)
}
