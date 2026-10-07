//! Core types shared by every `OpenReadout` format reader and front end.
//!
//! The vocabulary here is deliberately vendor-neutral and mirrors the OME data model
//! (the open standard for microscopy metadata), so that an agent sees the same field
//! names whether a file came from ZEISS, Nikon or Leica. Vendor-specific metadata is
//! preserved verbatim under a separate `vendor` tree.
//!
//! # Overview
//!
//! - [`FormatReader`] is implemented once per file format (in the `openreadout-<format>`
//!   crates). It recognises a file from its first bytes and opens it.
//! - [`Dataset`] is an opened file. `info()` reads headers only and returns a [`FileInfo`];
//!   pixels, table rows, spectra and signal samples are read lazily, one piece at a time.
//! - [`Registry`] holds readers in detection order and picks the right one for a file.
//! - [`source`] is where the bytes come from: a local file, a buffer in memory, or a callback
//!   the host provides (a Python file-like object, a browser `File`). Readers open an
//!   [`Input`] and read it through [`SourceFile`] and [`Fs`], which behave like `std::fs`.
//! - [`model`] is the normalized metadata. A file holds any mix of *images* (microscopy
//!   planes), *tables* (flow-cytometry events, plate-reader wells), *spectra* (mass
//!   spectrometry scans) and *traces* (uniformly sampled signals: electrophysiology sweeps,
//!   chromatograms, NMR signals).
//! - [`Error`] carries a stable code, a process exit code and an actionable hint;
//!   [`Envelope`] is the JSON wrapper every `--json` command prints.
//! - [`provenance`] records where the meaning of each normalized field came from.
//! - [`experiment`] derives the experiment (sample, instrument, method, acquisition,
//!   measurements) from the normalized model, with terms from [`vocab`]; `info` prints it as
//!   [`InfoOutput`].
//! - [`stats`] and [`trace`] implement those commands; [`batch`] and [`select`] parse their
//!   inputs. Other command-level operations (`compare`, `info --view explain`,
//!   `extract`, `--only`) live in the `openreadout-ops` crate.
//! - [`live`] judges whether an incomplete file is still being written (`acquisition` in
//!   `info`, `info --view structure`, `check` and `planes`).
//! - [`bytes`], [`cfb`], [`xml`], [`xmljson`], [`zip`], [`time`] and [`parallel`] are helpers for
//!   format crates.
//!
//! # Exit codes
//!
//! Part of the public interface (agents branch on them):
//!
//! | code | meaning | [`Error`] variant |
//! | --- | --- | --- |
//! | 0 | success | |
//! | 1 | other error | [`Error::Other`] |
//! | 2 | bad arguments or impossible request | [`Error::Usage`] |
//! | 3 | unrecognised file format | [`Error::UnknownFormat`] |
//! | 4 | corrupt or truncated file | [`Error::Corrupt`] |
//! | 5 | I/O error | [`Error::Io`] |
//! | 6 | known format, unsupported feature | [`Error::Unsupported`] |
//!
//! # Example
//!
//! ```
//! use std::path::Path;
//!
//! use openreadout_core::{Error, Registry};
//!
//! // Format crates register their readers: `Registry::new().with(Box::new(CziReader))`.
//! // An empty registry recognises nothing, which shows the error contract.
//! let registry = Registry::new();
//! let err = registry
//!     .detect_bytes(b"not an instrument file", Path::new("notes.txt"))
//!     .map(|_| ())
//!     .unwrap_err();
//! assert!(matches!(err, Error::UnknownFormat { .. }));
//! assert_eq!(err.code(), "unknown_format");
//! assert_eq!(err.exit_code(), 3);
//! assert!(err.hint().is_some());
//! ```
//!
//! Enums and option structs that will grow are `#[non_exhaustive]`: match them with a
//! wildcard arm, and build option structs from `Default::default()`.
#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod acquisition_mode;
pub mod assurance;
pub mod batch;
pub mod bytes;
pub mod cfb;
pub mod envelope;
pub mod error;
pub mod experiment;
pub mod flow;
pub mod gzip;
pub mod jet;
pub mod limits;
pub mod live;
pub mod model;
pub mod parallel;
pub mod pixel;
pub mod plate;
pub mod provenance;
pub mod reader;
pub mod region;
pub mod scans;
pub mod select;
pub mod series;
pub mod session;
pub mod source;
pub mod stats;
pub mod strict;
pub mod time;
pub mod trace;
pub mod vocab;
pub mod xml;
pub mod xmljson;
pub mod zip;

pub use envelope::{Envelope, ErrorBody, SCHEMA_VERSION};
pub use error::{Error, Result};
pub use experiment::{Experiment, InfoOutput};
pub use model::*;
pub use pixel::{PixelType, Plane};
pub use provenance::{Confidence, ProvenanceMap, Source};
pub use reader::{Dataset, Detection, FormatReader, PlaneIndex, Registry, SpectrumView};
pub use region::{Region, ResolutionLevel};
pub use scans::{ScanFilter, ScanHeader, ScanList};
pub use source::{ByteSource, DirSource, Fs, Input, SourceFile};
