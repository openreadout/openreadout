# Second opinion for chromatography files: chromConverter (Ethan Bass, GPL-3.0, CRAN) is RUN as
# a black box (docs/legal/clean-room-policy.md rule 3), with its own parsers
# (parser = "chromconverter"), not rainbow or Aston through reticulate.
#   Rscript chromconverter.R FORMAT_IN FILE OUTDIR
# writes OUTDIR/trace_<i>.csv (columns rt and the signal columns, 17 significant digits; long
# format for 2-D data: rt, lambda, intensity) and OUTDIR/meta_<i>.json (chromConverter's metadata
# attributes). CHROMCONVERTER_LIB names an R library holding chromConverter.
lib <- Sys.getenv("CHROMCONVERTER_LIB")
if (nzchar(lib)) .libPaths(c(lib, .libPaths()))
suppressMessages(library(chromConverter))
suppressMessages(library(jsonlite))
args <- commandArgs(trailingOnly = TRUE)
fmt <- args[1]
path <- args[2]
out <- args[3]
dir.create(out, showWarnings = FALSE, recursive = TRUE)
x <- read_chroms(path, format_in = fmt, parser = "chromconverter", format_out = "data.frame",
                 data_format = "long", read_metadata = TRUE, progress_bar = FALSE)
# read_chroms returns a list (one element per file); an element may itself be a list of traces
items <- list()
flatten <- function(v) {
  if (is.data.frame(v) || is.matrix(v)) {
    items[[length(items) + 1]] <<- v
  } else if (is.list(v)) {
    for (w in v) flatten(w)
  }
}
flatten(x)
for (i in seq_along(items)) {
  d <- as.data.frame(items[[i]])
  m <- attributes(items[[i]])
  m <- m[setdiff(names(m), c("names", "row.names", "class", "dim", "dimnames"))]
  cols <- lapply(d, function(c) if (is.numeric(c)) sprintf("%.17g", c) else as.character(c))
  write.csv(as.data.frame(cols, check.names = FALSE), file.path(out, sprintf("trace_%d.csv", i)),
            row.names = FALSE, quote = FALSE)
  write(toJSON(lapply(m, function(v) if (is.atomic(v)) v else as.character(v)), auto_unbox = TRUE,
               null = "null", na = "null", digits = NA),
        file.path(out, sprintf("meta_%d.json", i)))
}
cat(as.character(packageVersion("chromConverter")), "\n")
