//! Signal processing on top of OpenReadout's readers.
//!
//! - [`nmr`]: 1-D NMR processing from the free induction decay (digital-filter group-delay
//!   removal, apodization, zero filling, Fourier transform, phase correction from stored or
//!   automatically fitted phases, baseline correction, ppm referencing) and peak picking /
//!   integration on a processed spectrum (computed here or stored by the vendor software).
//!   Notes and vocabulary: `docs/formats/nmr-processing.md`; provenance:
//!   `docs/provenance/nmr-processing.md`.
//! - [`ephys`]: patch-clamp analysis (action-potential detection and features per sweep,
//!   rheobase and f–I curve from the protocol's command waveform, passive membrane properties,
//!   voltage-clamp holding current and test-pulse metrics) and extracellular threshold-crossing
//!   spike detection on band-pass filtered signals. Notes and vocabulary:
//!   `docs/formats/ephys-analysis.md`; provenance: `docs/provenance/ephys-analysis.md`.
//!
//! Everything works on the core `Dataset` interface, so any reader that exposes traces
//! (Bruker, Varian, JEOL; ABF, ATF, NWB, Neuralynx, Blackrock, SpikeGLX, Intan, …) can be
//! analysed. Nothing here panics on odd input: empty, constant or non-finite signals produce
//! empty results or a clean error.
#![forbid(unsafe_code)]
#![warn(missing_docs)]
// Numerical code: short names follow the formulas in the module docs, and `!(x > t)` is the
// deliberate NaN-rejecting form of a threshold test.
#![allow(clippy::many_single_char_names, clippy::neg_cmp_op_on_partial_ord)]

pub mod api;
pub mod ephys;
pub mod fft;
pub mod nmr;

pub use fft::Complex;
