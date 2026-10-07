# Black-box oracle for Shimadzu LabSolutions .lcd files: chromConverter (Ethan Bass, GPL-3.0,
# CRAN) is run as a black box (docs/legal/clean-room-policy.md rule 3).
#   Rscript shimadzu_chromconverter.R FILE.lcd WHAT OUT.csv   (WHAT = chroms | peak_table | pda)
# `pda` writes the PDA field as a matrix: one row per time point (row name: time in minutes), one
# column per wavelength (column name: nm), values as chromConverter returns them (stored units).
# Set CHROMCONVERTER_LIB to an R library holding chromConverter (install.packages("chromConverter",
# lib = ...); R >= 4.1 with a current Rcpp).
lib <- Sys.getenv("CHROMCONVERTER_LIB")
if (nzchar(lib)) .libPaths(c(lib, .libPaths()))
suppressMessages(library(chromConverter))
args <- commandArgs(trailingOnly = TRUE)
if (args[2] == "pda") {
  x <- read_chroms(args[1], format_in = "shimadzu_lcd", data_format = "wide",
                   format_out = "matrix", what = "pda", progress_bar = FALSE)
  x <- x[[1]]
  if (is.list(x) && !is.matrix(x)) x <- x[[1]]
  write.csv(x, args[3])
} else {
  x <- read_chroms(args[1], format_in = "shimadzu_lcd", data_format = "long",
                   format_out = "data.frame", what = args[2], progress_bar = FALSE)
  x <- x[[1]]
  if (is.list(x) && !is.data.frame(x)) x <- x[[1]]
  write.csv(x, args[3], row.names = FALSE)
}
cat(as.character(packageVersion("chromConverter")), "\n")
