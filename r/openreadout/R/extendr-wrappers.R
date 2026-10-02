# The native routines of crates/openreadout-r (registered by extendr_module!). Internal: the
# exported functions in the other files check their arguments and call these.
# nolint start

rs_open <- function(path) .Call(wrap__rs_open, path)
rs_close <- function(h) .Call(wrap__rs_close, h)
rs_handle_state <- function(h) .Call(wrap__rs_handle_state, h)
rs_version <- function() .Call(wrap__rs_version)
rs_formats <- function() .Call(wrap__rs_formats)
rs_detect <- function(path) .Call(wrap__rs_detect, path)
rs_info <- function(h) .Call(wrap__rs_info, h)
rs_info_view <- function(h, view, ask, vendor) .Call(wrap__rs_info_view, h, view, ask, vendor)
rs_stats <- function(h, image, select, per_plane) {
  .Call(wrap__rs_stats, h, image, select, per_plane)
}
rs_check <- function(h) .Call(wrap__rs_check, h)
rs_ome_xml <- function(h) .Call(wrap__rs_ome_xml, h)
rs_read_planes <- function(h, image, c, z, t, level, region) {
  .Call(wrap__rs_read_planes, h, image, c, z, t, level, region)
}
rs_columnar <- function(h, kind, index, sweep, first_row, last_row, centroid, max_rows) {
  .Call(wrap__rs_columnar, h, kind, index, sweep, first_row, last_row, centroid, max_rows)
}
rs_spectrum <- function(h, run, number, by_scan, centroid) {
  .Call(wrap__rs_spectrum, h, run, number, by_scan, centroid)
}
rs_scans <- function(h, run, filter, offset, limit) {
  .Call(wrap__rs_scans, h, run, filter, offset, limit)
}
rs_analyze_dataset <- function(h, kind, options) {
  .Call(wrap__rs_analyze_dataset, h, kind, options)
}
rs_analyze <- function(path, kind, options) .Call(wrap__rs_analyze, path, kind, options)
rs_chromatogram <- function(h, query) .Call(wrap__rs_chromatogram, h, query)
rs_peaks <- function(h, query) .Call(wrap__rs_peaks, h, query)
rs_well_stats <- function(h, select, wells, per_field) {
  .Call(wrap__rs_well_stats, h, select, wells, per_field)
}
rs_batch <- function(request) .Call(wrap__rs_batch, request)
rs_summarize <- function(request) .Call(wrap__rs_summarize, request)
rs_link <- function(request) .Call(wrap__rs_link, request)
rs_qpcr <- function(path, request) .Call(wrap__rs_qpcr, path, request)
rs_export <- function(h, to, output, options) .Call(wrap__rs_export, h, to, output, options)

# nolint end
