# Pixels through the R API against third-party readers' hashes (corpus/oracle): the array layout
# [x, y, z, c, t], 1-based selections, pyramid levels and regions.

plane_oracle_check <- function(file, oracle_id, max_images = Inf) {
  path <- corpus_file(file)
  o <- oracle(paste0(oracle_id, ".json"))
  f <- openreadout_open(path)
  on.exit(openreadout_close(f))
  n <- 0
  for (im in utils::head(o$images, max_images)) {
    img <- openreadout_read_image(f, image = im$index + 1)
    expect_equal(dim(img)[1:2], c(im$size_x, im$size_y))
    expect_equal(attr(img, "pixel_type"), im$pixel_type)
    for (p in im$planes) {
      v <- img[, , p$z + 1, p$c + 1, p$t + 1]
      expect_equal(xxh3(v, im$pixel_type), p$xxh3,
                   label = sprintf("%s image %d c%d z%d t%d", file, im$index, p$c, p$z, p$t))
      n <- n + 1
    }
  }
  n
}

test_that("LIF planes match liffile", {
  expect_gt(plane_oracle_check("aics-s-1-t-1-c-2-z-1.lif", "aics-s-1-t-1-c-2-z-1"), 0)
})

test_that("ND2 positions x z x c x t planes match nd2 (array order x, y, z, c, t)", {
  expect_equal(plane_oracle_check("aics-ND2-dims-p4z5t3c2y32x32.nd2", "aics-ND2-dims-p4z5t3c2y32x32"),
               4 * 5 * 3 * 2)
})

test_that("CZI scenes match czifile", {
  expect_gt(plane_oracle_check("aics-s-3-t-1-c-3-z-5.czi", "aics-s-3-t-1-c-3-z-5"), 0)
})

test_that("per-image, per-channel statistics computed in R match nd2 + numpy", {
  o <- oracle("batch/image-stats.json")
  entry <- Filter(function(e) e$id == "aics-ND2-dims-p4z5t3c2y32x32", o$files)[[1]]
  f <- openreadout_open(corpus_file("aics-ND2-dims-p4z5t3c2y32x32.nd2"))
  on.exit(openreadout_close(f))
  for (r in entry$rows) {
    v <- as.vector(openreadout_read_image(f, image = r$image + 1, c = r$channel + 1))
    expect_equal(length(v), r$count)
    expect_equal(mean(v), r$mean, tolerance = 1e-9)
    expect_equal(stats::median(v), r$median)
    expect_equal(c(min(v), max(v)), c(r$min, r$max))
  }
})

region_oracle_check <- function(id, lossless_only = TRUE) {
  o <- oracle(file.path("regions", paste0(id, ".json")))
  path <- corpus_file(o$file)
  f <- openreadout_open(path)
  on.exit(openreadout_close(f))
  checked <- 0
  for (r in o$regions) {
    if (!is.null(r$disputed)) next
    lv <- openreadout_levels(f, image = r$image + 1)
    row <- lv[lv$level == r$level, ]
    expect_equal(c(row$size_x, row$size_y), unlist(r$level_size))
    reg <- unlist(r$region)
    p <- openreadout_read_plane(f, image = r$image + 1, c = r$c + 1, z = r$z + 1, t = r$t + 1,
                       level = r$level, region = c(reg[1] + 1, reg[2] + 1, reg[3], reg[4]))
    expect_equal(dim(p)[1:2], reg[3:4])
    label <- sprintf("%s image %d level %d region %s", id, r$image, r$level, paste(reg, collapse = ","))
    if (isTRUE(o$lossy)) {
      expect_lt(abs(mean(p) - r$mean), 1.5, label = label)
    } else {
      h <- xxh3(interleave(p), attr(p, "pixel_type"))
      ok <- h == r$xxh3 || (!is.null(r$xxh3_alt) && h == r$xxh3_alt)
      expect_true(ok, label = label)
    }
    checked <- checked + 1
  }
  checked
}

test_that("CZI regions match czifile", {
  expect_gt(region_oracle_check("aics-s-3-t-1-c-3-z-5"), 0)
})

test_that("OME-TIFF pyramid levels and regions match tifffile + zarr", {
  expect_gt(region_oracle_check("ome-subresolutions-retina-large"), 0)
})

test_that("whole-slide RGB regions agree with tifffile (JPEG: mean within 1.5)", {
  expect_gt(region_oracle_check("openslide-aperio-cmu-1-small-region"), 0)
})

test_that("physical sizes and channel names come from the file", {
  o <- oracle("aics-ND2-dims-p4z5t3c2y32x32.json")
  im <- o$images[[1]]
  img <- openreadout_read_image(corpus_file("aics-ND2-dims-p4z5t3c2y32x32.nd2"), c = 2, z = 1, t = 1)
  expect_equal(unname(attr(img, "physical_size")[c("x", "y", "z")]),
               c(im$physical_size_um$x, im$physical_size_um$y, im$physical_size_um$z), tolerance = 1e-9)
  expect_equal(dimnames(img)$c, unlist(im$channel_names)[2])
})
