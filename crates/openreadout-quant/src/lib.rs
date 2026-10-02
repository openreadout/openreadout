//! Quantitation primitives for chromatography and mass spectrometry.
//!
//! - [`extract`]: chromatograms from any reader — total-ion (TIC), base-peak (BPC),
//!   extracted-ion (XIC/EIC) and SRM/MRM transition chromatograms streamed from mass spectra,
//!   stored chromatograms and detector signals (traces), and MRM tables.
//! - [`peaks`]: peak detection (Savitzky–Golay smoothing, robust noise, S/N threshold) and
//!   integration (drop-line, valley-to-valley or tangent-skim baselines; trapezoidal areas;
//!   widths, USP tailing, asymmetry, plates, resolution), retention-time targeting and
//!   area-percent reports.
//! - [`bands`]: bands and regions of spectra (any non-time axis: wavenumber, wavelength,
//!   Raman shift, ppm): band picking and region areas with linear or no baseline.
//! - [`targets`]: compound lists (name, m/z or transition, expected retention time and window)
//!   turned into one result row per compound.
//!
//! The method, its parameters and its validation against pyOpenMS, SciPy and vendor
//! integration results are documented in `book/src/guides/quantitation.md`.
#![forbid(unsafe_code)]
#![warn(missing_docs)]
// Signal arithmetic reads best with the conventional short names (t, y, a, b, l, r, …).
#![allow(clippy::many_single_char_names)]
// Exact equality of sample times (zero-width steps) and of flat tops is what is meant.
#![allow(clippy::float_cmp)]

pub mod analyze;
pub mod api;
pub mod bands;
pub mod dataset;
pub mod extract;
pub mod peaks;
pub mod plot;
pub mod rows;
pub mod smooth;
pub mod targets;
