//! Clean-room readers for DNA sequencer output.
//!
//! - **Illumina run folders** (`illumina-run`): `RunInfo.xml`, `RunParameters.xml`, the InterOp
//!   metrics (quality per cycle, % ≥ Q30, cluster density, pass filter, error rate, intensity,
//!   index counts) and the BCL or CBCL base calls with their filter and location files. `info`
//!   reads the XML and the metrics only; base calls are decoded by `check` and the FASTQ export
//!   ([`illumina::fastq::export_fastq`]).
//! - **Oxford Nanopore POD5** (`pod5`) and **FAST5** (`fast5`): every read's raw signal in pA (a
//!   trace with one sweep per read), the reads table (channel, well, start, duration, end
//!   reason, open-pore level) and the run's metadata.
//! - **FASTQ** (`fastq`, also gzip-compressed) and **BAM** (`bam`): read counts, length and
//!   quality distributions and the header facts, streamed; the reads themselves as a table.
//!
//! Notes: `docs/formats/illumina-run.md`, `docs/formats/nanopore.md`,
//! `docs/formats/sequence-reads.md`.
//!
//! # Example
//!
//! ```
//! use openreadout_core::FormatReader;
//! use openreadout_seq::IlluminaRunReader;
//!
//! assert_eq!(IlluminaRunReader.descriptor().id, openreadout_seq::ILLUMINA_RUN_FORMAT_ID);
//! ```
//!
//! # API stability
//!
//! The supported API is what this page documents: the readers, their format ids and the FASTQ
//! export. The parser modules are public only so that tests and fuzz targets can reach them.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod assurance;
#[doc(hidden)]
pub mod illumina;
#[doc(hidden)]
pub mod nanopore;
#[doc(hidden)]
pub mod table;

pub use illumina::fastq::{FastqFile, FastqOptions, FastqReport, export_fastq};
pub use illumina::{IlluminaRun, IlluminaRunReader};
pub use nanopore::{Fast5Reader, Pod5Reader};

/// Format id of Illumina run folders.
pub const ILLUMINA_RUN_FORMAT_ID: &str = illumina::FORMAT_ID;
/// Format id of POD5 files.
pub const POD5_FORMAT_ID: &str = nanopore::pod5::FORMAT_ID;
/// Format id of FAST5 files.
pub const FAST5_FORMAT_ID: &str = nanopore::fast5::FORMAT_ID;
