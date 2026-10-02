# Tables, traces, spectra, qPCR and chromatography through the R API against third-party readers
# (flowio, pyabf, pyteomics, scipy, rdmlpython, allotropy) and vendor-computed results.

test_that("FCS events match flowio (every value, f64 column-major hash)", {
  for (id in c("flowio-100715", "fcsparser-miltenyi-fcs31")) {
    o <- oracle(paste0(id, ".json"))
    ev <- openreadout_table(corpus_file(o$file))
    t0 <- o$tables[[1]]
    expect_equal(nrow(ev), t0$event_count)
    expect_equal(names(ev), unlist(t0$parameter_names))
    if (!is.null(t0$xxh3)) {
      expect_equal(xxh3(unlist(ev, use.names = FALSE), "f64"), t0$xxh3, label = id)
    }
  }
})

test_that("openreadout_batch FCS summaries (events, median, mean, SD, range) match flowio + numpy", {
  o <- oracle("batch/fcs-summaries.json")
  paths <- vapply(o$files, function(f) corpus_file(paste0(f$id, ".fcs")), "")
  res <- openreadout_batch("table", paths)
  expect_equal(attr(res, "inputs")$failed, 0)
  n <- 0
  for (k in seq_along(o$files)) {
    for (p in o$files[[k]]$parameters) {
      row <- res[res$path == paths[k] & res$parameter == p$parameter, ]
      expect_equal(nrow(row), 1)
      expect_equal(row$events, p$events)
      for (m in c("median", "mean", "sd", "min", "max")) {
        expect_equal(row[[m]], p[[m]], tolerance = 1e-9, label = paste(basename(paths[k]), p$parameter, m))
        n <- n + 1
      }
    }
  }
  expect_gt(n, 100)
  # R's own median over the raw events agrees where no $PnE scaling applies
  ev <- openreadout_table(paths[1])
  first <- o$files[[1]]$parameters[[1]]
  expect_equal(stats::median(ev[[first$parameter]]), first$median, tolerance = 1e-9)
})

test_that("plate-reader wells match allotropy", {
  o <- oracle("batch/plate-values.json")
  for (f in o$files) {
    files <- list.files(corpus_dir(), pattern = paste0("^", f$id, "\\.[a-z]+$"))
    skip_if(length(files) == 0, paste("corpus file missing:", f$id))
    path <- corpus_file(files[1])
    # a file may hold several plates or reads: one table each
    tabs <- lapply(seq_along(openreadout_info(path)$tables), function(i) openreadout_table(path, table = i))
    pt <- list(well = unlist(lapply(tabs, function(t) as.character(t$well))),
               value = unlist(lapply(tabs, function(t) t$value)))
    well <- pt$well
    for (w in names(f$wells)) {
      canon <- sub("^([A-Z]+)0*([0-9]+)$", "\\1\\2", w)
      ours <- pt$value[sub("^([A-Z]+)0*([0-9]+)$", "\\1\\2", well) == canon]
      for (v in unlist(f$wells[[w]])) {
        expect_true(any(abs(ours - v) <= 1e-9 * max(1, abs(v))), label = paste(f$id, w, v))
      }
    }
  }
})

test_that("ABF sweeps match pyabf (samples, first values, f64 hash)", {
  o <- oracle("pyabf-file-axon-2.json")
  f <- openreadout_open(corpus_file(o$file))
  on.exit(openreadout_close(f))
  tr0 <- o$traces[[1]]
  for (s in tr0$sweeps) {
    tr <- openreadout_trace(f, sweep = s$sweep + 1)
    expect_equal(nrow(tr), s$sample_count)
    for (k in seq_along(s$channels)) {
      col <- tr[[unlist(tr0$channel_names)[k]]]
      expect_equal(attr(tr, "units")[[unlist(tr0$channel_names)[k]]], unlist(tr0$channel_units)[k])
      expect_equal(utils::head(col, 8), unlist(s$channels[[k]]$first), tolerance = 1e-9)
      expect_equal(xxh3(col, "f64"), s$channels[[k]]$xxh3)
    }
  }
  expect_equal(openreadout_trace(f, first_sample = 11, n_max = 5)[[3]], openreadout_trace(f)[[3]][11:15])
})

test_that("per-sweep statistics computed in R match pyabf + numpy", {
  o <- oracle("batch/trace-stats.json")
  for (fl in o$files) {
    tr <- openreadout_trace(corpus_file(paste0(fl$id, ".abf")))
    ch <- setdiff(names(tr), c("sweep", "time_s"))[fl$channel + 1]
    expect_equal(unname(attr(tr, "units")[[ch]]), fl$unit)
    for (r in fl$rows) {
      v <- tr[[ch]][tr$sweep == r$sweep + 1]
      expect_equal(length(v), r$samples)
      # pyABF scales float32 samples: 1e-6 relative, as in the Rust batch corpus test
      expect_equal(mean(v), r$mean, tolerance = 1e-6)
      expect_equal(sqrt(mean((v - mean(v))^2)), r$std, tolerance = 1e-6)
      expect_equal(range(v), c(r$min, r$max), tolerance = 1e-6)
    }
  }
})

test_that("an ANDI chromatogram matches scipy's netCDF reader", {
  o <- oracle("cheminfo-agilent-hplc-cdf.json")
  tr <- openreadout_trace(corpus_file(o$file))
  t0 <- o$traces[[1]]
  sig <- tr[[unlist(t0$channel_names)[1]]]
  expect_equal(length(sig), t0$sweeps[[1]]$sample_count)
  expect_equal(utils::head(sig, 8), unlist(t0$sweeps[[1]]$channels[[1]]$first), tolerance = 1e-9)
  expect_equal(xxh3(sig, "f64"), t0$sweeps[[1]]$channels[[1]]$xxh3)
  expect_equal(unname(attr(tr, "units")[[unlist(t0$channel_names)[1]]]), unlist(t0$channel_units)[1])
})

test_that("automatic peaks agree with the vendor's (ChemStation) integration", {
  q <- oracle("quant/cheminfo-agilent-hplc-cdf.json")
  pk <- openreadout_analyze(corpus_file(q$input), "peaks", traces = q$signal$trace + 1,
                            area_seconds = TRUE)
  expect_equal(unique(pk$area_unit), "mAU·s")
  ratios <- c()
  for (v in q$peaks) {
    near <- which(abs(pk$rt_min - v$rt_min) <= 0.02)
    expect_length(near, 1)
    if (length(near) == 1) {
      ratios <- c(ratios, pk$area[near] / (v$area / q$vendor$height_scale))
      expect_equal(pk$height[near], v$height / q$vendor$height_scale, tolerance = 0.05)
    }
  }
  # same thresholds as the Rust corpus test (book/src/guides/quantitation.md): median area error < 5 %
  expect_lt(stats::median(abs(ratios - 1)), 0.05)
  ch <- attr(pk, "chromatograms")
  expect_equal(ch$peak_count, nrow(pk))
})

test_that("the chromatogram of a detector trace is the trace", {
  path <- corpus_file("cheminfo-agilent-hplc.cdf")
  ch <- openreadout_analyze(path, "chromatogram", traces = 1)
  tr <- openreadout_trace(path)
  expect_equal(nrow(ch), nrow(tr))
  expect_equal(ch$intensity, tr[[3]])
  expect_equal(ch$rt_min, tr$retention_time_min, tolerance = 1e-9)
  s <- attr(ch, "chromatograms")
  expect_equal(s$apex_intensity, max(tr[[3]]))
})

test_that("mzML spectra match pyteomics (m/z f64 and intensity f32 hashes)", {
  o <- oracle("synthetic-mzml-plain.json")
  f <- openreadout_open(corpus_file(o$file))
  on.exit(openreadout_close(f))
  for (s in utils::head(o$spectra$scans, 12)) {
    sp <- openreadout_spectra(f, index = s$index + 1)
    m <- attr(sp, "spectrum")
    expect_equal(m$scan_number, s$scan_number)
    expect_equal(m$ms_level, s$ms_level)
    expect_equal(m$rt_s, s$rt_s, tolerance = 1e-9)
    expect_equal(nrow(sp), s$n_peaks)
    expect_equal(xxh3(sp$mz, "f64"), s$xxh3_mz)
    expect_equal(xxh3(sp$intensity, "f32"), s$xxh3_intensity)
    if (!is.null(s$precursor_mz)) expect_equal(m$precursor_mz, s$precursor_mz, tolerance = 1e-9)
    by_scan <- openreadout_spectra(f, scan = s$scan_number)
    expect_identical(by_scan$mz, sp$mz)
  }
})

test_that("qPCR Cq values match rdmlpython", {
  # rdmlpython echoes the Cq stored in the RDML. Where the instrument software's own result
  # withholds it (the LightCycler 96 calls the well Negative), the reader reports no Cq and
  # keeps the stored value in `cq_stored`; both cases must carry the oracle's number.
  for (id in c("rdml-stepone-std", "rdml-lc96-bactxy")) {
    o <- oracle(file.path("qpcr", paste0(id, ".json")))
    q <- openreadout_analyze(corpus_file(o$file), "qpcr")
    determined <- 0
    for (r in o$records) {
      if (is.null(r$cq) || isTRUE(r$cq_undetermined)) next
      # A well and its dye identify the reaction; sample and target names may be shown as the
      # instrument software's display names rather than the RDML ids the oracle echoes.
      hit <- q[q$row == r$row & q$col == r$col & q$dye == r$dye, ]
      label <- paste(id, r$row, r$col, r$dye)
      expect_equal(nrow(hit), 1, label = label)
      if (nrow(hit) != 1) next
      if (identical(hit$cq_status, "determined")) {
        expect_equal(hit$cq, r$cq, tolerance = 1e-9, label = label)
        determined <- determined + 1
      } else {
        expect_true(is.na(hit$cq), label = label)
        expect_equal(hit$cq_stored, r$cq, tolerance = 1e-9, label = label)
      }
    }
    expect_gt(determined, 0)
  }
})
