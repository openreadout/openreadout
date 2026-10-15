# Pictures from R: previews with annotations, figures from plot specs, and the measurements on
# one plane (line, polygon, objects).

png_magic <- as.raw(c(0x89, 0x50, 0x4e, 0x47))

test_that("a preview carries its annotations and where they landed", {
  p <- openreadout_preview(corpus_file("zenodo7015307-S-2-T-3-Z-5-CH-1.czi"), max_size = 512,
    annotations = list(
      list(type = "point", x = 20, y = 30, label = "here"),
      list(type = "box", x = 5, y = 5, width = 40, height = 20)
    ))
  expect_s3_class(p, "openreadout_picture")
  expect_equal(p$data[1:4], png_magic)
  expect_lte(max(p$meta$width, p$meta$height), 512)
  expect_equal(vapply(p$meta$annotations, function(a) a$type, ""), c("point", "box"))
  expect_output(print(p), "image preview")
})

test_that("annotations can be a data frame, and svg and dark work", {
  a <- data.frame(type = c("vline", "text"), x = c(1, 2), y = c(NA, 1e6), label = c(NA, "peak"))
  p <- openreadout_preview(corpus_file("mtbls20-caffeine-pos.mzML"), annotations = a,
                           format = "svg", dark = TRUE)
  expect_equal(p$format, "svg")
  expect_match(rawToChar(p$data[1:200]), "<svg")
  expect_length(p$meta$annotations, 2)
})

test_that("preview indices start at 1 and spikes are marked", {
  p <- openreadout_preview(corpus_file("gh-swharden-pyabf-17o05028-ic-steps.abf"), sweep = 11,
                           marks = "spikes")
  expect_equal(p$meta$kind, "trace")
  expect_gt(length(p$meta$marks), 0)
})

test_that("bad preview options are usage errors", {
  f <- corpus_file("zenodo7015307-S-2-T-3-Z-5-CH-1.czi")
  expect_error(openreadout_preview(f, colour = "red"), class = "openreadout_usage_error")
  expect_error(openreadout_preview(f, region = c(0, 0, 10, 10)), class = "openreadout_usage_error")
})

test_that("a figure renders as png, svg and pdf and saves verified", {
  f <- corpus_file("mtbls20-caffeine-pos.mzML")
  fig <- openreadout_plot(data = list(file = f, chromatogram = "tic"), title = "Total ion current")
  expect_s3_class(fig, "openreadout_figure")
  expect_equal(fig$data[1:4], png_magic)
  expect_equal(fig$meta$width, 1200) # the size at scale 1; the PNG is drawn at scale 2
  expect_equal(fig$meta$title, "Total ion current")
  svg <- openreadout_plot(list(data = list(file = f, chromatogram = "tic")), format = "svg")
  expect_match(rawToChar(svg$data[1:200]), "<svg")
  out <- tempfile(fileext = ".pdf")
  pdf <- openreadout_plot(data = list(file = f, chromatogram = "tic"), format = "pdf", file = out)
  expect_equal(readBin(out, "raw", 4), charToRaw("%PDF"))
  expect_error(openreadout_plot(data = list(file = f, chromatogram = "tic"), format = "pdf", file = out),
               class = "openreadout_usage_error")
})

test_that("plate growth curves lay out in plate positions", {
  fig <- openreadout_plot(data = list(file = corpus_file("softmax5-kinetic-phage-120308b.pda"),
                                      wells = "A1:H12"),
                          facet = "plate")
  expect_gt(length(fig$meta$panels), 0)
  expect_error(openreadout_plot(data = list(values = c(1, 2)), colour = "red"),
               class = "openreadout_usage_error")
})

test_that("stats measures a line, a polygon and objects", {
  f <- openreadout_open(corpus_file("zenodo7015307-S-2-T-3-Z-5-CH-1.czi"))
  on.exit(openreadout_close(f))
  img <- openreadout_read_image(f, image = 1, c = 1, z = 3, t = 1, drop = TRUE)
  w <- dim(img)[1]
  h <- dim(img)[2]
  roi <- openreadout_stats(f, select = "z=2", polygon = cbind(c(0, w, w, 0), c(0, 0, h, h)))$roi
  expect_equal(roi$area_px, w * h, tolerance = 0.01)
  expect_equal(roi$channels[[1]]$mean, mean(img), tolerance = 0.01)
  prof <- openreadout_stats(f, select = "z=2", line = c(0, h / 2, w - 1, h / 2))$profile
  expect_lte(prof$channels[[1]]$max, max(img))
  expect_gte(prof$channels[[1]]$min, min(img))
  obj <- openreadout_stats(f, select = "z=2", objects = list(diameter_px = 6))$objects
  expect_gte(obj$count, 0)
  expect_equal(obj$expected_diameter_from, "given")
  expect_equal(openreadout_stats(f, select = "z=2", objects = TRUE)$objects$count,
               openreadout_stats(f, select = "z=2", objects = list())$objects$count)
  expect_error(openreadout_stats(f, line = c(0, 0, 1)), class = "openreadout_usage_error")
})

test_that("growth wells say whether they grew, crashed or regrew", {
  r <- openreadout_analyze(corpus_file("softmax5-kinetic-phage-120308b.pda"), "assay",
                           analysis = "growth")
  patterns <- vapply(r$growth, function(w) w$pattern, "")
  expect_true(all(patterns %in% c("grew", "no_growth", "crashed", "regrew")))
  expect_equal(sum(lengths(r$growth_summary$wells)), length(r$growth))
})
