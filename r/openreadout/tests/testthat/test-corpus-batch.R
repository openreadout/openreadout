# Batch tables, sample-sheet joins and group summaries against pandas/SciPy (corpus/oracle/batch)
# and against R's own stats package.

summarize_input <- function() {
  d <- oracle_dir()
  skip_if(!nzchar(d), "corpus/oracle not found")
  p <- file.path(d, "batch", "summarize-input.csv")
  skip_if(!file.exists(p), "summarize-input.csv missing")
  p
}

test_that("group summaries match pandas and SciPy, and R's t.test / wilcox.test", {
  o <- oracle("batch/summarize.json")
  input <- summarize_input()
  raw <- utils::read.csv(input, stringsAsFactors = FALSE)
  raw <- raw[is.na(raw$error) | raw$error == "", ]
  for (test in c("welch", "mann-whitney")) {
    s <- openreadout_summarize(input, by = "condition", values = "mean", test = test,
                               control = "ctrl")
    expect_equal(attr(s, "info")$rows_excluded, 1)
    for (e in o$plain) {
      row <- s[s$condition == e$condition & s$channel_name == e$channel_name, ]
      expect_equal(nrow(row), 1)
      for (m in c("n", "mean", "sd", "sem", "median", "min", "max", "cv_percent")) {
        expect_equal(row[[m]], e[[m]], tolerance = 1e-9, label = paste(e$condition, e$channel_name, m))
      }
      if (e$condition == "ctrl") next
      x <- raw$mean[raw$condition == e$condition & raw$channel_name == e$channel_name]
      y <- raw$mean[raw$condition == "ctrl" & raw$channel_name == e$channel_name]
      if (test == "welch") {
        tt <- stats::t.test(x, y, var.equal = FALSE)
        expect_equal(row$statistic, unname(tt$statistic), tolerance = 1e-9)
        expect_equal(row$df, unname(tt$parameter), tolerance = 1e-9)
        expect_equal(row$p_value, tt$p.value, tolerance = 1e-7)
        expect_equal(row$p_value, e$welch_p, tolerance = 1e-7)
      } else {
        wt <- suppressWarnings(stats::wilcox.test(x, y, exact = TRUE))
        expect_equal(row$statistic, unname(wt$statistic), tolerance = 1e-9)
        expect_equal(row$p_value, e$mwu_p, tolerance = 1e-7)
      }
    }
  }
})

test_that("a sample sheet written in R is joined, and the summary equals R's aggregate", {
  o <- oracle("batch/fcs-summaries.json")
  paths <- vapply(o$files, function(f) corpus_file(paste0(f$id, ".fcs")), "")
  sheet <- withr::local_tempfile(fileext = ".csv")
  cond <- rep(c("treated", "control"), length.out = length(paths))
  utils::write.csv(data.frame(file = basename(paths), condition = cond, donor = seq_along(paths)),
                   sheet, row.names = FALSE)
  res <- openreadout_batch("table", paths, sample_sheets = sheet, where = "parameter=FSC-A",
                  by = "condition", values = "median")
  expect_true(all(c("condition", "donor", "median") %in% names(res)))
  expect_equal(sort(unique(res$condition)), sort(unique(cond[vapply(o$files, function(f) {
    any(vapply(f$parameters, function(p) p$parameter == "FSC-A", TRUE))
  }, TRUE)])))
  expect_true(all(res$parameter == "FSC-A"))
  j <- attr(res, "joins")
  expect_equal(length(j), 1)
  # the joined condition is the one written for each file
  expect_equal(res$condition, cond[match(basename(res$path), basename(paths))])
  s <- attr(res, "summary")
  agg <- stats::aggregate(median ~ condition, data = as.data.frame(res), FUN = mean)
  for (i in seq_len(nrow(agg))) {
    expect_equal(s$mean[s$condition == agg$condition[i]], agg$median[i], tolerance = 1e-12)
  }
  cnt <- table(res$condition)
  expect_equal(s$n[match(names(cnt), s$condition)], as.vector(cnt))
})

test_that("trace batch rows match pyabf + numpy", {
  o <- oracle("batch/trace-stats.json")
  fl <- o$files[[1]]
  res <- openreadout_batch("trace", corpus_file(paste0(fl$id, ".abf")), trace = 1, channels = fl$channel + 1)
  for (r in fl$rows) {
    row <- res[res$sweep == r$sweep, ]   # batch rows keep the file's (0-based) sweep numbers
    expect_equal(nrow(row), 1)
    expect_equal(row$samples, r$samples)
    expect_equal(row$mean, r$mean, tolerance = 1e-6)
    expect_equal(row$min, r$min, tolerance = 1e-6)
  }
})

test_that("image stats batch rows match nd2 + numpy", {
  o <- oracle("batch/image-stats.json")
  f <- Filter(function(e) e$id == "aics-ND2-dims-p4z5t3c2y32x32", o$files)[[1]]
  res <- openreadout_batch("stats", corpus_file("aics-ND2-dims-p4z5t3c2y32x32.nd2"))
  for (r in f$rows) {
    row <- res[res$image == r$image & res$channel == r$channel, ]
    expect_equal(nrow(row), 1)
    expect_equal(row$mean, r$mean, tolerance = 1e-9)
    expect_equal(row$min, r$min)
    expect_equal(row$max, r$max)
  }
})

test_that("per-well plate statistics agree with the fields read in R", {
  path <- corpus_file("hcs/harmony-idr0034/Images/Index.idx.xml")
  f <- openreadout_open(path)
  on.exit(openreadout_close(f))
  info <- openreadout_info(f)
  all_wells <- openreadout_stats(f, per = "well", select = "c=0")
  w_name <- all_wells$well[all_wells$fields > 0][1]
  skip_if(is.na(w_name), "no well with planes on disk in this copy of the plate")
  w <- Filter(function(x) x$well == w_name, info$plate$wells)[[1]]
  ws <- openreadout_stats(f, per = "well", select = "c=0", wells = w_name)
  expect_equal(nrow(ws), 1)
  # the corpus holds a partial copy of the plate: fields whose plane file is absent are skipped,
  # and reading one is a clean file-not-found condition
  px <- unlist(lapply(w$images, function(i) {
    tryCatch(as.vector(openreadout_read_image(f, image = i + 1, c = 1)),
             openreadout_file_not_found = function(e) NULL)
  }))
  expect_equal(ws$count, length(px))
  expect_equal(ws$mean, mean(px), tolerance = 1e-9)
  expect_equal(ws$max, max(px))
})
