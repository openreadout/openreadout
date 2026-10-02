# Calls into the Rust library (crates/openreadout-r) and the helpers every wrapper uses.
#
# The native functions never raise R errors themselves: a failure comes back as a list of class
# `openreadout_native_error`, which .ic() turns into a condition of classes
# c(<specific>, "openreadout_error", "error", "condition") carrying `code`, `exit_code` and
# `hint` (the values of the CLI's JSON error envelope, https://openreadout.github.io/openreadout/reference/commands.html#exit-codes).

#' @useDynLib openreadout, .registration = TRUE
NULL

.ic <- function(res, call = sys.call(-1)) {
  if (inherits(res, "openreadout_native_error")) {
    msg <- res$message
    if (nzchar(res$hint)) {
      msg <- paste0(msg, "\nHint: ", res$hint)
    }
    classes <- unique(c(res$class, "openreadout_error", "error", "condition"))
    cond <- structure(
      class = classes,
      list(message = msg, call = call, code = res$code, exit_code = res$exit_code, hint = res$hint)
    )
    stop(cond)
  }
  res
}

# An R-side usage error, of the same classes as a native one.
.openreadout_abort <- function(msg, call = sys.call(-1)) {
  stop(structure(
    class = c("openreadout_usage_error", "openreadout_error", "error", "condition"),
    list(message = msg, call = call, code = "usage", exit_code = 2L, hint = "")
  ))
}

.from_json <- function(x) {
  jsonlite::fromJSON(x, simplifyVector = TRUE, simplifyDataFrame = FALSE, simplifyMatrix = FALSE)
}

# Without simplification: batch rows mix types, and simplifying them would pass numbers through
# strings.
.from_json_raw <- function(x) jsonlite::fromJSON(x, simplifyVector = FALSE)

# JSON for a request object: length-1 vectors become scalars unless wrapped in I().
.to_json <- function(x) {
  x <- x[!vapply(x, is.null, logical(1))]
  if (length(x) == 0) {
    return("{}")
  }
  as.character(jsonlite::toJSON(x, auto_unbox = TRUE, null = "null", digits = NA))
}

# A plain data.frame, or a tibble when the tibble package is installed and
# getOption("openreadout.tibble", TRUE) is TRUE.
.frame <- function(cols) {
  n <- if (length(cols) > 0) length(cols[[1]]) else 0L
  if (is.null(names(cols))) {
    names(cols) <- character(length(cols))
  }
  df <- structure(cols, class = "data.frame", row.names = .set_row_names(n))
  if (isTRUE(getOption("openreadout.tibble", TRUE)) && requireNamespace("tibble", quietly = TRUE)) {
    df <- tibble::as_tibble(df, .name_repair = "minimal")
  }
  df
}

# 1-based R index (a single positive whole number) -> 0-based index for the native side.
.index0 <- function(x, what) {
  if (!is.numeric(x) || length(x) != 1 || is.na(x) || x < 1 || x != round(x)) {
    .openreadout_abort(sprintf("%s must be a single positive whole number (R indices start at 1), not %s",
                      what, paste(deparse(x), collapse = "")), call = sys.call(-1))
  }
  as.integer(x - 1)
}

.count <- function(x, what, allow_null = TRUE) {
  if (is.null(x) && allow_null) {
    return(NULL)
  }
  if (!is.numeric(x) || length(x) != 1 || is.na(x) || x < 0 || x != round(x)) {
    .openreadout_abort(sprintf("%s must be a single non-negative whole number", what), call = sys.call(-1))
  }
  x
}

.path <- function(path) {
  if (!is.character(path) || length(path) != 1 || is.na(path)) {
    .openreadout_abort("path must be a single file or directory path", call = sys.call(-1))
  }
  path.expand(path)
}

# Run `f(handle)` on an openreadout_file, or on a path opened (and closed) for this call.
.with_file <- function(x, f) {
  if (inherits(x, "openreadout_file")) {
    return(f(x))
  }
  h <- openreadout_open(x)
  on.exit(openreadout_close(h), add = TRUE)
  f(h)
}

.ptr <- function(x) {
  if (!inherits(x, "openreadout_file")) {
    .openreadout_abort("expected a file opened with openreadout_open() or a path", call = sys.call(-1))
  }
  x$ptr
}

# A data frame from a list of records (JSON objects parsed with .from_json): one column per key,
# in order of first appearance; scalar values become atomic columns (NA where a record lacks the
# key), anything else a list column.
.records <- function(records) {
  if (length(records) == 0) {
    return(.frame(list()))
  }
  keys <- unique(unlist(lapply(records, names), use.names = FALSE))
  cols <- lapply(keys, function(k) {
    vals <- lapply(records, function(r) r[[k]])
    scalar <- vapply(vals, function(v) is.null(v) || (is.atomic(v) && length(v) == 1), logical(1))
    if (all(scalar)) {
      vals[vapply(vals, is.null, logical(1))] <- list(NA)
      unlist(vals, use.names = FALSE)
    } else {
      vals
    }
  })
  names(cols) <- keys
  .frame(cols)
}
