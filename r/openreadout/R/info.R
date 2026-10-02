#' Open an instrument file
#'
#' Opens a raw instrument file (or a data-set directory such as a Bruker `.d`, an Agilent
#' ChemStation `.D` or a Bruker NMR experiment) once, so that several reads do not reopen it. Every
#' function that takes `x` also accepts a path and opens it for that call only.
#'
#' The format is detected from the file's content, not its name; [openreadout_formats()] lists them.
#'
#' @param path Path to the file or data-set directory.
#' @return An `openreadout_file` object. Close it with [openreadout_close()]; it is also closed when garbage
#'   collected.
#' @examples
#' \dontrun{
#' f <- openreadout_open("run42.czi")
#' openreadout_info(f)$images[[1]]$size_x
#' img <- openreadout_read_image(f, c = 2)
#' openreadout_close(f)
#' }
#' @export
openreadout_open <- function(path) {
  path <- .path(path)
  ptr <- .ic(rs_open(path))
  state <- rs_handle_state(ptr)
  structure(
    list(ptr = ptr, path = path, format = state$format, cache = new.env(parent = emptyenv())),
    class = "openreadout_file"
  )
}

#' Close an instrument file
#'
#' Releases the file handles of an [openreadout_open()] file. Idempotent.
#'
#' @param x An `openreadout_file`.
#' @return `x`, invisibly.
#' @export
openreadout_close <- function(x) {
  if (inherits(x, "openreadout_file")) {
    rs_close(x$ptr)
    rm(list = ls(x$cache), envir = x$cache)
  }
  invisible(x)
}

#' @export
print.openreadout_file <- function(x, ...) {
  state <- rs_handle_state(x$ptr)
  cat(sprintf("<openreadout_file %s: %s%s>\n", x$format, x$path, if (isTRUE(state$open)) "" else " (closed)"))
  invisible(x)
}

.info_cached <- function(h) {
  if (is.null(h$cache$info)) {
    h$cache$info <- .from_json(.ic(rs_info(.ptr(h))))
  }
  h$cache$info
}

#' File metadata
#'
#' What a file holds, as `openreadout info --view VIEW --json` reports it. Reads headers only:
#' fast on multi-gigabyte files.
#'
#' The default view, `"summary"`, is the normalized summary: the format, and depending on the
#' file `images` (dimensions, pixel type, physical sizes, channels, objective, pyramid levels),
#' `tables` (FCS data sets, plate reads, event tables), `traces` (electrophysiology sweeps,
#' chromatograms, NMR FIDs and spectra), `spectra` (mass-spectrometry runs), `plate`, and the
#' `experiment` (sample, method, instrument, operator). The other views:
#'
#' - `"full"`: a list of `file` (the summary with per-frame records), `vendor` (the vendor's own
#'   metadata tree, names untouched; left out with `vendor = FALSE`) and `provenance` (where
#'   each normalized field came from: `spec`, `vendor-impl`, `prior-art` or `inferred`);
#' - `"structure"`: `path`, `format` and `entries`, the container's elements in file order;
#' - `"explain"`: what the file is, in sentences; `ask` answers a question in words, naming the
#'   fields it used;
#' - `"format"`: `format` (format id), `name`, `confidence` and `note`, from the file's
#'   signature only (the file is not parsed).
#'
#' Indices inside the returned list (`index`, `level`) are those of the file and start at 0;
#' the arguments of this package's functions (`image`, `c`, `table`, `trace`, ...) start at 1.
#'
#' @param x A path or an [openreadout_open()] file.
#' @param view `"summary"` (default), `"full"`, `"structure"`, `"explain"` or `"format"`.
#' @param ask With `view = "explain"`: a question to answer in words.
#' @param vendor With `view = "full"`: include the vendor metadata tree.
#' @return A list; for the summary, of class `openreadout_info` with the fields of
#'   `docs/schema/info.schema.json`.
#' @examples
#' \dontrun{
#' info <- openreadout_info("run42.czi")
#' info$format$id
#' vapply(info$images[[1]]$channels, function(ch) ch$name, "")
#' openreadout_info("run42.czi", view = "full")$vendor
#' openreadout_info("run42.czi", view = "explain", ask = "Which objective?")$answer
#' openreadout_info("unknown.bin", view = "format")$format
#' }
#' @export
openreadout_info <- function(x, view = c("summary", "full", "structure", "explain", "format"),
                             ask = NULL, vendor = TRUE) {
  view <- match.arg(view)
  if (!is.null(ask) && view != "explain") {
    .openreadout_abort("ask goes with view = \"explain\"")
  }
  if (view == "format") {
    path <- if (inherits(x, "openreadout_file")) x$path else .path(x)
    return(.from_json(.ic(rs_detect(path))))
  }
  if (view == "summary") {
    info <- .with_file(x, .info_cached)
    return(structure(info, class = c("openreadout_info", "list")))
  }
  ask <- if (is.null(ask)) "" else as.character(ask)
  .with_file(x, function(h) .from_json(.ic(rs_info_view(.ptr(h), view, ask, isTRUE(vendor)))))
}

#' @export
print.openreadout_info <- function(x, ...) {
  fmt <- x$format
  cat(sprintf("<openreadout_info %s (%s)>\n", fmt$id %||% "?", fmt$name %||% ""))
  n <- function(k) length(x[[k]])
  parts <- c(
    if (n("images")) sprintf("%d image(s)", n("images")),
    if (n("tables")) sprintf("%d table(s)", n("tables")),
    if (n("traces")) sprintf("%d trace(s)", n("traces")),
    if (n("spectra")) sprintf("%d spectra run(s)", n("spectra"))
  )
  if (length(parts)) cat(" ", paste(parts, collapse = ", "), "\n")
  for (im in x$images) {
    cat(sprintf("  image %d%s: %d x %d, z %d, c %d, t %d, %s\n", im$index + 1L,
                if (!is.null(im$name)) paste0(" (", im$name, ")") else "",
                im$size_x, im$size_y, im$size_z, im$size_c, im$size_t, im$pixel_type))
  }
  cat("  Use str(x) or x$<field> for everything.\n")
  invisible(x)
}

`%||%` <- function(a, b) if (is.null(a)) b else a

#' Supported formats
#'
#' @return A data frame: one row per format with its id, name, extensions, confidence and known
#'   gaps.
#' @export
openreadout_formats <- function() {
  .records(.from_json(.ic(rs_formats()))$formats)
}

#' Check a file's integrity
#'
#' Validates the file's structure and data (`openreadout check --json`): `ok`, and `findings`
#' (severity, code, message) for truncation, missing planes or parts and bad blocks.
#'
#' @inheritParams openreadout_info
#' @return A list.
#' @export
openreadout_check <- function(x) .with_file(x, function(h) .from_json(.ic(rs_check(.ptr(h)))))

#' OME-XML of a file
#'
#' OME-XML (2016-06, `MetadataOnly` pixels) describing every image, as an OME-TIFF export would
#' write it.
#'
#' @inheritParams openreadout_info
#' @return A string.
#' @export
openreadout_ome_xml <- function(x) .with_file(x, function(h) .ic(rs_ome_xml(.ptr(h))))

#' Version of the bundled OpenReadout library
#' @return A string such as `"0.1.0"`.
#' @export
openreadout_version <- function() rs_version()
