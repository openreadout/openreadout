# Files written here by R from the public specifications (helper-writers.R): no corpus needed.

test_that("a 16-bit ImageJ hyperstack reads back as [x, y, z, c, t] with its values", {
  set.seed(1)
  nx <- 7L; ny <- 5L; nc <- 2L; nz <- 3L; nt <- 2L
  truth <- array(sample.int(65535L, nx * ny * nz * nc * nt, replace = TRUE),
                 dim = c(nx, ny, nz, nc, nt))
  # ImageJ stores planes in c, z, t order (channel fastest)
  pages <- list()
  for (t in seq_len(nt)) for (z in seq_len(nz)) for (c in seq_len(nc)) {
    pages[[length(pages) + 1]] <- truth[, , z, c, t]
  }
  desc <- sprintf("ImageJ=1.54f\nimages=%d\nchannels=%d\nslices=%d\nframes=%d\nhyperstack=true\nunit=micron\nspacing=0.5\n",
                  length(pages), nc, nz, nt)
  path <- withr::local_tempfile(fileext = ".tif")
  write_tiff(path, pages, "uint16", description = desc, resolution = 4)

  info <- openreadout_info(path)
  im <- info$images[[1]]
  expect_equal(c(im$size_x, im$size_y, im$size_z, im$size_c, im$size_t), c(nx, ny, nz, nc, nt))
  img <- openreadout_read_image(path)
  expect_type(img, "integer")
  expect_equal(dim(img), c(nx, ny, nz, nc, nt))
  expect_equal(names(dimnames(img)), c("x", "y", "z", "c", "t"))
  expect_identical(as.vector(img), as.vector(truth))
  expect_equal(attr(img, "pixel_type"), "uint16")
  expect_equal(unname(attr(img, "physical_size")[c("x", "y", "z")]), c(0.25, 0.25, 0.5))

  # selections, one plane, a region (x, y from 1)
  sub <- openreadout_read_image(path, c = 2, z = c(3, 1), t = 2)
  expect_identical(as.vector(sub), as.vector(truth[, , c(3, 1), 2, 2]))
  p <- openreadout_read_plane(path, c = 1, z = 2, t = 1)
  expect_equal(dim(p), c(nx, ny))
  expect_identical(as.vector(p), as.vector(truth[, , 2, 1, 1]))
  r <- openreadout_read_plane(path, c = 2, z = 3, t = 2, region = c(2, 3, 4, 2))
  expect_identical(as.vector(r), as.vector(truth[2:5, 3:4, 3, 2, 2]))
  expect_equal(dim(openreadout_read_image(path, drop = TRUE)), c(nx, ny, nz, nc, nt))
  expect_equal(dim(openreadout_read_image(path, c = 1, t = 1, drop = TRUE)), c(nx, ny, nz))

  # the same through an open handle, and argument errors are usage errors
  f <- openreadout_open(path)
  on.exit(openreadout_close(f))
  expect_identical(openreadout_read_image(f), img)
  expect_error(openreadout_read_image(f, c = 3), class = "openreadout_usage_error")
  expect_error(openreadout_read_image(f, c = 0), "start at 1|from 1", class = "openreadout_usage_error")
  expect_error(openreadout_read_image(f, image = 2), class = "openreadout_usage_error")
  expect_error(openreadout_read_image(f, region = c(5, 1, 4, 2)), class = "openreadout_usage_error")
  expect_error(openreadout_read_image(f, level = 1), class = "openreadout_usage_error")
  expect_equal(openreadout_levels(f)$size_x, nx)
})

test_that("an 8-bit RGB TIFF reads back as [x, y, s]", {
  set.seed(2)
  rgb <- array(sample.int(256L, 6 * 4 * 3, replace = TRUE) - 1L, dim = c(6, 4, 3))
  path <- withr::local_tempfile(fileext = ".tif")
  write_tiff(path, list(rgb), "uint8")
  p <- openreadout_read_plane(path)
  expect_equal(dim(p), c(6, 4, 3))
  expect_equal(dimnames(p)$s, c("R", "G", "B"))
  expect_identical(as.vector(p), as.vector(rgb))
  img <- openreadout_read_image(path)
  expect_equal(names(dimnames(img)), c("x", "y", "s", "z", "c", "t"))
})

test_that("signed 16-bit and float32 planes keep their values and types", {
  set.seed(6)
  i16 <- matrix(sample(-32768:32767, 5 * 4), 5, 4)
  path <- withr::local_tempfile(fileext = ".tif")
  write_tiff(path, list(i16), "int16")
  p <- openreadout_read_plane(path)
  expect_type(p, "integer")
  expect_equal(attr(p, "pixel_type"), "int16")
  expect_identical(as.vector(p), as.vector(i16))

  f32 <- matrix(c(-1.5, 0, 0.25, 3.75e5, -2^-10, 1e-3, 7, 8), 4, 2)
  f32[6] <- readBin(writeBin(1e-3, raw(), size = 4), "double", size = 4)   # the float32 value
  path <- withr::local_tempfile(fileext = ".tif")
  write_tiff(path, list(f32), "float32")
  p <- openreadout_read_plane(path)
  expect_type(p, "double")
  expect_equal(attr(p, "pixel_type"), "float")
  expect_identical(as.vector(p), as.vector(f32))
})

test_that("an FCS 3.0 file reads back as a data frame of its events", {
  set.seed(3)
  m <- matrix(round(runif(40 * 3, -100, 5000) * 4) / 4, ncol = 3)   # exact in float32
  path <- withr::local_tempfile(fileext = ".fcs")
  write_fcs(path, m, c("FSC-A", "SSC-A", "FITC-A"), c("", "", "CD4"))
  ev <- openreadout_table(path)
  expect_s3_class(ev, "data.frame")
  expect_equal(names(ev), c("FSC-A", "SSC-A", "FITC-A"))
  expect_identical(unname(as.matrix(ev)), m)
  expect_equal(unname(attr(ev, "labels")[["FITC-A"]]), "CD4")
  expect_equal(unname(attr(ev, "dtypes")[["FSC-A"]]), "float32")
  expect_identical(unname(as.matrix(openreadout_table(path, first_row = 11, n_max = 5))), m[11:15, ])
  info <- openreadout_info(path)
  expect_equal(info$tables[[1]]$row_count, 40)
  expect_equal(openreadout_info(path, view = "format")$format, "fcs")
})

test_that("a JCAMP-DX spectrum reads back with its axis and Y factor", {
  y <- round(sin(seq(0, 3, length.out = 37)) * 1000) * 0.001
  path <- withr::local_tempfile(fileext = ".jdx")
  write_jcamp(path, y, firstx = 4000, lastx = 400, yfactor = 0.001)
  tr <- openreadout_trace(path)
  expect_equal(nrow(tr), 37)
  vals <- tr[[ncol(tr)]]
  expect_equal(vals, y, tolerance = 1e-12)
  axis <- tr[[2]]
  expect_equal(axis, seq(4000, 400, length.out = 37), tolerance = 1e-9)
})

test_that("a scan without a retention time reads NA, not 0 and not an error", {
  path <- withr::local_tempfile(fileext = ".mzXML")
  write_mzxml(path, list(
    list(num = 1, rt_s = 60, mz = c(100, 200), intensity = c(2, 3)),
    list(num = 2, rt_s = NULL, mz = 100, intensity = 5)
  ))
  expect_equal(attr(openreadout_spectrum(path, spectrum = 1), "spectrum")$rt_s, 60)
  sp <- openreadout_spectrum(path, spectrum = 2)
  expect_equal(sp$intensity, 5)
  meta <- attr(sp, "spectrum")
  expect_true("rt_s" %in% names(meta))
  expect_identical(meta$rt_s, NA_real_)
  expect_identical(attr(openreadout_spectrum(path, scan = 2), "spectrum")$rt_s, NA_real_)
  scans <- openreadout_scans(path)
  expect_equal(scans$scan_number, c(1, 2))
  expect_equal(attr(scans, "scans")$matched, 2)
  expect_equal(nrow(openreadout_scans(path, limit = 1)), 1)
  # chromatograms leave the scan out (never placed at 0) and say so
  tic <- openreadout_analyze(path, "chromatogram", tic = TRUE)
  expect_equal(tic$rt_min, 1)
  expect_true(any(grepl("no retention time", unlist(attr(tic, "chromatograms")$notes))))
})

test_that("exports are verified and read back identically", {
  set.seed(4)
  truth <- array(sample.int(4000L, 9 * 8 * 2, replace = TRUE), dim = c(9, 8, 2))
  src <- withr::local_tempfile(fileext = ".tif")
  desc <- "ImageJ=1.54f\nimages=2\nchannels=2\nhyperstack=true\n"
  write_tiff(src, list(truth[, , 1], truth[, , 2]), "uint16", description = desc)
  out <- withr::local_tempfile(fileext = ".ome.tiff")
  rep <- openreadout_export(src, out)
  expect_true(rep$verified)
  expect_identical(as.vector(openreadout_read_image(out)), as.vector(truth))
  expect_error(openreadout_export(src, out), class = "openreadout_usage_error")   # no silent overwrite
  expect_true(openreadout_export(src, out, overwrite = TRUE)$verified)

  zarr <- file.path(withr::local_tempdir(), "x.ome.zarr")
  expect_true(openreadout_export(src, zarr)$verified)
  expect_identical(as.vector(openreadout_read_image(zarr)), as.vector(truth))

  m <- matrix(c(1, 2.5, -3, 4, 5, 6), ncol = 2)
  fcs <- withr::local_tempfile(fileext = ".fcs")
  write_fcs(fcs, m, c("A", "B"))
  pq <- withr::local_tempfile(fileext = ".parquet")
  rep <- openreadout_export(fcs, pq)
  expect_true(rep$verified)
  expect_equal(rep$rows_written, 3)
  skip_if_not_installed("arrow")
  back <- as.data.frame(arrow::read_parquet(pq))
  expect_identical(unname(as.matrix(back)), m)
})

test_that("errors are classed conditions with exit codes and hints", {
  missing <- file.path(tempdir(), "does-not-exist.czi")
  e <- expect_error(openreadout_info(missing), class = "openreadout_file_not_found")
  expect_s3_class(e, "openreadout_error")
  expect_equal(e$exit_code, 5L)
  expect_true(nzchar(e$hint))

  junk <- withr::local_tempfile(fileext = ".bin")
  writeBin(as.raw(c(0x13, 0x37, rep(0xAB, 200))), junk)
  e <- expect_error(openreadout_info(junk), class = "openreadout_unknown_format")
  expect_equal(e$exit_code, 3L)

  # a TIFF cut short: a clean error, never a crash
  set.seed(5)
  good <- withr::local_tempfile(fileext = ".tif")
  write_tiff(good, list(matrix(1:600, 30, 20)), "uint16")
  bytes <- readBin(good, "raw", file.size(good))
  cut <- withr::local_tempfile(fileext = ".tif")
  writeBin(bytes[c(1:8, 300:length(bytes))], cut)
  expect_error(openreadout_read_image(cut), class = "openreadout_error")

  expect_error(openreadout_table(good), class = "openreadout_error")
  expect_error(openreadout_export(good, "out.xyz"), class = "openreadout_usage_error")
})

test_that("closed and restored handles fail cleanly", {
  path <- withr::local_tempfile(fileext = ".tif")
  write_tiff(path, list(matrix(1:12, 4, 3)), "uint16")
  f <- openreadout_open(path)
  openreadout_close(f)
  openreadout_close(f)
  expect_error(openreadout_info(f), "closed", class = "openreadout_usage_error")
  expect_output(print(f), "closed")
  restored <- unserialize(serialize(openreadout_open(path), NULL))
  expect_error(openreadout_read_image(restored), class = "openreadout_usage_error")
})

test_that("info views, pixel statistics and analysis kinds follow the CLI", {
  path <- withr::local_tempfile(fileext = ".tif")
  write_tiff(path, list(matrix(1:12, 4, 3), matrix(13:24, 4, 3)), "uint16")
  full <- openreadout_info(path, view = "full")
  expect_equal(full$file$format$id, "tiff")
  expect_true("provenance" %in% names(full))
  expect_false("vendor" %in% names(openreadout_info(path, view = "full", vendor = FALSE)))
  expect_true(length(openreadout_info(path, view = "structure")$entries) > 0)
  expect_type(openreadout_info(path, view = "explain")$summary, "character")
  expect_equal(openreadout_info(path, view = "format")$format, "tiff")
  expect_error(openreadout_info(path, ask = "what?"), class = "openreadout_usage_error")
  expect_error(openreadout_info(path, view = "dump"))

  st <- openreadout_stats(path)
  expect_equal(st$image, 1)
  expect_equal(st$min, 1)
  expect_equal(st$max, 24)
  planes <- openreadout_stats(path, per = "plane")
  expect_equal(nrow(planes), 2)
  expect_equal(planes$max, c(12, 24))
  expect_error(openreadout_stats(path, wells = "A01"), class = "openreadout_usage_error")

  expect_error(openreadout_analyze(path, "dump"), class = "openreadout_usage_error")
  expect_error(openreadout_analyze(path, "nmr-peaks"), class = "openreadout_error")
})

test_that("formats and version are available", {
  fm <- openreadout_formats()
  expect_true(all(c("czi", "nd2", "lif", "fcs", "thermo-raw") %in% fm$id))
  expect_match(openreadout_version(), "^[0-9]+\\.[0-9]+\\.[0-9]+")
})
