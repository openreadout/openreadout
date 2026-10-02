# Minimal writers for test files, written from the public format specifications (TIFF 6.0 baseline
# with the ImageJ hyperstack description, FCS 3.0, JCAMP-DX 4.24, mzXML 3.2) so the expected values are known
# independently of the package: whatever openreadout reads back must equal what R wrote.

# Baseline little-endian TIFF, uncompressed, one strip per page. `pages` is a list of
# [x, y] (or [x, y, 3] RGB) arrays, all the same size and type; `type` "uint8", "uint16",
# "int16" or "float32".
write_tiff <- function(path, pages, type = "uint16", description = NULL,
                       resolution = NULL) {
  con <- file(path, "wb")
  on.exit(close(con))
  u16 <- function(v) writeBin(as.integer(v), con, size = 2, endian = "little")
  u32 <- function(v) {
    v <- as.numeric(v)
    u16(v %% 65536)
    u16(v %/% 65536)
  }
  bytes <- switch(type, uint8 = 1, uint16 = 2, int16 = 2, float32 = 4)
  sample_format <- switch(type, uint8 = 1, uint16 = 1, int16 = 2, float32 = 3)
  d <- dim(pages[[1]])
  w <- d[1]
  h <- d[2]
  spp <- if (length(d) == 3) d[3] else 1
  strip <- w * h * spp * bytes
  desc <- if (!is.null(description)) c(charToRaw(description), as.raw(0)) else raw()
  if (length(desc) %% 2 == 1) desc <- c(desc, as.raw(0)) # keep offsets word-aligned
  ntags <- 11 + (length(desc) > 0) + 3 * !is.null(resolution)
  ifd_size <- 2 + ntags * 12 + 4
  # layout: header | page data ... | per page: IFD, BitsPerSample array, description, resolution
  writeBin(charToRaw("II"), con)
  u16(42)
  data_start <- 8
  extra_size <- (if (spp > 1) 2 * spp else 0) + length(desc) + (if (!is.null(resolution)) 16 else 0)
  first_ifd <- data_start + strip * length(pages)
  u32(first_ifd)
  for (p in pages) {
    v <- if (spp > 1) aperm(p, c(3, 1, 2)) else p
    v <- if (type == "float32") as.double(v) else as.integer(v)
    writeBin(v, con, size = bytes, endian = "little")
  }
  for (i in seq_along(pages)) {
    ifd <- first_ifd + (i - 1) * (ifd_size + extra_size)
    extra <- ifd + ifd_size
    bits_off <- extra
    desc_off <- bits_off + (if (spp > 1) 2 * spp else 0)
    res_off <- desc_off + length(desc)
    tag <- function(code, typ, count, value) {
      u16(code)
      u16(typ)
      u32(count)
      if (typ == 3 && count == 1) {
        u16(value)
        u16(0)
      } else {
        u32(value)
      }
    }
    u16(ntags)
    tag(256, 4, 1, w)
    tag(257, 4, 1, h)
    if (spp > 1) tag(258, 3, spp, bits_off) else tag(258, 3, 1, bytes * 8)
    tag(259, 3, 1, 1)
    tag(262, 3, 1, if (spp == 3) 2 else 1)
    if (length(desc)) tag(270, 2, length(desc), desc_off)
    tag(273, 4, 1, data_start + (i - 1) * strip)
    tag(277, 3, 1, spp)
    tag(278, 4, 1, h)
    tag(279, 4, 1, strip)
    if (!is.null(resolution)) {
      tag(282, 5, 1, res_off)
      tag(283, 5, 1, res_off + 8)
    }
    tag(284, 3, 1, 1)
    if (!is.null(resolution)) tag(296, 3, 1, 1)
    tag(339, 3, 1, sample_format)
    u32(if (i < length(pages)) ifd + ifd_size + extra_size else 0)
    if (spp > 1) u16(rep(bytes * 8, spp))
    if (length(desc)) writeBin(desc, con)
    if (!is.null(resolution)) {
      # pixels per unit as a rational: resolution * 1e6 / 1e6
      u32(round(resolution * 1e6)); u32(1e6)
      u32(round(resolution * 1e6)); u32(1e6)
    }
  }
  invisible(path)
}

# FCS 3.0 list-mode file with float32 little-endian data. `m` is an events x parameters matrix
# (values exactly representable as float32); `labels` the $PnS values.
write_fcs <- function(path, m, names, labels = NULL) {
  npar <- ncol(m)
  data <- writeBin(as.double(t(m)), raw(), size = 4, endian = "little")
  kw <- c(
    "$BEGINANALYSIS" = "0", "$ENDANALYSIS" = "0", "$BEGINSTEXT" = "0", "$ENDSTEXT" = "0",
    "$BEGINDATA" = "@@@@@@@@@@", "$ENDDATA" = "##########",
    "$BYTEORD" = "1,2,3,4", "$DATATYPE" = "F", "$MODE" = "L", "$NEXTDATA" = "0",
    "$PAR" = as.character(npar), "$TOT" = as.character(nrow(m)), "$CYT" = "R test writer"
  )
  for (j in seq_len(npar)) {
    kw[paste0("$P", j, "B")] <- "32"
    kw[paste0("$P", j, "E")] <- "0,0"
    kw[paste0("$P", j, "N")] <- names[j]
    kw[paste0("$P", j, "R")] <- "262144"
    # FCS values cannot be empty (a doubled delimiter is an escaped delimiter)
    if (!is.null(labels) && nzchar(labels[j])) kw[paste0("$P", j, "S")] <- labels[j]
  }
  text <- paste0("/", paste0(names(kw), "/", kw, "/", collapse = ""))
  text_start <- 58
  text_end <- text_start + nchar(text, type = "bytes") - 1
  data_start <- text_end + 1
  data_end <- data_start + length(data) - 1
  text <- sub("@@@@@@@@@@", formatC(data_start, width = 10, flag = "0"), text, fixed = TRUE)
  text <- sub("##########", formatC(data_end, width = 10, flag = "0"), text, fixed = TRUE)
  off <- function(v) formatC(v, width = 8)
  header <- paste0("FCS3.0    ", off(text_start), off(text_end), off(data_start), off(data_end),
                   off(0), off(0))
  con <- file(path, "wb")
  on.exit(close(con))
  writeBin(charToRaw(header), con)
  writeBin(charToRaw(text), con)
  writeBin(data, con)
  invisible(path)
}

# JCAMP-DX 4.24 spectrum, (X++(Y..Y)) AFFN, ten values per line.
write_jcamp <- function(path, y, firstx, lastx, yfactor = 1, xunits = "1/CM", yunits = "ABSORBANCE") {
  n <- length(y)
  dx <- (lastx - firstx) / (n - 1)
  yi <- round(y / yfactor)
  lines <- character()
  for (s in seq(1, n, by = 10)) {
    idx <- s:min(n, s + 9)
    lines <- c(lines, paste(format(firstx + (s - 1) * dx, nsmall = 4), paste(yi[idx], collapse = " ")))
  }
  txt <- c(
    "##TITLE=R test spectrum", "##JCAMP-DX=4.24", "##DATA TYPE=INFRARED SPECTRUM",
    paste0("##XUNITS=", xunits), paste0("##YUNITS=", yunits),
    paste0("##FIRSTX=", firstx), paste0("##LASTX=", lastx),
    "##XFACTOR=1", paste0("##YFACTOR=", yfactor), paste0("##NPOINTS=", n),
    paste0("##FIRSTY=", yi[1] * yfactor),
    "##XYDATA=(X++(Y..Y))", lines, "##END="
  )
  writeLines(txt, path)
  invisible(path)
}

# mzXML 3.2 with uncompressed 32-bit big-endian m/z-intensity pairs. `scans` is a list of
# list(num, rt_s, mz, intensity); `rt_s = NULL` writes no retentionTime attribute.
write_mzxml <- function(path, scans) {
  body <- vapply(scans, function(s) {
    vals <- as.vector(rbind(s$mz, s$intensity))
    b64 <- jsonlite::base64_enc(writeBin(as.double(vals), raw(), size = 4, endian = "big"))
    rt <- if (is.null(s$rt_s)) "" else sprintf(' retentionTime="PT%sS"', format(s$rt_s))
    sprintf(paste0('<scan num="%d" msLevel="1" peaksCount="%d" polarity="+" centroided="1"%s>',
                   '<peaks precision="32" byteOrder="network" pairOrder="m/z-int">%s</peaks></scan>'),
            as.integer(s$num), length(s$mz), rt, b64)
  }, "")
  writeLines(c('<?xml version="1.0" encoding="ISO-8859-1"?>',
               paste0('<mzXML xmlns="http://sashimi.sourceforge.net/schema_revision/mzXML_3.2">',
                      sprintf('<msRun scanCount="%d">', length(scans)), paste(body, collapse = ""),
                      "</msRun></mzXML>")), path)
  invisible(path)
}
