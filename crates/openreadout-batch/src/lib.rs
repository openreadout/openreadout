//! Measurements across many instrument files as one tidy table.
//!
//! Real questions span folders: "mean GFP intensity per condition", "which wells failed",
//! "median CD4 per donor". This crate is the layer that answers them:
//!
//! - [`run()`] runs a [`Measure`] (pixel statistics, trace statistics, table summaries, gated
//!   population counts and medians, header metadata, and any command that implements the trait)
//!   over files, directories, glob patterns or an index query, in parallel, with failures as
//!   rows, multi-file data sets counted once and results in a deterministic order;
//! - [`sheet`] reads sample sheets (CSV, TSV, XLSX/XLS/ODS) and plate layouts (plate-map grids,
//!   96 to 1536 wells) and [`join()`] attaches their annotations to the rows, choosing the key
//!   (file name, path, sample id recorded in the file, plate well, barcode, vial, run order)
//!   from the data and reporting unmatched and ambiguous rows;
//! - [`summarize()`] aggregates a table by annotation columns (n, mean, sd, median, CV %),
//!   optionally averaging technical replicates first and testing groups against a control
//!   (Welch's t-test, Mann–Whitney U);
//! - [`mod@write`] writes tables as JSON, JSON Lines, CSV, TSV or Parquet, and reads them back;
//! - [`link()`] groups files that measured the same sample across instruments, with the
//!   evidence and a confidence for every link.
//!
//! The CLI (`--tidy`, `batch`, `link`), the MCP server and the Python package are thin
//! layers over these functions. `book/src/guides/batch.md` in the repository documents the behaviour.
#![forbid(unsafe_code)]
#![warn(missing_docs)]
// Statistics and pairwise comparisons use the textbook names (a, b, n, t, u, x, …).
#![allow(clippy::many_single_char_names)]

pub mod analyze;
pub mod api;
pub mod companion;
pub mod join;
pub mod link;
pub mod measure;
pub mod measures;
pub mod output;
pub mod pipeline;
pub mod run;
pub mod sheet;
pub mod stat;
pub mod summarize;
pub mod table;
pub mod well;
pub mod write;

pub use join::{JoinOptions, JoinReport, join};
pub use link::{LinkOptions, LinkOutput, link};
pub use measure::{Item, Measure};
pub use output::BatchOutput;
pub use pipeline::{Pipeline, PipelineResult, run_pipeline};
pub use run::{BatchRequest, BatchResult, IndexQuery, InputReport, run};
pub use summarize::{SummarizeRequest, SummaryOutput, summarize};
pub use table::{Column, ColumnDoc, ColumnType, Role, Row, Table, Value};
