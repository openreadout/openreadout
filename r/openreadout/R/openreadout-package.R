#' openreadout: read raw laboratory instrument files
#'
#' Reads raw files from microscopes (Zeiss CZI, Nikon ND2, Leica LIF, Olympus, whole-slide
#' images, OME-TIFF, OME-Zarr, ...), flow cytometers (FCS), mass spectrometers (Thermo `.raw`,
#' mzML, Bruker timsTOF, Agilent, Waters, Sciex), chromatographs (ChemStation, ANDI, Shimadzu),
#' plate readers, qPCR cyclers, electrophysiology rigs and NMR spectrometers into R arrays and data
#' frames, without vendor software. The readers are the OpenReadout Rust library, compiled into
#' the package.
#'
#' The functions are named after the commands of the `openreadout` command-line tool:
#'
#' - Metadata: [openreadout_info()] (`view = "summary"`, `"full"`, `"structure"`, `"explain"`
#'   or `"format"`), [openreadout_check()], [openreadout_formats()], [openreadout_ome_xml()].
#' - Images: [openreadout_read_image()], [openreadout_read_plane()], [openreadout_levels()],
#'   [openreadout_stats()].
#' - Tables and signals: [openreadout_table()], [openreadout_trace()], [openreadout_spectra()].
#' - Analyses: [openreadout_analyze()] (chromatograms, peaks, NMR peaks, patch-clamp
#'   features, spikes, qPCR, plate assays, gating).
#' - Many files: [openreadout_batch()] (including `"summarize"`), [openreadout_link()].
#' - Open formats: [openreadout_export()].
#'
#' Indices given to these functions (`image`, `c`, `z`, `t`, `table`, `trace`, `sweep`, `run`,
#' `index`, and the `x`, `y` of a `region`) start at 1, like R vectors. The metadata lists keep the
#' file's own numbering (`index`, `level`: from 0), and pyramid `level` 0 is full resolution.
#'
#' Errors are conditions of class `openreadout_error` (and a specific class such as
#' `openreadout_corrupt_file` or `openreadout_unsupported_feature`) with fields `code`,
#' `exit_code` and `hint`.
#'
#' Options: `openreadout.tibble` (default `TRUE`: return tibbles when the tibble package is
#' installed), `openreadout.max_bytes` (default 4 GiB: the most memory [openreadout_read_image()] may
#' allocate before asking for a selection, level or region).
#'
#' @keywords internal
"_PACKAGE"
