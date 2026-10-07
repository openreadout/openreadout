# openreadout_analyze() kinds "chromatogram" and "peaks": the options of `analyze chromatogram`
# / `analyze peaks` (MCP `openreadout_chromatogram` and `openreadout_peaks`;
# https://openreadout.github.io/openreadout/guides/quantitation.html), with R's 1-based indices.

.source_query <- function(tic, bpc, mz, ppm, da, transitions, traces, channel, run, ms_level,
                          polarity, scan_filter, precursor, rt_range, mz_range, extra) {
  q <- list(
    tic = if (isTRUE(tic)) TRUE,
    bpc = if (isTRUE(bpc)) TRUE,
    mz = if (!is.null(mz)) I(as.numeric(mz)),
    ppm = ppm,
    da = da,
    transitions = if (!is.null(transitions)) I(as.character(transitions)),
    traces = if (!is.null(traces)) I(vapply(traces, .index0, 0L, what = "traces")),
    channel = if (!is.null(channel)) .index0(channel, "channel"),
    run = .index0(run, "run"),
    ms_level = ms_level,
    polarity = polarity,
    scan_filter = scan_filter,
    precursor = precursor,
    rt_range = if (!is.null(rt_range)) I(as.numeric(rt_range)),
    mz_range = if (!is.null(mz_range)) I(as.numeric(mz_range))
  )
  c(q, extra)
}

.analyze_chromatogram <- function(x, tic = FALSE, bpc = FALSE, mz = NULL, ppm = NULL, da = NULL,
                            transitions = NULL, traces = NULL, channel = NULL, run = 1,
                            ms_level = NULL, polarity = NULL, scan_filter = NULL, precursor = NULL,
                            rt_range = NULL, mz_range = NULL, max_points = 20000, ...) {
  q <- .source_query(tic, bpc, mz, ppm, da, transitions, traces, channel, run, ms_level, polarity,
                     scan_filter, precursor, rt_range, mz_range, list(...))
  q$max_points <- max_points
  .with_file(x, function(h) {
    out <- .from_json(.ic(rs_chromatogram(.ptr(h), .to_json(q))))
    ch <- out$chromatograms
    long <- .frame(list(
      chromatogram = unlist(lapply(ch, function(c) rep(c$label, length(c$rt_min))), use.names = FALSE),
      kind = unlist(lapply(ch, function(c) rep(c$kind, length(c$rt_min))), use.names = FALSE),
      rt_min = as.numeric(unlist(lapply(ch, function(c) c$rt_min), use.names = FALSE)),
      intensity = as.numeric(unlist(lapply(ch, function(c) c$intensity), use.names = FALSE))
    ))
    attr(long, "chromatograms") <- .records(lapply(ch, function(c) {
      c$rt_min <- NULL
      c$intensity <- NULL
      c
    }))
    attr(long, "notes") <- out$notes
    long
  })
}

.analyze_peaks <- function(x, tic = FALSE, bpc = FALSE, mz = NULL, ppm = NULL, da = NULL,
                     transitions = NULL, traces = NULL, channel = NULL, run = 1, ms_level = NULL,
                     polarity = NULL, scan_filter = NULL, precursor = NULL, rt_range = NULL,
                     mz_range = NULL, smooth = NULL, min_snr = NULL, min_height = NULL,
                     min_width = NULL, baseline = NULL, area_seconds = FALSE, rt = NULL,
                     window = NULL, pick = NULL, integrate = NULL, compounds = NULL,
                     summary_only = FALSE, x_range = NULL, ...) {
  q <- .source_query(tic, bpc, mz, ppm, da, transitions, traces, channel, run, ms_level, polarity,
                     scan_filter, precursor, rt_range, mz_range, list(...))
  if (!is.null(integrate)) {
    integrate <- matrix(as.numeric(integrate), ncol = 2)
  }
  if (!is.null(x_range)) {
    x_range <- matrix(as.numeric(x_range), ncol = 2)
  }
  if (!is.null(compounds) && !is.data.frame(compounds)) {
    .openreadout_abort("compounds must be a data frame (columns name, mz or q1/q3 or trace, rt, window, ...)")
  }
  q <- c(q, list(
    smooth = smooth, min_snr = min_snr, min_height = min_height, min_width = min_width,
    baseline = baseline, area_seconds = if (isTRUE(area_seconds)) TRUE, rt = rt, window = window,
    pick = pick, integrate = integrate, compounds = compounds, x_range = x_range,
    summary_only = if (isTRUE(summary_only)) TRUE
  ))
  if (!is.null(compounds) && "trace" %in% names(compounds)) {
    q$compounds$trace <- as.integer(compounds$trace) - 1L
  }
  .with_file(x, function(h) {
    res <- .from_json(.ic(rs_peaks(.ptr(h), .to_json(q))))
    df <- .records(res$rows)
    out <- res$output
    attr(df, "chromatograms") <- .records(lapply(out$chromatograms, function(c) {
      c[setdiff(names(c), c("peaks", "picked", "manual"))]
    }))
    picked <- Filter(Negate(is.null), lapply(out$chromatograms, function(c) {
      if (is.null(c$picked)) return(NULL)
      p <- c$picked
      row <- c(list(chromatogram = c$label, expected_rt_min = p$expected_rt_min,
                    rt_window_min = p$rt_window_min, rule = p$rule, found = !is.null(p$peak)),
               p$peak)
      row
    }))
    if (length(picked)) attr(df, "picked") <- .records(picked)
    manual <- unlist(lapply(out$chromatograms, function(c) {
      lapply(c$manual, function(m) c(list(chromatogram = c$label), m))
    }), recursive = FALSE)
    if (length(manual)) attr(df, "manual") <- .records(manual)
    if (length(out$compounds)) attr(df, "compounds") <- .records(out$compounds)
    if (length(res$regions)) attr(df, "regions") <- .records(res$regions)
    attr(df, "notes") <- out$notes
    df
  })
}
