# Test data: the OpenReadout corpus (OPENREADOUT_CORPUS_DIR, downloaded with
# `cargo xtask corpus fetch`) and its committed ground truth (corpus/oracle/*.json, written by
# third-party readers: liffile, czifile, tifffile, nd2, flowio, pyabf, pyteomics, scipy,
# rdmlpython, allotropy, numpy). Tests that need them skip when they are absent, so the package
# checks cleanly without the corpus (tests in test-synthetic.R write their own files).

corpus_dir <- function() Sys.getenv("OPENREADOUT_CORPUS_DIR", "")

oracle_dir <- function() {
  d <- Sys.getenv("OPENREADOUT_ORACLE_DIR", "")
  if (nzchar(d)) return(d)
  # a checkout: r/openreadout/tests/testthat -> corpus/oracle
  for (up in c("../../../../corpus/oracle", "../../../../../corpus/oracle")) {
    if (dir.exists(up)) return(normalizePath(up))
  }
  if (nzchar(corpus_dir())) {
    d <- file.path(dirname(corpus_dir()), "oracle")
    if (dir.exists(d)) return(d)
  }
  ""
}

corpus_file <- function(name) {
  skip_if(!nzchar(corpus_dir()), "OPENREADOUT_CORPUS_DIR is not set")
  p <- file.path(corpus_dir(), name)
  skip_if(!file.exists(p), paste("corpus file missing:", name))
  p
}

oracle <- function(rel) {
  d <- oracle_dir()
  skip_if(!nzchar(d), "corpus/oracle not found (set OPENREADOUT_ORACLE_DIR)")
  p <- file.path(d, rel)
  skip_if(!file.exists(p), paste("oracle missing:", rel))
  jsonlite::fromJSON(p, simplifyVector = FALSE)
}

# xxh3-128 of values as the little-endian bytes of `type` (the oracle hashes: raw samples for
# pixels, f64 for table and trace values, f32 for spectrum intensities).
xxh3 <- function(values, type) {
  skip_if_not_installed("digest")
  size <- switch(type, uint8 = 1, int8 = 1, uint16 = 2, int16 = 2, int32 = 4, uint32 = 4,
                 float = 4, f32 = 4, double = 8, f64 = 8)
  v <- if (type %in% c("float", "f32", "double", "f64")) as.double(values) else as.integer(values)
  if (type == "uint32") {
    # writeBin has no unsigned 32-bit: write the two 16-bit halves
    hi <- as.integer(values %/% 65536)
    lo <- as.integer(values %% 65536)
    b <- writeBin(as.integer(rbind(lo, hi)), raw(), size = 2, endian = "little")
  } else {
    b <- writeBin(v, raw(), size = size, endian = "little")
  }
  digest::digest(b, algo = "xxh3_128", serialize = FALSE)
}

# Interleave an [x, y, s] RGB plane back to the stored sample order (s fastest).
interleave <- function(p) {
  if (length(dim(p)) == 3) aperm(p, c(3, 1, 2)) else p
}
