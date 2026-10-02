# Black-box oracle for Agilent OpenLab CDS .dx files: chromConverter (Ethan Bass, GPL-3.0,
# CRAN) is run as a black box (docs/legal/clean-room-policy.md rule 3).
#   Rscript openlab_chromconverter.R FILE.dx OUTDIR
# writes OUTDIR/<kind>_<i>.csv (columns rt, intensity, 17 significant digits) and OUTDIR/index.csv (kind, i, rows,
# signal, units, sample_name, operator, method). Set CHROMCONVERTER_LIB to an R library holding
# chromConverter (install.packages("chromConverter", lib = ...); R >= 4.1 with a current Rcpp).
lib <- Sys.getenv("CHROMCONVERTER_LIB")
if (nzchar(lib)) .libPaths(c(lib, .libPaths()))
suppressMessages(library(chromConverter))
args <- commandArgs(trailingOnly = TRUE)
x <- read_agilent_dx(args[1], what = c("chroms", "instrument"), format_out = "data.frame",
                     data_format = "long", metadata_format = "raw", collapse = FALSE)
dir.create(args[2], showWarnings = FALSE, recursive = TRUE)
rows <- list()
for (kind in names(x)) {
  # a kind with one trace comes back as the data frame itself
  items <- if (is.data.frame(x[[kind]])) list(x[[kind]]) else x[[kind]]
  for (i in seq_along(items)) {
    d <- items[[i]]
    m <- attr(d, "metadata")
    # 17 significant digits: the doubles round-trip exactly
    write.csv(data.frame(rt = sprintf("%.17g", d$rt), intensity = sprintf("%.17g", d$intensity)),
              file.path(args[2], sprintf("%s_%d.csv", kind, i)), row.names = FALSE, quote = FALSE)
    g <- function(k) if (is.null(m[[k]])) "" else as.character(m[[k]])[1]
    rows[[length(rows) + 1]] <- data.frame(kind = kind, i = i, rows = nrow(d), signal = g("signal"),
                                           units = g("units"), sample_name = g("sample_name"),
                                           operator = g("operator"), method = g("method"))
  }
}
write.csv(do.call(rbind, rows), file.path(args[2], "index.csv"), row.names = FALSE,
          fileEncoding = "UTF-8")
cat(as.character(packageVersion("chromConverter")), "\n")
