# Black-box oracle for Shimadzu LabSolutions .lcd files: chromConverter (Ethan Bass, GPL-3.0,
# CRAN) is run as a black box (docs/legal/clean-room-policy.md rule 3).
#   Rscript shimadzu_chromconverter.R FILE.lcd WHAT OUT.csv   (WHAT = chroms | peak_table)
# Set CHROMCONVERTER_LIB to an R library holding chromConverter (install.packages("chromConverter",
# lib = ...); R >= 4.1 with a current Rcpp).
lib <- Sys.getenv("CHROMCONVERTER_LIB")
if (nzchar(lib)) .libPaths(c(lib, .libPaths()))
suppressMessages(library(chromConverter))
args <- commandArgs(trailingOnly = TRUE)
x <- read_chroms(args[1], format_in = "shimadzu_lcd", data_format = "long",
                 format_out = "data.frame", what = args[2], progress_bar = FALSE)
x <- x[[1]]
if (is.list(x) && !is.data.frame(x)) x <- x[[1]]
write.csv(x, args[3], row.names = FALSE)
cat(as.character(packageVersion("chromConverter")), "\n")
